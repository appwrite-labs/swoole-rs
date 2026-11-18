//! Native-compatible synchronization.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("Rust locks require validated macOS or Linux platform contracts");

// libc's surface is platform-specific here (e.g. __error on macOS vs
// __errno_location on Linux, selected by cfg below), so it is not portably enumerable.
#[cfg(target_os = "linux")]
use crate::CCast;
use crate::sys;
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "libc's platform-specific surface is not portably enumerable"
    )
)]
use libc::*;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

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

/// A rwlock and the attribute it was made from.
#[derive(Debug)]
struct RwLock {
    lock: Words<RW_LOCK>,
    attr: Words<RW_ATTR>,
}
/// A spinlock's storage: a word, the lock in its first bytes.
#[cfg(target_os = "linux")]
type Spin = AtomicU64;

// pthread takes each lock and attribute by its own field, so only each field's size
// (`words`) and alignment matter, not where the fields sit.
const _: () = assert!(
    align_of::<AtomicU64>() >= align_of::<pthread_mutex_t>()
        && align_of::<AtomicU64>() >= align_of::<pthread_mutexattr_t>()
        && align_of::<AtomicU64>() >= align_of::<pthread_rwlock_t>()
        && align_of::<AtomicU64>() >= align_of::<pthread_rwlockattr_t>(),
    "a word is aligned for each pthread type"
);
#[cfg(target_os = "linux")]
const _: () = assert!(
    size_of::<Spin>() >= size_of::<pthread_spinlock_t>()
        && align_of::<Spin>() >= align_of::<pthread_spinlock_t>(),
    "the word holds C's spinlock"
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

// Failed lock initialization owns an initialized attribute, but not a live lock.
fn init_result(result: c_int, destroy_attr: impl FnOnce()) -> Result<(), c_int> {
    if result == 0 {
        return Ok(());
    }
    // The failed init's errno, which destroying the attribute must not replace.
    let saved = sys::errno();
    destroy_attr();
    sys::set_errno(saved);
    Err(result)
}
/// Makes `lock` a mutex from `attr`, which it initializes, or the first error,
/// which leaves neither made.
fn init_mutex(
    lock: &mut Words<MUTEX_LOCK>,
    attr: &mut Words<MUTEX_ATTR>,
    shared: bool,
) -> Result<(), c_int> {
    let result = pthread_mutexattr_init(attr);
    if result != 0 {
        return Err(result);
    }
    // Miri has no pthread_mutexattr_setpshared, and with no fork a process-shared
    // mutex is no different.
    let attributes = if shared && !cfg!(miri) {
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
fn init_rwlock(
    lock: &mut Words<RW_LOCK>,
    attr: &mut Words<RW_ATTR>,
    shared: bool,
) -> Result<(), c_int> {
    let result = pthread_rwlockattr_init(attr);
    if result != 0 {
        return Err(result);
    }
    let attributes = if shared {
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
// Match sw_wait_for's exact attempt/sleep/backoff order, including its final attempt.
fn poll(mut timeout: i32, mut attempt: impl FnMut() -> i32) -> Result<(), i32> {
    let mut sleep: i32 = 1;
    while timeout >= 0 {
        if attempt() == 0 {
            return Ok(());
        }
        _ = sys::usleep(sleep.wrapping_mul(1000).cast_unsigned());
        sleep = sleep.wrapping_mul(2);
        if timeout > 0 && timeout.wrapping_sub(sleep) < 0 {
            sleep = timeout;
            timeout = 0;
        } else {
            timeout = timeout.wrapping_sub(sleep);
        }
    }
    Err(ETIMEDOUT)
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
/// A thread-private mutex in one futex word (Linux): 0 unlocked, 1 locked, 2 locked
/// with waiters. Like glibc's default mutex it has no owner, so any thread may unlock
/// it, as `Thread\Lock` lets one thread unlock what another locked; for a pthread
/// mutex that is undefined.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Word(AtomicU32);

#[cfg(target_os = "linux")]
impl Word {
    fn try_lock(&self) -> Result<(), i32> {
        self.0
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .map(drop)
            .map_err(|_| EBUSY)
    }

    /// Blocks until it holds the word, or `millis` (when positive) pass: `ETIMEDOUT`.
    fn lock(&self, millis: i32) -> Result<(), i32> {
        if self.try_lock().is_ok() {
            return Ok(());
        }
        let deadline = (millis > 0).then(|| {
            std::time::Instant::now()
                + std::time::Duration::from_millis(millis.unsigned_abs().into())
        });
        while self.0.swap(2, Ordering::Acquire) != 0 {
            let left = match deadline {
                None => None,
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        return Err(ETIMEDOUT);
                    }
                    Some(timespec {
                        tv_sec: left.as_secs().c_cast::<time_t>(),
                        tv_nsec: c_long::from(left.subsec_nanos().c_cast::<i32>()),
                    })
                }
            };
            // Parks while the word still says locked with waiters; a wake, a change or
            // the timeout returns, and the swap above decides.
            futex(&self.0, Futex::Wait, 2, left.as_ref());
        }
        Ok(())
    }

    fn unlock(&self) {
        if self.0.swap(0, Ordering::Release) == 2 {
            futex(&self.0, Futex::Wake, 1, None);
        }
    }
}

/// A made lock of one kind, live for `'a`.
#[derive(Clone, Copy)]
enum Live<'a> {
    Mutex(MutexRef<'a>),
    #[cfg(target_os = "linux")]
    Word(&'a Word),
    RwLock(RwLockRef<'a>),
    #[cfg(target_os = "linux")]
    Spin(SpinRef<'a>),
}

impl Live<'_> {
    /// `Lock::lock(operation, timeout)` with the timed waits `capabilities` names (bit 0
    /// mutex timedlock, bit 1 rwlock timed{rd,wr}lock, from native configure).
    fn lock(self, operation: i32, timeout: i32, capabilities: u32) -> Result<(), i32> {
        #[cfg(not(target_os = "linux"))]
        let _ = capabilities;
        match self {
            Self::Mutex(lock) => {
                let try_lock = || pthread_mutex_trylock(lock);
                if operation & LOCK_NB != 0 {
                    return pthread_result(try_lock());
                }
                if timeout > 0 {
                    #[cfg(target_os = "linux")]
                    if capabilities & 1 != 0 {
                        return pthread_result(pthread_mutex_timedlock(lock, &deadline(timeout)));
                    }
                    return poll(timeout, try_lock);
                }
                pthread_result(pthread_mutex_lock(lock))
            }
            #[cfg(target_os = "linux")]
            Self::Word(word) => {
                if operation & LOCK_NB != 0 {
                    word.try_lock()
                } else {
                    word.lock(timeout)
                }
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
                    return pthread_result(try_lock());
                }
                if timeout > 0 {
                    #[cfg(target_os = "linux")]
                    if capabilities & 2 != 0 {
                        let deadline = deadline(timeout);
                        return pthread_result(if read {
                            pthread_rwlock_timedrdlock(lock, &deadline)
                        } else {
                            pthread_rwlock_timedwrlock(lock, &deadline)
                        });
                    }
                    return poll(timeout, try_lock);
                }
                pthread_result(if read {
                    pthread_rwlock_rdlock(lock)
                } else {
                    pthread_rwlock_wrlock(lock)
                })
            }
            #[cfg(target_os = "linux")]
            Self::Spin(lock) => {
                let try_lock = || pthread_spin_trylock(lock);
                if operation & LOCK_NB != 0 {
                    return pthread_result(try_lock());
                }
                if timeout > 0 {
                    return poll(timeout, try_lock);
                }
                pthread_result(pthread_spin_lock(lock))
            }
        }
    }

    fn unlock(self) -> Result<(), i32> {
        match self {
            Self::Mutex(lock) => pthread_result(pthread_mutex_unlock(lock)),
            #[cfg(target_os = "linux")]
            Self::Word(word) => {
                word.unlock();
                Ok(())
            }
            Self::RwLock(lock) => pthread_result(pthread_rwlock_unlock(lock)),
            #[cfg(target_os = "linux")]
            Self::Spin(lock) => pthread_result(pthread_spin_unlock(lock)),
        }
    }
}
/// `swoole::Lock::Type`, as far as this build has it: upstream's `HAVE_RWLOCK` (both
/// platforms) and `HAVE_SPINLOCK` (Linux; macOS has no `pthread_spin_lock`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Kind {
    RwLock = 1,
    Mutex = 3,
    #[cfg(target_os = "linux")]
    Spin = 5,
}

impl Kind {
    /// Every kind this build has.
    pub const ALL: &[Self] = &[
        Self::Mutex,
        Self::RwLock,
        #[cfg(target_os = "linux")]
        Self::Spin,
    ];

    /// The kind of a `Lock::Type` code; None for a code no kind has, or one this
    /// platform lacks.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            1 => Self::RwLock,
            3 => Self::Mutex,
            #[cfg(target_os = "linux")]
            5 => Self::Spin,
            _ => return None,
        })
    }

    /// The kind's `Lock::Type` code.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }
}

/// An initialized lock of one [`Kind`], in heap storage it owns or in process-shared storage it keeps for the
/// program's life. pthread reports misuse (unlocking a lock this thread doesn't
/// hold, destroying a held one) as an error code, as upstream's locks see it, except
/// for [`Lock::unowned_mutex`]'s.
#[derive(Debug)]
pub struct Lock(Held);

/// A thread-private lock's storage, destroyed with it, or a shared lock's, which forked
/// processes may still hold.
#[derive(Debug)]
enum Held {
    Mutex(Box<SharedMutex>),
    #[cfg(target_os = "linux")]
    Word(Box<Word>),
    SharedMutex(&'static Words<MUTEX_LOCK>),
    RwLock(Box<RwLock>),
    SharedRwLock(&'static Words<RW_LOCK>),
    #[cfg(target_os = "linux")]
    Spin(Box<Spin>),
    #[cfg(target_os = "linux")]
    SharedSpin(&'static Spin),
}

impl Lock {
    /// The words [`shared`](Self::shared) needs for a lock of `kind`: a mutex's or
    /// rwlock's with its attribute, or a spinlock's.
    #[must_use]
    pub const fn words(kind: Kind) -> usize {
        match kind {
            Kind::Mutex => MUTEX_LOCK + MUTEX_ATTR,
            Kind::RwLock => RW_LOCK + RW_ATTR,
            #[cfg(target_os = "linux")]
            Kind::Spin => 1,
        }
    }

    /// A thread-private lock (`new Mutex(false)` and its rwlock and spinlock
    /// twins); None when it can't be made.
    #[must_use]
    pub fn private(kind: Kind) -> Option<Self> {
        match kind {
            Kind::Mutex => {
                let mut mutex = Box::new(SharedMutex::new());
                // Not `then_some`: dropping a `Lock` destroys it, and a failed init made none.
                init_mutex(&mut mutex.lock, &mut mutex.attr, false)
                    .is_ok()
                    .then(|| Self(Held::Mutex(mutex)))
            }
            Kind::RwLock => {
                let mut rwlock = Box::new(RwLock {
                    lock: [const { AtomicU64::new(0) }; RW_LOCK],
                    attr: [const { AtomicU64::new(0) }; RW_ATTR],
                });
                init_rwlock(&mut rwlock.lock, &mut rwlock.attr, false)
                    .is_ok()
                    .then(|| Self(Held::RwLock(rwlock)))
            }
            #[cfg(target_os = "linux")]
            Kind::Spin => {
                let mut spin = Box::new(Spin::new(0));
                (pthread_spin_init(&mut spin, 0) == 0).then(|| Self(Held::Spin(spin)))
            }
        }
    }

    /// `Thread\Lock`'s mutex: thread-private, and any thread may unlock it, as
    /// upstream's tests have one thread unlock what another locked. On Linux that is a
    /// [`Word`]; elsewhere a [`private`](Self::private) pthread mutex, for which an
    /// unlock by another thread is undefined.
    #[must_use]
    pub fn unowned_mutex() -> Option<Self> {
        #[cfg(target_os = "linux")]
        return Some(Self(Held::Word(Box::new(Word(AtomicU32::new(0))))));
        #[cfg(not(target_os = "linux"))]
        Self::private(Kind::Mutex)
    }

    /// A process-shared lock (`new Mutex(true)`) in `storage`, which it keeps:
    /// None when `storage` is too small for `kind`, or the lock can't be made.
    /// The storage is exclusive, so no other lock re-initializes it while held.
    #[must_use]
    pub fn shared(kind: Kind, storage: &'static mut [AtomicU64]) -> Option<Self> {
        match kind {
            Kind::Mutex => {
                let (lock, attr) = storage.split_first_chunk_mut()?;
                let lock: &'static mut Words<_> = lock;
                init_mutex(lock, attr.first_chunk_mut()?, true)
                    .is_ok()
                    .then_some(Self(Held::SharedMutex(lock)))
            }
            Kind::RwLock => {
                let (lock, attr) = storage.split_first_chunk_mut()?;
                let lock: &'static mut Words<_> = lock;
                init_rwlock(lock, attr.first_chunk_mut()?, true)
                    .is_ok()
                    .then_some(Self(Held::SharedRwLock(lock)))
            }
            #[cfg(target_os = "linux")]
            Kind::Spin => {
                // An exclusive word that lives for the program.
                let spin: &'static mut Spin = storage.first_mut()?;
                (pthread_spin_init(spin, 1) == 0).then_some(Self(Held::SharedSpin(spin)))
            }
        }
    }

    fn live(&self) -> Live<'_> {
        match &self.0 {
            Held::Mutex(mutex) => Live::Mutex(MutexRef(&mutex.lock)),
            #[cfg(target_os = "linux")]
            Held::Word(word) => Live::Word(word),
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
    /// `capabilities` names (bit 0 mutex, bit 1 rwlock).
    /// # Errors
    /// The errno of a lock not taken.
    pub fn lock(&self, operation: i32, timeout: i32, capabilities: u32) -> Result<(), i32> {
        let lock = self.live();
        // A blocking mutex lock, the common case, is small enough to inline.
        if let Live::Mutex(mutex) = lock
            && operation & LOCK_NB == 0
            && timeout <= 0
        {
            pthread_result(pthread_mutex_lock(mutex))
        } else {
            lock.lock(operation, timeout, capabilities)
        }
    }

    /// `Lock::unlock()`.
    /// # Errors
    /// The errno of a lock not released.
    pub fn unlock(&self) -> Result<(), i32> {
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

    /// Makes the storage a process-shared mutex. Processes that share it don't use
    /// it until this returns.
    /// # Errors
    /// The first errno, which leaves no mutex made.
    pub fn init(&mut self) -> Result<(), c_int> {
        init_mutex(&mut self.lock, &mut self.attr, true)
    }

    /// `Mutex::lock()`, blocking.
    /// # Errors
    /// pthread's errno.
    pub fn lock(&self) -> Result<(), c_int> {
        pthread_result(pthread_mutex_lock(MutexRef(&self.lock)))
    }

    /// `Mutex::unlock()`.
    /// # Errors
    /// pthread's errno.
    pub fn unlock(&self) -> Result<(), c_int> {
        pthread_result(pthread_mutex_unlock(MutexRef(&self.lock)))
    }

    /// Destroys the mutex `init` made. pthread refuses to destroy a held one, and
    /// refuses to lock it afterwards.
    pub fn destroy(&self) {
        pthread_mutexattr_destroy(&self.attr);
        pthread_mutex_destroy(MutexRef(&self.lock));
    }
}

/// A pthread call's result: 0, or the errno it returns.
fn pthread_result(result: c_int) -> Result<(), c_int> {
    if result == 0 { Ok(()) } else { Err(result) }
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
        pub fn $name(storage: Option<&$atomic>, op: u32, value: $word, expected: $word) -> $word {
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

/// Upstream's `swoole::Barrier`, made, awaited and destroyed by [`barrier_init`],
/// [`barrier_wait`] and [`barrier_destroy`]. Linux uses a pthread barrier, as upstream's `SW_USE_PTHREAD_BARRIER`
/// (glibc and musl have `pthread_barrier_init`); macOS keeps the arrival/count words.
/// The pthread barrier and its attribute are kept as atomic words only to be aligned,
/// `Sync` storage that pthread alone reads and writes. Its owner starts with a zeroed one
/// ([`Barrier::new`], or zeroed memory), as C++'s `Barrier barrier = {}`, so one is
/// zeroed, made by [`barrier_init`] or destroyed.
// `repr(C)`: `pthread_barrier_init` takes the record's address as its barrier.
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

impl Barrier {
    /// A zeroed barrier, as C++'s `Barrier barrier = {}`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            barrier: [const { AtomicU64::new(0) };
                size_of::<pthread_barrier_t>() / size_of::<AtomicU64>()],
            #[cfg(target_os = "linux")]
            attr: [const { AtomicU32::new(0) };
                size_of::<pthread_barrierattr_t>() / size_of::<AtomicU32>()],
            #[cfg(target_os = "linux")]
            shared: std::sync::atomic::AtomicBool::new(false),
            #[cfg(not(target_os = "linux"))]
            count: AtomicU32::new(0),
            #[cfg(not(target_os = "linux"))]
            arrived: AtomicU32::new(0),
        }
    }
}

impl Default for Barrier {
    fn default() -> Self {
        Self::new()
    }
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
pub fn barrier_init(barrier: &Barrier, shared: bool, count: c_int) {
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
pub fn barrier_wait(barrier: &Barrier) {
    #[cfg(target_os = "linux")]
    pthread_barrier_wait(barrier);
    #[cfg(not(target_os = "linux"))]
    {
        barrier.arrived.fetch_add(1, Ordering::SeqCst);
        while barrier.arrived.load(Ordering::Relaxed) != barrier.count.load(Ordering::Relaxed) {
            _ = sys::usleep(10_000);
            std::sync::atomic::fence(Ordering::SeqCst);
        }
    }
}

/// `Barrier::destroy()`.
pub fn barrier_destroy(barrier: &Barrier) {
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

/// The two futex operations the words use.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
enum Futex {
    /// Park while the word holds `value`, until woken or the timeout passes.
    Wait,
    /// Wake up to `value` waiters; the timeout is ignored.
    Wake,
}

/// `syscall(SYS_futex, word, op, value, timeout, NULL, 0)`.
#[cfg(target_os = "linux")]
fn futex(word: &AtomicU32, op: Futex, value: c_int, timeout: Option<&timespec>) -> c_long {
    let timeout = timeout.map_or(std::ptr::null(), std::ptr::from_ref);
    let op = match op {
        Futex::Wait => FUTEX_WAIT,
        Futex::Wake => FUTEX_WAKE,
    };
    // SAFETY: a live futex word, and null or a live timeout: neither operation reads
    // any other pointer.
    unsafe {
        syscall(
            SYS_futex,
            word.as_ptr(),
            op,
            value,
            timeout,
            std::ptr::null::<c_void>(),
            0,
        )
    }
}

/// Futex wait on an atomic word (process-shared for a forked waiter): consume a ready
/// token, otherwise one `FUTEX_WAIT` syscall. Whether it took the word's token.
#[cfg(target_os = "linux")]
#[must_use]
pub fn wait(a: &AtomicU32, timeout: f64) -> bool {
    if a.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        return true;
    }
    let duration = timespec {
        tv_sec: timeout as time_t,
        tv_nsec: ((timeout - (timeout as time_t).c_cast::<f64>()) * 1000.0 * 1000.0 * 1000.0)
            .c_cast::<c_long>(),
    };
    // A wakeup between the check above and the syscall makes FUTEX_WAIT fail with
    // EAGAIN; its token is still in the word, so the result alone doesn't decide.
    futex(a, Futex::Wait, 0, (timeout > 0.0).then_some(&duration));
    a.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}
/// Futex wake on an atomic word: the waiters woken, or none when the wake failed.
#[cfg(target_os = "linux")]
#[must_use]
pub fn wakeup(a: &AtomicU32, n: i32) -> Option<u32> {
    if a.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let woken = futex(a, Futex::Wake, n, None);
        // FUTEX_WAKE returns -1 or at most `n` woken waiters.
        u32::try_from(woken).ok()
    } else {
        Some(0)
    }
}

/// A coroutine lock's next sleep: `second` doubled up to `maximum`, then `initial` again.
#[must_use]
pub fn coroutine_backoff(second: f64, initial: f64, maximum: f64) -> f64 {
    if second >= maximum {
        initial
    } else if second * 2.0 < maximum {
        second * 2.0
    } else {
        maximum
    }
}

// core-tests/src/lock/lock.cpp's TESTs over `Lock`, which Swoole\Lock is (the Mutex, RWLock
// and SpinLock classes left the build), with the same inputs and expected values.
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

    /// `swoole_thread/lock.phpt`'s order: a thread locks the mutex and blocks locking
    /// it again until another thread unlocks it, and the unlocking thread's writes are
    /// visible to it once it holds the lock.
    #[test]
    #[cfg(target_os = "linux")]
    #[cfg_attr(miri, ignore = "Miri has no futex syscall")]
    fn another_thread_unlocks_a_private_mutex() {
        let lock = super::Lock::unowned_mutex().expect("a mutex");
        let written = AtomicU32::new(0);
        assert_eq!(lock.lock(LOCK_EX, -1, 0), Ok(()));
        assert_eq!(lock.lock(LOCK_NB, -1, 0), Err(EBUSY), "held");
        assert_eq!(lock.lock(LOCK_EX, 5, 0), Err(ETIMEDOUT), "still held");
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(20));
                written.store(1, Ordering::Relaxed);
                assert_eq!(lock.unlock(), Ok(()));
            });
            assert_eq!(lock.lock(LOCK_EX, -1, 0), Ok(()), "woken by the unlock");
            assert_eq!(written.load(Ordering::Relaxed), 1);
        });
        assert_eq!(lock.unlock(), Ok(()));
        assert_eq!(lock.lock(LOCK_NB, -1, 0), Ok(()), "free again");
        assert_eq!(lock.unlock(), Ok(()));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_barrier_init")]
    fn barrier_releases_every_party_together() {
        let barrier = Box::new(super::Barrier::new());
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
    fn shared_mutex_locks_once_made_and_refuses_after_destroy() {
        let mut mutex = SharedMutex::new();
        assert_eq!(mutex.init(), Ok(()));
        assert_eq!(mutex.lock(), Ok(()));
        assert_eq!(mutex.unlock(), Ok(()));
        mutex.destroy();
        #[cfg(target_os = "macos")]
        assert_eq!(mutex.lock(), Err(EINVAL), "a destroyed mutex");
    }

    #[test]
    fn safe_locks_lock_and_refuse_what_they_cannot_hold() {
        let lock = super::Lock::private(MUTEX).expect("a mutex");
        assert_eq!(lock.lock(LOCK_EX, -1, CAPABILITIES), Ok(()));
        assert_eq!(lock.unlock(), Ok(()));
        assert_eq!(super::Kind::from_code(42), None, "no such kind");
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
    const RW_LOCK: super::Kind = super::Kind::RwLock;
    const MUTEX: super::Kind = super::Kind::Mutex;
    #[cfg(target_os = "linux")]
    const SPIN_LOCK: super::Kind = super::Kind::Spin;

    // A private lock, or a shared one in a page of its own, which survives fork as
    // sw_mem_pool's does and is never unmapped.
    struct Lock(super::Lock);
    impl Lock {
        fn new(kind: super::Kind, shared: bool) -> Self {
            let lock = if shared {
                let words = super::Lock::words(kind);
                assert!(words * size_of::<AtomicU64>() <= 4096);
                let page = page(true).cast::<AtomicU64>();
                super::Lock::shared(kind, unsafe { std::slice::from_raw_parts_mut(page, words) })
            } else {
                super::Lock::private(kind)
            };
            Self(lock.expect("a lock"))
        }
        fn lock(&self, operation: i32, timeout: i32) -> i32 {
            self.0
                .lock(operation, timeout, CAPABILITIES)
                .err()
                .unwrap_or(0)
        }
        fn unlock(&self) -> i32 {
            self.0.unlock().err().unwrap_or(0)
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
        // First verify the held mutex times out; then publish the unlock before
        // the success check. A sleep cannot order the unlock before a 10 ms deadline
        // on a loaded runner. Contended wakeups are covered by `test_func`.
        // Keep upstream's timeout values and elapsed-time assertions.
        let attempt = || {
            let lock = Lock::new(MUTEX, false);
            lock.lock(LOCK_EX, -1);
            let timed_out = Barrier::new(2);
            let unlocked = Barrier::new(2);
            thread::scope(|s| {
                let waiter = s.spawn(|| {
                    let ms1 = millis();
                    assert_eq!(lock.lock(LOCK_EX, TIMEOUT_1), ETIMEDOUT);
                    timed_out.wait();
                    unlocked.wait();
                    let ms2 = millis();
                    assert!(ms2 - ms1 >= TIMEOUT_1 as u128);
                    let result = lock.lock(LOCK_EX, TIMEOUT_2);
                    (result, ms2, millis())
                });
                timed_out.wait();
                let unlock_started = millis();
                lock.unlock();
                let unlock_finished = millis();
                unlocked.wait();
                let (result, started, finished) = waiter.join().unwrap();
                assert_eq!(
                    result, 0,
                    "second wait [{started}, {finished}], unlock [{unlock_started}, {unlock_finished}] (Unix ms)"
                );
                finished - started
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
