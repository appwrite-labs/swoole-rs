//! The HTTP/2 server's frames, responses and request header blocks: upstream's
//! `swoole_http2_server_parse` and everything it and `Response` send (headers
//! over nghttp2's HPACK, bodies under flow control, trailers, files), over
//! the sessions and streams of `session`. The HTTP contexts are C++'s,
//! reached through the `swr_h2s_*` hosts of `ext-src/swoole_http2_server.cc`
//! (`native/http2/swoole_rs_phphttp2.h`).
use crate::native::{
    Header, swoole_http2_client_coro_deflate, swoole_http2_client_coro_deflate_bound,
    swoole_http2_client_coro_deflate_table, swoole_http2_client_coro_deflater,
    swoole_http2_client_coro_inflate, swoole_http2_client_coro_inflate_end,
    swoole_http2_client_coro_inflater, swoole_http2_client_coro_strerror,
};
use crate::session::{Session, SessionState, Stream};
use libc::{c_char, c_int, c_void};
use std::{
    ffi::{CStr, CString},
    ptr,
};
use swoole_rs_http2::{
    Http2ChunkPlan, Http2FrameFacts, Http2HeaderPlan, Http2InflateStep, Http2ReceiveFacts,
    Http2RespondFacts, Http2SettingPlan, Http2WindowPlan, receive_action as recv,
    sw_rs_http2_chunk, sw_rs_http2_content_length_rejected, sw_rs_http2_decode_frame,
    sw_rs_http2_file_complete, sw_rs_http2_header_defaults, sw_rs_http2_header_plan,
    sw_rs_http2_headers_flags, sw_rs_http2_inflate_step, sw_rs_http2_media_kind,
    sw_rs_http2_pack_frame_header, sw_rs_http2_receive_next, sw_rs_http2_respond_next,
    sw_rs_http2_response_status, sw_rs_http2_setting_plan, sw_rs_http2_window_action,
    sw_rs_http2_window_plan,
};
use swoole_rs_zend::{
    bailout, string,
    sys::{
        IS_ARRAY, IS_NULL, IS_STRING, add_assoc_str_ex, add_assoc_stringl_ex,
        swr_zend_hash_foreach_key, swr_zend_property_array, swr_zend_property_slot,
        swr_zend_url_decode, zend_hash_str_find, zend_object, zend_string, zend_ulong, zval,
    },
    type_of, value,
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
const FLAG_ACK: u32 = 0x1;
const FLAG_END_STREAM: u32 = 0x1;
const FLAG_END_HEADERS: u32 = 0x4;
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
/// `NGHTTP2_HD_INFLATE_EMIT`.
const INFLATE_EMIT: c_int = 2;
/// `NGHTTP2_NV_FLAG_NO_COPY_NAME | NGHTTP2_NV_FLAG_NO_COPY_VALUE`.
const NV_NO_COPY: u8 = 0x02 | 0x04;
/// The largest DATA frame sent from a stack buffer.
const SMALL_FRAME: usize = 4096;
/// Swoole error codes and log levels.
const PHP_FATAL_ERROR: c_int = 503;
const NO_PAYLOAD: c_int = 550;
const STREAM_NOT_FOUND: c_int = 3003;
const STREAM_IGNORE: c_int = 3004;
const LOG_WARNING: c_int = 4;
const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;

use swoole_rs_phphttp::http2::Field;

pub(crate) use swoole_rs_phphttp::http2::Value;

unsafe extern "C" {
    fn swr_h2s_query(array: *mut zval, query: *const c_char, length: usize);
    fn swr_h2s_path(ctx: *mut c_void, path: *const c_char, length: usize);
    fn swr_h2s_cookie(array: *mut zval, cookie: *const c_char, length: usize);
    fn swr_h2s_multipart(ctx: *mut c_void, value: *const c_char, length: usize) -> bool;
    fn swr_h2s_set_header(
        ctx: *mut c_void,
        name: *const c_char,
        name_length: usize,
        value: *const c_char,
        length: usize,
    );
    fn swr_h2s_in_coroutine() -> bool;
    fn swr_h2s_log(level: c_int, error: c_int, func: *const c_char, message: *const c_char);
    fn swr_h2s_error(code: c_int, always: c_int, message: *const c_char);
}

// --- hosts, one safe call each ---

pub(crate) fn get(ctx: *mut c_void, field: Field) -> u32 {
    // SAFETY: a live context.
    unsafe { swoole_rs_phphttp::http2::get(ctx, field) }
}

pub(crate) fn set(ctx: *mut c_void, field: Field, value: u32) {
    // SAFETY: a live context.
    unsafe { swoole_rs_phphttp::http2::set(ctx, field, value) };
}

fn send(ctx: *mut c_void, data: &[u8]) -> bool {
    // SAFETY: a live, bound context.
    unsafe { swoole_rs_phphttp::http2::send(ctx, data) }
}

fn close(ctx: *mut c_void) {
    // SAFETY: a live, bound context.
    unsafe { swoole_rs_phphttp::http2::close(ctx) };
}

pub(crate) fn zval_of(ctx: *mut c_void, which: Value) -> *mut zval {
    // SAFETY: a live context with its objects.
    unsafe { swoole_rs_phphttp::http2::value(ctx, which) }
}

/// `swoole_warning` (`error` 0) or `swoole_error_log(level, error)` as `func`.
pub(crate) fn log(level: c_int, error: c_int, func: &CStr, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: NUL-terminated strings, printed through "%s".
    unsafe { swr_h2s_log(level, error, func.as_ptr(), message.as_ptr()) };
}

fn warn(func: &CStr, message: &str) {
    log(LOG_WARNING, 0, func, message);
}

/// `php_swoole_error_ex(E_WARNING, code, ...)`, or `php_swoole_fatal_error` when `always`.
fn report(code: c_int, always: bool, message: &[u8]) {
    let end = message
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(message.len());
    let message = CString::new(&message[..end]).unwrap_or_default();
    // SAFETY: C++'s reporter, printing the message through "%s".
    bailout::defer(unsafe {
        bailout::report_code(swr_h2s_error, code, c_int::from(always), &message)
    });
}

/// nghttp2's text for `code`.
fn strerror(code: c_int) -> String {
    // SAFETY: a plain lookup.
    let text = unsafe { swoole_http2_client_coro_strerror(code) };
    // SAFETY: nghttp2's static error text.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// A frame's nine header bytes.
fn frame_header(kind: u32, length: usize, flags: u32, stream_id: u32) -> [u8; FRAME_HEADER] {
    let mut header = [0; FRAME_HEADER];
    #[expect(clippy::cast_possible_truncation, reason = "frame lengths are 24-bit")]
    // SAFETY: nine writable bytes.
    unsafe {
        sw_rs_http2_pack_frame_header(header.as_mut_ptr(), kind, length as u32, flags, stream_id);
    };
    header
}

/// The session and stream of a response's context.
pub(crate) fn of(ctx: *mut c_void) -> Option<(Session, Stream)> {
    let session = SessionState::of(ctx)?;
    let stream = session.stream(get(ctx, Field::StreamId), false)?;
    Some((session, stream))
}

/// The HPACK deflater, created on first use as `func`.
fn deflater(session: &SessionState, func: &CStr) -> Option<*mut c_void> {
    let deflater = session.deflater.get();
    if !deflater.is_null() {
        return Some(deflater);
    }
    let mut error = 0;
    let size = session.remote[0].get();
    // SAFETY: nghttp2's constructor; `error` is writable.
    let deflater = unsafe { swoole_http2_client_coro_deflater(size, &raw mut error) };
    if error != 0 {
        warn(
            func,
            &format!(
                "nghttp2_hd_deflate_new2() failed with error: {}",
                strerror(error)
            ),
        );
        return None;
    }
    session.deflater.set(deflater);
    Some(deflater)
}

// --- small frames ---

fn send_u32_frame(
    ctx: *mut c_void,
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

fn window_update(ctx: *mut c_void, stream_id: u32, size: u32) {
    send_u32_frame(ctx, TYPE_WINDOW_UPDATE, 4, 0, stream_id, size);
}

/// `Stream::reset`.
/// # Safety
/// `ctx` is the stream's live context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_reset(ctx: *mut c_void, stream_id: u32, code: u32) {
    send_u32_frame(ctx, TYPE_RST_STREAM, 4, 0, stream_id, code);
}

/// `swoole_http2_server_ping`.
/// # Safety
/// `ctx` is a live HTTP/2 context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_ping(ctx: *mut c_void) -> bool {
    let mut frame = [0; FRAME_HEADER + PING_PAYLOAD];
    frame[..FRAME_HEADER].copy_from_slice(&frame_header(TYPE_PING, PING_PAYLOAD, 0, 0));
    send(ctx, &frame)
}

/// `swoole_http2_server_goaway`.
/// # Safety
/// `ctx` is a live HTTP/2 context; `data` covers `length` bytes (null at 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_goaway(
    ctx: *mut c_void,
    code: i64,
    data: *const c_char,
    length: usize,
) -> bool {
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
    let mut frame = Vec::with_capacity(FRAME_HEADER + GOAWAY_SIZE + length);
    frame.extend_from_slice(&frame_header(TYPE_GOAWAY, GOAWAY_SIZE + length, flags, 0));
    frame.extend_from_slice(&last_stream_id.to_be_bytes());
    frame.extend_from_slice(&code.to_be_bytes());
    frame.extend_from_slice(bytes(data, length));
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
    list: Vec<Header>,
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

    fn add(&mut self, name: &[u8], value: &[u8]) {
        let buffers = &mut self.buffers;
        if buffers.slots.len() < self.size {
            let start = buffers.bytes.len();
            buffers.bytes.extend_from_slice(name);
            buffers.bytes[start..].make_ascii_lowercase();
            buffers.bytes.extend_from_slice(value);
            buffers.slots.push((start, name.len(), value.len()));
        } else {
            let mut message = b"unexpect http2 header [".to_vec();
            message.extend_from_slice(name);
            message.extend_from_slice(b"] (duplicated or overflow)");
            report(PHP_FATAL_ERROR, true, &message);
        }
    }

    /// Encode the set with `nghttp2_hd_deflate_hd` into the frame after its
    /// nine header bytes, which `seal` fills in (`func` names the caller in logs).
    fn deflate(mut self, session: &Session, func: &CStr) -> Option<Self> {
        let deflater = deflater(session, func)?;
        let Buffers {
            bytes,
            slots,
            list,
            frame,
        } = &mut self.buffers;
        let base = bytes.as_ptr();
        list.extend(slots.iter().map(|&(start, name, value)| Header {
            name: base.wrapping_add(start),
            value: base.wrapping_add(start + name),
            name_length: name,
            value_length: value,
            flags: NV_NO_COPY,
        }));
        // SAFETY: a live deflater and the list above.
        let bound =
            unsafe { swoole_http2_client_coro_deflate_bound(deflater, list.as_ptr(), list.len()) };
        frame.resize(FRAME_HEADER + bound, 0);
        // SAFETY: as above, into the `bound` writable bytes after the frame header.
        let written = unsafe {
            swoole_http2_client_coro_deflate(
                deflater,
                frame.as_mut_ptr().wrapping_add(FRAME_HEADER),
                bound,
                list.as_ptr(),
                list.len(),
            )
        };
        let Ok(written) = usize::try_from(written) else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "nghttp2 error codes are small ints"
            )]
            let code = written as c_int;
            warn(
                func,
                &format!(
                    "nghttp2_hd_deflate_hd() failed with error: {}",
                    strerror(code)
                ),
            );
            return None;
        };
        frame.truncate(FRAME_HEADER + written);
        Some(self)
    }

    /// The frame `deflate` built, its header filled in.
    fn seal(&mut self, kind: u32, flags: u32, stream_id: u32) -> &[u8] {
        let frame = &mut self.buffers.frame;
        let header = frame_header(kind, frame.len() - FRAME_HEADER, flags, stream_id);
        frame[..FRAME_HEADER].copy_from_slice(&header);
        frame
    }
}

impl Drop for Headers {
    fn drop(&mut self) {
        let mut buffers = std::mem::take(&mut self.buffers);
        buffers.bytes.clear();
        buffers.slots.clear();
        buffers.list.clear();
        buffers.frame.clear();
        POOL.with_borrow_mut(|pool| pool.push(buffers));
    }
}

/// A response property (`sw_zend_read_property_ex`).
fn property(ctx: *mut c_void, name: &CStr) -> *mut zval {
    let response = zval_of(ctx, Value::Response);
    // SAFETY: the live Response object and its declared property.
    unsafe { swr_zend_property_slot(object(response), name.as_ptr(), name.count_bytes()) }
}

fn object(value: *mut zval) -> *mut zend_object {
    // SAFETY: an object zval.
    unsafe { value::obj_of(value) }
}

fn is(value: *const zval, kind: u32) -> bool {
    // SAFETY: a live zval.
    !value.is_null() && unsafe { type_of(value) } == kind
}

/// `php_swoole_array_length_safe`.
fn length_safe(array: *const zval) -> usize {
    if is(array, IS_ARRAY) {
        // SAFETY: a live array.
        unsafe { value::count(array) as usize }
    } else {
        0
    }
}

/// The entries of an array in order, with their string keys (null for an
/// integer key), borrowed from the array.
fn entries(array: *const zval) -> Vec<(*mut zend_string, *mut zval)> {
    unsafe extern "C" fn each(
        _: zend_ulong,
        key: *mut zend_string,
        value: *mut zval,
        out: *mut c_void,
    ) -> bool {
        // SAFETY: the vector `entries` lends.
        unsafe { (*out.cast::<Vec<(*mut zend_string, *mut zval)>>()).push((key, value)) };
        true
    }
    let mut out = Vec::with_capacity(length_safe(array));
    // SAFETY: a live array.
    let table = unsafe { value::table(array) };
    // SAFETY: a live table; `each` only collects.
    unsafe { swr_zend_hash_foreach_key(table, Some(each), (&raw mut out).cast()) };
    out
}

/// A string key's bytes.
fn key_of<'a>(key: *mut zend_string) -> &'a [u8] {
    // SAFETY: a live key of a live array.
    unsafe { string::bytes(key) }
}

/// A reference `zval_get_string` returned, released on drop.
struct Text(*mut zend_string);

impl Text {
    fn bytes(&self) -> &[u8] {
        // SAFETY: a live string this holds a reference to.
        unsafe { string::bytes(self.0) }
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        // SAFETY: the reference `string_of` took.
        unsafe { swoole_rs_zend::sys::swr_zend_string_release(self.0) };
    }
}

/// `zval_get_string`, `None` once a bailout is pending.
fn string_of(value: *const zval) -> Option<Text> {
    // SAFETY: a live zval; the result owns a new reference.
    let string = unsafe { value::get_string(value) };
    if string.is_null() {
        return None;
    }
    let text = Text(string);
    (!bailout::pending()).then_some(text)
}

/// `swoole::rtrim`: trailing `isspace` bytes off.
fn rtrim(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .rposition(|&byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
        .map_or(0, |last| last + 1);
    &value[..end]
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

/// `http2_server_build_trailer`, as an unsealed frame: `None` on an error or
/// without a trailer.
fn build_trailer(ctx: *mut c_void, session: &Session) -> Option<Headers> {
    let trailer = property(ctx, c"trailer");
    let size = length_safe(trailer);
    if size == 0 {
        return None;
    }
    let mut headers = Headers::new(size);
    for (key, value) in entries(trailer) {
        if key.is_null() || is(value, IS_NULL) {
            continue;
        }
        let value = string_of(value)?;
        headers.add(key_of(key), value.bytes());
    }
    headers.deflate(session, c"http2_server_build_trailer")
}

/// The body a response header announces: its bytes (null for a file sent
/// later) and length.
#[derive(Clone, Copy)]
struct Body {
    data: *const c_char,
    length: usize,
}

/// `http2_server_build_header`.
fn build_header(ctx: *mut c_void, session: &Session, body: Option<Body>) -> Option<Headers> {
    let zheader = property(ctx, c"header");
    let zcookie = property(ctx, c"cookie");
    let mut headers = Headers::new(32 + length_safe(zheader) + length_safe(zcookie));
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
    if is(zheader, IS_ARRAY) {
        let can_capture =
            get(ctx, Field::AcceptCompression) != 0 && get(ctx, Field::CompressionTypes) != 0;
        let mut content_type = None;
        let mut add = |headers: &mut Headers, key: &[u8], value: *mut zval| -> Option<()> {
            if is(value, IS_NULL) {
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
            headers.add(key, text);
            Some(())
        };
        for (key, value) in entries(zheader) {
            if key.is_null() || is(value, IS_NULL) {
                continue;
            }
            let key = key_of(key);
            if is(value, IS_ARRAY) {
                for (_, item) in entries(value) {
                    add(&mut headers, key, item)?;
                }
            } else {
                add(&mut headers, key, value)?;
            }
        }
        if can_capture {
            let kind = content_type.as_ref().map_or(&b"text/html"[..], Text::bytes);
            // SAFETY: a live context.
            let accept = unsafe { swoole_rs_phphttp::http2::compressible(ctx, kind) };
            set(ctx, Field::AcceptCompression, u32::from(accept));
        }
    }

    let missing = sw_rs_http2_header_defaults(header_flags);
    if missing & MISSING_SERVER != 0 {
        headers.add(b"server", b"swoole-http-server");
    }
    if missing & MISSING_DATE != 0 {
        let date = swoole_rs_phphttp::http2::date();
        // SAFETY: the cached date string.
        let date = unsafe { string::bytes(date) };
        headers.add(b"date", date);
    }
    if missing & MISSING_CONTENT_TYPE != 0 {
        headers.add(b"content-type", b"text/html");
    }
    if is(zcookie, IS_ARRAY) {
        for (_, value) in entries(zcookie) {
            if is(value, IS_STRING) {
                // SAFETY: a live string zval.
                let cookie = unsafe { value::str_of(value) };
                // SAFETY: its live string.
                headers.add(b"set-cookie", unsafe { string::bytes(cookie) });
            }
        }
    }
    if let Some(body) = body {
        let mut content_length = body.length;
        // SAFETY: a live context; the body is only read when compression is accepted.
        let compressed =
            unsafe { swoole_rs_phphttp::http2::compress(ctx, bytes(body.data, body.length)) };
        if let Some((encoding, compressed)) = compressed {
            content_length = compressed.len();
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
    let headers = headers.deflate(session, c"http2_server_build_header")?;
    set(ctx, Field::SendHeader, 1);
    Some(headers)
}

/// `rust_header_plan`: the header name folded as `std::tolower` does.
fn header_plan(role: u32, name: &[u8], can_capture: bool) -> Http2HeaderPlan {
    let (folded, length) = fold(name);
    let mut plan = Http2HeaderPlan::default();
    // SAFETY: the folded prefix and a writable plan.
    unsafe {
        sw_rs_http2_header_plan(
            role,
            folded.as_ptr(),
            length,
            name.len(),
            u32::from(can_capture),
            &raw mut plan,
        );
    };
    plan
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

/// `value` in decimal, written into `buffer` (`swoole_itoa`).
fn decimal(buffer: &mut [u8; 20], value: i64) -> &[u8] {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(&mut buffer[..]);
    // Twenty bytes hold any i64.
    write!(cursor, "{value}").unwrap_or_default();
    let length = usize::try_from(cursor.position()).unwrap_or(0);
    &buffer[..length]
}

/// `Stream::send_header`.
fn send_header(
    ctx: *mut c_void,
    session: &Session,
    stream: &Stream,
    body: Option<Body>,
    end_stream: bool,
) -> bool {
    let Some(mut headers) = build_header(ctx, session, body) else {
        return false;
    };
    let empty = body.is_none_or(|body| body.length == 0);
    let flags = sw_rs_http2_headers_flags(u32::from(empty), u32::from(end_stream), 0);
    if flags & FLAG_END_STREAM != 0 {
        set(ctx, Field::End, 1);
    }
    if !send(ctx, headers.seal(TYPE_HEADERS, flags, stream.id)) {
        set(ctx, Field::SendHeader, 0);
        return false;
    }
    true
}

/// `Stream::send_body`: DATA frames within the peer's flow-control window.
pub(crate) fn send_body(
    ctx: *mut c_void,
    session: &Session,
    stream: &Stream,
    data: &[u8],
    end_stream: bool,
) -> bool {
    let flags = if end_stream { FLAG_END_STREAM } else { 0 };
    let max_frame_size = session.local(MAX_FRAME_SIZE);
    // SAFETY: plain libc.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096);
    let mut rest = data;
    while !rest.is_empty() {
        let mut chunk = Http2ChunkPlan::default();
        // SAFETY: a writable plan.
        unsafe { sw_rs_http2_chunk(rest.len(), max_frame_size, flags, &raw mut chunk) };
        let (mut send_n, mut send_flags) = (chunk.length, chunk.flags);
        let window = stream.remote_window_size.get();
        if send_n > window as usize {
            // The coroutine server receives frames serially and cannot be
            // suspended, so it has no flow control.
            // SAFETY: plain reads of Swoole's state.
            let may_suspend = get(ctx, Field::CoSocket) == 0 && unsafe { swr_h2s_in_coroutine() };
            match sw_rs_http2_window_action(send_n, window, u32::from(may_suspend)) {
                1 => warn(
                    c"Stream::send_body",
                    "The data sent exceeded remote_window_size",
                ),
                2 => {
                    // In a coroutine: WINDOW_UPDATE wakes it.
                    stream.wait();
                    continue;
                }
                _ => {
                    send_n = window as usize;
                    send_flags = 0;
                }
            }
        }
        let header = frame_header(TYPE_DATA, send_n, send_flags, stream.id);
        let part = &rest[..send_n];
        if send_n < page {
            // One send of header and payload, on the stack when it fits.
            let mut small = [0; FRAME_HEADER + SMALL_FRAME];
            let mut large = Vec::new();
            let frame = if send_n <= SMALL_FRAME {
                &mut small[..FRAME_HEADER + send_n]
            } else {
                large.resize(FRAME_HEADER + send_n, 0);
                &mut large[..]
            };
            frame[..FRAME_HEADER].copy_from_slice(&header);
            frame[FRAME_HEADER..].copy_from_slice(part);
            if !send(ctx, frame) {
                return false;
            }
        } else if !send(ctx, &header) || !send(ctx, part) {
            return false;
        }
        rest = &rest[send_n..];
        #[expect(
            clippy::cast_possible_truncation,
            reason = "native uint32_t window arithmetic"
        )]
        let sent = send_n as u32;
        stream
            .remote_window_size
            .set(stream.remote_window_size.get().wrapping_sub(sent));
        session
            .remote_window_size
            .set(session.remote_window_size.get().wrapping_sub(sent));
    }
    true
}

/// `Stream::send_trailer`.
fn send_trailer(ctx: *mut c_void, session: &Session, stream_id: u32) -> bool {
    let Some(mut headers) =
        build_trailer(ctx, session).filter(|headers| headers.buffers.frame.len() > FRAME_HEADER)
    else {
        return true;
    };
    send(
        ctx,
        headers.seal(TYPE_HEADERS, FLAG_END_HEADERS | FLAG_END_STREAM, stream_id),
    )
}

/// Whether the response has a trailer.
fn has_trailer(ctx: *mut c_void) -> bool {
    length_safe(property(ctx, c"trailer")) > 0
}

/// `http2_server_respond`: the header, the body and the trailer, then the
/// stream is done.
pub(crate) fn respond(ctx: *mut c_void, data: &[u8]) -> bool {
    let Some((session, stream)) = of(ctx) else {
        return false;
    };
    let trailer = has_trailer(ctx);
    let end_stream = !trailer;
    let mut body: Vec<u8> = Vec::new();
    let mut compressed = false;
    let mut facts = Http2RespondFacts {
        header_sent: get(ctx, Field::SendHeader),
        trailer_present: u32::from(trailer),
        chunked: 0,
        shutting_down: 0,
        streams_empty: 0,
        body_length: 0,
    };
    let next = |action, ok: bool, facts: &Http2RespondFacts| {
        // SAFETY: initialised facts.
        unsafe { sw_rs_http2_respond_next(action, u32::from(ok), facts) }
    };
    let mut action = next(RESP_START, true, &facts);
    loop {
        match action {
            RESP_HEADER => {
                let announced = Body {
                    data: data.as_ptr().cast(),
                    length: data.len(),
                };
                let sent = send_header(ctx, &session, &stream, Some(announced), end_stream);
                action = next(action, sent, &facts);
            }
            RESP_COMMIT_END => {
                // Headers are sent: body failures must not permit a retry.
                set(ctx, Field::End, 1);
                if get(ctx, Field::ContentCompressed) != 0 {
                    // SAFETY: a live context, which holds the compressed body.
                    body = unsafe { swoole_rs_phphttp::http2::compressed(ctx) }.to_vec();
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
                let sent = send_body(ctx, &session, &stream, payload, end_stream);
                action = next(action, sent, &facts);
            }
            RESP_TRAILER => {
                let sent = send_trailer(ctx, &session, stream.id);
                action = next(action, sent, &facts);
            }
            RESP_ERROR_CLOSE | RESP_REMOVE => {
                if action == RESP_ERROR_CLOSE {
                    close(ctx);
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

/// `http2_server_respond`: `length` body bytes (null at 0).
/// # Safety
/// `ctx` is a live HTTP/2 stream context and `data` covers `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_respond(
    ctx: *mut c_void,
    data: *const c_char,
    length: usize,
) -> bool {
    let done = respond(ctx, bytes(data, length));
    bailout::finish();
    done
}

/// `swoole_http2_server_write`: a chunk, the header first.
/// # Safety
/// As `swr_v1_phphttp2_server_respond`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_write(
    ctx: *mut c_void,
    data: *const c_char,
    length: usize,
) -> bool {
    let done = write(ctx, bytes(data, length));
    bailout::finish();
    done
}

fn write(ctx: *mut c_void, chunk: &[u8]) -> bool {
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

pub(crate) fn begin_file(ctx: *mut c_void, content_type: &[u8], length: usize) -> c_int {
    let Some((session, stream)) = of(ctx) else {
        return -1;
    };
    set(ctx, Field::AcceptCompression, 0);
    let trailer = has_trailer(ctx);
    let response = zval_of(ctx, Value::Response);
    let object = object(response);
    // SAFETY: the live Response object and its declared `header` property.
    let ce = unsafe { (*object).ce };
    // SAFETY: the live object of that class.
    let zheader = unsafe { swr_zend_property_array(ce, object, c"header".as_ptr(), 6) };
    // SAFETY: a live array.
    let table = unsafe { value::table(zheader) };
    // SAFETY: a live table.
    let present = unsafe { zend_hash_str_find(table, c"content-type".as_ptr(), 12) };
    if present.is_null() {
        // SAFETY: a live context; C++ copies both.
        unsafe {
            swr_h2s_set_header(
                ctx,
                c"content-type".as_ptr(),
                12,
                content_type.as_ptr().cast(),
                content_type.len(),
            );
        };
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

pub(crate) fn finish_file(ctx: *mut c_void, trailer: bool, error: bool) {
    let Some(session) = SessionState::of(ctx) else {
        return;
    };
    let stream_id = get(ctx, Field::StreamId);
    let mut action = sw_rs_http2_file_complete(u32::from(!error), u32::from(trailer));
    loop {
        match action {
            FILE_TRAILER => {
                let sent = send_trailer(ctx, &session, stream_id);
                action = sw_rs_http2_file_complete(u32::from(sent), 0);
            }
            FILE_CLOSE => return close(ctx),
            _ => return session.remove(stream_id),
        }
    }
}

// --- receiving ---

/// `swoole_http2_server_parse`: one frame from the peer of connection `fd`,
/// whose session stays alive until the frame is handled.
/// # Safety
/// `buf` holds a whole frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_session_parse(fd: i64, buf: *const c_char) -> c_int {
    let Some(session) = SessionState::find(fd) else {
        return SW_ERR;
    };
    let result = parse(&session, buf.cast());
    drop(session);
    bailout::finish();
    result
}

/// `swr_v1_phphttp2_session_parse` for a Rust caller (the coroutine server's
/// connection loop): a bailout stays pending for the caller's `finish`.
/// # Safety
/// As `swr_v1_phphttp2_session_parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_session_receive(fd: i64, buf: *const c_char) -> c_int {
    SessionState::find(fd).map_or(SW_ERR, |session| parse(&session, buf.cast()))
}

fn parse(session: &Session, buf: *const u8) -> c_int {
    let mut facts = Http2FrameFacts::default();
    let last_stream_id = session.last_stream_id.get();
    // SAFETY: nine readable header bytes and a writable result.
    unsafe { sw_rs_http2_decode_frame(buf, FRAME_HEADER, last_stream_id, 0, &raw mut facts) };
    let stream_id = facts.stream_id;
    if facts.publish_last_id != 0 {
        session.last_stream_id.set(stream_id);
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
        ACTION_HEADERS => {
            let Some(stream) = session
                .stream(stream_id, false)
                .or_else(|| session.stream(stream_id, true))
            else {
                return SW_ERR;
            };
            if parse_header(session, stream.ctx, flags, payload, length) < 0 {
                return SW_ERR;
            }
            if flags & FLAG_END_STREAM != 0 {
                session.handle(&stream);
            }
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

fn read_u32(at: *const u8) -> u32 {
    // SAFETY: four readable bytes of the frame.
    u32::from_be_bytes(unsafe { at.cast::<[u8; 4]>().read_unaligned() })
}

fn window(current: u32, amount: usize, initial: u32, credit: bool) -> Http2WindowPlan {
    let mut plan = Http2WindowPlan::default();
    // SAFETY: a writable plan.
    unsafe { sw_rs_http2_window_plan(current, amount, initial, u32::from(credit), &raw mut plan) };
    plan
}

fn credit(current: u32, amount: u32) -> u32 {
    window(current, amount as usize, 0, true).value
}

/// A SETTINGS frame: apply each, then acknowledge.
fn settings(session: &Session, mut at: *const u8, length: usize) -> c_int {
    let mut left = isize::try_from(length).unwrap_or(isize::MAX);
    while left > 0 {
        let (mut id, mut value) = (0_u16, 0_u32);
        // SAFETY: six readable bytes, as upstream reads them.
        unsafe { swoole_rs_http2::sw_rs_http2_unpack_setting(at, &raw mut id, &raw mut value) };
        let deflater = session.deflater.get();
        let old = if id == SETTING_HEADER_TABLE_SIZE {
            session.remote[0].get()
        } else {
            0
        };
        let change = id == SETTING_HEADER_TABLE_SIZE && value != old && !deflater.is_null();
        let mut plan = Http2SettingPlan::default();
        // SAFETY: a writable plan.
        unsafe {
            sw_rs_http2_setting_plan(
                u32::from(id),
                value,
                old,
                u32::from(change),
                1,
                &raw mut plan,
            );
        };
        if plan.set_value != 0
            && let Ok(field) = usize::try_from(plan.field)
            && let Some(setting) = session.remote.get(field)
        {
            setting.set(value);
        }
        if plan.set_window != 0 {
            session.remote_window_size.set(value);
        }
        if plan.change_deflater != 0 {
            // SAFETY: a live deflater.
            let code = unsafe { swoole_http2_client_coro_deflate_table(deflater, value) };
            if code != 0 {
                warn(
                    c"swoole_http2_server_parse",
                    &format!(
                        "nghttp2_hd_deflate_change_table_size() failed, errno={code}, errmsg={}",
                        strerror(code)
                    ),
                );
                return SW_ERR;
            }
        }
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
    let stream = session.stream(stream_id, false);
    let mut ctx = ptr::null_mut();
    let mut facts = Http2ReceiveFacts {
        length,
        buffered_length: 0,
        maximum: 0,
        ended: 0,
        end_stream: flags & FLAG_END_STREAM,
        parse_body: 0,
        form: 0,
        multipart: 0,
    };
    let next = |action, ok: bool, facts: &Http2ReceiveFacts| {
        // SAFETY: initialised facts.
        unsafe { sw_rs_http2_receive_next(action, u32::from(ok), facts) }
    };
    let mut action = next(recv::START, stream.is_some(), &facts);
    while action != recv::DONE {
        let mut ok = true;
        match (action, &stream) {
            (recv::CONTEXT, Some(stream)) => ctx = stream.ctx,
            (recv::BUFFER, _) => {
                // SAFETY: a live context.
                unsafe { swoole_rs_phphttp::http2::data_start(ctx) };
                // SAFETY: as above.
                facts.buffered_length = unsafe { swoole_rs_phphttp::http2::data(ctx) }.len();
                facts.maximum = session.max_body_size;
                facts.ended = get(ctx, Field::End);
            }
            (recv::RESET, _) => {
                send_u32_frame(ctx, TYPE_RST_STREAM, 0, FLAG_ACK, 0, 0);
            }
            (recv::REMOVE, _) => session.remove(stream_id),
            // SAFETY: a live context and the frame's payload.
            (recv::APPEND, _) => unsafe {
                swoole_rs_phphttp::http2::data_append(ctx, bytes(payload.cast(), length));
            },
            (recv::FLOW, Some(stream)) => flow(session, stream, ctx, stream_id, length),
            (recv::LENGTH, _) => {
                // SAFETY: a live context.
                ok = !unsafe { swoole_rs_phphttp::http2::data(ctx) }.is_empty();
            }
            (recv::END_BUFFER, _) => {
                facts.parse_body = get(ctx, Field::ParseBody);
                facts.form = get(ctx, Field::Form);
                facts.multipart = get(ctx, Field::Multipart);
            }
            (recv::FORM, _) => {
                // SAFETY: a live context.
                let body = unsafe { swoole_rs_phphttp::http2::data(ctx) };
                let post = zval_of(ctx, Value::Post);
                // SAFETY: the live `post` array and the body's bytes, which C++ copies.
                unsafe { swr_h2s_query(post, body.as_ptr().cast(), body.len()) };
            }
            // SAFETY: a live context with a multipart parser.
            (recv::MULTIPART, _) => ok = unsafe { swoole_rs_phphttp::http2::multipart(ctx) },
            (recv::HANDLE, Some(stream)) => session.handle(stream),
            (recv::WARN, _) => log(
                LOG_WARNING,
                STREAM_NOT_FOUND,
                c"swoole_http2_server_parse",
                &format!("http2 stream#{stream_id} not found"),
            ),
            _ => return SW_ERR,
        }
        action = next(action, ok, &facts);
    }
    SW_OK
}

/// Flow control: debit both receive windows, and reopen each once a quarter
/// remains.
fn flow(session: &Session, stream: &Stream, ctx: *mut c_void, stream_id: u32, length: usize) {
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
    ctx: *mut c_void,
    flags: u32,
    mut input: *const u8,
    mut length: usize,
) -> c_int {
    const FUNC: &CStr = c"http2_server_parse_header";
    let mut inflater = session.inflater.get();
    if inflater.is_null() {
        let mut error = 0;
        // SAFETY: nghttp2's constructor; `error` is writable.
        inflater = unsafe { swoole_http2_client_coro_inflater(&raw mut error) };
        if error != 0 {
            warn(
                FUNC,
                &format!(
                    "nghttp2_hd_inflate_new2() failed, Error: {}[{error}]",
                    strerror(error)
                ),
            );
            return SW_ERR;
        }
        session.inflater.set(inflater);
    }
    if flags & FLAG_PRIORITY != 0 {
        // The stream dependency and weight are ignored.
        // SAFETY: within the frame.
        input = unsafe { input.add(5) };
        length = length.saturating_sub(5);
    }
    let zheader = zval_of(ctx, Value::Header);
    let zserver = zval_of(ctx, Value::Server);
    loop {
        let mut header = Header {
            name: ptr::null(),
            value: ptr::null(),
            name_length: 0,
            value_length: 0,
            flags: 0,
        };
        let mut inflate_flags = 0;
        // SAFETY: a live inflater over the rest of the block.
        let read = unsafe {
            swoole_http2_client_coro_inflate(
                inflater,
                input,
                length,
                &raw mut inflate_flags,
                &raw mut header,
            )
        };
        let Ok(read) = usize::try_from(read) else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "nghttp2 error codes are small ints"
            )]
            let code = read as c_int;
            warn(
                FUNC,
                &format!("inflate failed, Error: {}[{read}]", strerror(code)),
            );
            return SW_ERR;
        };
        // SAFETY: nghttp2 consumed `read` bytes of the input.
        input = unsafe { input.add(read) };
        length -= read;
        if inflate_flags & INFLATE_EMIT != 0 {
            // SAFETY: nghttp2's header, live until the next inflate call.
            let name = unsafe { std::slice::from_raw_parts(header.name, header.name_length) };
            // SAFETY: as above.
            let value = unsafe { std::slice::from_raw_parts(header.value, header.value_length) };
            match request_header(session, ctx, zheader, zserver, name, value) {
                Some(true) => {}
                Some(false) => continue,
                None => return SW_ERR,
            }
        }
        let mut step = Http2InflateStep {
            end_headers: 0,
            finish: 0,
        };
        #[expect(clippy::cast_sign_loss, reason = "nghttp2's flag bits")]
        // SAFETY: a writable step.
        unsafe {
            sw_rs_http2_inflate_step(inflate_flags as u32, length, 0, &raw mut step);
        };
        if step.end_headers != 0 {
            // SAFETY: a live inflater.
            unsafe { swoole_http2_client_coro_inflate_end(inflater) };
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

/// One request header. `Some(false)` skips the rest of the step (a cookie),
/// `None` is an error.
fn request_header(
    session: &Session,
    ctx: *mut c_void,
    zheader: *mut zval,
    zserver: *mut zval,
    name: &[u8],
    value: &[u8],
) -> Option<bool> {
    let kind = header_plan(REQUEST_HEADERS, name, false).kind;
    if name.first() == Some(&b':') {
        match kind {
            HEADER_METHOD => add_string(zserver, b"request_method", value),
            HEADER_PATH => path(ctx, zserver, value),
            HEADER_AUTHORITY => add_string(zheader, b"host", value),
            _ => {}
        }
        return Some(true);
    }
    match kind {
        HEADER_CONTENT_TYPE => {
            let (folded, length) = fold(value);
            // SAFETY: the folded prefix.
            let media = unsafe { sw_rs_http2_media_kind(folded.as_ptr(), length) };
            if media == MEDIA_URLENCODED {
                set(ctx, Field::Form, 1);
            } else if media == MEDIA_MULTIPART {
                // SAFETY: a live context and the header value.
                if !unsafe { swr_h2s_multipart(ctx, value.as_ptr().cast(), value.len()) } {
                    return None;
                }
            }
        }
        HEADER_COOKIE => {
            // SAFETY: the request's `$cookie` and the header value.
            unsafe {
                swr_h2s_cookie(
                    zval_of(ctx, Value::Cookie),
                    value.as_ptr().cast(),
                    value.len(),
                );
            };
            return Some(false);
        }
        HEADER_CONTENT_LENGTH => {
            let (parsed, complete) = strtol(value);
            if sw_rs_http2_content_length_rejected(
                u32::from(complete),
                parsed,
                session.max_body_size,
            ) != 0
            {
                send_status_code(ctx, ENTITY_TOO_LARGE);
            }
        }
        HEADER_ACCEPT_ENCODING if get(ctx, Field::EnableCompression) != 0 => {
            // SAFETY: a live context.
            unsafe { swoole_rs_phphttp::http2::compression_method(ctx, value) };
        }
        _ => {}
    }
    add_string(zheader, name, value);
    Some(true)
}

/// `:path`: `request_uri`, `path_info` (decoded), `query_string` and `$get`.
fn path(ctx: *mut c_void, zserver: *mut zval, value: &[u8]) {
    // `strchr`: the value ends at its first NUL.
    let text = &value[..value
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(value.len())];
    let path = match text.iter().position(|&byte| byte == b'?') {
        Some(question) => {
            let query = &value[question + 1..];
            add_string(zserver, b"query_string", query);
            // SAFETY: the request's `$get`; C++ copies the query.
            unsafe { swr_h2s_query(zval_of(ctx, Value::Get), query.as_ptr().cast(), query.len()) };
            &value[..question]
        }
        None => value,
    };
    // SAFETY: a live context; C++ copies the path.
    unsafe { swr_h2s_path(ctx, value.as_ptr().cast(), value.len()) };
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
    let end = value
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(value.len());
    let text = &value[..end];
    let mut at = text
        .iter()
        .take_while(|&&byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
        .count();
    let negative = matches!(text.get(at), Some(b'-'));
    if matches!(text.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let digits = text[at.min(text.len())..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return (0, value.is_empty());
    }
    let mut parsed: i64 = 0;
    for &digit in &text[at..at + digits] {
        let digit = i64::from(digit - b'0');
        parsed = if negative {
            parsed.saturating_mul(10).saturating_sub(digit)
        } else {
            parsed.saturating_mul(10).saturating_add(digit)
        };
    }
    (parsed, at + digits == value.len())
}

/// `http2_server_send_status_code`: a header-only response.
fn send_status_code(ctx: *mut c_void, status: u32) {
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
