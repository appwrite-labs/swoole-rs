//! `swoole::Logger` (upstream `src/core/log.cc`): level, date format, log file,
//! rotation, stdout redirection and `put`, behind one lock. Diagnostics go out
//! through corebase's `log`; worker identity and `swoole_redirect_stdout` go
//! through `LoggerHost` (crates/process's). Both run after the lock is released, so a
//! diagnostic can log through this logger again. The log macros a Rust log runs
//! (`swr_v1_utils_log`) are here too.
use crate::lent::{Count, In};
use crate::log;
use crate::sys::{self, errno};
use crate::utilities::{log_level, log_line, log_micros, log_rotation};
use crate::{CCast, Stream};
use libc::{c_char, c_int};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

const SW_LOG_INFO: c_int = 2;
const SW_LOG_NOTICE: c_int = crate::log::LogLevel::SW_LOG_NOTICE as c_int;
const SW_LOG_ERROR: c_int = 5;
const SW_LOG_DATE_STRLEN: usize = 128;
const SW_LOG_BUFFER_SIZE: usize = 16384 + 256;
const SW_LOG_DEFAULT_DATE_FORMAT: &CStr = c"%F %T";

/// `SW_LOG_WARNING`, `SW_ERROR_SYSTEM_CALL_FAIL` and `SW_ERROR_INVALID_PARAMS`.
const LOG_WARNING: c_int = 4;
const SYSTEM_CALL_FAIL: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SYSTEM_CALL_FAIL as c_int;
const INVALID_PARAMS: c_int = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as c_int;
const REDIRECT_ORIGIN: &CStr = c"Logger::redirect_stdout_and_stderr";

#[derive(Default)]
pub struct LoggerIdentity {
    pub worker_id: c_int,
    pub worker_pid: c_int,
    pub worker_symbol: c_char,
}

/// The native effects a logger asks for.
#[derive(Clone, Copy)]
pub struct LoggerHost {
    /// `swoole_redirect_stdout(fd)`.
    pub redirect: extern "C" fn(fd: c_int),
    pub identity: fn() -> LoggerIdentity,
}

/// A diagnostic or native effect, run once the lock is released.
#[derive(Debug, PartialEq)]
enum Effect {
    /// errno, the real file.
    OpenFailed(c_int, CString),
    /// The format's length.
    DateFormatTooLong(c_int),
    /// `Logger::redirect_stdout_and_stderr`'s `swoole_warning`.
    Warning(&'static str),
    /// `Logger::redirect_stdout_and_stderr`'s `swoole_sys_warning`, with errno.
    SysWarning(&'static str, c_int),
    Redirect(c_int),
}

struct State {
    /// The descriptor `open` made, which takes each line in one `write(2)`
    /// under the lock; lines go to `stream` while there is none.
    file_fd: Option<c_int>,
    redirected: bool,
    microseconds: bool,
    rotation: c_int,
    stdout_fd: c_int,
    stderr_fd: c_int,
    /// `stdout`, or the stream `set_stream` gave, which only the writes under the
    /// logger's lock use.
    stream: Option<Stream>,
    stdout: Option<Stream>,
    date_format: CString,
    file: CString,
    real_file: CString,
}

pub struct Logger {
    host: LoggerHost,
    level: AtomicI32,
    state: Mutex<State>,
}

fn now() -> (libc::time_t, i64) {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (
        libc::time_t::try_from(since.as_secs()).unwrap_or(libc::time_t::MAX),
        i64::from(since.subsec_micros()),
    )
}

/// `std::strftime(out, format, std::localtime(&time))`, through the reentrant
/// `localtime_r`: the same calendar, without libc's shared record.
fn strftime(out: &mut [u8], format: &CStr, time: libc::time_t) -> usize {
    sys::localtime_r(time).map_or(0, |tm| sys::strftime(out, format, &tm))
}

/// `Logger::gen_real_file`'s date suffix for `rotation`.
fn rotation_date(rotation: c_int, out: &mut [u8; 16]) -> usize {
    strftime(out, log_rotation(rotation), now().0)
}

fn real_file(rotation: c_int, file: &CStr) -> CString {
    let mut date = [0u8; 16];
    let length = rotation_date(rotation, &mut date);
    let mut path = file.to_bytes().to_vec();
    path.push(b'.');
    path.extend(date.iter().take(length));
    CString::new(path).unwrap_or_default()
}

/// All of `bytes` to `fd`, as `fwrite` to an unbuffered stream would: short
/// writes continue, and an error drops the rest.
fn write_all(fd: c_int, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let Some(written @ 1..) = sys::write(fd, bytes) else {
            return;
        };
        bytes = bytes.get(written..).unwrap_or_default();
    }
}

impl State {
    fn close(&mut self) {
        if let Some(fd) = self.file_fd.take() {
            _ = sys::close(fd);
            self.stream = self.stdout;
            self.file = CString::default();
        }
    }

    fn open(&mut self, file: CString, effects: &mut Vec<Effect>) -> bool {
        self.close();
        self.real_file = if self.rotation == 0 {
            file.clone()
        } else {
            real_file(self.rotation, &file)
        };
        self.file = file;
        let flags = libc::O_APPEND | libc::O_RDWR | libc::O_CREAT;
        let Ok(fd) = sys::open(&self.real_file, flags, 0o666) else {
            effects.push(Effect::OpenFailed(
                errno(),
                std::mem::take(&mut self.real_file),
            ));
            self.file = CString::default();
            return false;
        };
        self.file_fd = Some(fd);
        true
    }

    fn reopen(&mut self, effects: &mut Vec<Effect>) {
        if self.file_fd.is_none() {
            return;
        }
        let file = self.file.clone();
        self.close();
        self.open(file, effects);
        if self.redirected
            && let Some(fd) = self.file_fd
        {
            effects.push(Effect::Redirect(fd));
        }
    }

    fn redirect(&mut self, effects: &mut Vec<Effect>) -> bool {
        let Some(fd) = self.file_fd else {
            effects.push(Effect::Warning("no log file opened"));
            return false;
        };
        if self.redirected {
            effects.push(Effect::Warning("has been redirected"));
            return false;
        }
        self.stdout_fd = sys::dup(libc::STDOUT_FILENO).unwrap_or(-1);
        if self.stdout_fd < 0 {
            effects.push(Effect::SysWarning("dup(STDOUT_FILENO) failed", errno()));
            return false;
        }
        self.stderr_fd = sys::dup(libc::STDERR_FILENO).unwrap_or(-1);
        if self.stderr_fd < 0 {
            effects.push(Effect::SysWarning("dup(STDERR_FILENO) failed", errno()));
            return false;
        }
        effects.push(Effect::Redirect(fd));
        self.redirected = true;
        true
    }

    fn restore(&mut self, effects: &mut Vec<Effect>) -> bool {
        if !self.redirected {
            effects.push(Effect::Warning("no redirected"));
            return false;
        }
        if sys::dup2(self.stdout_fd, libc::STDOUT_FILENO).is_err() {
            effects.push(Effect::SysWarning("dup2(STDOUT_FILENO) failed", errno()));
        }
        if sys::dup2(self.stderr_fd, libc::STDERR_FILENO).is_err() {
            effects.push(Effect::SysWarning("dup2(STDERR_FILENO) failed", errno()));
        }
        // The descriptors `redirect` duplicated, closed once.
        _ = sys::close(self.stdout_fd);
        _ = sys::close(self.stderr_fd);
        self.stdout_fd = -1;
        self.stderr_fd = -1;
        self.redirected = false;
        true
    }

    fn write(
        &mut self,
        level: c_int,
        content: &[u8],
        identity: &LoggerIdentity,
        effects: &mut Vec<Effect>,
    ) {
        let (seconds, micros) = now();
        let mut date = [0u8; SW_LOG_DATE_STRLEN];
        let mut length_date = strftime(&mut date, &self.date_format, seconds);
        if self.rotation != 0 && real_file(self.rotation, &self.file) != self.real_file {
            self.reopen(effects);
        }
        if self.microseconds {
            length_date += log_micros(date.get_mut(length_date..).unwrap_or_default(), micros);
        }
        let mut line = vec![0u8; SW_LOG_BUFFER_SIZE];
        let date = date.get(..length_date).unwrap_or_default();
        let symbol = u8::from_ne_bytes(identity.worker_symbol.to_ne_bytes());
        let (pid, worker) = (identity.worker_pid, identity.worker_id);
        let n = log_line(&mut line, date, symbol, pid, worker, level, content);
        let line = line.get(..n).unwrap_or_default();
        if let Some(fd) = self.file_fd {
            write_all(fd, line);
        } else if let Some(stream) = self.stream {
            stream.write(line);
            stream.flush();
        }
    }
}

impl Logger {
    fn run(&self, effect: Effect) {
        match effect {
            Effect::OpenFailed(code, file) => {
                // strerror's text: std's, without the code it appends.
                let reason = std::io::Error::from_raw_os_error(code).to_string();
                let suffix = format!(" (os error {code})");
                let reason = reason.trim_suffix(&suffix);
                log::error_log(
                    log::LogLevel::SW_LOG_WARNING,
                    SYSTEM_CALL_FAIL,
                    c"Logger::open",
                    format_args!(
                        "open('{}') failed. Error: {}[{code}]",
                        file.to_string_lossy(),
                        reason
                    ),
                );
            }
            Effect::DateFormatTooLong(length) => {
                log::set_last_error(INVALID_PARAMS);
                log::error_log(
                    log::LogLevel::SW_LOG_WARNING,
                    INVALID_PARAMS,
                    c"Logger::set_date_format",
                    format_args!("The date format string[length={length}] is too long"),
                );
            }
            Effect::Warning(message) => log::warning(REDIRECT_ORIGIN, format_args!("{message}")),
            Effect::SysWarning(message, code) => {
                log::sys_warning_errno(code, REDIRECT_ORIGIN, format_args!("{message}"));
            }
            Effect::Redirect(fd) => (self.host.redirect)(fd),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `operation` under the lock, then its native effects.
    fn with<T>(&self, operation: impl FnOnce(&mut State, &mut Vec<Effect>) -> T) -> T {
        let mut effects = Vec::new();
        let result = operation(&mut self.lock(), &mut effects);
        for effect in effects {
            self.run(effect);
        }
        result
    }
}

impl Logger {
    /// `Logger::open`.
    pub fn open(&self, file: &CStr) -> bool {
        let file = file.to_owned();
        self.with(|state, effects| state.open(file, effects))
    }

    /// `new Logger()` writing to `stdout_stream`.
    #[must_use]
    pub fn new(stdout_stream: Option<Stream>, host: LoggerHost) -> Self {
        Self {
            host,
            level: AtomicI32::new(SW_LOG_INFO),
            state: Mutex::new(State {
                file_fd: None,
                redirected: false,
                microseconds: false,
                rotation: 0,
                stdout_fd: -1,
                stderr_fd: -1,
                stream: stdout_stream,
                stdout: stdout_stream,
                date_format: SW_LOG_DEFAULT_DATE_FORMAT.into(),
                file: CString::default(),
                real_file: CString::default(),
            }),
        }
    }

    /// `Logger::reopen`.
    pub fn reopen(&self) {
        self.with(State::reopen);
    }

    /// `Logger::close`.
    pub fn close(&self) {
        self.lock().close();
    }

    /// `Logger::set_stream`.
    pub fn set_stream(&self, stream: Option<Stream>) {
        let mut state = self.lock();
        state.close();
        state.stream = stream;
    }

    /// `Logger::get_level`.
    pub fn level(&self) -> c_int {
        self.level.load(Relaxed)
    }

    /// `Logger::set_level`, clamped to the defined levels.
    pub fn set_level(&self, level: c_int) {
        self.level.store(log_level(level), Relaxed);
    }

    /// `Logger::set_rotation`.
    pub fn set_rotation(&self, rotation: c_int) {
        self.lock().rotation = rotation;
    }

    /// `Logger::set_date_with_microseconds`.
    pub fn set_date_with_microseconds(&self, enable: bool) {
        self.lock().microseconds = enable;
    }

    /// `Logger::reset`: the date format, microseconds, rotation and level.
    pub fn reset(&self) {
        let mut state = self.lock();
        state.date_format = SW_LOG_DEFAULT_DATE_FORMAT.into();
        state.microseconds = false;
        state.rotation = 0;
        self.level.store(SW_LOG_INFO, Relaxed);
    }

    /// `Logger::set_date_format`: false, with a warning, if the format renders nothing.
    /// The format is measured before `strftime` reads it; upstream measured it only to
    /// keep it or to log its length.
    pub fn set_date_format(&self, format: &CStr) -> bool {
        let mut date = [0u8; SW_LOG_DATE_STRLEN];
        let fits = strftime(&mut date, format, now().0) != 0;
        if !fits && LOG_WARNING < self.level() {
            log::set_last_error(INVALID_PARAMS);
            return false;
        }
        self.with(|state, effects| {
            if !fits {
                let length = c_int::try_from(format.to_bytes().len()).unwrap_or(c_int::MAX);
                effects.push(Effect::DateFormatTooLong(length));
                return false;
            }
            format.clone_into(&mut state.date_format);
            true
        })
    }

    /// `Logger::redirect_stdout_and_stderr`.
    pub fn redirect_stdout_and_stderr(&self, enable: bool) -> bool {
        self.with(|state, effects| {
            if enable {
                state.redirect(effects)
            } else {
                state.restore(effects)
            }
        })
    }

    /// `Logger::is_opened`.
    pub fn is_opened(&self) -> bool {
        self.lock().file_fd.is_some()
    }

    /// `Logger::put`: `content` through its first NUL, as upstream's `%.*s`
    /// reads it.
    pub fn put(&self, level: c_int, content: &[u8]) {
        if level < self.level() {
            return;
        }
        let mut identity = (self.host.identity)();
        if identity.worker_pid == 0 {
            identity.worker_pid = crate::sys::getpid();
        }
        self.with(|state, effects| state.write(level, content, &identity, effects));
    }
}

/// [`Logger::put`] of the `length` bytes C lends at `content`, through the first NUL,
/// as upstream's `%.*s` with `static_cast<int>(length)` reads them.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_put<'id>(
    logger: &Logger,
    level: c_int,
    content: In<'id, u8>,
    length: Count<'id>,
) {
    let content = content.drain(length);
    // printf precision is a signed int: negative means read through the terminator,
    // within the lent bytes.
    let limit = usize::try_from(content.len().c_cast::<i32>()).unwrap_or(usize::MAX);
    let content = content.first(limit).bytes();
    let content = content.split(|&byte| byte == 0).next().unwrap_or_default();
    logger.put(level, content);
}

/// The process logger, `sw_logger()`: a `swoole::Logger`, whose only member is
/// this crate's `Logger`, or null before `swoole_init`.
#[must_use]
pub fn process_logger() -> Option<&'static Logger> {
    let cpp = crate::swr_v1_core_logger().cast::<&'static Logger>();
    // SAFETY: null, or a live `swoole::Logger` whose only member is a non-null pointer
    // to a live Logger (crates/process's `LOGGER`, freed only with the object).
    unsafe { cpp.as_ref() }.copied()
}

/// `void swoole_set_log_level(int level)` (upstream `src/core/base.cc`).
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_set_log_leveli")
)]
pub extern "C" fn swoole_set_log_level(level: c_int) {
    if let Some(logger) = process_logger() {
        logger.set_level(level);
    }
}

/// `int swoole_get_log_level()`: `SW_LOG_NONE` without a logger.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_get_log_levelv")
)]
pub extern "C" fn swoole_get_log_level() -> c_int {
    const SW_LOG_NONE: c_int = 6;
    process_logger().map_or(SW_LOG_NONE, Logger::level)
}

/// `void swoole_set_log_file(const char *file)`.
pub fn set_log_file(file: &CStr) {
    if let Some(logger) = process_logger() {
        logger.open(file);
    }
}

/// The log macros (`swoole_log.h`) for a log Rust emits on behalf of `origin`, of
/// corebase's `LogKind` (crates/corebase/src/log.rs): the last error, the level
/// check, the line formatted into `sw_error`, the put and, for the errors, the exit.
/// corebase's `log` calls it once `swoole_init` has run.
pub fn swr_v1_utils_log(
    kind: log::LogKind,
    level: c_int,
    code: c_int,
    origin: &CStr,
    message: &CStr,
) {
    let origin = origin.to_bytes();
    let message = message.to_bytes();
    let threshold = swoole_get_log_level();
    let failed = || {
        let reason = swoole_rs_error::strerror_text(code, crate::sys::strerror);
        let code = code.to_string();
        let tail = [b", Error: ", reason.to_bytes(), b"[", code.as_bytes(), b"]"];
        [origin, b"(): ", message, &tail.concat()].concat()
    };
    if matches!(kind, log::LogKind::SysWarning | log::LogKind::SysError) {
        crate::set_errno(code);
    }
    if matches!(kind, log::LogKind::SysWarning | log::LogKind::Error) {
        log::set_last_error(code);
    }
    let (level, line) = match kind {
        log::LogKind::SysWarning if SW_LOG_ERROR >= threshold => (LOG_WARNING, failed()),
        log::LogKind::Warning if LOG_WARNING >= threshold => {
            (LOG_WARNING, [origin, b"(): ", message].concat())
        }
        log::LogKind::Notice if SW_LOG_NOTICE >= threshold => (SW_LOG_NOTICE, message.to_vec()),
        log::LogKind::Info if SW_LOG_INFO >= threshold => (SW_LOG_INFO, message.to_vec()),
        log::LogKind::Error
            if level >= threshold && !swoole_rs_error_ffi::swoole_is_ignored_error(code) =>
        {
            let code = code.to_string();
            (
                level,
                [origin, b"() (ERRNO ", code.as_bytes(), b"): ", message].concat(),
            )
        }
        log::LogKind::SysError => (SW_LOG_ERROR, failed()),
        log::LogKind::Fatal => (SW_LOG_ERROR, [origin, b"(): ", message].concat()),
        _ => return,
    };
    let length = swoole_rs_error::set_last_error_msg(&line);
    if let Some(logger) = process_logger() {
        logger.put(level, line.get(..length).unwrap_or(&line));
    }
    if matches!(kind, log::LogKind::SysError | log::LogKind::Fatal) {
        crate::swoole_exit(1);
    }
}

#[cfg(test)]
mod ported;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::lent::lend_in;

    extern "C" fn redirect(_: c_int) {}

    fn identity() -> LoggerIdentity {
        LoggerIdentity {
            worker_id: 3,
            worker_pid: 0,
            worker_symbol: c_char::from_ne_bytes(*b"#"),
        }
    }

    const HOST_FAKE: LoggerHost = LoggerHost { redirect, identity };

    /// The corebase log calls this thread made since the last call. The cases that read
    /// them run in a child, so the process logger stays out of the tests that reset the record.
    fn host() -> Vec<log::capture::Logged> {
        log::capture::logged()
    }

    fn path(name: &str) -> (CString, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("swr-logger-{}-{name}", std::process::id()));
        std::fs::remove_file(&dir).unwrap_or_default();
        (CString::new(dir.to_str().unwrap()).unwrap(), dir)
    }

    /// `Logger::get_file` (or `get_real_file`), copied.
    pub(crate) fn file(logger: &Logger, real: bool) -> String {
        let state = logger.lock();
        let file = if real { &state.real_file } else { &state.file };
        file.to_string_lossy().into_owned()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fdopen")]
    fn put_writes_the_upstream_line_to_the_log_file_at_or_above_the_level() {
        let case = "utilities::logger::tests::put_writes_the_upstream_line_to_the_log_file_at_or_above_the_level";
        if crate::utilities::isolated_process::run_in_child(case) {
            return;
        }
        host();
        let logger = &Logger::new(None, HOST_FAKE);
        let (name, file_path) = path("put");
        assert!(logger.open(&name), "opened");
        assert!(logger.is_opened(), "is_opened");
        assert!(logger.set_date_format(c"%Y"), "date format");
        lend_in(b"hidden", |at, n| swr_v1_logger_put(logger, 1, at, n));
        lend_in(b"hello", |at, n| swr_v1_logger_put(logger, 4, at, n));
        logger.close();
        assert!(!logger.is_opened(), "closed");
        assert_eq!(file(logger, false), "", "close clears the file");
        assert_eq!(
            file(logger, true),
            name.to_str().unwrap(),
            "but not the real file"
        );
        let written = std::fs::read_to_string(&file_path).unwrap();
        let (year, rest) = written.strip_prefix('[').unwrap().split_at(4);
        assert!(
            year.bytes().all(|byte| byte.is_ascii_digit()),
            "%Y: {written:?}"
        );
        assert_eq!(
            rest,
            format!(" #{}.3]\tWARNING\thello\n", std::process::id()),
            "line"
        );
        std::fs::remove_file(file_path).unwrap_or_default();
        assert!(host().is_empty(), "no diagnostics");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strerror")]
    fn failures_are_logged_after_the_state_changes() {
        let case = "utilities::logger::tests::failures_are_logged_after_the_state_changes";
        if crate::utilities::isolated_process::run_in_child(case) {
            return;
        }
        host();
        let logger = &Logger::new(None, HOST_FAKE);
        unsafe {
            assert!(!logger.open(c"/nonexistent/dir/log"), "open fails");
            assert_eq!(
                host(),
                [(
                    log::LogKind::Error,
                    LOG_WARNING,
                    SYSTEM_CALL_FAIL,
                    "Logger::open".into(),
                    format!(
                        "open('/nonexistent/dir/log') failed. Error: {}[{}]",
                        CStr::from_ptr(libc::strerror(libc::ENOENT)).to_string_lossy(),
                        libc::ENOENT
                    )
                )],
                "open diagnostic"
            );
            assert!(!logger.is_opened(), "not opened");
            assert_eq!(
                (file(logger, false), file(logger, true)),
                (String::new(), String::new()),
                "paths cleared"
            );

            let long = CString::new("%Y".repeat(100)).unwrap();
            assert!(!logger.set_date_format(&long), "too long");
            assert_eq!(
                host(),
                [(
                    log::LogKind::Error,
                    LOG_WARNING,
                    INVALID_PARAMS,
                    "Logger::set_date_format".into(),
                    "The date format string[length=200] is too long".into()
                )],
                "date diagnostic"
            );

            assert!(
                !logger.redirect_stdout_and_stderr(true),
                "nothing to redirect to"
            );
            assert!(
                !logger.redirect_stdout_and_stderr(false),
                "nothing to restore"
            );
            assert_eq!(log::last_error(), INVALID_PARAMS, "last error");
            let warning = |message: &str| {
                (
                    log::LogKind::Warning,
                    0,
                    0,
                    REDIRECT_ORIGIN.to_string_lossy().into_owned(),
                    message.into(),
                )
            };
            assert_eq!(
                host(),
                [warning("no log file opened"), warning("no redirected")],
                "redirect diagnostics"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no localtime")]
    fn rotation_names_the_real_file_by_date_and_reset_restores_defaults() {
        let case = "utilities::logger::tests::rotation_names_the_real_file_by_date_and_reset_restores_defaults";
        if crate::utilities::isolated_process::run_in_child(case) {
            return;
        }
        host();
        let logger = &Logger::new(None, HOST_FAKE);
        let (name, _) = path("rotate");
        logger.set_rotation(2);
        logger.set_level(99);
        assert_eq!(logger.level(), 6, "clamped");
        assert!(logger.open(&name), "opened");
        let mut date = [0u8; 16];
        let length = rotation_date(logger.lock().rotation, &mut date);
        assert_eq!(length, 8, "daily: %Y%m%d");
        let expected = format!(
            "{}.{}",
            name.to_str().unwrap(),
            std::str::from_utf8(&date[..length]).unwrap()
        );
        assert_eq!(file(logger, true), expected, "real file");
        logger.reopen();
        assert_eq!(file(logger, true), expected, "reopened");
        logger.reset();
        assert_eq!(logger.level(), SW_LOG_INFO, "reset level");
        assert_eq!(
            rotation_date(logger.lock().rotation, &mut date),
            8,
            "reset rotation is daily too"
        );
        logger.close();
        std::fs::remove_file(expected).unwrap_or_default();
        assert!(host().is_empty(), "no diagnostics");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fdopen")]
    fn concurrent_puts_and_reopens_keep_every_line_whole() {
        let logger = Logger::new(None, HOST_FAKE);
        let (name, file_path) = path("threads");
        assert!(logger.open(&name), "opened");
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let logger = &logger;
                scope.spawn(move || {
                    for i in 0..1000 {
                        let message = format!("t{thread}-{i}");
                        lend_in(message.as_bytes(), |at, n| {
                            swr_v1_logger_put(logger, 4, at, n);
                        });
                    }
                });
            }
            let logger = &logger;
            scope.spawn(move || {
                for _ in 0..100 {
                    logger.reopen();
                }
            });
        });
        let written = std::fs::read_to_string(&file_path).unwrap();
        let mut seen: Vec<&str> = written
            .lines()
            .map(|line| line.rsplit_once("\tWARNING\t").unwrap().1)
            .collect();
        seen.sort_unstable();
        let mut expected: Vec<String> = (0..4)
            .flat_map(|t| (0..1000).map(move |i| format!("t{t}-{i}")))
            .collect();
        expected.sort_unstable();
        assert_eq!(seen, expected, "every record once, whole");
        std::fs::remove_file(file_path).unwrap_or_default();
    }

    #[test]
    fn rust_logs_run_the_macros_last_error_and_sw_error() {
        let last = || unsafe { CStr::from_ptr(swoole_rs_error::swoole_get_last_error_msg()) };
        // No logger: the level is SW_LOG_NONE, which only a level-6 error_log reaches.
        let run = |kind, level, code, origin: &CStr| {
            swr_v1_utils_log(kind, level, code, origin, c"x");
        };
        run(log::LogKind::Error, 6, 1015, c"Socket::ssl_verify");
        assert_eq!(log::last_error(), 1015);
        assert_eq!(last(), c"Socket::ssl_verify() (ERRNO 1015): x");
        run(log::LogKind::SysWarning, 0, libc::ENOENT, c"f");
        assert_eq!(
            log::last_error(),
            libc::ENOENT,
            "published, though filtered"
        );
        assert_eq!(
            last(),
            c"Socket::ssl_verify() (ERRNO 1015): x",
            "sw_error untouched"
        );
    }
}
