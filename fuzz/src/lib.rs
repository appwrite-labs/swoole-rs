//! Differential fuzz targets: the same bytes go to a Rust entry point and to
//! the pinned upstream C++ in `swoole-rs-oracle`; return values and every output
//! byte, red zones included, must match.

use std::ffi::{CStr, c_char, c_int};
use swoole_rs_clients::{cookie, domain_decode};
use swoole_rs_corebase::log;
use swoole_rs_framing::{DefaultLength, default_length, unpack, width};
use swoole_rs_http::{BOUNDARY_SIZE, HttpStaticContext, HttpStaticObservation, StaticEffects};
use swoole_rs_oracle::{
    base64 as upstream, clients, framing as upstream_framing, framing_stream,
    http as upstream_http,
    http_frame::{self, Engine, UPSTREAM},
    http_proxy as upstream_proxy, http_range, protocols, resp_stream, socks5,
    websocket as upstream_ws,
};

mod bound;
mod framing_native;
mod protocols_native;

pub use bound::bounded;
mod network;
pub use network::{
    dns_reply, http_chunked, http_decompress, http_inflate, http_rewrite, http_static_path,
    http2_frame, http2_hpack, multipart, ws_inflate,
};
use swoole_rs_protocols::{RespKind, RespPhpState, resp_php_next};
use swoole_rs_websocket::{decode_in_place, frame_length, mask};

const RED_ZONE: usize = 17;

#[cfg(test)]
thread_local! {
    /// Left-out and compared inputs per target, for the seed replay in [`compared_share`].
    static TALLY: std::cell::RefCell<Vec<(&'static str, [usize; 2])>> =
        std::cell::RefCell::default();
}

/// Counts an input of `target` as compared with the pin or left out of its compared
/// domain. Only tests count; a fuzzing build does nothing here.
fn tally(target: &'static str, compared: bool) {
    #[cfg(test)]
    TALLY.with_borrow_mut(|tally| {
        let at = tally.iter().position(|&(name, _)| name == target).unwrap_or_else(|| {
            tally.push((target, [0; 2]));
            tally.len() - 1
        });
        tally[at].1[usize::from(compared)] += 1;
    });
    #[cfg(not(test))]
    let _ = (target, compared);
}

/// `swoole_rs_http::Request` behind the oracle's request framing entry points.
const CANDIDATE: Engine = Engine {
    frame: request::frame,
    expect: request::expect,
    header: request::header,
    method: method_string,
    header_info: request::header_info,
};

/// `get_method_string` as the oracle's engine table holds it: crates/http's name, or null.
extern "C" fn method_string(method: i32) -> *const c_char {
    swoole_rs_http::method_string(method).map_or(std::ptr::null(), CStr::as_ptr)
}

mod request {
    use std::ffi::{CStr, c_char};
    use swoole_rs_corebase::buffer::string::{StringBuffer, boxed_string};
    use swoole_rs_corebase::{set_errno, sys::errno};
    use swoole_rs_http::ChunkState;
    use swoole_rs_http::HttpRequest as Request;

    /// `parse` over a request and its received `swoole::String`: a copy of the
    /// `length` bytes at `data`, with the spare NUL after them, at `offset`. What
    /// `parse` returned, and the buffer's offset after.
    fn on<T>(
        data: *mut c_char,
        length: usize,
        offset: usize,
        parse: impl FnOnce(&mut Request, &mut StringBuffer) -> T,
    ) -> (T, usize) {
        let bytes = if data.is_null() {
            &[]
        } else {
            // SAFETY: the caller's `length` bytes.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }
        };
        let mut buffer = boxed_string(bytes);
        buffer.set_offset(offset as i64);
        let parsed = parse(&mut Request::default(), &mut buffer);
        let offset = buffer.offset() as usize;
        buffer.dispose();
        (parsed, offset)
    }

    pub unsafe extern "C" fn frame(
        operation: u32,
        data: *mut c_char,
        length: usize,
        state: *mut http_frame::State,
    ) -> i32 {
        // SAFETY: the caller's state.
        let s = unsafe { &mut *state };
        let (done, offset) = on(data, length, s.offset, |r, buffer| {
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
            let done = match operation {
                0 => r.get_protocol(buffer),
                1 => r.get_header_length(buffer),
                _ => r.get_chunked_body_length(buffer.view()),
            };
            *s = http_frame::State {
                offset: s.offset,
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
            done
        });
        s.offset = offset;
        // `SW_OK` or `SW_ERR`, as the oracle returns.
        if done { 0 } else { -1 }
    }

    pub unsafe extern "C" fn header_info(
        data: *mut c_char,
        length: usize,
        line_length: u32,
        header_length: u32,
        out: *mut http_frame::HeaderInfo,
    ) {
        let (info, _) = on(data, length, 0, |r, buffer| {
            (r.request_line_length, r.header_length) = (line_length, header_length);
            set_errno(0);
            r.parse_header_info(buffer.view());
            let (offset, boundary) = r.form.as_deref().map_or((-1, 0), |form| {
                let boundary = form.boundary();
                (boundary.offset as i64, boundary.length as i64)
            });
            let bit = |set: bool, flag: u32| if set { flag } else { 0 };
            http_frame::HeaderInfo {
                content_length: r.content_length,
                flags: bit(r.known_length, http_frame::flags::KNOWN_LENGTH)
                    | bit(r.keep_alive, http_frame::flags::KEEP_ALIVE)
                    | bit(r.chunked, http_frame::flags::CHUNKED)
                    | bit(r.header_parsed, http_frame::flags::HEADER_PARSED)
                    | bit(r.excepted, http_frame::flags::EXCEPTED),
                error: errno().cast_unsigned(),
                boundary_offset: offset,
                boundary_length: boundary,
            }
        });
        // SAFETY: the caller's output.
        unsafe { *out = info };
    }

    pub unsafe extern "C" fn expect(data: *mut c_char, length: usize) -> bool {
        on(data, length, 0, |r, b| r.has_expect_header(b.view())).0
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
        // SAFETY: a NUL-terminated name.
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        let (value, _) = on(data, length, 0, |r, buffer| {
            (r.url_offset, r.url_length, r.header_length) = (url_offset, url_length, header_length);
            r.get_header(buffer.view(), name)
        });
        let Some(value) = value else {
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
    let (got, got_header) = match frame_length(bytes) {
        Ok(frame) => (
            i64::try_from(frame.total).unwrap(),
            Some(frame.header_length),
        ),
        Err(error) => (
            match error {
                swoole_rs_websocket::LengthError::IncompleteHeader { .. } => 0,
                swoole_rs_websocket::LengthError::Overflow { .. } => -1,
            },
            error.header_length(),
        ),
    };
    let got_header = u32::try_from(got_header.unwrap_or(0)).unwrap();
    // Some lengths are outside the compared domain; Rust reports -1 there.
    tally("ws_frame_length", want as i64 == got);
    assert!(
        want as i64 == got || got == -1 && want < 0,
        "total {want} vs {got} for {bytes:?}"
    );
    assert_eq!(want_header, got_header, "header length for {bytes:?}");
}

/// Any bytes as one frame: the same verdict, fields and in-place unmasking.
pub fn ws_decode(bytes: &[u8]) {
    // Servers only decode frames whose length check has passed.
    if bytes.len() < 2 {
        return;
    }
    let (mut theirs, mut ours) = (bytes.to_vec(), bytes.to_vec());
    let mut want = upstream_ws::Frame::default();
    // SAFETY: upstream owns its copy of the frame and its output.
    let ok = unsafe { upstream_ws::decode(theirs.as_mut_ptr(), theirs.len(), &mut want) };
    let result = decode_in_place(&mut ours);
    assert_eq!(ok, result.is_ok(), "verdict ({result:?}) for {bytes:?}");
    if ok {
        let got = result.expect("the verdict comparison succeeded");
        let got = upstream_ws::Frame {
            payload_length: got.payload_length as u64,
            header_length: got.header_length as u16,
            opcode: got.opcode.code(),
            flags: got.flags.bits(),
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
    assert_ne!(want, usize::MAX, "upstream URL encoding failed");
    let capacity = swoole_rs_http::url_capacity(bytes).unwrap();
    let mut ours = vec![std::mem::MaybeUninit::new(0xa5); capacity];
    let written = swoole_rs_http::url_encode(bytes, &mut ours).unwrap();
    // SAFETY: every slot started initialized.
    let ours: Vec<u8> = ours
        .iter()
        .map(|slot| unsafe { slot.assume_init() })
        .collect();
    assert_eq!(
        (written, capacity),
        (want, want + 1),
        "length for {bytes:?}"
    );
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
        for range in [cookie.key, cookie.value.unwrap_or_default()] {
            ours.extend_from_slice(&(range.len() as u32).to_ne_bytes());
            ours.extend_from_slice(&buffer[range]);
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
    let got = default_length(format, data, offset.into(), size.into(), body_offset.into());
    tally("framing_length", got != DefaultLength::Invalid);
    match got {
        DefaultLength::Need(got) => {
            assert_eq!((want, header as i64), (0, got), "need more for {bytes:?}");
        }
        DefaultLength::Total(got) => assert_eq!(want as i64, got, "total for {bytes:?}"),
        // Outside the compared domain the pin's error must be matched by exactly `Invalid`:
        // every format FORMATS picks is known, so `Format` is never right here.
        // The pin's answer is SW_ERR, except that an 8-byte length's sum with the body
        // offset can wrap to another negative.
        DefaultLength::Invalid => assert!(
            want == -1 || size == 8 && want < 0,
            "invalid vs {want} for {bytes:?}"
        ),
        DefaultLength::Format => panic!("unknown format for a known one in {bytes:?}"),
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
    let left_out = operation == 0 && want.0 == 0 && want.1.offset > data.len();
    tally("http_frame", !left_out);
    if left_out {
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
    let got = swoole_rs_protocols::mqtt_frame_length(&bytes[..size as usize])
        .map_or(-1, |length| {
            length.map_or(0, |n| i64::try_from(n).unwrap_or(-1))
        });
    if size == 0 {
        return;
    }
    // SAFETY: it reads at most `size` bytes of `data`.
    let want = unsafe { protocols::mqtt_length(data.as_ptr(), size) };
    assert_eq!(got, want, "mqtt length for {bytes:?}");
}

/// Models the pinned reply parser or PHP request loop without calling either
/// parser. PHP falls through unrecognized prefixes to a string of the retained
/// length; its compared domain also requires both string trailer bytes.
fn resp_in_bounds(data: &[u8], php: bool) -> bool {
    let at = |index: usize| data.get(index).copied().unwrap_or(0);
    // get_number: strtol after the type byte, then CRLF; Some((ret, next)).
    let number = |p: usize| {
        let mut q = p + 1;
        while matches!(at(q), b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
            q += 1;
        }
        let sign = at(q);
        q += usize::from(matches!(sign, b'+' | b'-'));
        let digits = q;
        let mut value: i128 = 0;
        while at(q).is_ascii_digit() {
            value = (value * 10 + i128::from(at(q) - b'0')).min(1 << 70);
            q += 1;
        }
        let end = if q == digits { p + 1 } else { q };
        let value = if sign == b'-' { -value } else { value };
        // strtol clamps to long; the int conversion keeps the low 32 bits.
        let value = value.clamp(i64::MIN.into(), i64::MAX.into()) as i64 as i32;
        (at(end) == b'\r' && at(end + 1) == b'\n').then_some((value, end + 2))
    };
    // Upstream's switch: one `*` count, before any string; `$` lengths, each but -1
    // followed by a string of that length; and `:` integers. PHP reads any other
    // prefix as a string of the length last seen.
    let string = |p: usize, length: i32| {
        let trailer = if php { 2 } else { 0 };
        let end = usize::try_from(length).ok().map(|length| p + length);
        end.filter(|&end| end + trailer <= data.len())
            .map(|end| end + 2)
    };
    let (mut p, mut length, mut counted, mut pending) = (0, 0, false, false);
    while p < data.len() {
        let next = if std::mem::take(&mut pending) {
            string(p, length)
        } else {
            match at(p) {
                b'*' if !counted => number(p).map(|(_, next)| {
                    counted = true;
                    next
                }),
                b'$' => number(p).map(|(ret, next)| {
                    if ret != -1 {
                        (length, counted, pending) = (ret, true, true);
                    }
                    next
                }),
                b':' => number(p).map(|(_, next)| next),
                _ if php => {
                    counted = true;
                    string(p, length)
                }
                _ => None,
            }
        };
        let Some(next) = next else {
            return false;
        };
        p = next;
    }
    // An empty input fails the switch's one run.
    !data.is_empty()
}

type RespOracle =
    unsafe extern "C" fn(*const c_char, usize, c_int, *mut usize, *mut c_int) -> *mut c_char;

/// Upstream's packed items and, on glibc, whether errno is ERANGE. macOS strtol
/// also sets EINVAL when it converts nothing, which can overwrite an earlier
/// ERANGE; glibc leaves errno alone there, and callers only test ERANGE.
fn resp_upstream(data: &[u8], parse: RespOracle) -> (Vec<u8>, bool) {
    let buffer = terminated(data);
    let (mut size, mut error) = (0, 0);
    // SAFETY: the caller's bounds model admits this loop; the wire has a NUL
    // terminator and both output pointers name writable locals.
    let output = unsafe { parse(buffer.as_ptr(), data.len(), 0, &mut size, &mut error) };
    // SAFETY: the oracle returns a malloc'd buffer of `size` initialized bytes.
    let items = unsafe { std::slice::from_raw_parts(output.cast::<u8>(), size) }.to_vec();
    // SAFETY: the oracle allocated `output`, and its bytes have been copied.
    unsafe { protocols::free(output) };
    (items, cfg!(target_os = "linux") && error == libc::ERANGE)
}

/// Upstream `redis::parse` and `redis::parse` over Rust's spans, over the same bytes:
/// every string, in order, and ERANGE. Rust runs on every input.
pub fn resp_parse(bytes: &[u8]) {
    let (items, range) = protocols_native::resp_parse(bytes);
    let got = (items, cfg!(target_os = "linux") && range);
    let compared = resp_in_bounds(bytes, false);
    tally("resp_parse", compared);
    if !compared {
        return;
    }
    assert_eq!(
        got,
        resp_upstream(bytes, protocols::resp_parse),
        "resp parse for {bytes:?}"
    );
}

fn resp_item(out: &mut Vec<u8>, kind: u32, bytes: &[u8]) {
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_ne_bytes());
    out.extend_from_slice(bytes);
}

/// Candidate PHP request, packed like the oracle: command first, then typed
/// parameters. Every input still checks progress, span bounds and one command.
fn resp_request(bytes: &[u8]) -> Option<(Vec<u8>, bool)> {
    let mut state = RespPhpState::default();
    let (mut command, mut parameters, mut range) = (None, Vec::new(), false);
    for _ in 0..=bytes.len() {
        let before = state.cursor.position;
        let Some(span) = resp_php_next(bytes, &mut state, &mut range).ok()? else {
            let mut out = Vec::new();
            resp_item(
                &mut out,
                u32::from(command.is_some()),
                command.unwrap_or_default(),
            );
            out.extend(parameters);
            return Some((out, cfg!(target_os = "linux") && range));
        };
        assert!(state.cursor.position > before, "stalled on {bytes:?}");
        assert!(
            matches!(span.kind, RespKind::Integer | RespKind::Null)
                || span
                    .offset
                    .checked_add(span.length)
                    .is_some_and(|end| end <= bytes.len()),
            "span {span:?} outside {bytes:?}"
        );
        match span.kind {
            RespKind::Command => {
                assert!(command.is_none(), "second command in {bytes:?}");
                command = Some(&bytes[span.offset..span.offset + span.length]);
            }
            RespKind::String => resp_item(
                &mut parameters,
                0,
                &bytes[span.offset..span.offset + span.length],
            ),
            RespKind::Integer => resp_item(&mut parameters, 1, span.integer.to_string().as_bytes()),
            RespKind::Null => resp_item(&mut parameters, 2, &[]),
        }
    }
    panic!("no end for {bytes:?}");
}

/// The PHP request loop's command, parameter kinds/values and Linux ERANGE must
/// match the pinned loop on every input admitted by its independent bounds model.
pub fn resp_server(bytes: &[u8]) {
    let got = resp_request(bytes);
    let compared = resp_in_bounds(bytes, true);
    tally("resp_server", compared);
    if compared {
        assert_eq!(
            got,
            Some(resp_upstream(bytes, protocols::resp_php_request)),
            "resp request for {bytes:?}"
        );
    }
}

#[cfg(test)]
#[path = "../../tests/modules/protocols/fuzz_request.rs"]
mod fuzz_request_tests;

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
        Ok((kind, host, port)) => (1, kind.raw(), port, bytes[host].to_vec()),
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

/// Any bytes as an HTTP proxy's reply to the client's CONNECT: `proxy_accept`, which
/// the client calls, and the pinned `HttpProxy::handshake` agree on whether the tunnel
/// is up and on the last error they set.
pub fn http_proxy(bytes: &[u8]) {
    let mut error = 0;
    // SAFETY: reads `bytes`; `error` is a local.
    let accepted = unsafe { upstream_proxy::handshake(bytes.as_ptr(), bytes.len(), &mut error) };
    log::set_last_error(0);
    let got = (swoole_rs_http::proxy_accept(bytes), log::last_error());
    assert_eq!(got, (accepted, error), "proxy handshake {bytes:?}");
}

/// Any bytes as a received DNS name buffer: where Rust decodes it, upstream
/// decodes to the same bytes. Other inputs are outside the compared domain: Rust
/// reports 0 and upstream isn't run.
pub fn clients_domain_decode(bytes: &[u8]) {
    let (mut theirs, mut ours) = (bytes.to_vec(), bytes.to_vec());
    let compared = domain_decode(&mut ours, c_char::MIN != 0);
    tally("clients_domain_decode", compared);
    if compared {
        // SAFETY: `theirs` is a NUL-terminated name in the compared domain, decoded within its bytes.
        unsafe { clients::domain_decode(theirs.as_mut_ptr().cast()) };
        assert_eq!(theirs, ours, "domain {bytes:?}");
    }
    // A well-formed name made from the same bytes, decided here rather than by the
    // candidate: one or more labels of 1 to 63 nonzero bytes behind their length, then a
    // NUL, at most 255 octets in all (DNS's limit). The candidate must take it and decode
    // it as the pin does, so declining everything fails.
    let mut name = Vec::with_capacity(256);
    for label in bytes.split(|&b| b == 0).flat_map(|run| run.chunks(63)) {
        if name.len() + 1 + label.len() + 1 > 255 {
            break;
        }
        name.push(u8::try_from(label.len()).unwrap_or(63));
        name.extend_from_slice(label);
    }
    if name.is_empty() {
        return;
    }
    name.push(0);
    let (mut theirs, mut ours) = (name.clone(), name);
    assert!(
        domain_decode(&mut ours, c_char::MIN != 0),
        "well-formed {bytes:?} declined"
    );
    // SAFETY: `theirs` is a NUL-terminated well-formed name, which decodes within its bytes.
    unsafe { clients::domain_decode(theirs.as_mut_ptr().cast()) };
    assert_eq!(theirs, ours, "well-formed domain {bytes:?}");
}

/// Any bytes up to the first blank line as a request head: `parse_header_info` on both
/// engines gives the same lengths, flags, errno and multipart boundary.
pub fn http_header_info(bytes: &[u8]) {
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return;
    };
    let line = bytes.windows(2).position(|w| w == b"\r\n").unwrap_or(0);
    let info = |engine: &Engine, bytes: &[u8], end: usize| {
        let (mut buffer, mut info) = (terminated(bytes), http_frame::HeaderInfo::default());
        // SAFETY: the buffer holds bytes.len() bytes and a NUL; the head ends in a blank
        // line before end + 4; info is a local.
        unsafe {
            (engine.header_info)(
                buffer.as_mut_ptr(),
                bytes.len(),
                line as u32,
                (end + 4) as u32,
                &mut info,
            )
        };
        info
    };
    let got = info(&CANDIDATE, bytes, end);
    // Outside the compared domain the candidate rejects the request at that line, with
    // what the head before it gave and its scan unfinished.
    let left_out = multipart_nul(bytes, line + 2, end);
    tally("http_header_info", left_out.is_none());
    if let Some(at) = left_out {
        let head = [&bytes[..at], b"\r\n"].concat();
        let mut want = info(&UPSTREAM, &head, at - 2);
        let unfinished = http_frame::flags::HEADER_PARSED | http_frame::flags::CHUNKED;
        want.flags = want.flags & !unfinished | http_frame::flags::EXCEPTED;
        assert_eq!(got, want, "excepted header info for {bytes:?}");
        return;
    }
    assert_eq!(got, info(&UPSTREAM, bytes, end), "header info for {bytes:?}");
}

/// The first multipart/form-data Content-Type line in the head, as parse_header_info
/// finds it, that meets a NUL before its CR.
fn multipart_nul(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let starts = |at: usize, prefix: &[u8]| {
        bytes.get(at..end).is_some_and(|rest| {
            rest.len() >= prefix.len() && rest[..prefix.len()].eq_ignore_ascii_case(prefix)
        })
    };
    (start.max(2)..end).find(|&at| {
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
    _ = context.stat(&facade);
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
        swoole_rs_corebase::utilities::sni_wildcard_matches(subject, cert),
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

/// Replays each target's seeds (`fuzz/seeds`) and fails when the share it compares with
/// the pin, rather than leaving out, falls below a floor: today's share less ten points,
/// rounded down to a multiple of five (#5355).
#[cfg(test)]
mod compared_share {
    use super::TALLY;

    /// A target, its entry point, and the lowest compared share allowed, in percent.
    type Floor = (&'static str, fn(&[u8]), usize);

    const FLOORS: [Floor; 7] = [
        ("ws_frame_length", super::ws_frame_length, 80),
        ("framing_length", super::framing_length, 55),
        ("http_frame", super::http_frame, 85),
        ("resp_parse", super::resp_parse, 90),
        ("resp_server", super::resp_server, 90),
        ("clients_domain_decode", super::clients_domain_decode, 55),
        ("http_header_info", super::http_header_info, 90),
    ];

    #[test]
    fn seeds_stay_compared() {
        let seeds = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds");
        for (target, run, floor) in FLOORS {
            for entry in std::fs::read_dir(seeds.join(target)).unwrap() {
                run(&std::fs::read(entry.unwrap().path()).unwrap());
            }
            let [left_out, compared] = TALLY.with_borrow_mut(|tally| {
                let at = tally.iter().position(|&(name, _)| name == target).unwrap();
                tally.remove(at).1
            });
            eprintln!("{target}: {compared} compared, {left_out} left out");
            assert!(
                compared * 100 >= floor * (compared + left_out),
                "{target} compares {compared} of {} seeds, under {floor}%",
                compared + left_out
            );
        }
    }
}
