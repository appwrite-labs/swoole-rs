//! HTTP/2 policy for crates/phphttp2, and `src/protocol/http2.cc`'s functions under
//! their upstream symbols.
#![forbid(unsafe_code)]
mod files;
pub mod hpack;
pub use files::*;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2Settings {
    pub header_table_size: u32,
    pub enable_push: u32,
    pub max_concurrent_streams: u32,
    pub init_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2FrameFacts {
    pub length: u32,
    pub flags: u32,
    pub stream_id: u32,
    /// The stream id is past the previous last one.
    pub publish_last_id: bool,
    pub action: FrameAction,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2SettingPlan {
    pub field: i32,
    pub set_value: bool,
    pub set_window: bool,
    pub change_deflater: bool,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2ChunkPlan {
    pub length: usize,
    pub flags: u32,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2ResponsePlan {
    pub end: bool,
    pub ready: bool,
}

/// Size of the fixed HTTP/2 frame header, including its three-byte length.
pub const FRAME_HEADER_SIZE: u8 = 9;
/// Opaque payload of a PING or its acknowledgement.
pub const PING_PAYLOAD: usize = 8;
/// Last stream ID and error code preceding optional GOAWAY debug data.
pub const GOAWAY_SIZE: usize = 2 * std::mem::size_of::<u32>();
/// A `WINDOW_UPDATE` frame carries one network-order increment.
pub const WINDOW_UPDATE_SIZE: usize = std::mem::size_of::<u32>();
const HEADER_BYTES: usize = FRAME_HEADER_SIZE as usize;
pub const SETTING_BYTES: usize = size_of::<u16>() + size_of::<u32>();
/// Wire identifiers from `swHttp2SettingId`.
pub mod setting_id {
    pub const HEADER_TABLE_SIZE: u16 = 1;
    pub const ENABLE_PUSH: u16 = 2;
    pub const MAX_CONCURRENT_STREAMS: u16 = 3;
    pub const INIT_WINDOW_SIZE: u16 = 4;
    pub const MAX_FRAME_SIZE: u16 = 5;
    pub const MAX_HEADER_LIST_SIZE: u16 = 6;
}
const SETTING_COUNT: usize = setting_id::MAX_HEADER_LIST_SIZE as usize;
pub const SETTINGS_FRAME_SIZE: usize = HEADER_BYTES + SETTING_COUNT * SETTING_BYTES;

/// RFC 7540 connection window before any `WINDOW_UPDATE`.
pub const INITIAL_CONNECTION_WINDOW: u32 = 65_535;

/// The process-wide defaults (`SW_HTTP2_DEFAULT_*`), indexed by setting id - 1:
/// header table, push, concurrent streams, initial window, frame size, header list.
static DEFAULT_SETTINGS: [AtomicU32; SETTING_COUNT] = [
    AtomicU32::new(1 << 12),
    AtomicU32::new(0),
    AtomicU32::new(128),
    AtomicU32::new(INITIAL_CONNECTION_WINDOW),
    AtomicU32::new(1 << 14),
    AtomicU32::new(u32::MAX),
];

/// The slot of a setting id. Upstream asserts on any other id, so this aborts.
#[expect(
    clippy::disallowed_methods,
    reason = "upstream asserts on an unknown id; this crate has no dependencies, so no logger"
)]
fn default_slot(id: u32) -> &'static AtomicU32 {
    usize::try_from(id)
        .ok()
        .and_then(|id| DEFAULT_SETTINGS.get(id.checked_sub(1)?))
        .unwrap_or_else(|| std::process::abort())
}

/// `swoole::http2::put_default_setting(swHttp2SettingId, uint32_t)`.
pub fn put_default_setting(id: u32, value: u32) {
    default_slot(id).store(value, Ordering::Relaxed);
}

/// `swoole::http2::get_default_setting(swHttp2SettingId)`.
#[must_use]
pub fn get_default_setting(id: u32) -> u32 {
    default_slot(id).load(Ordering::Relaxed)
}

// This local decode limit is independent of the settings advertised to peers.
static MAX_HEADERS: AtomicU32 = AtomicU32::new(1000);

#[must_use]
pub fn max_headers() -> u32 {
    MAX_HEADERS.load(Ordering::Relaxed)
}

pub fn set_max_headers(limit: u32) {
    if limit != 0 {
        MAX_HEADERS.store(limit, Ordering::Relaxed);
    }
}

/// All defaults, for the server's SETTINGS frame.
#[must_use]
pub fn sw_rs_http2_default_settings() -> Http2Settings {
    let get = |id: u16| get_default_setting(u32::from(id));
    Http2Settings {
        header_table_size: get(setting_id::HEADER_TABLE_SIZE),
        enable_push: get(setting_id::ENABLE_PUSH),
        max_concurrent_streams: get(setting_id::MAX_CONCURRENT_STREAMS),
        init_window_size: get(setting_id::INIT_WINDOW_SIZE),
        max_frame_size: get(setting_id::MAX_FRAME_SIZE),
        max_header_list_size: get(setting_id::MAX_HEADER_LIST_SIZE),
    }
}

fn header(kind: u8, length: u32, flags: u8, stream_id: u32) -> [u8; HEADER_BYTES] {
    let id = stream_id.to_be_bytes();
    let [_, high, middle, low] = length.to_be_bytes();
    [high, middle, low, kind, flags, id[0], id[1], id[2], id[3]]
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2WindowPlan {
    pub value: u32,
    pub increment: u32,
}

/// The window after `amount` is credited or debited, and the update to send.
/// The caller refreshes facts after every native send/resume.
#[must_use]
pub fn sw_rs_http2_window_plan(
    current: u32,
    amount: usize,
    initial: u32,
    credit: bool,
) -> Http2WindowPlan {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream adds a size_t to a uint32_t window, which keeps the low 32 bits; \
                  this crate is below corebase, so it has no c_cast"
    )]
    let value = if credit {
        current.wrapping_add(amount as u32)
    } else {
        current.wrapping_sub(amount as u32)
    };
    let increment = if value < initial / 4 {
        initial - value
    } else {
        0
    };
    Http2WindowPlan { value, increment }
}

/// HTTP/2 error codes used by both the wire handlers and PHP registration.
pub mod error_code {
    pub const NO_ERROR: u32 = 0;
    pub const INTERNAL_ERROR: u32 = 2;
    pub const FRAME_SIZE_ERROR: u32 = 6;
}

/// The frame types this side sends (`SW_HTTP2_TYPE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Data = 0,
    Headers = 1,
    RstStream = 3,
    Settings = 4,
    Ping = 6,
    Goaway = 7,
    WindowUpdate = 8,
}

/// Wire flags from `swHttp2FrameFlag`. The frame type determines the meaning of bit zero.
pub mod frame_flag {
    pub const ACK: u32 = 0x01;
    pub const END_STREAM: u32 = 0x01;
    pub const END_HEADERS: u32 = 0x04;
    pub const PADDED: u32 = 0x08;
    pub const PRIORITY: u32 = 0x20;
}

/// Local HTTP/2 stream state from `swHttp2StreamFlag`.
pub mod stream_flag {
    pub const REQUEST_END: u32 = 1 << 0;
    pub const PIPELINE_REQUEST: u32 = 1 << 1;
    pub const PIPELINE_RESPONSE: u32 = 1 << 2;
    pub const USE_PIPELINE_READ: u32 = 1 << 3;
}

/// What a received frame asks of its session.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum FrameAction {
    Settings,
    Headers,
    Data,
    PingReply,
    WindowUpdate,
    Reset,
    Goaway,
    Continuation,
    #[default]
    Other,
    SettingsAck,
    PingAck,
    IgnorePush,
}

fn frame_action(client: bool, kind: u32, flags: u32) -> FrameAction {
    let ack = flags & frame_flag::ACK != 0;
    match kind {
        4 if ack => FrameAction::SettingsAck,
        4 => FrameAction::Settings,
        1 => FrameAction::Headers,
        0 => FrameAction::Data,
        6 if ack => FrameAction::PingAck,
        6 => FrameAction::PingReply,
        8 => FrameAction::WindowUpdate,
        3 => FrameAction::Reset,
        7 => FrameAction::Goaway,
        5 if client => FrameAction::IgnorePush,
        9 => FrameAction::Continuation,
        _ => FrameAction::Other,
    }
}

/// Where the client puts an outgoing frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueAction {
    /// Send it now.
    Send,
    /// Append it to the queue.
    Append,
    /// The queue is past its limit (strictly greater, as upstream).
    Full,
}

/// Queue storage and its allocation and release stay with the client.
#[must_use]
pub fn sw_rs_http2_queue_action(bound: bool, queued: usize, limit: u32) -> QueueAction {
    if !bound {
        QueueAction::Send
    } else if queued > limit as usize {
        QueueAction::Full
    } else {
        QueueAction::Append
    }
}

#[must_use]
pub fn sw_rs_http2_send_complete(sent: isize, wanted: usize) -> bool {
    // Native compares `send_all(...) != (ssize_t) len`: the cast wraps.
    sent == wanted.cast_signed()
}

/// Whose headers a name is looked up among.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderRole {
    /// A request the server received.
    Request,
    /// A response the client received.
    Response,
    /// A response the server sends.
    Server,
    /// A request the client sends.
    Client,
}

/// A header its role acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderKind {
    /// Any other header.
    #[default]
    Other,
    Method,
    Path,
    Authority,
    ContentType,
    Cookie,
    ContentLength,
    AcceptEncoding,
    Status,
    ContentEncoding,
    SetCookie,
    Server,
    Date,
    Host,
}

/// Match locale-folded native header bytes; Rust selects role-specific effects.
fn header_kind(role: HeaderRole, name: &[u8], logical: usize) -> HeaderKind {
    use HeaderRole::{Client, Request, Response, Server};
    if name.len() != logical {
        return HeaderKind::Other;
    }
    match (role, name) {
        (Request, b":method") => HeaderKind::Method,
        (Request, b":path") => HeaderKind::Path,
        (Request, b":authority") => HeaderKind::Authority,
        (Request | Server, b"content-type") => HeaderKind::ContentType,
        (Request, b"cookie") => HeaderKind::Cookie,
        (Request | Server, b"content-length") => HeaderKind::ContentLength,
        (Request, b"accept-encoding") => HeaderKind::AcceptEncoding,
        (Response, b":status") => HeaderKind::Status,
        (Response, b"content-encoding") => HeaderKind::ContentEncoding,
        (Response, b"set-cookie") => HeaderKind::SetCookie,
        (Server, b"server") => HeaderKind::Server,
        (Server, b"date") => HeaderKind::Date,
        (Client, b"host") => HeaderKind::Host,
        _ => HeaderKind::Other,
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2HeaderPlan {
    pub kind: HeaderKind,
    pub publish_flags: u32,
    pub suppress: bool,
    pub capture_content_type: bool,
}

/// The plan for a header whose `logical`-byte name folds to `folded`.
#[must_use]
pub fn sw_rs_http2_header_plan(
    role: HeaderRole,
    folded: &[u8],
    logical: usize,
    can_capture: bool,
) -> Http2HeaderPlan {
    let kind = header_kind(role, folded, logical);
    let server = role == HeaderRole::Server;
    let publish_flags = if server {
        match kind {
            HeaderKind::Server => 2,
            HeaderKind::Date => 16,
            HeaderKind::ContentType => 32,
            _ => 0,
        }
    } else {
        0
    };
    Http2HeaderPlan {
        kind,
        publish_flags,
        suppress: server && kind == HeaderKind::ContentLength,
        capture_content_type: server && kind == HeaderKind::ContentType && can_capture,
    }
}

pub struct Http2RespondFacts {
    pub header_sent: bool,
    pub trailer_present: bool,
    pub chunked: bool,
    pub shutting_down: bool,
    pub streams_empty: bool,
    pub body_length: usize,
}

/// A step of `Response::end` over HTTP/2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Respond {
    Start,
    Header,
    CommitEnd,
    Empty,
    Body,
    Trailer,
    ErrorClose,
    Remove,
    ErrorShutdown,
    SuccessShutdown,
    /// Return false.
    False,
    /// Return true.
    True,
}

/// The step after `previous`, which `succeeded` or not. The caller refreshes only
/// the facts needed after each step, keeping cached trailer presence and end flags.
#[must_use]
pub fn sw_rs_http2_respond_next(
    previous: Respond,
    succeeded: bool,
    facts: &Http2RespondFacts,
) -> Respond {
    use Respond::{
        Body, CommitEnd, Empty, ErrorClose, ErrorShutdown, False, Header, Remove, Start,
        SuccessShutdown, Trailer, True,
    };
    match previous {
        Start if !facts.header_sent => Header,
        Start => CommitEnd,
        Header if succeeded => CommitEnd,
        Header => False,
        // A trailer's HEADERS carry END_STREAM: a streamed response with one
        // ends with it, not with an empty DATA frame.
        CommitEnd if facts.chunked && facts.body_length == 0 && !facts.trailer_present => Empty,
        CommitEnd => Body,
        Empty | Body | Trailer if !succeeded => ErrorClose,
        Empty => Body,
        Body if facts.trailer_present => Trailer,
        Body | Trailer => Remove,
        ErrorClose if facts.shutting_down && facts.streams_empty => ErrorShutdown,
        ErrorClose => False,
        Remove if facts.shutting_down && facts.streams_empty => SuccessShutdown,
        Remove => True,
        ErrorShutdown | SuccessShutdown | False | True => False,
    }
}

#[must_use]
pub fn sw_rs_http2_response_status(status: i32) -> i32 {
    if status == 0 {
        200
    } else {
        status
    }
}

#[must_use]
pub fn sw_rs_http2_header_defaults(published: u32) -> u32 {
    (2 | 16 | 32) & !published
}

#[must_use]
pub fn sw_rs_http2_content_length_rejected(complete: bool, parsed: i64, maximum: u32) -> bool {
    !complete || parsed > i64::from(maximum)
}

/// A content type or encoding the session acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    UrlEncoded,
    Multipart,
    Gzip,
    Other,
}

/// The media kind of locale-folded bytes (at most 33 needed).
#[must_use]
pub fn sw_rs_http2_media_kind(value: &[u8]) -> Media {
    if value.starts_with(b"application/x-www-form-urlencoded") {
        Media::UrlEncoded
    } else if value.starts_with(b"multipart/form-data") {
        Media::Multipart
    } else if value.starts_with(b"gzip") {
        Media::Gzip
    } else {
        Media::Other
    }
}

/// A frame header. Integer truncation matches the native header API.
#[must_use]
pub fn sw_rs_http2_pack_frame_header(
    kind: FrameType,
    length: u32,
    flags: u32,
    stream_id: u32,
) -> [u8; HEADER_BYTES] {
    let [flags, ..] = flags.to_le_bytes();
    header(kind as u8, length, flags, stream_id)
}

/// `swoole::http2::pack_setting_frame` into the front of `out`: the frame's size, or 0
/// when `out` is shorter than the frame (45 bytes, 39 for a server).
pub fn sw_rs_http2_pack_settings(out: &mut [u8], s: &Http2Settings, server: bool) -> usize {
    let count = SETTING_COUNT - usize::from(server);
    let size = HEADER_BYTES + count * SETTING_BYTES;
    let Some(out) = out.get_mut(..size) else {
        return 0;
    };
    let values = [
        (setting_id::HEADER_TABLE_SIZE, s.header_table_size),
        (setting_id::ENABLE_PUSH, s.enable_push),
        (setting_id::MAX_CONCURRENT_STREAMS, s.max_concurrent_streams),
        (setting_id::INIT_WINDOW_SIZE, s.init_window_size),
        (setting_id::MAX_FRAME_SIZE, s.max_frame_size),
        (setting_id::MAX_HEADER_LIST_SIZE, s.max_header_list_size),
    ];
    let (head, entries) = out.split_at_mut(HEADER_BYTES);
    head.copy_from_slice(&header(
        FrameType::Settings as u8,
        u32::try_from(count * SETTING_BYTES).unwrap_or(0),
        0,
        0,
    ));
    let sent = values
        .into_iter()
        .filter(|&(id, _)| !server || id != setting_id::ENABLE_PUSH);
    for ((key, value), entry) in sent.zip(entries.as_chunks_mut::<SETTING_BYTES>().0) {
        let (id, value_bytes) = entry.split_at_mut(2);
        id.copy_from_slice(&key.to_be_bytes());
        value_bytes.copy_from_slice(&value.to_be_bytes());
    }
    size
}

/// Decode one setting: its id and value. Native code invokes the callback before
/// decoding another.
#[must_use]
pub fn sw_rs_http2_unpack_setting(b: &[u8; SETTING_BYTES]) -> (u16, u32) {
    (
        u16::from_be_bytes([b[0], b[1]]),
        u32::from_be_bytes([b[2], b[3], b[4], b[5]]),
    )
}

/// The facts of the frame header at the front of `b`; `None` before nine bytes.
#[must_use]
pub fn sw_rs_http2_decode_frame(
    b: &[u8],
    previous_last_id: u32,
    client: bool,
) -> Option<Http2FrameFacts> {
    let &[b0, b1, b2, kind, flags, i0, i1, i2, i3, ..] = b else {
        return None;
    };
    let id = u32::from_be_bytes([i0, i1, i2, i3]) & 0x7fff_ffff;
    Some(Http2FrameFacts {
        length: u32::from_be_bytes([0, b0, b1, b2]),
        flags: u32::from(flags),
        stream_id: id,
        publish_last_id: id > previous_last_id,
        action: frame_action(client, u32::from(kind), u32::from(flags)),
    })
}

/// The opaque eight-byte PING payload, echoed in its acknowledgement.
///
/// # Errors
/// RFC 9113 requires exactly eight bytes.
pub fn ping_payload(bytes: &[u8]) -> Result<[u8; PING_PAYLOAD], std::array::TryFromSliceError> {
    bytes.try_into()
}

/// The four-byte `WINDOW_UPDATE` field in network byte order.
///
/// # Errors
/// RFC 9113 requires exactly four bytes.
pub fn window_increment(bytes: &[u8]) -> Result<u32, std::array::TryFromSliceError> {
    bytes.try_into().map(u32::from_be_bytes)
}

/// The packet ended before the header or its declared payload was complete.
#[derive(Debug, PartialEq, Eq)]
pub struct ShortFrame;

/// Borrows the declared payload of the first complete frame in the buffer.
///
/// # Errors
/// Returns `ShortFrame` until both header and payload are present.
pub fn decode_packet(
    bytes: &[u8],
    previous_last_id: u32,
    client: bool,
) -> Result<(Http2FrameFacts, &[u8]), ShortFrame> {
    let (header, body) = bytes.split_at_checked(HEADER_BYTES).ok_or(ShortFrame)?;
    let facts = sw_rs_http2_decode_frame(header, previous_last_id, client).ok_or(ShortFrame)?;
    let payload = body.get(..facts.length as usize).ok_or(ShortFrame)?;
    Ok((facts, payload))
}

/// `swoole::http2::get_frame_length` over the buffered bytes: None until the nine-byte
/// frame header is buffered, then the whole frame's length.
#[must_use]
pub fn frame_length(buffered: &[u8]) -> Option<usize> {
    match buffered {
        &[b0, b1, b2, _, _, _, _, _, _, ..] => Some(
            ((usize::from(b0) << 16) | (usize::from(b1) << 8) | usize::from(b2)) + HEADER_BYTES,
        ),
        _ => None,
    }
}

/// The effects of one received setting; no native effects happen here.
#[must_use]
pub fn sw_rs_http2_setting_plan(
    id: u32,
    value: u32,
    old_value: u32,
    deflater_present: bool,
    server: bool,
) -> Http2SettingPlan {
    let id = u16::try_from(id).unwrap_or(0);
    let field = match id {
        setting_id::HEADER_TABLE_SIZE
        | setting_id::MAX_CONCURRENT_STREAMS
        | setting_id::INIT_WINDOW_SIZE
        | setting_id::MAX_FRAME_SIZE
        | setting_id::MAX_HEADER_LIST_SIZE => i32::from(id) - 1,
        _ => -1,
    };
    Http2SettingPlan {
        field,
        set_value: field >= 0
            && (id != setting_id::HEADER_TABLE_SIZE
                && (server || id != setting_id::MAX_HEADER_LIST_SIZE)
                || value != old_value),
        set_window: id == setting_id::INIT_WINDOW_SIZE,
        change_deflater: id == setting_id::HEADER_TABLE_SIZE
            && value != old_value
            && (!server || deflater_present),
    }
}

/// The largest DATA frame to send under the peer's `max_frame_size`: legacy
/// SETTINGS values outside RFC 9113's range are accepted but never used.
#[must_use]
pub fn send_frame_size(remote: u32) -> u32 {
    remote.clamp(1 << 14, (1 << 24) - 1)
}

/// The next DATA frame of `remaining` bytes under `max_frame`.
#[must_use]
pub fn sw_rs_http2_chunk(remaining: usize, max_frame: u32, flags: u32) -> Http2ChunkPlan {
    let more = remaining > max_frame as usize;
    Http2ChunkPlan {
        length: if more { max_frame as usize } else { remaining },
        flags: if more {
            0
        } else {
            u32::from(flags.to_le_bytes()[0])
        },
    }
}

/// What to do with DATA of some length under the remote window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// Send it as planned.
    Send,
    /// Warn that it exceeds the window (and send it).
    Warn,
    /// Wait for a `WINDOW_UPDATE`.
    Yield,
    /// Send what the window allows.
    Truncate,
}

#[must_use]
pub fn sw_rs_http2_window_action(length: usize, window: u32, may_suspend: bool) -> WindowAction {
    if length <= window as usize {
        WindowAction::Send
    } else if !may_suspend {
        WindowAction::Warn
    } else if window == 0 {
        WindowAction::Yield
    } else {
        WindowAction::Truncate
    }
}

#[must_use]
pub fn sw_rs_http2_stream_flags(pipeline: bool, pipeline_read: bool) -> u32 {
    (if pipeline {
        stream_flag::PIPELINE_REQUEST
    } else {
        0
    }) | (if pipeline_read {
        stream_flag::USE_PIPELINE_READ
    } else {
        0
    })
}

pub struct Http2InflateStep {
    pub end_headers: bool,
    pub finish: bool,
}

/// One inflate step; copied facts are reobserved after native header effects.
#[must_use]
pub fn sw_rs_http2_inflate_step(
    flags: u32,
    remaining: usize,
    client_role: bool,
) -> Http2InflateStep {
    let end_headers = flags & 1 != 0;
    Http2InflateStep {
        end_headers,
        finish: end_headers || remaining == 0 && (client_role || flags & 2 == 0),
    }
}

#[must_use]
pub fn sw_rs_http2_headers_flags(empty: bool, end_stream: bool, stream_flags: u32) -> u32 {
    frame_flag::END_HEADERS
        | if empty && end_stream && stream_flags & stream_flag::PIPELINE_REQUEST == 0 {
            frame_flag::END_STREAM
        } else {
            0
        }
}

/// Apply a copied stream flag transition. Callers publish into the sole native stream.
#[must_use]
pub fn sw_rs_http2_stream_update(flags: u32, end: bool, receiving: bool) -> u32 {
    flags
        | if receiving && !end {
            stream_flag::PIPELINE_RESPONSE
        } else if !receiving && end {
            stream_flag::REQUEST_END
        } else {
            0
        }
}

#[must_use]
pub fn sw_rs_http2_write_allowed(exists: bool, stream_flags: u32) -> bool {
    exists
        && stream_flags & stream_flag::PIPELINE_REQUEST != 0
        && stream_flags & stream_flag::REQUEST_END == 0
}

/// Whether a received frame ends the response and makes it ready.
#[must_use]
pub fn sw_rs_http2_response(
    action: FrameAction,
    flags: u32,
    stream_flags: u32,
    pipeline_read: bool,
) -> Http2ResponsePlan {
    let end = flags & frame_flag::END_STREAM != 0
        || matches!(action, FrameAction::Reset | FrameAction::Goaway);
    let pipeline = (pipeline_read || stream_flags & stream_flag::USE_PIPELINE_READ != 0)
        && stream_flags & stream_flag::PIPELINE_RESPONSE != 0;
    Http2ResponsePlan {
        end,
        ready: end || pipeline,
    }
}

/// `swoole::http2::get_type(int)`.
#[must_use]
pub fn get_type(kind: i32) -> &'static CStr {
    match kind {
        0 => c"DATA",
        1 => c"HEADERS",
        2 => c"PRIORITY",
        3 => c"RST_STREAM",
        4 => c"SETTINGS",
        5 => c"PUSH_PROMISE",
        6 => c"PING",
        7 => c"GOAWAY",
        8 => c"WINDOW_UPDATE",
        9 => c"CONTINUATION",
        _ => c"UNKNOWN",
    }
}

/// `swoole::http2::get_type_color(int)`: `SW_COLOR_MAGENTA`, `_GREEN`, `_WHITE` or `_RED`.
#[must_use]
pub fn get_type_color(kind: i32) -> i32 {
    match kind {
        0 | 8 => 5,
        1 | 4 | 5 | 9 => 2,
        2 | 6 => 7,
        _ => 1,
    }
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The process-wide defaults are shared by every test in this binary.
    static DEFAULTS: Mutex<()> = Mutex::new(());

    /// `swoole_rand(1, 100000)`.
    fn rand() -> u32 {
        use std::hash::{BuildHasher, RandomState};
        (RandomState::new().hash_one(0u8) % 100_000) as u32 + 1
    }

    // Upstream core-tests/src/protocol/http2.cpp, one test per TEST. `http2.ssl` is not
    // ported: it needs the nghttp2 client, so the native lane excluded it too.

    /// `TEST(http2, default_settings)`.
    #[test]
    fn http2_default_settings() {
        let _defaults = DEFAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            get_default_setting(1),
            4096,
            "SW_HTTP2_DEFAULT_HEADER_TABLE_SIZE"
        );
        assert_eq!(get_default_setting(2), 0, "SW_HTTP2_DEFAULT_ENABLE_PUSH");
        assert_eq!(
            get_default_setting(3),
            128,
            "SW_HTTP2_DEFAULT_MAX_CONCURRENT_STREAMS"
        );
        assert_eq!(
            get_default_setting(4),
            65535,
            "SW_HTTP2_DEFAULT_INIT_WINDOW_SIZE"
        );
        assert_eq!(
            get_default_setting(5),
            16384,
            "SW_HTTP2_DEFAULT_MAX_FRAME_SIZE"
        );
        assert_eq!(
            get_default_setting(6),
            u32::MAX,
            "SW_HTTP2_DEFAULT_MAX_HEADER_LIST_SIZE"
        );

        let original = sw_rs_http2_default_settings();
        let settings = [rand(), rand(), rand(), rand(), rand(), rand()];
        for (id, value) in (1..).zip(settings) {
            put_default_setting(id, value);
        }
        for (id, value) in (1..).zip(settings) {
            assert_eq!(get_default_setting(id), value);
        }
        // Each upstream case runs in its own process; these share one.
        for (id, value) in (1..).zip([
            original.header_table_size,
            original.enable_push,
            original.max_concurrent_streams,
            original.init_window_size,
            original.max_frame_size,
            original.max_header_list_size,
        ]) {
            put_default_setting(id, value);
        }
    }

    /// `TEST(http2, pack_setting_frame)`.
    #[test]
    fn http2_pack_setting_frame() {
        let _defaults = DEFAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SW_HTTP2_SETTING_FRAME_SIZE + SW_HTTP2_FRAME_HEADER_SIZE
        let mut frame = [0u8; 45 + 9];
        let settings_1 = sw_rs_http2_default_settings();
        let n = sw_rs_http2_pack_settings(&mut frame, &settings_1, false);

        assert!(n > 16);

        // unpack_setting_data over n bytes after the header, stopping at an unknown id.
        let mut settings_2 = Http2Settings::default();
        for item in frame[9..].chunks(6).take(n.div_ceil(6)) {
            let mut padded = [0u8; 6];
            padded[..item.len()].copy_from_slice(item);
            let (id, value) = sw_rs_http2_unpack_setting(&padded);
            match id {
                1 => settings_2.header_table_size = value,
                2 => settings_2.enable_push = value,
                3 => settings_2.max_concurrent_streams = value,
                4 => settings_2.init_window_size = value,
                5 => settings_2.max_frame_size = value,
                6 => settings_2.max_header_list_size = value,
                _ => break,
            }
        }

        assert_eq!(settings_1, settings_2);
    }

    /// `TEST(http2, get_type)`.
    #[test]
    fn http2_get_type() {
        let names = [
            c"DATA",
            c"HEADERS",
            c"PRIORITY",
            c"RST_STREAM",
            c"SETTINGS",
            c"PUSH_PROMISE",
            c"PING",
            c"GOAWAY",
            c"WINDOW_UPDATE",
            c"CONTINUATION",
        ];
        for (kind, name) in (0..).zip(names) {
            assert_eq!(get_type(kind), name);
        }
    }

    /// `TEST(http2, get_type_color)`.
    #[test]
    fn http2_get_type_color() {
        // SW_LOOP_N(SW_HTTP2_TYPE_GOAWAY + 2)
        for i in 0..7 + 2 {
            assert!(get_type_color(i) >= 0);
        }
    }

    #[test]
    fn default_settings_round_trip() {
        let _defaults = DEFAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            sw_rs_http2_default_settings(),
            Http2Settings {
                header_table_size: 4096,
                enable_push: 0,
                max_concurrent_streams: 128,
                init_window_size: 65535,
                max_frame_size: 16384,
                max_header_list_size: u32::MAX,
            }
        );
        put_default_setting(2, 1);
        assert_eq!(get_default_setting(2), 1);
        put_default_setting(2, 0);
    }
    #[test]
    fn send_complete_matches_the_wrapping_native_cast() {
        for wanted in [
            0,
            1,
            isize::MAX as usize,
            isize::MAX as usize + 1,
            usize::MAX,
        ] {
            for sent in [-1, 0, 1, isize::MIN, isize::MAX] {
                assert_eq!(
                    sw_rs_http2_send_complete(sent, wanted),
                    sent == wanted as isize
                );
            }
        }
        assert!(sw_rs_http2_send_complete(-1, usize::MAX));
    }
    #[test]
    fn codec_and_unknown_settings() {
        let s = Http2Settings {
            header_table_size: 0x0102_0304,
            enable_push: 1,
            max_concurrent_streams: u32::MAX,
            init_window_size: 65535,
            max_frame_size: 16384,
            max_header_list_size: 0,
        };
        for server in 0..=1 {
            let mut out = [0xcc; 47];
            let n = sw_rs_http2_pack_settings(&mut out[1..46], &s, server != 0);
            assert_eq!(n, if server == 0 { 45 } else { 39 });
            assert_eq!(out[0], 0xcc);
            assert_eq!(out[n + 1], 0xcc);
            let frame = sw_rs_http2_decode_frame(&out[1..=n], 0, false).unwrap();
            assert_eq!(
                (frame.action, frame.length),
                (FrameAction::Settings, (n - 9) as u32)
            );
            for item in out[10..=n].as_chunks::<6>().0 {
                let (id, value) = sw_rs_http2_unpack_setting(item);
                assert!((1..=6).contains(&id));
                if id == 1 {
                    assert_eq!(value, s.header_table_size);
                }
                if server != 0 {
                    assert_ne!(id, 2);
                }
            }
        }
    }
    #[test]
    fn framing_and_short_prefix() {
        let bytes = header(255, 0x00ff_ffff, 0x80, 0xffff_ffff);
        assert_eq!(frame_length(&bytes[..8]).map_or(0, usize::cast_signed), 0);
        assert_eq!(
            frame_length(&bytes).map_or(0, usize::cast_signed),
            0x00ff_ffff + 9
        );
        assert_eq!(sw_rs_http2_decode_frame(&bytes[..8], 0, false), None);
        let f = sw_rs_http2_decode_frame(&bytes, 0, false).unwrap();
        assert_eq!(f.stream_id, 0x7fff_ffff);
        assert!(f.publish_last_id);

        let mut packet = header(0, 3, 0, 1).to_vec();
        packet.extend_from_slice(b"abc");
        for end in 0..packet.len() {
            assert_eq!(decode_packet(&packet[..end], 0, true), Err(ShortFrame));
        }
        packet.extend_from_slice(&header(0, 0, 0, 3));
        assert_eq!(decode_packet(&packet, 0, true).unwrap().1, b"abc");
    }
    #[test]
    fn role_specific_setting_effects() {
        for id in (0..=7).chain([0x10001, u32::MAX]) {
            for server in [false, true] {
                for has_deflater in [false, true] {
                    let p = sw_rs_http2_setting_plan(id, 2, 1, has_deflater, server);
                    assert_eq!(
                        p.field,
                        if [1, 3, 4, 5, 6].contains(&id) {
                            id as i32 - 1
                        } else {
                            -1
                        }
                    );
                    assert_eq!(p.change_deflater, id == 1 && (!server || has_deflater));
                    assert_eq!(p.set_window, id == 4);
                }
            }
        }
    }
    #[test]
    fn frames_are_sent_within_the_rfc_frame_size_range() {
        assert_eq!(send_frame_size(0), 16384);
        assert_eq!(send_frame_size(16383), 16384);
        assert_eq!(send_frame_size(40000), 40000);
        assert_eq!(send_frame_size(16_777_216), 16_777_215);
    }

    #[test]
    fn chunk_window_and_pipeline_bits() {
        assert_eq!(
            sw_rs_http2_chunk(20, 16, 1),
            Http2ChunkPlan {
                length: 16,
                flags: 0
            }
        );
        assert_eq!(sw_rs_http2_chunk(16, 16, 257).flags, 1);
        assert_eq!(sw_rs_http2_window_action(16, 16, false), WindowAction::Send);
        assert_eq!(sw_rs_http2_window_action(16, 0, false), WindowAction::Warn);
        assert_eq!(sw_rs_http2_window_action(16, 0, true), WindowAction::Yield);
        assert_eq!(
            sw_rs_http2_window_action(16, 8, true),
            WindowAction::Truncate
        );
        for flags in 0..16 {
            for read in [false, true] {
                for kind in 0..=9 {
                    for frame_flags in 0..8 {
                        let action = frame_action(true, kind, frame_flags);
                        let p = sw_rs_http2_response(action, frame_flags, flags, read);
                        let end = frame_flags & 1 != 0 || kind == 3 || kind == 7;
                        assert_eq!(p.end, end);
                        assert_eq!(p.ready, end || ((read || flags & 8 != 0) && flags & 4 != 0));
                    }
                }
            }
        }
        assert_eq!(sw_rs_http2_stream_flags(true, true), 10);
        assert_eq!(sw_rs_http2_headers_flags(true, true, 0), 5);
        assert_eq!(sw_rs_http2_headers_flags(true, true, 2), 4);
        assert!(!sw_rs_http2_write_allowed(false, 2));
        assert!(!sw_rs_http2_write_allowed(true, 3));
        assert!(sw_rs_http2_write_allowed(true, 2));
    }

    /// Upstream's `SW_STRCASEEQ` role predicates (the client and server header
    /// parsers) over the unfolded name.
    fn upstream_kind(role: HeaderRole, name: &[u8]) -> HeaderKind {
        use HeaderKind::*;
        let colon = name.first() == Some(&b':');
        let keys: &[(&str, HeaderKind)] = match role {
            HeaderRole::Request if colon => {
                &[("method", Method), ("path", Path), ("authority", Authority)]
            }
            HeaderRole::Request => &[
                ("content-type", ContentType),
                ("cookie", Cookie),
                ("content-length", ContentLength),
                ("accept-encoding", AcceptEncoding),
            ],
            HeaderRole::Response if colon => &[("status", Status)],
            HeaderRole::Response => &[
                ("content-encoding", ContentEncoding),
                ("set-cookie", SetCookie),
            ],
            HeaderRole::Server => &[
                ("server", Server),
                ("content-length", ContentLength),
                ("date", Date),
                ("content-type", ContentType),
            ],
            HeaderRole::Client => &[("host", Host)],
        };
        let parsed = matches!(role, HeaderRole::Request | HeaderRole::Response);
        let text = if parsed && colon { &name[1..] } else { name };
        keys.iter()
            .find(|(key, _)| text.eq_ignore_ascii_case(key.as_bytes()))
            .map_or(Other, |(_, kind)| *kind)
    }

    fn upstream_media(value: &[u8]) -> Media {
        let starts = |prefix: &str| {
            value.len() >= prefix.len()
                && value[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
        };
        if starts("application/x-www-form-urlencoded") {
            Media::UrlEncoded
        } else if starts("multipart/form-data") {
            Media::Multipart
        } else if starts("gzip") {
            Media::Gzip
        } else {
            Media::Other
        }
    }

    /// crates/phphttp2's fold: at most 33 bytes, lowercased.
    fn folded(bytes: &[u8]) -> ([u8; 33], usize) {
        let mut out = [0_u8; 33];
        let n = bytes.len().min(out.len());
        for (slot, byte) in out.iter_mut().zip(&bytes[..n]) {
            *slot = byte.to_ascii_lowercase();
        }
        (out, n)
    }

    #[test]
    fn header_lookups_match_upstreams_predicates() {
        let mut names: Vec<Vec<u8>> = [
            "",
            ":status",
            ":STATUS",
            ":method",
            ":path",
            ":authority",
            "content-type",
            "CONTENT-TYPE",
            "content-length",
            "accept-encoding",
            "content-encoding",
            "set-cookie",
            "Set-Cookie",
            "server",
            "date",
            "HOST",
            "cookie",
        ]
        .iter()
        .map(|name| name.as_bytes().to_vec())
        .collect();
        names.push(vec![b'a'; 200]);
        for c in 0..=255_u8 {
            names.push([&[c][..], b"status"].concat());
            names.push([&b"content-"[..], &[c], b"ype"].concat());
            names.push([&b"set-cookie"[..], &[c]].concat());
        }
        for name in &names {
            let (bytes, n) = folded(name);
            for role in [
                HeaderRole::Request,
                HeaderRole::Response,
                HeaderRole::Server,
                HeaderRole::Client,
            ] {
                let plan = sw_rs_http2_header_plan(role, &bytes[..n], name.len(), false);
                assert_eq!(
                    plan.kind,
                    upstream_kind(role, name),
                    "role {role:?} {name:?}"
                );
            }
        }
        let values: [&[u8]; 13] = [
            b"",
            b"200",
            b"00301",
            b"application/x-www-form-urlencoded",
            b"APPLICATION/X-WWW-FORM-URLENCODED; charset=x",
            b"multipart/form-data; boundary=a",
            b"MULTIPART/FORM-DATA",
            b"gzip",
            b"GZip;stuff",
            b"gzipgarbage",
            b"a=b;c=d",
            b"gzip\0tail",
            &[b'a'; 40],
        ];
        for value in values {
            let (bytes, n) = folded(value);
            let kind = sw_rs_http2_media_kind(&bytes[..n]);
            assert_eq!(kind, upstream_media(value), "{value:?}");
        }
    }

    #[test]
    fn unknown_frame_types() {
        for kind in [-100, -1, 10, 99, i32::MIN, i32::MAX] {
            assert_eq!(get_type(kind), c"UNKNOWN");
            assert_eq!(get_type_color(kind), 1, "SW_COLOR_RED");
        }
    }
}

#[cfg(kani)]
#[path = "../../../tests/modules/http2/kani.rs"]
mod proofs;
