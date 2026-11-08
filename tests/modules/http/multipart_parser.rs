//! Upstream's `core-tests/src/server/multipart_parser.cpp`, one Rust test per TEST:
//! the bundled multipart parser (crates/oracle's copy of `thirdparty/multipart_parser.c`)
//! with the same boundary, inputs, callbacks and expected values. The random part
//! data is upstream's too: `swoole_rand(60, 120)` random bytes, base64-encoded by
//! upstream's `base64_encode` (`String::append_random_bytes(n, true)`).
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test helpers outside #[test] fns; a panic fails the case"
)]
use std::ffi::{c_char, c_int, c_void};
use std::hash::{BuildHasher, RandomState};
use swoole_rs_oracle::base64;

/// `multipart_parser`.
#[repr(C)]
struct Parser {
    settings: *const Settings,
    index: usize,
    error_i: usize,
    boundary_length: u8,
    fp: *mut c_void,
    data: *mut c_void,
    state: u8,
    error_reason: u8,
    boundary: [c_char; (2 + 70) * 2 + 9],
    error_expected: c_char,
    error_unexpected: c_char,
}

type DataCallback = unsafe extern "C" fn(*mut Parser, *const c_char, usize) -> c_int;
type Callback = unsafe extern "C" fn(*mut Parser) -> c_int;

/// `multipart_parser_settings`.
#[repr(C)]
struct Settings {
    on_header_field: DataCallback,
    on_header_value: DataCallback,
    on_part_data: DataCallback,
    on_part_data_begin: Callback,
    on_headers_complete: Callback,
    on_part_data_end: Callback,
    on_body_end: Callback,
}

unsafe extern "C" {
    fn multipart_parser_init(
        boundary: *const c_char,
        length: usize,
        settings: *const Settings,
    ) -> *mut Parser;
    fn multipart_parser_free(parser: *mut Parser);
    fn multipart_parser_execute(parser: *mut Parser, data: *const c_char, length: usize) -> isize;
    fn multipart_parser_error_msg(parser: *mut Parser, buf: *mut c_char, length: usize) -> c_int;
}

/// `enum multipart_error`.
const MPPE_OK: u8 = 0;
const MPPE_PAUSED: u8 = 1;
const MPPE_BOUNDARY_END_NO_CRLF: u8 = 3;
const MPPE_BAD_START_BOUNDARY: u8 = 4;
const MPPE_INVALID_HEADER_FIELD_CHAR: u8 = 5;
const MPPE_INVALID_HEADER_VALUE_CHAR: u8 = 6;
const MPPE_BAD_PART_END: u8 = 7;
const MPPE_END_BOUNDARY_NO_DASH: u8 = 8;
const MPPE_HEADER_VALUE_INCOMPLETE: u8 = 9;

/// `MppResult`.
#[derive(Default)]
struct MppResult {
    data: Vec<u8>,
    header_field: Vec<u8>,
    header_value: Vec<u8>,
    header_complete: bool,
    body_end: bool,
}

unsafe fn result<'a>(parser: *mut Parser) -> &'a mut MppResult {
    unsafe { &mut *(*parser).data.cast::<MppResult>() }
}

unsafe fn bytes<'a>(at: *const c_char, length: usize) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(at.cast(), length) }
}

unsafe extern "C" fn on_header_field(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    unsafe { result(parser).header_field = bytes(at, length).to_vec() };
    0
}

unsafe extern "C" fn on_header_value(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    unsafe { result(parser).header_value = bytes(at, length).to_vec() };
    0
}

unsafe extern "C" fn on_data(parser: *mut Parser, at: *const c_char, length: usize) -> c_int {
    unsafe { result(parser).data.extend_from_slice(bytes(at, length)) };
    0
}

unsafe extern "C" fn on_header_complete(parser: *mut Parser) -> c_int {
    unsafe { result(parser).header_complete = true };
    0
}

unsafe extern "C" fn on_body_end(parser: *mut Parser) -> c_int {
    unsafe { result(parser).body_end = true };
    0
}

unsafe extern "C" fn noop(_: *mut Parser) -> c_int {
    0
}

/// `_settings`.
static SETTINGS: Settings = Settings {
    on_header_field,
    on_header_value,
    on_part_data: on_data,
    on_part_data_begin: noop,
    on_headers_complete: on_header_complete,
    on_part_data_end: noop,
    on_body_end,
};

const BOUNDARY: &[u8] = b"--WebKitFormBoundaryeGOz80A8JnaO6kuw";

/// `create_parser()`, freed on drop.
struct Multipart(*mut Parser);

impl Multipart {
    fn new() -> Self {
        Multipart(unsafe {
            multipart_parser_init(
                BOUNDARY.as_ptr().cast(),
                BOUNDARY.len(),
                &raw const SETTINGS,
            )
        })
    }

    fn p(&mut self) -> &mut Parser {
        unsafe { &mut *self.0 }
    }

    fn execute(&mut self, data: &[u8]) -> isize {
        unsafe { multipart_parser_execute(self.0, data.as_ptr().cast(), data.len()) }
    }

    /// `std::string(parser->boundary, parser->boundary_length)`.
    fn boundary(&mut self) -> Vec<u8> {
        let p = self.p();
        p.boundary[..usize::from(p.boundary_length)]
            .iter()
            .map(|&c| c.cast_unsigned())
            .collect()
    }
}

impl Drop for Multipart {
    fn drop(&mut self) {
        unsafe { multipart_parser_free(self.0) };
    }
}

fn len(data: &[u8]) -> isize {
    data.len().try_into().unwrap()
}

/// `create_error`: the message for `reason` is non-empty and contains `error`.
fn create_error(parser: &mut Multipart, reason: u8, error: &str) {
    let mut buf = [0 as c_char; 1024];
    parser.p().error_reason = reason;
    let result_len = unsafe { multipart_parser_error_msg(parser.0, buf.as_mut_ptr(), buf.len()) };
    assert!(result_len > 0, "{error}");
    let response = unsafe { bytes(buf.as_ptr(), result_len.try_into().unwrap()) };
    assert!(String::from_utf8_lossy(response).contains(error), "{error}");
}

#[test]
fn error_message() {
    let mut buf = [0 as c_char; 1024];
    let mut parser = Multipart::new();
    parser.p().error_reason = MPPE_OK;
    assert_eq!(
        unsafe { multipart_parser_error_msg(parser.0, buf.as_mut_ptr(), buf.len()) },
        0,
        "MPPE_OK"
    );
    for expected in *b"\0\r\na" {
        parser.p().error_expected = expected.cast_signed();
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
    let mut parser = Multipart::new();
    let header = header(true);
    let mut result = MppResult::default();
    parser.p().data = (&raw mut result).cast();
    assert_eq!(parser.execute(&header), len(&header), "header parsed");
    assert_eq!(result.header_field, b"Content-Disposition", "field");
    assert!(
        result.header_value.windows(4).any(|w| w == b"test"),
        "value names test"
    );
    let boundary = parser.boundary();
    assert_eq!(parser.execute(b"\r\n"), 2, "CRLF");
    assert_eq!(parser.execute(&boundary), len(&boundary), "boundary");
    assert_eq!(parser.execute(b"--\r\n\r\n"), 6, "final hyphens");
}

#[test]
fn header_error() {
    let mut parser = Multipart::new();
    let header = header(false);
    let mut result = MppResult::default();
    parser.p().data = (&raw mut result).cast();
    assert_eq!(parser.execute(&header), -1, "incomplete header");
    assert_eq!(
        parser.p().error_reason,
        MPPE_HEADER_VALUE_INCOMPLETE,
        "reason"
    );
    assert_eq!(
        parser.p().error_expected,
        b'\r'.cast_signed(),
        "expected CR"
    );
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
    let n = unsafe { base64::encode(raw.as_ptr(), raw.len(), out.as_mut_ptr()) };
    data.extend_from_slice(&out[..n]);
}

/// The case's `append_data`: more random bytes, sent alone.
fn append_data(parser: &mut Multipart, data: &mut Vec<u8>) {
    let offset = data.len();
    append_random_bytes(data);
    let part = &data[offset..];
    assert_eq!(parser.execute(part), len(part), "appended data");
}

#[test]
fn data() {
    let mut parser = Multipart::new();
    let header = header(true);
    let mut result = MppResult::default();
    parser.p().data = (&raw mut result).cast();
    assert_eq!(parser.execute(&header), len(&header), "header parsed");
    let boundary = parser.boundary();
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
    assert_eq!(parser.execute(&data), len(&data), "first data");

    append_data(&mut parser, &mut data);
    data.push(b'\r');
    assert_eq!(parser.execute(b"\r"), 1, "CR");
    append_data(&mut parser, &mut data);
    data.extend_from_slice(b"\r\n");
    assert_eq!(parser.execute(b"\r\n"), 2, "CRLF");
    append_data(&mut parser, &mut data);

    let offset = data.len();
    data.extend_from_slice(&prefix(&boundary));
    assert_eq!(
        parser.execute(&data[offset..]),
        len(&data[offset..]),
        "boundary prefix"
    );

    assert_eq!(parser.execute(b"\r\n"), 2, "CRLF");
    assert_eq!(parser.execute(&boundary), len(&boundary), "boundary");
    assert_eq!(parser.execute(b"--\r\n\r\n"), 6, "final hyphens");

    assert_eq!(data[..result.data.len()], result.data, "part data");
    assert!(result.header_complete, "headers complete");
    assert!(result.body_end, "body end");
}
