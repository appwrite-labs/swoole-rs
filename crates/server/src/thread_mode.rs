//! Thread mode (upstream's `src/server/thread.cc`): every worker is a thread, and a
//! manager thread rejoins and respawns each one that exits and walks reloads one
//! worker at a time. The manager's state ([`ThreadManager`]: its threads, wait queue
//! and reload) and the hook a thread's body runs inside are here; the threads'
//! bodies, the factory and the server's thread-mode methods are in `native`.
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use swoole_rs_corebase::Thread;

pub mod native;

/// What the manager thread waits for.
enum Wake {
    /// The worker thread in this slot exited.
    Exited(u16),
    Reload(bool),
    Exit,
}

/// The manager's reload targets and the slots still awaiting replacement.
struct Reload {
    targets: std::ops::Range<u16>,
    pending: swoole_rs_corebase::memory::flat::BitSet,
    remaining: usize,
}

impl Reload {
    fn new(targets: std::ops::Range<u16>) -> Self {
        let mut pending = swoole_rs_corebase::memory::flat::BitSet::new();
        for _ in targets.clone() {
            pending.push(true);
        }
        Self {
            remaining: targets.len(),
            pending,
            targets,
        }
    }

    fn complete(&mut self, slot: u16) {
        let first = usize::from(self.targets.end) - self.pending.len();
        if let Some(index) = usize::from(slot).checked_sub(first)
            && self.pending.get(index) == Some(true)
        {
            self.pending.set(index, false);
            self.remaining -= 1;
        }
    }

    fn advance(&mut self, mut stop: impl FnMut(u16) -> bool) {
        while let Some(slot) = self.targets.next() {
            if stop(slot) {
                break;
            }
            self.complete(slot);
        }
    }
}

/// The thread factory's manager state, behind `ThreadFactory`'s opaque handle, and
/// a thread for each worker and the manager.
pub struct ThreadManager {
    wake: Sender<Wake>,
    woken: Mutex<Receiver<Wake>>,
    reloading: AtomicBool,
    shutting_down: AtomicBool,
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
            shutting_down: AtomicBool::default(),
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
        if self.shutting_down.load(Ordering::Acquire) || self.reloading.swap(true, Ordering::AcqRel)
        {
            return false;
        }
        self.push(Wake::Reload(all));
        true
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What runs a thread-mode worker thread's body (`Server::worker_thread_start`'s
/// `fn`) on its `swoole::Thread`, which that thread's [`swoole_rs_corebase::Fresh`]
/// lets enter PHP. The body is the thread's to keep: it may run it from deeper in
/// the thread's own calls.
pub type WorkerThreadStart = fn(swoole_rs_corebase::Fresh, &Thread, Rc<dyn Fn()>);

static WORKER_THREAD_START: Mutex<Option<WorkerThreadStart>> = Mutex::new(None);

/// `serv->worker_thread_start = start`; `None` restores upstream's default,
/// which runs the body directly.
pub fn set_worker_thread_start(start: Option<WorkerThreadStart>) {
    *lock(&WORKER_THREAD_START) = start;
}

/// `serv->worker_thread_start`: the starter [`set_worker_thread_start`] registered,
/// or `None` for upstream's default, which runs the body directly. Copied out: a
/// guard held while the body runs would make every other thread's start wait for it.
fn worker_thread_start() -> Option<WorkerThreadStart> {
    *lock(&WORKER_THREAD_START)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_wakes_on_push_or_times_out() {
        let manager = ThreadManager::new(0);
        assert!(manager.next(5).is_none());
        manager.push(Wake::Exited(3));
        manager.push(Wake::Reload(false));
        manager.push(Wake::Exit);
        assert!(matches!(manager.next(5), Some(Wake::Exited(3))));
        assert!(matches!(manager.next(0), Some(Wake::Reload(false))));
        assert!(matches!(manager.next(5), Some(Wake::Exit)));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn a_thread_body_runs_directly_or_inside_the_servers_hook() {
        static RAN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        fn hook(_: swoole_rs_corebase::Fresh, thread: &Thread, run: Rc<dyn Fn()>) {
            assert!(!thread.joinable());
            // The body is the hook's to keep.
            let kept = run;
            kept();
        }
        // A thread's start, as `native`'s `body` runs its main, on a thread of its own.
        let start = |offset: u32| {
            let runner = Thread::default();
            let body = move |fresh| {
                let thread = Thread::default();
                let run = Rc::new(move || {
                    // Other threads can start while this body runs.
                    assert!(WORKER_THREAD_START.try_lock().is_ok());
                    RAN.store(offset, Ordering::Release);
                });
                match worker_thread_start() {
                    Some(start) => start(fresh, &thread, run),
                    None => run(),
                }
            };
            runner.start(body).unwrap();
            assert_eq!(runner.join(), Ok(()));
        };
        start(0x10);
        assert_eq!(RAN.swap(0, Ordering::AcqRel), 0x10);
        set_worker_thread_start(Some(hook));
        start(1);
        set_worker_thread_start(None);
        assert_eq!(RAN.load(Ordering::Acquire), 1);
    }

    #[test]
    fn a_reload_kills_one_worker_per_step_until_done() {
        let manager = ThreadManager::new(0);
        assert!(manager.begin_reload(false));
        assert!(!manager.begin_reload(true));
        assert!(matches!(manager.next(0), Some(Wake::Reload(false))));
        let mut reload = Reload::new(2..5);
        let mut requested = Vec::new();
        reload.advance(|slot| {
            requested.push(slot);
            slot != 2
        });
        assert_eq!(requested, [2, 3]);
        reload.complete(3);
        reload.complete(3);
        reload.complete(0);
        reload.advance(|slot| {
            requested.push(slot);
            true
        });
        assert_eq!(requested, [2, 3, 4]);
        assert_eq!(reload.remaining, 1);
        reload.complete(4);
        assert_eq!(reload.remaining, 0);
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
            .start(move |_| flag.store(true, Ordering::Release))
            .unwrap();
        manager.join_all();
        assert!(seen.load(Ordering::Acquire));
        assert!(!manager.thread(1).joinable());
    }

    #[test]
    fn worker_types_are_upstreams() {
        use swoole_rs_corebase::process::worker_record::WorkerType;
        // Upstream's swoole_server.h: SW_EVENT_WORKER = 2, SW_TASK_WORKER = 4, SW_USER_WORKER = 5.
        let kinds = [WorkerType::Event, WorkerType::Task, WorkerType::User];
        assert_eq!(kinds.map(WorkerType::code), [2, 4, 5]);
    }
}
