//! `swoole::http_server`'s free helpers from `src/protocol/http.cc`, defined under
//! their upstream (Itanium) symbols when the hybrid links this crate
//! (`upstream-symbols`).
use std::ffi::{c_char, c_int};

use crate::{
    HttpSpan, swr_v1_http_boundary, swr_v1_http_url_capacity, swr_v1_http_url_decode,
    swr_v1_http_url_encode,
};

/// `const char *get_status_message(int code)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server18get_status_messageEi")
)]
#[must_use]
pub extern "C" fn get_status_message(code: c_int) -> *const c_char {
    crate::swr_v2_http_status_message(code)
}

/// `int get_method(const char *method_str, size_t method_len)`.
///
/// # Safety
/// `method` is readable for `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server10get_methodEPKcm")
)]
#[must_use]
pub unsafe extern "C" fn get_method(method: *const c_char, length: usize) -> c_int {
    // SAFETY: as the caller guarantees.
    unsafe { crate::swr_v1_http_method(method.cast(), length) }
}

/// `const char *get_method_string(int method)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server17get_method_stringEi")
)]
#[must_use]
pub extern "C" fn get_method_string(method: c_int) -> *const c_char {
    crate::swr_v1_http_method_string(method)
}

/// `bool parse_multipart_boundary(at, length, offset, out_boundary_str, out_boundary_len)`:
/// the outputs are written only when a boundary is found.
///
/// # Safety
/// `at` is readable for `length` bytes; the outputs are writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server24parse_multipart_boundaryEPKcmmPPcPi")
)]
pub unsafe extern "C" fn parse_multipart_boundary(
    at: *const c_char,
    length: usize,
    offset: usize,
    out_boundary_str: *mut *mut c_char,
    out_boundary_len: *mut c_int,
) -> bool {
    let mut span = HttpSpan::default();
    // SAFETY: as the caller guarantees; `span` is a live local.
    if unsafe { swr_v1_http_boundary(at.cast(), length, offset, &raw mut span) } != 1 {
        return false;
    }
    // SAFETY: the outputs are writable, as the caller guarantees.
    unsafe { out_boundary_str.write(at.wrapping_add(span.offset).cast_mut()) };
    // A boundary inside a header line is far shorter than `int`.
    let boundary_len = c_int::try_from(span.length).unwrap_or(c_int::MAX);
    // SAFETY: as above.
    unsafe { out_boundary_len.write(boundary_len) };
    true
}

/// `size_t url_decode(char *str, size_t len)`: decodes in place and terminates it.
///
/// # Safety
/// `str` is writable for `len + 1` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server10url_decodeEPcm")
)]
pub unsafe extern "C" fn url_decode(str: *mut c_char, len: usize) -> usize {
    let mut written = 0;
    // SAFETY: as the caller guarantees; `written` is a live local.
    if unsafe { swr_v1_http_url_decode(str.cast(), len, &raw mut written) } != 0 {
        // A null buffer: upstream's loop writes through it.
        std::process::abort();
    }
    written
}

/// `char *url_encode(const char *str, size_t len)`: a NUL-terminated copy from
/// `sw_malloc`, which the caller frees. Upstream's `bad_alloc` and `length_error`
/// went uncaught, so either ends the process.
///
/// # Safety
/// `str` is readable for `len` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server10url_encodeEPKcm")
)]
#[must_use]
pub unsafe extern "C" fn url_encode(str: *const c_char, len: usize) -> *mut c_char {
    let (mut capacity, mut written) = (0, 0);
    // SAFETY: as the caller guarantees; `capacity` is a live local.
    if unsafe { swr_v1_http_url_capacity(str.cast(), len, &raw mut capacity) } != 0 {
        std::process::abort();
    }
    let result = swoole_rs_corebase::sw_malloc(capacity).cast::<u8>();
    if result.is_null() {
        std::process::abort();
    }
    // SAFETY: `result` holds `capacity` bytes, disjoint from the input.
    if unsafe { swr_v1_http_url_encode(str.cast(), len, result, capacity, &raw mut written) } != 0 {
        std::process::abort();
    }
    result.cast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::ptr;

    fn text(pointer: *const c_char) -> &'static str {
        // SAFETY: the helpers return static NUL-terminated text.
        unsafe { CStr::from_ptr(pointer) }.to_str().unwrap()
    }

    /// Upstream's `http_server.get_method` and `get_method_string` tests.
    #[test]
    fn methods_match_upstreams_tests() {
        // SAFETY: each literal is readable for its length.
        let method = |name: &[u8]| unsafe { get_method(name.as_ptr().cast(), name.len()) };
        assert_eq!(method(b"POST"), 4);
        assert_eq!(method(b"post"), 4);
        assert_eq!(method(b"OPTIONS"), 8);
        assert_eq!(method(b"UNKNOWN"), -1);
        assert_eq!(text(get_method_string(4)), "POST");
        assert_eq!(text(get_method_string(2)), "GET");
        assert_eq!(text(get_method_string(8)), "OPTIONS");
        assert!(get_method_string(-1).is_null());
        assert!(get_method_string(999).is_null());
    }

    #[test]
    fn status_messages_forward() {
        assert_eq!(text(get_status_message(200)), "200 OK");
        assert_eq!(text(get_status_message(999)), "999 Unknown Status");
    }

    /// A miss leaves both outputs as they were (the retired C++ gate's property).
    #[test]
    fn boundary_outputs_untouched_on_a_miss() {
        let header = b"multipart/form-data; charset=utf-8";
        let sentinel = 0x55usize as *mut c_char;
        let (mut at, mut length) = (sentinel, -7);
        // SAFETY: `header` is readable; the outputs are live locals.
        let found = unsafe {
            parse_multipart_boundary(
                header.as_ptr().cast(),
                header.len(),
                19,
                &raw mut at,
                &raw mut length,
            )
        };
        assert!(!found);
        assert_eq!((at, length), (sentinel, -7));

        let header = b"multipart/form-data; boundary=abc";
        // SAFETY: as above.
        let found = unsafe {
            parse_multipart_boundary(
                header.as_ptr().cast(),
                header.len(),
                19,
                &raw mut at,
                &raw mut length,
            )
        };
        assert!(found);
        assert_eq!(at.cast_const(), header[30..].as_ptr().cast());
        assert_eq!(length, 3);
        assert_ne!(at, ptr::null_mut());
    }

    #[test]
    fn url_decode_decodes_in_place() {
        let mut buffer = *b"a%20b+c%zz\0";
        // SAFETY: `buffer` holds the 10 bytes and their terminator.
        let written = unsafe { url_decode(buffer.as_mut_ptr().cast(), 10) };
        assert_eq!(&buffer[..=written], b"a b c%zz\0");
    }

    /// Upstream's `http_server.EncodeDecodeBasic` and `EncodeDecodeWithSpecialChars`.
    #[test]
    fn url_encode_round_trips() {
        for (input, expected) in [
            ("Hello World!", "Hello%20World%21"),
            ("C++ Programming & C#", "C%2B%2B%20Programming%20%26%20C%23"),
        ] {
            // SAFETY: the literal is readable for its length.
            let encoded = unsafe { url_encode(input.as_ptr().cast(), input.len()) };
            assert_eq!(text(encoded), expected);
            // SAFETY: `encoded` is NUL-terminated and writable.
            let decoded = unsafe { url_decode(encoded, expected.len()) };
            assert_eq!(decoded, input.len());
            assert_eq!(text(encoded), input);
            // SAFETY: from `sw_malloc`, not used again.
            unsafe { swoole_rs_corebase::sw_free(encoded.cast()) };
        }
    }
}
