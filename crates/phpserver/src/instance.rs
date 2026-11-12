//! What the Server's other bindings ask of its PHP object: the running server's
//! object zval, a Server object's `Server`, its ports' callbacks and a
//! request's payload (were ext-src/swoole_server.cc's `php_swoole_server_*`),
//! for crates/phphttp, crates/phphttp2 and crates/phpredis too.
use crate::RecvData;
use crate::callable::Callable;
use crate::lifecycle::{ServerData, ServerObject};
use crate::port::PortProperty;
use std::os::unix::ffi::OsStrExt;
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_int, c_long},
    ptr,
    rc::Rc,
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{
    constants::EventDataFlag::{SW_EVENT_DATA_OBJ_PTR, SW_EVENT_DATA_POP_PTR},
    log,
};
use swoole_rs_process::{methods::worker_is_running, worker_globals, worker_record};
use swoole_rs_server::{SharedServer, event_worker_ops, server_methods, view};
use swoole_rs_zend::host::callable::Callable as Kept;
use swoole_rs_zend::{
    Owned, Value, bailout, string,
    sys::{swr_zend_last_fatal_error, swr_zend_mkdir_recursive, swr_zval_stringl, zval},
    value::{empty_string, set_str},
};

thread_local! {
    /// Upstream's `static SW_THREAD_LOCAL zval swoole_server_instance`: the running
    /// server's object, named without a reference as upstream's zval was, with the
    /// mark [`set_server_object`] gave it. Only the thread that runs PHP reads it.
    /// Undefined until bound, and again once the object is being freed.
    static INSTANCE: Cell<(zval, u64)> = const { Cell::new((swoole_rs_zend::UNDEF, 0)) };
}

/// `php_swoole_server_zval_ptr()`: the running server's object, with a reference
/// of its own; none before one is bound and once it is being freed.
#[must_use]
pub(crate) fn server_object() -> Option<Owned> {
    let mut object = server_zval();
    let object = Value::of(&mut object);
    object.object().map(|_| object.copy())
}

/// `*php_swoole_server_zval_ptr()` as upstream copies it: the running server's
/// object without a reference of its own (undefined for none), for an event's
/// arguments.
pub(crate) fn server_zval() -> zval {
    INSTANCE.get().0
}

/// `*php_swoole_server_zval_ptr() = *zserv`: no reference is taken.
pub(crate) fn set_server_object(object: ServerObject<'_>) {
    let mark = INSTANCE.get().1 + 1;
    object.set_mark(mark);
    // SAFETY: a live zval. The copy takes no reference: `forget` drops it as
    // the object's data is dropped, before Zend frees the object.
    let copy = unsafe { object.value().as_ptr().read() };
    INSTANCE.set((copy, mark));
}

/// The object marked `mark` is being freed: it is no longer the running one.
pub(crate) fn forget(mark: u64) {
    let (_, running) = INSTANCE.get();
    if running == mark {
        INSTANCE.set((swoole_rs_zend::UNDEF, running));
    }
}

/// `php_swoole_server_get_ptr`: the `Server` of a Server object, which `value`
/// keeps live; none before `__construct`, and for any other value.
#[must_use]
pub fn php_swoole_server_of(value: &Value) -> Option<view::Server<'_>> {
    ServerObject::server_of(value)
}

/// `php_swoole_server_get_port_property`: the port's `ServerPortProperty`, or null.
fn port_property<'a>(port: Option<view::Port<'_>>) -> Option<&'a PortProperty> {
    // SAFETY: a live `ListenPort`, whose property link only this thread touches.
    let config = unsafe { port?.config_mut() };
    let property =
        crate::server::port_property(config.object_id, &mut config.ptr, ptr::null_mut(), false);
    // SAFETY: null, or the property its Port object holds while the port lives.
    unsafe { property.cast::<PortProperty>().as_ref() }
}

/// The port's callback `kind`, or none.
fn port_callback(port: Option<view::Port<'_>>, kind: c_int) -> Option<Callable> {
    port_property(port)?.callback(kind)
}

/// `php_swoole_server_isset_callback`: the port, or the primary port, has callback `kind`.
#[must_use]
pub fn php_swoole_server_isset_callback(
    serv: view::Server<'_>,
    port: Option<view::Port<'_>>,
    kind: c_int,
) -> bool {
    let primary = serv.port_at(0);
    port_callback(port, kind).is_some() || port_callback(primary, kind).is_some()
}

/// `php_swoole_server_get_callback`: callback `kind` of the port listening on
/// `server_fd`, else the primary port's; none without that port. The caller
/// holds the reference through its call, so an `on()` during it drops only the
/// port's.
#[must_use]
pub fn php_swoole_server_get_callback(
    serv: view::Server<'_>,
    server_fd: c_int,
    kind: c_int,
) -> Option<Rc<Kept>> {
    let port = serv.port_by_server_fd(server_fd)?;
    port_callback(Some(port), kind).or_else(|| port_callback(serv.port_at(0), kind))
}

/// `serv->get_worker_message_bus()->move_packet()`.
fn move_packet(serv: view::Server<'_>) {
    // SAFETY: the worker's live bus; the packet is now the caller's.
    unsafe { swoole_rs_server::bus_move_packet(crate::dispatch::message_bus(serv)) };
}

/// `zend::assign_zend_string_by_val`: the zend string whose bytes start at
/// `data`, cut to `length`, into `zdata`.
/// # Safety
/// `data` is `ZSTR_VAL` of a live, unshared string of at least `length` bytes;
/// `zdata` is writable.
unsafe fn assign_by_val(zdata: *mut zval, data: *mut u8, length: usize) {
    let text = string::from_val(data);
    // SAFETY: per the caller.
    unsafe { string::truncate(text, length) };
    // SAFETY: per the caller; `zdata` takes the reference.
    unsafe { set_str(zdata, text) };
}

/// `php_swoole_get_recv_data`: a request's payload into `zdata`, taking the
/// message bus's packet or the connection's receive buffer when it holds them.
/// What `zdata` held is overwritten, not released.
pub fn php_swoole_get_recv_data(serv: view::Server<'_>, zdata: &Value, req: &RecvData) {
    let zdata = zdata.as_ptr();
    let (flags, length) = (req.info.flags, req.info.len as usize);
    if length == 0 {
        // SAFETY: a live zval; the interned string needs no reference.
        unsafe { set_str(zdata, empty_string()) };
    } else if flags & SW_EVENT_DATA_OBJ_PTR as u8 != 0 {
        // SAFETY: an OBJ_PTR payload is the bus's packet, a zend string.
        unsafe { assign_by_val(zdata, req.data.cast_mut().cast(), length) };
        move_packet(serv);
    } else if let Some(buffer) = (flags & SW_EVENT_DATA_POP_PTR as u8 != 0)
        .then(|| serv.by_session(req.info.fd))
        .flatten()
        .map(|conn| view::recv_buffer(conn))
        .filter(|buffer| !buffer.is_null())
    {
        let size = serv.config().recv_buffer_size;
        // SAFETY: the connection's receive buffer, which holds the payload; nothing else borrows it.
        let data = unsafe { &mut *buffer.cast::<CppString>() }.pop(size);
        // SAFETY: `pop` hands over its zend-allocated block of the payload.
        unsafe { assign_by_val(zdata, data, length) };
    } else {
        // SAFETY: `length` bytes at `data`.
        unsafe { swr_zval_stringl(zdata, req.data.cast(), length) };
    }
}

/// `php_swoole_create_dir`: whether `path`, up to its first NUL as C reads it,
/// exists or was made with its parents.
#[must_use]
pub fn php_swoole_create_dir(path: &[u8]) -> bool {
    let path = path.split(|&b| b == 0).next().unwrap_or_default();
    // `access(path, F_OK) == 0`: it resolves, symbolic links followed.
    let exists = std::path::Path::new(std::ffi::OsStr::from_bytes(path)).exists();
    let path = CString::new(path).unwrap_or_default();
    // SAFETY: a NUL-terminated path.
    exists || unsafe { swr_zend_mkdir_recursive(path.as_ptr()) }
}

/// `php_swoole_server_check_kernel_nobufs`: in a coroutine, pause 10 ms after a
/// send the kernel refused for lack of buffers.
fn check_kernel_nobufs(serv: view::Server<'_>, session_id: c_long) {
    if swoole_rs_phpcoroutines::in_coroutine()
        && server_methods::has_kernel_nobufs_error(serv, session_id)
    {
        _ = swoole_rs_hooks::sleep::system_sleep(0.01);
    }
}

/// `serv->send(fd, data)`, then the `send_yield` wait or the kernel-nobufs pause.
pub(crate) fn send(serv: view::Server<'_>, fd: c_long, bytes: &[u8]) -> bool {
    let sent = server_methods::send(serv, fd, bytes);
    if !sent
        && swoole_rs_corebase::log::last_error() == crate::lifecycle::SW_ERROR_OUTPUT_SEND_YIELD
    {
        return crate::lifecycle::php_swoole_server_send_yield(serv, fd, bytes).unwrap_or_else(
            |bailed| {
                bailout::raise(Err(bailed));
                false
            },
        );
    }
    check_kernel_nobufs(serv, fd);
    sent
}

/// `SW_USER_WORKER`, `SW_LOG_NOTICE`, `SW_LOG_ERROR` and `SW_ERROR_SERVER_WORKER_TERMINATED`.
const USER_WORKER: c_int = 5;
const LOG_NOTICE: c_int = 3;
const LOG_ERROR: c_int = 5;
const SW_ERROR_SERVER_WORKER_TERMINATED: c_int = 9003;

/// `php_swoole_server_rshutdown`: a worker whose request ended (`exit()`,
/// `die()` or a fatal error) drains its pipe, shuts down and logs why.
pub extern "C" fn php_swoole_server_rshutdown() {
    let worker = worker_globals::current_worker();
    // A worker's server, which its object keeps past the request's end.
    let Some(share) = SharedServer::current().filter(|_| !worker.is_null()) else {
        return;
    };
    let serv = share.view();
    event_worker_ops::drain_worker_pipe(serv);
    let kind = crate::server::worker_type();
    if !serv.is_started() || !worker_is_running() || kind == USER_WORKER {
        return;
    }
    // SAFETY: this thread's current worker.
    let worker = unsafe { worker_record::Worker::new(worker) };
    worker_record::swr_v1_worker_shutdown(worker);
    if kind == crate::server::EVENT_WORKER {
        event_worker_ops::clean_worker_connections(serv, worker);
    }
    let (mut message, mut file, mut line) = (ptr::null(), ptr::null(), 0);
    // SAFETY: writable outputs; PHP's globals during RSHUTDOWN.
    if unsafe { swr_zend_last_fatal_error(&raw mut message, &raw mut file, &raw mut line) } {
        // SAFETY: a NUL-terminated string PHP holds until the next error.
        let message = unsafe { CStr::from_ptr(message) };
        // SAFETY: as above.
        let file = unsafe { CStr::from_ptr(file) };
        log::error_log(
            LOG_ERROR,
            crate::server::SW_ERROR_PHP_FATAL_ERROR,
            c"php_swoole_server_rshutdown",
            format_args!(
                "Fatal error: {} in {} on line {line}",
                message.to_string_lossy(),
                file.to_string_lossy()
            ),
        );
    } else {
        log::error_log(
            LOG_NOTICE,
            SW_ERROR_SERVER_WORKER_TERMINATED,
            c"php_swoole_server_rshutdown",
            format_args!("worker process is terminated by exit()/die()"),
        );
    }
}
