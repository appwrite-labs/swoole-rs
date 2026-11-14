//! PHP `Swoole\Event`: registration, descriptor conversion, callback custody,
//! fd registry and loop admission, dispatch and shutdown policy over the
//! thread's reactor. Its reactor handlers and deferred tasks raise a contained
//! Zend bailout only after Rust cleanup has completed.
#![cfg(feature = "php")]
#![forbid(unsafe_code)]

mod descriptor;
mod loop_driver;
mod native;

use native::has_reactor;
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, c_int},
    mem::ManuallyDrop,
    rc::Rc,
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::constants::{EventType, FdType};
use swoole_rs_memory::flat::Table;
use swoole_rs_phpstubs::event;
use swoole_rs_phpswoole::reactor::{self, PhpFdHandler};
use swoole_rs_reactor::event_api::{self, CycleTask, Deferred};
use swoole_rs_tls::event_socket::EventSocket;
use swoole_rs_zend::host::callable::Callable;
use swoole_rs_zend::{
    Call, Class, E_WARNING, Minit, Owned, Php, Value,
    bailout::{self, Bailout},
    message, php_local,
};
use swoole_rs_zend::{method_table, sys as z};

/// `php_event_object`: the fd's PHP value and its callbacks.
struct EventObject {
    zsocket: Owned,
    readable: Option<Rc<Callable>>,
    writable: Option<Rc<Callable>>,
}

/// A user fd's socket and, until a failed callback detaches it, its object.
struct Registered {
    /// Which `add` made it: the fd may be deleted and added again while a
    /// handler runs PHP.
    id: u64,
    socket: EventSocket,
    object: Option<EventObject>,
}

/// `event_socket_map`, flat by fd.
struct Sockets(Table<Registered>);

impl Sockets {
    fn get(&self, fd: i32) -> Option<&Registered> {
        self.0.get(usize::try_from(fd).ok()?)
    }

    fn get_mut(&mut self, fd: i32) -> Option<&mut Registered> {
        self.0.get_mut(usize::try_from(fd).ok()?)
    }

    fn contains_key(&self, fd: i32) -> bool {
        self.get(fd).is_some()
    }

    fn remove(&mut self, fd: i32) -> Option<Registered> {
        self.0.remove(usize::try_from(fd).ok()?)
    }

    /// The registration `fd` replaced. `add` admits only a nonnegative fd.
    fn insert(&mut self, fd: i32, registered: Registered) -> Option<Registered> {
        let key = usize::try_from(fd).unwrap_or(usize::MAX);
        // Out of memory: abort, as upstream's map insert.
        self.0
            .insert(key, registered)
            .unwrap_or_else(|_| swoole_rs_corebase::fatal("out of memory registering an event"))
    }
}

const READ: i64 = EventType::SW_EVENT_READ as i64;
const WRITE: i64 = EventType::SW_EVENT_WRITE as i64;
const FD_USER: c_int = FdType::SW_FD_USER as c_int;

static EVENT: Class = Class::new();

php_local! {
    /// `event_socket_map`: fd to its socket. One process map on a non-thread-safe
    /// PHP, as upstream's static: a thread-context coroutine's callback runs on its
    /// own thread and must find the fds the loop's thread added. Never dropped: what
    /// a ZTS thread leaves registered is leaked, as upstream's, not released once
    /// PHP is gone.
    static SOCKETS: RefCell<ManuallyDrop<Sockets>> = RefCell::new(ManuallyDrop::new(Sockets(Table::new())));
    /// The last `Registered::id` handed out.
    static ADDED: Cell<u64> = Cell::new(0);
}

/// The global functions `SW_FUNCTION_ALIAS` registers, in upstream's order.
const ALIASES: [(&CStr, &CStr); 11] = [
    (c"add", c"swoole_event_add"),
    (c"del", c"swoole_event_del"),
    (c"set", c"swoole_event_set"),
    (c"wait", c"swoole_event_wait"),
    (c"isset", c"swoole_event_isset"),
    (c"dispatch", c"swoole_event_dispatch"),
    (c"defer", c"swoole_event_defer"),
    (c"cycle", c"swoole_event_cycle"),
    (c"write", c"swoole_event_write"),
    (c"exit", c"swoole_event_exit"),
    (c"rshutdown", c"swoole_event_rshutdown"),
];

method_table! {
    static METHODS = [
        c"add" => zif_swoole_event_add, &event::arginfo_swoole_event_add, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"del" => zif_swoole_event_del, &event::arginfo_swoole_event_del, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"set" => zif_swoole_event_set, &event::arginfo_swoole_event_set, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"isset" => zif_swoole_event_isset, &event::arginfo_swoole_event_isset, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"dispatch" => zif_swoole_event_dispatch, &event::arginfo_swoole_event_dispatch, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"defer" => zif_swoole_event_defer, &event::arginfo_swoole_event_defer, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"cycle" => zif_swoole_event_cycle, &event::arginfo_swoole_event_cycle, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"write" => zif_swoole_event_write, &event::arginfo_swoole_event_write, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"wait" => zif_swoole_event_wait, &event::arginfo_swoole_event_wait, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"rshutdown" => zif_swoole_event_rshutdown, &event::arginfo_swoole_event_rshutdown, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"exit" => zif_swoole_event_exit, &event::arginfo_swoole_event_exit, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

/// MINIT: `Swoole\Event` (not instantiable) and the `swoole_event_*` functions.
/// # Errors
/// A registration bailed out.
pub fn minit(minit: &Minit) -> Result<(), Bailout> {
    minit.register_static(&EVENT, c"Swoole\\Event", METHODS.table());
    minit.function_aliases(&EVENT, METHODS.table(), &ALIASES)
}

fn fatal_warning(message: &CStr) {
    bailout::defer(native::fatal_warning(message));
}

/// `php_swoole_convert_to_fd(value)`, deferring a bailout.
fn fd(value: Option<&Value>) -> i32 {
    match value.map_or(Ok(-1), descriptor::convert) {
        Ok(fd) => fd,
        Err(error) => {
            error.defer();
            -1
        }
    }
}

fn registered(php: Php, fd: i32) -> bool {
    SOCKETS.with_borrow(php, |sockets| sockets.contains_key(fd))
}

/// `int` events, as the reactor takes them.
fn events_of(events: i64) -> c_int {
    events.c_cast::<c_int>()
}

/// The callable argument `value`, deferring a bailout.
fn callable(value: Option<&Value>, name: Option<&CStr>, allow_null: bool) -> Option<Rc<Callable>> {
    let exception = swoole_rs_phpswoole::exception();
    Callable::argument(value, name, allow_null, Some(exception)).unwrap_or_else(|error| {
        error.defer();
        None
    })
}

/// Dropped once the loop is out of the pass that may still be using it.
struct Later<T>(T);

impl<T: 'static> Deferred for Later<T> {
    /// Drop it, then raise a destructor's bailout from this frame, which owns
    /// nothing by then.
    fn run(self) {
        drop(self);
        bailout::finish();
    }

    /// The reactor was freed first: drop it now, and leave a bailout to the
    /// handler that freed it rather than jump out of the reactor's drop.
    fn dispose(self) {
        drop(self);
    }
}

/// Drop `value` once the loop is out of this pass; at once without a reactor.
fn later<T: 'static>(value: T) {
    if let Err(value) = event_api::swoole_event_defer_value(Later(value)) {
        value.dispose();
    }
}

/// `add(mixed $fd, ?callable $read = null, ?callable $write = null, int $events = SWOOLE_EVENT_READ)`.
fn add(call: &Call) {
    let php = call.php();
    let parsed = call.arity(1, 4).and_then(|()| call.long(4, READ));
    let Ok(events) = parsed else {
        call.set_bool(false);
        return;
    };
    if let Err(error) = loop_driver::check(call.php()) {
        error.defer();
        return;
    }
    let zfd = call.value(1);
    let fd = fd(zfd);
    if bailout::pending() {
        return;
    }
    let refused = if fd < 0 {
        Some(message!("unknown fd type"))
    } else if fd == 0 && events & WRITE != 0 {
        Some(message!("invalid socket fd [{fd}]"))
    } else if registered(php, fd) {
        Some(message!("already exist"))
    } else if events & (WRITE | READ) == 0 {
        Some(message!("invalid events"))
    } else {
        None
    };
    if let Some(message) = refused {
        fatal_warning(message.as_cstr());
        call.set_bool(false);
        return;
    }
    let readable = callable(call.value(2), Some(c"readable_callback"), true);
    if bailout::pending() {
        return;
    }
    if events & READ != 0 && readable.is_none() {
        missing(fd, "readable");
        call.set_bool(false);
        return;
    }
    let writable = callable(call.value(3), Some(c"writable_callback"), true);
    if bailout::pending() {
        return;
    }
    if events & WRITE != 0 && writable.is_none() {
        missing(fd, "writable");
        call.set_bool(false);
        return;
    }
    let socket = EventSocket::new(fd, FD_USER);
    if !socket.add(events_of(events)) {
        fatal_warning(c"swoole_event_add failed");
        socket.free(false);
        call.set_bool(false);
        return;
    }
    let object = zfd.map(|zfd| EventObject {
        zsocket: zfd.copy(),
        readable,
        writable,
    });
    // Dropped outside the borrow: a destructor may call back in.
    let id = ADDED.get(php) + 1;
    ADDED.set(php, id);
    let registered = Registered { id, socket, object };
    drop(SOCKETS.with_borrow_mut(php, |sockets| sockets.insert(fd, registered)));
    call.set_long(fd.into());
}

/// "Swoole\Event: unable to find <kind> callback of fd [<fd>]".
fn missing(fd: i32, kind: &str) {
    fatal_warning(message!("Swoole\\Event: unable to find {kind} callback of fd [{fd}]").as_cstr());
}

fn not_found(fd: i32) {
    fatal_warning(message!("socket[{fd}] is not found in the reactor").as_cstr());
}

/// `write(mixed $fd, string $data): bool`.
fn write(call: &Call) {
    let php = call.php();
    let Ok(data) = call.arity(2, 2).and_then(|()| call.string(2, b"")) else {
        call.set_bool(false);
        return;
    };
    if data.is_empty() {
        fatal_warning(c"data empty");
        call.set_bool(false);
        return;
    }
    let fd = fd(call.value(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        fatal_warning(c"unknown type");
        call.set_bool(false);
        return;
    }
    if !registered(php, fd) {
        not_found(fd);
        call.set_bool(false);
        return;
    }
    if let Err(error) = loop_driver::check(call.php()) {
        error.defer();
        return;
    }
    let written = SOCKETS.with_borrow(php, |sockets| sockets.get(fd).map(|r| r.socket.write(data)));
    call.set_bool(written.is_some_and(|written| written >= 0));
}

/// `set(mixed $fd, ?callable $read = null, ?callable $write = null, int $events = 0)`.
fn set(call: &Call) {
    let php = call.php();
    if !has_reactor() {
        fatal_warning(c"reactor is not ready, cannot call swoole_event_set");
        call.set_bool(false);
        return;
    }
    let Ok(events) = call.arity(1, 4).and_then(|()| call.long(4, 0)) else {
        call.set_bool(false);
        return;
    };
    let fd = fd(call.value(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        call.set_bool(false);
        return;
    }
    if !SOCKETS.with_borrow(php, |sockets| {
        sockets.get(fd).is_some_and(|r| r.object.is_some())
    }) {
        not_found(fd);
        call.set_bool(false);
        return;
    }
    let readable = callable(call.value(2), Some(c"readable_callback"), true);
    if bailout::pending() {
        return;
    }
    let writable = callable(call.value(3), Some(c"writable_callback"), true);
    if bailout::pending() {
        return;
    }
    // The replaced callables, which the reactor may still be in.
    let (has, replaced) = SOCKETS.with_borrow_mut(php, |sockets| {
        let Some(object) = sockets.get_mut(fd).and_then(|r| r.object.as_mut()) else {
            return ((false, false), [readable, writable]);
        };
        let replace = |new: Option<Rc<Callable>>, slot: &mut Option<Rc<Callable>>| {
            new.and_then(|new| slot.replace(new))
        };
        let replaced = [
            replace(readable, &mut object.readable),
            replace(writable, &mut object.writable),
        ];
        (
            (object.readable.is_some(), object.writable.is_some()),
            replaced,
        )
    });
    replaced.into_iter().flatten().for_each(later);
    if events & READ != 0 && !has.0 {
        missing(fd, "readable");
        call.set_bool(false);
        return;
    }
    if events & WRITE != 0 && !has.1 {
        missing(fd, "writable");
        call.set_bool(false);
        return;
    }
    let set = SOCKETS.with_borrow(php, |sockets| {
        sockets
            .get(fd)
            .is_some_and(|r| r.socket.set(events_of(events)))
    });
    if !set {
        fatal_warning(c"Swoole\\Event::set failed");
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `del(mixed $fd): bool`.
fn del(call: &Call) {
    let php = call.php();
    if !has_reactor() {
        fatal_warning(c"reactor is not ready, cannot call swoole_event_del");
        call.set_bool(false);
        return;
    }
    if call.arity(1, 1).is_err() {
        call.set_bool(false);
        return;
    }
    let fd = fd(call.value(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        fatal_warning(c"unknown type");
        call.set_bool(false);
        return;
    }
    let Some(registered) = SOCKETS.with_borrow_mut(php, |sockets| sockets.remove(fd)) else {
        call.set_bool(false);
        return;
    };
    // Its object, freed once the loop is out of it.
    if let Some(object) = registered.object {
        later(object);
    }
    let deleted = registered.socket.del();
    // The fd stays the user's.
    registered.socket.free(true);
    call.set_bool(deleted);
}

/// `Swoole\Event::defer`'s callable: run, then released, even when it bails out.
struct Run(Rc<Callable>);

impl Deferred for Run {
    /// `event_defer_callback`.
    fn run(self) {
        let result = invoke(&self.0, None).and_then(|ok| {
            if ok {
                Ok(())
            } else {
                native::warning(c"Swoole\\Event::defer callback handler error")
            }
        });
        drop(self);
        bailout::raise(result);
    }

    /// The reactor was freed first: release it, leaving a bailout deferred.
    fn dispose(self) {
        drop(self);
    }
}

/// `defer(callable $callback): true`.
fn defer(call: &Call) {
    if call.arity(1, 1).is_err() {
        call.set_bool(false);
        return;
    }
    if let Err(error) = loop_driver::ensure() {
        error.defer();
        return;
    }
    let Some(callback) = callable(call.value(1), None, false) else {
        call.set_bool(false);
        return;
    };
    // The reactor runs and then frees it.
    if let Err(run) = event_api::swoole_event_defer_value(Run(callback)) {
        run.dispose();
    }
    call.set_bool(true);
}

/// The reactor's idle or future task: `event_cycle_callback` of its callable.
struct Cycle(Rc<Callable>);

impl CycleTask for Cycle {
    fn run(&self) {
        // Its own reference: the callback may clear this task.
        let callable = Rc::clone(&self.0);
        let result = invoke(&callable, None).and_then(|ok| {
            if ok {
                Ok(())
            } else {
                native::warning(c"Swoole\\Event::end callback handler error")
            }
        });
        drop(callable);
        bailout::raise(result);
    }

    /// Release the callable, then raise a destructor's bailout from this
    /// frame, which owns nothing by then.
    fn retire(self: Box<Self>) {
        drop(self);
        bailout::finish();
    }
}

/// `cycle(?callable $callback, bool $before = false): bool`.
fn cycle(call: &Call) {
    let Ok(before) = call.arity(1, 2).and_then(|()| call.bool(2, false)) else {
        call.set_bool(false);
        return;
    };
    if let Err(error) = loop_driver::check(call.php()) {
        error.defer();
        return;
    }
    let zcallback = call.value(1);
    if zcallback.is_none_or(Value::is_null) {
        if !event_api::swoole_event_cycle_isset(before) {
            call.set_bool(false);
            return;
        }
        event_api::swoole_event_cycle(before, None);
    } else {
        event_api::swoole_event_cycle(before, None);
        let callback = callable(zcallback, None, false);
        if bailout::pending() {
            return;
        }
        let Some(callback) = callback else {
            call.set_bool(false);
            return;
        };
        event_api::swoole_event_cycle(before, Some(Box::new(Cycle(callback))));
    }
    call.set_bool(true);
}

/// Clears the cycle tasks before `wait` frees the reactor, which then drops
/// their callables: a loop left with one set (a callback that exited, or one
/// never cleared) would lose it.
fn release_cycle_tasks() {
    event_api::swoole_event_cycle(false, None);
    event_api::swoole_event_cycle(true, None);
}

/// `isset(mixed $fd, int $events = SWOOLE_EVENT_READ | SWOOLE_EVENT_WRITE): bool`.
fn isset(call: &Call) {
    let php = call.php();
    if !has_reactor() {
        call.set_bool(false);
        return;
    }
    let Ok(events) = call.arity(1, 2).and_then(|()| call.long(2, READ | WRITE)) else {
        call.set_bool(false);
        return;
    };
    let fd = fd(call.value(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        fatal_warning(c"unknown type");
        call.set_bool(false);
        return;
    }
    let set = SOCKETS.with_borrow(php, |sockets| {
        sockets
            .get(fd)
            .is_some_and(|r| !r.socket.removed() && i64::from(r.socket.events()) & events != 0)
    });
    call.set_bool(set);
}

/// Call `callable` with `arg`: whether it succeeded. The caller holds its own
/// reference, which keeps it through self-replacement and coroutine suspension.
fn invoke(callable: &Callable, arg: Option<&Owned>) -> Result<bool, Bailout> {
    let args = arg.map_or(&[][..], std::slice::from_ref);
    let retval = Owned::default();
    swoole_rs_phpcoroutines::call_handler(
        callable,
        args,
        &retval,
        swoole_rs_phpprocess::enable_coroutine(),
    )
}

/// A reactor handler's result: a bailout is raised from its frame, which owns
/// nothing by then, after the Rust work completed.
fn handled(result: Result<bool, Bailout>) -> bool {
    result.unwrap_or_else(|error| {
        bailout::raise(Err(error));
        false
    })
}

/// The user fd type's read handler.
struct Readable;
/// The user fd type's write handler.
struct Writable;
/// The user fd type's error handler.
struct Error;

impl PhpFdHandler for Readable {
    fn handle(php: Php<'_>, fd: c_int) -> bool {
        handled(ready(php, fd, false))
    }
}

impl PhpFdHandler for Writable {
    fn handle(php: Php<'_>, fd: c_int) -> bool {
        handled(ready(php, fd, true))
    }
}

impl PhpFdHandler for Error {
    fn handle(php: Php<'_>, fd: c_int) -> bool {
        handled(error_event(php, fd))
    }
}

/// The user fd type's read, write and error handlers, unless they are set.
fn install_handlers(php: Php<'_>) {
    let read = EventType::SW_EVENT_READ as c_int;
    if event_api::swoole_event_isset_handler(FD_USER, read) {
        return;
    }
    reactor::set_fd_handler::<Readable>(php, FD_USER, read);
    reactor::set_fd_handler::<Writable>(php, FD_USER, EventType::SW_EVENT_WRITE as c_int);
    reactor::set_fd_handler::<Error>(php, FD_USER, EventType::SW_EVENT_ERROR as c_int);
}

/// The errno's description, as libc's `strerror`.
pub(crate) fn errno_message(code: c_int) -> String {
    let text = std::io::Error::from_raw_os_error(code).to_string();
    match text.strip_suffix(&format!(" (os error {code})")) {
        Some(description) => description.to_owned(),
        None => text,
    }
}

/// `apply` to `fd`'s registration `id`, unless PHP has since deleted or replaced it.
fn registration<T>(
    php: Php,
    fd: i32,
    id: u64,
    apply: impl FnOnce(&mut Registered) -> T,
) -> Option<T> {
    SOCKETS.with_borrow_mut(php, |sockets| {
        sockets.get_mut(fd).filter(|r| r.id == id).map(apply)
    })
}

/// The read (`write` false) or write handler of a user fd: run its callback;
/// on failure warn, detach and delete the fd.
fn ready(php: Php, fd: i32, write: bool) -> Result<bool, Bailout> {
    let found = SOCKETS.with_borrow(php, |sockets| {
        let registered = sockets.get(fd)?;
        let object = registered.object.as_ref()?;
        let callable = if write {
            &object.writable
        } else {
            &object.readable
        };
        Some((registered.id, callable.clone()?, object.zsocket.copy()))
    });
    let Some((id, callable, zsocket)) = found else {
        return Ok(true);
    };
    let called = invoke(&callable, Some(&zsocket));
    drop(callable);
    if called? {
        return Ok(true);
    }
    let reported = self::fd(Some(&zsocket));
    drop(zsocket);
    let kind = if write { "writable" } else { "readable" };
    fatal_warning(
        message!(
            "Swoole\\Event: {kind} callback handler error, fd [{reported}] will be removed from reactor"
        )
        .as_cstr(),
    );
    // The detached object, freed once the loop is out of it.
    let object = registration(php, fd, id, |r| r.object.take()).flatten();
    if let Some(object) = object {
        later(object);
    }
    registration(php, fd, id, |r| r.socket.del());
    if bailout::relay_to_c(Ok(())) {
        Err(Bailout)
    } else {
        Ok(false)
    }
}

/// The error handler: dispatch to the read or write handler unless the socket
/// reported an error; then warn, free the fd's object and delete it.
fn error_event(php: Php, fd: i32) -> Result<bool, Bailout> {
    let found = SOCKETS.with_borrow(php, |sockets| {
        sockets.get(fd).map(|r| (r.id, r.socket.events()))
    });
    let Some((id, events)) = found else {
        return Ok(true);
    };
    if events & EventType::SW_EVENT_ERROR as i32 == 0 {
        return ready(php, fd, events & EventType::SW_EVENT_READ as i32 == 0);
    }
    match swoole_rs_reactor::socket_error(fd) {
        Ok(0) => {}
        Ok(error) => {
            let description = errno_message(error);
            let message =
                message!("swoole_event->onError[1]: socket error. Error: {description} [{error}]");
            native::fatal_warning(message.as_cstr())?;
        }
        Err(code) => {
            let message = message!("swoole_event->onError[1]: getsockopt[sock={fd}] failed");
            native::system_error(code, E_WARNING, message.as_cstr())?;
        }
    }
    drop(registration(php, fd, id, |r| r.object.take()));
    bailout::outcome(())?;
    registration(php, fd, id, |r| r.socket.del());
    Ok(true)
}

swoole_rs_zend::handlers! {
    zif_swoole_event_wait => wait;
    zif_swoole_event_exit => exit;
    zif_swoole_event_dispatch => dispatch;
    zif_swoole_event_add => add;
    zif_swoole_event_set => set;
    zif_swoole_event_del => del;
    zif_swoole_event_write => write;
    zif_swoole_event_defer => defer;
    zif_swoole_event_cycle => cycle;
    zif_swoole_event_isset => isset;
    zif_swoole_event_rshutdown => rshutdown;
}

/// The shutdown function `init` registers. A Zend bailout ends it, as inside
/// upstream's `zend_try`, and is not raised; a thread exit still is.
fn rshutdown(_call: &Call) {
    let _caught = bailout::relay_to_c(loop_driver::shutdown());
}

fn wait(_call: &Call) {
    bailout::defer(loop_driver::wait());
}
fn exit(_call: &Call) {
    bailout::defer(loop_driver::exit());
}
fn dispatch(call: &Call) {
    match loop_driver::dispatch() {
        Ok(value) => call.set_bool(value),
        Err(error) => error.defer(),
    }
}

/// `php_swoole_reactor_init()`: whether the event loop is ready. Raises a
/// bailout, so only an export C calls may call it last.
#[must_use]
pub fn reactor_init() -> bool {
    let result = loop_driver::init().unwrap_or_else(|error| {
        error.defer();
        false
    });
    bailout::finish();
    result
}

/// `php_swoole_convert_to_fd(value)`, raising a bailout as [`reactor_init`].
#[must_use]
pub fn convert_to_fd(value: Option<&Value>) -> i32 {
    let result = fd(value);
    bailout::finish();
    result
}

/// `Swoole\Event::wait()`'s native lifecycle effect, completed before raising
/// a contained bailout, as [`reactor_init`].
pub fn wait_and_raise() {
    bailout::raise(loop_driver::wait());
}
