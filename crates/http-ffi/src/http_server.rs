//! `swoole::http_server`'s free helpers from `src/protocol/http.cc` that hand out or
//! take C strings, defined under their upstream (Itanium) symbols with
//! `upstream-symbols`. `get_method`, `url_encode` and `parse_multipart_boundary` are
//! crates/http's `method`, `url_encode` and `boundary`, over slices.
use std::cell::Cell;
use std::ffi::{c_char, c_int};

thread_local! {
    static UNKNOWN: [Cell<u8>; 128] = const { [const { Cell::new(0) }; 128] };
}
/// `const char *get_status_message(int code)`. Known messages have immutable static
/// backing. Unknown messages share this thread's 128-byte fallback until the next
/// unknown call, as the pinned function's do. The pointer must not be used after its
/// thread ends.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server18get_status_messageEi")
)]
#[must_use]
pub extern "C" fn get_status_message(code: c_int) -> *const c_char {
    let message = swoole_rs_http::status_message(code);
    if let std::borrow::Cow::Borrowed(known) = message {
        // The table's messages are C strings; the NUL follows the borrowed bytes.
        return known.as_ptr().cast();
    }
    UNKNOWN.with(|unknown| {
        // Each byte is a cell, so the pointer an earlier call returned stays usable
        // while this one rewrites the text; the last cell keeps its NUL.
        let length = message.len().min(unknown.len() - 1);
        let text = message
            .iter()
            .copied()
            .take(length)
            .chain(std::iter::once(0));
        for (to, from) in unknown.iter().zip(text) {
            to.set(from);
        }
        unknown.as_ptr().cast::<c_char>()
    })
}

/// `const char *get_method_string(int method)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11http_server17get_method_stringEi")
)]
#[must_use]
pub extern "C" fn get_method_string(method: c_int) -> *const c_char {
    swoole_rs_http::method_string(method).map_or(std::ptr::null(), std::ffi::CStr::as_ptr)
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

#[cfg(test)]
mod tests {
    // Upstream's `core-tests/src/server/http_server.cpp` TESTs over these helpers, one
    // test each (tests/modules/http/README.md).
    use super::*;
    use std::ffi::CStr;

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
        let capacity = swoole_rs_http::url_capacity(input.as_bytes()).unwrap();
        let mut encoded = vec![std::mem::MaybeUninit::new(0u8); capacity];
        let length = swoole_rs_http::url_encode(input.as_bytes(), &mut encoded).unwrap();
        assert_eq!(length + 1, capacity);
        let encoded = encoded.as_mut_ptr().cast::<c_char>();
        assert_eq!(text(encoded), expected);
        // SAFETY: `encoded` is NUL-terminated and writable.
        let decoded = unsafe { url_decode(encoded, expected.len()) };
        assert_eq!(decoded, input.len());
        assert_eq!(text(encoded), input);
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
        let method = swoole_rs_http::method;
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
        let mut boundary = None;
        let mut parse = |input: &'static [u8], offset| {
            let found = swoole_rs_http::boundary(input, offset).unwrap();
            if let Some(span) = &found {
                boundary = Some(&input[span.offset..][..span.length]);
            }
            (found.is_some(), boundary)
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

    #[test]
    fn url_decode_decodes_in_place() {
        let mut buffer = *b"a%20b+c%zz\0";
        // SAFETY: `buffer` holds the 10 bytes and their terminator.
        let written = unsafe { url_decode(buffer.as_mut_ptr().cast(), 10) };
        assert_eq!(&buffer[..=written], b"a b c%zz\0");
    }
}
