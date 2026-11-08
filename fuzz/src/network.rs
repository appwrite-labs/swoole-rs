//! Network parsers fuzzed on their own: each target checks the parser against a
//! reference decode or against invariants of its output, not against the oracle.

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_void};
use std::net::IpAddr;
use swoole_rs_clients::dns::swr_v1_clients_dns_query;
use swoole_rs_http::{swr_v1_http_multipart_filename, swr_v1_http_multipart_result};
use swoole_rs_http2::{
    Http2FrameFacts, PacketLength, get_frame_length, sw_rs_http2_decode_frame,
    sw_rs_http2_unpack_setting,
};
use swoole_rs_oracle::multipart;
use swoole_rs_protocols::{RespPhpState, RespSpan, swr_v1_resp_php_next};

/// Any bytes as an HTTP/2 frame header (RFC 9113 4.1) and a SETTINGS entry (6.5.1):
/// `get_frame_length`, `sw_rs_http2_decode_frame` and `sw_rs_http2_unpack_setting`
/// agree with the wire layout.
pub fn http2_frame(bytes: &[u8]) {
    let Some((&[previous, client], data)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let pl = PacketLength {
        buf: data.as_ptr(),
        buf_size: u32::try_from(data.len()).unwrap_or(u32::MAX),
        header_len: 0,
    };
    // SAFETY: `buf` addresses `buf_size` bytes; the protocol and socket are unused.
    let length = unsafe { get_frame_length(std::ptr::null(), std::ptr::null_mut(), &pl) };
    let mut facts = Http2FrameFacts {
        length: 0,
        kind: 0,
        flags: 0,
        stream_id: 0,
        publish_last_id: 0,
        action: 0,
    };
    let previous = u32::from(previous) << 23;
    // SAFETY: `data` holds `data.len()` bytes; `facts` is a local.
    let decoded = unsafe {
        sw_rs_http2_decode_frame(
            data.as_ptr(),
            data.len(),
            previous,
            u32::from(client & 1),
            &mut facts,
        )
    };
    let Some(header) = data.first_chunk::<9>() else {
        assert_eq!((length, decoded), (0, 0), "short header {data:?}");
        return;
    };
    let wire = u32::from_be_bytes([0, header[0], header[1], header[2]]);
    let id = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & 0x7fff_ffff;
    assert_eq!(length, wire as isize + 9, "frame length of {header:?}");
    assert_eq!(decoded, 1);
    assert_eq!(
        (facts.length, facts.kind, facts.flags, facts.stream_id),
        (wire, u32::from(header[3]), u32::from(header[4]), id),
        "frame header {header:?}"
    );
    assert_eq!(facts.publish_last_id, u32::from(id > previous));
    if let Some(entry) = data.get(9..).and_then(|rest| rest.first_chunk::<6>()) {
        let (mut setting, mut value) = (0u16, 0u32);
        // SAFETY: `entry` holds six bytes; both outputs are locals.
        unsafe { sw_rs_http2_unpack_setting(entry.as_ptr(), &mut setting, &mut value) };
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

unsafe extern "C" fn dns_send(context: *mut c_void, data: *const u8, length: usize) -> bool {
    // SAFETY: `context` is the caller's `RefCell<Exchange>`; `data` holds `length` bytes.
    let (exchange, query) = unsafe {
        (
            &*context.cast::<RefCell<Exchange>>(),
            std::slice::from_raw_parts(data, length),
        )
    };
    exchange.borrow_mut().query = query.to_vec();
    true
}

unsafe extern "C" fn dns_recv(context: *mut c_void, out: *mut u8, capacity: usize) -> isize {
    // SAFETY: `context` is the caller's `RefCell<Exchange>`.
    let exchange = unsafe { &*context.cast::<RefCell<Exchange>>() }.borrow();
    let mut reply = exchange.reply.to_vec();
    if exchange.echo_id {
        for (byte, id) in reply.iter_mut().zip(&exchange.query) {
            *byte = *id;
        }
    }
    let n = reply.len().min(capacity);
    // SAFETY: `out` holds `capacity` writable bytes.
    unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), out, n) };
    n as isize
}

unsafe extern "C" fn dns_push(context: *mut c_void, text: *const u8, length: usize) {
    // SAFETY: `context` is the caller's `RefCell<Exchange>`; `text` holds `length` bytes.
    let (exchange, text) = unsafe {
        (
            &*context.cast::<RefCell<Exchange>>(),
            std::slice::from_raw_parts(text, length),
        )
    };
    exchange.borrow_mut().pushed.push(text.to_vec());
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
    let exchange = RefCell::new(Exchange {
        reply,
        echo_id: flags & 2 != 0,
        query: Vec::new(),
        pushed: Vec::new(),
    });
    let context = (&raw const exchange).cast_mut().cast();
    // SAFETY: the domain is NUL-terminated; the callbacks take `context`.
    let result = unsafe {
        swr_v1_clients_dns_query(
            c"example.test".as_ptr(),
            family,
            u16::from(flags >> 2) + 1,
            context,
            dns_send,
            dns_recv,
            dns_push,
            c_char::MIN != 0,
        )
    };
    let exchange = exchange.into_inner();
    let expected = if exchange.pushed.is_empty() { 711 } else { 0 };
    assert_eq!(result, expected, "reply {reply:?}");
    for text in &exchange.pushed {
        let text = std::str::from_utf8(text).expect("address text");
        text.parse::<IpAddr>().expect("an IP address");
    }
}

thread_local! {
    /// The chunk `multipart_parser_execute` is reading.
    static CHUNK: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    /// The current part's header value, for the filename scan.
    static VALUE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// `multipart_parser_settings`.
#[repr(C)]
struct Settings {
    on_header_field: Option<DataCallback>,
    on_header_value: Option<DataCallback>,
    on_part_data: Option<DataCallback>,
    on_part_data_begin: Option<Callback>,
    on_headers_complete: Option<Callback>,
    on_part_data_end: Option<Callback>,
    on_body_end: Option<Callback>,
}
type DataCallback = unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> c_int;
type Callback = unsafe extern "C" fn(*mut c_void) -> c_int;

/// A span the parser emits lies inside the chunk it was handed.
fn in_chunk(at: *const c_char, length: usize) {
    let (start, end) = CHUNK.get();
    let at = at as usize;
    assert!(
        at >= start && at.checked_add(length).is_some_and(|stop| stop <= end),
        "span outside the chunk"
    );
}

unsafe extern "C" fn on_span(_: *mut c_void, at: *const c_char, length: usize) -> c_int {
    in_chunk(at, length);
    0
}

/// Part data comes from the chunk, a literal or the parser's own boundary copy (a
/// boundary prefix that turned out to be data), so it is only read, under ASan.
unsafe extern "C" fn on_part_data(_: *mut c_void, at: *const c_char, length: usize) -> c_int {
    // SAFETY: the parser hands out `length` readable bytes at `at`.
    let data = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
    std::hint::black_box(data.iter().fold(0u8, |sum, &byte| sum ^ byte));
    0
}

unsafe extern "C" fn on_value(_: *mut c_void, at: *const c_char, length: usize) -> c_int {
    in_chunk(at, length);
    // SAFETY: `in_chunk` placed the span inside the live chunk.
    let value = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
    VALUE.with_borrow_mut(|buffer| buffer.extend_from_slice(value));
    0
}

unsafe extern "C" fn on_headers_complete(_: *mut c_void) -> c_int {
    let mut value = VALUE.take();
    value.push(0);
    // SAFETY: `value` is writable and NUL-terminated.
    let status = unsafe { swr_v1_http_multipart_filename(value.as_mut_ptr(), value.len()) };
    assert!(matches!(status, 0 | 1), "filename status {status}");
    0
}

const SETTINGS: Settings = Settings {
    on_header_field: Some(on_span),
    on_header_value: Some(on_value),
    on_part_data: Some(on_part_data),
    on_part_data_begin: None,
    on_headers_complete: Some(on_headers_complete),
    on_part_data_end: None,
    on_body_end: None,
};

/// A boundary of 1 to 70 bytes (RFC 2046 5.1.1) and any body, handed to the bundled
/// multipart parser the server streams uploads through, in fuzzer-chosen chunks:
/// each call consumes at most its chunk or fails with -1, every header span stays inside
/// it, part data is readable, and the filename scan of each part's headers stays in
/// bounds.
pub fn multipart(bytes: &[u8]) {
    let Some((&[boundary, step], rest)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let Some((boundary, body)) = rest.split_at_checked(usize::from(boundary % 70) + 1) else {
        return;
    };
    VALUE.take();
    let settings = SETTINGS;
    // SAFETY: the boundary holds its length in bytes; the settings outlive the parser.
    let parser = unsafe {
        multipart::multipart_parser_init(
            boundary.as_ptr().cast(),
            boundary.len(),
            (&raw const settings).cast(),
        )
    };
    assert!(!parser.is_null());
    let step = usize::from(step).max(1);
    for chunk in body.chunks(step) {
        let start = chunk.as_ptr() as usize;
        CHUNK.set((start, start + chunk.len()));
        // SAFETY: `chunk` holds its length in bytes; `parser` is live.
        let processed = unsafe {
            multipart::multipart_parser_execute(parser, chunk.as_ptr().cast(), chunk.len())
        };
        assert!(
            usize::try_from(processed).map_or(processed == -1, |n| n <= chunk.len()),
            "processed {processed} of {}",
            chunk.len()
        );
        if swr_v1_http_multipart_result(processed as i64, chunk.len(), 0) != 2 {
            break;
        }
    }
    // SAFETY: `parser` came from `multipart_parser_init`.
    unsafe { multipart::multipart_parser_free(parser) };
}

/// `llhttp_t`'s storage: its fields need fewer than 128 bytes, eight-byte aligned.
#[repr(C, align(8))]
struct Llhttp([u8; 128]);

unsafe extern "C" {
    fn llhttp_settings_init(settings: *mut [usize; 25]);
    fn llhttp_init(parser: *mut Llhttp, kind: c_int, settings: *const [usize; 25]);
    fn llhttp_execute(parser: *mut Llhttp, data: *const c_char, length: usize) -> c_int;
}

/// `HPE_OK`; `llhttp_settings_t.on_body`.
const HPE_OK: c_int = 0;
const ON_BODY: usize = 11;

thread_local! {
    /// Body bytes llhttp delivered.
    static BODY: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn on_body(_: *mut Llhttp, at: *const c_char, length: usize) -> c_int {
    in_chunk(at, length);
    BODY.set(BODY.get() + length);
    0
}

/// A request (or, on an odd first byte, a response) head with chunked transfer
/// coding and any body, handed to the bundled llhttp the server and client parse with
/// in fuzzer-chosen chunks: every body span lies inside its chunk and the body never
/// exceeds what was received.
pub fn http_chunked(bytes: &[u8]) {
    let Some((&[kind, step], body)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let (kind, head): (c_int, &[u8]) = if kind & 1 == 0 {
        (
            1,
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n",
        )
    } else {
        (2, b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
    };
    let mut settings = [0usize; 25];
    // SAFETY: `settings` is `llhttp_settings_t`-sized: 25 callback pointers.
    unsafe { llhttp_settings_init(&mut settings) };
    settings[ON_BODY] = on_body as *const () as usize;
    let mut parser = Llhttp([0; 128]);
    // SAFETY: `parser` covers `llhttp_t`; the settings outlive it.
    unsafe { llhttp_init(&mut parser, kind, &settings) };
    BODY.set(0);
    let input = [head, body].concat();
    for chunk in input.chunks(usize::from(step).max(1)) {
        let start = chunk.as_ptr() as usize;
        CHUNK.set((start, start + chunk.len()));
        // SAFETY: `chunk` holds its length in bytes; `parser` is initialized.
        if unsafe { llhttp_execute(&mut parser, chunk.as_ptr().cast(), chunk.len()) } != HPE_OK {
            break;
        }
    }
    assert!(
        BODY.get() <= body.len(),
        "body {} of {}",
        BODY.get(),
        body.len()
    );
}

/// Any bytes as a `Redis\Server` request, walked with `swr_v1_resp_php_next` as the
/// server's receive loop does: every event's span lies inside the request, the walk
/// moves forward and ends, and only the first string is the command.
pub fn resp_server(bytes: &[u8]) {
    let mut state = RespPhpState::default();
    let mut span = RespSpan::default();
    let (mut range, mut commands) = (0u8, 0);
    for _ in 0..=bytes.len() {
        let before = state.cursor.position;
        // SAFETY: `bytes` holds its length; state, span and range are locals.
        let next = unsafe {
            swr_v1_resp_php_next(
                bytes.as_ptr(),
                bytes.len(),
                &mut state,
                &mut span,
                &mut range,
            )
        };
        if next <= 0 {
            return;
        }
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
