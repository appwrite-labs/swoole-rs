//! Thread mode (upstream's `src/server/thread.cc`; workers.cc's `ThreadContext`):
//! every worker is a thread, and a manager
//! thread rejoins and respawns each one that exits and walks reloads one worker
//! at a time. Rust owns the manager's state ([`ThreadManager`]: its threads, wait
//! queue, reload request and reload progress) and each thread's body;
//! crates/server's `thread_mode_ops.rs` answers the ops over the server.
use crate::worker_record::Worker;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use std::time::Duration;
use swoole_rs_corebase::Thread;

use super::event_worker::EVENT_WORKER;
use super::log::{
    LOG_CANNOT_RELOAD, LOG_MANAGER_ABNORMAL, LOG_NOTIFY_MANAGER, LOG_RELOADING,
    LOG_THREAD_UNSUPPORTED,
};
use super::ops::{
    LOG_CALLOC_CONNECTIONS, OP_ABORT, OP_ALL_CORE_WORKER_NUM, OP_ALL_WORKER_NUM,
    OP_CALL_WORKER_ERROR, OP_CALLOC_CONNECTIONS, OP_CREATE_EVENT_WORKERS, OP_CREATE_MESSAGE_BUS,
    OP_CREATE_USER_WORKERS, OP_CREATE_WORKER_PIPES, OP_DELETE_REACTOR_THREADS,
    OP_DESTROY_MESSAGE_BUS, OP_EVENT_FREE, OP_EVENT_INIT, OP_FREE_THREAD_CONNECTIONS,
    OP_GET_WORKER, OP_HAS_TIMER, OP_IS_MANAGER, OP_JOIN_THREAD, OP_KILL_WORKER,
    OP_MANAGER_THREAD_ID, OP_NEW_REACTOR_THREADS, OP_NEW_THREAD_FACTORY, OP_ON_AFTER_RELOAD,
    OP_ON_BEFORE_RELOAD, OP_ON_MANAGER_START, OP_ON_MANAGER_STOP, OP_ON_USER_WORKER_START,
    OP_ON_WORKER_START, OP_ON_WORKER_STOP, OP_POOL_MAIN_LOOP, OP_PORT_ADD, OP_PORT_COUNT,
    OP_PORT_DGRAM, OP_PORT_LISTEN, OP_REACTOR_THREAD_MAIN, OP_RUNNING, OP_SELECT_WORKER,
    OP_SET_MASTER_THREAD_ID, OP_SET_NOT_SUPPORTED, OP_SINGLE_REACTOR_PIPE, OP_SPAWN_THREAD,
    OP_START_MASTER_THREAD, OP_STOPPING, OP_STORE_LISTEN_SOCKET, OP_TASK_START_CHECK,
    OP_TASK_WORKER_NUM, OP_TERMINATE_MANAGER_THREAD, OP_THREAD_ENTER, OP_THREAD_EXIT,
    OP_THREAD_EXIT_STATUS, OP_THREAD_MANAGER, OP_THREAD_REACTOR_NUM, OP_THREAD_SUPPORTED,
    OP_TIMER_CREATE, OP_TIMER_FREE, OP_TIMER_NEXT_MSEC, OP_TIMER_SELECT, OP_USER_WORKER_NUM,
    OP_WORKER_ID, OP_WORKER_NUM, OP_WORKER_THREAD_START, OP_WORKER_TYPE, RUN_CREATE_THREAD_FACTORY,
    RUN_DESTROY_THREAD_FACTORY, RUN_START_WORKER_THREADS, RUN_STOP_WORKER_THREADS, RUN_THREAD_BODY,
    RUN_THREAD_FACTORY_START, RUN_THREAD_ID, RUN_THREAD_MAIN, RUN_THREAD_MANAGER_WAIT,
    RUN_THREAD_RELOAD, RUN_THREAD_TERMINATE_MANAGER,
};
use super::{Host, SW_ERR, SW_OK};
use crate::methods::{swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type};
use crate::worker_globals::thread_globals;
use crate::worker_record::WorkerRecord;

/// `swoole::WorkerType`.
const MANAGER: u64 = 3;
const TASK_WORKER: u64 = 4;
const USER_WORKER: u64 = 5;
/// `SW_WORKER_IDLE`.
const WORKER_IDLE: u8 = 2;

/// `OP_SPAWN_THREAD`'s, `RUN_THREAD_BODY`'s and `RUN_THREAD_MAIN`'s argument:
/// the thread's slot and the kind of worker it runs.
fn thread_arg(id: u64, kind: u64) -> u64 {
    id | (kind << 32)
}

/// What the manager thread waits for.
enum Wake {
    /// A worker thread exited (its native `Worker *`'s address).
    Exited(u64),
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

/// The thread factory's manager state, behind `ThreadFactory`'s opaque handle, and
/// a thread for each worker and the manager.
#[derive(Default)]
pub struct ThreadManager {
    queue: Mutex<VecDeque<Wake>>,
    ready: Condvar,
    reloading: AtomicBool,
    reload_all: AtomicBool,
    reload: Mutex<Option<Reload>>,
    threads: Box<[Arc<Thread>]>,
}

impl ThreadManager {
    /// `new ThreadFactory(serv)`'s state: `count` threads, none started.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            threads: (0..count).map(|_| Arc::default()).collect(),
            ..Self::default()
        }
    }

    /// The thread in slot `index`; an index past the table was upstream's
    /// out-of-range `std::shared_ptr` dereference.
    #[must_use]
    pub fn thread(&self, index: u64) -> &Arc<Thread> {
        let thread = usize::try_from(index)
            .ok()
            .and_then(|index| self.threads.get(index));
        thread.unwrap_or_else(|| std::process::abort())
    }

    /// `ThreadFactory::shutdown()`: joins every started thread. A failed join was a
    /// `std::system_error` nothing caught, which terminated the process.
    pub fn join_all(&self) {
        for thread in &self.threads {
            if thread.joinable() && thread.join().is_err() {
                std::process::abort();
            }
        }
    }

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
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
            SW_OK
        }
        RUN_START_WORKER_THREADS => start(h),
        RUN_THREAD_BODY => {
            body(h, arg);
            SW_OK
        }
        RUN_THREAD_MAIN => {
            thread_main(h, arg);
            SW_OK
        }
        RUN_THREAD_FACTORY_START => i64::from(factory_start(h)),
        RUN_STOP_WORKER_THREADS => {
            h.get(OP_TERMINATE_MANAGER_THREAD);
            for worker in 0..h.get(OP_ALL_CORE_WORKER_NUM) {
                h.op(OP_KILL_WORKER, worker);
            }
            SW_OK
        }
        RUN_CREATE_THREAD_FACTORY => create_factory(h),
        RUN_DESTROY_THREAD_FACTORY => {
            h.get(OP_FREE_THREAD_CONNECTIONS);
            h.get(OP_DELETE_REACTOR_THREADS);
            SW_OK
        }
        // The manager thread's id (`arg` 0) follows every worker's; the master's (1) is next.
        RUN_THREAD_ID => i64::from(
            u32::try_from(h.get(OP_ALL_WORKER_NUM))
                .unwrap_or(u32::MAX)
                .wrapping_add(u32::try_from(arg).unwrap_or(0)),
        ),
        _ => SW_ERR,
    }
}

/// `Server::create_thread_factory`: one reactor per worker over a private
/// connection list and one pipe each; null without thread support or memory.
fn create_factory<H: Host>(h: &mut H) -> i64 {
    if !h.is(OP_THREAD_SUPPORTED) {
        h.log(LOG_THREAD_UNSUPPORTED, 0, 0);
        return 0;
    }
    h.get(OP_THREAD_REACTOR_NUM);
    if !h.is(OP_CALLOC_CONNECTIONS) {
        h.get(LOG_CALLOC_CONNECTIONS);
        return 0;
    }
    h.get(OP_NEW_REACTOR_THREADS);
    h.get(OP_SINGLE_REACTOR_PIPE);
    h.signed(OP_NEW_THREAD_FACTORY)
}

/// `ThreadFactory::wait`: the manager thread's loop.
fn wait<H: Host>(h: &mut H) {
    let manager = manager(h);
    loop {
        let wake = manager.next(h.signed(OP_TIMER_NEXT_MSEC));
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
fn restart<H: Host>(h: &mut H, worker: u64) {
    h.op(OP_SELECT_WORKER, worker);
    let id = h.get(OP_WORKER_ID);
    if h.get(OP_THREAD_EXIT_STATUS) != 0 {
        h.get(OP_CALL_WORKER_ERROR);
    }
    h.op(OP_JOIN_THREAD, id);
    // A worker that exits once the server is stopping stays down: the master is
    // joining every thread, and a new one could block it forever (a user process
    // re-run waits on input its first run consumed).
    if h.is(OP_STOPPING) {
        return;
    }
    match h.get(OP_WORKER_TYPE) {
        kind @ (EVENT_WORKER | TASK_WORKER | USER_WORKER) => {
            h.op(OP_SPAWN_THREAD, thread_arg(id, kind))
        }
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
    let what = i64::from(all);
    if h.get(OP_TASK_WORKER_NUM) == 0 && !all {
        h.log(LOG_CANNOT_RELOAD, what, 0);
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
        h.log(LOG_RELOADING, what, 0);
        do_reload(h, manager);
    } else {
        h.log(LOG_NOTIFY_MANAGER, what, 0);
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
        h.op(OP_SPAWN_THREAD, thread_arg(workers + worker, TASK_WORKER));
    }
    for worker in 0..workers {
        h.op(OP_SPAWN_THREAD, thread_arg(worker, EVENT_WORKER));
    }
    for worker in 0..h.get(OP_USER_WORKER_NUM) {
        h.op(
            OP_SPAWN_THREAD,
            thread_arg(task_workers + workers + worker, USER_WORKER),
        );
    }
    let id = h.get(OP_MANAGER_THREAD_ID);
    h.op(OP_SPAWN_THREAD, thread_arg(id, MANAGER));
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

/// `ThreadFactory::start`: the pipes and the event, task and user workers' records.
fn factory_start<H: Host>(h: &mut H) -> bool {
    h.is(OP_CREATE_WORKER_PIPES)
        && h.is(OP_CREATE_EVENT_WORKERS)
        && (h.get(OP_TASK_WORKER_NUM) == 0 || h.signed(OP_TASK_START_CHECK) >= 0)
        && (h.get(OP_USER_WORKER_NUM) == 0 || h.is(OP_CREATE_USER_WORKERS))
}

/// A thread's body (`ThreadFactory::spawn_*`): enter the thread as its worker
/// (or the manager), run its main inside the server's `worker_thread_start`,
/// then leave, telling the manager thread when a worker exited.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the kind, slot and native thread id narrow as the C++ setters' parameters do"
)]
fn body<H: Host>(h: &mut H, arg: u64) {
    let (id, kind) = (arg & u64::from(u32::MAX), arg >> 32);
    let pid = h.op(OP_THREAD_ENTER, id) as i32;
    swoole_set_worker_type(kind as i32);
    swoole_set_worker_id(id as u32);
    swoole_set_worker_pid(pid);
    let message_bus = kind == TASK_WORKER || kind == USER_WORKER;
    if message_bus {
        h.get(OP_CREATE_MESSAGE_BUS);
    }
    let mut worker = core::ptr::null_mut::<WorkerRecord>();
    if kind == MANAGER {
        h.get(OP_TIMER_CREATE);
    } else {
        worker = usize::try_from(h.op(OP_GET_WORKER, id)).unwrap_or(0) as *mut WorkerRecord;
        // SAFETY: native lends the server's live record for this slot, which this thread runs.
        let worker = unsafe { Worker::new(worker) };
        worker.set_kind(kind as u8);
        worker.set_pid(pid);
        if kind == TASK_WORKER {
            worker.set_status(WORKER_IDLE);
        }
        // SAFETY: this thread's live worker-globals record.
        unsafe { (*thread_globals()).worker = worker.raw() };
    }
    h.op(OP_WORKER_THREAD_START, arg);
    if message_bus {
        h.get(OP_DESTROY_MESSAGE_BUS);
    }
    if kind == MANAGER {
        if h.is(OP_RUNNING) {
            h.log(LOG_MANAGER_ABNORMAL, 0, 0);
        }
        h.get(OP_TIMER_FREE);
    } else {
        manager(h).push(Wake::Exited(worker.expose_provenance() as u64));
    }
    h.op(OP_THREAD_EXIT, id);
}

/// A worker thread's body (`Server::worker_thread_start`'s `fn`).
pub type WorkerMain = unsafe extern "C" fn(*mut c_void);

/// What runs a thread-mode worker thread's body on its `swoole::Thread`.
pub type WorkerThreadStart = unsafe fn(*mut c_void, WorkerMain, *mut c_void);

static WORKER_THREAD_START: Mutex<Option<WorkerThreadStart>> = Mutex::new(None);

/// `serv->worker_thread_start = start`; `None` restores upstream's default,
/// which runs the body directly.
pub fn set_worker_thread_start(start: Option<WorkerThreadStart>) {
    *WORKER_THREAD_START
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = start;
}

/// `serv->worker_thread_start(thread, fn)`, from a `ThreadFactory` thread.
///
/// # Safety
/// `thread` is the thread's live `swoole::Thread` and `run(context)` its body.
pub unsafe fn worker_thread_start(thread: *mut c_void, run: WorkerMain, context: *mut c_void) {
    let start = *WORKER_THREAD_START
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    match start {
        // SAFETY: per the caller.
        Some(start) => unsafe { start(thread, run, context) },
        // SAFETY: per the caller.
        None => unsafe { run(context) },
    }
}

/// What a thread runs inside `worker_thread_start`.
fn thread_main<H: Host>(h: &mut H, arg: u64) {
    match arg >> 32 {
        EVENT_WORKER => {
            h.op(OP_REACTOR_THREAD_MAIN, arg & u64::from(u32::MAX));
        }
        TASK_WORKER => {
            h.get(OP_ON_WORKER_START);
            h.get(OP_POOL_MAIN_LOOP);
            h.get(OP_ON_WORKER_STOP);
        }
        USER_WORKER => {
            h.get(OP_ON_USER_WORKER_START);
        }
        _ => {
            h.get(OP_ON_MANAGER_START);
            wait(h);
            h.get(OP_ON_MANAGER_STOP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::script::Script;

    #[test]
    fn the_thread_factory_needs_thread_support_and_its_connections() {
        let reads = [OP_THREAD_SUPPORTED, OP_CALLOC_CONNECTIONS];
        let mut h = Script::with(&[]);
        assert_eq!(run(&mut h, RUN_CREATE_THREAD_FACTORY, 0), 0);
        assert_eq!(h.effects(&reads), [LOG_THREAD_UNSUPPORTED]);
        let mut h = Script::with(&[(OP_THREAD_SUPPORTED, 1)]);
        assert_eq!(run(&mut h, RUN_CREATE_THREAD_FACTORY, 0), 0);
        assert_eq!(
            h.effects(&reads),
            [OP_THREAD_REACTOR_NUM, LOG_CALLOC_CONNECTIONS]
        );
        let mut h = Script::with(&[
            (OP_THREAD_SUPPORTED, 1),
            (OP_CALLOC_CONNECTIONS, 1),
            (OP_NEW_THREAD_FACTORY, 0x40),
        ]);
        assert_eq!(run(&mut h, RUN_CREATE_THREAD_FACTORY, 0), 0x40);
        assert_eq!(
            h.effects(&reads),
            [
                OP_THREAD_REACTOR_NUM,
                OP_NEW_REACTOR_THREADS,
                OP_SINGLE_REACTOR_PIPE,
                OP_NEW_THREAD_FACTORY
            ]
        );
        let mut h = Script::with(&[(OP_ALL_WORKER_NUM, 6)]);
        assert_eq!(run(&mut h, RUN_THREAD_ID, 0), 6, "the manager thread");
        assert_eq!(run(&mut h, RUN_THREAD_ID, 1), 7, "the master thread");
    }

    #[test]
    fn a_task_thread_runs_as_its_worker_with_a_message_bus_and_reports_its_exit() {
        let manager = ThreadManager::default();
        // SAFETY (test): a zeroed record stands in for the server's.
        let mut record: WorkerRecord = unsafe { core::mem::zeroed() };
        let worker = core::ptr::from_mut(&mut record);
        let address = worker as u64;
        let (h, globals) = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let mut h = Script::with(&[
                        (OP_THREAD_MANAGER, handle(&manager)),
                        (OP_THREAD_ENTER, 4321),
                        (OP_GET_WORKER, address),
                    ]);
                    run_op(&mut h, RUN_THREAD_BODY, thread_arg(6, TASK_WORKER));
                    let g = thread_globals();
                    // SAFETY (test): this thread's record.
                    let globals = unsafe { ((*g).id, (*g).kind, (*g).pid, (*g).worker as u64) };
                    (h, globals)
                })
                .join()
                .unwrap()
        });
        assert_eq!(globals, (6, 4, 4321, address));
        // SAFETY (test): the thread above has ended.
        let fields = unsafe { (Worker::new(worker).kind(), Worker::new(worker).pid()) };
        assert_eq!(fields, (4, 4321));
        // SAFETY (test): as above.
        assert_eq!(unsafe { Worker::new(worker).status() }, WORKER_IDLE);
        assert_eq!(
            h.effects(&[OP_THREAD_MANAGER]),
            vec![
                OP_THREAD_ENTER,
                OP_CREATE_MESSAGE_BUS,
                OP_GET_WORKER,
                OP_WORKER_THREAD_START,
                OP_DESTROY_MESSAGE_BUS,
                OP_THREAD_EXIT,
            ]
        );
        assert_eq!(
            h.args(OP_WORKER_THREAD_START),
            vec![thread_arg(6, TASK_WORKER)]
        );
        assert!(matches!(manager.next(5), Some(Wake::Exited(w)) if w == address));
    }

    #[test]
    fn the_manager_thread_owns_a_timer_and_warns_if_it_exits_while_running() {
        let manager = ThreadManager::default();
        let mut h = Script::with(&[(OP_THREAD_MANAGER, handle(&manager)), (OP_RUNNING, 1)]);
        std::thread::scope(|scope| {
            scope.spawn(|| run_op(&mut h, RUN_THREAD_BODY, thread_arg(9, MANAGER)));
        });
        assert_eq!(
            h.effects(&[OP_THREAD_MANAGER, OP_RUNNING]),
            vec![
                OP_THREAD_ENTER,
                OP_TIMER_CREATE,
                OP_WORKER_THREAD_START,
                LOG_MANAGER_ABNORMAL,
                OP_TIMER_FREE,
                OP_THREAD_EXIT,
            ]
        );
        assert!(manager.next(1).is_none());
        // Its main waits on the queue between the server's callbacks.
        manager.push(Wake::Exit);
        let mut h = Script::with(&[(OP_THREAD_MANAGER, handle(&manager))]);
        run_op(&mut h, RUN_THREAD_MAIN, thread_arg(9, MANAGER));
        assert_eq!(h.called(OP_ON_MANAGER_START), 1);
        assert_eq!(h.called(OP_ON_MANAGER_STOP), 1);
    }

    #[test]
    fn each_worker_kind_runs_its_own_main() {
        let mut h = Script::default();
        run_op(&mut h, RUN_THREAD_MAIN, thread_arg(2, EVENT_WORKER));
        run_op(&mut h, RUN_THREAD_MAIN, thread_arg(3, TASK_WORKER));
        run_op(&mut h, RUN_THREAD_MAIN, thread_arg(4, USER_WORKER));
        assert_eq!(h.args(OP_REACTOR_THREAD_MAIN), vec![2]);
        assert_eq!(
            h.effects(&[]),
            vec![
                OP_REACTOR_THREAD_MAIN,
                OP_ON_WORKER_START,
                OP_POOL_MAIN_LOOP,
                OP_ON_WORKER_STOP,
                OP_ON_USER_WORKER_START,
            ]
        );
    }

    #[test]
    fn factory_start_stops_at_the_first_failure() {
        let mut h = Script::with(&[
            (OP_CREATE_WORKER_PIPES, 1),
            (OP_CREATE_EVENT_WORKERS, 1),
            (OP_TASK_WORKER_NUM, 1),
            (OP_TASK_START_CHECK, u64::MAX),
        ]);
        assert_eq!(run_op(&mut h, RUN_THREAD_FACTORY_START, 0), 0);
        assert_eq!(h.called(OP_CREATE_USER_WORKERS), 0);
        h.facts.insert(OP_TASK_START_CHECK, 0);
        h.facts.insert(OP_USER_WORKER_NUM, 1);
        assert_eq!(run_op(&mut h, RUN_THREAD_FACTORY_START, 0), 0);
        h.facts.insert(OP_CREATE_USER_WORKERS, 1);
        assert_eq!(run_op(&mut h, RUN_THREAD_FACTORY_START, 0), 1);
    }

    #[test]
    fn worker_types_are_upstreams() {
        // Upstream's swoole_server.h: SW_EVENT_WORKER = 2, SW_TASK_WORKER = 4, SW_USER_WORKER = 5.
        assert_eq!((EVENT_WORKER, TASK_WORKER, USER_WORKER), (2, 4, 5));
        assert_eq!((super::super::manager::worker_types()), (2, 4, 5));
    }

    fn run_op(h: &mut Script, operation: u32, arg: u64) -> i64 {
        crate::workers::run(h, operation, arg)
    }

    fn handle(manager: &ThreadManager) -> u64 {
        core::ptr::from_ref(manager) as u64
    }

    #[test]
    fn the_manager_thread_respawns_exited_workers_until_told_to_exit() {
        let manager = ThreadManager::default();
        let worker = 0x1000;
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
        assert_eq!(h.args(OP_SPAWN_THREAD), vec![thread_arg(3, TASK_WORKER)]);
        assert_eq!(h.called(OP_TIMER_SELECT), 1);
    }

    #[test]
    fn a_worker_exiting_while_the_server_stops_is_joined_not_respawned() {
        let manager = ThreadManager::default();
        manager.push(Wake::Exited(0x1000));
        manager.push(Wake::Exit);
        let mut h = Script::with(&[
            (OP_THREAD_MANAGER, handle(&manager)),
            (OP_WORKER_ID, 5),
            (OP_WORKER_TYPE, USER_WORKER),
            (OP_RUNNING, 1),
            (OP_STOPPING, 1),
        ]);
        run_op(&mut h, RUN_THREAD_MANAGER_WAIT, 0);
        assert_eq!(h.args(OP_JOIN_THREAD), vec![5], "joined");
        assert_eq!(h.called(OP_SPAWN_THREAD), 0, "not respawned");
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
        assert_eq!(
            h.args(OP_SPAWN_THREAD),
            vec![
                thread_arg(2, TASK_WORKER),
                thread_arg(0, EVENT_WORKER),
                thread_arg(1, EVENT_WORKER),
                thread_arg(3, USER_WORKER),
                thread_arg(4, MANAGER),
            ]
        );
        assert_eq!(h.args(OP_PORT_ADD), vec![1]);
    }

    #[test]
    fn the_queue_wakes_on_push_or_times_out() {
        let manager = ThreadManager::default();
        assert!(manager.next(5).is_none());
        manager.push(Wake::Exited(0x10));
        assert!(matches!(manager.next(5), Some(Wake::Exited(0x10))));
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn the_manager_owns_a_thread_per_slot_and_joins_only_started_ones() {
        let manager = ThreadManager::new(3);
        assert!(Arc::ptr_eq(manager.thread(0), manager.thread(0)));
        assert!(!Arc::ptr_eq(manager.thread(0), manager.thread(2)));
        let seen = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&seen);
        manager
            .thread(1)
            .start(move || flag.store(true, Ordering::Release))
            .unwrap();
        manager.join_all();
        assert!(seen.load(Ordering::Acquire));
        assert!(!manager.thread(1).joinable());
    }
}
