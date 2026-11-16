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

/// `swoole_set_last_error(code)` for native code.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_last_error(code: c_int) {
    set_last_error(code);
}

/// `swoole_log.h`'s levels (`SW_LOG_*`), which a test reads from that header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "the native level names")]
pub enum LogLevel {
    SW_LOG_DEBUG = 0,
    SW_LOG_TRACE = 1,
    SW_LOG_INFO = 2,
    SW_LOG_NOTICE = 3,
    SW_LOG_WARNING = 4,
    SW_LOG_ERROR = 5,
    SW_LOG_NONE = 6,
}

/// The logging operation, distinct from its severity level and error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    /// `swoole_sys_warning`, with errno set to the code first.
    SysWarning,
    /// `swoole_warning`.
    Warning,
    /// `swoole_notice`.
    Notice,
    /// `swoole_error_log(level, code, ...)`.
    Error,
    /// `swoole_info`.
    Info,
    /// `swoole_sys_error`, with errno set to the code first; it exits the process.
    SysError,
    /// `swoole_error`; it exits the process.
    Fatal,
}

/// The logging operation, its level and code, and text after `__SW_FUNC__`'s part.
pub type LogHost = fn(kind: LogKind, level: c_int, code: c_int, origin: &CStr, message: &CStr);

static HOST: OnceLock<LogHost> = OnceLock::new();

/// Registers native's log macros; later registrations are ignored.
pub fn swr_v1_core_set_log_host(host: LogHost) {
    HOST.get_or_init(|| host);
}

use crate::sys::errno;

fn emit(kind: LogKind, level: c_int, code: c_int, origin: &CStr, message: fmt::Arguments<'_>) {
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
    emit(LogKind::SysWarning, 0, code, origin, message);
}

/// `swoole_warning` from `origin`.
pub fn warning(origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LogKind::Warning, 0, 0, origin, message);
}

/// `swoole_notice`.
pub fn notice(message: fmt::Arguments<'_>) {
    emit(LogKind::Notice, 0, 0, c"", message);
}

/// `swoole_info`.
pub fn info(message: fmt::Arguments<'_>) {
    emit(LogKind::Info, 0, 0, c"", message);
}

/// `swoole_sys_error` from `origin`, with this thread's errno as on entry; the
/// process exits.
pub fn sys_error(origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LogKind::SysError, 0, errno(), origin, message);
}

/// `swoole_error` from `origin`; the process exits.
pub fn error(origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LogKind::Fatal, 0, 0, origin, message);
}

/// `swoole_error_log(level, code, ...)` from `origin`.
pub fn error_log(level: LogLevel, code: c_int, origin: &CStr, message: fmt::Arguments<'_>) {
    emit(LogKind::Error, level as c_int, code, origin, message);
}

/// Ends the process on a broken invariant. `unwrap`, `expect` and `panic` are
/// denied, and a bare `abort` says nothing, so code calls this instead: the
/// reason and the caller's location go to the log (stderr before `swoole_init`
/// sets one up), then the process aborts.
#[cold]
#[track_caller]
pub fn fatal(reason: &str) -> ! {
    const LEVEL: c_int = LogLevel::SW_LOG_ERROR as c_int;
    let at = std::panic::Location::caller();
    if HOST.get().is_some() {
        emit(
            LogKind::Error,
            LEVEL,
            0,
            c"fatal",
            format_args!("{reason} at {at}"),
        );
    } else {
        use std::io::Write;
        writeln!(std::io::stderr(), "fatal: {reason} at {at}").unwrap_or_default();
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "the one abort, after the log line"
    )]
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Seen = (c_int, c_int, c_int, String, String);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    #[test]
    fn levels_are_the_native_ones() {
        let header = include_str!("../../../native/utilities/swoole_log.h");
        for level in [
            LogLevel::SW_LOG_DEBUG,
            LogLevel::SW_LOG_TRACE,
            LogLevel::SW_LOG_INFO,
            LogLevel::SW_LOG_NOTICE,
            LogLevel::SW_LOG_WARNING,
            LogLevel::SW_LOG_ERROR,
            LogLevel::SW_LOG_NONE,
        ] {
            let line = format!("{level:?} = {},", level as c_int);
            assert!(header.contains(&line), "swoole_log.h lacks {line}");
        }
    }

    fn record(kind: LogKind, level: c_int, code: c_int, origin: &CStr, message: &CStr) {
        let text = |s: &CStr| s.to_string_lossy().into_owned();
        SEEN.lock()
            .unwrap()
            .push((kind as c_int, level, code, text(origin), text(message)));
    }

    #[test]
    fn last_error_is_per_thread() {
        set_last_error(1005);
        std::thread::spawn(|| {
            assert_eq!(last_error(), 0);
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
        error_log(
            LogLevel::SW_LOG_WARNING,
            1015,
            c"Socket::ssl_verify",
            format_args!("x"),
        );
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
