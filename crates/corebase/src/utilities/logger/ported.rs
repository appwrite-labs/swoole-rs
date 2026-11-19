// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 core-tests/src/core/log.cpp (Swoole contributors).
//! Upstream's `core-tests/src/core/log.cpp` TESTs, one function
//! per TEST with the same inputs and expected values, over a logger of their own.
//! `sw_logger()`'s worker identity is the host's: the worker type sets its symbol.
//! tests/modules/utilities/README.md maps each TEST.
use super::*;
use std::cell::Cell;
use std::ffi::OsStr;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::LazyLock;

use crate::utilities::{SW_LOG_DEBUG, SW_LOG_TRACE};
/// `const char *file` and `TEST_LOG_FILE`: upstream's names, in a directory of this
/// test process's own, so no other run's or user's /tmp file is appended to or unlinked.
static PATHS: LazyLock<(CString, CString)> = LazyLock::new(|| {
    let dir = std::env::temp_dir().join(format!("swoole-rs-logger-{}", std::process::id()));
    let path = |name: &str| CString::new(dir.join(name).into_os_string().into_vec()).unwrap();
    (path("swoole_log_test.log"), path("swoole.log"))
});

fn file() -> &'static CStr {
    &PATHS.0
}

fn test_log_file() -> &'static CStr {
    &PATHS.1
}

fn path(file: &CStr) -> &Path {
    Path::new(OsStr::from_bytes(file.to_bytes()))
}

// GoogleTest ran these one at a time on the same files.
static SERIAL: Mutex<()> = Mutex::new(());

thread_local! {
    /// `swoole_get_worker_symbol()`: `%` until a test sets a worker type.
    static SYMBOL: Cell<u8> = const { Cell::new(b'%') };
}

fn identity() -> LoggerIdentity {
    LoggerIdentity {
        worker_symbol: c_char::from_ne_bytes([SYMBOL.get()]),
        ..LoggerIdentity::default()
    }
}

/// `sw_logger()`, with the process's `swoole_redirect_stdout`.
fn sw_logger() -> Logger {
    let host = LoggerHost {
        redirect: crate::swoole_redirect_stdout,
        identity,
    };
    Logger::new(None, host)
}

fn put(logger: &Logger, level: c_int, text: &str) {
    logger.put(level, text.as_bytes());
}

fn open(logger: &Logger, file: &CStr) -> bool {
    // unlink removes the directory with the last file; a test that opens again recreates it.
    std::fs::create_dir_all(path(file).parent().unwrap()).unwrap();
    logger.open(file)
}

fn real_file(logger: &Logger) -> String {
    super::tests::file(logger, true)
}

/// `file_get_contents`: none for a missing or empty file.
fn file_get_contents(file: &CStr) -> Option<Vec<u8>> {
    std::fs::read(path(file))
        .ok()
        .filter(|content| !content.is_empty())
}

fn contains(content: &[u8], needle: &str) -> bool {
    memchr::memmem::find(content, needle.as_bytes()).is_some()
}

/// Also removes the process's directory once it is empty.
fn unlink(file: &Path) {
    std::fs::remove_file(file).unwrap_or_default();
    if let Some(dir) = file.parent() {
        std::fs::remove_dir(dir).unwrap_or_default();
    }
}

/// `access(file, R_OK)`.
fn access(file: &str) -> c_int {
    let file = CString::new(file).unwrap();
    unsafe { libc::access(file.as_ptr(), libc::R_OK) }
}

/// `printf` to a line-buffered stdout, as the upstream run's terminal left it: one write.
fn printf(text: &str) {
    // SAFETY: `text` is readable for its length.
    unsafe { libc::write(libc::STDOUT_FILENO, text.as_ptr().cast(), text.len()) };
}

#[test]
fn level() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let logger = &sw_logger();
    // SW_MASTER, SW_MANAGER, SW_WORKER and SW_TASK_WORKER.
    for symbol in *b"#$*^" {
        SYMBOL.set(symbol);
        logger.reset();

        assert!(!logger.is_opened());

        logger.set_level(999);
        assert_eq!(logger.level(), SW_LOG_NONE);

        logger.set_level(SW_LOG_DEBUG - 10);
        assert_eq!(logger.level(), SW_LOG_DEBUG);

        logger.set_level(SW_LOG_NOTICE);
        open(logger, file());

        assert!(logger.is_opened());

        put(logger, SW_LOG_DEBUG, "hello no debug");
        put(logger, SW_LOG_TRACE, "hello no trace");
        put(logger, SW_LOG_INFO, "hello info");
        put(logger, SW_LOG_NOTICE, "hello notice");
        put(logger, LOG_WARNING, "hello warning");

        logger.set_level(SW_LOG_DEBUG);
        put(logger, SW_LOG_DEBUG, "hello debug");
        put(logger, SW_LOG_TRACE, "hello trace");

        let content = file_get_contents(file()).unwrap();

        logger.close();
        unlink(path(file()));

        assert!(!contains(&content, "hello no debug"));
        assert!(!contains(&content, "hello no trace"));
        assert!(contains(&content, "hello debug"));
        assert!(contains(&content, "hello trace"));
        assert!(!contains(&content, "hello info"));
        assert!(contains(&content, "hello notice"));
        assert!(contains(&content, "hello warning"));
    }
    SYMBOL.set(b'%');
}

#[test]
fn date_format_long_string() {
    let logger = &sw_logger();
    logger.reset();
    logger.set_level(SW_LOG_ERROR);
    let format = "x".repeat(120) + "day %d of %B in the year %Y. Time: %I:%S %p";
    let format = CString::new(format).unwrap();

    let retval = logger.set_date_format(&format);

    assert!(!retval);
    assert_eq!(log::last_error(), INVALID_PARAMS);
}

/// `std::regex_search` of `\[\S+\s\d{2}:\d{2}:\d{2}\<\.(\d+)\>\s%\d+\.\d+\]\tWARNING\thello world`,
/// where ECMAScript reads `\<` and `\>` as `<` and `>`.
fn microsecond_line(content: &str) -> bool {
    fn digits(s: &str, exactly: Option<usize>) -> Option<&str> {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        (n > 0 && exactly.is_none_or(|exactly| n == exactly)).then(|| &s[n..])
    }
    let at = |start: usize| -> Option<()> {
        let rest = content[start..].strip_prefix('[')?;
        let (date, rest) = rest.split_once(char::is_whitespace)?;
        if date.is_empty() {
            return None;
        }
        let rest = digits(rest, Some(2))?.strip_prefix(':')?;
        let rest = digits(rest, Some(2))?.strip_prefix(':')?;
        let rest = digits(rest, Some(2))?.strip_prefix("<.")?;
        let rest = digits(rest, None)?.strip_prefix('>')?;
        let rest = rest.strip_prefix(char::is_whitespace)?.strip_prefix('%')?;
        let rest = digits(rest, None)?.strip_prefix('.')?;
        digits(rest, None)?
            .starts_with("]\tWARNING\thello world")
            .then_some(())
    };
    content
        .match_indices('[')
        .any(|(start, _)| at(start).is_some())
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no fdopen")]
fn date_with_microseconds() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let logger = &sw_logger();
    logger.reset();
    logger.set_date_with_microseconds(true);
    open(logger, file());

    put(logger, LOG_WARNING, "hello world");
    let content = file_get_contents(file()).unwrap();

    logger.close();
    unlink(path(file()));

    assert!(microsecond_line(&String::from_utf8_lossy(&content)));
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no fdopen")]
fn rotation() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let logger = &sw_logger();
    let types = [
        rotation::DAILY,
        rotation::EVERY_MINUTE,
        rotation::HOURLY,
        rotation::MONTHLY,
    ];
    for rotation in types {
        logger.reset();
        logger.set_rotation(rotation);
        open(logger, file());

        put(logger, SW_LOG_DEBUG, "hello world");
        put(logger, SW_LOG_TRACE, "hello world");
        put(logger, SW_LOG_NOTICE, "hello world");
        put(logger, LOG_WARNING, "hello world");
        put(logger, SW_LOG_ERROR, "hello world");
        put(logger, SW_LOG_INFO, "hello world");

        assert_eq!(access(&super::tests::file(logger, false)), -1);
        assert_eq!(errno(), libc::ENOENT);
        assert_eq!(access(&real_file(logger)), 0);

        logger.close();
        unlink(Path::new(&real_file(logger)));
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri can't spawn a process")]
fn redirect_1() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    // upstream's spawn_exec_and_wait: the body runs in a process of its own, a fresh
    // test binary rather than a fork of this threaded one (whose other threads may hold
    // libc locks), and fails the test unless it exits 0, where upstream's helper exited 0
    // anyway.
    if crate::utilities::isolated_process::run_in_child("utilities::logger::ported::redirect_1") {
        return;
    }
    let logger = &sw_logger();
    logger.reset();
    assert!(!logger.redirect_stdout_and_stderr(true)); // no log file opened
    assert!(!logger.redirect_stdout_and_stderr(false)); // no redirected

    assert!(open(logger, file()));
    assert!(logger.redirect_stdout_and_stderr(true));
    assert!(!logger.redirect_stdout_and_stderr(true)); // has been redirected

    printf("hello world\n");
    let content = file_get_contents(file());
    assert!(content.is_some());

    logger.close();
    assert!(logger.redirect_stdout_and_stderr(false));
    unlink(Path::new(&real_file(logger)));

    assert!(contains(&content.unwrap(), "hello world\n"));
}

#[test]
#[cfg_attr(miri, ignore = "Miri can't spawn a process")]
fn redirect_2() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    // upstream's spawn_exec_and_wait: the body runs in a process of its own, a fresh
    // test binary rather than a fork of this threaded one (whose other threads may hold
    // libc locks), and fails the test unless it exits 0, where upstream's helper exited 0
    // anyway.
    if crate::utilities::isolated_process::run_in_child("utilities::logger::ported::redirect_2") {
        return;
    }
    let file = test_log_file();
    let str = "hello world, hello swoole\n";

    let logger = &sw_logger();
    logger.reset();
    open(logger, file);
    logger.redirect_stdout_and_stderr(true);

    printf(&format!("{str}\n"));

    // File(file, File::READ).read_content()
    let rs = std::fs::read(path(file)).unwrap();

    assert!(contains(&rs, str));
    logger.redirect_stdout_and_stderr(false);
    printf(&format!("{str}\n"));

    logger.close();
    unlink(Path::new(&real_file(logger)));
}

#[test]
#[cfg_attr(miri, ignore = "Miri can't spawn a process")]
fn ignore_error() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    // A child, so the process logger and the ignored code stay out of the other tests.
    // upstream's spawn_exec_and_wait: the body runs in a process of its own, a fresh
    // test binary rather than a fork of this threaded one (whose other threads may hold
    // libc locks), and fails the test unless it exits 0, where upstream's helper exited 0
    // anyway.
    if crate::utilities::isolated_process::run_in_child("utilities::logger::ported::ignore_error") {
        return;
    }
    let logger: &'static Logger = Box::leak(Box::new(sw_logger()));
    let cpp: &'static &'static Logger = Box::leak(Box::new(logger));
    crate::swr_v1_core_set_logger(std::ptr::from_ref(cpp).cast_mut().cast());
    logger.reset();
    logger.set_level(SW_LOG_NOTICE);
    open(logger, file());

    let ignored_errcode = 999_999;
    let errcode = 888_888;

    swoole_rs_error_ffi::swoole_ignore_error(ignored_errcode);

    // swoole_error_log(SW_LOG_WARNING, code, ...)
    let error_log = |code, message: &CStr| {
        swr_v1_utils_log(
            log::LogKind::Error,
            LOG_WARNING,
            code,
            c"ignore_error",
            message,
        );
    };
    error_log(ignored_errcode, c"error 1");
    error_log(errcode, c"error 2");

    let content = file_get_contents(file()).unwrap();

    logger.close();
    unlink(path(file()));

    assert!(!contains(&content, "error 1"));
    assert!(contains(&content, "error 2"));
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no open_memstream")]
fn set_stream() {
    unsafe extern "C" {
        /// A stream open until the `fclose` below, as a [`Stream`] (`FILE *`).
        fn open_memstream(buffer: *mut *mut c_char, size: *mut usize) -> Option<Stream>;
    }
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let logger = &sw_logger();
    logger.reset();
    let mut buffer: *mut c_char = std::ptr::null_mut();
    let mut size = 0;
    let stream = unsafe { open_memstream(&raw mut buffer, &raw mut size) };

    logger.set_stream(stream);
    put(logger, SW_LOG_ERROR, "hello world");

    logger.set_stream(Some(Stream::stdout()));
    put(logger, SW_LOG_ERROR, "hello world");

    let written = unsafe { CStr::from_ptr(buffer) };
    assert!(contains(written.to_bytes(), "ERROR\thello world"));
    unsafe {
        libc::fclose(stream.unwrap().as_ptr());
        libc::free(buffer.cast());
    }
}
