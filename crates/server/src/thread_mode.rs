//! Thread mode (upstream's `src/server/thread.cc`): every worker is a thread, and a
//! manager thread rejoins and respawns each one that exits and walks reloads one
//! worker at a time. The manager's state ([`ThreadManager`]: its threads, wait queue
//! and reload) and the hook a thread's body runs inside are here; the threads'
//! bodies, the factory and the server's thread-mode methods are in `native`.
#![cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(dead_code, reason = "only the hybrid runs thread mode")
)]
use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use swoole_rs_corebase::Thread;

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod native;

/// Thread mode's `Server` methods, which the hybrid runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadOp {
    /// `Server::create_thread_factory`.
    CreateFactory,
    /// `Server::destroy_thread_factory`.
    DestroyFactory,
    /// `Server::start_worker_threads`.
    StartWorkers,
    /// `Server::stop_worker_threads`.
    StopWorkers,
}

/// What the manager thread waits for.
enum Wake {
    /// The worker thread in this slot exited.
    Exited(u16),
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
pub struct ThreadManager {
    wake: Sender<Wake>,
    woken: Mutex<Receiver<Wake>>,
    reloading: AtomicBool,
    reload_all: AtomicBool,
    reload: Mutex<Option<Reload>>,
    threads: Box<[Arc<Thread>]>,
}

impl ThreadManager {
    /// `new ThreadFactory(serv)`'s state: `count` threads, none started.
    #[must_use]
    pub fn new(count: usize) -> Self {
        let (wake, woken) = channel();
        Self {
            wake,
            woken: Mutex::new(woken),
            reloading: AtomicBool::default(),
            reload_all: AtomicBool::default(),
            reload: Mutex::default(),
            threads: (0..count).map(|_| Arc::default()).collect(),
        }
    }

    /// The thread in slot `index`; an index past the table was upstream's
    /// out-of-range `std::shared_ptr` dereference.
    #[must_use]
    pub fn thread(&self, index: usize) -> &Arc<Thread> {
        let thread = self.threads.get(index);
        thread.unwrap_or_else(|| swoole_rs_corebase::fatal("no worker thread at this index"))
    }

    /// `ThreadFactory::shutdown()`: joins every started thread. A failed join was a
    /// `std::system_error` nothing caught, which terminated the process.
    pub fn join_all(&self) {
        for thread in &self.threads {
            if thread.joinable() && thread.join().is_err() {
                swoole_rs_corebase::fatal("a worker thread's join failed");
            }
        }
    }

    fn push(&self, wake: Wake) {
        // The manager holds the receiver for as long as anyone can send.
        _ = self.wake.send(wake);
    }

    /// The next wake-up, waiting at most `timeout_ms` when it is positive.
    fn next(&self, timeout_ms: i64) -> Option<Wake> {
        let woken = lock(&self.woken);
        if timeout_ms > 0 {
            let timeout = Duration::from_millis(timeout_ms.cast_unsigned());
            woken.recv_timeout(timeout).ok()
        } else {
            woken.recv().ok()
        }
    }

    /// `ThreadFactory::reload`'s request: false while another runs.
    fn begin_reload(&self, all: bool) -> bool {
        let (from, to) = (false, true);
        let begun = self
            .reloading
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire);
        if begun.is_ok() {
            self.reload_all.store(all, Ordering::Release);
        }
        begun.is_ok()
    }

    /// Whether a reload is under way.
    fn reloading(&self) -> bool {
        self.reloading.load(Ordering::Acquire)
    }

    /// `ThreadFactory::do_reload`: `kill` the next worker of `workers` (a reload of
    /// the task workers skips the first `events`), after `before` when the reload
    /// starts; once the last has been asked, the reload is done and `after` runs.
    fn reload_next(
        &self,
        (workers, events): (u16, u16),
        before: impl FnOnce(),
        kill: impl FnOnce(u16),
        after: impl FnOnce(),
    ) {
        let mut reload = lock(&self.reload);
        let task = reload.unwrap_or_else(|| {
            let all = self.reload_all.load(Ordering::Acquire);
            before();
            Reload {
                workers,
                reloaded: if all { 0 } else { events },
            }
        });
        kill(task.reloaded);
        let reloaded = task.reloaded.wrapping_add(1);
        if reloaded == task.workers {
            *reload = None;
            self.reloading.store(false, Ordering::Release);
            drop(reload);
            after();
        } else {
            *reload = Some(Reload { reloaded, ..task });
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A worker thread's body (`Server::worker_thread_start`'s `fn`).
pub type WorkerMain = unsafe extern "C" fn(*mut c_void);

/// What runs a thread-mode worker thread's body on its `swoole::Thread`.
pub type WorkerThreadStart = fn(&Thread, WorkerMain, *mut c_void);

static WORKER_THREAD_START: Mutex<Option<WorkerThreadStart>> = Mutex::new(None);

/// `serv->worker_thread_start = start`; `None` restores upstream's default,
/// which runs the body directly.
pub fn set_worker_thread_start(start: Option<WorkerThreadStart>) {
    *lock(&WORKER_THREAD_START) = start;
}

/// `serv->worker_thread_start(thread, fn)`, from a `ThreadFactory` thread.
///
/// # Safety
/// `run(context)` is the thread's body, safe to call on this thread.
unsafe fn worker_thread_start(thread: &Thread, run: WorkerMain, context: *mut c_void) {
    // Copied out: a guard in the match would be held while the body runs, and every
    // other thread's start would wait for it.
    let start = *lock(&WORKER_THREAD_START);
    match start {
        Some(start) => start(thread, run, context),
        // SAFETY: per the caller.
        None => unsafe { run(context) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;

    #[test]
    fn the_queue_wakes_on_push_or_times_out() {
        let manager = ThreadManager::new(0);
        assert!(manager.next(5).is_none());
        manager.push(Wake::Exited(3));
        manager.push(Wake::Reload);
        manager.push(Wake::Exit);
        assert!(matches!(manager.next(5), Some(Wake::Exited(3))));
        assert!(matches!(manager.next(0), Some(Wake::Reload)));
        assert!(matches!(manager.next(5), Some(Wake::Exit)));
    }

    #[test]
    fn a_thread_body_runs_directly_or_inside_the_servers_hook() {
        static RAN: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn run(context: *mut c_void) {
            assert_eq!(context.addr(), 0x10);
            // Other threads can start while this body runs.
            assert!(WORKER_THREAD_START.try_lock().is_ok());
            RAN.store(true, Ordering::Release);
        }
        fn hook(thread: &Thread, run: WorkerMain, context: *mut c_void) {
            assert!(!thread.joinable());
            // SAFETY (test): the body above takes any context.
            unsafe { run(context.wrapping_byte_add(0x10)) };
        }
        let thread = Thread::default();
        // SAFETY (test): as above.
        unsafe { worker_thread_start(&thread, run, core::ptr::without_provenance_mut(0x10)) };
        assert!(RAN.swap(false, Ordering::AcqRel));
        set_worker_thread_start(Some(hook));
        // SAFETY (test): as above.
        unsafe { worker_thread_start(&thread, run, core::ptr::null_mut()) };
        set_worker_thread_start(None);
        assert!(RAN.load(Ordering::Acquire));
    }

    #[test]
    fn a_reload_kills_one_worker_per_step_until_done() {
        let manager = ThreadManager::new(0);
        let log = RefCell::new(Vec::new());
        let step = || {
            manager.reload_next(
                (4, 2),
                || log.borrow_mut().push("before".to_owned()),
                |id| log.borrow_mut().push(format!("kill {id}")),
                || log.borrow_mut().push("after".to_owned()),
            );
        };
        // Task workers only: the event workers are skipped.
        assert!(manager.begin_reload(false));
        // A second request while one runs is refused.
        assert!(!manager.begin_reload(true));
        step();
        assert!(manager.reloading());
        step();
        assert!(!manager.reloading());
        assert_eq!(*log.borrow(), ["before", "kill 2", "kill 3", "after"]);
        // A reload of every worker starts at the first.
        log.borrow_mut().clear();
        assert!(manager.begin_reload(true));
        step();
        assert_eq!(log.borrow()[..2], ["before", "kill 0"]);
    }

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

    #[test]
    fn worker_types_are_upstreams() {
        use swoole_rs_process::worker_record::WorkerType;
        // Upstream's swoole_server.h: SW_EVENT_WORKER = 2, SW_TASK_WORKER = 4, SW_USER_WORKER = 5.
        let kinds = [WorkerType::Event, WorkerType::Task, WorkerType::User];
        assert_eq!(kinds.map(WorkerType::code), [2, 4, 5]);
    }
}
