//! The HTTP/1 request parser: llhttp and multipart callbacks over
//! `swoole::http::ContextState`, filling `Swoole\Http\Request`'s `server`,
//! `header`, `get`, `post`, `cookie` and `files` as upstream's
//! `swoole_http_request.cc` did. llhttp and the multipart parser stay the
//! bundled C (third-party); what Swoole itself owns goes through `host`.
use crate::host;
use crate::state::{Slot, State};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering::Relaxed};
use swoole_rs_http::{HttpSpan, swr_v1_http_boundary};
use swoole_rs_httpbindings::{
    Text, Trimmed, UrlParts, swr_v1_httpbindings_body_prefix, swr_v1_httpbindings_compression,
    swr_v1_httpbindings_form_source, swr_v1_httpbindings_request_header,
    swr_v1_httpbindings_request_key, swr_v1_httpbindings_request_url, swr_v1_httpbindings_tokens,
    swr_v1_httpbindings_trim_quotes,
};
use swoole_rs_zend::sys::{
    IS_DOUBLE, IS_LONG, add_assoc_long_ex, add_assoc_stringl_ex, swr_zend_array_init,
    swr_zend_http_add_or_merge, swr_zend_http_estrndup, swr_zend_http_intern,
    swr_zend_http_max_input_vars, swr_zend_http_property_array, swr_zend_http_property_offset,
    swr_zend_http_upload, swr_zval_str, swr_zval_stringl, zend_class_entry, zend_hash_add_new,
    zend_hash_str_find, zend_hash_update, zend_object, zend_string, zend_value, zval,
    zval_ptr_dtor,
};
use swoole_rs_zend::{string, type_of, value};

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
/// `HPE_USER` and `F_CHUNKED`.
const INVALID_HEADER_TOKEN: i32 = 10;
const PAUSED: c_int = 21;
const PAUSED_UPGRADE: c_int = 22;
const USER: c_int = 24;
const CHUNKED: u16 = 8;
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
/// `swr_v1_httpbindings_request_header`'s actions.
const HEADER_APPEND: i32 = 0;
const HEADER_COOKIE: i32 = 1;
const HEADER_UPGRADE: i32 = 2;
const HEADER_FORM: i32 = 3;
const HEADER_MULTIPART: i32 = 4;
const HEADER_COMPRESSION: i32 = 5;
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
enum Key {
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
}

const KEY_NAMES: [&[u8]; 17] = [
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
];
/// The header keys `swr_v1_httpbindings_request_key` numbers 0 to 7.
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

static KEYS: [AtomicPtr<zend_string>; 17] = [const { AtomicPtr::new(ptr::null_mut()) }; 17];
/// `Swoole\Http\Request`'s `get`, `post`, `cookie`, `files` and `tmpfiles` slots.
static OFFSETS: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
pub(crate) static CAPABILITIES: AtomicU32 = AtomicU32::new(0);

fn key(key: Key) -> *mut zend_string {
    KEYS[key as usize].load(Relaxed)
}

/// The request's array properties, read (and created) on first use.
#[derive(Clone, Copy)]
enum Property {
    Get,
    Post,
    Cookie,
    Files,
    TmpFiles,
}

/// MINIT: intern the keys and find the array properties' slots.
/// # Safety
/// Called once at MINIT, after `Swoole\Http\Request` is registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_parse_minit(codecs: u32) {
    CAPABILITIES.store(codecs, Relaxed);
    for (slot, name) in KEYS.iter().zip(KEY_NAMES) {
        // SAFETY: MINIT, so the string is interned permanently.
        slot.store(
            unsafe { swr_zend_http_intern(name.as_ptr().cast(), name.len()) },
            Relaxed,
        );
    }
    let ce: *mut zend_class_entry = crate::request::entry();
    for (slot, name) in OFFSETS
        .iter()
        .zip(["get", "post", "cookie", "files", "tmpfiles"])
    {
        // SAFETY: the registered class, which declares the property.
        let offset = unsafe { swr_zend_http_property_offset(ce, name.as_ptr().cast(), name.len()) };
        slot.store(offset, Relaxed);
    }
}

/// `swoole_http_init_and_read_property`: the array behind a property slot,
/// creating it on first use. Like upstream, the slot keeps an uncounted copy.
fn property(state: &mut State, which: Property) -> *mut zval {
    let request = &mut state.request;
    let slot: &mut Slot = match which {
        Property::Get => &mut request.zget,
        Property::Post => &mut request.zpost,
        Property::Cookie => &mut request.zcookie,
        Property::Files => &mut request.zfiles,
        Property::TmpFiles => &mut request.ztmpfiles,
    };
    if slot.ptr.is_null() {
        // SAFETY: the request's live object zval.
        let object: *mut zend_object = unsafe { value::obj_of(request.zobject.ptr) };
        let offset = OFFSETS[which as usize].load(Relaxed);
        // SAFETY: a declared property slot of that object.
        let array = unsafe { swr_zend_http_property_array(object, offset, HT_MIN_SIZE) };
        // SAFETY: as above; the array zval just initialised.
        slot.value = unsafe { *array };
        slot.ptr = &raw mut slot.value;
    }
    slot.ptr
}

/// `http_server_add_server_array`: `$request->server[key] = value`.
fn add_server(state: &State, key_: Key, value_: &mut zval) {
    // SAFETY: the request's `server` array, created with the request.
    let table = unsafe { value::table(state.request.zserver.ptr) };
    // SAFETY: a live table, an interned key and a value it takes over.
    unsafe { zend_hash_add_new(table, key(key_), value_) };
}

fn new_zval() -> zval {
    // SAFETY: all zeroes is `IS_UNDEF`.
    unsafe { std::mem::zeroed() }
}

fn stringl(bytes: *const u8, length: usize) -> zval {
    let mut target = new_zval();
    // SAFETY: `bytes` spans `length` readable bytes, per every caller.
    unsafe { swr_zval_stringl(&raw mut target, bytes.cast(), length) };
    target
}

fn estrndup(bytes: *const u8, length: usize) -> *mut c_char {
    // SAFETY: `bytes` spans `length` readable bytes, per every caller.
    unsafe { swr_zend_http_estrndup(bytes.cast(), length) }
}

/// `swoole_php_treat_data(arg, estrndup(bytes), array)`, which frees the copy.
fn treat_data(arg: c_int, bytes: *const u8, length: usize, array: *mut zval) {
    // SAFETY: a fresh request-memory string and a live array.
    unsafe { host::swr_http_host_treat_data(arg, estrndup(bytes, length), array) };
}

/// `swoole_http_parse_cookie`.
fn parse_cookie(array: *mut zval, bytes: *const u8, length: usize) {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_parse(
    context: *mut c_void,
    data: *const c_char,
    length: usize,
) -> usize {
    execute(context.cast(), &SERVER, data, length)
}

/// `Request::parse()`: append to `request.zdata` and parse what was appended.
pub(crate) fn parse_piece(state: *mut State, data: &[u8]) -> usize {
    // SAFETY: a live context.
    let zdata = unsafe { &raw mut (*state).request.zdata };
    // SAFETY: the request's data zval.
    if unsafe { type_of(zdata) } == swoole_rs_zend::sys::IS_STRING {
        // SAFETY: a string zval.
        let old = unsafe { value::str_of(zdata) };
        // SAFETY: as above.
        let old_bytes = unsafe { string::bytes(old) };
        let joined = string::alloc(old_bytes.len() + data.len());
        let target = string::val(joined);
        // SAFETY: a fresh string with room for both.
        unsafe { ptr::copy_nonoverlapping(old_bytes.as_ptr(), target, old_bytes.len()) };
        // SAFETY: as above.
        unsafe {
            ptr::copy_nonoverlapping(
                data.as_ptr(),
                target.wrapping_add(old_bytes.len()),
                data.len(),
            );
        };
        // SAFETY: as above.
        unsafe { string::truncate(joined, old_bytes.len() + data.len()) };
        // SAFETY: `zval_dtor` of the string the zval held.
        unsafe { zval_ptr_dtor(zdata) };
        // SAFETY: the zval was just released.
        unsafe { swr_zval_str(zdata, joined) };
    } else {
        // SAFETY: an empty zval and the argument's bytes.
        unsafe { swr_zval_stringl(zdata, data.as_ptr().cast(), data.len()) };
    }
    let (start, length) = zdata_span(state);
    execute(
        state,
        &PIECES,
        start.wrapping_add(length - data.len()).cast(),
        data.len(),
    )
}

/// `request.zdata`'s bytes.
fn zdata_span(state: *const State) -> (*const u8, usize) {
    // SAFETY: a live context whose data zval holds a string.
    let zdata = unsafe { &raw const (*state).request.zdata };
    // SAFETY: as above.
    let string = unsafe { value::str_of(zdata) };
    // SAFETY: as above.
    let bytes = unsafe { string::bytes(string) };
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
    let mut tmp = stringl(query, parts.query_length);
    add_server(state, Key::QueryString, &mut tmp);
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
    header_field(unsafe { state(parser) }, at.cast(), length)
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
    header_value(unsafe { state(parser) }, at.cast(), length)
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

fn header_value(state: &mut State, at: *const u8, length: usize) -> c_int {
    let name = Text {
        data: state.parse.header_name,
        length: state.parse.header_name_len,
    };
    let mut flags = u32::from(state.parse_cookie != 0);
    if CAPABILITIES.load(Relaxed) & COMPRESSION != 0 && state.enable_compression != 0 {
        flags |= 2;
    }
    let method = u32::from(state.parser.method);
    // SAFETY: the current header's name and llhttp's value span.
    let action = unsafe {
        swr_v1_httpbindings_request_header(name, Text { data: at, length }, method, flags)
    };
    match action {
        HEADER_COOKIE => {
            let cookie = property(state, Property::Cookie);
            parse_cookie(cookie, at, length);
            return 0;
        }
        HEADER_UPGRADE => {
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
                    // SAFETY: the live context.
                    if unsafe {
                        host::swr_http_host_websocket_connection(ptr::from_mut(state).cast())
                    } < 0
                    {
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
        HEADER_FORM => state.request.post_form_urlencoded = 1,
        HEADER_MULTIPART => {
            let Some((boundary, boundary_len)) =
                multipart_boundary(state, at, length, b"multipart/form-data".len())
            else {
                return -1;
            };
            multipart_init(state, boundary, boundary_len);
        }
        HEADER_COMPRESSION if CAPABILITIES.load(Relaxed) & COMPRESSION != 0 => {
            compression_method(state, at, length);
        }
        HEADER_APPEND => {}
        _ => return -1,
    }
    let mut tmp = stringl(at, length);
    // The request's `header` array, created with the request.
    let header = state.request.zheader.ptr;
    // SAFETY: the current header's name.
    let index = unsafe { swr_v1_httpbindings_request_key(name) };
    if let Some(&known) = usize::try_from(index)
        .ok()
        .and_then(|index| HEADER_KEYS.get(index))
    {
        // SAFETY: a live array zval.
        let table = unsafe { value::table(header) };
        // SAFETY: a live table, an interned key and a value it takes over.
        unsafe { zend_hash_update(table, key(known), &raw mut tmp) };
    } else {
        // SAFETY: the current header's name, inside llhttp's input.
        let name = unsafe { std::slice::from_raw_parts(name.data, name.length) };
        let lower = name.to_ascii_lowercase();
        // SAFETY: a live array zval and a value it takes over.
        unsafe {
            swr_zend_http_add_or_merge(header, lower.as_ptr().cast(), lower.len(), &raw mut tmp);
        };
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
    let mut tmp = stringl(method.as_ptr().cast(), method.count_bytes());
    add_server(state, Key::RequestMethod, &mut tmp);
    let mut tmp = stringl(path.as_ptr(), path.len());
    add_server(state, Key::RequestUri, &mut tmp);

    // path_info should be decoded
    let decoded = string::copy(path);
    // SAFETY: a fresh string of `path.len()` bytes, decoded in place.
    let decoded_len = unsafe { php_url_decode(string::val(decoded).cast(), path.len()) };
    // SAFETY: the decoded bytes fit the string.
    unsafe { (*decoded).len = decoded_len };
    let mut tmp = new_zval();
    // SAFETY: the zval takes over the fresh string.
    unsafe { swr_zval_str(&raw mut tmp, decoded) };
    add_server(state, Key::PathInfo, &mut tmp);

    // SAFETY: `time(NULL)`.
    let now = unsafe { libc::time(ptr::null_mut()) };
    let mut tmp = new_zval();
    // SAFETY: an empty zval.
    unsafe { value::set_long(&raw mut tmp, now) };
    add_server(state, Key::RequestTime, &mut tmp);
    let mut tmp = new_zval();
    tmp.value = zend_value { dval: microtime() };
    tmp.u1.type_info = IS_DOUBLE;
    add_server(state, Key::RequestTimeFloat, &mut tmp);
    let protocol = key(if state.request.version == 101 {
        Key::Http11
    } else {
        Key::Http10
    });
    let mut tmp = new_zval();
    // SAFETY: an interned string, which ZVAL_STR does not count.
    unsafe { swr_zval_str(&raw mut tmp, protocol) };
    add_server(state, Key::ServerProtocol, &mut tmp);

    // SAFETY: the context's parser.
    state.keepalive = u8::from(unsafe { llhttp_should_keep_alive(&raw const state.parser) } != 0);
    state.recv_chunked = u8::from(state.parser.flags & CHUNKED != 0);
    state.parse.header_name = ptr::null();
    0
}

/// Swoole's `microtime()`.
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_parse_free(context: *mut c_void) {
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
    let mut span = HttpSpan {
        offset: 0,
        length: 0,
    };
    // SAFETY: the header value's span and a writable result.
    if unsafe { swr_v1_http_boundary(at, length, offset, &raw mut span) } != 1 {
        let message = format!("boundary of multipart/form-data not found, fd:{}", state.fd);
        host::log(0, 0, c"Context::get_multipart_boundary", message);
        /* make it same with protocol error */
        state.parser.error = INVALID_HEADER_TOKEN;
        return None;
    }
    Some((at.wrapping_add(span.offset), span.length))
}

/// `Context::get_multipart_boundary`, for the HTTP/2 server.
/// # Safety
/// `context` is a live context; `at` spans `length` bytes; the outputs are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_multipart_boundary(
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_multipart_init(
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
fn multipart_parse(state: &mut State, at: *const u8, length: usize) -> bool {
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

/// `Context::parse_multipart_data`, for the HTTP/2 server.
/// # Safety
/// `context` is a live context with a multipart parser; `at` spans `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_multipart_parse(
    context: *mut c_void,
    at: *const c_char,
    length: usize,
) -> bool {
    // SAFETY: per the caller.
    multipart_parse(unsafe { &mut *context.cast::<State>() }, at.cast(), length)
}

/// `Context::set_compression_method`.
fn compression_method(state: &mut State, at: *const u8, length: usize) {
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

/// `Context::set_compression_method`, for the HTTP/2 server.
/// # Safety
/// `context` is a live context and `at` spans `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_compression_method(
    context: *mut c_void,
    at: *const c_char,
    length: usize,
) {
    // SAFETY: per the caller.
    compression_method(unsafe { &mut *context.cast::<State>() }, at.cast(), length);
}

unsafe extern "C" fn on_part_header_field(
    parser: *mut Multipart,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the multipart parser calls back with the context's parser.
    header_field(unsafe { part_state(parser) }, at.cast(), length)
}

/// `zend_hash_str_find(Z_ARRVAL_P(array), key)`.
pub(crate) fn find(array: *mut zval, key: &[u8]) -> *mut zval {
    // SAFETY: a live array zval.
    let table = unsafe { value::table(array) };
    // SAFETY: a live table.
    unsafe { zend_hash_str_find(table, key.as_ptr().cast(), key.len()) }
}

/// `Z_TYPE_P(zv) == IS_LONG ? Z_LVAL_P(zv) : None`.
fn long(found: *mut zval) -> Option<i64> {
    // SAFETY: a live zval.
    (!found.is_null() && unsafe { type_of(found) } == IS_LONG).then(|| {
        // SAFETY: an int zval.
        unsafe { value::long_of(found) }
    })
}

fn add_long(array: *mut zval, key: &CStr, value_: i64) {
    // SAFETY: a live array zval.
    unsafe { add_assoc_long_ex(array, key.as_ptr(), key.count_bytes(), value_) };
}

fn add_bytes(array: *mut zval, key: &CStr, bytes: &[u8]) {
    // SAFETY: a live array zval; Zend copies the bytes.
    unsafe {
        add_assoc_stringl_ex(
            array,
            key.as_ptr(),
            key.count_bytes(),
            bytes.as_ptr().cast(),
            bytes.len(),
        );
    };
}

/// A string zval's bytes up to its first NUL (`%s`, `strlen`), and its length.
fn string_value<'a>(found: *mut zval) -> Option<(&'a [u8], usize)> {
    // SAFETY: a live zval from a live array.
    let bytes = unsafe { swoole_rs_zend::string_of(found) }?;
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    Some((&bytes[..end], bytes.len()))
}

/// `swoole_strlcpy` into a `SW_HTTP_FORM_KEYLEN` buffer, then
/// `http_trim_double_quote` over `length` bytes of it.
fn trimmed(text: &[u8], length: usize) -> Box<[u8]> {
    let mut buffer = [0u8; FORM_KEYLEN];
    let copied = text.len().min(FORM_KEYLEN - 1);
    buffer[..copied].copy_from_slice(&text[..copied]);
    let mut result = Trimmed::default();
    // SAFETY: the buffer holds `length` (< FORM_KEYLEN) writable bytes.
    if unsafe { swr_v1_httpbindings_trim_quotes(buffer.as_mut_ptr(), length, &raw mut result) } != 0
    {
        result = Trimmed { offset: 0, length };
    }
    buffer[result.offset..result.offset + result.length].into()
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
        let mut fields = new_zval();
        // SAFETY: an empty zval.
        unsafe { swr_zend_array_init(&raw mut fields) };
        parse_cookie(&raw mut fields, value_[offset..].as_ptr(), length - offset);

        let Some((form_name, form_name_len)) = string_value(find(&raw mut fields, b"name")) else {
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
        let filename = find(&raw mut fields, b"filename");
        if filename.is_null() {
            // POST form data
            state.parse.form_data_name = Some(c_copy(&form_name));
        } else {
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
            let mut header = new_zval();
            // SAFETY: an empty zval.
            unsafe { swr_zend_array_init(&raw mut header) };
            if state.parse.tmp_content_type.is_null() {
                add_bytes(&raw mut header, c"type", b"");
            } else {
                // SAFETY: a span of the parser's input, still live.
                let content_type = unsafe {
                    std::slice::from_raw_parts(
                        state.parse.tmp_content_type,
                        state.parse.tmp_content_type_len,
                    )
                };
                add_bytes(&raw mut header, c"type", content_type);
                state.parse.tmp_content_type = ptr::null();
            }
            add_bytes(&raw mut header, c"tmp_name", b"");
            add_long(&raw mut header, c"size", 0);
            let filename = trimmed(filename, filename_len);
            add_bytes(&raw mut header, c"name", &filename);
            add_long(
                &raw mut header,
                c"error",
                if filename.is_empty() {
                    UPLOAD_NO_FILE
                } else {
                    UPLOAD_OK
                },
            );
            state.parse.multipart_header = header;
        }
        // SAFETY: the array this function made.
        unsafe { zval_ptr_dtor(&raw mut fields) };
    } else if name.eq_ignore_ascii_case(b"content-type") {
        if has_header(state) {
            let header = &raw mut state.parse.multipart_header;
            if long(find(header, b"error")) == Some(UPLOAD_OK) {
                add_bytes(header, c"type", value_);
            }
        } else {
            state.parse.tmp_content_type = value_.as_ptr();
            state.parse.tmp_content_type_len = length;
        }
    } else if name.eq_ignore_ascii_case(b"swoole-upload-file") {
        // When the "SW_HTTP_UPLOAD_FILE" header appears in the request, it indicates that the uploaded file has been
        // saved in a temporary file. The binary content in the message body will be replaced with the temporary
        // filename. However, the Content-Length still reflects the original message size, causing llhttp to believe
        // there is still data to be received. As a result, llhttp fails to trigger the message callback. Therefore, we
        // need to set `ctx->completed = 1` to indicate that the message processing is complete.
        state.completed = 1;
        let header = &raw mut state.parse.multipart_header;
        add_bytes(header, c"tmp_name", value_);
        add_long(header, c"error", UPLOAD_FILE_READY);
        add_long(header, c"size", path_size(value_));
        let tmpfiles = property(state, Property::TmpFiles);
        // SAFETY: the request's `tmpfiles` array and the path's bytes.
        unsafe { swr_zend_http_upload(tmpfiles, value_.as_ptr().cast(), value_.len()) };
    }
    0
}

fn has_header(state: &State) -> bool {
    // SAFETY: an initialised (possibly undef) zval.
    (unsafe { type_of(&raw const state.parse.multipart_header) }) != 0
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
        add_long(
            &raw mut state.parse.multipart_header,
            c"error",
            UPLOAD_CANT_WRITE,
        );
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
    let header = &raw mut state.parse.multipart_header;
    let error = find(header, b"error");
    if error.is_null() || long(error).is_some_and(|code| code != UPLOAD_OK) {
        return 0;
    }
    // SAFETY: the live context.
    let directory = unsafe { host::swr_http_host_upload_tmp_dir(ptr::from_mut(state).cast()) };
    // SAFETY: the context's `std::string`, NUL-terminated.
    let directory = unsafe { CStr::from_ptr(directory) };
    let mut path = directory.to_bytes().to_vec();
    path.extend_from_slice(b"/swoole.upfile.XXXXXX");
    path.truncate(UPLOAD_TMPDIR_SIZE - 1);
    path.push(0);
    // SAFETY: a writable, NUL-terminated template.
    let fd = unsafe { host::swr_http_host_tmpfile(path.as_mut_ptr().cast()) };
    path.pop();
    if fd < 0 {
        add_long(header, c"error", UPLOAD_NO_TMP_DIR);
        return 0;
    }
    // SAFETY: a descriptor this function owns.
    let fp = unsafe { libc::fdopen(fd, c"wb+".as_ptr()) };
    if fp.is_null() {
        add_long(header, c"error", UPLOAD_NO_TMP_DIR);
        let mut message = b"fopen(".to_vec();
        message.extend_from_slice(&path);
        message.push(b')');
        message.extend_from_slice(b" failed");
        host::log(0, -errno(), c"multipart_body_on_header_complete", message);
        return 0;
    }
    // SAFETY: the live multipart parser.
    unsafe { (*parser).fp = fp };
    add_bytes(header, c"tmp_name", &path);
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
    let header = &raw mut state.parse.multipart_header;
    // SAFETY: the live multipart parser.
    let fp = unsafe { (*parser).fp };
    if !fp.is_null() {
        add_long(header, c"size", file_size(fp));
        // SAFETY: the stream, closed once.
        unsafe { libc::fclose(fp) };
        // SAFETY: the live multipart parser.
        unsafe { (*parser).fp = ptr::null_mut() };
    }
    let error = find(header, b"error");
    if error.is_null() {
        state.parse.input_name = Some(input_name);
        return 0;
    }
    // SAFETY: a live zval.
    if unsafe { swoole_rs_zend::to_long(error) } == UPLOAD_FILE_READY {
        add_long(header, c"error", UPLOAD_OK);
    }
    let files = property(state, Property::Files);
    let header = &raw mut state.parse.multipart_header;
    // `input_name` holds its NUL; C sees it up to the first one.
    let text = &input_name[..input_name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(input_name.len())];
    match text.iter().position(|&byte| byte == b'[') {
        Some(at) if state.parse_files != 0 && at > 0 => {
            const META: usize = FORM_KEYLEN + b"[tmp_name]".len() - 1;
            let input_path = &text[at..];
            for field in ["name", "type", "tmp_name", "error", "size"] {
                let mut meta = text[..at.min(META - 1)].to_vec();
                meta.extend_from_slice(format!("[{field}]").as_bytes());
                meta.extend_from_slice(input_path);
                meta.truncate(META - 1);
                meta.push(0);
                let entry = find(header, field.as_bytes());
                // SAFETY: a NUL-terminated name, a value from the upload's array and `$request->files`.
                unsafe { php_register_variable_ex(meta.as_ptr().cast(), entry, files) };
            }
        }
        _ => {
            // SAFETY: a NUL-terminated name, the upload's array and `$request->files`.
            unsafe { php_register_variable_ex(input_name.as_ptr().cast(), header, files) };
        }
    }
    // Upstream frees the zval holder, not the array it registered.
    state.parse.multipart_header = new_zval();
    0
}

/// `getContent()`: the body received so far.
pub(crate) fn body(state: *mut State) -> Option<&'static [u8]> {
    // SAFETY: a live context.
    let state_ref = unsafe { &*state };
    if state_ref.request.body_length > 0 {
        let (start, total) = zdata_span(state);
        let length = state_ref.request.body_length;
        // SAFETY: the body ends the request's data.
        return Some(unsafe {
            std::slice::from_raw_parts(start.wrapping_add(total - length), length)
        });
    }
    if let Some(chunked) = state_ref
        .parse
        .chunked_body
        .as_ref()
        .filter(|body| !body.is_empty())
    {
        // SAFETY: the context owns the buffer while the request object lives.
        return Some(unsafe { std::slice::from_raw_parts(chunked.as_ptr(), chunked.len()) });
    }
    let mut length = 0;
    // SAFETY: a live context and a writable length.
    let data = unsafe { host::swr_http_host_http2_data(state.cast(), &raw mut length) };
    // SAFETY: the HTTP/2 data buffer holds `length` bytes.
    (!data.is_null()).then(|| unsafe { std::slice::from_raw_parts(data.cast(), length) })
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
        assert_eq!(KEY_NAMES[HEADER_KEYS[7] as usize], b"accept-encoding");
    }
}
