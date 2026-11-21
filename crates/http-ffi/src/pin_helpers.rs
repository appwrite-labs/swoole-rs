//! The status, method, URL and boundary helpers against the pin's `src/protocol/http.cc`
//! (v6.2.5; these bodies are unchanged since v6.2.4).
//! `pin` transcribes the pinned bodies (C locale). The cases are the ones the retired
//! `tests/modules/http/gate.cc`, `status_buffer_gate.cc` and `dispatch_gate.cc` compared.
use crate::{
    HttpSpan, NATIVE_STATUS_CODES, swr_v1_http_boundary, swr_v1_http_method,
    swr_v1_http_method_string, swr_v2_http_status_message,
};
use std::ffi::{CStr, c_char, c_int};

mod pin {
    /// `get_status_message`'s cases, in `list_of_status_code` order. Code 0 is also "200 OK".
    pub(super) const STATUS: &str = "100 Continue|101 Switching Protocols|102 Processing|\
        103 Early Hints|200 OK|201 Created|202 Accepted|203 Non-Authoritative Information|\
        204 No Content|205 Reset Content|206 Partial Content|207 Multi-Status|208 Already Reported|\
        226 IM Used|300 Multiple Choices|301 Moved Permanently|302 Found|303 See Other|\
        304 Not Modified|305 Use Proxy|306 (Unused)|307 Temporary Redirect|308 Permanent Redirect|\
        400 Bad Request|401 Unauthorized|402 Payment Required|403 Forbidden|404 Not Found|\
        405 Method Not Allowed|406 Not Acceptable|407 Proxy Authentication Required|\
        408 Request Timeout|409 Conflict|410 Gone|411 Length Required|412 Precondition Failed|\
        413 Payload Too Large|414 URI Too Long|415 Unsupported Media Type|\
        416 Range Not Satisfiable|417 Expectation Failed|418 I'm a teapot|\
        421 Misdirected Request|422 Unprocessable Entity|423 Locked|424 Failed Dependency|\
        425 Too Early|426 Upgrade Required|428 Precondition Required|429 Too Many Requests|\
        431 Request Header Fields Too Large|451 Unavailable For Legal Reasons|\
        500 Internal Server Error|501 Not Implemented|502 Bad Gateway|503 Service Unavailable|\
        504 Gateway Timeout|505 HTTP Version Not Supported|506 Variant Also Negotiates|\
        507 Insufficient Storage|508 Loop Detected|510 Not Extended|\
        511 Network Authentication Required";
    pub(super) const METHODS: &str = "DELETE GET HEAD POST PUT PATCH CONNECT OPTIONS TRACE COPY \
        LOCK MKCOL MOVE PROPFIND PROPPATCH UNLOCK REPORT MKACTIVITY CHECKOUT MERGE M-SEARCH NOTIFY \
        SUBSCRIBE UNSUBSCRIBE PURGE QUERY PRI";

    pub(super) fn status(code: i32) -> String {
        let code = if code == 0 { 200 } else { code };
        let known = STATUS
            .split('|')
            .find(|entry| entry.split(' ').next() == Some(&code.to_string()));
        known.map_or_else(|| format!("{code} Unknown Status"), str::to_owned)
    }

    /// `get_method`: `swoole_strcaseeq` against every name up to `SW_HTTP_PRI`.
    pub(super) fn method(input: &[u8]) -> i32 {
        let found = METHODS
            .split(' ')
            .position(|name| name.as_bytes().eq_ignore_ascii_case(input));
        found.map_or(-1, |index| i32::try_from(index).unwrap() + 1)
    }

    pub(super) fn method_string(id: i32) -> Option<&'static str> {
        METHODS
            .split(' ')
            .nth(usize::try_from(id).ok()?.checked_sub(1)?)
    }

    pub(super) fn url_encode(input: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        for &byte in input {
            // `char c` is signed, so every byte from 0x80 sorts below '0' and is escaped.
            let escape = byte >= 0x80
                || (byte < b'0' && byte != b'-' && byte != b'.')
                || (byte < b'A' && byte > b'9')
                || (byte > b'Z' && byte < b'a' && byte != b'_')
                || (byte > b'z' && byte != b'~');
            if escape {
                output.extend(format!("%{byte:02X}").bytes());
            } else {
                output.push(byte);
            }
        }
        output
    }

    /// `url_decode`'s output bytes (it then writes a NUL after them).
    pub(super) fn url_decode(input: &[u8]) -> Vec<u8> {
        let (mut output, mut index) = (Vec::new(), 0);
        while let Some(&byte) = input.get(index) {
            let escape = input
                .get(index + 1..index + 3)
                .filter(|hex| byte == b'%' && hex.iter().all(u8::is_ascii_hexdigit));
            match (byte, escape) {
                (b'+', _) => output.push(b' '),
                (_, Some(hex)) => {
                    // `url_htoi` on two hex digits.
                    output.push(u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap());
                    index += 2;
                }
                _ => output.push(byte),
            }
            index += 1;
        }
        output
    }

    /// `parse_multipart_boundary`: the boundary's offset and length in `at`.
    pub(super) fn boundary(at: &[u8], mut offset: usize) -> Option<(usize, usize)> {
        let length = at.len();
        while offset < length {
            if at[offset] == b' ' || at[offset] == b';' {
                offset += 1;
                continue;
            }
            if at
                .get(offset..offset + 9)
                .is_some_and(|name| name.eq_ignore_ascii_case(b"boundary="))
            {
                offset += 9;
                break;
            }
            offset += at[offset..].iter().position(|&byte| byte == b';')?;
        }
        let rest = at.get(offset..).filter(|rest| !rest.is_empty())?;
        let size = rest
            .iter()
            .position(|&byte| byte == b';')
            .unwrap_or(rest.len());
        match &rest[..size] {
            [] | [b'"', b'"'] => None,
            [b'"', .., b'"'] => Some((offset + 1, size - 2)),
            _ => Some((offset, size)),
        }
    }
}

fn text(pointer: *const c_char) -> String {
    // SAFETY: the status table and the thread's fallback are NUL-terminated.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .unwrap()
        .to_owned()
}

unsafe extern "C" {
    #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
    #[cfg_attr(not(target_os = "linux"), link_name = "__error")]
    fn errno_location() -> *mut c_int;
}

#[test]
fn status_messages_and_errno_match_the_pin() {
    const EINTR: c_int = 4;
    for code in (-512..=1024).chain([i32::MIN, i32::MAX]) {
        // SAFETY: the calling thread's errno slot.
        unsafe { *errno_location() = EINTR };
        let message = text(swr_v2_http_status_message(code));
        // SAFETY: as above.
        assert_eq!(
            unsafe { *errno_location() },
            EINTR,
            "status {code} changed errno"
        );
        assert_eq!(message, pin::status(code));
    }
    // Unknown codes share the thread's one fallback buffer.
    assert_eq!(
        swr_v2_http_status_message(-999),
        swr_v2_http_status_message(-1000)
    );
}

#[test]
fn unknown_status_fallback_is_per_thread() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|id| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let pointer = swr_v2_http_status_message(-12345) as usize;
                barrier.wait();
                for code in (0..1024).map(|n| -(2000 + id * 1024 + n)) {
                    assert_eq!(text(swr_v2_http_status_message(code)), pin::status(code));
                }
                pointer
            })
        })
        .collect();
    let pointers: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_ne!(pointers[0], pointers[1]);
}

#[test]
fn public_status_code_array_matches_the_pin_and_is_independent_of_lookup() {
    let mut expected = [0; 128];
    let codes = pin::STATUS
        .split('|')
        .map(|entry| entry[..3].parse().unwrap())
        .chain([-1]);
    expected
        .iter_mut()
        .zip(codes)
        .for_each(|(slot, code)| *slot = code);
    let slots = &raw mut NATIVE_STATUS_CODES;
    // SAFETY: this test is the array's only user in the test binary.
    unsafe {
        assert_eq!(*slots, expected);
        (*slots)[0] = 777;
        assert_eq!(text(swr_v2_http_status_message(100)), "100 Continue");
        (*slots)[0] = 100;
    }
}

#[test]
fn methods_match_the_pin() {
    let method = |input: &[u8]| {
        // SAFETY: `input` is initialized for its length.
        unsafe { swr_v1_http_method(input.as_ptr(), input.len()) }
    };
    for id in -10..40 {
        let name = swr_v1_http_method_string(id);
        let expected = pin::method_string(id);
        assert_eq!((!name.is_null()).then(|| text(name)).as_deref(), expected);
        if let Some(name) = expected {
            let lower = name.to_ascii_lowercase();
            assert_eq!(method(lower.as_bytes()), pin::method(lower.as_bytes()));
            let spaced = lower + " ";
            assert_eq!(method(spaced.as_bytes()), pin::method(spaced.as_bytes()));
        }
    }
}

fn encode(input: &[u8]) -> Vec<u8> {
    // SAFETY: `input` is initialized for its length.
    let encoded = unsafe { crate::http_server::url_encode(input.as_ptr().cast(), input.len()) };
    // SAFETY: a NUL-terminated copy from `sw_malloc`.
    let output = unsafe { CStr::from_ptr(encoded) }.to_bytes().to_vec();
    // SAFETY: from `sw_malloc`, not used again.
    unsafe { swoole_rs_corebase::sw_free(encoded.cast()) };
    output
}

fn decode(input: &[u8]) -> Vec<u8> {
    let mut data = [input, b"\0"].concat();
    // SAFETY: `data` holds the input and its NUL.
    let written = unsafe { crate::http_server::url_decode(data.as_mut_ptr().cast(), input.len()) };
    assert_eq!(data[written], 0);
    data.truncate(written);
    data
}

#[test]
fn url_coding_matches_the_pin() {
    let mut random: u32 = 8293;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        random
    };
    for n in 0..4096 {
        // The pin's url_encode is compared from a leading unreserved byte, as the gate did.
        let input: Vec<u8> = std::iter::once(b'A')
            .chain((0..n % 257).map(|_| next().to_le_bytes()[0]))
            .collect();
        let encoded = encode(&input);
        assert_eq!(encoded, pin::url_encode(&input));
        assert_eq!(decode(&encoded), input);
        let raw: Vec<u8> = (0..n % 83)
            .map(|_| b"%+0aGZ19"[(next() % 8) as usize])
            .collect();
        assert_eq!(decode(&raw), pin::url_decode(&raw));
    }
    for input in [&b""[..], b"!", b"\0\xff"] {
        assert_eq!(decode(&encode(input)), input);
    }
}

#[test]
fn boundaries_match_the_pin() {
    let parameters = "boundary=x|BOUNDARY=x|x=y; boundary=abc;next=z|boundary=\"abc\"|boundary=\"\"|\
        boundary=|boundary = x|boundary=x |boundary=x|;;; boundary=x|boundary=x;|x=boundary=q|\
        boundary=;|boundary=\"x;y\"";
    for input in parameters.split('|').map(str::as_bytes) {
        for length in 0..=input.len() {
            for offset in 0..=length + 1 {
                let mut span = HttpSpan {
                    offset: 7,
                    length: 3,
                };
                // SAFETY: `input` is initialized for `length` bytes.
                let found = unsafe {
                    swr_v1_http_boundary(input.as_ptr(), length, offset, Some(&mut span))
                } == 1;
                let expected = pin::boundary(&input[..length], offset);
                assert_eq!(
                    found.then_some((span.offset, span.length)),
                    expected,
                    "{input:?} {length} {offset}"
                );
            }
        }
    }
}
