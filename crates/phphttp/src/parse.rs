//! The HTTP/1 request parser: llhttp and multipart callbacks over
//! `swoole::http::ContextState`, filling `Swoole\Http\Request`'s `server`,
//! `header`, `get`, `post`, `cookie` and `files` as upstream's
//! `swoole_http_request.cc` did. Both parsers are crates/http's; what Swoole
//! itself owns goes through `host`.
use crate::host;
use crate::state::{Request, State};
use std::ffi::{CStr, CString, c_char, c_int};
use std::fs::File;
use std::io::{BufWriter, IoSlice, Write};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::buffer::{BoundedBytes, string::find_delimiter};
use swoole_rs_corebase::lent::{Count, In, lend_in};
use swoole_rs_corebase::log;
use swoole_rs_corebase::sys::Text;
use swoole_rs_http::llhttp::{self, Callbacks, Flow as Step, Parser};
use swoole_rs_http::multipart_parser::{Flow, MultipartHeader, MultipartParser};
use swoole_rs_httpbindings::{
    FormSource, RequestHeader, body_prefix, compression, form_source, request_header, request_url,
    token_list_contains, token_list_fast, trim_quotes,
};
use swoole_rs_zend::{
    ArrayMut, Declared, HeldArray, Insertions, Interned, Minit, Owned, OwnedArray, Separated,
    Value, ZStr, ZString, bailout, contain_with,
};

/// A request's multipart parser and the upload it is writing.
pub(crate) struct Multipart {
    parser: MultipartParser,
    header: MultipartHeader,
    content_type: Option<Vec<u8>>,
    field: BoundedBytes,
    fp: Option<Upload>,
}

/// The upload file being written, buffered as upstream's `fdopen` stream was.
type Upload = BufWriter<File>;

struct Parts<'a> {
    state: &'a mut State,
    fp: &'a mut Option<Upload>,
    field: &'a mut BoundedBytes,
    header: &'a mut MultipartHeader,
    content_type: &'a mut Option<Vec<u8>>,
}

unsafe extern "C" {
    /// `$track[var] = (string) val`, by PHP's rules for request variable
    /// names: it copies the name and the lent bytes, and registers nothing
    /// without an array. Its nesting warning runs the error handler, which may
    /// bail out: call it inside `contain_with`.
    safe fn php_register_variable_safe(
        var: Text<'_>,
        val: In<'_, u8>,
        len: Count<'_>,
        track: Option<Separated<'_>>,
    );
}

use llhttp::{
    F_CHUNKED as CHUNKED, F_TRAILING as TRAILING, HPE_INVALID_HEADER_TOKEN as INVALID_HEADER_TOKEN,
    HPE_PAUSED as PAUSED, HPE_PAUSED_UPGRADE as PAUSED_UPGRADE, HPE_USER as USER,
};
use swoole_rs_corebase::log::LogLevel::{SW_LOG_NOTICE as LOG_NOTICE, SW_LOG_TRACE as LOG_TRACE};
/// `PARSE_COOKIE` and `PARSE_STRING` (`main/php_variables.h`).
const PARSE_COOKIE: c_int = 2;
const PARSE_STRING: c_int = 3;
/// `SW_LOG_TRACE`, `SW_LOG_NOTICE`, `SW_ERROR_SESSION_CLOSED` and
/// `SW_ERROR_SERVER_INVALID_REQUEST` (asserted in ext-src).
const SESSION_CLOSED: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SESSION_CLOSED as c_int;
const SERVER_INVALID_REQUEST: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SERVER_INVALID_REQUEST as c_int;
/// `SW_STACK_BUFFER_SIZE` (also `sw_tg_buffer()`'s size), `SW_BUFFER_SIZE_STD`,
/// `SW_HTTP_FORM_KEYLEN`, `HT_MIN_SIZE`.
const STACK_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize;
const BUFFER_SIZE_STD: usize = swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize;
const FORM_KEYLEN: usize = swoole_rs_corebase::constants::SW_HTTP_FORM_KEYLEN as usize;
const HT_MIN_SIZE: u32 = 8;
/// `http_upload_errno`.
const UPLOAD_OK: i64 = 0;
const UPLOAD_NO_FILE: i64 = 4;
const UPLOAD_NO_TMP_DIR: i64 = 6;
const UPLOAD_CANT_WRITE: i64 = 7;
const UPLOAD_FILE_READY: i64 = 8;

/// The server's llhttp callbacks, over a context's state, with the current
/// header's name, a span of the call's input, and the input, which holds the
/// whole body.
struct Server<'a, 'i>(&'a mut State, &'i [u8], &'i [u8], usize);

/// `Request::parse()`'s: input arrives in pieces appended to `request.zdata`,
/// whose bytes so far hold the whole body.
struct Pieces<'a, 'b>(&'a mut State, &'b [u8], usize);

impl<'i> Callbacks<'i> for Server<'_, 'i> {
    fn parser(&mut self) -> &mut Parser {
        &mut self.0.parser
    }

    fn on_url(&mut self, data: &[u8]) -> Step {
        url(self.0, data)
    }

    fn on_header_field(&mut self, data: &'i [u8]) -> Step {
        // A request's trailer fields are ignored.
        if self.0.parser.flags & TRAILING != 0 {
            return Step::Go;
        }
        self.1 = data;
        self.0.parse.header_value_len = 0;
        Step::Go
    }

    fn on_header_value(&mut self, data: &[u8]) -> Step {
        if self.0.parser.flags & TRAILING != 0 {
            return Step::Go;
        }
        header_value(self.0, self.1, data, self.3)
    }

    fn on_headers_complete(&mut self) -> Step {
        on_headers_complete(self.0)
    }

    fn on_body(&mut self, data: &[u8]) -> Step {
        on_body(self.0, data, self.2, self.3)
    }

    fn on_message_complete(&mut self) -> Step {
        on_message_complete(self.0, self.2)
    }
}

impl Callbacks<'_> for Pieces<'_, '_> {
    fn parser(&mut self) -> &mut Parser {
        &mut self.0.parser
    }

    fn on_url(&mut self, data: &[u8]) -> Step {
        on_piece_url(self.0, data.as_ptr().cast(), data.len())
    }

    fn on_header_field(&mut self, data: &[u8]) -> Step {
        on_piece_header_field(self.0, data.as_ptr().cast(), data.len())
    }

    fn on_header_value(&mut self, data: &[u8]) -> Step {
        on_piece_header_value(self.0, data.as_ptr().cast(), data.len(), self.2)
    }

    fn on_headers_complete(&mut self) -> Step {
        on_headers_complete(self.0)
    }

    fn on_body(&mut self, data: &[u8]) -> Step {
        on_body(self.0, data, self.1, self.2)
    }

    fn on_message_complete(&mut self) -> Step {
        on_message_complete(self.0, self.1)
    }
}

impl<'i> swoole_rs_http::multipart_parser::Parts<'i> for Parts<'_> {
    fn on_header_field(&mut self, data: &'i [u8]) -> Flow {
        self.state.parse.header_value_len = 0;
        self.header.append_name(data)
    }

    fn on_header_value(&mut self, data: &'i [u8]) -> Flow {
        self.header.append_value(data)
    }

    fn on_header_value_complete(&mut self) -> Flow {
        let (name, value) = self.header.parts();
        let ok = part_header_value(self.state, name, value, self.content_type);
        self.header.clear();
        if ok { Flow::Go } else { Flow::Fail }
    }

    fn on_part_data(&mut self, data: &[u8]) -> Flow {
        if self.state.parse.form_data_name.is_some() {
            return if self.field.append(data).is_ok() {
                Flow::Go
            } else {
                Flow::Fail
            };
        }
        part_data(self.state, self.fp, data);
        Flow::Go
    }

    fn on_headers_complete(&mut self) -> Flow {
        part_headers_complete(self.state, self.fp);
        Flow::Go
    }

    fn on_part_data_end(&mut self) -> Flow {
        *self.content_type = None;
        part_data_end(self.state, self.fp, self.field);
        Flow::Go
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

static KEYS: [Interned; 25] = [const { Interned::new() }; 25];
static METHODS: [Interned; llhttp::METHODS.len()] =
    [const { Interned::new() }; llhttp::METHODS.len()];
/// `Swoole\Http\Request`'s `get`, `post`, `cookie`, `files` and `tmpfiles` slots.
/// ... and `server` and `header`.
static PROPERTIES: [Declared; 7] = [const { Declared::new() }; 7];
/// `Swoole\Http\Request`'s declared `fd` slot.
pub(crate) static REQUEST_FD: Declared = Declared::new();
static CODECS: AtomicU32 = AtomicU32::new(0);

/// Codecs available in this build, as reported by MINIT.
pub(crate) fn enabled_codecs() -> u32 {
    CODECS.load(Relaxed)
}

/// The interned key, None before MINIT.
pub(crate) fn key(key: Key) -> Option<&'static ZStr> {
    KEYS.get(key as usize)?.get()
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
pub(crate) fn minit(minit: &Minit, codecs: u32) {
    CODECS.store(codecs, Relaxed);
    for (slot, name) in KEYS.iter().zip(KEY_NAMES) {
        minit.intern(slot, name);
    }
    for (slot, name) in METHODS.iter().zip(llhttp::METHODS) {
        minit.intern(slot, name.as_bytes());
    }
    for (property, name) in PROPERTIES.iter().zip([
        c"get",
        c"post",
        c"cookie",
        c"files",
        c"tmpfiles",
        c"server",
        c"header",
    ]) {
        property.resolve(&crate::request::REQUEST, name);
    }
    REQUEST_FD.resolve(&crate::request::REQUEST, c"fd");
}

/// Unlink the uploads `request`'s `tmpfiles` property lists now (user code may
/// have replaced the array), and forget them in `SG(rfc1867_uploaded_files)`.
pub(crate) fn release_uploads(properties: swoole_rs_zend::FreeProperties<'_>) {
    if let Some(tmpfiles) = PROPERTIES.get(Property::TmpFiles as usize) {
        properties.release_uploads(tmpfiles);
    }
}

/// `swoole_http_init_and_read_property`: the request object's `which`
/// property slot, its array made on first use.
#[inline]
fn property_owner(request: &mut Request, which: Property) -> Option<(&Declared, &Value)> {
    let property = PROPERTIES.get(which as usize)?;
    let size = if matches!(which, Property::Server) {
        HT_MIN_SIZE << 1
    } else {
        HT_MIN_SIZE
    };
    let object =
        request
            .zobject
            .array_owner(&mut request.arrays, which as usize, property, size)?;
    Some((property, object))
}

#[inline]
pub(crate) fn init_property(request: &mut Request, which: Property) -> Option<()> {
    property_owner(request, which).map(|_| ())
}

/// Hold the request object's `which` array, once [`init_property`] has made
/// it. A property PHP replaced with a scalar has no array to hold.
#[inline]
pub(crate) fn made(request: &Request, which: Property) -> Option<HeldArray> {
    if request.arrays & (1 << which as u8) == 0 {
        return None;
    }
    PROPERTIES.get(which as usize)?.hold(request.zobject.get()?)
}

/// [`init_property`]'s array, lent to `edit`; None without one.
#[inline(always)]
pub(crate) fn edit<R>(
    request: &mut Request,
    which: Property,
    edit: impl FnOnce(&mut ArrayMut) -> R,
) -> Option<R> {
    let (property, object) = property_owner(request, which)?;
    property.edit_array(object, edit)
}

/// New server keys share one separation as well as the property loan.
#[inline(always)]
pub(crate) fn edit_server(state: &mut State, edit: impl FnOnce(&mut Insertions<'_>)) {
    self::edit(&mut state.request, Property::Server, |server| {
        if let Some(mut server) = server.separate().and_then(Separated::insertions) {
            edit(&mut server);
        }
    });
}

/// `http_server_add_server_array`: `$request->server[key] = value`.
#[inline(always)]
pub(crate) fn add_server(state: &mut State, name: Key, value: Owned) {
    edit_server(state, |server| {
        add_field(server, name, value);
    });
}

/// One known key in a batch whose server array is already lent.
#[inline(always)]
pub(crate) fn add_field(server: &mut Insertions<'_>, name: Key, value: Owned) {
    if let Some(key) = key(name) {
        server.add_new(key, value);
    }
}

/// A new string of the bytes.
fn stringl(bytes: &[u8]) -> Owned {
    Owned::string(ZString::new(bytes))
}

unsafe extern "C" {
    /// `estrndup`: it reads the lent bytes (an `IoSlice` is an iovec) and returns
    /// an emalloc'd NUL-terminated copy.
    safe fn swr_bytes_estrndup(bytes: IoSlice<'_>) -> *mut c_char;
}

/// The bytes, copied into request memory.
pub(crate) fn estrndup(bytes: &[u8]) -> *mut c_char {
    swr_bytes_estrndup(IoSlice::new(bytes))
}

/// The request's path: `on_url`'s (or the HTTP/2 `:path`'s) copy up to its
/// query, empty before one.
pub(crate) fn request_path(request: &crate::state::Request) -> &[u8] {
    request.path.as_deref().unwrap_or_default()
}

/// Bytes copied into request memory (`estrndup`) for `swoole_php_treat_data`,
/// which frees them. Copied first, they leave nothing borrowed or owned here
/// while PHP parses them, which may bail out.
///
/// Invariant: a NUL-terminated string `estrndup` returned, which nothing else
/// holds, or null, which both calls read as no text. Only [`Copied::of`] makes
/// one, and the call it is moved into frees it.
#[repr(transparent)]
pub(crate) struct Copied(*mut c_char);

impl Copied {
    /// The bytes, or null once a bailout is pending: upstream never parsed on.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(if bailout::pending() {
            std::ptr::null_mut()
        } else {
            estrndup(bytes)
        })
    }
}

// Their warnings (`max_input_vars`, the nesting level) run the error handler,
// which may bail out: each is called inside `contain_with`.
unsafe extern "C" {
    /// `swoole_php_treat_data`: the pairs of the string, which it frees, into the
    /// array the zval holds, which it does not replace; false once it stopped at
    /// `max_input_vars`.
    safe fn swr_zend_http_treat_data(arg: c_int, text: Copied, array: Separated<'_>) -> bool;
    /// `sapi_module.treat_data(PARSE_STRING, text, array)`, which frees the string.
    safe fn swr_zend_treat_string(text: Copied, array: Separated<'_>);
}

/// `sapi_module.treat_data(PARSE_STRING, text, array)`; a bailout is deferred.
pub(crate) fn treat_string(text: Copied, array: Separated<'_>) {
    bailout::defer(contain_with(|| swr_zend_treat_string(text, array)));
}

/// `swoole_php_treat_data(arg, text, array)`; nothing without the array, and a
/// bailout is deferred.
fn treat_data(arg: c_int, text: Copied, array: Option<Separated<'_>>) {
    let Some(array) = array else { return };
    let mut within = true;
    bailout::defer(contain_with(|| {
        within = swr_zend_http_treat_data(arg, text, array);
    }));
    if !within {
        let limit = max_input_vars();
        log::warning(
            c"swoole_php_treat_data",
            format_args!(
                "Input variables exceeded {limit}. To increase the limit change max_input_vars in php.ini."
            ),
        );
    }
}

unsafe extern "C" {
    /// `PG(max_input_vars)`: one INI value of this thread's PHP is read.
    #[link_name = "swr_zend_http_max_input_vars"]
    safe fn max_input_vars() -> i64;
}

unsafe extern "C" {
    /// `$request->tmpfiles[] = path` into the array the zval holds, which it does
    /// not replace, and the upload `is_uploaded_file` knows; it copies the bytes.
    safe fn swr_zend_http_upload(tmpfiles: Separated<'_>, path: IoSlice<'_>);
}

/// `$request->tmpfiles[] = path`, and the upload `is_uploaded_file` knows;
/// nothing without the array.
fn add_upload(state: &mut State, path: &[u8]) -> bool {
    edit(&mut state.request, Property::TmpFiles, |array| {
        array
            .separate()
            .map(|array| swr_zend_http_upload(array, IoSlice::new(path)))
    })
    .flatten()
    .is_some()
}

/// `swoole_llhttp_parser_init(&ctx->parser, HTTP_REQUEST, ctx)`.
pub(crate) fn init_request_parser(mut ctx: crate::context::Ctx) {
    ctx.state().parser = Parser::new(llhttp::Kind::Request);
}

/// `swoole_http_parse_cookie`.
pub(crate) fn parse_cookie(array: Option<Separated<'_>>, bytes: &[u8]) {
    if !bytes.is_empty() {
        treat_data(PARSE_COOKIE, Copied::of(bytes), array);
    }
}

/// `swoole_llhttp_parser_execute` of `input`, the end of `pieces` for
/// `Request::parse()`.
fn execute(state: &mut State, pieces: Option<&[u8]>, input: &[u8], limit: usize) -> usize {
    let result = match pieces {
        Some(bytes) => llhttp::execute(&mut Pieces(state, bytes, limit), input),
        None => llhttp::execute(&mut Server(state, &[], input, limit), input),
    };
    let Err(error) = result else {
        return input.len();
    };
    let parser = &mut state.parser;
    match error {
        PAUSED => parser.resume(),
        PAUSED_UPGRADE => parser.resume_after_upgrade(),
        _ => {}
    }
    parser.error_pos()
}

/// `Context::parse`: the bytes parsed, by the context's initialised parser.
pub(crate) fn parse_request(state: &mut State, data: &[u8], limit: usize) -> usize {
    execute(state, None, data, limit)
}

/// `Request::parse()`: append to `request.zdata` and parse what was appended.
pub(crate) fn parse_piece(state: &mut State, data: &[u8]) -> Option<usize> {
    let zdata = &mut state.request.zdata;
    let old = zdata.bytes().unwrap_or_default();
    let limit = state
        .parse
        .package_max_length
        .unwrap_or(swoole_rs_corebase::constants::SW_INPUT_BUFFER_SIZE) as usize;
    let length = old
        .len()
        .checked_add(data.len())
        .filter(|&len| len <= limit)?;
    let mut joined = swoole_rs_zend::string::ZBuf::with_capacity(length);
    joined.extend_from_slice(old);
    joined.extend_from_slice(data);
    *zdata = Owned::string(joined.into_zstring());
    // A reference of its own to `zdata`, which the callbacks read but neither
    // write nor replace: the appended bytes are its end.
    let held = zdata.copy();
    let bytes = held.bytes().unwrap_or_default();
    let appended = bytes.get(bytes.len() - data.len()..).unwrap_or_default();
    Some(execute(state, Some(bytes), appended, limit))
}

/// The `length` bytes of `zdata` that end where the piece at `at` does: a span's
/// pieces so far.
fn pieces(zdata: &Value, at: *const c_char, piece: usize, length: usize) -> &[u8] {
    let bytes = zdata.bytes().unwrap_or_default();
    let end = at
        .addr()
        .wrapping_add(piece)
        .wrapping_sub(bytes.as_ptr().addr());
    end.checked_sub(length)
        .and_then(|from| bytes.get(from..end))
        .unwrap_or_default()
}

/// `request.zdata`'s bytes.
fn zdata_span(state: &State) -> (*const u8, usize) {
    let bytes = state.request.zdata.bytes().unwrap_or_default();
    (bytes.as_ptr(), bytes.len())
}

fn url(state: &mut State, span: &[u8]) -> Step {
    let (path, query) = request_url(span);
    state.request.path = Some(path.into());
    if query.is_empty() {
        return Step::Go;
    }
    add_server(state, Key::QueryString, stringl(query));
    edit(&mut state.request, Property::Get, |array| {
        treat_data(PARSE_STRING, Copied::of(query), array.separate());
    });
    Step::Go
}

fn on_piece_url(state: &mut State, at: *const c_char, length: usize) -> Step {
    state.parse.url_length += length;
    let (start, total) = zdata_span(state);
    // llhttp flushes unfinished spans at the end of each input buffer.
    if at.wrapping_add(length).cast() == start.wrapping_add(total) {
        return Step::Go;
    }
    // A reference of its own to `zdata`, which `url` reads but doesn't replace.
    let held = state.request.zdata.copy();
    let span = pieces(&held, at, length, state.parse.url_length);
    url(state, span)
}

fn on_piece_header_field(state: &mut State, at: *const c_char, length: usize) -> Step {
    if state.parser.flags & TRAILING != 0 {
        return Step::Go;
    }
    if state.parse.header_name_length == 0 {
        let (start, _) = zdata_span(state);
        state.parse.header_name_offset = at.addr().wrapping_sub(start.addr());
    }
    state.parse.header_name_length += length;
    Step::Go
}

fn on_piece_header_value(
    state: &mut State,
    at: *const c_char,
    length: usize,
    limit: usize,
) -> Step {
    if state.parser.flags & TRAILING != 0 {
        return Step::Go;
    }
    state.parse.header_value_length += length;
    let (start, total) = zdata_span(state);
    if at.wrapping_add(length).cast() == start.wrapping_add(total) {
        return Step::Go;
    }
    state.parse.header_value_len = 0;
    // A reference of its own to `zdata`, which `header_value` reads but doesn't replace.
    let held = state.request.zdata.copy();
    let offset = state.parse.header_name_offset;
    let name = held.bytes().unwrap_or_default();
    let name = name
        .get(offset..offset.saturating_add(state.parse.header_name_length))
        .unwrap_or_default();
    let value = pieces(&held, at, length, state.parse.header_value_length);
    let result = header_value(state, name, value, limit);
    state.parse.header_name_length = 0;
    state.parse.header_value_length = 0;
    result
}

fn set_error_reason(state: &mut State, reason: &'static &'static str) {
    state.parser.set_reason(reason);
}

/// `swoole_http_token_list_contains_value(at, length, "websocket")`, with
/// its pointer-sized fast comparison; `None` when the value is too long to scan.
fn contains_websocket(value: &[u8]) -> Option<bool> {
    match token_list_fast(value, c"websocket") {
        Some(found) => Some(found),
        // The scan copies the value into `sw_tg_buffer()`, which it must fit.
        None if value.len() >= STACK_BUFFER_SIZE => None,
        None => Some(token_list_contains(value, b"websocket")),
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
        .port_of_server_fd(conn.server_fd())
        .is_some_and(|port| port.open_websocket_protocol.get())
    {
        conn.websocket_requested();
    }
    true
}

fn header_value(state: &mut State, name: &[u8], mut value_: &[u8], limit: usize) -> Step {
    while let [rest @ .., b' ' | b'\t'] = value_ {
        value_ = rest;
    }
    let length = value_.len();
    let (action, known) =
        request_header(name, value_, state.parser.method, state.parse_cookie, true);
    match action {
        RequestHeader::Cookie => {
            edit(&mut state.request, Property::Cookie, |array| {
                parse_cookie(array.separate(), value_);
            });
        }
        RequestHeader::Upgrade => {
            const MAX_LENGTH: usize = STACK_BUFFER_SIZE - 1;
            let seen = state.parse.header_value_len;
            if seen > MAX_LENGTH || length > MAX_LENGTH - seen {
                set_error_reason(state, &"Upgrade header value exceeds the maximum length");
                return Step::Error(USER);
            }
            state.parse.header_value_len += length;
            match contains_websocket(value_) {
                None => {
                    set_error_reason(state, &"Upgrade header value exceeds the maximum length");
                    return Step::Error(USER);
                }
                Some(true) => {
                    state.websocket = true;
                    if !websocket_connection(state) {
                        let message = format!("session[{}] is closed", state.fd);
                        host::log(
                            LOG_TRACE,
                            SESSION_CLOSED,
                            c"http_request_on_header_value",
                            message,
                        );
                        return Step::Fail;
                    }
                }
                Some(false) => {}
            }
        }
        RequestHeader::Form => state.request.post_form_urlencoded = true,
        RequestHeader::Multipart => {
            if !multipart_start(state, value_, limit) {
                return Step::Fail;
            }
        }
        RequestHeader::Compression => {
            if state.enable_compression && enabled_codecs() != 0 {
                compression_method(state, value_);
            }
        }
        RequestHeader::Append => {}
    }
    let value_ = stringl(value_);
    let known = known.and_then(|index| HEADER_KEYS.get(index));
    // The request's `header` array, created with the request.
    edit(&mut state.request, Property::Header, |header| {
        match known.and_then(|&known| key(known)) {
            Some(known) => header.update(known, value_),
            None => header.merge(&name.to_ascii_lowercase(), value_),
        }
    });
    Step::Go
}

fn on_headers_complete(state: &mut State) -> Step {
    let request = &mut state.request;
    request.version =
        c_int::from(state.parser.http_major) * 100 + c_int::from(state.parser.http_minor);
    let path = request_path(request);
    // The URI, and path_info, which is decoded.
    let (uri, decoded) = (ZString::new(path), ZString::url_decode(path));

    let method = METHODS
        .get(usize::from(state.parser.method))
        .and_then(Interned::get)
        .map_or_else(
            || ZString::new(method_name(state).as_bytes()),
            ZStr::to_owned,
        );
    let method = Owned::string(method);
    let now = swoole_rs_corebase::time_now();
    let float = swoole_rs_corebase::microtime();
    let protocol = if state.request.version == 101 {
        Key::Http11
    } else {
        Key::Http10
    };
    edit_server(state, |server| {
        add_field(server, Key::RequestMethod, method);
        add_field(server, Key::RequestUri, Owned::string(uri));
        add_field(server, Key::PathInfo, Owned::string(decoded));
        add_field(server, Key::RequestTime, Owned::long(now));
        add_field(server, Key::RequestTimeFloat, Owned::double(float));
        if let (Some(name), Some(protocol)) = (key(Key::ServerProtocol), key(protocol)) {
            server.add_new(name, Owned::string(protocol.to_owned()));
        }
    });

    state.keepalive = state.parser.should_keep_alive();
    state.recv_chunked = state.parser.flags & CHUNKED != 0;
    Step::Go
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

/// A span of the body, which `input` holds.
fn on_body(state: &mut State, span: &[u8], input: &[u8], limit: usize) -> Step {
    let length = span.len();
    if length == 0 {
        return Step::Go;
    }
    if state.recv_chunked {
        let body = state
            .parse
            .chunked_body
            .get_or_insert_with(|| Box::new(BoundedBytes::new(limit, BUFFER_SIZE_STD)));
        if body.append(span).is_err() {
            return Step::Fail;
        }
    } else {
        let beginning = state.request.body_length == 0;
        let at = span.as_ptr().addr().wrapping_sub(input.as_ptr().addr());
        state.request.body_at = at.wrapping_sub(state.request.body_length);
        state.request.body_length += length;
        if state.mt_parser.is_some() {
            let prefix = body_prefix(span, beginning);
            if !multipart_parse(state, span.get(prefix..).unwrap_or_default()) {
                return Step::Fail;
            }
        }
    }
    if state.completed {
        Step::Pause
    } else {
        Step::Go
    }
}

/// The message is complete: `input` holds its body, unless it was chunked.
fn on_message_complete(state: &mut State, input: &[u8]) -> Step {
    let chunked = state.parse.chunked_body.is_some();
    if state.mt_parser.is_some()
        && let Some(body) = state.parse.chunked_body.take()
    {
        // The bundled multipart parser rejects calls that end inside a part header. llhttp body spans can end at
        // receive boundaries, so parse the dechunked body in one pass here instead of parsing each span. The body
        // is out of the state while the parser's callbacks take it, as `http2::Ctx::multipart`'s, then back for
        // `getContent()`.
        let bytes: &[u8] = (*body).as_ref();
        let prefix = body_prefix(bytes, true);
        let parsed = multipart_parse(state, bytes.get(prefix..).unwrap_or_default());
        state.parse.chunked_body = Some(body);
        if !parsed {
            return Step::Fail;
        }
    }
    let source = form_source(
        u32::from(chunked)
            | (u32::from(state.parse_body) << 1)
            | (u32::from(state.request.post_form_urlencoded) << 2)
            | (u32::from(state.recv_chunked) << 3)
            | (u32::from(state.request.body_length > 0) << 4),
    );
    match source {
        Some(FormSource::Chunked) => {
            let body = state.parse.chunked_body.as_deref();
            let text = Copied::of(body.map_or(&[], BoundedBytes::as_ref));
            edit(&mut state.request, Property::Post, |array| {
                treat_data(PARSE_STRING, text, array.separate());
            });
        }
        Some(FormSource::Body) => {
            let (at, length) = (state.request.body_at, state.request.body_length);
            let body = input.get(at..at.saturating_add(length));
            let text = Copied::of(body.unwrap_or_default());
            edit(&mut state.request, Property::Post, |array| {
                treat_data(PARSE_STRING, text, array.separate());
            });
        }
        None => {}
    }
    state.mt_parser = None;
    state.completed = true;
    // The analysis of the http protocol has been completed, no further processing is needed.
    Step::Pause
}

/// `Context::get_multipart_boundary` of a `multipart/form-data` content
/// type, then `Context::init_multipart_parser`; false when the boundary is missing or too long.
pub(crate) fn multipart_start(state: &mut State, content_type: &[u8], limit: usize) -> bool {
    let offset = b"multipart/form-data".len();
    let boundary = swoole_rs_http::boundary(content_type, offset)
        .ok()
        .flatten()
        .and_then(|span| content_type.get(span.offset..span.offset + span.length));
    let Some(parser) = boundary.and_then(MultipartParser::new) else {
        let message = format!("boundary of multipart/form-data not found, fd:{}", state.fd);
        host::log(
            swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
            0,
            c"Context::get_multipart_boundary",
            message,
        );
        /* make it same with protocol error */
        state.parser.error = INVALID_HEADER_TOKEN;
        return false;
    };
    state.mt_parser = Some(Box::new(Multipart {
        parser,
        header: MultipartHeader::new(limit),
        content_type: None,
        field: BoundedBytes::new(limit, 0),
        fp: None,
    }));
    true
}

/// `Context::parse_multipart_data`.
pub(crate) fn multipart_parse(state: &mut State, data: &[u8]) -> bool {
    let length = data.len();
    let Some(mut multipart) = state.mt_parser.take() else {
        return false;
    };
    let Multipart {
        parser,
        fp,
        field,
        header,
        content_type,
    } = &mut *multipart;
    let mut parts = Parts {
        state,
        fp,
        field,
        header,
        content_type,
    };
    let parsed = parser.execute(&mut parts, data);
    let mut reason = parser.error_message();
    state.mt_parser = Some(multipart);
    let func = c"Context::parse_multipart_data";
    let Ok(parsed) = parsed else {
        reason.truncate(STACK_BUFFER_SIZE);
        let mut message = b"parse multipart body failed, reason: ".to_vec();
        message.extend_from_slice(&reason);
        host::log(LOG_NOTICE, SERVER_INVALID_REQUEST, func, message);
        return false;
    };
    if parsed != length {
        let message = format!("parse multipart body failed, {parsed}/{length} bytes processed");
        host::log(LOG_NOTICE, SERVER_INVALID_REQUEST, func, message);
        return false;
    }
    true
}

/// `Context::set_compression_method`.
pub(crate) fn compression_method(state: &mut State, value: &[u8]) {
    let length = value.len().c_cast::<u32>() as usize;
    let codecs = enabled_codecs();
    let method = compression(value.get(..length).unwrap_or(value), codecs);
    state.accept_compression = method != swoole_rs_httpbindings::Compression::None;
    if state.accept_compression {
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
    buffer.get(..length).map_or_default(trim_quotes).into()
}

/// A NUL-terminated copy (`estrndup`).
fn c_copy(bytes: &[u8]) -> Box<[u8]> {
    let mut copy = Vec::with_capacity(bytes.len() + 1);
    copy.extend_from_slice(bytes);
    copy.push(0);
    copy.into_boxed_slice()
}

fn part_header_value(
    state: &mut State,
    name: &[u8],
    value_: &[u8],
    content_type: &mut Option<Vec<u8>>,
) -> bool {
    let func = c"multipart_body_on_header_value";
    // Hash collision attack
    let max_input_vars = max_input_vars();
    if i64::from(state.parse.input_var_num) > max_input_vars {
        host::notice(&format!(
            "Input variables exceeded {max_input_vars}. To increase the limit change max_input_vars in php.ini"
        ));
        return true;
    }
    state.parse.input_var_num = state.parse.input_var_num.wrapping_add(1);

    if name.eq_ignore_ascii_case(b"content-disposition") {
        let offset = if find_delimiter(value_, b"form-data;").is_some() {
            b"form-data;".len()
        } else if find_delimiter(value_, b"attachment;").is_some() {
            b"attachment;".len()
        } else {
            let mut message = b"Unsupported Content-Disposition [".to_vec();
            message.extend_from_slice(value_);
            message.push(b']');
            host::log(
                swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
                0,
                func,
                message,
            );
            return true;
        };
        // Released on every return, which upstream's early ones leak.
        let mut fields = OwnedArray::default();
        parse_cookie(fields.separate(), value_.get(offset..).unwrap_or_default());
        let field = |key: &[u8]| fields.array().and_then(|fields| fields.find(key));

        let Some((form_name, form_name_len)) = field(b"name").and_then(string_value) else {
            return true;
        };
        if form_name_len >= FORM_KEYLEN {
            let mut message = b"form_name[".to_vec();
            message.extend_from_slice(form_name);
            message.extend_from_slice(b"] is too large");
            host::log(
                swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
                0,
                func,
                message,
            );
            return false;
        }
        let form_name = trimmed(form_name, form_name_len);
        if let Some(filename) = field(b"filename") {
            // upload file
            let (filename, filename_len) = string_value(filename).unwrap_or((&[], 0));
            if filename_len >= FORM_KEYLEN {
                let mut message = b"filename[".to_vec();
                message.extend_from_slice(filename);
                message.extend_from_slice(b"] is too large");
                host::log(
                    swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
                    0,
                    func,
                    message,
                );
                return false;
            }
            state.parse.input_name = Some(c_copy(&form_name));
            let mut header = OwnedArray::default();
            let content_type = content_type.take();
            header.insert_bytes(b"type", Some(content_type.as_deref().unwrap_or_default()));
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
            *content_type = Some(value_.to_vec());
        }
    } else if name.eq_ignore_ascii_case(b"swoole-upload-file") && state.upload_preprocessed {
        // When the "SW_HTTP_UPLOAD_FILE" header appears in the request, it indicates that the uploaded file has been
        // saved in a temporary file. The binary content in the message body will be replaced with the temporary
        // filename. However, the Content-Length still reflects the original message size, causing llhttp to believe
        // there is still data to be received. As a result, llhttp fails to trigger the message callback. Therefore, we
        // need to set `ctx->completed = 1` to indicate that the message processing is complete.
        state.completed = true;
        upload_bytes(state, b"tmp_name", value_);
        upload_long(state, b"error", UPLOAD_FILE_READY);
        upload_long(state, b"size", path_size(value_));
        add_upload(state, value_);
    }
    true
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
fn file_size(upload: &mut Upload) -> i64 {
    // Upstream ignores fflush's result: a failure leaves the size what reached the file.
    upload.flush().unwrap_or_default();
    match upload.get_ref().metadata() {
        Ok(meta) if meta.is_file() => i64::try_from(meta.len()).unwrap_or(-1),
        _ => -1,
    }
}

fn part_data(state: &mut State, upload: &mut Option<Upload>, data: &[u8]) {
    let Some(file) = upload else {
        return;
    };
    if let Err(error) = file.write_all(data) {
        upload_long(state, b"error", UPLOAD_CANT_WRITE);
        *upload = None;
        host::log(
            swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
            -error.raw_os_error().unwrap_or(0),
            c"multipart_body_on_data",
            "write upload file failed",
        );
    }
}

fn part_headers_complete(state: &mut State, upload: &mut Option<Upload>) {
    if state.parse.input_name.is_none() {
        return;
    }
    if upload_error(state).is_none_or(|code| code.as_long().is_some_and(|code| code != UPLOAD_OK)) {
        return;
    }
    let mut path = [
        crate::context::upload_tmp_dir(state).to_bytes(),
        b"/swoole.upfile.XXXXXX\0",
    ]
    .concat();
    let Some(file) = swoole_rs_corebase::ossupport::TempFile::create(&mut path) else {
        upload_long(state, b"error", UPLOAD_NO_TMP_DIR);
        return;
    };
    let name = file.path().to_bytes();
    if !add_upload(state, name) {
        upload_long(state, b"error", UPLOAD_CANT_WRITE);
        return;
    }
    upload_bytes(state, b"tmp_name", name);
    *upload = Some(BufWriter::new(file.persist().1));
}

fn part_data_end(state: &mut State, upload: &mut Option<Upload>, data: &mut BoundedBytes) {
    if let Some(name) = state.parse.form_data_name.take() {
        // `c_copy`'s name holds its NUL.
        let name = CStr::from_bytes_until_nul(&name).unwrap_or_default();
        if !bailout::pending() {
            edit(&mut state.request, Property::Post, |array| {
                let post = array.separate();
                bailout::defer(contain_with(|| {
                    lend_in(data.as_ref(), |at, count| {
                        php_register_variable_safe(name.into(), at, count, post);
                    });
                }));
            });
        }
        data.clear();
        return;
    }
    let Some(input_name) = state.parse.input_name.take() else {
        return;
    };
    if let Some(mut file) = upload.take() {
        upload_long(state, b"size", file_size(&mut file));
    }
    let Some(code) = upload_error(state).map(Value::long) else {
        state.parse.input_name = Some(input_name);
        return;
    };
    if code == UPLOAD_FILE_READY {
        upload_long(state, b"error", UPLOAD_OK);
    }
    let header = std::mem::take(&mut state.parse.multipart_header);
    // `input_name` holds its NUL; C sees it up to the first one.
    let text = input_name
        .split(|&byte| byte == 0)
        .next()
        .unwrap_or_default();
    let bracket = text.iter().position(|&byte| byte == b'[');
    let parse_files = state.parse_files;
    let registered = edit(&mut state.request, Property::Files, |files| {
        match bracket.and_then(|at| text.split_at_checked(at)) {
            Some((name, input_path)) if parse_files && !name.is_empty() => {
                const META: usize = FORM_KEYLEN + b"[tmp_name]".len() - 1;
                for field in ["name", "type", "tmp_name", "error", "size"] {
                    let mut meta = name.get(..META - 1).unwrap_or(name).to_vec();
                    meta.extend_from_slice(format!("[{field}]").as_bytes());
                    meta.extend_from_slice(input_path);
                    meta.truncate(META - 1);
                    let meta = CString::new(meta).unwrap_or_default();
                    // The header declared each field when the part began.
                    let entry = header
                        .array()
                        .and_then(|header| header.find(field.as_bytes()));
                    files.register_variable(&meta, entry.map_or_else(Owned::null, Value::copy))?;
                }
                Ok(())
            }
            _ => files.register_variable(&CString::new(text).unwrap_or_default(), header),
        }
    });
    if let Some(Err(bailed)) = registered {
        bailed.defer();
    }
}

/// `getContent()`: the body received so far.
pub(crate) fn body(state: &State) -> Option<&[u8]> {
    let length = state.request.body_length;
    if length > 0 {
        let data = state.request.zdata.bytes()?;
        return data.get(data.len().checked_sub(length)?..);
    }
    if let Some(chunked) = state
        .parse
        .chunked_body
        .as_deref()
        .map(BoundedBytes::as_ref)
        .filter(|body| !body.is_empty())
    {
        return Some(chunked);
    }
    let data = state.request.h2_data.as_deref()?.as_ref();
    (!data.is_empty()).then_some(data)
}

/// `llhttp_method_name(parser.method)`.
pub(crate) fn method_name(state: &State) -> &'static str {
    llhttp::method_name(state.parser.method)
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
    fn names_keep_their_terminator() {
        assert_eq!(&*c_copy(b"a[b]"), b"a[b]\0");
    }

    #[test]
    fn form_bodies_follow_upstreams_methods() {
        // request_header takes llhttp's method numbers.
        for method in 0..=u8::MAX {
            let (action, _) = request_header(
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
