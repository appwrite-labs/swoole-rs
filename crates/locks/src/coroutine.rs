//! Coroutine lock state and complete acquisition/release loops.
use std::{
    cell::Cell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_corebase::constants::{SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND};
use swoole_rs_error::ErrorCode;

/// A native leaf outcome; PHP hosts contain bailouts before returning to Rust.
#[derive(Clone, Copy)]
pub struct CoroutineOutcome {
    pub code: i32,
    pub bailed: u8,
}

impl CoroutineOutcome {
    const fn code(code: i32) -> Self {
        Self { code, bailed: 0 }
    }
}

/// Immutable, process-lived effects supplied once when the lock is constructed.
/// No native lock state or per-call operation table is retained.
pub struct CoroutineHost {
    pub current: fn() -> *mut c_void,
    pub cid: fn(*mut c_void) -> i64,
    /// A process-shared word, for the program's life, or None.
    pub alloc_shared: fn() -> Option<&'static AtomicU32>,
    pub sleep: Option<fn(f64) -> CoroutineOutcome>,
    pub wait: Option<fn(&AtomicU32) -> CoroutineOutcome>,
    pub wake: Option<fn(&AtomicU32) -> CoroutineOutcome>,
    pub warning: fn() -> u8,
}

/// The word is shared across processes when requested; ownership tokens stay process-local.
/// Operations take `&self`, so a suspended acquisition and another coroutine's
/// unlock can both hold the lock.
pub struct CoroutineLock {
    host: &'static CoroutineHost,
    word: Word,
    cookie: Cell<*mut c_void>,
    cid: Cell<i64>,
}

/// A shared lock's word, which the host's storage keeps, or a private one.
enum Word {
    Shared(&'static AtomicU32),
    Private(Box<AtomicU32>),
}

fn outside(host: &CoroutineHost) -> CoroutineOutcome {
    CoroutineOutcome {
        code: ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32,
        bailed: (host.warning)(),
    }
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
            std::process::abort();
        }
        let word = if shared {
            let Some(word) = (host.alloc_shared)() else {
                std::process::abort();
            };
            word.store(0, Ordering::Relaxed);
            Word::Shared(word)
        } else {
            Word::Private(Box::new(AtomicU32::new(0)))
        };
        Box::new(Self {
            host,
            word,
            cookie: Cell::new(ptr::null_mut()),
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
    pub fn acquire(&self, blocking: bool) -> CoroutineOutcome {
        let host = self.host;
        // The current-coroutine identity, once, before possible suspension.
        let current = (host.current)();
        if current.is_null() {
            return outside(host);
        }
        let cid = (host.cid)(current);
        if current == self.cookie.get() && cid == self.cid.get() {
            return CoroutineOutcome::code(0);
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
                return CoroutineOutcome::code(libc::EBUSY);
            }
            if let Some(wait) = host.wait {
                let result = wait(word);
                if result.bailed != 0 {
                    return result;
                }
                if result.code != 0 {
                    return CoroutineOutcome::code(last_errno());
                }
            } else {
                let Some(sleep) = host.sleep else {
                    std::process::abort();
                };
                let result = sleep(second);
                if result.bailed != 0 {
                    return result;
                }
                if result.code != 0 {
                    return CoroutineOutcome::code(ErrorCode::SW_ERROR_CO_CANCELED as i32);
                }
                second = super::swr_v1_locks_coroutine_backoff(
                    second,
                    SW_FILE_LOCK_DEFAULT_SECOND,
                    SW_FILE_LOCK_MAX_SECOND,
                );
            }
        }
        // The acquired coroutine is live after suspension; refresh its token at
        // publication, in the same order as the native implementation.
        self.cid.set((host.cid)(current));
        self.cookie.set(current);
        CoroutineOutcome::code(0)
    }

    /// `CoroutineLock::unlock`.
    pub fn unlock(&self) -> CoroutineOutcome {
        let host = self.host;
        // Query the current coroutine before accessing ownership, as upstream does.
        if (host.current)().is_null() {
            return outside(host);
        }
        let word = self.word();
        if word.load(Ordering::Relaxed) == 0 {
            return CoroutineOutcome::code(0);
        }
        word.store(0, Ordering::Relaxed);
        // Release clears ownership before waking a waiter.
        self.cid.set(0);
        self.cookie.set(ptr::null_mut());
        let Some(wake) = host.wake else {
            return CoroutineOutcome::code(0);
        };
        let result = wake(word);
        if result.bailed != 0 {
            result
        } else if result.code >= 0 {
            CoroutineOutcome::code(0)
        } else {
            CoroutineOutcome::code(last_errno())
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
        bailed: u8,
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
    fn current() -> *mut c_void {
        STATE.with(|state| state.borrow().current as *mut c_void)
    }
    fn cid(_: *mut c_void) -> i64 {
        STATE.with(|state| state.borrow().cid)
    }
    /// A fresh word for the program's life, as the pool's are.
    fn allocate() -> Option<&'static AtomicU32> {
        static WORDS: [AtomicU32; 16] = [const { AtomicU32::new(7) }; 16];
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        STATE.with(|state| state.borrow_mut().allocated += 1);
        WORDS.get(NEXT.fetch_add(1, Ordering::Relaxed))
    }
    fn sleep(second: f64) -> CoroutineOutcome {
        let (lock, releases, result, bailed) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.sleeps.push(second);
            (state.lock, state.releases, state.result, state.bailed)
        });
        // Model the old owner resuming while the contender is suspended.
        if releases {
            unsafe { &*lock }.unlock();
        }
        if let Some(others) = ON_SLEEP.with(|f| f.borrow_mut().take()) {
            let (current, cid) = STATE.with(|state| (state.borrow().current, state.borrow().cid));
            others();
            switch(current, cid);
        }
        CoroutineOutcome {
            code: result,
            bailed,
        }
    }
    fn wait(_: &AtomicU32) -> CoroutineOutcome {
        sleep(0.0)
    }
    fn wake(_: &AtomicU32) -> CoroutineOutcome {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.wakes += 1;
            CoroutineOutcome {
                code: state.result,
                bailed: state.bailed,
            }
        })
    }
    fn warning() -> u8 {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.warnings += 1;
            state.bailed
        })
    }
    static SLEEP: CoroutineHost = CoroutineHost {
        current,
        cid,
        alloc_shared: allocate,
        sleep: Some(sleep),
        wait: None,
        wake: None,
        warning,
    };
    static FUTEX: CoroutineHost = CoroutineHost {
        current,
        cid,
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
        lock.acquire(operation & libc::LOCK_NB == 0).code
    }
    fn unlock(lock: &CoroutineLock) -> i32 {
        lock.unlock().code
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
        assert_eq!(lock.acquire(true).code, 0);
        assert_eq!(lock.acquire(false).code, 0);
        STATE.with(|state| state.borrow_mut().cid += 1);
        assert_eq!(lock.acquire(false).code, libc::EBUSY);
        assert_eq!(lock.unlock().code, 0);
        assert_eq!(lock.acquire(false).code, 0);
        assert_eq!(lock.unlock().code, 0);
        assert_eq!(lock.unlock().code, 0);
        drop(lock);
    }
    #[test]
    fn shared_allocation_and_sleeping_contender_transfer_ownership() {
        reset();
        let lock = CoroutineLock::new(true, &SLEEP);
        assert_eq!(lock.acquire(true).code, 0);
        contender(&lock);
        assert_eq!(lock.acquire(false).code, libc::EBUSY);
        STATE.with(|state| state.borrow_mut().releases = true);
        assert_eq!(lock.acquire(true).code, 0);
        assert_eq!(lock.acquire(false).code, 0);
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
        assert_eq!(lock.acquire(true).code, 0);
        contender(&lock);
        STATE.with(|state| state.borrow_mut().result = -1);
        assert_eq!(
            lock.acquire(true).code,
            ErrorCode::SW_ERROR_CO_CANCELED as i32
        );
        STATE.with(|state| state.borrow_mut().bailed = 1);
        assert_eq!(lock.acquire(true).bailed, 1);
        assert_eq!(lock.cookie.get() as usize, 1);
        assert_eq!(lock.cid.get(), 7);
        drop(lock);
    }
    #[test]
    fn futex_wait_and_wake_keep_errno_and_contained_bailout() {
        reset();
        let lock = CoroutineLock::new(false, &FUTEX);
        assert_eq!(lock.acquire(true).code, 0);
        contender(&lock);
        unsafe { *super::super::errno() = libc::EINTR };
        STATE.with(|state| state.borrow_mut().result = -1);
        assert_eq!(lock.acquire(true).code, libc::EINTR);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.result = 0;
            state.releases = true;
        });
        assert_eq!(lock.acquire(true).code, 0);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.result = -1;
            state.bailed = 1;
        });
        assert_eq!(lock.unlock().bailed, 1);
        assert_eq!(lock.cookie.get(), ptr::null_mut());
        assert_eq!(lock.cid.get(), 0);
        drop(lock);
        STATE.with(|state| assert_eq!(state.borrow().wakes, 2));
    }
    #[test]
    fn outside_coroutine_warning_is_contained_before_state_access() {
        reset();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.current = 0;
            state.bailed = 1;
        });
        let lock = CoroutineLock::new(false, &SLEEP);
        for result in [lock.acquire(true), lock.unlock()] {
            assert_eq!(result.code, ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32);
            assert_eq!(result.bailed, 1);
        }
        drop(lock);
        STATE.with(|state| assert_eq!(state.borrow().warnings, 2));
    }
}
