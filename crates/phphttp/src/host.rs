//! What the methods share: a `swoole::http::Context`'s state bits and fd, and
//! Swoole's error reporting. `Context` is that object, opaque here.
use std::cmp::Ordering;
use std::ffi::{CString, c_int, c_void};
use swoole_rs_corebase::log;
use swoole_rs_zend::{E_WARNING, bailout, sys::zend_long};

pub(crate) type Context = c_void;

/// `swoole_http_ctx` state bits, as `flags` packs them.
pub(crate) const END: u32 = 1;
pub(crate) const DETACHED: u32 = 1 << 1;
pub(crate) const COMPLETED: u32 = 1 << 2;
pub(crate) const HTTP2: u32 = 1 << 3;
pub(crate) const SEND_CHUNKED: u32 = 1 << 4;
pub(crate) const CO_SOCKET: u32 = 1 << 5;
pub(crate) const UPGRADE: u32 = 1 << 6;
pub(crate) const WEBSOCKET: u32 = 1 << 7;

/// The response's array properties (`header::response_property`).
pub(crate) const HEADER: c_int = 0;
pub(crate) const COOKIE: c_int = 1;
pub(crate) const TRAILER: c_int = 2;

/// Swoole error codes (`include/swoole_error.h`; ext-src asserts them).
pub(crate) const PHP_FATAL_ERROR: c_int = 503;
pub(crate) const PHP_RUNTIME_NOTICE: c_int = 510;
pub(crate) const NO_PAYLOAD: c_int = 550;
pub(crate) const SESSION_CLOSED: c_int = 1004;
pub(crate) const HTTP_CONTEXT_UNAVAILABLE: c_int = 7106;
pub(crate) const SERVER_IS_NOT_REGULAR_FILE: c_int = 9013;

/// `SW_HTTP_HEADER_KEY_SIZE`.
pub(crate) const HEADER_KEY_SIZE: usize = 128;

/// Re-raise a bailout a host call trapped. Host calls that can run user code
/// (a header value's `__toString`, `exit()` in it) trap a bailout in C and
/// report it; the handler finishes, then raises it as its last step
/// (docs/zend.md).
pub(crate) fn raise(bailed: bool) {
    if bailed {
        bailout::Bailout.defer();
    }
    bailout::finish();
}

pub(crate) use swoole_rs_corebase::log::set_last_error;

/// `php_swoole_error(E_WARNING, ...)`: a runtime notice, shown when Swoole's
/// `display_errors` is on.
pub(crate) fn notice(message: &str) {
    report(PHP_RUNTIME_NOTICE, false, message);
}

/// `php_swoole_fatal_error(E_WARNING, ...)`: always shown.
pub(crate) fn fatal(message: &str) {
    report(PHP_FATAL_ERROR, true, message);
}

/// Set `code` as the last error and raise `message` as an `E_WARNING`.
/// The message ends at its first NUL, as the `%s` upstream formats it with does.
pub(crate) fn report(code: c_int, always: bool, message: impl Into<Vec<u8>>) {
    let mut message = message.into();
    if let Some(nul) = message.iter().position(|&byte| byte == 0) {
        message.truncate(nul);
    }
    if bailout::pending() {
        return;
    }
    set_last_error(code);
    if always || swoole_rs_phpswoole::display_errors() {
        let message = CString::new(message).unwrap_or_default();
        bailout::defer(swoole_rs_zend::report(E_WARNING, &message));
    }
}

/// Swoole's log line as `func` would print it: `swoole_error_log(level, error)`
/// when `error > 0`, `swoole_warning` when 0, `swoole_sys_warning` for `-errno`.
/// The message ends at its first NUL, as `%s`/`%.*s` do.
pub(crate) fn log(level: c_int, error: c_int, func: &std::ffi::CStr, message: impl Into<Vec<u8>>) {
    let message = message.into();
    let message = String::from_utf8_lossy(&message);
    match error.cmp(&0) {
        Ordering::Greater => log::error_log(level, error, func, format_args!("{message}")),
        Ordering::Equal => log::warning(func, format_args!("{message}")),
        Ordering::Less => {
            log::sys_warning_errno(error.wrapping_neg(), func, format_args!("{message}"));
        }
    }
}

/// The context's state bits.
pub(crate) fn flags(context: *const Context) -> u32 {
    // SAFETY: a live context, which starts with its `ContextState`.
    let state = unsafe { &*context.cast::<crate::state::State>() };
    u32::from(state.end)
        | u32::from(state.detached) << 1
        | u32::from(state.completed) << 2
        | u32::from(state.http2) << 3
        | u32::from(state.send_chunked) << 4
        | u32::from(crate::context::co_socket(state)) << 5
        | u32::from(state.upgrade) << 6
        | u32::from(state.websocket) << 7
}

/// `ctx->fd`.
pub(crate) fn fd(context: *const Context) -> zend_long {
    // SAFETY: as above.
    unsafe { (*context.cast::<crate::state::State>()).fd }
}
