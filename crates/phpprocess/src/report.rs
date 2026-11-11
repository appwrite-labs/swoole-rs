//! Swoole's PHP error macros (`php_swoole_private.h`).
use std::ffi::CStr;
use swoole_rs_zend::{E_ERROR, Message, bailout, report};

pub(crate) const PHP_FATAL_ERROR: i32 = 503;
pub(crate) const INVALID_PARAMS: i32 = 505;
pub(crate) const OPERATION_NOT_SUPPORT: i32 = 507;
pub(crate) const PHP_RUNTIME_NOTICE: i32 = 510;

pub(crate) fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub(crate) use swoole_rs_corebase::log::set_last_error;
/// Set `errno` (to 0 before a call whose -1 result is ambiguous).
pub(crate) use swoole_rs_corebase::set_errno;

/// `SWOOLE_G(cli)`.
pub(crate) fn cli() -> bool {
    swoole_rs_phpswoole::globals().cli.get()
}

/// `php_swoole_fatal_error`: always reported.
pub(crate) fn fatal(level: i32, text: &Message) {
    set_last_error(PHP_FATAL_ERROR);
    bailout::defer(report(level, text.as_cstr()));
}

/// `php_swoole_error`: reported when `swoole.display_errors` is on.
pub(crate) fn notice(level: i32, text: &Message) {
    set_last_error(PHP_RUNTIME_NOTICE);
    displayed(level, text);
}

/// `php_swoole_sys_error`: with `strerror(errno)` and errno appended.
pub(crate) fn sys(level: i32, text: &Message) {
    let error = errno();
    set_last_error(error);
    let mut full = *text;
    full.push(b", Error: ");
    // SAFETY: strerror has no preconditions.
    let reason = unsafe { libc::strerror(error) };
    // SAFETY: strerror returns a NUL-terminated message, as upstream reads it.
    full.push(unsafe { CStr::from_ptr(reason) }.to_bytes());
    full.push(format!("[{error}]").as_bytes());
    displayed(level, &full);
}

/// Report `text` when it is an `E_ERROR` or `swoole.display_errors` is on.
fn displayed(level: i32, text: &Message) {
    if level == E_ERROR || swoole_rs_phpswoole::display_errors() {
        bailout::defer(report(level, text.as_cstr()));
    }
}
