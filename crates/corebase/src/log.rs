// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 include/swoole.h and include/swoole_log.h
// (Swoole contributors).
//! The thread's last error (`swoole_get_last_error`) and the logs Rust emits on
//! behalf of an upstream function. crates/ffi's `swoole_init` registers the log
//! macros once (`swr_v1_core_set_log_host`, crates/utilities'), so Rust names the
//! function upstream's `__SW_FUNC__` did and corebase links without utilities.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_int};
use std::fmt;
use std::sync::OnceLock;

thread_local! {
    static LAST_ERROR: Cell<c_int> = const { Cell::new(0) };
}

/// `swoole_get_last_error()`.
#[must_use]
pub fn last_error() -> c_int {
    LAST_ERROR.get()
}

/// `swoole_set_last_error(code)`.
pub fn set_last_error(code: c_int) {
    LAST_ERROR.set(code);
}

/// `swoole_get_last_error()` for native code.
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_last_error() -> c_int {
    last_error()
}

/// `swoole_set_last_error(code)` for native code.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_last_error(code: c_int) {
    set_last_error(code);
}

/// `swoole_sys_warning`, with errno set to the code first.
pub const LOG_SYS_WARNING: c_int = 0;
/// `swoole_warning`.
pub const LOG_WARNING: c_int = 1;
/// `swoole_notice`.
pub const LOG_NOTICE: c_int = 2;
/// `swoole_error_log(level, code, ...)`.
pub const LOG_ERROR: c_int = 3;
/// `swoole_info`.
pub const LOG_INFO: c_int = 4;
/// `swoole_sys_error`, with errno set to the code first; it exits the process.
pub const LOG_SYS_ERROR: c_int = 5;
/// `swoole_error`; it exits the process.
pub const LOG_FATAL: c_int = 6;

/// Native's log macros: `kind` is a `LOG_*`, `level` and `code` are what that
/// macro takes, and `message` is the formatted text after `__SW_FUNC__`'s part.
pub type LogHost = fn(kind: c_int, level: c_int, code: c_int, origin: &CStr, message: &CStr);

static HOST: OnceLock<LogHost> = OnceLock::new();

/// Registers native's log macros; later registrations are ignored.
pub fn swr_v1_core_set_log_host(host: LogHost) {
    HOST.get_or_init(|| host);
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn emit(kind: c_int, level: c_int, code: c_int, origin: &CStr, message: fmt::Arguments<'_>) {
    let Some(host) = HOST.get() else { return };
    // printf's %s stops at the first NUL, so the message does too.
    let mut text = message.to_string();
    text.truncate(text.find('\0').unwrap_or(text.len()));
    host(
        kind,
        level,
        code,
        origin,
        &CString::new(text).unwrap_or_default(),
    );
}

/// `swoole_sys_warning` from `origin`, with this thread's errno as on entry.
pub fn sys_warning(origin: &CStr, message: fmt::Arguments<'_>) {
    sys_warning_errno(errno(), origin, message);
}

/// `swoole_sys_warning` from `origin`, with errno `code`.
pub fn sys_warning_errno(code: c_int, origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LOG_SYS_WARNING, 0, code, origin, message);
}

/// `swoole_warning` from `origin`.
pub fn warning(origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LOG_WARNING, 0, 0, origin, message);
}

/// `swoole_notice`.
pub fn notice(message: fmt::Arguments<'_>) {
    emit(LOG_NOTICE, 0, 0, c"", message);
}

/// `swoole_info`.
pub fn info(message: fmt::Arguments<'_>) {
    emit(LOG_INFO, 0, 0, c"", message);
}

/// `swoole_sys_error` from `origin`, with this thread's errno as on entry; the
/// process exits.
pub fn sys_error(origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LOG_SYS_ERROR, 0, errno(), origin, message);
}

/// `swoole_error` from `origin`; the process exits.
pub fn error(origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LOG_FATAL, 0, 0, origin, message);
}

/// `swoole_error_log(level, code, ...)` from `origin`.
pub fn error_log(level: c_int, code: c_int, origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LOG_ERROR, level, code, origin, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Seen = (c_int, c_int, c_int, String, String);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    fn record(kind: c_int, level: c_int, code: c_int, origin: &CStr, message: &CStr) {
        let text = |s: &CStr| s.to_string_lossy().into_owned();
        SEEN.lock()
            .unwrap()
            .push((kind, level, code, text(origin), text(message)));
    }

    #[test]
    fn last_error_is_per_thread() {
        set_last_error(1005);
        std::thread::spawn(|| {
            assert_eq!(swr_v1_core_last_error(), 0);
            swr_v1_core_set_last_error(7);
        })
        .join()
        .unwrap();
        assert_eq!(last_error(), 1005);
    }

    #[test]
    fn logs_reach_the_host_with_origin_and_errno() {
        swr_v1_core_set_log_host(record);
        // SAFETY: test-only errno write on this thread.
        unsafe { *libc_errno() = 13 };
        sys_warning(
            c"Socket::connect_sync",
            format_args!("bind({}) failed\0tail", 3),
        );
        error_log(4, 1015, c"Socket::ssl_verify", format_args!("x"));
        let seen = SEEN.lock().unwrap();
        assert_eq!(
            seen[..],
            [
                (
                    0,
                    0,
                    13,
                    "Socket::connect_sync".into(),
                    "bind(3) failed".into()
                ),
                (3, 4, 1015, "Socket::ssl_verify".into(), "x".into()),
            ]
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn libc_errno() -> *mut c_int {
        unsafe { libc::__errno_location() }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn libc_errno() -> *mut c_int {
        unsafe { libc::__error() }
    }
}
