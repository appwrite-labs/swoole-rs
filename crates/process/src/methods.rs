//! The `ProcessPool` and `Worker` methods (once `src/os/process_pool.cc`) that are
//! whole Rust operations. Each runs over crates/server's [`PoolNative`] side as its
//! C++ forward did.

use crate::worker_record::Worker;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::pool::{
    raise_bailout, swr_v1_process_pool_ipc_address, swr_v1_process_pool_ipc_respond,
    swr_v1_process_pool_ipc_set_listener, swr_v1_process_worker_send,
};
use crate::process_pool::{
    PoolNative, PoolRecord, PoolRef, RAISE, RUN_CREATE_MESSAGE_BOX, RUN_CREATE_MESSAGE_BUS,
    RUN_DESTROY, RUN_DETACH, RUN_DROP_RELOAD_TASK, RUN_KILL_ALL, RUN_PUSH_MESSAGE, RUN_RELOAD,
    RUN_REOPEN_LOGGER, RUN_SET_MAX_REQUEST, RUN_SET_PROTOCOL, RUN_SET_START_ID, RUN_SET_TYPE,
    RUN_SHUTDOWN, RUN_SIGNAL, RUN_SPAWN, RUN_START, RUN_START_CHECK, RUN_STOP, RUN_WAIT,
    pool_run_op, swr_v1_process_pool_create, swr_v1_process_pool_dispatch_event,
    swr_v1_process_pool_push, swr_v1_worker_symbol,
};
use crate::reload_task::{WARN_KILL_ALL_FAILED, WARN_KILL_ONE_FAILED};
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
        // SAFETY: crates/server's, in the same archive; declared, since crates/server
        // depends on this crate.
        unsafe extern "Rust" {
            safe fn swr_v1_process_pool_native() -> &'static dyn PoolNative;
        }
        swr_v1_process_pool_native()
    }
    #[cfg(not(all(feature = "upstream-symbols", not(test))))]
    std::process::abort()
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the C++ method's (uint32_t) conversion of the export's result"
    )]
    fn narrow(value: i64) -> u32 {
        value as u32
    }
}

impl Narrow for *mut core::ffi::c_void {
    fn narrow(value: i64) -> Self {
        core::ptr::with_exposed_provenance_mut(usize::try_from(value).unwrap_or(0))
    }
}

impl Narrow for isize {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ method's (ssize_t) conversion of the export's result"
    )]
    fn narrow(value: i64) -> isize {
        value as isize
    }
}

impl Narrow for c_int {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ method's (int) conversion of the export's result"
    )]
    fn narrow(value: i64) -> c_int {
        value as c_int
    }
}

/// One `RUN_*` operation; a callback's bailout is raised once the export's
/// frames are gone. This frame holds nothing, so the `longjmp` may cross it.
///
/// # Safety
/// `worker` is null or one of the pool's workers.
unsafe fn run_on<R: Narrow>(pool: PoolRef<'_>, worker: *mut WorkerRecord, op: u32, arg: u64) -> R {
    // SAFETY: a live pool, and the caller's worker; the side is native's.
    let result = unsafe { pool_run_op(native(), pool.raw(), worker, op, arg) };
    if result == RAISE {
        raise_bailout();
    }
    R::narrow(result)
}

/// [`run_on`] the pool itself.
fn run<R: Narrow>(pool: PoolRef<'_>, op: u32, arg: u64) -> R {
    // SAFETY: no worker.
    unsafe { run_on(pool, ptr::null_mut(), op, arg) }
}

/// Each `ProcessPool` method that is one `RUN_*` operation on the pool alone: its
/// name, parameters, return type, and the operation's argument.
macro_rules! run_methods {
    ($(fn $name:ident($($p:ident: $t:ty),*) -> $r:ty = $op:ident($arg:expr);)*) => {$(
        #[doc = concat!("`ProcessPool::", stringify!($name), "`.")]
        pub fn $name(pool: PoolRef<'_>, $($p: $t),*) -> $r {
            run(pool, $op, $arg)
        }
    )*};
}

run_methods! {
    fn create_message_box(size: usize) -> c_int = RUN_CREATE_MESSAGE_BOX(size as u64);
    fn create_message_bus() -> c_int = RUN_CREATE_MESSAGE_BUS(0);
    fn set_protocol(protocol: c_int) -> () = RUN_SET_PROTOCOL(i64::from(protocol).cast_unsigned());
    fn start_check() -> c_int = RUN_START_CHECK(0);
    fn start() -> c_int = RUN_START(0);
    fn drop_reload_task() -> () = RUN_DROP_RELOAD_TASK(0);
    fn reload() -> bool = RUN_RELOAD(0);
    fn reopen_logger() -> () = RUN_REOPEN_LOGGER(0);
    fn kill_all_workers(signo: c_int) -> () = RUN_KILL_ALL(i64::from(signo).cast_unsigned());
    fn shutdown() -> bool = RUN_SHUTDOWN(0);
    fn set_max_request(max: u32, grace: u32) -> () = RUN_SET_MAX_REQUEST(u64::from(max) | (u64::from(grace) << 32));
    fn set_type(kind: c_int) -> () = RUN_SET_TYPE(u64::from(kind.cast_unsigned() & 0xff));
    fn set_start_id(id: c_int) -> () = RUN_SET_START_ID(u64::from(id.cast_unsigned() & 0xffff));
    fn detach() -> bool = RUN_DETACH(0);
    fn wait() -> c_int = RUN_WAIT(0);
    fn destroy() -> () = RUN_DESTROY(0);
}

/// `ProcessPool::push_message(event)`: the `EventData` at `event`.
///
/// # Safety
/// `event` is an `EventData`.
#[must_use]
pub unsafe fn push_event(pool: PoolRef<'_>, event: *const c_void) -> c_int {
    // SAFETY: no worker; the caller's event.
    unsafe { run_on(pool, ptr::null_mut(), RUN_PUSH_MESSAGE, event.addr() as u64) }
}

/// `ProcessPool::stop(worker)`.
///
/// # Safety
/// `worker` is one of the pool's workers.
pub unsafe fn stop(pool: PoolRef<'_>, worker: *mut WorkerRecord) {
    // SAFETY: as the caller guarantees.
    unsafe { run_on(pool, worker, RUN_STOP, 0) }
}

/// `ProcessPool::signal(signo, worker)`.
///
/// # Safety
/// `worker` is null or one of the pool's workers.
pub unsafe fn signal(pool: PoolRef<'_>, signo: c_int, worker: *mut WorkerRecord) {
    // SAFETY: as the caller guarantees.
    unsafe { run_on(pool, worker, RUN_SIGNAL, i64::from(signo).cast_unsigned()) }
}

/// `ProcessPool::spawn(worker)`.
///
/// # Safety
/// `worker` is one of the pool's workers.
pub unsafe fn spawn(pool: PoolRef<'_>, worker: *mut WorkerRecord) -> c_int {
    // SAFETY: as the caller guarantees.
    unsafe { run_on(pool, worker, RUN_SPAWN, 0) }
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
///
/// # Safety
/// `pool` is a live `ProcessPool`; `data` holds `length` bytes.
pub unsafe fn push_message(
    pool: *mut PoolRecord,
    kind: u8,
    data: *const u8,
    length: usize,
) -> c_int {
    // SAFETY: as the caller guarantees; the side is native's.
    c_int::narrow(unsafe { swr_v1_process_pool_push(native(), pool, kind, data, length) })
}

/// `ProcessPool::dispatch(data, dst_worker_id)`, or `dispatch_sync` when `blocking`.
///
/// # Safety
/// `pool` is a live `ProcessPool`; `event` an `EventData`; `worker_id` writable.
pub unsafe fn dispatch(
    pool: *mut PoolRecord,
    event: *mut u8,
    worker_id: *mut c_int,
    blocking: bool,
) -> c_int {
    // SAFETY: as the caller guarantees; the side is native's.
    c_int::narrow(unsafe {
        swr_v1_process_pool_dispatch_event(native(), pool, event, worker_id, blocking)
    })
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
pub fn is_worker_running(pool: &PoolRecord, worker: Worker<'_>) -> bool {
    pool.running && swr_v1_worker_global_get(4) == 0 && !worker.exceeded_max_request()
}

/// `Worker::send_pipe_message(buf, n, flags)`: through its pool's queue or the pipe
/// end `flags` names.
///
/// # Safety
/// `worker` is a live worker of a live pool; `data` holds `length` bytes.
#[must_use]
pub unsafe fn send_pipe_message(
    worker: *const WorkerRecord,
    data: *const u8,
    length: usize,
    flags: c_int,
) -> isize {
    /// `SW_PIPE_MASTER`.
    const PIPE_MASTER: c_int = 1;
    // SAFETY: the caller lends a live worker.
    let worker = unsafe { Worker::new(worker) };
    let pool = worker.pool();
    let id = worker.id();
    let pipe = if flags & PIPE_MASTER != 0 {
        worker.pipe_master()
    } else {
        worker.pipe_worker()
    };
    // SAFETY: the worker's pool is live.
    let queue = unsafe { (*pool.cast::<PoolRecord>()).use_msgqueue } != 0;
    // SAFETY: as the caller guarantees.
    unsafe { swr_v1_process_worker_send(native(), pool, pipe, queue, id, data, length, flags) }
}

/// `make_server_socket(type, host, port, backlog)`: a native socket, or null.
pub type MakeServerSocket<T> = unsafe extern "C-unwind" fn(
    kind: c_int,
    host: *const c_char,
    port: c_int,
    backlog: c_int,
) -> *mut T;

#[cfg(feature = "upstream-symbols")]
// SAFETY: crates/tls defines it under its upstream symbol; declared, since
// crates/tls (through crates/reactor) depends on this crate.
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole18make_server_socketE12swSocketTypePKcii"]
    fn make_server_socket(
        kind: c_int,
        host: *const c_char,
        port: c_int,
        backlog: c_int,
    ) -> *mut c_void;
}

/// `ProcessPool::listen(socket_file, backlog)`: a unix stream listener.
///
/// # Safety
/// `pool` is a created `ProcessPool`; `file` a NUL-terminated path.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub unsafe fn listen_unix(pool: *const PoolRecord, file: *const c_char, backlog: c_int) -> c_int {
    // SAFETY: as the caller guarantees; 5 is SW_SOCK_UNIX_STREAM.
    unsafe { listen(pool, 5, file, 0, backlog, make_server_socket) }
}

/// `ProcessPool::listen(host, port, backlog)`: a TCP listener.
///
/// # Safety
/// `pool` is a created `ProcessPool`; `host` a NUL-terminated address.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub unsafe fn listen_tcp(
    pool: *const PoolRecord,
    host: *const c_char,
    port: c_int,
    backlog: c_int,
) -> c_int {
    // SAFETY: as the caller guarantees; 1 is SW_SOCK_TCP.
    unsafe { listen(pool, 1, host, port, backlog, make_server_socket) }
}

/// Both `listen`s, over `make`: only an `SW_IPC_SOCKET` pool has a stream
/// listener, which it records with its address.
///
/// # Safety
/// `pool` is a created `ProcessPool`; `host` a NUL-terminated string; `make`
/// returns a native socket or null.
#[must_use]
pub unsafe fn listen<T>(
    pool: *const PoolRecord,
    kind: c_int,
    host: *const c_char,
    port: c_int,
    backlog: c_int,
    make: MakeServerSocket<T>,
) -> c_int {
    /// `SW_IPC_SOCKET`.
    const IPC_SOCKET: u8 = 3;
    // SAFETY: the caller lends a live pool.
    let pool = unsafe { &*pool };
    if pool.ipc_mode != IPC_SOCKET {
        // SW_LOG_WARNING, SW_ERROR_OPERATION_NOT_SUPPORT.
        swoole_rs_corebase::log::error_log(
            4,
            507,
            c"ProcessPool::listen",
            format_args!("not support, ipc_mode must be SW_IPC_SOCKET"),
        );
        return -1;
    }
    // SAFETY: a socket pool's IPC state, which nothing else borrows meanwhile.
    let ipc = unsafe { &mut *pool.ipc };
    // SAFETY: the caller's string.
    swr_v1_process_pool_ipc_address(ipc, unsafe { CStr::from_ptr(host) }, port);
    // SAFETY: as the caller guarantees.
    let socket = unsafe { make(kind, host, port, backlog) };
    if socket.is_null() {
        return -1;
    }
    // The pool now owns the listener.
    swr_v1_process_pool_ipc_set_listener(ipc, socket.cast());
    0
}

/// `ProcessPool::response(data, length)`: append to the stream connection's reply.
#[must_use]
pub fn response(pool: &PoolRecord, data: &[u8]) -> c_int {
    // SAFETY: null, or the pool's IPC state, which nothing else borrows meanwhile.
    let ipc = unsafe { pool.ipc.as_mut() };
    if ipc.is_some_and(|ipc| swr_v1_process_pool_ipc_respond(ipc, data)) {
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
pub fn send_message(pool: &PoolRecord, worker_id: u32, message: &[u8]) -> bool {
    use swoole_rs_corebase::DataHead;
    // SAFETY: crates/bus' write and crates/tls's; declared, since both depend on
    // this crate.
    unsafe extern "C" {
        fn swr_v1_bus_write(
            bus: *mut c_void,
            socket: *mut c_void,
            head: *mut DataHead,
            data: *const c_char,
        ) -> bool;
        #[link_name = "_ZN6swoole7network6Socket10send_asyncEPKvm"]
        fn send_async(socket: *mut c_void, data: *const c_void, length: usize) -> isize;
    }
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
        // SAFETY: the worker's pipe and the caller's bytes.
        return unsafe { send_async(pipe, message.as_ptr().cast(), length) } != 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "DataHead's reactor_id and len are upstream's int16_t and uint32_t"
    )]
    let mut head = DataHead {
        reactor_id: swoole_get_worker_id() as i16,
        len: length as u32,
        ..DataHead::default()
    };
    // SAFETY: `MessageBus` starts with its `swr_bus *`.
    let bus = unsafe { *pool.message_bus.cast::<*mut c_void>() };
    // SAFETY: the pool's bus, the worker's pipe and a header over the caller's bytes.
    unsafe { swr_v1_bus_write(bus, pipe, &raw mut head, message.as_ptr().cast()) }
}

/// `ProcessPool::pop_message(data, size)`: the next message in the master's box,
/// under the box's lock.
///
/// The pool's box is the one `create` made with `SW_CHAN_LOCK`.
#[must_use]
pub fn pop_message(pool: &PoolRecord, data: &mut [u8]) -> c_int {
    // SAFETY: null, or the pool's box: a live channel record whose mutex serializes
    // its users.
    let Some(message_box) = (unsafe {
        pool.message_box
            .cast::<swoole_rs_memory::channel::Record>()
            .as_ref()
    }) else {
        return -1;
    };
    message_box.pop(data, true)
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
pub extern "C" fn reload_warn(kind: c_int, pid: c_int, worker_id: u32) {
    use swoole_rs_corebase::log::{sys_warning, warning};
    match kind {
        WARN_KILL_ONE_FAILED => sys_warning(
            c"ReloadTask::kill_one",
            format_args!("kill({pid}, SIGTERM) [{worker_id}] failed"),
        ),
        WARN_KILL_ALL_FAILED => sys_warning(
            c"ReloadTask::kill_all",
            format_args!("failed to kill({pid}, SIGTERM) worker#[{worker_id}]"),
        ),
        _ => warning(
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

    unsafe extern "C-unwind" fn make(
        kind: c_int,
        _: *const c_char,
        port: c_int,
        _: c_int,
    ) -> *mut c_void {
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
        pool.ipc_mode = 1;
        // SAFETY: the pool is live and the host a NUL-terminated string.
        let refused = unsafe { listen(&raw const pool, 1, c"127.0.0.1".as_ptr(), 9501, 8, make) };
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
        pool.ipc_mode = 3;
        // SAFETY: as above.
        let failed = unsafe { listen(&raw const pool, 1, c"127.0.0.1".as_ptr(), 9502, 8, make) };
        assert_eq!(failed, -1, "no socket, no listener");
        // SAFETY: the pool's IPC state.
        assert!(
            swr_v1_process_pool_ipc_listener(unsafe { &*pool.ipc }).is_null(),
            "none kept"
        );
        // SAFETY: as above.
        let listened = unsafe { listen(&raw const pool, 1, c"127.0.0.1".as_ptr(), 9501, 8, make) };
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
        unsafe { pool_ipc_free(pool.ipc, keep) };
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
        let none = pop_message(&pool, &mut out);
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
        let n = pop_message(&pool, &mut out);
        assert_eq!(
            &out[..usize::try_from(n).unwrap_or(0)],
            b"first",
            "oldest first"
        );
        let n = pop_message(&pool, &mut out);
        assert_eq!(
            &out[..usize::try_from(n).unwrap_or(0)],
            b"second",
            "then the next"
        );
        let n = pop_message(&pool, &mut out);
        assert_eq!(n, -1, "then empty");
    }

    #[test]
    fn a_response_needs_a_connection() {
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        pool.ipc = pool_ipc_new(true);
        assert_eq!(response(&pool, b"ok"), -1, "no connection");
        assert_eq!(
            swoole_rs_corebase::log::last_error(),
            505,
            "SW_ERROR_INVALID_PARAMS"
        );
        // SAFETY: the pool's IPC state, not used again.
        unsafe { pool_ipc_free(pool.ipc, keep) };
    }

    #[test]
    fn reload_kills_warn_under_upstream_reload_task() {
        install();
        reload_warn(WARN_KILL_ALL_FAILED, 10, 1);
        reload_warn(2, 11, 2);
        reload_warn(WARN_KILL_ONE_FAILED, 12, 3);
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
