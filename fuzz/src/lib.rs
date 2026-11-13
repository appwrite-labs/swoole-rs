//! Differential fuzz targets: the same bytes go to a Rust entry point and to
//! the pinned upstream C++ in `swoole-rs-oracle`; return values and every output
//! byte, red zones included, must match.

use std::ffi::{CStr, c_char};
use swoole_rs_clients::{cookie, domain_decode};
use swoole_rs_framing::{DefaultLength, default_length, unpack, width};
use swoole_rs_http::{
    BOUNDARY_SIZE, HttpStaticContext, HttpStaticObservation, StaticEffects,
};
use swoole_rs_oracle::{
    base64 as upstream, clients, framing as upstream_framing, framing_stream,
    http as upstream_http,
    http_frame::{self, Engine, UPSTREAM},
    http_range, protocols, resp_stream, socks5, websocket as upstream_ws,
};

mod framing_native;
mod protocols_native;

mod network;
pub use network::{
    dns_reply, http_chunked, http_inflate, http_rewrite, http2_frame, http2_hpack, multipart,
    resp_server, ws_inflate,
};
use swoole_rs_websocket::{decode_in_place, frame_length, mask};

const RED_ZONE: usize = 17;

/// `swoole_rs_http::Request` behind the oracle's request framing entry points.
const CANDIDATE: Engine = Engine {
    frame: request::frame,
    expect: request::expect,
    header: request::header,
    method: swoole_rs_http_ffi::http_server::get_method_string,
    header_info: request::header_info,
};

mod request {
    use std::cell::Cell;
    use std::ffi::{CStr, c_char, c_void};
    use swoole_rs_http_ffi::{HttpRequest as Request, HttpString, NativeOp};
    use swoole_rs_http::ChunkState;

    thread_local! {
        /// The errno native code would leave after `strtoull`.
        static ERRNO: Cell<u32> = const { Cell::new(0) };
    }

    /// The request's effects (crates/http's `effects`), for the one framing has.
    unsafe extern "C" fn native(op: NativeOp, _: *mut c_void, _: *const u8, n: u64, _: u64) -> u64 {
        // glibc's strtoull leaves errno alone without digits; Darwin's sets EINVAL.
        const NO_DIGITS: u32 = if cfg!(target_os = "macos") { 22 } else { 0 };
        if op == NativeOp::NumberErrors {
            if n & 1 != 0 {
                ERRNO.set(34);
            }
            if n & 2 != 0 && NO_DIGITS != 0 {
                ERRNO.set(NO_DIGITS);
            }
        }
        0
    }

    /// A received `swoole::String` over `length` bytes and their spare NUL.
    fn on(data: *mut c_char, length: usize, offset: usize) -> HttpString {
        HttpString {
            length,
            size: length + 1,
            offset: offset as i64,
            str: data.cast(),
        }
    }

    pub unsafe extern "C" fn frame(
        operation: u32,
        data: *mut c_char,
        length: usize,
        state: *mut http_frame::State,
    ) -> i32 {
        // SAFETY: the caller's state.
        let s = unsafe { &mut *state };
        let mut r = Request::new(Some(native));
        let mut buffer = on(data, length, s.offset);
        (r.method, r.version) = (s.method as u8, s.version as u8);
        (r.excepted, r.known_length) = (s.excepted != 0, s.known_length != 0);
        (r.url_offset, r.url_length) = (s.url_offset, s.url_length);
        (r.request_line_length, r.header_length) = (s.line_length, s.header_length);
        r.content_length = s.content_length;
        (r.max_length, r.too_large) = (s.max_length, s.too_large != 0);
        r.chunk_state = match s.chunk_state {
            1 => ChunkState::Data,
            2 => ChunkState::Trailer,
            _ => ChunkState::Size,
        };
        (r.chunk_length, r.chunk_offset, r.chunk_scan_offset) =
            (s.chunk_length, s.chunk_offset, s.chunk_scan_offset);
        r.buffer = &raw mut buffer;
        let result = match operation {
            0 => r.get_protocol(),
            1 => r.get_header_length(),
            _ => r.get_chunked_body_length(),
        };
        *s = http_frame::State {
            offset: buffer.offset as usize,
            content_length: r.content_length,
            method: r.method.into(),
            version: r.version.into(),
            excepted: r.excepted.into(),
            known_length: r.known_length.into(),
            url_offset: r.url_offset,
            url_length: r.url_length,
            line_length: r.request_line_length,
            header_length: r.header_length,
            max_length: r.max_length,
            too_large: r.too_large.into(),
            chunk_state: r.chunk_state as u32,
            chunk_length: r.chunk_length,
            chunk_offset: r.chunk_offset,
            chunk_scan_offset: r.chunk_scan_offset,
        };
        result
    }

    pub unsafe extern "C" fn header_info(
        data: *mut c_char,
        length: usize,
        line_length: u32,
        header_length: u32,
        out: *mut http_frame::HeaderInfo,
    ) {
        let mut r = Request::new(Some(native));
        let mut buffer = on(data, length, 0);
        (r.request_line_length, r.header_length) = (line_length, header_length);
        r.buffer = &raw mut buffer;
        ERRNO.set(0);
        r.parse_header_info();
        let (offset, boundary) = r.form.as_deref().map_or((-1, 0), |form| {
            let boundary = form.boundary();
            (boundary.offset as i64, boundary.length as i64)
        });
        // SAFETY: the caller's output.
        unsafe {
            *out = http_frame::HeaderInfo {
                content_length: r.content_length,
                flags: u32::from(r.known_length)
                    | u32::from(r.keep_alive) << 1
                    | u32::from(r.chunked) << 2
                    | u32::from(r.header_parsed) << 4
                    | u32::from(r.excepted) << 5,
                error: ERRNO.get(),
                boundary_offset: offset,
                boundary_length: boundary,
            }
        };
        r.form = None;
    }

    pub unsafe extern "C" fn expect(data: *mut c_char, length: usize) -> bool {
        let mut r = Request::new(Some(native));
        let mut buffer = on(data, length, 0);
        r.buffer = &raw mut buffer;
        r.has_expect_header()
    }

    pub unsafe extern "C" fn header(
        data: *mut c_char,
        length: usize,
        url_offset: u32,
        url_length: u32,
        header_length: u32,
        name: *const c_char,
        output: *mut c_char,
    ) -> usize {
        let mut r = Request::new(Some(native));
        let mut buffer = on(data, length, 0);
        (r.url_offset, r.url_length, r.header_length) = (url_offset, url_length, header_length);
        r.buffer = &raw mut buffer;
        // SAFETY: a NUL-terminated name.
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        let Some(value) = r.get_header(name) else {
            return 0;
        };
        // SAFETY: the value is inside the buffer; output holds `length` bytes.
        unsafe { std::ptr::copy_nonoverlapping(data.add(value.offset), output, value.length) };
        value.length
    }

    use swoole_rs_oracle::http_frame;
}

pub fn base64(bytes: &[u8], encode: bool) {
    // Zero-length encode is undefined upstream.
    if encode && bytes.is_empty() {
        return;
    }
    let capacity = if encode {
        bytes.len().div_ceil(3) * 4 + 1
    } else {
        bytes.len() + 1
    };
    let mut expected = vec![0xa5; capacity + 2 * RED_ZONE];
    let mut actual = expected.clone();
    let e = expected.as_mut_ptr().wrapping_add(RED_ZONE);
    let a = &mut actual[RED_ZONE..RED_ZONE + capacity];
    // SAFETY: `expected` has `capacity` writable bytes after the red zone.
    let (want, got) = if encode {
        (
            unsafe { upstream::encode(bytes.as_ptr(), bytes.len(), e) },
            swoole_rs_base64::encode(bytes, a),
        )
    } else {
        (
            unsafe { upstream::decode(bytes.as_ptr(), bytes.len(), e) },
            swoole_rs_base64::decode(bytes, a),
        )
    };
    assert_eq!(got, want, "return for encode={encode} input={bytes:?}");
    assert!(
        actual == expected,
        "output for encode={encode} input={bytes:?}"
    );
}

/// Any bytes as the start of a frame: the same total and header length.
pub fn ws_frame_length(bytes: &[u8]) {
    let mut want_header = 0;
    // SAFETY: reads `bytes` and writes one local u32.
    let want = unsafe { upstream_ws::frame_length(bytes.as_ptr(), bytes.len(), &mut want_header) };
    let (got, got_header) = frame_length(bytes);
    let got_header = got_header.unwrap_or(0);
    // Some lengths are outside the compared domain; Rust reports -1 there.
    assert!(
        want as i64 == got || got == -1 && want < 0,
        "total {want} vs {got} for {bytes:?}"
    );
    assert_eq!(want_header, got_header, "header length for {bytes:?}");
}

/// Any bytes as one frame: the same verdict, fields and in-place unmasking.
pub fn ws_decode(bytes: &[u8]) {
    // Upstream reads the first two bytes unconditionally; servers only decode
    // frames whose length check has passed.
    if bytes.len() < 2 {
        return;
    }
    let (mut theirs, mut ours) = (bytes.to_vec(), bytes.to_vec());
    let mut want = upstream_ws::Frame::default();
    // SAFETY: upstream owns its copy of the frame and its output.
    let ok = unsafe { upstream_ws::decode(theirs.as_mut_ptr(), theirs.len(), &mut want) };
    let (total, got) = decode_in_place(&mut ours);
    assert_eq!(
        ok,
        total > 0 && bytes.len() as i64 >= total,
        "verdict ({total}) for {bytes:?}"
    );
    if ok {
        let got = upstream_ws::Frame {
            payload_length: got.payload_length as u64,
            header_length: got.header_length as u16,
            opcode: got.opcode,
            flags: got.flags,
            length_code: got.length_code,
            mask_key: got.mask_key,
        };
        assert_eq!(want, got, "frame for {bytes:?}");
    }
    assert!(theirs == ours, "payload bytes for {bytes:?}");
}

/// The first four bytes as a key: the same masking, and masking twice restores the data.
pub fn ws_mask(bytes: &[u8]) {
    let Some((key, data)) = bytes.split_first_chunk::<4>() else {
        return;
    };
    let (mut theirs, mut ours) = (data.to_vec(), data.to_vec());
    // SAFETY: upstream masks its own copy with the four-byte key.
    unsafe { upstream_ws::mask(theirs.as_mut_ptr(), theirs.len(), key.as_ptr()) };
    mask(&mut ours, *key);
    assert_eq!(theirs, ours, "mask for {bytes:?}");
    mask(&mut ours, *key);
    assert_eq!(ours, data, "unmask for {bytes:?}");
}

/// Any bytes as a URL component: the same decoded bytes, terminator and length.
pub fn http_url_decode(bytes: &[u8]) {
    let mut theirs = [bytes, &[0xa5]].concat();
    let mut ours = theirs.clone();
    // SAFETY: `theirs` owns `length + 1` bytes.
    let want = unsafe { upstream_http::url_decode(theirs.as_mut_ptr(), bytes.len()) };
    let written = swoole_rs_http::url_decode(&mut ours[..bytes.len()]);
    // Upstream terminates what it decoded; the Rust decoder leaves that to its caller.
    ours[written] = 0;
    assert_eq!(written, want, "length for {bytes:?}");
    assert!(theirs == ours, "decoded bytes for {bytes:?}");
}

/// Any bytes: the same percent-encoding, terminated.
pub fn http_url_encode(bytes: &[u8]) {
    let mut theirs = vec![0xa5; 3 * bytes.len() + 1];
    // SAFETY: `theirs` has upstream's capacity.
    let want =
        unsafe { upstream_http::url_encode(bytes.as_ptr(), bytes.len(), theirs.as_mut_ptr()) };
    let capacity = swoole_rs_http::url_capacity(bytes).unwrap();
    let mut ours = vec![std::mem::MaybeUninit::new(0xa5); capacity];
    let written = swoole_rs_http::url_encode(bytes, &mut ours).unwrap();
    // SAFETY: every slot started initialized.
    let ours: Vec<u8> = ours.iter().map(|slot| unsafe { slot.assume_init() }).collect();
    assert_eq!((written, capacity), (want, want + 1), "length for {bytes:?}");
    assert_eq!(ours, theirs[..=want], "encoding for {bytes:?}");
}

/// Any bytes as a Cookie header: the same keys and values, in order.
pub fn http_cookies(bytes: &[u8]) {
    let mut theirs = vec![0; 8 * bytes.len() + 16];
    // SAFETY: `theirs` has the documented capacity.
    let size = unsafe {
        upstream_http::cookies(
            bytes.as_ptr(),
            bytes.len(),
            theirs.as_mut_ptr(),
            false,
            std::ptr::null_mut(),
        )
    };
    theirs.truncate(size);
    // Like parse_cookie, tokenize a NUL-terminated copy in place.
    let mut buffer = [bytes, &[0]].concat();
    let (mut cursor, mut ours) = (0, Vec::new());
    loop {
        let cookie = match swoole_rs_http::cookie_next(&mut buffer, &mut cursor) {
            Ok(Some(cookie)) => cookie,
            Ok(None) => break,
            Err(status) => panic!("status {status} for {bytes:?}"),
        };
        for (offset, length) in [
            (cookie.key_offset, cookie.key_length),
            (cookie.value_offset, cookie.value_length),
        ] {
            ours.extend_from_slice(&(length as u32).to_ne_bytes());
            ours.extend_from_slice(&buffer[offset..offset + length]);
        }
    }
    assert_eq!(ours, theirs, "cookies for {bytes:?}");
}

/// Length types a server accepts for `package_length_type`.
const FORMATS: &[u8] = b"cCsSnvIiLlNVqQJP";

/// The first byte picks a length type, the rest is the field: the same value.
pub fn framing_unpack(bytes: &[u8]) {
    let Some((&pick, data)) = bytes.split_first() else {
        return;
    };
    let format = FORMATS[usize::from(pick) % FORMATS.len()];
    if data.len() < width(format) {
        return;
    }
    // SAFETY: upstream reads the format's width, which `data` covers.
    let want = unsafe { upstream_framing::unpack(format, data.as_ptr()) };
    assert_eq!(
        Ok(want),
        unpack(format, data),
        "unpack {} of {data:?}",
        format as char
    );
}

/// A length type, field offset and body offset, then any received bytes: the same
/// total length, "need more" header length, or error. Field size is the type's
/// width, as the server configures it.
pub fn framing_length(bytes: &[u8]) {
    let Some((&[pick, offset_low, offset_high, body_low, body_high], data)) =
        bytes.split_first_chunk::<5>()
    else {
        return;
    };
    let format = FORMATS[usize::from(pick) % FORMATS.len()];
    let size = width(format) as u8;
    let (offset, body_offset) = (
        u16::from_le_bytes([offset_low, offset_high]) % 64,
        u16::from_le_bytes([body_low, body_high]),
    );
    let mut header = 0;
    // SAFETY: both read at most `data.len()` bytes; outputs are locals.
    let want = unsafe {
        upstream_framing::length(
            format,
            size,
            offset,
            body_offset,
            data.as_ptr(),
            data.len() as u32,
            &mut header,
        )
    };
    match default_length(format, data, offset.into(), size.into(), body_offset.into()) {
        DefaultLength::Need(got) => {
            assert_eq!((want, header as i64), (0, got), "need more for {bytes:?}");
        }
        DefaultLength::Total(got) => assert_eq!(want as i64, got, "total for {bytes:?}"),
        // Outside the compared domain only an error must match.
        status => assert!(want < 0, "error {status:?} vs {want} for {bytes:?}"),
    }
}

/// `data` and a NUL, as both engines read it: get_header may look one byte past
/// the header end, and strtoul stops at the NUL.
fn terminated(data: &[u8]) -> Vec<c_char> {
    data.iter()
        .chain(&[0])
        .map(|&byte| byte as c_char)
        .collect()
}

fn frame(
    engine: &Engine,
    operation: u32,
    data: &[u8],
    state: http_frame::State,
) -> (i32, http_frame::State) {
    let (mut buffer, mut state) = (terminated(data), state);
    // SAFETY: the buffer holds data.len() bytes and a NUL; state is a local.
    let status = unsafe { (engine.frame)(operation, buffer.as_mut_ptr(), data.len(), &mut state) };
    (status, state)
}

/// get_protocol, get_header_length or get_chunked_body_length on both engines
/// from the same offset, header length and request state; a chunked body under a
/// package_max_length of half the input or none.
pub fn http_frame(bytes: &[u8]) {
    let Some((&[control, offset_low, offset_high, header_low, header_high], data)) =
        bytes.split_first_chunk::<5>()
    else {
        return;
    };
    let operation = u32::from(control % 3);
    let bound = |low, high| u16::from_le_bytes([low, high]) as usize % (data.len() + 1);
    let mut state = http_frame::State {
        offset: bound(offset_low, offset_high),
        header_length: bound(header_low, header_high) as u32,
        max_length: if control & 0x40 != 0 {
            (data.len() / 2) as u32
        } else {
            u32::MAX
        },
        ..Default::default()
    };
    if control & 0x80 != 0 {
        state = http_frame::State {
            method: 19,
            version: 3,
            known_length: 1,
            content_length: 123,
            url_offset: 3,
            url_length: 7,
            line_length: 9,
            ..state
        };
    }
    let got = frame(&CANDIDATE, operation, data, state);
    let want = frame(&UPSTREAM, operation, data, state);
    // Outside the compared domain the candidate must wait for more data.
    if operation == 0 && want.0 == 0 && want.1.offset > data.len() {
        assert_eq!(
            (got.0, got.1.offset),
            (-1, state.offset),
            "partial request line {bytes:?}"
        );
        return;
    }
    assert_eq!(got, want, "operation {operation} for {bytes:?}");
}

fn method(engine: &Engine, method: i32) -> Option<&'static CStr> {
    // SAFETY: the result is null or one of the engine's static strings.
    let string = unsafe { (engine.method)(method) };
    (!string.is_null()).then(|| unsafe { CStr::from_ptr(string) })
}

/// has_expect_header, get_header(name) and get_method_string on both engines.
pub fn http_header(bytes: &[u8]) {
    let Some((
        &[
            name_length,
            url_offset,
            url_length,
            header_low,
            header_high,
            code,
        ],
        rest,
    )) = bytes.split_first_chunk::<6>()
    else {
        return;
    };
    assert_eq!(
        method(&CANDIDATE, (code as i8).into()),
        method(&UPSTREAM, (code as i8).into()),
        "method {code}"
    );
    let (name, data) = rest.split_at(usize::from(name_length % 16).min(rest.len()));
    let name: Vec<c_char> = name
        .iter()
        .take_while(|&&byte| byte != 0)
        .chain(&[0])
        .map(|&byte| byte as c_char)
        .collect();
    let header_length = u16::from_le_bytes([header_low, header_high]) as usize % (data.len() + 1);
    let [want, got] = [&UPSTREAM, &CANDIDATE].map(|engine| {
        let (mut buffer, mut output) = (terminated(data), vec![0 as c_char; data.len()]);
        // SAFETY: the buffer holds data.len() bytes and a NUL; output has data.len() bytes.
        unsafe {
            let expect = (engine.expect)(buffer.as_mut_ptr(), data.len());
            let size = (engine.header)(
                buffer.as_mut_ptr(),
                data.len(),
                url_offset.into(),
                url_length.into(),
                header_length as u32,
                name.as_ptr(),
                output.as_mut_ptr(),
            );
            output.truncate(size);
            (expect, output)
        }
    });
    assert_eq!(got, want, "expect/header for {bytes:?}");
}

/// Upstream `mqtt::get_package_length` and `mqtt_frame_length` over the same received
/// bytes. With nothing received only Rust runs.
pub fn mqtt_length(bytes: &[u8]) {
    let data: Vec<c_char> = bytes.iter().map(|&byte| byte as c_char).collect();
    let size = data.len().min(u32::MAX as usize) as u32;
    let got = swoole_rs_protocols::mqtt_frame_length(&bytes[..size as usize]);
    if size == 0 {
        return;
    }
    // SAFETY: it reads at most `size` bytes of `data`.
    let want = unsafe { protocols::mqtt_length(data.as_ptr(), size) };
    assert_eq!(got, want, "mqtt length for {bytes:?}");
}

/// Walks upstream `redis::parse` over `data` (and the NUL after it) without
/// reading it: false for inputs outside the compared domain, including input
/// `recv_packet` never passes it. Those inputs are not compared.
fn resp_parse_in_bounds(data: &[u8]) -> bool {
    let at = |index: usize| data.get(index).copied().unwrap_or(0);
    // get_number: strtol after the type byte, then CRLF; Some((ret, next)).
    let number = |p: usize| {
        let start = p + 1;
        let mut q = start;
        while matches!(at(q), b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
            q += 1;
        }
        let negative = at(q) == b'-';
        if matches!(at(q), b'+' | b'-') {
            q += 1;
        }
        let digits = q;
        let mut value: i128 = 0;
        while at(q).is_ascii_digit() {
            value = (value * 10 + i128::from(at(q) - b'0')).min(1 << 70);
            q += 1;
        }
        let end = if q == digits { start } else { q };
        let value = if negative { -value } else { value };
        // strtol clamps to long; the int conversion keeps the low 32 bits.
        let value = value.clamp(i64::MIN.into(), i64::MAX.into()) as i64 as i32;
        (at(end) == b'\r' && at(end + 1) == b'\n').then_some((value, end + 2))
    };
    // One pass is one run of upstream's switch: `case` is where it enters, and a
    // case that doesn't break falls through to the next.
    let (mut state, mut p, mut length) = (0, 0usize, 0i32);
    loop {
        let mut case = state;
        if case == 0 {
            if at(p) == b'*' {
                let Some((_, next)) = number(p) else {
                    return false;
                };
                (p, state) = (next, 1);
            } else {
                case = 1;
            }
        }
        if case == 1 {
            if at(p) == b'$' {
                let Some((ret, next)) = number(p) else {
                    return false;
                };
                p = next;
                if ret != -1 {
                    (length, state) = (ret, 2);
                }
            } else if at(p) == b':' {
                let Some((_, next)) = number(p) else {
                    return false;
                };
                p = next;
            } else {
                return false;
            }
        }
        if case == 2 {
            if length < 0 || p + length as usize > data.len() {
                return false;
            }
            (p, state) = (p + length as usize + 2, 1);
        }
        if p >= data.len() {
            return true;
        }
    }
}

/// Upstream's strings (each after its u32 length) and, on glibc, whether errno is
/// ERANGE. macOS strtol also sets EINVAL when it converts nothing, which can overwrite
/// an earlier ERANGE; glibc leaves errno alone there, and callers only test ERANGE.
fn resp_parse_upstream(data: &[u8]) -> (Vec<u8>, bool) {
    let buffer = terminated(data);
    let (mut size, mut error) = (0, 0);
    // SAFETY: the buffer holds data.len() bytes and a NUL; the result is malloc'd.
    unsafe {
        let output = protocols::resp_parse(buffer.as_ptr(), data.len(), 0, &mut size, &mut error);
        let items = std::slice::from_raw_parts(output as *const u8, size).to_vec();
        protocols::free(output);
        (items, cfg!(target_os = "linux") && error == 34)
    }
}

/// Upstream `redis::parse` and `redis::parse` over Rust's spans, over the same bytes:
/// every string, in order, and ERANGE. Rust runs on every input.
pub fn resp_parse(bytes: &[u8]) {
    let (items, range) = protocols_native::resp_parse(bytes);
    let got = (items, cfg!(target_os = "linux") && range);
    if !resp_parse_in_bounds(bytes) {
        return;
    }
    assert_eq!(got, resp_parse_upstream(bytes), "resp parse for {bytes:?}");
}

/// Longest URL compared: libstdc++'s std::regex recurses once per character.
const URL_LIMIT: usize = 256;

fn client_url(data: &[u8]) -> (i32, i32, i32, Vec<u8>) {
    let (mut kind, mut port, mut host, mut host_length, mut logs) =
        (0, 0, vec![0; data.len()], 0, 0);
    // SAFETY: `data` is readable for its length; `host` has room for any substring of it.
    let status = unsafe {
        clients::url(
            data.as_ptr().cast(),
            data.len(),
            true,
            &mut kind,
            &mut port,
            host.as_mut_ptr().cast(),
            &mut host_length,
            &mut logs,
        )
    };
    host.truncate(host_length);
    // A rejected URL leaves the record's type unset.
    (status, if status == 1 { kind } else { 0 }, port, host)
}

/// Any bytes as a client URL: `Address::assign(url)` from the pinned address.cc and
/// `swoole_rs_clients::url` assign the same type, host and port, or both reject
/// (or both find the port past `int`, where `std::stoi` throws).
pub fn clients_url(bytes: &[u8]) {
    let got = match swoole_rs_clients::url(bytes) {
        Ok((kind, host, port)) => (1, kind, port, bytes[host].to_vec()),
        Err(swoole_rs_clients::URL_PORT_RANGE) => (-1, 0, 0, Vec::new()),
        Err(_) => (0, 0, 0, Vec::new()),
    };
    if bytes.len() > URL_LIMIT {
        return;
    }
    assert_eq!(got, client_url(bytes), "url {bytes:?}");
}

/// Any bytes as one Set-Cookie header: the same key, value offset and value.
pub fn clients_cookie(bytes: &[u8]) {
    let mut want = (0, 0, 0);
    // SAFETY: reads `bytes`; outputs are locals.
    unsafe {
        clients::cookie(
            bytes.as_ptr().cast(),
            bytes.len(),
            &mut want.0,
            &mut want.1,
            &mut want.2,
        );
    }
    let got = cookie(bytes);
    assert_eq!(
        (got.key_length, got.value_offset, got.value_length),
        want,
        "cookie {bytes:?}"
    );
}

/// Any bytes as a received DNS name buffer: where Rust decodes in bounds, upstream
/// decodes to the same bytes. Other inputs are outside the compared domain: Rust
/// reports 0 and upstream isn't run.
pub fn clients_domain_decode(bytes: &[u8]) {
    let (mut theirs, mut ours) = (bytes.to_vec(), bytes.to_vec());
    if !domain_decode(&mut ours, c_char::MIN != 0) {
        return;
    }
    // SAFETY: Rust decoded the same bytes in bounds, so upstream's identical walk does too.
    unsafe { clients::domain_decode(theirs.as_mut_ptr().cast()) };
    assert_eq!(theirs, ours, "domain {bytes:?}");
}

/// Any bytes up to the first blank line as a request head: `parse_header_info` on both
/// engines gives the same lengths, flags, errno and multipart boundary.
pub fn http_header_info(bytes: &[u8]) {
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return;
    };
    let line = bytes.windows(2).position(|w| w == b"\r\n").unwrap_or(0);
    let header_length = (end + 4) as u32;
    let info = |engine: &Engine| {
        let (mut buffer, mut info) = (terminated(bytes), http_frame::HeaderInfo::default());
        // SAFETY: the buffer holds bytes.len() bytes and a NUL; the head ends in a blank
        // line before header_length; info is a local.
        unsafe {
            (engine.header_info)(
                buffer.as_mut_ptr(),
                bytes.len(),
                line as u32,
                header_length,
                &mut info,
            )
        };
        info
    };
    let got = info(&CANDIDATE);
    // Outside the compared domain the candidate rejects the request.
    if multipart_nul(bytes, line + 2, end) {
        assert_eq!(got.flags & 32, 32, "excepted for {bytes:?}");
        return;
    }
    assert_eq!(got, info(&UPSTREAM), "header info for {bytes:?}");
}

/// Whether a multipart/form-data Content-Type line in the head, as parse_header_info
/// finds it, meets a NUL before its CR.
fn multipart_nul(bytes: &[u8], start: usize, end: usize) -> bool {
    let starts = |at: usize, prefix: &[u8]| {
        bytes.get(at..end).is_some_and(|rest| {
            rest.len() >= prefix.len() && rest[..prefix.len()].eq_ignore_ascii_case(prefix)
        })
    };
    (start.max(2)..end).any(|at| {
        if &bytes[at - 2..at] != b"\r\n" || !starts(at, b"Content-Type:") {
            return false;
        }
        let mut value = at + 13;
        while bytes.get(value) == Some(&b' ') {
            value += 1;
        }
        starts(value, b"multipart/form-data")
            && bytes[value..]
                .iter()
                .find(|&&byte| matches!(byte, b'\r' | 0))
                != Some(&b'\r')
    })
}

const RANGE_BOUNDARY: &[u8] = b"SwooleBoundary0123456789abcdefghijklmn\0";

/// Upstream's `parse_range` through its facade (`crates/oracle/shim/http_range.cc`).
fn range_upstream(
    range: &[c_char],
    filesize: u64,
    mime: &[c_char],
    flags: u8,
    capacity: usize,
) -> (http_range::Outcome, Vec<u8>) {
    let (mut outcome, mut tasks) = (
        http_range::Outcome::default(),
        vec![0; capacity * http_range::TASK_SIZE],
    );
    // SAFETY: range, mime and the boundary are NUL-terminated; tasks has room for
    // capacity tasks; outcome is a local.
    unsafe {
        http_range::UPSTREAM(
            range.as_ptr(),
            filesize,
            mime.as_ptr(),
            RANGE_BOUNDARY.as_ptr().cast(),
            i32::from(flags & 1),
            i32::from(flags & 2 != 0),
            &mut outcome,
            tasks.as_mut_ptr(),
            capacity,
        )
    };
    tasks.truncate(outcome.tasks.min(capacity) * http_range::TASK_SIZE);
    (outcome, tasks)
}

/// The facade's world: a file of `filesize` bytes with this MIME type, the boundary's
/// random characters and the If-Range verdict.
struct RangeFacade {
    filesize: usize,
    mime: Vec<u8>,
    modified: bool,
}
impl StaticEffects for RangeFacade {
    fn stat(&self, _: &[u8]) -> Option<HttpStaticObservation> {
        Some(HttpStaticObservation {
            filesize: self.filesize,
            ..HttpStaticObservation::default()
        })
    }
    fn mime(&self, _: &[u8]) -> Vec<u8> {
        self.mime.clone()
    }
    fn mime_exists(&self, _: &[u8]) -> bool {
        false
    }
    fn random(&self, out: &mut [u8; BOUNDARY_SIZE], count: usize) {
        let random = c_text(&RANGE_BOUNDARY[b"SwooleBoundary".len()..]);
        let count = count.min(random.len());
        out[..count].copy_from_slice(&random[..count]);
    }
    fn modified_range(&self, _: &[u8], _: i64) -> bool {
        self.modified
    }
    fn overflow(&self) {}
}

/// The selected `parse_range`, crates/http's static context, as the facade runs it.
fn range_candidate(
    range: &[u8],
    filesize: u64,
    mime: &[u8],
    flags: u8,
    capacity: usize,
) -> (http_range::Outcome, Vec<u8>) {
    let facade = RangeFacade {
        filesize: usize::try_from(filesize).unwrap_or(usize::MAX),
        mime: mime.to_vec(),
        modified: flags & 2 != 0,
    };
    let mut context = HttpStaticContext::new(b"/file", 4096).expect("a static context");
    context.stat(&facade);
    context.parse_range(Some(range), (flags & 1 != 0).then_some(b""), &facade);
    let facts = context.observe();
    let end = context.end_part();
    let mut outcome = http_range::Outcome {
        content_length: facts.content_length,
        status_code: facts.status,
        thrown: 0,
        tasks: facts.task_count,
        end_part_length: end.len(),
        ..http_range::Outcome::default()
    };
    let shown = end.len().min(outcome.end_part.len());
    outcome.end_part[..shown].copy_from_slice(&end[..shown]);
    let mut tasks = Vec::new();
    for task in context.tasks().iter().take(capacity) {
        tasks.extend_from_slice(&task.offset.cast_unsigned().to_ne_bytes());
        tasks.extend_from_slice(&(task.length as u64).to_ne_bytes());
        let header = c_text(&task.part_header);
        tasks.extend_from_slice(header);
        tasks.resize(tasks.len() + task.part_header.len() - header.len(), 0);
    }
    (outcome, tasks)
}

/// The text up to its first NUL, as a C string reads it.
fn c_text(text: &[u8]) -> &[u8] {
    text.split(|&byte| byte == 0).next().unwrap_or_default()
}

/// A flags byte (If-Range sent, file modified), a MIME type length, a file size and a
/// Range header: upstream's `StaticHandler::parse_range` and the selected one over
/// the Rust static context give the same status, content length, tasks, part
/// headers and closing boundary.
pub fn http_range(bytes: &[u8]) {
    let Some((&[flags, mime_length, size @ ..], range)) = bytes.split_first_chunk::<10>() else {
        return;
    };
    let filesize = u64::from_le_bytes(size);
    // `st_size` is an `off_t`: no file holds more than `i64::MAX` bytes, and the
    // selected handler keeps task offsets in that type.
    if i64::try_from(filesize).is_err() {
        return;
    }
    let mime: Vec<u8> = b"application/octet-stream"
        .iter()
        .cycle()
        .take(usize::from(mime_length))
        .copied()
        .collect();
    let range = c_text(range);
    let terminated = |text: &[u8]| -> Vec<c_char> {
        text.iter()
            .chain(&[0])
            .map(|&byte| byte as c_char)
            .collect()
    };
    let capacity = range.len() + 3;
    let want = range_upstream(
        &terminated(range),
        filesize,
        &terminated(&mime),
        flags,
        capacity,
    );
    let got = range_candidate(range, filesize, &mime, flags, capacity);
    assert_eq!(got, want, "range for {bytes:?}");
}

/// One engine's log for a stream case (see `framing_stream::Run`,
/// `resp_stream::Run` and `socks5::Run`).
fn stream_log<S>(
    run: unsafe extern "C" fn(*const S, *mut u8, usize) -> usize,
    stream: &S,
) -> Vec<u8> {
    let mut out = vec![0_u8; 1 << 16];
    // SAFETY: the case's pointers outlive the call; `out` has its capacity.
    let length = unsafe { run(stream, out.as_mut_ptr(), out.len()) };
    out.truncate(length.min(out.len()));
    out
}

/// The stream receive loops (`recv_with_length_protocol`, `recv_with_eof_protocol`
/// and `recv_split_by_eof`) from upstream and from the candidate (a step loop over
/// the Rust framing state machine), over the same stream in the same chunks. The EOF
/// loops must match every call's result, every packet and the final buffer; the
/// length loop, which reads greedily, every packet and whether the stream errors.
pub fn framing_stream(bytes: &[u8]) {
    let Some((
        &[
            mode,
            eof_len,
            flags,
            length_size,
            offset,
            body,
            max,
            chunk_count,
        ],
        rest,
    )) = bytes.split_first_chunk::<8>()
    else {
        return;
    };
    let mode = mode % 3;
    let eof_len = (eof_len % 4).max(1);
    let chunk_count = usize::from(chunk_count % 16);
    if rest.len() < usize::from(eof_len) + chunk_count {
        return;
    }
    let (eof, rest) = rest.split_at(usize::from(eof_len));
    let (sizes, data) = rest.split_at(chunk_count);
    // The configurations compared: chunk sizes 1..=64, length headers of 1..=4
    // bytes at offsets 0..8 with body offsets 1..=8, a 64-byte length buffer, and an
    // EOF buffer of the maximum length. A stream past the chunks waits (or closes).
    let chunks: Vec<u16> = sizes.iter().map(|&size| u16::from(size % 64) + 1).collect();
    let stream = framing_stream::Stream {
        mode,
        eof_len,
        close_at_end: flags & 1,
        length_size: length_size % 4 + 1,
        length_offset: u16::from(offset % 8),
        body_offset: u16::from(body % 8) + 1,
        // At least the EOF, and small enough that the limit is reached.
        max_length: u32::from(max % 64) + u32::from(eof_len) + 1,
        buffer_size: 64,
        eof: eof.as_ptr(),
        data: data.as_ptr(),
        length: data.len(),
        chunks: chunks.as_ptr(),
        chunk_count: chunks.len(),
    };
    // The candidate calls this crate's `swr_v1_framing_recv`: keep it linked.
    std::hint::black_box(framing_native::swr_v1_framing_recv as unsafe extern "C" fn(_, _, _) -> _);
    let want = stream_log(framing_stream::UPSTREAM, &stream);
    let got = stream_log(framing_stream::CANDIDATE, &stream);
    assert_eq!(
        String::from_utf8_lossy(&want),
        String::from_utf8_lossy(&got),
        "mode {mode} for {bytes:?}"
    );
}

/// `redis::recv_packet` from upstream and Rust's `recv_packet` (the loop over the RESP
/// receive state machine), over the same stream in the same chunks: every call's
/// result and request state, every package and the final buffer must match.
pub fn resp_stream(bytes: &[u8]) {
    let Some((&[flags, size, max, chunk_count], rest)) = bytes.split_first_chunk::<4>() else {
        return;
    };
    let chunk_count = usize::from(chunk_count % 16);
    let Some((sizes, data)) = rest.split_at_checked(chunk_count) else {
        return;
    };
    // Chunk sizes 1..=64, a starting buffer of 1..=64 bytes and a package limit of
    // 1..=256 bytes at least that size, so growth and the limit are both reached.
    let chunks: Vec<u16> = sizes.iter().map(|&size| u16::from(size % 64) + 1).collect();
    let buffer_size = u32::from(size % 64) + 1;
    let stream = resp_stream::Stream {
        data: data.as_ptr(),
        length: data.len(),
        chunks: chunks.as_ptr(),
        chunk_count: chunks.len(),
        buffer_size,
        max_length: buffer_size.max(u32::from(max) + 1),
        close_at_end: flags & 1,
    };
    let want = stream_log(resp_stream::UPSTREAM, &stream);
    let got = protocols_native::resp_stream(
        data,
        &chunks,
        buffer_size as usize,
        stream.max_length,
        flags & 1 != 0,
    );
    assert_eq!(
        String::from_utf8_lossy(&want),
        String::from_utf8_lossy(&got),
        "resp stream for {bytes:?}"
    );
}

/// `Socks5Proxy` from upstream and over the Rust SOCKS5 codec
/// (`protocols_native::Socks5Proxy`), from `create` through arbitrary server replies: every byte sent,
/// each handshake result and state, and each reply code's message must match.
pub fn socks5(bytes: &[u8]) {
    let Some((
        &[
            flags,
            user,
            password,
            host,
            host_extra,
            port_high,
            port_low,
            replies,
        ],
        rest,
    )) = bytes.split_first_chunk::<8>()
    else {
        return;
    };
    // Credentials of up to 255 bytes and hosts of up to 319, so both length limits
    // are crossed; socket types 1..=5 (5 is neither inet4 nor inet6).
    let Some((user, rest)) = rest.split_at_checked(usize::from(user)) else {
        return;
    };
    let Some((password, rest)) = rest.split_at_checked(usize::from(password)) else {
        return;
    };
    let host_length = usize::from(u16::from_be_bytes([host_extra, host]) % 320);
    let Some((host, rest)) = rest.split_at_checked(host_length) else {
        return;
    };
    let Some((sizes, replies)) = rest.split_at_checked(usize::from(replies % 8)) else {
        return;
    };
    // Replies of up to 15 bytes each, cut short where the input ends.
    let mut left = replies.len();
    let sizes: Vec<u16> = sizes
        .iter()
        .map(|&size| {
            let size = usize::from(size % 16).min(left);
            left -= size;
            size as u16
        })
        .collect();
    let case = socks5::Case {
        user: user.as_ptr(),
        user_length: user.len(),
        password: password.as_ptr(),
        password_length: password.len(),
        target_host: host.as_ptr(),
        target_host_length: host.len(),
        target_port: u16::from_be_bytes([port_high, port_low]),
        dns_tunnel: flags & 1,
        socket_type: (flags >> 1) % 5 + 1,
        send_short: (flags >> 4) & 1,
        replies: replies.as_ptr(),
        reply_sizes: sizes.as_ptr(),
        reply_count: sizes.len(),
    };
    let want = stream_log(socks5::UPSTREAM, &case);
    let got = protocols_native::socks5(&case, user, password, host, replies, &sizes);
    assert_eq!(
        String::from_utf8_lossy(&want),
        String::from_utf8_lossy(&got),
        "socks5 for {bytes:?}"
    );
}

/// `sni_wildcard_matches` over the two NUL-separated names in `bytes`, against a
/// glob over the lowercased names: the first `*`, if no `.` comes before it,
/// matches any run of bytes without a `.`.
pub fn sni_wildcard(bytes: &[u8]) {
    let mut names = bytes.split(|&b| b == 0);
    let (subject, cert) = (
        names.next().unwrap_or_default(),
        names.next().unwrap_or_default(),
    );
    assert_eq!(
        swoole_rs_utilities::sni_wildcard_matches(subject, cert),
        sni_glob(&subject.to_ascii_lowercase(), &cert.to_ascii_lowercase()),
        "sni wildcard for {subject:?} and {cert:?}"
    );
}

fn sni_glob(subject: &[u8], cert: &[u8]) -> bool {
    if subject == cert {
        return true;
    }
    let Some(star) = cert.iter().position(|&b| b == b'*') else {
        return false;
    };
    let (prefix, tail) = (&cert[..star], &cert[star + 1..]);
    let Some(after) = subject
        .strip_prefix(prefix)
        .filter(|_| !prefix.contains(&b'.'))
    else {
        return false;
    };
    (0..=after.len())
        .take_while(|&covered| covered == 0 || after[covered - 1] != b'.')
        .any(|covered| after[covered..] == *tail)
}
