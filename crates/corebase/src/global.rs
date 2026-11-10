// SPDX-License-Identifier: Apache-2.0
//! Process-wide core state: upstream's `SwooleG` scalars, held by Rust. Native
//! code reaches each through its accessor pair; `swoole::Global`'s members of
//! those names are `swoole::RustField` views over them, so upstream's
//! `SwooleG.field` reads and writes compile unchanged. Native object pointers
//! (the memory pool, the stdout stream, the logger, the server, the AIO pipe
//! socket, the fatal-error and backtrace hooks) are opaque: native code
//! allocates and frees them, Rust only holds the pointer.
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_void};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, Ordering,
};

unsafe extern "C" {
    fn srandom(seed: c_uint);
}

/// Upstream's `SW_MAX_FILE_CONTENT` (`include/swoole_config.h`).
const MAX_FILE_CONTENT: u32 = crate::constants::SW_MAX_FILE_CONTENT;

/// The record, one atomic per field, with a getter and setter for each (`$get`,
/// `$set`); `#[unsafe(no_mangle)]` exports the ones native code calls. Every
/// field starts at, and `swr_v1_core_reset` returns it to, `$zero`: upstream's
/// `SwooleG = {}`.
macro_rules! record {
    ($($field:ident: $store:ty, $value:ty = $zero:expr,
        $(#[$get_attr:meta])* $get:ident, $(#[$set_attr:meta])* $set:ident;)*) => {
        struct Record {
            init: AtomicBool,
            cpu_num: AtomicU16,
            dns_tries: AtomicI32,
            $($field: $store,)*
        }

        static RECORD: Record = Record {
            init: AtomicBool::new(false),
            cpu_num: AtomicU16::new(0),
            dns_tries: AtomicI32::new(0),
            $($field: <$store>::new($zero),)*
        };

        fn reset_record() {
            RECORD.init.store(false, Ordering::Relaxed);
            RECORD.cpu_num.store(0, Ordering::Relaxed);
            RECORD.dns_tries.store(0, Ordering::Relaxed);
            $(RECORD.$field.store($zero, Ordering::Release);)*
        }

        $(
            $(#[$get_attr])*
            pub extern "C" fn $get() -> $value {
                RECORD.$field.load(Ordering::Acquire)
            }

            $(#[$set_attr])*
            pub extern "C" fn $set(value: $value) {
                RECORD.$field.store(value, Ordering::Release);
            }
        )*
    };
}

record! {
    running: AtomicBool, bool = false,
        #[unsafe(no_mangle)] swr_v1_core_running, #[unsafe(no_mangle)] swr_v1_core_set_running;
    enable_signalfd: AtomicBool, bool = false,
        #[unsafe(no_mangle)] swr_v1_core_enable_signalfd, #[unsafe(no_mangle)] swr_v1_core_set_enable_signalfd;
    enable_kqueue: AtomicBool, bool = false, swr_v1_core_enable_kqueue, swr_v1_core_set_enable_kqueue;
    enable_coroutine: AtomicBool, bool = false,
        #[unsafe(no_mangle)] swr_v1_core_enable_coroutine, #[unsafe(no_mangle)] swr_v1_core_set_enable_coroutine;
    print_backtrace_on_error: AtomicBool, bool = false,
        swr_v1_core_print_backtrace_on_error, swr_v1_core_set_print_backtrace_on_error;
    wait_signal: AtomicBool, bool = false, swr_v1_core_wait_signal, swr_v1_core_set_wait_signal;
    dns_lookup_random: AtomicBool, bool = false,
        swr_v1_core_dns_lookup_random, swr_v1_core_set_dns_lookup_random;
    current_task_id: AtomicI64, c_long = 0, swr_v1_core_current_task_id, swr_v1_core_set_current_task_id;
    signal_fd: AtomicI32, c_int = 0, swr_v1_core_signal_fd, swr_v1_core_set_signal_fd;
    signal_alarm: AtomicBool, bool = false,
        #[unsafe(no_mangle)] swr_v1_core_signal_alarm, swr_v1_core_set_signal_alarm;
    signal_listener_num: AtomicU32, u32 = 0,
        #[unsafe(no_mangle)] swr_v1_core_signal_listener_num,
        #[unsafe(no_mangle)] swr_v1_core_set_signal_listener_num;
    signal_async_listener_num: AtomicU32, u32 = 0,
        #[unsafe(no_mangle)] swr_v1_core_signal_async_listener_num,
        #[unsafe(no_mangle)] swr_v1_core_set_signal_async_listener_num;
    trace_flags: AtomicI64, c_long = 0, #[unsafe(no_mangle)] swr_v1_core_trace_flags, swr_v1_core_set_trace_flags;
    pagesize: AtomicU32, u32 = 0,
        #[unsafe(no_mangle)] swr_v1_core_pagesize, #[unsafe(no_mangle)] swr_v1_core_set_pagesize;
    max_file_content: AtomicU32, u32 = 0, swr_v1_core_max_file_content, swr_v1_core_set_max_file_content;
    aio_core_worker_num: AtomicU32, u32 = 0,
        swr_v1_core_aio_core_worker_num, swr_v1_core_set_aio_core_worker_num;
    aio_worker_num: AtomicU32, u32 = 0, swr_v1_core_aio_worker_num, swr_v1_core_set_aio_worker_num;
    iouring_entries: AtomicU32, u32 = 0, swr_v1_core_iouring_entries, swr_v1_core_set_iouring_entries;
    iouring_workers: AtomicU32, u32 = 0, swr_v1_core_iouring_workers, swr_v1_core_set_iouring_workers;
    iouring_flag: AtomicU32, u32 = 0, swr_v1_core_iouring_flag, swr_v1_core_set_iouring_flag;
    aio_max_wait_time: AtomicU64, u64 = 0,
        swr_v1_core_aio_max_wait_time_bits, swr_v1_core_set_aio_max_wait_time_bits;
    aio_max_idle_time: AtomicU64, u64 = 0,
        swr_v1_core_aio_max_idle_time_bits, swr_v1_core_set_aio_max_idle_time_bits;
    stdout: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        #[unsafe(no_mangle)] swr_v1_core_stdout, swr_v1_core_set_stdout;
    logger: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        #[unsafe(no_mangle)] swr_v1_core_logger, swr_v1_core_set_logger;
    server: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        #[unsafe(no_mangle)] swr_v1_core_server, #[unsafe(no_mangle)] swr_v1_core_set_server;
    aio_default_socket: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        swr_v1_core_aio_default_socket, swr_v1_core_set_aio_default_socket;
    fatal_error: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        #[unsafe(no_mangle)] swr_v1_core_fatal_error, swr_v1_core_set_fatal_error;
    print_backtrace: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        swr_v1_core_print_backtrace, swr_v1_core_set_print_backtrace;
}

/// `swoole_fatal_error(code, "%s", message)`: `SwooleG.fatal_error`, which exits
/// the process or bails out; nothing when no hook is set.
///
/// # Safety
/// Native code may longjmp across the caller, so the frames it skips own nothing.
pub unsafe fn fatal_error(code: c_int, message: &std::ffi::CStr) {
    type FatalError = unsafe extern "C" fn(code: c_int, format: *const c_char, ...);
    let hook = swr_v1_core_fatal_error();
    if !hook.is_null() {
        // SAFETY: native code stores its `fatal_error` there.
        let fatal = unsafe { std::mem::transmute::<*mut c_void, FatalError>(hook) };
        // SAFETY: "%s" takes the NUL-terminated message.
        unsafe { fatal(code, c"%s".as_ptr(), message.as_ptr()) };
    }
}

/// The two `double` fields travel as their bit patterns, so the record needs
/// no float atomics; these are the `double` views native code calls.
macro_rules! doubles {
    ($($get:ident, $set:ident => $get_bits:ident, $set_bits:ident;)*) => {$(
        pub fn $get() -> f64 {
            f64::from_bits($get_bits())
        }

        pub fn $set(value: f64) {
            $set_bits(value.to_bits());
        }
    )*};
}

doubles! {
    swr_v1_core_aio_max_wait_time, swr_v1_core_set_aio_max_wait_time
        => swr_v1_core_aio_max_wait_time_bits, swr_v1_core_set_aio_max_wait_time_bits;
    swr_v1_core_aio_max_idle_time, swr_v1_core_set_aio_max_idle_time
        => swr_v1_core_aio_max_idle_time_bits, swr_v1_core_set_aio_max_idle_time_bits;
}

/// `SwooleG.bug_report_message`, NUL-terminated once it has any text.
static BUG_REPORT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn bug_report() -> std::sync::MutexGuard<'static, Vec<u8>> {
    BUG_REPORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `SwooleG.bug_report_message += text`.
pub fn bug_report_append(text: &[u8]) {
    let mut message = bug_report();
    message.pop();
    message.extend_from_slice(text);
    message.push(0);
}

/// Zero every field, as upstream's `SwooleG = {}` does.
pub fn swr_v1_core_reset() {
    reset_record();
    bug_report().clear();
}

/// `swoole_init`'s Rust half: false when already initialised. Otherwise resets
/// the record, publishes the guard, then sets the defaults upstream set: running,
/// coroutines on, the page size, the file-content cap, the CPU count, DNS tries
/// and the `random()` seed.
pub fn swr_v1_core_init() -> bool {
    if RECORD.init.load(Ordering::Relaxed) {
        return false;
    }
    swr_v1_core_reset();
    RECORD.init.store(true, Ordering::Relaxed);
    swr_v1_core_set_running(true);
    swr_v1_core_set_enable_coroutine(true);
    // SAFETY: sysconf has no memory preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    // SAFETY: sysconf has no memory preconditions.
    let cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's uint32_t pagesize of getpagesize() and uint16_t cpu_num of a positive count"
    )]
    {
        swr_v1_core_set_pagesize(page as u32);
        RECORD.cpu_num.store(cpus.max(1) as u16, Ordering::Relaxed);
    }
    swr_v1_core_set_max_file_content(MAX_FILE_CONTENT);
    RECORD.dns_tries.store(1, Ordering::Relaxed);
    let now = time_now();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "srandom(time(nullptr)) narrows as upstream"
    )]
    let seed = now as c_uint;
    // SAFETY: srandom has no memory preconditions.
    unsafe { srandom(seed) };
    true
}

/// The descriptor limit upstream derives from `RLIMIT_NOFILE`: the soft limit,
/// truncated to `uint32_t` as upstream does, capped at `limit`. -1 (errno kept)
/// when getrlimit fails.
#[must_use]
pub fn swr_v1_core_max_sockets(limit: u32) -> i64 {
    let mut rlimit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `rlimit` is a live, writable local.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut rlimit) } < 0 {
        return -1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (uint32_t) rlmt.rlim_cur"
    )]
    let current = rlimit.rlim_cur as u32;
    i64::from(current.min(limit))
}

pub fn swr_v1_core_initialized() -> bool {
    RECORD.init.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_cpu_num() -> u16 {
    RECORD.cpu_num.load(Ordering::Relaxed)
}

pub fn swr_v1_core_dns_tries() -> c_int {
    RECORD.dns_tries.load(Ordering::Relaxed)
}

/// `SwooleG.bug_report_message.c_str()`: valid until the message next changes.
#[must_use]
pub fn swr_v1_core_bug_report_message() -> *const c_char {
    let message = bug_report();
    if message.is_empty() {
        return c"".as_ptr();
    }
    message.as_ptr().cast()
}

unsafe extern "C" {
    /// native/corebase/switches.cc: the banner, the compiler and the TLS library
    /// line, as C++ compiled them.
    safe static swr_cpp_bug_report: [*const c_char; 3];
}

/// base.cc's `bug_report_message_init`, from the lines C++ compiled. Inline, so
/// only `swoole_init`'s object references switches.cc: gates that link the Rust
/// archive without libswoole still resolve every symbol.
#[inline]
pub fn bug_report_init() {
    // SAFETY: switches.cc's NUL-terminated strings.
    let [banner, compiler, ssl] = swr_cpp_bug_report.map(|line| unsafe { CStr::from_ptr(line) });
    bug_report_lines(banner, compiler, ssl);
}

/// The banner line, the host's uname, the compiler and the TLS library line.
fn bug_report_lines(banner: &CStr, compiler: &CStr, ssl: &CStr) {
    let mut message = b"\n".to_vec();
    message.extend(banner.to_bytes());
    message.push(b'\n');
    // SAFETY: utsname is plain bytes, valid zeroed.
    let mut host: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `host` is a live, writable utsname.
    if unsafe { libc::uname(&raw mut host) } != -1 {
        message.extend_from_slice(b"OS:");
        for field in [host.sysname, host.release, host.version, host.machine] {
            message.push(b' ');
            // SAFETY: uname's fields are NUL-terminated.
            message.extend(unsafe { CStr::from_ptr(field.as_ptr()) }.to_bytes());
        }
        message.push(b'\n');
    }
    message.extend_from_slice(b"GCC_VERSION: ");
    message.extend(compiler.to_bytes());
    message.push(b'\n');
    message.extend(ssl.to_bytes());
    bug_report_append(&message);
}

/// `swoole_set_print_backtrace_on_error(bool enable = true)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z35swoole_set_print_backtrace_on_errorb")
)]
pub extern "C" fn swoole_set_print_backtrace_on_error(enable: bool) {
    swr_v1_core_set_print_backtrace_on_error(enable);
}

/// `swoole::Logger *sw_logger()`.
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z9sw_loggerv"))]
#[must_use]
pub extern "C" fn sw_logger() -> *mut c_void {
    swr_v1_core_logger()
}

/// `swoole_set_stdout_stream(FILE *)`; `FILE` mangles as its libc's struct tag.
#[cfg_attr(
    all(feature = "upstream-symbols", target_vendor = "apple"),
    unsafe(export_name = "_Z24swoole_set_stdout_streamP7__sFILE")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_vendor = "apple")),
    unsafe(export_name = "_Z24swoole_set_stdout_streamP8_IO_FILE")
)]
pub extern "C" fn swoole_set_stdout_stream(stream: *mut c_void) {
    swr_v1_core_set_stdout(stream);
}

/// `FILE *swoole_get_stdout_stream()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z24swoole_get_stdout_streamv")
)]
#[must_use]
pub extern "C" fn swoole_get_stdout_stream() -> *mut c_void {
    swr_v1_core_stdout()
}

/// `swoole_set_trace_flags(long)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_set_trace_flagsl")
)]
pub extern "C" fn swoole_set_trace_flags(flags: c_long) {
    swr_v1_core_set_trace_flags(flags);
}

/// `::time(nullptr)`: the wall clock's seconds.
#[must_use]
pub fn time_now() -> libc::time_t {
    let seconds = |elapsed: std::time::Duration| {
        libc::time_t::try_from(elapsed.as_secs()).unwrap_or(libc::time_t::MAX)
    };
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => seconds(elapsed),
        Err(before) => -seconds(before.duration()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    /// Upstream's `time.get_seconds`: the wall clock's seconds against `time(NULL)`.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no libc time")]
    fn time_get_seconds() {
        let sec1 = time_now();
        let sec2 = unsafe { libc::time(ptr::null_mut()) };
        assert!(sec1 == sec2 || sec1 == sec2 - 1);
    }

    // One test owns the process-wide record, so parallel tests never race on it.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no srandom")]
    fn record_lifecycle() {
        swr_v1_core_reset();
        let pool = ptr::from_mut(&mut 7_u8).cast::<c_void>();
        swr_v1_core_set_stdout(pool);
        assert!(swr_v1_core_init());
        assert!(swr_v1_core_initialized());
        // Initialisation starts from a reset record.
        assert!(swr_v1_core_stdout().is_null());
        assert!(swr_v1_core_cpu_num() >= 1);
        assert_eq!(swr_v1_core_dns_tries(), 1);
        assert!(swr_v1_core_running() && swr_v1_core_enable_coroutine());
        assert!(!swr_v1_core_enable_signalfd() && !swr_v1_core_enable_kqueue());
        assert!(swr_v1_core_pagesize().is_power_of_two());
        assert_eq!(swr_v1_core_max_file_content(), 64 * 1024 * 1024);
        swr_v1_core_set_wait_signal(true);
        swr_v1_core_set_dns_lookup_random(true);
        swr_v1_core_set_stdout(pool);
        swr_v1_core_set_logger(pool);
        swr_v1_core_set_signal_fd(-1);
        swr_v1_core_set_current_task_id(c_long::MAX);
        swr_v1_core_set_aio_max_wait_time(0.25);
        swoole_set_print_backtrace_on_error(true);
        swoole_set_trace_flags(0x7fff_ffff);
        assert!(!swr_v1_core_init(), "a second init is a no-op");
        assert!(swr_v1_core_wait_signal() && swr_v1_core_dns_lookup_random());
        assert_eq!(swr_v1_core_stdout(), pool);
        assert_eq!(swr_v1_core_logger(), pool);
        assert_eq!(swr_v1_core_signal_fd(), -1);
        assert_eq!(swr_v1_core_current_task_id(), c_long::MAX);
        assert!((swr_v1_core_aio_max_wait_time() - 0.25).abs() < f64::EPSILON);
        assert!(swr_v1_core_print_backtrace_on_error());
        assert_eq!(swr_v1_core_trace_flags(), 0x7fff_ffff);

        bug_report_lines(c"Banner", c"cc 1", c"SSL\n");
        bug_report_append(b"PHP_VERSION : 8\n");
        // SAFETY: the message is NUL-terminated and unchanged while read.
        let message = unsafe { CStr::from_ptr(swr_v1_core_bug_report_message()) }
            .to_str()
            .unwrap();
        assert!(message.starts_with("\nBanner\nOS: "), "{message}");
        assert!(
            message.ends_with("\nGCC_VERSION: cc 1\nSSL\nPHP_VERSION : 8\n"),
            "{message}"
        );

        swr_v1_core_reset();
        assert!(!swr_v1_core_initialized());
        assert_eq!(swr_v1_core_cpu_num(), 0);
        assert!(!swr_v1_core_running() && !swr_v1_core_wait_signal());
        assert!(swr_v1_core_stdout().is_null() && swr_v1_core_logger().is_null());
        assert_eq!(swr_v1_core_signal_fd(), 0);
        assert_eq!(swr_v1_core_aio_max_wait_time().to_bits(), 0);
        // SAFETY: the empty message is a static NUL-terminated string.
        assert!(unsafe { CStr::from_ptr(swr_v1_core_bug_report_message()) }.is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no getrlimit")]
    fn max_sockets_caps_the_soft_limit() {
        assert_eq!(swr_v1_core_max_sockets(1), 1);
        assert!(swr_v1_core_max_sockets(u32::MAX) >= 1);
    }
}
