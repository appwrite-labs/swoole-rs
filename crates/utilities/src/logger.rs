//! `swoole::Logger` (upstream `src/core/log.cc`): level, date format, log file,
//! rotation, stdout redirection and `put`, behind one lock. Diagnostics, worker
//! identity and backtraces are native (`LoggerHost`, from `src/core/log.cc`) and
//! run after the lock is released, so a diagnostic can log through this logger
//! again.
use crate::{
    swr_v1_utils_log_level, swr_v1_utils_log_line, swr_v1_utils_log_micros,
    swr_v1_utils_log_rotation,
};
use libc::{FILE, c_char, c_int};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

const SW_LOG_INFO: c_int = 2;
const SW_LOG_DATE_STRLEN: usize = 128;
const SW_LOG_BUFFER_SIZE: usize = 16384 + 256;
const SW_LOG_DEFAULT_DATE_FORMAT: &CStr = c"%F %T";

/// What `LoggerHost::effect` reports, with its `value` and `text`.
mod host {
    use libc::c_int;
    /// errno, the real file.
    pub(super) const OPEN_FAILED: c_int = 0;
    /// The format's length.
    pub(super) const DATE_FORMAT_TOO_LONG: c_int = 1;
    pub(super) const NO_LOG_FILE: c_int = 2;
    pub(super) const ALREADY_REDIRECTED: c_int = 3;
    /// errno.
    pub(super) const DUP_STDOUT_FAILED: c_int = 4;
    pub(super) const DUP_STDERR_FAILED: c_int = 5;
    pub(super) const NOT_REDIRECTED: c_int = 6;
    pub(super) const DUP2_STDOUT_FAILED: c_int = 7;
    pub(super) const DUP2_STDERR_FAILED: c_int = 8;
    /// `swoole_redirect_stdout(value)`.
    pub(super) const REDIRECT: c_int = 9;
    pub(super) const BACKTRACE: c_int = 10;
}

#[repr(C)]
#[derive(Default)]
pub struct LoggerIdentity {
    pub worker_id: c_int,
    pub worker_pid: c_int,
    pub worker_symbol: c_char,
}

// POSIX stream locks, which libc omits on Darwin.
unsafe extern "C" {
    fn flockfile(stream: *mut FILE);
    fn funlockfile(stream: *mut FILE);
}

/// The native effects a logger asks for.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoggerHost {
    /// A diagnostic or effect from `host`; `text` is null or NUL-terminated.
    pub effect: unsafe extern "C" fn(what: c_int, value: c_int, text: *const c_char),
    pub identity: unsafe extern "C" fn(identity: *mut LoggerIdentity),
}

/// A native effect, run once the lock is released.
struct Effect(c_int, c_int, Option<CString>);

struct State {
    opened: bool,
    redirected: bool,
    backtrace: bool,
    microseconds: bool,
    rotation: c_int,
    stdout_fd: c_int,
    stderr_fd: c_int,
    fp: *mut FILE,
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

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
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

/// `std::strftime(out, format, std::localtime(&time))`.
fn strftime(out: &mut [u8], format: &CStr, time: libc::time_t) -> usize {
    // SAFETY: localtime returns its static record or null; it is read at once, as upstream did.
    let tm = unsafe { libc::localtime(&raw const time) };
    if tm.is_null() {
        return 0;
    }
    // SAFETY: `out` is writable for its length, `format` is NUL-terminated and `tm` is live.
    unsafe { libc::strftime(out.as_mut_ptr().cast(), out.len(), format.as_ptr(), tm) }
}

/// `Logger::gen_real_file`'s date suffix for `rotation`.
fn rotation_date(rotation: c_int, out: &mut [u8; 16]) -> usize {
    // SAFETY: a static NUL-terminated format.
    let format = unsafe { CStr::from_ptr(swr_v1_utils_log_rotation(rotation)) };
    strftime(out, format, now().0)
}

fn real_file(rotation: c_int, file: &CStr) -> CString {
    let mut date = [0u8; 16];
    let length = rotation_date(rotation, &mut date);
    let mut path = file.to_bytes().to_vec();
    path.push(b'.');
    path.extend(date.iter().take(length));
    CString::new(path).unwrap_or_default()
}

impl State {
    fn close(&mut self) {
        if self.opened {
            // SAFETY: the stream `open` created, closed once.
            unsafe { libc::fclose(self.fp) };
            self.fp = self.stdout;
            self.file = CString::default();
            self.opened = false;
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
        // SAFETY: a NUL-terminated path.
        let fd = unsafe {
            libc::open(
                self.real_file.as_ptr(),
                libc::O_APPEND | libc::O_RDWR | libc::O_CREAT,
                0o666,
            )
        };
        if fd < 0 {
            effects.push(Effect(
                host::OPEN_FAILED,
                errno(),
                Some(std::mem::take(&mut self.real_file)),
            ));
            self.file = CString::default();
            return false;
        }
        self.opened = true;
        // SAFETY: a descriptor this logger now owns.
        self.fp = unsafe { libc::fdopen(fd, c"a".as_ptr()) };
        true
    }

    fn reopen(&mut self, effects: &mut Vec<Effect>) {
        if !self.opened {
            return;
        }
        let file = self.file.clone();
        self.close();
        self.open(file, effects);
        if self.redirected {
            // SAFETY: the live log stream.
            effects.push(Effect(
                host::REDIRECT,
                unsafe { libc::fileno(self.fp) },
                None,
            ));
        }
    }

    fn redirect(&mut self, effects: &mut Vec<Effect>) -> bool {
        if !self.opened {
            effects.push(Effect(host::NO_LOG_FILE, 0, None));
            return false;
        }
        if self.redirected {
            effects.push(Effect(host::ALREADY_REDIRECTED, 0, None));
            return false;
        }
        // SAFETY: dup has no memory preconditions.
        self.stdout_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if self.stdout_fd < 0 {
            effects.push(Effect(host::DUP_STDOUT_FAILED, errno(), None));
            return false;
        }
        // SAFETY: as above.
        self.stderr_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        if self.stderr_fd < 0 {
            effects.push(Effect(host::DUP_STDERR_FAILED, errno(), None));
            return false;
        }
        // SAFETY: the live log stream.
        effects.push(Effect(
            host::REDIRECT,
            unsafe { libc::fileno(self.fp) },
            None,
        ));
        self.redirected = true;
        true
    }

    fn restore(&mut self, effects: &mut Vec<Effect>) -> bool {
        if !self.redirected {
            effects.push(Effect(host::NOT_REDIRECTED, 0, None));
            return false;
        }
        // SAFETY: dup2 has no memory preconditions.
        if unsafe { libc::dup2(self.stdout_fd, libc::STDOUT_FILENO) } < 0 {
            effects.push(Effect(host::DUP2_STDOUT_FAILED, errno(), None));
        }
        // SAFETY: as above.
        if unsafe { libc::dup2(self.stderr_fd, libc::STDERR_FILENO) } < 0 {
            effects.push(Effect(host::DUP2_STDERR_FAILED, errno(), None));
        }
        // SAFETY: the descriptors `redirect` duplicated, closed once.
        unsafe { libc::close(self.stdout_fd) };
        // SAFETY: as above.
        unsafe { libc::close(self.stderr_fd) };
        self.stdout_fd = -1;
        self.stderr_fd = -1;
        self.redirected = false;
        true
    }

    fn write(
        &mut self,
        level: c_int,
        content: *const u8,
        length: usize,
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
            let rest = date.get_mut(length_date..).unwrap_or_default();
            // SAFETY: `rest` is writable for its length.
            length_date +=
                unsafe { swr_v1_utils_log_micros(rest.as_mut_ptr(), rest.len(), micros) };
        }
        let mut line = vec![0u8; SW_LOG_BUFFER_SIZE];
        // SAFETY: disjoint buffers; `content` is readable for `length` bytes or through its NUL,
        // as upstream's `%.*s` with `static_cast<int>(length)` read it.
        let mut n = unsafe {
            swr_v1_utils_log_line(
                line.as_mut_ptr(),
                line.len(),
                date.as_ptr(),
                i32::try_from(length_date).unwrap_or(i32::MAX),
                u8::from_ne_bytes(identity.worker_symbol.to_ne_bytes()),
                identity.worker_pid,
                identity.worker_id,
                level,
                content,
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_possible_wrap,
                    reason = "upstream's static_cast<int>(length)"
                )]
                {
                    length as i32
                },
            )
        };
        if n == usize::MAX {
            n = 0;
        }
        if self.opened {
            // SAFETY: the live log stream.
            unsafe { flockfile(self.fp) };
        }
        // SAFETY: `line` holds `n` bytes; the stream is live.
        unsafe { libc::fwrite(line.as_ptr().cast(), n, 1, self.fp) };
        // SAFETY: as above.
        unsafe { libc::fflush(self.fp) };
        if self.opened {
            // SAFETY: locked above.
            unsafe { funlockfile(self.fp) };
        }
        if self.backtrace {
            effects.push(Effect(host::BACKTRACE, 0, None));
        }
    }
}

impl Logger {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `operation` under the lock, then its native effects.
    fn with<T>(&self, operation: impl FnOnce(&mut State, &mut Vec<Effect>) -> T) -> T {
        let mut effects = Vec::new();
        let result = operation(&mut self.lock(), &mut effects);
        for Effect(what, value, text) in effects {
            let text = text.as_deref().map_or(std::ptr::null(), CStr::as_ptr);
            // SAFETY: `text` is null or NUL-terminated and outlives the call.
            unsafe { (self.host.effect)(what, value, text) };
        }
        result
    }
}

/// `new Logger()` writing to `stdout`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_logger_new(stdout_stream: *mut FILE, host: LoggerHost) -> *mut Logger {
    Box::into_raw(Box::new(Logger {
        host,
        level: AtomicI32::new(SW_LOG_INFO),
        state: Mutex::new(State {
            opened: false,
            redirected: false,
            backtrace: false,
            microseconds: false,
            rotation: 0,
            stdout_fd: -1,
            stderr_fd: -1,
            fp: stdout_stream,
            stdout: stdout_stream,
            date_format: SW_LOG_DEFAULT_DATE_FORMAT.into(),
            file: CString::default(),
            real_file: CString::default(),
        }),
    }))
}

/// # Safety
/// `logger` came from `swr_v1_logger_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_free(logger: *mut Logger) {
    // SAFETY: the caller's unique logger.
    drop(unsafe { Box::from_raw(logger) });
}

/// The live logger behind a native `Logger`.
unsafe fn live<'a>(logger: *const Logger) -> &'a Logger {
    // SAFETY: the caller passes a logger from `swr_v1_logger_new` that is still live.
    unsafe { &*logger }
}

/// `Logger::open`.
/// # Safety
/// `logger` is live and `file` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_open(logger: *const Logger, file: *const c_char) -> bool {
    // SAFETY: a NUL-terminated path.
    let file = unsafe { CStr::from_ptr(file) }.to_owned();
    // SAFETY: per the contract.
    unsafe { live(logger) }.with(|state, effects| state.open(file, effects))
}

/// `Logger::reopen`.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_reopen(logger: *const Logger) {
    // SAFETY: per the contract.
    unsafe { live(logger) }.with(State::reopen);
}

/// `Logger::close`.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_close(logger: *const Logger) {
    // SAFETY: per the contract.
    unsafe { live(logger) }.lock().close();
}

/// `Logger::set_stream`.
/// # Safety
/// `logger` is live and `stream` is a stream the caller keeps open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_set_stream(logger: *const Logger, stream: *mut FILE) {
    // SAFETY: per the contract.
    let mut state = unsafe { live(logger) }.lock();
    state.close();
    state.fp = stream;
}

/// `Logger::put`: the line, then the backtrace if one was asked for.
/// # Safety
/// `logger` is live; `content` is readable as upstream's `%.*s` reads it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_put(
    logger: *const Logger,
    level: c_int,
    content: *const c_char,
    length: usize,
) {
    // SAFETY: per the contract.
    let logger = unsafe { live(logger) };
    if level < logger.level.load(Relaxed) {
        return;
    }
    let mut identity = LoggerIdentity::default();
    // SAFETY: a writable record.
    unsafe { (logger.host.identity)(&raw mut identity) };
    if identity.worker_pid == 0 {
        // SAFETY: getpid has no preconditions.
        identity.worker_pid = unsafe { libc::getpid() };
    }
    logger.with(|state, effects| state.write(level, content.cast(), length, &identity, effects));
}

/// `Logger::get_level`.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_level(logger: *const Logger) -> c_int {
    // SAFETY: per the contract.
    unsafe { live(logger) }.level.load(Relaxed)
}

/// `Logger::set_level`, clamped to the defined levels.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_set_level(logger: *const Logger, level: c_int) {
    // SAFETY: per the contract.
    unsafe { live(logger) }
        .level
        .store(swr_v1_utils_log_level(level), Relaxed);
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
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_set(
    logger: *const Logger,
    setting: LoggerSetting,
    value: c_int,
) {
    // SAFETY: per the contract.
    let logger = unsafe { live(logger) };
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
/// `logger` is live and `format` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_set_date_format(
    logger: *const Logger,
    format: *const c_char,
) -> bool {
    // SAFETY: a NUL-terminated format.
    let format = unsafe { CStr::from_ptr(format) };
    // SAFETY: per the contract.
    unsafe { live(logger) }.with(|state, effects| {
        if strftime(&mut [0u8; SW_LOG_DATE_STRLEN], format, now().0) == 0 {
            let length = c_int::try_from(format.to_bytes().len()).unwrap_or(c_int::MAX);
            effects.push(Effect(host::DATE_FORMAT_TOO_LONG, length, None));
            return false;
        }
        format.clone_into(&mut state.date_format);
        true
    })
}

/// `Logger::redirect_stdout_and_stderr`.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_redirect(logger: *const Logger, enable: bool) -> bool {
    // SAFETY: per the contract.
    unsafe { live(logger) }.with(|state, effects| {
        if enable {
            state.redirect(effects)
        } else {
            state.restore(effects)
        }
    })
}

/// `Logger::is_opened`.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_is_opened(logger: *const Logger) -> bool {
    // SAFETY: per the contract.
    unsafe { live(logger) }.lock().opened
}

/// `Logger::get_file` (0) or `get_real_file` (1): valid until the file next changes.
/// # Safety
/// `logger` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_file(logger: *const Logger, real: bool) -> *const c_char {
    // SAFETY: per the contract.
    let state = unsafe { live(logger) }.lock();
    if real {
        state.real_file.as_ptr()
    } else {
        state.file.as_ptr()
    }
}

/// `Logger::gen_real_file`'s date suffix, without the dot; returns its length.
/// # Safety
/// `logger` is live and `date` is writable for 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_logger_rotation_date(
    logger: *const Logger,
    date: *mut [u8; 16],
) -> usize {
    // SAFETY: per the contract.
    let rotation = unsafe { live(logger) }.lock().rotation;
    // SAFETY: as above.
    rotation_date(rotation, unsafe { &mut *date })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static HOST: RefCell<Vec<(c_int, c_int, Option<String>)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn effect(what: c_int, value: c_int, text: *const c_char) {
        let text = (!text.is_null()).then(|| {
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        });
        HOST.with(|host| host.borrow_mut().push((what, value, text)));
    }

    unsafe extern "C" fn identity(identity: *mut LoggerIdentity) {
        unsafe {
            (*identity).worker_id = 3;
            (*identity).worker_symbol = c_char::from_ne_bytes(*b"#");
        }
    }

    const HOST_FAKE: LoggerHost = LoggerHost { effect, identity };

    fn host() -> Vec<(c_int, c_int, Option<String>)> {
        HOST.with(RefCell::take)
    }

    fn path(name: &str) -> (CString, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("swr-logger-{}-{name}", std::process::id()));
        std::fs::remove_file(&dir).ok();
        (CString::new(dir.to_str().unwrap()).unwrap(), dir)
    }

    fn file(logger: *const Logger, real: bool) -> String {
        unsafe { CStr::from_ptr(swr_v1_logger_file(logger, real)) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn put_writes_the_upstream_line_to_the_log_file_at_or_above_the_level() {
        let logger = swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE);
        let (name, file_path) = path("put");
        unsafe {
            assert!(swr_v1_logger_open(logger, name.as_ptr()), "opened");
            assert!(swr_v1_logger_is_opened(logger), "is_opened");
            assert!(
                swr_v1_logger_set_date_format(logger, c"%Y".as_ptr()),
                "date format"
            );
            swr_v1_logger_put(logger, 1, c"hidden".as_ptr(), 6);
            swr_v1_logger_put(logger, 4, c"hello world".as_ptr(), 5);
            swr_v1_logger_close(logger);
            assert!(!swr_v1_logger_is_opened(logger), "closed");
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
    fn failures_are_reported_to_the_host_after_the_state_changes() {
        let logger = swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE);
        unsafe {
            assert!(
                !swr_v1_logger_open(logger, c"/nonexistent/dir/log".as_ptr()),
                "open fails"
            );
            assert_eq!(
                host(),
                [(
                    host::OPEN_FAILED,
                    libc::ENOENT,
                    Some("/nonexistent/dir/log".into())
                )],
                "open diagnostic"
            );
            assert!(!swr_v1_logger_is_opened(logger), "not opened");
            assert_eq!(
                (file(logger, false), file(logger, true)),
                (String::new(), String::new()),
                "paths cleared"
            );

            let long = CString::new("%Y".repeat(100)).unwrap();
            assert!(
                !swr_v1_logger_set_date_format(logger, long.as_ptr()),
                "too long"
            );
            assert_eq!(
                host(),
                [(host::DATE_FORMAT_TOO_LONG, 200, None)],
                "date diagnostic"
            );

            assert!(
                !swr_v1_logger_redirect(logger, true),
                "nothing to redirect to"
            );
            assert!(!swr_v1_logger_redirect(logger, false), "nothing to restore");
            assert_eq!(
                host(),
                [
                    (host::NO_LOG_FILE, 0, None),
                    (host::NOT_REDIRECTED, 0, None)
                ],
                "redirect diagnostics"
            );
            swr_v1_logger_free(logger);
        }
    }

    #[test]
    fn rotation_names_the_real_file_by_date_and_reset_restores_defaults() {
        let logger = swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE);
        let (name, _) = path("rotate");
        unsafe {
            swr_v1_logger_set(logger, LoggerSetting::Rotation, 2);
            swr_v1_logger_set_level(logger, 99);
            assert_eq!(swr_v1_logger_level(logger), 6, "clamped");
            assert!(swr_v1_logger_open(logger, name.as_ptr()), "opened");
            let mut date = [0u8; 16];
            let length = swr_v1_logger_rotation_date(logger, &raw mut date);
            assert_eq!(length, 8, "daily: %Y%m%d");
            let expected = format!(
                "{}.{}",
                name.to_str().unwrap(),
                std::str::from_utf8(&date[..length]).unwrap()
            );
            assert_eq!(file(logger, true), expected, "real file");
            swr_v1_logger_reopen(logger);
            assert_eq!(file(logger, true), expected, "reopened");
            swr_v1_logger_set(logger, LoggerSetting::Reset, 0);
            assert_eq!(swr_v1_logger_level(logger), SW_LOG_INFO, "reset level");
            assert_eq!(
                swr_v1_logger_rotation_date(logger, &raw mut date),
                8,
                "reset rotation is daily too"
            );
            swr_v1_logger_close(logger);
            swr_v1_logger_free(logger);
            std::fs::remove_file(expected).ok();
        }
        assert!(host().is_empty(), "no diagnostics");
    }

    #[test]
    fn concurrent_puts_and_reopens_keep_every_line_whole() {
        struct Shared(*mut Logger);
        // SAFETY: the logger is thread-safe for put and reopen.
        unsafe impl Sync for Shared {}
        let logger = Shared(swr_v1_logger_new(std::ptr::null_mut(), HOST_FAKE));
        let (name, file_path) = path("threads");
        unsafe { assert!(swr_v1_logger_open(logger.0, name.as_ptr()), "opened") };
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let logger = &logger;
                scope.spawn(move || {
                    for i in 0..1000 {
                        let message = CString::new(format!("t{thread}-{i}")).unwrap();
                        let length = message.as_bytes().len();
                        unsafe { swr_v1_logger_put(logger.0, 4, message.as_ptr(), length) };
                    }
                });
            }
            let logger = &logger;
            scope.spawn(move || {
                for _ in 0..100 {
                    unsafe { swr_v1_logger_reopen(logger.0) };
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
}
