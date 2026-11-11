//! The HTTP/1 request parser: llhttp and multipart callbacks over
//! `swoole::http::ContextState`, filling `Swoole\Http\Request`'s `server`,
//! `header`, `get`, `post`, `cookie` and `files` as upstream's
//! `swoole_http_request.cc` did. Both parsers are crates/http's; what Swoole
//! itself owns goes through `host`.
use crate::host;
use crate::state::{Slot, State};
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering::Relaxed};
use std::{mem::ManuallyDrop, ptr};
use swoole_rs_corebase::log;
use swoole_rs_http::llhttp::{self, Callbacks, Parser};
use swoole_rs_http::multipart_parser::MultipartParser;
use swoole_rs_httpbindings::{
    RequestHeader, body_prefix, compression, request_header, request_key, request_url,
    swr_v1_httpbindings_form_source, token_list_contains, token_list_fast, trim_quotes,
};
use swoole_rs_zend::sys::{
    swr_zend_http_estrndup, swr_zend_http_intern, swr_zend_http_max_input_vars,
    swr_zend_http_property_offset, swr_zend_http_treat_data, swr_zend_http_upload,
    zend_class_entry, zend_string, zval,
};
use swoole_rs_zend::{Owned, OwnedArray, Value, ZStr, ZString, string};

/// A request's multipart parser and the upload it is writing.
pub(crate) struct Multipart {
    parser: MultipartParser,
    fp: *mut libc::FILE,
}

/// The multipart parser's callbacks: the context and the current upload's stream.
struct Parts<'a> {
    state: &'a mut State,
    fp: &'a mut *mut libc::FILE,
}

unsafe extern "C" {
    fn php_url_decode(text: *mut c_char, len: usize) -> usize;
    fn php_register_variable_safe(
        var: *const c_char,
        val: *const c_char,
        len: usize,
        track: *mut zval,
    );
    fn php_register_variable_ex(var: *const c_char, val: *mut zval, track: *mut zval);
}

use llhttp::{
    F_CHUNKED as CHUNKED, F_TRAILING as TRAILING, HPE_INVALID_HEADER_TOKEN as INVALID_HEADER_TOKEN,
    HPE_PAUSED as PAUSED, HPE_PAUSED_UPGRADE as PAUSED_UPGRADE, HPE_USER as USER,
};
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

/// The server's llhttp callbacks, over a context's state.
struct Server<'a>(&'a mut State);

/// `Request::parse()`'s: input arrives in pieces appended to `request.zdata`.
struct Pieces<'a>(&'a mut State);

impl Callbacks for Server<'_> {
    fn parser(&mut self) -> &mut Parser {
        &mut self.0.parser
    }

    fn on_url(&mut self, data: &[u8]) -> c_int {
        url(self.0, data)
    }

    fn on_header_field(&mut self, data: &[u8]) -> c_int {
        // A request's trailer fields are ignored.
        if self.0.parser.flags & TRAILING != 0 {
            return 0;
        }
        header_field(self.0, data.as_ptr(), data.len())
    }

    fn on_header_value(&mut self, data: &[u8]) -> c_int {
        if self.0.parser.flags & TRAILING != 0 {
            return 0;
        }
        header_value(self.0, data)
    }

    fn on_headers_complete(&mut self) -> c_int {
        on_headers_complete(self.0)
    }

    fn on_body(&mut self, data: &[u8]) -> c_int {
        on_body(self.0, data)
    }

    fn on_message_complete(&mut self) -> c_int {
        on_message_complete(self.0)
    }
}

impl Callbacks for Pieces<'_> {
    fn parser(&mut self) -> &mut Parser {
        &mut self.0.parser
    }

    fn on_url(&mut self, data: &[u8]) -> c_int {
        on_piece_url(self.0, data.as_ptr().cast(), data.len())
    }

    fn on_header_field(&mut self, data: &[u8]) -> c_int {
        on_piece_header_field(self.0, data.as_ptr().cast(), data.len())
    }

    fn on_header_value(&mut self, data: &[u8]) -> c_int {
        on_piece_header_value(self.0, data.as_ptr().cast(), data.len())
    }

    fn on_headers_complete(&mut self) -> c_int {
        on_headers_complete(self.0)
    }

    fn on_body(&mut self, data: &[u8]) -> c_int {
        on_body(self.0, data)
    }

    fn on_message_complete(&mut self) -> c_int {
        on_message_complete(self.0)
    }
}

impl swoole_rs_http::multipart_parser::Parts for Parts<'_> {
    fn on_header_field(&mut self, data: &[u8]) -> c_int {
        header_field(self.state, data.as_ptr(), data.len())
    }

    fn on_header_value(&mut self, data: &[u8]) -> c_int {
        part_header_value(self.state, data)
    }

    fn on_part_data(&mut self, data: &[u8]) -> c_int {
        part_data(self.state, self.fp, data)
    }

    fn on_headers_complete(&mut self) -> c_int {
        part_headers_complete(self.state, self.fp)
    }

    fn on_part_data_end(&mut self) -> c_int {
        part_data_end(self.state, self.fp)
    }
}

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
#[inline(always)]
pub(crate) fn add_server(state: &mut State, key_: Key, value_: Owned) {
    if let Some(server) = state.request.zserver.array_mut()
        && let Some(key) = key(key_)
    {
        server.add_new(key, value_);
    }
}

/// A new string of the bytes.
fn stringl(bytes: &[u8]) -> Owned {
    Owned::string(ZString::new(bytes))
}

fn estrndup(bytes: &[u8]) -> *mut c_char {
    // SAFETY: the bytes, copied into request memory.
    unsafe { swr_zend_http_estrndup(bytes.as_ptr().cast(), bytes.len()) }
}

/// `swoole_php_treat_data(arg, estrndup(bytes), array)`, which frees the copy.
fn treat_data(arg: c_int, bytes: &[u8], array: *mut zval) {
    // SAFETY: a fresh request-memory string and a live array.
    if !unsafe { swr_zend_http_treat_data(arg, estrndup(bytes), array) } {
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
    crate::send::state(ctx).parser = Parser::new(llhttp::REQUEST);
}

/// `swoole_http_parse_cookie`.
pub(crate) fn parse_cookie(array: *mut zval, bytes: &[u8]) {
    if !bytes.is_empty() {
        treat_data(PARSE_COOKIE, bytes, array);
    }
}

/// `swoole_llhttp_parser_execute`.
fn execute(state: &mut State, pieces: bool, input: &[u8]) -> usize {
    let result = if pieces {
        llhttp::execute(&mut Pieces(state), input)
    } else {
        llhttp::execute(&mut Server(state), input)
    };
    if result == 0 {
        return input.len();
    }
    let parser = &mut state.parser;
    match result {
        PAUSED => parser.resume(),
        PAUSED_UPGRADE => parser.resume_after_upgrade(),
        _ => {}
    }
    parser.error_pos()
}

/// `Context::parse`: the bytes parsed.
/// # Safety
/// `context` is a live `swoole::http::Context` whose parser is initialised.
pub(crate) unsafe fn parse_request(context: *mut c_void, data: &[u8]) -> usize {
    execute(crate::send::state(context), false, data)
}

/// `Request::parse()`: append to `request.zdata` and parse what was appended.
pub(crate) fn parse_piece(state: &mut State, data: &[u8]) -> usize {
    let zdata = &mut state.request.zdata;
    let joined = [zdata.bytes().unwrap_or_default(), data].concat();
    *zdata = Owned::string(ZString::new(&joined));
    let (start, length) = zdata_span(state);
    // SAFETY: the appended bytes, the end of `zdata`, which the callbacks
    // read but neither write nor replace.
    let appended =
        unsafe { std::slice::from_raw_parts(start.wrapping_add(length - data.len()), data.len()) };
    execute(state, true, appended)
}

/// `request.zdata`'s bytes.
fn zdata_span(state: &State) -> (*const u8, usize) {
    let bytes = state.request.zdata.bytes().unwrap_or_default();
    (bytes.as_ptr(), bytes.len())
}

fn url(state: &mut State, span: &[u8]) -> c_int {
    let (path, query) = request_url(span);
    state.request.path = estrndup(path);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Request::path_len is uint32_t"
    )]
    let path_len = path.len() as u32;
    state.request.path_len = path_len;
    if query.is_empty() {
        return 0;
    }
    add_server(state, Key::QueryString, stringl(query));
    let get = property(state, Property::Get);
    treat_data(PARSE_STRING, query, get);
    0
}

fn header_field(state: &mut State, at: *const u8, length: usize) -> c_int {
    state.parse.header_name = at;
    state.parse.header_name_len = length;
    state.parse.header_value_len = 0;
    0
}

fn on_piece_url(state: &mut State, at: *const c_char, length: usize) -> c_int {
    state.parse.url_length += length;
    let (start, total) = zdata_span(state);
    // llhttp flushes unfinished spans at the end of each input buffer.
    if at.wrapping_add(length).cast() == start.wrapping_add(total) {
        return 0;
    }
    let url_length = state.parse.url_length;
    // SAFETY: the URL's pieces so far end at this one, inside `zdata`.
    let span = unsafe {
        std::slice::from_raw_parts(
            at.wrapping_add(length).wrapping_sub(url_length).cast(),
            url_length,
        )
    };
    url(state, span)
}

fn on_piece_header_field(state: &mut State, at: *const c_char, length: usize) -> c_int {
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

fn on_piece_header_value(state: &mut State, at: *const c_char, length: usize) -> c_int {
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
    // SAFETY: the value's pieces so far end at this one, inside `zdata`.
    let value = unsafe {
        std::slice::from_raw_parts(
            at.wrapping_add(length).wrapping_sub(value_length).cast(),
            value_length,
        )
    };
    let result = header_value(state, value);
    state.parse.header_name_length = 0;
    state.parse.header_value_length = 0;
    result
}

fn set_error_reason(state: &mut State, reason: &'static &'static str) {
    state.parser.set_reason(reason);
}

/// `swoole_http_token_list_contains_value(at, length, "websocket")`, with
/// its pointer-sized fast comparison.
fn contains_websocket(value: &[u8]) -> i32 {
    match token_list_fast(value, c"websocket") {
        Some(found) => i32::from(found),
        // The scan copies the value into `sw_tg_buffer()`, which it must fit.
        None if value.len() >= STACK_BUFFER_SIZE => -1,
        None => i32::from(token_list_contains(value, b"websocket")),
    }
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
    if server
        .port_of_server_fd(conn.server_fd)
        .is_some_and(|port| port.open_websocket_protocol)
    {
        conn.websocket_status = STATUS_CONNECTION;
    }
    true
}

/// `WebSocket::STATUS_CONNECTION`.
const STATUS_CONNECTION: u8 = 1;

fn header_value(state: &mut State, value_: &[u8]) -> c_int {
    let length = value_.len();
    // SAFETY: the current header's name, inside llhttp's input.
    let name =
        unsafe { std::slice::from_raw_parts(state.parse.header_name, state.parse.header_name_len) };
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
            parse_cookie(cookie, value_);
            return 0;
        }
        RequestHeader::Upgrade => {
            const MAX_LENGTH: usize = STACK_BUFFER_SIZE - 1;
            let seen = state.parse.header_value_len;
            if seen > MAX_LENGTH || length > MAX_LENGTH - seen {
                set_error_reason(state, &"Upgrade header value exceeds the maximum length");
                return USER;
            }
            state.parse.header_value_len += length;
            match contains_websocket(value_) {
                -1 => {
                    set_error_reason(state, &"Upgrade header value exceeds the maximum length");
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
            if !multipart_start(state, value_) {
                return -1;
            }
        }
        RequestHeader::Compression => compression_method(state, value_),
        RequestHeader::Append => {}
    }
    let value_ = stringl(value_);
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

fn on_headers_complete(state: &mut State) -> c_int {
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
    let method = Owned::string(ZString::new(method.as_bytes()));
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
    state.keepalive = u8::from(state.parser.should_keep_alive());
    state.recv_chunked = u8::from(state.parser.flags & CHUNKED != 0);
    state.parse.header_name = ptr::null();
    0
}

/// Swoole's `microtime()`.
/// `$request->server`'s `request_time` and `request_time_float`.
pub(crate) fn request_times(state: &mut State) {
    let now = swoole_rs_corebase::time_now();
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

fn on_body(state: &mut State, span: &[u8]) -> c_int {
    let (at, length) = (span.as_ptr(), span.len());
    if length == 0 {
        return 0;
    }
    let received = state
        .parse
        .chunked_body
        .as_ref()
        .map_or(state.request.body_length, |body| body.len());
    let beginning = received == 0;
    if state.recv_chunked != 0 {
        state
            .parse
            .chunked_body
            .get_or_insert_with(|| Box::new(Vec::with_capacity(BUFFER_SIZE_STD)))
            .extend_from_slice(span);
    } else {
        state.request.body_at = at.wrapping_sub(state.request.body_length).cast();
        state.request.body_length += length;
    }
    if state.mt_parser.is_some() && state.recv_chunked == 0 {
        let prefix = body_prefix(span, beginning);
        if !multipart_parse(state, span.get(prefix..).unwrap_or_default()) {
            return -1;
        }
    }
    if state.completed != 0 { PAUSED } else { 0 }
}

fn on_message_complete(state: &mut State) -> c_int {
    let chunked = state
        .parse
        .chunked_body
        .as_ref()
        .map(|body| (body.as_ptr(), body.len()));
    let content_length = chunked.map_or(state.request.body_length, |(_, length)| length);
    if let Some((body, length)) = chunked
        && state.mt_parser.is_some()
    {
        // The bundled multipart parser rejects calls that end inside a part header. llhttp body spans can end at
        // receive boundaries, so parse the dechunked body in one pass here instead of parsing each span.
        // SAFETY: the dechunked body, which the multipart callbacks leave alone.
        let body = unsafe { std::slice::from_raw_parts(body, length) };
        let prefix = body_prefix(body, true);
        if !multipart_parse(state, body.get(prefix..).unwrap_or_default()) {
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
            // SAFETY: the dechunked body.
            treat_data(
                PARSE_STRING,
                unsafe { std::slice::from_raw_parts(body, content_length) },
                post,
            );
        }
        (2, _) => {
            let post = property(state, Property::Post);
            let body = state.request.body_at.cast();
            // SAFETY: the body's span of the input, which is live until the request completes.
            treat_data(
                PARSE_STRING,
                unsafe { std::slice::from_raw_parts(body, state.request.body_length) },
                post,
            );
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
    state.mt_parser = None;
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

/// `Context::get_multipart_boundary` of a `multipart/form-data` content
/// type, then `Context::init_multipart_parser`; false when it has no boundary.
pub(crate) fn multipart_start(state: &mut State, content_type: &[u8]) -> bool {
    let offset = b"multipart/form-data".len();
    let boundary = swoole_rs_http::boundary(content_type, offset)
        .ok()
        .flatten()
        .and_then(|span| content_type.get(span.offset..span.offset + span.length));
    let Some(boundary) = boundary else {
        let message = format!("boundary of multipart/form-data not found, fd:{}", state.fd);
        host::log(0, 0, c"Context::get_multipart_boundary", message);
        /* make it same with protocol error */
        state.parser.error = INVALID_HEADER_TOKEN;
        return false;
    };
    state.mt_parser = Some(Box::new(Multipart {
        parser: MultipartParser::new(boundary),
        fp: ptr::null_mut(),
    }));
    state.parse.form_data = Some(Box::new(Vec::with_capacity(BUFFER_SIZE_STD)));
    true
}

/// `Context::parse_multipart_data`.
pub(crate) fn multipart_parse(state: &mut State, data: &[u8]) -> bool {
    let length = data.len();
    let Some(mut multipart) = state.mt_parser.take() else {
        return false;
    };
    let Multipart { parser, fp } = &mut *multipart;
    let parsed = parser.execute(&mut Parts { state, fp }, data);
    let mut reason = parser.error_message();
    state.mt_parser = Some(multipart);
    let func = c"Context::parse_multipart_data";
    if parsed < 0 {
        reason.truncate(STACK_BUFFER_SIZE);
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
pub(crate) fn compression_method(state: &mut State, value: &[u8]) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream truncates to uint32_t"
    )]
    let length = value.len() as u32 as usize;
    let codecs = CAPABILITIES.load(Relaxed) & CODECS;
    let method = compression(value.get(..length).unwrap_or(value), codecs);
    state.accept_compression = u8::from(method > 0);
    if method > 0 {
        #[expect(clippy::cast_possible_truncation, reason = "methods are 1 to 4")]
        let method = method as i8;
        state.compression_method = method;
    }
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
    buffer
        .get(..length)
        .map(trim_quotes)
        .unwrap_or_default()
        .into()
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

fn part_header_value(state: &mut State, value_: &[u8]) -> c_int {
    let length = value_.len();
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
        parse_cookie(fields.as_ptr(), value_.get(offset..).unwrap_or_default());
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

fn part_data(state: &mut State, upload: &mut *mut libc::FILE, data: &[u8]) -> c_int {
    if state.parse.form_data_name.is_some() {
        state
            .parse
            .form_data
            .get_or_insert_with(|| Box::new(Vec::with_capacity(BUFFER_SIZE_STD)))
            .extend_from_slice(data);
        return 0;
    }
    let fp = *upload;
    if fp.is_null() {
        return 0;
    }
    // SAFETY: an open stream and the parser's span.
    let written = unsafe { libc::fwrite(data.as_ptr().cast(), 1, data.len(), fp) };
    if written != data.len() {
        upload_long(state, b"error", UPLOAD_CANT_WRITE);
        // SAFETY: the stream, closed once.
        unsafe { libc::fclose(fp) };
        *upload = ptr::null_mut();
        host::log(
            0,
            -errno(),
            c"multipart_body_on_data",
            "write upload file failed",
        );
    }
    0
}

fn part_headers_complete(state: &mut State, upload: &mut *mut libc::FILE) -> c_int {
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
    *upload = fp;
    upload_bytes(state, b"tmp_name", &path);
    let tmpfiles = property(state, Property::TmpFiles);
    // SAFETY: the request's `tmpfiles` array and the path's bytes.
    unsafe { swr_zend_http_upload(tmpfiles, path.as_ptr().cast(), path.len()) };
    0
}

fn part_data_end(state: &mut State, upload: &mut *mut libc::FILE) -> c_int {
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
    let fp = *upload;
    if !fp.is_null() {
        upload_long(state, b"size", file_size(fp));
        // SAFETY: the stream, closed once.
        unsafe { libc::fclose(fp) };
        *upload = ptr::null_mut();
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
pub(crate) fn method_name(state: *const State) -> &'static str {
    // SAFETY: a live context.
    llhttp::method_name(unsafe { (*state).parser.method })
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
    #[cfg_attr(miri, ignore = "libc's strncasecmp is a foreign call")]
    fn form_bodies_follow_upstreams_methods() {
        // request_header takes llhttp's method numbers.
        for method in 0..=u8::MAX {
            let action = request_header(
                b"Content-Type",
                b"multipart/form-data",
                method,
                false,
                false,
            );
            let form = matches!(
                llhttp::method_name(method),
                "POST" | "PUT" | "DELETE" | "PATCH" | "QUERY"
            );
            assert_eq!(action == RequestHeader::Multipart, form, "{method}");
        }
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
