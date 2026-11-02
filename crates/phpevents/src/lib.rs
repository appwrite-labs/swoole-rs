//! PHP `Swoole\Event` (with the `php` feature, see docs/zend.md): class and
//! function registration, the fd registry, each fd's callbacks and the add,
//! set, del, write, isset, defer and cycle bodies. The reactor, its sockets and
//! Swoole's `zend::Callable` stay C++, lent as a table; the loop-driven
//! functions (wait, dispatch, exit, rshutdown) stay C++, and the reactor and
//! defer callbacks are C++ trampolines over the bodies here that re-raise a
//! bailout Rust trapped.
//!
//! A warning can run a user error handler, and converting an fd or a callable
//! can run user code (a stream wrapper, an autoloader); when such code bails
//! out from inside a handler here, the jump crosses only frames that own
//! nothing to drop, so nothing here holds a heap value across those calls.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_char, c_void},
    fmt,
    io::Write,
    ptr,
    sync::OnceLock,
};
use swoole_rs_phpstubs::event;
use swoole_rs_zend::{
    Call, Class,
    bailout::{self, Bailout, trap},
    sys::{IS_NULL, swr_zend_zval_release, swr_zval_try_addref, zval},
};
use swoole_rs_zend::{method_table, sys as z};

/// The reactor, socket and callable operations (`swr_v1_phpevents_ops`).
#[repr(C)]
pub struct Ops {
    /// `event_check_reactor()`: start the reactor and install the user-fd handlers.
    check_reactor: unsafe extern "C" fn(),
    /// `php_swoole_check_reactor()`.
    init_reactor: unsafe extern "C" fn(),
    /// `sw_reactor() != nullptr`.
    has_reactor: unsafe extern "C" fn() -> u8,
    /// `php_swoole_convert_to_fd`.
    fd: unsafe extern "C" fn(zsocket: *mut zval) -> i32,
    /// `sw_callable_create_ex(zfn, name, allow_null)`, or `sw_callable_create`
    /// when `name` is null: a `zend::Callable *` or null.
    callable:
        unsafe extern "C" fn(zfn: *mut zval, name: *const c_char, allow_null: u8) -> *mut c_void,
    /// `sw_callable_free`, a body for `swr_zend_try`.
    callable_free: unsafe extern "C" fn(callable: *mut c_void),
    /// `swoole_event_defer(sw_callable_free, callable)`.
    defer_callable_free: unsafe extern "C" fn(callable: *mut c_void),
    /// `zend::function::call` of an `Invocation`, a body for `swr_zend_try`.
    call: unsafe extern "C" fn(invocation: *mut c_void),
    /// `make_socket(fd, SW_FD_USER)`, non-blocking, carrying `object`.
    socket_make: unsafe extern "C" fn(fd: i32, object: *mut c_void) -> *mut c_void,
    /// `swoole_event_add`.
    socket_add: unsafe extern "C" fn(socket: *mut c_void, events: i64) -> i32,
    /// `swoole_event_write`.
    socket_write:
        unsafe extern "C" fn(socket: *mut c_void, data: *const c_char, length: usize) -> i32,
    /// `swoole_event_set`.
    socket_set: unsafe extern "C" fn(socket: *mut c_void, events: i64) -> i32,
    /// `swoole_event_del`.
    socket_del: unsafe extern "C" fn(socket: *mut c_void) -> i32,
    /// `socket->free()`, after `socket->fd = -1` when `detach_fd`.
    socket_free: unsafe extern "C" fn(socket: *mut c_void, detach_fd: u8),
    /// `socket->object`.
    socket_object: unsafe extern "C" fn(socket: *mut c_void) -> *mut c_void,
    /// `socket->object = nullptr`.
    socket_detach: unsafe extern "C" fn(socket: *mut c_void),
    /// `socket->events`, and `socket->removed`.
    socket_events: unsafe extern "C" fn(socket: *mut c_void, removed: *mut u8) -> i32,
    /// `swoole_event_defer(event_object_free, object)`.
    defer_object: unsafe extern "C" fn(object: *mut c_void),
    /// `swoole_event_defer(event_defer_callback, callable)`.
    defer: unsafe extern "C" fn(callable: *mut c_void),
    /// The reactor's future (`before`) or idle task: its data, and whether
    /// its callback is set.
    cycle_get: unsafe extern "C" fn(before: u8, armed: *mut u8) -> *mut c_void,
    /// Set that task to `event_cycle_callback` over `callable`, or clear it.
    cycle_set: unsafe extern "C" fn(before: u8, callable: *mut c_void, armed: u8),
    /// `php_swoole_fatal_error(E_WARNING, "%s", message)`.
    fatal_warning: unsafe extern "C" fn(message: *const c_char),
    /// `php_swoole_error(E_WARNING, "%s", message)`.
    warning: unsafe extern "C" fn(message: *const c_char),
}

/// One callable call (`swr_v1_phpevents_invocation`).
#[repr(C)]
struct Invocation {
    callable: *mut c_void,
    argv: *mut zval,
    argc: u32,
    ok: u8,
}

/// `EventObject`: the fd's PHP value and its callbacks (`zend::Callable *`).
#[repr(C)]
struct EventObject {
    zsocket: zval,
    readable: *mut c_void,
    writable: *mut c_void,
}

const READ: i64 = 512;
const WRITE: i64 = 1024;

static OPS: OnceLock<&'static Ops> = OnceLock::new();
static EVENT: Class = Class::new();

thread_local! {
    /// `event_socket_map`: fd to `Socket *`.
    static SOCKETS: RefCell<HashMap<i32, usize>> = RefCell::new(HashMap::new());
}

fn ops() -> &'static Ops {
    // Functions run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
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

// The handlers that stay C++.
unsafe extern "C" {
    fn zif_swoole_event_dispatch(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_event_exit(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_event_rshutdown(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_event_wait(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

/// MINIT: `Swoole\Event` (not instantiable) and the `swoole_event_*` functions,
/// as `php_swoole_event_minit` registered them.
/// # Safety
/// Called once, at MINIT, with a static operations table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpevents_minit(ops: *const Ops) {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    // SAFETY: MINIT, with a static table.
    unsafe { EVENT.register_static(c"Swoole\\Event", METHODS.as_ptr()) };
    // SAFETY: MINIT, after registration, with the class's own table.
    unsafe { EVENT.function_aliases(METHODS.as_ptr(), &ALIASES) };
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

    fn as_ptr(&self) -> *const c_char {
        self.0.as_ptr().cast()
    }
}

fn fatal_warning(message: *const c_char) {
    // SAFETY: a NUL-terminated message.
    let message = unsafe { CStr::from_ptr(message) };
    // SAFETY: C++'s reporter.
    bailout::defer(unsafe { bailout::report(ops().fatal_warning, message) });
}

fn fd(zsocket: *mut zval) -> i32 {
    // SAFETY: a live zval.
    unsafe { (ops().fd)(zsocket) }
}

fn socket(fd: i32) -> Option<*mut c_void> {
    SOCKETS.with_borrow(|sockets| sockets.get(&fd).map(|&socket| socket as *mut c_void))
}

fn has_reactor() -> bool {
    // SAFETY: C++'s check.
    unsafe { (ops().has_reactor)() != 0 }
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
    // SAFETY: a live zval or null, as `sw_callable_create_ex` takes.
    unsafe { (ops().callable)(zfn, name, u8::from(allow_null)) }
}

/// `add(mixed $fd, ?callable $read = null, ?callable $write = null, int $events = SWOOLE_EVENT_READ)`.
fn add(call: &Call) {
    let parsed = call.arity(1, 4).and_then(|()| call.long(4, READ));
    let Ok(events) = parsed else {
        call.set_bool(false);
        return;
    };
    let (zfd, zread, zwrite) = (call.zval(1), call.zval(2), call.zval(3));
    let ops = ops();
    // SAFETY: C++'s reactor setup.
    unsafe { (ops.check_reactor)() };
    let fd = fd(zfd);
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
        fatal_warning(message.as_ptr());
        call.set_bool(false);
        return;
    }
    let readable = callable(zread, Some(c"readable_callback"), true);
    if events & READ != 0 && readable.is_null() {
        missing(fd, "readable");
        call.set_bool(false);
        return;
    }
    let writable = callable(zwrite, Some(c"writable_callback"), true);
    if events & WRITE != 0 && writable.is_null() {
        missing(fd, "writable");
        if !readable.is_null() {
            // SAFETY: the callable just made; nothing else holds it.
            bailout::raise(unsafe { trap(ops.callable_free, readable) });
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
    }))
    .cast::<c_void>();
    // SAFETY: a valid fd and the object the socket carries.
    let socket = unsafe { (ops.socket_make)(fd, object) };
    // SAFETY: the socket just made.
    if unsafe { (ops.socket_add)(socket, events) } < 0 {
        fatal_warning(c"swoole_event_add failed".as_ptr());
        // SAFETY: the reactor refused the socket; nothing else holds it.
        unsafe { (ops.socket_free)(socket, 0) };
        // SAFETY: the object made above, which nothing else holds.
        let freed = unsafe { swr_v1_phpevents_free(object) };
        bailout::raise(if freed == 0 { Ok(()) } else { Err(Bailout) });
        call.set_bool(false);
        return;
    }
    SOCKETS.with_borrow_mut(|sockets| sockets.insert(fd, socket as usize));
    call.set_long(fd.into());
}

/// "Swoole\Event: unable to find <kind> callback of fd [<fd>]".
fn missing(fd: i32, kind: &str) {
    let message = Message::new(format_args!(
        "Swoole\\Event: unable to find {kind} callback of fd [{fd}]"
    ));
    fatal_warning(message.as_ptr());
}

fn not_found(fd: i32) {
    let message = Message::new(format_args!("socket[{fd}] is not found in the reactor"));
    fatal_warning(message.as_ptr());
}

/// `write(mixed $fd, string $data): bool`.
fn write(call: &Call) {
    let Ok(data) = call.arity(2, 2).and_then(|()| call.string(2, b"")) else {
        call.set_bool(false);
        return;
    };
    if data.is_empty() {
        fatal_warning(c"data empty".as_ptr());
        call.set_bool(false);
        return;
    }
    let fd = fd(call.zval(1));
    if fd < 0 {
        fatal_warning(c"unknown type".as_ptr());
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        not_found(fd);
        call.set_bool(false);
        return;
    };
    let ops = ops();
    // SAFETY: C++'s reactor setup.
    unsafe { (ops.check_reactor)() };
    // SAFETY: a registered socket and the argument's bytes.
    let written = unsafe { (ops.socket_write)(socket, data.as_ptr().cast(), data.len()) };
    call.set_bool(written >= 0);
}

/// `set(mixed $fd, ?callable $read = null, ?callable $write = null, int $events = 0)`.
fn set(call: &Call) {
    if !has_reactor() {
        fatal_warning(c"reactor is not ready, cannot call swoole_event_set".as_ptr());
        call.set_bool(false);
        return;
    }
    let Ok(events) = call.arity(1, 4).and_then(|()| call.long(4, 0)) else {
        call.set_bool(false);
        return;
    };
    let fd = fd(call.zval(1));
    if fd < 0 {
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        not_found(fd);
        call.set_bool(false);
        return;
    };
    let ops = ops();
    // SAFETY: a registered socket carries its EventObject.
    let object = unsafe { (ops.socket_object)(socket) }.cast::<EventObject>();
    let readable = callable(call.zval(2), Some(c"readable_callback"), true);
    let writable = callable(call.zval(3), Some(c"writable_callback"), true);
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
            // SAFETY: the replaced callable; the reactor may still be in it.
            unsafe { (ops.defer_callable_free)(*slot) };
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
    if unsafe { (ops.socket_set)(socket, events) } < 0 {
        fatal_warning(c"Swoole\\Event::set failed".as_ptr());
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `del(mixed $fd): bool`.
fn del(call: &Call) {
    if !has_reactor() {
        fatal_warning(c"reactor is not ready, cannot call swoole_event_del".as_ptr());
        call.set_bool(false);
        return;
    }
    if call.arity(1, 1).is_err() {
        call.set_bool(false);
        return;
    }
    let fd = fd(call.zval(1));
    if fd < 0 {
        fatal_warning(c"unknown type".as_ptr());
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        call.set_bool(false);
        return;
    };
    let ops = ops();
    // SAFETY: a registered socket.
    let object = unsafe { (ops.socket_object)(socket) };
    // SAFETY: its object, freed once the loop is out of it.
    unsafe { (ops.defer_object)(object) };
    // SAFETY: a registered socket.
    let deleted = unsafe { (ops.socket_del)(socket) };
    SOCKETS.with_borrow_mut(|sockets| sockets.remove(&fd));
    // SAFETY: removed from the reactor and the registry; the fd stays the user's.
    unsafe { (ops.socket_free)(socket, 1) };
    call.set_bool(deleted == 0);
}

/// `defer(callable $callback): true`.
fn defer(call: &Call) {
    if call.arity(1, 1).is_err() {
        call.set_bool(false);
        return;
    }
    let ops = ops();
    // SAFETY: C++'s reactor setup.
    unsafe { (ops.init_reactor)() };
    let callback = callable(call.zval(1), None, false);
    // SAFETY: the reactor runs and then frees it.
    unsafe { (ops.defer)(callback) };
    call.set_bool(true);
}

/// `cycle(?callable $callback, bool $before = false): bool`.
fn cycle(call: &Call) {
    let Ok(before) = call.arity(1, 2).and_then(|()| call.bool(2, false)) else {
        call.set_bool(false);
        return;
    };
    let zcallback = call.zval(1);
    let ops = ops();
    // SAFETY: C++'s reactor setup.
    unsafe { (ops.check_reactor)() };
    let before = u8::from(before);
    let mut armed = 0;
    // SAFETY: the reactor exists now; a writable flag.
    let data = unsafe { (ops.cycle_get)(before, &raw mut armed) };
    if is_null(zcallback) {
        if armed == 0 {
            call.set_bool(false);
            return;
        }
        // SAFETY: the task's callable, released after the loop is out of it.
        unsafe { (ops.defer_callable_free)(data) };
        // SAFETY: the reactor exists.
        unsafe { (ops.cycle_set)(before, ptr::null_mut(), 0) };
    } else {
        if !data.is_null() {
            // SAFETY: as above.
            unsafe { (ops.defer_callable_free)(data) };
        }
        let callback = callable(zcallback, None, false);
        // SAFETY: the reactor exists; it holds the callable from now on.
        unsafe { (ops.cycle_set)(before, callback, 1) };
    }
    call.set_bool(true);
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
    if fd < 0 {
        fatal_warning(c"unknown type".as_ptr());
        call.set_bool(false);
        return;
    }
    let Some(socket) = socket(fd) else {
        call.set_bool(false);
        return;
    };
    let mut removed = 0;
    // SAFETY: a registered socket; a writable flag.
    let current = unsafe { (ops().socket_events)(socket, &raw mut removed) };
    call.set_bool(removed == 0 && i64::from(current) & events != 0);
}

/// Call `callable` with `argv`: whether it succeeded.
fn invoke(callable: *mut c_void, argv: *mut zval, argc: u32) -> Result<bool, Bailout> {
    let mut invocation = Invocation {
        callable,
        argv,
        argc,
        ok: 0,
    };
    // SAFETY: the C++ call body, with an invocation it may write.
    unsafe { trap(ops().call, (&raw mut invocation).cast()) }?;
    Ok(invocation.ok != 0)
}

/// The reactor's read (`write` 0) or write handler of a user fd: run its
/// callback; on failure warn, detach and delete the fd.
/// # Safety
/// `socket` is a user socket `add` registered, carrying its object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpevents_ready(socket: *mut c_void, write: u8) -> i32 {
    let ops = ops();
    // SAFETY: per the caller.
    let object = unsafe { (ops.socket_object)(socket) };
    // SAFETY: as above, the live object.
    let peo = unsafe { &*object.cast::<EventObject>() };
    let callable = if write == 0 {
        peo.readable
    } else {
        peo.writable
    };
    // `argv[0] = peo->zsocket`: a borrowed copy.
    let mut argv = peo.zsocket;
    match invoke(callable, &raw mut argv, 1) {
        Err(_) => 1,
        Ok(true) => 0,
        Ok(false) => {
            let mut zsocket = peo.zsocket;
            let fd = fd(&raw mut zsocket);
            let kind = if write == 0 { "readable" } else { "writable" };
            let message = Message::new(format_args!(
                "Swoole\\Event: {kind} callback handler error, fd [{fd}] will be removed from reactor"
            ));
            fatal_warning(message.as_ptr());
            // SAFETY: per the caller.
            unsafe { (ops.socket_detach)(socket) };
            // SAFETY: the detached object, freed once the loop is out of it.
            unsafe { (ops.defer_object)(object) };
            // SAFETY: per the caller.
            unsafe { (ops.socket_del)(socket) };
            bailout::finish();
            -1
        }
    }
}

/// `event_object_free`: drop an fd's callbacks and PHP value; 1 if a
/// destructor bailed out.
/// # Safety
/// `object` is an `EventObject` from `add`, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpevents_free(object: *mut c_void) -> u8 {
    // SAFETY: per the caller.
    let mut peo = unsafe { Box::from_raw(object.cast::<EventObject>()) };
    let ops = ops();
    let mut bailed = false;
    for callable in [peo.readable, peo.writable] {
        if !callable.is_null() {
            // SAFETY: a callable the object owned.
            bailed |= unsafe { trap(ops.callable_free, callable) }.is_err();
        }
    }
    // SAFETY: the object's reference to its PHP value.
    bailed |= unsafe { trap(swr_zend_zval_release, (&raw mut peo.zsocket).cast()) }.is_err();
    u8::from(bailed)
}

/// `event_defer_callback`: run and free a deferred callable; 1 if it bailed out.
/// # Safety
/// `callable` is the `zend::Callable *` `defer` scheduled, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpevents_deferred(callable: *mut c_void) -> u8 {
    let ops = ops();
    match invoke(callable, ptr::null_mut(), 0) {
        Err(_) => 1,
        Ok(ok) => {
            let warned = if ok {
                Ok(())
            } else {
                // SAFETY: C++'s reporter.
                unsafe {
                    bailout::report(ops.warning, c"Swoole\\Event::defer callback handler error")
                }
            };
            // SAFETY: per the caller.
            let freed = unsafe { trap(ops.callable_free, callable) };
            u8::from(warned.is_err() || freed.is_err())
        }
    }
}

/// `event_cycle_callback`: run the cycle callable; 1 if it bailed out.
/// # Safety
/// `callable` is the `zend::Callable *` `cycle` set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpevents_cycle(callable: *mut c_void) -> u8 {
    match invoke(callable, ptr::null_mut(), 0) {
        Err(_) => 1,
        Ok(true) => 0,
        // SAFETY: C++'s reporter.
        Ok(false) => u8::from(
            unsafe { bailout::report(ops().warning, c"Swoole\\Event::end callback handler error") }
                .is_err(),
        ),
    }
}

swoole_rs_zend::handlers! {
    zif_swoole_event_add => add;
    zif_swoole_event_set => set;
    zif_swoole_event_del => del;
    zif_swoole_event_write => write;
    zif_swoole_event_defer => defer;
    zif_swoole_event_cycle => cycle;
    zif_swoole_event_isset => isset;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_cut_and_terminated() {
        let message = Message::new(format_args!("socket[{}] is not found in the reactor", 7));
        // SAFETY: `Message` always ends in NUL.
        let text = unsafe { CStr::from_ptr(message.as_ptr()) };
        assert_eq!(text, c"socket[7] is not found in the reactor", "formatted");
        let long = Message::new(format_args!("{}", "x".repeat(400)));
        // SAFETY: as above.
        let text = unsafe { CStr::from_ptr(long.as_ptr()) };
        assert_eq!(text.count_bytes(), 159, "cut to the buffer");
    }
}
