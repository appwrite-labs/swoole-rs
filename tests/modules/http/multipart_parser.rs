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
use swoole_rs_http::multipart_parser::{Flow, MultipartError, MultipartParser, ParseError, Parts};

/// `MppResult`, filled by `_settings`' callbacks.
#[derive(Default)]
struct MppResult {
    data: Vec<u8>,
    header_field: Vec<u8>,
    header_value: Vec<u8>,
    header_value_complete_count: usize,
    header_complete: bool,
    body_end: bool,
}

impl Parts<'_> for MppResult {
    fn on_header_field(&mut self, data: &[u8]) -> Flow {
        self.header_field.extend_from_slice(data);
        Flow::Go
    }

    fn on_header_value(&mut self, data: &[u8]) -> Flow {
        self.header_value.extend_from_slice(data);
        Flow::Go
    }

    fn on_header_value_complete(&mut self) -> Flow {
        self.header_value_complete_count += 1;
        Flow::Go
    }

    fn on_part_data(&mut self, data: &[u8]) -> Flow {
        self.data.extend_from_slice(data);
        Flow::Go
    }

    fn on_headers_complete(&mut self) -> Flow {
        self.header_complete = true;
        Flow::Go
    }

    fn on_body_end(&mut self, _: &[u8]) -> Flow {
        self.body_end = true;
        Flow::Go
    }
}

const BOUNDARY: &[u8] = b"--WebKitFormBoundaryeGOz80A8JnaO6kuw";

/// `create_parser()`.
fn create_parser() -> MultipartParser {
    MultipartParser::new(BOUNDARY).unwrap()
}

/// `create_error`: the message for `reason` is non-empty and contains `error`.
fn create_error(parser: &mut MultipartParser, reason: MultipartError, error: &str) {
    parser.error_reason = Some(reason);
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
    parser.error_reason = None;
    assert!(parser.error_message().is_empty(), "MPPE_OK");
    for expected in *b"\0\r\na" {
        parser.error_expected = expected;
        create_error(&mut parser, MultipartError::Paused, "parser paused");
        create_error(
            &mut parser,
            MultipartError::BoundaryEndNoCrlf,
            "no CRLF at first boundary end: ",
        );
        create_error(
            &mut parser,
            MultipartError::BadStartBoundary,
            "first boundary mismatching: ",
        );
        create_error(
            &mut parser,
            MultipartError::InvalidHeaderFieldChar,
            "invalid char in header field: ",
        );
        create_error(
            &mut parser,
            MultipartError::InvalidHeaderValueChar,
            "invalid char in header value: ",
        );
        create_error(
            &mut parser,
            MultipartError::BadPartEnd,
            "no next part or final hyphen: expecting CR or '-' ",
        );
        create_error(
            &mut parser,
            MultipartError::EndBoundaryNoDash,
            "bad final hyphen: ",
        );
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
fn callback_error() {
    enum Reject {
        Field,
        Begin,
    }
    impl Parts<'_> for Reject {
        fn on_header_field(&mut self, _: &[u8]) -> Flow {
            Flow::Fail
        }
        fn on_part_data_begin(&mut self, _: &[u8]) -> Flow {
            match self {
                Self::Begin => Flow::Fail,
                Self::Field => Flow::Go,
            }
        }
    }
    for mut callback in [Reject::Field, Reject::Begin] {
        let mut parser = create_parser();
        assert_eq!(
            parser.execute(&mut callback, &header(true)),
            Err(ParseError::Callback)
        );
        assert_eq!(parser.error_message(), b"callback error");
    }
}

#[test]
fn header_field() {
    let mut parser = create_parser();
    let header = header(true);
    let mut result = MppResult::default();
    assert_eq!(
        parser.execute(&mut result, &header),
        Ok(header.len()),
        "header parsed"
    );
    assert_eq!(result.header_field, b"Content-Disposition", "field");
    assert!(
        result.header_value.windows(4).any(|w| w == b"test"),
        "value names test"
    );
    assert_eq!(result.header_value_complete_count, 1);
    let boundary = parser.boundary().to_vec();
    assert_eq!(parser.execute(&mut result, b"\r\n"), Ok(2), "CRLF");
    assert_eq!(
        parser.execute(&mut result, &boundary),
        Ok(boundary.len()),
        "boundary"
    );
    assert_eq!(
        parser.execute(&mut result, b"--\r\n\r\n"),
        Ok(6),
        "final hyphens"
    );
}

#[test]
fn fragmented_header() {
    let header = header(true);
    let check = |chunks: &mut dyn Iterator<Item = &[u8]>| {
        let mut parser = create_parser();
        let mut result = MppResult::default();
        for chunk in chunks {
            assert_eq!(parser.execute(&mut result, chunk), Ok(chunk.len()));
        }
        assert_eq!(result.header_field, b"Content-Disposition");
        assert_eq!(result.header_value, br#"form-data; name="test""#);
        assert_eq!(result.header_value_complete_count, 1);
    };
    for split in 1..header.len() {
        let (first, second) = header.split_at(split);
        check(&mut [first, second].into_iter());
    }
    check(&mut header.chunks(1));
}

#[test]
fn incomplete_header_value() {
    let mut parser = create_parser();
    let mut result = MppResult::default();
    let header = header(false);
    assert_eq!(parser.execute(&mut result, &header), Ok(header.len()));
    assert!(!parser.is_complete());
    assert_eq!(result.header_value, br#"form-data; name="test""#);
    assert_eq!(result.header_value_complete_count, 0);
    assert_eq!(parser.execute(&mut result, b"\r\n\r\n"), Ok(4));
    assert_eq!(result.header_value_complete_count, 1);
}

#[test]
fn malformed_header() {
    for (line, reason) in [
        (
            b"Content@Disposition: value\r\n\r\n".as_slice(),
            MultipartError::InvalidHeaderFieldChar,
        ),
        (
            b"Content-Disposition: value\rX",
            MultipartError::InvalidHeaderValueChar,
        ),
    ] {
        let mut parser = create_parser();
        let input = [b"--", BOUNDARY, b"\r\n", line].concat();
        assert_eq!(
            parser.execute(&mut MppResult::default(), &input),
            Err(ParseError::Parser(reason))
        );
        assert_eq!(parser.error_reason, Some(reason));
    }
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
    assert_eq!(
        parser.execute(result, part),
        Ok(part.len()),
        "appended data"
    );
}

#[test]
fn data() {
    let mut parser = create_parser();
    let header = header(true);
    let mut result = MppResult::default();
    assert_eq!(
        parser.execute(&mut result, &header),
        Ok(header.len()),
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
    assert_eq!(
        parser.execute(&mut result, &data),
        Ok(data.len()),
        "first data"
    );

    append_data(&mut parser, &mut result, &mut data);
    data.push(b'\r');
    assert_eq!(parser.execute(&mut result, b"\r"), Ok(1), "CR");
    append_data(&mut parser, &mut result, &mut data);
    data.extend_from_slice(b"\r\n");
    assert_eq!(parser.execute(&mut result, b"\r\n"), Ok(2), "CRLF");
    append_data(&mut parser, &mut result, &mut data);

    let offset = data.len();
    data.extend_from_slice(&prefix(&boundary));
    assert_eq!(
        parser.execute(&mut result, &data[offset..]),
        Ok(data[offset..].len()),
        "boundary prefix"
    );

    assert_eq!(parser.execute(&mut result, b"\r\n"), Ok(2), "CRLF");
    assert_eq!(
        parser.execute(&mut result, &boundary),
        Ok(boundary.len()),
        "boundary"
    );
    assert_eq!(
        parser.execute(&mut result, b"--\r\n\r\n"),
        Ok(6),
        "final hyphens"
    );

    assert_eq!(data[..result.data.len()], result.data, "part data");
    assert!(result.header_complete, "headers complete");
    assert!(result.body_end, "body end");
    assert!(parser.is_complete());
}

#[test]
fn boundary_length_includes_both_leading_dashes() {
    let boundary = [b'b'; 253];
    let parser = MultipartParser::new(&boundary).unwrap();
    assert_eq!(
        parser.boundary().split_at(2),
        (b"--".as_slice(), boundary.as_slice())
    );
    assert!(MultipartParser::new(&[b'b'; 254]).is_none());
}
