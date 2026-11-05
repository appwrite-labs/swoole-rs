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
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "upstream truncates the idle time to whole seconds; only positive values reach here"
)]
fn idle_timeout(idle: f64) -> Duration {
    Duration::from_secs(idle as u64)
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

    fn spawn(&'static self, core: bool) {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        // Registered under the lock, so a worker can never retire before it is known.
        let mut threads = lock(&self.threads);
        match std::thread::Builder::new()
            .stack_size(STACK_SIZE)
            .spawn(move || self.work(core, key))
        {
            Ok(thread) => {
                threads.insert(key, thread);
            }
            Err(_) => (self.hooks.spawn_failed)(),
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

    fn dispatch(&'static self, event: usize) {
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
                break;
            }
            self.waiting.fetch_add(1, Ordering::SeqCst);
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
        }
        (self.hooks.thread_clean)();
    }

    /// Joins a retired worker; false if it is not (or no longer) registered.
    fn join(&self, key: usize) -> bool {
        let thread = lock(&self.threads).remove(&key);
        thread.is_some_and(|thread| thread.join().is_ok())
    }
}

/// The process-wide pool, created once and restarted after a full shutdown.
static SHARED: OnceLock<Pool> = OnceLock::new();
/// References held by reactor threads (upstream's `shared_ptr` use count); its
/// lock is upstream's `async_thread_lock`.
static REFS: Mutex<usize> = Mutex::new(0);

/// Takes a reference to the shared pool, creating it with this configuration and
/// these hooks on first use and starting it if it is not running.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_pool_acquire(
    hooks: PoolHooks,
    cpu: usize,
    core: usize,
    workers: usize,
    wait: f64,
    idle: f64,
) -> *const Pool {
    let mut refs = lock(&REFS);
    let pool = SHARED.get_or_init(|| Pool::new(config(cpu, core, workers, wait, idle), hooks));
    if !lock(&pool.state).running {
        pool.start();
    }
    *refs += 1;
    pool
}

/// Drops a reference; the last one shuts the pool down and joins every worker.
/// # Safety
/// `pool` came from `swr_v1_os_pool_acquire` and is released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_release(pool: *const Pool) {
    let mut refs = lock(&REFS);
    *refs = refs.saturating_sub(1);
    if *refs == 0 {
        // SAFETY: null or the shared pool per the contract.
        if let Some(pool) = unsafe { borrow(pool) } {
            pool.shutdown();
        }
    }
}

/// `AsyncThreads::callback`: reads up to `SW_AIO_EVENT_NUM` (128) completed event
/// tokens from the pipe and hands each to `complete`. Returns how many, or -1 if
/// the read failed (errno intact for native's warning).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_pool_complete(
    fd: c_int,
    complete: extern "C" fn(event: usize),
) -> isize {
    let mut events = [0_usize; 128];
    // SAFETY: `events` is writable for its whole size.
    let n = unsafe { libc::read(fd, events.as_mut_ptr().cast(), size_of_val(&events)) };
    if n < 0 {
        return -1;
    }
    let count = n.unsigned_abs() / size_of::<usize>();
    for &event in events.iter().take(count) {
        complete(event);
    }
    count.cast_signed()
}

/// # Safety
/// `pool` is null or a pointer `swr_v1_os_pool_acquire` returned.
unsafe fn borrow(pool: *const Pool) -> Option<&'static Pool> {
    // SAFETY: null or the address of `SHARED`'s value, which is never dropped.
    unsafe { pool.as_ref() }
}

/// The next task id; ids restart at zero whenever the pool starts.
/// # Safety
/// `pool` is an acquired, unreleased reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_task_id(pool: *const Pool) -> usize {
    // SAFETY: an acquired, unreleased reference per the contract.
    unsafe { borrow(pool) }.map_or(0, |pool| pool.task_id.fetch_add(1, Ordering::Relaxed))
}

/// Queues an event token, grows the pool if needed and wakes one worker.
/// # Safety
/// `pool` is an acquired, unreleased reference; `event` stays valid until a
/// worker hands it to `execute`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_dispatch(pool: *const Pool, event: usize) {
    // SAFETY: an acquired, unreleased reference per the contract.
    if let Some(pool) = unsafe { borrow(pool) } {
        pool.dispatch(event);
    }
}

/// Joins idle worker `key` after its retirement notice reached the main thread.
/// # Safety
/// `pool` is an acquired, unreleased reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_join(pool: *const Pool, key: usize) -> bool {
    // SAFETY: an acquired, unreleased reference per the contract.
    unsafe { borrow(pool) }.is_some_and(|pool| pool.join(key))
}

/// # Safety
/// `pool` is null or an acquired, unreleased reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_worker_num(pool: *const Pool) -> usize {
    // SAFETY: null or an acquired, unreleased reference per the contract.
    unsafe { borrow(pool) }.map_or(0, |pool| lock(&pool.threads).len())
}

/// # Safety
/// `pool` is null or an acquired, unreleased reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_queue_size(pool: *const Pool) -> usize {
    // SAFETY: null or an acquired, unreleased reference per the contract.
    unsafe { borrow(pool) }.map_or(0, |pool| lock(&pool.state).queue.len())
}

/// # Safety
/// `pool` is null or an acquired, unreleased reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_pool_notify_one(pool: *const Pool) {
    // SAFETY: null or an acquired, unreleased reference per the contract.
    if let Some(pool) = unsafe { borrow(pool) } {
        pool.ready.notify_one();
    }
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
    fn grows_to_the_cap_retires_idle_workers_and_restarts() {
        // One core worker, at most four; idle 0.5s truncates to an immediate retirement.
        let pool = swr_v1_os_pool_acquire(HOOKS, 2, 1, 4, 0.0, 0.5);
        unsafe {
            assert_eq!(
                swr_v1_os_pool_worker_num(pool),
                1,
                "core workers start eagerly"
            );
            assert_eq!(swr_v1_os_pool_task_id(pool), 0, "task ids start at zero");
            for event in 0..40 {
                swr_v1_os_pool_dispatch(pool, event);
                PEAK.fetch_max(swr_v1_os_pool_worker_num(pool), Ordering::SeqCst);
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
                    assert!(
                        swr_v1_os_pool_join(pool, key),
                        "a retired worker is registered"
                    );
                }
                swr_v1_os_pool_worker_num(pool) == 1
            });
            assert!(
                !swr_v1_os_pool_join(pool, usize::MAX),
                "unknown workers are reported"
            );
            assert_eq!(swr_v1_os_pool_queue_size(pool), 0, "the queue drains");

            // A second reference keeps the pool alive; the last one shuts it down.
            let second = swr_v1_os_pool_acquire(HOOKS, 0, 0, 0, 0.0, 0.0);
            swr_v1_os_pool_release(pool);
            assert_eq!(
                swr_v1_os_pool_worker_num(second),
                1,
                "another reference keeps it running"
            );
            swr_v1_os_pool_release(second);
            assert!(
                SHARED
                    .get()
                    .is_some_and(|pool| lock(&pool.threads).is_empty()),
                "shutdown joins every worker"
            );

            // Reacquiring restarts the same pool, with its original configuration.
            SLOW.store(false, Ordering::SeqCst);
            let pool = swr_v1_os_pool_acquire(HOOKS, 0, 0, 0, 0.0, 0.0);
            assert_eq!(swr_v1_os_pool_task_id(pool), 0, "task ids restart");
            assert_eq!(
                swr_v1_os_pool_worker_num(pool),
                1,
                "the original core size is kept"
            );
            swr_v1_os_pool_dispatch(pool, 99);
            until(|| EXECUTED.load(Ordering::SeqCst) == 41);
            swr_v1_os_pool_release(pool);
        }
    }

    static COMPLETED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    extern "C" fn complete(event: usize) {
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
            swr_v1_os_pool_complete(reader.as_raw_fd(), complete),
            2,
            "two tokens"
        );
        assert_eq!(*lock(&COMPLETED), [7, 9], "in pipe order");
        assert_eq!(swr_v1_os_pool_complete(-1, complete), -1, "read failure");
    }

    #[test]
    fn null_references_are_inert() {
        unsafe {
            assert_eq!(
                swr_v1_os_pool_worker_num(std::ptr::null()),
                0,
                "no pool, no workers"
            );
            assert_eq!(
                swr_v1_os_pool_queue_size(std::ptr::null()),
                0,
                "no pool, no queue"
            );
            swr_v1_os_pool_notify_one(std::ptr::null());
        }
    }
}
