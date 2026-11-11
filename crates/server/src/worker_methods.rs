//! The `Server` methods over its worker arrays that were `src/server/master.cc`'s.
//! Each reads the Rust-owned configuration straight from the `Server` ([`crate::view`]).

use core::ffi::c_int;
use core::mem::MaybeUninit;
use swoole_rs_locks::SharedMutex;
use swoole_rs_process::worker_record::Worker;

use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::server_workers::{
    swr_v1_process_server_worker_by_pid, swr_v1_process_server_workers_start,
};
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_init};

use crate::view;

const SW_LOG_ERROR: c_int = 5;
const SW_USER_WORKER: u8 = 5;

/// `Server::get_worker_by_pid(worker_pid)`: the first event worker with the pid, then
/// the task pool's, then a user worker's; null for none.
#[must_use]
pub fn get_worker_by_pid(serv: view::Server<'_>, pid: c_int) -> *mut WorkerRecord {
    let workers = serv.workers();
    let registry = serv.config().worker_registry;
    // SAFETY: the server's live arrays and registry.
    unsafe { swr_v1_process_server_worker_by_pid(&workers, registry.cast(), pid) }
}

/// Startup's `LC_PREPARE_WORKERS`: publish the event pool and number the workers.
pub(crate) fn workers_start(serv: view::Server<'_>) {
    let workers = serv.workers();
    let pending = serv.registry().pending().as_ptr();
    // SAFETY: the server's live arrays and its pending user workers.
    unsafe { swr_v1_process_server_workers_start(&workers, serv.raw(), pending.cast()) };
}

/// `Server::add_worker(worker)`: a user worker, added before the server is created.
///
/// # Safety
/// `serv` is a live `Server` and `worker` a live `Worker` that outlives it.
pub unsafe fn add_worker(serv: view::Server<'_>, worker: *mut WorkerRecord) -> c_int {
    let config = serv.config();
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
    // SAFETY: the server's live registry, configured only by the master before it starts.
    let index = unsafe { serv.registry_mut() }.pending_add(worker.cast());
    // `user_worker_list.size() - 1`, as a `WorkerId`.
    let id = u32::try_from(index & u32::MAX as usize).unwrap_or_default();
    // SAFETY: the caller's live worker.
    let worker = unsafe { Worker::new(worker) };
    worker.set_id(id);
    worker.set_kind(SW_USER_WORKER);
    worker.id().cast_signed()
}

/// `Server::reset_worker_counter(worker)`: the worker's concurrency leaves the server's.
pub fn reset_worker_counter(serv: view::Server<'_>, worker: Worker<'_>) {
    worker.reset_counters(&serv.counters().concurrency);
}

/// `Server::abort_worker(worker)`: reset its counters and, in base mode, retire its sessions.
pub fn abort_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    reset_worker_counter(serv, worker);
    if u64::from(serv.config().mode_) == crate::MODE_BASE {
        serv.state().retire_sessions(worker.id());
    }
}

/// `Server::init_event_worker(worker)`, for the current process's worker.
pub fn init_event_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    swr_v1_worker_init(worker);
    let config = serv.config();
    swoole_rs_process::methods::worker_set_max_request(
        config.max_request,
        config.max_request_grace,
    );
}

/// `Server::init_ipc_max_size()`.
///
/// # Safety
/// `serv` is not started.
pub unsafe fn init_ipc_max_size(serv: view::Server<'_>) {
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    unsafe { serv.config_mut() }.ipc_max_size =
        swoole_rs_corebase::constants::SW_IPC_BUFFER_MAX_SIZE;
}

/// `Server::destroy_task_workers()`: free the task results, then destroy the task pool.
///
/// # Safety
/// `serv` is shutting down.
pub unsafe fn destroy_task_workers(serv: view::Server<'_>) {
    // SAFETY: the server is shutting down on this thread, with no other reference to the field.
    unsafe { serv.config_mut() }.task_results.free();
    swoole_rs_process::methods::destroy(serv.task_pool_ref());
}

/// `Server::create_worker(worker)`: the worker's process-shared mutex, from the
/// global memory pool as upstream's `new Mutex(true)` took it, and its pipe's fds.
///
/// # Safety
/// `serv` is a live `Server` and `worker` one of its workers.
pub unsafe fn create_worker(serv: view::Server<'_>, worker: *mut WorkerRecord) {
    let size = u32::try_from(size_of::<SharedMutex>()).unwrap_or(u32::MAX);
    let lock = swoole_rs_memory::members::pool::alloc(size);
    // SAFETY: null, or `size` fresh bytes, aligned by the pool, which the global pool
    // keeps for the program's life; nothing else sees them yet.
    let lock = unsafe { lock.cast::<MaybeUninit<SharedMutex>>().as_mut() };
    // Upstream's `Mutex` constructor throws, which ends the process here.
    let lock = lock.unwrap_or_else(|| std::process::abort());
    let lock = lock.write(SharedMutex::new());
    if lock.init() != 0 {
        std::process::abort();
    }
    // SAFETY: per the caller.
    let worker = unsafe { &mut *worker };
    worker.lock = Some(lock);
    if !worker.pipe_object.is_null() {
        // SAFETY: per the caller; the worker's live pipe.
        unsafe { crate::server_methods::store_pipe_fd(serv.raw(), worker.pipe_object) };
    }
}

/// `Server::destroy_worker(worker)`: frees the lock [`create_worker`] gave it.
pub fn destroy_worker(worker: &mut WorkerRecord) {
    // The global pool's `free` is a no-op: the storage stays.
    if let Some(lock) = worker.lock.take() {
        lock.destroy();
    }
}

/// `Server::create_event_workers()`.
///
/// # Safety
/// `serv` is being created, with its worker arrays.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub unsafe fn create_event_workers(serv: view::Server<'_>) -> bool {
    let count = serv.config().worker_num;
    for id in 0..count {
        // `get_worker(uint16_t)`, as the C++ loop called it.
        let id = u16::try_from(id & 0xffff).unwrap_or_default();
        let worker = serv.worker(id);
        // SAFETY: one of the server's event workers.
        unsafe { create_worker(serv, worker) };
    }
    true
}

/// `Server::create_user_workers()`: copy the added user workers into shared
/// memory (no process is forked) and give each its lock.
///
/// # Safety
/// `serv` is in the manager's startup.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub unsafe fn create_user_workers(serv: view::Server<'_>) -> bool {
    let size = size_of::<WorkerRecord>();
    let count = serv.user_worker_num();
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    let config = unsafe { serv.config_mut() };
    config.user_workers = swoole_rs_memory::members::Shm::calloc(count, size);
    let copies = config.user_workers.as_ptr().cast::<WorkerRecord>();
    if copies.is_null() {
        let m = format_args!("sw_shm_calloc({count}, {size}) for user_workers failed");
        log::sys_warning(c"Server::create_user_workers", m);
        return false;
    }
    let pending = serv.registry().pending().as_ptr();
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
        unsafe { Worker::new(worker).set_kind(SW_USER_WORKER) };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::SessionRecord;
    use core::ffi::c_void;
    use core::sync::atomic::Ordering::SeqCst;

    /// The repo-owned Server witness, formerly `tests/modules/process/worker_retirement.cc` and
    /// then the hooks fixture's `rust_worker_retirement.abort_and_reset`: first-match PID lookup,
    /// the session bits `abort_worker` clears (including the last slot and the reserved bits)
    /// and the worker/shared concurrency reset, over a base-mode server. Unit tests link no
    /// `swr_v1_server_new` or `create()`, so the test lends the server its shared record, the
    /// two event workers and the session table those would have allocated.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn abort_and_reset() {
        let state = crate::swr_v1_server_state_new();
        let head = [state.cast::<c_void>(), unsafe { (*state).config() }.cast()];
        let serv = unsafe { view::Server::from_raw(head.as_ptr().cast_mut().cast()) };
        unsafe { serv.config_mut() }.mode_ = 1;
        unsafe { serv.config_mut() }.gs =
            swoole_rs_memory::members::Shm::calloc(1, size_of::<crate::shared::ServerShared>());
        let workers = swoole_rs_memory::members::Shm::calloc(2, size_of::<WorkerRecord>());
        let worker = workers.as_ptr().cast::<WorkerRecord>();
        let other = worker.wrapping_add(1);
        unsafe { worker.write(*WorkerRecord::boxed()) };
        unsafe { other.write(*WorkerRecord::boxed()) };
        unsafe { serv.config_mut() }.worker_num = 2;
        unsafe { serv.config_mut() }.workers = workers;
        unsafe { (*worker).set_pid(79) };
        unsafe { (*other).set_pid(79) };
        assert_eq!(get_worker_by_pid(serv, 79), worker);
        unsafe { (*worker).set_pid(80) };
        assert_eq!(get_worker_by_pid(serv, 79), other);
        unsafe { (*other).set_pid(-7) };
        assert_eq!(get_worker_by_pid(serv, -7), other);
        assert!(get_worker_by_pid(serv, 79).is_null());

        unsafe { (*worker).set_id(1) };
        unsafe { (*worker).concurrency = 3 };
        let worker = unsafe { Worker::new(worker) };
        worker.set_request_count(7);
        worker.set_response_count(8);
        worker.set_dispatch_count(9);
        serv.counters().concurrency.store(7, SeqCst);
        let size = 1024 * 1024;
        let last = size - 1;
        // reactor_bits: the reactor id in the low byte, the reserved bits above it.
        let mut sessions: Vec<_> = (0..size)
            .map(|index| SessionRecord {
                id: i64::try_from(index + 41).unwrap(),
                fd: 23,
                reactor_bits: 0x5a12_3400 | 2,
            })
            .collect();
        sessions[0].reactor_bits = 0x5a12_3400 | 1;
        sessions[last].reactor_bits = 0x5a12_3400 | 1;
        unsafe {
            (*serv.state().tables()).session_list = crate::records::tests::naming(&mut sessions);
        };
        abort_worker(serv, worker);
        assert_eq!(worker.counters(), [0, 0, 0, 0]);
        assert_eq!(serv.counters().concurrency.load(SeqCst), 4);
        for (index, slot) in sessions.iter().enumerate() {
            let edge = index == 0 || index == last;
            assert_eq!(slot.id, i64::try_from(index + 41).unwrap());
            assert_eq!(slot.reactor_bits >> 8, 0x5a_1234);
            assert_eq!(slot.reactor_bits & 0xff, if edge { 1 } else { 2 });
            assert_eq!(slot.fd, if edge { 0 } else { 23 });
        }
        unsafe { (*worker.raw()).concurrency = 9 };
        serv.counters().concurrency.store(3, SeqCst);
        reset_worker_counter(serv, worker);
        assert_eq!(worker.counters()[3], 0);
        assert_eq!(serv.counters().concurrency.load(SeqCst), 0);

        unsafe { (*serv.state().tables()).session_list = swoole_rs_memory::members::Shm::NULL };
        unsafe { serv.config_mut() }.workers.free();
        unsafe { serv.config_mut() }.gs.free();
        unsafe { crate::state::free_state(state) };
    }
}
