//! The parts of `native.cc` whose Rust side had no caller but that gate: the receive
//! loop against the pinned `redis::recv_packet` (`swoole_rs_oracle::resp_stream`), the
//! handler-key model, and RESP's reply formats, parse and PHP request walk against the
//! pinned `redis::format`, `redis::parse` and PHP request loop
//! (`swoole_rs_oracle::protocols`). Each C export the gate called is replaced by the
//! safe function that was its body (README.md maps them).
#![expect(
    clippy::disallowed_types,
    reason = "a modeled socket's reads; tools/budget.py doesn't count test code"
)]
use core::ffi::{c_char, c_int, c_long};
use std::collections::VecDeque;

use swoole_rs_corebase::{set_errno, sys::errno};
use swoole_rs_oracle::{protocols as pinned, resp_stream};
use swoole_rs_protocols::{
    Malformed, RespKind, RespPhpState, RespRequest, mqtt_frame_length, resp_format,
    resp_handler_key, resp_php_next, resp_spans, socks_auth, socks_connect,
};
use swoole_rs_protocols_ffi::{RESP_RECV_DONE, RespConnection, SW_ERR, recv_packet};

/// The oracle shim's socket, String and connection: each recv hands out at most the
/// next chunk, then would block. Its log is the shim's trace.
struct Model {
    chunks: VecDeque<Vec<u8>>,
    content: Vec<u8>,
    size: usize,
    offset: i64,
    request: RespRequest,
    log: Vec<u8>,
}

impl RespConnection for Model {
    fn recv(&mut self) -> isize {
        let Some(chunk) = self.chunks.front_mut() else {
            set_errno(libc::EAGAIN);
            return RESP_RECV_DONE;
        };
        let count = chunk.len().min(self.size - self.content.len());
        self.content.extend(chunk.drain(..count));
        if chunk.is_empty() {
            self.chunks.pop_front();
        }
        isize::try_from(count).unwrap_or(isize::MAX)
    }

    fn view(&mut self) -> (&[u8], usize, &mut RespRequest) {
        (&self.content, self.size, &mut self.request)
    }

    fn set_offset(&mut self, offset: i64) {
        self.offset = offset;
    }

    fn extend(&mut self, size: usize) {
        self.size = size;
    }

    fn dispatch(&mut self) -> c_int {
        let error = errno();
        self.log
            .extend_from_slice(format!("P{}:", self.content.len()).as_bytes());
        self.log.extend_from_slice(&self.content);
        self.log.extend_from_slice(format!("E{error}").as_bytes());
        0
    }

    fn clear(&mut self) {
        self.content.clear();
        self.offset = 0;
    }
}

/// The shim's driver over Rust's loop: `recv_packet` until it fails or the chunks are
/// drained, logged as `resp_stream::UPSTREAM_TRACE` logs the pinned one.
fn rust_trace(command: &[u8], split: usize, buffer_size: usize, maximum: u32) -> Vec<u8> {
    let (first, second) = command.split_at(split);
    let mut model = Model {
        chunks: [first, second]
            .into_iter()
            .filter(|chunk| !chunk.is_empty())
            .map(<[u8]>::to_vec)
            .collect(),
        content: Vec::new(),
        size: buffer_size,
        offset: 0,
        request: RespRequest::default(),
        log: Vec::new(),
    };
    for _ in 0..4096 {
        let drained = model.chunks.is_empty();
        set_errno(libc::EDOM);
        let result = recv_packet(&mut model, maximum, 4096);
        let error = errno();
        let (length, offset, r) = (model.content.len(), model.offset, model.request);
        let call = format!(
            "R{result}E{error}L{length}O{offset},{},{}/{},{}/{},{};",
            r.state, r.lines_received, r.lines_total, r.bytes_received, r.bytes_total, r.offset
        );
        model.log.extend_from_slice(call.as_bytes());
        if result == SW_ERR {
            break;
        }
        if drained {
            model
                .log
                .extend_from_slice(format!("L{length}O{offset}").as_bytes());
            break;
        }
    }
    model.log
}

/// The pinned `redis::recv_packet` over the same stream, chunks and buffer.
fn reference_trace(command: &[u8], split: usize, buffer_size: u32, maximum: u32) -> Vec<u8> {
    let chunks = [split, command.len() - split].map(|size| u16::try_from(size).unwrap_or(u16::MAX));
    let stream = resp_stream::Stream {
        data: command.as_ptr(),
        length: command.len(),
        chunks: chunks.as_ptr(),
        chunk_count: chunks.len(),
        buffer_size,
        max_length: maximum,
        close_at_end: 0,
    };
    let mut out = vec![0u8; 1 << 16];
    // SAFETY: the stream's pointers outlive the call; `out` has its capacity.
    let length =
        unsafe { resp_stream::UPSTREAM_TRACE(&raw const stream, out.as_mut_ptr(), out.len()) };
    assert!(length <= out.len(), "the log fits");
    out.truncate(length);
    out
}

/// `native.cc`'s receive states: every split of eight commands at three package
/// limits, into a 64-byte buffer. Each call's result, errno (at every package and on
/// return), packages, buffer length and offset, and request state must match.
#[test]
fn receive_states() {
    let mut receives = 0;
    let commands: [&[u8]; 8] = [
        b"*1\r\n$3\r\nGET\r\n",
        b"*2\r\n$3\r\nSET\r\n$3\r\nabc\r\n",
        b"*1\r\n$10\r\nabc\r\n",
        b":1\r\n",
        b"*1\r\n$-1\r\n",
        b"*1\r\n:9223372036854775808\r\n",
        b"*1\r\n$9223372036854775808x\r\n",
        b"*-9223372036854775809\r\n",
    ];
    for command in commands {
        for split in 2..=command.len() {
            for maximum in [12, 64, 1_048_576] {
                let expected = reference_trace(command, split, 64, maximum);
                let actual = rust_trace(command, split, 64, maximum);
                assert_eq!(
                    String::from_utf8_lossy(&expected),
                    String::from_utf8_lossy(&actual),
                    "split {split}, maximum {maximum}"
                );
                receives += 1;
            }
        }
    }
    assert_eq!(
        receives,
        3 * commands.iter().map(|c| c.len() - 1).sum::<usize>()
    );
}

/// `native.cc`'s `handler_key_trace`: C printf truncation plus an ASCII case model.
/// Zend's case folding and property behaviour are compared by handlers.py on real
/// PHP engines.
#[test]
fn handler_keys() {
    fn check(wire: &[u8]) {
        let mut expected = [0u8; 64];
        let terminated = [wire, &[0]].concat();
        // SAFETY: a 64-byte output and a NUL-terminated argument for `%s`.
        let needed = unsafe {
            libc::snprintf(
                expected.as_mut_ptr().cast(),
                expected.len(),
                c"_handler_%s".as_ptr(),
                terminated.as_ptr(),
            )
        };
        let length = usize::try_from(needed)
            .expect("snprintf succeeds")
            .min(expected.len() - 1);
        expected[..length].make_ascii_lowercase();
        let (actual, actual_length) = resp_handler_key(wire);
        assert_eq!(actual_length, length);
        assert_eq!(expected[..=length], actual[..=length]);
    }
    let mut cases = 0;
    for byte in 0..=255u8 {
        for length in [0, 1, 53, 54, 55, 63, 64, 65, 1024] {
            check(&vec![byte; length]);
            cases += 1;
        }
    }
    for offset in 0..128 {
        let mut wire = [b'A'; 128];
        wire[offset] = 0;
        check(&wire);
        cases += 1;
    }
    assert_eq!(cases, 256 * 9 + 128);
}

/// `wire` and the NUL the pinned parsers may read after it.
fn terminated(wire: &[u8]) -> Vec<c_char> {
    wire.iter()
        .chain(&[0])
        .map(|&byte| c_char::from_ne_bytes([byte]))
        .collect()
}

/// The `size` bytes of an oracle buffer, which is released.
///
/// # Safety
/// `output` is an oracle buffer of `size` bytes not yet released.
unsafe fn taken(output: *mut c_char, size: usize) -> Vec<u8> {
    // SAFETY: as the caller promises.
    unsafe {
        let bytes = std::slice::from_raw_parts(output.cast::<u8>(), size).to_vec();
        pinned::free(output);
        bytes
    }
}

/// The items of an oracle buffer: a u32 length before each, after a u32 kind when
/// `kinds`.
fn items(mut bytes: &[u8], kinds: bool) -> Vec<(u32, Vec<u8>)> {
    let word = |bytes: &mut &[u8]| {
        let (word, rest) = bytes.split_first_chunk::<4>()?;
        *bytes = rest;
        Some(u32::from_ne_bytes(*word))
    };
    let mut items = Vec::new();
    while !bytes.is_empty() {
        let kind = if kinds { word(&mut bytes) } else { Some(0) };
        let length = word(&mut bytes).map(|length| length as usize);
        let item = length.and_then(|length| bytes.split_at_checked(length));
        let (Some(kind), Some((item, rest))) = (kind, item) else {
            break;
        };
        items.push((kind, item.to_vec()));
        bytes = rest;
    }
    items
}

/// The pinned `redis::parse` from errno `initial`: its strings and errno after.
fn pinned_parse(wire: &[u8], initial: c_int) -> (Vec<Vec<u8>>, c_int) {
    let buffer = terminated(wire);
    let (mut size, mut error) = (0, 0);
    // SAFETY: the buffer holds the wire and a NUL; the result is the oracle's buffer.
    let bytes = unsafe {
        let output = pinned::resp_parse(
            buffer.as_ptr(),
            wire.len(),
            initial,
            &raw mut size,
            &raw mut error,
        );
        taken(output, size)
    };
    let strings = items(&bytes, false).into_iter().map(|(_, item)| item);
    (strings.collect(), error)
}

/// `redis_reply.inc`'s `redis::parse` over [`resp_spans`], from errno `initial`: an
/// integer span in decimal, any other span's bytes, nothing for a malformed wire, and
/// `ERANGE` when a number overflowed.
fn rust_parse(wire: &[u8], initial: c_int) -> (Vec<Vec<u8>>, c_int) {
    set_errno(initial);
    let mut range = false;
    let mut strings = Vec::new();
    let count = resp_spans(
        wire,
        |span| {
            strings.push(if span.kind == RespKind::Integer {
                span.integer.to_string().into_bytes()
            } else {
                let end = span.offset + span.length;
                wire.get(span.offset..end).unwrap_or_default().to_vec()
            });
        },
        &mut range,
    );
    if range {
        set_errno(libc::ERANGE);
    }
    if count.is_none() {
        strings.clear();
    }
    (strings, errno())
}

/// A PHP request: the command (with whether a string was taken as it) and each
/// parameter as 0 string, 1 integer in decimal or 2 null.
type Request = (Vec<u8>, bool, Vec<(u32, Vec<u8>)>);

/// The pinned PHP request loop from errno `initial`, with errno after.
fn pinned_request(wire: &[u8], initial: c_int) -> (Request, c_int) {
    let buffer = terminated(wire);
    let (mut size, mut error) = (0, 0);
    // SAFETY: the buffer holds the wire and a NUL; the result is the oracle's buffer.
    let bytes = unsafe {
        let output = pinned::resp_php_request(
            buffer.as_ptr(),
            wire.len(),
            initial,
            &raw mut size,
            &raw mut error,
        );
        taken(output, size)
    };
    let mut parameters = items(&bytes, true);
    let (seen, command) = parameters.remove(0);
    ((command, seen != 0, parameters), error)
}

/// `native.cc`'s `rust_php_request` over [`resp_php_next`], from errno `initial`:
/// `ERANGE` when any step's number overflowed. No request when the walk ends in an
/// error, which the gate asserted it doesn't.
fn rust_request(wire: &[u8], initial: c_int) -> (Option<Request>, c_int) {
    set_errno(initial);
    let mut state = RespPhpState::default();
    let (mut command, mut seen, mut parameters) = (Vec::new(), false, Vec::new());
    loop {
        let mut range = false;
        let event = resp_php_next(wire, &mut state, &mut range);
        if range {
            set_errno(libc::ERANGE);
        }
        let Ok(event) = event else {
            return (None, errno());
        };
        let Some(event) = event else {
            break;
        };
        let end = event.offset + event.length;
        let bytes = || wire.get(event.offset..end).unwrap_or_default().to_vec();
        match event.kind {
            RespKind::Command => (command, seen) = (bytes(), true),
            RespKind::String => parameters.push((0, bytes())),
            RespKind::Integer => parameters.push((1, event.integer.to_string().into_bytes())),
            RespKind::Null => parameters.push((2, Vec::new())),
        }
    }
    (Some((command, seen, parameters)), errno())
}

/// `redis_reply.inc`'s `append_reply` into an empty String: the exact size, then the
/// bytes; nothing when there is no reply of that kind.
fn rust_format(kind: u8, value: &[u8], integer: i64) -> Vec<u8> {
    let size = resp_format(kind, value, integer, None);
    let mut reply = vec![0; size];
    if size == 0 || resp_format(kind, value, integer, Some(&mut reply)) != size {
        reply.clear();
    }
    reply
}

/// `native.cc`'s reply formats: five values and five integers as each of the seven
/// reply types, against the pinned `redis::format` overloads.
#[test]
fn reply_formats() {
    let mut replies = 0;
    let long = vec![b'b'; 65536];
    let values: [&[u8]; 5] = [b"", b"OK", b"\0binary", b"a\0z", &long];
    for value in values {
        for kind in 0..=6u8 {
            let mut size = 0;
            // SAFETY: `value` is readable for its length; the result is the oracle's buffer.
            let expected = unsafe {
                let output = pinned::resp_format_string(
                    kind.into(),
                    value.as_ptr().cast(),
                    value.len(),
                    &raw mut size,
                );
                taken(output, size)
            };
            assert_eq!(expected, rust_format(kind, value, 0), "type {kind}");
            replies += 1;
        }
    }
    for integer in [c_long::MIN, -1, 0, 1, c_long::MAX] {
        for kind in 0..=6 {
            let mut size = 0;
            // SAFETY: the result is the oracle's buffer.
            let expected = unsafe {
                let output = pinned::resp_format_long(kind, integer, &raw mut size);
                taken(output, size)
            };
            // The integer overload ignores its reply type: the codec's kind 8.
            assert_eq!(expected, rust_format(8, &[], integer), "{integer}");
            replies += 1;
        }
    }
    assert_eq!(replies, 70);
}

/// `native.cc`'s parse cases against the pinned `redis::parse`: integers, numbers at
/// and past a signed long from each errno, argument counts either side of the gate's
/// sixteen-span buffer, permissive fallthrough, and bulk payloads.
#[test]
fn parse_replies() {
    let mut cases = 0;
    let same = |wire: &[u8]| {
        assert_eq!(
            pinned_parse(wire, 0).0,
            rust_parse(wire, 0).0,
            "{}",
            String::from_utf8_lossy(wire)
        );
    };
    for number in [
        "0",
        "-1",
        "+1",
        " 12",
        "\x0b42",
        "2147483648",
        "4294967295",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        same(format!(":{number}\r\n").as_bytes());
        cases += 1;
    }
    let digits = [
        "9223372036854775807".to_owned(),
        "9223372036854775808".to_owned(),
        "-9223372036854775808".to_owned(),
        "-9223372036854775809".to_owned(),
        "0009223372036854775807".to_owned(),
        "9".repeat(4096),
    ];
    for digits in &digits {
        for prefix in ['*', '$', ':'] {
            for suffix in ["\r\n", "x\r\n"] {
                let command = format!("{prefix}{digits}{suffix}");
                for initial in [0, libc::EDOM, libc::ERANGE] {
                    // The pinned parse dereferences nullptr after a malformed numeric
                    // delimiter; compare its defined get_number helper instead.
                    if suffix.starts_with('x') {
                        let line = terminated(command.as_bytes());
                        let mut before = 0;
                        // SAFETY: the line is NUL-terminated.
                        let found = unsafe {
                            pinned::resp_get_number(line.as_ptr(), initial, &raw mut before)
                        };
                        assert!(!found);
                        set_errno(initial);
                        let mut range = false;
                        assert_eq!(resp_spans(command.as_bytes(), |_| {}, &mut range), None);
                        if range {
                            set_errno(libc::ERANGE);
                        }
                        assert_eq!(before, errno());
                    } else {
                        assert_eq!(
                            pinned_parse(command.as_bytes(), initial),
                            rust_parse(command.as_bytes(), initial),
                            "{prefix}, from errno {initial}"
                        );
                    }
                    cases += 1;
                }
            }
        }
    }
    for arguments in [1, 16, 17, 64, 128] {
        let command = format!("*{arguments}\r\n{}", "$1\r\nx\r\n".repeat(arguments));
        assert_eq!(
            pinned_parse(command.as_bytes(), libc::EDOM),
            rust_parse(command.as_bytes(), libc::EDOM),
            "{arguments} arguments"
        );
        cases += 1;
    }
    let commands: [&[u8]; 7] = [
        b"+OK\r\n",
        b"abc",
        b"x",
        b":1\r\n$3\r\n",
        b"*1\r\n$3\r\nabc",
        b"$0\r\njunk",
        b"$-1\r\n:2\r\n",
    ];
    for command in commands {
        same(command);
        cases += 1;
    }
    for size in [0, 1, 125, 4096, 65536] {
        let mut payload = vec![b'x'; size];
        if size != 0 {
            payload[size / 2] = 0;
        }
        let command = [
            format!("*3\r\n$3\r\nSET\r\n$-1\r\n${size}\r\n").as_bytes(),
            &payload,
            b"\r\n",
        ]
        .concat();
        same(&command);
        cases += 1;
    }
    assert_eq!(cases, 9 + 108 + 5 + 7 + 5);
}

/// `native.cc`'s bounded failures, where the codec has them: a refused request or
/// reply leaves its output alone.
#[test]
fn bounded_failures_leave_output() {
    let untouched = [0xa5u8; 16];
    let mut output = untouched;
    assert_eq!(mqtt_frame_length(&[]), 0);
    assert_eq!(socks_auth(&[], &[], &mut output[..2]), -1);
    assert_eq!(output, untouched);
    assert_eq!(socks_connect(1, &untouched[..3], 80, &mut output), -1);
    assert_eq!(output, untouched);
    assert_eq!(resp_format(8, &[], i64::MIN, Some(&mut output[..1])), 0);
    assert_eq!(output, untouched);
}

/// `native.cc`'s `seeded_resp_trace`: 10,000 generated wires, each through the pinned
/// `redis::parse` and PHP request loop and through Rust's spans and PHP events, from
/// errno `EDOM`; strings, parameters and errno must match. The grammar keeps every
/// native bulk copy in bounds: malformed numeric delimiters and negative non-null
/// bulk sizes are excluded, because the pinned parsers read outside the input there.
#[test]
fn seeded_requests() {
    let mut fingerprint = 14_695_981_039_346_656_037_u64;
    let mut random = 0x7377_7213_4263_2026_u64;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    let whitespace: [&[u8]; 9] = [
        b"",
        b" ",
        b"\t",
        b"\n",
        b"\x0b",
        b"\x0c",
        b"\r",
        b"\r\n",
        b" \t\x0b\x0c\r\n",
    ];
    let numbers = [
        "0".to_owned(),
        "+0".to_owned(),
        "-0".to_owned(),
        "1".to_owned(),
        "+17".to_owned(),
        "00012".to_owned(),
        "2147483647".to_owned(),
        "2147483648".to_owned(),
        "4294967295".to_owned(),
        "9223372036854775807".to_owned(),
        "9223372036854775808".to_owned(),
        "-9223372036854775808".to_owned(),
        "-9223372036854775809".to_owned(),
        "9".repeat(128),
        format!("-{}", "9".repeat(128)),
    ];
    // An index below `count`, from the next random number.
    let mut pick = |count: usize| usize::try_from(next() % count as u64).expect("an index");
    let mut wires = 0;
    for trial in 0..10_000 {
        let mut wire = vec![b'*'];
        let numeric = |wire: &mut Vec<u8>, pick: &mut dyn FnMut(usize) -> usize| {
            wire.extend_from_slice(whitespace[pick(whitespace.len())]);
            wire.extend_from_slice(numbers[pick(numbers.len())].as_bytes());
        };
        numeric(&mut wire, &mut pick);
        wire.extend_from_slice(b"\r\n");
        for _ in 0..pick(33) {
            match pick(4) {
                0 => {
                    wire.push(b'$');
                    wire.extend_from_slice(whitespace[pick(whitespace.len())]);
                    wire.extend_from_slice(b"-1\r\n");
                }
                1 => {
                    wire.push(b':');
                    numeric(&mut wire, &mut pick);
                    wire.extend_from_slice(b"\r\n");
                }
                _ => {
                    let length = pick(65);
                    let digits = match (pick(4), length) {
                        (0, _) => format!("+{length}"),
                        (1, _) => format!("000{length}"),
                        (2, 0) => "-000".to_owned(),
                        _ => length.to_string(),
                    };
                    wire.push(b'$');
                    wire.extend_from_slice(whitespace[pick(whitespace.len())]);
                    wire.extend_from_slice(digits.as_bytes());
                    wire.extend_from_slice(b"\r\n");
                    // The payload, then two bytes the legacy parse skips without
                    // validating CRLF.
                    wire.extend((0..length + 2).map(|_| pick(256).to_le_bytes()[0]));
                }
            }
        }
        for byte in (wire.len() as u64).to_le_bytes().iter().chain(&wire) {
            fingerprint = (fingerprint ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211);
        }
        assert_eq!(
            pinned_parse(&wire, libc::EDOM),
            rust_parse(&wire, libc::EDOM),
            "defined RESP mismatch trial={trial}"
        );
        let (expected, error) = pinned_request(&wire, libc::EDOM);
        assert_eq!(
            (Some(expected), error),
            rust_request(&wire, libc::EDOM),
            "PHP request mismatch trial={trial}"
        );
        wires += 1;
    }
    assert_eq!(wires, 10_000);
    // The gate's `resp_corpus_fnv1a64`: the same wires as `native.cc` generated.
    assert_eq!(fingerprint, 0xc51a_e5cf_4cf3_3d44);
}

/// A malformed request leaves the walk's state where it was and still reports the
/// overflow before it.
#[test]
fn php_next_leaves_state_on_error() {
    let mut state = RespPhpState::default();
    let mut range = false;
    let event = resp_php_next(b"$9223372036854775808x\r\n", &mut state, &mut range);
    assert_eq!(event.err(), Some(Malformed));
    assert_eq!((range, state.cursor.position), (true, 0));
}

/// A scan emits the spans before a malformed one, and reports its overflow.
#[test]
fn spans_before_an_error_and_its_range() {
    let wire = b":1\r\n:2\r\n:3\r\n";
    let (mut integers, mut range) = (Vec::new(), false);
    let count = resp_spans(wire, |span| integers.push(span.integer), &mut range);
    assert_eq!((count, range), (Some(3), false));
    assert_eq!(integers, [1, 2, 3]);
    let wire = b":1\r\n:99999999999999999999999x\r\n";
    let (mut integers, mut range) = (Vec::new(), false);
    let count = resp_spans(wire, |span| integers.push(span.integer), &mut range);
    assert_eq!((count, integers[0], range), (None, 1, true));
}

/// `core-tests/src/protocol/redis.cpp`, `TEST(redis, format)`: `REPLY_STATUS` (2)
/// and `REPLY_ERROR` (0) of an empty value.
#[test]
fn format_empty_status_and_error() {
    assert_eq!(rust_format(2, b"", 0), b"+OK\r\n");
    assert_eq!(rust_format(0, b"", 0), b"-ERR\r\n");
}

/// `TEST(redis, parse)`: `redis::parse(":3\r\n")[0] == "3"`, an integer span in
/// decimal.
#[test]
fn parse_integer_reply() {
    let (mut spans, mut range) = (Vec::new(), false);
    let count = resp_spans(b":3\r\n", |span| spans.push(span), &mut range);
    assert_eq!((count, range), (Some(1), false));
    assert_eq!(spans[0].kind, RespKind::Integer);
    assert_eq!(spans[0].integer.to_string(), "3");
}
