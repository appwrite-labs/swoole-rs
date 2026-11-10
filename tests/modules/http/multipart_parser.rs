//! Upstream's `core-tests/src/server/multipart_parser.cpp`, one Rust test per TEST,
//! over crates/http's port of `thirdparty/multipart_parser.c` with the same boundary,
//! inputs, callbacks and expected values. The random part data is upstream's too:
//! `swoole_rand(60, 120)` random bytes, base64-encoded by upstream's `base64_encode`
//! (`String::append_random_bytes(n, true)`).
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test helpers outside #[test] fns; a panic fails the case"
)]
use std::hash::{BuildHasher, RandomState};
use swoole_rs_http::multipart_parser::{
    MPPE_BAD_PART_END, MPPE_BAD_START_BOUNDARY, MPPE_BOUNDARY_END_NO_CRLF,
    MPPE_END_BOUNDARY_NO_DASH, MPPE_HEADER_VALUE_INCOMPLETE, MPPE_INVALID_HEADER_FIELD_CHAR,
    MPPE_INVALID_HEADER_VALUE_CHAR, MPPE_OK, MultipartParser, Parts,
};

/// `MPPE_PAUSED`, as an error reason.
const MPPE_PAUSED: u8 = 1;

/// `MppResult`, filled by `_settings`' callbacks.
#[derive(Default)]
struct MppResult {
    data: Vec<u8>,
    header_field: Vec<u8>,
    header_value: Vec<u8>,
    header_complete: bool,
    body_end: bool,
}

impl Parts for MppResult {
    fn on_header_field(&mut self, data: &[u8]) -> i32 {
        self.header_field = data.to_vec();
        0
    }

    fn on_header_value(&mut self, data: &[u8]) -> i32 {
        self.header_value = data.to_vec();
        0
    }

    fn on_part_data(&mut self, data: &[u8]) -> i32 {
        self.data.extend_from_slice(data);
        0
    }

    fn on_headers_complete(&mut self) -> i32 {
        self.header_complete = true;
        0
    }

    fn on_body_end(&mut self, _: &[u8]) -> i32 {
        self.body_end = true;
        0
    }
}

const BOUNDARY: &[u8] = b"--WebKitFormBoundaryeGOz80A8JnaO6kuw";

/// `create_parser()`.
fn create_parser() -> MultipartParser {
    MultipartParser::new(BOUNDARY)
}

fn len(data: &[u8]) -> isize {
    data.len().try_into().unwrap()
}

/// `create_error`: the message for `reason` is non-empty and contains `error`.
fn create_error(parser: &mut MultipartParser, reason: u8, error: &str) {
    parser.error_reason = reason;
    let response = parser.error_message();
    assert!(!response.is_empty(), "{error}");
    assert!(
        String::from_utf8_lossy(&response).contains(error),
        "{error}"
    );
}

#[test]
fn error_message() {
    let mut parser = create_parser();
    parser.error_reason = MPPE_OK;
    assert!(parser.error_message().is_empty(), "MPPE_OK");
    for expected in *b"\0\r\na" {
        parser.error_expected = expected;
        create_error(&mut parser, MPPE_PAUSED, "parser paused");
        create_error(
            &mut parser,
            MPPE_BOUNDARY_END_NO_CRLF,
            "no CRLF at first boundary end: ",
        );
        create_error(
            &mut parser,
            MPPE_BAD_START_BOUNDARY,
            "first boundary mismatching: ",
        );
        create_error(
            &mut parser,
            MPPE_INVALID_HEADER_FIELD_CHAR,
            "invalid char in header field: ",
        );
        create_error(
            &mut parser,
            MPPE_INVALID_HEADER_VALUE_CHAR,
            "invalid char in header value: ",
        );
        create_error(
            &mut parser,
            MPPE_BAD_PART_END,
            "no next part or final hyphen: expecting CR or '-' ",
        );
        create_error(&mut parser, MPPE_END_BOUNDARY_NO_DASH, "bad final hyphen: ");
    }
}

/// The header part every case sends: `--` + boundary + CRLF + `Content-Disposition`.
fn header(complete: bool) -> Vec<u8> {
    let mut header = b"--".to_vec();
    header.extend_from_slice(BOUNDARY);
    header.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"test\"");
    if complete {
        header.extend_from_slice(b"\r\n\r\n");
    }
    header
}

#[test]
fn header_field() {
    let mut parser = create_parser();
    let header = header(true);
    let mut result = MppResult::default();
    assert_eq!(
        parser.execute(&mut result, &header),
        len(&header),
        "header parsed"
    );
    assert_eq!(result.header_field, b"Content-Disposition", "field");
    assert!(
        result.header_value.windows(4).any(|w| w == b"test"),
        "value names test"
    );
    let boundary = parser.boundary().to_vec();
    assert_eq!(parser.execute(&mut result, b"\r\n"), 2, "CRLF");
    assert_eq!(
        parser.execute(&mut result, &boundary),
        len(&boundary),
        "boundary"
    );
    assert_eq!(
        parser.execute(&mut result, b"--\r\n\r\n"),
        6,
        "final hyphens"
    );
}

#[test]
fn header_error() {
    let mut parser = create_parser();
    let header = header(false);
    let mut result = MppResult::default();
    assert_eq!(
        parser.execute(&mut result, &header),
        -1,
        "incomplete header"
    );
    assert_eq!(parser.error_reason, MPPE_HEADER_VALUE_INCOMPLETE, "reason");
    assert_eq!(parser.error_expected, b'\r', "expected CR");
}

/// `swoole_rand(min, max)`: uniform over `min..=max`.
fn rand(min: usize, max: usize) -> usize {
    let random = usize::try_from(RandomState::new().hash_one(0u8) >> 1).unwrap();
    min + random % (max - min + 1)
}

/// `String::append_random_bytes(swoole_rand(60, 120), true)`.
fn append_random_bytes(data: &mut Vec<u8>) {
    let raw: Vec<u8> = (0..rand(60, 120))
        .map(|_| RandomState::new().hash_one(0u8).to_le_bytes()[0])
        .collect();
    let mut out = vec![0; raw.len().div_ceil(3) * 4 + 1];
    let n = swoole_rs_base64::encode(&raw, &mut out);
    data.extend_from_slice(&out[..n]);
}

/// The case's `append_data`: more random bytes, sent alone.
fn append_data(parser: &mut MultipartParser, result: &mut MppResult, data: &mut Vec<u8>) {
    let offset = data.len();
    append_random_bytes(data);
    let part = &data[offset..];
    assert_eq!(parser.execute(result, part), len(part), "appended data");
}

#[test]
fn data() {
    let mut parser = create_parser();
    let header = header(true);
    let mut result = MppResult::default();
    assert_eq!(
        parser.execute(&mut result, &header),
        len(&header),
        "header parsed"
    );
    let boundary = parser.boundary().to_vec();
    let prefix = |boundary: &[u8]| boundary[..rand(1, boundary.len() - 2)].to_vec();

    // data part
    let mut data = Vec::with_capacity(128);
    append_random_bytes(&mut data);
    data.push(b'\r');
    append_random_bytes(&mut data);
    data.extend_from_slice(b"\r\n");
    append_random_bytes(&mut data);
    data.extend_from_slice(b"\r\n");
    data.extend_from_slice(&prefix(&boundary));
    append_random_bytes(&mut data);
    assert_eq!(parser.execute(&mut result, &data), len(&data), "first data");

    append_data(&mut parser, &mut result, &mut data);
    data.push(b'\r');
    assert_eq!(parser.execute(&mut result, b"\r"), 1, "CR");
    append_data(&mut parser, &mut result, &mut data);
    data.extend_from_slice(b"\r\n");
    assert_eq!(parser.execute(&mut result, b"\r\n"), 2, "CRLF");
    append_data(&mut parser, &mut result, &mut data);

    let offset = data.len();
    data.extend_from_slice(&prefix(&boundary));
    assert_eq!(
        parser.execute(&mut result, &data[offset..]),
        len(&data[offset..]),
        "boundary prefix"
    );

    assert_eq!(parser.execute(&mut result, b"\r\n"), 2, "CRLF");
    assert_eq!(
        parser.execute(&mut result, &boundary),
        len(&boundary),
        "boundary"
    );
    assert_eq!(
        parser.execute(&mut result, b"--\r\n\r\n"),
        6,
        "final hyphens"
    );

    assert_eq!(data[..result.data.len()], result.data, "part data");
    assert!(result.header_complete, "headers complete");
    assert!(result.body_end, "body end");
}
