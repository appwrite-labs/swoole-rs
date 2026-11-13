//! The former `gate.cc`, check for check: crates/locks' `Lock`, `Barrier`, atomics,
//! futex wait and coroutine lock against the pinned pthread bodies and the driver that
//! ran both sides (`swoole_rs_oracle::locks`). Each lock ABI export the gate called is
//! replaced by the safe method that was its body (README.md maps them).
use std::cell::Cell;
use std::ffi::c_int;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use libc::{EBUSY, ETIMEDOUT, LOCK_EX, LOCK_NB, LOCK_SH};
use swoole_rs_error::ErrorCode;
use swoole_rs_locks::coroutine::{CoroutineHost, CoroutineLock};
use swoole_rs_locks::{
    Barrier, Lock, SharedMutex, barrier_destroy, barrier_init, barrier_wait, swr_v1_locks_atomic32,
    swr_v1_locks_atomic64, swr_v1_locks_coroutine_backoff,
};
use swoole_rs_oracle::locks as reference;

const RW_LOCK: i32 = 1;
const MUTEX: i32 = 3;
#[cfg(target_os = "linux")]
const SPIN_LOCK: i32 = 5;
// What configure finds, as the oracle is built: Linux has pthread_mutex_timedlock (1)
// and pthread_rwlock_timed{rd,wr}lock (2).
const CAPABILITIES: u32 = if cfg!(target_os = "linux") { 3 } else { 0 };

/// A zeroed page shared across fork, as `sw_mem_pool`'s are.
fn page<T>() -> *mut T {
    // SAFETY: an anonymous mapping, which borrows nothing.
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(page, libc::MAP_FAILED, "mmap");
    page.cast()
}

/// The child's exit status.
fn wait_for(child: libc::pid_t) -> c_int {
    let mut status = 0;
    // SAFETY: `status` is live for the call.
    let waited = unsafe { libc::waitpid(child, &raw mut status, 0) };
    assert!(
        waited == child && libc::WIFEXITED(status),
        "the child exits"
    );
    libc::WEXITSTATUS(status)
}

/// The gate's `RustLock<Kind>`: a private lock, or a shared one in a page of its own,
/// which is never unmapped (`Lock::shared` keeps its storage).
struct RustLock(Lock);

impl RustLock {
    fn new(kind: i32, shared: bool) -> Self {
        let lock = if shared {
            let words = Lock::words(kind);
            assert!(words * size_of::<AtomicU64>() <= 4096, "a page holds it");
            // SAFETY: zeroed words of a page nothing else uses, mapped for the program's life.
            Lock::shared(kind, unsafe {
                std::slice::from_raw_parts_mut(page::<AtomicU64>(), words)
            })
        } else {
            Lock::private(kind)
        };
        let Some(lock) = lock else {
            unreachable!("a lock of kind {kind}");
        };
        Self(lock)
    }
    fn lock(&self, operation: i32, timeout: i32) -> i32 {
        self.0.lock(operation, timeout, CAPABILITIES)
    }
    fn unlock(&self) -> i32 {
        self.0.unlock()
    }
}

/// `main`'s geometry checks: the storage `Lock::shared` asks for, and `SharedMutex`,
/// are the pinned `MutexImpl` and `RWLockImpl`.
#[test]
#[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
fn layout() {
    assert_eq!(
        Lock::words(MUTEX) * size_of::<AtomicU64>(),
        reference::size(MUTEX)
    );
    assert_eq!(align_of::<AtomicU64>(), reference::align(MUTEX));
    assert_eq!(
        Lock::words(RW_LOCK) * size_of::<AtomicU64>(),
        reference::size(RW_LOCK)
    );
    assert_eq!(align_of::<AtomicU64>(), reference::align(RW_LOCK));
    assert_eq!(size_of::<SharedMutex>(), reference::size(MUTEX));
    assert_eq!(align_of::<SharedMutex>(), reference::align(MUTEX));
}

/// `entry::exercise() == exercise<RustLock<3>, RustLock<1>, swoole::Barrier>()`.
#[test]
#[cfg_attr(
    miri,
    ignore = "Miri can't call the C++ oracle, fork or share a mapping"
)]
fn exercise_matches_the_pinned_locks() {
    /// `exercise<RustLock<3>, RustLock<1>, swoole::Barrier>()`.
    fn exercise() -> Vec<i32> {
        let mut results = Vec::new();
        let mutex = RustLock::new(MUTEX, false);
        // A plain read-modify-write: only the mutex keeps the threads' increments apart.
        let counter = AtomicI32::new(0);
        thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..2000 {
                        assert_eq!(mutex.lock(LOCK_EX, -1), 0);
                        counter.store(counter.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
                        assert_eq!(mutex.unlock(), 0);
                    }
                });
            }
        });
        assert_eq!(counter.into_inner(), 8000);
        assert_eq!(mutex.lock(LOCK_EX, -1), 0);
        for operation in [LOCK_NB, LOCK_NB | LOCK_SH, LOCK_NB | LOCK_EX, -1] {
            results.push(mutex.lock(operation, 10));
        }
        results.push(mutex.lock(LOCK_EX, 5));
        assert_eq!(results.last(), Some(&ETIMEDOUT));
        assert_eq!(mutex.unlock(), 0);
        results.push(mutex.lock(LOCK_EX, 5));
        assert_eq!(results.last(), Some(&0));
        assert_eq!(mutex.unlock(), 0);
        // Zero/negative timeouts retain blocking semantics, not immediate timeouts.
        for timeout in [0, -1, -100] {
            assert_eq!(mutex.lock(LOCK_EX, -1), 0);
            let result = thread::scope(|scope| {
                let waiter = scope.spawn(|| {
                    let result = mutex.lock(LOCK_EX, timeout);
                    if result == 0 {
                        assert_eq!(mutex.unlock(), 0);
                    }
                    result
                });
                thread::sleep(Duration::from_micros(2000));
                assert_eq!(mutex.unlock(), 0);
                waiter.join().expect("the waiter")
            });
            assert_eq!(result, 0);
            results.push(result);
        }
        let rw = RustLock::new(RW_LOCK, false);
        assert_eq!(rw.lock(LOCK_SH, -1), 0);
        let (read, write) = thread::scope(|scope| {
            scope
                .spawn(|| {
                    let write = rw.lock(LOCK_EX | LOCK_NB, -1);
                    let read = rw.lock(LOCK_SH | LOCK_NB, -1);
                    if read == 0 {
                        assert_eq!(rw.unlock(), 0);
                    }
                    (read, write)
                })
                .join()
                .expect("the readers")
        });
        assert_eq!(read, 0);
        assert_ne!(write, 0);
        results.push(write);
        assert_eq!(rw.unlock(), 0);
        assert_eq!(rw.lock(LOCK_EX, -1), 0);
        let timed = thread::scope(|scope| {
            scope
                .spawn(|| rw.lock(LOCK_SH, 5))
                .join()
                .expect("the timed reader")
        });
        assert_eq!(timed, ETIMEDOUT);
        results.push(timed);
        assert_eq!(rw.unlock(), 0);
        // Parent/child share the same pthread mutex and payload.
        let shared = RustLock::new(MUTEX, true);
        let payload = page::<AtomicI32>();
        // SAFETY: a zeroed word of a shared page, unmapped below.
        let word = unsafe { &*payload };
        assert_eq!(shared.lock(LOCK_EX, -1), 0);
        // SAFETY: the child only locks, stores, unlocks and exits.
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            let code = if shared.lock(LOCK_EX, -1) != 0 {
                11
            } else {
                word.store(12345, Ordering::Relaxed);
                if shared.unlock() == 0 { 0 } else { 12 }
            };
            // SAFETY: ends the child without unwinding into the test harness.
            unsafe { libc::_exit(code) };
        }
        thread::sleep(Duration::from_micros(2000));
        assert_eq!(shared.unlock(), 0);
        assert_eq!(wait_for(child), 0);
        assert_eq!(word.load(Ordering::Relaxed), 12345);
        // SAFETY: the page mapped above, which nothing refers to after this.
        unsafe { libc::munmap(payload.cast(), 4096) };
        // SAFETY: all-zero is an unset barrier, as C++'s `Barrier barrier{}`.
        let barrier = Box::new(unsafe { std::mem::zeroed::<Barrier>() });
        barrier_init(&barrier, false, 4);
        let slots = [const { AtomicI32::new(0) }; 4];
        thread::scope(|scope| {
            for slot in &slots {
                let (barrier, slots) = (&barrier, &slots);
                scope.spawn(move || {
                    slot.store(1, Ordering::Relaxed);
                    barrier_wait(barrier);
                    for slot in slots {
                        assert_eq!(slot.load(Ordering::Relaxed), 1);
                    }
                });
            }
        });
        barrier_destroy(&barrier);
        results
    }

    let mut pinned = [0; 32];
    // SAFETY: `pinned` is writable for its length.
    let count = unsafe { reference::exercise(pinned.as_mut_ptr(), pinned.len()) };
    assert_eq!(pinned.get(..count), Some(&exercise()[..]));
}

/// `entry::spin()` against `spin<RustLock<5>>()`, element by element.
#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    miri,
    ignore = "Miri can't call the C++ oracle and has no pthread_spin_init"
)]
fn spin_matches_the_pinned_spinlock() {
    let mut pinned = [0; 4];
    // SAFETY: four writable results.
    unsafe { reference::spin(&raw mut pinned) };
    let lock = RustLock::new(SPIN_LOCK, false);
    let rust = [
        lock.lock(LOCK_EX, -1),
        lock.lock(LOCK_NB, -1),
        lock.lock(LOCK_EX, 3),
        lock.unlock(),
    ];
    for (pinned, rust) in pinned.iter().zip(rust) {
        assert_eq!(*pinned, rust);
    }
}

/// `swr_v1_locks_size(99) == 0` and `swr_v1_locks_init(nullptr, 3, 0) == EINVAL`, as the
/// safe constructors refuse them: no such kind, and no storage.
#[test]
fn unknown_kinds_and_missing_storage_are_refused() {
    assert_eq!(Lock::words(99), 0);
    assert!(Lock::private(99).is_none());
    assert!(Lock::shared(99, &mut []).is_none());
    assert!(Lock::shared(MUTEX, &mut []).is_none());
}

/// `shared_barrier<swoole::Barrier>()`, and the pinned one where it is the word barrier.
#[test]
#[cfg_attr(miri, ignore = "Miri can't fork or share a mapping")]
fn shared_barrier() {
    let page = page::<AtomicU64>();
    // SAFETY: the zeroed page is an unset barrier (C++'s `new(page) Barrier{}`) and,
    // 2048 bytes in, a word; both are unmapped below.
    let (barrier, value) = unsafe {
        (
            &*page.cast::<Barrier>(),
            &*page.wrapping_add(256).cast::<AtomicI32>(),
        )
    };
    barrier_init(barrier, true, 2);
    // SAFETY: the child only stores, waits on the barrier and exits.
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        value.store(9876, Ordering::Relaxed);
        barrier_wait(barrier);
        // SAFETY: ends the child without unwinding into the test harness.
        unsafe { libc::_exit(0) };
    }
    barrier_wait(barrier);
    assert_eq!(value.load(Ordering::Relaxed), 9876);
    assert_eq!(wait_for(child), 0);
    barrier_destroy(barrier);
    // SAFETY: the page mapped above, which nothing refers to after this.
    unsafe { libc::munmap(page.cast(), 4096) };
    #[cfg(not(target_os = "linux"))]
    reference::shared_barrier();
}

/// `atomics()`: the atomic exports against the `__sync` builtins over the gate's
/// xorshift sequence, then four threads' 20,000 increments each.
#[test]
#[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
fn atomics() {
    let mut state: u64 = 0x2857_4a92;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // `(uint32_t) v` and `(int64_t) v`.
    let low = |v: u64| u32::from_le_bytes(*v.to_le_bytes().first_chunk().expect("four bytes"));
    let signed = u64::cast_signed;
    let (mut a, b) = (0u32, AtomicU32::new(0));
    let (mut c, d) = (0i64, AtomicI64::new(0));
    // SAFETY: each is called on a live local word.
    let sync32 = |a: &mut u32, operation, value, expected| unsafe {
        reference::sync32(a, operation, value, expected)
    };
    // SAFETY: as above.
    let sync64 = |c: &mut i64, operation, value, expected| unsafe {
        reference::sync64(c, operation, value, expected)
    };
    for i in 0..20000 {
        let v = next();
        let expected = if i % 3 != 0 { low(next()) } else { a };
        assert_eq!(
            swr_v1_locks_atomic32(Some(&b), 2, low(v), 0),
            sync32(&mut a, 2, low(v), 0)
        );
        assert_eq!(
            swr_v1_locks_atomic32(Some(&b), 3, low(v >> 32), 0),
            sync32(&mut a, 3, low(v >> 32), 0)
        );
        assert_eq!(
            swr_v1_locks_atomic32(Some(&b), 4, low(v), expected),
            sync32(&mut a, 4, low(v), expected)
        );
        assert_eq!(swr_v1_locks_atomic32(Some(&b), 0, 0, 0), a);
        swr_v1_locks_atomic32(Some(&b), 1, low(v), 0);
        a = low(v);
        let expected = if i % 3 != 0 { signed(next()) } else { c };
        assert_eq!(
            swr_v1_locks_atomic64(Some(&d), 2, signed(v), 0),
            sync64(&mut c, 2, signed(v), 0)
        );
        assert_eq!(
            swr_v1_locks_atomic64(Some(&d), 3, signed(v >> 1), 0),
            sync64(&mut c, 3, signed(v >> 1), 0)
        );
        assert_eq!(
            swr_v1_locks_atomic64(Some(&d), 4, signed(v), expected),
            sync64(&mut c, 4, signed(v), expected)
        );
        assert_eq!(swr_v1_locks_atomic64(Some(&d), 0, 0, 0), c);
        swr_v1_locks_atomic64(Some(&d), 1, signed(v), 0);
        c = signed(v);
    }
    let sum = AtomicI64::new(0);
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..20000 {
                    swr_v1_locks_atomic64(Some(&sum), 2, 1, 0);
                }
            });
        }
    });
    assert_eq!(sum.into_inner(), 80000);
    assert_eq!(swr_v1_locks_atomic32(None, 2, 1, 0), 0);
}

/// One side of `wait_wake()`: a ready token is consumed, an empty word times out, and a
/// forked waiter is woken.
fn wait_wake(wait: impl Fn(&AtomicU32, f64) -> i32, wake: impl Fn(&AtomicU32, i32) -> i32) {
    let page = page::<AtomicU32>();
    // SAFETY: a zeroed word of a shared page, unmapped below.
    let word = unsafe { &*page };
    word.store(1, Ordering::Relaxed);
    assert_eq!(wait(word, 0.01), 0, "a ready token");
    assert_eq!(word.load(Ordering::Relaxed), 0, "consumed");
    assert_eq!(wait(word, 0.002), -1, "a timeout");
    assert_eq!(word.load(Ordering::Relaxed), 0, "still empty");
    // SAFETY: the child only waits on the word and exits.
    let child = unsafe { libc::fork() };
    assert!(child >= 0, "fork");
    if child == 0 {
        let code = if wait(word, 1.0) == 0 { 0 } else { 21 };
        // SAFETY: ends the child without unwinding into the test harness.
        unsafe { libc::_exit(code) };
    }
    thread::sleep(Duration::from_micros(10000));
    assert!(wake(word, 1) >= 0, "a wake");
    assert_eq!(wait_for(child), 0, "the waiter was woken");
    // SAFETY: the page mapped above, which nothing refers to after this.
    unsafe { libc::munmap(page.cast(), 4096) };
}

/// `wait_wake()`: the pinned futex wait, then Rust's, which is only the futex path;
/// elsewhere the pinned registry alone, as the gate ran it.
#[test]
#[cfg_attr(
    miri,
    ignore = "Miri can't call the C++ oracle, fork or share a mapping"
)]
fn wait_and_wake() {
    #[cfg(not(target_os = "linux"))]
    reference::wait_init();
    wait_wake(
        // SAFETY: a live, shared word.
        |word, timeout| unsafe { reference::wait(word.as_ptr(), timeout) },
        // SAFETY: as above.
        |word, count| unsafe { reference::wake(word.as_ptr(), count) },
    );
    #[cfg(target_os = "linux")]
    wait_wake(
        |word, timeout| swoole_rs_locks::swr_v1_locks_wait(Some(word), timeout),
        |word, count| swoole_rs_locks::swr_v1_locks_wakeup(Some(word), count),
    );
}

thread_local! {
    /// The running coroutine of `coroutine_policy`, and how often its lock slept.
    static CID: Cell<i64> = const { Cell::new(0) };
    static SLEEPS: Cell<u32> = const { Cell::new(0) };
}
/// `coroutine_policy`'s shared word.
static WORD: AtomicU32 = AtomicU32::new(0);

/// `coroutine_policy()`: `swr_v1_locks_coroutine_try` and `_release` were the policy of
/// `CoroutineLock::acquire` and `unlock`, which hold it now. The gate's results are the
/// outcomes here: acquired (1) and reentrant (0) are code 0, with and without the word
/// taken; busy (2) is `EBUSY`; wait (3) is a sleep, which this host cancels.
#[test]
fn coroutine_policy() {
    #[expect(clippy::unnecessary_wraps, reason = "CoroutineHost::current's type")]
    fn current() -> Option<i64> {
        Some(CID.get())
    }
    #[expect(
        clippy::unnecessary_wraps,
        reason = "CoroutineHost::alloc_shared's type"
    )]
    fn shared() -> Option<&'static AtomicU32> {
        Some(&WORD)
    }
    fn sleep(_: f64) -> i32 {
        SLEEPS.set(SLEEPS.get() + 1);
        -1
    }
    fn warning() {}
    static HOST: CoroutineHost = CoroutineHost {
        current,
        alloc_shared: shared,
        sleep: Some(sleep),
        wait: None,
        wake: None,
        warning,
    };
    let word = || WORD.load(Ordering::Relaxed);
    let lock = CoroutineLock::new(true, &HOST);
    CID.set(7);
    assert_eq!(lock.acquire(true), Ok(()));
    assert_eq!(word(), 1);
    assert_eq!(lock.acquire(true), Ok(()));
    assert_eq!((word(), SLEEPS.get()), (1, 0));
    CID.set(8);
    assert_eq!(lock.acquire(false), Err(EBUSY));
    assert_eq!(word(), 1);
    CID.set(9);
    assert_eq!(
        lock.acquire(true),
        Err(ErrorCode::SW_ERROR_CO_CANCELED as i32)
    );
    assert_eq!((word(), SLEEPS.get()), (1, 1));
    CID.set(7);
    assert_eq!(lock.unlock(), Ok(()));
    assert_eq!(word(), 0);
    assert_eq!(lock.unlock(), Ok(()));
    assert_eq!(word(), 0);
    // Pinned owner reentrancy precedes CAS even if the shared word is already zero, as
    // after another process's unlock.
    assert_eq!(lock.acquire(false), Ok(()));
    WORD.store(0, Ordering::Relaxed);
    assert_eq!(lock.acquire(false), Ok(()));
    assert_eq!(word(), 0);
    CID.set(8);
    assert_eq!(lock.acquire(false), Ok(()));
    assert_eq!(word(), 1);
    let mut second = 0.001;
    for expected in [0.002, 0.004, 0.008, 0.016, 0.032, 0.064, 0.1, 0.001] {
        second = swr_v1_locks_coroutine_backoff(second, 0.001, 0.1);
        assert_eq!(second, expected);
    }
}
