//! Zend bailouts (`zend_bailout`, a `longjmp` to the nearest `zend_try`) around
//! code Rust calls. A bailout must never jump over a Rust frame, so Rust traps
//! it in C, returns normally, and hands a [`Bailout`] back to the C or C++ that
//! called it, which re-raises it with `zend_bailout()`. See docs/zend.md.
use crate::{
    Invalid,
    sys::{
        swr_zend_guard_pending, swr_zend_guard_resume, swr_zend_parse_func, swr_zend_try,
        swr_zend_try_call, swr_zend_try_call_method, swr_zend_try_report, swr_zend_try_report_code,
        swr_zend_try_report_flag, zend_class_entry, zend_fcall_info, zend_fcall_info_cache,
        zend_object, zval,
    },
};
use std::ffi::{CStr, c_char, c_int, c_void};

/// A trapped bailout. Its owner finishes its Rust work, then reports it to
/// the C/C++ caller (for example as a flag), which calls `zend_bailout()`.
#[must_use = "a trapped bailout must be re-raised by the C/C++ caller"]
#[derive(Debug)]
pub struct Bailout;

// Per thread on a thread-safe PHP, as the shim's `ZEND_TLS` guard state; one
// process flag otherwise, with no TLS lookup on each check. Tests (which
// unwind) keep it per thread, as they run in parallel.
#[cfg(any(zts, panic = "unwind"))]
thread_local! {
    static PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(any(zts, panic = "unwind")))]
static PENDING: Flag = Flag(std::sync::atomic::AtomicBool::new(false));

/// The process's pending flag, with `Cell`'s interface.
#[cfg(not(any(zts, panic = "unwind")))]
struct Flag(std::sync::atomic::AtomicBool);

#[cfg(not(any(zts, panic = "unwind")))]
impl Flag {
    fn get(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn set(&self, value: bool) {
        self.0.store(value, std::sync::atomic::Ordering::Relaxed);
    }
    /// Only the PHP thread writes it, so no atomic swap is needed.
    fn replace(&self, value: bool) -> bool {
        let previous = self.get();
        self.set(value);
        previous
    }
}

/// The native destination of a contained escape. Whole-thread provenance remains
/// pending until the request boundary acknowledges it, including across PHP catches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BailoutKind {
    Zend,
    ThreadExit,
}

impl Bailout {
    /// Native provenance of this contained escape; no PHP operation is run.
    #[must_use]
    pub fn kind(&self) -> BailoutKind {
        if native_thread_exit() {
            BailoutKind::ThreadExit
        } else {
            BailoutKind::Zend
        }
    }

    /// Leave the bailout for [`finish`], the last step of the handler or export
    /// that is the outermost Rust frame.
    pub fn defer(self) {
        PENDING.set(true);
    }
}

/// [`Bailout::defer`] of a failed result.
pub fn defer(result: Result<(), Bailout>) {
    if let Err(bailout) = result {
        bailout.defer();
    }
}

/// Whole-thread provenance remains native and survives ordinary PHP catches.
/// Only `Swoole\Thread` marks one, and it needs ZTS: never on NTS.
#[inline]
fn native_thread_exit() -> bool {
    // SAFETY: a pure read of this OS thread's native pending outcome.
    cfg!(zts) && unsafe { swr_zend_guard_pending() != 0 }
}

/// Whether a bailout waits for [`finish`]. No error is raised meanwhile:
/// upstream never ran past the first bailout.
#[must_use]
pub fn pending() -> bool {
    PENDING.get() || native_thread_exit()
}

/// `value`, or a [`Bailout`] while one is pending, which stays pending.
/// # Errors
/// A bailout is pending.
pub fn outcome<T>(value: T) -> Result<T, Bailout> {
    if pending() { Err(Bailout) } else { Ok(value) }
}

/// Hand a contained bailout to an outer C trampoline after Rust cleanup.
/// The caller must re-raise a true result in C; consuming the pending bit here
/// prevents a caught shutdown bailout from poisoning a later request operation.
#[must_use = "the C caller must re-raise a returned bailout"]
pub fn relay_to_c(result: Result<(), Bailout>) -> bool {
    defer(result);
    PENDING.replace(false) || native_thread_exit()
}

/// Raise a deferred bailout (`zend_bailout()`). Called last by every handler
/// and every export C or C++ calls, from a frame that owns nothing that needs
/// a drop by then: the `longjmp` crosses only that frame.
#[inline]
pub fn finish() {
    if PENDING.replace(false) || native_thread_exit() {
        // SAFETY: Zend's own bailout, from the outermost Rust frame (above).
        unsafe { swr_zend_guard_resume() };
    }
}

/// A free or destructor handler's `body`, then [`finish`] of what it deferred.
/// A bailout already pending is its deferrer's to raise: Zend frees objects
/// while one waits (a whole-thread exit stays pending until its request's
/// boundary), some outside any `zend_try`, where raising it aborts.
pub fn finish_after(body: impl FnOnce()) {
    let pending = pending();
    body();
    if !pending {
        finish();
    }
}

/// [`defer`] then [`finish`]: a handler's last step.
pub fn raise(result: Result<(), Bailout>) {
    defer(result);
    finish();
}

/// A native trap's flag as a result, without resuming a PHP bailout.
/// # Errors
/// Returns `Bailout` when the native leaf reports a nonzero contained-bailout flag.
pub fn trapped(bailed: c_int) -> Result<(), Bailout> {
    if bailed != 0 || native_thread_exit() {
        Err(Bailout)
    } else {
        Ok(())
    }
}

/// `report(message)` inside `zend_try`, for a C or C++ reporter of a PHP
/// error (Swoole's `php_swoole_error` family lives in C++). Nothing is
/// reported while a bailout is pending.
/// # Errors
/// A user error handler bailed out.
/// # Safety
/// `report` is C or C++ and may be called from here.
pub unsafe fn report(
    report: unsafe extern "C" fn(*const c_char),
    message: &CStr,
) -> Result<(), Bailout> {
    if pending() {
        return Err(Bailout);
    }
    // SAFETY: per the caller; the shim's setjmp sits between this frame and `report`.
    trapped(unsafe { swr_zend_try_report(Some(report), message.as_ptr()) })
}

/// [`report`] of a reporter that takes an error code and an int flag.
/// # Errors
/// A user error handler bailed out.
/// # Safety
/// As [`report`].
pub unsafe fn report_code(
    report: unsafe extern "C" fn(c_int, c_int, *const c_char),
    code: c_int,
    flag: c_int,
    message: &CStr,
) -> Result<(), Bailout> {
    if pending() {
        return Err(Bailout);
    }
    // SAFETY: as in `report`.
    trapped(unsafe { swr_zend_try_report_code(Some(report), code, flag, message.as_ptr()) })
}

/// [`report`] of a reporter that takes the level and a flag.
/// # Errors
/// A user error handler bailed out.
/// # Safety
/// As [`report`].
pub unsafe fn report_flag(
    report: unsafe extern "C" fn(c_int, bool, *const c_char),
    level: c_int,
    flag: bool,
    message: &CStr,
) -> Result<(), Bailout> {
    if pending() {
        return Err(Bailout);
    }
    // SAFETY: as in `report`.
    trapped(unsafe { swr_zend_try_report_flag(Some(report), level, flag, message.as_ptr()) })
}

/// Run `body(arg)` inside `zend_try`.
/// # Errors
/// `body` bailed out; Zend's state is as `zend_catch` leaves it.
/// # Safety
/// `body` may be called with `arg`. A bailout inside it jumps over its frame
/// and those it calls, so a Rust `body` (and any Rust it calls before the
/// bailout) holds nothing that needs dropping.
pub unsafe fn trap(
    body: unsafe extern "C" fn(*mut c_void),
    arg: *mut c_void,
) -> Result<(), Bailout> {
    // SAFETY: per the caller; the shim's setjmp sits in C, between this frame
    // and `body`.
    trapped(unsafe { swr_zend_try(Some(body), arg) })
}

/// Run `body()` inside `zend_try`: [`trap`] of a Rust closure, without a
/// `*mut c_void` argument record and a native body to unpack it.
/// # Errors
/// `body` bailed out; Zend's state is as `zend_catch` leaves it.
/// # Safety
/// As [`trap`]: a bailout jumps over `body`'s frame and those it calls, so
/// neither `body`, its captures, nor any Rust it calls before the bailout holds
/// anything that needs dropping.
pub unsafe fn trap_with<F: FnOnce()>(body: F) -> Result<(), Bailout> {
    /// Takes and runs the closure: a bailout leaves the slot empty, so nothing
    /// is dropped twice.
    unsafe extern "C" fn run<F: FnOnce()>(slot: *mut c_void) {
        // SAFETY: `trap_with`'s live slot, which it lends for this call only.
        let body = unsafe { &mut *slot.cast::<Option<F>>() }.take();
        if let Some(body) = body {
            body();
        }
    }
    let mut slot = Some(body);
    // SAFETY: per the caller; `run::<F>` takes the slot of an `F`.
    unsafe { trap(run::<F>, (&raw mut slot).cast()) }
}

/// `zend_call_function(fci, fcc)` inside `zend_try`: whether the call
/// succeeded. Nothing is called, and the result slot is left as it was, while
/// a bailout is pending: upstream never ran past it.
/// # Errors
/// The callable bailed out (`exit()`, a fatal error), or a bailout is pending.
/// # Safety
/// `fci` and `fcc` describe a live callable, as `zend_call_function` requires.
pub unsafe fn call(
    fci: *mut zend_fcall_info,
    fcc: *mut zend_fcall_info_cache,
) -> Result<bool, Bailout> {
    if pending() {
        return Err(Bailout);
    }
    let mut result = 0;
    // SAFETY: per the caller; Zend runs in the C shim's frame.
    if unsafe { swr_zend_try_call(fci, fcc, &raw mut result) } != 0 {
        return Err(Bailout);
    }
    Ok(result != 0)
}

/// `zend_call_method(object, scope, NULL, method, ...)` with `args` (at most
/// two) inside `zend_try`. A null `scope` is the object's class; otherwise the
/// method is `scope`'s, called on `object`. A null `retval` discards the result.
/// Nothing is called while a bailout is pending, as [`call`].
/// # Errors
/// The method bailed out, or a bailout is pending.
/// # Safety
/// `object` is a live object, `scope` null or its class or an ancestor,
/// `retval` null or writable and `args` live zvals.
pub unsafe fn call_method(
    object: *mut zend_object,
    scope: *mut zend_class_entry,
    method: &[u8],
    retval: *mut zval,
    args: &[*mut zval],
) -> Result<(), Bailout> {
    debug_assert!(
        args.len() <= 2,
        "zend_call_method takes at most two arguments"
    );
    if pending() {
        return Err(Bailout);
    }
    let arg = |n: usize| args.get(n).copied().unwrap_or(core::ptr::null_mut());
    #[expect(clippy::cast_possible_truncation, reason = "at most two arguments")]
    let argc = args.len() as u32;
    // SAFETY: per the caller; Zend runs in the C shim's frame.
    trapped(unsafe {
        swr_zend_try_call_method(
            object,
            scope,
            method.as_ptr().cast(),
            method.len(),
            retval,
            argc,
            arg(0),
            arg(1),
        )
    })
}

/// `Z_PARAM_FUNC` of a passed argument (1-based).
/// # Errors
/// Not a callable; Zend has raised its parameter error.
/// # Safety
/// `arg` is the live zval of argument `number`; `fci` and `fcc` are writable.
pub unsafe fn parse_func(
    arg: *mut zval,
    number: u32,
    fci: *mut zend_fcall_info,
    fcc: *mut zend_fcall_info_cache,
) -> Result<(), Invalid> {
    // SAFETY: per the caller.
    if unsafe { swr_zend_parse_func(arg, number, fci, fcc) } != 0 {
        Ok(())
    } else {
        Err(Invalid)
    }
}
