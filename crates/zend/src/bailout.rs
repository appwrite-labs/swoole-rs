//! Zend bailouts (`zend_bailout`, a `longjmp` to the nearest `zend_try`) around
//! code Rust calls. A bailout must never jump over a Rust frame, so Rust traps
//! it in C, returns normally, and hands a [`Bailout`] back to the C or C++ that
//! called it, which re-raises it with `zend_bailout()`. See docs/zend.md.
use crate::{
    Invalid,
    sys::{
        _zend_bailout, swr_zend_parse_func, swr_zend_try, swr_zend_try_call, swr_zend_try_report,
        swr_zend_try_report_code, swr_zend_try_report_flag, swr_zend_try_report_length,
        swr_zend_try_report_level, zend_fcall_info, zend_fcall_info_cache, zval,
    },
};
use std::{
    cell::Cell,
    ffi::{CStr, c_char, c_int, c_void},
};

/// A trapped bailout. Its owner finishes its Rust work, then reports it to
/// the C/C++ caller (for example as a flag), which calls `zend_bailout()`.
#[must_use = "a trapped bailout must be re-raised by the C/C++ caller"]
#[derive(Debug)]
pub struct Bailout;

thread_local! {
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

impl Bailout {
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

/// Whether a bailout waits for [`finish`]. No error is raised meanwhile:
/// upstream never ran past the first bailout.
#[must_use]
pub fn pending() -> bool {
    PENDING.get()
}

/// Raise a deferred bailout (`zend_bailout()`). Called last by every handler
/// and every export C or C++ calls, from a frame that owns nothing that needs
/// a drop by then: the `longjmp` crosses only that frame.
#[inline]
pub fn finish() {
    if PENDING.replace(false) {
        // SAFETY: Zend's own bailout, from the outermost Rust frame (above).
        unsafe { _zend_bailout(c"swoole-rs-zend".as_ptr().cast_mut(), 0) };
    }
}

/// [`defer`] then [`finish`]: a handler's last step.
pub fn raise(result: Result<(), Bailout>) {
    defer(result);
    finish();
}

/// A trap's flag as a result.
pub(crate) fn trapped(bailed: c_int) -> Result<(), Bailout> {
    if bailed != 0 { Err(Bailout) } else { Ok(()) }
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

/// [`report`] of a reporter that takes the error level.
/// # Errors
/// A user error handler bailed out.
/// # Safety
/// As [`report`].
pub unsafe fn report_level(
    report: unsafe extern "C" fn(c_int, *const c_char),
    level: c_int,
    message: &CStr,
) -> Result<(), Bailout> {
    if pending() {
        return Err(Bailout);
    }
    // SAFETY: as in `report`.
    trapped(unsafe { swr_zend_try_report_level(Some(report), level, message.as_ptr()) })
}

/// [`report`] of a reporter that takes raw text and its length.
/// # Errors
/// A user error handler bailed out.
/// # Safety
/// As [`report`]; `text` is what `report` reads.
pub unsafe fn report_length(
    report: unsafe extern "C" fn(*const c_char, c_int),
    text: *const c_char,
    length: c_int,
) -> Result<(), Bailout> {
    if pending() {
        return Err(Bailout);
    }
    // SAFETY: as in `report`.
    trapped(unsafe { swr_zend_try_report_length(Some(report), text, length) })
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
/// `body` is C or C++, not Rust (a bailout inside it would jump over its
/// frame), and may be called with `arg`.
pub unsafe fn trap(
    body: unsafe extern "C" fn(*mut c_void),
    arg: *mut c_void,
) -> Result<(), Bailout> {
    // SAFETY: per the caller; the shim's setjmp sits in C, between this frame
    // and `body`.
    trapped(unsafe { swr_zend_try(Some(body), arg) })
}

/// `zend_call_function(fci, fcc)` inside `zend_try`: whether the call
/// succeeded.
/// # Errors
/// The callable bailed out (`exit()`, a fatal error).
/// # Safety
/// `fci` and `fcc` describe a live callable, as `zend_call_function` requires.
pub unsafe fn call(
    fci: *mut zend_fcall_info,
    fcc: *mut zend_fcall_info_cache,
) -> Result<bool, Bailout> {
    let mut result = 0;
    // SAFETY: per the caller; Zend runs in the C shim's frame.
    if unsafe { swr_zend_try_call(fci, fcc, &raw mut result) } != 0 {
        return Err(Bailout);
    }
    Ok(result != 0)
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
