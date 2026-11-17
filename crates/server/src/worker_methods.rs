//! The `Server` methods over its worker arrays that were `src/server/master.cc`'s.
//! Each reads the Rust-owned configuration straight from the `Server` ([`crate::view`]).

use crate::Mode;
use core::ffi::c_int;
use swoole_rs_corebase::locks::SharedMutex;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::worker_record::WorkerType;

use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::server_workers::swr_v1_process_server_worker_by_pid;
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_init};

use crate::view;
use swoole_rs_corebase::log::LogLevel::SW_LOG_ERROR;

/// `Server::get_worker_by_pid(worker_pid)`: the first event worker with the pid, then
/// the task pool's, then a user worker's; null for none.
#[must_use]
pub fn get_worker_by_pid(serv: view::Server<'_>, pid: c_int) -> *mut WorkerRecord {
    let registry = serv.config().worker_registry.as_deref();
    swr_v1_process_server_worker_by_pid(&serv.workers(), registry, pid)
}

/// `Server::add_worker(worker)`: a user worker, added before the server is created.
/// The server retains the worker and its pipe and queue. Its id, or none
/// (`SW_ERR`) once the server is created.
#[must_use]
pub fn add_worker(
    serv: view::Server<'_>,
    owner: crate::process_worker::ProcessWorker,
) -> Option<u32> {
    let config = serv.config();
    if !config.factory_.get().is_null() {
        let m = format_args!("must add worker before server is created");
        log::error_log(
            SW_LOG_ERROR,
            E::SW_ERROR_WRONG_OPERATION as c_int,
            c"Server::add_worker",
            m,
        );
        return None;
    }
    let index = serv.registry().pending_add(owner);
    let worker = serv.pending_worker(index)?;
    // `user_worker_list.size() - 1`, as a `WorkerId`.
    let id = u32::try_from(index & u32::MAX as usize).unwrap_or_default();
    worker.set_id(id);
    worker.set_kind(WorkerType::User);
    Some(id)
}

/// `Server::reset_worker_counter(worker)`: the worker's concurrency leaves the server's.
pub fn reset_worker_counter(serv: view::Server<'_>, worker: Worker<'_>) {
    worker.reset_counters(&serv.counters().concurrency);
}

/// `Server::abort_worker(worker)`: reset its counters and, in base mode, retire its sessions.
pub fn abort_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    reset_worker_counter(serv, worker);
    if serv.config().mode_ == Mode::Base {
        serv.state().retire_sessions(worker.id());
    }
}

/// `Server::init_event_worker(worker)`, for the current process's worker.
pub fn init_event_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    swr_v1_worker_init(worker);
    let config = serv.config();
    swoole_rs_process::methods::worker_set_max_request(
        config.max_request.get(),
        config.max_request_grace.get(),
    );
}

/// `Server::create_worker(worker)`: the worker's process-shared mutex, from the
/// global memory pool as upstream's `new Mutex(true)` took it, and its pipe's fds.
pub fn create_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    // A zeroed `SharedMutex`, which the pool keeps for the program's life.
    let Some([lock]) = swoole_rs_memory::members::pool::words::<SharedMutex>(1) else {
        // Upstream's `Mutex` constructor throws, which ends the process here.
        swoole_rs_corebase::fatal("no shared memory for a worker lock");
    };
    if lock.init().is_err() {
        swoole_rs_corebase::fatal("a worker lock can't be initialized");
    }
    worker.set_lock(Some(lock));
    crate::server_methods::store_pipe_fd(serv, worker);
}

/// `Server::destroy_worker(worker)`: frees the lock [`create_worker`] gave it.
pub fn destroy_worker(worker: Worker<'_>) {
    // The global pool's `free` is a no-op: the storage stays.
    if let Some(lock) = worker.lock() {
        worker.set_lock(None);
        lock.destroy();
    }
}

/// `Server::create_event_workers()`, once the server has its worker arrays.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn create_event_workers(serv: view::Server<'_>) -> bool {
    let count = serv.config().worker_num.get();
    for id in 0..count {
        // `get_worker(uint16_t)`, as the C++ loop called it.
        let id = u16::try_from(id & 0xffff).unwrap_or_default();
        if let Some(worker) = serv.worker_record(id) {
            create_worker(serv, worker);
        }
    }
    true
}

/// `Server::create_user_workers()`: copy the added user workers into shared
/// memory (no process is forked), store them and give each its lock. False,
/// after a warning, when they couldn't be mapped.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn create_user_workers(serv: view::Server<'_>) -> bool {
    let size = size_of::<WorkerRecord>();
    let count = serv.registry().pending().len();
    let workers = swoole_rs_memory::members::Shm::calloc(count, size);
    let mapped = !workers.is_null();
    // A zeroed record is an empty worker, so the array is `count` records from now on.
    serv.state().tables_ref().user_workers.fill(workers);
    if !mapped {
        let m = format_args!("sw_shm_calloc({count}, {size}) for user_workers failed");
        log::sys_warning(c"Server::create_user_workers", m);
        return false;
    }
    let copies = serv.workers();
    for index in 0..count {
        let Some(worker) = serv.pending_worker(index) else {
            continue;
        };
        if let Some(copy) = copies.user(index) {
            copy.put(worker.get());
        }
        create_worker(serv, worker);
        worker.set_kind(WorkerType::User);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::SessionRecord;
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
        let mut server = crate::TestServer::new();
        let config = server.config_mut();
        config.mode_ = Mode::Base;
        config.gs =
            swoole_rs_memory::members::Shm::calloc(1, size_of::<crate::shared::ServerShared>());
        let workers = swoole_rs_memory::members::Shm::calloc(2, size_of::<WorkerRecord>());
        let worker = workers.as_ptr().cast::<WorkerRecord>();
        let other = worker.wrapping_add(1);
        unsafe { worker.write(*WorkerRecord::boxed()) };
        unsafe { other.write(*WorkerRecord::boxed()) };
        config.worker_num.set(2);
        server.tables_mut().workers = workers;
        let serv = server.serv();
        unsafe { (*worker).set_pid(79) };
        unsafe { (*other).set_pid(79) };
        assert_eq!(get_worker_by_pid(serv, 79), worker);
        unsafe { (*worker).set_pid(80) };
        assert_eq!(get_worker_by_pid(serv, 79), other);
        unsafe { (*other).set_pid(-7) };
        assert_eq!(get_worker_by_pid(serv, -7), other);
        assert!(get_worker_by_pid(serv, 79).is_null());

        unsafe { (*worker).set_id(1) };
        unsafe { &(*worker).concurrency }.store(3, SeqCst);
        let worker = unsafe { Worker::from(&*worker) };
        worker.set_request_count(7);
        worker.set_response_count(8);
        worker.set_dispatch_count(9);
        serv.counters().concurrency.store(7, SeqCst);
        let size = 1024 * 1024;
        let last = size - 1;
        // reactor_bits: the reactor id in the low byte, the reserved bits above it.
        let mut sessions: Vec<_> = (0..size)
            .map(|index| SessionRecord {
                id: core::cell::Cell::new(i64::try_from(index + 41).unwrap()),
                fd: core::cell::Cell::new(23),
                reactor_bits: core::cell::Cell::new(0x5a12_3400 | 2),
            })
            .collect();
        sessions[0].reactor_bits.set(0x5a12_3400 | 1);
        sessions[last].reactor_bits.set(0x5a12_3400 | 1);
        *server.tables_mut().session_list_mut() = crate::records::tests::naming(&mut sessions);
        let serv = server.serv();
        abort_worker(serv, worker);
        assert_eq!(worker.counters(), [0, 0, 0, 0]);
        assert_eq!(serv.counters().concurrency.load(SeqCst), 4);
        for (index, slot) in sessions.iter().enumerate() {
            let edge = index == 0 || index == last;
            assert_eq!(slot.id.get(), i64::try_from(index + 41).unwrap());
            assert_eq!(slot.reactor_bits.get() >> 8, 0x5a_1234);
            assert_eq!(slot.reactor_bits.get() & 0xff, if edge { 1 } else { 2 });
            assert_eq!(slot.fd.get(), if edge { 0 } else { 23 });
        }
        unsafe { &(*worker.raw()).concurrency }.store(9, SeqCst);
        serv.counters().concurrency.store(3, SeqCst);
        reset_worker_counter(serv, worker);
        assert_eq!(worker.counters()[3], 0);
        assert_eq!(serv.counters().concurrency.load(SeqCst), 0);

        *server.tables_mut().session_list_mut() = swoole_rs_memory::members::Shm::empty();
        server.tables_mut().workers.free();
        server.config_mut().gs.free();
    }
}
