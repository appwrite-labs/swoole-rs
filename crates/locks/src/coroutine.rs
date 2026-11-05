//! Coroutine lock state and complete acquisition/release loops.
use std::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_corebase::constants::{SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND};
use swoole_rs_error::ErrorCode;

/// A native leaf outcome; PHP hosts contain bailouts before returning to Rust.
#[derive(Clone, Copy)]
#[repr(C)]
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
#[repr(C)]
pub struct CoroutineHost {
    pub current: unsafe extern "C" fn() -> *mut c_void,
    pub cid: unsafe extern "C" fn(*mut c_void) -> i64,
    pub alloc_shared: unsafe extern "C" fn() -> *mut u32,
    pub free_shared: unsafe extern "C" fn(*mut u32),
    pub sleep: Option<unsafe extern "C" fn(f64) -> CoroutineOutcome>,
    pub wait: Option<unsafe extern "C" fn(*mut u32) -> CoroutineOutcome>,
    pub wake: Option<unsafe extern "C" fn(*mut u32) -> CoroutineOutcome>,
    pub warning: unsafe extern "C" fn() -> u8,
}

/// The word is shared across processes when requested; ownership tokens stay process-local.
/// Only atomic word references span a suspension, never a Rust borrow of this record.
pub struct CoroutineLock {
    host: *const CoroutineHost,
    word: *mut AtomicU32,
    cookie: *mut c_void,
    cid: i64,
    shared: bool,
}

impl Drop for CoroutineLock {
    fn drop(&mut self) {
        if self.shared {
            // SAFETY: the process-lived host frees the shared word it allocated.
            let host = unsafe { &*self.host };
            // SAFETY: this record owns exactly one pool allocation.
            unsafe { (host.free_shared)(self.word.cast()) };
        } else {
            // SAFETY: the unshared word was allocated by Box in new.
            drop(unsafe { Box::from_raw(self.word) });
        }
    }
}

/// # Safety
/// The immutable host and its leaf effects outlive every lock and cannot unwind.
#[must_use]
pub unsafe fn coroutine_new(shared: bool, host: *const CoroutineHost) -> *mut CoroutineLock {
    // SAFETY: the caller supplies a process-lived immutable host.
    let effects = unsafe { &*host };
    if effects.wait.is_some() != effects.wake.is_some()
        || (effects.wait.is_none() && effects.sleep.is_none())
    {
        std::process::abort();
    }
    let word = if shared {
        // SAFETY: the host allocates aligned process-shared storage for one word.
        let word = unsafe { (effects.alloc_shared)() }.cast::<AtomicU32>();
        if word.is_null() {
            std::process::abort();
        }
        // SAFETY: initialize the newly owned allocation before publishing it.
        unsafe { word.write(AtomicU32::new(0)) };
        word
    } else {
        Box::into_raw(Box::new(AtomicU32::new(0)))
    };
    Box::into_raw(Box::new(CoroutineLock {
        host,
        word,
        cookie: ptr::null_mut(),
        cid: 0,
        shared,
    }))
}

/// # Safety
/// A uniquely owned lock, no operation is suspended on it; consumes it exactly once.
pub unsafe fn coroutine_free(lock: *mut CoroutineLock) {
    // SAFETY: the caller transfers the record and its word back to their allocators.
    drop(unsafe { Box::from_raw(lock) });
}

unsafe fn outside(host: &CoroutineHost) -> CoroutineOutcome {
    // SAFETY: the host contains any PHP warning bailout in its native leaf.
    let bailed = unsafe { (host.warning)() };
    CoroutineOutcome {
        code: ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32,
        bailed,
    }
}

fn last_errno() -> i32 {
    // SAFETY: the platform's errno address is live in this calling thread.
    unsafe { *super::errno() }
}

/// # Safety
/// The live lock and its atomic word remain allocated across any host suspension.
/// Its process-local metadata is accessed on one cooperative thread.
pub unsafe fn coroutine_acquire(lock: *mut CoroutineLock, blocking: bool) -> CoroutineOutcome {
    // SAFETY: the host is immutable and outlives the lock.
    let host_ptr = unsafe { (*lock).host };
    // SAFETY: the immutable host record is process-lived.
    let host = unsafe { &*host_ptr };
    // SAFETY: obtain the native current-coroutine identity once, before possible suspension.
    let current = unsafe { (host.current)() };
    if current.is_null() {
        // SAFETY: the host's warning is a contained leaf effect.
        return unsafe { outside(host) };
    }
    // SAFETY: a non-null native coroutine is live while its method executes.
    let cid = unsafe { (host.cid)(current) };
    // SAFETY: copied ownership tokens; no Rust record borrow spans a suspension.
    let held_cookie = unsafe { (*lock).cookie };
    // SAFETY: the record's initialized integer token is plain data.
    let held_cid = unsafe { (*lock).cid };
    if current == held_cookie && cid == held_cid {
        return CoroutineOutcome::code(0);
    }
    // SAFETY: the word outlives this operation; only atomic operations access it.
    let word_ptr = unsafe { (*lock).word };
    // SAFETY: the owned atomic word remains live across this operation.
    let word = unsafe { &*word_ptr };
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
            // SAFETY: the host borrows one live aligned atomic futex word.
            let result = unsafe { wait(ptr::from_ref(word).cast_mut().cast()) };
            if result.bailed != 0 {
                return result;
            }
            if result.code != 0 {
                return CoroutineOutcome::code(last_errno());
            }
        } else {
            // SAFETY: the host contains any PHP bailout before returning from its sleep.
            let Some(sleep) = host.sleep else {
                std::process::abort();
            };
            // SAFETY: the configured sleep leaf contains PHP bailouts.
            let result = unsafe { sleep(second) };
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
    // SAFETY: the acquired coroutine remains live after suspension; refresh its token
    // at publication, in the same order as the native implementation.
    let owner_cid = unsafe { (host.cid)(current) };
    // SAFETY: acquisition succeeded; publish the owner's process-local token.
    unsafe { (*lock).cid = owner_cid };
    // SAFETY: as above; the cookie is opaque and never dereferenced by Rust.
    unsafe { (*lock).cookie = current };
    CoroutineOutcome::code(0)
}

/// # Safety
/// The live lock remains allocated through the wake leaf; metadata belongs to one cooperative thread.
pub unsafe fn coroutine_unlock(lock: *mut CoroutineLock) -> CoroutineOutcome {
    // SAFETY: the host is immutable and process-lived.
    let host_ptr = unsafe { (*lock).host };
    // SAFETY: the immutable host record is process-lived.
    let host = unsafe { &*host_ptr };
    // SAFETY: query the current coroutine before accessing ownership, as upstream does.
    if unsafe { (host.current)() }.is_null() {
        // SAFETY: the host contains any PHP warning bailout.
        return unsafe { outside(host) };
    }
    // SAFETY: the word outlives the operation and is accessed only atomically.
    let word_ptr = unsafe { (*lock).word };
    // SAFETY: the owned atomic word remains live across this operation.
    let word = unsafe { &*word_ptr };
    if word.load(Ordering::Relaxed) == 0 {
        return CoroutineOutcome::code(0);
    }
    word.store(0, Ordering::Relaxed);
    // SAFETY: release clears ownership before waking a waiter.
    unsafe { (*lock).cid = 0 };
    // SAFETY: the process-local cookie is no longer held.
    unsafe { (*lock).cookie = ptr::null_mut() };
    let Some(wake) = host.wake else {
        return CoroutineOutcome::code(0);
    };
    // SAFETY: the host borrows the still-live atomic word; it contains PHP bailouts.
    let result = unsafe { wake(ptr::from_ref(word).cast_mut().cast()) };
    if result.bailed != 0 {
        result
    } else if result.code >= 0 {
        CoroutineOutcome::code(0)
    } else {
        CoroutineOutcome::code(last_errno())
    }
}

/// # Safety
/// Same constructor contract as `coroutine_new`; the C++ facade lends an immutable static host.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_coroutine_new(
    shared: u8,
    host: *const CoroutineHost,
) -> *mut CoroutineLock {
    // SAFETY: forwarding the caller's static host and requested allocation family.
    unsafe { coroutine_new(shared != 0, host) }
}

/// # Safety
/// Same unique ownership contract as `coroutine_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_coroutine_free(lock: *mut CoroutineLock) {
    // SAFETY: the caller transfers this live record exactly once.
    unsafe { coroutine_free(lock) };
}

/// # Safety
/// Same live-lock/suspension contract as `coroutine_acquire`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_coroutine_lock(
    lock: *mut CoroutineLock,
    operation: i32,
) -> CoroutineOutcome {
    // SAFETY: the caller keeps the lock live across its native leaf effects.
    unsafe { coroutine_acquire(lock, operation & libc::LOCK_NB == 0) }
}

/// # Safety
/// Same live-lock contract as `coroutine_unlock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_locks_coroutine_unlock(
    lock: *mut CoroutineLock,
) -> CoroutineOutcome {
    // SAFETY: the caller lends a live record through the wake effect.
    unsafe { coroutine_unlock(lock) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct State {
        current: usize,
        cid: i64,
        lock: *mut CoroutineLock,
        sleeps: Vec<f64>,
        result: i32,
        bailed: u8,
        releases: bool,
        warnings: usize,
        allocated: usize,
        freed: usize,
        wakes: usize,
    }
    thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
    unsafe extern "C" fn current() -> *mut c_void {
        STATE.with(|state| state.borrow().current as *mut c_void)
    }
    unsafe extern "C" fn cid(_: *mut c_void) -> i64 {
        STATE.with(|state| state.borrow().cid)
    }
    unsafe extern "C" fn allocate() -> *mut u32 {
        STATE.with(|state| state.borrow_mut().allocated += 1);
        Box::into_raw(Box::new(0))
    }
    unsafe extern "C" fn release(word: *mut u32) {
        STATE.with(|state| state.borrow_mut().freed += 1);
        drop(unsafe { Box::from_raw(word) });
    }
    unsafe extern "C" fn sleep(second: f64) -> CoroutineOutcome {
        let (lock, releases, result, bailed) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.sleeps.push(second);
            (state.lock, state.releases, state.result, state.bailed)
        });
        // Model the old owner resuming while the contender is suspended.
        if releases {
            unsafe { coroutine_unlock(lock) };
        }
        CoroutineOutcome {
            code: result,
            bailed,
        }
    }
    unsafe extern "C" fn wait(_: *mut u32) -> CoroutineOutcome {
        unsafe { sleep(0.0) }
    }
    unsafe extern "C" fn wake(_: *mut u32) -> CoroutineOutcome {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.wakes += 1;
            CoroutineOutcome {
                code: state.result,
                bailed: state.bailed,
            }
        })
    }
    unsafe extern "C" fn warning() -> u8 {
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
        free_shared: release,
        sleep: Some(sleep),
        wait: None,
        wake: None,
        warning,
    };
    static FUTEX: CoroutineHost = CoroutineHost {
        current,
        cid,
        alloc_shared: allocate,
        free_shared: release,
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
    fn contender(lock: *mut CoroutineLock) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.current = 2;
            state.cid = 8;
            state.lock = lock;
        });
    }
    #[test]
    fn ownership_reentry_and_recycled_coroutine_identity() {
        reset();
        unsafe {
            let lock = coroutine_new(false, &raw const SLEEP);
            assert_eq!(coroutine_acquire(lock, true).code, 0);
            assert_eq!(coroutine_acquire(lock, false).code, 0);
            STATE.with(|state| state.borrow_mut().cid += 1);
            assert_eq!(coroutine_acquire(lock, false).code, libc::EBUSY);
            assert_eq!(coroutine_unlock(lock).code, 0);
            assert_eq!(coroutine_acquire(lock, false).code, 0);
            assert_eq!(coroutine_unlock(lock).code, 0);
            assert_eq!(coroutine_unlock(lock).code, 0);
            coroutine_free(lock);
        }
    }
    #[test]
    fn shared_allocation_and_sleeping_contender_transfer_ownership() {
        reset();
        unsafe {
            let lock = coroutine_new(true, &raw const SLEEP);
            assert_eq!(coroutine_acquire(lock, true).code, 0);
            contender(lock);
            assert_eq!(coroutine_acquire(lock, false).code, libc::EBUSY);
            STATE.with(|state| state.borrow_mut().releases = true);
            assert_eq!(coroutine_acquire(lock, true).code, 0);
            assert_eq!(coroutine_acquire(lock, false).code, 0);
            coroutine_free(lock);
        }
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.sleeps, [SW_FILE_LOCK_DEFAULT_SECOND]);
            assert_eq!((state.allocated, state.freed), (1, 1));
        });
    }
    #[test]
    fn cancellation_and_bailout_do_not_publish_contender() {
        reset();
        unsafe {
            let lock = coroutine_new(false, &raw const SLEEP);
            assert_eq!(coroutine_acquire(lock, true).code, 0);
            contender(lock);
            STATE.with(|state| state.borrow_mut().result = -1);
            assert_eq!(
                coroutine_acquire(lock, true).code,
                ErrorCode::SW_ERROR_CO_CANCELED as i32
            );
            STATE.with(|state| state.borrow_mut().bailed = 1);
            assert_eq!(coroutine_acquire(lock, true).bailed, 1);
            assert_eq!((*lock).cookie as usize, 1);
            assert_eq!((*lock).cid, 7);
            coroutine_free(lock);
        }
    }
    #[test]
    fn futex_wait_and_wake_keep_errno_and_contained_bailout() {
        reset();
        unsafe {
            let lock = coroutine_new(false, &raw const FUTEX);
            assert_eq!(coroutine_acquire(lock, true).code, 0);
            contender(lock);
            *super::super::errno() = libc::EINTR;
            STATE.with(|state| state.borrow_mut().result = -1);
            assert_eq!(coroutine_acquire(lock, true).code, libc::EINTR);
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.result = 0;
                state.releases = true;
            });
            assert_eq!(coroutine_acquire(lock, true).code, 0);
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.result = -1;
                state.bailed = 1;
            });
            assert_eq!(coroutine_unlock(lock).bailed, 1);
            assert_eq!((*lock).cookie, ptr::null_mut());
            assert_eq!((*lock).cid, 0);
            coroutine_free(lock);
        }
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
        unsafe {
            let lock = coroutine_new(false, &raw const SLEEP);
            for result in [coroutine_acquire(lock, true), coroutine_unlock(lock)] {
                assert_eq!(result.code, ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as i32);
                assert_eq!(result.bailed, 1);
            }
            coroutine_free(lock);
        }
        STATE.with(|state| assert_eq!(state.borrow().warnings, 2));
    }
}
