//! `swoole::Logger` (upstream `src/core/log.cc`): level, date format, log file,
//! rotation, stdout redirection and `put`, behind one lock. Diagnostics go out
//! through corebase's log host; worker identity, `swoole_redirect_stdout` and
//! backtraces go through `LoggerHost` (`swr_logger_host`, in crates/process). Both run after
//! the lock is released, so a diagnostic can log through this logger again. The log
//! macros a Rust log runs (`swr_v1_utils_log`) are here too.
use crate::{log_bytes, log_level, log_line, log_micros, log_rotation};
use libc::{FILE, c_char, c_int};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};
use swoole_rs_corebase::log;
use swoole_rs_corebase::sys::{self, errno};

const SW_LOG_INFO: c_int = 2;
const SW_LOG_NOTICE: c_int = 3;
const SW_LOG_ERROR: c_int = 5;
const SW_LOG_DATE_STRLEN: usize = 128;
const SW_LOG_BUFFER_SIZE: usize = 16384 + 256;
const SW_LOG_DEFAULT_DATE_FORMAT: &CStr = c"%F %T";

/// `SW_LOG_WARNING`, `SW_ERROR_SYSTEM_CALL_FAIL` and `SW_ERROR_INVALID_PARAMS`.
const LOG_WARNING: c_int = 4;
const SYSTEM_CALL_FAIL: c_int = 502;
const INVALID_PARAMS: c_int = 505;
const REDIRECT_ORIGIN: &CStr = c"Logger::redirect_stdout_and_stderr";

#[repr(C)]
#[derive(Default)]
pub struct LoggerIdentity {
    pub worker_id: c_int,
    pub worker_pid: c_int,
    pub worker_symbol: c_char,
}

/// The native effects a logger asks for.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoggerHost {
    /// `swoole_redirect_stdout(fd)`.
    pub redirect: extern "C" fn(fd: c_int),
    pub backtrace: extern "C" fn(),
    pub identity: extern "C" fn() -> LoggerIdentity,
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
    Backtrace,
}

struct State {
    /// The descriptor `open` made, which takes each line in one `write(2)`
    /// under the lock; lines go to `stream` while there is none.
    file_fd: Option<c_int>,
    redirected: bool,
    backtrace: bool,
    microseconds: bool,
    rotation: c_int,
    stdout_fd: c_int,
    stderr_fd: c_int,
    /// `stdout`, or the stream `set_stream` gave.
    stream: *mut FILE,
    stdout: *mut FILE,
    date_format: CString,
    file: CString,
    real_file: CString,
}

// SAFETY: the streams are only used under the logger's lock.
unsafe impl Send for State {}

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

/// `std::localtime(&time)`: its static record, or null; read it at once, as upstream did.
fn localtime(time: libc::time_t) -> *const libc::tm {
    // SAFETY: localtime has no memory preconditions beyond its argument.
    unsafe { libc::localtime(&raw const time) }
}

/// `std::strftime(out, format, std::localtime(&time))`.
fn strftime(out: &mut [u8], format: &CStr, time: libc::time_t) -> usize {
    let tm = localtime(time);
    if tm.is_null() {
        return 0;
    }
    // SAFETY: `out` is writable for its length, `format` is NUL-terminated and `tm` is live.
    unsafe { libc::strftime(out.as_mut_ptr().cast(), out.len(), format.as_ptr(), tm) }
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
        let Ok(written) = usize::try_from(sys::write(fd, bytes)) else {
            return;
        };
        if written == 0 {
            return;
        }
        bytes = bytes.get(written..).unwrap_or_default();
    }
}

impl State {
    fn close(&mut self) {
        if let Some(fd) = self.file_fd.take() {
            sys::close(fd);
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
        let fd = sys::open(
            &self.real_file,
            libc::O_APPEND | libc::O_RDWR | libc::O_CREAT,
            0o666,
        );
        if fd < 0 {
            effects.push(Effect::OpenFailed(
                errno(),
                std::mem::take(&mut self.real_file),
            ));
            self.file = CString::default();
            return false;
        }
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
        self.stdout_fd = sys::dup(libc::STDOUT_FILENO);
        if self.stdout_fd < 0 {
            effects.push(Effect::SysWarning("dup(STDOUT_FILENO) failed", errno()));
            return false;
        }
        self.stderr_fd = sys::dup(libc::STDERR_FILENO);
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
        if sys::dup2(self.stdout_fd, libc::STDOUT_FILENO) < 0 {
            effects.push(Effect::SysWarning("dup2(STDOUT_FILENO) failed", errno()));
        }
        if sys::dup2(self.stderr_fd, libc::STDERR_FILENO) < 0 {
            effects.push(Effect::SysWarning("dup2(STDERR_FILENO) failed", errno()));
        }
        // The descriptors `redirect` duplicated, closed once.
        sys::close(self.stdout_fd);
        sys::close(self.stderr_fd);
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
        } else {
            // SAFETY: `line` is readable for its length; the stream is the caller's live one.
            unsafe { libc::fwrite(line.as_ptr().cast(), line.len(), 1, self.stream) };
            // SAFETY: as above.
            unsafe { libc::fflush(self.stream) };
        }
        if self.backtrace {
            effects.push(Effect::Backtrace);
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
                let reason = reason.strip_suffix(&suffix).unwrap_or(&reason);
                log::error_log(
                    LOG_WARNING,
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
                    LOG_WARNING,
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
            Effect::Backtrace => (self.host.backtrace)(),
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
    /// `new Logger()` writing to `stdout_stream`.
    #[must_use]
    pub fn new(stdout_stream: *mut FILE, host: LoggerHost) -> Self {
        Self {
            host,
            level: AtomicI32::new(SW_LOG_INFO),
            state: Mutex::new(State {
                file_fd: None,
                redirected: false,
                backtrace: false,
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

    /// `Logger::put`: `content` through its first NUL, as upstream's `%.*s`
    /// reads it, then the backtrace if one was asked for.
    pub fn put(&self, level: c_int, content: &[u8]) {
        if level < self.level.load(Relaxed) {
            return;
        }
        let mut identity = (self.host.identity)();
        if identity.worker_pid == 0 {
            identity.worker_pid = swoole_rs_corebase::sys::getpid();
        }
        self.with(|state, effects| state.write(level, content, &identity, effects));
    }
}

/// `new Logger()` writing to `stdout`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_new(stdout_stream: *mut FILE, host: LoggerHost) -> *mut Logger {
    Box::into_raw(Box::new(Logger::new(stdout_stream, host)))
}

/// # Safety
/// `logger` came from `swr_v1_logger_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_free(logger: *mut Logger) {
    // SAFETY: the caller's unique logger.
    drop(unsafe { Box::from_raw(logger) });
}

/// `Logger::open`.
/// # Safety
/// `file` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_open(logger: &Logger, file: *const c_char) -> bool {
    // SAFETY: a NUL-terminated path.
    let file = unsafe { CStr::from_ptr(file) }.to_owned();
    logger.with(|state, effects| state.open(file, effects))
}

/// `Logger::reopen`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_reopen(logger: &Logger) {
    logger.reopen();
}

/// `Logger::close`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_close(logger: &Logger) {
    logger.close();
}

/// `Logger::set_stream`.
/// # Safety
/// `stream` is a stream the caller keeps open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_set_stream(logger: &Logger, stream: *mut FILE) {
    let mut state = logger.lock();
    state.close();
    state.stream = stream;
}

/// [`Logger::put`].
/// # Safety
/// `content` is readable as upstream's `%.*s` with `static_cast<int>(length)` reads it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_put(
    logger: &Logger,
    level: c_int,
    content: *const c_char,
    length: usize,
) {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "upstream's static_cast<int>(length)"
    )]
    let precision = length as i32;
    // SAFETY: per the contract.
    logger.put(level, unsafe { log_bytes(content.cast(), precision) });
}

/// `Logger::get_level`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_level(logger: &Logger) -> c_int {
    logger.level.load(Relaxed)
}

/// `Logger::set_level`, clamped to the defined levels.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_set_level(logger: &Logger, level: c_int) {
    logger.level.store(log_level(level), Relaxed);
}

/// Logger settings `swr_v1_logger_set` changes.
#[repr(C)]
pub enum LoggerSetting {
    Rotation = 0,
    Microseconds = 1,
    Backtrace = 2,
    /// `Logger::reset`: the date format, microseconds, rotation and level.
    Reset = 3,
}

/// `Logger::set_rotation`, `set_date_with_microseconds`, `display_backtrace` and `reset`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_set(logger: &Logger, setting: LoggerSetting, value: c_int) {
    let mut state = logger.lock();
    match setting {
        LoggerSetting::Rotation => state.rotation = value,
        LoggerSetting::Microseconds => state.microseconds = value != 0,
        LoggerSetting::Backtrace => state.backtrace = true,
        LoggerSetting::Reset => {
            state.date_format = SW_LOG_DEFAULT_DATE_FORMAT.into();
            state.microseconds = false;
            state.rotation = 0;
            logger.level.store(SW_LOG_INFO, Relaxed);
        }
    }
}

/// `Logger::set_date_format`: false, with a warning, if the format renders nothing.
/// # Safety
/// `format` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_set_date_format(
    logger: &Logger,
    format: *const c_char,
) -> bool {
    // Upstream hands `format` to strftime unmeasured, which stops reading once the date
    // overflows, and measures it only to keep it or for a warning that logs: its own
    // test passes an unterminated one.
    let tm = localtime(now().0);
    let mut date = [0u8; SW_LOG_DATE_STRLEN];
    let fits = !tm.is_null()
        // SAFETY: `date` is writable for its length, `tm` is live and strftime reads
        // `format` only as far as upstream's did.
        && unsafe { libc::strftime(date.as_mut_ptr().cast(), date.len(), format, tm) } != 0;
    if !fits && LOG_WARNING < logger.level.load(Relaxed) {
        log::set_last_error(INVALID_PARAMS);
        return false;
    }
    // SAFETY: NUL-terminated wherever upstream measured it: kept, or in a warning that logs.
    let format = unsafe { CStr::from_ptr(format) };
    logger.with(|state, effects| {
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_redirect(logger: &Logger, enable: bool) -> bool {
    logger.with(|state, effects| {
        if enable {
            state.redirect(effects)
        } else {
            state.restore(effects)
        }
    })
}

/// `Logger::is_opened`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_is_opened(logger: &Logger) -> bool {
    logger.lock().file_fd.is_some()
}

/// `Logger::get_file` (0) or `get_real_file` (1): valid until the file next changes.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_file(logger: &Logger, real: bool) -> *const c_char {
    let state = logger.lock();
    if real {
        state.real_file.as_ptr()
    } else {
        state.file.as_ptr()
    }
}

/// `Logger::gen_real_file`'s date suffix, without the dot; returns its length.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_rotation_date(logger: &Logger, date: &mut [u8; 16]) -> usize {
    rotation_date(logger.lock().rotation, date)
}

/// The process logger, `sw_logger()`: a `swoole::Logger`, whose only member is
/// this crate's `Logger`, or null before `swoole_init`.
#[must_use]
pub fn process_logger() -> Option<&'static Logger> {
    let cpp = swoole_rs_corebase::swr_v1_core_logger().cast::<*const Logger>();
    // SAFETY: a live `swoole::Logger` whenever set.
    let member = unsafe { cpp.as_ref() }?;
    // SAFETY: its only member is a live Logger.
    Some(unsafe { &**member })
}

/// `void swoole_set_log_level(int level)` (upstream `src/core/base.cc`).
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_set_log_leveli")
)]
pub extern "C" fn swoole_set_log_level(level: c_int) {
    if let Some(logger) = process_logger() {
        logger.level.store(log_level(level), Relaxed);
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
    process_logger().map_or(SW_LOG_NONE, |logger| logger.level.load(Relaxed))
}

/// `void swoole_set_log_file(const char *file)`.
/// # Safety
/// `file` is a NUL-terminated path.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_set_log_filePKc")
)]
pub unsafe extern "C" fn swoole_set_log_file(file: *const c_char) {
    if let Some(logger) = process_logger() {
        // SAFETY: a live logger and, per the contract, a NUL-terminated path.
        unsafe { swr_v1_logger_open(logger, file) };
    }
}

/// The log macros (`swoole_log.h`) for a log Rust emits on behalf of `origin`, of
/// corebase's `LOG_*` `kind` (crates/corebase/src/log.rs): the last error, the level
/// check, the line formatted into `sw_error`, the put and, for the errors, the exit.
/// `swoole_init` registers it as corebase's log host.
///
/// # Safety
/// `origin` and `message` are NUL-terminated.
pub unsafe extern "C" fn swr_v1_utils_log(
    kind: c_int,
    level: c_int,
    code: c_int,
    origin: *const c_char,
    message: *const c_char,
) {
    // SAFETY: per the contract.
    let origin = unsafe { CStr::from_ptr(origin) }.to_bytes();
    // SAFETY: as above.
    let message = unsafe { CStr::from_ptr(message) }.to_bytes();
    let threshold = swoole_get_log_level();
    let failed = || {
        // SAFETY: swoole_strerror returns a NUL-terminated message.
        let reason = unsafe { CStr::from_ptr(swoole_rs_error_ffi::swoole_strerror(code)) };
        let code = code.to_string();
        let tail = [b", Error: ", reason.to_bytes(), b"[", code.as_bytes(), b"]"];
        [origin, b"(): ", message, &tail.concat()].concat()
    };
    if matches!(kind, log::LOG_SYS_WARNING | log::LOG_SYS_ERROR) {
        swoole_rs_corebase::set_errno(code);
    }
    if matches!(kind, log::LOG_SYS_WARNING | log::LOG_ERROR) {
        log::set_last_error(code);
    }
    let (level, line) = match kind {
        log::LOG_SYS_WARNING if SW_LOG_ERROR >= threshold => (LOG_WARNING, failed()),
        log::LOG_WARNING if LOG_WARNING >= threshold => {
            (LOG_WARNING, [origin, b"(): ", message].concat())
        }
        log::LOG_NOTICE if SW_LOG_NOTICE >= threshold => (SW_LOG_NOTICE, message.to_vec()),
        log::LOG_INFO if SW_LOG_INFO >= threshold => (SW_LOG_INFO, message.to_vec()),
        log::LOG_ERROR
            if level >= threshold && !swoole_rs_error_ffi::swoole_is_ignored_error(code) =>
        {
            let code = code.to_string();
            (
                level,
                [origin, b"() (ERRNO ", code.as_bytes(), b"): ", message].concat(),
            )
        }
        log::LOG_SYS_ERROR => (SW_LOG_ERROR, failed()),
        log::LOG_FATAL => (SW_LOG_ERROR, [origin, b"(): ", message].concat()),
        _ => return,
    };
    let length = swoole_rs_error::set_last_error_msg(&line);
    if let Some(logger) = process_logger() {
        logger.put(level, line.get(..length).unwrap_or(&line));
    }
    if matches!(kind, log::LOG_SYS_ERROR | log::LOG_FATAL) {
        swoole_rs_corebase::swoole_exit(1);
    }
}

#[cfg(test)]
mod ported;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A corebase log call: kind, level, code, origin and message.
    type Logged = (c_int, c_int, c_int, String, String);

    thread_local! {
        static HOST: RefCell<Vec<Logged>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn logged(
        kind: c_int,
        level: c_int,
        code: c_int,
        origin: *const c_char,
        message: *const c_char,
    ) {
        let text = |p| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        let entry = (kind, level, code, text(origin), text(message));
        HOST.with(|host| host.borrow_mut().push(entry));
    }

    extern "C" fn redirect(_: c_int) {}
    extern "C" fn backtrace() {}

    extern "C" fn identity() -> LoggerIdentity {
        LoggerIdentity {
            worker_id: 3,
            worker_pid: 0,
            worker_symbol: c_char::from_ne_bytes(*b"#"),
        }
    }

    const HOST_FAKE: LoggerHost = LoggerHost {
        redirect,
        backtrace,
        identity,
    };

    fn host() -> Vec<Logged> {
        swoole_rs_corebase::log::swr_v1_core_set_log_host(logged);
        HOST.with(RefCell::take)
    }

    fn path(name: &str) -> (CString, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("swr-logger-{}-{name}", std::process::id()));
        std::fs::remove_file(&dir).ok();
        (CString::new(dir.to_str().unwrap()).unwrap(), dir)
    }

    fn file(logger: *const Logger, real: bool) -> String {
        unsafe { CStr::from_ptr(swr_v1_logger_file(&*logger, real)) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fdopen")]
    fn put_writes_the_upstream_line_to_the_log_file_at_or_above_the_level() {
        let logger = swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE);
        let (name, file_path) = path("put");
        unsafe {
            assert!(swr_v1_logger_open(&*logger, name.as_ptr()), "opened");
            assert!(swr_v1_logger_is_opened(&*logger), "is_opened");
            assert!(
                swr_v1_logger_set_date_format(&*logger, c"%Y".as_ptr()),
                "date format"
            );
            swr_v1_logger_put(&*logger, 1, c"hidden".as_ptr(), 6);
            swr_v1_logger_put(&*logger, 4, c"hello world".as_ptr(), 5);
            swr_v1_logger_close(&*logger);
            assert!(!swr_v1_logger_is_opened(&*logger), "closed");
            assert_eq!(file(logger, false), "", "close clears the file");
            assert_eq!(
                file(logger, true),
                name.to_str().unwrap(),
                "but not the real file"
            );
            swr_v1_logger_free(logger);
        }
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
        std::fs::remove_file(file_path).ok();
        assert!(host().is_empty(), "no diagnostics");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strerror")]
    fn failures_are_logged_after_the_state_changes() {
        let logger = swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE);
        host();
        unsafe {
            assert!(
                !swr_v1_logger_open(&*logger, c"/nonexistent/dir/log".as_ptr()),
                "open fails"
            );
            assert_eq!(
                host(),
                [(
                    log::LOG_ERROR,
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
            assert!(!swr_v1_logger_is_opened(&*logger), "not opened");
            assert_eq!(
                (file(logger, false), file(logger, true)),
                (String::new(), String::new()),
                "paths cleared"
            );

            let long = CString::new("%Y".repeat(100)).unwrap();
            assert!(
                !swr_v1_logger_set_date_format(&*logger, long.as_ptr()),
                "too long"
            );
            assert_eq!(
                host(),
                [(
                    log::LOG_ERROR,
                    LOG_WARNING,
                    INVALID_PARAMS,
                    "Logger::set_date_format".into(),
                    "The date format string[length=200] is too long".into()
                )],
                "date diagnostic"
            );

            assert!(
                !swr_v1_logger_redirect(&*logger, true),
                "nothing to redirect to"
            );
            assert!(
                !swr_v1_logger_redirect(&*logger, false),
                "nothing to restore"
            );
            assert_eq!(log::last_error(), INVALID_PARAMS, "last error");
            let warning = |message: &str| {
                (
                    log::LOG_WARNING,
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
            swr_v1_logger_free(logger);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no localtime")]
    fn rotation_names_the_real_file_by_date_and_reset_restores_defaults() {
        let logger = swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE);
        let (name, _) = path("rotate");
        unsafe {
            swr_v1_logger_set(&*logger, LoggerSetting::Rotation, 2);
            swr_v1_logger_set_level(&*logger, 99);
            assert_eq!(swr_v1_logger_level(&*logger), 6, "clamped");
            assert!(swr_v1_logger_open(&*logger, name.as_ptr()), "opened");
            let mut date = [0u8; 16];
            let length = swr_v1_logger_rotation_date(&*logger, &mut date);
            assert_eq!(length, 8, "daily: %Y%m%d");
            let expected = format!(
                "{}.{}",
                name.to_str().unwrap(),
                std::str::from_utf8(&date[..length]).unwrap()
            );
            assert_eq!(file(logger, true), expected, "real file");
            swr_v1_logger_reopen(&*logger);
            assert_eq!(file(logger, true), expected, "reopened");
            swr_v1_logger_set(&*logger, LoggerSetting::Reset, 0);
            assert_eq!(swr_v1_logger_level(&*logger), SW_LOG_INFO, "reset level");
            assert_eq!(
                swr_v1_logger_rotation_date(&*logger, &mut date),
                8,
                "reset rotation is daily too"
            );
            swr_v1_logger_close(&*logger);
            swr_v1_logger_free(logger);
            std::fs::remove_file(expected).ok();
        }
        assert!(host().is_empty(), "no diagnostics");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fdopen")]
    fn concurrent_puts_and_reopens_keep_every_line_whole() {
        struct Shared(*mut Logger);
        // SAFETY: the logger is thread-safe for put and reopen.
        unsafe impl Sync for Shared {}
        let logger = Shared(swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE));
        let (name, file_path) = path("threads");
        unsafe { assert!(swr_v1_logger_open(&*logger.0, name.as_ptr()), "opened") };
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let logger = &logger;
                scope.spawn(move || {
                    for i in 0..1000 {
                        let message = CString::new(format!("t{thread}-{i}")).unwrap();
                        let length = message.as_bytes().len();
                        unsafe { swr_v1_logger_put(&*logger.0, 4, message.as_ptr(), length) };
                    }
                });
            }
            let logger = &logger;
            scope.spawn(move || {
                for _ in 0..100 {
                    unsafe { swr_v1_logger_reopen(&*logger.0) };
                }
            });
        });
        unsafe { swr_v1_logger_free(logger.0) };
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
        std::fs::remove_file(file_path).ok();
    }

    #[test]
    fn rust_logs_run_the_macros_last_error_and_sw_error() {
        let last = || unsafe { CStr::from_ptr(swoole_rs_error::swoole_get_last_error_msg()) };
        // No logger: the level is SW_LOG_NONE, which only a level-6 error_log reaches.
        let run = |kind, level, code, origin: &CStr| unsafe {
            swr_v1_utils_log(kind, level, code, origin.as_ptr(), c"x".as_ptr());
        };
        run(log::LOG_ERROR, 6, 1015, c"Socket::ssl_verify");
        assert_eq!(log::last_error(), 1015);
        assert_eq!(last(), c"Socket::ssl_verify() (ERRNO 1015): x");
        run(log::LOG_SYS_WARNING, 0, libc::ENOENT, c"f");
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
