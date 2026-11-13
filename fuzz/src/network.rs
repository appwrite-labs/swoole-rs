//! Network parsers fuzzed on their own: each target checks the parser against a
//! reference decode or against invariants of its output, not against the oracle.

use std::cell::Cell;
use std::ffi::{c_char, c_int};
use std::net::IpAddr;
use swoole_rs_clients::dns::{DnsExchange, query};
use swoole_rs_http::llhttp::{self, Callbacks, Parser};
use swoole_rs_http::multipart_parser::MultipartParser;
use swoole_rs_http2::{
    frame_length, sw_rs_http2_decode_frame, sw_rs_http2_inflate_step, sw_rs_http2_unpack_setting,
};
use swoole_rs_inflate::Inflate;
use swoole_rs_oracle::nghttp2;
use swoole_rs_protocols::{RespPhpState, resp_php_next};

/// Any bytes as an HTTP/2 frame header (RFC 9113 4.1) and a SETTINGS entry (6.5.1):
/// `frame_length`, `sw_rs_http2_decode_frame` and `sw_rs_http2_unpack_setting`
/// agree with the wire layout.
pub fn http2_frame(bytes: &[u8]) {
    let Some((&[previous, client], data)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let length = frame_length(data);
    let previous = u32::from(previous) << 23;
    let decoded = sw_rs_http2_decode_frame(data, previous, client & 1 != 0);
    let Some(header) = data.first_chunk::<9>() else {
        assert_eq!((length, decoded), (0, None), "short header {data:?}");
        return;
    };
    let wire = u32::from_be_bytes([0, header[0], header[1], header[2]]);
    let id = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & 0x7fff_ffff;
    assert_eq!(length, wire as isize + 9, "frame length of {header:?}");
    let facts = decoded.expect("a whole header decodes");
    assert_eq!(
        (facts.length, facts.kind, facts.flags, facts.stream_id),
        (wire, u32::from(header[3]), u32::from(header[4]), id),
        "frame header {header:?}"
    );
    assert_eq!(facts.publish_last_id, id > previous);
    if let Some(entry) = data.get(9..).and_then(|rest| rest.first_chunk::<6>()) {
        let (setting, value) = sw_rs_http2_unpack_setting(entry);
        assert_eq!(
            (setting, value),
            (
                u16::from_be_bytes([entry[0], entry[1]]),
                u32::from_be_bytes([entry[2], entry[3], entry[4], entry[5]])
            )
        );
    }
}

/// The DNS reply `dns_reply` hands the resolver, and what it pushed.
struct Exchange<'a> {
    reply: &'a [u8],
    echo_id: bool,
    query: Vec<u8>,
    pushed: Vec<Vec<u8>>,
}

impl DnsExchange for Exchange<'_> {
    fn send(&mut self, data: &[u8]) -> bool {
        self.query = data.to_vec();
        true
    }

    fn recv(&mut self, out: &mut [u8]) -> isize {
        let mut reply = self.reply.to_vec();
        if self.echo_id {
            for (byte, id) in reply.iter_mut().zip(&self.query) {
                *byte = *id;
            }
        }
        let n = reply.len().min(out.len());
        out[..n].copy_from_slice(&reply[..n]);
        n as isize
    }

    fn push(&mut self, text: &[u8]) {
        self.pushed.push(text.to_vec());
    }
}

/// Any bytes as the DNS server's reply to an A or AAAA query: the resolver reads only
/// inside its packet, succeeds exactly when it pushed an address, and every address
/// it pushes is IP text.
pub fn dns_reply(bytes: &[u8]) {
    let Some((&flags, reply)) = bytes.split_first() else {
        return;
    };
    let family = if flags & 1 != 0 {
        libc::AF_INET6
    } else {
        libc::AF_INET
    };
    let mut exchange = Exchange {
        reply,
        echo_id: flags & 2 != 0,
        query: Vec::new(),
        pushed: Vec::new(),
    };
    let result = query(
        c"example.test",
        family,
        u16::from(flags >> 2) + 1,
        &mut exchange,
        c_char::MIN != 0,
    );
    let expected = if exchange.pushed.is_empty() { 711 } else { 0 };
    assert_eq!(result, expected, "reply {reply:?}");
    for text in &exchange.pushed {
        let text = std::str::from_utf8(text).expect("address text");
        text.parse::<IpAddr>().expect("an IP address");
    }
}

thread_local! {
    /// The chunk the parser is reading.
    static CHUNK: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// A span the parser emits lies inside the chunk it was handed.
fn in_chunk(at: *const c_char, length: usize) {
    let (start, end) = CHUNK.get();
    let at = at as usize;
    assert!(
        at >= start && at.checked_add(length).is_some_and(|stop| stop <= end),
        "span outside the chunk"
    );
}

/// The multipart callbacks: header spans checked, the current part's header value
/// kept for the filename scan.
#[derive(Default)]
struct Parts {
    value: Vec<u8>,
}

impl swoole_rs_http::multipart_parser::Parts for Parts {
    fn on_header_field(&mut self, data: &[u8]) -> i32 {
        in_chunk(data.as_ptr().cast(), data.len());
        0
    }

    fn on_header_value(&mut self, data: &[u8]) -> i32 {
        in_chunk(data.as_ptr().cast(), data.len());
        self.value.extend_from_slice(data);
        0
    }

    fn on_headers_complete(&mut self) -> i32 {
        let mut value = std::mem::take(&mut self.value);
        value.push(0);
        // The scan reads the value through its first NUL.
        let end = value.iter().position(|&byte| byte == 0).unwrap_or_default();
        let status = swoole_rs_http::multipart_filename(&mut value[..=end]);
        assert!(matches!(status, 0 | 1), "filename status {status}");
        0
    }
}

/// A boundary of 1 to 70 bytes (RFC 2046 5.1.1) and any body, handed to the multipart
/// parser the server streams uploads through, in fuzzer-chosen chunks: each call
/// consumes at most its chunk or fails with -1, every header span stays inside it, and
/// the filename scan of each part's headers stays in bounds.
pub fn multipart(bytes: &[u8]) {
    let Some((&[boundary, step], rest)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let Some((boundary, body)) = rest.split_at_checked(usize::from(boundary % 70) + 1) else {
        return;
    };
    let mut parser = MultipartParser::new(boundary);
    let mut parts = Parts::default();
    let step = usize::from(step).max(1);
    for chunk in body.chunks(step) {
        let start = chunk.as_ptr() as usize;
        CHUNK.set((start, start + chunk.len()));
        let processed = parser.execute(&mut parts, chunk);
        assert!(
            usize::try_from(processed).map_or(processed == -1, |n| n <= chunk.len()),
            "processed {processed} of {}",
            chunk.len()
        );
        if swoole_rs_http::multipart_result(processed as i64, chunk.len(), 0) != 2 {
            break;
        }
    }
}

/// Body bytes the parser delivered; each span must lie inside its chunk.
struct Chunked {
    parser: Parser,
    body: usize,
}

impl Callbacks for Chunked {
    fn parser(&mut self) -> &mut Parser {
        &mut self.parser
    }

    fn on_body(&mut self, data: &[u8]) -> c_int {
        in_chunk(data.as_ptr().cast(), data.len());
        self.body += data.len();
        0
    }
}

/// A request (or, on an odd first byte, a response) head with chunked transfer
/// coding and any body, handed to crates/http's llhttp the server and client parse with
/// in fuzzer-chosen chunks: every body span lies inside its chunk and the body never
/// exceeds what was received.
pub fn http_chunked(bytes: &[u8]) {
    let Some((&[kind, step], body)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let (kind, head): (u8, &[u8]) = if kind & 1 == 0 {
        (
            llhttp::REQUEST,
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n",
        )
    } else {
        (
            llhttp::RESPONSE,
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        )
    };
    let mut parser = Chunked {
        parser: Parser::new(kind),
        body: 0,
    };
    let input = [head, body].concat();
    for chunk in input.chunks(usize::from(step).max(1)) {
        let start = chunk.as_ptr() as usize;
        CHUNK.set((start, start + chunk.len()));
        if llhttp::execute(&mut parser, chunk) != llhttp::HPE_OK {
            break;
        }
    }
    assert!(
        parser.body <= body.len(),
        "body {} of {}",
        parser.body,
        body.len()
    );
}

/// Any bytes as HEADERS or CONTINUATION payloads of one connection (a fuzzer-chosen
/// frame size, server or client role, final or not), each decoded by
/// `swoole_rs_http2::hpack` and by the nghttp2 inflater upstream bundles, through
/// the step loop the HTTP/2 server and client run (`sw_rs_http2_inflate_step`):
/// both consume the same bytes, decode the same fields and fail on the same step,
/// and the loop ends within one step per input byte.
pub fn http2_hpack(bytes: &[u8]) {
    let Some((&[role, size], blocks)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let last = role & 2 == 0;
    let mut decoder = swoole_rs_http2::hpack::Decoder::default();
    let mut inflater = std::ptr::null_mut();
    // SAFETY: `inflater` is a writable pointer.
    assert_eq!(unsafe { nghttp2::nghttp2_hd_inflate_new(&mut inflater) }, 0);
    'blocks: for block in blocks.chunks(usize::from(size).max(1)) {
        let mut input = block;
        let mut steps = 0;
        loop {
            steps += 1;
            assert!(steps <= block.len() + 1, "inflate loop over {block:?}");
            let mut nv = nghttp2::Nv {
                name: std::ptr::null(),
                value: std::ptr::null(),
                name_length: 0,
                value_length: 0,
                flags: 0,
            };
            let mut flags = 0;
            // SAFETY: a live inflater over `input`; `nv` and `flags` are writable.
            let read = unsafe {
                nghttp2::nghttp2_hd_inflate_hd(
                    inflater,
                    &mut nv,
                    &mut flags,
                    input.as_ptr(),
                    input.len(),
                    c_int::from(last),
                )
            };
            let rust = decoder.decode(input, last);
            // A decoding error ends the connection.
            let Ok(read) = usize::try_from(read) else {
                assert!(rust.is_err(), "nghttp2 failed on {input:?}");
                break 'blocks;
            };
            let rust = rust.expect("hpack failed where nghttp2 decoded");
            assert_eq!(rust.read, read, "bytes read from {input:?}");
            #[expect(clippy::cast_sign_loss, reason = "nghttp2's flag bits")]
            let flags = flags as u32;
            assert_eq!(rust.flags(), flags, "flags over {input:?}");
            if let Some((name, value)) = rust.header {
                // SAFETY: nghttp2's field, live until the next call.
                let expected = unsafe { std::slice::from_raw_parts(nv.name, nv.name_length) };
                assert_eq!(name, expected, "name from {input:?}");
                // SAFETY: as above.
                let expected = unsafe { std::slice::from_raw_parts(nv.value, nv.value_length) };
                assert_eq!(value, expected, "value from {input:?}");
            }
            input = input.get(read..).expect("inflate read past its input");
            let step = sw_rs_http2_inflate_step(flags, input.len(), role & 1 != 0);
            // The client ends its (always final) block on any finished step too.
            if step.end_headers || step.finish && role & 1 != 0 && last {
                // SAFETY: a live inflater.
                unsafe { nghttp2::nghttp2_hd_inflate_end_headers(inflater) };
                decoder.end_headers();
                break;
            }
            if step.finish {
                break;
            }
        }
    }
    // SAFETY: the inflater made above.
    unsafe { nghttp2::nghttp2_hd_inflate_del(inflater) };
}

/// Any bytes as a `Redis\Server` request, walked with `resp_php_next` as the
/// server's receive loop does: every event's span lies inside the request, the walk
/// moves forward and ends, and only the first string is the command.
pub fn resp_server(bytes: &[u8]) {
    let mut state = RespPhpState::default();
    let mut commands = 0;
    for _ in 0..=bytes.len() {
        let before = state.cursor.position;
        let Ok(Some(span)) = resp_php_next(bytes, &mut state, &mut false) else {
            return;
        };
        assert!(state.cursor.position > before, "stalled on {bytes:?}");
        assert!(
            span.kind == 1
                || span.kind == 2
                || span
                    .offset
                    .checked_add(span.length)
                    .is_some_and(|end| end <= bytes.len()),
            "span {span:?} outside {bytes:?}"
        );
        commands += usize::from(span.kind == 3);
        assert!(commands <= 1, "second command in {bytes:?}");
    }
    panic!("no end for {bytes:?}");
}

/// Up to 32 bytes of pattern, a replacement and up to 64 bytes of URL, NUL-separated, as one
/// `~pattern~` rule of `RewriteRules`: compiling and applying it ends, and a pattern of
/// letters, digits and `/` (all literal) replaces each occurrence, left to right, as
/// `std::regex_replace` does. libstdc++ itself is compared in `tests/modules/http/rewrite.rs`.
pub fn http_rewrite(bytes: &[u8]) {
    let mut parts = bytes.splitn(3, |&b| b == 0);
    let (pattern, replacement, url) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    if pattern.len() > 32 || url.len() > 64 {
        return;
    }
    let mut rules = swoole_rs_http::RewriteRules::default();
    rules.add(&[b"~", pattern, b"~"].concat(), replacement);
    let rewritten = rules.apply(url);
    let literal = pattern
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'/');
    if pattern.is_empty() || !literal || replacement.contains(&b'$') {
        return;
    }
    let (mut expected, mut rest) = (Vec::new(), url);
    while let Some(at) = rest.windows(pattern.len()).position(|w| w == pattern) {
        expected.extend_from_slice(&rest[..at]);
        expected.extend_from_slice(replacement);
        rest = &rest[at + pattern.len()..];
    }
    expected.extend_from_slice(rest);
    assert_eq!(
        rewritten,
        Some(expected).filter(|expected| expected != url),
        "~{pattern:?}~ -> {replacement:?} on {url:?}"
    );
}

/// A permessage-deflate payload (RFC 7692): any bytes into
/// `swoole_rs_inflate::message` return, and the first byte as a level (-1 to 9) with
/// the rest as a message round-trips through `swoole_rs_deflate::message`.
pub fn ws_inflate(bytes: &[u8]) {
    let _ = swoole_rs_inflate::message(bytes);
    let Some((&level, data)) = bytes.split_first() else {
        return;
    };
    let level = c_int::from(level % 11) - 1;
    let compressed = swoole_rs_deflate::message(data, level)
        .unwrap_or_else(|failure| panic!("deflate at {level}: {failure:?}"));
    assert_eq!(
        swoole_rs_inflate::message(&compressed).as_deref(),
        Ok(data),
        "level {level}"
    );
}

/// An HTTP response body: the first byte picks gzip, zlib or raw deflate (and the
/// HTTP/2 client's gzip-only decoder), the second how many of the next bytes are
/// chunk lengths; the rest, compressed, fed to `Inflate::decode` in those chunks
/// gives back what one call over the whole stream does: the rest. The rest as it
/// is, whole or in those chunks, decodes without a panic.
pub fn http_inflate(bytes: &[u8]) {
    let Some((&[mode, count], rest)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let (lengths, body) = rest.split_at(usize::from(count % 16).min(rest.len()));
    let (window_bits, gzip) = match mode % 3 {
        0 => (swoole_rs_deflate::GZIP, true),
        1 => (swoole_rs_deflate::DEFLATE, false),
        _ => (-swoole_rs_deflate::DEFLATE, false),
    };
    let decoder = || {
        if gzip && mode & 4 != 0 {
            Inflate::gzip_only()
        } else {
            Inflate::new(gzip)
        }
    };
    let _ = decoder().decode(body);
    let _ = in_chunks(decoder(), body, lengths);
    let level = c_int::from(mode >> 3) % 10;
    let stream = swoole_rs_deflate::compress(body, 2 * body.len() + 64, window_bits, level)
        .unwrap_or_else(|failure| panic!("compress {window_bits} at {level}: {failure:?}"));
    assert_eq!(decoder().decode(&stream), Some(body), "one call");
    // The raw retry is the first chunk's alone: a raw stream whose first two bytes
    // read as a zlib header, or whose first chunk is shorter, fails in pieces.
    if window_bits < 0
        && (lengths.first().is_some_and(|&n| n < 2)
            || u16::from_be_bytes([stream[0], stream[1]]) % 31 == 0 && stream[0] & 0xf == 8)
    {
        return;
    }
    assert_eq!(
        in_chunks(decoder(), &stream, lengths).as_deref(),
        Some(body),
        "chunks {lengths:?}"
    );
}

/// `stream` through `inflate` in chunks of `lengths` (0 as 1), then the rest in one:
/// what they produced, or None once one fails.
fn in_chunks(mut inflate: Inflate, mut stream: &[u8], lengths: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let lengths = lengths.iter().map(|&n| usize::from(n.max(1)));
    for length in lengths.chain([usize::MAX]) {
        if stream.is_empty() {
            break;
        }
        let (chunk, tail) = stream.split_at(length.min(stream.len()));
        out.extend_from_slice(inflate.decode(chunk)?);
        stream = tail;
    }
    Some(out)
}
