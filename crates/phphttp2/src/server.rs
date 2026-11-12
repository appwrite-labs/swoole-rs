//! The HTTP/2 server's frames, responses and request header blocks: upstream's
//! `swoole_http2_server_parse` and everything it and `Response` send (headers
//! over crates/http2's HPACK, bodies under flow control, trailers, files), over
//! the sessions and streams of `session`, and an async server's
//! `onReceive` and `onClose` for its HTTP/2 connections. The HTTP contexts are
//! crates/phphttp's.
use crate::session::{Session, SessionState, Stream};
use libc::{c_char, c_int};
use std::{
    ffi::{CStr, CString},
    ptr,
};
use swoole_rs_http2::{
    Http2HeaderPlan, Http2RespondFacts, Http2WindowPlan,
    hpack::{Encoder, Error},
    send_frame_size, sw_rs_http2_chunk, sw_rs_http2_content_length_rejected,
    sw_rs_http2_data_rejected, sw_rs_http2_decode_frame, sw_rs_http2_file_complete,
    sw_rs_http2_header_defaults, sw_rs_http2_header_plan, sw_rs_http2_headers_flags,
    sw_rs_http2_inflate_step, sw_rs_http2_media_kind, sw_rs_http2_pack_frame_header,
    sw_rs_http2_respond_next, sw_rs_http2_response_status, sw_rs_http2_setting_plan,
    sw_rs_http2_window_action, sw_rs_http2_window_plan,
};
use swoole_rs_phpserver::RecvData;
use swoole_rs_server::{ConnectionRecord, view};
use swoole_rs_zend::{
    Array, HeldArray, Owned, Value as Zval, ZString, bailout, string,
    sys::{add_assoc_str_ex, add_assoc_stringl_ex, swr_zend_url_decode, zval},
};

/// Frame sizes, types and flags (`swoole_http2.h`).
const FRAME_HEADER: usize = 9;
const PING_PAYLOAD: usize = 8;
const GOAWAY_SIZE: usize = 8;
const TYPE_DATA: u32 = 0;
const TYPE_HEADERS: u32 = 1;
const TYPE_RST_STREAM: u32 = 3;
const TYPE_SETTINGS: u32 = 4;
const TYPE_PING: u32 = 6;
const TYPE_GOAWAY: u32 = 7;
const TYPE_WINDOW_UPDATE: u32 = 8;
const TYPE_CONTINUATION: u32 = 9;
const FLAG_ACK: u32 = 0x1;
const FLAG_END_STREAM: u32 = 0x1;
const FLAG_END_HEADERS: u32 = 0x4;
const FLAG_PADDED: u32 = 0x8;
const FLAG_PRIORITY: u32 = 0x20;
/// `SwRsHttp2FrameAction`.
const ACTION_SETTINGS: u32 = 0;
const ACTION_HEADERS: u32 = 1;
const ACTION_DATA: u32 = 2;
const ACTION_PING_REPLY: u32 = 3;
const ACTION_WINDOW_UPDATE: u32 = 4;
const ACTION_RESET: u32 = 5;
/// `SwRsHttp2RespondAction`.
const RESP_START: u32 = 0;
const RESP_HEADER: u32 = 1;
const RESP_COMMIT_END: u32 = 2;
const RESP_EMPTY: u32 = 3;
const RESP_BODY: u32 = 4;
const RESP_TRAILER: u32 = 5;
const RESP_ERROR_CLOSE: u32 = 6;
const RESP_REMOVE: u32 = 7;
const RESP_ERROR_SHUTDOWN: u32 = 8;
const RESP_SUCCESS_SHUTDOWN: u32 = 9;
const RESP_TRUE: u32 = 11;
/// `SwRsHttp2FileCompletion`.
const FILE_TRAILER: u32 = 0;
const FILE_CLOSE: u32 = 1;
/// `SwRsHttp2HeaderRole` and `SwRsHttp2HeaderKind`.
const REQUEST_HEADERS: u32 = 0;
const SERVER_HEADERS: u32 = 2;
const HEADER_METHOD: u32 = 1;
const HEADER_PATH: u32 = 2;
const HEADER_AUTHORITY: u32 = 3;
const HEADER_CONTENT_TYPE: u32 = 4;
const HEADER_COOKIE: u32 = 5;
const HEADER_CONTENT_LENGTH: u32 = 6;
const HEADER_ACCEPT_ENCODING: u32 = 7;
const MEDIA_URLENCODED: u32 = 1;
const MEDIA_MULTIPART: u32 = 2;
/// `HTTP_HEADER_SERVER`, `_DATE` and `_CONTENT_TYPE`.
const MISSING_SERVER: u32 = 2;
const MISSING_DATE: u32 = 16;
const MISSING_CONTENT_TYPE: u32 = 32;
/// `SW_HTTP_REQUEST_ENTITY_TOO_LARGE`.
const ENTITY_TOO_LARGE: u32 = 413;
/// `SW_HTTP2_SETTING_HEADER_TABLE_SIZE`.
const SETTING_HEADER_TABLE_SIZE: u16 = 1;
/// `swoole::http2::Settings` field indexes.
const INIT_WINDOW_SIZE: usize = 3;
const MAX_FRAME_SIZE: usize = 4;
/// Swoole error codes and log levels.
const PHP_FATAL_ERROR: c_int = 503;
const NO_PAYLOAD: c_int = 550;
const STREAM_NOT_FOUND: c_int = 3003;
const STREAM_IGNORE: c_int = 3004;
const LOG_WARNING: c_int = 4;
const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;

use swoole_rs_phphttp::decimal;
use swoole_rs_phphttp::http2::Field;

pub(crate) use swoole_rs_phphttp::http2::{Ctx, ResponseArray, Value};

pub(crate) fn get(ctx: Ctx, field: Field) -> u32 {
    ctx.get(field)
}

pub(crate) fn set(ctx: Ctx, field: Field, value: u32) {
    ctx.set(field, value);
}

fn send(ctx: Ctx, data: &[u8]) -> bool {
    ctx.send(data)
}

fn close(ctx: Ctx) {
    ctx.close();
}

pub(crate) fn zval_of(ctx: Ctx, which: Value) -> *mut zval {
    ctx.value(which)
}

/// `swoole_warning` (`error` 0) or `swoole_error_log(level, error)` as `func`.
pub(crate) fn log(level: c_int, error: c_int, func: &CStr, message: &str) {
    if error > 0 {
        swoole_rs_corebase::log::error_log(level, error, func, format_args!("{message}"));
    } else {
        swoole_rs_corebase::log::warning(func, format_args!("{message}"));
    }
}

fn warn(func: &CStr, message: &str) {
    log(LOG_WARNING, 0, func, message);
}

/// `php_swoole_error_ex(E_WARNING, code, ...)`, or `php_swoole_fatal_error` when `always`.
fn report(code: c_int, always: bool, message: &[u8]) {
    if bailout::pending() {
        return;
    }
    swoole_rs_corebase::log::set_last_error(code);
    if always || swoole_rs_phpswoole::display_errors() {
        let text = message.split(|&byte| byte == 0).next().unwrap_or_default();
        let message = CString::new(text).unwrap_or_default();
        bailout::defer(swoole_rs_zend::report(swoole_rs_zend::E_WARNING, &message));
    }
}

/// A frame's nine header bytes.
fn frame_header(kind: u32, length: usize, flags: u32, stream_id: u32) -> [u8; FRAME_HEADER] {
    #[expect(clippy::cast_possible_truncation, reason = "frame lengths are 24-bit")]
    sw_rs_http2_pack_frame_header(kind, length as u32, flags, stream_id)
}

/// The session and stream of a response's context.
pub(crate) fn of(ctx: Ctx) -> Option<(Session, Stream)> {
    let session = SessionState::of(ctx)?;
    let stream = session.stream(get(ctx, Field::StreamId), false)?;
    Some((session, stream))
}

/// `http2_server_check_stream`: whether `stream` is still its session's. A
/// stream the peer reset, or whose connection closed, ends its response.
pub(crate) fn current(session: &Session, stream: &Stream) -> bool {
    if session
        .stream(stream.id, false)
        .is_some_and(|found| std::rc::Rc::ptr_eq(&found, stream))
    {
        return true;
    }
    set(stream.ctx, Field::End, 1);
    false
}

// --- small frames ---

fn send_u32_frame(
    ctx: Ctx,
    kind: u32,
    length: usize,
    flags: u32,
    stream_id: u32,
    value: u32,
) -> bool {
    let mut frame = [0; FRAME_HEADER + 4];
    frame[..FRAME_HEADER].copy_from_slice(&frame_header(kind, length, flags, stream_id));
    frame[FRAME_HEADER..].copy_from_slice(&value.to_be_bytes());
    send(ctx, &frame)
}

fn window_update(ctx: Ctx, stream_id: u32, size: u32) {
    send_u32_frame(ctx, TYPE_WINDOW_UPDATE, 4, 0, stream_id, size);
}

/// `Stream::reset`.
pub(crate) fn reset(ctx: Ctx, stream_id: u32, code: u32) {
    send_u32_frame(ctx, TYPE_RST_STREAM, 4, 0, stream_id, code);
}

/// `swoole_http2_server_ping`.
fn ping(ctx: Ctx) -> bool {
    let mut frame = [0; FRAME_HEADER + PING_PAYLOAD];
    frame[..FRAME_HEADER].copy_from_slice(&frame_header(TYPE_PING, PING_PAYLOAD, 0, 0));
    send(ctx, &frame)
}

/// `swoole_http2_server_goaway`.
fn goaway(ctx: Ctx, code: i64, data: &[u8]) -> bool {
    let Some(session) = SessionState::of(ctx) else {
        return false;
    };
    let last_stream_id = session.last_stream_id.get();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's uint8_t flags and uint32_t code"
    )]
    let (flags, code) = (u32::from(code as u8), code as u32);
    let mut frame = Vec::with_capacity(FRAME_HEADER + GOAWAY_SIZE + data.len());
    frame.extend_from_slice(&frame_header(
        TYPE_GOAWAY,
        GOAWAY_SIZE + data.len(),
        flags,
        0,
    ));
    frame.extend_from_slice(&last_stream_id.to_be_bytes());
    frame.extend_from_slice(&code.to_be_bytes());
    frame.extend_from_slice(data);
    let sent = send(ctx, &frame);
    session.shutting_down.set(true);
    sent
}

/// `length` bytes at `data`, empty for null.
fn bytes<'a>(data: *const c_char, length: usize) -> &'a [u8] {
    if data.is_null() || length == 0 {
        return &[];
    }
    // SAFETY: the caller lends `length` bytes.
    unsafe { std::slice::from_raw_parts(data.cast(), length) }
}

// --- the response header block ---

/// A header block's buffers, kept between responses (`POOL`).
#[derive(Default)]
struct Buffers {
    bytes: Vec<u8>,
    /// Each header's start, name length and value length in `bytes`.
    slots: Vec<(usize, usize, usize)>,
    /// The frame: nine header bytes, then the HPACK block.
    frame: Vec<u8>,
}

thread_local! {
    /// Buffers no header block is using. A block being built can outlive a
    /// coroutine switch (`__toString`), so each takes its own.
    static POOL: std::cell::RefCell<Vec<Buffers>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Upstream's `HeaderSet`: at most `size` headers, names lowercased, in one
/// byte arena.
struct Headers {
    size: usize,
    buffers: Buffers,
}

impl Headers {
    fn new(size: usize) -> Self {
        let buffers = POOL.with_borrow_mut(Vec::pop).unwrap_or_default();
        Self { size, buffers }
    }

    /// A header the script named: its name lowercased.
    fn add_folded(&mut self, name: &[u8], value: &[u8]) {
        let start = self.buffers.bytes.len();
        self.add(name, value);
        if let Some(added) = self.buffers.bytes.get_mut(start..start + name.len()) {
            added.make_ascii_lowercase();
        }
    }

    /// A header whose name is already lowercase.
    fn add(&mut self, name: &[u8], value: &[u8]) {
        let buffers = &mut self.buffers;
        if buffers.slots.len() < self.size {
            let start = buffers.bytes.len();
            buffers.bytes.extend_from_slice(name);
            buffers.bytes.extend_from_slice(value);
            buffers.slots.push((start, name.len(), value.len()));
        } else {
            let mut message = b"unexpect http2 header [".to_vec();
            message.extend_from_slice(name);
            message.extend_from_slice(b"] (duplicated or overflow)");
            report(PHP_FATAL_ERROR, true, &message);
        }
    }

    /// HPACK-encode the set into the frame after its nine header bytes, which
    /// `seal` fills in, with the session's encoder, made on first use.
    fn deflate(mut self, session: &Session) -> Self {
        let mut deflater = session.deflater.borrow_mut();
        let encoder = deflater.get_or_insert_with(|| Encoder::new(session.remote[0].get()));
        let Buffers {
            bytes,
            slots,
            frame,
        } = &mut self.buffers;
        frame.resize(FRAME_HEADER, 0);
        let field = |start: usize, length: usize| bytes.get(start..start + length);
        encoder.encode(
            slots.iter().map(|&(start, name, value)| {
                let value = field(start + name, value).unwrap_or_default();
                (field(start, name).unwrap_or_default(), value)
            }),
            frame,
        );
        drop(deflater);
        self
    }

    /// The frame `deflate` built, its header filled in.
    fn seal(&mut self, kind: u32, flags: u32, stream_id: u32) -> &mut Vec<u8> {
        let frame = &mut self.buffers.frame;
        let header = frame_header(kind, frame.len() - FRAME_HEADER, flags, stream_id);
        if let Some(head) = frame.first_chunk_mut::<FRAME_HEADER>() {
            *head = header;
        }
        frame
    }
}

impl Drop for Headers {
    fn drop(&mut self) {
        let mut buffers = std::mem::take(&mut self.buffers);
        buffers.bytes.clear();
        buffers.slots.clear();
        buffers.frame.clear();
        POOL.with_borrow_mut(|pool| pool.push(buffers));
    }
}

/// A response array property (`sw_zend_read_property_ex`), with a reference of
/// its own. This is the server's only way to read the `header`, `cookie` and
/// `trailer` properties, and [`length_safe`] and [`entries`] take only a
/// [`HeldArray`]: a value's `__toString` may replace or unset the property while
/// the array is walked, and the hold keeps the walked table live.
fn property(ctx: Ctx, which: ResponseArray) -> Option<HeldArray> {
    ctx.response_array(which)
}

/// `php_swoole_array_length_safe`.
fn length_safe(array: Option<&HeldArray>) -> usize {
    array
        .and_then(|array| array.array())
        .map_or(0, |array| array.count() as usize)
}

/// Each element of a held array, with its string key (None for an integer key).
/// The server's and the client's header walks go through here, so the table
/// they walk is one a [`HeldArray`] keeps live.
pub(crate) fn entries(array: Option<&HeldArray>) -> impl Iterator<Item = (Option<&[u8]>, &Zval)> {
    array
        .and_then(|array| array.array())
        .into_iter()
        .flat_map(Array::entries)
}

/// `zval_get_string`, `None` once a bailout is pending.
fn string_of(value: &Zval) -> Option<ZString> {
    let text = value.string();
    (!bailout::pending()).then_some(text)
}

/// `swoole::rtrim`: trailing `isspace` bytes off.
fn rtrim(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .rposition(|&byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
        .map_or(0, |last| last + 1);
    value.get(..end).unwrap_or(value)
}

/// `swoole_http_has_crlf`, with its warnings.
fn has_crlf(value: &[u8]) -> bool {
    for &byte in value {
        if byte == b'\n' || byte == b'\r' {
            report(
                510,
                false,
                b"Header may not contain more than a single header, new line detected",
            );
            return true;
        }
        if byte == 0 {
            report(510, false, b"Header may not contain NUL bytes");
            return true;
        }
    }
    false
}

/// `http2_server_build_trailer`, as an unsealed frame (an empty block
/// without a trailer): `None` on an error.
fn build_trailer(ctx: Ctx, session: &Session) -> Option<Headers> {
    let trailer = property(ctx, ResponseArray::Trailer);
    let size = length_safe(trailer.as_ref());
    let mut headers = Headers::new(size);
    if size == 0 {
        headers.buffers.frame.resize(FRAME_HEADER, 0);
        return Some(headers);
    }
    for (key, value) in entries(trailer.as_ref()) {
        let Some(key) = key.filter(|_| !value.is_null()) else {
            continue;
        };
        let value = string_of(value)?;
        headers.add_folded(key, value.bytes());
    }
    Some(headers.deflate(session))
}

/// The body a response header announces: its bytes (null for a file sent
/// later) and length.
#[derive(Clone, Copy)]
struct Body {
    data: *const c_char,
    length: usize,
}

/// `http2_server_build_header`.
fn build_header(ctx: Ctx, session: &Session, body: Option<Body>) -> Option<Headers> {
    let zheader = property(ctx, ResponseArray::Header);
    let zcookie = property(ctx, ResponseArray::Cookie);
    let mut headers =
        Headers::new(32 + length_safe(zheader.as_ref()) + length_safe(zcookie.as_ref()));
    #[expect(
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        reason = "the native int status"
    )]
    let status = sw_rs_http2_response_status(get(ctx, Field::Status) as i32) as u32;
    set(ctx, Field::Status, status);
    #[expect(clippy::cast_possible_wrap, reason = "the native int status")]
    headers.add(b":status", decimal(&mut [0; 20], i64::from(status as i32)));

    let mut header_flags = 0;
    if zheader.is_some() {
        let can_capture =
            get(ctx, Field::AcceptCompression) != 0 && get(ctx, Field::CompressionTypes) != 0;
        let mut content_type = None;
        let mut add = |headers: &mut Headers, key: &[u8], value: &Zval| -> Option<()> {
            if value.is_null() {
                return Some(());
            }
            let owned = string_of(value)?;
            let text = rtrim(owned.bytes());
            if has_crlf(text) {
                return Some(());
            }
            let plan = header_plan(SERVER_HEADERS, key, can_capture);
            if plan.suppress != 0 {
                return Some(());
            }
            header_flags |= plan.publish_flags;
            if plan.capture_content_type != 0 {
                content_type = Some(string_of(value)?);
            }
            headers.add_folded(key, text);
            Some(())
        };
        for (key, value) in entries(zheader.as_ref()) {
            let Some(key) = key.filter(|_| !value.is_null()) else {
                continue;
            };
            if let Some(items) = value.array() {
                for (_, item) in items.entries() {
                    add(&mut headers, key, item)?;
                }
            } else {
                add(&mut headers, key, value)?;
            }
        }
        if can_capture {
            let kind = content_type
                .as_ref()
                .map_or(&b"text/html"[..], ZString::bytes);
            // SAFETY: a live context.
            let accept = ctx.compressible(kind);
            set(ctx, Field::AcceptCompression, u32::from(accept));
        }
    }

    let missing = sw_rs_http2_header_defaults(header_flags);
    if missing & MISSING_SERVER != 0 {
        headers.add(b"server", b"swoole-http-server");
    }
    if missing & MISSING_DATE != 0 {
        swoole_rs_phphttp::http2::date(|date| headers.add(b"date", date));
    }
    if missing & MISSING_CONTENT_TYPE != 0 {
        headers.add(b"content-type", b"text/html");
    }
    for (_, value) in entries(zcookie.as_ref()) {
        if let Some(cookie) = value.bytes() {
            headers.add(b"set-cookie", cookie);
        }
    }
    if let Some(body) = body {
        let mut content_length = body.length;
        if let Some((encoding, compressed)) = ctx.compress(bytes(body.data, body.length)) {
            content_length = compressed;
            headers.add(b"content-encoding", encoding.to_bytes());
        }
        headers.add(
            b"content-length",
            decimal(
                &mut [0; 20],
                i64::try_from(content_length).unwrap_or(i64::MAX),
            ),
        );
    }
    let headers = headers.deflate(session);
    set(ctx, Field::SendHeader, 1);
    Some(headers)
}

/// `rust_header_plan`: the header name folded as `std::tolower` does.
fn header_plan(role: u32, name: &[u8], can_capture: bool) -> Http2HeaderPlan {
    let (folded, length) = fold(name);
    sw_rs_http2_header_plan(
        role,
        folded.get(..length).unwrap_or_default(),
        name.len(),
        u32::from(can_capture),
    )
}

/// The first 33 bytes, lowercased (`rust_fold_header`).
fn fold(text: &[u8]) -> ([u8; 33], usize) {
    let mut folded = [0; 33];
    let length = text.len().min(33);
    for (out, byte) in folded.iter_mut().zip(text) {
        *out = byte.to_ascii_lowercase();
    }
    (folded, length)
}

/// `Stream::send_header`.
fn send_header(
    ctx: Ctx,
    session: &Session,
    stream: &Stream,
    body: Option<Body>,
    end_stream: bool,
) -> bool {
    send_header_then(ctx, session, stream, body, end_stream, |_| ())
}

/// `Stream::send_header`, sending what `then` appends to the HEADERS frame in
/// the same write.
fn send_header_then(
    ctx: Ctx,
    session: &Session,
    stream: &Stream,
    body: Option<Body>,
    end_stream: bool,
    then: impl FnOnce(&mut Vec<u8>),
) -> bool {
    let Some(mut headers) = build_header(ctx, session, body) else {
        // Upstream closes once the session has an encoder.
        if session.deflater.borrow().is_some() {
            set(ctx, Field::End, 1);
            close(ctx);
        }
        return false;
    };
    let empty = body.is_none_or(|body| body.length == 0);
    let flags = sw_rs_http2_headers_flags(u32::from(empty), u32::from(end_stream), 0);
    if flags & FLAG_END_STREAM != 0 {
        set(ctx, Field::End, 1);
    }
    let frame = headers.seal(TYPE_HEADERS, flags, stream.id);
    then(frame);
    if !send(ctx, frame) {
        set(ctx, Field::SendHeader, 0);
        return false;
    }
    true
}

/// `Stream::send_body`: DATA frames within the peer's flow-control window.
pub(crate) fn send_body(
    ctx: Ctx,
    session: &Session,
    stream: &Stream,
    data: &[u8],
    end_stream: bool,
) -> bool {
    if !current(session, stream) {
        return false;
    }
    let flags = if end_stream { FLAG_END_STREAM } else { 0 };
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    let mut rest = data;
    while !rest.is_empty() {
        // Re-read after a flow-control wait: the peer may have changed it.
        let max_frame_size = send_frame_size(session.remote[MAX_FRAME_SIZE].get());
        let chunk = sw_rs_http2_chunk(rest.len(), max_frame_size, flags);
        let (mut send_n, mut send_flags) = (chunk.length, chunk.flags);
        let window = stream.remote_window_size.get();
        if send_n > window as usize {
            // The coroutine server receives frames serially and cannot be
            // suspended, so it has no flow control.
            // SAFETY: plain reads of Swoole's state.
            let may_suspend =
                get(ctx, Field::CoSocket) == 0 && swoole_rs_phpcoroutines::in_coroutine();
            match sw_rs_http2_window_action(send_n, window, u32::from(may_suspend)) {
                1 => warn(
                    c"Stream::send_body",
                    "The data sent exceeded remote_window_size",
                ),
                2 => {
                    // In a coroutine: WINDOW_UPDATE, or the stream's removal, wakes it.
                    if !current(session, stream) {
                        return false;
                    }
                    stream.wait();
                    if !current(session, stream) {
                        return false;
                    }
                    continue;
                }
                _ => {
                    send_n = window as usize;
                    send_flags = 0;
                }
            }
        }
        let header = frame_header(TYPE_DATA, send_n, send_flags, stream.id);
        let (part, tail) = rest.split_at(send_n.min(rest.len()));
        if send_n < page {
            // One send of header and payload. Not a zeroed stack buffer: zeroing
            // it costs more than the allocation (#331).
            let frame = [header.as_slice(), part].concat();
            if !send(ctx, &frame) {
                return false;
            }
        } else if !send(ctx, &header) || !send(ctx, part) {
            return false;
        }
        rest = tail;
        consume_window(session, stream, send_n);
    }
    true
}

/// The windows after `sent` bytes of DATA.
fn consume_window(session: &Session, stream: &Stream, sent: usize) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native uint32_t window arithmetic"
    )]
    let sent = sent as u32;
    stream
        .remote_window_size
        .set(stream.remote_window_size.get().wrapping_sub(sent));
    session
        .remote_window_size
        .set(session.remote_window_size.get().wrapping_sub(sent));
}

/// Append `data` to `frame` as the one DATA frame `send_body` would send in
/// one write (the stream's window holds it, one frame carries it, and it is
/// below a page), and say how many bytes that was; 0 when it isn't. A small
/// response's HEADERS and DATA then take one write and one segment, as
/// upstream's HTTP/1 header and small body do.
fn append_body(
    frame: &mut Vec<u8>,
    session: &Session,
    stream: &Stream,
    data: &[u8],
    end_stream: bool,
) -> usize {
    let max_frame_size = send_frame_size(session.remote[MAX_FRAME_SIZE].get()) as usize;
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    // An empty body sends nothing: the HEADERS end the stream.
    if data.is_empty()
        || data.len() > max_frame_size.min(page - 1)
        || data.len() > stream.remote_window_size.get() as usize
    {
        return 0;
    }
    let flags = if end_stream { FLAG_END_STREAM } else { 0 };
    frame.extend_from_slice(&frame_header(TYPE_DATA, data.len(), flags, stream.id));
    frame.extend_from_slice(data);
    data.len()
}

/// `Stream::send_trailer`: HEADERS with `END_STREAM`, even an empty block.
fn send_trailer(ctx: Ctx, session: &Session, stream_id: u32) -> bool {
    let Some(mut headers) = build_trailer(ctx, session) else {
        return false;
    };
    send(
        ctx,
        headers.seal(TYPE_HEADERS, FLAG_END_HEADERS | FLAG_END_STREAM, stream_id),
    )
}

/// Whether the response has a trailer.
fn has_trailer(ctx: Ctx) -> bool {
    length_safe(property(ctx, ResponseArray::Trailer).as_ref()) > 0
}

/// `http2_server_respond`: the header, the body and the trailer, then the
/// stream is done.
pub(crate) fn respond(ctx: Ctx, data: &[u8]) -> bool {
    let Some((session, stream)) = of(ctx) else {
        return false;
    };
    let trailer = has_trailer(ctx);
    let end_stream = !trailer;
    let mut body: Vec<u8> = Vec::new();
    let mut compressed = false;
    // Whether the body went out with the header.
    let mut body_sent = false;
    let mut facts = Http2RespondFacts {
        header_sent: get(ctx, Field::SendHeader),
        trailer_present: u32::from(trailer),
        chunked: 0,
        shutting_down: 0,
        streams_empty: 0,
        body_length: 0,
    };
    let next = |action, ok: bool, facts: &Http2RespondFacts| {
        sw_rs_http2_respond_next(action, u32::from(ok), facts)
    };
    let mut action = next(RESP_START, true, &facts);
    loop {
        match action {
            RESP_HEADER => {
                let announced = Body {
                    data: data.as_ptr().cast(),
                    length: data.len(),
                };
                let mut appended = 0;
                let sent = send_header_then(
                    ctx,
                    &session,
                    &stream,
                    Some(announced),
                    end_stream,
                    |frame| {
                        // The header decided the compression.
                        if get(ctx, Field::ContentCompressed) != 0 {
                            body = ctx.compressed();
                            compressed = true;
                        }
                        let payload = if compressed { &body[..] } else { data };
                        appended = append_body(frame, &session, &stream, payload, end_stream);
                    },
                );
                if sent {
                    consume_window(&session, &stream, appended);
                }
                body_sent = appended > 0;
                action = next(action, sent, &facts);
            }
            RESP_COMMIT_END => {
                // Headers are sent: body failures must not permit a retry.
                set(ctx, Field::End, 1);
                if !compressed && get(ctx, Field::ContentCompressed) != 0 {
                    body = ctx.compressed();
                    compressed = true;
                }
                facts.chunked = get(ctx, Field::SendChunked);
                facts.body_length = if compressed { body.len() } else { data.len() };
                action = next(action, true, &facts);
            }
            RESP_EMPTY => {
                let sent = send(ctx, &frame_header(TYPE_DATA, 0, FLAG_END_STREAM, stream.id));
                action = next(action, sent, &facts);
            }
            RESP_BODY => {
                let payload = if compressed { &body[..] } else { data };
                // `send_body`'s check of the stream, at the same point.
                let sent = if body_sent {
                    current(&session, &stream)
                } else {
                    send_body(ctx, &session, &stream, payload, end_stream)
                };
                action = next(action, sent, &facts);
            }
            RESP_TRAILER => {
                let sent = send_trailer(ctx, &session, stream.id);
                action = next(action, sent, &facts);
            }
            RESP_ERROR_CLOSE | RESP_REMOVE => {
                if action == RESP_ERROR_CLOSE {
                    // Not when the peer reset the stream or closed meanwhile.
                    if session.stream(stream.id, false).is_some() {
                        close(ctx);
                    }
                } else {
                    session.remove(stream.id);
                }
                facts.shutting_down = u32::from(session.shutting_down.get());
                facts.streams_empty = u32::from(facts.shutting_down != 0 && session.idle());
                action = next(action, true, &facts);
            }
            RESP_ERROR_SHUTDOWN | RESP_SUCCESS_SHUTDOWN => {
                close(ctx);
                return action == RESP_SUCCESS_SHUTDOWN;
            }
            RESP_TRUE => return true,
            _ => return false,
        }
    }
}

/// `swoole_http2_server_write`: a chunk, the header first.
fn write(ctx: Ctx, chunk: &[u8]) -> bool {
    if chunk.is_empty() {
        report(NO_PAYLOAD, false, b"the data sent must not be empty");
        return false;
    }
    let Some((session, stream)) = of(ctx) else {
        return false;
    };
    set(ctx, Field::SendChunked, 1);
    if get(ctx, Field::SendHeader) == 0 && !send_header(ctx, &session, &stream, None, false) {
        return false;
    }
    send_body(ctx, &session, &stream, chunk, false)
}

pub(crate) fn begin_file(ctx: Ctx, content_type: &[u8], length: usize) -> c_int {
    let Some((session, stream)) = of(ctx) else {
        return -1;
    };
    set(ctx, Field::AcceptCompression, 0);
    let trailer = has_trailer(ctx);
    let zheader = property(ctx, ResponseArray::Header);
    let typed = zheader
        .as_ref()
        .and_then(|zheader| zheader.array())
        .is_some_and(|zheader| zheader.find(b"content-type").is_some());
    if !typed {
        ctx.set_header(b"content-type", content_type);
    }
    let body = Body {
        data: ptr::null(),
        length,
    };
    if !send_header(ctx, &session, &stream, Some(body), !trailer) {
        return -1;
    }
    // Headers are sent: retries are no longer allowed, even if the body fails.
    set(ctx, Field::End, 1);
    c_int::from(trailer)
}

/// The trailer, then the stream is done: false after an error.
pub(crate) fn finish_file(ctx: Ctx, trailer: bool, error: bool) -> bool {
    let Some(session) = SessionState::of(ctx) else {
        return false;
    };
    let stream_id = get(ctx, Field::StreamId);
    let mut action = sw_rs_http2_file_complete(u32::from(!error), u32::from(trailer));
    loop {
        match action {
            FILE_TRAILER => {
                let sent = send_trailer(ctx, &session, stream_id);
                action = sw_rs_http2_file_complete(u32::from(sent), 0);
            }
            FILE_CLOSE => {
                // Not when the peer reset the stream or closed meanwhile.
                if session.stream(stream_id, false).is_some() {
                    close(ctx);
                }
                return false;
            }
            _ => {
                session.remove(stream_id);
                return true;
            }
        }
    }
}

// --- receiving ---

/// `swoole_http2_server_onReceive`: an async server's data on an HTTP/2
/// connection, whose session (and settings context) the first data makes.
fn on_receive(view: view::Server<'_>, conn: &ConnectionRecord, request: &RecvData) -> c_int {
    let session_id = request.info().fd;
    let session = SessionState::find(session_id).or_else(|| {
        let default_ctx = Ctx::server_context(view, session_id);
        let max_body_size = swoole_rs_server::server_methods::package_max_length(view, conn);
        crate::session::session_new(
            session_id,
            default_ctx,
            max_body_size,
            false,
            Box::new(|ctx, _| swoole_rs_phphttp::http2_request(ctx)),
        );
        SessionState::find(session_id)
    });
    // The empty slot `zdata` owns takes a new string.
    let zdata = Owned::default();
    swoole_rs_phpserver::php_swoole_get_recv_data(view, &zdata, request);
    let data = zdata.bytes().unwrap_or_default();
    // The session stays alive until its frame is handled.
    let result = session.map_or(SW_ERR, |session| parse(&session, data.as_ptr()));
    // Released before a deferred bailout can jump past it.
    drop(zdata);
    bailout::finish();
    result
}

/// `php_swoole_http2_server_onClose`: the connection's session goes.
pub fn server_on_close(session_id: i64) {
    crate::session::session_free(session_id);
}

/// `swoole_http2_server_parse` for the coroutine server's connection loop: one
/// frame from the peer of connection `fd`, which `frame` starts with; a bailout
/// stays pending for the caller's `finish`.
fn session_receive(fd: i64, frame: &[u8]) -> c_int {
    SessionState::find(fd).map_or(SW_ERR, |session| parse(&session, frame.as_ptr()))
}

/// The entry points crates/phphttp's HTTP/1 code calls.
pub(crate) static HOOKS: swoole_rs_phphttp::http2::Server = swoole_rs_phphttp::http2::Server {
    on_receive,
    static_file: crate::files::static_file,
    respond,
    write,
    ping,
    send_file: crate::files::send_file,
    goaway,
    reset,
    session_new: crate::session::session_new,
    session_receive,
    session_free: crate::session::session_free,
};

fn parse(session: &Session, buf: *const u8) -> c_int {
    let last_stream_id = session.last_stream_id.get();
    // SAFETY: nine readable header bytes.
    let head = unsafe { std::slice::from_raw_parts(buf, FRAME_HEADER) };
    let facts = sw_rs_http2_decode_frame(head, last_stream_id, 0).unwrap_or_default();
    let stream_id = facts.stream_id;
    let new_stream = facts.publish_last_id != 0;
    if facts.kind == TYPE_HEADERS && new_stream {
        session.last_stream_id.set(stream_id);
    }
    // A header block continues only in CONTINUATION frames of its own stream.
    let pending = session.header_stream_id.get();
    if pending != 0 && (facts.kind != TYPE_CONTINUATION || stream_id != pending) {
        return SW_ERR;
    }
    if session.shutting_down.get() {
        log(
            LOG_WARNING,
            STREAM_IGNORE,
            c"swoole_http2_server_parse",
            &format!("ignore http2 stream#{stream_id} after sending goaway"),
        );
        return SW_ERR;
    }
    let length = facts.length as usize;
    // SAFETY: the payload follows the header in the frame.
    let payload = unsafe { buf.add(FRAME_HEADER) };
    let flags = facts.flags & 0xff;
    match facts.action {
        ACTION_SETTINGS => return settings(session, payload, length),
        ACTION_HEADERS => return headers(session, stream_id, flags, payload, length, new_stream),
        _ if facts.kind == TYPE_CONTINUATION => {
            let stream = session.stream(stream_id, false);
            let Some(stream) = stream.filter(|_| pending == stream_id) else {
                return SW_ERR;
            };
            return header_block(session, &stream, flags & FLAG_END_HEADERS, payload, length);
        }
        ACTION_DATA => return data(session, stream_id, flags, payload, length),
        ACTION_PING_REPLY => {
            let mut frame = [0; FRAME_HEADER + PING_PAYLOAD];
            frame[..FRAME_HEADER].copy_from_slice(&frame_header(
                TYPE_PING,
                PING_PAYLOAD,
                FLAG_ACK,
                stream_id,
            ));
            // SAFETY: the frame's eight payload bytes.
            frame[FRAME_HEADER..]
                .copy_from_slice(unsafe { std::slice::from_raw_parts(payload, PING_PAYLOAD) });
            send(session.default_ctx, &frame);
        }
        ACTION_WINDOW_UPDATE => {
            let value = read_u32(payload);
            if stream_id == 0 {
                session
                    .remote_window_size
                    .set(credit(session.remote_window_size.get(), value));
            } else if let Some(stream) = session.stream(stream_id, false) {
                stream
                    .remote_window_size
                    .set(credit(stream.remote_window_size.get(), value));
                if !session.is_coro {
                    stream.wake();
                }
            }
        }
        ACTION_RESET => session.remove(stream_id),
        _ => {}
    }
    SW_OK
}

/// A HEADERS frame: a new stream's request headers, or its trailers, which
/// are decoded (the HPACK table is the connection's) and discarded.
fn headers(
    session: &Session,
    stream_id: u32,
    flags: u32,
    payload: *const u8,
    length: usize,
    new_stream: bool,
) -> c_int {
    let stream = match session.stream(stream_id, false) {
        Some(stream) => stream,
        // A completed stream is never recreated from a late trailer block.
        None if new_stream => match session.stream(stream_id, true) {
            Some(stream) => stream,
            None => return SW_ERR,
        },
        None => return SW_ERR,
    };
    session.header_end_stream.set(flags & FLAG_END_STREAM != 0);
    let (mut payload, mut length) = (payload, length);
    if flags & FLAG_PADDED != 0 {
        let Some(&pad) = bytes(payload.cast(), length).first() else {
            return SW_ERR;
        };
        let pad = usize::from(pad);
        if pad >= length {
            return SW_ERR;
        }
        length -= pad + 1;
        payload = payload.wrapping_add(1);
    }
    header_block(session, &stream, flags, payload, length)
}

/// One HEADERS or CONTINUATION frame's part of a header block. At the block's
/// end the request is configured from its headers and, with `END_STREAM`,
/// dispatched.
fn header_block(
    session: &Session,
    stream: &Stream,
    flags: u32,
    payload: *const u8,
    length: usize,
) -> c_int {
    if stream.request_received.get() {
        return SW_ERR;
    }
    let discard = stream.headers_received.get();
    if parse_header(session, stream, flags, payload, length, discard) < 0 {
        return SW_ERR;
    }
    if flags & FLAG_END_HEADERS == 0 {
        session.header_stream_id.set(stream.id);
        return SW_OK;
    }
    session.header_stream_id.set(0);
    if !discard && !headers_complete(stream) {
        return SW_ERR;
    }
    stream.headers_received.set(true);
    if session.header_end_stream.get() {
        return complete(session, stream);
    }
    SW_OK
}

/// `std::strtol` consumed the whole Content-Length and it is within the body limit.
fn content_length_valid(session: &Session, value: &[u8]) -> bool {
    let (parsed, complete) = strtol(value);
    sw_rs_http2_content_length_rejected(u32::from(complete), parsed, session.max_body_size) == 0
}

/// `http2_server_on_headers_complete`: the request's Content-Length, body
/// parser and compression, from its whole initial header block.
fn headers_complete(stream: &Stream) -> bool {
    let ctx = stream.ctx;
    if stream.bad_length.get() {
        send_status_code(ctx, ENTITY_TOO_LARGE);
        return false;
    }
    if !stream.negotiates.get() {
        return true;
    }
    if let Some(value) = header(ctx, b"content-type") {
        let (folded, length) = fold(value.bytes());
        let media = sw_rs_http2_media_kind(folded.get(..length).unwrap_or_default());
        if media == MEDIA_URLENCODED {
            set(ctx, Field::Form, 1);
        } else if media == MEDIA_MULTIPART
            // SAFETY: a live context and the header value.
            && !ctx.multipart_start(value.bytes())
        {
            return false;
        }
    }
    if get(ctx, Field::EnableCompression) != 0
        && let Some(value) = header(ctx, b"accept-encoding")
    {
        // SAFETY: a live context.
        ctx.compression_method(value.bytes());
    }
    true
}

/// `http2_server_complete_request`: parse the buffered body and dispatch the
/// request, once.
fn complete(session: &Session, stream: &Stream) -> c_int {
    if stream.request_received.replace(true) {
        return SW_ERR;
    }
    let ctx = stream.ctx;
    if ctx.has_body() {
        if get(ctx, Field::ParseBody) != 0 && get(ctx, Field::Form) != 0 {
            ctx.treat_post();
        } else if get(ctx, Field::Multipart) != 0
            // SAFETY: a live context with a multipart parser.
            && !ctx.multipart()
        {
            return SW_ERR;
        }
    }
    session.handle(stream);
    SW_OK
}

/// The request header `key` as a string, if it has one.
fn header(ctx: Ctx, key: &[u8]) -> Option<ZString> {
    let entry = ctx.entry(Value::Header, key)?;
    string_of(&entry)
}

fn read_u32(at: *const u8) -> u32 {
    // SAFETY: four readable bytes of the frame.
    u32::from_be_bytes(unsafe { at.cast::<[u8; 4]>().read_unaligned() })
}

fn window(current: u32, amount: usize, initial: u32, credit: bool) -> Http2WindowPlan {
    sw_rs_http2_window_plan(current, amount, initial, u32::from(credit))
}

fn credit(current: u32, amount: u32) -> u32 {
    window(current, amount as usize, 0, true).value
}

/// A SETTINGS frame: apply each, then acknowledge.
fn settings(session: &Session, mut at: *const u8, length: usize) -> c_int {
    let mut left = isize::try_from(length).unwrap_or(isize::MAX);
    while left > 0 {
        // SAFETY: six readable bytes, as upstream reads them.
        let entry = unsafe { at.cast::<[u8; 6]>().read_unaligned() };
        let (id, value) = swoole_rs_http2::sw_rs_http2_unpack_setting(&entry);
        let mut deflater = session.deflater.borrow_mut();
        let old = if id == SETTING_HEADER_TABLE_SIZE {
            session.remote[0].get()
        } else {
            0
        };
        let change = id == SETTING_HEADER_TABLE_SIZE && value != old && deflater.is_some();
        let plan = sw_rs_http2_setting_plan(u32::from(id), value, old, u32::from(change), 1);
        if plan.set_value != 0
            && let Ok(field) = usize::try_from(plan.field)
            && let Some(setting) = session.remote.get(field)
        {
            setting.set(value);
        }
        if plan.set_window != 0 {
            session.remote_window_size.set(value);
        }
        if plan.change_deflater != 0
            && let Some(encoder) = deflater.as_mut()
        {
            encoder.set_table_size(value);
        }
        drop(deflater);
        // SAFETY: the next setting, as upstream steps.
        at = unsafe { at.add(6) };
        left -= 6;
    }
    // The client assumes the server has not applied its settings until it acknowledges them.
    send(
        session.default_ctx,
        &frame_header(TYPE_SETTINGS, 0, FLAG_ACK, 0),
    );
    SW_OK
}

/// A DATA frame: buffer the body, keep the windows open and dispatch the
/// request at the end of the stream.
fn data(session: &Session, stream_id: u32, flags: u32, payload: *const u8, length: usize) -> c_int {
    let Some(stream) = session.stream(stream_id, false) else {
        log(
            LOG_WARNING,
            STREAM_NOT_FOUND,
            c"swoole_http2_server_parse",
            &format!("http2 stream#{stream_id} not found"),
        );
        return SW_ERR;
    };
    if !stream.headers_received.get() || stream.request_received.get() {
        return SW_ERR;
    }
    let ctx = stream.ctx;
    if length > 0 {
        let buffered = ctx.data_start();
        let ended = get(ctx, Field::End);
        if sw_rs_http2_data_rejected(buffered, length, session.max_body_size, ended) != 0 {
            reset(ctx, stream_id, 0);
            session.remove(stream_id);
            return SW_OK;
        }
        // SAFETY: a live context and the frame's payload.
        ctx.data_append(bytes(payload.cast(), length));
        flow(session, &stream, ctx, stream_id, length);
    }
    if flags & FLAG_END_STREAM != 0 {
        return complete(session, &stream);
    }
    SW_OK
}

/// Flow control: debit both receive windows, and reopen each once a quarter
/// remains.
fn flow(session: &Session, stream: &Stream, ctx: Ctx, stream_id: u32, length: usize) {
    let initial = session.local(INIT_WINDOW_SIZE);
    let connection = window(session.local_window_size.get(), length, initial, false);
    session.local_window_size.set(connection.value);
    let debited = window(stream.local_window_size.get(), length, 0, false).value;
    stream.local_window_size.set(debited);
    if connection.increment != 0 {
        window_update(ctx, 0, connection.increment);
        session.local_window_size.set(initial);
    }
    let reopen = window(debited, 0, initial, false);
    if reopen.increment != 0 {
        window_update(ctx, stream_id, reopen.increment);
        stream.local_window_size.set(initial);
    }
}

/// `http2_server_parse_header`: a HEADERS block into the request's
/// `$server`, `$header`, `$get` and `$cookie`.
fn parse_header(
    session: &Session,
    stream: &Stream,
    flags: u32,
    input: *const u8,
    length: usize,
    discard: bool,
) -> c_int {
    // SAFETY: the frame's `length` payload bytes.
    let mut input = unsafe { std::slice::from_raw_parts(input, length) };
    if flags & FLAG_PRIORITY != 0 {
        // The stream dependency and weight are ignored.
        let Some(block) = input.get(5..) else {
            return SW_ERR;
        };
        input = block;
    }
    let zheader = zval_of(stream.ctx, Value::Header);
    let zserver = zval_of(stream.ctx, Value::Server);
    let mut inflater = session.inflater.borrow_mut();
    loop {
        let step = match inflater.decode(input, flags & FLAG_END_HEADERS != 0) {
            Ok(step) => step,
            Err(error) => {
                let code = Error::CODE;
                let text = format!("inflate failed, Error: {error}[{code}]");
                warn(c"http2_server_parse_header", &text);
                return SW_ERR;
            }
        };
        input = input.get(step.read..).unwrap_or_default();
        let inflate_flags = step.flags();
        // Trailers are decoded for the HPACK table but never exposed or acted on.
        if let Some((name, value)) = step.header.filter(|_| !discard)
            && !request_header(session, stream, zheader, zserver, name, value)
        {
            continue;
        }
        let step = sw_rs_http2_inflate_step(inflate_flags, input.len(), 0);
        if step.end_headers != 0 {
            inflater.end_headers();
            break;
        }
        if step.finish != 0 {
            break;
        }
    }
    SW_OK
}

fn add_string(array: *mut zval, key: &[u8], value: &[u8]) {
    // SAFETY: a live array; Zend copies both.
    unsafe {
        add_assoc_stringl_ex(
            array,
            key.as_ptr().cast(),
            key.len(),
            value.as_ptr().cast(),
            value.len(),
        );
    };
}

/// One request header; `false` skips the rest of the step (a Cookie, parsed
/// into `$cookie` and never kept in `$header`, or a Content-Length, checked
/// here and judged once the block is complete).
fn request_header(
    session: &Session,
    stream: &Stream,
    zheader: *mut zval,
    zserver: *mut zval,
    name: &[u8],
    value: &[u8],
) -> bool {
    let ctx = stream.ctx;
    let kind = header_plan(REQUEST_HEADERS, name, false).kind;
    if name.first() == Some(&b':') {
        match kind {
            HEADER_METHOD => add_string(zserver, b"request_method", value),
            HEADER_PATH => path(ctx, zserver, value),
            HEADER_AUTHORITY => add_string(zheader, b"host", value),
            _ => {}
        }
        return true;
    }
    match kind {
        HEADER_COOKIE => {
            // SAFETY: the request's `$cookie`.
            ctx.cookie(value);
            false
        }
        HEADER_CONTENT_LENGTH => {
            if !content_length_valid(session, value) {
                stream.bad_length.set(true);
            }
            // The header keeps the last value.
            add_string(zheader, b"content-length", value);
            false
        }
        _ => {
            if matches!(kind, HEADER_CONTENT_TYPE | HEADER_ACCEPT_ENCODING) {
                stream.negotiates.set(true);
            }
            add_string(zheader, name, value);
            true
        }
    }
}

/// `:path`: `request_uri`, `path_info` (decoded), `query_string` and `$get`.
fn path(ctx: Ctx, zserver: *mut zval, value: &[u8]) {
    let question = value.iter().position(|&byte| byte == b'?');
    let path = match question.and_then(|question| value.split_at_checked(question)) {
        Some((path, query)) => {
            let query = query.get(1..).unwrap_or_default();
            add_string(zserver, b"query_string", query);
            // SAFETY: the request's `$get`.
            ctx.treat_query(Value::Get, query);
            path
        }
        None => value,
    };
    // SAFETY: a live context without a path.
    ctx.set_path(value);
    // SAFETY: a live array taking the new string.
    unsafe { add_assoc_str_ex(zserver, c"request_uri".as_ptr(), 11, string::copy(path)) };
    let decoded = string::copy(path);
    // SAFETY: a fresh string of `path.len()` bytes, decoded in place.
    let length = unsafe { swr_zend_url_decode(string::val(decoded).cast(), path.len()) };
    // SAFETY: as above; decoding only shortens.
    unsafe { string::truncate(decoded, length) };
    // SAFETY: a live array taking the string.
    unsafe { add_assoc_str_ex(zserver, c"path_info".as_ptr(), 9, decoded) };
}

/// `std::strtol(value, &end, 10)` and whether it consumed the whole value.
fn strtol(value: &[u8]) -> (i64, bool) {
    match swoole_rs_corebase::strtol(value) {
        (_, 0) => (0, value.is_empty()),
        (parsed, end) => (parsed, end == value.len()),
    }
}

/// `http2_server_send_status_code`: a header-only response.
fn send_status_code(ctx: Ctx, status: u32) {
    if let Some((session, stream)) = of(ctx) {
        set(ctx, Field::Status, status);
        send_header(ctx, &session, &stream, None, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strtol_matches_libc() {
        assert_eq!(strtol(b"123"), (123, true));
        assert_eq!(strtol(b" 12"), (12, true));
        assert_eq!(strtol(b"12a"), (12, false));
        assert_eq!(strtol(b"-5"), (-5, true));
        assert_eq!(strtol(b"abc"), (0, false));
        assert_eq!(strtol(b""), (0, true));
        assert_eq!(strtol(b"99999999999999999999999"), (i64::MAX, true));
    }

    #[test]
    fn rtrim_drops_trailing_space_only() {
        assert_eq!(rtrim(b" a b \t\r\n"), b" a b");
        assert_eq!(rtrim(b"  "), b"");
    }

    #[test]
    fn frames_are_big_endian() {
        assert_eq!(
            frame_header(TYPE_PING, 8, FLAG_ACK, 3),
            [0, 0, 8, 6, 1, 0, 0, 0, 3]
        );
    }
}
