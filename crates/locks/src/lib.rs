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
use swoole_rs_corebase::sys;

pub mod coroutine;

/// The words pthread keeps a lock or its attribute in: 8-aligned, `Sync` storage that
/// pthread alone reads and writes.
type Words<const N: usize> = [AtomicU64; N];

const fn words<T>() -> usize {
    size_of::<T>().div_ceil(size_of::<AtomicU64>())
}
const MUTEX_LOCK: usize = words::<pthread_mutex_t>();
const MUTEX_ATTR: usize = words::<pthread_mutexattr_t>();
const RW_LOCK: usize = words::<pthread_rwlock_t>();
const RW_ATTR: usize = words::<pthread_rwlockattr_t>();

/// The mutex storage of the lock ABI as C sees it; [`SharedMutex`] is the same bytes.
#[repr(C)]
struct Mutex {
    lock: pthread_mutex_t,
    attr: pthread_mutexattr_t,
}
/// The rwlock storage of the lock ABI as C sees it; [`RwLock`] is the same bytes.
#[repr(C)]
struct RwLockLayout {
    lock: pthread_rwlock_t,
    attr: pthread_rwlockattr_t,
}
/// A rwlock and the attribute it was made from, in the lock ABI's storage.
#[derive(Debug)]
#[repr(C)]
struct RwLock {
    lock: Words<RW_LOCK>,
    attr: Words<RW_ATTR>,
}
/// A spinlock's storage.
#[cfg(target_os = "linux")]
type Spin = AtomicU32;

const _: () = {
    assert!(
        size_of::<SharedMutex>() == size_of::<Mutex>()
            && align_of::<SharedMutex>() == align_of::<Mutex>()
            && offset_of!(SharedMutex, attr) == offset_of!(Mutex, attr),
        "the words are C's mutex and attribute"
    );
    assert!(
        size_of::<RwLock>() == size_of::<RwLockLayout>()
            && align_of::<RwLock>() == align_of::<RwLockLayout>()
            && offset_of!(RwLock, attr) == offset_of!(RwLockLayout, attr),
        "the words are C's rwlock and attribute"
    );
};
#[cfg(target_os = "linux")]
const _: () = assert!(
    size_of::<Spin>() == size_of::<pthread_spinlock_t>()
        && align_of::<Spin>() == align_of::<pthread_spinlock_t>(),
    "the word is C's spinlock"
);

/// A mutex in storage live for `'a`: one [`init_mutex`] made, or [`SharedMutex`]'s
/// zeroed or destroyed storage, which pthread on these platforms refuses or (glibc's
/// zeroed mutex) takes as an unlocked one. Only this module makes one, from storage in
/// one of those states, so pthread's calls on it are declared `safe`.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct MutexRef<'a>(&'a Words<MUTEX_LOCK>);

/// A rwlock [`init_rwlock`] made and nothing has destroyed, live for `'a`. Only this
/// module makes one.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct RwLockRef<'a>(&'a Words<RW_LOCK>);

/// A spinlock `pthread_spin_init` made and nothing has destroyed, live for `'a`. Only
/// this module makes one.
#[cfg(target_os = "linux")]
#[repr(transparent)]
#[derive(Clone, Copy)]
struct SpinRef<'a>(&'a Spin);

// pthread.h's prototypes, with each pointer as the reference or handle it stands for.
// The init calls write the one object their `&mut` names, whatever it held, from an
// attribute, which on Linux and macOS is plain bits its own calls read and write
// (macOS refuses one `init` didn't make). The lock calls take a handle, whose type says
// the lock was made, and report misuse of it (unlocking a lock this thread doesn't
// hold, destroying a held one) as an error code. None keeps a reference past its
// return, and the deadline is read during the call.
unsafe extern "C" {
    safe fn pthread_mutexattr_init(attr: &mut Words<MUTEX_ATTR>) -> c_int;
    safe fn pthread_mutexattr_setpshared(attr: &mut Words<MUTEX_ATTR>, shared: c_int) -> c_int;
    safe fn pthread_mutexattr_destroy(attr: &Words<MUTEX_ATTR>) -> c_int;
    safe fn pthread_mutex_init(lock: &mut Words<MUTEX_LOCK>, attr: &Words<MUTEX_ATTR>) -> c_int;
    safe fn pthread_mutex_lock(lock: MutexRef<'_>) -> c_int;
    safe fn pthread_mutex_trylock(lock: MutexRef<'_>) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn pthread_mutex_timedlock(lock: MutexRef<'_>, deadline: &timespec) -> c_int;
    safe fn pthread_mutex_unlock(lock: MutexRef<'_>) -> c_int;
    safe fn pthread_mutex_destroy(lock: MutexRef<'_>) -> c_int;

    safe fn pthread_rwlockattr_init(attr: &mut Words<RW_ATTR>) -> c_int;
    safe fn pthread_rwlockattr_setpshared(attr: &mut Words<RW_ATTR>, shared: c_int) -> c_int;
    safe fn pthread_rwlockattr_destroy(attr: &Words<RW_ATTR>) -> c_int;
    safe fn pthread_rwlock_init(lock: &mut Words<RW_LOCK>, attr: &Words<RW_ATTR>) -> c_int;
    safe fn pthread_rwlock_rdlock(lock: RwLockRef<'_>) -> c_int;
    safe fn pthread_rwlock_wrlock(lock: RwLockRef<'_>) -> c_int;
    safe fn pthread_rwlock_tryrdlock(lock: RwLockRef<'_>) -> c_int;
    safe fn pthread_rwlock_trywrlock(lock: RwLockRef<'_>) -> c_int;
    // libc 0.2.191 omits these two Linux declarations.
    #[cfg(target_os = "linux")]
    safe fn pthread_rwlock_timedrdlock(lock: RwLockRef<'_>, deadline: &timespec) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn pthread_rwlock_timedwrlock(lock: RwLockRef<'_>, deadline: &timespec) -> c_int;
    safe fn pthread_rwlock_unlock(lock: RwLockRef<'_>) -> c_int;
    safe fn pthread_rwlock_destroy(lock: RwLockRef<'_>) -> c_int;

    #[cfg(target_os = "linux")]
    safe fn pthread_spin_init(lock: &mut Spin, shared: c_int) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn pthread_spin_lock(lock: SpinRef<'_>) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn pthread_spin_trylock(lock: SpinRef<'_>) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn pthread_spin_unlock(lock: SpinRef<'_>) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn pthread_spin_destroy(lock: SpinRef<'_>) -> c_int;
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_size(kind: i32) -> usize {
    match kind {
        3 => size_of::<Mutex>(),
        1 => size_of::<RwLockLayout>(),
        #[cfg(target_os = "linux")]
        5 => size_of::<pthread_spinlock_t>(),
        _ => 0,
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_locks_align(kind: i32) -> usize {
    match kind {
        3 => align_of::<Mutex>(),
        1 => align_of::<RwLockLayout>(),
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
/// Makes `lock` a mutex from `attr`, which it initializes: 0, or the first error,
/// which leaves neither made.
fn init_mutex(lock: &mut Words<MUTEX_LOCK>, attr: &mut Words<MUTEX_ATTR>, shared: i32) -> c_int {
    let result = pthread_mutexattr_init(attr);
    if result != 0 {
        return result;
    }
    let attributes = if shared != 0 {
        pthread_mutexattr_setpshared(attr, PTHREAD_PROCESS_SHARED)
    } else {
        0
    };
    let result = if attributes != 0 {
        attributes
    } else {
        pthread_mutex_init(lock, attr)
    };
    init_result(result, || {
        pthread_mutexattr_destroy(attr);
    })
}
/// [`init_mutex`] for a rwlock.
fn init_rwlock(lock: &mut Words<RW_LOCK>, attr: &mut Words<RW_ATTR>, shared: i32) -> c_int {
    let result = pthread_rwlockattr_init(attr);
    if result != 0 {
        return result;
    }
    let attributes = if shared != 0 {
        pthread_rwlockattr_setpshared(attr, PTHREAD_PROCESS_SHARED)
    } else {
        0
    };
    let result = if attributes != 0 {
        attributes
    } else {
        pthread_rwlock_init(lock, attr)
    };
    init_result(result, || {
        pthread_rwlockattr_destroy(attr);
    })
}
impl RwLock {
    fn live(&self) -> RwLockRef<'_> {
        RwLockRef(&self.lock)
    }

    /// Destroys the attribute and the rwlock [`init_rwlock`] made.
    fn destroy(&self) {
        pthread_rwlockattr_destroy(&self.attr);
        pthread_rwlock_destroy(self.live());
    }
}
// Lock storage below passed `valid`: non-null and aligned for its kind, and the caller
// lends `swr_v1_locks_size(kind)` bytes of it for the call. Native code serializes
// init and destroy against every other use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_init(storage: *mut c_void, kind: i32, shared: i32) -> i32 {
    if !valid(storage, kind) {
        return EINVAL;
    }
    // SAFETY: the kind's size in writable bytes, as the caller lends: zeroed, so that
    // the words below are values before pthread makes a lock of them.
    unsafe { storage.cast::<u8>().write_bytes(0, swr_v1_locks_size(kind)) };
    match kind {
        3 => {
            // SAFETY: writable, unused storage of a mutex's size and alignment, lent to
            // this call alone.
            let mutex = unsafe { &mut *storage.cast::<SharedMutex>() };
            init_mutex(&mut mutex.lock, &mut mutex.attr, shared)
        }
        1 => {
            // SAFETY: as above, of a rwlock's.
            let rwlock = unsafe { &mut *storage.cast::<RwLock>() };
            init_rwlock(&mut rwlock.lock, &mut rwlock.attr, shared)
        }
        #[cfg(target_os = "linux")]
        // SAFETY: as above, of a spinlock's.
        5 => pthread_spin_init(unsafe { &mut *storage.cast::<Spin>() }, shared),
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
/// A made lock of one kind, live for `'a`.
#[derive(Clone, Copy)]
enum Live<'a> {
    Mutex(MutexRef<'a>),
    RwLock(RwLockRef<'a>),
    #[cfg(target_os = "linux")]
    Spin(SpinRef<'a>),
}

impl Live<'_> {
    /// `Lock::lock(operation, timeout)` with the timed waits `capabilities` names (bit 0
    /// mutex timedlock, bit 1 rwlock timed{rd,wr}lock, from native configure).
    fn lock(self, operation: i32, timeout: i32, capabilities: u32) -> i32 {
        #[cfg(not(target_os = "linux"))]
        let _ = capabilities;
        match self {
            Self::Mutex(lock) => {
                let try_lock = || pthread_mutex_trylock(lock);
                if operation & LOCK_NB != 0 {
                    return try_lock();
                }
                if timeout > 0 {
                    #[cfg(target_os = "linux")]
                    if capabilities & 1 != 0 {
                        return pthread_mutex_timedlock(lock, &deadline(timeout));
                    }
                    return poll(timeout, try_lock);
                }
                pthread_mutex_lock(lock)
            }
            Self::RwLock(lock) => {
                let read = operation & LOCK_SH != 0;
                let try_lock = || {
                    if read {
                        pthread_rwlock_tryrdlock(lock)
                    } else {
                        pthread_rwlock_trywrlock(lock)
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
                            pthread_rwlock_timedrdlock(lock, &deadline)
                        } else {
                            pthread_rwlock_timedwrlock(lock, &deadline)
                        };
                    }
                    return poll(timeout, try_lock);
                }
                if read {
                    pthread_rwlock_rdlock(lock)
                } else {
                    pthread_rwlock_wrlock(lock)
                }
            }
            #[cfg(target_os = "linux")]
            Self::Spin(lock) => {
                let try_lock = || pthread_spin_trylock(lock);
                if operation & LOCK_NB != 0 {
                    return try_lock();
                }
                if timeout > 0 {
                    return poll(timeout, try_lock);
                }
                pthread_spin_lock(lock)
            }
        }
    }

    fn unlock(self) -> i32 {
        match self {
            Self::Mutex(lock) => pthread_mutex_unlock(lock),
            Self::RwLock(lock) => pthread_rwlock_unlock(lock),
            #[cfg(target_os = "linux")]
            Self::Spin(lock) => pthread_spin_unlock(lock),
        }
    }
}
// Every call below takes a lock that `valid` checked and the caller initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_lock(
    storage: *mut c_void,
    kind: i32,
    operation: i32,
    timeout: i32,
    capabilities: u32,
) -> i32 {
    let lock = match kind {
        // SAFETY: an initialized mutex, live for the call.
        3 if holds::<Mutex>(storage) => unsafe { &*storage.cast::<SharedMutex>() }.live(),
        1 if holds::<RwLockLayout>(storage) => {
            // SAFETY: an initialized rwlock, live for the call.
            Live::RwLock(unsafe { &*storage.cast::<RwLock>() }.live())
        }
        #[cfg(target_os = "linux")]
        5 if holds::<pthread_spinlock_t>(storage) => {
            // SAFETY: an initialized spinlock, live for the call.
            Live::Spin(SpinRef(unsafe { &*storage.cast::<Spin>() }))
        }
        _ => return EINVAL,
    };
    lock.lock(operation, timeout, capabilities)
}
/// Marks its caller's path unlikely: `std::hint::cold_path`, which the fuzz toolchain
/// doesn't have yet.
#[cold]
const fn cold() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_unlock(storage: *mut c_void, kind: i32) -> i32 {
    if kind == 3 && holds::<Mutex>(storage) {
        // SAFETY: an initialized mutex that this thread holds.
        return unsafe { &*storage.cast::<SharedMutex>() }.unlock();
    }
    // After the mutex every Table row, task lock and default `Lock` is.
    cold();
    match kind {
        1 if holds::<RwLockLayout>(storage) => {
            // SAFETY: an initialized rwlock that this thread holds.
            pthread_rwlock_unlock(unsafe { &*storage.cast::<RwLock>() }.live())
        }
        #[cfg(target_os = "linux")]
        5 if holds::<pthread_spinlock_t>(storage) => {
            // SAFETY: an initialized spinlock that this thread holds.
            pthread_spin_unlock(SpinRef(unsafe { &*storage.cast::<Spin>() }))
        }
        _ => EINVAL,
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_destroy(storage: *mut c_void, kind: i32) {
    if !valid(storage, kind) {
        return;
    }
    match kind {
        // SAFETY: the mutex `swr_v1_locks_init` made, unlocked and destroyed once.
        3 => unsafe { &*storage.cast::<SharedMutex>() }.destroy(),
        // SAFETY: the rwlock `swr_v1_locks_init` made, unlocked and destroyed once.
        1 => unsafe { &*storage.cast::<RwLock>() }.destroy(),
        #[cfg(target_os = "linux")]
        5 => {
            // SAFETY: an initialized, unlocked spinlock, destroyed once.
            pthread_spin_destroy(SpinRef(unsafe { &*storage.cast::<Spin>() }));
        }
        _ => (),
    }
}

/// An initialized lock of one kind (`Lock::Type`: rwlock 1, mutex 3, spinlock 5),
/// in heap storage it owns or in process-shared storage it keeps for the
/// program's life. pthread reports misuse (unlocking a lock this thread doesn't
/// hold, destroying a held one) as an error code, as upstream's locks see it.
#[derive(Debug)]
pub struct Lock(Held);

/// A thread-private lock's storage, destroyed with it, or a shared lock's, which forked
/// processes may still hold.
#[derive(Debug)]
enum Held {
    Mutex(Box<SharedMutex>),
    SharedMutex(&'static Words<MUTEX_LOCK>),
    RwLock(Box<RwLock>),
    SharedRwLock(&'static Words<RW_LOCK>),
    #[cfg(target_os = "linux")]
    Spin(Box<Spin>),
    #[cfg(target_os = "linux")]
    SharedSpin(&'static Spin),
}

impl Lock {
    /// A thread-private lock (`new Mutex(false)` and its rwlock and spinlock
    /// twins); None for a kind this platform lacks, or when it can't be made.
    #[must_use]
    pub fn private(kind: i32) -> Option<Self> {
        match kind {
            3 => {
                let mut mutex = Box::new(SharedMutex::new());
                (init_mutex(&mut mutex.lock, &mut mutex.attr, 0) == 0)
                    .then_some(Self(Held::Mutex(mutex)))
            }
            1 => {
                let mut rwlock = Box::new(RwLock {
                    lock: [const { AtomicU64::new(0) }; RW_LOCK],
                    attr: [const { AtomicU64::new(0) }; RW_ATTR],
                });
                (init_rwlock(&mut rwlock.lock, &mut rwlock.attr, 0) == 0)
                    .then_some(Self(Held::RwLock(rwlock)))
            }
            #[cfg(target_os = "linux")]
            5 => {
                let mut spin = Box::new(Spin::new(0));
                (pthread_spin_init(&mut spin, 0) == 0).then_some(Self(Held::Spin(spin)))
            }
            _ => None,
        }
    }

    /// A process-shared lock (`new Mutex(true)`) in `storage`, which it keeps:
    /// None when `storage` is too small for `kind`, or the lock can't be made.
    /// The storage is exclusive, so no other lock re-initializes it while held.
    #[must_use]
    pub fn shared(kind: i32, storage: &'static mut [AtomicU64]) -> Option<Self> {
        match kind {
            3 => {
                let (lock, attr) = storage.split_first_chunk_mut()?;
                let lock: &'static mut Words<_> = lock;
                (init_mutex(lock, attr.first_chunk_mut()?, 1) == 0)
                    .then_some(Self(Held::SharedMutex(lock)))
            }
            1 => {
                let (lock, attr) = storage.split_first_chunk_mut()?;
                let lock: &'static mut Words<_> = lock;
                (init_rwlock(lock, attr.first_chunk_mut()?, 1) == 0)
                    .then_some(Self(Held::SharedRwLock(lock)))
            }
            #[cfg(target_os = "linux")]
            5 => {
                let word: *mut AtomicU64 = storage.first_mut()?;
                // SAFETY: the first bytes of an exclusive word that lives for the program,
                // which has a spinlock's size and alignment to spare.
                let spin = unsafe { &mut *word.cast::<Spin>() };
                (pthread_spin_init(spin, 1) == 0).then_some(Self(Held::SharedSpin(spin)))
            }
            _ => None,
        }
    }

    fn live(&self) -> Live<'_> {
        match &self.0 {
            Held::Mutex(mutex) => mutex.live(),
            Held::SharedMutex(lock) => Live::Mutex(MutexRef(lock)),
            Held::RwLock(rwlock) => Live::RwLock(rwlock.live()),
            Held::SharedRwLock(lock) => Live::RwLock(RwLockRef(lock)),
            #[cfg(target_os = "linux")]
            Held::Spin(spin) => Live::Spin(SpinRef(spin)),
            #[cfg(target_os = "linux")]
            Held::SharedSpin(spin) => Live::Spin(SpinRef(spin)),
        }
    }

    /// `Lock::lock(operation, timeout)`: `LOCK_SH`/`LOCK_NB` from `operation`,
    /// `timeout` in milliseconds (none when not positive), the timed waits
    /// `capabilities` names (bit 0 mutex, bit 1 rwlock); 0 or an errno.
    #[must_use]
    pub fn lock(&self, operation: i32, timeout: i32, capabilities: u32) -> i32 {
        let lock = self.live();
        // A blocking mutex lock, the common case, is small enough to inline.
        if let Live::Mutex(mutex) = lock
            && operation & LOCK_NB == 0
            && timeout <= 0
        {
            return pthread_mutex_lock(mutex);
        }
        lock.lock(operation, timeout, capabilities)
    }

    /// `Lock::unlock()`: 0 or an errno.
    #[must_use]
    pub fn unlock(&self) -> i32 {
        self.live().unlock()
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        match &self.0 {
            Held::Mutex(mutex) => mutex.destroy(),
            Held::RwLock(rwlock) => rwlock.destroy(),
            #[cfg(target_os = "linux")]
            Held::Spin(spin) => {
                pthread_spin_destroy(SpinRef(spin));
            }
            _ => (),
        }
    }
}

/// A process-shared mutex (`new Mutex(true)`) in storage its owner places, such as a
/// shared-memory header: zeroed until [`init`](Self::init) and after
/// [`destroy`](Self::destroy). On these platforms pthread reports locking
/// zeroed or destroyed storage, and misuse of an initialized mutex, as an error code.
#[derive(Debug)]
#[repr(C)]
pub struct SharedMutex {
    lock: Words<MUTEX_LOCK>,
    attr: Words<MUTEX_ATTR>,
}

impl SharedMutex {
    /// Zeroed storage, not yet a mutex.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lock: [const { AtomicU64::new(0) }; MUTEX_LOCK],
            attr: [const { AtomicU64::new(0) }; MUTEX_ATTR],
        }
    }

    fn live(&self) -> Live<'_> {
        Live::Mutex(MutexRef(&self.lock))
    }

    /// Makes the storage a process-shared mutex: 0 or an errno. Processes that share
    /// it don't use it until this returns.
    #[must_use]
    pub fn init(&mut self) -> i32 {
        init_mutex(&mut self.lock, &mut self.attr, 1)
    }

    /// `Mutex::lock()`, blocking: 0 or an errno.
    #[must_use]
    pub fn lock(&self) -> i32 {
        pthread_mutex_lock(MutexRef(&self.lock))
    }

    /// `Mutex::unlock()`: 0 or an errno.
    #[must_use]
    pub fn unlock(&self) -> i32 {
        pthread_mutex_unlock(MutexRef(&self.lock))
    }

    /// Destroys the mutex `init` made. pthread refuses to destroy a held one, and
    /// refuses to lock it afterwards.
    pub fn destroy(&self) {
        pthread_mutexattr_destroy(&self.attr);
        pthread_mutex_destroy(MutexRef(&self.lock));
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
/// The pthread barrier and its attribute are kept as atomic words only to be aligned,
/// `Sync` storage that pthread alone reads and writes. Nothing here makes a `Barrier`:
/// its owner zeroes one, as C++'s `Barrier barrier = {}`, so one is zeroed, made by
/// [`barrier_init`] or destroyed.
#[repr(C)]
pub struct Barrier {
    #[cfg(target_os = "linux")]
    barrier: Words<{ size_of::<pthread_barrier_t>() / size_of::<AtomicU64>() }>,
    #[cfg(target_os = "linux")]
    attr: BarrierAttr,
    #[cfg(target_os = "linux")]
    shared: std::sync::atomic::AtomicBool,
    #[cfg(not(target_os = "linux"))]
    count: AtomicU32,
    #[cfg(not(target_os = "linux"))]
    arrived: AtomicU32,
}

/// A `pthread_barrierattr_t`'s storage: on Linux an int of plain bits.
#[cfg(target_os = "linux")]
type BarrierAttr = [AtomicU32; size_of::<pthread_barrierattr_t>() / size_of::<AtomicU32>()];

#[cfg(target_os = "linux")]
const _: () = assert!(
    size_of::<pthread_barrier_t>().is_multiple_of(size_of::<AtomicU64>())
        && align_of::<pthread_barrier_t>() == align_of::<AtomicU64>()
        && size_of::<BarrierAttr>() == size_of::<pthread_barrierattr_t>()
        && align_of::<BarrierAttr>() == align_of::<pthread_barrierattr_t>(),
    "the words are C's barrier and attribute"
);

// pthread.h's prototypes. A `Barrier` begins with its pthread barrier, so the reference
// is the `pthread_barrier_t *`, to a barrier in one of the states the type keeps:
// pthread waits on or destroys one `barrier_init` made, and finds no waiters or
// instance in a zeroed one. The attribute calls read and write the attribute's bits,
// and `pthread_barrier_init` writes the barrier from them, or from the defaults for
// none. None keeps a reference past its return.
#[cfg(target_os = "linux")]
unsafe extern "C" {
    safe fn pthread_barrierattr_init(attr: &BarrierAttr) -> c_int;
    safe fn pthread_barrierattr_setpshared(attr: &BarrierAttr, shared: c_int) -> c_int;
    safe fn pthread_barrierattr_destroy(attr: &BarrierAttr) -> c_int;
    safe fn pthread_barrier_init(
        barrier: &Barrier,
        attr: Option<&BarrierAttr>,
        count: c_uint,
    ) -> c_int;
    safe fn pthread_barrier_wait(barrier: &Barrier) -> c_int;
    safe fn pthread_barrier_destroy(barrier: &Barrier) -> c_int;
}

/// `Barrier::init(bool shared, int count)`. Shared attrs are initialized before
/// `setpshared`, fixing upstream's use of an uninitialized attribute.
#[unsafe(export_name = "_ZN6swoole7Barrier4initEbi")]
pub extern "C" fn barrier_init(barrier: &Barrier, shared: bool, count: c_int) {
    #[cfg(target_os = "linux")]
    {
        barrier.shared.store(shared, Ordering::Relaxed);
        if shared {
            pthread_barrierattr_init(&barrier.attr);
            pthread_barrierattr_setpshared(&barrier.attr, PTHREAD_PROCESS_SHARED);
        }
        // The caller initializes an unused barrier once, before any wait.
        pthread_barrier_init(
            barrier,
            shared.then_some(&barrier.attr),
            count.cast_unsigned(),
        );
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
    pthread_barrier_wait(barrier);
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
        // The barrier is initialized, has no waiters and is destroyed once.
        pthread_barrier_destroy(barrier);
        if barrier.shared.load(Ordering::Relaxed) {
            pthread_barrierattr_destroy(&barrier.attr);
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "upstream's split of the seconds; Rust's float casts saturate where C's are undefined"
    )]
    let duration = timespec {
        tv_sec: timeout as time_t,
        tv_nsec: ((timeout - (timeout as time_t) as f64) * 1000.0 * 1000.0 * 1000.0) as c_long,
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
