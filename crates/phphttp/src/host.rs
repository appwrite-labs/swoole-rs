//! What the methods ask of `swoole::http::Context`, implemented in ext-src
//! (`native/httpbindings/swoole_rs_httpbindings.h`) until the context moves to
//! Rust. `Context` is that object, opaque here.
use std::ffi::{CString, c_char, c_int, c_void};
use swoole_rs_zend::{
    bailout,
    sys::{zend_long, zend_string, zval},
};

pub(crate) type Context = c_void;

/// `swoole_http_ctx` state bits, as `swr_http_host_flags` packs them.
pub(crate) const END: u32 = 1;
pub(crate) const DETACHED: u32 = 1 << 1;
pub(crate) const COMPLETED: u32 = 1 << 2;
pub(crate) const HTTP2: u32 = 1 << 3;
pub(crate) const SEND_CHUNKED: u32 = 1 << 4;
pub(crate) const CO_SOCKET: u32 = 1 << 5;
pub(crate) const UPGRADE: u32 = 1 << 6;
pub(crate) const WEBSOCKET: u32 = 1 << 7;
/// `swr_http_host_mark`'s second flag (`SWR_PHPHTTP_SEND_TRAILER`).
pub(crate) const SEND_TRAILER: u32 = 256;

/// `swr_http_host_response_property`'s properties.
pub(crate) const HEADER: c_int = 0;
pub(crate) const COOKIE: c_int = 1;
pub(crate) const TRAILER: c_int = 2;

/// Swoole error codes (`include/swoole_error.h`; ext-src asserts them).
pub(crate) const PHP_FATAL_ERROR: c_int = 503;
pub(crate) const PHP_RUNTIME_NOTICE: c_int = 510;
pub(crate) const SESSION_CLOSED: c_int = 1004;
pub(crate) const HTTP_CONTEXT_UNAVAILABLE: c_int = 7106;
pub(crate) const SERVER_IS_NOT_REGULAR_FILE: c_int = 9013;

/// `SW_HTTP_HEADER_KEY_SIZE`.
pub(crate) const HEADER_KEY_SIZE: usize = 128;

unsafe extern "C" {
    pub(crate) fn swr_http_host_error(code: c_int, always: c_int, message: *const c_char);
    pub(crate) fn swr_http_host_flags(context: *const Context) -> u32;
    pub(crate) fn swr_http_host_fd(context: *const Context) -> zend_long;
    pub(crate) fn swr_http_host_mark(context: *mut Context, flag: u32);
    pub(crate) fn swr_http_host_request_free(context: *mut Context);
    pub(crate) fn swr_http_host_request_create(return_value: *mut zval, options: *mut zval)
    -> bool;
    fn swr_http_host_log(level: c_int, error: c_int, func: *const c_char, message: *const c_char);
    pub(crate) fn swr_http_host_treat_data(arg: c_int, text: *mut c_char, array: *mut zval);
    pub(crate) fn swr_http_host_websocket_connection(context: *mut Context) -> c_int;
    pub(crate) fn swr_http_host_upload_tmp_dir(context: *const Context) -> *const c_char;
    pub(crate) fn swr_http_host_tmpfile(path: *mut c_char) -> c_int;
    pub(crate) fn swr_http_host_http2_data(
        context: *mut Context,
        length: *mut usize,
    ) -> *const c_char;
    pub(crate) fn swr_http_host_response_free(context: *mut Context) -> bool;
    pub(crate) fn swr_http_host_response_create(
        object: *mut zval,
        fd: zend_long,
        return_value: *mut zval,
    );
    pub(crate) fn swr_http_host_response_property(context: *mut Context, which: c_int)
    -> *mut zval;
    pub(crate) fn swr_http_host_write(
        context: *mut Context,
        data: *mut zend_string,
        return_value: *mut zval,
    ) -> bool;
    pub(crate) fn swr_http_host_end(
        context: *mut Context,
        data: *mut zend_string,
        return_value: *mut zval,
    ) -> bool;
    pub(crate) fn swr_http_host_send_file(
        context: *mut Context,
        file: *mut zend_string,
        offset: zend_long,
        length: zend_long,
    ) -> c_int;
    pub(crate) fn swr_http_host_redirect(
        context: *mut Context,
        url: *mut zval,
        code: *mut zval,
        return_value: *mut zval,
    ) -> bool;
    pub(crate) fn swr_http_host_status(
        context: *mut Context,
        status: zend_long,
        reason: *const c_char,
        reason_length: usize,
    );
    pub(crate) fn swr_http_host_set_header(
        context: *mut Context,
        key: *const c_char,
        key_length: usize,
        value: *mut zval,
        format: bool,
    ) -> bool;
    pub(crate) fn swr_http_host_disconnect(
        context: *mut Context,
        code: zend_long,
        reason: *mut zend_string,
        return_value: *mut zval,
    );
    pub(crate) fn swr_http_host_push(
        context: *mut Context,
        data: *mut zval,
        opcode: zend_long,
        flags: *mut zval,
        return_value: *mut zval,
    ) -> bool;
    pub(crate) fn swr_http_host_ping(
        context: *mut Context,
        data: *mut zend_string,
        return_value: *mut zval,
    );
    pub(crate) fn swr_http_host_goaway(
        context: *mut Context,
        code: zend_long,
        debug: *mut zend_string,
    ) -> bool;
    pub(crate) fn swr_http_host_upgrade(context: *mut Context) -> c_int;
    pub(crate) fn swr_http_host_close(context: *mut Context) -> bool;
    pub(crate) fn swr_http_host_recv(context: *mut Context, timeout: f64, return_value: *mut zval);
}

/// Re-raise a bailout a host call trapped. Host calls that can run user code
/// (a header value's `__toString`, `exit()` in it) trap a bailout in C++ and
/// report it; the handler finishes, then raises it as its last step
/// (docs/zend.md).
pub(crate) fn raise(bailed: bool) {
    if bailed {
        bailout::Bailout.defer();
    }
    bailout::finish();
}

/// `swoole_set_last_error(code)`.
pub(crate) fn set_last_error(code: c_int) {
    // SAFETY: no message, so only the thread's last error changes.
    unsafe { swr_http_host_error(code, 0, std::ptr::null()) };
}

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
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: C++'s reporter, printing the message through "%s".
    bailout::defer(unsafe {
        bailout::report_code(swr_http_host_error, code, c_int::from(always), &message)
    });
}

/// Swoole's log line as `func` would print it: `swoole_error_log(level, error)`
/// when `error > 0`, `swoole_warning` when 0, `swoole_sys_warning` for `-errno`.
/// The message ends at its first NUL, as `%s`/`%.*s` do.
pub(crate) fn log(level: c_int, error: c_int, func: &std::ffi::CStr, message: impl Into<Vec<u8>>) {
    let mut message = message.into();
    if let Some(nul) = message.iter().position(|&byte| byte == 0) {
        message.truncate(nul);
    }
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: NUL-terminated strings, printed through "%s".
    unsafe { swr_http_host_log(level, error, func.as_ptr(), message.as_ptr()) };
}

/// The context's state bits.
pub(crate) fn flags(context: *const Context) -> u32 {
    // SAFETY: a live context.
    unsafe { swr_http_host_flags(context) }
}

/// `ctx->fd`.
pub(crate) fn fd(context: *const Context) -> zend_long {
    // SAFETY: a live context.
    unsafe { swr_http_host_fd(context) }
}
