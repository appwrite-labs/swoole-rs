// SPDX-License-Identifier: Apache-2.0
//! Process-wide core state: upstream's `SwooleG` scalars, held by Rust, each
//! behind its accessor pair. Native object pointers (the memory pool, the stdout
//! stream, the logger, the server, the AIO pipe socket, the fatal-error and
//! backtrace hooks) are opaque: native code allocates and frees them, Rust only
//! holds the pointer.
use crate::CCast;
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_void};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, Ordering,
};

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
    running: AtomicBool, bool = false, swr_v1_core_running, swr_v1_core_set_running;
    enable_signalfd: AtomicBool, bool = false, swr_v1_core_enable_signalfd, swr_v1_core_set_enable_signalfd;
    enable_kqueue: AtomicBool, bool = false, swr_v1_core_enable_kqueue, swr_v1_core_set_enable_kqueue;
    enable_coroutine: AtomicBool, bool = false,
        swr_v1_core_enable_coroutine, swr_v1_core_set_enable_coroutine;
    print_backtrace_on_error: AtomicBool, bool = false,
        swr_v1_core_print_backtrace_on_error, swr_v1_core_set_print_backtrace_on_error;
    wait_signal: AtomicBool, bool = false, swr_v1_core_wait_signal, swr_v1_core_set_wait_signal;
    dns_lookup_random: AtomicBool, bool = false,
        swr_v1_core_dns_lookup_random, swr_v1_core_set_dns_lookup_random;
    current_task_id: AtomicI64, c_long = 0, swr_v1_core_current_task_id, swr_v1_core_set_current_task_id;
    signal_fd: AtomicI32, c_int = 0, swr_v1_core_signal_fd, swr_v1_core_set_signal_fd;
    signal_alarm: AtomicBool, bool = false,
        swr_v1_core_signal_alarm, swr_v1_core_set_signal_alarm;
    signal_listener_num: AtomicU32, u32 = 0,
        swr_v1_core_signal_listener_num, swr_v1_core_set_signal_listener_num;
    signal_async_listener_num: AtomicU32, u32 = 0,
        swr_v1_core_signal_async_listener_num, swr_v1_core_set_signal_async_listener_num;
    trace_flags: AtomicI64, c_long = 0, swr_v1_core_trace_flags, swr_v1_core_set_trace_flags;
    pagesize: AtomicU32, u32 = 0, swr_v1_core_pagesize, swr_v1_core_set_pagesize;
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
    logger: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        #[unsafe(no_mangle)] swr_v1_core_logger, swr_v1_core_set_logger;
    server: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(), swr_v1_core_server, swr_v1_core_set_server;
    aio_default_socket: AtomicPtr<c_void>, *mut c_void = ptr::null_mut(),
        swr_v1_core_aio_default_socket, swr_v1_core_set_aio_default_socket;
}

/// `swoole_fatal_error(code, "%s", message)`: `SwooleG.fatal_error`, which exits
/// the process or bails out; nothing when no hook is set.
///
/// # Safety
/// Native code may longjmp across the caller, so the frames it skips own nothing.
pub unsafe fn fatal_error(code: c_int, message: &std::ffi::CStr) {
    if let Some(fatal) = swr_v1_core_fatal_error() {
        // SAFETY: "%s" takes the NUL-terminated message.
        unsafe { fatal(code, c"%s".as_ptr(), message.as_ptr()) };
    }
}

/// `SwooleG.fatal_error`: `printf`-style, it exits the process or bails out.
pub type FatalError = unsafe extern "C" fn(code: c_int, format: *const c_char, ...);

static FATAL_ERROR: Mutex<Option<FatalError>> = Mutex::new(None);

fn fatal_hook() -> std::sync::MutexGuard<'static, Option<FatalError>> {
    FATAL_ERROR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `SwooleG.fatal_error`, which native `swoole_fatal_error` calls.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_fatal_error() -> Option<FatalError> {
    *fatal_hook()
}

/// `SwooleG.fatal_error = hook`.
pub fn set_fatal_error(hook: Option<FatalError>) {
    *fatal_hook() = hook;
}

/// A C stdio stream that stays open for the rest of the process: C's `stdout`,
/// or one native code hands over under that contract (a `FILE *` parameter of an
/// export typed as this). stdio locks a stream inside each call, so any thread may
/// write through it.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream(ptr::NonNull<libc::FILE>);

// SAFETY: stdio serializes the calls on a stream (POSIX `flockfile`), and the
// stream stays open (the type's invariant), so it may be used from any thread.
unsafe impl Send for Stream {}

// C's `stdout` (Apple's libc names it `__stdoutp`), which a hosted C program always
// has open.
unsafe extern "C" {
    #[cfg_attr(target_vendor = "apple", link_name = "__stdoutp")]
    #[cfg_attr(not(target_vendor = "apple"), link_name = "stdout")]
    safe static C_STDOUT: Stream;
}

impl Stream {
    /// C's `stdout`.
    #[must_use]
    pub fn stdout() -> Self {
        C_STDOUT
    }

    /// The stream at `raw`, none for null.
    ///
    /// # Safety
    /// `raw` is null or an open stream that nothing closes while this, or a copy of
    /// it, is in use.
    #[must_use]
    pub unsafe fn new(raw: *mut libc::FILE) -> Option<Self> {
        ptr::NonNull::new(raw).map(Self)
    }

    #[must_use]
    pub const fn as_ptr(self) -> *mut libc::FILE {
        self.0.as_ptr()
    }

    /// `fwrite` of `bytes`, buffered as the stream is.
    pub fn write(self, bytes: &[u8]) {
        use crate::lent::{OneByte, fwrite, lend_in};
        lend_in(bytes, |at, count| {
            fwrite(at.bytes(), OneByte::SIZE, count, self)
        });
    }

    /// `fflush`.
    pub fn flush(self) {
        crate::lent::fflush(self);
    }
}

/// `SwooleG.stdout`: null, or the [`Stream`] last set.
static STDOUT: AtomicPtr<libc::FILE> = AtomicPtr::new(ptr::null_mut());

/// `SwooleG.stdout`, none before one is set.
#[unsafe(no_mangle)]
#[must_use]
pub extern "C" fn swr_v1_core_stdout() -> Option<Stream> {
    // Only `swr_v1_core_set_stdout` stores, a stream's address or null.
    ptr::NonNull::new(STDOUT.load(Ordering::Acquire)).map(Stream)
}

/// `SwooleG.stdout = stream`.
pub fn swr_v1_core_set_stdout(stream: Option<Stream>) {
    STDOUT.store(
        stream.map_or(ptr::null_mut(), Stream::as_ptr),
        Ordering::Release,
    );
}

/// `SwooleG.print_backtrace`: the host's own backtrace, typed as the call it is.
static PRINT_BACKTRACE: Mutex<Option<extern "C" fn()>> = Mutex::new(None);

fn backtrace_hook() -> std::sync::MutexGuard<'static, Option<extern "C" fn()>> {
    PRINT_BACKTRACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `SwooleG.print_backtrace`, none until the host sets one.
#[must_use]
pub fn swr_v1_core_print_backtrace() -> Option<extern "C" fn()> {
    *backtrace_hook()
}

/// `SwooleG.print_backtrace = hook`.
pub fn swr_v1_core_set_print_backtrace(hook: Option<extern "C" fn()>) {
    *backtrace_hook() = hook;
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
    set_fatal_error(None);
    swr_v1_core_set_print_backtrace(None);
    swr_v1_core_set_stdout(None);
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
    let page = crate::sys::sysconf(libc::_SC_PAGESIZE);
    let cpus = crate::sys::sysconf(libc::_SC_NPROCESSORS_ONLN);
    {
        swr_v1_core_set_pagesize(page.c_cast::<u32>());
        RECORD
            .cpu_num
            .store(cpus.max(1).c_cast::<u16>(), Ordering::Relaxed);
    }
    swr_v1_core_set_max_file_content(MAX_FILE_CONTENT);
    RECORD.dns_tries.store(1, Ordering::Relaxed);
    let now = time_now();
    let seed = now.c_cast::<c_uint>();
    crate::sys::srandom(seed);
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
    if crate::sys::getrlimit(libc::RLIMIT_NOFILE, &mut rlimit) < 0 {
        return -1;
    }
    let current = rlimit.rlim_cur.c_cast::<u32>();
    i64::from(current.min(limit))
}

pub fn swr_v1_core_initialized() -> bool {
    RECORD.init.load(Ordering::Relaxed)
}

#[must_use]
pub fn swr_v1_core_cpu_num() -> u16 {
    RECORD.cpu_num.load(Ordering::Relaxed)
}

pub fn swr_v1_core_dns_tries() -> c_int {
    RECORD.dns_tries.load(Ordering::Relaxed)
}

/// `SwooleG.bug_report_message`, copied.
#[must_use]
pub fn bug_report_message() -> String {
    let message = bug_report();
    String::from_utf8_lossy(message.split_last().map_or(&[], |(_, text)| text)).into_owned()
}

unsafe extern "C" {
    /// native/corebase/switches.cc: the banner, the compiler and the TLS library
    /// line, as C++ compiled them.
    safe static swr_cpp_bug_report: [crate::sys::Text<'static>; 3];
}

/// base.cc's `bug_report_message_init`, from the lines C++ compiled. Inline, so
/// only `swoole_init`'s object references switches.cc: gates that link the Rust
/// archive without libswoole still resolve every symbol.
#[inline]
pub fn bug_report_init() {
    let [banner, compiler, ssl] = swr_cpp_bug_report.map(crate::sys::Text::get);
    bug_report_lines(banner, compiler, ssl);
}

/// The banner line, the host's uname, the compiler and the TLS library line.
fn bug_report_lines(banner: &CStr, compiler: &CStr, ssl: &CStr) {
    let mut message = b"\n".to_vec();
    message.extend(banner.to_bytes());
    message.push(b'\n');
    let mut host: libc::utsname = crate::sys::zeroed();
    if crate::sys::uname(&mut host) != -1 {
        message.extend_from_slice(b"OS:");
        for field in [host.sysname, host.release, host.version, host.machine] {
            message.push(b' ');
            // Each field is NUL-terminated inside its array.
            let field = field.map(|byte| byte.to_ne_bytes()[0]);
            message.extend(field.split(|&byte| byte == 0).next().unwrap_or_default());
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
pub extern "C" fn swoole_set_stdout_stream(stream: Option<Stream>) {
    swr_v1_core_set_stdout(stream);
}

/// `FILE *swoole_get_stdout_stream()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z24swoole_get_stdout_streamv")
)]
#[must_use]
pub extern "C" fn swoole_get_stdout_stream() -> Option<Stream> {
    swr_v1_core_stdout()
}

/// `sw_printf("%s", ...)` of `bytes`: Swoole's stdout stream, which
/// `swoole_set_stdout_stream` replaces; nothing before one is set.
pub fn sw_print(bytes: &[u8]) {
    if let Some(out) = swr_v1_core_stdout() {
        out.write(bytes);
    }
}

/// `swoole_set_trace_flags(long)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_set_trace_flagsl")
)]
pub extern "C" fn swoole_set_trace_flags(flags: c_long) {
    swr_v1_core_set_trace_flags(flags);
}

/// `::time(nullptr)`: the wall clock's seconds, from the clock glibc's `time`
/// reads (`CLOCK_REALTIME_COARSE` on Linux) in one vDSO call. `SystemTime::now`
/// and `duration_since` cost five times as much, and every HTTP request asks.
#[must_use]
pub fn time_now() -> libc::time_t {
    #[cfg(target_os = "linux")]
    const CLOCK: libc::clockid_t = libc::CLOCK_REALTIME_COARSE;
    #[cfg(not(target_os = "linux"))]
    const CLOCK: libc::clockid_t = libc::CLOCK_REALTIME;
    crate::sys::clock_gettime(CLOCK).map_or(0, |now| now.tv_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upstream's `time.get_seconds`: the wall clock's seconds against the C library's.
    /// glibc's `time(NULL)` reads `CLOCK_REALTIME_COARSE`, which trails the precise clock
    /// by up to a tick, so just after a second boundary it reads one less than a later
    /// precise reading. `time_now` reads that clock; the reference reads `CLOCK_REALTIME`.
    #[test]
    fn time_get_seconds() {
        let sec1 = time_now();
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &raw mut ts) },
            0
        );
        let sec2 = ts.tv_sec;
        assert!(sec1 == sec2 || sec1 == sec2 - 1);
    }

    // One test owns the process-wide record, so parallel tests never race on it.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no srandom")]
    fn record_lifecycle() {
        swr_v1_core_reset();
        let pool = ptr::from_mut(&mut 7_u8).cast::<c_void>();
        let stdout = Some(Stream::stdout());
        swr_v1_core_set_stdout(stdout);
        assert!(swr_v1_core_init());
        assert!(swr_v1_core_initialized());
        // Initialisation starts from a reset record.
        assert!(swr_v1_core_stdout().is_none());
        assert!(swr_v1_core_cpu_num() >= 1);
        assert_eq!(swr_v1_core_dns_tries(), 1);
        assert!(swr_v1_core_running() && swr_v1_core_enable_coroutine());
        assert!(!swr_v1_core_enable_signalfd() && !swr_v1_core_enable_kqueue());
        assert!(swr_v1_core_pagesize().is_power_of_two());
        assert_eq!(swr_v1_core_max_file_content(), 64 * 1024 * 1024);
        swr_v1_core_set_wait_signal(true);
        swr_v1_core_set_dns_lookup_random(true);
        swr_v1_core_set_stdout(stdout);
        swr_v1_core_set_logger(pool);
        swr_v1_core_set_signal_fd(-1);
        swr_v1_core_set_current_task_id(c_long::MAX);
        swr_v1_core_set_aio_max_wait_time(0.25);
        swoole_set_print_backtrace_on_error(true);
        swoole_set_trace_flags(0x7fff_ffff);
        assert!(!swr_v1_core_init(), "a second init is a no-op");
        assert!(swr_v1_core_wait_signal() && swr_v1_core_dns_lookup_random());
        assert_eq!(swr_v1_core_stdout(), stdout);
        assert_eq!(swr_v1_core_logger(), pool);
        assert_eq!(swr_v1_core_signal_fd(), -1);
        assert_eq!(swr_v1_core_current_task_id(), c_long::MAX);
        assert!((swr_v1_core_aio_max_wait_time() - 0.25).abs() < f64::EPSILON);
        assert!(swr_v1_core_print_backtrace_on_error());
        assert_eq!(swr_v1_core_trace_flags(), 0x7fff_ffff);

        bug_report_lines(c"Banner", c"cc 1", c"SSL\n");
        bug_report_append(b"PHP_VERSION : 8\n");
        let message = bug_report_message();
        assert!(message.starts_with("\nBanner\nOS: "), "{message}");
        assert!(
            message.ends_with("\nGCC_VERSION: cc 1\nSSL\nPHP_VERSION : 8\n"),
            "{message}"
        );

        swr_v1_core_reset();
        assert!(!swr_v1_core_initialized());
        assert_eq!(swr_v1_core_cpu_num(), 0);
        assert!(!swr_v1_core_running() && !swr_v1_core_wait_signal());
        assert!(swr_v1_core_stdout().is_none() && swr_v1_core_logger().is_null());
        assert_eq!(swr_v1_core_signal_fd(), 0);
        assert_eq!(swr_v1_core_aio_max_wait_time().to_bits(), 0);
        assert_eq!(bug_report_message(), "");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no getrlimit")]
    fn max_sockets_caps_the_soft_limit() {
        assert_eq!(swr_v1_core_max_sockets(1), 1);
        assert!(swr_v1_core_max_sockets(u32::MAX) >= 1);
    }
}
