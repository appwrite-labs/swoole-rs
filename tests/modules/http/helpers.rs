//! crates/http's status, method, URL and boundary helpers, which were
//! `src/protocol/http.cc`'s: upstream's `core-tests/src/server/http_server.cpp` TESTs
//! over them, one test each (tests/modules/http/README.md); their contracts (errors
//! leave outputs alone, overflows are refused); and comparisons with the pin's bodies
//! (v6.2.5; unchanged since v6.2.4). `pin` transcribes the pinned bodies (C locale).
//! The pin cases are the ones the retired `tests/modules/http/gate.cc`,
//! `status_buffer_gate.cc` and `dispatch_gate.cc` compared.
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::missing_assert_message,
    reason = "test helpers outside #[test] fns; a panic fails the case"
)]
use std::ffi::{CStr, c_int};
use std::mem::MaybeUninit;
use swoole_rs_http::{STATUS_CODES, method_string, status_message};

/// `get_status_message(code)`'s text.
fn status(code: i32) -> String {
    String::from_utf8(status_message(code).into_owned()).unwrap()
}

/// `get_method_string(method)`'s text, or None for its null.
fn method_name(method: i32) -> Option<&'static str> {
    method_string(method).map(|name| name.to_str().unwrap())
}

/// `SW_HTTP_GET`, `SW_HTTP_POST`, `SW_HTTP_OPTIONS`.
const SW_HTTP_GET: c_int = 2;
const SW_HTTP_POST: c_int = 4;
const SW_HTTP_OPTIONS: c_int = 8;

/// Upstream's `url_decode(str, len)`: crates/http's decoder over the `len` bytes,
/// then the terminator upstream writes after what it decoded.
fn url_decode(buffer: &mut [u8], len: usize) -> usize {
    let written = swoole_rs_http::url_decode(&mut buffer[..len]);
    buffer[written] = 0;
    written
}

fn terminated(buffer: &[u8]) -> &str {
    CStr::from_bytes_until_nul(buffer)
        .unwrap()
        .to_str()
        .unwrap()
}

/// `url_encode`, then `url_decode` in place.
fn encode_decode(input: &str, expected: &str) {
    let mut encoded = encode(input.as_bytes());
    encoded.push(0);
    assert_eq!(terminated(&encoded), expected);
    let decoded = url_decode(&mut encoded, expected.len());
    assert_eq!(decoded, input.len());
    assert_eq!(terminated(&encoded), input);
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
    assert_eq!(method_name(SW_HTTP_POST), Some("POST"));
    assert_eq!(method_name(SW_HTTP_GET), Some("GET"));
    assert_eq!(method_name(SW_HTTP_OPTIONS), Some("OPTIONS"));
    assert!(method_string(-1).is_none());
    assert!(method_string(999).is_none());
}

#[test]
fn get_status_message_cases() {
    for code in STATUS_CODES.into_iter().take_while(|code| *code != -1) {
        assert!(status(code).starts_with(&format!("{code} ")), "{code}");
    }
    assert_eq!(status(999), "999 Unknown Status");
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
        parse(
            b"Content-Type: multipart/form-data; boundary=----WebKitFormBoundaryX; charset=UTF-8",
            30
        ),
        (true, x),
        "a parameter after"
    );
    assert_eq!(
        parse(
            b"Content-Type: multipart/form-data; charset=UTF-8; boundary=----WebKitFormBoundaryX",
            30
        ),
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
    let written = url_decode(&mut buffer, 10);
    assert_eq!(&buffer[..=written], b"a b c%zz\0");
}

// The contracts.

const INVALID: i32 = -1;

/// `swoole_rs_http::url_encode` into `output`, which holds only bytes.
fn encode_into(input: &[u8], output: &mut [u8]) -> Result<usize, i32> {
    let mut slots: Vec<_> = output.iter().copied().map(MaybeUninit::new).collect();
    let result = swoole_rs_http::url_encode(input, &mut slots);
    // SAFETY: every slot started initialized, and the encoder writes only bytes.
    let bytes = slots.iter().map(|slot| unsafe { slot.assume_init() });
    output
        .iter_mut()
        .zip(bytes)
        .for_each(|(to, from)| *to = from);
    result
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
            let capacity = swoole_rs_http::url_capacity(&input).unwrap();
            assert_eq!(capacity, expected.len());
            let mut output = vec![0xa5; capacity + 3];
            let written = encode_into(&input, &mut output[..capacity]).unwrap();
            assert_eq!(written + 1, capacity);
            assert_eq!(&output[..capacity], expected);
            assert_eq!(&output[capacity..], &[0xa5; 3]);
        }
    }
}
#[test]
fn every_byte_round_trips_including_embedded_nuls() {
    let input: Vec<u8> = (0..=255).collect();
    let mut encoded = vec![0; swoole_rs_http::url_capacity(&input).unwrap()];
    let length = encode_into(&input, &mut encoded).unwrap();
    assert_eq!(length, 636);
    let length = url_decode(&mut encoded, length);
    assert_eq!(length, 256);
    assert_eq!(encoded[length], 0);
    assert_eq!(&encoded[..length], input);
    let mut malformed = b"%+%0%GG%a0+%00!".to_vec();
    let length = swoole_rs_http::url_decode(&mut malformed);
    assert_eq!(&malformed[..length], b"% %0%GG\xa0 \0!");
}
#[test]
fn errors_are_atomic_and_empty_encoding_has_terminator_storage() {
    let mut output = [99; 4];
    assert_eq!(encode_into(b"!!", &mut output), Err(INVALID));
    assert_eq!(output, [99; 4]);
    assert_eq!(swoole_rs_http::url_capacity(&[]), Ok(1));
    assert_eq!(encode_into(&[], &mut output[..1]), Ok(0));
    assert_eq!(output[0], 0);
}
#[test]
fn extreme_method_ids_do_not_panic() {
    assert!(method_string(i32::MIN).is_none());
    assert!(method_string(i32::MAX).is_none());
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
        let found = swoole_rs_http::boundary(input, offset).unwrap();
        assert_eq!(i32::from(found.is_some()), i32::from(expected.is_some()));
        if let Some(expected) = expected {
            let span = found.unwrap();
            assert_eq!(&input[span.offset..span.offset + span.length], expected);
        }
    }
}
#[test]
fn known_and_unknown_messages() {
    assert_eq!(status(-701), "-701 Unknown Status");
    assert_eq!(status(200), "200 OK");
    assert_eq!(status(0), status(200), "code 0 is 200's message");
    assert_eq!(status(-702), "-702 Unknown Status");
}
#[test]
fn multipart_exports_refuse_nulls_and_unterminated_scratch() {
    assert_eq!(
        i32::from(swoole_rs_http::multipart_disposition(
            b"Content-Disposition"
        )),
        1
    );
    assert_eq!(swoole_rs_http::multipart_filename(&mut []), Err(INVALID));
    let mut missing_nul = *b"filename";
    assert_eq!(
        swoole_rs_http::multipart_filename(&mut missing_nul),
        Err(INVALID)
    );
    let mut scratch = *b"name=x; filename=y\0";
    assert_eq!(swoole_rs_http::multipart_filename(&mut scratch), Ok(true));
    assert_eq!(swoole_rs_http::multipart_result(Ok(2), 2, false), Ok(()));
}

// Against the pin.

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
        let mut output = vec![0; input.len() * 3 + 1];
        // SAFETY: input is readable and output has room for maximum expansion and NUL.
        let written = unsafe {
            swoole_rs_oracle::http::url_encode(input.as_ptr(), input.len(), output.as_mut_ptr())
        };
        assert!(written < output.len());
        output.truncate(written);
        output
    }

    pub(super) fn url_decode(input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        output.push(0);
        // SAFETY: output has input.len() writable bytes plus the terminator slot.
        let written =
            unsafe { swoole_rs_oracle::http::url_decode(output.as_mut_ptr(), input.len()) };
        output.truncate(written);
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
        let message = status(code);
        // SAFETY: as above.
        assert_eq!(
            unsafe { *errno_location() },
            EINTR,
            "status {code} changed errno"
        );
        assert_eq!(message, pin::status(code));
    }
}

#[test]
fn status_code_array_matches_the_pin() {
    let mut expected = [0; 128];
    let codes = pin::STATUS
        .split('|')
        .map(|entry| entry[..3].parse().unwrap())
        .chain([-1]);
    expected
        .iter_mut()
        .zip(codes)
        .for_each(|(slot, code)| *slot = code);
    assert_eq!(STATUS_CODES, expected);
}

#[test]
fn methods_match_the_pin() {
    assert_eq!(i32::from(swoole_rs_http::METHOD_HEAD), pin::method(b"HEAD"));
    let method = swoole_rs_http::method;
    for id in -10..40 {
        let expected = pin::method_string(id);
        assert_eq!(method_name(id), expected);
        if let Some(name) = expected {
            let lower = name.to_ascii_lowercase();
            assert_eq!(method(lower.as_bytes()), pin::method(lower.as_bytes()));
            let spaced = lower + " ";
            assert_eq!(method(spaced.as_bytes()), pin::method(spaced.as_bytes()));
        }
    }
}

fn encode(input: &[u8]) -> Vec<u8> {
    let capacity = swoole_rs_http::url_capacity(input).unwrap();
    let mut output = vec![0xa5; capacity];
    let length = encode_into(input, &mut output).unwrap();
    assert_eq!(length + 1, capacity);
    assert_eq!(output.pop(), Some(0));
    output
}

fn decode(input: &[u8]) -> Vec<u8> {
    let mut data = input.to_vec();
    let written = swoole_rs_http::url_decode(&mut data);
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
        let input: Vec<u8> = (0..n % 257).map(|_| next().to_le_bytes()[0]).collect();
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
                let found = swoole_rs_http::boundary(&input[..length], offset).unwrap();
                let expected = pin::boundary(&input[..length], offset);
                assert_eq!(
                    found.map(|span| (span.offset, span.length)),
                    expected,
                    "{input:?} {length} {offset}"
                );
            }
        }
    }
}

#[test]
fn terminator_stays_within_the_requested_allocation() {
    for (input, length) in [(&b""[..], 0), (b" ", 3), (&[0xff; 31], 93)] {
        let mut output = vec![0xa5; length + 2];
        assert_eq!(encode_into(input, &mut output[..=length]), Ok(length));
        assert_eq!(&output[length..], &[0, 0xa5]);
    }
}

#[test]
fn unreserved_characters_remain_literal() {
    let literal = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    assert_eq!(encode(literal), literal);
}

#[test]
fn encode_respects_the_explicit_input_length() {
    assert_eq!(encode(&b"a b?ignored"[..3]), b"a%20b");
}

#[test]
fn allocation_failure_preserves_errno_and_releases_storage() {
    // Rust's encoder takes caller-owned storage, so neither upstream allocation is needed.
    let mut storage = [MaybeUninit::new(0); 6];
    assert_eq!(swoole_rs_http::url_encode(b"a b", &mut storage), Ok(5));
    // SAFETY: every slot was initialized before encoding.
    assert_eq!(
        storage.map(|slot| unsafe { slot.assume_init() }),
        *b"a%20b\0"
    );
}

#[test]
fn empty_input_has_a_terminator() {
    assert_eq!(encode(b""), b"");
    let mut buffer = *b"x";
    assert_eq!(url_decode(&mut buffer, 0), 0);
    assert_eq!(buffer, [0]);
}

#[test]
fn reserved_characters_and_binary_bytes_are_percent_encoded() {
    assert_eq!(
        encode(b" :/?#[]@!$&'()*+,;=\"\\\0\xff"),
        b"%20%3A%2F%3F%23%5B%5D%40%21%24%26%27%28%29%2A%2B%2C%3B%3D%22%5C%00%FF"
    );
}

#[test]
fn decode_accepts_both_hex_cases_and_preserves_malformed_escapes() {
    for (input, expected) in [
        (&b"a+b%2Bc%20d"[..], &b"a b+c d"[..]),
        (b"%41%4a%4A%7e%7E", b"AJJ~~"),
        (b"%00%Ff%ff", b"\0\xff\xff"),
        (b"%", b"%"),
        (b"%A", b"%A"),
        (b"%GG%4G%G4", b"%GG%4G%G4"),
        (b"%%41", b"%A"),
        (b"%\x80A%F\xff", b"%\x80A%F\xff"),
    ] {
        let mut output = input.to_vec();
        output.push(0xa5);
        assert_eq!(url_decode(&mut output, input.len()), expected.len());
        assert_eq!(&output[..expected.len()], expected);
        assert_eq!(output[expected.len()], 0);
    }
}

#[test]
fn decode_respects_the_input_boundary() {
    let mut output = *b"%41+B?!";
    assert_eq!(url_decode(&mut output, 5), 3);
    assert_eq!(terminated(&output), "A B");
    assert_eq!(&output[5..], b"?!");
}
