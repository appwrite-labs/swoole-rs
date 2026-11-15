//! `Swoole\Coroutine\Http\Client`: registration, the object, the client's
//! state and every operation over it (connect and keep-alive, request
//! building with multipart uploads, the response loop and llhttp's callbacks,
//! decompression, downloads, the WebSocket upgrade, push and receive). The
//! coroutine socket, files and callables are reached through
//! `crate::http_native`, as is WebSocket framing, which stays C++.
use crate::{
    http_native::{self as native, PushError, WebSocket},
    http_request::{self as request, Compress},
    native::{Co, swoole_rs_php_error},
    socket::Sock,
};
use libc::c_int;
use std::{
    borrow::Cow,
    cell::{Cell, OnceCell, RefCell, RefMut},
    ffi::{CStr, CString},
    ptr,
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_corebase::CCast;
use swoole_rs_http::llhttp::{self, Callbacks, Parser};
use swoole_rs_inflate::{Inflate, brotli, zstd};
use swoole_rs_phpstubs::http_client_coro;
use swoole_rs_zend::{
    Array, ArrayMut, Call, Class, Custom, CustomClass, Minit, ObjectRef, Owned, OwnedArray, Props,
    Value, ZStr, ZString,
    bailout::{self, Bailout},
    host::callable::Callable,
    sys::{ZEND_ACC_PUBLIC, zend_object},
};
use swoole_rs_zend::{method_table, sys as z};

unsafe extern "C" {
    /// `sw_zend_read_property_not_null_ex`: null (`None`) for a null property,
    /// or the slot of the property `object`'s class declares as the `length`
    /// bytes at `name`, which lives as long as the object.
    fn swr_zend_property_not_null<'a>(
        object: *mut z::zend_object,
        name: *const std::ffi::c_char,
        length: usize,
    ) -> Option<&'a Value>;
}

/// `swHttpErrorStatusCode`: the negative `statusCode` a failed request leaves.
#[derive(Clone, Copy)]
enum ErrorStatus {
    ConnectFailed = -1,
    RequestTimeout = -2,
    ServerReset = -3,
    SendFailed = -4,
}

impl ErrorStatus {
    const ALL: [(Self, &'static CStr); 4] = [
        (
            Self::ConnectFailed,
            c"SWOOLE_HTTP_CLIENT_ESTATUS_CONNECT_FAILED",
        ),
        (
            Self::RequestTimeout,
            c"SWOOLE_HTTP_CLIENT_ESTATUS_REQUEST_TIMEOUT",
        ),
        (
            Self::ServerReset,
            c"SWOOLE_HTTP_CLIENT_ESTATUS_SERVER_RESET",
        ),
        (Self::SendFailed, c"SWOOLE_HTTP_CLIENT_ESTATUS_SEND_FAILED"),
    ];

    const fn code(self) -> i64 {
        self as i64
    }
}
/// `SW_ERROR_*`.
const INVALID_PARAMS: c_int = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as c_int;
const NO_CONNECTION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_CLIENT_NO_CONNECTION as c_int;
const CO_HAS_BEEN_BOUND: c_int = swoole_rs_error::ErrorCode::SW_ERROR_CO_HAS_BEEN_BOUND as c_int;
const INVALID_PROTOCOL: c_int = swoole_rs_error::ErrorCode::SW_ERROR_HTTP_INVALID_PROTOCOL as c_int;
const HANDSHAKE_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_WEBSOCKET_HANDSHAKE_FAILED as c_int;
const PACK_FAILED: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WEBSOCKET_PACK_FAILED as c_int;
/// `SW_EVENT_WRITE` and `SW_EVENT_RDWR`.
const EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;
const EVENT_RDWR: c_int = (1 << 9) | (1 << 10);
/// `SW_HTTP_SWITCHING_PROTOCOLS`, `NO_CONTENT`, `BAD_GATEWAY`, `SERVICE_UNAVAILABLE`.
const SWITCHING_PROTOCOLS: u16 = 101;
const NO_CONTENT: u16 = 204;
const BAD_GATEWAY: u16 = 502;
const SERVICE_UNAVAILABLE: u16 = 503;
/// `SW_WEBSOCKET_HEADER_LEN`, `SW_WEBSOCKET_KEY_LENGTH`.
const WEBSOCKET_HEADER: usize = 2;
const WEBSOCKET_KEY: usize = 16;
/// `SW_WEBSOCKET_EXTENSION_DEFLATE`.
const DEFLATE_EXTENSION: &CStr =
    c"permessage-deflate; client_no_context_takeover; server_no_context_takeover";
/// WebSocket opcodes, flags and close code.
const OPCODE_TEXT: i64 = 1;
const OPCODE_CLOSE: i64 = 8;
const OPCODE_PING: i64 = 9;
const FLAG_FIN: i64 = 1;
const FLAGS_ALL: i64 = 63;
const CLOSE_NORMAL: i64 = 1000;
/// `SW_HTTP_RESPONSE_INIT_SIZE`.
const BODY_SIZE: usize = 65536;

/// Which compression Swoole was built with (1 zlib, 2 brotli, 4 zstd).
static FEATURES: AtomicU32 = AtomicU32::new(0);
static CLIENT: CustomClass<Data> = CustomClass::new();
static EXCEPTION: Class = Class::new();

/// `struct HttpClientObject { Client *client; zend_object std; }`: the client
/// the constructor makes. Methods, and the PHP code they run (`write_func`,
/// or another coroutine while one yields), share it, so it is only ever
/// borrowed shared and its mutable state is in cells.
#[derive(Default)]
pub(crate) struct Data {
    client: OnceCell<Client>,
}

impl Custom for Data {
    fn class() -> &'static CustomClass<Self> {
        &CLIENT
    }
    fn new() -> Self {
        Self::default()
    }
}

/// What the socket's close hook changes, shared with the hook apart from the
/// client so the hook never writes through a live `&mut Client`.
struct Conn {
    object: *mut zend_object,
    socket: Cell<Option<Co>>,
    zsocket: RefCell<Owned>,
    /// The write buffer the closed socket left (`tmp_write_buffer`).
    written: RefCell<native::Written>,
}

/// Upstream's `swoole::coroutine::http::Client`. Every method takes `&self`:
/// a `&mut` would stay live across `write_func` and socket yields, while the
/// PHP code they run calls the client's methods.
struct Client {
    object: *mut zend_object,
    conn: Rc<Conn>,
    host: Vec<u8>,
    socket_type: i64,
    port: u16,
    ssl: bool,
    use_default_port: bool,
    connect_timeout: Cell<f64>,
    response_timeout: Cell<f64>,
    max_retries: Cell<u8>,
    defer: Cell<bool>,
    lowercase_header: Cell<bool>,
    keep_alive: Cell<bool>,
    http_compression: Cell<bool>,
    body_decompression: Cell<bool>,
    websocket: Cell<bool>,
    accept_websocket_compression: Cell<bool>,
    ws: Option<Box<dyn WebSocket>>,
    /// Cloned out before each call, so `set()` from the callback can replace it.
    write_func: RefCell<Option<Rc<Callable>>>,
    basic_auth: RefCell<Vec<u8>>,
    head: Cell<bool>,
    /// The socket writes it through a pointer while it connects.
    resolve: RefCell<swoole_rs_corebase::NameResolverContext>,
    /// The last response's status, which `exec` and `upgrade` read.
    status: Cell<u16>,
    wait_response: Cell<bool>,
    body: RefCell<Vec<u8>>,
    compress: Cell<Compress>,
    compression_error: Cell<bool>,
    decoder: RefCell<Option<Decoder>>,
    connection_close: Cell<bool>,
    completed: Cell<bool>,
    event_stream: Cell<bool>,
    in_callback: Cell<bool>,
    /// The response parser; whoever borrows it owns the socket's receive.
    parser: RefCell<Parser>,
    has_upload_files: Cell<bool>,
    download_name: RefCell<Option<ZString>>,
    download_offset: Cell<i64>,
    /// The open download's descriptor.
    download_file: Cell<Option<c_int>>,
    /// The `write_func` callback bailed out; unwind and re-raise.
    bailout: Cell<bool>,
}

/// One response's parser and the header name it last saw, with the client
/// it reports to, shared: the callbacks can run `write_func`.
struct Hooks<'a> {
    parser: &'a mut Parser,
    client: &'a Client,
    field: (*const u8, usize),
}

impl Callbacks for Hooks<'_> {
    fn parser(&mut self) -> &mut Parser {
        self.parser
    }

    fn on_header_field(&mut self, data: &[u8]) -> c_int {
        self.field = (data.as_ptr(), data.len());
        0
    }

    fn on_header_value(&mut self, data: &[u8]) -> c_int {
        on_header_value(self, data)
    }

    fn on_headers_complete(&mut self) -> c_int {
        c_int::from(self.client.head.get() || self.parser.status_code == NO_CONTENT)
    }

    fn on_body(&mut self, data: &[u8]) -> c_int {
        on_body(self.client, data)
    }

    fn on_message_complete(&mut self) -> c_int {
        on_message_complete(self)
    }
}

impl Hooks<'_> {
    /// `swoole_llhttp_parser_execute`: how many bytes parsed; a pause resumes.
    fn parse(&mut self, data: &[u8]) -> usize {
        let result = llhttp::execute(self, data);
        if result == llhttp::HPE_OK {
            return data.len();
        }
        let parsed = self.parser.error_pos();
        self.parser.resume();
        self.parser.resume_after_upgrade();
        parsed
    }
}

fn features() -> u32 {
    FEATURES.load(Ordering::Relaxed)
}

/// `swoole_strerror(code)`.
fn strerror(code: c_int) -> Cow<'static, CStr> {
    swoole_rs_phpruntime::strerror(code)
}

fn c_bytes(bytes: &[u8]) -> CString {
    CString::new(bytes.split(|&b| b == 0).next().unwrap_or_default()).unwrap_or_default()
}

/// `php_swoole_error(E_WARNING, ...)`, or with `fatal` `php_swoole_fatal_error`.
fn warn(fatal: bool, message: &CStr) {
    swoole_rs_php_error(swoole_rs_zend::E_WARNING, fatal, message);
}

/// `php_swoole_sys_error(E_WARNING, "%s", message)`.
fn sys_error(message: &[u8]) {
    swoole_rs_phpswoole::sys_warning(message);
}

/// Property access on a client object.
mod props {
    use super::{CLIENT, Call};
    use swoole_rs_zend::Props;

    /// Writes on `$this`, a client method's object, under the client class;
    /// None in a static call.
    pub(super) fn this(call: &Call) -> Option<Props<'_>> {
        call.props(Some(&CLIENT))
    }
}

/// `php_swoole_socket_set_error_properties(zobject, code, message)`.
fn set_error_properties(props: Props, code: c_int, message: &CStr) {
    swoole_rs_corebase::log::set_last_error(code);
    props.long(c"errCode", i64::from(code));
    props.bytes(c"errMsg", message.to_bytes());
}

/// `php_swoole_socket_set_error_properties(zobject, socket)`.
fn socket_error_properties(props: Props, socket: Co) -> c_int {
    socket.with_error(|code, message| {
        set_error_properties(props, code, message);
        code
    })
}

/// Fill `rv` as `RETURN_ZVAL(property, 1, 0)` does.
fn return_property(call: &Call, name: &CStr) {
    if let Some(property) = call.this_value().and_then(|this| this.property(name)) {
        call.set_owned(property.dereferenced().into_owned());
    }
}

/// The socket's close hook (`Client::socket_dtor`): keep the written bytes,
/// drop the socket and say so on the object.
fn socket_closed(conn: &Conn, socket: Co) {
    // The buffer the conn took from its last socket goes.
    drop(conn.written.replace(socket.take_write_buffer()));
    conn.socket.set(None);
    conn.props().bool(c"connected", false);
    conn.props().null(c"socket");
    // Emptied before the socket object's reference goes.
    let zsocket = conn.zsocket.take();
    drop(zsocket);
}

/// `php_swoole_http_parse_set_cookies`: one `Set-Cookie` value into the
/// `cookies` map (URL-decoded) and the raw `set_cookie_headers` list.
pub fn parse_set_cookies(bytes: &[u8], cookies: &mut ArrayMut, set_cookie_headers: &mut ArrayMut) {
    let plan = swoole_rs_clients::cookie(bytes);
    let end = plan.value_offset + plan.value_length;
    let mut text = bytes
        .get(plan.value_offset..end)
        .unwrap_or_default()
        .to_vec();
    let length = swoole_rs_http::url_decode(&mut text);
    let cookie = Owned::from(ZString::new(text.get(..length).unwrap_or_default()));
    if plan.key_length == 0 {
        cookies.push(cookie);
    } else {
        cookies.insert(bytes.get(..plan.key_length).unwrap_or_default(), cookie);
    }
    set_cookie_headers.push_bytes(bytes);
}

impl Conn {
    /// Writes on the client object, which owns the client and so this conn.
    fn props(&self) -> Props<'_> {
        // SAFETY: the object outlives its client's conn; the class declares every name written.
        let object = unsafe { ObjectRef::from_ptr(self.object) };
        // Never null: `Client::new` takes a constructor's `$this`.
        let object =
            object.unwrap_or_else(|| swoole_rs_corebase::fatal("a client without its object"));
        Props::of(Some(&*CLIENT), object)
    }
}

impl Client {
    /// Writes on the client object, which owns this client.
    fn props(&self) -> Props<'_> {
        self.conn.props()
    }

    /// The property `name`, which the client class declares.
    fn property(&self, name: &CStr) -> &Value {
        let slot = self.props().slot(name);
        slot.unwrap_or_else(|| swoole_rs_corebase::fatal("an undeclared client property"))
    }

    /// `sw_zend_read_property_not_null_ex`: the declared property `name` unless it is null.
    fn not_null(&self, name: &CStr) -> Option<&Value> {
        let (name, length) = (name.as_ptr(), name.count_bytes());
        // SAFETY: the object outlives its client, as its slots do, and its class
        // declares `name`.
        unsafe { swr_zend_property_not_null(self.object, name, length) }
    }

    /// `callable($client, $data)` inside `zend_try`: whether the call succeeded, or
    /// its bailout.
    fn call_write(&self, callable: &Callable, data: &[u8]) -> Result<bool, Bailout> {
        // The string argument's reference, released when it drops.
        let data = Owned::string(ZString::new(data));
        // The object outlives its client, so the call; it is borrowed for the
        // call as upstream's `ZVAL_OBJ`.
        let this = self.props().object().copy_value();
        let mut args = [this, data.copy_value()];
        callable.call(&mut args, None, false)
    }

    /// `zval_get_long` of a declared property of the client object.
    fn get_long(&self, name: &CStr) -> i64 {
        self.property(name).long()
    }

    fn new(object: *mut zend_object, host: &[u8], port: i64, ssl: bool) -> Self {
        let use_default_port = port == 0;
        let port = if use_default_port {
            if ssl { 443 } else { 80 }
        } else {
            port
        };
        let conn = Rc::new(Conn {
            object,
            socket: Cell::new(None),
            zsocket: RefCell::default(),
            written: RefCell::default(),
        });
        let mut host = host.to_vec();
        Self {
            object,
            conn,
            socket_type: request::socket_type(&mut host),
            host,
            port: port.c_cast::<u16>(),
            ssl,
            use_default_port,
            connect_timeout: Cell::new(0.0),
            response_timeout: Cell::new(0.0),
            max_retries: Cell::new(0),
            defer: Cell::new(false),
            lowercase_header: Cell::new(true),
            keep_alive: Cell::new(true),
            http_compression: Cell::new(true),
            body_decompression: Cell::new(true),
            websocket: Cell::new(false),
            accept_websocket_compression: Cell::new(false),
            ws: native::websocket(),
            write_func: RefCell::default(),
            basic_auth: RefCell::default(),
            head: Cell::new(false),
            resolve: RefCell::default(),
            status: Cell::new(0),
            wait_response: Cell::new(false),
            body: RefCell::default(),
            compress: Cell::new(Compress::None),
            compression_error: Cell::new(false),
            decoder: RefCell::default(),
            connection_close: Cell::new(false),
            completed: Cell::new(false),
            event_stream: Cell::new(false),
            in_callback: Cell::new(false),
            parser: RefCell::new(Parser::new(llhttp::Kind::Response)),
            has_upload_files: Cell::new(false),
            download_name: RefCell::default(),
            download_offset: Cell::new(0),
            download_file: Cell::new(None),
            bailout: Cell::new(false),
        }
    }

    /// The live socket, read fresh: the close hook may have cleared it.
    fn co(&self) -> Option<Co> {
        self.conn.socket.get()
    }

    /// A reference to the socket object for the rest of a call
    /// (`SW_CLIENT_PRESERVE_SOCKET`), so a close inside it cannot free the socket.
    fn hold(&self) -> Option<Owned> {
        let zsocket = self.conn.zsocket.borrow();
        zsocket.object().map(|_| zsocket.copy())
    }

    fn set_error(&self, code: c_int, message: &CStr, status: i64) {
        self.props().long(c"errCode", i64::from(code));
        self.props().bytes(c"errMsg", message.to_bytes());
        self.props().long(c"statusCode", status);
    }

    fn set_status(&self, status: ErrorStatus) {
        self.props().long(c"statusCode", status.code());
    }

    fn set_socket_error(&self, socket: Co, status: ErrorStatus) {
        socket.with_error(|code, message| self.set_error(code, message, status.code()));
    }

    /// `is_available`: connected, else errCode says no connection.
    fn is_available(&self) -> bool {
        if !self.co().is_some_and(Co::is_connected) {
            set_error_properties(self.props(), NO_CONNECTION, &strerror(NO_CONNECTION));
            return false;
        }
        true
    }

    fn apply_setting(&self, settings: &Value, check_all: bool) {
        let Some(array) = settings.array().filter(|array| array.count() > 0) else {
            return;
        };
        if check_all {
            let get = |key: &[u8]| array.get(key);
            let truthy = Value::truthy;
            // A conversion's bailout stops the settings, as upstream's did.
            if let Some(found) = get(b"connect_timeout") {
                let Some(timeout) = found.applied_double() else {
                    return;
                };
                self.connect_timeout.set(timeout);
            }
            if let Some(found) = get(b"timeout") {
                let Some(timeout) = found.applied_double() else {
                    return;
                };
                self.response_timeout.set(timeout);
            }
            if let Some(found) = get(b"max_retries") {
                let Some(retries) = found.applied_long() else {
                    return;
                };
                let retries = retries.min(i64::from(u8::MAX));
                self.max_retries.set(retries.c_cast::<u8>());
            }
            if let Some(found) = get(b"defer") {
                self.defer.set(truthy(found));
            }
            if let Some(found) = get(b"lowercase_header") {
                self.lowercase_header.set(truthy(found));
            }
            if let Some(found) = get(b"keep_alive") {
                self.keep_alive.set(truthy(found));
            }
            if let Some(found) = get(b"http_compression") {
                self.http_compression.set(truthy(found));
            }
            if let Some(found) = get(b"body_decompression") {
                self.body_decompression.set(truthy(found));
            }
            if let Some(found) = get(b"write_func") {
                drop(self.write_func.take());
                if bailout::pending() {
                    return;
                }
                let created = Callable::new(found).unwrap_or_else(|bailed| {
                    bailed.defer();
                    None
                });
                // The old callable's destructor may have set a write_func: this one replaces it.
                drop(self.write_func.replace(created));
                if bailout::pending() {
                    return;
                }
            }
            if let Some(ws) = &self.ws {
                ws.apply(array);
            }
        }
        if let Some(socket) = self.co() {
            crate::socket_options::apply(socket, array, swoole_rs_phpswoole::SocketSettings::All);
            if !swoole_rs_zend::bailout::pending() {
                socket.skip_proxy_handshake();
            }
        }
    }

    fn set_basic_auth(&self, username: &[u8], password: &[u8]) {
        let input = [username, b":", password].concat();
        let mut encoded = vec![0_u8; input.len().div_ceil(3) * 4 + 1];
        let written = swoole_rs_base64::encode(&input, &mut encoded);
        let encoded = encoded.get(..written).unwrap_or(&encoded);
        self.basic_auth
            .replace([b"Basic ".as_slice(), encoded].concat());
    }

    fn connect(&self) -> bool {
        if self.co().is_some() {
            return true;
        }
        {
            let mut body = self.body.borrow_mut();
            if body.capacity() == 0 {
                body.reserve(BODY_SIZE);
            }
        }
        let zsocket = match native::swoole_http_client_coro_create_socket(self.socket_type) {
            Ok(zsocket) => zsocket,
            Err(error) => {
                if !bailout::pending() {
                    self.set_error(error, &strerror(error), ErrorStatus::ConnectFailed.code());
                }
                return false;
            }
        };
        let Some(socket) = Sock::of(&zsocket).and_then(Sock::co) else {
            return false;
        };
        let published = zsocket.copy();
        drop(self.conn.zsocket.replace(zsocket));
        self.conn.socket.set(Some(socket));
        if self.ssl && !socket.enable_ssl() {
            self.set_socket_error(socket, ErrorStatus::ConnectFailed);
            self.close(true);
            return false;
        }
        let settings = self.property(c"setting");
        self.apply_setting(settings, false);
        self.websocket.set(false);
        self.accept_websocket_compression.set(false);
        let conn = Rc::clone(&self.conn);
        // SAFETY: the client's resolve context outlives its socket, and no
        // borrow of it is held while the socket writes it.
        unsafe {
            socket
                .handle()
                .set_resolve_context(self.resolve.as_ptr().cast());
        }
        socket.configure_http(
            self.connect_timeout.get(),
            Box::new(move |_| socket_closed(&conn, socket)),
        );
        if !socket.connect_http(&self.host, c_int::from(self.port)) {
            self.set_socket_error(socket, ErrorStatus::ConnectFailed);
            self.close(true);
            return false;
        }
        self.props().value(c"socket", &published);
        self.props().bool(c"connected", true);
        true
    }

    fn keep_liveness(&self) -> bool {
        if !self.co().is_some_and(Co::check_liveness) {
            if let Some(socket) = self.co() {
                socket.check_bound(EVENT_RDWR);
                self.set_socket_error(socket, ErrorStatus::ServerReset);
                self.close(false);
            }
            let mut attempt = 0;
            while attempt < u32::from(self.max_retries.get()) + 1 && !bailout::pending() {
                if self.connect() {
                    return true;
                }
                attempt += 1;
            }
            return false;
        }
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "upstream's send_request, in its order"
    )]
    fn send_request(&self, path: &[u8]) -> bool {
        if path.is_empty() {
            set_error_properties(self.props(), INVALID_PARAMS, &strerror(INVALID_PARAMS));
            return false;
        }
        self.props().null(c"headers");
        self.props().null(c"set_cookie_headers");
        self.props().bytes(c"body", b"");
        if !self.keep_liveness() {
            return false;
        }
        self.props().long(c"errCode", 0);
        self.props().bytes(c"errMsg", b"");
        self.props().long(c"statusCode", 0);
        let Some(socket) = self.co() else {
            return false;
        };
        socket.check_bound(EVENT_WRITE);
        swoole_rs_corebase::log::set_last_error(0);
        self.body.borrow_mut().clear();

        let method = self.not_null(c"requestMethod");
        let headers = self.property(c"requestHeaders").array();
        // The body and files with references of their own: a multipart send yields
        // between its writes, and `setData()` or `addFile()` may replace or add to
        // them meanwhile (an add separates the shared array).
        let body = self.not_null(c"requestBody").map(Value::copy);
        let files = self.property(c"uploadFiles").copy();
        let body = body.as_deref();
        let cookies = self.property(c"cookies").array();
        let download = self.not_null(c"downloadFile");

        let host_header = headers
            .and_then(|headers| headers.find(b"Host").or_else(|| headers.find(b"host")))
            .map(Value::string);
        if let Some(download) = download {
            self.download_name.replace(Some(download.string()));
            self.download_offset.set(self.get_long(c"downloadOffset"));
        }

        let mut out = Vec::with_capacity(256);
        let method = method.map(Value::string);
        let method_name: &[u8] = match &method {
            Some(method) => method.bytes(),
            None if body.is_some() => b"POST",
            None => b"GET",
        };
        self.head.set(method_name.eq_ignore_ascii_case(b"HEAD"));
        out.extend_from_slice(method_name);
        out.push(b' ');

        if socket.proxied() {
            let host = host_header.as_ref().map_or(&self.host[..], ZString::bytes);
            request::proxy_uri(&mut out, host, path, self.port);
        } else {
            out.extend_from_slice(path);
        }
        out.extend_from_slice(b" HTTP/1.1\r\n");

        match &host_header {
            Some(host) => request::header(&mut out, b"Host", host.bytes()),
            None => request::header(
                &mut out,
                b"Host",
                &request::host_value(&self.host, self.port, self.ssl),
            ),
        }

        let compression = request::accept_encoding(features());
        let (mut has_length, mut has_connection, mut has_encoding) = (false, false, false);
        if let Some(headers) = headers {
            for (key, value) in headers.entries() {
                let Some(key) = key.filter(|_| !value.is_null()) else {
                    continue;
                };
                if key.eq_ignore_ascii_case(b"Host") {
                    continue;
                }
                if key.eq_ignore_ascii_case(b"Content-Length") {
                    has_length = true;
                    continue;
                }
                if key.eq_ignore_ascii_case(b"Accept-Encoding") {
                    if compression.is_none() {
                        warn(
                            false,
                            c"Missing a compression package, 'Accept-Encoding' is ignored",
                        );
                        continue;
                    }
                    has_encoding = true;
                }
                let text = value.string();
                request::header(&mut out, key, text.bytes());
                if key.eq_ignore_ascii_case(b"Connection") {
                    has_connection = true;
                    if text.bytes().eq_ignore_ascii_case(b"close") {
                        self.keep_alive.set(false);
                    }
                }
            }
        }
        if let Some(auth) = socket.proxy_authorization() {
            request::header(&mut out, b"Proxy-Authorization", &auth);
        }
        let auth = self.basic_auth.borrow();
        if !auth.is_empty() {
            request::header(&mut out, b"Authorization", &auth);
        }
        drop(auth);
        if !has_connection {
            let value: &[u8] = if self.keep_alive.get() {
                b"keep-alive"
            } else {
                b"closed"
            };
            request::header(&mut out, b"Connection", value);
        }
        if let Some(encoding) = compression.filter(|_| self.http_compression.get() && !has_encoding)
        {
            request::header(&mut out, b"Accept-Encoding", encoding);
        }

        if let Some(cookies) = cookies {
            out.extend_from_slice(b"Cookie: ");
            let count = cookies.count();
            let mut index = 0;
            for (key, value) in cookies.entries() {
                index += 1;
                let Some(key) = key else { continue };
                let text = value.string();
                if text.bytes().is_empty() {
                    continue;
                }
                out.extend_from_slice(key);
                out.push(b'=');
                out.extend_from_slice(ZString::url_encode(text.bytes()).bytes());
                if index < count {
                    out.extend_from_slice(b"; ");
                }
            }
            out.extend_from_slice(b"\r\n");
        }

        let files = files.array().filter(|files| files.count() > 0);
        self.has_upload_files.set(files.is_some());
        if let Some(files) = files {
            return self.send_multipart(out, body, files);
        }
        if let Some(body) = body {
            if let Some(fields) = body.array() {
                request::header(
                    &mut out,
                    b"Content-Type",
                    b"application/x-www-form-urlencoded",
                );
                if fields.count() > 0 {
                    let Some(query) = body.http_build_query() else {
                        warn(false, c"http_build_query failed");
                        return false;
                    };
                    request::content_length(&mut out, query.bytes().len());
                    out.extend_from_slice(query.bytes());
                } else {
                    request::content_length(&mut out, 0);
                }
            } else {
                let data = body.string();
                request::content_length(&mut out, data.bytes().len());
                out.extend_from_slice(data.bytes());
            }
        } else if has_length {
            request::content_length(&mut out, 0);
        } else {
            out.extend_from_slice(b"\r\n");
        }
        if !self.send(&out) {
            return self.send_failed();
        }
        self.wait_response.set(true);
        true
    }

    /// `send_request`'s multipart/form-data branch, from the headers so far.
    fn send_multipart(&self, mut out: Vec<u8>, body: Option<&Value>, files: &Array) -> bool {
        let boundary = request::boundary();
        request::multipart_type(&mut out, &boundary);
        let fields = body.and_then(Value::array);
        let field_entries = || {
            fields
                .into_iter()
                .flat_map(Array::entries)
                .filter_map(|(key, value)| key.filter(|_| !value.is_null()).map(|key| (key, value)))
        };
        let mut length = 0_usize;
        for (key, value) in field_entries() {
            let text = value.string();
            length = length.wrapping_add(request::field_length(key.len(), text.bytes().len()));
        }
        for (_, file) in files.entries() {
            let Some(part) = FilePart::sizes(file) else {
                continue;
            };
            length = length.wrapping_add(part);
        }
        request::content_length(&mut out, request::multipart_length(length));
        for (key, value) in field_entries() {
            let text = value.string();
            request::field_header(&mut out, &boundary, key);
            out.extend_from_slice(text.bytes());
            out.extend_from_slice(b"\r\n");
        }
        if !self.send(&out) {
            return self.send_failed();
        }
        for (_, file) in files.entries() {
            let Some(part) = FilePart::read(file) else {
                continue;
            };
            let header = request::file_header(&boundary, part.name, part.filename, part.kind);
            let sent = match part.source {
                Source::Memory(content) => {
                    let mut data = header;
                    data.extend_from_slice(content);
                    data.extend_from_slice(b"\r\n");
                    self.send(&data)
                }
                Source::Disk(path, offset) => {
                    self.send_raw(&header)
                        && self.sendfile(path, offset, part.size)
                        && self.send_raw(b"\r\n")
                }
            };
            if !sent {
                return self.send_failed();
            }
        }
        if !self.send_raw(&request::closing(&boundary)) {
            return self.send_failed();
        }
        self.wait_response.set(true);
        true
    }

    fn send(&self, data: &[u8]) -> bool {
        self.co().is_some_and(|socket| socket.send_buffered(data))
    }

    fn send_raw(&self, data: &[u8]) -> bool {
        self.co().is_some_and(|socket| socket.send_all(data))
    }

    fn sendfile(&self, path: &[u8], offset: i64, size: i64) -> bool {
        self.co()
            .is_some_and(|socket| socket.sendfile(path, offset, size))
    }

    fn send_failed(&self) -> bool {
        if let Some(socket) = self.co() {
            self.set_socket_error(socket, ErrorStatus::SendFailed);
        }
        self.close(true);
        false
    }

    /// `exec`: send, and unless deferred receive, retrying 502 and 503.
    fn exec(&self, path: &[u8]) -> bool {
        let Some(mut parser) = self.receiver() else {
            return false;
        };
        crate::resolver::swoole_name_resolver_context_reset(
            &mut self.resolve.borrow_mut(),
            self.use_default_port,
        );
        let mut attempt = 0;
        while attempt < u32::from(self.max_retries.get()) + 1 {
            if !self.send_request(path) {
                return false;
            }
            if self.defer.get() {
                return true;
            }
            if !self.recv_response(&mut parser, 0.0) || self.bailout.get() {
                return false;
            }
            let status = self.status.get();
            if self.max_retries.get() > 0 && matches!(status, BAD_GATEWAY | SERVICE_UNAVAILABLE) {
                self.close(true);
                attempt += 1;
                continue;
            }
            return true;
        }
        false
    }

    /// The response parser, held for a whole request or `recv()`, HTTP or
    /// WebSocket. While another holds it, a coroutine reading or writing the
    /// socket raises the bound-socket error; otherwise (`write_func`, which runs
    /// inside the parse) the caller gets none and fails.
    fn receiver(&self) -> Option<RefMut<'_, Parser>> {
        let parser = self.parser.try_borrow_mut().ok();
        if parser.is_none() {
            if let Some(socket) = self.co() {
                socket.check_bound(EVENT_RDWR);
            }
            set_error_properties(
                self.props(),
                CO_HAS_BEEN_BOUND,
                &strerror(CO_HAS_BEEN_BOUND),
            );
        }
        parser
    }

    fn recv_response(&self, parser: &mut Parser, timeout: f64) -> bool {
        if !self.wait_response.get() {
            return false;
        }
        let Some(socket) = self.co() else {
            return false;
        };
        let mut received: isize;
        let mut total = 0_usize;
        let mut header_completed = false;
        let mut header_offset = 0_usize;
        parser.reset();
        let mut hooks = Hooks {
            parser,
            client: self,
            field: (ptr::null(), 0),
        };
        let timeout = if timeout == 0.0 {
            self.response_timeout.get()
        } else {
            timeout
        };
        let mut controller = socket.read_timeout(timeout);
        let mut success = false;
        loop {
            if controller.timed_out(swoole_rs_reactor::TimeoutKind::READ) {
                break;
            }
            received = socket.recv_input();
            let (buffer, mut length, size) =
                socket.with_input(|input| (input.as_mut_ptr(), input.len(), input.size()));
            if received <= 0 {
                if received == 0 {
                    socket.set_err(libc::ECONNRESET);
                    if total > 0 && !hooks.parser.should_keep_alive() {
                        llhttp::finish(&mut hooks);
                        success = true;
                    }
                }
                break;
            }
            let mut count = received.unsigned_abs();
            if !header_completed {
                length += count;
                let ended = socket.with_input(|input| {
                    // SAFETY: the receive just wrote `count` bytes after the content.
                    unsafe { input.set_len(length) };
                    input.set_offset(0);
                    let unparsed = input.view().get(header_offset..).unwrap_or_default();
                    request::header_end(unparsed).is_some()
                });
                if !ended {
                    if length == size {
                        socket.set_err(INVALID_PROTOCOL);
                        break;
                    }
                    header_offset = length.saturating_sub(4);
                    continue;
                }
                header_completed = true;
                header_offset = 0;
                count = length;
                socket.with_input(swoole_rs_buffer::string::CppString::clear);
            }
            total += count;
            // SAFETY: the buffer's first `count` bytes are this response's data.
            let data = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), count) };
            let parsed = hooks.parse(data);
            self.status.set(hooks.parser.status_code);
            if self.bailout.get() {
                return false;
            }
            if socket
                .with_net(|net| net.read(|record| record.output.close_wait))
                .unwrap_or(false)
            {
                break;
            }
            if hooks.parser.error != llhttp::HPE_OK {
                socket.set_err(INVALID_PROTOCOL);
                break;
            }
            if self.event_stream.get() && hooks.parser.needs_eof() {
                llhttp::finish(&mut hooks);
            }
            if self.completed.get() {
                if hooks.parser.upgrade && count > parsed + WEBSOCKET_HEADER {
                    // Keep the bytes past the handshake (`reduce`).
                    #[expect(clippy::cast_possible_wrap, reason = "a buffer offset")]
                    let parsed = parsed as i64;
                    socket.with_input(|input| {
                        // SAFETY: the buffer's first `count` bytes are this response's data.
                        unsafe { input.set_len(count) };
                        input.set_offset(parsed);
                        input.reduce(parsed);
                    });
                }
                success = true;
                break;
            }
        }
        drop(controller);
        if !success {
            let code = socket_error_properties(self.props(), socket);
            let status = if code == libc::ETIMEDOUT {
                ErrorStatus::RequestTimeout
            } else {
                ErrorStatus::ServerReset
            };
            self.set_status(status);
            self.close(true);
            return false;
        }
        if self.websocket.get() {
            if !self.verify_handshake() {
                self.set_error(
                    HANDSHAKE_FAILED,
                    c"websocket handshake failed: invalid Upgrade/Connection/Sec-WebSocket-Accept headers",
                    ErrorStatus::ServerReset.code(),
                );
                self.close(true);
                return false;
            }
            socket.websocket_protocol();
        }
        if !self.websocket.get() && (!self.keep_alive.get() || self.connection_close.get()) {
            self.close(true);
        } else {
            self.reset();
        }
        true
    }

    /// `websocket_verify_server_handshake`.
    fn verify_handshake(&self) -> bool {
        fn text<'a>(array: &'a Array, key: &[u8]) -> Option<&'a [u8]> {
            array.find(key).and_then(Value::bytes)
        }
        let request = self.property(c"requestHeaders").array();
        let response = self.property(c"headers").array();
        let (Some(request), Some(response)) = (request, response) else {
            return false;
        };
        let Some(key) = text(request, b"Sec-WebSocket-Key").filter(|key| !key.is_empty()) else {
            return false;
        };
        let upgrade = text(response, b"upgrade");
        if upgrade.is_none_or(|value| !request::token_list_contains(value, b"websocket")) {
            return false;
        }
        let connection = text(response, b"connection");
        if connection.is_none_or(|value| !request::token_list_contains(value, b"Upgrade")) {
            return false;
        }
        let Some(accept) =
            text(response, b"sec-websocket-accept").filter(|accept| !accept.is_empty())
        else {
            return false;
        };
        swoole_rs_clients::ws::accept_valid(key, accept)
    }

    fn recv_websocket_frame(&self, call: &Call, timeout: f64) {
        let Some(socket) = self.co() else {
            return call.set_bool(false);
        };
        let Some(ws) = &self.ws else {
            return call.set_bool(false);
        };
        let frame = ws.recv(socket, timeout);
        let kind = frame.kind();
        let empty = frame.bytes().is_some_and(<[u8]>::is_empty);
        call.set_owned(frame);
        if bailout::pending() {
            return;
        }
        if empty {
            self.close(true);
            return;
        }
        if kind == swoole_rs_zend::sys::IS_FALSE {
            let code = socket_error_properties(self.props(), socket);
            self.set_status(ErrorStatus::ServerReset);
            if code != libc::ETIMEDOUT {
                self.close(true);
            }
            return;
        }
        if kind == swoole_rs_zend::sys::IS_NULL {
            call.set_bool(false);
        }
    }

    fn upgrade(&self, path: &[u8]) -> bool {
        self.defer.set(false);
        self.props().bytes(c"requestMethod", b"GET");
        let token = request::token::<WEBSOCKET_KEY>();
        let mut key = vec![0_u8; WEBSOCKET_KEY.div_ceil(3) * 4 + 1];
        let written = swoole_rs_base64::encode(&token, &mut key);
        self.props().edit_array(c"requestHeaders", |headers| {
            headers.insert_bytes(b"Connection", Some(b"Upgrade"));
            headers.insert_bytes(b"Upgrade", Some(b"websocket"));
            headers.insert_bytes(b"Sec-WebSocket-Version", Some(b"13"));
            headers.insert_bytes(
                b"Sec-WebSocket-Key",
                Some(key.get(..written).unwrap_or(&key)),
            );
            if features() & 1 != 0 && self.ws.as_ref().is_some_and(|ws| ws.compression()) {
                headers.insert_bytes(
                    b"Sec-Websocket-Extensions",
                    Some(DEFLATE_EXTENSION.to_bytes()),
                );
            }
        });
        if !self.exec(path) {
            return false;
        }
        let status = self.status.get();
        if status != SWITCHING_PROTOCOLS || !self.websocket.get() {
            self.set_error(
                HANDSHAKE_FAILED,
                c"websocket handshake failed: expected HTTP 101 with valid WebSocket upgrade headers",
                i64::from(status),
            );
            self.close(true);
            return false;
        }
        true
    }

    fn push(&self, data: &Value, opcode: i64, flags: i64, code: i64) -> bool {
        if !self.websocket.get() {
            swoole_rs_corebase::log::set_last_error(HANDSHAKE_FAILED);
            warn(true, c"websocket handshake failed, cannot push data");
            let last = swoole_rs_corebase::log::last_error();
            self.props().long(c"errCode", i64::from(last));
            self.props()
                .bytes(c"errMsg", b"websocket handshake failed, cannot push data");
            self.set_status(ErrorStatus::ConnectFailed);
            return false;
        }
        let Some(socket) = self.co() else {
            return false;
        };
        let flags = c_int::from(flags.to_le_bytes()[0]);
        let result = self.ws.as_ref().map_or(Err(PushError::Pack), |ws| {
            ws.push(
                socket,
                data,
                opcode,
                flags,
                code,
                self.accept_websocket_compression.get(),
            )
        });
        match result {
            Ok(()) => true,
            Err(PushError::Pack) if bailout::pending() => false,
            Err(PushError::Pack) => {
                swoole_rs_corebase::log::set_last_error(PACK_FAILED);
                let last = swoole_rs_corebase::log::last_error();
                self.props().long(c"errCode", i64::from(last));
                self.props()
                    .bytes(c"errMsg", b"websocket frame pack failed");
                false
            }
            Err(PushError::Send) => {
                socket_error_properties(self.props(), socket);
                self.set_status(ErrorStatus::ServerReset);
                self.close(true);
                false
            }
        }
    }

    fn reset(&self) {
        self.wait_response.set(false);
        self.completed.set(false);
        self.event_stream.set(false);
        self.compress.set(Compress::None);
        self.compression_error.set(false);
        drop(self.decoder.take());
        if self.has_upload_files.get() {
            self.props().null(c"uploadFiles");
        }
        // The close can yield, and another coroutine's fatal error can free
        // the client meanwhile, so the client lets go of the file first.
        if let Some(fd) = self.download_file.take() {
            drop(self.download_name.take());
            self.download_offset.set(0);
            self.props().null(c"downloadFile");
            self.props().long(c"downloadOffset", 0);
            native::swoole_http_client_coro_file_close(fd);
        }
    }

    /// `close`: the socket's members must not be read after its close, which
    /// can free it (the close hook clears `socket`).
    fn close(&self, should_be_reset: bool) -> bool {
        let Some(socket) = self.co() else {
            return false;
        };
        if self.in_callback.get() {
            socket.with_net(|net| net.edit(|record| record.output.close_wait = true));
            return true;
        }
        self.props().bool(c"connected", false);
        if !socket.close() {
            socket_error_properties(self.props(), socket);
            return false;
        }
        if should_be_reset {
            self.reset();
        }
        true
    }

    /// `decompress_response`: decode a body chunk onto `body`.
    fn decompress(&self, input: &[u8]) -> bool {
        if input.is_empty() {
            return false;
        }
        let mut decoder = self.decoder.borrow_mut();
        if decoder.is_none() {
            let compress = self.compress.get();
            let made = match compress {
                Compress::Gzip | Compress::Deflate => {
                    Ok(Decoder::Zlib(Inflate::new(compress == Compress::Gzip)))
                }
                Compress::Br => brotli::Decoder::new().map(Decoder::Brotli),
                Compress::Zstd => zstd::Decoder::new().map(Decoder::Zstd),
                Compress::None => Err(format!(
                    "HttpClient::decompress_response unknown compress method [{}]",
                    compress as c_int
                )),
            };
            match made {
                Ok(made) => *decoder = Some(made),
                Err(message) => {
                    native::swoole_http_client_coro_inflate_warning(&message);
                    return false;
                }
            }
        }
        let output = match decoder.as_mut() {
            Some(Decoder::Zlib(inflate)) => inflate
                .decode(input)
                .ok_or_else(|| "HttpClient::decompress_response failed".to_owned()),
            Some(Decoder::Brotli(decoder)) => decoder.decode(input),
            Some(Decoder::Zstd(decoder)) => decoder.decode(input),
            None => return false,
        };
        match output {
            Ok(output) => {
                self.body.borrow_mut().extend_from_slice(output);
                true
            }
            Err(message) => {
                native::swoole_http_client_coro_inflate_warning(&message);
                false
            }
        }
    }

    fn add_header(&self, key: &[u8], data: &[u8]) {
        self.props().edit_array(c"headers", |headers| {
            headers.merge(key, Owned::from(ZString::new(data)));
        });
    }
}

/// A response's decoder, made on its first compressed chunk.
enum Decoder {
    Zlib(Inflate),
    Brotli(brotli::Decoder),
    Zstd(zstd::Decoder),
}

impl Drop for Client {
    /// Runs to the end even when a destructor the callable's release runs
    /// bails out; that bailout stays pending for the `free` handler to raise.
    fn drop(&mut self) {
        self.close(true);
        drop(self.write_func.take());
        if let Some(fd) = self.download_file.take() {
            native::swoole_http_client_coro_file_close(fd);
        }
        // Emptied first, as the close hook does, in case releasing it runs the
        // hook; the conn, with its saved write buffer, goes with the client.
        let zsocket = self.conn.zsocket.take();
        drop(zsocket);
    }
}

/// An upload file's part: from memory (`addData`) or disk (`addFile`).
enum Source<'a> {
    Memory(&'a [u8]),
    Disk(&'a [u8], i64),
}

struct FilePart<'a> {
    name: &'a [u8],
    filename: &'a [u8],
    kind: &'a [u8],
    size: i64,
    source: Source<'a>,
}

impl<'a> FilePart<'a> {
    /// `name`, `filename`, `size` and `type` of an upload entry, as the first
    /// pass reads them: its part's size.
    fn sizes(file: &'a Value) -> Option<usize> {
        let file = file.array()?;
        let name = Self::field(file, b"name")?;
        let filename = Self::field(file, b"filename")?;
        let size = Self::field(file, b"size")?;
        let kind = Self::field(file, b"type")?;
        Some(request::file_length(
            Self::text(name).len(),
            Self::text(filename).len(),
            Self::text(kind).len(),
            Self::number(size),
        ))
    }

    /// The second pass's reading: content, or path and offset, then size and type.
    fn read(file: &'a Value) -> Option<Self> {
        let file = file.array()?;
        let name = Self::text(Self::field(file, b"name")?);
        let filename = Self::text(Self::field(file, b"filename")?);
        let source = if let Some(content) = Self::field(file, b"content") {
            Source::Memory(Self::text(content))
        } else {
            let path = Self::field(file, b"path")?;
            let offset = Self::field(file, b"offset")?;
            Source::Disk(Self::text(path), Self::number(offset))
        };
        let size = Self::number(Self::field(file, b"size")?);
        let kind = Self::text(Self::field(file, b"type")?);
        Some(Self {
            name,
            filename,
            kind,
            size,
            source,
        })
    }

    fn field(file: &'a Array, key: &[u8]) -> Option<&'a Value> {
        file.find(key)
    }

    /// `Z_STRVAL_P`/`Z_STRLEN_P`, which upstream assumes; a non-string is empty.
    fn text(value: &'a Value) -> &'a [u8] {
        value.bytes().unwrap_or_default()
    }

    /// `Z_LVAL_P`, which upstream assumes; a non-int is 0.
    fn number(value: &Value) -> i64 {
        if value.kind() == swoole_rs_zend::sys::IS_LONG {
            value.long()
        } else {
            0
        }
    }
}

fn on_header_value(hooks: &Hooks, data: &[u8]) -> c_int {
    let client = hooks.client;
    let (field, field_length) = hooks.field;
    // SAFETY: llhttp's span points into the buffer being parsed.
    let name = unsafe { std::slice::from_raw_parts(field, field_length) };
    let lowered;
    let name = if client.lowercase_header.get() {
        lowered = name.to_ascii_lowercase();
        &lowered[..]
    } else {
        name
    };
    client.add_header(name, data);
    let status = hooks.parser.status_code;
    if status == SWITCHING_PROTOCOLS && name == b"upgrade" {
        if request::token_list_contains(data, b"websocket") {
            client.websocket.set(true);
        }
    } else if features() & 1 != 0
        && client.websocket.get()
        && client.ws.as_ref().is_some_and(|ws| ws.compression())
        && name == b"sec-websocket-extensions"
    {
        if request::contains_ignore_case(data, b"permessage-deflate") {
            client.accept_websocket_compression.set(true);
        }
    } else if name == b"set-cookie" {
        let props = client.props();
        props.edit_array(c"cookies", |cookies| {
            props.edit_array(c"set_cookie_headers", |headers| {
                parse_set_cookies(data, cookies, headers);
            })
        });
    } else if features() != 0 && name == b"content-encoding" {
        if let Some(method) = request::content_encoding(data, features()) {
            client.compress.set(method);
        }
    } else if name == b"connection" {
        client
            .connection_close
            .set(request::starts_with_ignore_case(data, b"close"));
    } else if name == b"content-type" {
        client
            .event_stream
            .set(request::starts_with_ignore_case(data, b"text/event-stream"));
    }
    0
}

fn on_body(client: &Client, data: &[u8]) -> c_int {
    let decompress = client.body_decompression.get()
        && !client.compression_error.get()
        && client.compress.get() != Compress::None;
    let write_func = client.write_func.borrow().clone();
    if let Some(callable) = write_func {
        // The decoded chunk leaves the client for the call, which can reenter it.
        let decoded = if !decompress {
            None
        } else if client.decompress(data) {
            let decoded = client.body.take();
            if decoded.is_empty() {
                client.body.replace(decoded);
                return 0;
            }
            Some(decoded)
        } else {
            client.compression_error.set(true);
            None
        };
        let chunk = decoded.as_deref().unwrap_or(data);
        client.in_callback.set(true);
        let result = client.call_write(&callable, chunk);
        // Cleared after the call, keeping the buffer.
        match decoded {
            Some(mut decoded) => {
                decoded.clear();
                drop(client.body.replace(decoded));
            }
            None => client.body.borrow_mut().clear(),
        }
        client.in_callback.set(false);
        if result.is_err() {
            client.bailout.set(true);
        }
        return if matches!(result, Ok(true)) { 0 } else { -1 };
    }
    if decompress {
        if !client.decompress(data) {
            client.compression_error.set(true);
            client.body.borrow_mut().extend_from_slice(data);
        }
    } else {
        client.body.borrow_mut().extend_from_slice(data);
    }
    if client.download_name.borrow().is_none() {
        return 0;
    }
    // Out of the client while the file calls yield to other coroutines.
    let mut body = client.body.take();
    let written = body.is_empty() || write_download(client, &body);
    if written {
        body.clear();
    }
    drop(client.body.replace(body));
    if written { 0 } else { -1 }
}

/// Append a body chunk to the download, opening its file first: whether it was written.
fn write_download(client: &Client, body: &[u8]) -> bool {
    if client.download_file.get().is_none() {
        let name = client.download_name.borrow();
        let path = c_bytes(name.as_ref().map_or(&[][..], ZString::bytes));
        drop(name);
        let offset = client.download_offset.get();
        client
            .download_file
            .set(native::swoole_http_client_coro_file_open(&path, offset));
    }
    client
        .download_file
        .get()
        .is_some_and(|fd| native::swoole_http_client_coro_file_write(fd, body))
}

fn on_message_complete(hooks: &mut Hooks) -> c_int {
    let client = hooks.client;
    client.completed.set(true);
    if hooks.parser.upgrade && !client.websocket.get() {
        hooks.parser.upgrade = false;
        return llhttp::HPE_PAUSED;
    }
    client
        .props()
        .long(c"statusCode", i64::from(hooks.parser.status_code));
    if client.download_file.get().is_none() {
        // Out of the client while the write can run PHP code.
        let body = client.body.take();
        client.props().bytes(c"body", &body);
        drop(client.body.replace(body));
    } else {
        drop(client.download_name.take());
    }
    llhttp::HPE_PAUSED
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_http_client_coro___construct, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_http_client_coro___destruct, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client___destruct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_http_client_coro_set, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_set, z::ZEND_ACC_PUBLIC;
        c"getDefer" => zim_swoole_http_client_coro_getDefer, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getDefer, z::ZEND_ACC_PUBLIC;
        c"setDefer" => zim_swoole_http_client_coro_setDefer, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_setDefer, z::ZEND_ACC_PUBLIC;
        c"setMethod" => zim_swoole_http_client_coro_setMethod, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_setMethod, z::ZEND_ACC_PUBLIC;
        c"setHeaders" => zim_swoole_http_client_coro_setHeaders, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_setHeaders, z::ZEND_ACC_PUBLIC;
        c"setBasicAuth" => zim_swoole_http_client_coro_setBasicAuth, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_setBasicAuth, z::ZEND_ACC_PUBLIC;
        c"setCookies" => zim_swoole_http_client_coro_setCookies, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_setCookies, z::ZEND_ACC_PUBLIC;
        c"setData" => zim_swoole_http_client_coro_setData, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_setData, z::ZEND_ACC_PUBLIC;
        c"addFile" => zim_swoole_http_client_coro_addFile, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_addFile, z::ZEND_ACC_PUBLIC;
        c"addData" => zim_swoole_http_client_coro_addData, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_addData, z::ZEND_ACC_PUBLIC;
        c"execute" => zim_swoole_http_client_coro_execute, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_execute, z::ZEND_ACC_PUBLIC;
        c"getpeername" => zim_swoole_http_client_coro_getpeername, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getpeername, z::ZEND_ACC_PUBLIC;
        c"getsockname" => zim_swoole_http_client_coro_getsockname, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getsockname, z::ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_http_client_coro_get, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_get, z::ZEND_ACC_PUBLIC;
        c"post" => zim_swoole_http_client_coro_post, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_post, z::ZEND_ACC_PUBLIC;
        c"download" => zim_swoole_http_client_coro_download, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_download, z::ZEND_ACC_PUBLIC;
        c"getBody" => zim_swoole_http_client_coro_getBody, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getBody, z::ZEND_ACC_PUBLIC;
        c"getHeaders" => zim_swoole_http_client_coro_getHeaders, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getHeaders, z::ZEND_ACC_PUBLIC;
        c"getCookies" => zim_swoole_http_client_coro_getCookies, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getCookies, z::ZEND_ACC_PUBLIC;
        c"getStatusCode" => zim_swoole_http_client_coro_getStatusCode, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getStatusCode, z::ZEND_ACC_PUBLIC;
        c"getHeaderOut" => zim_swoole_http_client_coro_getHeaderOut, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getHeaderOut, z::ZEND_ACC_PUBLIC;
        c"getPeerCert" => zim_swoole_http_client_coro_getPeerCert, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_getPeerCert, z::ZEND_ACC_PUBLIC;
        c"upgrade" => zim_swoole_http_client_coro_upgrade, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_upgrade, z::ZEND_ACC_PUBLIC;
        c"push" => zim_swoole_http_client_coro_push, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_push, z::ZEND_ACC_PUBLIC;
        c"recv" => zim_swoole_http_client_coro_recv, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_recv, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_http_client_coro_close, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_close, z::ZEND_ACC_PUBLIC;
        c"ping" => zim_swoole_http_client_coro_ping, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_ping, z::ZEND_ACC_PUBLIC;
        c"disconnect" => zim_swoole_http_client_coro_disconnect, &http_client_coro::arginfo_class_Swoole_Coroutine_Http_Client_disconnect, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT, as `php_swoole_http_client_coro_minit` did: the class (and short
/// name), the `SensitiveParameter` on `setBasicAuth`'s password, the
/// properties, the exception class and the error-status constants.
/// # Errors
/// A registration bailed out.
pub fn http_client_minit(minit: &Minit) -> Result<(), Bailout> {
    let shortname = swoole_rs_phpswoole::globals().use_shortname.get();
    FEATURES.store(swoole_rs_phpswoole::codecs(), Ordering::Relaxed);
    minit.register_object::<Data>(c"Swoole\\Coroutine\\Http\\Client", METHODS.table(), 0);
    if shortname {
        minit.alias(&CLIENT, c"Co\\Http\\Client")?;
    }
    minit.sensitive_parameter(&CLIENT, c"setbasicauth", 1);
    let public = ZEND_ACC_PUBLIC;
    minit.property_null(&CLIENT, c"socket", public);
    minit.property_long(&CLIENT, c"errCode", 0, public);
    minit.property_string(&CLIENT, c"errMsg", c"", public);
    minit.property_bool(&CLIENT, c"connected", false, public);
    minit.property_string(&CLIENT, c"host", c"", public);
    minit.property_long(&CLIENT, c"port", 0, public);
    minit.property_bool(&CLIENT, c"ssl", false, public);
    for name in [
        c"setting",
        c"requestMethod",
        c"requestHeaders",
        c"requestBody",
        c"uploadFiles",
        c"downloadFile",
    ] {
        minit.property_null(&CLIENT, name, public);
    }
    minit.property_long(&CLIENT, c"downloadOffset", 0, public);
    minit.property_long(&CLIENT, c"statusCode", 0, public);
    for name in [c"headers", c"set_cookie_headers", c"cookies"] {
        minit.property_null(&CLIENT, name, public);
    }
    minit.property_string(&CLIENT, c"body", c"", public);
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Coroutine\\Http\\Client\\Exception",
        swoole_rs_zend::arginfo::Table::NONE,
        minit.parent(swoole_rs_phpswoole::exception()),
        |_, _| {},
    );
    if shortname {
        minit.alias(&EXCEPTION, c"Co\\Http\\Client\\Exception")?;
    }
    for (status, name) in ErrorStatus::ALL {
        minit.long_constant(name, status.code());
    }
    Ok(())
}

/// `http_client_coro_get_client`: the constructed client; else Swoole's fatal
/// error, and the method returns.
fn client(call: &Call) -> Option<&Client> {
    let client = call.custom::<Data>().and_then(|data| data.client.get());
    if client.is_none() {
        crate::native::unconstructed();
    }
    client
}

/// `__construct(string $host, int $port = 0, bool $ssl = false)`.
fn construct(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.bool(3, false)?)));
    let Ok((host, port, ssl)) = parsed else {
        return call.set_bool(false);
    };
    let object = call.this();
    let Some(data) = call.custom::<Data>() else {
        return call.set_bool(false);
    };
    if data.client.get().is_some() {
        call.throw_constructed_twice();
        return call.set_bool(false);
    }
    if let Some(props) = props::this(call) {
        props.bytes(c"host", host);
        props.long(c"port", port);
        props.bool(c"ssl", ssl);
    }
    if host.is_empty() {
        call.throw(&EXCEPTION, c"host is empty", i64::from(libc::EINVAL));
        return call.set_bool(false);
    }
    drop(data.client.set(Client::new(object, host, port, ssl)));
}

fn destruct(_call: &Call) {}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        return call.set_bool(false);
    };
    let Some(settings) = call.value(1).filter(|_| array.count() > 0) else {
        return call.set_bool(false);
    };
    crate::net::merge_setting(call, &CLIENT, settings);
    client.apply_setting(settings, true);
    call.set_bool(true);
}

fn get_defer(call: &Call) {
    if let Some(client) = client(call) {
        call.set_bool(client.defer.get());
    }
}

/// `setDefer(bool $defer = true): bool`.
fn set_defer(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Ok(defer) = call.arity(0, 1).and_then(|()| call.bool(1, true)) else {
        return call.set_bool(false);
    };
    client.defer.set(defer);
    call.set_bool(true);
}

/// `setMethod(string $method): bool`.
fn set_method(call: &Call) {
    let Ok(method) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    if let Some(props) = props::this(call) {
        props.bytes(c"requestMethod", method);
    }
    call.set_bool(true);
}

/// `Z_PARAM_ARRAY_EX(.., 0, 1)` then a property update: the stored array is
/// the argument, separated.
fn set_array_property(call: &Call, name: &CStr) {
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: an array argument the frame owns.
    unsafe { swoole_rs_zend::sys::swr_zend_separate_array(array.as_ptr()) };
    if let Some(props) = props::this(call) {
        props.value(name, array);
    }
    call.set_bool(true);
}

fn set_headers(call: &Call) {
    set_array_property(call, c"requestHeaders");
}

fn set_cookies(call: &Call) {
    set_array_property(call, c"cookies");
}

/// `setBasicAuth(string $username, string $password): void`.
fn set_basic_auth(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.string(2, b"")?)));
    let Ok((username, password)) = parsed else {
        return call.set_bool(false);
    };
    client.set_basic_auth(username, password);
}

/// `setData(array|string $data): bool`.
fn set_data(call: &Call) {
    let Some(data) = call.arity(1, 1).ok().and(call.value(1)) else {
        return call.set_bool(false);
    };
    if let Some(props) = props::this(call) {
        props.value(c"requestBody", data);
    }
    call.set_bool(true);
}

/// The upload entry `addFile` and `addData` append.
fn add_upload(call: &Call, entry: &[(&CStr, Upload<'_>)]) {
    let mut file = OwnedArray::default();
    for (key, item) in entry {
        match item {
            Upload::Text(text) => file.insert_bytes(key.to_bytes(), Some(text)),
            Upload::Number(number) => file.insert_long(key.to_bytes(), *number),
        }
    }
    let Some(props) = props::this(call) else {
        return call.set_bool(false);
    };
    // The next-index insert fails once the array's next key would pass
    // `ZEND_LONG_MAX`; upstream returns false then.
    let count = |files: &ArrayMut| files.array().map_or(0, Array::count);
    let added = props.edit_array(c"uploadFiles", |files| {
        let before = count(files);
        files.push(file.into_owned());
        count(files) > before
    });
    call.set_bool(added.unwrap_or(false));
}

enum Upload<'a> {
    Text(&'a [u8]),
    Number(i64),
}

/// `addFile(string $path, string $name, ?string $type = null, ?string $filename = null, int $offset = 0, int $length = 0): bool`.
fn add_file(call: &Call) {
    let parsed = call.arity(2, 6).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.string(2, b"")?,
            call.string(3, b"")?,
            (call.num_args() >= 4)
                .then(|| call.string(4, b""))
                .transpose()?,
            call.long(5, 0)?,
            call.long(6, 0)?,
        ))
    });
    let Ok((path, name, mut kind, filename, offset, length)) = parsed else {
        return call.set_bool(false);
    };
    let offset = offset.max(0);
    let mut length = length.max(0);
    let c_path = c_bytes(path);
    let Some(stat) = swoole_rs_corebase::sys::stat(&c_path) else {
        let message = [&b"stat("[..], c_path.to_bytes(), b") failed"].concat();
        sys_error(&message);
        return call.set_bool(false);
    };
    let size = stat.st_size;
    if size == 0 {
        let shown = filename.map_or(&b"(null)"[..], |filename| filename);
        let message = [&b"cannot send empty file["[..], shown, b"]"].concat();
        sys_error(&message);
        return call.set_bool(false);
    }
    if size <= offset {
        let message = format!("parameter $offset[{offset}] exceeds the file size");
        warn(false, &CString::new(message).unwrap_or_default());
        return call.set_bool(false);
    }
    if length > size - offset {
        let message = format!("parameter $length[{length}] exceeds the file size");
        sys_error(message.as_bytes());
        return call.set_bool(false);
    }
    if length == 0 {
        length = size - offset;
    }
    let guessed;
    if kind.is_empty() {
        guessed = swoole_rs_phpswoole::mime_type(c_path.to_bytes());
        kind = &guessed;
    }
    let filename = if let Some(filename) = filename.filter(|filename| !filename.is_empty()) {
        filename
    } else {
        let path = c_path.to_bytes();
        path.iter()
            .rposition(|&b| b == b'/')
            .and_then(|slash| path.get(slash + 1..))
            .unwrap_or(path)
    };
    add_upload(
        call,
        &[
            (c"path", Upload::Text(path)),
            (c"name", Upload::Text(name)),
            (c"filename", Upload::Text(filename)),
            (c"type", Upload::Text(kind)),
            (c"size", Upload::Number(length)),
            (c"offset", Upload::Number(offset)),
        ],
    );
}

/// `addData(string $data, string $name, ?string $type = null, ?string $filename = null): bool`.
fn add_data(call: &Call) {
    let parsed = call.arity(2, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.string(2, b"")?,
            call.string(3, b"")?,
            call.string(4, b"")?,
        ))
    });
    let Ok((data, name, kind, filename)) = parsed else {
        return call.set_bool(false);
    };
    let kind = if kind.is_empty() {
        &b"application/octet-stream"[..]
    } else {
        kind
    };
    let filename = if filename.is_empty() { name } else { filename };
    #[expect(clippy::cast_possible_wrap, reason = "a string length")]
    let size = data.len() as i64;
    add_upload(
        call,
        &[
            (c"content", Upload::Text(data)),
            (c"name", Upload::Text(name)),
            (c"filename", Upload::Text(filename)),
            (c"type", Upload::Text(kind)),
            (c"size", Upload::Number(size)),
        ],
    );
}

/// The result of a method that can run `write_func`: whether it bailed out.
fn finish(client: &Client, call: &Call, result: bool) -> bool {
    let bailout = client.bailout.take();
    if !bailout {
        call.set_bool(result);
    }
    bailout
}

/// The path argument of `execute`, `get`, `upgrade`.
fn path_arg(call: &Call) -> Option<&[u8]> {
    let path = call.arity(1, 1).and_then(|()| call.string(1, b"")).ok();
    if path.is_none() {
        call.set_bool(false);
    }
    path
}

/// The last step of a method that can run PHP: a bailout its `write_func`
/// callback left goes on, as any other deferred bailout, once the handler's
/// frames are gone.
fn settle(bailed: bool) {
    if bailed {
        Bailout.defer();
    }
}

fn execute(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Some(path) = path_arg(call) else {
        return;
    };
    let result = client.exec(path);
    settle(finish(client, call, result));
}

fn get(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Some(path) = path_arg(call) else {
        return;
    };
    client.props().bytes(c"requestMethod", b"GET");
    let result = client.exec(path);
    settle(finish(client, call, result));
}

/// `post(string $path, mixed $data): bool`.
fn post(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Ok(path) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    client.props().bytes(c"requestMethod", b"POST");
    if let Some(data) = call.value(2) {
        client.props().value(c"requestBody", data);
    }
    let result = client.exec(path);
    settle(finish(client, call, result));
}

/// `download(string $path, string $file, int $offset = 0): bool`.
fn download(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(3, 0)?)));
    let Ok((path, offset)) = parsed else {
        call.set_bool(false);
        return;
    };
    if let Some(file) = call.value(2) {
        client.props().value(c"downloadFile", file);
    }
    client.props().long(c"downloadOffset", offset);
    let result = client.exec(path);
    settle(finish(client, call, result));
}

fn upgrade(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Some(path) = path_arg(call) else {
        return;
    };
    let result = client.upgrade(path);
    settle(finish(client, call, result));
}

/// `recv(float $timeout = 0)`: a WebSocket frame, or the deferred response.
fn recv(call: &Call) {
    settle(receive(call));
}

/// [`recv`]: whether `write_func` bailed out.
fn receive(call: &Call) -> bool {
    let Some(client) = client(call) else {
        return false;
    };
    if !client.is_available() {
        call.set_bool(false);
        return false;
    }
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return false;
    };
    let _hold = client.hold();
    let Some(mut parser) = client.receiver() else {
        return finish(client, call, false);
    };
    if client.websocket.get() {
        client.recv_websocket_frame(call, timeout);
        return false;
    }
    let result = client.recv_response(&mut parser, timeout);
    finish(client, call, result)
}

/// `push(mixed $data, int $opcode = TEXT, int $flags = FIN): bool`.
fn push(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let parsed = call.arity(1, 3).and_then(|()| call.long(2, OPCODE_TEXT));
    let Ok(opcode) = parsed else {
        return call.set_bool(false);
    };
    let flags = call
        .value(3)
        .filter(|flags| !flags.is_null())
        .map_or(FLAG_FIN, Value::long);
    let Some(data) = call.value(1) else {
        return call.set_bool(false);
    };
    let _hold = client.hold();
    let result = client.push(data, opcode, flags & FLAGS_ALL, CLOSE_NORMAL);
    call.set_bool(result);
}

/// `close(): bool`.
fn close(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    let Some(_hold) = client.hold() else {
        // A failed request already closed its socket; keep its error.
        if client.get_long(c"errCode") == 0 {
            set_error_properties(client.props(), NO_CONNECTION, &strerror(NO_CONNECTION));
        }
        return call.set_bool(false);
    };
    let result = client.close(true);
    call.set_bool(result);
}

fn get_body(call: &Call) {
    return_property(call, c"body");
}

fn get_headers(call: &Call) {
    return_property(call, c"headers");
}

fn get_cookies(call: &Call) {
    return_property(call, c"cookies");
}

fn get_status_code(call: &Call) {
    return_property(call, c"statusCode");
}

/// `getHeaderOut(): string|false`: the request head last written.
fn get_header_out(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    // The buffer the socket's close left, which the conn keeps until the next close.
    let written = client.conn.written.borrow();
    let respond = |data: &[u8]| match request::header_end(data) {
        Some(end) if end > 0 => call.set_bytes(data.get(..end).unwrap_or(data)),
        _ => call.set_bool(false),
    };
    match (client.co(), written.buffer()) {
        (Some(socket), _) => socket.with_write_buffer(|buffer| respond(buffer.view())),
        (None, Some(buffer)) => respond(buffer.view()),
        (None, None) => call.set_bool(false),
    }
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let Some(socket) = client.co() else {
        return call.set_bool(false);
    };
    if !socket.getsockname() {
        socket_error_properties(client.props(), socket);
        return call.set_bool(false);
    }
    let (address, port) = socket.address();
    set_endpoint(call, &address, port);
}

fn set_endpoint(call: &Call, address: &CStr, port: c_int) {
    let mut endpoint = OwnedArray::default();
    endpoint.insert_bytes(b"address", Some(address.to_bytes()));
    endpoint.insert_long(b"port", i64::from(port));
    call.set_owned(endpoint.into_owned());
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let Some(socket) = client.co() else {
        return call.set_bool(false);
    };
    let Some((address, port)) = socket.getpeername() else {
        socket_error_properties(client.props(), socket);
        return call.set_bool(false);
    };
    set_endpoint(call, &address, port);
}

/// `getPeerCert(): string|false`.
fn get_peer_cert(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let Some(socket) = client.co() else {
        return call.set_bool(false);
    };
    let cert = socket.peer_cert_pem();
    if cert.is_empty() {
        socket_error_properties(client.props(), socket);
        return call.set_bool(false);
    }
    call.set_bytes(&cert);
}

/// The payload of `ping` and `disconnect`: the argument, or the empty string.
fn payload(data: Option<&ZStr>) -> Owned {
    Owned::from(data.map_or_else(|| ZString::new(b""), ZStr::to_owned))
}

/// `ping(string $data = ''): bool`.
fn ping(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    if !client.is_available() || !client.websocket.get() {
        return call.set_bool(false);
    }
    let Ok(data) = call.arity(0, 1).and_then(|()| call.zstr(1)) else {
        return call.set_bool(false);
    };
    let result = client.push(&payload(data), OPCODE_PING, FLAG_FIN, CLOSE_NORMAL);
    call.set_bool(result);
}

/// `disconnect(int $code = 1000, string $reason = ''): bool`: push a close
/// frame, then close regardless.
fn disconnect(call: &Call) {
    let Some(client) = client(call) else {
        return;
    };
    if !client.is_available() || !client.websocket.get() {
        return call.set_bool(false);
    }
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, CLOSE_NORMAL)?, call.zstr(2)?)));
    let Ok((code, reason)) = parsed else {
        return call.set_bool(false);
    };
    let result = client.push(&payload(reason), OPCODE_CLOSE, FLAG_FIN, code);
    call.set_bool(result);
    client.close(true);
}

swoole_rs_zend::handlers! {
    zim_swoole_http_client_coro___construct => construct;
    zim_swoole_http_client_coro_execute => execute;
    zim_swoole_http_client_coro_get => get;
    zim_swoole_http_client_coro_post => post;
    zim_swoole_http_client_coro_download => download;
    zim_swoole_http_client_coro_upgrade => upgrade;
    zim_swoole_http_client_coro_recv => recv;
    zim_swoole_http_client_coro___destruct => destruct;
    zim_swoole_http_client_coro_set => set;
    zim_swoole_http_client_coro_getDefer => get_defer;
    zim_swoole_http_client_coro_setDefer => set_defer;
    zim_swoole_http_client_coro_setMethod => set_method;
    zim_swoole_http_client_coro_setHeaders => set_headers;
    zim_swoole_http_client_coro_setBasicAuth => set_basic_auth;
    zim_swoole_http_client_coro_setCookies => set_cookies;
    zim_swoole_http_client_coro_setData => set_data;
    zim_swoole_http_client_coro_addFile => add_file;
    zim_swoole_http_client_coro_addData => add_data;
    zim_swoole_http_client_coro_getsockname => getsockname;
    zim_swoole_http_client_coro_getpeername => getpeername;
    zim_swoole_http_client_coro_getBody => get_body;
    zim_swoole_http_client_coro_getHeaders => get_headers;
    zim_swoole_http_client_coro_getCookies => get_cookies;
    zim_swoole_http_client_coro_getStatusCode => get_status_code;
    zim_swoole_http_client_coro_getHeaderOut => get_header_out;
    zim_swoole_http_client_coro_getPeerCert => get_peer_cert;
    zim_swoole_http_client_coro_push => push;
    zim_swoole_http_client_coro_close => close;
    zim_swoole_http_client_coro_ping => ping;
    zim_swoole_http_client_coro_disconnect => disconnect;
}
