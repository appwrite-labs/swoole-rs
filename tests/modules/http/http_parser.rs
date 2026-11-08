//! Upstream's `core-tests/src/server/http_parser.cpp`, one Rust test per TEST: the
//! bundled llhttp (crates/oracle's copy of `thirdparty/llhttp`) driven through
//! `swoole_llhttp_parser_init`/`_execute`, with the same inputs, callbacks and
//! expected values.
use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_oracle as _;

/// `llhttp_t`.
#[repr(C)]
struct Parser {
    index: i32,
    span_pos0: *mut c_void,
    span_cb0: *mut c_void,
    error: i32,
    reason: *const c_char,
    error_pos: *const c_char,
    data: *mut c_void,
    current: *mut c_void,
    content_length: u64,
    kind: u8,
    method: u8,
    http_major: u8,
    http_minor: u8,
    header_state: u8,
    lenient_flags: u16,
    upgrade: u8,
    finish: u8,
    flags: u16,
    status_code: u16,
    initial_message_completed: u8,
    settings: *const c_void,
}

type DataCallback = unsafe extern "C" fn(*mut Parser, *const c_char, usize) -> c_int;
type Callback = unsafe extern "C" fn(*mut Parser) -> c_int;

/// `llhttp_settings_t`.
#[repr(C)]
struct Settings {
    on_message_begin: Callback,
    on_protocol: DataCallback,
    on_url: DataCallback,
    on_status: DataCallback,
    on_method: DataCallback,
    on_version: DataCallback,
    on_header_field: DataCallback,
    on_header_value: DataCallback,
    on_chunk_extension_name: DataCallback,
    on_chunk_extension_value: DataCallback,
    on_headers_complete: Callback,
    on_body: DataCallback,
    on_message_complete: Callback,
    on_protocol_complete: Callback,
    on_url_complete: Callback,
    on_status_complete: Callback,
    on_method_complete: Callback,
    on_version_complete: Callback,
    on_header_field_complete: Callback,
    on_header_value_complete: Callback,
    on_chunk_extension_name_complete: Callback,
    on_chunk_extension_value_complete: Callback,
    on_chunk_header: Callback,
    on_chunk_complete: Callback,
    on_reset: Callback,
}

unsafe extern "C" {
    fn llhttp_init(parser: *mut Parser, kind: c_int, settings: *const Settings);
    fn llhttp_execute(parser: *mut Parser, data: *const c_char, length: usize) -> c_int;
    fn llhttp_resume(parser: *mut Parser);
    fn llhttp_resume_after_upgrade(parser: *mut Parser);
    fn llhttp_get_error_pos(parser: *const Parser) -> *const c_char;
    fn llhttp_get_errno(parser: *const Parser) -> c_int;
    fn llhttp_get_error_reason(parser: *const Parser) -> *const c_char;
    fn llhttp_get_http_major(parser: *mut Parser) -> u8;
    fn llhttp_get_http_minor(parser: *mut Parser) -> u8;
    fn llhttp_get_method(parser: *mut Parser) -> u8;
    fn llhttp_get_status_code(parser: *mut Parser) -> c_int;
    fn llhttp_get_upgrade(parser: *mut Parser) -> u8;
    fn llhttp_should_keep_alive(parser: *const Parser) -> c_int;
    fn llhttp_reset(parser: *mut Parser);
    fn llhttp_method_name(method: c_int) -> *const c_char;
}

const HTTP_REQUEST: c_int = 1;
const HTTP_RESPONSE: c_int = 2;
const HPE_OK: c_int = 0;
const HPE_PAUSED: c_int = 21;
const HPE_PAUSED_UPGRADE: c_int = 22;
const HPE_PAUSED_H2_UPGRADE: c_int = 23;
const HTTP_GET: u8 = 1;
const HTTP_PRI: u8 = 34;
const HTTP_STATUS_OK: c_int = 200;

/// The test's `HttpContext`: the fields its callbacks write.
#[derive(Default)]
struct HttpContext {
    completed: u8,
    header_fields: Vec<String>,
    header_values: Vec<String>,
    query_string: String,
}

unsafe fn context<'a>(parser: *mut Parser) -> &'a mut HttpContext {
    unsafe { &mut *(*parser).data.cast::<HttpContext>() }
}

unsafe fn text(at: *const c_char, length: usize) -> String {
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) })
        .into_owned()
}

unsafe extern "C" fn on_url(parser: *mut Parser, at: *const c_char, length: usize) -> c_int {
    unsafe { context(parser).query_string = text(at, length) };
    0
}

unsafe extern "C" fn on_header_field(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    unsafe { context(parser).header_fields.push(text(at, length)) };
    0
}

unsafe extern "C" fn on_header_value(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    unsafe { context(parser).header_values.push(text(at, length)) };
    0
}

unsafe extern "C" fn on_message_complete(parser: *mut Parser) -> c_int {
    unsafe { context(parser).completed = 1 };
    0
}

unsafe extern "C" fn data_cb(_: *mut Parser, _: *const c_char, _: usize) -> c_int {
    0
}

unsafe extern "C" fn cb(_: *mut Parser) -> c_int {
    0
}

/// `http_parser_settings`.
static SETTINGS: Settings = Settings {
    on_message_begin: cb,
    on_protocol: data_cb,
    on_url,
    on_status: data_cb,
    on_method: data_cb,
    on_version: data_cb,
    on_header_field,
    on_header_value,
    on_chunk_extension_name: data_cb,
    on_chunk_extension_value: data_cb,
    on_headers_complete: cb,
    on_body: data_cb,
    on_message_complete,
    on_protocol_complete: cb,
    on_url_complete: cb,
    on_status_complete: cb,
    on_method_complete: cb,
    on_version_complete: cb,
    on_header_field_complete: cb,
    on_header_value_complete: cb,
    on_chunk_extension_name_complete: cb,
    on_chunk_extension_value_complete: cb,
    on_chunk_header: cb,
    on_chunk_complete: cb,
    on_reset: cb,
};

/// A parser and its context (`swoole_http_parser_create`), freed on drop
/// (`swoole_http_destroy_context`).
struct Http {
    parser: Box<Parser>,
    ctx: Box<HttpContext>,
}

impl Http {
    fn new(kind: c_int) -> Self {
        let mut http = Http {
            parser: Box::new(unsafe { std::mem::zeroed() }),
            ctx: Box::default(),
        };
        unsafe { llhttp_init(&raw mut *http.parser, kind, std::ptr::null()) };
        http.parser.data = (&raw mut *http.ctx).cast();
        http
    }

    fn p(&mut self) -> *mut Parser {
        &raw mut *self.parser
    }

    /// `swoole_llhttp_parser_execute(parser, &http_parser_settings, data, length)`.
    fn execute(&mut self, data: &[u8]) -> usize {
        let parser = self.p();
        self.parser.settings = (&raw const SETTINGS).cast();
        let result = unsafe { llhttp_execute(parser, data.as_ptr().cast(), data.len()) };
        if result == HPE_OK {
            return data.len();
        }
        let parsed = unsafe { llhttp_get_error_pos(parser) }.addr() - data.as_ptr().addr();
        match result {
            HPE_PAUSED => unsafe { llhttp_resume(parser) },
            HPE_PAUSED_UPGRADE => unsafe { llhttp_resume_after_upgrade(parser) },
            _ => {}
        }
        parsed
    }

    fn errno(&mut self) -> c_int {
        unsafe { llhttp_get_errno(self.p()) }
    }

    fn reason(&mut self) -> &[u8] {
        unsafe { CStr::from_ptr(llhttp_get_error_reason(self.p())) }.to_bytes()
    }

    fn reset(&mut self) {
        unsafe { llhttp_reset(self.p()) };
    }
}

const GET_REQUEST: &[u8] = b"GET /get HTTP/1.1\r\n\
Host: www.maria.com\r\n\
User-Agent: curl/7.64.1\r\n\
Accept: */*\r\n\
Connection: keep-alive\r\n\
\r\n";

#[test]
fn get_request() {
    let mut http = Http::new(HTTP_REQUEST);
    let length = http.execute(GET_REQUEST);
    assert_eq!(length, GET_REQUEST.len(), "whole request parsed");
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(http.ctx.completed, 1, "message complete");
    assert_eq!(
        unsafe { llhttp_should_keep_alive(http.p()) },
        1,
        "keep-alive"
    );
}

#[test]
fn version() {
    let mut http = Http::new(HTTP_REQUEST);
    let http11 = b"GET /get HTTP/1.1\r\n\r\n";
    assert_eq!(http.execute(http11), http11.len(), "whole request parsed");
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(http.ctx.completed, 1, "message complete");
    assert_eq!(unsafe { llhttp_get_http_major(http.p()) }, 1, "major");
    assert_eq!(unsafe { llhttp_get_http_minor(http.p()) }, 1, "minor");
}

#[test]
fn incomplete() {
    let mut http = Http::new(HTTP_REQUEST);
    let incomplete = b"GET /get HTTP/1.1\r\n";
    assert_eq!(
        http.execute(incomplete),
        incomplete.len(),
        "whole input parsed"
    );
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(http.ctx.completed, 0, "message incomplete");
}

#[test]
fn method() {
    let mut http = Http::new(HTTP_REQUEST);
    let incomplete = b"GET /get HTTP/1.1\r\n\r\n";
    assert_eq!(
        http.execute(incomplete),
        incomplete.len(),
        "whole request parsed"
    );
    assert_eq!(unsafe { llhttp_get_method(http.p()) }, HTTP_GET, "method");
    let name = unsafe { CStr::from_ptr(llhttp_method_name(HTTP_GET.into())) };
    assert_eq!(name.to_bytes(), b"GET", "method name");
}

#[test]
fn websocket() {
    let mut http = Http::new(HTTP_REQUEST);
    let websocket = b"GET /chat HTTP/1.1\r\n\
Host: example.com\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
Sec-WebSocket-Version: 13\r\n\
Origin: http://example.com\r\n\r\n";
    assert_eq!(
        http.execute(websocket),
        websocket.len(),
        "whole request parsed"
    );
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(unsafe { llhttp_get_upgrade(http.p()) }, 1, "upgrade");
    assert_eq!(http.ctx.completed, 1, "message complete");
}

#[test]
fn http2() {
    let mut http = Http::new(HTTP_REQUEST);
    let http2 = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    assert_eq!(http.execute(http2), http2.len(), "whole preface parsed");
    assert_eq!(http.errno(), HPE_PAUSED_H2_UPGRADE, "paused for h2");
    assert_eq!(unsafe { llhttp_get_method(http.p()) }, HTTP_PRI, "method");
}

#[test]
fn header_field_and_value() {
    let mut http = Http::new(HTTP_REQUEST);
    assert_eq!(
        http.execute(GET_REQUEST),
        GET_REQUEST.len(),
        "whole request parsed"
    );
    assert_eq!(http.ctx.completed, 1, "message complete");
    assert_eq!(
        http.ctx.header_fields[..4],
        ["Host", "User-Agent", "Accept", "Connection"],
        "fields"
    );
    assert_eq!(
        http.ctx.header_values[..4],
        ["www.maria.com", "curl/7.64.1", "*/*", "keep-alive"],
        "values"
    );
}

#[test]
fn query_string() {
    let mut http = Http::new(HTTP_REQUEST);
    let request = b"GET /get/swoole?a=1&b=2 HTTP/1.1\r\n\r\n";
    assert_eq!(http.execute(request), request.len(), "whole request parsed");
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(http.ctx.query_string, "/get/swoole?a=1&b=2", "url");
}

#[test]
fn chunk() {
    let mut http = Http::new(HTTP_RESPONSE);
    let chunk =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n\
5\r\nHello\r\n6\r\n World\r\n3\r\n!!!\r\n0\r\n\r\n";
    assert_eq!(http.execute(chunk), chunk.len(), "whole response parsed");
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(http.ctx.completed, 1, "message complete");
}

#[test]
fn response() {
    let mut http = Http::new(HTTP_RESPONSE);
    let response = b"HTTP/1.1 200 OK\r\n\
Server: CLOUD ELB 1.0.0\r\n\
Date: Sat, 04 Feb 2023 08:47:14 GMT\r\n\
Content-Type: application/json\r\n\
Content-Length: 18\r\n\
Connection: close\r\n\
\r\n\
{\"name\" : \"laala\"}";
    assert_eq!(
        http.execute(response),
        response.len(),
        "whole response parsed"
    );
    assert_eq!(http.errno(), HPE_OK, "no error");
    assert_eq!(
        unsafe { llhttp_get_status_code(http.p()) },
        HTTP_STATUS_OK,
        "status"
    );
    assert_eq!(unsafe { llhttp_get_http_major(http.p()) }, 1, "major");
    assert_eq!(unsafe { llhttp_get_http_minor(http.p()) }, 1, "minor");
    assert_eq!(http.ctx.completed, 1, "message complete");
    assert_eq!(
        http.ctx.header_fields[..5],
        [
            "Server",
            "Date",
            "Content-Type",
            "Content-Length",
            "Connection"
        ],
        "fields"
    );
    assert_eq!(
        http.ctx.header_values[..5],
        [
            "CLOUD ELB 1.0.0",
            "Sat, 04 Feb 2023 08:47:14 GMT",
            "application/json",
            "18",
            "close"
        ],
        "values"
    );
}

/// `request_error_protocols` and `request_error_messages`, paired.
const REQUEST_ERRORS: &[(&[u8], &str)] = &[
    // request/connection
    (b"PUT /url HTTP/1.0\r\n\r\nPUT /url HTTP/1.1\r\n\r\n", "Data after `Connection: close`"),
    (b"POST / HTTP/1.1\r\nHost: www.example.com\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 4\r\nConnection: close\r\n\r\nq=42\r\n\r\nGET / HTTP/1.1\r\n", "Data after `Connection: close`"),
    (b"PUT /url HTTP/1.1\r\nConnection : upgrade\r\nContent-Length: 4\r\nUpgrade: ws\r\n\r\nabcdefgh", "Invalid header field char"),
    // request/content-length
    (b"PUT /url HTTP/1.1\r\nContent-Length: 1000000000000000000000\r\n\r\n", "Content-Length overflow"),
    (b"PUT /url HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n", "Duplicate Content-Length"),
    (b"PUT /url HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: identity\r\n\r\n", "Transfer-Encoding can't be present with Content-Length"),
    (b"PUT /url HTTP/1.1\r\nConnection: upgrade\r\nContent-Length : 4\r\nUpgrade: ws\r\n\r\nabcdefgh", "Invalid header field char"),
    (b"POST / HTTP/1.1\r\nContent-Length: 4 2\r\n\r\n", "Invalid character in Content-Length"),
    (b"POST / HTTP/1.1\r\nContent-Length: 13 37\r\n\r\n", "Invalid character in Content-Length"),
    (b"POST / HTTP/1.1\r\nContent-Length:\r\n\r\n", "Empty Content-Length"),
    (b"PUT /url HTTP/1.1\r\nContent\rLength: 003\r\n\r\nabc", "Invalid header token"),
    (b"PUT /url HTTP/1.1\r\nContent-Length: 3\r\n\rabc", "Expected LF after headers"),
    // request/method
    (b"PRI * HTTP/1.1\r\n\r\nSM\r\n\r\n", "Pause on PRI/Upgrade"),
    // request/sample
    (b"GET / HTTP/1.1\rLine: 1\r\n\r\n", "Expected CRLF after version"),
    (b"GET / HTTP/1.1\r\nLine1:   abc\n\tdef\n ghi\n\t\tjkl\n  mno \n\t \tqrs\nLine2: \t line2\t\nLine3:\n line3\nLine4: \n \nConnection:\n close\n\n", "Missing expected CR after header value"),
    // request/transfer-encoding
    (b"POST /chunked_w_unicorns_after_length HTTP/1.1\r\nHost: localhost\r\nTransfer-encoding: chunked\r\n\r\n2 erfrferferf\r\naa\r\n0 rrrr\r\n\r\n", "Invalid character in chunk size"),
    (b"POST /chunked_w_unicorns_after_length HTTP/1.1\r\nHost: localhost\r\nTransfer-encoding: chunked\r\n\r\n2;\r\naa\r\n0\r\n\r\n", "Invalid character in chunk extensions"),
    (b"POST /chunked_w_unicorns_after_length HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5;ilovew3=\"abc\";somuchlove=\"def; ghi\r\nhello\r\n6;blahblah;blah\r\n world\r\n0\r\n\r\n", "Invalid character in chunk extensions quoted value"),
    (b"PUT /url HTTP/1.1\r\nTransfer-Encoding: pigeons\r\n\r\n", "Request has invalid `Transfer-Encoding`"),
    (b"POST /post_identity_body_world?q=search#hey HTTP/1.1\r\nAccept: */*\r\nTransfer-Encoding: identity\r\nContent-Length: 5\r\n\r\nWorld", "Content-Length can't be present with Transfer-Encoding"),
    (b"POST / HTTP/1.1\r\nHost: foo\r\nContent-Length: 10\r\nTransfer-Encoding:\r\nTransfer-Encoding:\r\nTransfer-Encoding:\r\n\r\n2\r\nAA\r\n0\r\n", "Transfer-Encoding can't be present with Content-Length"),
    (b"POST /post_identity_body_world?q=search#hey HTTP/1.1\r\nAccept: */*\r\nTransfer-Encoding: chunked, deflate\r\n\r\nWorld", "Invalid `Transfer-Encoding` header value"),
    (b"POST /post_identity_body_world?q=search#hey HTTP/1.1\r\nAccept: */*\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: deflate\r\n\r\nWorld", "Invalid `Transfer-Encoding` header value"),
    (b"POST /post_identity_body_world?q=search#hey HTTP/1.1\r\nAccept: */*\r\nTransfer-Encoding: chunkedchunked\r\n\r\n5\r\nWorld\r\n0\r\n\r\n", "Request has invalid `Transfer-Encoding`"),
    (b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nfoo\r\n\r\n", "Invalid character in chunk size"),
    (b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3 \n  \r\n\\\r\nfoo\r\n\r\n", "Invalid character in chunk size"),
    (b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked  abc\r\n\r\n5\r\nWorld\r\n0\r\n\r\n", "Request has invalid `Transfer-Encoding`"),
    (b"GET / HTTP/1.1\r\nHost: a\r\nConnection: close \r\nTransfer-Encoding: chunked \r\n\r\n5\r\r;ABCD\r\n34\r\nE\r\n0\r\n\r\nGET / HTTP/1.1 \r\nHost: a\r\nContent-Length: 5\r\n\r\n0\r\n\r\n", "Expected LF after chunk size"),
    (b"GET / HTTP/1.1\r\nHost: a\r\nConnection: close \r\nTransfer-Encoding: chunked \r\n\r\n5\r\nABCDE0\r\n\r\n", "Expected LF after chunk data"),
    (b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\na \r\n0123456789\r\n0\r\n\r\n", "Invalid character in chunk size"),
    // request/invalid
    (b"GET /music/sweet/music ICE/1.0\r\nHost: example.com\r\n\r\n", "Expected SOURCE method for ICE/x.x request"),
    (b"GET /music/sweet/music IHTTP/1.0\r\nHost: example.com\r\n\r\n", "Expected HTTP/, RTSP/ or ICE/"),
    (b"PUT /music/sweet/music RTSP/1.0\r\nHost: example.com\r\n\r\n", "Invalid method for RTSP/x.x request"),
    (b"ANNOUNCE /music/sweet/music HTTP/1.0\r\nHost: example.com\r\n\r\n", "Invalid method for HTTP/x.x request"),
    (b"GET / HTTP/1.1\r\nFoo: 1\rBar: 2\r\n\r\n", "Missing expected LF after header value"),
    (b"POST / HTTP/1.1\r\nHost: localhost:5000\r\nx:x\nTransfer-Encoding: chunked\r\n\r\n1\r\nA\r\n0\r\n\r\n", "Missing expected CR after header value"),
    (b"GET / HTTP/1.1\r\nConnection: close\r\nHost: a\r\n\rZGET /evil: HTTP/1.1\r\nHost: a\r\n\r\n", "Expected LF after headers"),
    (b"GET / HTTP/1.1\r\nConnection: close\r\nHost: a\r\n\r\nZGET /evil: HTTP/1.1\r\nHost: a\r\n\r\n", "Data after `Connection: close`"),
    (b"POST / HTTP/1.1\r\nConnection: Close\r\nHost: localhost:5000\r\nx:\rTransfer-Encoding: chunked\r\n\r\n1\r\nA\r\n0\r\n\r\n", "Expected LF after CR"),
    (b"POST / HTTP/1.1\r\nHost: localhost:5000\r\nx:\nTransfer-Encoding: chunked\r\n\r\n1\r\nA\r\n0\r\n\r\n", "Invalid header value char"),
    (b"GET / HTTP/1.1\r\nFo@: Failure\r\n\r\n", "Invalid header token"),
    (b"GET / HTTP/1.1\r\nFoo\x01\test: Bar\r\n\r\n", "Invalid header token"),
    (b"GET / HTTP/1.1\r\n: Bar\r\n\r\n", "Invalid header token"),
    (b"MKCOLA / HTTP/1.1\r\n\r\n", "Expected space after method"),
    (b"GET / HTTP/1.1\r\nname\r\n : value\r\n\r\n", "Invalid header token"),
    (b"GET / HTTP/1.1\r\nHost: www.example.com\r\nConnection\r\x1b\x35\xd5eep-Alive\r\nAccept-Encoding: gzip\r\n\r\n", "Invalid header token"),
    (b"GET / HTTP/1.1\r\nHost: www.example.com\r\nX-Some-Header\r\x1b\x35\xd5eep-Alive\r\nAccept-Encoding: gzip\r\n\r\n", "Invalid header token"),
    (b"GET / HTTP/1.1\r\nHost: localhost\r\nDummy: x\nContent-Length: 23\r\n\r\nGET / HTTP/1.1\r\nDummy: GET /admin HTTP/1.1\r\nHost: localhost\r\n\r\n", "Missing expected CR after header value"),
    (b"GET / HTTP/5.6", "Invalid HTTP version"),
    (b"GET / HTTP/1.1\r\n Host: foo\r\n", "Unexpected space after start line"),
    (b"POST / HTTP/1.1\nTransfer-Encoding: chunked\nTrailer: Baz\nFoo: abc\nBar: def\n\n1\nA\n1;abc\nB\n1;def=ghi\nC\n1;jkl=\"mno\"\nD\n0\n\nBaz: ghi\n\n", "Expected CRLF after version"),
    (b"POST /hello HTTP/1.1\r\nHost: localhost\r\nFoo: bar\r\n Content-Length: 38\r\n\r\nGET /bye HTTP/1.1\r\nHost: localhost\r\n\r\n", "Unexpected whitespace after header value"),
    // request/uri
    ("GET /δ¶/δt/pope?q=1#narf HTTP/1.1\r\nHost: github.com\r\n\r\n".as_bytes(), "Invalid char in url path"),
    (b"GET /foo bar/ HTTP/1.1\r\n\r\n", "Expected HTTP/, RTSP/ or ICE/"),
];

/// `response_error_protocols` and `response_error_messages`, paired.
const RESPONSE_ERRORS: &[(&[u8], &str)] = &[
    // response/connection
    (b"HTTP/1.1 204 No content\r\nConnection: close\r\n\r\nHTTP/1.1 200 OK", "Data after `Connection: close`"),
    (b"HTTP/1.1 200 No content\r\nContent-Length: 5\r\nConnection: close\r\n\r\n2ad731e3-4dcd-4f70-b871-0ad284b29ffc", "Data after `Connection: close`"),
    // response/invalid
    (b"HTP/1.1 200 OK\r\n\r\n", "Expected HTTP/, RTSP/ or ICE/"),
    (b"HTTP/01.1 200 OK\r\n\r\n", "Expected dot"),
    (b"HTTP/11.1 200 OK\r\n\r\n", "Expected dot"),
    (b"HTTP/1.01 200 OK\r\n\r\n", "Expected space after version"),
    (b"HTTP/1.1\t200 OK\r\n\r\n", "Expected space after version"),
    (b"\rHTTP/1.1\t200 OK\r\n\r\n", "Expected space after version"),
    (b"HTTP/1.1 200 OK\r\nFoo: 1\rBar: 2\r\n\r\n", "Missing expected LF after header value"),
    (b"HTTP/5.6 200 OK\r\n\r\n", "Invalid HTTP version"),
    (b"HTTP/1.1 200 OK\r\n Host: foo\r\n", "Unexpected space after start line"),
    (b"HTTP/1.1  200 OK\r\n\r\n", "Invalid status code"),
    (b"HTTP/1.1 2 OK\r\n\r\n", "Invalid status code"),
    (b"HTTP/1.1 200 OK\nContent-Length: 0\n\n", "Missing expected CR after response line"),
    (b"HTTP/1.1 200 OK\nFoo: abc\nBar: def\n\nBODY\n", "Missing expected CR after response line"),
    // response/sample
    (b"HTTPER/1.1 200 OK\r\n\r\n", "Expected HTTP/, RTSP/ or ICE/"),
    (b"HTTP/1.1 200 OK\nContent-Type: text/html; charset=utf-8\nConnection: close\n\nthese headers are from http://news.ycombinator.com/", "Missing expected CR after response line"),
    (b"HTTP/1.1 200 OK\r\nServer: Microsoft-IIS/6.0\r\nX-Powered-By: ASP.NET\r\nen-US Content-Type: text/xml\r\nContent-Type: text/xml\r\nContent-Length: 16\r\nDate: Fri, 23 Jul 2010 18:45:38 GMT\r\nConnection: keep-alive\r\n\r\n<xml>hello</xml>", "Invalid header token"),
    // response/transfer-encoding
    (b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n25 \r\nThis is the data in the first chunk\r\n1C\r\nand this is the second one\r\n0 \r\n\r\n", "Invalid character in chunk size"),
    (b"HTTP/1.1 200 OK\r\nHost: localhost\r\nTransfer-encoding: chunked\r\n\r\n2 erfrferferf\r\naa\r\n0 rrrr\r\n\r\n", "Invalid character in chunk size"),
    (b"HTTP/1.1 200 OK\r\nHost: localhost\r\nTransfer-encoding: chunked\r\n\r\n2;\r\naa\r\n0\r\n\r\n", "Invalid character in chunk extensions"),
    (b"HTTP/1.1 200 OK\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5;ilovew3=\"abc\";somuchlove=\"def; ghi\r\nhello\r\n6;blahblah;blah\r\n world\r\n0\r\n", "Invalid character in chunk extensions quoted value"),
];

fn error_cases(kind: c_int, cases: &[(&[u8], &str)]) {
    let mut http = Http::new(kind);
    for (i, (protocol, message)) in cases.iter().enumerate() {
        http.execute(protocol);
        assert_eq!(http.reason(), message.as_bytes(), "case {i}");
        assert_ne!(http.errno(), HPE_OK, "case {i}");
        http.reset();
    }
}

#[test]
fn request_error_case() {
    assert_eq!(
        REQUEST_ERRORS.len(),
        54,
        "upstream's 54 protocols and messages"
    );
    error_cases(HTTP_REQUEST, REQUEST_ERRORS);
}

#[test]
fn response_error_case() {
    assert_eq!(
        RESPONSE_ERRORS.len(),
        22,
        "upstream's 22 protocols and messages"
    );
    error_cases(HTTP_RESPONSE, RESPONSE_ERRORS);
}

/// `request_success_case`.
const REQUEST_SUCCESS: &[&[u8]] = &[
    b"PUT /url HTTP/1.1\r\nConnection: keep-alive\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nConnection: keep-alive\r\n\r\nPUT /url HTTP/1.1\r\nConnection: keep-alive\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nConnection: close\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nConnection: close, token, upgrade, token, keep-alive\r\n\r\n",
    b"GET /demo HTTP/1.1\r\nHost: example.com\r\nConnection: keep-alive, upgrade\r\nUpgrade: WebSocket\r\n\r\nHot diggity dogg",
    b"PUT /url HTTP/1.1\r\nConnection: upgrade\r\nUpgrade: ws\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nConnection: upgrade\r\nContent-Length: 4\r\nUpgrade: ws\r\n\r\nabcdefgh",
    b"GET /demo HTTP/1.1\r\nHost: example.com\r\nConnection: Upgrade\r\nSec-WebSocket-Key2: 12998 5 Y3 1  .P00\r\nSec-WebSocket-Protocol: sample\r\nUpgrade: WebSocket\r\nSec-WebSocket-Key1: 4 @1  46546xW%0l 1 5\r\nOrigin: http://example.com\r\n\r\nHot diggity dogg",
    b"POST /demo HTTP/1.1\r\nHost: example.com\r\nConnection: Upgrade\r\nUpgrade: HTTP/2.0\r\nContent-Length: 15\r\n\r\nsweet post body\\Hot diggity dogg",
    b"PUT /url HTTP/1.1\r\nContent-Length: 003\r\n\r\nabc",
    b"PUT /url HTTP/1.1\r\nContent-Length: 003\r\nOhai: world\r\n\r\nabc",
    b"GET /get_funky_content_length_body_hello HTTP/1.0\r\nconTENT-Length: 5\r\n\r\nHELLO",
    b"POST / HTTP/1.1\r\nContent-Length:  42 \r\n\r\n",
    b"REPORT /test HTTP/1.1\r\n\r\n",
    b"CONNECT 0-home0.netscape.com:443 HTTP/1.0\r\nUser-agent: Mozilla/1.1N\r\nProxy-authorization: basic aGVsbG86d29ybGQ=\r\n\r\nsome data\nand yet even more data",
    b"CONNECT HOME0.NETSCAPE.COM:443 HTTP/1.0\r\nUser-agent: Mozilla/1.1N\r\nProxy-authorization: basic aGVsbG86d29ybGQ=\r\n\r\n",
    b"CONNECT foo.bar.com:443 HTTP/1.0\r\nUser-agent: Mozilla/1.1N\r\nProxy-authorization: basic aGVsbG86d29ybGQ=\r\nContent-Length: 10\r\n\r\nblarfcicle\"",
    b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nST: \"ssdp:all\"\r\n\r\n",
    b"PATCH /file.txt HTTP/1.1\r\nHost: www.example.com\r\nContent-Type: application/example\r\nIf-Match: \"e0023aa4e\"\r\nContent-Length: 10\r\n\r\ncccccccccc",
    b"PURGE /file.txt HTTP/1.1\r\nHost: www.example.com\r\n\r\n",
    b"SEARCH / HTTP/1.1\r\nHost: www.example.com\r\n\r\n",
    b"LINK /images/my_dog.jpg HTTP/1.1\r\nHost: example.com\r\nLink: <http://example.com/profiles/joe>; rel=\"tag\"\r\nLink: <http://example.com/profiles/sally>; rel=\"tag\"\r\n\r\n",
    b"UNLINK /images/my_dog.jpg HTTP/1.1\r\nHost: example.com\r\nLink: <http://example.com/profiles/sally>; rel=\"tag\"\r\n\r\n",
    b"SOURCE /music/sweet/music HTTP/1.1\r\nHost: example.com\r\n\r\n",
    b"SOURCE /music/sweet/music ICE/1.0\r\nHost: example.com\r\n\r\n",
    b"OPTIONS /music/sweet/music RTSP/1.0\r\nHost: example.com\r\n\r\n",
    b"ANNOUNCE /music/sweet/music RTSP/1.0\r\nHost: example.com\r\n\r\n",
    b"QUERY /contacts HTTP/1.1\r\nHost: example.org\r\nContent-Type: example/query\r\nAccept: text/csv\r\nContent-Length: 41\r\n\r\nselect surname, givenname, email limit 10",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc",
    b"PUT / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n",
    b"PUT / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\na;foo=bar\r\n0123456789\r\n0\r\n\r\n",
    b"PUT / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\na;foo=bar\r\n0123456789\r\n0\r\n\r\n",
    b"PUT / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n",
    b"POST /aaa HTTP/1.1\r\nContent-Length: 3\r\n\r\nAAA\r\nPUT /bbb HTTP/1.1\r\nContent-Length: 4\r\n\r\nBBBB\r\nPATCH /ccc HTTP/1.1\r\nContent-Length: 5\r\n\r\nCCCC",
    b"OPTIONS /url HTTP/1.1\r\nHeader1: Value1\r\nHeader2:\t Value2\r\n\r\n",
    b"HEAD /url HTTP/1.1\r\n\r\n",
    b"GET /test HTTP/1.1\r\nUser-Agent: curl/7.18.0 (i486-pc-linux-gnu) libcurl/7.18.0 OpenSSL/0.9.8g zlib/1.2.3.3 libidn/1.1\r\nHost: 0.0.0.0=5000\r\nAccept: */*\r\n\r\n",
    b"GET /favicon.ico HTTP/1.1\r\nHost: 0.0.0.0=5000\r\nUser-Agent: Mozilla/5.0 (X11; U; Linux i686; en-US; rv:1.9) Gecko/2008061015 Firefox/3.0\r\nAccept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\nAccept-Language: en-us,en;q=0.5\r\nAccept-Encoding: gzip,deflate\r\nAccept-Charset: ISO-8859-1,utf-8;q=0.7,*;q=0.7\r\nKeep-Alive: 300\r\nConnection: keep-alive\r\n\r\n",
    b"GET /dumbpack HTTP/1.1\r\naaaaaaaaaaaaa:++++++++++\r\n\r\n",
    b"GET /get_no_headers_no_body/world HTTP/1.1\r\n\r\n",
    b"GET /get_one_header_no_body HTTP/1.1\r\nAccept: */*\r\n\r\n",
    b"GET /test HTTP/1.0\r\nHost: 0.0.0.0:5000\r\nUser-Agent: ApacheBench/2.3\r\nAccept: */*\r\n\r\n",
    b"\r\nGET /test HTTP/1.1\r\n\r\n",
    b"GET /\r\n\r\n",
    b"\r\nGET /url HTTP/1.1\r\nHeader1: Value1\r\n\r\n",
    "GET / HTTP/1.1\r\nTest: DÃ¼sseldorf\r\n\r\n".as_bytes(),
    b"OPTIONS /url HTTP/1.1\r\nHeader1: Value1\r\nHeader2: \xffValue2\r\n\r\n",
    b"GET / HTTP/1.1\r\nX-SSL-Nonsense:   -----BEGIN CERTIFICATE-----\tMIIFbTCCBFWgAwIBAgICH4cwDQYJKoZIhvcNAQEFBQAwcDELMAkGA1UEBhMCVUsx\tETAPBgNVBAoTCGVTY2llbmNlMRIwEAYDVQQLEwlBdXRob3JpdHkxCzAJBgNVBAMT\tAkNBMS0wKwYJKoZIhvcNAQkBFh5jYS1vcGVyYXRvckBncmlkLXN1cHBvcnQuYWMu\tdWswHhcNMDYwNzI3MTQxMzI4WhcNMDcwNzI3MTQxMzI4WjBbMQswCQYDVQQGEwJV\tSzERMA8GA1UEChMIZVNjaWVuY2UxEzARBgNVBAsTCk1hbmNoZXN0ZXIxCzAJBgNV\tBAcTmrsogriqMWLAk1DMRcwFQYDVQQDEw5taWNoYWVsIHBhcmQYJKoZIhvcNAQEB\tBQADggEPADCCAQoCggEBANPEQBgl1IaKdSS1TbhF3hEXSl72G9J+WC/1R64fAcEF\tW51rEyFYiIeZGx/BVzwXbeBoNUK41OK65sxGuflMo5gLflbwJtHBRIEKAfVVp3YR\tgW7cMA/s/XKgL1GEC7rQw8lIZT8RApukCGqOVHSi/F1SiFlPDxuDfmdiNzL31+sL\t0iwHDdNkGjy5pyBSB8Y79dsSJtCW/iaLB0/n8Sj7HgvvZJ7x0fr+RQjYOUUfrePP\tu2MSpFyf+9BbC/aXgaZuiCvSR+8Snv3xApQY+fULK/xY8h8Ua51iXoQ5jrgu2SqR\twgA7BUi3G8LFzMBl8FRCDYGUDy7M6QaHXx1ZWIPWNKsCAwEAAaOCAiQwggIgMAwG\tA1UdEwEB/wQCMAAwEQYJYIZIAYb4QgHTTPAQDAgWgMA4GA1UdDwEB/wQEAwID6DAs\tBglghkgBhvhCAQ0EHxYdVUsgZS1TY2llbmNlIFVzZXIgQ2VydGlmaWNhdGUwHQYD\tVR0OBBYEFDTt/sf9PeMaZDHkUIldrDYMNTBZMIGaBgNVHSMEgZIwgY+AFAI4qxGj\tloCLDdMVKwiljjDastqooXSkcjBwMQswCQYDVQQGEwJVSzERMA8GA1UEChMIZVNj\taWVuY2UxEjAQBgNVBAsTCUF1dGhvcml0eTELMAkGA1UEAxMCQ0ExLTArBgkqhkiG\t9w0BCQEWHmNhLW9wZXJhdG9yQGdyaWQtc3VwcG9ydC5hYy51a4IBADApBgNVHRIE\tIjAggR5jYS1vcGVyYXRvckBncmlkLXN1cHBvcnQuYWMudWswGQYDVR0gBBIwEDAO\tBgwrBgEEAdkvAQEBAQYwPQYJYIZIAYb4QgEEBDAWLmh0dHA6Ly9jYS5ncmlkLXN1\tcHBvcnQuYWMudmT4sopwqlBWsvcHViL2NybC9jYWNybC5jcmwwPQYJYIZIAYb4QgEDBDAWLmh0\tdHA6Ly9jYS5ncmlkLXN1cHBvcnQuYWMudWsvcHViL2NybC9jYWNybC5jcmwwPwYD\tVR0fBDgwNjA0oDKgMIYuaHR0cDovL2NhLmdyaWQt5hYy51ay9wdWIv\tY3JsL2NhY3JsLmNybDANBgkqhkiG9w0BAQUFAAOCAQEAS/U4iiooBENGW/Hwmmd3\tXCy6Zrt08YjKCzGNjorT98g8uGsqYjSxv/hmi0qlnlHs+k/3Iobc3LjS5AMYr5L8\tUO7OSkgFFlLHQyC9JzPfmLCAugvzEbyv4Olnsr8hbxF1MbKZoQxUZtMVu29wjfXk\thTeApBv7eaKCWpSp7MCbvgzm74izKhu3vlDk9w6qVrxePfGgpKPqfHiOoGhFnbTK\twTC6o2xq5y0qZ03JonF7OJspEd3I5zKY3E+ov7/ZhW6DqT8UFvsAdjvQbXyhV8Eu\tYhixw1aKEPzNjNowuIseVogKOLXxWI5vAi5HgXdS0/ES5gDGsABo4fqovUKlgop3\tRA==\t-----END CERTIFICATE-----\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n",
    b"PUT /url HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nA\r\n0123456789\r\n0\r\n\r\n",
    b"POST /post_chunked_all_your_base HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1e\r\nall your base are belong to us\r\n0\r\n\r\n",
    b"POST /two_chunks_mult_zero_end HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n000\r\n\r\n",
    b"POST /chunked_w_trailing_headers HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\nVary: *\r\nContent-Type: text/plain\r\n\r\n",
    b"POST /chunked_w_unicorns_after_length HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5;ilovew3;somuchlove=aretheseparametersfor;another=withvalue\r\nhello\r\n6;blahblah;blah\r\n world\r\n0\r\n\r\n",
    b"GET /with_\"lovely\"_quotes?foo=\"bar\" HTTP/1.1\r\n\r\n",
    b"GET /test.cgi?foo=bar?baz HTTP/1.1\r\n\r\n",
    b"GET http://hypnotoad.org?hail=all HTTP/1.1\r\n\r\n",
    b"GET http://hypnotoad.org:1234?hail=all HTTP/1.1\r\n\r\n",
    b"GET /test.cgi?query=| HTTP/1.1\r\n\r\n",
    b"GET http://hypnotoad.org:1234 HTTP/1.1\r\n\r\n",
    b"GET /forums/1/topics/2375?page=1#posts-17408 HTTP/1.1\r\n\r\n",
    b"GET http://a%12:b!&*$@hypnotoad.org:1234/toto HTTP/1.1\r\n\r\n",
];

/// `response_success_case`.
const RESPONSE_SUCCESS: &[&[u8]] = &[
    b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\nContent-Length: 11\r\nProxy-Connection: close\r\nDate: Thu, 31 Dec 2009 20:55:48 +0000\r\n\r\nhello world",
    b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\n\r\nHTTP/1.0 200 OK",
    b"HTTP/1.0 204 No content\r\nConnection: keep-alive\r\n\r\nHTTP/1.0 200 OK",
    b"HTTP/1.1 200 OK\r\n\r\nHTTP/1.1 200 OK",
    b"HTTP/1.1 204 No content\r\n\r\nHTTP/1.1 200 OK",
    b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: h2c\r\nContent-Length: 4\r\n\r\nbody\\\r\nproto",
    b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: h2c\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nbo\r\n2\r\ndy\r\n0\r\n\r\nproto",
    b"HTTP/1.1 200 OK\r\nConnection: upgrade\r\nUpgrade: h2c\r\n\r\nbody",
    b"HTTP/1.1 200 OK\r\nConnection: upgrade\r\nUpgrade: h2c\r\nContent-Length: 4\r\n\r\nbody",
    b"HTTP/1.1 200 OK\r\nConnection: upgrade\r\nUpgrade: h2c\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nbo\r\n2\r\ndy\r\n0\r\n\r\n",
    b"HTTP/1.1 304 Not Modified\r\nContent-Length: 10\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
    b"HTTP/1.1 304 Not Modified\r\nTransfer-Encoding: chunked\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 14\r\nDate: Fri, 15 Sep 2023 19:47:23 GMT\r\nServer: Python/3.10 aiohttp/4.0.0a2.dev0\r\n\r\n404: Not Found",
    b"HTTP/1.1 103 Early Hints\r\nLink: </styles.css>; rel=preload; as=style\r\n\r\nHTTP/1.1 200 OK\r\nDate: Wed, 13 Sep 2023 11:09:41 GMT\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nContent-Length: 17\r\n\r\nresponse content",
    b"HTTP/1.1 200 OK\r\nDate: Tue, 04 Aug 2009 07:59:32 GMT\r\nServer: Apache\r\nX-Powered-By: Servlet/2.5 JSP/2.1\r\nContent-Type: text/xml; charset=utf-8\r\nConnection: close\r\n\r\n<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SOAP-ENV:Envelope xmlns:SOAP-ENV=\"http://schemas.xmlsoap.org/soap/envelope/\">\n  <SOAP-ENV:Body>\n    <SOAP-ENV:Fault>\n       <faultcode>SOAP-ENV:Client</faultcode>\n       <faultstring>Client Error</faultstring>\n    </SOAP-ENV:Fault>\n  </SOAP-ENV:Body>\n</SOAP-ENV:Envelope>",
    b"HTTP/1.1 200 OK\r\nContent-Length-X: 0\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nOK\r\n0\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nContent-Length: 123\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 456\r\n\r\n",
    b"HTTP/1.1 200 OK\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc",
    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\na;foo=bar\r\n0123456789\r\n0\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nAAA",
    b"HTTP/1.1 201 Created\r\nContent-Length: 4\r\n\r\nBBBB",
    b"HTTP/1.1 202 Accepted\r\nContent-Length: 5\r\n\r\nCCCC",
    b"HTTP/1.1 200 OK\r\nHeader1: Value1\r\nHeader2: Value2\r\nContent-Length: 0\r\n\r\n",
    b"RTSP/1.1 200 OK\r\n\r\n",
    b"ICE/1.1 200 OK\r\n\r\n",
    b"HTTP/1.1 200 OK\r\n\r\n",
    b"HTTP/1.1 301 Moved Permanently\r\nLocation: http://www.google.com/\r\nContent-Type: text/html; charset=UTF-8\r\nDate: Sun, 26 Apr 2009 11:11:49 GMT\r\nExpires: Tue, 26 May 2009 11:11:49 GMT\r\nX-$PrototypeBI-Version: 1.6.0.3\r\nCache-Control: public, max-age=2592000\r\nServer: gws\r\nContent-Length: 219\r\n\r\n<HTML><HEAD><meta http-equiv=content-type content=text/html;charset=utf-8>\n<TITLE>301 Moved</TITLE></HEAD><BODY>\n<H1>301 Moved</H1>\nThe document has moved\n<A HREF=\"http://www.google.com/\">here</A>.\r\n</BODY></HTML>",
    b"HTTP/1.1 301 MovedPermanently\r\nDate: Wed, 15 May 2013 17:06:33 GMT\r\nServer: Server\r\nx-amz-id-1: 0GPHKXSJQ826RK7GZEB2\r\np3p: policyref=\"http://www.amazon.com/w3c/p3p.xml\",CP=\"CAO DSP LAW CUR ADM IVAo IVDo CONo OTPo OUR DELi PUBi OTRi BUS PHY ONL UNI PUR FIN COM NAV INT DEM CNT STA HEA PRE LOC GOV OTC \"\r\nx-amz-id-2: STN69VZxIFSz9YJLbz1GDbxpbjG6Qjmmq5E3DxRhOUw+Et0p4hr7c/Q8qNcx4oAD\r\nLocation: http://www.amazon.com/Dan-Brown/e/B000AP9DSU/ref=s9_pop_gw_al1?_encoding=UTF8&refinementId=618073011&pf_rd_m=ATVPDKIKX0DER&pf_rd_s=center-2&pf_rd_r=0SHYY5BZXN3KR20BNFAY&pf_rd_t=101&pf_rd_p=1263340922&pf_rd_i=507846\r\nVary: Accept-Encoding,User-Agent\r\nContent-Type: text/html; charset=ISO-8859-1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n\n\r\n0\r\n\r\n",
    b"HTTP/1.1 404 Not Found\r\n\r\n",
    b"HTTP/1.1 301\r\n\r\n",
    b"HTTP/1.1 200 \r\n\r\n",
    b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\nthese headers are from http://news.ycombinator.com/",
    b"HTTP/1.1 200 OK\r\nServer: DCLK-AdSvr\r\nContent-Type: text/xml\r\nContent-Length: 0\r\nDCLK_imp: v7;x;114750856;0-0;0;17820020;0/0;21603567/21621457/1;;~okv=;dcmt=text/xml;;~cs=o\r\n\r\n",
    b"HTTP/1.0 301 Moved Permanently\r\nDate: Thu, 03 Jun 2010 09:56:32 GMT\r\nServer: Apache/2.2.3 (Red Hat)\r\nCache-Control: public\r\nPragma: \r\nLocation: http://www.bonjourmadame.fr/\r\nVary: Accept-Encoding\r\nContent-Length: 0\r\nContent-Type: text/html; charset=UTF-8\r\nConnection: keep-alive\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nDate: Tue, 28 Sep 2010 01:14:13 GMT\r\nServer: Apache\r\nCache-Control: no-cache, must-revalidate\r\nExpires: Mon, 26 Jul 1997 05:00:00 GMT\r\n.et-Cookie: PlaxoCS=1274804622353690521; path=/; domain=.plaxo.com\r\nVary: Accept-Encoding\r\n_eep-Alive: timeout=45\r\n_onnection: Keep-Alive\r\nTransfer-Encoding: chunked\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n0\r\n\r\n",
    b"HTTP/0.9 200 OK\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello world",
    b"HTTP/1.1 200 OK\r\nHeader1: Value1\r\nHeader2: Value2\r\nContent-Length: 0\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nAccept: */*\r\nTransfer-Encoding: chunked, deflate\r\n\r\nWorld",
    b"HTTP/1.1 200 OK\r\nAccept: */*\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: identity\r\n\r\nWorld",
    b"HTTP/1.1 200 OK\r\nAccept: */*\r\nTransfer-Encoding: chunkedchunked\r\n\r\n2\r\nOK\r\n0\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nHost: localhost\r\nTransfer-encoding: chunked\r\n\r\n5;ilovew3;somuchlove=aretheseparametersfor\r\nhello\r\n6;blahblah;blah\r\n world\r\n0\r\n\r\n",
    b"HTTP/1.1 200 OK\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5;ilovew3=\"I love; extensions\";somuchlove=\"aretheseparametersfor\";blah;foo=bar\r\nhello\r\n6;blahblah;blah\r\n world\r\n0\r\n",
];

fn success_cases(kind: c_int, cases: &[&[u8]]) {
    let mut http = Http::new(kind);
    for (i, protocol) in cases.iter().enumerate() {
        http.execute(protocol);
        assert_eq!(http.errno(), HPE_OK, "case {i}");
        assert_eq!(http.ctx.completed, 1, "case {i}");
        http.reset();
    }
}

#[test]
fn request_success_case() {
    success_cases(HTTP_REQUEST, REQUEST_SUCCESS);
}

#[test]
fn response_success_case() {
    success_cases(HTTP_RESPONSE, RESPONSE_SUCCESS);
}
