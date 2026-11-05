//! The `ProcessPool` and `Worker` methods of `src/os/process_pool.cc` that are
//! whole Rust operations, defined under their upstream (Itanium) symbols when
//! the hybrid links this crate (`upstream-symbols`). Native code registers its
//! [`PoolNative`] table once; each method runs over it as its C++ forward did.

use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::pool::{swr_v1_process_bailout, swr_v1_process_worker_send};
use crate::process_pool::{
    PoolNative, PoolRecord, RAISE, RUN_CREATE_MESSAGE_BOX, RUN_CREATE_MESSAGE_BUS, RUN_DESTROY,
    RUN_DETACH, RUN_DROP_RELOAD_TASK, RUN_KILL_ALL, RUN_PUSH_MESSAGE, RUN_RELOAD,
    RUN_REOPEN_LOGGER, RUN_SET_MAX_REQUEST, RUN_SET_PROTOCOL, RUN_SET_START_ID, RUN_SET_TYPE,
    RUN_SHUTDOWN, RUN_SIGNAL, RUN_SPAWN, RUN_START, RUN_START_CHECK, RUN_STOP, RUN_WAIT,
    swr_v1_process_pool_create, swr_v1_process_pool_dispatch_event, swr_v1_process_pool_push,
    swr_v1_process_pool_run_op, swr_v1_worker_symbol,
};
use crate::worker_globals::{swr_v1_worker_global_get, swr_v1_worker_global_set};
use crate::worker_record::{
    WorkerRecord, pipe_master, pipe_worker, pool_of, swr_v1_worker_exceeded_max_requests,
    swr_v1_worker_id, swr_v1_worker_pid,
};
use crate::worker_registry::{
    swr_v1_process_worker_registry_insert, swr_v1_process_worker_registry_lookup,
    swr_v1_process_worker_registry_unregister,
};

static NATIVE: AtomicPtr<PoolNative> = AtomicPtr::new(ptr::null_mut());

/// Registers native's table, which the methods below lend every export.
///
/// # Safety
/// `native` lives as long as the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_pool_set_native(native: *const PoolNative) {
    NATIVE.store(native.cast_mut(), Ordering::Release);
}

fn native() -> *const PoolNative {
    NATIVE.load(Ordering::Acquire)
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
    // SAFETY: as the caller guarantees; the table is native's.
    let result = unsafe { swr_v1_process_pool_run_op(native(), pool, worker, op, arg) };
    if result == RAISE {
        swr_v1_process_bailout();
    }
    R::narrow(result)
}

/// Each `ProcessPool` method that is one `RUN_*` operation: its symbol, Rust
/// name, parameters, return type, and the operation's worker and argument.
macro_rules! run_methods {
    ($($symbol:literal fn $name:ident($($p:ident: $t:ty),*) -> $r:ty = $op:ident($worker:expr, $arg:expr);)*) => {$(
        #[doc = concat!("`", $symbol, "`.")]
        ///
        /// # Safety
        /// `pool` is a live `ProcessPool`; a `worker` is one of its workers.
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C" fn $name(pool: *mut PoolRecord, $($p: $t),*) -> $r {
            // SAFETY: as the caller guarantees.
            unsafe { run(pool, $worker, $op, $arg) }
        }
    )*};
}

run_methods! {
    "_ZN6swoole11ProcessPool18create_message_boxEm"
    fn create_message_box(size: usize) -> c_int = RUN_CREATE_MESSAGE_BOX(ptr::null_mut(), size as u64);
    "_ZN6swoole11ProcessPool18create_message_busEv"
    fn create_message_bus() -> c_int = RUN_CREATE_MESSAGE_BUS(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool12set_protocolENS_12ProtocolTypeE"
    fn set_protocol(protocol: c_int) -> () = RUN_SET_PROTOCOL(ptr::null_mut(), i64::from(protocol).cast_unsigned());
    "_ZN6swoole11ProcessPool11start_checkEv"
    fn start_check() -> c_int = RUN_START_CHECK(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool5startEv"
    fn start() -> c_int = RUN_START(ptr::null_mut(), 0);
    "_ZNK6swoole11ProcessPool12push_messageEPKNS_9EventDataE"
    fn push_event(event: *const c_void) -> c_int = RUN_PUSH_MESSAGE(ptr::null_mut(), event.addr() as u64);
    "_ZN6swoole11ProcessPool16drop_reload_taskEv"
    fn drop_reload_task() -> () = RUN_DROP_RELOAD_TASK(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool6reloadEv"
    fn reload() -> bool = RUN_RELOAD(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool4stopEPNS_6WorkerE"
    fn stop(worker: *mut WorkerRecord) -> () = RUN_STOP(worker, 0);
    "_ZN6swoole11ProcessPool13reopen_loggerEv"
    fn reopen_logger() -> () = RUN_REOPEN_LOGGER(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool6signalEiPNS_6WorkerE"
    fn signal(signo: c_int, worker: *mut WorkerRecord) -> () = RUN_SIGNAL(worker, i64::from(signo).cast_unsigned());
    "_ZN6swoole11ProcessPool16kill_all_workersEi"
    fn kill_all_workers(signo: c_int) -> () = RUN_KILL_ALL(ptr::null_mut(), i64::from(signo).cast_unsigned());
    "_ZN6swoole11ProcessPool8shutdownEv"
    fn shutdown() -> bool = RUN_SHUTDOWN(ptr::null_mut(), 0);
    "_ZN6swoole11ProcessPool5spawnEPNS_6WorkerE"
    fn spawn(worker: *mut WorkerRecord) -> c_int = RUN_SPAWN(worker, 0);
    "_ZN6swoole11ProcessPool15set_max_requestEjj"
    fn set_max_request(max: u32, grace: u32) -> () = RUN_SET_MAX_REQUEST(ptr::null_mut(), u64::from(max) | (u64::from(grace) << 32));
    "_ZN6swoole11ProcessPool8set_typeEi"
    fn set_type(kind: c_int) -> () = RUN_SET_TYPE(ptr::null_mut(), u64::from(kind.cast_unsigned() & 0xff));
    "_ZN6swoole11ProcessPool12set_start_idEi"
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
    // SAFETY: as the caller guarantees; the table is native's.
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
    // SAFETY: as the caller guarantees; the table is native's.
    c_int::narrow(unsafe { swr_v1_process_pool_push(native(), pool, kind, data, length) })
}

/// `ProcessPool::dispatch(data, dst_worker_id)`.
///
/// # Safety
/// `pool` is a live `ProcessPool`; `event` an `EventData`; `worker_id` writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11ProcessPool8dispatchEPNS_9EventDataEPi")
)]
pub unsafe extern "C" fn dispatch(
    pool: *mut PoolRecord,
    event: *mut u8,
    worker_id: *mut c_int,
) -> c_int {
    // SAFETY: as the caller guarantees; the table is native's.
    c_int::narrow(unsafe {
        swr_v1_process_pool_dispatch_event(native(), pool, event, worker_id, false)
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
    // SAFETY: as the caller guarantees; the table is native's.
    c_int::narrow(unsafe {
        swr_v1_process_pool_dispatch_event(native(), pool, event, worker_id, true)
    })
}

/// `ProcessPool::add_worker(worker)`: keeps an existing entry for the pid.
///
/// # Safety
/// `pool` is a live, created `ProcessPool`; `worker` a live worker.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool10add_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn add_worker(pool: *const PoolRecord, worker: *mut WorkerRecord) {
    // SAFETY: the caller lends a live pool.
    let registry = unsafe { (*pool).registry };
    // SAFETY: the caller lends a live worker.
    let pid = unsafe { swr_v1_worker_pid(worker) };
    // SAFETY: a created pool's registry is live.
    unsafe { swr_v1_process_worker_registry_insert(registry, pid, worker.cast(), 0) };
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
pub unsafe extern "C" fn del_worker(pool: *const PoolRecord, worker: *const WorkerRecord) -> bool {
    // SAFETY: the caller lends a live pool.
    let registry = unsafe { (*pool).registry };
    // SAFETY: the caller lends a live worker.
    let pid = unsafe { swr_v1_worker_pid(worker) };
    // SAFETY: a created pool's registry is live.
    unsafe { swr_v1_process_worker_registry_unregister(registry, pid) }
}

/// `ProcessPool::get_worker_by_pid(pid)`: null before `create` or for an unknown pid.
///
/// # Safety
/// `pool` is a live `ProcessPool`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool17get_worker_by_pidEi")
)]
#[must_use]
pub unsafe extern "C" fn get_worker_by_pid(
    pool: *const PoolRecord,
    pid: c_int,
) -> *mut WorkerRecord {
    // SAFETY: the caller lends a live pool.
    let registry = unsafe { (*pool).registry };
    if registry.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: a non-null registry is live.
    unsafe { swr_v1_process_worker_registry_lookup(registry, pid) }.cast()
}

/// `ProcessPool::is_worker_running(worker)`.
///
/// # Safety
/// `pool` is a live `ProcessPool`; `worker` a live worker.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole11ProcessPool17is_worker_runningEPNS_6WorkerE")
)]
#[must_use]
pub unsafe extern "C" fn is_worker_running(
    pool: *const PoolRecord,
    worker: *const WorkerRecord,
) -> bool {
    // SAFETY: the caller lends a live pool.
    let running = unsafe { (*pool).running };
    // SAFETY: the caller lends a live worker.
    running
        && swr_v1_worker_global_get(4) == 0
        && !unsafe { swr_v1_worker_exceeded_max_requests(worker) }
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
    let pool = unsafe { pool_of(worker) };
    // SAFETY: as above.
    let id = unsafe { swr_v1_worker_id(worker) };
    let pipe = if flags & PIPE_MASTER != 0 {
        // SAFETY: as above.
        unsafe { pipe_master(worker) }
    } else {
        // SAFETY: as above.
        unsafe { pipe_worker(worker) }
    };
    // SAFETY: the worker's pool is live.
    let queue = unsafe { (*pool.cast::<PoolRecord>()).use_msgqueue } != 0;
    // SAFETY: the registered table's hooks are native's for the process.
    let hooks = unsafe { (*native()).hooks };
    // SAFETY: as the caller guarantees.
    unsafe { swr_v1_process_worker_send(hooks, pool, pipe, queue, id, data, length, flags) }
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_get_worker_idv")
)]
#[must_use]
pub extern "C" fn swoole_get_worker_id() -> u32 {
    u32::try_from(swr_v1_worker_global_get(0)).unwrap_or_default()
}

/// `swoole_get_worker_pid()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z21swoole_get_worker_pidv")
)]
#[must_use]
pub extern "C" fn swoole_get_worker_pid() -> c_int {
    c_int::try_from(swr_v1_worker_global_get(1)).unwrap_or_default()
}

/// `swoole_get_worker_type()`.
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z21swoole_set_worker_pidi")
)]
pub extern "C" fn swoole_set_worker_pid(pid: c_int) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // SAFETY: the pool is live for the call.
        assert!(
            unsafe { get_worker_by_pid(&raw const pool, 1) }.is_null(),
            "no registry, no worker"
        );
    }
}
