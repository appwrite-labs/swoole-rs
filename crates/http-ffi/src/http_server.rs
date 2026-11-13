//! `swoole::http_server`'s free helpers from `src/protocol/http.cc`, defined under
//! their upstream (Itanium) symbols when the hybrid links this crate
//! (`upstream-symbols`).
use std::ffi::{c_char, c_int};
use std::mem::MaybeUninit;

use crate::bytes;

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
    unsafe { bytes(method.cast(), length) }.map_or_else(|status| status, swoole_rs_http::method)
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
/// `at` is readable for `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server24parse_multipart_boundaryEPKcmmPPcPi")
)]
pub unsafe extern "C" fn parse_multipart_boundary(
    at: *const c_char,
    length: usize,
    offset: usize,
    out_boundary_str: &mut *mut c_char,
    out_boundary_len: &mut c_int,
) -> bool {
    // SAFETY: as the caller guarantees.
    let found = unsafe { bytes(at.cast(), length) }
        .and_then(|input| swoole_rs_http::boundary(input, offset));
    let Ok(Some(span)) = found else {
        return false;
    };
    *out_boundary_str = at.wrapping_add(span.offset).cast_mut();
    // A boundary inside a header line is far shorter than `int`.
    let boundary_len = c_int::try_from(span.length).unwrap_or(c_int::MAX);
    *out_boundary_len = boundary_len;
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
    // A null buffer: upstream's loop writes through it.
    if str.is_null() || len >= isize::MAX as usize {
        swoole_rs_corebase::fatal("url_decode of a null or oversized buffer");
    }
    // SAFETY: non-null, `len + 1 <= isize::MAX`; the caller lends that many writable bytes.
    let buffer = unsafe { std::slice::from_raw_parts_mut(str.cast::<u8>(), len + 1) };
    let written = swoole_rs_http::url_decode(buffer.get_mut(..len).unwrap_or_default());
    if let Some(nul) = buffer.get_mut(written) {
        *nul = 0;
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
    // SAFETY: as the caller guarantees.
    let Ok(input) = (unsafe { crate::bytes(str.cast(), len) }) else {
        swoole_rs_corebase::fatal("url_encode of a null buffer");
    };
    let Ok(capacity) = swoole_rs_http::url_capacity(input) else {
        swoole_rs_corebase::fatal("url_encode's output size overflows");
    };
    let result = swoole_rs_corebase::sw_malloc(capacity).cast::<MaybeUninit<u8>>();
    if result.is_null() {
        swoole_rs_corebase::fatal("url_encode is out of memory");
    }
    // SAFETY: `result` holds `capacity` bytes, disjoint from the input.
    let output = unsafe { std::slice::from_raw_parts_mut(result, capacity) };
    if swoole_rs_http::url_encode(input, output).is_err() {
        swoole_rs_corebase::fatal("url_encode overran its output");
    }
    result.cast()
}

#[cfg(test)]
mod tests {
    // Upstream's `core-tests/src/server/http_server.cpp` TESTs over these helpers, one
    // test each (tests/modules/http/README.md).
    use super::*;
    use std::ffi::CStr;
    use std::ptr;

    /// `SW_HTTP_GET`, `SW_HTTP_POST`, `SW_HTTP_OPTIONS`.
    const SW_HTTP_GET: c_int = 2;
    const SW_HTTP_POST: c_int = 4;
    const SW_HTTP_OPTIONS: c_int = 8;

    fn text(pointer: *const c_char) -> &'static str {
        // SAFETY: the helpers return static NUL-terminated text.
        unsafe { CStr::from_ptr(pointer) }.to_str().unwrap()
    }

    /// `url_encode`, then `url_decode` in place.
    fn encode_decode(input: &str, expected: &str) {
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

    #[test]
    fn encode_decode_basic() {
        encode_decode("Hello World!", "Hello%20World%21");
    }

    #[test]
    fn encode_decode_with_special_chars() {
        encode_decode("C++ Programming & C#", "C%2B%2B%20Programming%20%26%20C%23");
    }

    #[test]
    fn get_method_cases() {
        // SAFETY: each literal is readable for its length.
        let method = |name: &[u8]| unsafe { get_method(name.as_ptr().cast(), name.len()) };
        assert_eq!(method(b"POST"), SW_HTTP_POST);
        assert_eq!(method(b"post"), SW_HTTP_POST);
        assert_eq!(method(b"OPTIONS"), SW_HTTP_OPTIONS);
        assert_eq!(method(b"UNKNOWN"), -1);
    }

    #[test]
    fn get_method_str() {
        assert_eq!(text(get_method_string(SW_HTTP_POST)), "POST");
        assert_eq!(text(get_method_string(SW_HTTP_GET)), "GET");
        assert_eq!(text(get_method_string(SW_HTTP_OPTIONS)), "OPTIONS");
        assert!(get_method_string(-1).is_null());
        assert!(get_method_string(999).is_null());
    }

    #[test]
    fn get_status_message_cases() {
        // SAFETY: nothing writes the table in this test binary.
        let codes = unsafe { (&raw const crate::NATIVE_STATUS_CODES).read() };
        for code in codes.into_iter().take_while(|code| *code != -1) {
            assert!(
                text(get_status_message(code)).starts_with(&format!("{code} ")),
                "{code}"
            );
        }
        assert_eq!(text(get_status_message(999)), "999 Unknown Status");
    }

    #[test]
    fn parse_multipart_boundary_cases() {
        // The outputs live across the cases, as upstream's do: a miss leaves them alone.
        let (mut at, mut length) = (ptr::null_mut::<c_char>(), 0);
        let mut parse = |input: &'static [u8], offset| {
            // SAFETY: `input` is readable; the outputs are live locals.
            let found = unsafe {
                parse_multipart_boundary(
                    input.as_ptr().cast(),
                    input.len(),
                    offset,
                    &mut at,
                    &mut length,
                )
            };
            // SAFETY: a boundary found now or before lies inside its static input.
            let boundary = (!at.is_null()).then(|| unsafe {
                std::slice::from_raw_parts(at.cast::<u8>(), usize::try_from(length).unwrap())
            });
            (found, boundary)
        };
        let header: &[u8] = b"Content-Type: multipart/form-data; boundary=----WebKitFormBoundaryX";
        let x = Some(&b"----WebKitFormBoundaryX"[..]);
        assert_eq!(parse(header, 30), (true, x), "plain");
        assert_eq!(
            parse(
                b"Content-Type: multipart/form-data; boundary=\"----WebKitFormBoundaryX\"",
                30
            ),
            (true, x),
            "quoted"
        );
        assert_eq!(
            parse(
                b"Content-Type: multipart/form-data; boundary = ----WebKitFormBoundaryX",
                30
            ),
            (false, x),
            "spaced: the outputs from before"
        );
        assert_eq!(
            parse(b"Content-Type: multipart/form-data; boundary=----WebKitFormBoundaryX; charset=UTF-8", 30),
            (true, x),
            "a parameter after"
        );
        assert_eq!(
            parse(b"Content-Type: multipart/form-data; charset=UTF-8; boundary=----WebKitFormBoundaryX", 30),
            (true, x),
            "a parameter before"
        );
        assert!(
            !parse(b"Content-Type: multipart/form-data; boundary=", 30).0,
            "empty"
        );
        assert!(
            !parse(b"Content-Type: multipart/form-data; boundary=\"\"", 30).0,
            "empty quotes"
        );
        assert!(
            !parse(b"Content-Type: multipart/form-data; charset=UTF-8", 30).0,
            "no boundary"
        );
        assert_eq!(
            parse(
                b"Content-Type: multipart/form-data; BOUNDARY=----WebKitFormBoundaryX",
                30
            ),
            (true, x),
            "case"
        );
        assert_eq!(
            parse(b"boundary=----WebKitFormBoundaryX", 0),
            (true, x),
            "offset 0"
        );
        assert!(!parse(header, header.len() + 1).0, "offset past the end");
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
                &mut at,
                &mut length,
            )
        };
        assert!(!found);
        assert_eq!((at, length), (sentinel, -7));
    }

    #[test]
    fn url_decode_decodes_in_place() {
        let mut buffer = *b"a%20b+c%zz\0";
        // SAFETY: `buffer` holds the 10 bytes and their terminator.
        let written = unsafe { url_decode(buffer.as_mut_ptr().cast(), 10) };
        assert_eq!(&buffer[..=written], b"a b c%zz\0");
    }
}
