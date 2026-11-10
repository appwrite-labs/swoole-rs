//! PHP `Swoole\Event`: registration, descriptor conversion, callback custody,
//! fd registry and loop admission, dispatch and shutdown policy over the
//! thread's reactor. Its reactor handlers and deferred tasks raise a contained
//! Zend bailout only after Rust cleanup has completed.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod descriptor;
mod loop_driver;
mod native;

use native::has_reactor;
use std::{
    cell::RefCell,
    ffi::{CStr, c_int, c_void},
    fmt,
    io::Write,
    ptr,
};
use swoole_rs_corebase::constants::EventType;
use swoole_rs_memory::flat::Table;
use swoole_rs_phphelpers::{callable, function_call};
use swoole_rs_phpstubs::event;
use swoole_rs_reactor::{Event, ReactorTask, Socket, event_api};
use swoole_rs_zend::{
    Call, Class, Minit,
    bailout::{self, Bailout, trap},
    php_local,
    sys::{IS_NULL, swr_zend_callable_warning, swr_zend_zval_release, swr_zval_try_addref, zval},
};
use swoole_rs_zend::{method_table, sys as z};

/// `EventObject`: the fd's PHP value and its callbacks (callables).
#[repr(C)]
struct EventObject {
    zsocket: zval,
    readable: *mut c_void,
    writable: *mut c_void,
}

const READ: i64 = EventType::SW_EVENT_READ as i64;
const WRITE: i64 = EventType::SW_EVENT_WRITE as i64;

static EVENT: Class = Class::new();

php_local! {
    /// `event_socket_map`: fd to `Socket *`. One process map on a non-thread-safe
    /// PHP, as upstream's static: a thread-context coroutine's callback runs on
    /// its own thread and must find the fds the loop's thread added.
    static SOCKETS: RefCell<Table<usize>> = RefCell::new(Table::new());
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
/// # Safety
/// Zend's MINIT of this module, with its number.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_event_minit(module_number: c_int) {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module_number) };
    minit.register_static(&EVENT, c"Swoole\\Event", &METHODS.0);
    bailout::raise(minit.function_aliases(&EVENT, &METHODS.0, &ALIASES));
}

/// A NUL-terminated message on the stack.
struct Message([u8; 160]);

impl Message {
    fn new(args: fmt::Arguments) -> Self {
        let mut buffer = [0; 160];
        let mut out = &mut buffer[..159];
        // A message too long for the buffer is cut, as `snprintf` would.
        let _cut = out.write_fmt(args);
        Self(buffer)
    }

    fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.0).unwrap_or(c"")
    }
}

fn fatal_warning(message: &CStr) {
    bailout::defer(native::fatal_warning(message));
}

fn fd(zsocket: *mut zval) -> i32 {
    // SAFETY: the handler lends its live descriptor candidate.
    match unsafe { descriptor::convert(zsocket) } {
        Ok(fd) => fd,
        Err(error) => {
            error.defer();
            -1
        }
    }
}

/// The user socket `add` registered for `fd`.
fn socket<'a>(fd: i32) -> Option<Socket<'a>> {
    let socket = SOCKETS.with_borrow(|sockets| sockets.get(usize::try_from(fd).ok()?).copied())?;
    // SAFETY: the registry holds records `add` made by this PHP's threads, until `del` removes
    // and frees them.
    unsafe { Socket::from_ptr(socket as *mut _) }
}

/// The `EventObject` a user socket carries.
fn object_of(socket: Socket<'_>) -> *mut EventObject {
    socket.read(|record| record.object).cast()
}

/// `int` events, as the reactor takes them.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream passes the zend_long to an int parameter"
)]
fn events_of(events: i64) -> c_int {
    events as c_int
}

fn is_null(value: *mut zval) -> bool {
    if value.is_null() {
        return true;
    }
    // SAFETY: a live zval.
    let value = unsafe { *value };
    // SAFETY: every zval sets `u1.type_info`.
    let kind = unsafe { value.u1.type_info };
    kind & 0xff == IS_NULL
}

fn callable(zfn: *mut zval, name: Option<&CStr>, allow_null: bool) -> *mut c_void {
    let name = name.map_or(ptr::null(), CStr::as_ptr);
    let exception = swoole_rs_phpswoole::exception_class();
    // SAFETY: the live candidate, static diagnostic name and C reporter remain
    // valid through the contained SDK resolution.
    match unsafe { callable::factory(zfn, name, allow_null, exception, swr_zend_callable_warning) }
    {
        Ok(handle) => handle.cast(),
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    }
}
fn release(handle: *mut c_void) -> Result<(), Bailout> {
    // SAFETY: callers transfer one live callback owner, exactly once.
    unsafe { callable::release(handle.cast()) }
}

/// `add(mixed $fd, ?callable $read = null, ?callable $write = null, int $events = SWOOLE_EVENT_READ)`.
fn add(call: &Call) {
    let parsed = call.arity(1, 4).and_then(|()| call.long(4, READ));
    let Ok(events) = parsed else {
        call.set_bool(false);
        return;
    };
    let (zfd, zread, zwrite) = (call.zval(1), call.zval(2), call.zval(3));
    if let Err(error) = loop_driver::check() {
        error.defer();
        return;
    }
    let fd = fd(zfd);
    if bailout::pending() {
        return;
    }
    let refused = if fd < 0 {
        Some(Message::new(format_args!("unknown fd type")))
    } else if fd == 0 && events & WRITE != 0 {
        Some(Message::new(format_args!("invalid socket fd [{fd}]")))
    } else if socket(fd).is_some() {
        Some(Message::new(format_args!("already exist")))
    } else if events & (WRITE | READ) == 0 {
        Some(Message::new(format_args!("invalid events")))
    } else {
        None
    };
    if let Some(message) = refused {
        fatal_warning(message.as_cstr());
        call.set_bool(false);
        return;
    }
    let readable = callable(zread, Some(c"readable_callback"), true);
    if bailout::pending() {
        return;
    }
    if events & READ != 0 && readable.is_null() {
        missing(fd, "readable");
        call.set_bool(false);
        return;
    }
    let writable = callable(zwrite, Some(c"writable_callback"), true);
    if bailout::pending() {
        if !readable.is_null() {
            bailout::defer(release(readable));
        }
        return;
    }
    if events & WRITE != 0 && writable.is_null() {
        missing(fd, "writable");
        if !readable.is_null() {
            bailout::defer(release(readable));
        }
        call.set_bool(false);
        return;
    }
    // SAFETY: argument 1 is passed and live; the object holds a reference.
    unsafe { swr_zval_try_addref(zfd) };
    let object = Box::into_raw(Box::new(EventObject {
        // SAFETY: as above.
        zsocket: unsafe { zfd.read() },
        readable,
        writable,
    }));
    let socket = native::socket_make(fd, object.cast());
    // SAFETY: the socket just made, on the live reactor.
    if unsafe { event_api::swoole_event_add(socket, events_of(events)) } < 0 {
        fatal_warning(c"swoole_event_add failed");
        // SAFETY: the reactor refused the socket; nothing else holds it.
        unsafe { native::socket_free(socket) };
        // SAFETY: the object made above, which nothing else holds.
        bailout::raise(unsafe { free_object(object) });
        call.set_bool(false);
        return;
    }
    let key = usize::try_from(fd).unwrap_or(usize::MAX);
    if SOCKETS
        .with_borrow_mut(|sockets| sockets.insert(key, socket as usize))
        .is_err()
    {
        // Out of memory: abort, as upstream's map insert.
        std::process::abort();
    }
    call.set_long(fd.into());
}

/// "Swoole\Event: unable to find <kind> callback of fd [<fd>]".
fn missing(fd: i32, kind: &str) {
    let message = Message::new(format_args!(
        "Swoole\\Event: unable to find {kind} callback of fd [{fd}]"
    ));
    fatal_warning(message.as_cstr());
}

fn not_found(fd: i32) {
    let message = Message::new(format_args!("socket[{fd}] is not found in the reactor"));
    fatal_warning(message.as_cstr());
}

/// `write(mixed $fd, string $data): bool`.
fn write(call: &Call) {
    let Ok(data) = call.arity(2, 2).and_then(|()| call.string(2, b"")) else {
        call.set_bool(false);
        return;
    };
    if data.is_empty() {
        fatal_warning(c"data empty");
        call.set_bool(false);
        return;
    }
    let fd = fd(call.zval(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        fatal_warning(c"unknown type");
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        not_found(fd);
        call.set_bool(false);
        return;
    };
    if let Err(error) = loop_driver::check() {
        error.defer();
        return;
    }
    // SAFETY: a registered socket and the argument's bytes.
    let written =
        unsafe { event_api::swoole_event_write(socket.as_ptr(), data.as_ptr().cast(), data.len()) };
    call.set_bool(written >= 0);
}

/// `set(mixed $fd, ?callable $read = null, ?callable $write = null, int $events = 0)`.
fn set(call: &Call) {
    if !has_reactor() {
        fatal_warning(c"reactor is not ready, cannot call swoole_event_set");
        call.set_bool(false);
        return;
    }
    let Ok(events) = call.arity(1, 4).and_then(|()| call.long(4, 0)) else {
        call.set_bool(false);
        return;
    };
    let fd = fd(call.zval(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        not_found(fd);
        call.set_bool(false);
        return;
    };
    let object = object_of(socket);
    let readable = callable(call.zval(2), Some(c"readable_callback"), true);
    if bailout::pending() {
        return;
    }
    let writable = callable(call.zval(3), Some(c"writable_callback"), true);
    if bailout::pending() {
        if !readable.is_null() {
            bailout::defer(release(readable));
        }
        return;
    }
    // SAFETY: the registered socket's live object.
    let object = unsafe { &mut *object };
    for (new, slot) in [
        (readable, &mut object.readable),
        (writable, &mut object.writable),
    ] {
        if new.is_null() {
            continue;
        }
        if !slot.is_null() {
            // The replaced callable: the reactor may still be in it.
            native::defer(deferred_release, *slot, release_unrun);
        }
        *slot = new;
    }
    if events & READ != 0 && object.readable.is_null() {
        missing(fd, "readable");
        call.set_bool(false);
        return;
    }
    if events & WRITE != 0 && object.writable.is_null() {
        missing(fd, "writable");
        call.set_bool(false);
        return;
    }
    // SAFETY: a registered socket.
    if unsafe { event_api::swoole_event_set(socket.as_ptr(), events_of(events)) } < 0 {
        fatal_warning(c"Swoole\\Event::set failed");
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `del(mixed $fd): bool`.
fn del(call: &Call) {
    if !has_reactor() {
        fatal_warning(c"reactor is not ready, cannot call swoole_event_del");
        call.set_bool(false);
        return;
    }
    if call.arity(1, 1).is_err() {
        call.set_bool(false);
        return;
    }
    let fd = fd(call.zval(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        fatal_warning(c"unknown type");
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        call.set_bool(false);
        return;
    };
    // Its object, freed once the loop is out of it.
    native::defer(
        deferred_free_object,
        object_of(socket).cast(),
        free_unrun_object,
    );
    // SAFETY: a registered socket.
    let deleted = unsafe { event_api::swoole_event_del(socket.as_ptr()) };
    SOCKETS.with_borrow_mut(|sockets| usize::try_from(fd).ok().and_then(|fd| sockets.remove(fd)));
    // The fd stays the user's.
    socket.edit(|record| record.fd = -1);
    // SAFETY: removed from the reactor and the registry.
    unsafe { native::socket_free(socket.as_ptr()) };
    call.set_bool(deleted == 0);
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
    let callback = callable(call.zval(1), None, false);
    if bailout::pending() {
        return;
    }
    // The reactor runs and then frees it.
    native::defer(run_deferred, callback, release_unrun);
    call.set_bool(true);
}

/// `cycle(?callable $callback, bool $before = false): bool`.
fn cycle(call: &Call) {
    let Ok(before) = call.arity(1, 2).and_then(|()| call.bool(2, false)) else {
        call.set_bool(false);
        return;
    };
    let zcallback = call.zval(1);
    if let Err(error) = loop_driver::check() {
        error.defer();
        return;
    }
    let task = *native::cycle_task(before);
    if is_null(zcallback) {
        if task.callback.is_none() {
            call.set_bool(false);
            return;
        }
        // The task's callable, released after the loop is out of it.
        native::defer(deferred_release, task.data, release_unrun);
        *native::cycle_task(before) = ReactorTask {
            callback: None,
            data: ptr::null_mut(),
        };
    } else {
        if !task.data.is_null() {
            native::defer(deferred_release, task.data, release_unrun);
        }
        let callback = callable(zcallback, None, false);
        if bailout::pending() {
            return;
        }
        // The reactor holds the callable from now on.
        *native::cycle_task(before) = ReactorTask {
            callback: Some(run_cycle),
            data: callback,
        };
    }
    call.set_bool(true);
}

/// Releases the cycle callables the reactor holds, as `wait` frees it: a loop left
/// with one set (a callback that exited, or one never cleared) would lose it.
fn release_cycle_tasks() -> Result<(), Bailout> {
    let mut result = Ok(());
    for before in [false, true] {
        let unset = ReactorTask {
            callback: None,
            data: ptr::null_mut(),
        };
        let task = std::mem::replace(native::cycle_task(before), unset);
        if !task.data.is_null() {
            result = result.and(release(task.data));
        }
    }
    result
}

/// `isset(mixed $fd, int $events = SWOOLE_EVENT_READ | SWOOLE_EVENT_WRITE): bool`.
fn isset(call: &Call) {
    if !has_reactor() {
        call.set_bool(false);
        return;
    }
    let Ok(events) = call.arity(1, 2).and_then(|()| call.long(2, READ | WRITE)) else {
        call.set_bool(false);
        return;
    };
    let fd = fd(call.zval(1));
    if bailout::pending() {
        return;
    }
    if fd < 0 {
        fatal_warning(c"unknown type");
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        call.set_bool(false);
        return;
    };
    let (removed, registered) = socket.read(|record| (record.removed, record.events));
    call.set_bool(removed == 0 && i64::from(registered) & events != 0);
}

/// Call `callable` with `argv`: whether it succeeded.
fn invoke(callable: *mut c_void, argv: *mut zval, argc: u32) -> Result<bool, Bailout> {
    // SAFETY: keep an independent callback owner alive during self-replacement
    // and coroutine suspension; no Rust borrow survives invocation.
    let retained = unsafe { callable::copy(callable.cast()) };
    // SAFETY: this owner is stable until release after the contained invocation.
    let cache = unsafe { callable::cache(retained) };
    // SAFETY: live cache/arguments; the coroutine leaf and C warning reporter
    // are called inside Zend's C traps by the common Rust API.
    let result = unsafe {
        function_call(
            cache,
            argc,
            argv,
            ptr::null_mut(),
            swoole_rs_phpprocess::enable_coroutine(),
            swoole_rs_phpcoroutines::create_coroutine,
            swr_zend_callable_warning,
        )
    };
    // SAFETY: release the independent owner after invocation, including errors.
    let released = unsafe { callable::release(retained) };
    result.and_then(|value| released.map(|()| value))
}

/// A reactor handler's result: a bailout is raised from its frame, which owns
/// nothing by then, after the Rust work completed.
fn handled(result: Result<c_int, Bailout>) -> c_int {
    result.unwrap_or_else(|error| {
        bailout::raise(Err(error));
        -1
    })
}

/// The user socket of an event the reactor lends.
/// # Safety
/// `event` is a live `swoole::Event` of a user socket `add` registered.
unsafe fn event_socket<'a>(event: *mut c_void) -> Option<Socket<'a>> {
    // SAFETY: per the caller.
    let socket = unsafe { (*event.cast::<Event>()).socket };
    // SAFETY: per the caller, a live registered socket while the reactor lends its event.
    unsafe { Socket::from_ptr(socket.cast()) }
}

/// The user fd type's read handler.
pub(crate) unsafe extern "C" fn readable(_reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: the reactor lends the event of a user socket `add` registered.
    handled(unsafe { ready(event, false) })
}

/// The user fd type's write handler.
pub(crate) unsafe extern "C" fn writable(_reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: as above.
    handled(unsafe { ready(event, true) })
}

/// The user fd type's error handler.
pub(crate) unsafe extern "C" fn error(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: the reactor lends the event of a user socket `add` registered.
    handled(unsafe { error_event(reactor, event) })
}

/// The read (`write` false) or write handler of a user fd: run its callback;
/// on failure warn, detach and delete the fd.
/// # Safety
/// `event` is a live event of a user socket `add` registered, carrying its object.
unsafe fn ready(event: *mut c_void, write: bool) -> Result<c_int, Bailout> {
    // SAFETY: per the caller.
    let Some(socket) = (unsafe { event_socket(event) }) else {
        return Ok(-1);
    };
    let object = object_of(socket);
    // SAFETY: per the caller, the live object.
    let peo = unsafe { &*object };
    let callable = if write { peo.writable } else { peo.readable };
    // `argv[0] = peo->zsocket`: a borrowed copy.
    let mut argv = peo.zsocket;
    if invoke(callable, &raw mut argv, 1)? {
        return Ok(0);
    }
    let mut zsocket = peo.zsocket;
    let fd = fd(&raw mut zsocket);
    let kind = if write { "writable" } else { "readable" };
    let message = Message::new(format_args!(
        "Swoole\\Event: {kind} callback handler error, fd [{fd}] will be removed from reactor"
    ));
    fatal_warning(message.as_cstr());
    socket.edit(|record| record.object = ptr::null_mut());
    // The detached object, freed once the loop is out of it.
    native::defer(deferred_free_object, object.cast(), free_unrun_object);
    // SAFETY: per the caller.
    unsafe { event_api::swoole_event_del(socket.as_ptr()) };
    if bailout::relay_to_c(Ok(())) {
        Err(Bailout)
    } else {
        Ok(-1)
    }
}

/// `event_object_free`: drop an fd's callbacks and PHP value.
/// # Safety
/// `object` is an `EventObject` from `add`, not used again.
unsafe fn free_object(object: *mut EventObject) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    let mut peo = unsafe { Box::from_raw(object) };
    let mut bailed = false;
    for callable in [peo.readable, peo.writable] {
        if !callable.is_null() {
            bailed |= release(callable).is_err();
        }
    }
    // SAFETY: the object's reference to its PHP value.
    bailed |= unsafe { trap(swr_zend_zval_release, (&raw mut peo.zsocket).cast()) }.is_err();
    if bailed { Err(Bailout) } else { Ok(()) }
}

/// The deferred `event_object_free`.
extern "C-unwind" fn deferred_free_object(object: *mut c_void) {
    // SAFETY: the object `del` or a failed callback scheduled, freed once.
    bailout::raise(unsafe { free_object(object.cast()) });
}

/// The deferred release of a replaced callable.
extern "C-unwind" fn deferred_release(callable: *mut c_void) {
    bailout::raise(release(callable));
}

/// The reactor was freed before a deferred release ran: release now, and leave a
/// bailout to the handler that freed it rather than jump out of the reactor's drop.
extern "C-unwind" fn release_unrun(callable: *mut c_void) {
    bailout::defer(release(callable));
}

/// As [`release_unrun`], for a deferred `event_object_free`.
extern "C-unwind" fn free_unrun_object(object: *mut c_void) {
    // SAFETY: the object `del` or a failed callback scheduled, which never ran.
    bailout::defer(unsafe { free_object(object.cast()) });
}

/// `event_defer_callback`: run and free a deferred callable, which is freed even when
/// the callback bails out.
extern "C-unwind" fn run_deferred(callable: *mut c_void) {
    let result = invoke(callable, ptr::null_mut(), 0).and_then(|ok| {
        if ok {
            Ok(())
        } else {
            native::warning(c"Swoole\\Event::defer callback handler error")
        }
    });
    let released = release(callable);
    bailout::raise(result.and(released));
}

/// `event_cycle_callback`: run the cycle callable.
extern "C-unwind" fn run_cycle(callable: *mut c_void) {
    let result = invoke(callable, ptr::null_mut(), 0).and_then(|ok| {
        if ok {
            Ok(())
        } else {
            native::warning(c"Swoole\\Event::end callback handler error")
        }
    });
    bailout::raise(result);
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

/// `php_swoole_reactor_init()`: 0, or -1 after an error.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_reactor_init() -> i32 {
    let result = loop_driver::init().unwrap_or_else(|error| {
        error.defer();
        -1
    });
    bailout::finish();
    result
}
/// # Safety
/// The caller lends a live initialized descriptor candidate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_convert_to_fd(value: *mut zval) -> i32 {
    let result = fd(value);
    bailout::finish();
    result
}
/// Native lifecycle effect, completed before resuming a contained bailout.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpevents_wait() {
    bailout::raise(loop_driver::wait());
}

/// The error handler: dispatch to the read or write handler unless the socket
/// reported an error; then warn, free the fd's object and delete it.
/// # Safety
/// The reactor lends a live event of a user socket `add` registered.
unsafe fn error_event(reactor: *mut c_void, event: *mut c_void) -> Result<c_int, Bailout> {
    // SAFETY: per the caller.
    let reported_fd = unsafe { (*event.cast::<Event>()).fd };
    // SAFETY: per the caller.
    let Some(socket) = (unsafe { event_socket(event) }) else {
        return Ok(-1);
    };
    let (fd, fd_type, events) = socket.read(|record| (record.fd, record.fd_type, record.events));
    if events & EventType::SW_EVENT_ERROR as i32 == 0 {
        let kind = u32::from(events & EventType::SW_EVENT_READ as i32 == 0);
        // SAFETY: the live reactor's handler tables.
        let handlers = unsafe { swoole_rs_reactor::reactor_upstream::prepared(reactor) }.handlers;
        // SAFETY: as above; `get_handler` always finds one, as upstream calls it unchecked.
        let Some(handler) = (unsafe {
            swoole_rs_reactor::swr_v1_reactor_handlers_get(handlers, kind, fd_type, true)
        }) else {
            std::process::abort()
        };
        // SAFETY: the registered handler, with the reactor's own event.
        return Ok(unsafe { handler(reactor, event) });
    }
    let mut error = 0_i32;
    let mut length = 4 as libc::socklen_t;
    // SAFETY: valid writable integer/length storage and a borrowed descriptor.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&raw mut error).cast(),
            &raw mut length,
        )
    } < 0
    {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        let message = Message::new(format_args!(
            "swoole_event->onError[1]: getsockopt[sock={reported_fd}] failed"
        ));
        native::system_error(code, swoole_rs_zend::E_WARNING, message.as_cstr())?;
    }
    if error != 0 {
        let description = native::strerror(error);
        let message = Message::new(format_args!(
            "swoole_event->onError[1]: socket error. Error: {} [{error}]",
            description.to_string_lossy()
        ));
        native::fatal_warning(message.as_cstr())?;
    }
    // SAFETY: the error path consumes this object once before removing the socket.
    unsafe { free_object(object_of(socket)) }?;
    // SAFETY: the event still lends this socket until deletion completes.
    unsafe { event_api::swoole_event_del(socket.as_ptr()) };
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_cut_and_terminated() {
        let message = Message::new(format_args!("socket[{}] is not found in the reactor", 7));
        assert_eq!(
            message.as_cstr(),
            c"socket[7] is not found in the reactor",
            "formatted"
        );
        let long = Message::new(format_args!("{}", "x".repeat(400)));
        assert_eq!(long.as_cstr().count_bytes(), 159, "cut to the buffer");
    }
}
