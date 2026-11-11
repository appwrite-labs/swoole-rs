//! What the Server's other bindings ask of its PHP object: the running server's
//! object zval, a Server object's `Server`, its ports' callbacks and a
//! request's payload (were ext-src/swoole_server.cc's `php_swoole_server_*`),
//! for crates/phphttp, crates/phphttp2 and crates/phpredis too.
use crate::RecvData;
use crate::port::PortProperty;
use crate::server::{ServerData, THREAD};
use std::os::unix::ffi::OsStrExt;
use std::{
    cell::UnsafeCell,
    ffi::{CStr, CString, c_int, c_long, c_void},
    mem::MaybeUninit,
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{
    constants::EventDataFlag::{SW_EVENT_DATA_OBJ_PTR, SW_EVENT_DATA_POP_PTR},
    log,
};
use swoole_rs_process::{
    methods::worker_is_running, worker_globals, worker_record, workers::entry,
};
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::{
    Object, bailout, object_of, string,
    sys::{swr_zend_last_fatal_error, swr_zend_mkdir_recursive, swr_zval_stringl, zval},
    value::{empty_string, set_str},
};

/// Upstream's `static SW_THREAD_LOCAL zval swoole_server_instance`: one per
/// process, or one per thread under `SW_THREAD`.
struct Instance(UnsafeCell<MaybeUninit<zval>>);

// SAFETY: written only by the thread that constructs the Server and read by the
// PHP threads after it, as upstream's global was.
unsafe impl Sync for Instance {}

static INSTANCE: Instance = Instance(UnsafeCell::new(MaybeUninit::zeroed()));

thread_local! {
    static THREAD_INSTANCE: Instance = const { Instance(UnsafeCell::new(MaybeUninit::zeroed())) };
}

/// `php_swoole_server_zval_ptr`: the running server's object zval (`IS_UNDEF` until bound).
#[must_use]
pub fn php_swoole_server_zval_ptr() -> *mut zval {
    if THREAD.load(Ordering::Relaxed) {
        THREAD_INSTANCE.with(|slot| slot.0.get().cast())
    } else {
        INSTANCE.0.get().cast()
    }
}

/// `php_swoole_server_get_ptr`: the `Server` of a Server object, or null.
/// # Safety
/// `zobject` is a live Server object zval.
pub unsafe fn php_swoole_server_get_ptr(zobject: *mut zval) -> *mut c_void {
    // SAFETY: per the caller.
    let object = unsafe { object_of(zobject) };
    // SAFETY: C++ and Rust allocate Servers as `Object<ServerData>`.
    unsafe { Object::<ServerData>::data(object) }.serv
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

/// The port's callback `kind`, or null.
fn port_callback(port: Option<view::Port<'_>>, kind: c_int) -> *mut c_void {
    port_property(port).map_or(ptr::null_mut(), |property| property.callback(kind))
}

/// `php_swoole_server_isset_callback`: the port, or the primary port, has callback `kind`.
#[must_use]
pub fn php_swoole_server_isset_callback(
    serv: view::Server<'_>,
    port: Option<view::Port<'_>>,
    kind: c_int,
) -> bool {
    let primary = serv.port_at(0);
    !port_callback(port, kind).is_null() || !port_callback(primary, kind).is_null()
}

/// `php_swoole_server_get_callback`: callback `kind` of the port listening on
/// `server_fd`, else the primary port's; null without that port.
#[must_use]
pub fn php_swoole_server_get_callback(
    serv: view::Server<'_>,
    server_fd: c_int,
    kind: c_int,
) -> *mut c_void {
    let Some(port) = serv.port_by_server_fd(server_fd) else {
        return ptr::null_mut();
    };
    let callback = port_callback(Some(port), kind);
    if !callback.is_null() {
        return callback;
    }
    port_callback(serv.port_at(0), kind)
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
pub fn php_swoole_get_recv_data(serv: view::Server<'_>, zdata: &mut zval, req: &RecvData) {
    let (flags, length) = (req.info.flags, req.info.len as usize);
    if length == 0 {
        // SAFETY: per the caller.
        unsafe { set_str(zdata, empty_string()) };
    } else if flags & SW_EVENT_DATA_OBJ_PTR as u8 != 0 {
        // SAFETY: an OBJ_PTR payload is the bus's packet, a zend string.
        unsafe { assign_by_val(zdata, req.data.cast_mut().cast(), length) };
        move_packet(serv);
    } else if let Some(buffer) = (flags & SW_EVENT_DATA_POP_PTR as u8 != 0)
        .then(|| serv.by_session(req.info.fd))
        .flatten()
        // SAFETY: the connection's live socket.
        .map(|conn| unsafe { view::recv_buffer(conn) })
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
        // SAFETY: in a coroutine.
        _ = unsafe { swoole_rs_hooks::sleep::system_sleep(0.01) };
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
    let serv = swoole_rs_corebase::swr_v1_core_server();
    let globals = worker_globals::thread_globals();
    // SAFETY: this thread's worker globals.
    let worker = unsafe { (*globals).worker };
    if serv.is_null() || worker.is_null() {
        return;
    }
    // SAFETY: the running server.
    unsafe { entry::drain_worker_pipe(serv) };
    let kind = crate::server::worker_type();
    // SAFETY: `serv` is the live server here.
    let started = unsafe { view::Server::from_raw(serv) }.is_started();
    if !started || !worker_is_running() || kind == USER_WORKER {
        return;
    }
    // SAFETY: this thread's current worker.
    worker_record::swr_v1_worker_shutdown(unsafe { worker_record::Worker::new(worker) });
    if kind == crate::server::EVENT_WORKER {
        // SAFETY: the running server and its current worker.
        unsafe { entry::clean_worker_connections(serv, worker.cast()) };
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
