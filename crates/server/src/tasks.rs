//! What `Server::task` and the `Server::task_sync` pair borrow of a server: its task
//! pool, and the calling worker's notify pipe, lock and `task_results` slot
//! ([`tasks`]). crates/process's [`Tasks`] counts, sends and waits over them;
//! `Swoole\Server`'s task methods (crates/phpserver) and the core-tests cases call it.
use core::ffi::{c_int, c_void};
use core::ptr;

use swoole_rs_locks::{swr_v1_locks_lock, swr_v1_locks_unlock};
use swoole_rs_process::{
    methods::{self, swoole_get_worker_id},
    process_pool::{PoolRecord, SW_IPC_MAX_SIZE},
    task_sync::{TaskHost, Tasks},
};
use swoole_rs_tls::socket_pair::{self, SocketPair};

use crate::view;

/// `Lock::MUTEX`.
const MUTEX: i32 = 3;
const SW_OK: c_int = 0;

/// The server's task pool, and this worker's notify pipe and lock.
pub struct TaskPool {
    pool: *mut PoolRecord,
    pipe: *mut SocketPair,
    lock: *mut c_void,
}

impl TaskHost for TaskPool {
    #[expect(
        clippy::not_unsafe_ptr_arg_deref,
        reason = "TaskHost's contract: `task` is a packed EventData, which Tasks passes on"
    )]
    fn dispatch(&mut self, task: *mut u8, worker_id: &mut c_int, blocking: bool) -> bool {
        let dispatch = if blocking {
            methods::dispatch_sync
        } else {
            methods::dispatch
        };
        // SAFETY: the live task pool and a packed task.
        unsafe { dispatch(self.pool, task, worker_id) == SW_OK }
    }

    fn pipe_clean(&mut self) {
        // SAFETY: this worker's live notify pipe.
        unsafe { socket_pair::clean(self.pipe) };
    }

    fn set_timeout(&mut self, timeout: f64) {
        // SAFETY: as above.
        unsafe { socket_pair::set_timeout(self.pipe, timeout) };
    }

    fn read_notify(&mut self) -> isize {
        let mut notify = 0u64;
        // SAFETY: as above, into a local of that size.
        unsafe { socket_pair::read(self.pipe, (&raw mut notify).cast(), size_of::<u64>()) }
    }

    fn lock(&mut self, on: bool) {
        if on {
            // SAFETY: this worker's live mutex.
            unsafe { swr_v1_locks_lock(self.lock, MUTEX, libc::LOCK_EX, -1, 0) };
        } else {
            // SAFETY: as above.
            unsafe { swr_v1_locks_unlock(self.lock, MUTEX) };
        }
    }
}

/// `serv`'s task state; for a call that waits (`sync`), with this worker's notify
/// pipe, lock and `task_results` slot (`Server::get_task_result()`).
#[must_use]
pub fn tasks(serv: view::Server<'_>, sync: bool) -> Tasks<'_, TaskPool> {
    let counters = serv.counters();
    let mut host = TaskPool {
        pool: serv.task_pool(),
        pipe: ptr::null_mut(),
        lock: ptr::null_mut(),
    };
    let mut result = ptr::null_mut();
    if sync {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's uint16_t worker id"
        )]
        let id = swoole_get_worker_id() as u16;
        host.pipe = serv.task_notify_pipe(id);
        let worker = serv.worker(id);
        // SAFETY: this worker's live record.
        host.lock = unsafe { (*worker).lock };
        let results = serv.config().task_results.cast::<u8>();
        let size = usize::try_from(SW_IPC_MAX_SIZE).unwrap_or_default();
        result = results.wrapping_add(usize::from(id) * size);
    }
    Tasks {
        host,
        tasking: &counters.tasking_num,
        count: &counters.task_count,
        result,
    }
}
