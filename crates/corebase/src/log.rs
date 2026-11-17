// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 include/swoole.h and include/swoole_log.h
// (Swoole contributors).
//! The thread's last error (`swoole_get_last_error`) and the logs Rust emits on
//! behalf of an upstream function, naming the function upstream's `__SW_FUNC__` did.
//! Once `swoole_init` has run, they run the log macros (`utilities::swr_v1_utils_log`).

use std::cell::Cell;
use std::ffi::{CStr, CString, c_int};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Whether `swoole_init` has run; Rust logs before it are dropped. `swoole_clean`
/// leaves it set.
static MACROS: AtomicBool = AtomicBool::new(false);

/// `swoole_init`'s first step: Rust logs run the log macros from here on.
pub(crate) fn init() {
    MACROS.store(true, Ordering::Relaxed);
}

use crate::sys::errno;

fn emit(kind: LogKind, level: c_int, code: c_int, origin: &CStr, message: fmt::Arguments<'_>) {
    if !MACROS.load(Ordering::Relaxed) {
        return;
    }
    // printf's %s stops at the first NUL, so the message does too.
    let mut text = message.to_string();
    text.truncate(text.find('\0').unwrap_or(text.len()));
    crate::utilities::swr_v1_utils_log(
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
    if MACROS.load(Ordering::Relaxed) {
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

/// What tests see of Rust logs: [`install`] runs `swoole_init`'s first step and gives
/// the process a logger at `SW_LOG_DEBUG` whose worker id tags the logging thread, so
/// [`logged`] reads this thread's lines back into the calls that wrote them.
#[cfg(any(test, feature = "log-capture"))]
pub mod capture {
    use super::LogKind;
    use crate::utilities::{Logger, LoggerHost, LoggerIdentity};
    use std::cell::Cell;
    use std::ffi::{CString, c_char, c_int};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// A log call: kind, level (0 but for `Error`), code (0 but for errors), origin
    /// and message.
    pub type Logged = (LogKind, c_int, c_int, String, String);

    static NEXT: AtomicI32 = AtomicI32::new(1);

    thread_local! {
        static TAG: c_int = NEXT.fetch_add(1, Ordering::Relaxed);
        static READ: Cell<usize> = const { Cell::new(0) };
    }

    extern "C" fn redirect(_: c_int) {}

    fn identity() -> LoggerIdentity {
        LoggerIdentity {
            worker_id: TAG.with(|tag| *tag),
            worker_pid: 1,
            worker_symbol: c_char::from_ne_bytes(*b"#"),
        }
    }

    static FILE: LazyLock<PathBuf> = LazyLock::new(|| {
        let path = std::env::temp_dir().join(format!("swr-logs-{}", std::process::id()));
        let logger = Box::leak(Box::new(Logger::new(
            None,
            LoggerHost { redirect, identity },
        )));
        logger.set_level(0);
        logger.open(&CString::new(path.as_os_str().as_bytes()).unwrap_or_default());
        // `SwooleG.logger` is a `swoole::Logger`, whose only member points at the Logger.
        let cpp: &mut &'static Logger = Box::leak(Box::new(&*logger));
        crate::swr_v1_core_set_logger(std::ptr::from_mut(cpp).cast());
        super::init();
        path
    });

    /// Run Rust logs through the log macros into the tagged logger, once per process.
    pub fn install() {
        LazyLock::force(&FILE);
    }

    const LABELS: [&str; 6] = ["DEBUG", "TRACE", "INFO", "NOTICE", "WARNING", "ERROR"];

    /// A logged line (after the tag) as the call that wrote it.
    fn parse(line: &str) -> Option<Logged> {
        use LogKind::{Error, Fatal, Info, Notice, SysError, SysWarning, Warning};
        let (label, content) = line.split_once('\t')?;
        let level = c_int::try_from(LABELS.iter().position(|known| *known == label)?).ok()?;
        let (sys, plain) = if label == "ERROR" {
            (SysError, Fatal)
        } else {
            (SysWarning, Warning)
        };
        let (kind, level, code, origin, message) =
            if let Some((origin, rest)) = content.split_once("() (ERRNO ") {
                let (code, message) = rest.split_once("): ")?;
                (Error, level, code, origin, message)
            } else if let Some((origin, message)) = content.split_once("(): ") {
                let failed = message
                    .rsplit_once(", Error: ")
                    .and_then(|(message, reason)| {
                        Some((message, reason.strip_suffix(']')?.rsplit_once('[')?.1))
                    });
                failed.map_or((plain, 0, "0", origin, message), |(message, code)| {
                    (sys, 0, code, origin, message)
                })
            } else {
                let kind = if label == "NOTICE" { Notice } else { Info };
                (kind, 0, "0", "", content)
            };
        Some((
            kind,
            level,
            code.parse().ok()?,
            origin.into(),
            message.into(),
        ))
    }

    /// This thread's logs since the last call.
    #[must_use]
    pub fn logged() -> Vec<Logged> {
        install();
        let tag = format!(".{}]\t", TAG.with(|tag| *tag));
        let lines = std::fs::read_to_string(&*FILE).unwrap_or_default();
        let mine: Vec<Logged> = lines
            .lines()
            .filter_map(|line| parse(line.split_once(&tag)?.1))
            .collect();
        let new = mine.get(READ.get()..).unwrap_or_default().to_vec();
        READ.set(mine.len());
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn logs_reach_the_host_with_origin_and_errno() {
        // A child, so the process logger stays out of the tests that reset the record.
        let case = "log::tests::logs_reach_the_host_with_origin_and_errno";
        if crate::utilities::isolated_process::run_in_child(case) {
            return;
        }
        capture::install();
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
        assert_eq!(
            capture::logged(),
            [
                (
                    LogKind::SysWarning,
                    0,
                    13,
                    "Socket::connect_sync".into(),
                    "bind(3) failed".into()
                ),
                (
                    LogKind::Error,
                    4,
                    1015,
                    "Socket::ssl_verify".into(),
                    "x".into()
                ),
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
