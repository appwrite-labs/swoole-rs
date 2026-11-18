//! What the Server's other bindings ask of its PHP object: the running server's
//! object zval, a Server object's `Server`, its ports' callbacks and a
//! request's payload (were ext-src/swoole_server.cc's `php_swoole_server_*`),
//! for crates/phphttp, crates/phphttp2 and crates/phpredis too.
use crate::RecvData;
use crate::lifecycle::{ServerData, ServerObject};
use std::os::unix::ffi::OsStrExt;
use std::{
    ffi::{CString, c_int, c_long},
    rc::Rc,
    sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed},
};
use swoole_rs_corebase::log::LogLevel::{SW_LOG_ERROR as LOG_ERROR, SW_LOG_NOTICE as LOG_NOTICE};
use swoole_rs_corebase::process::{methods::worker_is_running, worker_globals, worker_record};
use swoole_rs_corebase::{constants::EventDataFlag, log};
pub use swoole_rs_ports::PortCallback;
use swoole_rs_server::{SharedServer, event_worker, server_methods, view};
use swoole_rs_zend::host::callable::Callable as Kept;
use swoole_rs_zend::{Owned, Value, Weak, ZString};

/// Upstream's `static SW_THREAD_LOCAL zval swoole_server_instance`: the running
/// server's handle (0 for none: Zend's handles start at 1) and the mark
/// [`set_server_object`] gave it. Empty before binding and while being freed.
struct Instance {
    handle: AtomicU32,
    mark: AtomicU64,
}

// Per thread on a thread-safe PHP and in tests, one process value otherwise,
// as `SW_THREAD_LOCAL` is: a non-thread-safe build reads it on every event
// callback without a TLS lookup.
#[cfg(any(zts, panic = "unwind"))]
thread_local! {
    static INSTANCE: Instance = const { Instance::new() };
}
#[cfg(not(any(zts, panic = "unwind")))]
static INSTANCE: Instance = Instance::new();

impl Instance {
    const fn new() -> Self {
        Self {
            handle: AtomicU32::new(0),
            mark: AtomicU64::new(0),
        }
    }

    #[inline]
    fn with<R>(f: impl FnOnce(&Self) -> R) -> R {
        #[cfg(any(zts, panic = "unwind"))]
        return INSTANCE.with(f);
        #[cfg(not(any(zts, panic = "unwind")))]
        f(&INSTANCE)
    }
}

/// `php_swoole_server_zval_ptr()`: the running server's object, with a reference
/// of its own; none before one is bound and once it is being freed.
#[must_use]
#[inline]
pub(crate) fn server_object() -> Option<Owned> {
    let handle = Instance::with(|instance| instance.handle.load(Relaxed));
    // Each lookup checks the executor's live object store.
    Weak::<ServerData>::at(handle).get()
}

/// `*php_swoole_server_zval_ptr() = *zserv`: no reference is taken.
pub(crate) fn set_server_object(object: ServerObject<'_>) {
    // The weak handle takes no reference. `forget` clears it before Zend
    // reuses the handle.
    let handle = Weak::<ServerData>::of(object.value()).map_or(0, Weak::handle);
    let mark = Instance::with(|instance| {
        instance.handle.store(handle, Relaxed);
        instance.mark.fetch_add(1, Relaxed) + 1
    });
    object.set_mark(mark);
}

/// The object marked `mark` is being freed: it is no longer the running one.
pub(crate) fn forget(mark: u64) {
    Instance::with(|instance| {
        if instance.mark.load(Relaxed) == mark {
            instance.handle.store(0, Relaxed);
        }
    });
}

/// `php_swoole_server_get_ptr`: the `Server` of a Server object, which `value`
/// keeps live; none before `__construct`, and for any other value.
#[must_use]
pub fn php_swoole_server_of(value: &Value) -> Option<view::Server<'_>> {
    ServerObject::server_of(value)
}

/// `php_swoole_server_isset_callback`: the port, or the primary port, has callback `kind`.
#[must_use]
#[inline]
pub fn php_swoole_server_isset_callback(
    serv: view::Server<'_>,
    port: Option<view::Port<'_>>,
    kind: PortCallback,
) -> bool {
    let has = |port| crate::port::has_callback(port, kind);
    port.is_some_and(has) || serv.port_at(0).is_some_and(has)
}

/// `php_swoole_server_get_callback`: callback `kind` of the port listening on
/// `server_fd`, else the primary port's; none without that port. The caller
/// holds the reference through its call, so an `on()` during it drops only the
/// port's.
#[must_use]
#[inline(always)]
pub fn php_swoole_server_get_callback(
    serv: view::Server<'_>,
    server_fd: c_int,
    kind: PortCallback,
) -> Option<Rc<Kept>> {
    let port = serv.port_by_server_fd(server_fd)?;
    if let Some(callback) = crate::port::callback(port, kind) {
        return Some(callback);
    }
    crate::port::callback(serv.port_at(0)?, kind)
}

/// `php_swoole_get_recv_data`: a request's payload, taking the message bus's
/// packet or the connection's receive buffer when it holds them.
#[must_use]
pub fn php_swoole_get_recv_data(_serv: view::Server<'_>, req: &mut RecvData<'_>) -> Owned {
    let (flags, length) = (
        EventDataFlag::from_bits(req.info().flags),
        req.info().len as usize,
    );
    if length == 0 {
        return Owned::string(ZString::empty());
    }
    let buffer = if flags.contains(EventDataFlag::SW_EVENT_DATA_OBJ_PTR)
        || flags.contains(EventDataFlag::SW_EVENT_DATA_POP_PTR)
    {
        req.take_packet()
    } else {
        None
    };
    let Some(mut buffer) = buffer else {
        return Owned::string(ZString::new(req.payload()));
    };
    buffer.truncate(length);
    Owned::string(swoole_rs_phpswoole::received_string(buffer))
}

/// `php_swoole_create_dir`: whether `path`, up to its first NUL as C reads it,
/// exists or was made with its parents.
#[must_use]
pub fn php_swoole_create_dir(path: &[u8]) -> bool {
    let path = path.split(|&b| b == 0).next().unwrap_or_default();
    // `access(path, F_OK) == 0`: it resolves, symbolic links followed.
    let exists = std::path::Path::new(std::ffi::OsStr::from_bytes(path)).exists();
    let path = CString::new(path).unwrap_or_default();
    exists || swoole_rs_zend::mkdir_recursive(&path)
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
                bailed.defer();
                false
            },
        );
    }
    check_kernel_nobufs(serv, fd);
    sent
}

/// `SW_LOG_NOTICE`, `SW_LOG_ERROR` and `SW_ERROR_SERVER_WORKER_TERMINATED`.
const SW_ERROR_SERVER_WORKER_TERMINATED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SERVER_WORKER_TERMINATED as c_int;

/// `php_swoole_server_rshutdown`: a worker whose request ended (`exit()`,
/// `die()` or a fatal error) drains its pipe, shuts down and logs why.
pub extern "C" fn php_swoole_server_rshutdown() {
    // A worker's server, which its object keeps past the request's end.
    let Some(share) =
        SharedServer::current().filter(|_| !worker_globals::current_worker().is_null())
    else {
        return;
    };
    let serv = share.view();
    event_worker::native::drain_worker_pipe(serv);
    let kind = crate::server::worker_type();
    if !serv.is_started() || !worker_is_running() || kind == Some(crate::server::WorkerType::User) {
        return;
    }
    worker_globals::with_current(|worker| {
        worker_record::swr_v1_worker_shutdown(*worker);
        if kind == Some(crate::server::WorkerType::Event) {
            event_worker::native::clean_worker_connections(serv, *worker);
        }
    });
    if let Some(error) = swoole_rs_zend::fatal_error() {
        let (message, file, line) = (error.message, error.file, error.line);
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
