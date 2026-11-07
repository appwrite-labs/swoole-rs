//! HTTP policy and private directory storage. Native effects run after Rust returns.
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
mod cookie_flow;
pub use cookie_flow::{HttpCookieFlow, swr_v2_http_cookie_flow};
mod conditional;
pub use conditional::{
    HttpCalendar, HttpConditional, swr_v1_http_conditional, swr_v1_http_date_format,
    swr_v3_http_conditional, swr_v3_http_date,
};
mod directory;
pub use directory::{
    HttpDirectory, swr_v2_http_directory_clone, swr_v2_http_directory_contains,
    swr_v2_http_directory_count, swr_v2_http_directory_drop, swr_v2_http_directory_emit,
    swr_v2_http_directory_insert, swr_v2_http_directory_new, swr_v2_http_directory_page,
    swr_v2_http_directory_scan_append, swr_v2_http_directory_scan_finish,
};
mod dispatch;
pub use dispatch::{
    swr_v1_http_proxy_auth_enabled, swr_v1_http_transport_route, swr_v1_http_unknown_status,
};
mod framing;
pub use framing::{HttpChunkStep, HttpFrame, swr_v1_http_expect};
mod headers;
mod index_page;
pub use headers::{HttpCookie, HttpHeaderInfo, swr_v1_http_cookie_next};
pub use index_page::{HttpIndexPage, swr_v1_http_index_page};
mod maps;
mod multipart;
#[cfg(test)]
mod pin_helpers;
pub use multipart::{
    swr_v1_http_multipart_disposition, swr_v1_http_multipart_filename, swr_v1_http_multipart_result,
};
mod proxy;
pub use proxy::{
    HttpProxyRequest, HttpProxyText, swr_v1_http_proxy_credentials, swr_v1_http_proxy_handshake,
    swr_v1_http_proxy_pack, swr_v2_http_proxy_authorization,
};
mod range;
mod range_response;
pub use range_response::{HttpRangeFinish, HttpRangeFormat, swr_v1_http_range_format};
mod request;
pub use request::{
    HttpForm, HttpNative, HttpRequest, HttpString, destroy_multipart_parser, init_multipart_parser,
    parse_multipart_data, swr_v1_http_form_free, swr_v1_http_request_chunked_length,
    swr_v1_http_request_expect, swr_v1_http_request_free, swr_v1_http_request_header,
    swr_v1_http_request_header_length, swr_v1_http_request_multipart_destroy,
    swr_v1_http_request_multipart_init, swr_v1_http_request_multipart_parse,
    swr_v1_http_request_new, swr_v1_http_request_parse_header, swr_v1_http_request_protocol,
    swr_v3_http_request_clean, swr_v3_http_request_init,
};
mod rewrite;
pub use rewrite::RewriteRules;
mod static_context;
pub use static_context::*;
mod tasks;
pub use tasks::{
    HttpStaticTask, HttpTasks, swr_v2_http_tasks_begin, swr_v2_http_tasks_clear,
    swr_v2_http_tasks_copy, swr_v2_http_tasks_count, swr_v2_http_tasks_drop, swr_v2_http_tasks_new,
    swr_v2_http_tasks_prepare, swr_v2_http_tasks_push, swr_v2_http_tasks_snapshot,
};
mod static_meta;
pub use static_meta::{
    swr_v1_http_static_boundary, swr_v1_http_static_catch, swr_v1_http_static_content_prefix,
    swr_v1_http_static_content_type,
};
mod status_buffer;
pub use status_buffer::{NATIVE_STATUS_CODES, swr_v2_http_status_message};
pub mod http_server;
mod static_headers;
mod static_path;
pub use maps::{swr_v1_http_method, swr_v1_http_method_string, swr_v1_http_status};
pub use range::HttpRangeState;
pub use static_headers::{HttpStaticHeaders, swr_v1_http_static_headers};
pub use static_path::{
    HttpStaticPath, swr_v1_http_static_index_path, swr_v1_http_static_location,
    swr_v1_http_static_path,
};

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
/// Required encoded capacity, INCLUDING the trailing NUL. No allocation occurs.
/// # Safety
/// Input is readable and output is aligned/writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_url_capacity(
    data: *const u8,
    length: usize,
    output: *mut usize,
) -> i32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(0);
    }
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    match unsafe { bytes(data, length) }.and_then(encoded_length) {
        Ok(n) => {
            // SAFETY: `output` is non-null (checked above), aligned and writable.
            unsafe {
                output.write(n);
            }
            0
        }
        Err(status) => status,
    }
}
/// Encode into disjoint native-owned storage, terminating it. No mutation on error.
/// # Safety
/// Input is readable; output capacity is writable, disjoint from input/length.
/// Length output is valid/aligned. No allocation, callback or borrow escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_url_encode(
    data: *const u8,
    length: usize,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if written.is_null() {
        return INVALID;
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable.
    unsafe {
        written.write(0);
    }
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let input = match unsafe { bytes(data, length) } {
        Ok(v) => v,
        Err(e) => return e,
    };
    let needed = match encoded_length(input) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if output.is_null() || capacity < needed {
        return INVALID;
    }
    // SAFETY: non-null, `needed <= capacity`; the caller lends writable, disjoint storage.
    let output = unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), needed) };
    if needed == input.len() + 1 {
        // Exact capacity proves every byte is unreserved, so emission can copy
        // the entire initialized input without reclassifying each byte.
        // SAFETY: `input.len() < needed` slots in the disjoint output.
        unsafe {
            std::ptr::copy_nonoverlapping(input.as_ptr(), output.as_mut_ptr().cast(), input.len());
        };
    } else {
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
    }
    let at = needed - 1;
    if let Some(slot) = output.get_mut(at) {
        slot.write(0);
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable.
    unsafe {
        written.write(at);
    }
    0
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
/// Decode in place, preserving malformed escapes and decoding '+' as space.
/// # Safety
/// Data is writable for length+1 initialized input bytes; written is disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_url_decode(
    data: *mut u8,
    length: usize,
    written: *mut usize,
) -> i32 {
    if written.is_null() {
        return INVALID;
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable.
    unsafe {
        written.write(0);
    }
    let Some(_capacity) = length
        .checked_add(1)
        .filter(|&n| isize::try_from(n).is_ok())
    else {
        return OVERFLOW;
    };
    if data.is_null() {
        return INVALID;
    }
    // SAFETY: non-null, `length + 1 <= isize::MAX` (checked above), writable initialized bytes.
    let input = unsafe { slice::from_raw_parts_mut(data, length) };
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
    // SAFETY: `write <= length`, and the caller lends `length + 1` writable bytes.
    unsafe { data.wrapping_add(write).write(0) };
    // SAFETY: `written` is non-null (checked above), aligned and writable.
    unsafe {
        written.write(write);
    }
    0
}
#[repr(C)]
#[derive(Default)]
pub struct HttpSpan {
    pub offset: usize,
    pub length: usize,
}
/// Find legacy multipart boundary without allocating or retaining input.
/// # Safety
/// Input is readable; output is valid/aligned/disjoint. Status 1=found, 0=absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_boundary(
    data: *const u8,
    length: usize,
    mut offset: usize,
    output: *mut HttpSpan,
) -> i32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(HttpSpan::default());
    }
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let input = match unsafe { bytes(data, length) } {
        Ok(v) => v,
        Err(e) => return e,
    };
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
            return 0;
        };
        offset += next;
    }
    if offset >= length {
        return 0;
    }
    let rest = tail(input, offset);
    let mut count = rest.iter().position(|&b| b == b';').unwrap_or(rest.len());
    let value = rest.get(..count).unwrap_or_default();
    if count >= 2 && value.first() == Some(&b'"') && value.last() == Some(&b'"') {
        offset += 1;
        count -= 2;
    }
    if count == 0 {
        return 0;
    }
    if count > i32::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(HttpSpan {
            offset,
            length: count,
        });
    }
    1
}

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
    #[test]
    fn positive_url_encoding_capacity_and_bytes_cover_loop_boundaries() {
        for length in [0, 1, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 1024] {
            for escaped in [false, true] {
                let mut input = b"Az09-._~".repeat(length);
                if escaped {
                    input.extend_from_slice(b"/+%\0\xff");
                }
                let mut expected = b"Az09-._~".repeat(length);
                if escaped {
                    expected.extend_from_slice(b"%2F%2B%25%00%FF");
                }
                expected.push(0);
                let mut capacity = 0;
                assert_eq!(
                    unsafe {
                        swr_v1_http_url_capacity(input.as_ptr(), input.len(), &raw mut capacity)
                    },
                    0
                );
                assert_eq!(capacity, expected.len());
                let mut output = vec![0xa5; capacity + 3];
                let mut written = 999;
                assert_eq!(
                    unsafe {
                        swr_v1_http_url_encode(
                            input.as_ptr(),
                            input.len(),
                            output.as_mut_ptr(),
                            capacity,
                            &raw mut written,
                        )
                    },
                    0
                );
                assert_eq!(written + 1, capacity);
                assert_eq!(&output[..capacity], expected);
                assert_eq!(&output[capacity..], &[0xa5; 3]);
            }
        }
    }
    #[test]
    fn url_roundtrips_all_bytes_and_handles_malformed_escapes() {
        let input: Vec<u8> = (0..=255).collect();
        let mut encoded = vec![0; encoded_length(&input).unwrap()];
        let mut length = 0;
        unsafe {
            assert_eq!(
                swr_v1_http_url_encode(
                    input.as_ptr(),
                    input.len(),
                    encoded.as_mut_ptr(),
                    encoded.len(),
                    &raw mut length
                ),
                0
            );
            assert_eq!(
                swr_v1_http_url_decode(encoded.as_mut_ptr(), length, &raw mut length),
                0
            );
        }
        assert_eq!(&encoded[..length], input);
        assert_eq!(encoded[length], 0);
        let mut malformed = b"%+%0%GG%a0+%00!\0".to_vec();
        unsafe {
            assert_eq!(
                swr_v1_http_url_decode(
                    malformed.as_mut_ptr(),
                    malformed.len() - 1,
                    &raw mut length
                ),
                0
            );
        }
        assert_eq!(&malformed[..length], b"% %0%GG\xa0 \0!");
    }
    #[test]
    fn errors_are_atomic_and_empty_encoding_has_terminator_storage() {
        let mut output = [99; 4];
        let mut length = 7;
        unsafe {
            assert_eq!(
                swr_v1_http_url_encode(b"!!".as_ptr(), 2, output.as_mut_ptr(), 4, &raw mut length),
                INVALID
            );
            assert_eq!(output, [99; 4]);
            assert_eq!(length, 0);
            assert_eq!(
                swr_v1_http_url_capacity(std::ptr::null(), 0, &raw mut length),
                0
            );
            assert_eq!(length, 1);
            assert_eq!(
                swr_v1_http_url_encode(
                    std::ptr::null(),
                    0,
                    output.as_mut_ptr(),
                    1,
                    &raw mut length
                ),
                0
            );
            assert_eq!(output[0], 0);
        }
    }
    #[test]
    fn ffi_rejects_overflow_before_access_and_extreme_method_ids_do_not_panic() {
        let mut written = 9;
        unsafe {
            assert_eq!(
                swr_v1_http_url_capacity(std::ptr::null(), usize::MAX, &raw mut written),
                OVERFLOW
            );
            assert_eq!(written, 0);
            assert_eq!(
                swr_v1_http_url_decode(std::ptr::null_mut(), usize::MAX, &raw mut written),
                OVERFLOW
            );
            assert_eq!(written, 0);
            assert_eq!(
                swr_v1_http_url_capacity(std::ptr::null(), 0, std::ptr::null_mut()),
                INVALID
            );
        }
        assert!(swr_v1_http_method_string(i32::MIN).is_null());
        assert!(swr_v1_http_method_string(i32::MAX).is_null());
    }
    #[test]
    fn boundary_preserves_quotes_spaces_and_parameter_rules() {
        for (input, offset, expected) in [
            (&b"x; boundary=\"abc\"; foo=x"[..], 0, Some(&b"abc"[..])),
            (&b"BounDary=a "[..], 0, Some(&b"a "[..])),
            (&b"boundary=\"\""[..], 0, None),
            (&b"x=a"[..], 0, None),
            (&b"boundary=x"[..], 99, None),
        ] {
            let mut result = HttpSpan::default();
            let status = unsafe {
                swr_v1_http_boundary(input.as_ptr(), input.len(), offset, &raw mut result)
            };
            assert_eq!(status, i32::from(expected.is_some()));
            if let Some(expected) = expected {
                assert_eq!(
                    &input[result.offset..result.offset + result.length],
                    expected
                );
            }
        }
    }
}

mod owned_proxy;
pub use owned_proxy::*;
