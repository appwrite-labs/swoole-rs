// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! Whole host operations from base.cc, under their C++ symbols with
//! `upstream-symbols`. Logs name the function as upstream's `__SW_FUNC__` did.

use crate::CCast;
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};

unsafe extern "C" {
    fn snprintf(buffer: *mut c_char, size: usize, format: *const c_char, ...) -> c_int;
    fn exit(status: c_int) -> !;
}

#[cfg(any(all(target_os = "linux", target_env = "gnu"), target_vendor = "apple"))]
unsafe extern "C" {
    fn backtrace(frames: *mut *mut c_void, size: c_int) -> c_int;
    fn backtrace_symbols(frames: *const *mut c_void, size: c_int) -> *mut *mut c_char;
}

use crate::sys::{Text, close, dup2, errno, set_errno};

/// `int swoole_shell_exec(const char *command, pid_t *pid, bool get_error_stream)`:
/// run `command` under `/bin/sh -c` with its stdout (and stderr when
/// `get_error_stream`) on a pipe. Returns the read end, or -1 when `pipe` or
/// `fork` fails (the latter warns). The child's pid goes in `pid`.
pub fn swoole_shell_exec(command: &CStr, pid: &mut libc::pid_t, get_error_stream: bool) -> c_int {
    let Some([read, write]) = crate::sys::pipe() else {
        return -1;
    };
    // The child only makes async-signal-safe calls before exec, as upstream's did.
    let child = crate::sys::fork();
    if child == -1 {
        let error = errno();
        close(read);
        close(write);
        set_errno(error);
        crate::log::sys_warning(c"swoole_shell_exec", format_args!("fork() failed"));
        return -1;
    }
    if child == 0 {
        close(read);
        if get_error_stream {
            if write == libc::STDOUT_FILENO {
                dup2(write, libc::STDERR_FILENO);
            } else if write == libc::STDERR_FILENO {
                dup2(write, libc::STDOUT_FILENO);
            } else {
                dup2(write, libc::STDOUT_FILENO);
                dup2(write, libc::STDERR_FILENO);
                close(write);
            }
        } else if write != libc::STDOUT_FILENO {
            dup2(write, libc::STDOUT_FILENO);
            close(write);
        }
        let end = ptr::null::<c_char>();
        // SAFETY: the path and argument strings are NUL-terminated and the list ends in null.
        unsafe {
            libc::execl(
                c"/bin/sh".as_ptr(),
                c"sh".as_ptr(),
                c"-c".as_ptr(),
                command.as_ptr(),
                end,
            )
        };
        // SAFETY: exit(3) runs the process's exit handlers, as upstream's child did.
        unsafe { exit(127) };
    }
    *pid = child;
    close(write);
    read
}

/// `printf` of text that holds no conversion, to C's stdout.
fn put(text: &str) {
    crate::sys::print(text.as_bytes());
}

/// `swoole_dump_ascii`: each byte as its native `(unsigned) char` value.
pub fn swoole_dump_ascii(data: &[u8]) {
    for &byte in data {
        // `%u` of the promoted char.
        let byte = c_char::from_ne_bytes([byte]);
        put(&format!("{} ", c_int::from(byte).cast_unsigned()));
    }
    put("\n");
}

/// `swoole_dump_hex`: sixteen bytes per row, each row led by its offset.
pub fn swoole_dump_hex(data: &[u8]) {
    for (index, byte) in data.iter().enumerate() {
        if index % 16 == 0 {
            put(&format!("{index:08X}: "));
        }
        put(&format!("{byte:02X} "));
        if (index + 1) % 16 == 0 {
            put("\n");
        }
    }
    put("\n");
}

/// `swoole_dump_bin`: each whole `kind`-sized value, unpacked as `swoole_unpack`
/// does, printed as `%ld,`, then a newline; nothing for a type without a size.
pub fn swoole_dump_bin(data: &[u8], kind: c_char) {
    let Ok(width) = usize::try_from(crate::swoole_type_size(kind)) else {
        return;
    };
    if width == 0 {
        return;
    }
    let [format] = kind.to_ne_bytes();
    for field in data.chunks_exact(width) {
        let value = swoole_rs_framing::unpack(format, field).unwrap_or(-1);
        put(&format!("{value},"));
    }
    put("\n");
}

/// `void swoole_print_backtrace()`: up to 16 frames, one symbol per line (upstream's
/// build without Boost.Stacktrace).
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_print_backtracev")
)]
pub extern "C" fn swoole_print_backtrace() {
    #[cfg(any(all(target_os = "linux", target_env = "gnu"), target_vendor = "apple"))]
    {
        let mut frames = [ptr::null_mut::<c_void>(); 16];
        // SAFETY: `frames` holds the 16 entries backtrace(3) may write.
        let count = unsafe { backtrace(frames.as_mut_ptr(), 16) };
        // SAFETY: the first `count` frames were just filled.
        let symbols = unsafe { backtrace_symbols(frames.as_ptr(), count) };
        if symbols.is_null() {
            return;
        }
        let count = usize::try_from(count).unwrap_or(0);
        // SAFETY: backtrace_symbols returns `count` NUL-terminated strings, each a
        // `Text` until the array is freed below.
        let lines = unsafe { std::slice::from_raw_parts(symbols.cast::<Text<'_>>(), count) };
        for line in lines {
            crate::sys::print(line.get().to_bytes());
            put("\n");
        }
        // SAFETY: backtrace_symbols' array is one malloc'd block.
        unsafe { libc::free(symbols.cast()) };
    }
}

/// `swoole::microtime`: wall-clock seconds, as libstdc++'s
/// `duration<double>` of the nanosecond `system_clock` count.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9microtimeEv")
)]
pub extern "C" fn microtime() -> f64 {
    // One clock_gettime, as libstdc++'s system_clock::now(); std's SystemTime adds
    // a checked subtraction on every request (#853).
    // CLOCK_REALTIME cannot fail.
    let now = crate::sys::clock_gettime(libc::CLOCK_REALTIME).unwrap_or(libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    });
    #[expect(
        clippy::cast_precision_loss,
        clippy::useless_conversion,
        reason = "system_clock's signed nanosecond count, converted as libstdc++ does; \
                  time_t and c_long are i64 on the 64-bit targets only on some hosts"
    )]
    let nanos = i64::from(now.tv_sec)
        .wrapping_mul(1_000_000_000)
        .wrapping_add(i64::from(now.tv_nsec)) as f64;
    nanos / 1e9
}

/// `DataHead`'s layout (include/swoole.h), checked by a native static assertion.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DataHead {
    pub fd: c_long,
    pub msg_id: u64,
    pub len: u32,
    pub reactor_id: i16,
    /// `type`.
    pub kind: u8,
    pub flags: u8,
    pub server_fd: u16,
    pub ext_flags: u16,
    pub reserved: u32,
    pub time: f64,
}

/// `DataHead::dump`: the fields in upstream's format, with `sw_snprintf`'s
/// result: at most `buffer.len() - 1` bytes written, always NUL-terminated.
pub fn datahead_dump(h: &DataHead, buffer: &mut [u8]) -> usize {
    let size = buffer.len();
    // SAFETY: the format's conversions match the promoted arguments one for one, and
    // `buffer` is writable for `size` bytes.
    let written = unsafe {
        snprintf(
            buffer.as_mut_ptr().cast(),
            size,
            c"DataHead[%p]\n{\n    long fd = %ld;\n    uint64_t msg_id = %llu;\n    uint32_t len = %d;\n    int16_t reactor_id = %d;\n    uint8_t type = %d;\n    uint8_t flags = %d;\n    uint16_t server_fd = %d;\n    uint16_t ext_flags = %d;\n    double time = %f;\n}\n".as_ptr(),
            ptr::from_ref(h),
            h.fd,
            h.msg_id,
            h.len,
            c_int::from(h.reactor_id),
            c_int::from(h.kind),
            c_int::from(h.flags),
            c_int::from(h.server_fd),
            c_int::from(h.ext_flags),
            h.time,
        )
    };
    if size == 0 {
        return usize::try_from(written).unwrap_or(usize::MAX);
    }
    let Ok(written) = usize::try_from(written) else {
        if let Some(first) = buffer.first_mut() {
            *first = 0;
        }
        return 0;
    };
    written.min(size - 1)
}

thread_local! {
    static GENERATOR: Cell<Option<[u64; 4]>> = const { Cell::new(None) };
}

fn seed() -> [u64; 4] {
    let mut state = [0u64; 4];
    let mut entropy = [0u8; 32];
    if crate::random_bytes(&mut entropy) == entropy.len() {
        for (word, bytes) in state.iter_mut().zip(entropy.as_chunks::<8>().0) {
            *word = u64::from_ne_bytes(*bytes);
        }
    } else {
        state[0] = microtime().to_bits();
    }
    if state == [0; 4] {
        state[0] = 0x9e37_79b9_7f4a_7c15;
    }
    state
}

/// One xoshiro256** step on this thread's generator, seeded from the OS.
fn next() -> u64 {
    GENERATOR.with(|cell| {
        let mut s = cell.get().unwrap_or_else(seed);
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        cell.set(Some(s));
        result
    })
}

/// `uint64_t swoole_random_int()`: a uniform 64-bit value.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z17swoole_random_intv")
)]
pub extern "C" fn swoole_random_int() -> u64 {
    next()
}

/// `int swoole_rand()`: `swoole_rand(0, INT_MAX)`.
#[must_use]
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z11swoole_randv"))]
pub extern "C" fn swoole_rand_any() -> c_int {
    swoole_rand(0, c_int::MAX)
}

/// `void *sw_malloc(size_t)`. Upstream calls through `SwooleG.std_allocator`,
/// which only ever holds libc's allocator.
#[must_use]
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z9sw_mallocm"))]
pub extern "C" fn sw_malloc(size: usize) -> *mut c_void {
    // SAFETY: malloc has no preconditions.
    unsafe { libc::malloc(size) }
}

/// `void sw_free(void *)`.
/// # Safety
/// `pointer` is null or a live allocation from this allocator.
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z7sw_freePv"))]
pub unsafe extern "C" fn sw_free(pointer: *mut c_void) {
    // SAFETY: the caller passes null or a live allocation.
    unsafe { libc::free(pointer) }
}

/// `void *sw_calloc(size_t, size_t)`.
#[must_use]
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z9sw_callocmm"))]
pub extern "C" fn sw_calloc(count: usize, size: usize) -> *mut c_void {
    crate::sys::calloc(count, size)
}

/// `swoole_rand(min, max)`: uniform over the closed range; `min` when it is empty.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z11swoole_randii")
)]
pub extern "C" fn swoole_rand(min: c_int, max: c_int) -> c_int {
    let Some(span) = u64::try_from(i64::from(max) - i64::from(min) + 1)
        .ok()
        .filter(|span| *span > 0)
    else {
        return min;
    };
    let limit = u64::MAX - u64::MAX % span;
    loop {
        let value = next();
        if value < limit {
            let offset = i64::try_from(value % span).unwrap_or_default();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "min + offset lies in min..=max"
            )]
            return (i64::from(min) + offset) as c_int;
        }
    }
}

static DEV_RANDOM: AtomicI32 = AtomicI32::new(-1);

/// `int swoole_system_random(int min, int max)`: four bytes from a cached
/// `/dev/urandom` descriptor mapped onto `min..=max`; [`swoole_rand`] when it can't
/// be opened. A short read warns and returns -1.
///
/// # Panics
/// Unless `max > min`, as upstream asserts.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_system_randomii")
)]
pub extern "C" fn swoole_system_random(min: c_int, max: c_int) -> c_int {
    assert!(max > min, "swoole_system_random: max > min");
    let mut fd = DEV_RANDOM.load(Ordering::Relaxed);
    if fd == -1 {
        fd = crate::sys::open(c"/dev/urandom", libc::O_RDONLY, 0);
        DEV_RANDOM.store(fd, Ordering::Relaxed);
        if fd < 0 {
            return swoole_rand(min, max);
        }
    }
    let mut value = [0u8; 4];
    let read = crate::sys::read(fd, &mut value);
    if read < 4 {
        crate::log::sys_warning(
            c"swoole_system_random",
            format_args!("read() from /dev/urandom failed"),
        );
        return -1;
    }
    // Upstream's `min + (random % (max - min + 1))` in unsigned arithmetic.
    let width = max.wrapping_sub(min).wrapping_add(1).cast_unsigned();
    let value = u32::from_ne_bytes(value);
    let offset = value.checked_rem(width).unwrap_or(value);
    min.cast_unsigned().wrapping_add(offset).cast_signed()
}

/// `dup2` onto stdout, then stderr: each call's errno, 0 when it succeeded.
fn redirect(new_fd: c_int) -> [c_int; 2] {
    [libc::STDOUT_FILENO, libc::STDERR_FILENO]
        .map(|target| if dup2(new_fd, target) < 0 { errno() } else { 0 })
}

/// `void swoole_redirect_stdout(int new_fd)`: `dup2` onto stdout, then stderr.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_redirect_stdouti")
)]
pub extern "C" fn swoole_redirect_stdout(new_fd: c_int) {
    let failed = redirect(new_fd);
    let origin = c"swoole_redirect_stdout";
    if failed[0] != 0 {
        crate::log::sys_warning_errno(
            failed[0],
            origin,
            format_args!("dup2(STDOUT_FILENO) failed"),
        );
    }
    if failed[1] != 0 {
        crate::log::sys_warning_errno(
            failed[1],
            origin,
            format_args!("dup2(STDERR_FILENO) failed"),
        );
    }
}

/// `void swoole_redirect_stdout(const char *file)`: append to `file`, created 0644.
pub fn swoole_redirect_stdout_file(file: &CStr) {
    let fd = crate::sys::open(file, libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT, 0o644);
    if fd >= 0 {
        swoole_redirect_stdout(fd);
        close(fd);
    } else {
        let file = file.to_string_lossy();
        crate::log::sys_warning(
            c"swoole_redirect_stdout",
            format_args!("open('{file}') failed"),
        );
    }
}

/// `void swoole_print_backtrace_on_error()`: the backtrace and the host's own,
/// when `print_backtrace_on_error` is set.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z31swoole_print_backtrace_on_errorv")
)]
pub extern "C" fn swoole_print_backtrace_on_error() {
    if !crate::swr_v1_core_print_backtrace_on_error() {
        return;
    }
    swoole_print_backtrace();
    let host = crate::swr_v1_core_print_backtrace();
    // SAFETY: the host stores null or a `void (*)()` in `SwooleG.print_backtrace`.
    let host = unsafe { std::mem::transmute::<*mut c_void, Option<extern "C" fn()>>(host) };
    if let Some(host) = host {
        host();
    }
}

thread_local! {
    static EXITING: Cell<bool> = const { Cell::new(false) };
}

/// `void swoole_exit(int status)`. Concurrent exits from several threads can
/// crash, so callers queue on a lock that the exiting thread keeps; an exit
/// handler on that thread that exits again goes straight through.
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z11swoole_exiti"))]
pub extern "C" fn swoole_exit(status: c_int) -> ! {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if !EXITING.replace(true) {
        #[expect(
            clippy::mem_forget,
            reason = "the exit lock stays held until the process ends"
        )]
        std::mem::forget(LOCK.lock());
    }
    // SAFETY: exit(3) never returns.
    unsafe { exit(status) }
}

/// Commit libc formatting's bounded result and terminator. Native keeps only
/// the platform `va_list` effect and forwards its exact returned integer.
/// # Safety
/// buffer covers size writable bytes, with a positive bound unless
/// `zero_query` is true and size is zero. formatted is libc's returned count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_format_result(
    buffer: *mut c_char,
    size: usize,
    formatted: c_int,
    zero_query: bool,
) -> usize {
    if zero_query && size == 0 {
        return formatted.c_cast::<usize>();
    }
    let result = if formatted < 0 {
        0
    } else {
        usize::try_from(formatted).unwrap_or(0).min(size - 1)
    };
    if formatted < 0 || usize::try_from(formatted).is_ok_and(|value| value >= size) {
        // SAFETY: the caller's positive bounded destination contains this terminator slot.
        unsafe { buffer.wrapping_add(result).write(0) };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn formatting_result_keeps_query_error_and_termination_boundaries() {
        unsafe {
            assert_eq!(
                swr_v1_core_format_result(std::ptr::null_mut(), 0, -1, true),
                usize::MAX
            );
            assert_eq!(
                swr_v1_core_format_result(std::ptr::null_mut(), 0, 17, true),
                17
            );
            for size in [1usize, 2, 8, 16] {
                for formatted in [-7, -1, 0, 1, 7, 8, 16, 99] {
                    let mut bytes = [0x55u8; 18];
                    let mut expected = bytes;
                    let length = if formatted < 0 {
                        0
                    } else {
                        usize::try_from(formatted).unwrap().min(size - 1)
                    };
                    if formatted < 0 || usize::try_from(formatted).is_ok_and(|count| count >= size)
                    {
                        expected[length] = 0;
                    }
                    assert_eq!(
                        swr_v1_core_format_result(
                            bytes.as_mut_ptr().cast(),
                            size,
                            formatted,
                            false
                        ),
                        length
                    );
                    assert_eq!(bytes, expected);
                }
            }
        }
    }
    #[test]
    fn dump_bin_skips_types_without_a_size() {
        swoole_dump_bin(&[], c_char::from_ne_bytes(*b"x"));
    }

    #[test]
    fn rand_covers_the_closed_range_only() {
        let mut seen = [false; 7];
        for _ in 0..2000 {
            let value = swoole_rand(-3, 3);
            assert!((-3..=3).contains(&value), "{value} out of range");
            seen[usize::try_from(value + 3).unwrap()] = true;
        }
        assert!(seen.iter().all(|hit| *hit), "every value drawn");
        assert_eq!(swoole_rand(5, 5), 5);
        assert_eq!(swoole_rand(5, 4), 5, "empty range");
        let full = swoole_rand(c_int::MIN, c_int::MAX);
        assert!((c_int::MIN..=c_int::MAX).contains(&full), "full range");
        assert_ne!(swoole_random_int(), swoole_random_int());
    }

    #[test]
    fn system_random_reads_urandom_into_the_range() {
        for _ in 0..100 {
            let value = swoole_system_random(10, 20);
            assert!((10..=20).contains(&value), "{value} out of range");
        }
    }

    #[test]
    fn microtime_is_wall_clock_seconds() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert!((microtime() - now).abs() < 1.0, "within a second");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no snprintf")]
    fn datahead_dump_formats_and_truncates_like_sw_snprintf() {
        let head = DataHead {
            fd: -7,
            msg_id: u64::MAX,
            len: 12,
            reactor_id: -1,
            kind: 3,
            flags: 255,
            server_fd: 9,
            ext_flags: 65535,
            reserved: 0,
            time: 1.5,
        };
        let mut buffer = [0; 512];
        let n = datahead_dump(&head, &mut buffer);
        let text = CStr::from_bytes_until_nul(&buffer)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(text.len(), n);
        let body = &text[text.find(']').unwrap() + 1..];
        assert_eq!(
            body,
            "\n{\n    long fd = -7;\n    uint64_t msg_id = 18446744073709551615;\n    uint32_t len = 12;\n    int16_t reactor_id = -1;\n    uint8_t type = 3;\n    uint8_t flags = 255;\n    uint16_t server_fd = 9;\n    uint16_t ext_flags = 65535;\n    double time = 1.500000;\n}\n"
        );
        let mut short = [0x55; 8];
        let n = datahead_dump(&head, &mut short);
        assert_eq!((n, short[7]), (7, 0));
    }

    /// Upstream `TEST(base, datahead_dump)`, its inputs and its assertion as written:
    /// `std::string::find` returns `npos` (greater than 1) when the text is absent.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no snprintf")]
    fn base_datahead_dump() {
        let head = DataHead {
            fd: 123,
            msg_id: 0,
            len: 0,
            reactor_id: 0,
            kind: 0,
            flags: 0,
            server_fd: 0,
            ext_flags: 0,
            reserved: 0,
            time: 0.0,
        };
        let mut buffer = [0; 128];
        let n = datahead_dump(&head, &mut buffer);
        let text = String::from_utf8_lossy(&buffer[..n]);
        assert!(text.find("int fd = 123;").unwrap_or(usize::MAX) > 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn shell_exec_pipes_the_command_output() {
        let mut pid = 0;
        let fd = swoole_shell_exec(c"echo out; echo err >&2", &mut pid, true);
        assert!(fd >= 0 && pid > 0, "spawned");
        let mut output = Vec::new();
        let mut chunk = [0u8; 64];
        loop {
            let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            if n <= 0 {
                break;
            }
            output.extend_from_slice(&chunk[..usize::try_from(n).unwrap()]);
        }
        close(fd);
        let mut status = 0;
        unsafe { libc::waitpid(pid, &raw mut status, 0) };
        assert_eq!(output, b"out\nerr\n");
    }

    #[test]
    fn redirect_reports_each_dup2_errno() {
        assert_eq!(redirect(-1), [libc::EBADF, libc::EBADF]);
    }
}
