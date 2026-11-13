//! The `ProcessPool` and `Worker` methods (once `src/os/process_pool.cc`) that are
//! whole Rust operations. Each runs over crates/server's [`PoolNative`] side as its
//! C++ forward did.

use crate::process_pool::Protocol;
use crate::task_file::Packed;
use crate::worker_record::{Worker, WorkerType};
#[cfg(any(feature = "upstream-symbols", test))]
use std::ffi::CStr;
use std::ffi::{c_int, c_void};
use std::ptr;
use swoole_rs_corebase::CCast;

use crate::pool::{raise_bailout, swr_v1_process_pool_ipc_respond, swr_v1_process_worker_send};
#[cfg(any(feature = "upstream-symbols", test))]
use crate::pool::{swr_v1_process_pool_ipc_address, swr_v1_process_pool_ipc_set_listener};
use crate::process_pool::{
    PoolNative, PoolRecord, PoolRef, RAISE, Run, run_op, swr_v1_process_pool_create,
    swr_v1_process_pool_dispatch_event, swr_v1_process_pool_push, swr_v1_process_pool_push_event,
    swr_v1_worker_symbol,
};
use crate::reload_task::KillWarning;
use crate::worker_globals::{
    swr_v1_worker_global_get, swr_v1_worker_global_set, worker_global_max_requests,
};
use crate::worker_record::WorkerRecord;

/// crates/server's side, which every method below lends its export
/// (`crates/server/src/pool_native.rs`); tests drive the exports with their own
/// hosts and never reach it.
fn native() -> &'static dyn PoolNative {
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    {
        // crates/server's, in the same archive; declared, since crates/server
        // depends on this crate.
        unsafe extern "Rust" {
            safe fn swr_v1_process_pool_native() -> &'static dyn PoolNative;
        }
        swr_v1_process_pool_native()
    }
    #[cfg(not(all(feature = "upstream-symbols", not(test))))]
    swoole_rs_corebase::fatal("no native process pool without upstream-symbols")
}

/// A method's C++ return type, from the `i64` an export returns.
pub(crate) trait Narrow {
    fn narrow(value: i64) -> Self;
}

impl Narrow for () {
    fn narrow(_: i64) {}
}

impl Narrow for bool {
    fn narrow(value: i64) -> bool {
        value != 0
    }
}

impl Narrow for u32 {
    fn narrow(value: i64) -> u32 {
        value.c_cast::<u32>()
    }
}

impl Narrow for *mut core::ffi::c_void {
    fn narrow(value: i64) -> Self {
        core::ptr::with_exposed_provenance_mut(usize::try_from(value).unwrap_or(0))
    }
}

impl Narrow for isize {
    fn narrow(value: i64) -> isize {
        value.c_cast::<isize>()
    }
}

impl Narrow for c_int {
    fn narrow(value: i64) -> c_int {
        value.c_cast::<c_int>()
    }
}

/// One `ProcessPool` method on `worker`, one of the pool's, or none; a callback's
/// bailout is raised once the export's frames are gone. This frame holds nothing,
/// so the `longjmp` may cross it.
fn run_on<R: Narrow>(pool: PoolRef<'_>, worker: Option<Worker<'_>>, run: Run) -> R {
    let result = run_op(native(), pool, worker, run);
    if result == RAISE {
        raise_bailout();
    }
    R::narrow(result)
}

/// [`run_on`] the pool itself.
fn run<R: Narrow>(pool: PoolRef<'_>, run: Run) -> R {
    run_on(pool, None, run)
}

/// Each `ProcessPool` method that runs on the pool alone: its name, parameters,
/// return type, and the [`Run`].
macro_rules! run_methods {
    ($(fn $name:ident($($p:ident: $t:ty),*) -> $r:ty = $run:expr;)*) => {$(
        #[doc = concat!("`ProcessPool::", stringify!($name), "`.")]
        pub fn $name(pool: PoolRef<'_>, $($p: $t),*) -> $r {
            run(pool, $run)
        }
    )*};
}

run_methods! {
    fn create_message_box(size: usize) -> c_int = Run::CreateMessageBox(size as u64);
    fn create_message_bus() -> c_int = Run::CreateMessageBus;
    fn set_protocol(protocol: Protocol) -> () = Run::SetProtocol(protocol);
    fn start_check() -> c_int = Run::StartCheck;
    fn start() -> c_int = Run::Start;
    fn drop_reload_task() -> () = Run::DropReloadTask;
    fn reload() -> bool = Run::Reload;
    fn reopen_logger() -> () = Run::ReopenLogger;
    fn kill_all_workers(signo: c_int) -> () = Run::KillAll(signo);
    fn shutdown() -> bool = Run::Shutdown;
    fn set_max_request(max: u32, grace: u32) -> () = Run::SetMaxRequest { max, grace };
    fn set_type(kind: c_int) -> () = Run::SetType(kind.c_cast::<u8>());
    fn set_start_id(id: c_int) -> () = Run::SetStartId(id.c_cast::<u16>());
    fn detach() -> bool = Run::Detach;
    fn wait() -> c_int = Run::Wait;
    fn destroy() -> () = Run::Destroy;
    fn free_message_box() -> () = Run::FreeMessageBox;
}

/// `ProcessPool::push_message(event)`: the `EventData` with `head` (its `len` set to
/// `data`'s) and `data`.
#[must_use]
pub fn push_event(pool: PoolRef<'_>, head: &swoole_rs_corebase::DataHead, data: &[u8]) -> c_int {
    c_int::narrow(swr_v1_process_pool_push_event(native(), pool, head, data))
}

/// `ProcessPool::stop(worker)`; nothing when `worker` is none of the pool's.
pub fn stop(pool: PoolRef<'_>, worker: *mut WorkerRecord) {
    if let Some(worker) = pool.worker_of(worker) {
        run_on::<()>(pool, Some(worker), Run::Stop);
    }
}

/// `ProcessPool::signal(signo, worker)`, in `worker` or (null, or none of the
/// pool's) the master.
pub fn signal(pool: PoolRef<'_>, signo: c_int, worker: *mut WorkerRecord) {
    let worker = pool.worker_of(worker);
    run_on::<()>(pool, worker, Run::Signal(signo));
}

/// `ProcessPool::spawn(worker)`; -1 when `worker` is none of the pool's.
pub fn spawn(pool: PoolRef<'_>, worker: *mut WorkerRecord) -> c_int {
    let Some(worker) = pool.worker_of(worker) else {
        return -1;
    };
    run_on(pool, Some(worker), Run::Spawn)
}

/// `ProcessPool::create(worker_num, msgqueue_key, ipc_mode)`.
#[must_use]
pub fn create(pool: PoolRef<'_>, worker_num: u32, key: c_int, mode: c_int) -> c_int {
    let mode = u8::try_from(mode.cast_unsigned() & 0xff).unwrap_or_default();
    c_int::narrow(swr_v1_process_pool_create(
        native(),
        pool,
        worker_num,
        key,
        mode,
    ))
}

/// `ProcessPool::push_message(type, data, length)`.
#[must_use]
pub fn push_message(pool: PoolRef<'_>, kind: u8, data: &[u8]) -> c_int {
    c_int::narrow(swr_v1_process_pool_push(native(), pool, kind, data))
}

/// `push_message(SW_WORKER_MESSAGE_STOP, WorkerStopMessage { pid, worker_id })`:
/// the C layout's 4-byte pid, 2-byte id and 2 bytes of padding.
#[must_use]
pub fn push_stop(pool: PoolRef<'_>, pid: c_int, worker_id: u16) -> c_int {
    /// `SW_WORKER_MESSAGE_STOP`.
    const WORKER_MESSAGE_STOP: u8 = 1;
    let ([a, b, c, d], [e, f]) = (pid.to_ne_bytes(), worker_id.to_ne_bytes());
    push_message(pool, WORKER_MESSAGE_STOP, &[a, b, c, d, e, f, 0, 0])
}

/// `ProcessPool::dispatch(data, dst_worker_id)`, or `dispatch_sync` when
/// `blocking`, of `task`.
pub fn dispatch(
    pool: PoolRef<'_>,
    task: Packed<'_>,
    worker_id: &mut c_int,
    blocking: bool,
) -> c_int {
    c_int::narrow(swr_v1_process_pool_dispatch_event(
        native(),
        pool,
        task.bytes(),
        worker_id,
        blocking,
    ))
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
    pool.running() && swr_v1_worker_global_get(4) == 0 && !worker.exceeded_max_request()
}

/// `Worker::send_pipe_message(buf, n, flags)` of `data`: through its pool's queue or
/// the pipe end `flags` names. -1 for a worker no pool holds, which upstream
/// doesn't send through.
#[must_use]
pub fn send_pipe_message(worker: Worker<'_>, data: &[u8], flags: c_int) -> isize {
    /// `SW_PIPE_MASTER`.
    const PIPE_MASTER: c_int = 1;
    let Some(pool) = worker.pool_ref() else {
        return -1;
    };
    let id = worker.id();
    let pipe = if flags & PIPE_MASTER != 0 {
        worker.pipe_master()
    } else {
        worker.pipe_worker()
    };
    let queue = pool.use_msgqueue();
    swr_v1_process_worker_send(native(), pool, pipe, queue, id, data, flags)
}

#[cfg(feature = "upstream-symbols")]
// crates/tls defines it under its upstream symbol; declared, since
// crates/tls (through crates/reactor) depends on this crate.
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole18make_server_socketE12swSocketTypePKcii"]
    fn make_server_socket(
        kind: c_int,
        host: *const std::ffi::c_char,
        port: c_int,
        backlog: c_int,
    ) -> *mut c_void;
}

/// `ProcessPool::listen(socket_file, backlog)`: a unix stream listener.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn listen_unix(pool: PoolRef<'_>, file: &CStr, backlog: c_int) -> c_int {
    // 5 is SW_SOCK_UNIX_STREAM.
    listen_socket(pool, 5, file, 0, backlog)
}

/// `ProcessPool::listen(host, port, backlog)` on a socket of `kind` (`SW_SOCK_*`),
/// made by `make_server_socket`.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn listen_socket(
    pool: PoolRef<'_>,
    kind: c_int,
    host: &CStr,
    port: c_int,
    backlog: c_int,
) -> c_int {
    listen(pool.record(), host, port, || {
        // SAFETY: crates/tls's `make_server_socket` over a NUL-terminated host, read
        // for the call: a native socket the caller owns, or null.
        unsafe { make_server_socket(kind, host.as_ptr(), port, backlog) }
    })
}

/// Both `listen`s, over `make`, which makes the listener (a native socket the pool
/// then owns and frees, or null): only an `SW_IPC_SOCKET` pool has a stream
/// listener, which it records with its address.
#[cfg(any(feature = "upstream-symbols", test))]
fn listen(
    pool: &PoolRecord,
    host: &CStr,
    port: c_int,
    make: impl FnOnce() -> *mut c_void,
) -> c_int {
    if pool.ipc_mode != crate::process_pool::IpcMode::Socket {
        // SW_LOG_WARNING, SW_ERROR_OPERATION_NOT_SUPPORT.
        swoole_rs_corebase::log::error_log(
            4,
            507,
            c"ProcessPool::listen",
            format_args!("not support, ipc_mode must be SW_IPC_SOCKET"),
        );
        return -1;
    }
    if pool
        .with_ipc(|ipc| swr_v1_process_pool_ipc_address(ipc, host, port))
        .is_none()
    {
        return -1;
    }
    let socket = make();
    if socket.is_null() {
        return -1;
    }
    // The pool now owns the listener.
    pool.with_ipc(|ipc| swr_v1_process_pool_ipc_set_listener(ipc, socket));
    0
}

/// `ProcessPool::response(data, length)`: append to the stream connection's reply.
#[must_use]
pub fn response(pool: PoolRef<'_>, data: &[u8]) -> c_int {
    if pool
        .record()
        .with_ipc(|ipc| swr_v1_process_pool_ipc_respond(ipc, data))
        .unwrap_or(false)
    {
        0
    } else {
        // SW_ERROR_INVALID_PARAMS.
        swoole_rs_corebase::log::set_last_error(505);
        -1
    }
}

/// `ProcessPool::send_message(worker_id, message, length)`: to the worker's
/// master pipe, through the message bus when the pool has one.
///
/// False for a `worker_id` the pool has no worker for, or a pool without pipes.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn send_message(pool: PoolRef<'_>, worker_id: u32, message: &[u8]) -> bool {
    use swoole_rs_corebase::DataHead;
    let handle = pool;
    let pool = pool.record();
    // `get_worker`: the pool's record for `worker_id`.
    let index = worker_id.wrapping_sub(u32::from(pool.start_id)) as usize;
    // `pipes` owns the workers' pipes: `create` makes one per worker or none, and
    // `destroy` frees them and clears it.
    let Some(worker) = pool.worker(index).filter(|_| !pool.pipes.is_null()) else {
        return false;
    };
    let pipe = worker.pipe_master();
    let length = message.len();
    if pool.message_bus.is_null() {
        // `pipe_master->send_async(message)`: the pipe's write, through the event
        // loop while one is available.
        let (id, flags) = (worker.id(), crate::pool::PIPE_NONBLOCK);
        return swr_v1_process_worker_send(native(), handle, pipe, false, id, message, flags) != 0;
    }
    let mut head = DataHead {
        reactor_id: swoole_get_worker_id().c_cast::<i16>(),
        len: length.c_cast::<u32>(),
        ..DataHead::default()
    };
    // The pool's bus (not null, checked above) and the worker's master pipe.
    native().bus_write(handle, worker, &mut head, message)
}

/// `ProcessPool::pop_message(data, size)`: the next message in the master's box,
/// under the box's lock.
///
/// The pool's box is the one `create` made with `SW_CHAN_LOCK`.
#[must_use]
pub fn pop_message(pool: PoolRef<'_>, data: &mut [u8]) -> c_int {
    pool.record()
        .message_box_channel()
        .map_or(-1, |message_box| message_box.pop(data, true))
}

/// `Worker::set_max_request(max_request, grace)`: this process's request limit.
pub fn worker_set_max_request(max_request: u32, grace: u32) {
    worker_global_max_requests(max_request, grace, swoole_rs_corebase::swoole_system_random);
}

/// `swoole::ExitStatus`: a child's pid and `waitpid` status.
#[repr(C)]
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
        swoole_rs_corebase::bug_report_message()
    } else {
        String::new()
    };
    swoole_rs_corebase::log::warning(
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
    use swoole_rs_corebase::log::{sys_warning, warning};
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
    swr_v1_worker_global_get(4) != 0
}

/// `Worker::is_running()`.
#[must_use]
pub fn worker_is_running() -> bool {
    swr_v1_worker_global_get(3) != 0
}

/// `swoole_get_worker_id()`.
#[must_use]
pub fn swoole_get_worker_id() -> u32 {
    u32::try_from(swr_v1_worker_global_get(0)).unwrap_or_default()
}

/// `swoole_get_worker_pid()`.
#[must_use]
pub fn swoole_get_worker_pid() -> c_int {
    c_int::try_from(swr_v1_worker_global_get(1)).unwrap_or_default()
}

/// `swoole_get_worker_type()`: upstream's core-tests (`log.cpp`'s `log_level`) call it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_get_worker_typev")
)]
#[must_use]
pub extern "C" fn swoole_get_worker_type() -> c_int {
    c_int::try_from(swr_v1_worker_global_get(2)).unwrap_or_default()
}

/// `swoole_get_worker_type()`, `None` before the process has a type.
#[must_use]
pub fn worker_type() -> Option<WorkerType> {
    WorkerType::from_code(swr_v1_worker_global_get(2))
}

/// `swoole_set_worker_type(type)`.
pub fn set_worker_type(kind: WorkerType) {
    swr_v1_worker_global_set(2, kind.code().into());
}

/// `swoole_set_worker_id(worker_id)`.
pub fn swoole_set_worker_id(id: u32) {
    swr_v1_worker_global_set(0, i64::from(id));
}

/// `swoole_set_worker_pid(pid)`.
pub fn swoole_set_worker_pid(pid: c_int) {
    swr_v1_worker_global_set(1, i64::from(pid));
}

/// `swoole_set_worker_type(type)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_set_worker_typei")
)]
pub extern "C" fn swoole_set_worker_type(kind: c_int) {
    swr_v1_worker_global_set(2, i64::from(kind));
}

/// `swoole_get_worker_symbol()`: the log's mark for the current worker's type.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z24swoole_get_worker_symbolv")
)]
#[must_use]
pub extern "C" fn swoole_get_worker_symbol() -> std::ffi::c_char {
    swr_v1_worker_symbol(swoole_get_worker_type())
}

/// The logger's native effects, the table `swoole::Logger` hands
/// `swr_v1_logger_new` (once `src/core/log.cc`).
#[cfg(feature = "upstream-symbols")]
#[unsafe(no_mangle)]
pub static swr_logger_host: swoole_rs_utilities::LoggerHost = swoole_rs_utilities::LoggerHost {
    redirect: swoole_rs_corebase::swoole_redirect_stdout,
    backtrace: swoole_rs_corebase::swoole_print_backtrace,
    identity: logger_identity,
};

#[cfg(feature = "upstream-symbols")]
extern "C" fn logger_identity() -> swoole_rs_utilities::LoggerIdentity {
    swoole_rs_utilities::LoggerIdentity {
        worker_id: swoole_get_worker_id().cast_signed(),
        worker_pid: swoole_get_worker_pid(),
        worker_symbol: swoole_get_worker_symbol(),
    }
}

/// The process logger. `SwooleG.logger` points here, as at a `swoole::Logger`,
/// whose only member is its logger.
#[cfg(feature = "upstream-symbols")]
static LOGGER: std::sync::atomic::AtomicPtr<swoole_rs_utilities::Logger> =
    std::sync::atomic::AtomicPtr::new(ptr::null_mut());

/// The process logger's `new Logger()` on `SwooleG.stdout`, `->reopen()`, `->close()`
/// and `delete` (corebase's `LOGGER_*`), which `swoole_init` registers for corebase's
/// lifecycles.
#[cfg(feature = "upstream-symbols")]
pub fn logger_op(op: c_int) {
    use std::sync::atomic::Ordering::Relaxed;
    use swoole_rs_corebase::native::{LOGGER_CLOSE, LOGGER_NEW, LOGGER_REOPEN};
    use swoole_rs_utilities::{Logger, process_logger};
    match op {
        LOGGER_NEW => {
            let stdout = swoole_rs_corebase::swr_v1_core_stdout().cast();
            let logger = Logger::new(stdout, swr_logger_host);
            LOGGER.store(Box::into_raw(Box::new(logger)), Relaxed);
            swoole_rs_corebase::swr_v1_core_set_logger(LOGGER.as_ptr().cast());
        }
        LOGGER_REOPEN => process_logger().into_iter().for_each(Logger::reopen),
        LOGGER_CLOSE => process_logger().into_iter().for_each(Logger::close),
        _ => {
            swoole_rs_corebase::swr_v1_core_set_logger(ptr::null_mut());
            let logger = LOGGER.swap(ptr::null_mut(), Relaxed);
            if !logger.is_null() {
                // SAFETY: the logger LOGGER_NEW made, taken once.
                drop(unsafe { Box::from_raw(logger) });
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pool::{
        pool_ipc_address, pool_ipc_free, pool_ipc_new, swr_v1_process_pool_ipc_listener,
    };
    use std::sync::Mutex;
    use std::thread::{self, ThreadId};

    /// A log line: kind, level, code, origin, message and the logging thread,
    /// so a test sees only its own logs while others run in parallel.
    type Logged = (c_int, c_int, c_int, String, String, ThreadId);

    static LOGGED: Mutex<Vec<Logged>> = Mutex::new(Vec::new());

    fn record(kind: c_int, level: c_int, code: c_int, origin: &CStr, message: &CStr) {
        let text = |s: &CStr| s.to_string_lossy().into_owned();
        if let Ok(mut logged) = LOGGED.lock() {
            logged.push((
                kind,
                level,
                code,
                text(origin),
                text(message),
                thread::current().id(),
            ));
        }
    }

    /// What `origin` logged on this thread: (kind, level, code, message).
    pub(crate) fn logged(origin: &str) -> Vec<(c_int, c_int, c_int, String)> {
        LOGGED
            .lock()
            .map(|logged| {
                logged
                    .iter()
                    .filter(|entry| entry.3 == origin && entry.5 == thread::current().id())
                    .map(|(kind, level, code, _, message, _)| {
                        (*kind, *level, *code, message.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Route this binary's logs to [`record`].
    pub(crate) fn install() {
        swoole_rs_corebase::log::swr_v1_core_set_log_host(record);
    }

    static LISTENER: u8 = 0;

    fn make(kind: c_int, port: c_int) -> *mut c_void {
        if kind == 1 && port == 9501 {
            (&raw const LISTENER).cast_mut().cast()
        } else {
            ptr::null_mut()
        }
    }

    fn keep(_: *mut c_void) {}

    #[test]
    fn listen_needs_a_socket_pool_and_keeps_its_listener() {
        install();
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        pool.ipc = pool_ipc_new(true);
        pool.ipc_mode = crate::process_pool::IpcMode::UnixSock;
        let refused = listen(&pool, c"127.0.0.1", 9501, || make(1, 9501));
        assert_eq!(
            refused, -1,
            "an SW_IPC_UNIXSOCK pool has no stream listener"
        );
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
        pool.ipc_mode = crate::process_pool::IpcMode::Socket;
        let failed = listen(&pool, c"127.0.0.1", 9502, || make(1, 9502));
        assert_eq!(failed, -1, "no socket, no listener");
        // SAFETY: the pool's IPC state.
        assert!(
            swr_v1_process_pool_ipc_listener(unsafe { &*pool.ipc }).is_null(),
            "none kept"
        );
        let listened = listen(&pool, c"127.0.0.1", 9501, || make(1, 9501));
        assert_eq!(listened, 0, "listening");
        // SAFETY: the pool's IPC state.
        let listener = swr_v1_process_pool_ipc_listener(unsafe { &*pool.ipc });
        assert_eq!(
            listener.cast_const(),
            (&raw const LISTENER).cast(),
            "the socket is kept"
        );
        // SAFETY: the pool's IPC state.
        let address = pool_ipc_address(unsafe { &*pool.ipc });
        assert_eq!(address, Some((c"127.0.0.1", 9501)), "and its address");
        // SAFETY: the pool's IPC state, not used again.
        pool_ipc_free(*unsafe { Box::from_raw(pool.ipc) }, keep);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
    fn pop_message_takes_the_oldest_message_from_the_box() {
        use swoole_rs_memory::channel::{
            Record, swr_v1_memory_channel_bytes, swr_v1_memory_channel_init,
        };
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        let mut out = [0u8; 16];
        let none = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(none, -1, "no box");
        let mut memory = vec![0u64; swr_v1_memory_channel_bytes(256, 16).div_ceil(8)];
        let record = memory.as_mut_ptr().cast::<Record>();
        // SAFETY: the buffer is aligned and as long as the record needs; 2 is SW_CHAN_LOCK.
        assert_eq!(
            unsafe { swr_v1_memory_channel_init(record, 256, 16, 2) },
            0,
            "a box"
        );
        pool.message_box = record.cast();
        for message in [&b"first"[..], b"second"] {
            // SAFETY: the record made above.
            assert_eq!(unsafe { &*record }.push(message), 0, "pushed");
        }
        let n = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(
            &out[..usize::try_from(n).unwrap_or(0)],
            b"first",
            "oldest first"
        );
        let n = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(
            &out[..usize::try_from(n).unwrap_or(0)],
            b"second",
            "then the next"
        );
        let n = pop_message(PoolRef::from(&mut pool), &mut out);
        assert_eq!(n, -1, "then empty");
    }

    #[test]
    fn a_response_needs_a_connection() {
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        pool.ipc = pool_ipc_new(true);
        assert_eq!(
            response(PoolRef::from(&mut pool), b"ok"),
            -1,
            "no connection"
        );
        assert_eq!(
            swoole_rs_corebase::log::last_error(),
            505,
            "SW_ERROR_INVALID_PARAMS"
        );
        // SAFETY: the pool's IPC state, not used again.
        pool_ipc_free(*unsafe { Box::from_raw(pool.ipc) }, keep);
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
        // SAFETY: a live record.
        unsafe { Worker::new(&raw mut worker).set_id(4) };
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
