//! HTTP policy and private directory storage. Native effects run after Rust returns.
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_types,
        reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
    )
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::{mem::MaybeUninit, slice};
const INVALID: i32 = -1;
const OVERFLOW: i32 = -2;
mod conditional;
pub use conditional::{HttpCalendar, HttpConditional, swr_v3_http_conditional, swr_v3_http_date};
mod directory;
use directory::HttpDirectory;
mod dispatch;
pub use dispatch::transport_route;
mod framing;
pub use framing::HttpFrame;
mod headers;
mod index_page;
pub use headers::{HttpCookie, HttpHeaderInfo, cookie_next};
mod maps;
mod multipart;
pub use multipart::{multipart_disposition, multipart_filename, multipart_result};
mod proxy;
pub use proxy::{
    HttpProxyRequest, HttpProxyText, swr_v1_http_proxy_credentials, swr_v1_http_proxy_handshake,
    swr_v1_http_proxy_pack, swr_v2_http_proxy_authorization,
};
mod range;
mod range_response;
use range_response::HttpRangeFinish;
mod request;
pub use request::{
    HttpForm, HttpNative, HttpRequest, HttpString, destroy_multipart_parser, init_multipart_parser,
    parse_multipart_data, request_free, request_new,
};
mod rewrite;
pub use rewrite::RewriteRules;
mod static_context;
pub use static_context::*;
mod tasks;
pub use tasks::HttpStaticTask;
use tasks::HttpTasks;
mod static_meta;
mod status_buffer;
pub use status_buffer::{STATUS_CODES, status_message};
mod static_headers;
mod static_path;
pub use maps::{method, method_string};
use range::HttpRangeState;
pub use static_headers::{HttpStaticHeaders, swr_v1_http_static_headers};
pub use static_path::{HttpStaticPath, location_match};

/// # Safety
/// Nonempty input is readable and initialized for its length; no borrow escapes.
/// `input` up to its first NUL, or all of it.
fn c_text(input: &[u8]) -> &[u8] {
    input.split(|&b| b == 0).next().unwrap_or_default()
}
/// `input[at..]`, or empty past the end.
fn tail(input: &[u8], at: usize) -> &[u8] {
    input.get(at..).unwrap_or_default()
}
unsafe fn bytes<'a>(data: *const u8, length: usize) -> Result<&'a [u8], i32> {
    if length > isize::MAX as usize {
        return Err(OVERFLOW);
    }
    if length == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err(INVALID);
    }
    // SAFETY: non-null, `length <= isize::MAX`; callers lend that many initialized bytes.
    Ok(unsafe { slice::from_raw_parts(data, length) })
}
unsafe extern "C" {
    fn strncasecmp(
        left: *const std::ffi::c_char,
        right: *const std::ffi::c_char,
        length: usize,
    ) -> i32;
}
/// Preserve the pinned POSIX comparison's current locale and matching-NUL rules.
/// This libc leaf borrows initialized bytes only; it has no allocator, callback,
/// IO or Zend boundary. ASCII folding alone differs for configured binary paths.
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
#[inline]
fn case_prefix(input: &[u8], expected: &[u8]) -> bool {
    input.len() >= expected.len()
        && !matches!((input.first(), expected.first()), (Some(&left), Some(&right)) if differs_at_first(left, right))
        // SAFETY: both slices hold at least `expected.len()` readable bytes.
        && unsafe {
            strncasecmp(
                input.as_ptr().cast(),
                expected.as_ptr().cast(),
                expected.len(),
            ) == 0
        }
}
fn case_equal(input: &[u8], expected: &[u8]) -> bool {
    input.len() == expected.len() && case_prefix(input, expected)
}
fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~".contains(&byte)
}
fn encoded_length(input: &[u8]) -> Result<usize, i32> {
    let escaped = input.iter().filter(|&&byte| !unreserved(byte)).count();
    input
        .len()
        .checked_add(1)
        .and_then(|length| length.checked_add(escaped.checked_mul(2)?))
        .filter(|&length| isize::try_from(length).is_ok())
        .ok_or(OVERFLOW)
}
/// The encoded capacity of `input`, its NUL included.
///
/// # Errors
/// `OVERFLOW` when it exceeds `isize::MAX`.
pub fn url_capacity(input: &[u8]) -> Result<usize, i32> {
    encoded_length(input)
}
/// Percent-encode `input` into the start of `output`, NUL-terminated, returning the
/// encoded length. Nothing is written on error.
///
/// # Errors
/// `INVALID` when `output` is shorter than [`url_capacity`], `OVERFLOW` as there.
pub fn url_encode(input: &[u8], output: &mut [MaybeUninit<u8>]) -> Result<usize, i32> {
    let needed = encoded_length(input)?;
    let Some((nul, output)) = output
        .get_mut(..needed)
        .and_then(<[MaybeUninit<u8>]>::split_last_mut)
    else {
        return Err(INVALID);
    };
    let mut slots = output.iter_mut();
    let mut put = |byte: u8| {
        if let Some(slot) = slots.next() {
            slot.write(byte);
        }
    };
    let digit = |nibble: u8| {
        if nibble < 10 {
            b'0' + nibble
        } else {
            b'A' + nibble - 10
        }
    };
    for &byte in input {
        if unreserved(byte) {
            put(byte);
        } else {
            put(b'%');
            put(digit(byte >> 4));
            put(digit(byte & 15));
        }
    }
    nul.write(0);
    Ok(needed - 1)
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
/// Decode `input` in place, preserving malformed escapes and decoding '+' as space.
/// Returns the decoded length; the bytes after it are left as they were.
pub fn url_decode(input: &mut [u8]) -> usize {
    let (mut read, mut write) = (0, 0);
    while let Some(&byte) = input.get(read) {
        let pair = if byte == b'%' {
            let digit = |at: usize| input.get(at).copied().and_then(hex);
            digit(read + 1).zip(digit(read + 2))
        } else {
            None
        };
        let decoded = if let Some((high, low)) = pair {
            read += 2;
            (high << 4) | low
        } else if byte == b'+' {
            b' '
        } else {
            byte
        };
        if let Some(slot) = input.get_mut(write) {
            *slot = decoded;
        }
        read += 1;
        write += 1;
    }
    write
}
#[repr(C)]
#[derive(Default, Debug, PartialEq, Eq)]
pub struct HttpSpan {
    pub offset: usize,
    pub length: usize,
}
/// Upstream's `parse_multipart_boundary`: the boundary in a `Content-Type` value from
/// `offset` on, without allocating, or None.
///
/// # Errors
/// `OVERFLOW` for a boundary longer than `int`.
pub fn boundary(input: &[u8], mut offset: usize) -> Result<Option<HttpSpan>, i32> {
    while let Some(&byte) = input.get(offset) {
        if matches!(byte, b' ' | b';') {
            offset += 1;
            continue;
        }
        if case_prefix(tail(input, offset), b"boundary=") {
            offset += 9;
            break;
        }
        let Some(next) = tail(input, offset).iter().position(|&b| b == b';') else {
            return Ok(None);
        };
        offset += next;
    }
    if offset >= input.len() {
        return Ok(None);
    }
    let rest = tail(input, offset);
    let mut count = rest.iter().position(|&b| b == b';').unwrap_or(rest.len());
    let value = rest.get(..count).unwrap_or_default();
    if count >= 2 && value.first() == Some(&b'"') && value.last() == Some(&b'"') {
        offset += 1;
        count -= 2;
    }
    if count == 0 {
        return Ok(None);
    }
    if count > i32::MAX as usize {
        return Err(OVERFLOW);
    }
    Ok(Some(HttpSpan {
        offset,
        length: count,
    }))
}

/// Held by the test that switches `LC_CTYPE`, which `strncasecmp` reads, and by tests
/// whose case-insensitive matches a Turkish locale would change.
#[cfg(test)]
pub(crate) static LOCALE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_first_bytes_differ_under_libc_in_each_locale() {
        unsafe extern "C" {
            fn setlocale(category: i32, locale: *const std::ffi::c_char) -> *mut std::ffi::c_char;
        }
        const LC_CTYPE: i32 = if cfg!(target_os = "linux") { 0 } else { 2 };
        let locales = [
            c"C",
            c"C.UTF-8",
            c"en_US.ISO8859-1",
            c"en_US.UTF-8",
            c"tr_TR.ISO8859-9",
            c"tr_TR.UTF-8",
        ];
        let _locale = LOCALE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for locale in locales {
            // SAFETY: a NUL-terminated name; this test alone changes LC_CTYPE, then restores it.
            if unsafe { setlocale(LC_CTYPE, locale.as_ptr()) }.is_null() {
                continue;
            }
            for left in 0..=u8::MAX {
                for right in 0..=u8::MAX {
                    if differs_at_first(left, right) {
                        let (a, b) = ([left, 0], [right, 0]);
                        // SAFETY: both arrays hold the one compared byte.
                        let result =
                            unsafe { strncasecmp(a.as_ptr().cast(), b.as_ptr().cast(), 1) };
                        assert_ne!(result, 0, "{left:#x} {right:#x} in {locale:?}");
                    }
                }
            }
        }
        // SAFETY: restores the default locale.
        unsafe { setlocale(LC_CTYPE, c"C".as_ptr()) };
    }
}

mod owned_proxy;
pub use owned_proxy::*;
