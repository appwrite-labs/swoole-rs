//! The `ProcessPool` and `Worker` methods (once `src/os/process_pool.cc`) that are
//! whole Rust operations. Each receives its [`PoolNative`] effects from the
//! caller: crates/server's implementation in production, a supplied host in tests.

use crate::CCast;
use crate::process::process_pool::Protocol;
use crate::process::task_file::Packed;
use crate::process::worker_record::{Worker, WorkerType};
use std::ffi::CStr;
use std::ffi::{c_int, c_void};
use std::ptr;

use crate::process::pool::Bailed;
use crate::process::pool::{
    defer_bailout, swr_v1_process_pool_ipc_respond, swr_v1_process_worker_send,
};
use crate::process::pool::{swr_v1_process_pool_ipc_address, swr_v1_process_pool_ipc_set_listener};
use crate::process::process_pool::{
    PoolNative, PoolRecord, PoolRef, swr_v1_process_pool_create,
    swr_v1_process_pool_dispatch_event, swr_v1_process_pool_push, swr_v1_process_pool_push_event,
    swr_v1_worker_symbol, with,
};
use crate::process::reload_task::KillWarning;
use crate::process::worker_globals::{with_globals, worker_global_max_requests};
use crate::process::worker_record::WorkerRecord;
use std::sync::atomic::Ordering::Relaxed;

/// A method's result, leaving any callback bailout pending for its caller.
pub fn deferred<T: Default>(result: Result<T, Bailed>) -> T {
    result.unwrap_or_else(|Bailed| {
        defer_bailout();
        T::default()
    })
}

/// Each `ProcessPool` method that runs on the pool alone: its name, parameters,
/// return type, and the call, [`deferred`] for one whose callbacks may bail out.
macro_rules! pool_methods {
    ($(fn $name:ident($($p:ident: $t:ty),*) -> $r:ty = $($deferred:ident)? |$pool:ident| $call:expr;)*) => {$(
        #[doc = concat!("`ProcessPool::", stringify!($name), "`.")]
        pub fn $name(native: &dyn PoolNative, pool: PoolRef<'_>, $($p: $t),*) -> $r {
            $($deferred)?(with(native, pool, |$pool| $call))
        }
    )*};
}

pool_methods! {
    fn create_message_box(size: usize) -> bool = |p| p.create_message_box(size as u64);
    fn create_message_bus() -> bool = |p| p.create_message_bus();
    fn set_protocol(protocol: Protocol) -> () = |p| p.set_protocol(protocol);
    fn start_check() -> bool = |p| p.start_check();
    fn start() -> bool = deferred |p| p.start();
    fn drop_reload_task() -> () = |p| p.drop_reload_task();
    fn reload() -> bool = deferred |p| p.reload();
    fn reopen_logger() -> () = |p| p.reopen_logger();
    fn kill_all_workers(signo: c_int) -> () = |p| p.kill_all_workers(signo);
    fn shutdown() -> bool = |p| p.shutdown();
    fn set_max_request(max: u32, grace: u32) -> () = |p| p.set_max_requests(max, grace);
    fn set_type(kind: WorkerType) -> () = |p| p.set_type(kind);
    fn set_start_id(id: c_int) -> () = |p| p.number_from(id.c_cast::<u16>());
    fn detach() -> bool = |p| p.detach();
    fn wait() -> () = deferred |p| p.wait();
    fn destroy() -> () = |p| p.destroy();
    fn free_message_box() -> () = |p| p.free_message_box();
}

/// `ProcessPool::push_message(event)`: the `EventData` with `head` (its `len` set to
/// `data`'s) and `data`; false when the pool has no message box, `data` does not
/// fit, or the push fails.
#[must_use]
pub fn push_event(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    head: &crate::DataHead,
    data: &[u8],
) -> bool {
    swr_v1_process_pool_push_event(native, pool, head, data)
}

/// `ProcessPool::stop(worker)`; nothing when `worker` is none of the pool's.
pub fn stop(native: &dyn PoolNative, pool: PoolRef<'_>, worker: *mut WorkerRecord) {
    if let Some(worker) = pool.worker_of(worker) {
        with(native, pool, |p| p.stop(worker.raw()));
    }
}

/// `ProcessPool::signal(signo, worker)`, in `worker` or (null, or none of the
/// pool's) the master.
pub fn signal(native: &dyn PoolNative, pool: PoolRef<'_>, signo: c_int, worker: *mut WorkerRecord) {
    let worker = pool.worker_of(worker).map_or(ptr::null_mut(), Worker::raw);
    deferred(with(native, pool, |p| p.signal(signo, worker)));
}

/// `ProcessPool::spawn(worker)`, whose pid every caller ignores; nothing when `worker`
/// is none of the pool's.
pub fn spawn(native: &dyn PoolNative, pool: PoolRef<'_>, worker: *mut WorkerRecord) {
    if let Some(worker) = pool.worker_of(worker) {
        deferred(with(native, pool, |p| p.spawn(worker.raw())));
    }
}

/// `ProcessPool::create(worker_num, msgqueue_key, ipc_mode)`.
#[must_use]
pub fn create(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker_num: u32,
    key: c_int,
    mode: c_int,
) -> bool {
    let mode = u8::try_from(mode.cast_unsigned() & 0xff).unwrap_or_default();
    swr_v1_process_pool_create(native, pool, worker_num, key, mode)
}

/// `ProcessPool::push_message(type, data, length)`.
#[must_use]
pub fn push_message(native: &dyn PoolNative, pool: PoolRef<'_>, kind: u8, data: &[u8]) -> bool {
    swr_v1_process_pool_push(native, pool, kind, data)
}

/// `push_message(SW_WORKER_MESSAGE_STOP, WorkerStopMessage { pid, worker_id })`:
/// the C layout's 4-byte pid, 2-byte id and 2 bytes of padding.
#[must_use]
pub fn push_stop(native: &dyn PoolNative, pool: PoolRef<'_>, pid: c_int, worker_id: u16) -> bool {
    /// `SW_WORKER_MESSAGE_STOP`.
    const WORKER_MESSAGE_STOP: u8 = 1;
    let ([a, b, c, d], [e, f]) = (pid.to_ne_bytes(), worker_id.to_ne_bytes());
    push_message(native, pool, WORKER_MESSAGE_STOP, &[a, b, c, d, e, f, 0, 0])
}

/// `ProcessPool::dispatch(data, dst_worker_id)`, or `dispatch_sync` when
/// `blocking`, of `task` to `worker`, else one the pool schedules.
#[must_use]
pub fn dispatch(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    task: Packed<'_>,
    worker: Option<u32>,
    blocking: bool,
) -> bool {
    swr_v1_process_pool_dispatch_event(native, pool, task.bytes(), worker, blocking)
}

/// `ProcessPool::add_worker(worker)`: keeps an existing entry for the pid; nothing
/// before `create`.
pub fn add_worker(pool: &PoolRecord, worker: Worker<'_>) {
    pool.with_registry(|registry| registry.insert(worker.pid(), worker.raw().cast(), 0));
}

/// `ProcessPool::del_worker(worker)`: whether the worker's pid was registered.
#[must_use]
pub fn del_worker(pool: &PoolRecord, worker: Worker<'_>) -> bool {
    pool.with_registry(|registry| registry.unregister(worker.pid()))
        .unwrap_or(false)
}

/// `ProcessPool::get_worker_by_pid(pid)`: null before `create` or for an unknown pid.
#[must_use]
pub fn get_worker_by_pid(pool: &PoolRecord, pid: c_int) -> *mut WorkerRecord {
    pool.with_registry(|registry| registry.lookup(pid))
        .map_or(ptr::null_mut(), <*mut c_void>::cast)
}

/// `ProcessPool::is_worker_running(worker)`.
#[must_use]
pub fn is_worker_running(pool: PoolRef<'_>, worker: Worker<'_>) -> bool {
    pool.running() && !worker_is_shutdown() && !worker.exceeded_max_request()
}

/// `Worker::send_pipe_message(buf, n, flags)` of `data`: through its pool's queue or
/// the pipe end `flags` names. None for a worker no pool holds, which upstream
/// doesn't send through.
#[must_use]
pub fn send_pipe_message(
    native: &dyn PoolNative,
    worker: Worker<'_>,
    data: &[u8],
    flags: c_int,
) -> Option<usize> {
    /// `SW_PIPE_MASTER`.
    const PIPE_MASTER: c_int = 1;
    let pool = worker.pool_ref()?;
    let id = worker.id();
    let pipe = if flags & PIPE_MASTER != 0 {
        worker.pipe_master()
    } else {
        worker.pipe_worker()
    };
    let queue = pool.use_msgqueue();
    swr_v1_process_worker_send(native, pool, pipe, queue, id, data, flags)
}

/// `ProcessPool::listen(socket_file, backlog)`: a unix stream listener.
#[must_use]
pub fn listen_unix(pool: PoolRef<'_>, file: &CStr, backlog: c_int) -> bool {
    listen_socket(
        pool,
        crate::constants::SocketType::SW_SOCK_UNIX_STREAM as c_int,
        file,
        0,
        backlog,
    )
}

/// `ProcessPool::listen(host, port, backlog)` on a socket of `kind` (`SW_SOCK_*`),
/// made by `make_server_socket`. False when the pool is not an `SW_IPC_SOCKET` pool
/// or the listener cannot be made.
#[must_use]
pub fn listen_socket(
    pool: PoolRef<'_>,
    kind: c_int,
    host: &CStr,
    port: c_int,
    backlog: c_int,
) -> bool {
    listen(pool.record(), host, port, || {
        let socket = crate::tls::upstream::server_socket(kind, host, port, backlog)?;
        Some(std::rc::Rc::new(socket))
    })
}

/// Both `listen`s, over `make`, which makes the listener (which the pool then owns
/// and frees, or `None`): only an `SW_IPC_SOCKET` pool has a stream listener, which
/// it records with its address.
fn listen(
    pool: &PoolRecord,
    host: &CStr,
    port: c_int,
    make: impl FnOnce() -> Option<crate::process::pool::Connection>,
) -> bool {
    if pool.ipc_mode.get() != crate::process::process_pool::IpcMode::Socket {
        crate::log::error_log(
            crate::log::LogLevel::SW_LOG_WARNING,
            swoole_rs_error::ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT as c_int,
            c"ProcessPool::listen",
            format_args!("not support, ipc_mode must be SW_IPC_SOCKET"),
        );
        return false;
    }
    if pool
        .with_ipc(|ipc| swr_v1_process_pool_ipc_address(ipc, host, port))
        .is_none()
    {
        return false;
    }
    let Some(listener) = make() else {
        return false;
    };
    pool.with_ipc(|ipc| swr_v1_process_pool_ipc_set_listener(ipc, listener));
    true
}

/// `ProcessPool::response(data, length)`: append to the stream connection's reply.
/// False, setting `SW_ERROR_INVALID_PARAMS`, when there is no connection to answer.
#[must_use]
pub fn response(pool: PoolRef<'_>, data: &[u8]) -> bool {
    let written = pool
        .record()
        .with_ipc(|ipc| swr_v1_process_pool_ipc_respond(ipc, data))
        .unwrap_or(false);
    if !written {
        // SW_ERROR_INVALID_PARAMS.
        crate::log::set_last_error(505);
    }
    written
}

/// `ProcessPool::send_message(worker_id, message, length)`: to the worker's
/// master pipe, through the message bus when the pool has one.
///
/// False for a `worker_id` the pool has no worker for, or a pool without pipes.
#[must_use]
pub fn send_message(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker_id: u32,
    message: &[u8],
) -> bool {
    use crate::DataHead;
    let handle = pool;
    let pool = pool.record();
    // `get_worker`: the pool's record for `worker_id`.
    let index = worker_id.wrapping_sub(u32::from(pool.start_id.get())) as usize;
    // `pipes` owns the workers' pipes: `create` makes one per worker or none, and
    // `destroy` frees them and clears it.
    let Some(worker) = pool.worker(index).filter(|_| pool.has_pipes()) else {
        return false;
    };
    let pipe = worker.pipe_master();
    let length = message.len();
    if !pool.has_message_bus() {
        // `pipe_master->send_async(message)`: the pipe's write, through the event
        // loop while one is available. Upstream returns its ssize_t as the bool, so a
        // failure is true too.
        let (id, flags) = (worker.id(), crate::process::pool::PIPE_NONBLOCK);
        let sent = swr_v1_process_worker_send(native, handle, pipe, false, id, message, flags);
        return sent != Some(0);
    }
    let mut head = DataHead {
        reactor_id: swoole_get_worker_id().c_cast::<i16>(),
        len: length.c_cast::<u32>(),
        ..DataHead::default()
    };
    // The pool's bus (not null, checked above) and the worker's master pipe.
    native.bus_write(handle, worker, &mut head, message)
}

/// `ProcessPool::pop_message(data, size)`: the next message in the master's box,
/// under the box's lock.
///
/// The pool's box is the one `create` made with `SW_CHAN_LOCK`.
#[must_use]
pub fn pop_message(pool: PoolRef<'_>, data: &mut [u8]) -> Option<usize> {
    pool.record()
        .message_box_channel()
        .and_then(|message_box| message_box.pop(data, true))
}

/// `Worker::set_max_request(max_request, grace)`: this process's request limit.
pub fn worker_set_max_request(max_request: u32, grace: u32) {
    worker_global_max_requests(max_request, grace, crate::swoole_system_random);
}

/// `swoole::ExitStatus`: a child's pid and `waitpid` status.
pub struct ExitStatus {
    pid: c_int,
    status: c_int,
}

impl ExitStatus {
    #[must_use]
    pub fn new(pid: c_int, status: c_int) -> Self {
        Self { pid, status }
    }

    /// `ExitStatus::get_pid()`.
    #[must_use]
    pub fn pid(&self) -> c_int {
        self.pid
    }

    /// `ExitStatus::get_status()`: the `waitpid` status.
    #[must_use]
    pub fn status(&self) -> c_int {
        self.status
    }
}

/// `Worker::report_error(exit_status)`: the abnormal exit warning.
pub fn worker_report_error(worker: Worker<'_>, exit_status: &ExitStatus) {
    let id = worker.id();
    let ExitStatus { pid, status } = exit_status;
    let signal = libc::WTERMSIG(*status);
    let report = if signal == libc::SIGSEGV {
        crate::bug_report_message()
    } else {
        String::new()
    };
    crate::log::warning(
        c"Worker::report_error",
        format_args!(
            "worker(pid={pid}, id={id}) abnormal exit, status={}, signal={signal}{report}",
            libc::WEXITSTATUS(*status)
        ),
    );
}

/// `swoole::reload_warn`: a reload's failed or forced kill, under the
/// `ReloadTask` method upstream logged it from; `errno` is the failed kill's.
pub fn reload_warn(kind: KillWarning, pid: c_int, worker_id: u32) {
    use crate::log::{sys_warning, warning};
    match kind {
        KillWarning::KillOneFailed => sys_warning(
            c"ReloadTask::kill_one",
            format_args!("kill({pid}, SIGTERM) [{worker_id}] failed"),
        ),
        KillWarning::KillAllFailed => sys_warning(
            c"ReloadTask::kill_all",
            format_args!("failed to kill({pid}, SIGTERM) worker#[{worker_id}]"),
        ),
        KillWarning::ForceKill => warning(
            c"ReloadTask::kill_all",
            format_args!("force kill worker process(pid={pid}, id={worker_id})"),
        ),
    }
}

/// `Worker::is_shutdown()`: the current worker's.
#[must_use]
pub fn worker_is_shutdown() -> bool {
    with_globals(|globals| globals.shutdown.load(Relaxed))
}

/// `Worker::is_running()`.
#[must_use]
pub fn worker_is_running() -> bool {
    with_globals(|globals| globals.running.load(Relaxed))
}

/// `swoole_get_worker_id()`.
#[must_use]
pub fn swoole_get_worker_id() -> u32 {
    with_globals(|globals| globals.id.load(Relaxed))
}

/// `swoole_get_worker_pid()`.
#[must_use]
pub fn swoole_get_worker_pid() -> c_int {
    with_globals(|globals| globals.pid.load(Relaxed))
}

/// `swoole_get_worker_type()`: upstream's core-tests (`log.cpp`'s `log_level`) call it.
#[must_use]
pub fn swoole_get_worker_type() -> c_int {
    c_int::from(with_globals(|globals| globals.kind.load(Relaxed)))
}

/// `swoole_get_worker_type()`, `None` before the process has a type.
#[must_use]
pub fn worker_type() -> Option<WorkerType> {
    WorkerType::from_code(swoole_get_worker_type().into())
}

/// `swoole_set_worker_type(type)`.
pub fn set_worker_type(kind: WorkerType) {
    with_globals(|globals| globals.kind.store(kind.code(), Relaxed));
}

/// `swoole_set_worker_id(worker_id)`.
pub fn swoole_set_worker_id(id: u32) {
    with_globals(|globals| globals.id.store(id, Relaxed));
}

/// `swoole_set_worker_pid(pid)`.
pub fn swoole_set_worker_pid(pid: c_int) {
    with_globals(|globals| globals.pid.store(pid, Relaxed));
}

/// `swoole_set_worker_type(type)`, narrowed to the record's `uint8_t`.
pub fn swoole_set_worker_type(kind: c_int) {
    let kind = u8::try_from(kind & 0xFF).unwrap_or_default();
    with_globals(|globals| globals.kind.store(kind, Relaxed));
}

/// `swoole_get_worker_symbol()`: the log's mark for the current worker's type.
#[must_use]
pub fn swoole_get_worker_symbol() -> std::ffi::c_char {
    swr_v1_worker_symbol(swoole_get_worker_type())
}

/// The process logger's native effects.
const LOGGER_HOST: crate::utilities::LoggerHost = crate::utilities::LoggerHost {
    redirect: crate::swoole_redirect_stdout,
    identity: logger_identity,
};

fn logger_identity() -> crate::utilities::LoggerIdentity {
    crate::utilities::LoggerIdentity {
        worker_id: swoole_get_worker_id().cast_signed(),
        worker_pid: swoole_get_worker_pid(),
        worker_symbol: swoole_get_worker_symbol(),
    }
}

/// The process logger. `SwooleG.logger` points here, as at a `swoole::Logger`,
/// whose only member is its logger.
static LOGGER: std::sync::atomic::AtomicPtr<crate::utilities::Logger> =
    std::sync::atomic::AtomicPtr::new(ptr::null_mut());

/// The logger [`LOGGER`] points at, which this owns from `new` until `delete`.
static OWNER: std::sync::Mutex<Option<std::sync::Arc<crate::utilities::Logger>>> =
    std::sync::Mutex::new(None);

/// Create the process logger on `SwooleG.stdout`.
pub fn logger_new() {
    use crate::utilities::Logger;
    let stdout = crate::swr_v1_core_stdout();
    let logger = std::sync::Arc::new(Logger::new(stdout, LOGGER_HOST));
    LOGGER.store(std::sync::Arc::as_ptr(&logger).cast_mut(), Relaxed);
    // A logger `new` replaces is leaked, as upstream's pointer overwrite does.
    _ = OWNER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(logger)
        .map(std::sync::Arc::into_raw);
    crate::swr_v1_core_set_logger(LOGGER.as_ptr().cast());
}

/// Drop the process logger after its final close.
pub(crate) fn logger_delete() {
    crate::swr_v1_core_set_logger(ptr::null_mut());
    LOGGER.store(ptr::null_mut(), Relaxed);
    drop(
        OWNER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take(),
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::log::capture;
    use crate::process::pool::{
        pool_ipc_address, pool_ipc_free, pool_ipc_new, swr_v1_process_pool_ipc_listener,
    };

    thread_local! {
        /// This thread's logs, kept so each origin reads its own.
        static LOGGED: std::cell::RefCell<Vec<capture::Logged>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    /// What `origin` logged on this thread: (kind, level, code, message).
    pub(crate) fn logged(origin: &str) -> Vec<(c_int, c_int, c_int, String)> {
        LOGGED.with_borrow_mut(|logged| logged.extend(capture::logged()));
        LOGGED.with_borrow(|logged| {
            logged
                .iter()
                .filter(|entry| entry.3 == origin)
                .map(|(kind, level, code, _, message)| {
                    (*kind as c_int, *level, *code, message.clone())
                })
                .collect()
        })
    }

    /// Run this binary's logs through the log macros, where [`logged`] reads them.
    pub(crate) fn install() {
        capture::install();
    }

    fn make(kind: c_int, port: c_int) -> Option<crate::process::pool::Connection> {
        (kind == 1 && port == 9501)
            .then(|| -> crate::process::pool::Connection { std::rc::Rc::new(port) })
    }

    fn keep(_: crate::process::pool::Connection) {}

    #[test]
    fn listen_needs_a_socket_pool_and_keeps_its_listener() {
        install();
        // SAFETY: a zeroed record is a pool before `create`.
        let pool: PoolRecord = unsafe { std::mem::zeroed() };
        pool.ipc.set(pool_ipc_new(true));
        pool.ipc_mode
            .set(crate::process::process_pool::IpcMode::UnixSock);
        let refused = listen(&pool, c"127.0.0.1", 9501, || make(1, 9501));
        assert!(!refused, "an SW_IPC_UNIXSOCK pool has no stream listener");
        assert_eq!(
            logged("ProcessPool::listen"),
            [(
                3,
                4,
                507,
                "not support, ipc_mode must be SW_IPC_SOCKET".to_owned()
            )],
            "the upstream warning"
        );
        pool.ipc_mode
            .set(crate::process::process_pool::IpcMode::Socket);
        let failed = listen(&pool, c"127.0.0.1", 9502, || make(1, 9502));
        assert!(!failed, "no socket, no listener");
        // SAFETY: the pool's IPC state.
        assert!(
            swr_v1_process_pool_ipc_listener(unsafe { &*pool.ipc.get() }).is_none(),
            "none kept"
        );
        let listened = listen(&pool, c"127.0.0.1", 9501, || make(1, 9501));
        assert!(listened, "listening");
        // SAFETY: the pool's IPC state.
        let listener = swr_v1_process_pool_ipc_listener(unsafe { &*pool.ipc.get() });
        assert_eq!(
            listener.and_then(|listener| listener.downcast_ref::<c_int>()),
            Some(&9501),
            "the socket is kept"
        );
        // SAFETY: the pool's IPC state.
        let address = pool_ipc_address(unsafe { &*pool.ipc.get() });
        assert_eq!(address, Some((c"127.0.0.1", 9501)), "and its address");
        // SAFETY: the pool's IPC state, not used again.
        pool_ipc_free(*unsafe { Box::from_raw(pool.ipc.get()) }, keep);
    }

    #[test]
    fn pop_message_takes_the_oldest_message_from_the_box() {
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        let mut out = [0u8; 16];
        let none = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(none, None, "no box");
        let channel = crate::memory::SharedChannel::make(256, 16, crate::memory::channel::LOCK);
        pool.message_box = channel.expect("a box");
        for message in [&b"first"[..], b"second"] {
            let pushed = pool
                .message_box
                .get()
                .is_some_and(|box_| box_.push(message));
            assert!(pushed, "pushed");
        }
        let n = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(&out[..n.unwrap_or(0)], b"first", "oldest first");
        let n = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(&out[..n.unwrap_or(0)], b"second", "then the next");
        let n = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(n, None, "then empty");
        pool.message_box.free();
    }

    #[test]
    fn a_response_needs_a_connection() {
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        pool.ipc.set(pool_ipc_new(true));
        assert!(!response(PoolRef::from(&mut pool), b"ok"), "no connection");
        assert_eq!(crate::log::last_error(), 505, "SW_ERROR_INVALID_PARAMS");
        // SAFETY: the pool's IPC state, not used again.
        pool_ipc_free(*unsafe { Box::from_raw(pool.ipc.get()) }, keep);
    }

    #[test]
    fn reload_kills_warn_under_upstream_reload_task() {
        install();
        reload_warn(KillWarning::KillAllFailed, 10, 1);
        reload_warn(KillWarning::ForceKill, 11, 2);
        reload_warn(KillWarning::KillOneFailed, 12, 3);
        let all = logged("ReloadTask::kill_all");
        assert_eq!(all.len(), 2, "two kill_all warnings: {all:?}");
        assert_eq!(
            (all[0].0, all[0].3.as_str()),
            (0, "failed to kill(10, SIGTERM) worker#[1]"),
            "a system warning"
        );
        assert_eq!(
            (all[1].0, all[1].3.as_str()),
            (1, "force kill worker process(pid=11, id=2)"),
            "a plain warning"
        );
        let one = logged("ReloadTask::kill_one");
        assert_eq!(one.len(), 1, "one kill_one warning: {one:?}");
        assert_eq!(
            (one[0].0, one[0].3.as_str()),
            (0, "kill(12, SIGTERM) [3] failed"),
            "a system warning"
        );
    }

    #[test]
    fn an_abnormal_exit_reports_its_code_and_signal() {
        install();
        // SAFETY: a zeroed record is a worker before `init`.
        let mut worker: WorkerRecord = unsafe { std::mem::zeroed() };
        Worker::from(&worker).set_id(4);
        let status = ExitStatus {
            pid: 321,
            status: 3 << 8,
        };
        worker_report_error((&mut worker).into(), &status);
        assert_eq!(
            logged("Worker::report_error"),
            [(
                1,
                0,
                0,
                "worker(pid=321, id=4) abnormal exit, status=3, signal=0".to_owned()
            )],
            "upstream's warning"
        );
    }

    #[test]
    fn worker_identity_round_trips_through_the_upstream_functions() {
        let checks = std::thread::spawn(|| {
            swoole_set_worker_id(7);
            swoole_set_worker_pid(4321);
            swoole_set_worker_type(4);
            assert_eq!(swoole_get_worker_id(), 7, "id");
            assert_eq!(swoole_get_worker_pid(), 4321, "pid");
            assert_eq!(swoole_get_worker_type(), 4, "type");
            assert_eq!(
                swoole_get_worker_symbol(),
                std::ffi::c_char::from_ne_bytes(*b"^"),
                "task worker mark"
            );
            swoole_set_worker_type(-1);
            assert_eq!(swoole_get_worker_type(), 255, "type narrows to uint8_t");
            assert!(!worker_is_running(), "not running before init");
            assert!(!worker_is_shutdown(), "not shut down before init");
        });
        assert!(checks.join().is_ok(), "identity checks on a fresh thread");
    }

    #[test]
    fn worker_lookup_needs_a_created_pool() {
        // SAFETY: a zeroed record is a pool before `create`, with a null registry.
        let pool: PoolRecord = unsafe { std::mem::zeroed() };
        assert!(
            get_worker_by_pid(&pool, 1).is_null(),
            "no registry, no worker"
        );
    }
}
