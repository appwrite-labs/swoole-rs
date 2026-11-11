//! Upstream's `core-tests/src/os/wait.cpp`, `os/async.cpp` and `os/os.cpp`'s `os.daemon`,
//! one function per TEST with the same inputs and expected values: the coroutine waits
//! (crates/process's registry, crates/reactor's symbols), the async thread pool
//! (crates/hooks over crates/ossupport) and `swoole_daemon` (crates/ossupport), under
//! their upstream symbols. `async.thread_starts_with_signals_blocked` is crates/ossupport's
//! `pool::tests::thread_starts_with_signals_blocked`.

use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicI32, Ordering};

use super::timer::{swoole_timer_del, tick};
use super::{_exit, fork, getpid, run, spawn_exec};

const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_ERROR_AIO_BAD_REQUEST: c_int = 4001;
const STDIN_FILENO: c_int = 0;

/// `swoole::AsyncEvent` (native/ossupport/swoole_async.h).
#[repr(C)]
pub(super) struct AsyncEvent {
    task_id: usize,
    canceled: u8,
    error: c_int,
    data: *mut c_void,
    retval: isize,
    pipe_socket: *mut c_void,
    timestamp: f64,
    object: *mut c_void,
    handler: Option<extern "C" fn(*mut AsyncEvent)>,
    callback: Option<extern "C" fn(*mut AsyncEvent)>,
}

impl AsyncEvent {
    /// `AsyncEvent event = {}`, with these members set.
    pub(super) fn new(
        object: *mut c_void,
        handler: Option<extern "C" fn(*mut AsyncEvent)>,
        callback: Option<extern "C" fn(*mut AsyncEvent)>,
    ) -> Self {
        Self {
            task_id: 0,
            canceled: 0,
            error: 0,
            data: std::ptr::null_mut(),
            retval: 0,
            pipe_socket: std::ptr::null_mut(),
            timestamp: 0.0,
            object,
            handler,
            callback,
        }
    }
}

unsafe extern "C" {
    fn swoole_coroutine_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn swoole_coroutine_wait(status: *mut c_int) -> c_int;
    #[link_name = "_ZN6swoole9coroutine6System12waitpid_safeEiPii"]
    fn system_waitpid_safe(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    #[link_name = "_Z14swoole_waitpidiPii"]
    fn swoole_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    #[link_name = "_Z13swoole_daemonii"]
    fn swoole_daemon(nochdir: c_int, noclose: c_int) -> c_int;
    #[link_name = "_Z11swoole_randii"]
    fn swoole_rand(min: c_int, max: c_int) -> c_int;
    #[link_name = "_ZN6swoole5async8dispatchEPKNS_10AsyncEventE"]
    pub(super) fn async_dispatch(event: *const AsyncEvent) -> *mut AsyncEvent;
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    fn swoole_event_wait() -> c_int;
    /// `sw_async_threads()`'s `get_task_num()`, `get_worker_num()` and `get_queue_size()`.
    fn swr_v1_async_threads_stats(task_num: &mut i64, worker_num: &mut i64, queue_size: &mut i64);
    fn usleep(usec: u32) -> c_int;
    fn getsid(pid: c_int) -> c_int;
    fn isatty(fd: c_int) -> c_int;
}

/// `fork_child`: the child exits after 100 ms.
fn fork_child() -> c_int {
    // SAFETY: fork(2); the child sleeps and exits.
    let pid = unsafe { fork() };
    assert_ne!(pid, -1);
    if pid == 0 {
        // SAFETY: the child's own sleep and exit.
        unsafe {
            usleep(100_000);
            _exit(0);
        }
    }
    pid
}

/// `fork_child2`: the child exits at once; the parent waits 100 ms.
fn fork_child2() -> c_int {
    // SAFETY: fork(2); the child exits.
    let pid = unsafe { fork() };
    assert_ne!(pid, -1);
    if pid == 0 {
        // SAFETY: ends the child.
        unsafe { _exit(0) };
    }
    // SAFETY: usleep(3).
    unsafe { usleep(100_000) };
    pid
}

fn coroutine_waitpid(pid: c_int) -> (c_int, c_int) {
    let mut status = -1;
    // SAFETY: a writable status, in a coroutine.
    let pid2 = unsafe { swoole_coroutine_waitpid(pid, &raw mut status, 0) };
    (pid2, status)
}

/// `for (;;) { pid2 = swoole_coroutine_wait(&status); if (pid2 == pid) break; }`.
fn coroutine_wait_for(pid: c_int) -> c_int {
    let mut status = -1;
    loop {
        // SAFETY: a writable status, in a coroutine.
        if unsafe { swoole_coroutine_wait(&raw mut status) } == pid {
            return status;
        }
    }
}

fn waitpid_before_child_exit() {
    run(|| {
        let pid = fork_child();
        let (pid2, status) = coroutine_waitpid(pid);
        assert_eq!(status, 0);
        assert_eq!(pid, pid2);
    });
}

fn waitpid_after_child_exit() {
    run(|| {
        let pid = fork_child2();
        let (pid2, status) = coroutine_waitpid(pid);
        assert_eq!(status, 0);
        assert_eq!(pid, pid2);
    });
}

fn wait_before_child_exit() {
    run(|| {
        let pid = fork_child();
        assert_eq!(wexitstatus(coroutine_wait_for(pid)), 0);
    });
}

fn wait_after_child_exit() {
    run(|| {
        let pid = fork_child2();
        assert_eq!(wexitstatus(coroutine_wait_for(pid)), 0);
    });
}

fn waitpid_safe() {
    run(|| {
        let pid = fork_child();
        let mut status = -1;
        // SAFETY: a writable status, in a coroutine.
        let pid2 = unsafe { system_waitpid_safe(pid, &raw mut status, 0) };
        assert_eq!(pid2, pid);
        assert_eq!(wexitstatus(status), 0);
    });
}

fn wexitstatus(status: c_int) -> c_int {
    (status >> 8) & 0xff
}

/// `callback_count`: callbacks run on the dispatching thread.
static CALLBACK_COUNT: AtomicI32 = AtomicI32::new(0);

extern "C" fn count_callback(_: *mut AsyncEvent) {
    CALLBACK_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub(super) extern "C" fn count_handle(event: *mut AsyncEvent) {
    // SAFETY: the event's object is the case's live counter.
    unsafe { (*(*event).object.cast::<AtomicI32>()).fetch_add(1, Ordering::SeqCst) };
}

extern "C" fn sleep_and_count_handle(event: *mut AsyncEvent) {
    // SAFETY: usleep(3) on a pool thread.
    unsafe { usleep(swoole_rand(50_000, 100_000).unsigned_abs()) };
    count_handle(event);
}

fn dispatch(event: &AsyncEvent) -> *mut AsyncEvent {
    // SAFETY: a live event whose handler may run on a pool thread.
    unsafe { async_dispatch(event) }
}

/// `(get_worker_num(), get_queue_size(), get_task_num())`.
fn stats() -> (i64, i64, i64) {
    let (mut task_num, mut worker_num, mut queue_size) = (0, 0, 0);
    // SAFETY: three writable counts.
    unsafe { swr_v1_async_threads_stats(&mut task_num, &mut worker_num, &mut queue_size) };
    (worker_num, queue_size, task_num)
}

pub(super) fn event_init() {
    // SAFETY: a fresh loop on this thread.
    unsafe { swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) };
}

pub(super) fn event_wait() {
    // SAFETY: the loop `event_init` made.
    unsafe { swoole_event_wait() };
}

fn async_dispatch_() {
    let count = 1000;
    CALLBACK_COUNT.store(0, Ordering::SeqCst);
    let handle_count = AtomicI32::new(0);
    let event = AsyncEvent::new(
        (&raw const handle_count).cast_mut().cast(),
        Some(count_handle),
        Some(count_callback),
    );

    event_init();

    for _ in 0..count {
        let ret = dispatch(&event);
        // SAFETY: the pool's live copy.
        assert_eq!(unsafe { (*ret).object }, event.object);
    }

    event_wait();

    assert_eq!(handle_count.load(Ordering::SeqCst), count);
    assert_eq!(CALLBACK_COUNT.load(Ordering::SeqCst), count);
}

fn schedule() {
    CALLBACK_COUNT.store(0, Ordering::SeqCst);
    let handle_count = AtomicI32::new(0);

    let n: i64 = 1000;

    event_init();

    let event = AsyncEvent::new(
        (&raw const handle_count).cast_mut().cast(),
        Some(sleep_and_count_handle),
        Some(count_callback),
    );

    // SwooleG's settings, before the pool exists.
    swoole_rs_corebase::swr_v1_core_set_aio_core_worker_num(4);
    swoole_rs_corebase::swr_v1_core_set_aio_worker_num(128);
    swoole_rs_corebase::swr_v1_core_set_aio_max_wait_time_bits(0.05_f64.to_bits());
    swoole_rs_corebase::swr_v1_core_set_aio_max_idle_time_bits(0.5_f64.to_bits());

    let count = Cell::new(n);
    let event = &event;
    tick(2, |timer| {
        // SW_LOOP_N(swoole_rand(5, 15)): the bound is drawn again before every iteration.
        let mut i = 0;
        // SAFETY: swoole_rand(3).
        while i < unsafe { swoole_rand(5, 15) } {
            i += 1;
            let ret = dispatch(event);
            // SAFETY: the pool's live copy.
            assert_eq!(unsafe { (*ret).object }, event.object);
            count.set(count.get() - 1);
            let (worker_num, queue_size, task_num) = stats();
            if count.get() == 0 {
                // SAFETY: the running tick's node.
                unsafe { swoole_timer_del(timer) };
                assert!(worker_num > 16);
                assert!(queue_size > 100);
                assert!(task_num > 100);
                break;
            } else if count.get() == n - 1 {
                assert_eq!(worker_num, 4);
                assert!(queue_size <= 1);
                assert_eq!(task_num, 1);
            } else if count.get() < n / 2 {
                assert!(worker_num > 4);
            }
        }
    });

    // core-tests' DEBUG() lines, which print only in its debug builds, are left out.
    tick(2000, |timer| {
        if stats().0 < 16 {
            // SAFETY: the running tick's node.
            unsafe { swoole_timer_del(timer) };
        }
    });

    event_wait();

    assert_eq!(i64::from(handle_count.load(Ordering::SeqCst)), n);
    assert_eq!(i64::from(CALLBACK_COUNT.load(Ordering::SeqCst)), n);
}

extern "C" fn bad_request_callback(event: *mut AsyncEvent) {
    // SAFETY: the completed event.
    let event = unsafe { &*event };
    assert_eq!(event.retval, -1);
    assert_eq!(event.error, SW_ERROR_AIO_BAD_REQUEST);
    CALLBACK_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn misc() {
    CALLBACK_COUNT.store(0, Ordering::SeqCst);
    let handle_count = AtomicI32::new(0);
    let event = AsyncEvent::new(
        (&raw const handle_count).cast_mut().cast(),
        Some(count_handle),
        Some(count_callback),
    );

    event_init();

    let ret = dispatch(&event);
    // SAFETY: the pool's live copy.
    assert_eq!(unsafe { (*ret).object }, event.object);

    // sw_async_threads()->notify_one() has no exported counterpart (tests README).

    let event2 = AsyncEvent::new(std::ptr::null_mut(), None, Some(bad_request_callback));
    let rv = dispatch(&event2);
    assert!(!rv.is_null());

    event_wait();

    assert_eq!(handle_count.load(Ordering::SeqCst), 1);
    assert_eq!(CALLBACK_COUNT.load(Ordering::SeqCst), 2);
}

fn daemon() {
    // SAFETY: getsid(2) and getpid(2).
    let sid = unsafe { getsid(getpid()) };
    let mut status = 0;
    let pid = spawn_exec(|| {
        // SAFETY: as above, and isatty(3).
        unsafe {
            assert_eq!(sid, getsid(getpid()));
            assert!(isatty(STDIN_FILENO) != 0);

            assert_eq!(swoole_daemon(0, 0), 0);
            assert_ne!(sid, getsid(getpid()));

            assert!(isatty(STDIN_FILENO) == 0);
        }
    });
    // As upstream, the child's status is discarded.
    // SAFETY: our child, a writable status.
    unsafe { swoole_waitpid(pid, &raw mut status, 0) };
}

/// `os_wait.<name>`, in upstream's order.
pub(super) const WAIT_CASES: super::Cases = &[
    ("waitpid_before_child_exit", waitpid_before_child_exit),
    ("waitpid_after_child_exit", waitpid_after_child_exit),
    ("wait_before_child_exit", wait_before_child_exit),
    ("wait_after_child_exit", wait_after_child_exit),
    ("waitpid_safe", waitpid_safe),
];

/// `async.<name>`, in upstream's order.
pub(super) const ASYNC_CASES: super::Cases = &[
    ("dispatch", async_dispatch_),
    ("schedule", schedule),
    ("misc", misc),
];

/// `os.daemon`.
pub(super) const OS_CASES: super::Cases = &[("daemon", daemon)];
