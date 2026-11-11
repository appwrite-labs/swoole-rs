#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.2 src/core/base.cc (Swoole contributors).
//! Core numeric and byte helpers; native code retains allocation and effect custody.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
pub mod constants;
mod global;
pub mod hook;
mod host;
mod lifecycle;
pub mod log;
pub mod native;
pub mod net_address;
pub mod net_types;
mod random;
mod signal_mask;
pub mod sys;
pub use sys::set_errno;
pub mod switches;
pub use switches::Switch;

unsafe extern "C" {
    /// native/corebase/switches.cc: a bit per [`Switch`] the build has.
    safe static swr_cpp_switches: u64;
}

/// Whether the build compiled C++ with `switch` (its `#if` in [`switches::SWITCHES`]).
#[must_use]
pub fn built_with(switch: Switch) -> bool {
    swr_cpp_switches & (1 << switch as u8) != 0
}
pub use signal_mask::{
    signal_block_all, signal_blocking_all, signal_unblock_all, with_signals_blocked,
};
mod thread;
pub mod worker_registry;
pub use global::*;
pub use hook::{swoole_add_hook, swoole_call_hook, swoole_isset_hook};
pub use host::{
    DataHead, datahead_dump, microtime, sw_calloc, sw_free, sw_malloc, sw_realloc,
    swoole_dump_ascii, swoole_dump_bin, swoole_dump_hex, swoole_exit, swoole_hex2dec,
    swoole_print_backtrace, swoole_print_backtrace_on_error, swoole_rand, swoole_rand_any,
    swoole_random_int, swoole_redirect_stdout, swoole_redirect_stdout_file, swoole_shell_exec,
    swoole_system_random, swr_v1_core_format_result,
};
pub use lifecycle::{SharedThread, adopt_shared_thread, shared_thread, thread_buffer};
pub use lifecycle::{
    ThreadGlobal, fork_exec, set_thread, set_thread_async_threads, set_thread_message_bus,
    set_thread_reactor, swoole_clean, swoole_fork, swoole_is_main_thread, swoole_thread_clean,
    swoole_thread_init, swr_v1_core_swoole_init, swr_v1_core_thread_global, swr_v1_core_timer_slot,
    thread_async_threads, thread_id, thread_kind, thread_message_bus, thread_reactor, thread_timer,
};
pub use random::{random_bytes, swoole_random_string};
pub use thread::*;

use std::ffi::{CStr, c_char, c_int, c_long, c_ulong};
use std::{cmp::Ordering, ptr, slice};

unsafe extern "C" {
    fn strtol(input: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long;
    fn strtoul(input: *const c_char, end: *mut *mut c_char, base: c_int) -> c_ulong;
    fn strncasecmp(left: *const c_char, right: *const c_char, count: usize) -> c_int;
    fn toupper(value: c_int) -> c_int;
}

/// `SWOOLE_VERSION`, `SWOOLE_VERSION_ID` and `SWOOLE_API_VERSION_ID` of the pin;
/// core-tests' `base.version` compares the functions below with the header's.
pub const VERSION: &std::ffi::CStr = c"6.2.5";
const VERSION_ID: c_int = 60205;
const API_VERSION_ID: c_int = 0x0202_208a;

/// `const char *swoole_version(void)`.
#[must_use]
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "swoole_version"))]
pub extern "C" fn swoole_version() -> *const c_char {
    VERSION.as_ptr()
}

/// `int swoole_version_id(void)`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "swoole_version_id")
)]
pub extern "C" fn swoole_version_id() -> c_int {
    VERSION_ID
}

/// `int swoole_api_version_id(void)`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "swoole_api_version_id")
)]
pub extern "C" fn swoole_api_version_id() -> c_int {
    API_VERSION_ID
}

/// Parse the successful, representable portion of `std::stoi` using the host
/// decimal parser, leaving errno as it was, as a successful `std::stoi` does.
/// Native code retains the throwing path.
/// # Safety
/// `input` is a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_parse_stoi(
    input: *const c_char,
    output: Option<&mut c_int>,
) -> u8 {
    let Some(output) = output else {
        return 0;
    };
    if input.is_null() {
        return 0;
    }
    let saved = sys::errno();
    sys::set_errno(0);
    let mut end = ptr::null_mut();
    // SAFETY: `input` is a non-null, NUL-terminated string; `end` is a live local.
    let value = unsafe { strtol(input, &raw mut end, 10) };
    let range = sys::errno() == libc::ERANGE;
    sys::set_errno(saved);
    if end.cast_const() == input || range {
        return 0;
    }
    let Ok(value) = c_int::try_from(value) else {
        return 0;
    };
    *output = value;
    1
}

/// `swoole_get_env(name, &value)`: the variable as `std::stoi` reads it, or `None`
/// when it is unset. Where `std::stoi` throws, which nothing above `swoole_get_env`
/// catches, the process aborts as `std::terminate` does.
#[must_use]
pub fn get_env(name: &CStr) -> Option<c_int> {
    // SAFETY: a NUL-terminated name; the value is read before the environment changes.
    let value = unsafe { libc::getenv(name.as_ptr()) };
    if value.is_null() {
        return None;
    }
    let mut output = 0;
    // SAFETY: getenv's NUL-terminated value and a live output.
    if unsafe { swr_v1_corebase_parse_stoi(value, Some(&mut output)) } == 0 {
        std::process::abort();
    }
    Some(output)
}

/// `int swoole_type_size(char)`: the byte width of a pack-style length type, or 0.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z16swoole_type_sizec")
)]
pub extern "C" fn swoole_type_size(kind: c_char) -> c_int {
    match kind.to_ne_bytes()[0] {
        b'c' | b'C' => 1,
        b's' | b'S' | b'n' | b'v' => 2,
        b'l' | b'L' | b'N' | b'V' => 4,
        b'q' | b'Q' | b'J' | b'P' => 8,
        _ => 0,
    }
}

/// `uint32_t swoole_common_divisor(uint32_t, uint32_t)`.
/// # Panics
/// On a zero operand, which upstream asserts against (a panic here aborts).
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z21swoole_common_divisorjj")
)]
pub extern "C" fn swoole_common_divisor(left: u32, right: u32) -> u32 {
    assert!(left > 0 && right > 0, "swoole_common_divisor: zero operand");
    gcd(left, right)
}

/// `uint32_t swoole_common_multiple(uint32_t, uint32_t)`: the `uint32_t` product
/// wraps before the division, as upstream's does (65536 * 65536 gives 0).
/// # Panics
/// On a zero operand, which upstream asserts against (a panic here aborts).
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_common_multiplejj")
)]
pub extern "C" fn swoole_common_multiple(left: u32, right: u32) -> u32 {
    assert!(
        left > 0 && right > 0,
        "swoole_common_multiple: zero operand"
    );
    left.wrapping_mul(right) / gcd(left, right)
}

fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

unsafe extern "C" {
    fn strndup(input: *const c_char, length: usize) -> *mut c_char;
}

/// `char *swoole_dec2hex(ulong_t value, int base)`: the digits in a fresh
/// `strndup` allocation the caller frees.
/// # Panics
/// On a base outside 2..=36, which upstream asserts against (a panic here aborts).
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z14swoole_dec2hexmi")
)]
pub extern "C" fn swoole_dec2hex(value: c_ulong, base: c_int) -> *mut c_char {
    assert!(base > 1 && base < 37, "swoole_dec2hex: base out of range");
    let mut buffer = [0_u8; c_ulong::BITS as usize + 1];
    // SAFETY: `buffer` is a live local of the capacity `radix` requires; `base` is checked.
    let first = unsafe {
        radix(
            value,
            base.cast_unsigned(),
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    };
    let digits = buffer.as_ptr().wrapping_add(first).cast();
    // SAFETY: `digits` starts the NUL-terminated digits `radix` just wrote into `buffer`.
    unsafe { strndup(digits, buffer.len() - first - 1) }
}

/// format.c's printf family under its public names, which swoole.h's inline logging
/// calls from native extensions. A C definition would stay hidden in the Cargo-built
/// swoole.so, as a cdylib exports only Rust's symbols, so each is a naked tail jump to
/// the C body, which keeps its variadic arguments in place.
macro_rules! format_exports {
    ($($export:ident => $definition:ident),* $(,)?) => {
        #[cfg(feature = "upstream-symbols")]
        unsafe extern "C" {
            $(fn $definition();)*
        }
        $(
            #[doc = concat!("`", stringify!($export), "`, exported.")]
            #[cfg(feature = "upstream-symbols")]
            #[unsafe(naked)]
            #[unsafe(no_mangle)]
            pub extern "C" fn $export() {
                #[cfg(target_arch = "x86_64")]
                core::arch::naked_asm!("jmp {}@PLT", sym $definition);
                #[cfg(target_arch = "aarch64")]
                core::arch::naked_asm!("b {}", sym $definition);
            }
        )*
    };
}

format_exports! {
    sw_vsnprintf => swr_c_sw_vsnprintf,
    sw_snprintf => swr_c_sw_snprintf,
    sw_printf => swr_c_sw_printf,
}

/// `SW_LOG_ERROR` and `SW_ERROR_SERVER_TOO_MANY_LISTEN_PORT`.
#[cfg(feature = "upstream-symbols")]
const LOG_ERROR: c_int = 5;
#[cfg(feature = "upstream-symbols")]
const TOO_MANY_LISTEN_PORT: c_int = 9005;

/// `LISTEN_FDS`'s verdict: the count to return, or which log line (`true`: too big).
#[cfg(any(test, feature = "upstream-symbols"))]
fn systemd_listen_fds(count: Option<c_int>) -> Result<c_int, bool> {
    match count {
        None => Err(false),
        Some(count) if u32::try_from(count).is_ok_and(|n| n >= constants::SW_MAX_LISTEN_PORT) => {
            Err(true)
        }
        Some(count) => Ok(count),
    }
}

/// `int swoole_get_systemd_listen_fds()`: `LISTEN_FDS`, or -1 when it is missing
/// or reaches `SW_MAX_LISTEN_PORT`.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_Z29swoole_get_systemd_listen_fdsv")]
#[must_use]
pub extern "C-unwind" fn swoole_get_systemd_listen_fds() -> c_int {
    let origin = c"swoole_get_systemd_listen_fds";
    match systemd_listen_fds(get_env(c"LISTEN_FDS")) {
        Ok(count) => count,
        Err(false) => {
            log::warning(origin, format_args!("invalid LISTEN_FDS"));
            -1
        }
        Err(true) => {
            log::error_log(
                LOG_ERROR,
                TOO_MANY_LISTEN_PORT,
                origin,
                format_args!("LISTEN_FDS is too big"),
            );
            -1
        }
    }
}

/// `int swoole_itoa(char *buf, long value)`: the decimal digits and a NUL; returns the
/// byte count before the NUL.
/// # Safety
/// `output` is writable for the value's decimal representation plus NUL.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z11swoole_itoaPcl")
)]
pub unsafe extern "C" fn swoole_itoa(output: *mut c_char, value: c_long) -> c_int {
    let mut buffer = itoa::Buffer::new();
    let bytes = buffer.format(value).as_bytes();
    // SAFETY: the caller sizes `output` for these digits plus NUL, disjoint from `buffer`.
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), bytes.len()) };
    // SAFETY: the NUL lands at offset `bytes.len()`, inside that same extent.
    unsafe { output.wrapping_add(bytes.len()).write(0) };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "a c_long has at most 20 digits"
    )]
    let length = bytes.len() as c_int;
    length
}

/// Format backwards into native scratch storage; return the first digit offset.
/// `SIZE_MAX` rejects invalid radix, null output or insufficient maximum capacity.
/// # Safety
/// Nonnull output is writable for capacity bytes, with no overlapping live borrow.
pub unsafe fn radix(mut value: c_ulong, radix: u32, output: *mut u8, capacity: usize) -> usize {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if !(2..=36).contains(&radix) || output.is_null() || capacity < c_ulong::BITS as usize + 1 {
        return usize::MAX;
    }
    let put = |index: usize, byte: u8| {
        // SAFETY: `index < capacity`; `output` is non-null and lends `capacity` writable bytes.
        unsafe { output.wrapping_add(index).write(byte) }
    };
    let mut index = capacity - 1;
    put(index, 0);
    loop {
        // At least `c_ulong::BITS` digit slots precede the NUL, enough for radix 2.
        index -= 1;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a digit is below the radix, at most 36"
        )]
        let digit = (value % c_ulong::from(radix)) as usize;
        put(index, DIGITS.get(digit).copied().unwrap_or(b'0'));
        value /= c_ulong::from(radix);
        if value == 0 {
            return index;
        }
    }
}

/// Preserve native prefix, locale, signed-char and `size_t` wrap semantics.
/// # Safety
/// Input is a valid NUL-terminated C string.
/// Bytes passed to toupper must satisfy the original host's ctype precondition.
pub unsafe fn swr_v1_corebase_hex_mode(
    input: *const c_char,
    parsed: &mut usize,
    native_char_signed: u8,
) -> c_ulong {
    let mut current = input;
    let mut value = 0_usize;
    // `current` walks the caller's NUL-terminated string and stops at the first non-digit
    // (at the latest, the NUL), so every read below is inside it.
    *parsed = 0;
    // SAFETY: `input` is NUL-terminated, so strncasecmp stops at its end; the literal is too.
    if unsafe { strncasecmp(input, c"0x".as_ptr(), 2) } == 0 {
        current = current.wrapping_add(2);
    }
    loop {
        // SAFETY: see above; `current` has not passed the NUL.
        let byte = unsafe { current.cast::<u8>().read() };
        let digit = if byte.is_ascii_digit() {
            (byte - b'0') as usize
        } else {
            // C passes the char as the host's (un)signed char would promote it.
            let input = if native_char_signed != 0 {
                c_int::from(byte.cast_signed())
            } else {
                c_int::from(byte)
            };
            // SAFETY: the caller guarantees the host ctype precondition for these bytes.
            let upper = unsafe { toupper(input) };
            let low = upper.to_le_bytes()[0];
            let byte = if native_char_signed != 0 {
                c_int::from(low.cast_signed())
            } else {
                c_int::from(low)
            };
            if !(c_int::from(b'A')..=c_int::from(b'Z')).contains(&byte) {
                break;
            }
            (byte - c_int::from(b'A')).unsigned_abs() as usize + 10
        };
        value = value.wrapping_mul(16).wrapping_add(digit);
        current = current.wrapping_add(1);
    }
    *parsed = current.addr() - input.addr();
    value as c_ulong
}

/// Compare components using the same host libc parser and observable errno order.
/// # Safety
/// Both inputs are valid NUL-terminated C strings for the duration of this call.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_version_comparePKcS0_")
)]
pub unsafe extern "C" fn swoole_version_compare(
    mut left: *const c_char,
    mut right: *const c_char,
) -> c_int {
    // `left` and `right` always point into the caller's NUL-terminated strings: strtoul
    // ends on a non-digit, and the code steps past it only when it isn't the NUL.
    loop {
        let (mut left_end, mut right_end) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: see above; `left_end` is a live local.
        let first = unsafe { strtoul(left, &raw mut left_end, 10) };
        // SAFETY: see above; `right_end` is a live local.
        let second = unsafe { strtoul(right, &raw mut right_end, 10) };
        match first.cmp(&second) {
            Ordering::Less => return -1,
            Ordering::Greater => return 1,
            Ordering::Equal => {}
        }
        // SAFETY: strtoul set `left_end` inside the left string.
        let left_done = unsafe { left_end.read() } == 0;
        // SAFETY: strtoul set `right_end` inside the right string.
        let right_done = unsafe { right_end.read() } == 0;
        match (left_done, right_done) {
            (true, true) => return 0,
            (true, false) => return -1,
            (false, true) => return 1,
            (false, false) => {
                left = left_end.wrapping_add(1);
                right = right_end.wrapping_add(1);
            }
        }
    }
}

/// Native stack-owned iterator state; no retained pointer or allocation.
#[repr(C)]
#[derive(Default)]
pub struct MkdirState {
    pub length: usize,
    pub next: usize,
}

/// Prepare/cut the next native mkdir prefix. 0=done, 1=prefix, 2=too long,
/// 3=invalid state/storage. Native access/mkdir/logging execute after return.
/// # Safety
/// Input is readable for `input_length` bytes. Output is writable for capacity
/// bytes, disjoint from input. State starts zeroed, then
/// stays unmodified between calls; output stays unchanged by native IO. Stop on
/// any result other than1. All pointers remain valid during each call only.
pub unsafe fn swr_v1_corebase_mkdir_next(
    input: *const u8,
    input_length: usize,
    output: *mut u8,
    capacity: usize,
    state: Option<&mut MkdirState>,
) -> c_int {
    let Some(state) = state else {
        return 3;
    };
    if output.is_null() || capacity == 0 || capacity > isize::MAX as usize {
        return 3;
    }
    let put = |index: usize, byte: u8| {
        // SAFETY: every caller keeps `index < capacity`; `output` lends `capacity` writable
        // bytes disjoint from the input.
        unsafe { output.wrapping_add(index).write(byte) }
    };
    if state.next == 0 {
        if input_length >= capacity {
            return 2;
        }
        if input.is_null() || input_length == 0 {
            return 3;
        }
        // SAFETY: non-null input lends `input_length < capacity <= isize::MAX` readable bytes.
        let input = unsafe { slice::from_raw_parts(input, input_length) };
        let prefix = input
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(input_length);
        let append = input.last() != Some(&b'/');
        let length = prefix + usize::from(append);
        if length >= capacity {
            return 2;
        }
        // SAFETY: `prefix <= length < capacity` writable output bytes, disjoint from input.
        unsafe { ptr::copy_nonoverlapping(input.as_ptr(), output, prefix) };
        if append {
            put(prefix, b'/');
        }
        put(length, 0);
        state.length = length;
        state.next = 1;
    } else {
        if state.length >= capacity || state.next > state.length {
            return 3;
        }
        put(state.next - 1, b'/');
    }
    for index in state.next..state.length {
        // SAFETY: `index < state.length < capacity`, written earlier by this iterator.
        if unsafe { output.wrapping_add(index).read() } == b'/' {
            put(index, 0);
            state.next = index + 1;
            return 1;
        }
    }
    0
}

/// Create every path prefix directly, retaining the public warning facade.
/// 0 success,1 syscall failure (the failing terminated prefix is in scratch),
/// 2 path bound exceeded. No allocation or native callback occurs here.
/// # Safety
/// input lends length bytes; scratch lends capacity writable bytes and is disjoint.
pub unsafe fn swr_v1_corebase_mkdir_complete(
    input: *const u8,
    length: usize,
    scratch: *mut u8,
    capacity: usize,
) -> i32 {
    let mut state = MkdirState::default();
    loop {
        // SAFETY: caller lends stable input and exclusive scratch through prefix iteration.
        let step = unsafe {
            swr_v1_corebase_mkdir_next(input, length, scratch, capacity, Some(&mut state))
        };
        if step != 1 {
            return step;
        }
        // SAFETY: the prefix cursor just terminated a readable path inside scratch.
        if unsafe { libc::access(scratch.cast(), libc::R_OK) } != 0
            // SAFETY: the same terminated prefix remains unchanged through mkdir.
            && unsafe { libc::mkdir(scratch.cast(), 0o755) } == -1
        {
            return 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkdir_prefixes_preserve_slashes_and_original_nul_order() {
        fn prefixes(input: &[u8]) -> Vec<Vec<u8>> {
            let mut storage = [std::mem::MaybeUninit::<u8>::uninit(); 64];
            let mut state = MkdirState::default();
            let mut prefixes = Vec::new();
            loop {
                let step = unsafe {
                    swr_v1_corebase_mkdir_next(
                        input.as_ptr(),
                        input.len(),
                        storage.as_mut_ptr().cast(),
                        storage.len(),
                        Some(&mut state),
                    )
                };
                if step == 0 {
                    return prefixes;
                }
                assert_eq!(step, 1);
                let prefix = unsafe { std::ffi::CStr::from_ptr(storage.as_ptr().cast()) };
                prefixes.push(prefix.to_bytes().to_vec());
            }
        }
        assert_eq!(
            prefixes(b"foo//bar"),
            [b"foo".to_vec(), b"foo/".to_vec(), b"foo//bar".to_vec()]
        );
        assert_eq!(prefixes(b"a\0/b"), [b"a".to_vec()]);
        assert_eq!(prefixes(b"a\0/b/"), [] as [std::vec::Vec<u8>; 0]);
        assert_eq!(prefixes(b"/"), [] as [std::vec::Vec<u8>; 0]);
    }

    /// Upstream `TEST(base, mkdir_recursive)`: a PATH_MAX-byte random path is refused.
    #[test]
    fn base_mkdir_recursive() {
        let max = libc::PATH_MAX as usize;
        let (mut dir, mut scratch) = (vec![0u8; max + 1], vec![0u8; max]);
        unsafe { crate::random::swoole_random_string(dir.as_mut_ptr().cast(), max) };
        let status =
            unsafe { swr_v1_corebase_mkdir_complete(dir.as_ptr(), max, scratch.as_mut_ptr(), max) };
        assert_ne!(status, 0);
    }

    #[test]
    fn wrapped_product_precedes_division() {
        assert_eq!(swoole_common_multiple(65536, 65536), 0);
        assert_eq!(swoole_common_divisor(u32::MAX, u32::MAX - 1), 1);
        assert_eq!(swoole_common_multiple(4, 6), 12);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strndup")]
    fn dec2hex_returns_an_owned_digit_string() {
        for (value, base, expected) in [
            (255, 16, c"ff"),
            (0, 2, c"0"),
            (c_ulong::MAX, 36, c"3w5e11264sgsf"),
        ] {
            let digits = swoole_dec2hex(value, base);
            assert_eq!(unsafe { std::ffi::CStr::from_ptr(digits) }, expected);
            unsafe { libc::free(digits.cast()) };
        }
    }

    #[test]
    fn systemd_listener_policy_preserves_missing_limit_and_signed_values() {
        assert_eq!(systemd_listen_fds(None), Err(false));
        assert_eq!(systemd_listen_fds(Some(59999)), Ok(59999));
        assert_eq!(systemd_listen_fds(Some(60000)), Err(true));
        assert_eq!(systemd_listen_fds(Some(60001)), Err(true));
        assert_eq!(systemd_listen_fds(Some(-1)), Ok(-1));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strtol")]
    fn stoi_parser_publishes_only_representable_decimal_values() {
        for (text, expected) in [
            ("0", 0),
            ("+42tail", 42),
            (" -2147483648x", i32::MIN),
            ("2147483647", i32::MAX),
        ] {
            let input = std::ffi::CString::new(text).unwrap();
            let mut output = 0x1357_9bdf;
            assert_eq!(
                unsafe { swr_v1_corebase_parse_stoi(input.as_ptr(), Some(&mut output)) },
                1
            );
            assert_eq!(output, expected);
        }
        for text in ["", "x", "  ", "+", "2147483648", "-2147483649"] {
            let input = std::ffi::CString::new(text).unwrap();
            let mut output = 0x1357_9bdf;
            assert_eq!(
                unsafe { swr_v1_corebase_parse_stoi(input.as_ptr(), Some(&mut output)) },
                0
            );
            assert_eq!(output, 0x1357_9bdf);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strncasecmp")]
    fn native_hex_accepts_non_hex_letters_and_counts_prefix() {
        let mut parsed = 0;
        unsafe {
            assert_eq!(swoole_hex2dec(c"FFtail".as_ptr(), &mut parsed), 16_833_333);
            assert_eq!(parsed, 6);
            assert_eq!(swoole_hex2dec(c"0x".as_ptr(), &mut parsed), 0);
            assert_eq!(parsed, 2);
        }
    }

    #[test]
    fn borrowed_binary_path_and_output_bounds() {
        let mut output = [0xa5; 2];
        assert_eq!(unsafe { radix(0, 10, output.as_mut_ptr(), 2) }, usize::MAX);
        assert_eq!(output, [0xa5; 2]);
        let mut decimal = [0xa5u8; 4];
        assert_eq!(unsafe { swoole_itoa(decimal.as_mut_ptr().cast(), -1) }, 2);
        assert_eq!(decimal, [b'-', b'1', 0, 0xa5]);
    }
}

mod string_slot;
pub use string_slot::*;

mod proxy_state;
pub use proxy_state::*;

mod config_state;
pub use config_state::*;

mod resolver;
pub use resolver::*;

mod resolver_request;
pub use resolver_request::*;

mod name_resolver;
pub use name_resolver::*;
