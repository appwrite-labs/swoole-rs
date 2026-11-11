//! Header and URL decisions for the existing native PHP request callbacks.
use crate::Text;
use core::ffi::{c_char, c_int};

unsafe extern "C" {
    fn strncasecmp(left: *const c_char, right: *const c_char, count: usize) -> c_int;
    fn isspace(value: c_int) -> c_int;
}

pub(crate) fn case_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && case_prefix(left, right)
}
/// True when libc's case-insensitive comparison differs at this first byte pair in
/// every locale, so the call can be skipped: `tolower` keeps ASCII in ASCII except
/// Turkish `I` (to `ı`), and only Turkish `İ` lowers to an ASCII byte (`i`).
fn differs_at_first(left: u8, right: u8) -> bool {
    left.is_ascii()
        && right.is_ascii()
        && left | 0x20 != b'i'
        && right | 0x20 != b'i'
        && !left.eq_ignore_ascii_case(&right)
}
pub(crate) fn case_prefix(left: &[u8], right: &[u8]) -> bool {
    left.len() >= right.len()
        && !matches!((left.first(), right.first()), (Some(&a), Some(&b)) if differs_at_first(a, b))
        && (right.is_empty()
            // SAFETY: both slices hold at least `right.len()` readable bytes.
            || unsafe {
                strncasecmp(left.as_ptr().cast(), right.as_ptr().cast(), right.len()) == 0
            })
}
pub(crate) unsafe fn bytes<'a>(text: Text) -> Option<&'a [u8]> {
    if text.length > isize::MAX as usize || (text.data.is_null() && text.length != 0) {
        return None;
    }
    if text.length == 0 {
        Some(&[])
    } else {
        // SAFETY: non-null, `length <= isize::MAX`, and the caller lends that many
        // initialized bytes for the borrow's lifetime.
        Some(unsafe { core::slice::from_raw_parts(text.data, text.length) })
    }
}

/// What the parser does with a request header beyond storing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestHeader {
    Append,
    Cookie,
    Upgrade,
    Form,
    Multipart,
    Compression,
}

/// The header's action. Method is the pinned llhttp method number.
#[must_use]
pub fn request_header(
    name: &[u8],
    value: &[u8],
    method: u8,
    cookie: bool,
    compression: bool,
) -> RequestHeader {
    if cookie && case_equal(name, b"cookie") {
        return RequestHeader::Cookie;
    }
    if case_equal(name, b"upgrade") {
        return RequestHeader::Upgrade;
    }
    // DELETE, POST, PUT, PATCH and QUERY (llhttp 46, upstream cf66ff7e8) carry form bodies.
    if matches!(method, 0 | 3 | 4 | 28 | 46) && case_equal(name, b"content-type") {
        if case_prefix(value, b"application/x-www-form-urlencoded") {
            return RequestHeader::Form;
        }
        if case_prefix(value, b"multipart/form-data") {
            return RequestHeader::Multipart;
        }
    } else if compression && case_equal(name, b"accept-encoding") {
        return RequestHeader::Compression;
    }
    RequestHeader::Append
}

/// The known header keys, in the order `request_key` numbers them.
pub const REQUEST_KEYS: [&[u8]; 8] = [
    b"host",
    b"user-agent",
    b"accept",
    b"content-type",
    b"content-length",
    b"authorization",
    b"connection",
    b"accept-encoding",
];

/// The name's index in `REQUEST_KEYS`, or `None` for the unknown-name merge path.
#[must_use]
pub fn request_key(name: &[u8]) -> Option<usize> {
    REQUEST_KEYS.iter().position(|key| case_equal(name, key))
}

/// Native first calls mode0 on the original span, preserving its no-copy fast
/// path. A return2 requests native scratch copy+terminator before mode1; mode1
/// mutates that scratch exactly like `strtok_r` and the original trim helpers.
/// # Safety
/// Read spans are initialized; mode1 additionally requires writable data with a
/// terminator at data[length]. Value is readable through its native C-string terminator on scan paths, or for the bounded comparison on fast paths. `fast_extent` preserves the native pointer-size expression.
pub unsafe fn swr_v1_httpbindings_tokens(
    data: *mut u8,
    length: usize,
    value: *const u8,
    fast_extent: usize,
    mode: u32,
) -> i32 {
    // SAFETY: `data` spans `length` initialized bytes (null only when empty) for this call.
    let Some(input) = (unsafe { bytes(Text { data, length }) }) else {
        return -1;
    };
    if mode == 0 {
        if length == 0 {
            return 0;
        }
        if length != fast_extent {
            return 2;
        }
        if value.is_null() {
            return -1;
        }
        // SW_STRCASEEQ uses sizeof(value)-1 on a pointer in the pinned helper.
        // Preserve its architecture-dependent bounded comparison, not strlen.
        // SAFETY: `data` spans `length` bytes and the caller makes `value` readable for
        // this bounded comparison.
        return if unsafe { strncasecmp(data.cast(), value.cast(), length) } == 0 {
            1
        } else {
            2
        };
    }
    if mode != 1 {
        return -1;
    }
    if length == 0 {
        return 0;
    }
    let end = input.iter().position(|&c| c == 0).unwrap_or(length);
    // Raw byte effects permit the original value pointer to alias scratch.
    // Immutable token/value views exist only during the final pure comparison.
    // Every index stays below `end <= length`, inside the caller's writable scratch.
    let read = |index: usize| {
        // SAFETY: as above.
        unsafe { data.wrapping_add(index).read() }
    };
    let write = |index: usize, byte| {
        // SAFETY: as above.
        unsafe { data.wrapping_add(index).write(byte) }
    };
    let mut cursor = 0;
    while cursor < end {
        while cursor < end && read(cursor) == b',' {
            cursor += 1;
        }
        if cursor == end {
            break;
        }
        let start = cursor;
        while cursor < end && read(cursor) != b',' {
            cursor += 1;
        }
        let stop = cursor;
        if cursor < end {
            write(cursor, 0);
            cursor += 1;
        }
        let mut first = start;
        let mut last = stop;
        while first < last
            // SAFETY: isspace receives the native char value, as the pinned helper did.
            && unsafe { isspace(c_int::from(c_char::from_ne_bytes([read(first)]))) } != 0
        {
            first += 1;
        }
        while last > first
            // SAFETY: as above.
            && unsafe { isspace(c_int::from(c_char::from_ne_bytes([read(last - 1)]))) } != 0
        {
            last -= 1;
            write(last, 0);
        }
        if value.is_null() {
            return -1;
        }
        // SAFETY: non-null and, on scan paths, readable through its C-string terminator.
        let value = unsafe { core::ffi::CStr::from_ptr(value.cast()) }.to_bytes();
        // SAFETY: `first <= last <= end <= length`, inside the scratch, and no write
        // happens while this view is alive.
        let token = unsafe { core::slice::from_raw_parts(data.wrapping_add(first), last - first) };
        if case_equal(token, value) {
            return 1;
        }
    }
    0
}

#[repr(C)]
#[derive(Default)]
pub struct UrlParts {
    pub path_length: usize,
    pub query_offset: usize,
    pub query_length: usize,
}

/// # Safety
/// Input contains initialized bytes.
#[must_use]
pub unsafe fn swr_v1_httpbindings_request_url(input: Text, output: Option<&mut UrlParts>) -> i32 {
    // SAFETY: `input` spans `length` initialized bytes (null only when empty) for this call.
    let input = unsafe { bytes(input) };
    let (Some(input), Some(output)) = (input, output) else {
        return -1;
    };
    let path_length = input.iter().position(|&c| c == b'?').unwrap_or(input.len());
    let query_offset = if path_length < input.len() {
        path_length + 1
    } else {
        input.len()
    };
    *output = UrlParts {
        path_length,
        query_offset,
        query_length: input.len() - query_offset,
    };
    0
}

#[repr(C)]
#[derive(Default)]
pub struct Trimmed {
    pub offset: usize,
    pub length: usize,
}

/// # Safety
/// Data is initialized/writable for length bytes. The original shrinking
/// left-loop bound is intentional; this is not `trim_matches`.
pub unsafe fn swr_v1_httpbindings_trim_quotes(
    data: *mut u8,
    length: usize,
    output: Option<&mut Trimmed>,
) -> i32 {
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    let Some(output) = output else {
        return -1;
    };
    let input = if length == 0 {
        &mut []
    } else {
        // SAFETY: non-null, `length <= isize::MAX`, initialized and exclusively writable.
        unsafe { core::slice::from_raw_parts_mut(data, length) }
    };
    // Leading quotes go while they are fewer than what remains after them.
    let offset = input
        .iter()
        .take_while(|&&byte| byte == b'"')
        .count()
        .min(length.div_ceil(2));
    let rest = input.get_mut(offset..).unwrap_or_default();
    let mut remaining = rest.len();
    for byte in rest.iter_mut().rev().take_while(|byte| **byte == b'"') {
        *byte = 0;
        remaining -= 1;
    }
    *output = Trimmed {
        offset,
        length: remaining,
    };
    0
}

/// Canonical header case (`content-type` to `Content-Type`), keeping the
/// original state machine's leading and consecutive hyphen behaviour.
pub fn canonical_case(name: &mut [u8]) {
    let mut initial = true;
    for byte in name {
        if initial {
            *byte = byte.to_ascii_uppercase();
            initial = false;
        } else if *byte == b'-' {
            initial = true;
        } else {
            *byte = byte.to_ascii_lowercase();
        }
    }
}
