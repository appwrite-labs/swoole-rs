//! Native-compatible synchronization. Callers allocate/free storage outside Rust.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#![expect(
    clippy::missing_safety_doc,
    reason = "every export is covered by the module-local C contract"
)]
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("Rust locks require validated macOS or Linux platform contracts");

// libc's surface is platform-specific here (e.g. __error on macOS vs
// __errno_location on Linux, selected by cfg below), so it is not portably enumerable.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "libc's platform-specific surface is not portably enumerable"
    )
)]
use libc::*;
use std::{
    mem::{align_of, offset_of, size_of},
    sync::atomic::{AtomicI64, AtomicU32, Ordering},
};

pub mod coroutine;

// libc 0.2.191 omits these Linux declarations; signatures match pthread.h.
#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn pthread_rwlock_timedrdlock(lock: *mut pthread_rwlock_t, deadline: *const timespec) -> c_int;
    fn pthread_rwlock_timedwrlock(lock: *mut pthread_rwlock_t, deadline: *const timespec) -> c_int;
}

#[repr(C)]
struct Mutex {
    lock: pthread_mutex_t,
    attr: pthread_mutexattr_t,
}
#[repr(C)]
struct RwLock {
    lock: pthread_rwlock_t,
    attr: pthread_rwlockattr_t,
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_size(kind: i32) -> usize {
    match kind {
        3 => size_of::<Mutex>(),
        1 => size_of::<RwLock>(),
        #[cfg(target_os = "linux")]
        5 => size_of::<pthread_spinlock_t>(),
        _ => 0,
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_align(kind: i32) -> usize {
    match kind {
        3 => align_of::<Mutex>(),
        1 => align_of::<RwLock>(),
        #[cfg(target_os = "linux")]
        5 => align_of::<pthread_spinlock_t>(),
        _ => 0,
    }
}
fn valid(storage: *mut c_void, kind: i32) -> bool {
    let align = swr_v1_locks_align(kind);
    !storage.is_null() && align != 0 && (storage as usize).is_multiple_of(align)
}
// Field addresses inside borrowed storage, computed without dereferencing it.
fn mutex(storage: *mut c_void) -> (*mut pthread_mutex_t, *mut pthread_mutexattr_t) {
    let s = storage.cast::<Mutex>();
    (
        s.cast(),
        s.wrapping_byte_add(offset_of!(Mutex, attr)).cast(),
    )
}
fn rwlock(storage: *mut c_void) -> (*mut pthread_rwlock_t, *mut pthread_rwlockattr_t) {
    let s = storage.cast::<RwLock>();
    (
        s.cast(),
        s.wrapping_byte_add(offset_of!(RwLock, attr)).cast(),
    )
}
// The thread's errno slot.
fn errno() -> *mut c_int {
    #[cfg(target_os = "linux")]
    // SAFETY: always callable; returns this thread's errno location.
    let error = unsafe { __errno_location() };
    #[cfg(target_os = "macos")]
    // SAFETY: always callable; returns this thread's errno location.
    let error = unsafe { __error() };
    error
}
// Failed lock initialization owns an initialized attribute, but not a live lock.
fn init_result(result: c_int, destroy_attr: impl FnOnce()) -> c_int {
    if result != 0 {
        let error = errno();
        // SAFETY: `errno()` is this thread's live errno slot.
        let saved = unsafe { *error };
        destroy_attr();
        // SAFETY: as above.
        unsafe { *error = saved };
    }
    result
}
// Lock storage below passed `valid`: non-null and aligned for its kind, and the caller
// lends `swr_v1_locks_size(kind)` bytes of it for the call. Native code serializes
// init and destroy against every other use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_init(storage: *mut c_void, kind: i32, shared: i32) -> i32 {
    if !valid(storage, kind) {
        return EINVAL;
    }
    match kind {
        3 => {
            let (lock, attr) = mutex(storage);
            // SAFETY: `attr` addresses writable attribute storage inside the lent lock.
            let result = unsafe { pthread_mutexattr_init(attr) };
            if result != 0 {
                return result;
            }
            let attributes = if shared != 0 {
                // SAFETY: `attr` was initialized above.
                unsafe { pthread_mutexattr_setpshared(attr, PTHREAD_PROCESS_SHARED) }
            } else {
                0
            };
            let result = if attributes != 0 {
                attributes
            } else {
                // SAFETY: `lock` addresses writable, unused mutex storage; `attr` is initialized.
                unsafe { pthread_mutex_init(lock, attr) }
            };
            init_result(result, || {
                // SAFETY: `attr` is initialized and no lock was created from it.
                unsafe { pthread_mutexattr_destroy(attr) };
            })
        }
        1 => {
            let (lock, attr) = rwlock(storage);
            // SAFETY: `attr` addresses writable attribute storage inside the lent lock.
            let result = unsafe { pthread_rwlockattr_init(attr) };
            if result != 0 {
                return result;
            }
            let attributes = if shared != 0 {
                // SAFETY: `attr` was initialized above.
                unsafe { pthread_rwlockattr_setpshared(attr, PTHREAD_PROCESS_SHARED) }
            } else {
                0
            };
            let result = if attributes != 0 {
                attributes
            } else {
                // SAFETY: `lock` addresses writable, unused rwlock storage; `attr` is initialized.
                unsafe { pthread_rwlock_init(lock, attr) }
            };
            init_result(result, || {
                // SAFETY: `attr` is initialized and no lock was created from it.
                unsafe { pthread_rwlockattr_destroy(attr) };
            })
        }
        #[cfg(target_os = "linux")]
        // SAFETY: `storage` is writable, unused spinlock storage.
        5 => unsafe { pthread_spin_init(storage.cast(), shared) },
        _ => EINVAL,
    }
}
// Match sw_wait_for's exact attempt/sleep/backoff order, including its final attempt.
fn poll(mut timeout: i32, mut attempt: impl FnMut() -> i32) -> i32 {
    let mut sleep: i32 = 1;
    while timeout >= 0 {
        if attempt() == 0 {
            return 0;
        }
        // SAFETY: usleep has no memory preconditions.
        unsafe { usleep(sleep.wrapping_mul(1000).cast_unsigned()) };
        sleep = sleep.wrapping_mul(2);
        if timeout > 0 && timeout.wrapping_sub(sleep) < 0 {
            sleep = timeout;
            timeout = 0;
        } else {
            timeout = timeout.wrapping_sub(sleep);
        }
    }
    ETIMEDOUT
}
#[cfg(target_os = "linux")]
fn deadline(millis: i32) -> timespec {
    let mut now = timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a live, writable timespec.
    unsafe { clock_gettime(CLOCK_REALTIME, &raw mut now) };
    now.tv_sec += (millis / 1000) as time_t;
    now.tv_nsec += c_long::from(millis % 1000) * 1_000_000;
    now.tv_sec += (now.tv_nsec / 1_000_000_000) as time_t;
    now.tv_nsec %= 1_000_000_000;
    now
}
// capabilities: bit0 mutex timedlock; bit1 rwlock timed{rd,wr}lock. From native configure.
// Every call below takes a lock that `valid` checked and the caller initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_lock(
    storage: *mut c_void,
    kind: i32,
    operation: i32,
    timeout: i32,
    capabilities: u32,
) -> i32 {
    if !valid(storage, kind) {
        return EINVAL;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = capabilities;
    match kind {
        3 => {
            let (lock, _) = mutex(storage);
            if operation & LOCK_NB != 0 {
                // SAFETY: `lock` is an initialized mutex.
                return unsafe { pthread_mutex_trylock(lock) };
            }
            if timeout > 0 {
                #[cfg(target_os = "linux")]
                if capabilities & 1 != 0 {
                    // SAFETY: `lock` is an initialized mutex; the deadline is a live timespec.
                    return unsafe { pthread_mutex_timedlock(lock, &deadline(timeout)) };
                }
                // SAFETY: `lock` is an initialized mutex.
                return poll(timeout, || unsafe { pthread_mutex_trylock(lock) });
            }
            // SAFETY: `lock` is an initialized mutex.
            unsafe { pthread_mutex_lock(lock) }
        }
        1 => {
            let (lock, _) = rwlock(storage);
            let read = operation & LOCK_SH != 0;
            let try_lock = || {
                if read {
                    // SAFETY: `lock` is an initialized rwlock.
                    unsafe { pthread_rwlock_tryrdlock(lock) }
                } else {
                    // SAFETY: `lock` is an initialized rwlock.
                    unsafe { pthread_rwlock_trywrlock(lock) }
                }
            };
            if operation & LOCK_NB != 0 {
                return try_lock();
            }
            if timeout > 0 {
                #[cfg(target_os = "linux")]
                if capabilities & 2 != 0 {
                    let deadline = deadline(timeout);
                    return if read {
                        // SAFETY: `lock` is an initialized rwlock; `deadline` is live.
                        unsafe { pthread_rwlock_timedrdlock(lock, &raw const deadline) }
                    } else {
                        // SAFETY: as above.
                        unsafe { pthread_rwlock_timedwrlock(lock, &raw const deadline) }
                    };
                }
                return poll(timeout, try_lock);
            }
            if read {
                // SAFETY: `lock` is an initialized rwlock.
                unsafe { pthread_rwlock_rdlock(lock) }
            } else {
                // SAFETY: `lock` is an initialized rwlock.
                unsafe { pthread_rwlock_wrlock(lock) }
            }
        }
        #[cfg(target_os = "linux")]
        5 => {
            let lock = storage.cast();
            if operation & LOCK_NB != 0 {
                // SAFETY: `lock` is an initialized spinlock.
                return unsafe { pthread_spin_trylock(lock) };
            }
            if timeout > 0 {
                // SAFETY: `lock` is an initialized spinlock.
                return poll(timeout, || unsafe { pthread_spin_trylock(lock) });
            }
            // SAFETY: `lock` is an initialized spinlock.
            unsafe { pthread_spin_lock(lock) }
        }
        _ => EINVAL,
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_unlock(storage: *mut c_void, kind: i32) -> i32 {
    if !valid(storage, kind) {
        return EINVAL;
    }
    match kind {
        // SAFETY: an initialized mutex that this thread holds.
        3 => unsafe { pthread_mutex_unlock(mutex(storage).0) },
        // SAFETY: an initialized rwlock that this thread holds.
        1 => unsafe { pthread_rwlock_unlock(rwlock(storage).0) },
        #[cfg(target_os = "linux")]
        // SAFETY: an initialized spinlock that this thread holds.
        5 => unsafe { pthread_spin_unlock(storage.cast()) },
        _ => EINVAL,
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_destroy(storage: *mut c_void, kind: i32) {
    if !valid(storage, kind) {
        return;
    }
    match kind {
        3 => {
            let (lock, attr) = mutex(storage);
            // SAFETY: `attr` was initialized by `swr_v1_locks_init` and is destroyed once.
            unsafe { pthread_mutexattr_destroy(attr) };
            // SAFETY: `lock` is initialized, unlocked and destroyed once.
            unsafe { pthread_mutex_destroy(lock) };
        }
        1 => {
            let (lock, attr) = rwlock(storage);
            // SAFETY: `attr` was initialized by `swr_v1_locks_init` and is destroyed once.
            unsafe { pthread_rwlockattr_destroy(attr) };
            // SAFETY: `lock` is initialized, unlocked and destroyed once.
            unsafe { pthread_rwlock_destroy(lock) };
        }
        #[cfg(target_os = "linux")]
        5 => {
            // SAFETY: an initialized, unlocked spinlock, destroyed once.
            unsafe { pthread_spin_destroy(storage.cast()) };
        }
        _ => (),
    }
}

// Generic operations preserve unsigned/signed wrapping and __sync CAS/add/sub fences.
// op: load=0, store=1, add-fetch=2, sub-fetch=3, compare-set=4 (expected last).
macro_rules! atomic_api {
    ($name:ident, $rust:ident, $atomic:ty, $word:ty) => {
        /// The operation itself, inlined into Rust callers (PHP Atomic) so a
        /// constant `op` folds away.
        /// # Safety
        /// `storage` is null or a live word that is only accessed atomically.
        #[inline]
        pub unsafe fn $rust(storage: *mut $word, op: u32, value: $word, expected: $word) -> $word {
            if storage.is_null() || !(storage as usize).is_multiple_of(align_of::<$atomic>()) {
                return 0;
            }
            // SAFETY: non-null and aligned (checked above); the caller lends a live word
            // that is only accessed atomically.
            let a = unsafe { <$atomic>::from_ptr(storage) };
            match op {
                0 => a.load(Ordering::Relaxed),
                1 => {
                    a.store(value, Ordering::Relaxed);
                    value
                }
                2 => a.fetch_add(value, Ordering::SeqCst).wrapping_add(value),
                3 => a.fetch_sub(value, Ordering::SeqCst).wrapping_sub(value),
                4 => a
                    .compare_exchange(expected, value, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok() as $word,
                _ => 0,
            }
        }

        /// # Safety
        /// As for the Rust function it wraps.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            storage: *mut $word,
            op: u32,
            value: $word,
            expected: $word,
        ) -> $word {
            // SAFETY: per the caller.
            unsafe { $rust(storage, op, value, expected) }
        }
    };
}
atomic_api!(swr_v1_locks_atomic32, atomic32, AtomicU32, u32);
atomic_api!(swr_v1_locks_atomic64, atomic64, AtomicI64, i64);

// Barrier words follow the atomic API's contract; it rejects null and misaligned ones.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_barrier_init(
    count: *mut u32,
    arrived: *mut u32,
    parties: u32,
) {
    // SAFETY: `count` is null or a live, atomically accessed word.
    unsafe { swr_v1_locks_atomic32(count, 1, parties, 0) };
    // SAFETY: `arrived` is null or a live, atomically accessed word.
    unsafe { swr_v1_locks_atomic32(arrived, 1, 0, 0) };
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_barrier_wait(count: *mut u32, arrived: *mut u32) {
    if count.is_null() || arrived.is_null() {
        return;
    }
    let load = |word| {
        // SAFETY: `word` is one of the caller's live, atomically accessed barrier words.
        unsafe { swr_v1_locks_atomic32(word, 0, 0, 0) }
    };
    // SAFETY: as above.
    unsafe { swr_v1_locks_atomic32(arrived, 2, 1, 0) };
    while load(arrived) != load(count) {
        // SAFETY: usleep has no memory preconditions.
        unsafe { usleep(10_000) };
        std::sync::atomic::fence(Ordering::SeqCst);
    }
}

// Native pthread storage is borrowed, never moved or exposed as a Rust reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_pthread_barrier(
    storage: *mut c_void,
    storage_len: usize,
    attr: *mut c_void,
    attr_len: usize,
    operation: u32,
    shared: i32,
    parties: u32,
) -> i32 {
    #[cfg(target_os = "linux")]
    {
        if storage.is_null()
            || storage_len != size_of::<pthread_barrier_t>()
            || !(storage as usize).is_multiple_of(align_of::<pthread_barrier_t>())
            || attr.is_null()
            || attr_len != size_of::<pthread_barrierattr_t>()
            || !(attr as usize).is_multiple_of(align_of::<pthread_barrierattr_t>())
        {
            return EINVAL;
        }
        // Both regions are non-null, exactly sized and aligned (checked above) and lent
        // by the caller, which serializes init and destroy against waits.
        let (barrier, attr) = (storage.cast(), attr.cast());
        match operation {
            0 => {
                if shared != 0 {
                    // SAFETY: `attr` is writable attribute storage.
                    unsafe { pthread_barrierattr_init(attr) };
                    // SAFETY: `attr` was initialized above.
                    unsafe { pthread_barrierattr_setpshared(attr, PTHREAD_PROCESS_SHARED) };
                }
                let attr = if shared != 0 { attr } else { std::ptr::null() };
                // SAFETY: `barrier` is writable, unused storage; `attr` is null or initialized.
                unsafe { pthread_barrier_init(barrier, attr, parties) }
            }
            // SAFETY: `barrier` was initialized by operation 0.
            1 => unsafe { pthread_barrier_wait(barrier) },
            2 => {
                // SAFETY: `barrier` is initialized, has no waiters and is destroyed once.
                let result = unsafe { pthread_barrier_destroy(barrier) };
                if shared != 0 {
                    // SAFETY: `attr` was initialized by operation 0 and is destroyed once.
                    unsafe { pthread_barrierattr_destroy(attr) };
                }
                result
            }
            _ => EINVAL,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            storage,
            storage_len,
            attr,
            attr_len,
            operation,
            shared,
            parties,
        );
        ENOTSUP
    }
}

// The word functions below take null or a pointer to live u32 storage that is only
// accessed atomically (process-shared for the futex pair); they reject misalignment.
unsafe fn word<'a>(word: *mut u32) -> Option<&'a AtomicU32> {
    if word.is_null() || !(word as usize).is_multiple_of(align_of::<AtomicU32>()) {
        return None;
    }
    // SAFETY: non-null and aligned (checked above); live and atomic-only per this
    // function's callers, for as long as they use the reference.
    Some(unsafe { AtomicU32::from_ptr(word) })
}

/// Futex wait on a native atomic word: consume a ready token, otherwise one
/// `FUTEX_WAIT` syscall. Non-futex builds use upstream's C++ wait registry.
/// # Safety
/// Word is aligned, live, process-shared u32 storage accessed only atomically.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_wait(word: *mut u32, timeout: f64) -> i32 {
    // SAFETY: the caller lends null or a live, atomic-only word for this call.
    let Some(a) = (unsafe { self::word(word) }) else {
        return -1;
    };
    if a.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "upstream's split of the seconds; Rust's float casts saturate where C's are undefined"
    )]
    let duration = timespec {
        tv_sec: timeout as time_t,
        tv_nsec: ((timeout - (timeout as time_t) as f64) * 1000.0 * 1000.0 * 1000.0) as c_long,
    };
    // SAFETY: `word` is a live futex word; the timeout is null or the live `duration`.
    let result = unsafe {
        syscall(
            SYS_futex,
            word,
            FUTEX_WAIT,
            0,
            if timeout > 0.0 {
                &raw const duration
            } else {
                std::ptr::null()
            },
            std::ptr::null::<c_void>(),
            0,
        )
    };
    if result == 0
        && a.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        0
    } else {
        -1
    }
}
/// # Safety
/// Same requirements as [`swr_v1_locks_wait`].
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_wakeup(word: *mut u32, n: i32) -> i32 {
    // SAFETY: the caller lends null or a live, atomic-only word for this call.
    let Some(a) = (unsafe { self::word(word) }) else {
        return -1;
    };
    if a.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "FUTEX_WAKE returns -1 or at most `n` woken waiters"
        )]
        // SAFETY: `word` is a live futex word; FUTEX_WAKE reads no other pointer.
        unsafe {
            syscall(
                SYS_futex,
                word,
                FUTEX_WAKE,
                n,
                std::ptr::null::<c_void>(),
                std::ptr::null::<c_void>(),
                0,
            ) as i32
        }
    } else {
        0
    }
}

// 0=reentrant, 1=acquired, 2=busy, 3=wait. Cookie/CID values are copied tokens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_coroutine_try(
    word: *mut u32,
    held_cookie: usize,
    held_cid: i64,
    current_cookie: usize,
    current_cid: i64,
    blocking: u8,
) -> i32 {
    // SAFETY: the caller lends null or a live, atomic-only word for this call.
    let Some(a) = (unsafe { self::word(word) }) else {
        return -1;
    };
    if current_cookie == held_cookie && current_cid == held_cid {
        return 0;
    }
    if a.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        1
    } else if blocking == 0 {
        2
    } else {
        3
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_coroutine_release(word: *mut u32) -> i32 {
    // SAFETY: the caller lends null or a live, atomic-only word for this call.
    let Some(a) = (unsafe { self::word(word) }) else {
        return -1;
    };
    if a.load(Ordering::Relaxed) == 0 {
        return 0;
    }
    a.store(0, Ordering::Relaxed);
    1
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_coroutine_backoff(second: f64, initial: f64, maximum: f64) -> f64 {
    if second >= maximum {
        initial
    } else if second * 2.0 < maximum {
        second * 2.0
    } else {
        maximum
    }
}

// core-tests/src/lock/lock.cpp's TESTs over the ABI that Mutex and Swoole\Lock call (the
// RWLock and SpinLock classes left the build), with the same inputs and expected values.
// The coroutine-lock TESTs are in coroutine.rs.
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ptr,
        sync::Barrier,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    // What configure finds: Linux has pthread_mutex_timedlock and pthread_rwlock_timed*.
    const CAPABILITIES: u32 = if cfg!(target_os = "linux") { 3 } else { 0 };
    const RW_LOCK: i32 = 1;
    const MUTEX: i32 = 3;
    #[cfg(target_os = "linux")]
    const SPIN_LOCK: i32 = 5;

    // A lock in its own page; a shared one survives fork, as sw_mem_pool's does.
    struct Lock {
        storage: *mut c_void,
        kind: i32,
    }
    // SAFETY: pthread locks are made to be used from any thread.
    unsafe impl Sync for Lock {}
    impl Lock {
        fn new(kind: i32, shared: bool) -> Self {
            let storage = page(shared);
            assert!(swr_v1_locks_size(kind) <= 4096);
            assert_eq!(
                unsafe { swr_v1_locks_init(storage, kind, i32::from(shared)) },
                0
            );
            Self { storage, kind }
        }
        fn lock(&self, operation: i32, timeout: i32) -> i32 {
            unsafe { swr_v1_locks_lock(self.storage, self.kind, operation, timeout, CAPABILITIES) }
        }
        fn unlock(&self) -> i32 {
            unsafe { swr_v1_locks_unlock(self.storage, self.kind) }
        }
    }
    impl Drop for Lock {
        fn drop(&mut self) {
            unsafe { swr_v1_locks_destroy(self.storage, self.kind) };
            unsafe { munmap(self.storage, 4096) };
        }
    }
    fn page(shared: bool) -> *mut c_void {
        let flags = MAP_ANON | if shared { MAP_SHARED } else { MAP_PRIVATE };
        let page = unsafe { mmap(ptr::null_mut(), 4096, PROT_READ | PROT_WRITE, flags, -1, 0) };
        assert_ne!(page, MAP_FAILED);
        page
    }
    fn millis() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    }

    fn test_func(lock: &Lock) {
        const N: i64 = 100_000;
        // A plain read-modify-write: only the lock keeps the two threads' increments apart.
        let count = AtomicI64::new(0);
        thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    for _ in 0..N {
                        assert_eq!(lock.lock(LOCK_EX, -1), 0);
                        count.store(count.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
                        assert_eq!(lock.unlock(), 0);
                    }
                });
            }
        });
        assert_eq!(count.into_inner(), N * 2);
    }

    fn test_lock_rd_func(lock: &Lock) {
        thread::scope(|s| {
            s.spawn(|| {
                assert_eq!(lock.lock(LOCK_SH, -1), 0);
                thread::sleep(Duration::from_micros(2000));
                lock.unlock();
            });
            s.spawn(|| {
                thread::sleep(Duration::from_micros(1000));
                assert!(lock.lock(LOCK_SH | LOCK_NB, -1) >= 0);
            });
        });
    }

    fn test_share_lock_fun(lock: &Lock) {
        lock.lock(LOCK_EX, -1);
        let magic_num = 100_000 + (millis() % (9_999_999 - 100_000)) as i32;
        let num = page(true).cast::<i32>();
        unsafe { *num = 0 };
        let pid = unsafe { fork() };
        if pid == 0 {
            lock.lock(LOCK_EX, -1);
            unsafe { *num = magic_num };
            unsafe { usleep(1) };
            unsafe { _exit(0) };
        }
        thread::sleep(Duration::from_micros(10_000));
        lock.unlock();
        let mut status = 0;
        assert_eq!(unsafe { waitpid(pid, &raw mut status, 0) }, pid);
        assert_eq!(unsafe { *num }, magic_num);
        unsafe { munmap(num.cast(), 4096) };
    }

    #[test]
    fn lock_mutex() {
        test_func(&Lock::new(MUTEX, false));
    }

    // Without pthread_mutex_timedlock (macOS) the 10 ms wait polls as sw_wait_for does, and its
    // last attempt can come before the unlock; upstream runs this case on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn lock_lockwait() {
        const TIMEOUT_1: i32 = 2;
        const TIMEOUT_2: i32 = 10;
        let lock = Lock::new(MUTEX, false);
        lock.lock(LOCK_EX, -1);
        thread::scope(|s| {
            s.spawn(|| {
                let ms1 = millis();
                assert_eq!(lock.lock(LOCK_EX, TIMEOUT_1), ETIMEDOUT);
                let ms2 = millis();
                assert!(ms2 - ms1 >= TIMEOUT_1 as u128);
                assert_eq!(lock.lock(LOCK_EX, TIMEOUT_2), 0);
                let ms3 = millis();
                assert!(ms3 - ms2 <= TIMEOUT_2 as u128);
            });
            thread::sleep(Duration::from_millis(10));
            lock.unlock();
        });
    }

    #[test]
    fn lock_shared() {
        test_share_lock_fun(&Lock::new(MUTEX, true));
    }

    #[test]
    fn lock_try_rd() {
        test_lock_rd_func(&Lock::new(MUTEX, false));
    }

    #[test]
    fn lock_rwlock_shared() {
        test_share_lock_fun(&Lock::new(RW_LOCK, true));
    }

    #[test]
    fn lock_rwlock() {
        test_func(&Lock::new(RW_LOCK, false));
    }

    #[test]
    fn lock_rwlock_try_rd() {
        test_lock_rd_func(&Lock::new(RW_LOCK, false));
    }

    // Upstream orders the two threads with 1 ms and 2 ms sleeps and fails now and then on
    // either engine; barriers keep the write lock held across the try instead.
    #[test]
    fn lock_rw_try_wr() {
        let lock = Lock::new(RW_LOCK, false);
        let (held, tried) = (Barrier::new(2), Barrier::new(2));
        thread::scope(|s| {
            s.spawn(|| {
                assert_eq!(lock.lock(LOCK_EX, -1), 0);
                held.wait();
                tried.wait();
                lock.unlock();
            });
            s.spawn(|| {
                held.wait();
                assert!(lock.lock(LOCK_NB, -1) > 0);
                tried.wait();
            });
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lock_spinlock_shared() {
        test_share_lock_fun(&Lock::new(SPIN_LOCK, true));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lock_spinlock() {
        test_func(&Lock::new(SPIN_LOCK, false));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lock_spinlock_try_rd() {
        test_lock_rd_func(&Lock::new(SPIN_LOCK, false));
    }
}
