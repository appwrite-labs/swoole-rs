//! Coroutine lock state and complete acquisition/release loops.
use std::{
    cell::Cell,
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_corebase::constants::{SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND};
use swoole_rs_error::ErrorCode;

/// Immutable, process-lived effects supplied once when the lock is constructed.
/// No native lock state or per-call operation table is retained.
pub struct CoroutineHost {
    /// The running coroutine's id, `None` outside one.
    pub current: fn() -> Option<i64>,
    /// A process-shared word, for the program's life, or None.
    pub alloc_shared: fn() -> Option<&'static AtomicU32>,
    /// `System::sleep`: false when cancelled.
    pub sleep: Option<fn(f64) -> bool>,
    /// The futex wait: 0, or nonzero with errno set.
    pub wait: Option<fn(&AtomicU32) -> i32>,
    /// The futex wake: negative with errno set on failure.
    pub wake: Option<fn(&AtomicU32) -> i32>,
    /// Warns that the lock was used outside a coroutine.
    pub warning: fn(),
}

/// The word is shared across processes when requested; ownership tokens stay process-local.
/// Operations take `&self`, so a suspended acquisition and another coroutine's
/// unlock can both hold the lock.
pub struct CoroutineLock {
    host: &'static CoroutineHost,
    word: Word,
    /// The owner's id, 0 for none (ids start at 1).
    cid: Cell<i64>,
}

/// A shared lock's word, which the host's storage keeps, or a private one.
enum Word {
    Shared(&'static AtomicU32),
    Private(Box<AtomicU32>),
}

fn outside(host: &CoroutineHost) -> Result<(), i32> {
    (host.warning)();
    Err(ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32)
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl CoroutineLock {
    /// A new unlocked lock; aborts when the host is inconsistent or has no storage.
    #[must_use]
    pub fn new(shared: bool, host: &'static CoroutineHost) -> Box<Self> {
        if host.wait.is_some() != host.wake.is_some()
            || (host.wait.is_none() && host.sleep.is_none())
        {
            swoole_rs_corebase::fatal(
                "a coroutine lock host needs wait and wake together, or sleep",
            );
        }
        let word = if shared {
            let Some(word) = (host.alloc_shared)() else {
                swoole_rs_corebase::fatal("no shared memory for a coroutine lock");
            };
            word.store(0, Ordering::Relaxed);
            Word::Shared(word)
        } else {
            Word::Private(Box::new(AtomicU32::new(0)))
        };
        Box::new(Self {
            host,
            word,
            cid: Cell::new(0),
        })
    }

    fn word(&self) -> &AtomicU32 {
        match &self.word {
            Word::Shared(word) => word,
            Word::Private(word) => word,
        }
    }

    /// `CoroutineLock::lock_impl`: blocks (sleeping or on the futex) unless `blocking` is false.
    ///
    /// # Errors
    /// The error code: out of coroutine, `EBUSY`, the wait's errno, or cancelled.
    pub fn acquire(&self, blocking: bool) -> Result<(), i32> {
        let host = self.host;
        // The current-coroutine identity, once, before possible suspension.
        let Some(cid) = (host.current)() else {
            return outside(host);
        };
        if cid == self.cid.get() {
            return Ok(());
        }
        let word = self.word();
        let mut second = SW_FILE_LOCK_DEFAULT_SECOND;
        loop {
            if word
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
            if !blocking {
                return Err(libc::EBUSY);
            }
            if let Some(wait) = host.wait {
                if wait(word) != 0 {
                    return Err(last_errno());
                }
            } else {
                let Some(sleep) = host.sleep else {
                    swoole_rs_corebase::fatal("a coroutine lock host has neither wait nor sleep");
                };
                if !sleep(second) {
                    return Err(ErrorCode::SW_ERROR_CO_CANCELED as i32);
                }
                second = super::swr_v1_locks_coroutine_backoff(
                    second,
                    SW_FILE_LOCK_DEFAULT_SECOND,
                    SW_FILE_LOCK_MAX_SECOND,
                );
            }
        }
        self.cid.set(cid);
        Ok(())
    }

    /// `CoroutineLock::unlock`.
    ///
    /// # Errors
    /// The error code: out of coroutine, or the wake's errno.
    pub fn unlock(&self) -> Result<(), i32> {
        let host = self.host;
        // Query the current coroutine before accessing ownership, as upstream does.
        if (host.current)().is_none() {
            return outside(host);
        }
        let word = self.word();
        if word.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        // Release: the next holder, perhaps another process, sees what this one wrote.
        word.store(0, Ordering::Release);
        // Release clears ownership before waking a waiter.
        self.cid.set(0);
        match host.wake {
            Some(wake) if wake(word) < 0 => Err(last_errno()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct State {
        current: usize,
        cid: i64,
        lock: *const CoroutineLock,
        sleeps: Vec<f64>,
        result: i32,
        releases: bool,
        warnings: usize,
        allocated: usize,
        wakes: usize,
    }
    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State::default());
        // What the other coroutines do while the sleeping one is suspended.
        static ON_SLEEP: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }
    fn current() -> Option<i64> {
        STATE.with(|state| {
            let state = state.borrow();
            (state.current != 0).then_some(state.cid)
        })
    }
    /// A fresh word for the program's life, as the pool's are.
    fn allocate() -> Option<&'static AtomicU32> {
        static WORDS: [AtomicU32; 16] = [const { AtomicU32::new(7) }; 16];
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        STATE.with(|state| state.borrow_mut().allocated += 1);
        WORDS.get(NEXT.fetch_add(1, Ordering::Relaxed))
    }
    fn sleep(second: f64) -> bool {
        pause(second) == 0
    }
    /// A sleep, or a futex wait: the state's result.
    fn pause(second: f64) -> i32 {
        let (lock, releases, result) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.sleeps.push(second);
            (state.lock, state.releases, state.result)
        });
        // Model the old owner resuming while the contender is suspended.
        if releases {
            _ = unsafe { &*lock }.unlock();
        }
        if let Some(others) = ON_SLEEP.with(|f| f.borrow_mut().take()) {
            let (current, cid) = STATE.with(|state| (state.borrow().current, state.borrow().cid));
            others();
            switch(current, cid);
        }
        result
    }
    fn wait(_: &AtomicU32) -> i32 {
        pause(0.0)
    }
    fn wake(_: &AtomicU32) -> i32 {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.wakes += 1;
            state.result
        })
    }
    fn warning() {
        STATE.with(|state| state.borrow_mut().warnings += 1);
    }
    static SLEEP: CoroutineHost = CoroutineHost {
        current,
        alloc_shared: allocate,
        sleep: Some(sleep),
        wait: None,
        wake: None,
        warning,
    };
    static FUTEX: CoroutineHost = CoroutineHost {
        current,
        alloc_shared: allocate,
        sleep: None,
        wait: Some(wait),
        wake: Some(wake),
        warning,
    };
    fn reset() {
        STATE.with(|state| {
            *state.borrow_mut() = State {
                current: 1,
                cid: 7,
                ..State::default()
            }
        });
    }
    fn contender(lock: &CoroutineLock) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.current = 2;
            state.cid = 8;
            state.lock = lock;
        });
    }
    fn switch(current: usize, cid: i64) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.current = current;
            state.cid = cid;
        });
    }
    fn lock(lock: &CoroutineLock, operation: i32) -> i32 {
        lock.acquire(operation & libc::LOCK_NB == 0)
            .err()
            .unwrap_or(0)
    }
    fn unlock(lock: &CoroutineLock) -> i32 {
        lock.unlock().err().unwrap_or(0)
    }

    // core-tests/src/lock/lock.cpp's coroutine TESTs (the C++ CoroutineLock left the build),
    // with the same calls and expected values. Coroutine `n` is current `n`, cid `n`. Three
    // coroutines start in order; a contended lock's sleep runs the others until it can go on.
    const OUT_OF_COROUTINE: i32 = ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32;

    fn three_coroutines(operation: i32) {
        reset();
        switch(0, 0);
        let owner = Box::into_raw(CoroutineLock::new(false, &SLEEP));
        // A shared borrow can't free the lock, so `owner` keeps the box's own pointer.
        let lock: &'static CoroutineLock = unsafe { &*owner };
        assert_eq!(self::lock(lock, operation), OUT_OF_COROUTINE);
        if operation == libc::LOCK_EX {
            assert_eq!(unlock(lock), OUT_OF_COROUTINE);
        }
        // Coroutine 1 takes the lock twice and sleeps.
        switch(1, 1);
        assert_eq!(self::lock(lock, operation), 0);
        assert_eq!(self::lock(lock, operation), 0);
        // Coroutine 2 waits for it: coroutine 3 fails to take it without blocking, then
        // coroutine 1 wakes and unlocks.
        ON_SLEEP.with(|f| {
            *f.borrow_mut() = Some(Box::new(move || {
                switch(3, 3);
                assert_eq!(self::lock(lock, operation | libc::LOCK_NB), libc::EBUSY);
                switch(1, 1);
                assert_eq!(unlock(lock), 0);
            }));
        });
        switch(2, 2);
        assert_eq!(self::lock(lock, operation), 0);
        assert!(ON_SLEEP.with(|f| f.borrow().is_none()));
        // Coroutine 2 sleeps, then unlocks twice; the second has no effect.
        assert_eq!(unlock(lock), 0);
        if operation == libc::LOCK_EX {
            assert_eq!(unlock(lock), 0);
        }
        drop(unsafe { Box::from_raw(owner) });
    }

    #[test]
    fn lock_coroutine_lock() {
        three_coroutines(libc::LOCK_EX);
    }

    #[test]
    fn lock_coroutine_lock_rd() {
        three_coroutines(libc::LOCK_SH);
    }

    #[test]
    fn lock_coroutine_lock_cancel() {
        reset();
        let lock = CoroutineLock::new(true, &SLEEP);
        let lock = &*lock;
        assert_eq!(self::lock(lock, libc::LOCK_EX), 0);
        // The child's wait is cancelled by a timer: the host's sleep fails.
        contender(lock);
        STATE.with(|state| state.borrow_mut().result = -1);
        assert_eq!(
            self::lock(lock, libc::LOCK_EX),
            ErrorCode::SW_ERROR_CO_CANCELED as i32
        );
    }

    #[test]
    fn ownership_reentry_and_recycled_coroutine_identity() {
        reset();
        let lock = CoroutineLock::new(false, &SLEEP);
        assert_eq!(lock.acquire(true), Ok(()));
        assert_eq!(lock.acquire(false), Ok(()));
        STATE.with(|state| state.borrow_mut().cid += 1);
        assert_eq!(lock.acquire(false), Err(libc::EBUSY));
        assert_eq!(lock.unlock(), Ok(()));
        assert_eq!(lock.acquire(false), Ok(()));
        assert_eq!(lock.unlock(), Ok(()));
        assert_eq!(lock.unlock(), Ok(()));
        drop(lock);
    }
    #[test]
    fn shared_allocation_and_sleeping_contender_transfer_ownership() {
        reset();
        let lock = CoroutineLock::new(true, &SLEEP);
        assert_eq!(lock.acquire(true), Ok(()));
        contender(&lock);
        assert_eq!(lock.acquire(false), Err(libc::EBUSY));
        STATE.with(|state| state.borrow_mut().releases = true);
        assert_eq!(lock.acquire(true), Ok(()));
        assert_eq!(lock.acquire(false), Ok(()));
        drop(lock);
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.sleeps, [SW_FILE_LOCK_DEFAULT_SECOND]);
            assert_eq!(state.allocated, 1);
        });
    }
    #[test]
    fn cancellation_and_bailout_do_not_publish_contender() {
        reset();
        let lock = CoroutineLock::new(false, &SLEEP);
        assert_eq!(lock.acquire(true), Ok(()));
        contender(&lock);
        STATE.with(|state| state.borrow_mut().result = -1);
        assert_eq!(
            lock.acquire(true),
            Err(ErrorCode::SW_ERROR_CO_CANCELED as i32)
        );
        assert_eq!(lock.cid.get(), 7);
        drop(lock);
    }
    #[test]
    fn futex_wait_and_wake_keep_errno_and_contained_bailout() {
        reset();
        let lock = CoroutineLock::new(false, &FUTEX);
        assert_eq!(lock.acquire(true), Ok(()));
        contender(&lock);
        swoole_rs_corebase::sys::set_errno(libc::EINTR);
        STATE.with(|state| state.borrow_mut().result = -1);
        assert_eq!(lock.acquire(true), Err(libc::EINTR));
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.result = 0;
            state.releases = true;
        });
        assert_eq!(lock.acquire(true), Ok(()));
        STATE.with(|state| state.borrow_mut().result = -1);
        swoole_rs_corebase::sys::set_errno(libc::EPERM);
        assert_eq!(lock.unlock(), Err(libc::EPERM));
        assert_eq!(lock.cid.get(), 0);
        drop(lock);
        STATE.with(|state| assert_eq!(state.borrow().wakes, 2));
    }
    #[test]
    fn outside_coroutine_warning_is_contained_before_state_access() {
        reset();
        STATE.with(|state| state.borrow_mut().current = 0);
        let lock = CoroutineLock::new(false, &SLEEP);
        for result in [lock.acquire(true), lock.unlock()] {
            assert_eq!(result, Err(ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32));
        }
        drop(lock);
        STATE.with(|state| assert_eq!(state.borrow().warnings, 2));
    }

    /// A shared lock's holders are other processes (threads here, which Miri checks): an
    /// unlock must publish what its holder wrote to the next holder. Nothing else orders the
    /// two: the `HELD` flag only says when to start contending.
    #[test]
    fn an_unlock_publishes_the_holders_writes_to_the_next_holder() {
        struct Data(std::cell::UnsafeCell<u64>);
        unsafe impl Sync for Data {}
        static DATA: Data = Data(std::cell::UnsafeCell::new(0));
        static WORD: AtomicU32 = AtomicU32::new(0);
        static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        #[expect(clippy::unnecessary_wraps, reason = "CoroutineHost::current's type")]
        fn current() -> Option<i64> {
            Some(1)
        }
        #[expect(
            clippy::unnecessary_wraps,
            reason = "CoroutineHost::alloc_shared's type"
        )]
        fn shared() -> Option<&'static AtomicU32> {
            Some(&WORD)
        }
        fn sleep(_: f64) -> bool {
            std::thread::yield_now();
            true
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
        // Each process's own lock over the shared word, made before either holds it.
        struct Process(Box<CoroutineLock>);
        unsafe impl Send for Process {}
        let first = Process(CoroutineLock::new(true, &HOST));
        let second = Process(CoroutineLock::new(true, &HOST));
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let first = first;
                let first = &first.0;
                assert_eq!(first.acquire(true), Ok(()));
                unsafe { *DATA.0.get() = 42 };
                HELD.store(true, Ordering::Relaxed);
                assert_eq!(first.unlock(), Ok(()));
            });
            scope.spawn(move || {
                let second = second;
                let second = &second.0;
                while !HELD.load(Ordering::Relaxed) {
                    std::thread::yield_now();
                }
                assert_eq!(second.acquire(true), Ok(()));
                assert_eq!(unsafe { *DATA.0.get() }, 42);
                assert_eq!(second.unlock(), Ok(()));
            });
        });
    }
}
