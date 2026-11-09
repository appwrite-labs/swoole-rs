//! The `Server` methods over its worker arrays that were `src/server/master.cc`'s,
//! defined under their upstream (Itanium) symbols when the hybrid links this crate
//! (`upstream-symbols`). Each reads the Rust-owned configuration straight from the
//! `Server` pointer ([`crate::view`]).

use core::ffi::{c_int, c_void};

use swoole_rs_corebase::{log, worker_registry};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::server_workers::{
    swr_v1_process_server_worker_by_pid, swr_v1_process_server_workers_start,
};
use swoole_rs_process::worker_record::{
    WorkerRecord, swr_v1_worker_id, swr_v1_worker_init, swr_v1_worker_reset_counters,
    swr_v1_worker_set_id, swr_v1_worker_set_type,
};

use crate::view;

const SW_LOG_ERROR: c_int = 5;
const SW_USER_WORKER: u8 = 5;

/// `Server::get_worker(worker_id)`.
///
/// # Safety
/// `serv` is a live `Server` with its shared memory.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server10get_workerEt")
)]
pub unsafe extern "C" fn get_worker(serv: *mut c_void, worker_id: u16) -> *mut WorkerRecord {
    // SAFETY: per the caller.
    unsafe { view::worker(serv, worker_id) }
}

/// `Server::get_worker_by_pid(worker_pid)`.
///
/// # Safety
/// `serv` is a live `Server` with its shared memory.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server17get_worker_by_pidEi")
)]
pub unsafe extern "C" fn get_worker_by_pid(serv: *mut c_void, pid: c_int) -> *mut WorkerRecord {
    // SAFETY: per the caller.
    let workers = unsafe { view::workers(serv) };
    // SAFETY: per the caller.
    let registry = unsafe { view::config(serv) }.worker_registry;
    // SAFETY: the server's live arrays and registry.
    unsafe { swr_v1_process_server_worker_by_pid(&raw const workers, registry.cast(), pid) }
}

/// Startup's `LC_PREPARE_WORKERS`: publish the event pool and number the workers.
///
/// # Safety
/// `serv` is a live `Server`, starting, with its shared memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_workers_start(serv: *mut c_void) {
    // SAFETY: per the caller.
    let workers = unsafe { view::workers(serv) };
    // SAFETY: per the caller.
    let registry = unsafe { view::config(serv) }.worker_registry;
    // SAFETY: the server's live registry, configured only by the master.
    let pending =
        unsafe { worker_registry::swr_v1_process_worker_registry_pending_data(registry.cast()) };
    // SAFETY: the server's live arrays and its pending user workers.
    unsafe { swr_v1_process_server_workers_start(&raw const workers, serv, pending.cast()) };
}

/// `Server::add_worker(worker)`: a user worker, added before the server is created.
///
/// # Safety
/// `serv` is a live `Server` and `worker` a live `Worker` that outlives it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server10add_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn add_worker(serv: *mut c_void, worker: *mut WorkerRecord) -> c_int {
    // SAFETY: per the caller.
    let config = unsafe { view::config(serv) };
    if !config.factory_.is_null() {
        let m = format_args!("must add worker before server is created");
        log::error_log(
            SW_LOG_ERROR,
            E::SW_ERROR_WRONG_OPERATION as c_int,
            c"Server::add_worker",
            m,
        );
        return -1;
    }
    // SAFETY: the server's live registry, configured only by the master.
    let index = unsafe {
        worker_registry::swr_v1_process_worker_registry_pending_add(
            config.worker_registry.cast(),
            worker.cast(),
        )
    };
    // `user_worker_list.size() - 1`, as a `WorkerId`.
    let id = u32::try_from(index & u32::MAX as usize).unwrap_or_default();
    // SAFETY: the caller's live worker.
    unsafe { swr_v1_worker_set_id(worker, id) };
    // SAFETY: as above.
    unsafe { swr_v1_worker_set_type(worker, SW_USER_WORKER) };
    // SAFETY: as above.
    unsafe { swr_v1_worker_id(worker) }.cast_signed()
}

/// `Server::reset_worker_counter(worker)`.
///
/// # Safety
/// `serv` is a live `Server` with its shared memory; `worker` one of its workers.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server20reset_worker_counterEPNS_6WorkerE")
)]
pub unsafe extern "C" fn reset_worker_counter(serv: *mut c_void, worker: *mut WorkerRecord) {
    // SAFETY: per the caller.
    let total = unsafe { view::counters(serv) }.concurrency.as_ptr();
    // SAFETY: the live worker and the server's shared concurrency counter.
    unsafe { swr_v1_worker_reset_counters(worker, total) };
}

/// `Server::abort_worker(worker)`: reset its counters and, in base mode, retire its sessions.
///
/// # Safety
/// As [`reset_worker_counter`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server12abort_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn abort_worker(serv: *mut c_void, worker: *mut WorkerRecord) {
    // SAFETY: per the caller.
    unsafe { reset_worker_counter(serv, worker) };
    // SAFETY: per the caller.
    if u64::from(unsafe { view::config(serv) }.mode_) == crate::MODE_BASE {
        // SAFETY: per the caller.
        let id = unsafe { swr_v1_worker_id(worker) };
        // SAFETY: per the caller.
        let state = unsafe { view::state_ptr(serv) };
        // SAFETY: the server's live state and session table.
        unsafe { crate::records::swr_v1_server_sessions_retire(state, id) };
    }
}

/// `Server::init_event_worker(worker)`.
///
/// # Safety
/// `serv` is a live `Server`; `worker` the current process's worker.
pub unsafe extern "C" fn init_event_worker(serv: *mut c_void, worker: *mut WorkerRecord) {
    // SAFETY: per the caller.
    unsafe { swr_v1_worker_init(worker) };
    // SAFETY: per the caller.
    let config = unsafe { view::config(serv) };
    swoole_rs_process::methods::worker_set_max_request(
        worker,
        config.max_request,
        config.max_request_grace,
    );
}

/// `Server::init_ipc_max_size()`.
///
/// # Safety
/// `serv` is a live `Server`, not started.
pub unsafe extern "C" fn init_ipc_max_size(serv: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { view::config(serv) }.ipc_max_size =
        swoole_rs_corebase::constants::SW_IPC_BUFFER_MAX_SIZE;
}

/// `Server::destroy_task_workers()`: free the task results, then destroy the task pool.
///
/// # Safety
/// `serv` is a live `Server` with its shared memory, shutting down.
pub unsafe extern "C" fn destroy_task_workers(serv: *mut c_void) {
    // SAFETY: per the caller.
    let results = unsafe { view::config(serv) }.task_results;
    if !results.is_null() {
        swoole_rs_memory::members::sw_shm_free(results);
    }
    // SAFETY: per the caller.
    let pool = unsafe { view::task_pool(serv) };
    // SAFETY: the server's live task pool.
    unsafe { swoole_rs_process::methods::destroy(pool) };
}

/// `Lock::MUTEX`: the kind of a worker's lock.
const MUTEX: i32 = 3;

/// `Server::create_worker(worker)`: the worker's process-shared mutex, from the
/// global memory pool as upstream's `new Mutex(true)` took it, and its pipe's fds.
///
/// # Safety
/// `serv` is a live `Server` and `worker` one of its workers.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server13create_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn create_worker(serv: *mut c_void, worker: *mut WorkerRecord) {
    let size = u32::try_from(swoole_rs_locks::swr_v1_locks_size(MUTEX)).unwrap_or(u32::MAX);
    // SAFETY: the server's global memory pool is live.
    let lock = unsafe { swoole_rs_memory::members::pool::alloc(size) };
    // SAFETY: `size` fresh bytes, aligned by the pool; nothing else sees them yet.
    if lock.is_null() || unsafe { swoole_rs_locks::swr_v1_locks_init(lock, MUTEX, 1) } != 0 {
        // Upstream's `Mutex` constructor throws, which ends the process here.
        std::process::abort();
    }
    // SAFETY: per the caller.
    let worker = unsafe { &mut *worker };
    worker.lock = lock;
    if !worker.pipe_object.is_null() {
        // SAFETY: per the caller; the worker's live pipe.
        unsafe { crate::server_methods::store_pipe_fd(serv, worker.pipe_object) };
    }
}

/// `Server::destroy_worker(worker)`: frees the lock [`create_worker`] gave it.
///
/// # Safety
/// `worker` is a worker [`create_worker`] set up, whose lock nobody holds.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server14destroy_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn destroy_worker(_serv: *mut c_void, worker: *mut WorkerRecord) {
    // SAFETY: per the caller.
    let worker = unsafe { &mut *worker };
    let lock = core::mem::replace(&mut worker.lock, core::ptr::null_mut());
    if lock.is_null() {
        return;
    }
    // SAFETY: an initialized mutex nobody holds; the global pool's `free` is a no-op.
    unsafe { swoole_rs_locks::swr_v1_locks_destroy(lock, MUTEX) };
}

/// `Server::create_event_workers()`.
///
/// # Safety
/// `serv` is a live `Server`, being created, with its worker arrays.
#[cfg(feature = "upstream-symbols")]
pub unsafe extern "C" fn create_event_workers(serv: *mut c_void) -> bool {
    // SAFETY: per the caller.
    let count = unsafe { view::config(serv) }.worker_num;
    for id in 0..count {
        // `get_worker(uint16_t)`, as the C++ loop called it.
        let id = u16::try_from(id & 0xffff).unwrap_or_default();
        // SAFETY: per the caller.
        let worker = unsafe { view::worker(serv, id) };
        // SAFETY: one of the server's event workers.
        unsafe { create_worker(serv, worker) };
    }
    true
}

/// `Server::create_user_workers()`: copy the added user workers into shared
/// memory (no process is forked) and give each its lock.
///
/// # Safety
/// `serv` is a live `Server`, in the manager's startup.
#[cfg(feature = "upstream-symbols")]
pub unsafe extern "C" fn create_user_workers(serv: *mut c_void) -> bool {
    let size = size_of::<WorkerRecord>();
    // SAFETY: per the caller.
    let count = unsafe { view::user_worker_num(serv) };
    let copies = swoole_rs_memory::members::sw_shm_calloc(count, size).cast::<WorkerRecord>();
    // SAFETY: per the caller.
    let config = unsafe { view::config(serv) };
    config.user_workers = copies.cast();
    if copies.is_null() {
        let m = format_args!("sw_shm_calloc({count}, {size}) for user_workers failed");
        log::sys_warning(c"Server::create_user_workers", m);
        return false;
    }
    // SAFETY: the server's live registry, configured only by the master.
    let pending = unsafe {
        worker_registry::swr_v1_process_worker_registry_pending_data(config.worker_registry.cast())
    };
    for index in 0..count {
        // SAFETY: `count` live worker pointers.
        let worker = unsafe { pending.wrapping_add(index).read() }.cast::<WorkerRecord>();
        // SAFETY: the caller's live worker.
        let record = unsafe { worker.read() };
        // SAFETY: its slot in the fresh shared array of `count` records.
        unsafe { copies.wrapping_add(index).write(record) };
        // SAFETY: per the caller.
        unsafe { create_worker(serv, worker) };
        // SAFETY: the caller's live worker.
        unsafe { swr_v1_worker_set_type(worker, SW_USER_WORKER) };
    }
    true
}
