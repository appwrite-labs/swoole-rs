//! What this crate asks of the executor with nothing in hand that says where it
//! runs: each function takes a [`Php`], so only a thread that runs PHP reaches
//! the shim behind it, and the declarations stay private to this module. The
//! trace printer can run PHP code (output handlers), so its call stays `unsafe`.
use std::ffi::c_int;
use swoole_rs_zend::{Php, bailout::Bailout};

unsafe extern "C" {
    /// Whether the frame below the executor's current one runs a function named
    /// `__destruct`; false without a current frame, a frame below it or a
    /// function there. It reads this thread's executor and runs no PHP code.
    safe fn swr_zend_coroutine_called_by_destructor() -> bool;
    /// Whether the constant `SWOOLE_LIBRARY` is defined and true, as
    /// `zend_is_true` reads it: the constant table and one value, with no PHP
    /// code run.
    safe fn swr_zend_coroutine_library_loaded() -> bool;
    /// `sw_php_print_backtrace_impl(skip_last, options, limit)` inside
    /// `zend_try`: the executor's frames, written through PHP's output layer,
    /// whose handlers may run PHP code; nonzero if that bailed out.
    fn swr_zend_coroutine_print_trace(skip_last: c_int, options: c_int, limit: c_int) -> c_int;
}

/// Whether the function that called the running internal function is named
/// `__destruct` (`go()` during shutdown functions).
pub(crate) fn called_by_destructor(_php: Php<'_>) -> bool {
    swr_zend_coroutine_called_by_destructor()
}

/// Whether the PHP library is loaded: `SWOOLE_LIBRARY` is defined and true.
pub(crate) fn library_loaded(_php: Php<'_>) -> bool {
    swr_zend_coroutine_library_loaded()
}

/// Print the frames from Zend's current one, less the last `skip_last`.
/// # Errors
/// An output handler bailed out.
pub(crate) fn print_trace(
    _php: Php<'_>,
    skip_last: c_int,
    options: c_int,
    limit: c_int,
) -> Result<(), Bailout> {
    // SAFETY: on the PHP thread `_php` proves, with the frames Zend has installed;
    // a bailout from an output handler is trapped and returned.
    if unsafe { swr_zend_coroutine_print_trace(skip_last, options, limit) } == 0 {
        Ok(())
    } else {
        Err(Bailout)
    }
}
