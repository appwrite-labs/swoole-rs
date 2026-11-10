//! The HTTP/1 request parser: llhttp and multipart callbacks over
//! `swoole::http::ContextState`, filling `Swoole\Http\Request`'s `server`,
//! `header`, `get`, `post`, `cookie` and `files` as upstream's
//! `swoole_http_request.cc` did. llhttp and the multipart parser stay the
//! bundled C (third-party); what Swoole itself owns goes through `host`.
use crate::host;
use crate::state::{Slot, State};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering::Relaxed};
use std::{mem::ManuallyDrop, ptr};
use swoole_rs_corebase::log;
use swoole_rs_httpbindings::{
    RequestHeader, Text, Trimmed, UrlParts, request_header, request_key,
    swr_v1_httpbindings_body_prefix, swr_v1_httpbindings_compression,
    swr_v1_httpbindings_form_source, swr_v1_httpbindings_request_url, swr_v1_httpbindings_tokens,
    swr_v1_httpbindings_trim_quotes,
};
use swoole_rs_server::view;
use swoole_rs_zend::sys::{
    swr_zend_http_estrndup, swr_zend_http_intern, swr_zend_http_max_input_vars,
    swr_zend_http_property_offset, swr_zend_http_treat_data, swr_zend_http_upload,
    zend_class_entry, zend_string, zval,
};
use swoole_rs_zend::{Owned, OwnedArray, Value, ZStr, ZString, string};

/// The bundled `multipart_parser` (`thirdparty/multipart_parser.h`), up to
/// the fields its callbacks use.
#[repr(C)]
pub(crate) struct Multipart {
    settings: *const c_void,
    index: usize,
    error_i: usize,
    boundary_length: u8,
    fp: *mut libc::FILE,
    data: *mut c_void,
}

type DataCb = unsafe extern "C" fn(*mut crate::state::Llhttp, *const c_char, usize) -> c_int;
type NotifyCb = unsafe extern "C" fn(*mut crate::state::Llhttp) -> c_int;
type PartDataCb = unsafe extern "C" fn(*mut Multipart, *const c_char, usize) -> c_int;
type PartNotifyCb = unsafe extern "C" fn(*mut Multipart) -> c_int;

/// `llhttp_settings_t`.
#[repr(C)]
struct Settings {
    on_message_begin: Option<NotifyCb>,
    on_protocol: Option<DataCb>,
    on_url: Option<DataCb>,
    on_status: Option<DataCb>,
    on_method: Option<DataCb>,
    on_version: Option<DataCb>,
    on_header_field: Option<DataCb>,
    on_header_value: Option<DataCb>,
    on_chunk_extension_name: Option<DataCb>,
    on_chunk_extension_value: Option<DataCb>,
    on_headers_complete: Option<NotifyCb>,
    on_body: Option<DataCb>,
    on_message_complete: Option<NotifyCb>,
    /// `on_protocol_complete` to `on_reset`.
    rest: [Option<NotifyCb>; 12],
}

/// `multipart_parser_settings`.
#[repr(C)]
struct PartSettings {
    on_header_field: Option<PartDataCb>,
    on_header_value: Option<PartDataCb>,
    on_part_data: Option<PartDataCb>,
    on_part_data_begin: Option<PartNotifyCb>,
    on_headers_complete: Option<PartNotifyCb>,
    on_part_data_end: Option<PartNotifyCb>,
    on_body_end: Option<PartNotifyCb>,
}

unsafe extern "C" {
    fn llhttp_init(parser: *mut crate::state::Llhttp, kind: c_int, settings: *const c_void);
    fn llhttp_execute(parser: *mut crate::state::Llhttp, data: *const c_char, len: usize) -> c_int;
    fn llhttp_get_error_pos(parser: *const crate::state::Llhttp) -> *const c_char;
    fn llhttp_resume(parser: *mut crate::state::Llhttp);
    fn llhttp_resume_after_upgrade(parser: *mut crate::state::Llhttp);
    fn llhttp_should_keep_alive(parser: *const crate::state::Llhttp) -> c_int;
    fn llhttp_set_error_reason(parser: *mut crate::state::Llhttp, reason: *const c_char);
    fn llhttp_method_name(method: c_int) -> *const c_char;
    fn multipart_parser_init(
        boundary: *const c_char,
        length: usize,
        settings: *const PartSettings,
    ) -> *mut Multipart;
    fn multipart_parser_free(parser: *mut Multipart);
    fn multipart_parser_execute(parser: *mut Multipart, data: *const c_char, len: usize) -> isize;
    fn multipart_parser_error_msg(parser: *mut Multipart, buf: *mut c_char, len: usize) -> c_int;
    fn php_url_decode(text: *mut c_char, len: usize) -> usize;
    fn php_register_variable_safe(
        var: *const c_char,
        val: *const c_char,
        len: usize,
        track: *mut zval,
    );
    fn php_register_variable_ex(var: *const c_char, val: *mut zval, track: *mut zval);
}

/// llhttp's `HPE_INVALID_HEADER_TOKEN`, `HPE_PAUSED`, `HPE_PAUSED_UPGRADE`,
/// `HPE_USER`, `F_CHUNKED` and `F_TRAILING`.
const INVALID_HEADER_TOKEN: i32 = 10;
const PAUSED: c_int = 21;
const PAUSED_UPGRADE: c_int = 22;
const USER: c_int = 24;
const CHUNKED: u16 = 8;
const TRAILING: u16 = 0x80;
/// `PARSE_COOKIE` and `PARSE_STRING` (`main/php_variables.h`).
const PARSE_COOKIE: c_int = 2;
const PARSE_STRING: c_int = 3;
/// `SW_LOG_TRACE`, `SW_LOG_NOTICE`, `SW_ERROR_SESSION_CLOSED` and
/// `SW_ERROR_SERVER_INVALID_REQUEST` (asserted in ext-src).
const LOG_TRACE: c_int = 1;
const LOG_NOTICE: c_int = 3;
const SESSION_CLOSED: c_int = 1004;
const SERVER_INVALID_REQUEST: c_int = 9010;
/// `SW_STACK_BUFFER_SIZE` (also `sw_tg_buffer()`'s size), `SW_BUFFER_SIZE_STD`,
/// `SW_HTTP_FORM_KEYLEN`, `SW_HTTP_UPLOAD_TMPDIR_SIZE`, `HT_MIN_SIZE`.
const STACK_BUFFER_SIZE: usize = 65536;
const BUFFER_SIZE_STD: usize = 8192;
const FORM_KEYLEN: usize = 512;
const UPLOAD_TMPDIR_SIZE: usize = 256;
const HT_MIN_SIZE: u32 = 8;
/// `http_upload_errno`.
const UPLOAD_OK: i64 = 0;
const UPLOAD_NO_FILE: i64 = 4;
const UPLOAD_NO_TMP_DIR: i64 = 6;
const UPLOAD_CANT_WRITE: i64 = 7;
const UPLOAD_FILE_READY: i64 = 8;
/// MINIT's codec bits: zlib, brotli, zstd, and `SW_HAVE_COMPRESSION`.
const CODECS: u32 = 7;
const COMPRESSION: u32 = 8;

static SERVER: Settings = Settings {
    on_message_begin: None,
    on_protocol: None,
    on_url: Some(on_url),
    on_status: None,
    on_method: None,
    on_version: None,
    on_header_field: Some(on_header_field),
    on_header_value: Some(on_header_value),
    on_chunk_extension_name: None,
    on_chunk_extension_value: None,
    on_headers_complete: Some(on_headers_complete),
    on_body: Some(on_body),
    on_message_complete: Some(on_message_complete),
    rest: [None; 12],
};

/// `Request::parse()`: input arrives in pieces appended to `request.zdata`.
static PIECES: Settings = Settings {
    on_url: Some(on_piece_url),
    on_header_field: Some(on_piece_header_field),
    on_header_value: Some(on_piece_header_value),
    ..SERVER
};

static PARTS: PartSettings = PartSettings {
    on_header_field: Some(on_part_header_field),
    on_header_value: Some(on_part_header_value),
    on_part_data: Some(on_part_data),
    on_part_data_begin: None,
    on_headers_complete: Some(on_part_headers_complete),
    on_part_data_end: Some(on_part_data_end),
    on_body_end: None,
};

/// Keys interned at MINIT (Swoole's `SW_ZSTR_KNOWN`; interning returns the
/// same strings).
#[derive(Clone, Copy)]
pub(crate) enum Key {
    Host,
    UserAgent,
    Accept,
    ContentType,
    ContentLength,
    Authorization,
    Connection,
    AcceptEncoding,
    QueryString,
    RequestMethod,
    RequestUri,
    PathInfo,
    RequestTime,
    RequestTimeFloat,
    ServerProtocol,
    Http10,
    Http11,
    ServerPort,
    RemotePort,
    ServerAddr,
    RemoteAddr,
    LoopbackV4,
    LoopbackV6,
    MasterTime,
    Http2,
}

const KEY_NAMES: [&[u8]; 25] = [
    b"host",
    b"user-agent",
    b"accept",
    b"content-type",
    b"content-length",
    b"authorization",
    b"connection",
    b"accept-encoding",
    b"query_string",
    b"request_method",
    b"request_uri",
    b"path_info",
    b"request_time",
    b"request_time_float",
    b"server_protocol",
    b"HTTP/1.0",
    b"HTTP/1.1",
    b"server_port",
    b"remote_port",
    b"server_addr",
    b"remote_addr",
    b"127.0.0.1",
    b"::1",
    b"master_time",
    b"HTTP/2",
];
/// The header keys `request_key` numbers 0 to 7.
const HEADER_KEYS: [Key; 8] = [
    Key::Host,
    Key::UserAgent,
    Key::Accept,
    Key::ContentType,
    Key::ContentLength,
    Key::Authorization,
    Key::Connection,
    Key::AcceptEncoding,
];

static KEYS: [AtomicPtr<zend_string>; 25] = [const { AtomicPtr::new(ptr::null_mut()) }; 25];
/// `Swoole\Http\Request`'s `get`, `post`, `cookie`, `files` and `tmpfiles` slots.
/// ... and `server`, `header` and `fd`.
static OFFSETS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
pub(crate) static CAPABILITIES: AtomicU32 = AtomicU32::new(0);

/// The interned key, None before MINIT.
pub(crate) fn key(key: Key) -> Option<&'static ZStr> {
    let key = KEYS.get(key as usize)?.load(Relaxed);
    // SAFETY: null, or a string MINIT interned permanently.
    unsafe { ZStr::from_ptr(key) }
}

/// The request's array properties, read (and created) on first use.
#[derive(Clone, Copy)]
pub(crate) enum Property {
    Get,
    Post,
    Cookie,
    Files,
    TmpFiles,
    Server,
    Header,
}

/// MINIT: intern the keys and find the array properties' slots.
/// Called once at MINIT, after `Swoole\Http\Request` is registered.
pub(crate) fn minit(codecs: u32) {
    CAPABILITIES.store(codecs, Relaxed);
    for (slot, name) in KEYS.iter().zip(KEY_NAMES) {
        // SAFETY: MINIT, so the string is interned permanently.
        slot.store(
            unsafe { swr_zend_http_intern(name.as_ptr().cast(), name.len()) },
            Relaxed,
        );
    }
    let ce: *mut zend_class_entry = crate::request::entry();
    for (slot, name) in OFFSETS.iter().zip([
        "get", "post", "cookie", "files", "tmpfiles", "server", "header", "fd",
    ]) {
        // SAFETY: the registered class, which declares the property.
        let offset = unsafe { swr_zend_http_property_offset(ce, name.as_ptr().cast(), name.len()) };
        slot.store(offset, Relaxed);
    }
}

/// `Swoole\Http\Request`'s declared `fd` slot.
pub(crate) fn fd_offset() -> u32 {
    OFFSETS[7].load(Relaxed)
}

/// `Swoole\Http\Request`'s declared `tmpfiles` slot.
pub(crate) fn tmpfiles_offset() -> u32 {
    OFFSETS
        .get(Property::TmpFiles as usize)
        .map_or(0, |offset| offset.load(Relaxed))
}

/// `swoole_http_init_and_read_property`: the array behind a property slot,
/// creating it on first use. Like upstream, the slot keeps an uncounted copy.
pub(crate) fn property(state: &mut State, which: Property) -> *mut zval {
    let request = &mut state.request;
    let slot: &mut Slot = match which {
        Property::Get => &mut request.zget,
        Property::Post => &mut request.zpost,
        Property::Cookie => &mut request.zcookie,
        Property::Files => &mut request.zfiles,
        Property::TmpFiles => &mut request.ztmpfiles,
        Property::Server => &mut request.zserver,
        Property::Header => &mut request.zheader,
    };
    let offset = OFFSETS
        .get(which as usize)
        .map_or(0, |offset| offset.load(Relaxed));
    let size = if matches!(which, Property::Server) {
        HT_MIN_SIZE << 1
    } else {
        HT_MIN_SIZE
    };
    slot.init_array(&request.zobject, offset, size)
}

/// `http_server_add_server_array`: `$request->server[key] = value`.
pub(crate) fn add_server(state: &mut State, key_: Key, value_: Owned) {
    if let Some(server) = state.request.zserver.array_mut()
        && let Some(key) = key(key_)
    {
        server.add_new(key, value_);
    }
}

/// A new string of the `length` bytes at `bytes`.
fn stringl(bytes: *const u8, length: usize) -> Owned {
    // SAFETY: `bytes` spans `length` readable bytes, per every caller.
    Owned::string(ZString::new(unsafe {
        std::slice::from_raw_parts(bytes, length)
    }))
}

fn estrndup(bytes: *const u8, length: usize) -> *mut c_char {
    // SAFETY: `bytes` spans `length` readable bytes, per every caller.
    unsafe { swr_zend_http_estrndup(bytes.cast(), length) }
}

/// `swoole_php_treat_data(arg, estrndup(bytes), array)`, which frees the copy.
fn treat_data(arg: c_int, bytes: *const u8, length: usize, array: *mut zval) {
    // SAFETY: a fresh request-memory string and a live array.
    if !unsafe { swr_zend_http_treat_data(arg, estrndup(bytes, length), array) } {
        // SAFETY: reads an INI value.
        let limit = unsafe { swr_zend_http_max_input_vars() };
        log::warning(
            c"swoole_php_treat_data",
            format_args!(
                "Input variables exceeded {limit}. To increase the limit change max_input_vars in php.ini."
            ),
        );
    }
}

/// `swoole_llhttp_parser_init(&ctx->parser, HTTP_REQUEST, ctx)`.
pub(crate) fn init_request_parser(ctx: *mut crate::host::Context) {
    let state = crate::send::state(ctx);
    // SAFETY: the context's own parser; llhttp keeps no settings pointer from init.
    unsafe { llhttp_init(&raw mut state.parser, HTTP_REQUEST, ptr::null()) };
    state.parser.data = ctx;
}

/// `HTTP_REQUEST`.
const HTTP_REQUEST: c_int = 1;

/// `swoole_http_parse_cookie`.
pub(crate) fn parse_cookie(array: *mut zval, bytes: *const u8, length: usize) {
    if length != 0 {
        treat_data(PARSE_COOKIE, bytes, length, array);
    }
}

/// The context the parser belongs to (`parser->data`).
/// # Safety
/// `parser` is a live context's parser, inside its callbacks.
unsafe fn state<'a>(parser: *mut crate::state::Llhttp) -> &'a mut State {
    // SAFETY: per the caller.
    let data = unsafe { (*parser).data };
    // SAFETY: `data` is the context, whose prefix `State` is.
    unsafe { &mut *data.cast::<State>() }
}

/// The context a multipart parser belongs to (`p->data`).
/// # Safety
/// `parser` is a context's multipart parser, inside its callbacks.
unsafe fn part_state<'a>(parser: *mut Multipart) -> &'a mut State {
    // SAFETY: per the caller.
    let data = unsafe { (*parser).data };
    // SAFETY: `data` is the context, set by `multipart_init`.
    unsafe { &mut *data.cast::<State>() }
}

/// `swoole_llhttp_parser_execute`.
fn execute(
    state: *mut State,
    settings: &'static Settings,
    data: *const c_char,
    length: usize,
) -> usize {
    // SAFETY: a live context; only the field's address is taken.
    let parser = unsafe { &raw mut (*state).parser };
    // SAFETY: as above.
    unsafe { (*parser).settings = ptr::from_ref(settings).cast() };
    // SAFETY: an initialised parser and `length` readable bytes.
    let result = unsafe { llhttp_execute(parser, data, length) };
    if result == 0 {
        return length;
    }
    // SAFETY: the parser stopped inside `data`.
    let parsed = unsafe { llhttp_get_error_pos(parser) }
        .addr()
        .wrapping_sub(data.addr());
    match result {
        // SAFETY: a paused parser.
        PAUSED => unsafe { llhttp_resume(parser) },
        // SAFETY: as above.
        PAUSED_UPGRADE => unsafe { llhttp_resume_after_upgrade(parser) },
        _ => {}
    }
    parsed
}

/// `Context::parse`.
/// # Safety
/// `context` is a live `swoole::http::Context` whose parser is initialised.
pub(crate) unsafe fn parse_request(
    context: *mut c_void,
    data: *const c_char,
    length: usize,
) -> usize {
    execute(context.cast(), &SERVER, data, length)
}

/// `Request::parse()`: append to `request.zdata` and parse what was appended.
pub(crate) fn parse_piece(state: *mut State, data: &[u8]) -> usize {
    // SAFETY: a live context.
    let zdata = unsafe { &mut (*state).request.zdata };
    let joined = [zdata.bytes().unwrap_or_default(), data].concat();
    *zdata = Owned::string(ZString::new(&joined));
    // SAFETY: as above.
    let (start, length) = zdata_span(unsafe { &*state });
    execute(
        state,
        &PIECES,
        start.wrapping_add(length - data.len()).cast(),
        data.len(),
    )
}

/// `request.zdata`'s bytes.
fn zdata_span(state: &State) -> (*const u8, usize) {
    let bytes = state.request.zdata.bytes().unwrap_or_default();
    (bytes.as_ptr(), bytes.len())
}

unsafe extern "C" fn on_url(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    url(unsafe { state(parser) }, at.cast(), length)
}

fn url(state: &mut State, at: *const u8, length: usize) -> c_int {
    let mut parts = UrlParts::default();
    // SAFETY: llhttp's span and a writable result.
    if unsafe { swr_v1_httpbindings_request_url(Text { data: at, length }, &raw mut parts) } != 0 {
        return -1;
    }
    state.request.path = estrndup(at, parts.path_length);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Request::path_len is uint32_t"
    )]
    let path_len = parts.path_length as u32;
    state.request.path_len = path_len;
    if parts.query_length == 0 {
        return 0;
    }
    let query = at.wrapping_add(parts.query_offset);
    add_server(state, Key::QueryString, stringl(query, parts.query_length));
    let get = property(state, Property::Get);
    treat_data(PARSE_STRING, query, parts.query_length, get);
    0
}

unsafe extern "C" fn on_header_field(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    // A request's trailer fields are ignored.
    if state.parser.flags & TRAILING != 0 {
        return 0;
    }
    header_field(state, at.cast(), length)
}

fn header_field(state: &mut State, at: *const u8, length: usize) -> c_int {
    state.parse.header_name = at;
    state.parse.header_name_len = length;
    state.parse.header_value_len = 0;
    0
}

unsafe extern "C" fn on_piece_url(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    state.parse.url_length += length;
    let (start, total) = zdata_span(state);
    // llhttp flushes unfinished spans at the end of each input buffer.
    if at.wrapping_add(length).cast() == start.wrapping_add(total) {
        return 0;
    }
    let url_length = state.parse.url_length;
    url(
        state,
        at.wrapping_add(length).wrapping_sub(url_length).cast(),
        url_length,
    )
}

unsafe extern "C" fn on_piece_header_field(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    if state.parser.flags & TRAILING != 0 {
        return 0;
    }
    if state.parse.header_name_length == 0 {
        let (start, _) = zdata_span(state);
        state.parse.header_name_offset = at.addr().wrapping_sub(start.addr());
    }
    state.parse.header_name_length += length;
    0
}

unsafe extern "C" fn on_piece_header_value(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    if state.parser.flags & TRAILING != 0 {
        return 0;
    }
    state.parse.header_value_length += length;
    let (start, total) = zdata_span(state);
    if at.wrapping_add(length).cast() == start.wrapping_add(total) {
        return 0;
    }
    let name = start.wrapping_add(state.parse.header_name_offset);
    header_field(state, name, state.parse.header_name_length);
    let value_length = state.parse.header_value_length;
    let result = header_value(
        state,
        at.wrapping_add(length).wrapping_sub(value_length).cast(),
        value_length,
    );
    state.parse.header_name_length = 0;
    state.parse.header_value_length = 0;
    result
}

unsafe extern "C" fn on_header_value(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    if state.parser.flags & TRAILING != 0 {
        return 0;
    }
    header_value(state, at.cast(), length)
}

fn set_error_reason(state: &mut State, reason: &CStr) {
    // SAFETY: the context's parser and a static reason.
    unsafe { llhttp_set_error_reason(&raw mut state.parser, reason.as_ptr()) };
}

/// `swoole_http_token_list_contains_value(at, length, "websocket")`, with
/// its pointer-sized fast comparison.
fn contains_websocket(at: *const u8, length: usize) -> i32 {
    let value = c"websocket".as_ptr().cast::<u8>();
    let fast = size_of::<*const c_char>() - 1;
    // SAFETY: llhttp's span; mode 0 only reads it.
    let result = unsafe { swr_v1_httpbindings_tokens(at.cast_mut(), length, value, fast, 0) };
    if result != 2 {
        return i32::from(result == 1);
    }
    if length >= STACK_BUFFER_SIZE {
        return -1;
    }
    let mut scratch = Vec::with_capacity(length + 1);
    // SAFETY: llhttp's span of `length` bytes.
    scratch.extend_from_slice(unsafe { std::slice::from_raw_parts(at, length) });
    scratch.push(0);
    // SAFETY: a writable copy with its terminator.
    let found = unsafe { swr_v1_httpbindings_tokens(scratch.as_mut_ptr(), length, value, fast, 1) };
    i32::from(found == 1)
}

/// An Upgrade: websocket request on a server's WebSocket port starts its
/// connection's handshake. False when the session is gone.
fn websocket_connection(state: &State) -> bool {
    if crate::context::co_socket(state) {
        return true;
    }
    let server = crate::context::server(state);
    let Some(conn) = server.by_session(state.fd) else {
        return false;
    };
    let port = server.port_by_server_fd(conn.server_fd);
    // SAFETY: the connection's live port.
    if !port.is_null() && unsafe { view::port_websocket(port) } {
        conn.websocket_status = STATUS_CONNECTION;
    }
    true
}

/// `WebSocket::STATUS_CONNECTION`.
const STATUS_CONNECTION: u8 = 1;

fn header_value(state: &mut State, at: *const u8, length: usize) -> c_int {
    // SAFETY: the current header's name, inside llhttp's input.
    let name =
        unsafe { std::slice::from_raw_parts(state.parse.header_name, state.parse.header_name_len) };
    // SAFETY: llhttp's value span.
    let value_ = unsafe { std::slice::from_raw_parts(at, length) };
    let compression =
        CAPABILITIES.load(Relaxed) & COMPRESSION != 0 && state.enable_compression != 0;
    let action = request_header(
        name,
        value_,
        state.parser.method,
        state.parse_cookie != 0,
        compression,
    );
    match action {
        RequestHeader::Cookie => {
            let cookie = property(state, Property::Cookie);
            parse_cookie(cookie, at, length);
            return 0;
        }
        RequestHeader::Upgrade => {
            const MAX_LENGTH: usize = STACK_BUFFER_SIZE - 1;
            let seen = state.parse.header_value_len;
            if seen > MAX_LENGTH || length > MAX_LENGTH - seen {
                set_error_reason(state, c"Upgrade header value exceeds the maximum length");
                return USER;
            }
            state.parse.header_value_len += length;
            match contains_websocket(at, length) {
                -1 => {
                    set_error_reason(state, c"Upgrade header value exceeds the maximum length");
                    return USER;
                }
                1 => {
                    state.websocket = 1;
                    if !websocket_connection(state) {
                        let message = format!("session[{}] is closed", state.fd);
                        host::log(
                            LOG_TRACE,
                            SESSION_CLOSED,
                            c"http_request_on_header_value",
                            message,
                        );
                        return -1;
                    }
                }
                _ => {}
            }
        }
        RequestHeader::Form => state.request.post_form_urlencoded = 1,
        RequestHeader::Multipart => {
            let Some((boundary, boundary_len)) =
                multipart_boundary(state, at, length, b"multipart/form-data".len())
            else {
                return -1;
            };
            multipart_init(state, boundary, boundary_len);
        }
        RequestHeader::Compression => compression_method(state, at, length),
        RequestHeader::Append => {}
    }
    let value_ = stringl(at, length);
    // The request's `header` array, created with the request.
    let Some(header) = state.request.zheader.array_mut() else {
        return 0;
    };
    let known = request_key(name).and_then(|index| HEADER_KEYS.get(index));
    match known.and_then(|&known| key(known)) {
        Some(known) => header.update(known, value_),
        None => header.merge(&name.to_ascii_lowercase(), value_),
    }
    0
}

unsafe extern "C" fn on_headers_complete(parser: *mut crate::state::Llhttp) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    let request = &mut state.request;
    request.version =
        c_int::from(state.parser.http_major) * 100 + c_int::from(state.parser.http_minor);
    let path_start = request.path.cast::<u8>().cast_const();
    let path_len = request.path_len as usize;
    let path: &[u8] = if path_start.is_null() {
        &[]
    } else {
        // SAFETY: `on_url` set the path to `path_len` request-memory bytes.
        unsafe { std::slice::from_raw_parts(path_start, path_len) }
    };
    let ext = path
        .iter()
        .rposition(|&byte| byte == b'.')
        .map_or(path_len, |dot| dot + 1);
    request.ext = path_start.wrapping_add(ext).cast();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Request::ext_len is uint32_t"
    )]
    let ext_len = (path_len - ext) as u32;
    request.ext_len = ext_len;

    let method = method_name(state);
    let method = Owned::string(ZString::new(method.to_bytes()));
    add_server(state, Key::RequestMethod, method);
    add_server(state, Key::RequestUri, Owned::string(ZString::new(path)));

    // path_info should be decoded
    let decoded = ZString::new(path);
    // SAFETY: a fresh string of `path.len()` bytes, decoded in place.
    let decoded_len = unsafe { php_url_decode(string::val(decoded.as_ptr()).cast(), path.len()) };
    // SAFETY: the decoded bytes fit the string, which nothing else holds.
    unsafe { (*decoded.as_ptr()).len = decoded_len };
    add_server(state, Key::PathInfo, Owned::string(decoded));
    request_times(state);
    let protocol = if state.request.version == 101 {
        Key::Http11
    } else {
        Key::Http10
    };
    add_protocol(state, protocol);

    // SAFETY: the context's parser.
    state.keepalive = u8::from(unsafe { llhttp_should_keep_alive(&raw const state.parser) } != 0);
    state.recv_chunked = u8::from(state.parser.flags & CHUNKED != 0);
    state.parse.header_name = ptr::null();
    0
}

/// Swoole's `microtime()`.
/// `$request->server`'s `request_time` and `request_time_float`.
pub(crate) fn request_times(state: &mut State) {
    // SAFETY: `time(NULL)`.
    let now = unsafe { libc::time(ptr::null_mut()) };
    add_server(state, Key::RequestTime, Owned::long(now));
    add_server(state, Key::RequestTimeFloat, Owned::double(microtime()));
}

/// `$request->server['server_protocol']`, an interned protocol name.
pub(crate) fn add_protocol(state: &mut State, protocol: Key) {
    if let Some(protocol) = key(protocol) {
        add_server(
            state,
            Key::ServerProtocol,
            Owned::string(protocol.to_owned()),
        );
    }
}

fn microtime() -> f64 {
    let mut now = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    // SAFETY: a writable timeval and no timezone.
    unsafe { libc::gettimeofday(&raw mut now, ptr::null_mut()) };
    #[expect(clippy::cast_precision_loss, reason = "as upstream's (double) casts")]
    let seconds =
        now.tv_sec as f64 + (f64::from(u32::try_from(now.tv_usec).unwrap_or(0)) / 1_000_000.0);
    seconds
}

unsafe extern "C" fn on_body(
    parser: *mut crate::state::Llhttp,
    at: *const c_char,
    length: usize,
) -> c_int {
    if length == 0 {
        return 0;
    }
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    let at = at.cast::<u8>();
    let received = state
        .parse
        .chunked_body
        .as_ref()
        .map_or(state.request.body_length, |body| body.len());
    let beginning = received == 0;
    if state.recv_chunked != 0 {
        // SAFETY: llhttp's span.
        let span = unsafe { std::slice::from_raw_parts(at, length) };
        state
            .parse
            .chunked_body
            .get_or_insert_with(|| Box::new(Vec::with_capacity(BUFFER_SIZE_STD)))
            .extend_from_slice(span);
    } else {
        state.request.body_at = at.wrapping_sub(state.request.body_length).cast();
        state.request.body_length += length;
    }
    if !state.mt_parser.is_null() && state.recv_chunked == 0 {
        let mut prefix = 0;
        // SAFETY: llhttp's span and a writable result.
        unsafe {
            swr_v1_httpbindings_body_prefix(
                Text { data: at, length },
                u32::from(beginning),
                &raw mut prefix,
            )
        };
        if !multipart_parse(state, at.wrapping_add(prefix), length - prefix) {
            return -1;
        }
    }
    if state.completed != 0 { PAUSED } else { 0 }
}

unsafe extern "C" fn on_message_complete(parser: *mut crate::state::Llhttp) -> c_int {
    // SAFETY: llhttp calls back with the context's parser.
    let state = unsafe { state(parser) };
    let chunked = state
        .parse
        .chunked_body
        .as_ref()
        .map(|body| (body.as_ptr(), body.len()));
    let content_length = chunked.map_or(state.request.body_length, |(_, length)| length);
    if let Some((body, length)) = chunked
        && !state.mt_parser.is_null()
    {
        // The bundled multipart parser rejects calls that end inside a part header. llhttp body spans can end at
        // receive boundaries, so parse the dechunked body in one pass here instead of parsing each span.
        let mut prefix = 0;
        // SAFETY: the dechunked body and a writable result.
        unsafe { swr_v1_httpbindings_body_prefix(Text { data: body, length }, 1, &raw mut prefix) };
        if !multipart_parse(state, body.wrapping_add(prefix), length - prefix) {
            return -1;
        }
    }
    let form_source = swr_v1_httpbindings_form_source(
        u32::from(chunked.is_some())
            | (u32::from(state.parse_body != 0) << 1)
            | (u32::from(state.request.post_form_urlencoded != 0) << 2)
            | (u32::from(state.recv_chunked != 0) << 3)
            | (u32::from(!state.request.body_at.is_null()) << 4),
    );
    match (form_source, chunked) {
        (1, Some((body, _))) => {
            let post = property(state, Property::Post);
            treat_data(PARSE_STRING, body, content_length, post);
        }
        (2, _) => {
            let post = property(state, Property::Post);
            let body = state.request.body_at.cast();
            treat_data(PARSE_STRING, body, state.request.body_length, post);
        }
        _ => {}
    }
    multipart_free(state);
    state.parse.form_data = None;
    state.completed = 1;
    // The analysis of the http protocol has been completed, no further processing is needed.
    PAUSED
}

fn multipart_free(state: &mut State) {
    if !state.mt_parser.is_null() {
        // SAFETY: the parser `multipart_init` made, freed once.
        unsafe { multipart_parser_free(state.mt_parser) };
        state.mt_parser = ptr::null_mut();
    }
}

/// `Context::free`'s share: what the parser still holds.
/// # Safety
/// `context` is a live `swoole::http::Context`, about to be deleted.
pub(crate) unsafe fn parse_free(context: *mut c_void) {
    // SAFETY: per the caller.
    let state = unsafe { &mut *context.cast::<State>() };
    multipart_free(state);
    state.parse.form_data = None;
    state.parse.chunked_body = None;
    state.parse.form_data_name = None;
    state.parse.input_name = None;
}

/// `Context::get_multipart_boundary`.
fn multipart_boundary(
    state: &mut State,
    at: *const u8,
    length: usize,
    offset: usize,
) -> Option<(*const u8, usize)> {
    let input = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the header value's span.
        unsafe { core::slice::from_raw_parts(at, length) }
    };
    let Ok(Some(span)) = swoole_rs_http::boundary(input, offset) else {
        let message = format!("boundary of multipart/form-data not found, fd:{}", state.fd);
        host::log(0, 0, c"Context::get_multipart_boundary", message);
        /* make it same with protocol error */
        state.parser.error = INVALID_HEADER_TOKEN;
        return None;
    };
    Some((at.wrapping_add(span.offset), span.length))
}

/// `Context::get_multipart_boundary`, for the HTTP/2 server.
/// # Safety
/// `context` is a live context; `at` spans `length` bytes; the outputs are writable.
pub(crate) unsafe fn multipart_boundary_of(
    context: *mut c_void,
    at: *const c_char,
    length: usize,
    offset: usize,
    boundary: *mut *mut c_char,
    boundary_length: *mut c_int,
) -> bool {
    // SAFETY: per the caller.
    let state = unsafe { &mut *context.cast::<State>() };
    let Some((start, len)) = multipart_boundary(state, at.cast(), length, offset) else {
        return false;
    };
    // SAFETY: per the caller.
    unsafe { *boundary = start.cast_mut().cast() };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "upstream's int boundary length"
    )]
    let len = len as c_int;
    // SAFETY: per the caller.
    unsafe { *boundary_length = len };
    true
}

/// `Context::init_multipart_parser`.
fn multipart_init(state: &mut State, boundary: *const u8, length: usize) -> bool {
    // SAFETY: the boundary's span and the static settings.
    let parser = unsafe { multipart_parser_init(boundary.cast(), length, &raw const PARTS) };
    state.mt_parser = parser;
    if parser.is_null() {
        host::fatal("multipart_parser_init() failed");
        return false;
    }
    state.parse.form_data = Some(Box::new(Vec::with_capacity(BUFFER_SIZE_STD)));
    // SAFETY: the fresh parser.
    unsafe { (*parser).data = ptr::from_mut(state).cast() };
    true
}

/// `Context::init_multipart_parser`, for the HTTP/2 server.
/// # Safety
/// `context` is a live context and `boundary` spans `length` bytes.
pub(crate) unsafe fn init_multipart(
    context: *mut c_void,
    boundary: *const c_char,
    length: c_int,
) -> bool {
    // SAFETY: per the caller.
    let state = unsafe { &mut *context.cast::<State>() };
    #[expect(clippy::cast_sign_loss, reason = "upstream passes int to size_t")]
    let length = length as usize;
    multipart_init(state, boundary.cast(), length)
}

/// `Context::parse_multipart_data`.
pub(crate) fn multipart_parse(state: &mut State, at: *const u8, length: usize) -> bool {
    let parser = state.mt_parser;
    // SAFETY: the context's multipart parser and `length` readable bytes.
    let parsed = unsafe { multipart_parser_execute(parser, at.cast(), length) };
    let func = c"Context::parse_multipart_data";
    if parsed < 0 {
        let mut reason = vec![0u8; STACK_BUFFER_SIZE];
        // SAFETY: a writable buffer of its length.
        let written =
            unsafe { multipart_parser_error_msg(parser, reason.as_mut_ptr().cast(), reason.len()) };
        reason.truncate(usize::try_from(written).unwrap_or(0).min(STACK_BUFFER_SIZE));
        let mut message = b"parse multipart body failed, reason: ".to_vec();
        message.extend_from_slice(&reason);
        host::log(LOG_NOTICE, SERVER_INVALID_REQUEST, func, message);
        return false;
    }
    if parsed.unsigned_abs() != length {
        let message = format!("parse multipart body failed, {parsed}/{length} bytes processed");
        host::log(LOG_NOTICE, SERVER_INVALID_REQUEST, func, message);
        return false;
    }
    true
}

/// `Context::set_compression_method`.
pub(crate) fn compression_method(state: &mut State, at: *const u8, length: usize) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream truncates to uint32_t"
    )]
    let length = length as u32 as usize;
    let codecs = CAPABILITIES.load(Relaxed) & CODECS;
    // SAFETY: the header value's span.
    let method = unsafe { swr_v1_httpbindings_compression(Text { data: at, length }, codecs) };
    state.accept_compression = u8::from(method > 0);
    if method > 0 {
        #[expect(clippy::cast_possible_truncation, reason = "methods are 1 to 4")]
        let method = method as i8;
        state.compression_method = method;
    }
}

unsafe extern "C" fn on_part_header_field(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the context's parser.
    header_field(unsafe { part_state(parser) }, at.cast(), length)
}

/// A string zval's bytes up to its first NUL (`%s`, `strlen`), and its length.
fn string_value(found: &Value) -> Option<(&[u8], usize)> {
    let bytes = found.bytes()?;
    let text = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
    Some((text, bytes.len()))
}

/// `swoole_strlcpy` into a `SW_HTTP_FORM_KEYLEN` buffer, then
/// `http_trim_double_quote` over `length` bytes of it.
fn trimmed(text: &[u8], length: usize) -> Box<[u8]> {
    let mut buffer = [0u8; FORM_KEYLEN];
    for (slot, byte) in buffer.iter_mut().zip(text).take(FORM_KEYLEN - 1) {
        *slot = *byte;
    }
    let mut result = Trimmed::default();
    // SAFETY: the buffer holds `length` (< FORM_KEYLEN) writable bytes.
    if unsafe { swr_v1_httpbindings_trim_quotes(buffer.as_mut_ptr(), length, &raw mut result) } != 0
    {
        result = Trimmed { offset: 0, length };
    }
    let trimmed = buffer.get(result.offset..result.offset + result.length);
    trimmed.unwrap_or_default().into()
}

/// A NUL-terminated copy (`estrndup`).
fn c_copy(bytes: &[u8]) -> Box<[u8]> {
    let mut copy = Vec::with_capacity(bytes.len() + 1);
    copy.extend_from_slice(bytes);
    copy.push(0);
    copy.into_boxed_slice()
}

/// `swoole_strnpos`.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

unsafe extern "C" fn on_part_header_value(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the context's parser.
    let state = unsafe { part_state(parser) };
    // SAFETY: the parser's span.
    let value_ = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
    let func = c"multipart_body_on_header_value";
    // Hash collision attack
    // SAFETY: reads a PHP INI global.
    let max_input_vars = unsafe { swr_zend_http_max_input_vars() };
    if i64::from(state.parse.input_var_num) > max_input_vars {
        host::notice(&format!(
            "Input variables exceeded {max_input_vars}. To increase the limit change max_input_vars in php.ini"
        ));
        return 0;
    }
    state.parse.input_var_num = state.parse.input_var_num.wrapping_add(1);

    // SAFETY: the current part header's name, inside the parser's input.
    let name =
        unsafe { std::slice::from_raw_parts(state.parse.header_name, state.parse.header_name_len) };
    if name.eq_ignore_ascii_case(b"content-disposition") {
        let offset = if position(value_, b"form-data;").is_some() {
            b"form-data;".len()
        } else if position(value_, b"attachment;").is_some() {
            b"attachment;".len()
        } else {
            let mut message = b"Unsupported Content-Disposition [".to_vec();
            message.extend_from_slice(value_);
            message.push(b']');
            host::log(0, 0, func, message);
            return 0;
        };
        // Released on every return, which upstream's early ones leak.
        let fields = OwnedArray::default().into_owned();
        parse_cookie(
            fields.as_ptr(),
            value_.as_ptr().wrapping_add(offset),
            length - offset,
        );
        let field = |key: &[u8]| fields.array().and_then(|fields| fields.find(key));

        let Some((form_name, form_name_len)) = field(b"name").and_then(string_value) else {
            return 0;
        };
        if form_name_len >= FORM_KEYLEN {
            let mut message = b"form_name[".to_vec();
            message.extend_from_slice(form_name);
            message.extend_from_slice(b"] is too large");
            host::log(0, 0, func, message);
            return -1;
        }
        let form_name = trimmed(form_name, form_name_len);
        if let Some(filename) = field(b"filename") {
            // upload file
            let (filename, filename_len) = string_value(filename).unwrap_or((&[], 0));
            if filename_len >= FORM_KEYLEN {
                let mut message = b"filename[".to_vec();
                message.extend_from_slice(filename);
                message.extend_from_slice(b"] is too large");
                host::log(0, 0, func, message);
                return -1;
            }
            state.parse.input_name = Some(c_copy(&form_name));
            let mut header = OwnedArray::default();
            if state.parse.tmp_content_type.is_null() {
                header.insert_bytes(b"type", Some(b""));
            } else {
                // SAFETY: a span of the parser's input, still live.
                let content_type = unsafe {
                    std::slice::from_raw_parts(
                        state.parse.tmp_content_type,
                        state.parse.tmp_content_type_len,
                    )
                };
                header.insert_bytes(b"type", Some(content_type));
                state.parse.tmp_content_type = ptr::null();
            }
            header.insert_bytes(b"tmp_name", Some(b""));
            header.insert_long(b"size", 0);
            let filename = trimmed(filename, filename_len);
            header.insert_bytes(b"name", Some(&filename));
            header.insert_long(
                b"error",
                if filename.is_empty() {
                    UPLOAD_NO_FILE
                } else {
                    UPLOAD_OK
                },
            );
            state.parse.multipart_header = header.into_owned();
        } else {
            // POST form data
            state.parse.form_data_name = Some(c_copy(&form_name));
        }
    } else if name.eq_ignore_ascii_case(b"content-type") {
        if state.parse.multipart_header.array().is_some() {
            if upload_error(state).and_then(Value::as_long) == Some(UPLOAD_OK) {
                upload_bytes(state, b"type", value_);
            }
        } else {
            state.parse.tmp_content_type = value_.as_ptr();
            state.parse.tmp_content_type_len = length;
        }
    } else if name.eq_ignore_ascii_case(b"swoole-upload-file") && state.upload_preprocessed != 0 {
        // When the "SW_HTTP_UPLOAD_FILE" header appears in the request, it indicates that the uploaded file has been
        // saved in a temporary file. The binary content in the message body will be replaced with the temporary
        // filename. However, the Content-Length still reflects the original message size, causing llhttp to believe
        // there is still data to be received. As a result, llhttp fails to trigger the message callback. Therefore, we
        // need to set `ctx->completed = 1` to indicate that the message processing is complete.
        state.completed = 1;
        upload_bytes(state, b"tmp_name", value_);
        upload_long(state, b"error", UPLOAD_FILE_READY);
        upload_long(state, b"size", path_size(value_));
        let tmpfiles = property(state, Property::TmpFiles);
        // SAFETY: the request's `tmpfiles` array and the path's bytes.
        unsafe { swr_zend_http_upload(tmpfiles, value_.as_ptr().cast(), value_.len()) };
    }
    0
}

/// The current upload's `error`, when it has one.
fn upload_error(state: &State) -> Option<&Value> {
    state.parse.multipart_header.array()?.find(b"error")
}

/// `add_assoc_long` to the current upload's `$_FILES` entry.
fn upload_long(state: &mut State, key: &[u8], value: i64) {
    if let Some(header) = state.parse.multipart_header.array_mut() {
        header.insert_long(key, value);
    }
}

/// `add_assoc_stringl` to the current upload's `$_FILES` entry.
fn upload_bytes(state: &mut State, key: &[u8], value: &[u8]) {
    if let Some(header) = state.parse.multipart_header.array_mut() {
        header.insert_bytes(key, Some(value));
    }
}

/// `swoole::file_get_size(path)`.
fn path_size(path: &[u8]) -> i64 {
    let path = <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path);
    std::fs::metadata(path).map_or(-1, |meta| i64::try_from(meta.len()).unwrap_or(-1))
}

/// `swoole::file_get_size(fp)`: flushed, and only for a regular file.
fn file_size(fp: *mut libc::FILE) -> i64 {
    // SAFETY: an open stream.
    unsafe { libc::fflush(fp) };
    // SAFETY: as above.
    let fd = unsafe { libc::fileno(fp) };
    // SAFETY: all zeroes is a valid `stat`.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: an open descriptor and a writable `stat`.
    if unsafe { libc::fstat(fd, &raw mut status) } < 0
        || status.st_mode & libc::S_IFMT != libc::S_IFREG
    {
        return -1;
    }
    status.st_size
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

unsafe extern "C" fn on_part_data(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the context's parser.
    let state = unsafe { part_state(parser) };
    // SAFETY: the parser's span.
    let data = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
    if state.parse.form_data_name.is_some() {
        state
            .parse
            .form_data
            .get_or_insert_with(|| Box::new(Vec::with_capacity(BUFFER_SIZE_STD)))
            .extend_from_slice(data);
        return 0;
    }
    // SAFETY: the live multipart parser.
    let fp = unsafe { (*parser).fp };
    if fp.is_null() {
        return 0;
    }
    // SAFETY: an open stream and the parser's span.
    let written = unsafe { libc::fwrite(at.cast(), 1, length, fp) };
    if written != length {
        upload_long(state, b"error", UPLOAD_CANT_WRITE);
        // SAFETY: the stream, closed once.
        unsafe { libc::fclose(fp) };
        // SAFETY: the live multipart parser.
        unsafe { (*parser).fp = ptr::null_mut() };
        host::log(
            0,
            -errno(),
            c"multipart_body_on_data",
            "write upload file failed",
        );
    }
    0
}

unsafe extern "C" fn on_part_headers_complete(parser: *mut Multipart) -> c_int {
    // SAFETY: the multipart parser calls back with the context's parser.
    let state = unsafe { part_state(parser) };
    if state.parse.input_name.is_none() {
        return 0;
    }
    if upload_error(state).is_none_or(|code| code.as_long().is_some_and(|code| code != UPLOAD_OK)) {
        return 0;
    }
    let mut path = crate::context::upload_tmp_dir(state).to_bytes().to_vec();
    path.extend_from_slice(b"/swoole.upfile.XXXXXX");
    path.truncate(UPLOAD_TMPDIR_SIZE - 1);
    path.push(0);
    // SAFETY: a writable, NUL-terminated template.
    let fd = unsafe { swoole_rs_ossupport::swoole_tmpfile(path.as_mut_ptr().cast()) };
    path.pop();
    if fd < 0 {
        upload_long(state, b"error", UPLOAD_NO_TMP_DIR);
        return 0;
    }
    // SAFETY: a descriptor this function owns.
    let fp = unsafe { libc::fdopen(fd, c"wb+".as_ptr()) };
    if fp.is_null() {
        upload_long(state, b"error", UPLOAD_NO_TMP_DIR);
        let mut message = b"fopen(".to_vec();
        message.extend_from_slice(&path);
        message.push(b')');
        message.extend_from_slice(b" failed");
        host::log(0, -errno(), c"multipart_body_on_header_complete", message);
        return 0;
    }
    // SAFETY: the live multipart parser.
    unsafe { (*parser).fp = fp };
    upload_bytes(state, b"tmp_name", &path);
    let tmpfiles = property(state, Property::TmpFiles);
    // SAFETY: the request's `tmpfiles` array and the path's bytes.
    unsafe { swr_zend_http_upload(tmpfiles, path.as_ptr().cast(), path.len()) };
    0
}

unsafe extern "C" fn on_part_data_end(parser: *mut Multipart) -> c_int {
    // SAFETY: the multipart parser calls back with the context's parser.
    let state = unsafe { part_state(parser) };
    if let Some(name) = state.parse.form_data_name.take() {
        let post = property(state, Property::Post);
        let data = state.parse.form_data.get_or_insert_with(Default::default);
        // SAFETY: a NUL-terminated name, the field's bytes and the request's `post` array.
        unsafe {
            php_register_variable_safe(
                name.as_ptr().cast(),
                data.as_ptr().cast(),
                data.len(),
                post,
            );
        };
        data.clear();
        return 0;
    }
    let Some(input_name) = state.parse.input_name.take() else {
        return 0;
    };
    // SAFETY: the live multipart parser.
    let fp = unsafe { (*parser).fp };
    if !fp.is_null() {
        upload_long(state, b"size", file_size(fp));
        // SAFETY: the stream, closed once.
        unsafe { libc::fclose(fp) };
        // SAFETY: the live multipart parser.
        unsafe { (*parser).fp = ptr::null_mut() };
    }
    let Some(code) = upload_error(state).map(Value::long) else {
        state.parse.input_name = Some(input_name);
        return 0;
    };
    if code == UPLOAD_FILE_READY {
        upload_long(state, b"error", UPLOAD_OK);
    }
    let files = property(state, Property::Files);
    // The array moves into `$request->files`; like upstream, only the holder
    // goes, not the array it registered.
    let header = ManuallyDrop::new(std::mem::take(&mut state.parse.multipart_header));
    // `input_name` holds its NUL; C sees it up to the first one.
    let text = input_name
        .split(|&byte| byte == 0)
        .next()
        .unwrap_or_default();
    let bracket = text.iter().position(|&byte| byte == b'[');
    match bracket.and_then(|at| text.split_at_checked(at)) {
        Some((name, input_path)) if state.parse_files != 0 && !name.is_empty() => {
            const META: usize = FORM_KEYLEN + b"[tmp_name]".len() - 1;
            for field in ["name", "type", "tmp_name", "error", "size"] {
                let mut meta = name.get(..META - 1).unwrap_or(name).to_vec();
                meta.extend_from_slice(format!("[{field}]").as_bytes());
                meta.extend_from_slice(input_path);
                meta.truncate(META - 1);
                meta.push(0);
                let entry = header
                    .array()
                    .and_then(|header| header.find(field.as_bytes()));
                let entry = entry.map_or(ptr::null_mut(), Value::as_ptr);
                // SAFETY: a NUL-terminated name, a value from the upload's array and `$request->files`.
                unsafe { php_register_variable_ex(meta.as_ptr().cast(), entry, files) };
            }
        }
        _ => {
            // SAFETY: a NUL-terminated name, the upload's array and `$request->files`.
            unsafe { php_register_variable_ex(input_name.as_ptr().cast(), header.as_ptr(), files) };
        }
    }
    0
}

/// `getContent()`: the body received so far.
pub(crate) fn body(state: *mut State) -> Option<&'static [u8]> {
    // SAFETY: a live context, which owns its data and buffers while the
    // request object lives.
    let state: &'static State = unsafe { &*state };
    let length = state.request.body_length;
    if length > 0 {
        let data = state.request.zdata.bytes()?;
        return data.get(data.len().checked_sub(length)?..);
    }
    if let Some(chunked) = state
        .parse
        .chunked_body
        .as_deref()
        .filter(|body| !body.is_empty())
    {
        return Some(chunked);
    }
    let data = state.request.h2_data.as_deref()?;
    (!data.is_empty()).then_some(data.as_slice())
}

/// `llhttp_method_name(parser.method)`.
pub(crate) fn method_name(state: *const State) -> &'static CStr {
    // SAFETY: a live context.
    let method = unsafe { (*state).parser.method };
    // SAFETY: llhttp's own name table, for any method number.
    let name = unsafe { llhttp_method_name(c_int::from(method)) };
    // SAFETY: a static NUL-terminated name.
    unsafe { CStr::from_ptr(name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_names_are_copied_to_the_keylen_buffer_and_unquoted() {
        assert_eq!(&*trimmed(b"\"file\"", 6), b"file");
        assert_eq!(&*trimmed(b"plain", 5), b"plain");
        // A name with a NUL: strlcpy stops there, the length stays Z_STRLEN.
        assert_eq!(&*trimmed(b"ab", 3), b"ab\0");
    }

    #[test]
    fn strnpos_finds_the_needle_anywhere() {
        assert_eq!(position(b"x; form-data; name=a", b"form-data;"), Some(3));
        assert_eq!(position(b"inline", b"attachment;"), None);
    }

    #[test]
    fn names_keep_their_terminator() {
        assert_eq!(&*c_copy(b"a[b]"), b"a[b]\0");
    }

    #[test]
    fn settings_follow_the_c_structs() {
        assert_eq!(
            size_of::<Settings>(),
            25 * size_of::<usize>(),
            "llhttp_settings_t"
        );
        assert_eq!(
            size_of::<PartSettings>(),
            7 * size_of::<usize>(),
            "multipart_parser_settings"
        );
        assert_eq!(
            std::mem::offset_of!(Multipart, data),
            40,
            "multipart_parser.data"
        );
    }

    #[test]
    fn keys_are_upstreams_known_strings() {
        assert_eq!(KEY_NAMES[Key::RequestMethod as usize], b"request_method");
        assert_eq!(KEY_NAMES[Key::Http11 as usize], b"HTTP/1.1");
        assert_eq!(KEY_NAMES[Key::ServerAddr as usize], b"server_addr");
        assert_eq!(KEY_NAMES[Key::LoopbackV6 as usize], b"::1");
        assert_eq!(KEY_NAMES[HEADER_KEYS[7] as usize], b"accept-encoding");
    }
}
