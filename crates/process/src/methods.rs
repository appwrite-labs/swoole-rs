//! The `ProcessPool` and `Worker` methods (once `src/os/process_pool.cc`) that are
//! whole Rust operations, defined under their upstream (Itanium) symbols when
//! the hybrid links this crate (`upstream-symbols`). Each runs over crates/server's
//! [`PoolNative`] side as its C++ forward did.

use crate::worker_record::Worker;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::pool::{
    raise_bailout, swr_v1_process_pool_ipc_address, swr_v1_process_pool_ipc_respond,
    swr_v1_process_pool_ipc_set_listener, swr_v1_process_worker_send,
};
use crate::process_pool::{
    PoolNative, PoolRecord, RAISE, RUN_CREATE_MESSAGE_BOX, RUN_CREATE_MESSAGE_BUS, RUN_DESTROY,
    RUN_DETACH, RUN_DROP_RELOAD_TASK, RUN_KILL_ALL, RUN_PUSH_MESSAGE, RUN_RELOAD,
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
/// `pool` is a live `ProcessPool` and `worker` null or one of its workers.
unsafe fn run<R: Narrow>(pool: *mut PoolRecord, worker: *mut WorkerRecord, op: u32, arg: u64) -> R {
    // SAFETY: as the caller guarantees; the side is native's.
    let result = unsafe { pool_run_op(native(), pool, worker, op, arg) };
    if result == RAISE {
        raise_bailout();
    }
    R::narrow(result)
}

/// Each `ProcessPool` method that is one `RUN_*` operation: its symbol if something links
/// it by name, Rust name, parameters, return type, and the operation's worker and argument.
macro_rules! run_methods {
    ($($($symbol:literal)? fn $name:ident($($p:ident: $t:ty),*) -> $r:ty = $op:ident($worker:expr, $arg:expr);)*) => {$(
        #[doc = concat!("`ProcessPool::", stringify!($name), "`.")]
        ///
        /// # Safety
        /// `pool` is a live `ProcessPool`; a `worker` is one of its workers.
        $(#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))])?
        pub unsafe extern "C" fn $name(pool: *mut PoolRecord, $($p: $t),*) -> $r {
            // SAFETY: as the caller guarantees.
            unsafe { run(pool, $worker, $op, $arg) }
        }
    )*};
}

run_methods! {
    fn create_message_box(size: usize) -> c_int = RUN_CREATE_MESSAGE_BOX(ptr::null_mut(), size as u64);
    "_ZN6swoole11ProcessPool18create_message_busEv"
    fn create_message_bus() -> c_int = RUN_CREATE_MESSAGE_BUS(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool12set_protocolENS_12ProtocolTypeE"
    fn set_protocol(protocol: c_int) -> () = RUN_SET_PROTOCOL(ptr::null_mut(), i64::from(protocol).cast_unsigned());
    fn start_check() -> c_int = RUN_START_CHECK(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool5startEv"
    fn start() -> c_int = RUN_START(ptr::null_mut(), 0);
    fn push_event(event: *const c_void) -> c_int = RUN_PUSH_MESSAGE(ptr::null_mut(), event.addr() as u64);
    fn drop_reload_task() -> () = RUN_DROP_RELOAD_TASK(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool6reloadEv"
    fn reload() -> bool = RUN_RELOAD(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool4stopEPNS_6WorkerE"
    fn stop(worker: *mut WorkerRecord) -> () = RUN_STOP(worker, 0);
    "_ZN6swoole11ProcessPool13reopen_loggerEv"
    fn reopen_logger() -> () = RUN_REOPEN_LOGGER(ptr::null_mut(), 0);
    fn signal(signo: c_int, worker: *mut WorkerRecord) -> () = RUN_SIGNAL(worker, i64::from(signo).cast_unsigned());
    fn kill_all_workers(signo: c_int) -> () = RUN_KILL_ALL(ptr::null_mut(), i64::from(signo).cast_unsigned());
    "_ZN6swoole11ProcessPool8shutdownEv"
    fn shutdown() -> bool = RUN_SHUTDOWN(ptr::null_mut(), 0);
    fn spawn(worker: *mut WorkerRecord) -> c_int = RUN_SPAWN(worker, 0);
    fn set_max_request(max: u32, grace: u32) -> () = RUN_SET_MAX_REQUEST(ptr::null_mut(), u64::from(max) | (u64::from(grace) << 32));
    fn set_type(kind: c_int) -> () = RUN_SET_TYPE(ptr::null_mut(), u64::from(kind.cast_unsigned() & 0xff));
    fn set_start_id(id: c_int) -> () = RUN_SET_START_ID(ptr::null_mut(), u64::from(id.cast_unsigned() & 0xffff));
    "_ZN6swoole11ProcessPool6detachEv"
    fn detach() -> bool = RUN_DETACH(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool4waitEv"
    fn wait() -> c_int = RUN_WAIT(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool7destroyEv"
    fn destroy() -> () = RUN_DESTROY(ptr::null_mut(), 0);
}

/// `ProcessPool::create(worker_num, msgqueue_key, ipc_mode)`.
///
/// # Safety
/// `pool` is a zeroed or reset `ProcessPool`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11ProcessPool6createEji9swIPCMode")
)]
pub unsafe extern "C" fn create(
    pool: *mut PoolRecord,
    worker_num: u32,
    key: c_int,
    mode: c_int,
) -> c_int {
    // SAFETY: as the caller guarantees; the side is native's.
    let result = unsafe {
        swr_v1_process_pool_create(
            native(),
            pool,
            worker_num,
            key,
            u8::try_from(mode.cast_unsigned() & 0xff).unwrap_or_default(),
        )
    };
    c_int::narrow(result)
}

/// `ProcessPool::push_message(type, data, length)`.
///
/// # Safety
/// `pool` is a live `ProcessPool`; `data` holds `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool12push_messageEhPKvm")
)]
pub unsafe extern "C" fn push_message(
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

/// `ProcessPool::dispatch_sync(data, dst_worker_id)`.
///
/// # Safety
/// As [`dispatch`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11ProcessPool13dispatch_syncEPNS_9EventDataEPi")
)]
pub unsafe extern "C" fn dispatch_sync(
    pool: *mut PoolRecord,
    event: *mut u8,
    worker_id: *mut c_int,
) -> c_int {
    // SAFETY: as the caller guarantees; the side is native's.
    c_int::narrow(unsafe {
        swr_v1_process_pool_dispatch_event(native(), pool, event, worker_id, true)
    })
}

/// `ProcessPool::add_worker(worker)`: keeps an existing entry for the pid.
///
/// # Safety
/// `pool` is created.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool10add_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn add_worker(pool: &PoolRecord, worker: Worker<'_>) {
    // SAFETY: a created pool's registry is live.
    unsafe { (*pool.registry).insert(worker.pid(), worker.raw().cast(), 0) };
}

/// `ProcessPool::del_worker(worker)`: whether the worker's pid was registered.
///
/// # Safety
/// As [`add_worker`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool10del_workerEPKNS_6WorkerE")
)]
#[must_use]
pub unsafe extern "C" fn del_worker(pool: &PoolRecord, worker: Worker<'_>) -> bool {
    // SAFETY: a created pool's registry is live.
    unsafe { (*pool.registry).unregister(worker.pid()) }
}

/// `ProcessPool::get_worker_by_pid(pid)`: null before `create` or for an unknown pid.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool17get_worker_by_pidEi")
)]
#[must_use]
pub extern "C" fn get_worker_by_pid(pool: &PoolRecord, pid: c_int) -> *mut WorkerRecord {
    let registry = pool.registry;
    if registry.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: a pool's non-null registry is the live one `create` made.
    unsafe { (*registry).lookup(pid) }.cast()
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Worker17send_pipe_messageEPKvmi")
)]
#[must_use]
pub unsafe extern "C" fn send_pipe_message(
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
#[unsafe(export_name = "_ZNK6swoole11ProcessPool6listenEPKci")]
#[must_use]
pub unsafe extern "C" fn listen_unix(
    pool: *const PoolRecord,
    file: *const c_char,
    backlog: c_int,
) -> c_int {
    // SAFETY: as the caller guarantees; 5 is SW_SOCK_UNIX_STREAM.
    unsafe { listen(pool, 5, file, 0, backlog, make_server_socket) }
}

/// `ProcessPool::listen(host, port, backlog)`: a TCP listener.
///
/// # Safety
/// `pool` is a created `ProcessPool`; `host` a NUL-terminated address.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZNK6swoole11ProcessPool6listenEPKcii")]
#[must_use]
pub unsafe extern "C" fn listen_tcp(
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
    // SAFETY: a socket pool's IPC state; the caller's string.
    unsafe { swr_v1_process_pool_ipc_address(pool.ipc, host, port) };
    // SAFETY: as the caller guarantees.
    let socket = unsafe { make(kind, host, port, backlog) };
    if socket.is_null() {
        return -1;
    }
    // SAFETY: a socket pool's IPC state, which nothing else borrows meanwhile; the pool
    // now owns the listener.
    swr_v1_process_pool_ipc_set_listener(unsafe { &mut *pool.ipc }, socket.cast());
    0
}

/// `ProcessPool::response(data, length)`: append to the stream connection's reply.
///
/// # Safety
/// `pool` is a created `ProcessPool`; `data` is null or holds `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool8responseEPKcj")
)]
#[must_use]
pub unsafe extern "C" fn response(pool: *const PoolRecord, data: *const u8, length: u32) -> c_int {
    // SAFETY: the caller lends a live pool.
    let ipc = unsafe { (*pool).ipc };
    // SAFETY: its IPC state and the caller's bytes.
    if unsafe { swr_v1_process_pool_ipc_respond(ipc, data, length) } {
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
/// # Safety
/// `pool` is a created `ProcessPool` with a worker `worker_id`; `message` holds
/// `length` bytes.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZNK6swoole11ProcessPool12send_messageEjPKcm")]
#[must_use]
pub unsafe extern "C" fn send_message(
    pool: *const PoolRecord,
    worker_id: u32,
    message: *const c_char,
    length: usize,
) -> bool {
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
    // SAFETY: the caller lends a live pool.
    let pool = unsafe { &*pool };
    // `get_worker`: the pool's record for `worker_id`.
    let index = worker_id.wrapping_sub(u32::from(pool.start_id)) as usize;
    // SAFETY: as the caller guarantees.
    let pipe = unsafe { Worker::new(pool.workers.wrapping_add(index)).pipe_master() };
    if pool.message_bus.is_null() {
        // SAFETY: the worker's pipe and the caller's bytes.
        return unsafe { send_async(pipe, message.cast(), length) } != 0;
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
    unsafe { swr_v1_bus_write(bus, pipe, &raw mut head, message) }
}

/// `ProcessPool::pop_message(data, size)`: the next message in the master's box,
/// under the box's lock.
///
/// # Safety
/// `pool` is a created `ProcessPool` whose box, if any, was made with
/// `SW_CHAN_LOCK`; `data` is writable for `size` bytes.
#[must_use]
pub unsafe fn pop_message(pool: *const PoolRecord, data: *mut c_void, size: usize) -> c_int {
    use swoole_rs_memory::channel::{swr_v1_memory_channel_lock, swr_v1_memory_channel_out};
    // SAFETY: the caller lends a live pool.
    let message_box = unsafe { (*pool).message_box };
    if message_box.is_null() {
        return -1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Channel::pop takes upstream's int length"
    )]
    let size = size as c_int;
    // SAFETY: the box is a live channel record with its mutex.
    unsafe { swr_v1_memory_channel_lock(message_box.cast(), true) };
    // SAFETY: the locked channel (its record's first field) and the caller's buffer.
    let length = unsafe { swr_v1_memory_channel_out(message_box.cast(), data, size, 1) };
    // SAFETY: this thread holds the lock.
    unsafe { swr_v1_memory_channel_lock(message_box.cast(), false) };
    length
}

/// `Worker::set_max_request(max_request, grace)`: this process's request limit.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker15set_max_requestEjj")
)]
pub extern "C" fn worker_set_max_request(_worker: *mut WorkerRecord, max_request: u32, grace: u32) {
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
        // SAFETY: SwooleG's message, valid until it next changes.
        unsafe { CStr::from_ptr(swoole_rs_corebase::swr_v1_core_bug_report_message()) }
            .to_string_lossy()
    } else {
        "".into()
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker11is_shutdownEv")
)]
#[must_use]
pub extern "C" fn worker_is_shutdown(_worker: *const WorkerRecord) -> bool {
    swr_v1_worker_global_get(4) != 0
}

/// `Worker::is_running()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker10is_runningEv")
)]
#[must_use]
pub extern "C" fn worker_is_running() -> bool {
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_set_worker_idj")
)]
pub extern "C" fn swoole_set_worker_id(id: u32) {
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
        pool_ipc_free, pool_ipc_new, swr_v1_process_pool_ipc_address_of,
        swr_v1_process_pool_ipc_listener,
    };
    use std::sync::Mutex;
    use std::thread::{self, ThreadId};

    /// A log line: kind, level, code, origin, message and the logging thread,
    /// so a test sees only its own logs while others run in parallel.
    type Logged = (c_int, c_int, c_int, String, String, ThreadId);

    static LOGGED: Mutex<Vec<Logged>> = Mutex::new(Vec::new());

    unsafe extern "C" fn record(
        kind: c_int,
        level: c_int,
        code: c_int,
        origin: *const c_char,
        message: *const c_char,
    ) {
        // SAFETY: the log host's NUL-terminated strings.
        let text = |s| unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
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
        let mut port = 0;
        // SAFETY: the pool's IPC state; `port` is writable.
        let address =
            unsafe { CStr::from_ptr(swr_v1_process_pool_ipc_address_of(pool.ipc, &raw mut port)) };
        assert_eq!((address, port), (c"127.0.0.1", 9501), "and its address");
        // SAFETY: the pool's IPC state, not used again.
        unsafe { pool_ipc_free(pool.ipc, keep) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
    fn pop_message_takes_the_oldest_message_from_the_box() {
        use swoole_rs_memory::channel::{
            swr_v1_memory_channel_bytes, swr_v1_memory_channel_in, swr_v1_memory_channel_init,
        };
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        let mut out = [0u8; 16];
        // SAFETY: the pool is live and `out` writable.
        let none = unsafe { pop_message(&raw const pool, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(none, -1, "no box");
        let mut memory = vec![0u64; swr_v1_memory_channel_bytes(256, 16).div_ceil(8)];
        let record = memory.as_mut_ptr().cast();
        // SAFETY: the buffer is aligned and as long as the record needs; 2 is SW_CHAN_LOCK.
        assert_eq!(
            unsafe { swr_v1_memory_channel_init(record, 256, 16, 2) },
            0,
            "a box"
        );
        pool.message_box = record.cast();
        for message in [&b"first"[..], b"second"] {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "short"
            )]
            // SAFETY: the box's channel is its record's first field.
            let pushed = unsafe {
                swr_v1_memory_channel_in(
                    record.cast(),
                    message.as_ptr().cast(),
                    message.len() as i32,
                )
            };
            assert_eq!(pushed, 0, "pushed");
        }
        // SAFETY: as above.
        let n = unsafe { pop_message(&raw const pool, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(
            &out[..usize::try_from(n).unwrap_or(0)],
            b"first",
            "oldest first"
        );
        // SAFETY: as above.
        let n = unsafe { pop_message(&raw const pool, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(
            &out[..usize::try_from(n).unwrap_or(0)],
            b"second",
            "then the next"
        );
        // SAFETY: as above.
        let n = unsafe { pop_message(&raw const pool, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(n, -1, "then empty");
    }

    #[test]
    fn a_response_needs_a_connection() {
        // SAFETY: a zeroed record is a pool before `create`.
        let mut pool: PoolRecord = unsafe { std::mem::zeroed() };
        pool.ipc = pool_ipc_new(true);
        // SAFETY: the pool is live and the bytes readable.
        assert_eq!(
            unsafe { response(&raw const pool, b"ok".as_ptr(), 2) },
            -1,
            "no connection"
        );
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
            assert!(
                !worker_is_shutdown(ptr::null()),
                "not shut down before init"
            );
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
