//! HTTP/2 policy for crates/phphttp2, and `src/protocol/http2.cc`'s functions under
//! their upstream symbols.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod receive;
pub use receive::*;
mod files;
pub use files::*;
use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2Settings {
    pub header_table_size: u32,
    pub enable_push: u32,
    pub max_concurrent_streams: u32,
    pub init_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2FrameFacts {
    pub length: u32,
    pub kind: u32,
    pub flags: u32,
    pub stream_id: u32,
    pub publish_last_id: u32,
    pub action: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2SettingPlan {
    pub field: i32,
    pub set_value: u32,
    pub set_window: u32,
    pub change_deflater: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2ChunkPlan {
    pub length: usize,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2ResponsePlan {
    pub end: u32,
    pub ready: u32,
}

/// The process-wide defaults (`SW_HTTP2_DEFAULT_*`), indexed by setting id - 1:
/// header table, push, concurrent streams, initial window, frame size, header list.
static DEFAULT_SETTINGS: [AtomicU32; 6] = [
    AtomicU32::new(1 << 12),
    AtomicU32::new(0),
    AtomicU32::new(u32::MAX),
    AtomicU32::new((1 << 16) - 1),
    AtomicU32::new(1 << 14),
    AtomicU32::new(u32::MAX),
];

/// The slot of a setting id. Upstream asserts on any other id, so this aborts.
fn default_slot(id: u32) -> &'static AtomicU32 {
    usize::try_from(id)
        .ok()
        .and_then(|id| DEFAULT_SETTINGS.get(id.checked_sub(1)?))
        .unwrap_or_else(|| std::process::abort())
}

/// `swoole::http2::put_default_setting(swHttp2SettingId, uint32_t)`.
#[unsafe(export_name = "_ZN6swoole5http219put_default_settingE16swHttp2SettingIdj")]
pub extern "C" fn put_default_setting(id: u32, value: u32) {
    default_slot(id).store(value, Ordering::Relaxed);
}

/// `swoole::http2::get_default_setting(swHttp2SettingId)`.
#[must_use]
#[unsafe(export_name = "_ZN6swoole5http219get_default_settingE16swHttp2SettingId")]
pub extern "C" fn get_default_setting(id: u32) -> u32 {
    default_slot(id).load(Ordering::Relaxed)
}

/// All defaults, for the server's SETTINGS frame.
#[no_mangle]
pub extern "C" fn sw_rs_http2_default_settings() -> Http2Settings {
    let get = get_default_setting;
    Http2Settings {
        header_table_size: get(1),
        enable_push: get(2),
        max_concurrent_streams: get(3),
        init_window_size: get(4),
        max_frame_size: get(5),
        max_header_list_size: get(6),
    }
}

fn header(kind: u8, length: u32, flags: u8, stream_id: u32) -> [u8; 9] {
    let id = stream_id.to_be_bytes();
    let [_, high, middle, low] = length.to_be_bytes();
    [high, middle, low, kind, flags, id[0], id[1], id[2], id[3]]
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2WindowPlan {
    pub value: u32,
    pub increment: u32,
}

/// # Safety
/// `out` is writable POD. The caller refreshes facts after every native send/resume.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_window_plan(
    current: u32,
    amount: usize,
    initial: u32,
    credit: u32,
    out: *mut Http2WindowPlan,
) {
    if out.is_null() {
        return;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native uint32_t window arithmetic"
    )]
    let value = if credit != 0 {
        current.wrapping_add(amount as u32)
    } else {
        current.wrapping_sub(amount as u32)
    };
    let increment = if value < initial / 4 {
        initial - value
    } else {
        0
    };
    ptr::write(out, Http2WindowPlan { value, increment });
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_data_rejected(
    current: usize,
    additional: usize,
    maximum: u32,
    ended: u32,
) -> u32 {
    u32::from(current.wrapping_add(additional) > maximum as usize || ended != 0)
}

#[repr(u32)]
enum FrameAction {
    Settings,
    Headers,
    Data,
    PingReply,
    WindowUpdate,
    Reset,
    Goaway,
    Other,
    SettingsAck,
    PingAck,
    IgnorePush,
}

fn frame_action(client: u32, kind: u32, flags: u32) -> u32 {
    let ack = flags & 1 != 0;
    let action = match kind {
        4 if ack => FrameAction::SettingsAck,
        4 => FrameAction::Settings,
        1 => FrameAction::Headers,
        0 => FrameAction::Data,
        6 if ack => FrameAction::PingAck,
        6 => FrameAction::PingReply,
        8 => FrameAction::WindowUpdate,
        3 => FrameAction::Reset,
        7 => FrameAction::Goaway,
        5 if client != 0 => FrameAction::IgnorePush,
        _ => FrameAction::Other,
    };
    action as u32
}

/// Queue storage and native allocation/release remain owned by the client.
#[no_mangle]
pub extern "C" fn sw_rs_http2_queue_action(bound: u32, queued: usize, limit: u32) -> u32 {
    if bound == 0 {
        0 // direct send
    } else if queued > limit as usize {
        2 // preserve the original strict greater-than limit
    } else {
        1 // append one native-owned frame
    }
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_send_complete(sent: isize, wanted: usize) -> u32 {
    // Native compares `send_all(...) != (ssize_t) len`: the cast wraps.
    u32::from(sent == wanted.cast_signed())
}

/// Match locale-folded native header bytes; Rust selects role-specific effects.
/// # Safety
/// `folded` addresses `copied` initialized bytes, or is null when copied is zero.
unsafe fn header_kind(role: u32, folded: *const u8, copied: usize, logical: usize) -> u32 {
    if copied == 0 || copied != logical || folded.is_null() {
        return 0;
    }
    let name = std::slice::from_raw_parts(folded, copied);
    match (role, name) {
        (0, b":method") => 1,
        (0, b":path") => 2,
        (0, b":authority") => 3,
        (0 | 2, b"content-type") => 4,
        (0, b"cookie") => 5,
        (0 | 2, b"content-length") => 6,
        (0, b"accept-encoding") => 7,
        (1, b":status") => 8,
        (1, b"content-encoding") => 9,
        (1, b"set-cookie") => 10,
        (2, b"server") => 11,
        (2, b"date") => 12,
        (3, b"host") => 13,
        _ => 0,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Http2HeaderPlan {
    pub kind: u32,
    pub publish_flags: u32,
    pub suppress: u32,
    pub capture_content_type: u32,
}

/// # Safety
/// `folded` addresses `copied` initialized bytes; `out` is writable POD.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_header_plan(
    role: u32,
    folded: *const u8,
    copied: usize,
    logical: usize,
    can_capture: u32,
    out: *mut Http2HeaderPlan,
) {
    if out.is_null() {
        return;
    }
    let kind = header_kind(role, folded, copied, logical);
    let server = role == 2;
    let publish_flags = if server {
        match kind {
            11 => 2,
            12 => 16,
            4 => 32,
            _ => 0,
        }
    } else {
        0
    };
    ptr::write(
        out,
        Http2HeaderPlan {
            kind,
            publish_flags,
            suppress: u32::from(server && kind == 6),
            capture_content_type: u32::from(server && kind == 4 && can_capture != 0),
        },
    );
}

#[repr(C)]
pub struct Http2RespondFacts {
    pub header_sent: u32,
    pub trailer_present: u32,
    pub chunked: u32,
    pub shutting_down: u32,
    pub streams_empty: u32,
    pub body_length: usize,
}

// Module-local effect tags mirror the private native executor.
mod respond_action {
    pub(crate) const START: u32 = 0;
    pub(crate) const HEADER: u32 = 1;
    pub(crate) const COMMIT_END: u32 = 2;
    pub(crate) const EMPTY: u32 = 3;
    pub(crate) const BODY: u32 = 4;
    pub(crate) const TRAILER: u32 = 5;
    pub(crate) const ERROR_CLOSE: u32 = 6;
    pub(crate) const REMOVE: u32 = 7;
    pub(crate) const ERROR_SHUTDOWN: u32 = 8;
    pub(crate) const SUCCESS_SHUTDOWN: u32 = 9;
    pub(crate) const FALSE: u32 = 10;
    pub(crate) const TRUE: u32 = 11;
}

/// Choose one native effect. No borrowed state or Rust frame survives its execution.
///
/// # Safety
/// `facts` addresses initialized POD; the caller refreshes only the facts needed
/// after each native effect, preserving cached trailer presence and end flags.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_respond_next(
    previous: u32,
    succeeded: u32,
    facts: *const Http2RespondFacts,
) -> u32 {
    use respond_action::{
        BODY, COMMIT_END, EMPTY, ERROR_CLOSE, ERROR_SHUTDOWN, FALSE, HEADER, REMOVE, START,
        SUCCESS_SHUTDOWN, TRAILER, TRUE,
    };
    if facts.is_null() {
        return FALSE;
    }
    let facts = &*facts;
    match previous {
        START if facts.header_sent == 0 => HEADER,
        START => COMMIT_END,
        HEADER if succeeded != 0 => COMMIT_END,
        HEADER => FALSE,
        // A trailer's HEADERS carry END_STREAM: a streamed response with one
        // ends with it, not with an empty DATA frame.
        COMMIT_END
            if facts.chunked != 0 && facts.body_length == 0 && facts.trailer_present == 0 =>
        {
            EMPTY
        }
        COMMIT_END => BODY,
        EMPTY | BODY | TRAILER if succeeded == 0 => ERROR_CLOSE,
        EMPTY => BODY,
        BODY if facts.trailer_present != 0 => TRAILER,
        BODY | TRAILER => REMOVE,
        ERROR_CLOSE if facts.shutting_down != 0 && facts.streams_empty != 0 => ERROR_SHUTDOWN,
        ERROR_CLOSE => FALSE,
        REMOVE if facts.shutting_down != 0 && facts.streams_empty != 0 => SUCCESS_SHUTDOWN,
        REMOVE => TRUE,
        _ => FALSE,
    }
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_response_status(status: i32) -> i32 {
    if status == 0 {
        200
    } else {
        status
    }
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_header_defaults(published: u32) -> u32 {
    (2 | 16 | 32) & !published
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_content_length_rejected(
    complete: u32,
    parsed: i64,
    maximum: u32,
) -> u32 {
    u32::from(complete == 0 || parsed > i64::from(maximum))
}

/// # Safety
/// `folded` addresses `copied` initialized locale-folded bytes (maximum 33 needed).
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_media_kind(folded: *const u8, copied: usize) -> u32 {
    if copied == 0 || folded.is_null() {
        return 0;
    }
    let value = std::slice::from_raw_parts(folded, copied);
    if value.starts_with(b"application/x-www-form-urlencoded") {
        1
    } else if value.starts_with(b"multipart/form-data") {
        2
    } else if value.starts_with(b"gzip") {
        3
    } else {
        0
    }
}

/// # Safety
/// `out` addresses nine writable bytes. Integer truncation matches the native header API.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_pack_frame_header(
    out: *mut u8,
    kind: u32,
    length: u32,
    flags: u32,
    stream_id: u32,
) {
    if !out.is_null() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the native uint8_t frame type and flags"
        )]
        let bytes = header(kind as u8, length, flags as u8, stream_id);
        ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    }
}

/// # Safety
/// `out` addresses `capacity` writable bytes; `settings` addresses an initialized POD.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_pack_settings(
    out: *mut u8,
    capacity: usize,
    settings: *const Http2Settings,
    server: u32,
) -> usize {
    let count = if server != 0 { 5 } else { 6 };
    let size = 9 + count * 6;
    if capacity < size || out.is_null() || settings.is_null() {
        return 0;
    }
    let s = ptr::read(settings);
    let values = [
        s.header_table_size,
        s.enable_push,
        s.max_concurrent_streams,
        s.init_window_size,
        s.max_frame_size,
        s.max_header_list_size,
    ];
    let mut bytes = [0u8; 45];
    let (head, entries) = bytes.split_at_mut(9);
    head.copy_from_slice(&header(4, u32::try_from(count * 6).unwrap_or(0), 0, 0));
    let sent = values
        .iter()
        .enumerate()
        .filter(|&(index, _)| server == 0 || index != 1);
    for ((index, value), entry) in sent.zip(entries.as_chunks_mut::<6>().0) {
        let (id, value_bytes) = entry.split_at_mut(2);
        id.copy_from_slice(&u16::try_from(index + 1).unwrap_or(0).to_be_bytes());
        value_bytes.copy_from_slice(&value.to_be_bytes());
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), out, size);
    size
}

/// `swoole::http2::pack_setting_frame(char *, const Settings &, bool)`.
/// # Safety
/// `out` addresses a whole frame (45 bytes, 39 for a server); `settings` is initialized.
#[unsafe(export_name = "_ZN6swoole5http218pack_setting_frameEPcRKNS0_8SettingsEb")]
pub unsafe extern "C" fn pack_setting_frame(
    out: *mut u8,
    settings: *const Http2Settings,
    server: bool,
) -> usize {
    sw_rs_http2_pack_settings(
        out,
        if server { 39 } else { 45 },
        settings,
        u32::from(server),
    )
}

/// Decode one item only. Native code invokes the callback before decoding another.
/// # Safety
/// `bytes` addresses six initialized bytes, including original backing-storage
/// preconditions when the caller's positive logical length is smaller than six.
/// Outputs each address initialized writable scalar storage.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_unpack_setting(
    bytes: *const u8,
    id: *mut u16,
    value: *mut u32,
) {
    if bytes.is_null() || id.is_null() || value.is_null() {
        return;
    }
    let mut b = [0u8; 6];
    ptr::copy_nonoverlapping(bytes, b.as_mut_ptr(), 6);
    ptr::write(id, u16::from_be_bytes([b[0], b[1]]));
    ptr::write(value, u32::from_be_bytes([b[2], b[3], b[4], b[5]]));
}

/// # Safety
/// For `available >= 9`, bytes addresses nine initialized bytes. `out` is writable POD.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_decode_frame(
    bytes: *const u8,
    available: usize,
    previous_last_id: u32,
    client: u32,
    out: *mut Http2FrameFacts,
) -> i32 {
    if available < 9 || bytes.is_null() || out.is_null() {
        return 0;
    }
    let mut b = [0u8; 9];
    ptr::copy_nonoverlapping(bytes, b.as_mut_ptr(), 9);
    let id = u32::from_be_bytes([b[5], b[6], b[7], b[8]]) & 0x7fff_ffff;
    ptr::write(
        out,
        Http2FrameFacts {
            length: (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]),
            kind: u32::from(b[3]),
            flags: u32::from(b[4]),
            stream_id: id,
            publish_last_id: u32::from(id > previous_last_id),
            action: frame_action(client, u32::from(b[3]), u32::from(b[4])),
        },
    );
    1
}

/// `swoole::PacketLength`.
#[repr(C)]
pub struct PacketLength {
    pub buf: *const u8,
    pub buf_size: u32,
    pub header_len: u32,
}

/// `swoole::http2::get_frame_length(const Protocol *, Socket *, PacketLength *)`: 0 until
/// the nine-byte frame header is buffered, then the whole frame's length.
/// # Safety
/// `pl` is valid and its `buf` addresses `buf_size` initialized bytes.
#[unsafe(export_name = "_ZN6swoole5http216get_frame_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE")]
pub unsafe extern "C" fn get_frame_length(
    _protocol: *const c_void,
    _conn: *mut c_void,
    pl: *const PacketLength,
) -> isize {
    let pl = &*pl;
    if pl.buf_size < 9 {
        return 0;
    }
    let mut b = [0u8; 4];
    ptr::copy_nonoverlapping(pl.buf, b.as_mut_ptr().add(1), 3);
    isize::try_from(u32::from_be_bytes(b)).map_or(-1, |length| length + 9)
}

/// # Safety
/// `out` is writable POD; no native effects happen here.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_setting_plan(
    id: u32,
    value: u32,
    old_value: u32,
    deflater_present: u32,
    server: u32,
    out: *mut Http2SettingPlan,
) {
    if out.is_null() {
        return;
    }
    let field = match id {
        1 | 3 | 4 | 5 | 6 => (id - 1).cast_signed(),
        _ => -1,
    };
    ptr::write(
        out,
        Http2SettingPlan {
            field,
            set_value: u32::from(
                field >= 0 && (id != 1 && (server != 0 || id != 6) || value != old_value),
            ),
            set_window: u32::from(id == 4),
            change_deflater: u32::from(
                id == 1 && value != old_value && (server == 0 || deflater_present != 0),
            ),
        },
    );
}

/// The largest DATA frame to send under the peer's `max_frame_size`: legacy
/// SETTINGS values outside RFC 9113's range are accepted but never used.
#[must_use]
pub fn send_frame_size(remote: u32) -> u32 {
    remote.clamp(1 << 14, (1 << 24) - 1)
}

/// # Safety
/// `out` is writable POD.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_chunk(
    remaining: usize,
    max_frame: u32,
    flags: u32,
    out: *mut Http2ChunkPlan,
) {
    if out.is_null() {
        return;
    }
    let more = remaining > max_frame as usize;
    ptr::write(
        out,
        Http2ChunkPlan {
            length: if more { max_frame as usize } else { remaining },
            flags: if more {
                0
            } else {
                u32::from(flags.to_le_bytes()[0])
            },
        },
    );
}

/// 0: send as planned, 1: native warning, 2: native yield, 3: truncate to window.
#[no_mangle]
pub extern "C" fn sw_rs_http2_window_action(length: usize, window: u32, may_suspend: u32) -> u32 {
    if length <= window as usize {
        0
    } else if may_suspend == 0 {
        1
    } else if window == 0 {
        2
    } else {
        3
    }
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_stream_flags(pipeline: u32, pipeline_read: u32) -> u32 {
    (if pipeline != 0 { 2 } else { 0 }) | (if pipeline_read != 0 { 8 } else { 0 })
}

#[repr(C)]
pub struct Http2InflateStep {
    pub end_headers: u32,
    pub finish: u32,
}

/// # Safety
/// `out` is writable POD; copied facts are reobserved after native header effects.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_inflate_step(
    flags: u32,
    remaining: usize,
    client_role: u32,
    out: *mut Http2InflateStep,
) {
    if out.is_null() {
        return;
    }
    let end_headers = flags & 1 != 0;
    ptr::write(
        out,
        Http2InflateStep {
            end_headers: u32::from(end_headers),
            finish: u32::from(
                end_headers || remaining == 0 && (client_role != 0 || flags & 2 == 0),
            ),
        },
    );
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_headers_flags(empty: u32, end_stream: u32, stream_flags: u32) -> u32 {
    4 | u32::from(empty != 0 && end_stream != 0 && stream_flags & 2 == 0)
}

/// Apply a copied stream flag transition. Callers publish into the sole native stream.
#[no_mangle]
pub extern "C" fn sw_rs_http2_stream_update(flags: u32, end: u32, receiving: u32) -> u32 {
    flags
        | if receiving != 0 && end == 0 {
            4
        } else {
            u32::from(receiving == 0 && end != 0)
        }
}

#[no_mangle]
pub extern "C" fn sw_rs_http2_write_allowed(exists: u32, stream_flags: u32) -> u32 {
    u32::from(exists != 0 && stream_flags & 2 != 0 && stream_flags & 1 == 0)
}

/// # Safety
/// `out` is writable POD.
#[no_mangle]
pub unsafe extern "C" fn sw_rs_http2_response(
    kind: u32,
    flags: u32,
    stream_flags: u32,
    pipeline_read: u32,
    out: *mut Http2ResponsePlan,
) {
    if out.is_null() {
        return;
    }
    let end = flags & 1 != 0 || kind == 3 || kind == 7;
    let pipeline = (pipeline_read != 0 || stream_flags & 8 != 0) && stream_flags & 4 != 0;
    ptr::write(
        out,
        Http2ResponsePlan {
            end: u32::from(end),
            ready: u32::from(end || pipeline),
        },
    );
}

/// `swoole::http2::get_type(int)`.
#[must_use]
#[unsafe(export_name = "_ZN6swoole5http28get_typeEi")]
pub extern "C" fn get_type(kind: i32) -> *const c_char {
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
    .as_ptr()
}

/// `swoole::http2::get_type_color(int)`: `SW_COLOR_MAGENTA`, `_GREEN`, `_WHITE` or `_RED`.
#[must_use]
#[unsafe(export_name = "_ZN6swoole5http214get_type_colorEi")]
pub extern "C" fn get_type_color(kind: i32) -> i32 {
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
            u32::MAX,
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
        let n = unsafe { pack_setting_frame(frame.as_mut_ptr(), &raw const settings_1, false) };

        assert!(n > 16);

        // unpack_setting_data over n bytes after the header, stopping at an unknown id.
        let mut settings_2 = Http2Settings::default();
        for item in frame[9..].chunks(6).take(n.div_ceil(6)) {
            let mut padded = [0u8; 6];
            padded[..item.len()].copy_from_slice(item);
            let (mut id, mut value) = (0, 0);
            unsafe { sw_rs_http2_unpack_setting(padded.as_ptr(), &raw mut id, &raw mut value) };
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
            assert_eq!(unsafe { std::ffi::CStr::from_ptr(get_type(kind)) }, name);
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
                max_concurrent_streams: u32::MAX,
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
                    u32::from(sent == wanted as isize)
                );
            }
        }
        assert_eq!(sw_rs_http2_send_complete(-1, usize::MAX), 1);
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
            let n = unsafe {
                sw_rs_http2_pack_settings(out.as_mut_ptr().add(1), 45, &raw const s, server)
            };
            assert_eq!(n, if server == 0 { 45 } else { 39 });
            assert_eq!(out[0], 0xcc);
            assert_eq!(out[n + 1], 0xcc);
            let mut frame = Http2FrameFacts::default();
            assert_eq!(
                unsafe { sw_rs_http2_decode_frame(out.as_ptr().add(1), n, 0, 0, &raw mut frame) },
                1
            );
            assert_eq!((frame.kind, frame.length), (4, (n - 9) as u32));
            for item in out[10..=n].as_chunks::<6>().0 {
                let mut id = 0;
                let mut value = 0;
                unsafe { sw_rs_http2_unpack_setting(item.as_ptr(), &raw mut id, &raw mut value) };
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
        let length = |buf_size| {
            let pl = PacketLength {
                buf: bytes.as_ptr(),
                buf_size,
                header_len: 0,
            };
            unsafe { get_frame_length(ptr::null(), ptr::null_mut(), &raw const pl) }
        };
        assert_eq!(length(8), 0);
        assert_eq!(length(9), 0x00ff_ffff + 9);
        let mut f = Http2FrameFacts::default();
        assert_eq!(
            unsafe { sw_rs_http2_decode_frame(bytes.as_ptr(), 9, 0, 0, &raw mut f) },
            1
        );
        assert_eq!(f.stream_id, 0x7fff_ffff);
        assert_eq!(f.publish_last_id, 1);
    }
    #[test]
    fn role_specific_setting_effects() {
        for id in 0..=7 {
            for server in 0..=1 {
                for has_deflater in 0..=1 {
                    let mut p = Http2SettingPlan::default();
                    unsafe { sw_rs_http2_setting_plan(id, 2, 1, has_deflater, server, &raw mut p) };
                    assert_eq!(
                        p.field,
                        if [1, 3, 4, 5, 6].contains(&id) {
                            id as i32 - 1
                        } else {
                            -1
                        }
                    );
                    assert_eq!(
                        p.change_deflater,
                        u32::from(id == 1 && (server == 0 || has_deflater != 0))
                    );
                    assert_eq!(p.set_window, u32::from(id == 4));
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
        let mut chunk = Http2ChunkPlan::default();
        unsafe { sw_rs_http2_chunk(20, 16, 1, &raw mut chunk) };
        assert_eq!(
            chunk,
            Http2ChunkPlan {
                length: 16,
                flags: 0
            }
        );
        unsafe { sw_rs_http2_chunk(16, 16, 257, &raw mut chunk) };
        assert_eq!(chunk.flags, 1);
        assert_eq!(sw_rs_http2_window_action(16, 16, 0), 0);
        assert_eq!(sw_rs_http2_window_action(16, 0, 0), 1);
        assert_eq!(sw_rs_http2_window_action(16, 0, 1), 2);
        assert_eq!(sw_rs_http2_window_action(16, 8, 1), 3);
        for flags in 0..16 {
            for read in 0..=1 {
                for kind in 0..=9 {
                    for frame_flags in 0..8 {
                        let mut p = Http2ResponsePlan::default();
                        unsafe { sw_rs_http2_response(kind, frame_flags, flags, read, &raw mut p) };
                        let end = frame_flags & 1 != 0 || kind == 3 || kind == 7;
                        assert_eq!(p.end, u32::from(end));
                        assert_eq!(
                            p.ready,
                            u32::from(end || ((read != 0 || flags & 8 != 0) && flags & 4 != 0))
                        );
                    }
                }
            }
        }
        assert_eq!(sw_rs_http2_stream_flags(1, 1), 10);
        assert_eq!(sw_rs_http2_headers_flags(1, 1, 0), 5);
        assert_eq!(sw_rs_http2_headers_flags(1, 1, 2), 4);
        assert_eq!(sw_rs_http2_write_allowed(0, 2), 0);
        assert_eq!(sw_rs_http2_write_allowed(1, 3), 0);
        assert_eq!(sw_rs_http2_write_allowed(1, 2), 1);
    }

    /// Upstream's `SW_STRCASEEQ` role predicates (the client and server header
    /// parsers) over the unfolded name.
    fn upstream_kind(role: u32, name: &[u8]) -> u32 {
        let colon = name.first() == Some(&b':');
        let keys: &[(&str, u32)] = match role {
            0 if colon => &[("method", 1), ("path", 2), ("authority", 3)],
            0 => &[
                ("content-type", 4),
                ("cookie", 5),
                ("content-length", 6),
                ("accept-encoding", 7),
            ],
            1 if colon => &[("status", 8)],
            1 => &[("content-encoding", 9), ("set-cookie", 10)],
            2 => &[
                ("server", 11),
                ("content-length", 6),
                ("date", 12),
                ("content-type", 4),
            ],
            _ => &[("host", 13)],
        };
        let text = if role < 2 && colon { &name[1..] } else { name };
        keys.iter()
            .find(|(key, _)| text.eq_ignore_ascii_case(key.as_bytes()))
            .map_or(0, |(_, kind)| *kind)
    }

    fn upstream_media(value: &[u8]) -> u32 {
        let starts = |prefix: &str| {
            value.len() >= prefix.len()
                && value[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
        };
        if starts("application/x-www-form-urlencoded") {
            1
        } else if starts("multipart/form-data") {
            2
        } else if starts("gzip") {
            3
        } else {
            0
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
            for role in 0..4 {
                let mut plan = Http2HeaderPlan::default();
                unsafe {
                    sw_rs_http2_header_plan(role, bytes.as_ptr(), n, name.len(), 0, &raw mut plan);
                }
                assert_eq!(plan.kind, upstream_kind(role, name), "role {role} {name:?}");
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
            let kind = unsafe { sw_rs_http2_media_kind(bytes.as_ptr(), n) };
            assert_eq!(kind, upstream_media(value), "{value:?}");
        }
    }

    #[test]
    fn unknown_frame_types() {
        for kind in [-100, -1, 10, 99, i32::MIN, i32::MAX] {
            assert_eq!(
                unsafe { std::ffi::CStr::from_ptr(get_type(kind)) },
                c"UNKNOWN"
            );
            assert_eq!(get_type_color(kind), 1, "SW_COLOR_RED");
        }
    }
}
