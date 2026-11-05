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
mod hook;
mod host;
mod lifecycle;
pub mod net_types;
mod random;
pub use global::*;
pub use hook::swr_v1_corebase_hook_add_action;
pub use host::{
    DataHead, datahead_dump, microtime, sw_calloc, sw_free, sw_malloc, sw_realloc,
    swoole_dump_ascii, swoole_dump_bin, swoole_dump_hex, swoole_hex2dec, swoole_rand,
    swoole_rand_any, swoole_random_int, swr_v1_corebase_print_backtrace,
    swr_v1_corebase_redirect_stdout, swr_v1_corebase_shell_exec, swr_v1_corebase_system_random,
    swr_v1_corebase_wait_for,
};
pub use lifecycle::{
    LifecycleState, swr_v1_corebase_lifecycle_next, swr_v1_corebase_task_tmp_next,
};
pub use random::{swoole_random_string, swr_v1_corebase_random_char};

use std::ffi::{c_char, c_int, c_long, c_ulong};
use std::{cmp::Ordering, ptr, slice};

unsafe extern "C" {
    fn strtol(input: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long;
    fn strtoul(input: *const c_char, end: *mut *mut c_char, base: c_int) -> c_ulong;
    fn strncasecmp(left: *const c_char, right: *const c_char, count: usize) -> c_int;
    fn toupper(value: c_int) -> c_int;
}

/// Parse the successful, representable portion of `std::stoi` using the host
/// decimal parser. Native code retains the throwing path and output publication.
/// # Safety
/// `input` is a valid NUL-terminated string and `output` is writable and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_parse_stoi(
    input: *const c_char,
    output: *mut c_int,
) -> u8 {
    if input.is_null() || output.is_null() {
        return 0;
    }
    let mut end = ptr::null_mut();
    // SAFETY: `input` is a non-null, NUL-terminated string; `end` is a live local.
    let value = unsafe { strtol(input, &raw mut end, 10) };
    if end.cast_const() == input {
        return 0;
    }
    let Ok(value) = c_int::try_from(value) else {
        return 0;
    };
    // SAFETY: `output` is non-null, writable and disjoint from `input`.
    unsafe { output.write(value) };
    1
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

/// Select the native warning, error, or accepted-value path for systemd fds.
/// Native environment parsing and all logging/error-state effects stay outside Rust.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_corebase_systemd_listen_fds_action(
    found: u8,
    count: i32,
    maximum: i32,
) -> u32 {
    if found == 0 {
        1 // missing LISTEN_FDS: native warning, then -1
    } else if count >= maximum {
        2 // too many: native error-log path, then -1
    } else {
        0 // native returns the original parsed signed value
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

/// Target-default char mode retained for the frozen two-argument ABI.
/// # Safety
/// Same input/output requirements as `swr_v1_corebase_hex_mode`; native char mode
/// must match this Rust target's default `c_char`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_hex(input: *const c_char, parsed: *mut usize) -> c_ulong {
    // SAFETY: forwards the caller's own input and output contract unchanged.
    unsafe { swr_v1_corebase_hex_mode(input, parsed, u8::from(c_char::MIN != 0)) }
}

/// Preserve native prefix, locale, signed-char and `size_t` wrap semantics.
/// # Safety
/// Input is a valid NUL-terminated C string; parsed is writable and disjoint.
/// Bytes passed to toupper must satisfy the original host's ctype precondition.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_hex_mode(
    input: *const c_char,
    parsed: *mut usize,
    native_char_signed: u8,
) -> c_ulong {
    let mut current = input;
    let mut value = 0_usize;
    // `current` walks the caller's NUL-terminated string and stops at the first non-digit
    // (at the latest, the NUL), so every read below is inside it.
    // SAFETY: `parsed` is writable and disjoint from `input`.
    unsafe { parsed.write(0) };
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
    // SAFETY: `parsed` is writable and disjoint from `input`.
    unsafe { parsed.write(current.addr() - input.addr()) };
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

/// Return the prefix length for native `std::string` construction, including root.
/// # Safety
/// Nonempty input is readable for length bytes, with length at most `isize::MAX`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_dirname(input: *const u8, length: usize) -> usize {
    if length == 0 {
        return 0;
    }
    // SAFETY: nonempty input lends `length <= isize::MAX` readable bytes.
    unsafe { slice::from_raw_parts(input, length) }
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |index| index.max(1))
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
/// bytes, disjoint from input and initialized state. State starts zeroed, then
/// stays unmodified between calls; output stays unchanged by native IO. Stop on
/// any result other than1. All pointers remain valid during each call only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_mkdir_next(
    input: *const u8,
    input_length: usize,
    output: *mut u8,
    capacity: usize,
    state: *mut MkdirState,
) -> c_int {
    if output.is_null() || state.is_null() || capacity == 0 || capacity > isize::MAX as usize {
        return 3;
    }
    // SAFETY: non-null, initialized state lent exclusively for this call.
    let state = unsafe { &mut *state };
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
                        &raw mut state,
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

    #[test]
    fn wrapped_product_precedes_division() {
        assert_eq!(swoole_common_multiple(65536, 65536), 0);
        assert_eq!(swoole_common_divisor(u32::MAX, u32::MAX - 1), 1);
        assert_eq!(swoole_common_multiple(4, 6), 12);
    }

    #[test]
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
        assert_eq!(swr_v1_corebase_systemd_listen_fds_action(0, 0, 60000), 1);
        assert_eq!(
            swr_v1_corebase_systemd_listen_fds_action(1, 59999, 60000),
            0
        );
        assert_eq!(
            swr_v1_corebase_systemd_listen_fds_action(1, 60000, 60000),
            2
        );
        assert_eq!(
            swr_v1_corebase_systemd_listen_fds_action(1, 60001, 60000),
            2
        );
        assert_eq!(swr_v1_corebase_systemd_listen_fds_action(1, -1, 60000), 0);
    }

    #[test]
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
                unsafe { swr_v1_corebase_parse_stoi(input.as_ptr(), &raw mut output) },
                1
            );
            assert_eq!(output, expected);
        }
        for text in ["", "x", "  ", "+", "2147483648", "-2147483649"] {
            let input = std::ffi::CString::new(text).unwrap();
            let mut output = 0x1357_9bdf;
            assert_eq!(
                unsafe { swr_v1_corebase_parse_stoi(input.as_ptr(), &raw mut output) },
                0
            );
            assert_eq!(output, 0x1357_9bdf);
        }
    }

    #[test]
    fn native_hex_accepts_non_hex_letters_and_counts_prefix() {
        let mut parsed = 0;
        unsafe {
            assert_eq!(
                swr_v1_corebase_hex(c"FFtail".as_ptr(), &raw mut parsed),
                16_833_333
            );
            assert_eq!(parsed, 6);
            assert_eq!(swr_v1_corebase_hex(c"0x".as_ptr(), &raw mut parsed), 0);
            assert_eq!(parsed, 2);
        }
    }

    #[test]
    fn borrowed_binary_path_and_output_bounds() {
        assert_eq!(unsafe { swr_v1_corebase_dirname(b"a\0/b".as_ptr(), 4) }, 2);
        let mut output = [0xa5; 2];
        assert_eq!(unsafe { radix(0, 10, output.as_mut_ptr(), 2) }, usize::MAX);
        assert_eq!(output, [0xa5; 2]);
        let mut decimal = [0xa5u8; 4];
        assert_eq!(unsafe { swoole_itoa(decimal.as_mut_ptr().cast(), -1) }, 2);
        assert_eq!(decimal, [b'-', b'1', 0, 0xa5]);
    }
}
