//! Borrowed inline utility policy. Locale classification remains the host libc.
mod span;
pub use span::*;
unsafe extern "C" {
    fn strncmp(a: *const std::ffi::c_char, b: *const std::ffi::c_char, n: usize) -> i32;
    fn isspace(value: std::ffi::c_int) -> std::ffi::c_int;
    fn tolower(value: std::ffi::c_int) -> std::ffi::c_int;
    fn strncasecmp(a: *const std::ffi::c_char, b: *const std::ffi::c_char, n: usize) -> i32;
}

/// Lowercase a native char buffer in place, retaining host libc locale semantics.
/// # Safety
/// Positive length requires an exclusive initialized writable range of that many
/// bytes. Each promoted char must be a defined native tolower argument (unsigned
/// char or EOF). Locale changes obey libc's concurrency rules. No borrow survives
/// return; nonpositive lengths perform no access, as the original traversal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_lowercase(data: *mut u8, length: i32, signed_char: u8) {
    if length <= 0 || data.is_null() {
        return;
    }
    // SAFETY: non-null, positive `length` (checked above); an exclusive writable range.
    let data = unsafe { std::slice::from_raw_parts_mut(data, length.unsigned_abs() as usize) };
    for byte in data {
        let value = if signed_char != 0 {
            i32::from(byte.cast_signed())
        } else {
            i32::from(*byte)
        };
        // SAFETY: the caller guarantees each promoted char is a defined tolower argument.
        *byte = unsafe { tolower(value) }.to_le_bytes()[0];
    }
}

unsafe fn bytes<'a>(data: *const u8, length: usize) -> Option<&'a [u8]> {
    if length == 0 {
        Some(&[])
    } else if data.is_null() || length > isize::MAX as usize {
        None
    } else {
        // SAFETY: non-null, `length <= isize::MAX`; the caller lends that many readable bytes.
        Some(unsafe { std::slice::from_raw_parts(data, length) })
    }
}

/// Return the whitespace prefix count (right=0), or trimmed suffix length (right=1).
/// # Safety
/// Data is readable for length bytes; null is allowed for empty input. Input bytes
/// must have defined native isspace arguments under the selected char signedness.
/// No pointer is retained; host locale changes must obey libc's concurrency rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_trim(
    data: *const u8,
    length: usize,
    right: u8,
    signed_char: u8,
) -> usize {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let Some(data) = (unsafe { bytes(data, length) }) else {
        return 0;
    };
    let whitespace = |byte: &u8| {
        let value = if signed_char != 0 {
            i32::from(byte.cast_signed())
        } else {
            i32::from(*byte)
        };
        // SAFETY: the caller guarantees each promoted byte is a defined isspace argument.
        unsafe { isspace(value) != 0 }
    };
    if right != 0 {
        data.iter()
            .rposition(|b| !whitespace(b))
            .map_or(0, |p| p + 1)
    } else {
        data.iter()
            .position(|b| *b == 0 || !whitespace(b))
            .unwrap_or(length)
    }
}

/// Bounded native search: binary=0, native case-insensitive=1, reverse=2.
/// Reverse deliberately omits position zero; case-insensitive preserves the
/// original exact first-byte check and locale/NUL-aware strncasecmp remainder.
/// # Safety
/// Ranges are initialized/readable, disjoint or shared immutable. Needle is
/// nonempty; reverse requires length>=count. Undefined original bounds return -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_find(
    data: *const u8,
    length: usize,
    needle: *const u8,
    count: usize,
    mode: u8,
) -> isize {
    if count == 0 || count > length {
        return -1;
    }
    // SAFETY: the caller lends `length` initialized bytes at `data`.
    let data = unsafe { bytes(data, length) };
    // SAFETY: the caller lends `count` initialized bytes at `needle`.
    let needle = unsafe { bytes(needle, count) };
    let (Some(data), Some(needle)) = (data, needle) else {
        return -1;
    };
    let found = match mode {
        0 => memchr::memmem::find(data, needle),
        1 => needle.first().and_then(|&first| {
            memchr::memchr_iter(first, data.get(..=length - count)?).find(|&p| {
                data.get(p..).is_some_and(|rest| {
                    // SAFETY: `p <= length - count`, so both ranges hold `count` readable bytes.
                    unsafe { strncasecmp(rest.as_ptr().cast(), needle.as_ptr().cast(), count) == 0 }
                })
            })
        }),
        2 => data
            .get(1..)
            .and_then(|data| memchr::memmem::rfind(data, needle))
            .map(|p| p + 1),
        _ => None,
    };
    found.map_or(-1, |p| isize::try_from(p).unwrap_or(-1))
}

/// Equality/prefix policy: C equality=0/1 (case-sensitive/libc casefold),
/// C prefix=2/3, binary prefix=4, binary suffix=5.
/// # Safety
/// Only compared ranges need be initialized/readable (C modes stop at NUL).
/// Nonempty compared inputs are nonnull; binary ranges fit `isize::MAX`.
/// No pointer is retained; libc locale changes follow its native rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_compare(
    data: *const u8,
    length: usize,
    needle: *const u8,
    count: usize,
    mode: u8,
) -> u8 {
    if count > length || (mode <= 1 && count != length) || mode > 5 {
        return 0;
    }
    if count == 0 {
        return 1;
    }
    if data.is_null() || needle.is_null() {
        return 0;
    }
    let equal = match mode {
        // SAFETY: both are non-null (checked above); C comparison stops at `count` or NUL,
        // within the caller's readable ranges.
        0 | 2 => unsafe { strncmp(data.cast(), needle.cast(), count) == 0 },
        // SAFETY: as above.
        1 | 3 => unsafe { strncasecmp(data.cast(), needle.cast(), count) == 0 },
        4 | 5 => {
            if count > isize::MAX as usize || (mode == 5 && length > isize::MAX as usize) {
                return 0;
            }
            let offset = if mode == 5 { length - count } else { 0 };
            if offset > isize::MAX as usize {
                return 0;
            }
            // SAFETY: non-null; `offset + count <= length <= isize::MAX`, inside the readable range.
            let compared = unsafe { std::slice::from_raw_parts(data.wrapping_add(offset), count) };
            // SAFETY: non-null, `count <= isize::MAX`, and the caller lends `count` readable bytes.
            let needle = unsafe { std::slice::from_raw_parts(needle, count) };
            compared == needle
        }
        _ => return 0,
    };
    u8::from(equal)
}

/// First separator: bytes before it or bytes after it; -1 if absent.
/// # Safety
/// Range is initialized/readable, null only if empty; no pointer is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_partition(
    data: *const u8,
    length: usize,
    separator: u8,
    before: u8,
) -> isize {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let Some(data) = (unsafe { bytes(data, length) }) else {
        return -1;
    };
    memchr::memchr(separator, data).map_or(-1, |offset| {
        if before != 0 {
            isize::try_from(offset).unwrap_or(-1)
        } else {
            isize::try_from(length - offset - 1).unwrap_or(-1)
        }
    })
}
