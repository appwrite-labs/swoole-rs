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
    sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering},
};
#[cfg(target_os = "linux")]
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::sys;

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
/// Whether `storage` can be a `T`: not null, and aligned for it. With the kind's
/// match this is [`valid`] in one branch, for the calls made per lock and unlock.
fn holds<T>(storage: *mut c_void) -> bool {
    !storage.is_null() && storage.cast::<T>().is_aligned()
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
// Failed lock initialization owns an initialized attribute, but not a live lock.
fn init_result(result: c_int, destroy_attr: impl FnOnce()) -> c_int {
    if result != 0 {
        // The failed init's errno, which destroying the attribute must not replace.
        let saved = sys::errno();
        destroy_attr();
        sys::set_errno(saved);
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
        sys::usleep(sleep.wrapping_mul(1000).cast_unsigned());
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
    // Zero if the clock can't be read, as the unchecked call left it.
    let mut now = sys::clock_gettime(CLOCK_REALTIME).unwrap_or(timespec {
        tv_sec: 0,
        tv_nsec: 0,
    });
    now.tv_sec += (millis / 1000) as time_t;
    now.tv_nsec += c_long::from(millis % 1000) * 1_000_000;
    now.tv_sec += now.tv_nsec / 1_000_000_000;
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
    #[cfg(not(target_os = "linux"))]
    let _ = capabilities;
    match kind {
        3 if holds::<Mutex>(storage) => {
            let (lock, _) = mutex(storage);
            // SAFETY: `lock` is an initialized mutex.
            let try_lock = || unsafe { pthread_mutex_trylock(lock) };
            if operation & LOCK_NB != 0 {
                return try_lock();
            }
            if timeout > 0 {
                #[cfg(target_os = "linux")]
                if capabilities & 1 != 0 {
                    // SAFETY: `lock` is an initialized mutex; the deadline is a live timespec.
                    return unsafe { pthread_mutex_timedlock(lock, &deadline(timeout)) };
                }
                return poll(timeout, try_lock);
            }
            // SAFETY: `lock` is an initialized mutex.
            unsafe { pthread_mutex_lock(lock) }
        }
        1 if holds::<RwLock>(storage) => {
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
        5 if holds::<pthread_spinlock_t>(storage) => {
            let lock = storage.cast();
            // SAFETY: `lock` is an initialized spinlock.
            let try_lock = || unsafe { pthread_spin_trylock(lock) };
            if operation & LOCK_NB != 0 {
                return try_lock();
            }
            if timeout > 0 {
                return poll(timeout, try_lock);
            }
            // SAFETY: `lock` is an initialized spinlock.
            unsafe { pthread_spin_lock(lock) }
        }
        _ => EINVAL,
    }
}
/// Marks its caller's path unlikely: `std::hint::cold_path`, which the fuzz toolchain
/// doesn't have yet.
#[cold]
const fn cold() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_unlock(storage: *mut c_void, kind: i32) -> i32 {
    if kind == 3 && holds::<Mutex>(storage) {
        // SAFETY: an initialized mutex that this thread holds.
        return unsafe { pthread_mutex_unlock(mutex(storage).0) };
    }
    // After the mutex every Table row, task lock and default `Lock` is.
    cold();
    match kind {
        // SAFETY: an initialized rwlock that this thread holds.
        1 if holds::<RwLock>(storage) => unsafe { pthread_rwlock_unlock(rwlock(storage).0) },
        #[cfg(target_os = "linux")]
        // SAFETY: an initialized spinlock that this thread holds.
        5 if holds::<pthread_spinlock_t>(storage) => unsafe { pthread_spin_unlock(storage.cast()) },
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

/// An initialized lock of `kind` (`Lock::Type`: rwlock 1, mutex 3, spinlock 5),
/// in heap storage it owns or in process-shared storage it keeps for the
/// program's life. Its words are `AtomicU64`s only to be 8-aligned, `Sync`
/// storage that pthread alone accesses. pthread reports misuse (unlocking a
/// lock this thread doesn't hold, destroying a held one) as an error code, as
/// upstream's locks see it.
#[derive(Debug)]
pub struct Lock {
    storage: Storage,
    kind: i32,
}

#[derive(Debug)]
enum Storage {
    /// A thread-private lock's, destroyed with it.
    Private(Box<[AtomicU64]>),
    /// A shared lock's, which forked processes may still hold.
    Shared(&'static [AtomicU64]),
}

impl Storage {
    fn words(&self) -> &[AtomicU64] {
        match self {
            Self::Private(words) => words,
            Self::Shared(words) => words,
        }
    }

    fn ptr(&self) -> *mut c_void {
        self.words().as_ptr().cast_mut().cast()
    }
}

impl Lock {
    /// A thread-private lock (`new Mutex(false)` and its rwlock and spinlock
    /// twins); None for a kind this platform lacks, or when it can't be made.
    #[must_use]
    pub fn private(kind: i32) -> Option<Self> {
        let words = swr_v1_locks_size(kind).div_ceil(size_of::<AtomicU64>());
        Self::new(
            kind,
            Storage::Private((0..words).map(|_| AtomicU64::new(0)).collect()),
            0,
        )
    }

    /// A process-shared lock (`new Mutex(true)`) in `storage`, which it keeps:
    /// None when `storage` is too small for `kind`, or the lock can't be made.
    /// The storage is exclusive, so no other lock re-initializes it while held.
    #[must_use]
    pub fn shared(kind: i32, storage: &'static mut [AtomicU64]) -> Option<Self> {
        Self::new(kind, Storage::Shared(storage), 1)
    }

    fn new(kind: i32, storage: Storage, shared: i32) -> Option<Self> {
        let size = swr_v1_locks_size(kind);
        let room = size_of_val::<[AtomicU64]>(storage.words());
        if size == 0 || room < size || swr_v1_locks_align(kind) > align_of::<AtomicU64>() {
            return None;
        }
        // SAFETY: at least `size` 8-aligned bytes, which only this lock uses.
        (unsafe { swr_v1_locks_init(storage.ptr(), kind, shared) } == 0)
            .then_some(Self { storage, kind })
    }

    /// `Lock::lock(operation, timeout)`: `LOCK_SH`/`LOCK_NB` from `operation`,
    /// `timeout` in milliseconds (none when not positive), the timed waits
    /// `capabilities` names (bit 0 mutex, bit 1 rwlock); 0 or an errno.
    #[must_use]
    pub fn lock(&self, operation: i32, timeout: i32, capabilities: u32) -> i32 {
        let storage = self.storage.ptr();
        // A blocking mutex lock, the common case, is small enough to inline.
        if self.kind == 3 && operation & LOCK_NB == 0 && timeout <= 0 {
            // SAFETY: a mutex `new` initialized.
            return unsafe { pthread_mutex_lock(mutex(storage).0) };
        }
        // SAFETY: a lock `new` initialized.
        unsafe { swr_v1_locks_lock(storage, self.kind, operation, timeout, capabilities) }
    }

    /// `Lock::unlock()`: 0 or an errno.
    #[must_use]
    pub fn unlock(&self) -> i32 {
        // SAFETY: as in `lock`.
        unsafe { swr_v1_locks_unlock(self.storage.ptr(), self.kind) }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Storage::Private(_) = self.storage {
            // SAFETY: the last reference to a private lock `new` initialized.
            unsafe { swr_v1_locks_destroy(self.storage.ptr(), self.kind) };
        }
    }
}

/// A process-shared mutex (`new Mutex(true)`) in storage its owner places, such as a
/// shared-memory header: zeroed until [`init`](Self::init) and after
/// [`destroy`](Self::destroy). Its words are `AtomicU64`s only to be 8-aligned, `Sync`
/// storage that pthread alone accesses. On these platforms pthread reports locking
/// zeroed or destroyed storage, and misuse of an initialized mutex, as an error code.
#[derive(Debug)]
#[repr(C)]
pub struct SharedMutex([AtomicU64; MUTEX_WORDS]);

const MUTEX_WORDS: usize = size_of::<Mutex>().div_ceil(size_of::<AtomicU64>());
const _: () = assert!(
    align_of::<Mutex>() <= align_of::<AtomicU64>(),
    "the words align a mutex"
);

impl SharedMutex {
    /// Zeroed storage, not yet a mutex.
    #[must_use]
    pub const fn new() -> Self {
        Self([const { AtomicU64::new(0) }; MUTEX_WORDS])
    }

    fn ptr(&self) -> *mut c_void {
        self.0.as_ptr().cast_mut().cast()
    }

    /// Makes the storage a process-shared mutex: 0 or an errno. Processes that share
    /// it don't use it until this returns.
    #[must_use]
    pub fn init(&mut self) -> i32 {
        // SAFETY: storage sized and aligned for a mutex, which `&mut` keeps from this
        // process's other users while it is made.
        unsafe { swr_v1_locks_init(self.ptr(), 3, 1) }
    }

    /// `Mutex::lock()`, blocking: 0 or an errno.
    #[must_use]
    pub fn lock(&self) -> i32 {
        // SAFETY: a mutex `init` made, or storage pthread refuses, as above.
        unsafe { pthread_mutex_lock(mutex(self.ptr()).0) }
    }

    /// `Mutex::unlock()`: 0 or an errno.
    #[must_use]
    pub fn unlock(&self) -> i32 {
        // SAFETY: as in `lock`.
        unsafe { pthread_mutex_unlock(mutex(self.ptr()).0) }
    }

    /// Destroys the mutex `init` made. pthread refuses to destroy a held one, and
    /// refuses to lock it afterwards.
    pub fn destroy(&self) {
        // SAFETY: storage sized and aligned for a mutex, which only pthread accesses.
        unsafe { swr_v1_locks_destroy(self.ptr(), 3) };
    }
}

impl Default for SharedMutex {
    fn default() -> Self {
        Self::new()
    }
}

// Generic operations preserve unsigned/signed wrapping and __sync CAS/add/sub fences.
// op: load=0, store=1, add-fetch=2, sub-fetch=3, compare-set=4 (expected last).
macro_rules! atomic_api {
    ($name:ident, $atomic:ty, $word:ty) => {
        /// `storage` is null or a live, aligned word that is only accessed atomically.
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(
            storage: Option<&$atomic>,
            op: u32,
            value: $word,
            expected: $word,
        ) -> $word {
            let Some(a) = storage else {
                return 0;
            };
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
    };
}
atomic_api!(swr_v1_locks_atomic32, AtomicU32, u32);
atomic_api!(swr_v1_locks_atomic64, AtomicI64, i64);

/// `swoole::Barrier` (`native/locks/swoole_lock.h`), whose methods are these exports under
/// their C++ symbols. Linux uses a pthread barrier, as upstream's `SW_USE_PTHREAD_BARRIER`
/// (glibc and musl have `pthread_barrier_init`); macOS keeps the arrival/count words.
#[repr(C)]
pub struct Barrier {
    #[cfg(target_os = "linux")]
    barrier: std::cell::UnsafeCell<pthread_barrier_t>,
    #[cfg(target_os = "linux")]
    attr: std::cell::UnsafeCell<pthread_barrierattr_t>,
    #[cfg(target_os = "linux")]
    shared: std::sync::atomic::AtomicBool,
    #[cfg(not(target_os = "linux"))]
    count: AtomicU32,
    #[cfg(not(target_os = "linux"))]
    arrived: AtomicU32,
}

// SAFETY: pthread barriers are made for concurrent waits; the caller serializes init and
// destroy against them, as upstream's Barrier requires.
#[cfg(target_os = "linux")]
unsafe impl Sync for Barrier {}

/// `Barrier::init(bool shared, int count)`. Shared attrs are initialized before
/// `setpshared`, fixing upstream's use of an uninitialized attribute.
#[unsafe(export_name = "_ZN6swoole7Barrier4initEbi")]
pub extern "C" fn barrier_init(barrier: &Barrier, shared: bool, count: c_int) {
    #[cfg(target_os = "linux")]
    {
        barrier.shared.store(shared, Ordering::Relaxed);
        let attr = barrier.attr.get();
        if shared {
            // SAFETY: `attr` is this barrier's writable attribute storage.
            unsafe { pthread_barrierattr_init(attr) };
            // SAFETY: `attr` was initialized above.
            unsafe { pthread_barrierattr_setpshared(attr, PTHREAD_PROCESS_SHARED) };
        }
        let attr = if shared {
            attr.cast_const()
        } else {
            std::ptr::null()
        };
        // SAFETY: the caller initializes an unused barrier once, before any wait;
        // `attr` is null or initialized.
        unsafe { pthread_barrier_init(barrier.barrier.get(), attr, count.cast_unsigned()) };
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = shared;
        barrier
            .count
            .store(count.cast_unsigned(), Ordering::Relaxed);
        barrier.arrived.store(0, Ordering::Relaxed);
    }
}

/// `Barrier::wait()`.
#[unsafe(export_name = "_ZN6swoole7Barrier4waitEv")]
pub extern "C" fn barrier_wait(barrier: &Barrier) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: the barrier was initialized by `barrier_init` and is not yet destroyed.
        unsafe { pthread_barrier_wait(barrier.barrier.get()) };
    }
    #[cfg(not(target_os = "linux"))]
    {
        barrier.arrived.fetch_add(1, Ordering::SeqCst);
        while barrier.arrived.load(Ordering::Relaxed) != barrier.count.load(Ordering::Relaxed) {
            sys::usleep(10_000);
            std::sync::atomic::fence(Ordering::SeqCst);
        }
    }
}

/// `Barrier::destroy()`.
#[unsafe(export_name = "_ZN6swoole7Barrier7destroyEv")]
pub extern "C" fn barrier_destroy(barrier: &Barrier) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: the barrier is initialized, has no waiters and is destroyed once.
        unsafe { pthread_barrier_destroy(barrier.barrier.get()) };
        if barrier.shared.load(Ordering::Relaxed) {
            // SAFETY: init initialized the attribute of a shared barrier.
            unsafe { pthread_barrierattr_destroy(barrier.attr.get()) };
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = barrier;
}

// The word functions below take null or a pointer to live, aligned u32 storage that is
// only accessed atomically (process-shared for the futex pair).
/// Futex wait on a native atomic word: consume a ready token, otherwise one
/// `FUTEX_WAIT` syscall. Non-futex builds use upstream's C++ wait registry.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_wait(word: Option<&AtomicU32>, timeout: f64) -> i32 {
    word.map_or(-1, |word| wait(word, timeout))
}

/// [`swr_v1_locks_wait`] on a word.
#[cfg(target_os = "linux")]
pub fn wait(a: &AtomicU32, timeout: f64) -> i32 {
    if a.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        return 0;
    }
    let duration = timespec {
        tv_sec: timeout as time_t,
        tv_nsec: ((timeout - (timeout as time_t).c_cast::<f64>()) * 1000.0 * 1000.0 * 1000.0)
            .c_cast::<c_long>(),
    };
    // A wakeup between the check above and the syscall makes FUTEX_WAIT fail with
    // EAGAIN; its token is still in the word, so the result alone doesn't decide.
    // SAFETY: `a` is a live futex word; the timeout is null or the live `duration`.
    unsafe {
        syscall(
            SYS_futex,
            a.as_ptr(),
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
    if a.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        0
    } else {
        -1
    }
}
/// Futex wake on a native atomic word.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_wakeup(word: Option<&AtomicU32>, n: i32) -> i32 {
    word.map_or(-1, |word| wakeup(word, n))
}

/// [`swr_v1_locks_wakeup`] on a word.
#[cfg(target_os = "linux")]
pub fn wakeup(a: &AtomicU32, n: i32) -> i32 {
    if a.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "FUTEX_WAKE returns -1 or at most `n` woken waiters"
        )]
        // SAFETY: `a` is a live futex word; FUTEX_WAKE reads no other pointer.
        unsafe {
            syscall(
                SYS_futex,
                a.as_ptr(),
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
pub extern "C" fn swr_v1_locks_coroutine_try(
    word: Option<&AtomicU32>,
    held_cookie: usize,
    held_cid: i64,
    current_cookie: usize,
    current_cid: i64,
    blocking: u8,
) -> i32 {
    let Some(a) = word else {
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
pub extern "C" fn swr_v1_locks_coroutine_release(word: Option<&AtomicU32>) -> i32 {
    let Some(a) = word else {
        return -1;
    };
    if a.load(Ordering::Relaxed) == 0 {
        return 0;
    }
    // Release: the next holder sees what this one wrote.
    a.store(0, Ordering::Release);
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

    #[test]
    fn lock_and_unlock_refuse_null_misaligned_and_unknown_storage() {
        let mut words = [0u64; 16];
        let misaligned = words
            .as_mut_ptr()
            .cast::<u8>()
            .wrapping_add(1)
            .cast::<c_void>();
        for kind in [3, 1, 99] {
            for storage in [ptr::null_mut(), misaligned] {
                assert_eq!(unsafe { swr_v1_locks_lock(storage, kind, 0, 0, 0) }, EINVAL);
                assert_eq!(unsafe { swr_v1_locks_unlock(storage, kind) }, EINVAL);
            }
        }
        let aligned = words.as_mut_ptr().cast::<c_void>();
        assert_eq!(unsafe { swr_v1_locks_lock(aligned, 99, 0, 0, 0) }, EINVAL);
        assert_eq!(unsafe { swr_v1_locks_unlock(aligned, 99) }, EINVAL);
    }

    /// `swr_v1_locks_coroutine_release` publishes its holder's writes to the next holder
    /// `swr_v1_locks_coroutine_try` admits, whichever process (here thread) that is. The
    /// `HELD` flag only says when to start trying; it orders nothing.
    #[test]
    fn coroutine_release_publishes_the_holders_writes() {
        struct Data(std::cell::UnsafeCell<u64>);
        unsafe impl Sync for Data {}
        impl Data {
            fn get(&self) -> *mut u64 {
                self.0.get()
            }
        }
        let data = Data(std::cell::UnsafeCell::new(0));
        let word = AtomicU32::new(0);
        let held = std::sync::atomic::AtomicBool::new(false);
        let acquire = |cid| {
            while swr_v1_locks_coroutine_try(Some(&word), 0, 0, 1, cid, 1) != 1 {
                thread::yield_now();
            }
        };
        thread::scope(|scope| {
            scope.spawn(|| {
                acquire(1);
                unsafe { *data.get() = 42 };
                held.store(true, Ordering::Relaxed);
                assert_eq!(swr_v1_locks_coroutine_release(Some(&word)), 1);
            });
            scope.spawn(|| {
                while !held.load(Ordering::Relaxed) {
                    thread::yield_now();
                }
                acquire(2);
                assert_eq!(unsafe { *data.get() }, 42);
                assert_eq!(swr_v1_locks_coroutine_release(Some(&word)), 1);
            });
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_barrier_init")]
    fn barrier_releases_every_party_together() {
        let barrier = Box::new(unsafe { std::mem::zeroed::<super::Barrier>() });
        barrier_init(&barrier, false, 4);
        let arrived = AtomicU32::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    arrived.fetch_add(1, Ordering::SeqCst);
                    barrier_wait(&barrier);
                    assert_eq!(arrived.load(Ordering::SeqCst), 4);
                });
            }
        });
        barrier_destroy(&barrier);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
    fn shared_mutex_locks_once_made_and_refuses_after_destroy() {
        let mut mutex = SharedMutex::new();
        assert_eq!(mutex.init(), 0);
        assert_eq!(mutex.lock(), 0);
        assert_eq!(mutex.unlock(), 0);
        mutex.destroy();
        #[cfg(target_os = "macos")]
        assert_eq!(mutex.lock(), EINVAL, "a destroyed mutex");
    }

    #[test]
    fn safe_locks_lock_and_refuse_what_they_cannot_hold() {
        let lock = super::Lock::private(MUTEX).expect("a mutex");
        assert_eq!(lock.lock(LOCK_EX, -1, CAPABILITIES), 0);
        assert_eq!(lock.unlock(), 0);
        assert!(super::Lock::private(42).is_none(), "no such kind");
        // Taken back after the refusal, which keeps no reference, so Miri sees no leak.
        let short = Box::into_raw(Box::new([AtomicU64::new(0)]));
        assert!(
            super::Lock::shared(MUTEX, unsafe { &mut *short }).is_none(),
            "too small"
        );
        drop(unsafe { Box::from_raw(short) });
    }

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

    // Upstream orders the threads with 1 ms and 2 ms sleeps. When load delays the first thread
    // past 1 ms, the try takes a free mutex or spinlock and keeps it, and the first thread's
    // lock blocks forever; barriers keep the read lock held across the try instead.
    fn test_lock_rd_func(lock: &Lock) {
        let (held, tried) = (Barrier::new(2), Barrier::new(2));
        thread::scope(|s| {
            s.spawn(|| {
                assert_eq!(lock.lock(LOCK_SH, -1), 0);
                held.wait();
                tried.wait();
                lock.unlock();
            });
            s.spawn(|| {
                held.wait();
                assert!(lock.lock(LOCK_SH | LOCK_NB, -1) >= 0);
                tried.wait();
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
    #[cfg_attr(miri, ignore = "Miri has no pthread_mutex_timedlock")]
    fn lock_lockwait() {
        const TIMEOUT_1: i32 = 2;
        const TIMEOUT_2: i32 = 10;
        // Upstream unlocks after a fixed 10 ms sleep, which races the waiter's start and its
        // second wait on a loaded runner. Main instead unlocks 1 ms after the first timeout,
        // so the waiter blocks briefly in the second wait with TIMEOUT_2's whole margin left.
        // How soon the waiter runs after the unlock is the scheduler's, and the gates run 40
        // phases at once (#2939), so the bound holds if one of three attempts meets it.
        let attempt = || {
            let lock = Lock::new(MUTEX, false);
            lock.lock(LOCK_EX, -1);
            let timed_out = Barrier::new(2);
            thread::scope(|s| {
                let waiter = s.spawn(|| {
                    let ms1 = millis();
                    assert_eq!(lock.lock(LOCK_EX, TIMEOUT_1), ETIMEDOUT);
                    timed_out.wait();
                    let ms2 = millis();
                    assert!(ms2 - ms1 >= TIMEOUT_1 as u128);
                    assert_eq!(lock.lock(LOCK_EX, TIMEOUT_2), 0);
                    millis() - ms2
                });
                timed_out.wait();
                thread::sleep(Duration::from_millis(1));
                lock.unlock();
                waiter.join().unwrap()
            })
        };
        let waits: Vec<u128> = (0..3)
            .map(|_| attempt())
            .take_while(|&wait| wait > TIMEOUT_2 as u128)
            .collect();
        assert!(waits.len() < 3, "the second wait took {waits:?} ms");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn lock_shared() {
        test_share_lock_fun(&Lock::new(MUTEX, true));
    }

    #[test]
    fn lock_try_rd() {
        test_lock_rd_func(&Lock::new(MUTEX, false));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn lock_rwlock_shared() {
        test_share_lock_fun(&Lock::new(RW_LOCK, true));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_rwlockattr_init")]
    fn lock_rwlock() {
        test_func(&Lock::new(RW_LOCK, false));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_rwlockattr_init")]
    fn lock_rwlock_try_rd() {
        test_lock_rd_func(&Lock::new(RW_LOCK, false));
    }

    // Upstream orders the two threads with 1 ms and 2 ms sleeps and fails now and then on
    // either engine; barriers keep the write lock held across the try instead.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_rwlockattr_init")]
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
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn lock_spinlock_shared() {
        test_share_lock_fun(&Lock::new(SPIN_LOCK, true));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_spin_init")]
    fn lock_spinlock() {
        test_func(&Lock::new(SPIN_LOCK, false));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_spin_init")]
    fn lock_spinlock_try_rd() {
        test_lock_rd_func(&Lock::new(SPIN_LOCK, false));
    }
}
