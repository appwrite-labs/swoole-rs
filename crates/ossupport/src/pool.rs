//! The async thread pool (upstream `swoole::async::ThreadPool`): one process-wide
//! pool shared by every reactor thread, its task queue, worker threads, growth
//! scheduling and idle retirement. Events are opaque native tokens; native code
//! runs them, writes them to the completion pipe and frees them.
use libc::c_int;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use swoole_rs_corebase::CCast;

/// Native callbacks, fixed when the shared pool is first created.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PoolHooks {
    /// `swoole_thread_init(false)`, first thing on a worker thread.
    pub thread_init: extern "C" fn(),
    /// `swoole_thread_clean(false)`, last thing on a worker thread.
    pub thread_clean: extern "C" fn(),
    /// Runs a popped event's handler and sends it to its completion pipe.
    pub execute: extern "C" fn(event: usize),
    /// Sends the main thread a notice to join idle worker `key`.
    pub retire: extern "C" fn(key: usize),
    /// Thread creation failed (upstream's `std::system_error` notice).
    pub spawn_failed: extern "C" fn(),
}

/// Worker threads get the 8 MiB stack a default `std::thread` gets from glibc.
const STACK_SIZE: usize = 8 << 20;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Config {
    core: usize,
    workers: usize,
    wait: f64,
    idle: f64,
}

/// Upstream's `ThreadPool` constructor defaults: `SW_AIO_THREAD_NUM_MULTIPLE`,
/// `SW_AIO_TASK_MAX_WAIT_TIME` and `SW_AIO_THREAD_MAX_IDLE_TIME`.
fn config(cpu: usize, core: usize, workers: usize, wait: f64, idle: f64) -> Config {
    let core = if core == 0 { cpu } else { core.max(1) };
    Config {
        core,
        workers: if workers == 0 {
            cpu.wrapping_mul(8)
        } else {
            workers.max(core)
        },
        wait: if wait == 0.0 { 0.001 } else { wait },
        idle: if idle == 0.0 { 1.0 } else { idle },
    }
}

/// Threads to create (upstream d513884e0): queued tasks beyond the idle workers,
/// or one once the oldest task waited past the threshold, capped at free slots.
fn expand(
    idle: usize,
    queued: usize,
    threads: usize,
    workers: usize,
    threshold: f64,
    age: f64,
) -> usize {
    if threads >= workers || queued == 0 {
        return 0;
    }
    let wanted = if queued > idle {
        queued - idle
    } else {
        usize::from(threshold > 0.0 && age > threshold)
    };
    wanted.min(workers - threads)
}

/// Upstream waits `static_cast<long>(max_idle_time) * 1000 * 1000` microseconds:
/// whole seconds, so an idle time under one second retires immediately.
fn idle_timeout(idle: f64) -> Duration {
    Duration::from_secs(idle.c_cast::<u64>())
}

struct State {
    running: bool,
    queue: VecDeque<(usize, Instant)>,
}

pub struct Pool {
    config: Config,
    hooks: PoolHooks,
    state: Mutex<State>,
    ready: Condvar,
    waiting: AtomicUsize,
    closing: AtomicUsize,
    task_id: AtomicUsize,
    next_key: AtomicUsize,
    threads: Mutex<HashMap<usize, JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pool {
    fn new(config: Config, hooks: PoolHooks) -> Self {
        Self {
            config,
            hooks,
            state: Mutex::new(State {
                running: false,
                queue: VecDeque::new(),
            }),
            ready: Condvar::new(),
            waiting: AtomicUsize::new(0),
            closing: AtomicUsize::new(0),
            task_id: AtomicUsize::new(0),
            next_key: AtomicUsize::new(0),
            threads: Mutex::new(HashMap::new()),
        }
    }

    fn start(&'static self) {
        lock(&self.state).running = true;
        self.task_id.store(0, Ordering::Relaxed);
        for _ in 0..self.config.core {
            self.spawn(true);
        }
    }

    fn shutdown(&self) {
        {
            let mut state = lock(&self.state);
            if !state.running {
                return;
            }
            state.running = false;
        }
        self.ready.notify_all();
        let threads = std::mem::take(&mut *lock(&self.threads));
        // Workers never unwind: a panicking native hook aborts at the FFI edge.
        for (_, thread) in threads {
            drop(thread.join());
        }
    }

    /// A new worker counts as idle from here, not from its first wait: it takes the
    /// next task, so a dispatch before it starts waiting must not grow the pool for it
    /// (upstream counts it only at its wait, so one task right after `start` can add a worker).
    fn spawn(&'static self, core: bool) {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        self.waiting.fetch_add(1, Ordering::SeqCst);
        // Registered under the lock, so a worker can never retire before it is known.
        let mut threads = lock(&self.threads);
        // The worker inherits a fully blocked mask (upstream's `ThreadPool::create_thread`).
        let spawned = swoole_rs_corebase::with_signals_blocked(|| {
            std::thread::Builder::new()
                .stack_size(STACK_SIZE)
                .spawn(move || self.work(core, key))
        });
        if let Ok(thread) = spawned {
            threads.insert(key, thread);
        } else {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            (self.hooks.spawn_failed)();
        }
    }

    fn schedule(&'static self, state: &State) {
        let threads = lock(&self.threads).len();
        let age = state
            .queue
            .front()
            .map_or(0.0, |(_, queued)| queued.elapsed().as_secs_f64());
        let n = expand(
            self.waiting.load(Ordering::SeqCst),
            state.queue.len(),
            threads,
            self.config.workers,
            self.config.wait,
            age,
        );
        for _ in 0..n {
            self.spawn(false);
        }
    }

    /// Queues an event token, grows the pool if needed and wakes one worker.
    /// The token stays valid until a worker hands it to `execute`.
    pub fn dispatch(&'static self, event: usize) {
        {
            let mut state = lock(&self.state);
            state.queue.push_back((event, Instant::now()));
            // After the push, so the queued task is counted; a task that arrives
            // while every worker is blocked would otherwise never grow the pool.
            self.schedule(&state);
        }
        self.ready.notify_one();
    }

    fn work(&self, core: bool, key: usize) {
        (self.hooks.thread_init)();
        loop {
            let mut state = lock(&self.state);
            if !state.running {
                self.waiting.fetch_sub(1, Ordering::SeqCst);
                break;
            }
            let pending = |state: &mut State| state.queue.is_empty() && state.running;
            let timeout = if core || self.config.idle <= 0.0 {
                state = self
                    .ready
                    .wait_while(state, pending)
                    .unwrap_or_else(PoisonError::into_inner);
                false
            } else {
                let (guard, result) = self
                    .ready
                    .wait_timeout_while(state, idle_timeout(self.config.idle), pending)
                    .unwrap_or_else(PoisonError::into_inner);
                state = guard;
                result.timed_out()
            };
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            let event = state.queue.pop_front();
            drop(state);
            if let Some((event, _)) = event {
                (self.hooks.execute)(event);
            } else if timeout && self.closing.load(Ordering::SeqCst) == 0 {
                self.closing.fetch_add(1, Ordering::SeqCst);
                (self.hooks.retire)(key);
                self.closing.fetch_sub(1, Ordering::SeqCst);
                break;
            }
            self.waiting.fetch_add(1, Ordering::SeqCst);
        }
        (self.hooks.thread_clean)();
    }

    /// Joins a retired worker; false if it is not (or no longer) registered.
    pub fn join(&self, key: usize) -> bool {
        let thread = lock(&self.threads).remove(&key);
        thread.is_some_and(|thread| thread.join().is_ok())
    }
}

/// The process-wide pool, created once and restarted after a full shutdown.
static SHARED: OnceLock<Pool> = OnceLock::new();
/// References held by reactor threads (upstream's `shared_ptr` use count); its
/// lock is upstream's `async_thread_lock`.
static REFS: Mutex<usize> = Mutex::new(0);

impl Pool {
    /// Takes a reference to the shared pool, creating it with this configuration
    /// and these hooks on first use and starting it if it is not running.
    pub fn acquire(
        hooks: PoolHooks,
        cpu: usize,
        core: usize,
        workers: usize,
        wait: f64,
        idle: f64,
    ) -> &'static Self {
        let mut refs = lock(&REFS);
        let pool = SHARED.get_or_init(|| Self::new(config(cpu, core, workers, wait, idle), hooks));
        if !lock(&pool.state).running {
            pool.start();
        }
        *refs += 1;
        pool
    }

    /// Drops a reference `acquire` took; the last one shuts the pool down and
    /// joins every worker.
    pub fn release(&self) {
        let mut refs = lock(&REFS);
        *refs = refs.saturating_sub(1);
        if *refs == 0 {
            self.shutdown();
        }
    }

    /// The next task id; ids restart at zero whenever the pool starts.
    pub fn task_id(&self) -> usize {
        self.task_id.fetch_add(1, Ordering::Relaxed)
    }

    #[must_use]
    pub fn worker_num(&self) -> usize {
        lock(&self.threads).len()
    }

    #[must_use]
    pub fn queue_size(&self) -> usize {
        lock(&self.state).queue.len()
    }

    pub fn notify_one(&self) {
        self.ready.notify_one();
    }
}

/// `AsyncThreads::callback`: reads up to `SW_AIO_EVENT_NUM` (128) completed event
/// tokens from the pipe and hands each to `complete`. Returns how many, or none if
/// the read failed (errno intact for native's warning).
pub fn complete(fd: c_int, mut complete: impl FnMut(usize)) -> Option<usize> {
    const TOKEN: usize = size_of::<usize>();
    let mut events = [0_u8; 128 * TOKEN];
    let n = swoole_rs_corebase::sys::read(fd, &mut events);
    let count = usize::try_from(n).ok()? / TOKEN;
    for token in events.as_chunks::<TOKEN>().0.iter().take(count) {
        complete(usize::from_ne_bytes(*token));
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    // Each policy against the native expression it replaces (upstream v6.2.4),
    // over every combination of boundary values.
    const SIZES: [usize; 8] = [0, 1, 2, 8, 64, 128, 1024, usize::MAX];
    const TIMES: [f64; 7] = [-1., -0., 0.0005, 0.5, 1., 100., f64::INFINITY];
    #[test]
    fn pool_policy_matches_the_native_expressions() {
        for cpu in SIZES {
            for core in SIZES {
                for workers in SIZES {
                    for wait in TIMES {
                        for idle in TIMES {
                            let pool = config(cpu, core, workers, wait, idle);
                            let expected_core = if core == 0 { cpu } else { core.max(1) };
                            let expected_workers = if workers == 0 {
                                cpu.wrapping_mul(8)
                            } else {
                                expected_core.max(workers)
                            };
                            assert_eq!(
                                (pool.core, pool.workers),
                                (expected_core, expected_workers)
                            );
                            assert_eq!(
                                pool.wait.to_bits(),
                                if wait == 0. { 0.001_f64 } else { wait }.to_bits()
                            );
                            assert_eq!(
                                pool.idle.to_bits(),
                                if idle == 0. { 1_f64 } else { idle }.to_bits()
                            );
                        }
                    }
                }
            }
        }
        // ThreadPool::schedule.
        for idle in SIZES {
            for queued in SIZES {
                for threads in SIZES {
                    for workers in SIZES {
                        for threshold in TIMES {
                            for age in TIMES {
                                let mut expected = 0;
                                if threads < workers && queued != 0 {
                                    let insufficient = queued > idle;
                                    if insufficient || (threshold > 0. && age > threshold) {
                                        expected = if insufficient { queued - idle } else { 1 }
                                            .min(workers - threads);
                                    }
                                }
                                assert_eq!(
                                    expand(idle, queued, threads, workers, threshold, age),
                                    expected
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn idle_timeout_truncates_to_whole_seconds() {
        assert_eq!(idle_timeout(0.5), Duration::ZERO);
        assert_eq!(idle_timeout(1.9), Duration::from_secs(1));
    }

    static EXECUTED: AtomicUsize = AtomicUsize::new(0);
    static SLOW: AtomicBool = AtomicBool::new(true);
    static RETIRED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn nothing() {}
    extern "C" fn execute(_event: usize) {
        if SLOW.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        EXECUTED.fetch_add(1, Ordering::SeqCst);
    }
    extern "C" fn retire(key: usize) {
        lock(&RETIRED).push(key);
    }
    const HOOKS: PoolHooks = PoolHooks {
        thread_init: nothing,
        thread_clean: nothing,
        execute,
        retire,
        spawn_failed: nothing,
    };

    fn until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not reached in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn grows_to_the_cap_retires_idle_workers_and_restarts() {
        // One core worker, at most four; idle 0.5s truncates to an immediate retirement.
        let pool = Pool::acquire(HOOKS, 2, 1, 4, 0.0, 0.5);
        assert_eq!(pool.worker_num(), 1, "core workers start eagerly");
        assert_eq!(pool.task_id(), 0, "task ids start at zero");
        pool.dispatch(0);
        assert_eq!(
            pool.worker_num(),
            1,
            "a core worker that has not reached its wait yet takes the first task"
        );
        for event in 1..40 {
            pool.dispatch(event);
            PEAK.fetch_max(pool.worker_num(), Ordering::SeqCst);
        }
        assert_eq!(
            PEAK.load(Ordering::SeqCst),
            4,
            "a backlog grows the pool to its cap"
        );
        until(|| EXECUTED.load(Ordering::SeqCst) == 40);
        // The main thread joins each worker whose retirement notice arrives.
        until(|| {
            for key in std::mem::take(&mut *lock(&RETIRED)) {
                assert!(pool.join(key), "a retired worker is registered");
            }
            pool.worker_num() == 1
        });
        assert!(!pool.join(usize::MAX), "unknown workers are reported");
        assert_eq!(pool.queue_size(), 0, "the queue drains");

        // A second reference keeps the pool alive; the last one shuts it down.
        let second = Pool::acquire(HOOKS, 0, 0, 0, 0.0, 0.0);
        pool.release();
        assert_eq!(second.worker_num(), 1, "another reference keeps it running");
        second.release();
        assert!(
            SHARED
                .get()
                .is_some_and(|pool| lock(&pool.threads).is_empty()),
            "shutdown joins every worker"
        );

        // Reacquiring restarts the same pool, with its original configuration.
        SLOW.store(false, Ordering::SeqCst);
        let pool = Pool::acquire(HOOKS, 0, 0, 0, 0.0, 0.0);
        assert_eq!(pool.task_id(), 0, "task ids restart");
        assert_eq!(pool.worker_num(), 1, "the original core size is kept");
        pool.dispatch(99);
        until(|| EXECUTED.load(Ordering::SeqCst) == 41);
        pool.release();
    }

    static COMPLETED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    fn complete(event: usize) {
        lock(&COMPLETED).push(event);
    }

    #[test]
    fn completions_are_read_whole_tokens_at_a_time() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let (reader, mut writer) =
            std::os::unix::net::UnixStream::pair().unwrap_or_else(|e| unreachable!("{e}"));
        let mut bytes = Vec::new();
        for token in [7_usize, 9] {
            bytes.extend_from_slice(&token.to_ne_bytes());
        }
        bytes.push(1); // a torn trailing token is ignored, as upstream
        writer
            .write_all(&bytes)
            .unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(
            super::complete(reader.as_raw_fd(), complete),
            Some(2),
            "two tokens"
        );
        assert_eq!(*lock(&COMPLETED), [7, 9], "in pipe order");
        assert_eq!(super::complete(-1, complete), None, "read failure");
    }

    const NSIG: c_int = if cfg!(target_os = "linux") { 65 } else { 32 };
    static UNBLOCKED: AtomicBool = AtomicBool::new(false);
    static OBSERVED: AtomicUsize = AtomicUsize::new(0);
    static RAN: AtomicUsize = AtomicUsize::new(0);

    fn mask() -> libc::sigset_t {
        let mut mask = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &raw mut mask) },
            0
        );
        mask
    }
    /// The worker's inherited mask, before `swoole_thread_init` (native's `thread_init`)
    /// blocks signals itself.
    extern "C" fn observe() {
        let mask = mask();
        for signo in [libc::SIGCHLD, libc::SIGUSR1, libc::SIGTERM] {
            if unsafe { libc::sigismember(&raw const mask, signo) } != 1 {
                UNBLOCKED.store(true, Ordering::SeqCst);
            }
        }
        OBSERVED.fetch_add(1, Ordering::SeqCst);
    }
    extern "C" fn ran(_event: usize) {
        RAN.fetch_add(1, Ordering::SeqCst);
    }
    extern "C" fn kept(_key: usize) {}

    /// core-tests/src/os/async.cpp `TEST(async, thread_starts_with_signals_blocked)`, each
    /// case in a new thread rather than a forked child (the mask and the flag are per
    /// thread), over a pool of its own, as the child's process had. The `thread_init` hook
    /// observes the worker's mask where upstream's `std_allocator.malloc` hook did.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigemptyset")]
    fn thread_starts_with_signals_blocked() {
        for already_blocked in [false, true] {
            std::thread::spawn(move || unsafe {
                let mut expected = std::mem::zeroed();
                libc::sigemptyset(&raw mut expected);
                libc::sigaddset(&raw mut expected, libc::SIGUSR1);
                assert_eq!(
                    libc::pthread_sigmask(
                        libc::SIG_SETMASK,
                        &raw const expected,
                        std::ptr::null_mut()
                    ),
                    0
                );
                if already_blocked {
                    swoole_rs_corebase::signal_block_all();
                }
                let expected = mask();
                UNBLOCKED.store(false, Ordering::SeqCst);
                OBSERVED.store(0, Ordering::SeqCst);
                let hooks = PoolHooks {
                    thread_init: observe,
                    retire: kept,
                    execute: ran,
                    ..HOOKS
                };
                // aio_core_worker_num = 1, aio_worker_num = 2.
                let pool: &'static Pool =
                    Box::leak(Box::new(Pool::new(config(1, 1, 2, 0.0, 0.0), hooks)));
                let before = RAN.load(Ordering::SeqCst);
                pool.start();
                pool.dispatch(0);
                until(|| RAN.load(Ordering::SeqCst) > before);
                pool.shutdown();

                let context = format!("already_blocked={already_blocked}");
                assert!(!UNBLOCKED.load(Ordering::SeqCst), "{context}");
                assert!(OBSERVED.load(Ordering::SeqCst) > 0, "{context}");
                assert_eq!(
                    swoole_rs_corebase::signal_blocking_all(),
                    already_blocked,
                    "{context}"
                );
                let actual = mask();
                for signo in 1..NSIG {
                    assert_eq!(
                        libc::sigismember(&raw const expected, signo),
                        libc::sigismember(&raw const actual, signo),
                        "{context}"
                    );
                }
            })
            .join()
            .unwrap();
        }
    }
}
