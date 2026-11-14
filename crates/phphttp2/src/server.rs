//! The HTTP/2 server's frames, responses and request header blocks: upstream's
//! `swoole_http2_server_parse` and everything it and `Response` send (headers
//! over crates/http2's HPACK, bodies under flow control, trailers, files), over
//! the sessions and streams of `session`, and an async server's
//! `onReceive` and `onClose` for its HTTP/2 connections. The HTTP contexts are
//! crates/phphttp's.
use crate::session::{Session, SessionState, Stream};
use libc::c_int;
use std::ffi::{CStr, CString};
use swoole_rs_corebase::CCast;
use swoole_rs_http2::{
    FileCompletion, FrameAction, FrameType, HeaderKind, HeaderRole, Http2HeaderPlan,
    Http2RespondFacts, Http2WindowPlan, Media, Respond, WindowAction,
    hpack::{Encoder, Error},
    send_frame_size, sw_rs_http2_chunk, sw_rs_http2_content_length_rejected,
    sw_rs_http2_data_rejected, sw_rs_http2_decode_frame, sw_rs_http2_file_complete,
    sw_rs_http2_header_defaults, sw_rs_http2_header_plan, sw_rs_http2_headers_flags,
    sw_rs_http2_inflate_step, sw_rs_http2_media_kind, sw_rs_http2_pack_frame_header,
    sw_rs_http2_respond_next, sw_rs_http2_response_status, sw_rs_http2_setting_plan,
    sw_rs_http2_window_action, sw_rs_http2_window_plan,
};
use swoole_rs_phpserver::RecvData;
use swoole_rs_server::{Connection, view};
use swoole_rs_zend::{Array, HeldArray, Value as Zval, ZString, bailout};

/// Frame sizes, types and flags (`swoole_http2.h`).
const FRAME_HEADER: usize = 9;
const PING_PAYLOAD: usize = 8;
const GOAWAY_SIZE: usize = 8;
const FLAG_ACK: u32 = 0x1;
const FLAG_END_STREAM: u32 = 0x1;
const FLAG_END_HEADERS: u32 = 0x4;
const FLAG_PADDED: u32 = 0x8;
const FLAG_PRIORITY: u32 = 0x20;
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
const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;

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
fn frame_header(kind: FrameType, length: usize, flags: u32, stream_id: u32) -> [u8; FRAME_HEADER] {
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
    kind: FrameType,
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
    send_u32_frame(ctx, FrameType::WindowUpdate, 4, 0, stream_id, size);
}

/// `Stream::reset`.
pub(crate) fn reset(ctx: Ctx, stream_id: u32, code: u32) {
    send_u32_frame(ctx, FrameType::RstStream, 4, 0, stream_id, code);
}

/// `swoole_http2_server_ping`.
fn ping(ctx: Ctx) -> bool {
    let mut frame = [0; FRAME_HEADER + PING_PAYLOAD];
    frame[..FRAME_HEADER].copy_from_slice(&frame_header(FrameType::Ping, PING_PAYLOAD, 0, 0));
    send(ctx, &frame)
}

/// `swoole_http2_server_goaway`.
fn goaway(ctx: Ctx, code: i64, data: &[u8]) -> bool {
    let Some(session) = SessionState::of(ctx) else {
        return false;
    };
    let last_stream_id = session.last_stream_id.get();
    let (flags, code) = (u32::from(code.c_cast::<u8>()), code.c_cast::<u32>());
    let mut frame = Vec::with_capacity(FRAME_HEADER + GOAWAY_SIZE + data.len());
    frame.extend_from_slice(&frame_header(
        FrameType::Goaway,
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
    fn seal(&mut self, kind: FrameType, flags: u32, stream_id: u32) -> &mut Vec<u8> {
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

/// The body a response header announces: its bytes (none for a file sent
/// later) and length.
#[derive(Clone, Copy)]
struct Body<'a> {
    data: &'a [u8],
    length: usize,
}

/// `http2_server_build_header`.
fn build_header(ctx: Ctx, session: &Session, body: Option<Body<'_>>) -> Option<Headers> {
    let zheader = property(ctx, ResponseArray::Header);
    let zcookie = property(ctx, ResponseArray::Cookie);
    let mut headers =
        Headers::new(32 + length_safe(zheader.as_ref()) + length_safe(zcookie.as_ref()));
    let status =
        sw_rs_http2_response_status(get(ctx, Field::Status).c_cast::<i32>()).c_cast::<u32>();
    set(ctx, Field::Status, status);
    headers.add(
        b":status",
        decimal(&mut [0; 20], i64::from(status.c_cast::<i32>())),
    );

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
            let plan = header_plan(HeaderRole::Server, key, can_capture);
            if plan.suppress {
                return Some(());
            }
            header_flags |= plan.publish_flags;
            if plan.capture_content_type {
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
        if let Some((encoding, compressed)) = ctx.compress(body.data) {
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
fn header_plan(role: HeaderRole, name: &[u8], can_capture: bool) -> Http2HeaderPlan {
    let (folded, length) = fold(name);
    sw_rs_http2_header_plan(
        role,
        folded.get(..length).unwrap_or_default(),
        name.len(),
        can_capture,
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
    body: Option<Body<'_>>,
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
    body: Option<Body<'_>>,
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
    let flags = sw_rs_http2_headers_flags(empty, end_stream, 0);
    if flags & FLAG_END_STREAM != 0 {
        set(ctx, Field::End, 1);
    }
    let frame = headers.seal(FrameType::Headers, flags, stream.id);
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
            let may_suspend =
                get(ctx, Field::CoSocket) == 0 && swoole_rs_phpcoroutines::in_coroutine();
            match sw_rs_http2_window_action(send_n, window, may_suspend) {
                WindowAction::Warn => warn(
                    c"Stream::send_body",
                    "The data sent exceeded remote_window_size",
                ),
                WindowAction::Yield => {
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
                WindowAction::Send | WindowAction::Truncate => {
                    send_n = window as usize;
                    send_flags = 0;
                }
            }
        }
        let header = frame_header(FrameType::Data, send_n, send_flags, stream.id);
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
    let sent = sent.c_cast::<u32>();
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
    frame.extend_from_slice(&frame_header(FrameType::Data, data.len(), flags, stream.id));
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
        headers.seal(
            FrameType::Headers,
            FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id,
        ),
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
        header_sent: get(ctx, Field::SendHeader) != 0,
        trailer_present: trailer,
        chunked: false,
        shutting_down: false,
        streams_empty: false,
        body_length: 0,
    };
    let next =
        |action, ok: bool, facts: &Http2RespondFacts| sw_rs_http2_respond_next(action, ok, facts);
    let mut action = next(Respond::Start, true, &facts);
    loop {
        match action {
            Respond::Header => {
                let announced = Body {
                    data,
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
            Respond::CommitEnd => {
                // Headers are sent: body failures must not permit a retry.
                set(ctx, Field::End, 1);
                if !compressed && get(ctx, Field::ContentCompressed) != 0 {
                    body = ctx.compressed();
                    compressed = true;
                }
                facts.chunked = get(ctx, Field::SendChunked) != 0;
                facts.body_length = if compressed { body.len() } else { data.len() };
                action = next(action, true, &facts);
            }
            Respond::Empty => {
                let sent = send(
                    ctx,
                    &frame_header(FrameType::Data, 0, FLAG_END_STREAM, stream.id),
                );
                action = next(action, sent, &facts);
            }
            Respond::Body => {
                let payload = if compressed { &body[..] } else { data };
                // `send_body`'s check of the stream, at the same point.
                let sent = if body_sent {
                    current(&session, &stream)
                } else {
                    send_body(ctx, &session, &stream, payload, end_stream)
                };
                action = next(action, sent, &facts);
            }
            Respond::Trailer => {
                let sent = send_trailer(ctx, &session, stream.id);
                action = next(action, sent, &facts);
            }
            Respond::ErrorClose | Respond::Remove => {
                if action == Respond::ErrorClose {
                    // Not when the peer reset the stream or closed meanwhile.
                    if session.stream(stream.id, false).is_some() {
                        close(ctx);
                    }
                } else {
                    session.remove(stream.id);
                }
                facts.shutting_down = session.shutting_down.get();
                facts.streams_empty = facts.shutting_down && session.idle();
                action = next(action, true, &facts);
            }
            Respond::ErrorShutdown | Respond::SuccessShutdown => {
                close(ctx);
                return action == Respond::SuccessShutdown;
            }
            Respond::True => return true,
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
    // The held array's reference ends here: a write to a shared array asserts.
    let typed = property(ctx, ResponseArray::Header)
        .as_ref()
        .and_then(|zheader| zheader.array())
        .is_some_and(|zheader| zheader.find(b"content-type").is_some());
    if !typed {
        ctx.set_header(b"content-type", content_type);
    }
    let body = Body { data: &[], length };
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
    let mut action = sw_rs_http2_file_complete(!error, trailer);
    loop {
        match action {
            FileCompletion::Trailer => {
                let sent = send_trailer(ctx, &session, stream_id);
                action = sw_rs_http2_file_complete(sent, false);
            }
            FileCompletion::Close => {
                // Not when the peer reset the stream or closed meanwhile.
                if session.stream(stream_id, false).is_some() {
                    close(ctx);
                }
                return false;
            }
            FileCompletion::Remove => {
                session.remove(stream_id);
                return true;
            }
        }
    }
}

// --- receiving ---

/// `swoole_http2_server_onReceive`: an async server's data on an HTTP/2
/// connection, whose session (and settings context) the first data makes.
fn on_receive(view: view::Server<'_>, conn: Connection<'_>, request: &RecvData) -> c_int {
    let session_id = request.info().fd;
    let session = SessionState::find(session_id).or_else(|| {
        let default_ctx = Ctx::server_context(view, session_id);
        let max_body_size = swoole_rs_server::server_methods::package_max_length(view, conn.fd());
        crate::session::session_own(
            session_id,
            default_ctx,
            max_body_size,
            Box::new(|ctx, _| swoole_rs_phphttp::http2_request(ctx)),
        );
        SessionState::find(session_id)
    });
    let zdata = swoole_rs_phpserver::php_swoole_get_recv_data(view, request);
    let data = zdata.bytes().unwrap_or_default();
    // The session stays alive until its frame is handled.
    let parsed = session.is_some_and(|session| parse(&session, data));
    // Released before a deferred bailout can jump past it.
    drop(zdata);
    bailout::finish();
    if parsed { SW_OK } else { SW_ERR }
}

/// `php_swoole_http2_server_onClose`: the connection's session goes.
pub fn server_on_close(session_id: i64) {
    crate::session::session_free(session_id);
}

/// `swoole_http2_server_parse` for the coroutine server's connection loop: one
/// frame from the peer of connection `fd`, which `frame` starts with; a bailout
/// stays pending for the caller's `finish`.
fn session_receive(fd: i64, frame: &[u8]) -> bool {
    SessionState::find(fd).is_some_and(|session| parse(&session, frame))
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

/// One frame, which `frame` starts with: false for one shorter than its header
/// says, or one the session refuses.
fn parse(session: &Session, frame: &[u8]) -> bool {
    let last_stream_id = session.last_stream_id.get();
    let Some(facts) = sw_rs_http2_decode_frame(frame, last_stream_id, false) else {
        return false;
    };
    let stream_id = facts.stream_id;
    let new_stream = facts.publish_last_id;
    if facts.action == FrameAction::Headers && new_stream {
        session.last_stream_id.set(stream_id);
    }
    // A header block continues only in CONTINUATION frames of its own stream.
    let pending = session.header_stream_id.get();
    if pending != 0 && (facts.action != FrameAction::Continuation || stream_id != pending) {
        return false;
    }
    if session.shutting_down.get() {
        log(
            LOG_WARNING,
            STREAM_IGNORE,
            c"swoole_http2_server_parse",
            &format!("ignore http2 stream#{stream_id} after sending goaway"),
        );
        return false;
    }
    // The payload follows the header in the frame.
    let body = frame.get(FRAME_HEADER..).unwrap_or_default();
    let Some(payload) = body.get(..facts.length as usize) else {
        return false;
    };
    let flags = facts.flags & 0xff;
    match facts.action {
        FrameAction::Settings => return settings(session, payload),
        FrameAction::Headers => return headers(session, stream_id, flags, payload, new_stream),
        FrameAction::Continuation => {
            let stream = session.stream(stream_id, false);
            let Some(stream) = stream.filter(|_| pending == stream_id) else {
                return false;
            };
            return header_block(session, &stream, flags & FLAG_END_HEADERS, payload);
        }
        FrameAction::Data => return data(session, stream_id, flags, payload),
        FrameAction::PingReply => {
            // RFC 9113 6.7: the ACK echoes the PING's eight payload bytes; any
            // other length is a FRAME_SIZE_ERROR.
            let Ok(echo) = <[u8; PING_PAYLOAD]>::try_from(payload) else {
                return false;
            };
            let mut ack = [0; FRAME_HEADER + PING_PAYLOAD];
            ack[..FRAME_HEADER].copy_from_slice(&frame_header(
                FrameType::Ping,
                PING_PAYLOAD,
                FLAG_ACK,
                stream_id,
            ));
            ack[FRAME_HEADER..].copy_from_slice(&echo);
            send(session.default_ctx, &ack);
        }
        FrameAction::WindowUpdate => {
            // RFC 9113 6.9: the increment's four bytes; any other length is a
            // FRAME_SIZE_ERROR.
            let Ok(increment) = <[u8; 4]>::try_from(payload) else {
                return false;
            };
            let value = u32::from_be_bytes(increment);
            if stream_id == 0 {
                session
                    .remote_window_size
                    .set(credit(session.remote_window_size.get(), value));
            } else if let Some(stream) = session.stream(stream_id, false) {
                stream
                    .remote_window_size
                    .set(credit(stream.remote_window_size.get(), value));
                if !session.is_coro() {
                    stream.wake();
                }
            }
        }
        FrameAction::Reset => session.remove(stream_id),
        _ => {}
    }
    true
}

/// A HEADERS frame: a new stream's request headers, or its trailers, which
/// are decoded (the HPACK table is the connection's) and discarded.
fn headers(
    session: &Session,
    stream_id: u32,
    flags: u32,
    payload: &[u8],
    new_stream: bool,
) -> bool {
    let stream = match session.stream(stream_id, false) {
        Some(stream) => stream,
        // A completed stream is never recreated from a late trailer block.
        None if new_stream => match session.stream(stream_id, true) {
            Some(stream) => stream,
            None => return false,
        },
        None => return false,
    };
    session.header_end_stream.set(flags & FLAG_END_STREAM != 0);
    let mut block = payload;
    if flags & FLAG_PADDED != 0 {
        // The pad length, the block, then that much padding.
        let Some((&pad, rest)) = payload.split_first() else {
            return false;
        };
        let kept = rest.len().checked_sub(usize::from(pad));
        let Some(kept) = kept.and_then(|kept| rest.get(..kept)) else {
            return false;
        };
        block = kept;
    }
    header_block(session, &stream, flags, block)
}

/// One HEADERS or CONTINUATION frame's part of a header block. At the block's
/// end the request is configured from its headers and, with `END_STREAM`,
/// dispatched.
fn header_block(session: &Session, stream: &Stream, flags: u32, payload: &[u8]) -> bool {
    if stream.request_received.get() {
        return false;
    }
    let discard = stream.headers_received.get();
    if !parse_header(session, stream, flags, payload, discard) {
        return false;
    }
    if flags & FLAG_END_HEADERS == 0 {
        session.header_stream_id.set(stream.id);
        return true;
    }
    session.header_stream_id.set(0);
    if !discard && !headers_complete(stream) {
        return false;
    }
    stream.headers_received.set(true);
    if session.header_end_stream.get() {
        return complete(session, stream);
    }
    true
}

/// `std::strtol` consumed the whole Content-Length and it is within the body limit.
fn content_length_valid(session: &Session, value: &[u8]) -> bool {
    let (parsed, complete) = strtol(value);
    !sw_rs_http2_content_length_rejected(complete, parsed, session.max_body_size)
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
        if media == Media::UrlEncoded {
            set(ctx, Field::Form, 1);
        } else if media == Media::Multipart
            // SAFETY: a live context and the header value.
            && !ctx.multipart_start(value.bytes())
        {
            return false;
        }
    }
    if get(ctx, Field::EnableCompression) != 0
        && let Some(value) = header(ctx, b"accept-encoding")
    {
        ctx.compression_method(value.bytes());
    }
    true
}

/// `http2_server_complete_request`: parse the buffered body and dispatch the
/// request, once.
fn complete(session: &Session, stream: &Stream) -> bool {
    if stream.request_received.replace(true) {
        return false;
    }
    let ctx = stream.ctx;
    if ctx.has_body() {
        if get(ctx, Field::ParseBody) != 0 && get(ctx, Field::Form) != 0 {
            ctx.treat_post();
        } else if get(ctx, Field::Multipart) != 0
            // SAFETY: a live context with a multipart parser.
            && !ctx.multipart()
        {
            return false;
        }
    }
    session.handle(stream);
    true
}

/// The request header `key` as a string, if it has one.
fn header(ctx: Ctx, key: &[u8]) -> Option<ZString> {
    let entry = ctx.entry(Value::Header, key)?;
    string_of(&entry)
}

fn window(current: u32, amount: usize, initial: u32, credit: bool) -> Http2WindowPlan {
    sw_rs_http2_window_plan(current, amount, initial, credit)
}

fn credit(current: u32, amount: u32) -> u32 {
    window(current, amount as usize, 0, true).value
}

/// A SETTINGS frame: apply each, then acknowledge.
fn settings(session: &Session, payload: &[u8]) -> bool {
    // RFC 9113 6.5: six bytes a setting; any other length is a FRAME_SIZE_ERROR.
    let (entries, &[]) = payload.as_chunks::<6>() else {
        return false;
    };
    for entry in entries {
        let (id, value) = swoole_rs_http2::sw_rs_http2_unpack_setting(entry);
        let mut deflater = session.deflater.borrow_mut();
        let old = if id == SETTING_HEADER_TABLE_SIZE {
            session.remote[0].get()
        } else {
            0
        };
        let change = id == SETTING_HEADER_TABLE_SIZE && value != old && deflater.is_some();
        let plan = sw_rs_http2_setting_plan(u32::from(id), value, old, change, true);
        if plan.set_value
            && let Ok(field) = usize::try_from(plan.field)
            && let Some(setting) = session.remote.get(field)
        {
            setting.set(value);
        }
        if plan.set_window {
            session.remote_window_size.set(value);
        }
        if plan.change_deflater
            && let Some(encoder) = deflater.as_mut()
        {
            encoder.set_table_size(value);
        }
        drop(deflater);
    }
    // The client assumes the server has not applied its settings until it acknowledges them.
    send(
        session.default_ctx,
        &frame_header(FrameType::Settings, 0, FLAG_ACK, 0),
    );
    true
}

/// A DATA frame: buffer the body, keep the windows open and dispatch the
/// request at the end of the stream.
fn data(session: &Session, stream_id: u32, flags: u32, payload: &[u8]) -> bool {
    let Some(stream) = session.stream(stream_id, false) else {
        log(
            LOG_WARNING,
            STREAM_NOT_FOUND,
            c"swoole_http2_server_parse",
            &format!("http2 stream#{stream_id} not found"),
        );
        return false;
    };
    if !stream.headers_received.get() || stream.request_received.get() {
        return false;
    }
    let ctx = stream.ctx;
    let length = payload.len();
    if length > 0 {
        let buffered = ctx.data_start();
        let ended = get(ctx, Field::End) != 0;
        if sw_rs_http2_data_rejected(buffered, length, session.max_body_size, ended) {
            reset(ctx, stream_id, 0);
            session.remove(stream_id);
            return true;
        }
        ctx.data_append(payload);
        flow(session, &stream, ctx, stream_id, length);
    }
    if flags & FLAG_END_STREAM != 0 {
        return complete(session, &stream);
    }
    true
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
    mut input: &[u8],
    discard: bool,
) -> bool {
    if flags & FLAG_PRIORITY != 0 {
        // The stream dependency and weight are ignored.
        let Some(block) = input.get(5..) else {
            return false;
        };
        input = block;
    }
    stream.ctx.header_arrays();
    let mut inflater = session.inflater.borrow_mut();
    loop {
        let step = match inflater.decode(input, flags & FLAG_END_HEADERS != 0) {
            Ok(step) => step,
            Err(error) => {
                let code = Error::CODE;
                let text = format!("inflate failed, Error: {error}[{code}]");
                warn(c"http2_server_parse_header", &text);
                return false;
            }
        };
        input = input.get(step.read..).unwrap_or_default();
        let inflate_flags = step.flags();
        // Trailers are decoded for the HPACK table but never exposed or acted on.
        if let Some((name, value)) = step.header.filter(|_| !discard)
            && !request_header(session, stream, name, value)
        {
            continue;
        }
        let step = sw_rs_http2_inflate_step(inflate_flags, input.len(), false);
        if step.end_headers {
            inflater.end_headers();
            break;
        }
        if step.finish {
            break;
        }
    }
    true
}

/// `add_assoc_stringl_ex` on the request's `$server` or `$header`.
fn add_string(ctx: Ctx, array: Value, key: &[u8], value: &[u8]) {
    ctx.insert(array, key, ZString::new(value));
}

/// One request header; `false` skips the rest of the step (a Cookie, parsed
/// into `$cookie` and never kept in `$header`, or a Content-Length, checked
/// here and judged once the block is complete).
fn request_header(session: &Session, stream: &Stream, name: &[u8], value: &[u8]) -> bool {
    let ctx = stream.ctx;
    let kind = header_plan(HeaderRole::Request, name, false).kind;
    if name.first() == Some(&b':') {
        match kind {
            HeaderKind::Method => add_string(ctx, Value::Server, b"request_method", value),
            HeaderKind::Path => path(ctx, value),
            HeaderKind::Authority => add_string(ctx, Value::Header, b"host", value),
            _ => {}
        }
        return true;
    }
    match kind {
        HeaderKind::Cookie => {
            ctx.cookie(value);
            false
        }
        HeaderKind::ContentLength => {
            if !content_length_valid(session, value) {
                stream.bad_length.set(true);
            }
            // The header keeps the last value.
            add_string(ctx, Value::Header, b"content-length", value);
            false
        }
        _ => {
            if matches!(kind, HeaderKind::ContentType | HeaderKind::AcceptEncoding) {
                stream.negotiates.set(true);
            }
            add_string(ctx, Value::Header, name, value);
            true
        }
    }
}

/// `:path`: `request_uri`, `path_info` (decoded), `query_string` and `$get`.
fn path(ctx: Ctx, value: &[u8]) {
    let question = value.iter().position(|&byte| byte == b'?');
    let path = match question.and_then(|question| value.split_at_checked(question)) {
        Some((path, query)) => {
            let query = query.get(1..).unwrap_or_default();
            add_string(ctx, Value::Server, b"query_string", query);
            ctx.treat_query(Value::Get, query);
            path
        }
        None => value,
    };
    ctx.set_path(value);
    add_string(ctx, Value::Server, b"request_uri", path);
    ctx.insert(Value::Server, b"path_info", ZString::url_decode(path));
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
            frame_header(FrameType::Ping, 8, FLAG_ACK, 3),
            [0, 0, 8, 6, 1, 0, 0, 0, 3]
        );
    }
}
