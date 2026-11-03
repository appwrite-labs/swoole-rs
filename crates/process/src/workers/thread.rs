//! Thread mode (`src/server/thread.cc`): every worker is a thread, and a manager
//! thread rejoins and respawns each one that exits and walks reloads one worker
//! at a time. Rust owns the manager's state ([`ThreadManager`]: its wait queue,
//! the reload request and the reload's progress); native keeps the threads, the
//! thread bodies and the server.
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::event_worker::EVENT_WORKER;
use super::ops::{
    LOG_CANNOT_RELOAD, LOG_NOTIFY_MANAGER, LOG_RELOADING, OP_ABORT, OP_ALL_CORE_WORKER_NUM,
    OP_CALL_WORKER_ERROR, OP_EVENT_FREE, OP_EVENT_INIT, OP_HAS_TIMER, OP_IS_MANAGER,
    OP_JOIN_THREAD, OP_KILL_WORKER, OP_MANAGER_THREAD_ID, OP_ON_AFTER_RELOAD, OP_ON_BEFORE_RELOAD,
    OP_PORT_ADD, OP_PORT_COUNT, OP_PORT_DGRAM, OP_PORT_LISTEN, OP_RUNNING, OP_SELECT_WORKER,
    OP_SET_MASTER_THREAD_ID, OP_SET_NOT_SUPPORTED, OP_SPAWN_EVENT_THREAD, OP_SPAWN_MANAGER_THREAD,
    OP_SPAWN_TASK_THREAD, OP_SPAWN_USER_THREAD, OP_START_MASTER_THREAD, OP_STORE_LISTEN_SOCKET,
    OP_TASK_WORKER_NUM, OP_TERMINATE_MANAGER_THREAD, OP_THREAD_EXIT_STATUS, OP_THREAD_MANAGER,
    OP_TIMER_NEXT_MSEC, OP_TIMER_SELECT, OP_USER_WORKER_NUM, OP_WORKER_ID, OP_WORKER_NUM,
    OP_WORKER_TYPE, RUN_START_WORKER_THREADS, RUN_STOP_WORKER_THREADS, RUN_THREAD_MANAGER_WAIT,
    RUN_THREAD_RELOAD, RUN_THREAD_TERMINATE_MANAGER, TRACE_MANAGER_EXITED, TRACE_MANAGER_WAKE,
};
use super::{Host, SW_ERR, SW_OK};

/// `swoole::WorkerType`.
const TASK_WORKER: u64 = 3;
const USER_WORKER: u64 = 4;

/// What the manager thread waits for.
enum Wake {
    /// A worker thread exited (its native `Worker *`).
    Exited(*mut c_void),
    Reload,
    Exit,
}

/// The reload in progress: the workers from `reloaded` up to `workers` are asked to
/// stop one by one (`ThreadReloadTask`).
#[derive(Clone, Copy)]
struct Reload {
    workers: u16,
    reloaded: u16,
}

/// The thread factory's manager state, behind `ThreadFactory`'s opaque handle.
#[derive(Default)]
pub struct ThreadManager {
    queue: Mutex<VecDeque<Wake>>,
    ready: Condvar,
    reloading: AtomicBool,
    reload_all: AtomicBool,
    reload: Mutex<Option<Reload>>,
}

// SAFETY: the queued `Worker *` values are opaque tokens handed back to native
// code on the manager thread; Rust never dereferences them.
unsafe impl Send for ThreadManager {}
// SAFETY: as above; every other field is synchronised.
unsafe impl Sync for ThreadManager {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ThreadManager {
    fn push(&self, wake: Wake) {
        lock(&self.queue).push_back(wake);
        self.ready.notify_one();
    }

    /// The next wake-up, waiting at most `timeout_ms` when it is positive.
    fn next(&self, timeout_ms: i64) -> Option<Wake> {
        let queue = lock(&self.queue);
        let mut queue = if timeout_ms > 0 {
            self.ready
                .wait_timeout_while(
                    queue,
                    Duration::from_millis(timeout_ms.cast_unsigned()),
                    |q| q.is_empty(),
                )
                .unwrap_or_else(PoisonError::into_inner)
                .0
        } else {
            self.ready
                .wait_while(queue, |q| q.is_empty())
                .unwrap_or_else(PoisonError::into_inner)
        };
        queue.pop_front()
    }

    fn len(&self) -> usize {
        lock(&self.queue).len()
    }
}

/// A new manager state; native frees it with [`swr_v1_thread_manager_free`].
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_thread_manager_new() -> *mut ThreadManager {
    Box::into_raw(Box::<ThreadManager>::default())
}

/// Free a manager state.
///
/// # Safety
/// `manager` is null or a live handle from `_new`, not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_thread_manager_free(manager: *mut ThreadManager) {
    if !manager.is_null() {
        // SAFETY: the caller hands back the handle `_new` leaked.
        drop(unsafe { Box::from_raw(manager) });
    }
}

/// Queue a worker thread's exit for the manager thread (`at_thread_exit`).
///
/// # Safety
/// `manager` is a live handle; `worker` is an opaque `Worker *`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_thread_manager_exited(
    manager: *const ThreadManager,
    worker: *mut c_void,
) {
    // SAFETY: the caller lends a live handle.
    unsafe { &*manager }.push(Wake::Exited(worker));
}

/// `ThreadFactory`'s manager state, from `OP_THREAD_MANAGER`.
fn manager<'a, H: Host>(h: &mut H) -> &'a ThreadManager {
    let address = usize::try_from(h.get(OP_THREAD_MANAGER)).unwrap_or(0);
    // SAFETY: native passes the live handle its ThreadFactory owns for the
    // server's lifetime; it is shared and synchronised.
    unsafe { &*(address as *const ThreadManager) }
}

pub(super) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
        RUN_THREAD_MANAGER_WAIT => {
            wait(h);
            SW_OK
        }
        RUN_THREAD_RELOAD => i64::from(reload(h, arg != 0)),
        RUN_THREAD_TERMINATE_MANAGER => {
            manager(h).push(Wake::Exit);
            // The manager thread may still be joining workers when the server stops:
            // reclaim it first, so it has surely exited.
            let id = h.get(OP_MANAGER_THREAD_ID);
            h.op(OP_JOIN_THREAD, id);
            h.get(TRACE_MANAGER_EXITED);
            SW_OK
        }
        RUN_START_WORKER_THREADS => start(h),
        RUN_STOP_WORKER_THREADS => {
            h.get(OP_TERMINATE_MANAGER_THREAD);
            for worker in 0..h.get(OP_ALL_CORE_WORKER_NUM) {
                h.op(OP_KILL_WORKER, worker);
            }
            SW_OK
        }
        _ => SW_ERR,
    }
}

/// `ThreadFactory::wait`: the manager thread's loop.
fn wait<H: Host>(h: &mut H) {
    let manager = manager(h);
    loop {
        let wake = manager.next(h.signed(OP_TIMER_NEXT_MSEC));
        h.op(TRACE_MANAGER_WAKE, manager.len() as u64);
        match wake {
            Some(Wake::Reload) => {
                do_reload(h, manager);
                continue;
            }
            Some(Wake::Exit) => break,
            Some(Wake::Exited(worker)) => restart(h, worker),
            None => {}
        }
        if h.is(OP_HAS_TIMER) {
            h.get(OP_TIMER_SELECT);
        }
        if h.is(OP_RUNNING) && manager.reloading.load(Ordering::Acquire) {
            do_reload(h, manager);
        }
    }
}

/// Rejoin an exited worker thread, report an abnormal exit, and respawn it.
fn restart<H: Host>(h: &mut H, worker: *mut c_void) {
    h.op(OP_SELECT_WORKER, worker as u64);
    let id = h.get(OP_WORKER_ID);
    if h.get(OP_THREAD_EXIT_STATUS) != 0 {
        h.get(OP_CALL_WORKER_ERROR);
    }
    h.op(OP_JOIN_THREAD, id);
    match h.get(OP_WORKER_TYPE) {
        EVENT_WORKER => h.op(OP_SPAWN_EVENT_THREAD, id),
        TASK_WORKER => h.op(OP_SPAWN_TASK_THREAD, id),
        USER_WORKER => h.op(OP_SPAWN_USER_THREAD, id),
        _ => h.get(OP_ABORT),
    };
}

/// `ThreadFactory::do_reload`: ask the next worker to stop; when the last has
/// been asked, the reload is done.
fn do_reload<H: Host>(h: &mut H, manager: &ThreadManager) {
    let mut reload = lock(&manager.reload);
    let task = if let Some(task) = *reload {
        task
    } else {
        // A task-worker reload skips the event workers.
        let workers = u16::try_from(h.get(OP_ALL_CORE_WORKER_NUM)).unwrap_or(u16::MAX);
        let reloaded = if manager.reload_all.load(Ordering::Acquire) {
            0
        } else {
            u16::try_from(h.get(OP_WORKER_NUM)).unwrap_or(u16::MAX)
        };
        h.get(OP_ON_BEFORE_RELOAD);
        Reload { workers, reloaded }
    };
    h.op(OP_KILL_WORKER, u64::from(task.reloaded));
    let task = Reload {
        reloaded: task.reloaded.wrapping_add(1),
        ..task
    };
    if task.reloaded == task.workers {
        *reload = None;
        manager.reloading.store(false, Ordering::Release);
        drop(reload);
        h.get(OP_ON_AFTER_RELOAD);
    } else {
        *reload = Some(task);
    }
}

/// `ThreadFactory::reload`: start a reload, in the manager thread or by asking it.
fn reload<H: Host>(h: &mut H, all: bool) -> bool {
    let what = u64::from(all);
    if h.get(OP_TASK_WORKER_NUM) == 0 && !all {
        h.op(LOG_CANNOT_RELOAD, what);
        return false;
    }
    let manager = manager(h);
    // One reload at a time.
    if manager
        .reloading
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        h.get(OP_SET_NOT_SUPPORTED);
        return false;
    }
    manager.reload_all.store(all, Ordering::Release);
    if h.is(OP_IS_MANAGER) {
        h.op(LOG_RELOADING, what);
        do_reload(h, manager);
    } else {
        h.op(LOG_NOTIFY_MANAGER, what);
        manager.push(Wake::Reload);
    }
    true
}

/// `Server::start_worker_threads`: the task, event and user worker threads, the
/// manager thread, then the master thread's listeners and loop.
fn start<H: Host>(h: &mut H) -> i64 {
    let workers = h.get(OP_WORKER_NUM);
    let task_workers = h.get(OP_TASK_WORKER_NUM);
    for worker in 0..task_workers {
        h.op(OP_SPAWN_TASK_THREAD, workers + worker);
    }
    for worker in 0..workers {
        h.op(OP_SPAWN_EVENT_THREAD, worker);
    }
    for worker in 0..h.get(OP_USER_WORKER_NUM) {
        h.op(OP_SPAWN_USER_THREAD, task_workers + workers + worker);
    }
    let id = h.get(OP_MANAGER_THREAD_ID);
    h.op(OP_SPAWN_MANAGER_THREAD, id);
    if h.signed(OP_EVENT_INIT) < 0 {
        return SW_ERR;
    }
    for port in 0..h.get(OP_PORT_COUNT) {
        if h.op(OP_PORT_DGRAM, port) != 0 {
            continue;
        }
        if h.op(OP_PORT_LISTEN, port).cast_signed() < 0 {
            h.get(OP_EVENT_FREE);
            return SW_ERR;
        }
        h.op(OP_PORT_ADD, port);
    }
    h.get(OP_SET_MASTER_THREAD_ID);
    h.get(OP_STORE_LISTEN_SOCKET);
    h.signed(OP_START_MASTER_THREAD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::script::Script;

    fn run_op(h: &mut Script, operation: u32, arg: u64) -> i64 {
        crate::workers::run(h, operation, arg)
    }

    fn handle(manager: &ThreadManager) -> u64 {
        core::ptr::from_ref(manager) as u64
    }

    #[test]
    fn the_manager_thread_respawns_exited_workers_until_told_to_exit() {
        let manager = ThreadManager::default();
        let worker = 0x1000 as *mut c_void;
        manager.push(Wake::Exited(worker));
        manager.push(Wake::Exit);
        let mut h = Script::with(&[
            (OP_THREAD_MANAGER, handle(&manager)),
            (OP_WORKER_ID, 3),
            (OP_WORKER_TYPE, TASK_WORKER),
            (OP_THREAD_EXIT_STATUS, 1),
            (OP_HAS_TIMER, 1),
        ]);
        run_op(&mut h, RUN_THREAD_MANAGER_WAIT, 0);
        assert_eq!(h.args(OP_SELECT_WORKER), vec![0x1000]);
        assert_eq!(h.called(OP_CALL_WORKER_ERROR), 1);
        assert_eq!(h.args(OP_JOIN_THREAD), vec![3]);
        assert_eq!(h.args(OP_SPAWN_TASK_THREAD), vec![3]);
        assert_eq!(h.called(OP_TIMER_SELECT), 1);
    }

    #[test]
    fn a_reload_kills_one_worker_per_wake_until_done() {
        let manager = ThreadManager::default();
        let mut h = Script::with(&[
            (OP_THREAD_MANAGER, handle(&manager)),
            (OP_TASK_WORKER_NUM, 2),
            (OP_WORKER_NUM, 2),
            (OP_ALL_CORE_WORKER_NUM, 4),
        ]);
        // Task workers only, from a worker thread: the manager is asked.
        assert_eq!(run_op(&mut h, RUN_THREAD_RELOAD, 0), 1);
        assert_eq!(h.args(LOG_NOTIFY_MANAGER), vec![0]);
        // A second request while one runs is refused.
        assert_eq!(run_op(&mut h, RUN_THREAD_RELOAD, 1), 0);
        assert_eq!(h.called(OP_SET_NOT_SUPPORTED), 1);
        manager.push(Wake::Exit);
        h.facts.insert(OP_RUNNING, 1);
        run_op(&mut h, RUN_THREAD_MANAGER_WAIT, 0);
        // The queued reload kills worker 2; Exit ends the loop before worker 3.
        assert_eq!(h.args(OP_KILL_WORKER), vec![2]);
        assert_eq!(h.called(OP_ON_BEFORE_RELOAD), 1);
        assert!(manager.reloading.load(Ordering::Acquire));
        do_reload(&mut h, &manager);
        assert_eq!(h.args(OP_KILL_WORKER), vec![2, 3]);
        assert_eq!(h.called(OP_ON_AFTER_RELOAD), 1);
        assert!(!manager.reloading.load(Ordering::Acquire));
    }

    #[test]
    fn reload_in_the_manager_thread_starts_at_once() {
        let manager = ThreadManager::default();
        let mut h = Script::with(&[
            (OP_THREAD_MANAGER, handle(&manager)),
            (OP_IS_MANAGER, 1),
            (OP_ALL_CORE_WORKER_NUM, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_THREAD_RELOAD, 1), 1);
        assert_eq!(h.args(OP_KILL_WORKER), vec![0]);
        assert_eq!(h.called(OP_ON_AFTER_RELOAD), 1);
        let mut h = Script::with(&[(OP_THREAD_MANAGER, handle(&manager))]);
        assert_eq!(run_op(&mut h, RUN_THREAD_RELOAD, 0), 0);
        assert_eq!(h.args(LOG_CANNOT_RELOAD), vec![0]);
    }

    #[test]
    fn start_spawns_task_event_and_user_threads_then_the_manager() {
        let mut h = Script::with(&[
            (OP_WORKER_NUM, 2),
            (OP_TASK_WORKER_NUM, 1),
            (OP_USER_WORKER_NUM, 1),
            (OP_MANAGER_THREAD_ID, 4),
            (OP_PORT_COUNT, 2),
            (OP_START_MASTER_THREAD, 0),
        ])
        .queue(OP_PORT_DGRAM, &[1, 0]);
        assert_eq!(run_op(&mut h, RUN_START_WORKER_THREADS, 0), SW_OK);
        assert_eq!(h.args(OP_SPAWN_TASK_THREAD), vec![2]);
        assert_eq!(h.args(OP_SPAWN_EVENT_THREAD), vec![0, 1]);
        assert_eq!(h.args(OP_SPAWN_USER_THREAD), vec![3]);
        assert_eq!(h.args(OP_SPAWN_MANAGER_THREAD), vec![4]);
        assert_eq!(h.args(OP_PORT_ADD), vec![1]);
    }

    #[test]
    fn the_queue_wakes_on_push_or_times_out() {
        let manager = ThreadManager::default();
        assert!(manager.next(5).is_none());
        // SAFETY (test): a live handle.
        unsafe { swr_v1_thread_manager_exited(&raw const manager, 0x10 as *mut c_void) };
        assert!(matches!(manager.next(5), Some(Wake::Exited(w)) if w as usize == 0x10));
    }
}
