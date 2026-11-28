//! The HTTP/2 server's frames, responses and request header blocks: upstream's
//! `swoole_http2_server_parse` and everything it and `Response` send (headers
//! over crates/http2's HPACK, bodies under flow control, trailers, files), over
//! the sessions and streams of `session`, and an async server's
//! `onReceive` and `onClose` for its HTTP/2 connections. The HTTP contexts are
//! crates/phphttp's.
use super::session::{Session, SessionState, Stream};
use libc::c_int;
use std::ffi::{CStr, CString};
use swoole_rs_corebase::timer::Deadline;
use swoole_rs_corebase::{CCast, CellExt};
use swoole_rs_http::status::{BAD_REQUEST, REQUEST_ENTITY_TOO_LARGE};
use swoole_rs_http2::frame_flag::{
    ACK as FLAG_ACK, END_HEADERS as FLAG_END_HEADERS, END_STREAM as FLAG_END_STREAM,
    PADDED as FLAG_PADDED, PRIORITY as FLAG_PRIORITY,
};
use swoole_rs_http2::{
    FileCompletion, FrameAction, FrameType, GOAWAY_SIZE, HeaderKind, HeaderRole, Http2HeaderPlan,
    Http2RespondFacts, Media, PING_PAYLOAD, Respond, decode_packet,
    hpack::{Encoder, Error},
    send_frame_size, sw_rs_http2_file_complete, sw_rs_http2_header_defaults,
    sw_rs_http2_header_plan, sw_rs_http2_headers_flags, sw_rs_http2_inflate_step,
    sw_rs_http2_media_kind, sw_rs_http2_pack_frame_header, sw_rs_http2_respond_next,
    sw_rs_http2_response_status, sw_rs_http2_setting_plan, valid_content_length,
};
use swoole_rs_phpserver::RecvData;
use swoole_rs_server::{Connection, server_methods, view};
use swoole_rs_zend::{Array, HeldArray, Owned, OwnedArray, Value as Zval, ZString, bailout};

/// Frame sizes, types and flags (`swoole_http2.h`).
const FRAME_HEADER: usize = swoole_rs_http2::FRAME_HEADER_SIZE as usize;
/// `HTTP_HEADER_SERVER`, `_DATE` and `_CONTENT_TYPE`.
const MISSING_SERVER: u32 = 2;
const MISSING_DATE: u32 = 16;
const MISSING_CONTENT_TYPE: u32 = 32;
use swoole_rs_http2::setting_id;
/// `swoole::http2::Settings` field indexes.
const MAX_CONCURRENT_STREAMS: usize = 2;
const INIT_WINDOW_SIZE: usize = 3;
const MAX_FRAME_SIZE: usize = 4;
/// Swoole error codes and log levels.
const PHP_FATAL_ERROR: c_int = swoole_rs_error::ErrorCode::SW_ERROR_PHP_FATAL_ERROR as c_int;
const NO_PAYLOAD: c_int = swoole_rs_error::ErrorCode::SW_ERROR_NO_PAYLOAD as c_int;
const STREAM_NOT_FOUND: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_HTTP2_STREAM_NOT_FOUND as c_int;
use super::CO_TIMEDOUT;

use crate::decimal;
use crate::http2::Field;
use swoole_rs_httpbindings::{header_warning, trim_header_end};

pub(crate) use crate::http2::{Ctx, ResponseArray, Value};
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;

pub(crate) fn get(ctx: Ctx, field: Field) -> u32 {
    ctx.get(field)
}

pub(crate) fn set(ctx: Ctx, field: Field, value: u32) {
    ctx.set(field, value);
}

fn send(ctx: Ctx, data: &[u8]) -> bool {
    if get(ctx, Field::CoSocket) != 0 {
        SessionState::of(ctx).is_some_and(|session| session.send(ctx, &[data]))
    } else {
        ctx.send(data)
    }
}

fn close(ctx: Ctx) {
    ctx.close();
}

/// `swoole_warning` (`error` 0) or `swoole_error_log(level, error)` as `func`.
pub(crate) fn log(
    level: swoole_rs_corebase::log::LogLevel,
    error: c_int,
    func: &CStr,
    message: &str,
) {
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
    // Frame lengths are 24-bit.
    sw_rs_http2_pack_frame_header(kind, length.c_cast::<u32>(), flags, stream_id)
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
    set(stream.ctx(), Field::End, 1);
    false
}

// --- small frames ---

fn send_u32_frame(ctx: Ctx, kind: FrameType, stream_id: u32, value: u32) -> bool {
    let payload = value.to_be_bytes();
    let mut frame = [0; FRAME_HEADER + size_of::<u32>()];
    frame[..FRAME_HEADER].copy_from_slice(&frame_header(kind, payload.len(), 0, stream_id));
    frame[FRAME_HEADER..].copy_from_slice(&payload);
    send(ctx, &frame)
}

fn window_update(ctx: Ctx, stream_id: u32, size: u32) -> bool {
    send_u32_frame(ctx, FrameType::WindowUpdate, stream_id, size)
}

/// `Stream::reset`.
pub(crate) fn reset(ctx: Ctx, stream_id: u32, code: u32) -> bool {
    send_u32_frame(ctx, FrameType::RstStream, stream_id, code)
}

/// `swoole_http2_server_ping`.
pub(crate) fn ping(ctx: Ctx) -> bool {
    let mut frame = [0; FRAME_HEADER + PING_PAYLOAD];
    frame[..FRAME_HEADER].copy_from_slice(&frame_header(FrameType::Ping, PING_PAYLOAD, 0, 0));
    send(ctx, &frame)
}

/// `swoole_http2_server_goaway`.
pub(crate) fn goaway(ctx: Ctx, code: i64, data: &[u8]) -> bool {
    let Some(session) = SessionState::of(ctx) else {
        return false;
    };
    let last_stream_id = session.last_stream_id.get();
    let max_frame = send_frame_size(session.remote[MAX_FRAME_SIZE].get()) as usize;
    if data.len() > max_frame - GOAWAY_SIZE {
        bailout::defer(swoole_rs_zend::report(
            swoole_rs_zend::E_WARNING,
            c"GOAWAY debug data exceeds the peer's maximum frame size",
        ));
        return false;
    }
    let sent = send(ctx, &goaway_frame(last_stream_id, code.c_cast(), data));
    if sent {
        session.shutting_down.set(true);
    }
    sent
}

fn goaway_frame(last_stream_id: u32, code: u32, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(FRAME_HEADER + GOAWAY_SIZE + data.len());
    frame.extend_from_slice(&frame_header(
        FrameType::Goaway,
        GOAWAY_SIZE + data.len(),
        0,
        0,
    ));
    frame.extend_from_slice(&last_stream_id.to_be_bytes());
    frame.extend_from_slice(&code.to_be_bytes());
    frame.extend_from_slice(data);
    frame
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
    static POOL: std::cell::Cell<Vec<Buffers>> = const { std::cell::Cell::new(Vec::new()) };
}

/// `f` on the pool, taken out and put back: `f` only pops or pushes, and what it
/// pops it returns.
fn with_pool<R>(f: impl FnOnce(&mut Vec<Buffers>) -> R) -> R {
    POOL.with(|cell| {
        let mut pool = cell.take();
        let result = f(&mut pool);
        // `f` can't reach the cell, which still holds the empty vector `take` left.
        #[expect(
            clippy::mem_forget,
            reason = "an empty vector owns no allocation: dropping it frees nothing"
        )]
        std::mem::forget(cell.replace(pool));
        result
    })
}

/// Upstream's `HeaderSet`: at most `size` headers, names lowercased, in one
/// byte arena.
struct Headers {
    size: usize,
    buffers: Buffers,
}

impl Headers {
    fn new(size: usize) -> Self {
        let buffers = with_pool(Vec::pop).unwrap_or_default();
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
    fn deflate(mut self, session: &Session) -> Option<Self> {
        let Buffers {
            bytes,
            slots,
            frame,
        } = &mut self.buffers;
        frame.resize(FRAME_HEADER, 0);
        let field = |start: usize, length: usize| bytes.get(start..start + length);
        session.with_deflater(|deflater| {
            let encoder = deflater.get_or_insert_with(|| Encoder::new(session.remote[0].get()));
            encoder.encode(
                slots.iter().map(|&(start, name, value)| {
                    let value = field(start + name, value).unwrap_or_default();
                    (field(start, name).unwrap_or_default(), value)
                }),
                frame,
            );
        });
        let length = frame.len() - FRAME_HEADER;
        let limit = session.remote[MAX_FRAME_SIZE].get();
        if length > limit as usize {
            warn(
                c"http2_server_deflate_header_block",
                &format!(
                    "encoded HTTP/2 header block size {length} exceeds peer max frame size {limit}"
                ),
            );
            return None;
        }
        Some(self)
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
        with_pool(|pool| pool.push(buffers));
    }
}

/// A response array property (`sw_zend_read_property_ex`), with a reference of
/// its own. This is the server's only way to read the `header`, `cookie` and
/// `trailer` properties, and [`length_safe`] and [`entries`] take only a
/// [`HeldArray`]: a value's `__toString` may replace or unset the property while
/// the array is walked, and the hold keeps the walked table live.
fn property(mut ctx: Ctx, which: ResponseArray) -> Option<HeldArray> {
    if let Some(fields) = &ctx.state().response.scheduled_fields {
        match which {
            ResponseArray::Header => return fields.0.hold(),
            ResponseArray::Trailer => return fields.1.hold(),
            ResponseArray::Cookie => {}
        }
    }
    ctx.response_array(which)
}

fn detached_fields(ctx: Ctx, which: ResponseArray) -> Owned {
    let mut fields = ctx
        .response_array(which)
        .map_or_else(Owned::empty_array, |a| a.copy());
    if let Some(array) = fields.array_mut() {
        array.separate();
    }
    fields
}

fn snapshot(source: &Owned, multiple: bool) -> Option<Owned> {
    let mut result = OwnedArray::default();
    for (key, value) in source.array()?.entries() {
        let value = value.dereferenced();
        let Some(key) = key.filter(|_| !value.is_null()) else {
            continue;
        };
        let converted = if multiple && let Some(items) = value.hold() {
            let mut values = OwnedArray::default();
            for (_, item) in entries(Some(&items)) {
                let item = item.dereferenced();
                if !item.is_null() {
                    values.push(Owned::string(item.try_string()?));
                }
            }
            values.into_owned()
        } else {
            Owned::string(value.try_string()?)
        };
        result.insert(key, converted);
    }
    Some(result.into_owned())
}

/// Convert user values in the caller's scope before the writer can suspend.
pub(crate) fn schedule(
    mut ctx: Ctx,
    method: &std::ffi::CStr,
    args: swoole_rs_zend::Zvals<'_>,
) -> Result<bool, bailout::Bailout> {
    if !swoole_rs_zend::host::exception().is_null() || of(ctx).is_none() {
        return Ok(false);
    }
    let headers = detached_fields(ctx, ResponseArray::Header);
    let trailers = detached_fields(ctx, ResponseArray::Trailer);
    let fields = snapshot(&headers, true).and_then(|h| Some((h, snapshot(&trailers, false)?)));
    let Some(fields) = fields else {
        return bailout::outcome(false);
    };
    if ctx.state().end || ctx.state().detached || !crate::context::available(ctx) {
        return Ok(false);
    }
    let response = &mut ctx.state().response;
    let object = response.zobject.value().copy();
    response.scheduled_fields = Some(Box::new(fields));
    let started = swoole_rs_phpcoroutines::CREATOR.call_method(
        &object,
        &crate::response::RESPONSE,
        method,
        args,
    );
    let accepted = matches!(started, Ok(true)) && ctx.state().response.scheduled_fields.is_some();
    if accepted {
        ctx.state().end = true;
    } else {
        ctx.state().response.scheduled_fields = None;
    }
    started.map(|_| accepted)
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
pub fn entries(array: Option<&HeldArray>) -> impl Iterator<Item = (Option<&[u8]>, &Zval)> {
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
    headers.deflate(session)
}

/// The body a response header announces: its bytes (none for a file sent
/// later) and length.
#[derive(Clone, Copy)]
struct Body<'a> {
    data: &'a [u8],
    length: usize,
}

/// `http2_server_build_header`.
fn build_header(mut ctx: Ctx, session: &Session, body: Option<Body<'_>>) -> Option<Headers> {
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
    let mut explicit_lengths = None;
    if zheader.is_some() {
        let can_capture =
            get(ctx, Field::AcceptCompression) != 0 && get(ctx, Field::CompressionTypes) != 0;
        let mut content_type = None;
        let mut add = |headers: &mut Headers, key: &[u8], value: &Zval| -> Option<()> {
            if value.is_null() {
                return Some(());
            }
            let owned = string_of(value)?;
            let text = trim_header_end(owned.bytes());
            if let Some(message) = header_warning(text) {
                report(510, false, message.as_bytes());
                return Some(());
            }
            let plan = header_plan(HeaderRole::Server, key, can_capture);
            if plan.suppress {
                let lengths = explicit_lengths.get_or_insert_with(Vec::new);
                if !text.is_empty() {
                    lengths.push(text.to_vec());
                }
                return Some(());
            }
            if key.eq_ignore_ascii_case(b"content-encoding") {
                set(ctx, Field::AcceptCompression, 0);
                if text.is_empty() {
                    return Some(());
                }
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
        if can_capture && get(ctx, Field::AcceptCompression) != 0 {
            let kind = content_type
                .as_ref()
                .map_or(&b"text/html"[..], ZString::bytes);
            let accept = ctx.compressible(kind);
            set(ctx, Field::AcceptCompression, u32::from(accept));
        }
    }

    let missing = sw_rs_http2_header_defaults(header_flags);
    if missing & MISSING_SERVER != 0 {
        headers.add(
            b"server",
            swoole_rs_corebase::constants::SW_HTTP_SERVER_SOFTWARE.as_bytes(),
        );
    }
    if missing & MISSING_DATE != 0 {
        crate::http2::date(|date| headers.add(b"date", date));
    }
    if missing & MISSING_CONTENT_TYPE != 0 {
        headers.add(b"content-type", b"text/html");
    }
    for (_, value) in entries(zcookie.as_ref()) {
        if let Some(cookie) = value.bytes() {
            headers.add(b"set-cookie", cookie);
        }
    }
    let compressed = body.and_then(|body| ctx.compress(body.data));
    let length = if let Some((encoding, length)) = compressed {
        headers.add(b"content-encoding", encoding.to_bytes());
        if explicit_lengths.is_some() {
            log(
                LOG_WARNING,
                swoole_rs_error::ErrorCode::SW_ERROR_HTTP_CONFLICT_HEADER as c_int,
                c"http2_server_build_header",
                "The client has set 'Accept-Encoding', 'Content-Length' will be ignored",
            );
        }
        Some(length)
    } else if let Some(lengths) = explicit_lengths {
        for value in lengths {
            headers.add(b"content-length", &value);
        }
        None
    } else {
        body.map(|body| body.length).filter(|&length| {
            length != 0 || ctx.state().parser.method != swoole_rs_http::llhttp::HEAD
        })
    };
    if let Some(length) = length {
        headers.add(
            b"content-length",
            decimal(&mut [0; 20], i64::try_from(length).unwrap_or(i64::MAX)),
        );
    }
    let headers = headers.deflate(session)?;
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
    send_header_then(
        ctx,
        session,
        stream,
        body,
        body.is_none_or(|body| body.length == 0),
        end_stream,
        |_| (),
    )
}

/// `Stream::send_header`, sending what `then` appends to the HEADERS frame in
/// the same write.
fn send_header_then(
    ctx: Ctx,
    session: &Session,
    stream: &Stream,
    body: Option<Body<'_>>,
    empty: bool,
    end_stream: bool,
    then: impl FnOnce(&mut Vec<u8>),
) -> bool {
    session.writing(ctx, || {
        if !current(session, stream) {
            return false;
        }
        let Some(mut headers) = build_header(ctx, session, body) else {
            // Upstream closes once the session has an encoder.
            if session.with_deflater(|deflater| deflater.is_some()) {
                set(ctx, Field::End, 1);
                close(ctx);
            }
            return false;
        };
        let flags = sw_rs_http2_headers_flags(empty, end_stream, 0);
        if flags & FLAG_END_STREAM != 0 {
            set(ctx, Field::End, 1);
        }
        let frame = headers.seal(FrameType::Headers, flags, stream.id);
        then(frame);
        if !send_header_frame(ctx, session, stream, frame) {
            set(ctx, Field::SendHeader, 0);
            return false;
        }
        true
    })
}

// An encoded block must retain its queue position if its request is reset.
fn send_header_frame(mut ctx: Ctx, session: &Session, stream: &Stream, frame: &[u8]) -> bool {
    let previous = if stream.request_cid.get() != 0 {
        stream
            .header_sender
            .replace(swoole_rs_phpcoroutines::Coroutine::current())
    } else {
        None
    };
    let sent = send(ctx, frame);
    stream.header_sender.set(previous);
    if !sent {
        // The peer cannot decode subsequent blocks after missing an HPACK update.
        set(ctx, Field::End, 1);
        if get(ctx, Field::CoSocket) != 0 {
            close(ctx);
        } else if !session.terminating.get()
            && SessionState::of(ctx).is_some_and(|live| std::rc::Rc::ptr_eq(&live, session))
        {
            if swoole_rs_corebase::log::last_error() == CO_TIMEDOUT {
                write_timed_out(session);
            } else {
                let state = ctx.state();
                server_methods::close(crate::context::server(state), state.fd, true);
            }
        }
    }
    sent
}

/// Process workers send complete frames to the reactor over IPC.
fn send_limit(mut ctx: Ctx) -> usize {
    let state = ctx.state();
    if !crate::context::co_socket(state) {
        let config = crate::context::server(state).config();
        if config.mode_ == swoole_rs_server::Mode::Process {
            return config.output_buffer_size.get() as usize;
        }
    }
    usize::MAX
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
    let max_payload = send_limit(ctx).saturating_sub(FRAME_HEADER);
    if max_payload == 0 {
        swoole_rs_corebase::log::set_last_error(
            swoole_rs_error::ErrorCode::SW_ERROR_DATA_LENGTH_TOO_LARGE as c_int,
        );
        return false;
    }
    let co_socket = get(ctx, Field::CoSocket) != 0;
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    let mut rest = data;
    // Set on the first flow-control wait, so a send that never waits reads no clock.
    let mut deadline = None;
    while !rest.is_empty() {
        let window = send_credit(session, stream);
        if window == 0 {
            if !current(session, stream) {
                return false;
            }
            if !swoole_rs_phpcoroutines::in_coroutine() {
                swoole_rs_corebase::log::set_last_error(
                    swoole_rs_error::ErrorCode::SW_ERROR_OUTPUT_SEND_YIELD as c_int,
                );
                return false;
            }
            let deadline = *deadline.get_or_insert_with(|| ctx.send_deadline());
            if deadline.remaining().is_some_and(|time| time.is_zero()) {
                write_timed_out(session);
                return false;
            }
            if !stream.wait(deadline) {
                if swoole_rs_corebase::log::last_error() == CO_TIMEDOUT {
                    write_timed_out(session);
                }
                return false;
            }
            continue;
        }
        if !session.writing(ctx, || {
            if !current(session, stream) {
                return false;
            }
            // Other streams can spend connection credit while this one waits for the writer.
            let window = send_credit(session, stream);
            if window == 0 {
                return true;
            }
            let peer_limit = send_frame_size(session.remote[MAX_FRAME_SIZE].get()) as usize;
            let send_n = rest.len().min(window).min(peer_limit).min(max_payload);
            let send_flags = if send_n == rest.len() { flags } else { 0 };
            let header = frame_header(FrameType::Data, send_n, send_flags, stream.id);
            let (part, tail) = rest.split_at(send_n);
            consume_window(session, stream, send_n);
            let sent = if !co_socket || send_n < page {
                // A yielding async send must enqueue the entire frame together.
                send(ctx, &[header.as_slice(), part].concat())
            } else {
                session.send(ctx, &[&header, part])
            };
            if !sent {
                if !co_socket {
                    refund_window(session, stream, send_n);
                }
                return false;
            }
            rest = tail;
            true
        }) {
            return false;
        }
    }
    true
}

/// `http2_server_write_timed_out` (upstream 6.3): a peer that never opened the
/// window by the send's deadline loses its connection, after a GOAWAY that says why.
fn write_timed_out(session: &Session) {
    abort_session(session, 0, b"flow control or write timeout");
    swoole_rs_corebase::log::set_last_error(CO_TIMEDOUT);
}

fn send_credit(session: &Session, stream: &Stream) -> usize {
    usize::try_from(
        session
            .remote_window_size
            .get()
            .min(stream.remote_window_size.get()),
    )
    .unwrap_or(0)
}

fn consume_window(session: &Session, stream: &Stream, sent: usize) {
    let sent = sent.c_cast::<i64>();
    stream
        .remote_window_size
        .set(stream.remote_window_size.get() - sent);
    session
        .remote_window_size
        .set(session.remote_window_size.get() - sent);
}

fn refund_window(session: &Session, stream: &Stream, length: usize) {
    let amount = length.c_cast::<u32>();
    if credit(&session.remote_window_size, amount).is_err()
        || credit(&stream.remote_window_size, amount).is_err()
    {
        abort_session(
            session,
            swoole_rs_http2::error_code::FLOW_CONTROL_ERROR,
            b"window overflow after canceled send",
        );
    } else {
        wake_streams(&session.open_streams());
    }
}

fn wake_streams(streams: &[Stream]) -> bool {
    let mut ok = true;
    for stream in streams {
        // A resumed sender can bail out while other senders still need the wakeup.
        ok &= stream.wake();
    }
    ok
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
        || data.len() > send_credit(session, stream)
        || frame.len() + FRAME_HEADER + data.len() > send_limit(stream.ctx())
    {
        return 0;
    }
    let flags = if end_stream { FLAG_END_STREAM } else { 0 };
    frame.extend_from_slice(&frame_header(FrameType::Data, data.len(), flags, stream.id));
    frame.extend_from_slice(data);
    consume_window(session, stream, data.len());
    data.len()
}

/// `Stream::send_trailer`: HEADERS with `END_STREAM`, even an empty block.
fn send_trailer(ctx: Ctx, session: &Session, stream: &Stream, deadline: Deadline) -> bool {
    session.writing(ctx, || {
        if !current(session, stream) {
            return false;
        }
        let Some(mut headers) = build_trailer(ctx, session) else {
            return false;
        };
        // Converting a trailer value to a PHP string can suspend the writer.
        if deadline.remaining().is_some_and(|time| time.is_zero()) {
            write_timed_out(session);
            return false;
        }
        send_header_frame(
            ctx,
            session,
            stream,
            headers.seal(
                FrameType::Headers,
                FLAG_END_HEADERS | FLAG_END_STREAM,
                stream.id,
            ),
        )
    })
}

/// Whether the response has a trailer.
fn has_trailer(ctx: Ctx) -> bool {
    length_safe(property(ctx, ResponseArray::Trailer).as_ref()) > 0
}

/// `http2_server_respond`: the header, the body and the trailer, then the
/// stream is done.
pub(crate) fn respond(ctx: Ctx, data: &[u8]) -> bool {
    respond_with_length(ctx, data, data.len())
}

/// HEAD sends an empty body but announces the selected representation's length.
pub(crate) fn respond_with_length(ctx: Ctx, data: &[u8], length: usize) -> bool {
    let Some((session, stream)) = of(ctx) else {
        return false;
    };
    let trailer = has_trailer(ctx).then(|| ctx.send_deadline());
    let end_stream = trailer.is_none();
    let mut body: Vec<u8> = Vec::new();
    let mut compressed = false;
    // Whether the body went out with the header.
    let mut body_sent = false;
    let mut facts = Http2RespondFacts {
        header_sent: get(ctx, Field::SendHeader) != 0,
        trailer_present: trailer.is_some(),
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
                let announced = Body { data, length };
                let mut appended = 0;
                let sent = send_header_then(
                    ctx,
                    &session,
                    &stream,
                    Some(announced),
                    data.is_empty(),
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
                if !sent && appended > 0 && get(ctx, Field::CoSocket) == 0 {
                    refund_window(&session, &stream, appended);
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
                let sent =
                    trailer.is_some_and(|deadline| send_trailer(ctx, &session, &stream, deadline));
                action = next(action, sent, &facts);
            }
            Respond::ErrorClose | Respond::Remove => {
                if action == Respond::ErrorClose {
                    // Not when the peer reset the stream or closed meanwhile.
                    if session.stream(stream.id, false).is_some() {
                        close(ctx);
                    }
                } else if !session.remove(stream.id) {
                    return false;
                }
                facts.shutting_down = session.shutting_down.get();
                facts.streams_empty = facts.shutting_down && session.stream_count() == 0;
                action = next(action, true, &facts);
            }
            Respond::ErrorShutdown | Respond::SuccessShutdown => {
                session.shutdown(ctx);
                return action == Respond::SuccessShutdown;
            }
            Respond::True => return true,
            _ => return false,
        }
    }
}

/// `swoole_http2_server_write`: a chunk, the header first.
pub(crate) fn write(ctx: Ctx, chunk: &[u8]) -> bool {
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

/// Sends a file response's headers: whether trailers follow the body, `None` when
/// the headers could not be sent.
pub(crate) fn begin_file(mut ctx: Ctx, content_type: &[u8], length: usize) -> Option<bool> {
    let (session, stream) = of(ctx)?;
    set(ctx, Field::AcceptCompression, 0);
    let trailer = has_trailer(ctx);
    // The held array's reference ends here: a write to a shared array asserts.
    let typed = property(ctx, ResponseArray::Header)
        .as_ref()
        .and_then(|zheader| zheader.array())
        .is_some_and(|zheader| zheader.find(b"content-type").is_some());
    if !typed {
        ctx.set_header(b"content-type", content_type);
        if let Some(fields) = &mut ctx.state().response.scheduled_fields
            && let Some(headers) = fields.0.array_mut()
        {
            headers.insert_bytes(b"content-type", Some(content_type));
        }
    }
    let body = Body { data: &[], length };
    if !send_header(ctx, &session, &stream, Some(body), !trailer) {
        return None;
    }
    // Headers are sent: retries are no longer allowed, even if the body fails.
    set(ctx, Field::End, 1);
    Some(trailer)
}

/// The trailer, then the stream is done: false after an error.
pub(crate) fn finish_file(ctx: Ctx, trailer: bool, error: bool, deadline: Deadline) -> bool {
    let Some(session) = SessionState::of(ctx) else {
        return false;
    };
    let stream_id = get(ctx, Field::StreamId);
    let mut action = sw_rs_http2_file_complete(!error, trailer);
    loop {
        match action {
            FileCompletion::Trailer => {
                let sent = session
                    .stream(stream_id, false)
                    .is_some_and(|stream| send_trailer(ctx, &session, &stream, deadline));
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
                return session.remove(stream_id);
            }
        }
    }
}

// --- receiving ---

/// `swoole_http2_server_onReceive`: an async server's data on an HTTP/2
/// connection, whose session (and settings context) the first data makes.
pub(crate) fn on_receive(
    view: view::Server<'_>,
    conn: Connection<'_>,
    request: &mut RecvData<'_>,
) -> bool {
    let session_id = request.info().fd;
    let session = SessionState::find(session_id).unwrap_or_else(|| {
        let default_ctx = Ctx::server_context(view, session_id);
        let max_body_size = swoole_rs_server::server_methods::package_max_length(view, conn.fd());
        super::session::session_new(
            session_id,
            default_ctx,
            max_body_size,
            Box::new(|ctx: Ctx<'_>, _: Ctx<'_>| crate::http2_request(ctx)),
        )
    });
    let zdata = swoole_rs_phpserver::php_swoole_get_recv_data(view, request);
    let data = zdata.bytes().unwrap_or_default();
    // The session stays alive until its frame is handled.
    let parsed = session_receive(&session, data);
    // Released before a deferred bailout can jump past it.
    drop(zdata);
    if bailout::pending() {
        swoole_rs_corebase::reactor::event_api::swoole_event_bail_out();
    }
    parsed
}

/// `php_swoole_http2_server_onClose`: the connection's session goes.
pub fn server_on_close(session_id: i64) {
    super::session::session_free(session_id);
}

/// `swoole_http2_server_parse` for the coroutine server's connection loop: one
/// frame from the peer of connection `fd`, which `frame` starts with; a bailout
/// stays pending for the caller's `finish`.
pub(crate) fn session_receive(session: &Session, frame: &[u8]) -> bool {
    let last_stream_id = session.last_stream_id.get();
    let Ok((facts, payload)) = decode_packet(frame, last_stream_id, false) else {
        return false;
    };
    let stream_id = facts.stream_id;
    let new_stream = facts.publish_last_id && !session.shutting_down.get();
    // A header block continues only in CONTINUATION frames of its own stream.
    let pending = session.header_stream_id.get();
    if (facts.action == FrameAction::Continuation) != (pending != 0)
        || (pending != 0 && stream_id != pending)
    {
        return abort_session(
            session,
            swoole_rs_http2::error_code::PROTOCOL_ERROR,
            b"invalid header block continuation",
        );
    }
    let flags = facts.flags & 0xff;
    match facts.action {
        FrameAction::Settings => return settings(session, payload),
        FrameAction::Headers => return headers(session, stream_id, flags, payload, new_stream),
        FrameAction::Continuation => {
            let stream = session.stream(stream_id, false);
            return header_block(
                session,
                stream_id,
                stream.as_ref(),
                flags & FLAG_END_HEADERS,
                payload,
            );
        }
        FrameAction::Data => return data(session, stream_id, flags, payload),
        FrameAction::PingReply | FrameAction::PingAck => {
            // RFC 9113 6.7: the ACK echoes the PING's eight payload bytes; any
            // other length is a FRAME_SIZE_ERROR.
            let Ok(echo) = swoole_rs_http2::ping_payload(payload) else {
                close(session.default_ctx());
                return false;
            };
            if facts.action == FrameAction::PingAck {
                return true;
            }
            let mut ack = [0; FRAME_HEADER + PING_PAYLOAD];
            ack[..FRAME_HEADER].copy_from_slice(&frame_header(
                FrameType::Ping,
                PING_PAYLOAD,
                FLAG_ACK,
                stream_id,
            ));
            ack[FRAME_HEADER..].copy_from_slice(&echo);
            return send_ack(session.default_ctx(), &ack);
        }
        FrameAction::WindowUpdate => {
            // RFC 9113 6.9: the increment's four bytes; any other length is a
            // FRAME_SIZE_ERROR.
            let Ok(value) = swoole_rs_http2::window_increment(payload) else {
                return abort_session(
                    session,
                    swoole_rs_http2::error_code::FRAME_SIZE_ERROR,
                    b"invalid WINDOW_UPDATE length",
                );
            };
            let value = value & i32::MAX.cast_unsigned();
            if stream_id == 0 {
                if let Err(code) = credit(&session.remote_window_size, value) {
                    return abort_session(session, code, b"invalid connection WINDOW_UPDATE");
                }
                return wake_streams(&session.open_streams());
            } else if stream_id & 1 == 0
                || (stream_id > last_stream_id && !session.shutting_down.get())
            {
                return abort_session(
                    session,
                    swoole_rs_http2::error_code::PROTOCOL_ERROR,
                    b"WINDOW_UPDATE on idle stream",
                );
            } else if let Some(stream) = session.stream(stream_id, false) {
                if let Err(code) = credit(&stream.remote_window_size, value) {
                    return reset_stream(session, &stream, code);
                }
                if !stream.wake() {
                    return false;
                }
            }
        }
        FrameAction::Reset => {
            let Ok(code) = payload.try_into() else {
                close(session.default_ctx());
                return false;
            };
            if let Some(stream) = session.take_stream(stream_id) {
                set(stream.ctx(), Field::End, 1);
                let cid = stream.request_cid.get();
                if cid > 0 {
                    let mut reason = swoole_rs_zend::OwnedArray::default();
                    reason.insert_long(b"session_id", stream.ctx().state().fd);
                    reason.insert_long(b"stream_id", i64::from(stream_id));
                    reason.insert_long(b"code", i64::from(u32::from_be_bytes(code)));
                    reason.insert_bytes(b"message", Some(b"HTTP2 Stream Reset"));
                    let wake = stream.header_sender.get().is_none_or(|co| co.cid() != cid);
                    bailout::defer(swoole_rs_zend::contain_with(|| {
                        swoole_rs_phpcoroutines::cancel_callback(cid, &reason, wake);
                    }));
                }
                return stream.wake();
            }
            return true;
        }
        FrameAction::Goaway if payload.len() >= GOAWAY_SIZE => {}
        FrameAction::Goaway => {
            close(session.default_ctx());
            return false;
        }
        _ => {}
    }
    true
}

/// HEADERS and DATA share the same optional pad length and trailing padding.
fn unpadded(flags: u32, payload: &[u8]) -> Option<&[u8]> {
    if flags & FLAG_PADDED == 0 {
        return Some(payload);
    }
    let (&padding, rest) = payload.split_first()?;
    rest.get(..rest.len().checked_sub(usize::from(padding))?)
}

/// A HEADERS frame: a new stream's request headers, or its trailers, which
/// share the connection's HPACK table.
fn headers(
    session: &Session,
    stream_id: u32,
    flags: u32,
    payload: &[u8],
    new_stream: bool,
) -> bool {
    if stream_id & 1 == 0 {
        close(session.default_ctx());
        return false;
    }
    let stream = session.stream(stream_id, new_stream);
    if new_stream {
        session.last_stream_id.set(stream_id);
    }
    session.header_end_stream.set(flags & FLAG_END_STREAM != 0);
    let Some(block) = unpadded(flags, payload) else {
        close(session.default_ctx());
        return false;
    };
    let mut remaining = Ok(swoole_rs_http2::max_headers());
    if new_stream && session.stream_count() > session.local(MAX_CONCURRENT_STREAMS) as usize {
        // Decode refused streams through END_HEADERS to preserve the connection's HPACK table.
        remaining = Err(swoole_rs_http2::error_code::REFUSED_STREAM);
    } else if let Some(stream) = &stream
        && stream.request_received.get()
    {
        remaining = Err(swoole_rs_http2::error_code::STREAM_CLOSED);
    } else if let Some(stream) = &stream
        && stream.headers_received.get()
    {
        crate::parse::init_property(
            &mut stream.ctx().state().request,
            crate::parse::Property::Trailer,
        );
        if flags & FLAG_END_STREAM == 0 {
            remaining = Err(swoole_rs_http2::error_code::PROTOCOL_ERROR);
        }
    }
    session.remaining_headers.set(remaining);
    header_block(session, stream_id, stream.as_ref(), flags, block)
}

/// One HEADERS or CONTINUATION frame's part of a header block. At the block's
/// end the request is configured from its headers and, with `END_STREAM`,
/// dispatched.
fn header_block(
    session: &Session,
    stream_id: u32,
    stream: Option<&Stream>,
    flags: u32,
    payload: &[u8],
) -> bool {
    if !parse_header(session, stream_id, stream, flags, payload) {
        // HPACK state belongs to the connection and cannot resume after a decoding error.
        close(session.default_ctx());
        return false;
    }
    if flags & FLAG_END_HEADERS == 0 {
        session.header_stream_id.set(stream_id);
        return true;
    }
    session.header_stream_id.set(0);
    let Some(stream) = stream else {
        // Even after reset, indexing in this block affects subsequent streams.
        return true;
    };
    if let Err(code) = session.remaining_headers.get() {
        return reset_stream(session, stream, code);
    }
    if !stream.headers_received.get() && !headers_complete(stream, session.max_body_size) {
        return false;
    }
    stream.headers_received.set(true);
    if session.header_end_stream.get() {
        return complete(session, stream);
    }
    true
}

/// `http2_server_on_headers_complete`: the request's Content-Length, body
/// parser and compression, from its whole initial header block.
fn headers_complete(stream: &Stream, limit: u32) -> bool {
    let ctx = stream.ctx();
    if let Some(cookie) = stream.cookie_header.take() {
        add_string(ctx, Value::Header, b"cookie", &cookie);
    }
    if !stream.negotiates.get() {
        return true;
    }
    if let Some(value) = header(ctx, b"content-type") {
        let (folded, length) = fold(value.bytes());
        let media = sw_rs_http2_media_kind(folded.get(..length).unwrap_or_default());
        if media == Media::UrlEncoded {
            set(ctx, Field::Form, 1);
        } else if media == Media::Multipart && !ctx.multipart_start(value.bytes(), limit) {
            close(ctx);
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
    let ctx = stream.ctx();
    if ctx.has_body() {
        if get(ctx, Field::ParseBody) != 0 && get(ctx, Field::Form) != 0 {
            ctx.treat_post();
        } else if get(ctx, Field::Multipart) != 0 && !ctx.multipart() {
            set(ctx, Field::Status, BAD_REQUEST.cast_unsigned());
            let sent = send_header(ctx, session, stream, None, true);
            return session.remove(stream.id) && sent;
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

fn credit(window: &std::cell::Cell<i64>, amount: u32) -> Result<(), u32> {
    if amount == 0 {
        return Err(swoole_rs_http2::error_code::PROTOCOL_ERROR);
    }
    let next = window
        .get()
        .checked_add(i64::from(amount))
        .filter(|n| *n <= i64::from(i32::MAX))
        .ok_or(swoole_rs_http2::error_code::FLOW_CONTROL_ERROR)?;
    window.set(next);
    Ok(())
}

fn abort_session(session: &Session, code: u32, message: &[u8]) -> bool {
    if session.terminating.replace(true) {
        return false;
    }
    let mut ctx = session.default_ctx();
    if get(ctx, Field::CoSocket) != 0 {
        session.abort_writer(
            ctx,
            &goaway_frame(session.last_stream_id.get(), code, message),
        );
    } else {
        // A timed-out writer must not re-enter the output-buffer wait to send GOAWAY.
        let fd = ctx.fd();
        let server = crate::context::server(ctx.state());
        let frame = goaway_frame(session.last_stream_id.get(), code, message);
        if server_methods::send(server, fd, &frame)
            && server_methods::close(server, fd, false)
            && let Some(owner) = swoole_rs_server::SharedServer::current()
            && owner.view().raw() == server.raw()
            && swoole_rs_corebase::timer::add_fn(100, false, move || {
                let server = owner.view();
                if server.verified(fd).is_some() {
                    server_methods::close(server, fd, true);
                }
            })
            .is_some()
        {
            return false;
        }
        server_methods::close(server, fd, true);
    }
    false
}

/// A SETTINGS frame: apply each, then acknowledge.
fn settings(session: &Session, payload: &[u8]) -> bool {
    // RFC 9113 6.5: six bytes a setting; any other length is a FRAME_SIZE_ERROR.
    let (entries, &[]) = payload.as_chunks::<{ swoole_rs_http2::SETTING_BYTES }>() else {
        close(session.default_ctx());
        return false;
    };
    for entry in entries {
        let (id, value) = swoole_rs_http2::sw_rs_http2_unpack_setting(entry);
        if id == setting_id::INIT_WINDOW_SIZE && value > i32::MAX.cast_unsigned() {
            return abort_session(
                session,
                swoole_rs_http2::error_code::FLOW_CONTROL_ERROR,
                b"invalid initial window",
            );
        }
        if id == setting_id::MAX_FRAME_SIZE && send_frame_size(value) != value {
            warn(
                c"swoole_http2_server_parse",
                &format!("invalid SETTINGS_MAX_FRAME_SIZE value {value}"),
            );
            close(session.default_ctx());
            return false;
        }
        if id == setting_id::INIT_WINDOW_SIZE {
            let delta = i64::from(value) - i64::from(session.remote[INIT_WINDOW_SIZE].get());
            let streams = session.open_streams();
            if streams
                .iter()
                .any(|s| s.remote_window_size.get() + delta > i64::from(i32::MAX))
            {
                return abort_session(
                    session,
                    swoole_rs_http2::error_code::FLOW_CONTROL_ERROR,
                    b"stream window overflow",
                );
            }
            session.remote[INIT_WINDOW_SIZE].set(value);
            for stream in &streams {
                stream
                    .remote_window_size
                    .set(stream.remote_window_size.get() + delta);
            }
            if delta > 0 && !wake_streams(&streams) {
                return false;
            }
            continue;
        }
        session.with_deflater(|deflater| {
            let old = if id == setting_id::HEADER_TABLE_SIZE {
                session.remote[0].get()
            } else {
                0
            };
            let change = id == setting_id::HEADER_TABLE_SIZE && value != old && deflater.is_some();
            let plan = sw_rs_http2_setting_plan(u32::from(id), value, old, change, true);
            if plan.set_value
                && let Ok(field) = usize::try_from(plan.field)
                && let Some(setting) = session.remote.get(field)
            {
                setting.set(value);
            }
            if plan.change_deflater
                && let Some(encoder) = deflater.as_mut()
            {
                encoder.set_table_size(value);
            }
        });
    }
    // The client assumes the server has not applied its settings until it acknowledges them.
    send_ack(
        session.default_ctx(),
        &frame_header(FrameType::Settings, 0, FLAG_ACK, 0),
    )
}

/// A full worker output buffer would silently lose the ACK. A peer already gone
/// still has receive events queued ahead of its close notification.
fn send_ack(ctx: Ctx, frame: &[u8]) -> bool {
    use swoole_rs_error::ErrorCode as E;
    if send(ctx, frame) {
        return true;
    }
    if [
        E::SW_ERROR_DATA_LENGTH_TOO_LARGE as c_int,
        E::SW_ERROR_OUTPUT_BUFFER_OVERFLOW as c_int,
        E::SW_ERROR_OUTPUT_SEND_YIELD as c_int,
    ]
    .contains(&swoole_rs_corebase::log::last_error())
    {
        close(ctx);
        return false;
    }
    true
}

fn reset_stream(session: &Session, stream: &Stream, code: u32) -> bool {
    if !reset(stream.ctx(), stream.id, code)
        && swoole_rs_corebase::log::last_error()
            == swoole_rs_error::ErrorCode::SW_ERROR_OUTPUT_SEND_YIELD as c_int
    {
        close(stream.ctx());
        return false;
    }
    session.remove(stream.id)
}

/// A DATA frame: buffer the body, keep the windows open and dispatch the
/// request at the end of the stream.
fn data(session: &Session, stream_id: u32, flags: u32, payload: &[u8]) -> bool {
    use swoole_rs_http2::error_code::{FLOW_CONTROL_ERROR, PROTOCOL_ERROR, STREAM_CLOSED};
    if stream_id & 1 == 0
        || (stream_id > session.last_stream_id.get() && !session.shutting_down.get())
    {
        log(
            LOG_WARNING,
            STREAM_NOT_FOUND,
            c"swoole_http2_server_parse",
            &format!("http2 stream#{stream_id} not found"),
        );
        return abort_session(session, PROTOCOL_ERROR, b"DATA on idle stream");
    }
    let length = payload.len().c_cast::<u32>();
    let initial = session.local(INIT_WINDOW_SIZE);
    let Some(remaining) = session.local_window_size.get().checked_sub(length) else {
        return abort_session(
            session,
            FLOW_CONTROL_ERROR,
            b"connection receive window exceeded",
        );
    };
    session.local_window_size.set(remaining);
    let target = initial.max(swoole_rs_http2::INITIAL_CONNECTION_WINDOW);
    if !replenish(session.default_ctx(), 0, &session.local_window_size, target) {
        return false;
    }
    let Some(stream) = session.stream(stream_id, false) else {
        // DATA can be in flight when a previously opened stream is reset.
        return true;
    };
    if !stream.headers_received.get() || stream.request_received.get() {
        return reset_stream(session, &stream, STREAM_CLOSED);
    }
    let ctx = stream.ctx();
    let Some(remaining) = stream.local_window_size.get().checked_sub(length) else {
        return reset_stream(session, &stream, FLOW_CONTROL_ERROR);
    };
    stream.local_window_size.set(remaining);
    let Some(body) = unpadded(flags, payload) else {
        return abort_session(session, PROTOCOL_ERROR, b"invalid DATA padding");
    };
    if !body.is_empty() && ctx.data_append(body, session.max_body_size).is_err() {
        if !send_413(session, &stream) {
            close(ctx);
            return false;
        }
        return reset_stream(session, &stream, swoole_rs_http2::error_code::NO_ERROR);
    }
    if !replenish(ctx, stream_id, &stream.local_window_size, initial) {
        return false;
    }
    flags & FLAG_END_STREAM == 0 || complete(session, &stream)
}

fn send_413(session: &Session, stream: &Stream) -> bool {
    let ctx = stream.ctx();
    set(ctx, Field::Status, REQUEST_ENTITY_TOO_LARGE.cast_unsigned());
    send_header(ctx, session, stream, None, true)
}

/// Restore a receive window at half capacity, once the credit is sent.
fn replenish(ctx: Ctx, id: u32, window: &std::cell::Cell<u32>, target: u32) -> bool {
    let remaining = window.get();
    if target > 0 && remaining <= target / 2 {
        if !window_update(ctx, id, target - remaining) {
            close(ctx);
            return false;
        }
        window.set(target);
    }
    true
}

/// `http2_server_parse_header`: a HEADERS block into the request's
/// `$server`, `$header`, `$get` and `$cookie`.
fn parse_header(
    session: &Session,
    stream_id: u32,
    stream: Option<&Stream>,
    flags: u32,
    mut input: &[u8],
) -> bool {
    let trailers = stream.is_some_and(|s| s.headers_received.get());
    if flags & FLAG_PRIORITY != 0 {
        // The stream dependency and weight are ignored.
        let Some(block) = input.get(5..) else {
            return false;
        };
        input = block;
    }
    if let Some(stream) = stream {
        stream.ctx().header_arrays();
    }
    let mut field = session.field.take().unwrap_or_default();
    let parsed = loop {
        let step = match session
            .inflater
            .decode(input, flags & FLAG_END_HEADERS != 0, &mut field)
        {
            Ok(step) => step,
            Err(error) => {
                let code = Error::CODE;
                let text = format!("inflate failed, Error: {error}[{code}]");
                warn(c"http2_server_parse_header", &text);
                break false;
            }
        };
        let Some(rest) = input.get(step.read..) else {
            break false;
        };
        input = rest;
        let inflate_flags = step.flags();
        // After reset, trailers can still update HPACK. A pseudo-header starts a
        // new request, which cannot reuse a closed stream identifier.
        if stream.is_none()
            && stream_id <= session.last_stream_id.get()
            && step.header.is_some_and(|(name, _)| name.starts_with(b":"))
        {
            break false;
        }
        if let Some(stream) = stream
            && let Some((name, value)) = step.header
            && let Ok(count) = session.remaining_headers.get()
        {
            const ENHANCE_YOUR_CALM: u32 = 11;
            let remaining = if count == 0 {
                Err(ENHANCE_YOUR_CALM)
            } else if name.is_empty()
                || (name.eq_ignore_ascii_case(b"te") && !value.eq_ignore_ascii_case(b"trailers"))
                || (trailers && !valid_trailer(name, value))
            {
                Err(swoole_rs_http2::error_code::PROTOCOL_ERROR)
            } else if !trailers
                && name.eq_ignore_ascii_case(b"content-length")
                && !valid_content_length(value, session.max_body_size)
            {
                // Reply as soon as HPACK emits the length, then decode through END_HEADERS.
                if !send_413(session, stream) {
                    break false;
                }
                Err(swoole_rs_http2::error_code::NO_ERROR)
            } else {
                if trailers {
                    crate::parse::edit(
                        &mut stream.ctx().state().request,
                        crate::parse::Property::Trailer,
                        |array| {
                            array.merge(name, swoole_rs_zend::Owned::string(ZString::new(value)));
                        },
                    );
                } else {
                    request_header(stream, name, value);
                }
                Ok(count - 1)
            };
            session.remaining_headers.set(remaining);
        }
        let step = sw_rs_http2_inflate_step(inflate_flags, input.len(), false);
        if step.end_headers {
            session.inflater.end_headers();
            break true;
        }
        if step.finish {
            break true;
        }
    };
    session.field.set(Some(field));
    parsed
}

/// `add_assoc_stringl_ex` on the request's `$server` or `$header`.
fn add_string(ctx: Ctx, array: Value, key: &[u8], value: &[u8]) {
    ctx.insert(array, key, ZString::new(value));
}

/// A lowercase HTTP field name and a value without leading or trailing whitespace.
fn valid_trailer(name: &[u8], value: &[u8]) -> bool {
    name.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"!#$%&'*+-.^_`|~".contains(c))
        && !matches!(value.first(), Some(b' ' | b'\t'))
        && !matches!(value.last(), Some(b' ' | b'\t'))
        && value
            .iter()
            .all(|&c| c == b'\t' || (c >= 0x20 && c != 0x7f))
}

/// Apply one field from the initial request header block.
fn request_header(stream: &Stream, name: &[u8], value: &[u8]) {
    let ctx = stream.ctx();
    let kind = header_plan(HeaderRole::Request, name, false).kind;
    if name.first() == Some(&b':') {
        match kind {
            HeaderKind::Method => {
                if value.eq_ignore_ascii_case(b"HEAD") {
                    stream.ctx().state().parser.method = swoole_rs_http::llhttp::HEAD;
                }
                add_string(ctx, Value::Server, b"request_method", value);
            }
            HeaderKind::Path => path(ctx, value),
            HeaderKind::Authority => add_string(ctx, Value::Header, b"host", value),
            _ => {}
        }
        return;
    }
    match kind {
        HeaderKind::Cookie => {
            ctx.cookie(value);
            stream.cookie_header.with_taken(|cookie| match cookie {
                Some(joined) => {
                    joined.extend_from_slice(b"; ");
                    joined.extend_from_slice(value);
                }
                None => *cookie = Some(value.to_vec()),
            });
        }
        HeaderKind::ContentLength => add_string(ctx, Value::Header, b"content-length", value),
        _ => {
            if matches!(kind, HeaderKind::ContentType | HeaderKind::AcceptEncoding) {
                stream.negotiates.set(true);
            }
            add_string(ctx, Value::Header, name, value);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length_is_unsigned_decimal_within_limit() {
        for bytes in [b"123".as_slice(), b"000123", b"0"] {
            assert!(valid_content_length(bytes, 123));
        }
        for bytes in [
            b" 12".as_slice(),
            b"+12",
            b"-5",
            b"12a",
            b"",
            b"124",
            b"99999999999999999999999",
        ] {
            assert!(!valid_content_length(bytes, 123));
        }
    }

    #[test]
    fn frames_are_big_endian() {
        assert_eq!(
            frame_header(FrameType::Ping, 8, FLAG_ACK, 3),
            [0, 0, 8, 6, 1, 0, 0, 0, 3]
        );
    }
}
