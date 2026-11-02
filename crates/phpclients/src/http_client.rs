//! `Swoole\Coroutine\Http\Client`: registration, the object, the client's
//! state and every operation over it (connect and keep-alive, request
//! building with multipart uploads, the response loop and llhttp's callbacks,
//! decompression, downloads, the WebSocket upgrade, push and receive). The
//! coroutine socket, WebSocket framing, files and callables stay C++ and are
//! reached one call at a time (`crate::http_native`).
use crate::{
    http_native as native,
    http_request::{self as request, Compress},
    llhttp::{self, Parser, Settings},
    native::{
        Socket, swoole_client_coro_is_connected, swoole_rs_php_error, swoole_rs_set_last_error,
        swoole_socket_coro_address, swoole_socket_coro_check_liveness, swoole_socket_coro_error,
        swoole_socket_coro_getpeername, swoole_socket_coro_getsockname,
        swoole_socket_coro_sendfile, swoole_socket_coro_unconstructed,
    },
    socket::socket_of,
};
use libc::{c_char, c_int, c_void};
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
    sync::{
        LazyLock,
        atomic::{AtomicU32, Ordering},
    },
};
use swoole_rs_error::swr_v1_error_strerror;
use swoole_rs_inflate::Inflate;
use swoole_rs_phptable::{self as table, Owned};
use swoole_rs_zend::{
    Call, Class, Object, declare_bool, declare_long, declare_string, register_long_constant,
    set_object, string,
    sys::{
        E_WARNING, IS_OBJECT, IS_REFERENCE, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC,
        add_assoc_str_ex, add_assoc_string_ex, add_assoc_zval_ex, add_next_index_stringl,
        php_url_encode, swr_zend_http_build_query, swr_zend_property_merge,
        swr_zend_sensitive_parameter, swr_zend_url_decode, swr_zval_empty_string,
        swr_zval_try_addref, zend_class_entry, zend_function_entry, zend_object,
        zend_object_handlers, zend_string, zval, zval_ptr_dtor,
    },
    throw_constructed_twice, throw_exception, type_of, unset_property_deny, value,
};

/// `swHttpErrorStatusCode`.
const CONNECT_FAILED: i64 = -1;
const REQUEST_TIMEOUT: i64 = -2;
const SERVER_RESET: i64 = -3;
const SEND_FAILED: i64 = -4;
/// `SW_ERROR_*`.
const INVALID_PARAMS: c_int = 505;
const NO_CONNECTION: c_int = 5001;
const INVALID_PROTOCOL: c_int = 7102;
const HANDSHAKE_FAILED: c_int = 8504;
const PACK_FAILED: c_int = 8505;
/// `SW_EVENT_WRITE` and `SW_EVENT_RDWR`.
const EVENT_WRITE: c_int = 1 << 10;
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
static CLIENT: Class = Class::new();
static EXCEPTION: Class = Class::new();

/// `struct HttpClientObject { Client *client; zend_object std; }`.
#[repr(C)]
pub(crate) struct Data {
    client: *mut Client,
}

type ClientObject = Object<Data>;

/// What the socket's close hook changes, allocated apart from the client so
/// the hook never writes through a live `&mut Client`.
struct Conn {
    object: *mut zend_object,
    socket: Socket,
    zsocket: zval,
    /// The write buffer the closed socket left (`tmp_write_buffer`).
    written: *mut c_void,
}

/// Upstream's `swoole::coroutine::http::Client`.
struct Client {
    object: *mut zend_object,
    conn: *mut Conn,
    host: Vec<u8>,
    socket_type: i64,
    port: u16,
    ssl: bool,
    use_default_port: bool,
    connect_timeout: f64,
    response_timeout: f64,
    max_retries: u8,
    defer: bool,
    lowercase_header: bool,
    keep_alive: bool,
    http_compression: bool,
    body_decompression: bool,
    websocket: bool,
    accept_websocket_compression: bool,
    ws: *mut c_void,
    write_func: *mut c_void,
    path: Vec<u8>,
    basic_auth: Vec<u8>,
    head: bool,
    resolve: *mut c_void,
    parser: Parser,
    wait_response: bool,
    header_field: (*const u8, usize),
    body: Vec<u8>,
    compress: Compress,
    compression_error: bool,
    inflate: Option<Inflate>,
    chunked: bool,
    connection_close: bool,
    completed: bool,
    event_stream: bool,
    in_callback: bool,
    has_upload_files: bool,
    download_name: Option<Owned>,
    download_offset: i64,
    download_file: *mut c_void,
    /// The `write_func` callback bailed out; unwind and re-raise.
    bailout: bool,
}

static PARSER: LazyLock<Settings> = LazyLock::new(|| Settings {
    on_header_field: Some(on_header_field),
    on_header_value: Some(on_header_value),
    on_headers_complete: Some(on_headers_complete),
    on_body: Some(on_body),
    on_message_complete: Some(on_message_complete),
    ..Settings::default()
});

fn features() -> u32 {
    FEATURES.load(Ordering::Relaxed)
}

/// `swoole_strerror(code)`.
fn strerror<'a>(code: c_int) -> &'a CStr {
    extern "C" fn system(code: i32) -> *const c_char {
        // SAFETY: a pure query; the text stays valid until the next call.
        unsafe { libc::strerror(code) }
    }
    // SAFETY: known and system messages are static; the unknown-message buffer
    // is only used on the PHP thread, as upstream's is.
    let message = unsafe { swr_v1_error_strerror(code, Some(system)) };
    if message.is_null() {
        c""
    } else {
        // SAFETY: a NUL-terminated message.
        unsafe { CStr::from_ptr(message) }
    }
}

fn c_bytes(bytes: &[u8]) -> CString {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    CString::new(&bytes[..end]).unwrap_or_default()
}

/// `php_swoole_error(E_WARNING, ...)`, or with `fatal` `php_swoole_fatal_error`.
fn warn(fatal: bool, message: &CStr) {
    #[expect(clippy::cast_possible_wrap, reason = "E_WARNING is 2")]
    // SAFETY: a NUL-terminated message, reported from a handler.
    unsafe {
        swoole_rs_php_error(E_WARNING as c_int, fatal, message.as_ptr());
    }
}

fn ce() -> *mut zend_class_entry {
    CLIENT.entry()
}

/// Property updates on a client object.
mod props {
    use super::{CStr, ce, table, value, zend_object, zval};

    pub(super) fn long(object: *mut zend_object, name: &CStr, number: i64) {
        // SAFETY: a live client object, which declares `name`.
        unsafe { value::update_long(ce(), object, name, number) };
    }

    pub(super) fn bool(object: *mut zend_object, name: &CStr, flag: bool) {
        // SAFETY: as above.
        unsafe { value::update_bool(ce(), object, name, flag) };
    }

    pub(super) fn string(object: *mut zend_object, name: &CStr, text: &CStr) {
        // SAFETY: as above.
        unsafe { value::update_string(ce(), object, name, text) };
    }

    pub(super) fn bytes(object: *mut zend_object, name: &CStr, data: &[u8]) {
        // SAFETY: as above.
        unsafe { table::update_bytes(ce(), object, name, data) };
    }

    pub(super) fn null(object: *mut zend_object, name: &CStr) {
        // SAFETY: as above.
        unsafe { table::update_null(ce(), object, name) };
    }

    pub(super) fn zval(object: *mut zend_object, name: &CStr, data: *mut zval) {
        // SAFETY: as above; `data` is a live zval.
        unsafe { table::update(ce(), object, name, data) };
    }

    pub(super) fn array(object: *mut zend_object, name: &CStr) -> *mut zval {
        // SAFETY: as above.
        unsafe { table::property_array(ce(), object, name) }
    }
}

/// `php_swoole_socket_set_error_properties(zobject, code, message)`.
fn set_error_properties(object: *mut zend_object, code: c_int, message: &CStr) {
    // SAFETY: a plain store into Swoole's thread error.
    unsafe { swoole_rs_set_last_error(code) };
    props::long(object, c"errCode", i64::from(code));
    props::string(object, c"errMsg", message);
}

/// `php_swoole_socket_set_error_properties(zobject, socket)`.
fn socket_error_properties(object: *mut zend_object, socket: Socket) -> c_int {
    let (code, message) = socket_error(socket);
    set_error_properties(object, code, message);
    code
}

/// The socket's `errCode` and `errMsg`.
fn socket_error<'a>(socket: Socket) -> (c_int, &'a CStr) {
    let mut message: *const c_char = ptr::null();
    // SAFETY: a live socket; `message` is a writable local.
    let code = unsafe { swoole_socket_coro_error(socket, &raw mut message) };
    let message = if message.is_null() {
        c""
    } else {
        // SAFETY: errMsg is a NUL-terminated string the socket keeps.
        unsafe { CStr::from_ptr(message) }
    };
    (code, message)
}

/// Fill `rv` as `RETURN_ZVAL(property, 1, 0)` does.
fn return_property(call: &Call, name: &CStr) {
    // SAFETY: `$this` is a live client object, which declares `name`.
    let mut property = unsafe { table::property(call.this(), name) };
    // SAFETY: a live property slot.
    if unsafe { type_of(property) } == IS_REFERENCE {
        // SAFETY: a live zval.
        let held = unsafe { (*property).value };
        // SAFETY: a reference zval holds `ref`.
        let reference = unsafe { held.ref_ };
        // SAFETY: a live reference.
        property = unsafe { &raw mut (*reference).val };
    }
    // SAFETY: a live zval, copied with a new reference into the return slot.
    let copy = unsafe { *property };
    call.set_value(copy);
    // SAFETY: the return slot now shares the value.
    unsafe { swr_zval_try_addref(call.return_value()) };
}

/// A reference to the client's socket object for the rest of a call
/// (`SW_CLIENT_PRESERVE_SOCKET`), so a close inside it cannot free the socket.
struct Hold(*mut zend_object);

impl Hold {
    fn new(zsocket: *const zval) -> Option<Self> {
        // SAFETY: the client's own zval.
        if unsafe { type_of(zsocket) } != IS_OBJECT {
            return None;
        }
        // SAFETY: an object zval.
        let object = unsafe { value::obj_of(zsocket) };
        // SAFETY: a live object; the hold releases this reference.
        unsafe { (*object).gc.refcount += 1 };
        Some(Self(object))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut held = MaybeUninit::<zval>::zeroed();
        // SAFETY: the held object, whose reference this zval takes over.
        unsafe { set_object(held.as_mut_ptr(), self.0) };
        // SAFETY: releases the reference `new` took.
        unsafe { zval_ptr_dtor(held.as_mut_ptr()) };
    }
}

/// A read `TimeoutController` for the response loop.
struct Timeout(*mut c_void);

impl Timeout {
    fn timed_out(&self) -> bool {
        // SAFETY: a live controller.
        unsafe { native::swoole_http_client_coro_timed_out(self.0) }
    }
}

impl Drop for Timeout {
    fn drop(&mut self) {
        // SAFETY: the controller `swoole_http_client_coro_timeout` made.
        unsafe { native::swoole_http_client_coro_timeout_end(self.0) };
    }
}

/// The socket's close hook (`Client::socket_dtor`): keep the written bytes,
/// drop the socket and say so on the object.
/// # Safety
/// `conn` is the live `Conn` the client registered; `written` is the socket's
/// popped write buffer (or null), which this takes over.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_http_client_socket_dtor(
    conn: *mut c_void,
    written: *mut c_void,
) {
    let conn = conn.cast::<Conn>();
    // SAFETY: per the caller; no `&mut` to the conn is live across this hook.
    let saved = unsafe { (*conn).written };
    // SAFETY: the buffer the conn held, freed once.
    unsafe { native::swoole_http_client_coro_buffer_free(saved) };
    // SAFETY: per the caller.
    unsafe { (*conn).written = written };
    // SAFETY: as above.
    unsafe { (*conn).socket = ptr::null_mut() };
    // SAFETY: as above.
    let object = unsafe { (*conn).object };
    props::bool(object, c"connected", false);
    props::null(object, c"socket");
    // SAFETY: as above.
    let zsocket = unsafe { &raw mut (*conn).zsocket };
    // SAFETY: the conn's reference to the socket object.
    unsafe { zval_ptr_dtor(zsocket) };
    // SAFETY: as above.
    unsafe {
        (*zsocket).u1 = swoole_rs_zend::sys::_zval_struct__bindgen_ty_1 {
            type_info: swoole_rs_zend::sys::IS_NULL,
        };
    }
}

/// `php_swoole_http_parse_set_cookies`: one `Set-Cookie` value into the
/// `cookies` map (URL-decoded) and the raw `set_cookie_headers` list.
/// # Safety
/// `at` lends `length` bytes; both arrays are live, separated array zvals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_parse_set_cookies(
    at: *const c_char,
    length: usize,
    cookies: *mut zval,
    set_cookie_headers: *mut zval,
) {
    // SAFETY: per the caller.
    let plan = unsafe { swoole_rs_clients::swr_v1_clients_cookie(at.cast(), length) };
    let mut cookie = MaybeUninit::<zval>::uninit();
    if plan.value_length == 0 {
        // SAFETY: a writable zval.
        unsafe { swr_zval_empty_string(cookie.as_mut_ptr()) };
    } else {
        // SAFETY: the value lies inside the caller's bytes.
        let start = at.wrapping_add(plan.value_offset).cast::<u8>();
        // SAFETY: as above.
        let text = unsafe { std::slice::from_raw_parts(start, plan.value_length) };
        let decoded = string::copy(text);
        // SAFETY: a fresh, unshared string whose bytes php_url_decode rewrites.
        let length = unsafe { swr_zend_url_decode(string::val(decoded).cast(), plan.value_length) };
        // SAFETY: the decoded length is no longer than the string.
        unsafe { (*decoded).len = length };
        // SAFETY: a writable zval takes the string's reference.
        unsafe { value::set_str(cookie.as_mut_ptr(), decoded) };
    }
    if plan.key_length == 0 {
        // SAFETY: a live array takes the value.
        unsafe { table::push(cookies, cookie.as_mut_ptr()) };
    } else {
        // SAFETY: as above; Zend copies the key.
        unsafe { add_assoc_zval_ex(cookies, at, plan.key_length, cookie.as_mut_ptr()) };
    }
    // SAFETY: as above; Zend copies the bytes.
    unsafe { add_next_index_stringl(set_cookie_headers, at, length) };
}

impl Client {
    fn new(object: *mut zend_object, host: &[u8], port: i64, ssl: bool) -> Box<Self> {
        let use_default_port = port == 0;
        let port = if use_default_port {
            if ssl { 443 } else { 80 }
        } else {
            port
        };
        let mut zsocket = MaybeUninit::<zval>::zeroed();
        // SAFETY: zeroed storage, typed null (`ZVAL_NULL`).
        unsafe {
            (*zsocket.as_mut_ptr()).u1 = swoole_rs_zend::sys::_zval_struct__bindgen_ty_1 {
                type_info: swoole_rs_zend::sys::IS_NULL,
            }
        };
        let conn = Box::into_raw(Box::new(Conn {
            object,
            socket: ptr::null_mut(),
            // SAFETY: initialised above.
            zsocket: unsafe { zsocket.assume_init() },
            written: ptr::null_mut(),
        }));
        let mut host = host.to_vec();
        Box::new(Self {
            object,
            conn,
            socket_type: request::socket_type(&mut host),
            host,
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "upstream's uint16_t port"
            )]
            port: port as u16,
            ssl,
            use_default_port,
            connect_timeout: 0.0,
            response_timeout: 0.0,
            max_retries: 0,
            defer: false,
            lowercase_header: true,
            keep_alive: true,
            http_compression: true,
            body_decompression: true,
            websocket: false,
            accept_websocket_compression: false,
            // SAFETY: a fresh C++ allocation the client frees.
            ws: unsafe { native::swoole_http_client_coro_ws_new() },
            write_func: ptr::null_mut(),
            path: Vec::new(),
            basic_auth: Vec::new(),
            head: false,
            // SAFETY: as above.
            resolve: unsafe { native::swoole_http_client_coro_resolve_new() },
            parser: Parser::new(),
            wait_response: false,
            header_field: (ptr::null(), 0),
            body: Vec::new(),
            compress: Compress::None,
            compression_error: false,
            inflate: None,
            chunked: false,
            connection_close: false,
            completed: false,
            event_stream: false,
            in_callback: false,
            has_upload_files: false,
            download_name: None,
            download_offset: 0,
            download_file: ptr::null_mut(),
            bailout: false,
        })
    }

    /// The live socket, read fresh: the close hook may have cleared it.
    fn socket(&self) -> Socket {
        // SAFETY: the conn lives as long as the client.
        unsafe { (*self.conn).socket }
    }

    fn zsocket(&self) -> *mut zval {
        // SAFETY: as above.
        unsafe { &raw mut (*self.conn).zsocket }
    }

    fn set_error(&self, code: c_int, message: &CStr, status: i64) {
        props::long(self.object, c"errCode", i64::from(code));
        props::string(self.object, c"errMsg", message);
        props::long(self.object, c"statusCode", status);
    }

    fn set_socket_error(&self, socket: Socket, status: i64) {
        let (code, message) = socket_error(socket);
        self.set_error(code, message, status);
    }

    /// `is_available`: connected, else errCode says no connection.
    fn is_available(&self) -> bool {
        let socket = self.socket();
        // SAFETY: a live socket.
        if socket.is_null() || !unsafe { swoole_client_coro_is_connected(socket) } {
            set_error_properties(self.object, NO_CONNECTION, strerror(NO_CONNECTION));
            return false;
        }
        true
    }

    fn apply_setting(&mut self, settings: *mut zval, check_all: bool) {
        // SAFETY: a live zval.
        if !unsafe { table::is_array(settings) } || unsafe { table::length(settings) } == 0 {
            return;
        }
        if check_all {
            // SAFETY: an array zval, read as `php_swoole_array_get_value` does.
            let get = |key: &[u8]| unsafe { table::get(settings, key) };
            // SAFETY: values Zend found in the array (each conversion below too).
            let truthy = |value: *mut zval| unsafe { table::truthy(value) };
            if let Some(found) = get(b"connect_timeout") {
                // SAFETY: as above.
                self.connect_timeout = unsafe { table::double(found) };
            }
            if let Some(found) = get(b"timeout") {
                // SAFETY: as above.
                self.response_timeout = unsafe { table::double(found) };
            }
            if let Some(found) = get(b"max_retries") {
                // SAFETY: as above.
                let retries = unsafe { table::long(found) }.min(i64::from(u8::MAX));
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "upstream's (uint8_t) cast"
                )]
                {
                    self.max_retries = retries as u8;
                }
            }
            if let Some(found) = get(b"defer") {
                self.defer = truthy(found);
            }
            if let Some(found) = get(b"lowercase_header") {
                self.lowercase_header = truthy(found);
            }
            if let Some(found) = get(b"keep_alive") {
                self.keep_alive = truthy(found);
            }
            if let Some(found) = get(b"http_compression") {
                self.http_compression = truthy(found);
            }
            if let Some(found) = get(b"body_decompression") {
                self.body_decompression = truthy(found);
            }
            if let Some(found) = get(b"write_func") {
                // SAFETY: the callable the client owns, if any.
                unsafe { native::swoole_http_client_coro_callable_free(self.write_func) };
                // SAFETY: a live zval.
                self.write_func = unsafe { native::swoole_http_client_coro_callable(found) };
            }
            // SAFETY: the client's WebSocket state and a live array.
            unsafe { native::swoole_http_client_coro_ws_apply(self.ws, settings) };
        }
        let socket = self.socket();
        if !socket.is_null() {
            // SAFETY: a live socket and array.
            unsafe { native::swoole_http_client_coro_apply(socket, settings) };
        }
    }

    fn set_basic_auth(&mut self, username: &[u8], password: &[u8]) {
        let input = [username, b":", password].concat();
        let mut encoded = vec![0_u8; input.len().div_ceil(3) * 4 + 1];
        // SAFETY: the input and an output of 4 * ceil(n / 3) + 1 bytes.
        let written = unsafe {
            swoole_rs_base64::swr_v1_base64_encode(
                input.as_ptr(),
                input.len(),
                encoded.as_mut_ptr(),
            )
        };
        self.basic_auth = [&b"Basic "[..], &encoded[..written]].concat();
    }

    fn connect(&mut self) -> bool {
        if !self.socket().is_null() {
            return true;
        }
        if self.body.capacity() == 0 {
            self.body.reserve(BODY_SIZE);
        }
        let mut error = 0;
        // SAFETY: the host bytes and a writable local.
        let object = unsafe {
            native::swoole_http_client_coro_create_socket(self.socket_type, &raw mut error)
        };
        if object.is_null() {
            self.set_error(error, strerror(error), CONNECT_FAILED);
            return false;
        }
        // SAFETY: the conn takes the new object's reference.
        unsafe { set_object(self.zsocket(), object) };
        // SAFETY: a live `Coroutine\Socket` object.
        let socket = unsafe { socket_of(object) };
        // SAFETY: the conn lives as long as the client.
        unsafe { (*self.conn).socket = socket };
        // SAFETY: a live socket.
        if self.ssl && !unsafe { native::swoole_http_client_coro_enable_ssl(socket) } {
            self.set_socket_error(socket, CONNECT_FAILED);
            self.close(true);
            return false;
        }
        // SAFETY: `$this` declares `setting`.
        let settings = unsafe { table::property(self.object, c"setting") };
        self.apply_setting(settings, false);
        self.websocket = false;
        self.accept_websocket_compression = false;
        // SAFETY: a live socket, the client's resolve context and conn.
        unsafe {
            native::swoole_http_client_coro_configure(
                socket,
                self.connect_timeout,
                self.resolve,
                self.conn.cast(),
            );
        }
        // SAFETY: a live socket and the host bytes.
        let connected = unsafe {
            native::swoole_http_client_coro_connect(
                socket,
                self.host.as_ptr().cast(),
                self.host.len(),
                c_int::from(self.port),
            )
        };
        if !connected {
            self.set_socket_error(socket, CONNECT_FAILED);
            self.close(true);
            return false;
        }
        props::zval(self.object, c"socket", self.zsocket());
        props::bool(self.object, c"connected", true);
        true
    }

    fn keep_liveness(&mut self) -> bool {
        let socket = self.socket();
        // SAFETY: a live socket.
        if socket.is_null() || !unsafe { swoole_socket_coro_check_liveness(socket) } {
            if !socket.is_null() {
                // SAFETY: as above.
                unsafe { native::swoole_http_client_coro_check_bound(socket, EVENT_RDWR) };
                self.set_socket_error(socket, SERVER_RESET);
                self.close(false);
            }
            let mut attempt = 0;
            while attempt < u32::from(self.max_retries) + 1 {
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
        clippy::cognitive_complexity,
        reason = "upstream's send_request, in its order"
    )]
    fn send_request(&mut self) -> bool {
        let object = self.object;
        if self.path.is_empty() {
            set_error_properties(object, INVALID_PARAMS, strerror(INVALID_PARAMS));
            return false;
        }
        props::null(object, c"headers");
        props::null(object, c"set_cookie_headers");
        props::string(object, c"body", c"");
        if !self.keep_liveness() {
            return false;
        }
        props::long(object, c"errCode", 0);
        props::string(object, c"errMsg", c"");
        props::long(object, c"statusCode", 0);
        let socket = self.socket();
        // SAFETY: a live socket (each native call below too).
        unsafe { native::swoole_http_client_coro_check_bound(socket, EVENT_WRITE) };
        // SAFETY: as above.
        unsafe { swoole_rs_set_last_error(0) };
        self.body.clear();

        // SAFETY: `$this` declares each property.
        let method = unsafe { table::property_not_null(object, c"requestMethod") };
        // SAFETY: as above.
        let headers = unsafe { table::property(object, c"requestHeaders") };
        // SAFETY: as above.
        let body = unsafe { table::property_not_null(object, c"requestBody") };
        // SAFETY: as above.
        let files = unsafe { table::property(object, c"uploadFiles") };
        // SAFETY: as above.
        let cookies = unsafe { table::property(object, c"cookies") };
        // SAFETY: as above.
        let download = unsafe { table::property_not_null(object, c"downloadFile") };
        // SAFETY: live property zvals (each table call below reads one).
        let headers_array = unsafe { table::is_array(headers) };

        let mut host_header = None;
        if headers_array {
            // SAFETY: as above.
            let mut found = unsafe { table::find(headers, b"Host") };
            if found.is_null() {
                // SAFETY: as above.
                found = unsafe { table::find(headers, b"host") };
            }
            if !found.is_null() {
                // SAFETY: as above.
                host_header = Some(unsafe { Owned::of(found) });
            }
        }
        if let Some(download) = download {
            // SAFETY: as above.
            self.download_name = Some(unsafe { Owned::of(download) });
            // SAFETY: as above.
            let offset = unsafe { table::property(object, c"downloadOffset") };
            // SAFETY: as above.
            self.download_offset = unsafe { table::long(offset) };
        }

        let mut out = Vec::with_capacity(256);
        let method = method.map(|method| {
            // SAFETY: as above.
            unsafe { Owned::of(method) }
        });
        let method_name: &[u8] = match &method {
            Some(method) => method.bytes(),
            None if body.is_some() => b"POST",
            None => b"GET",
        };
        self.head = method_name.eq_ignore_ascii_case(b"HEAD");
        out.extend_from_slice(method_name);
        out.push(b' ');

        // SAFETY: as above.
        let proxy = unsafe { native::swoole_http_client_coro_proxy(socket) };
        if proxy != 0 {
            let host = host_header.as_ref().map_or(&self.host[..], Owned::bytes);
            request::proxy_uri(&mut out, host, &self.path, self.port);
        } else {
            out.extend_from_slice(&self.path);
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
        if headers_array {
            // SAFETY: as above.
            for (key, value) in unsafe { table::entries(headers) } {
                // SAFETY: a value in the array.
                let Some(key) = key.filter(|_| !unsafe { table::is_null(value) }) else {
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
                // SAFETY: as above.
                let text = unsafe { Owned::of(value) };
                request::header(&mut out, key, text.bytes());
                if key.eq_ignore_ascii_case(b"Connection") {
                    has_connection = true;
                    if text.bytes().eq_ignore_ascii_case(b"close") {
                        self.keep_alive = false;
                    }
                }
            }
        }
        if proxy == 2 {
            // SAFETY: a live socket with an authenticating proxy.
            let auth = unsafe { native::swoole_http_client_coro_proxy_auth(socket) };
            // SAFETY: the new string's reference.
            let auth = unsafe { Owned::from_raw(auth) };
            request::header(&mut out, b"Proxy-Authorization", auth.bytes());
        }
        if !self.basic_auth.is_empty() {
            request::header(&mut out, b"Authorization", &self.basic_auth);
        }
        if !has_connection {
            let value: &[u8] = if self.keep_alive {
                b"keep-alive"
            } else {
                b"closed"
            };
            request::header(&mut out, b"Connection", value);
        }
        if let Some(encoding) = compression.filter(|_| self.http_compression && !has_encoding) {
            request::header(&mut out, b"Accept-Encoding", encoding);
        }

        // SAFETY: as above.
        if unsafe { table::is_array(cookies) } {
            out.extend_from_slice(b"Cookie: ");
            // SAFETY: as above.
            let count = unsafe { table::length(cookies) };
            let mut index = 0;
            // SAFETY: as above.
            for (key, value) in unsafe { table::entries(cookies) } {
                index += 1;
                let Some(key) = key else { continue };
                // SAFETY: a value in the array.
                let text = unsafe { Owned::of(value) };
                if text.bytes().is_empty() {
                    continue;
                }
                out.extend_from_slice(key);
                out.push(b'=');
                // SAFETY: the value's bytes; Zend returns a new string.
                let encoded =
                    unsafe { php_url_encode(text.bytes().as_ptr().cast(), text.bytes().len()) };
                // SAFETY: the reference php_url_encode returned.
                out.extend_from_slice(unsafe { Owned::from_raw(encoded) }.bytes());
                if index < count {
                    out.extend_from_slice(b"; ");
                }
            }
            out.extend_from_slice(b"\r\n");
        }

        // SAFETY: as above.
        self.has_upload_files = unsafe { table::length_safe(files) } > 0;
        if self.has_upload_files {
            return self.send_multipart(out, body, files);
        }
        if let Some(body) = body {
            // SAFETY: as above.
            if unsafe { table::is_array(body) } {
                request::header(
                    &mut out,
                    b"Content-Type",
                    b"application/x-www-form-urlencoded",
                );
                // SAFETY: as above.
                if unsafe { table::length(body) } > 0 {
                    // SAFETY: as above.
                    let query = unsafe { swr_zend_http_build_query(body) };
                    if query.is_null() {
                        warn(false, c"http_build_query failed");
                        return false;
                    }
                    // SAFETY: the reference the query builder returned.
                    let query = unsafe { Owned::from_raw(query) };
                    request::content_length(&mut out, query.bytes().len());
                    out.extend_from_slice(query.bytes());
                } else {
                    request::content_length(&mut out, 0);
                }
            } else {
                // SAFETY: as above.
                let data = unsafe { Owned::of(body) };
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
        self.wait_response = true;
        true
    }

    /// `send_request`'s multipart/form-data branch, from the headers so far.
    fn send_multipart(
        &mut self,
        mut out: Vec<u8>,
        body: Option<*mut zval>,
        files: *mut zval,
    ) -> bool {
        let boundary = request::boundary();
        request::multipart_type(&mut out, &boundary);
        // SAFETY: a live property zval.
        let fields = body.filter(|&body| unsafe { table::is_array(body) });
        let field_entries = || {
            // SAFETY: as above; nothing changes the array while sending.
            fields
                .into_iter()
                .flat_map(|fields| unsafe { table::entries(fields) })
                .filter_map(|(key, value)| {
                    // SAFETY: values in the array.
                    let null = unsafe { table::is_null(value) };
                    key.filter(|_| !null).map(|key| (key, value))
                })
        };
        let mut length = 0_usize;
        for (key, value) in field_entries() {
            // SAFETY: a value in the array.
            let text = unsafe { Owned::of(value) };
            length = length.wrapping_add(request::field_length(key.len(), text.bytes().len()));
        }
        // SAFETY: `uploadFiles` is a non-empty array.
        for (_, file) in unsafe { table::entries(files) } {
            let Some(part) = FilePart::sizes(file) else {
                continue;
            };
            length = length.wrapping_add(part);
        }
        request::content_length(&mut out, request::multipart_length(length));
        for (key, value) in field_entries() {
            // SAFETY: a value in the array.
            let text = unsafe { Owned::of(value) };
            request::field_header(&mut out, &boundary, key);
            out.extend_from_slice(text.bytes());
            out.extend_from_slice(b"\r\n");
        }
        if !self.send(&out) {
            return self.send_failed();
        }
        // SAFETY: as above.
        for (_, file) in unsafe { table::entries(files) } {
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
        self.wait_response = true;
        true
    }

    fn send(&self, data: &[u8]) -> bool {
        // SAFETY: a live socket and the bytes to send.
        unsafe {
            native::swoole_http_client_coro_send(self.socket(), data.as_ptr().cast(), data.len())
        }
    }

    fn send_raw(&self, data: &[u8]) -> bool {
        // SAFETY: as above.
        unsafe {
            native::swoole_http_client_coro_send_raw(
                self.socket(),
                data.as_ptr().cast(),
                data.len(),
            )
        }
    }

    fn sendfile(&self, path: &[u8], offset: i64, size: i64) -> bool {
        let path = c_bytes(path);
        // SAFETY: a live socket and a NUL-terminated path.
        unsafe { swoole_socket_coro_sendfile(self.socket(), path.as_ptr(), offset, size) }
    }

    fn send_failed(&mut self) -> bool {
        self.set_socket_error(self.socket(), SEND_FAILED);
        self.close(true);
        false
    }

    /// `exec`: send, and unless deferred receive, retrying 502 and 503.
    fn exec(&mut self, path: &[u8]) -> bool {
        self.path = path.to_vec();
        // SAFETY: the client's resolve context.
        unsafe {
            native::swoole_http_client_coro_resolve_reset(self.resolve, self.use_default_port);
        };
        let mut attempt = 0;
        while attempt < u32::from(self.max_retries) + 1 {
            if !self.send_request() {
                return false;
            }
            if self.defer {
                return true;
            }
            if !self.recv_response(0.0) || self.bailout {
                return false;
            }
            let status = self.parser.status_code;
            if self.max_retries > 0 && matches!(status, BAD_GATEWAY | SERVICE_UNAVAILABLE) {
                self.close(true);
                attempt += 1;
                continue;
            }
            return true;
        }
        false
    }

    fn recv_response(&mut self, timeout: f64) -> bool {
        if !self.wait_response {
            return false;
        }
        let socket = self.socket();
        let mut received: isize;
        let mut total = 0_usize;
        let mut header_completed = false;
        let mut header_offset = 0_usize;
        let this: *mut Self = self;
        self.parser.init_response(this.cast());
        let timeout = if timeout == 0.0 {
            self.response_timeout
        } else {
            timeout
        };
        // SAFETY: a live socket; the controller is ended when dropped.
        let controller =
            Timeout(unsafe { native::swoole_http_client_coro_timeout(socket, timeout) });
        let mut success = false;
        loop {
            if controller.timed_out() {
                break;
            }
            let (mut length, mut size) = (0, 0);
            // SAFETY: a live socket and writable locals.
            let buffer = unsafe {
                native::swoole_http_client_coro_read_buffer(socket, &raw mut length, &raw mut size)
            };
            // SAFETY: the read buffer has `size` bytes, `length` of them used.
            received = unsafe {
                native::swoole_http_client_coro_recv(
                    socket,
                    buffer.wrapping_add(length),
                    size - length,
                )
            };
            if received <= 0 {
                if received == 0 {
                    // SAFETY: a live socket.
                    unsafe { native::swoole_http_client_coro_set_err(socket, libc::ECONNRESET) };
                    if total > 0 && !self.parser.keep_alive() {
                        self.parser.finish();
                        success = true;
                    }
                }
                break;
            }
            let mut count = received.unsigned_abs();
            if !header_completed {
                length += count;
                // SAFETY: as above.
                unsafe { native::swoole_http_client_coro_read_buffer_set(socket, length) };
                // SAFETY: the buffer's first `length` bytes are received data.
                let data = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), length) };
                if request::header_end(&data[header_offset..]).is_none() {
                    if length == size {
                        // SAFETY: as above.
                        unsafe {
                            native::swoole_http_client_coro_set_err(socket, INVALID_PROTOCOL);
                        };
                        break;
                    }
                    header_offset = length.saturating_sub(4);
                    continue;
                }
                header_completed = true;
                header_offset = 0;
                count = length;
                // SAFETY: as above.
                unsafe { native::swoole_http_client_coro_read_buffer_set(socket, 0) };
            }
            total += count;
            // SAFETY: the buffer's first `count` bytes are this response's data.
            let data = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), count) };
            let parsed = self.parser.execute(&PARSER, data);
            if self.bailout {
                return false;
            }
            // SAFETY: as above.
            if unsafe { native::swoole_http_client_coro_closing(socket) } {
                break;
            }
            if self.parser.error != llhttp::OK {
                // SAFETY: as above.
                unsafe { native::swoole_http_client_coro_set_err(socket, INVALID_PROTOCOL) };
                break;
            }
            if self.event_stream && self.parser.needs_eof() {
                self.parser.finish();
            }
            if self.completed {
                if self.parser.upgrade != 0 && count > parsed + WEBSOCKET_HEADER {
                    // SAFETY: as above.
                    unsafe {
                        native::swoole_http_client_coro_read_buffer_keep(socket, count, parsed);
                    };
                }
                success = true;
                break;
            }
        }
        drop(controller);
        if !success {
            let code = socket_error_properties(self.object, socket);
            let status = if code == libc::ETIMEDOUT {
                REQUEST_TIMEOUT
            } else {
                SERVER_RESET
            };
            props::long(self.object, c"statusCode", status);
            self.close(true);
            return false;
        }
        if self.websocket {
            if !self.verify_handshake() {
                self.set_error(
                    HANDSHAKE_FAILED,
                    c"websocket handshake failed: invalid Upgrade/Connection/Sec-WebSocket-Accept headers",
                    SERVER_RESET,
                );
                self.close(true);
                return false;
            }
            // SAFETY: a live socket.
            unsafe { native::swoole_http_client_coro_ws_protocol(socket) };
        }
        if !self.websocket && (!self.keep_alive || self.connection_close) {
            self.close(true);
        } else {
            self.reset();
        }
        true
    }

    /// `websocket_verify_server_handshake`.
    fn verify_handshake(&self) -> bool {
        // SAFETY: `$this` declares both properties.
        let request = unsafe { table::property(self.object, c"requestHeaders") };
        // SAFETY: as above.
        let response = unsafe { table::property(self.object, c"headers") };
        // SAFETY: live property zvals (each lookup below reads one).
        if !unsafe { table::is_array(request) } || !unsafe { table::is_array(response) } {
            return false;
        }
        let text = |array: *mut zval, key: &[u8]| {
            // SAFETY: as above.
            let found = unsafe { table::find(array, key) };
            // SAFETY: a value Zend found.
            (!found.is_null())
                .then(|| unsafe { table::string_bytes(found) })
                .flatten()
        };
        let Some(key) = text(request, b"Sec-WebSocket-Key").filter(|key| !key.is_empty()) else {
            return false;
        };
        let upgrade = text(response, b"upgrade");
        if upgrade.is_none_or(|value| request::token_list_contains(value, b"websocket") == 0) {
            return false;
        }
        let connection = text(response, b"connection");
        if connection.is_none_or(|value| request::token_list_contains(value, b"Upgrade") == 0) {
            return false;
        }
        let Some(accept) =
            text(response, b"sec-websocket-accept").filter(|accept| !accept.is_empty())
        else {
            return false;
        };
        // SAFETY: both slices are live for the call.
        unsafe {
            swoole_rs_clients::ws::swr_v1_clients_ws_accept_valid(
                key.as_ptr(),
                key.len(),
                accept.as_ptr(),
                accept.len(),
            ) != 0
        }
    }

    fn recv_websocket_frame(&mut self, call: &Call, timeout: f64) {
        let socket = self.socket();
        let value = call.return_value();
        // SAFETY: a live socket, the client's WebSocket state and the return slot.
        unsafe { native::swoole_http_client_coro_ws_recv(socket, self.ws, value, timeout) };
        // SAFETY: the return slot recv_frame filled.
        let kind = unsafe { type_of(value) };
        // SAFETY: as above.
        if kind == IS_STRING && unsafe { table::string_bytes(value) }.is_some_and(<[u8]>::is_empty)
        {
            self.close(true);
            return;
        }
        if kind == swoole_rs_zend::sys::IS_FALSE {
            let code = socket_error_properties(self.object, socket);
            props::long(self.object, c"statusCode", SERVER_RESET);
            if code != libc::ETIMEDOUT {
                self.close(true);
            }
            return;
        }
        if kind == swoole_rs_zend::sys::IS_NULL {
            call.set_bool(false);
        }
    }

    fn upgrade(&mut self, path: &[u8]) -> bool {
        self.defer = false;
        let headers = props::array(self.object, c"requestHeaders");
        props::string(self.object, c"requestMethod", c"GET");
        let token = request::token::<WEBSOCKET_KEY>();
        let mut key = vec![0_u8; WEBSOCKET_KEY.div_ceil(3) * 4 + 1];
        // SAFETY: the token and an output of 4 * ceil(n / 3) + 1 bytes.
        let written = unsafe {
            swoole_rs_base64::swr_v1_base64_encode(token.as_ptr(), token.len(), key.as_mut_ptr())
        };
        let add = |name: &CStr, text: &CStr| {
            // SAFETY: a live array property; Zend copies both.
            unsafe {
                add_assoc_string_ex(headers, name.as_ptr(), name.count_bytes(), text.as_ptr());
            };
        };
        add(c"Connection", c"Upgrade");
        add(c"Upgrade", c"websocket");
        add(c"Sec-WebSocket-Version", c"13");
        let key = string::copy(&key[..written]);
        // SAFETY: as above; the array takes the key string's reference.
        unsafe { add_assoc_str_ex(headers, c"Sec-WebSocket-Key".as_ptr(), 17, key) };
        // SAFETY: the client's WebSocket state.
        if features() & 1 != 0 && unsafe { native::swoole_http_client_coro_ws_compression(self.ws) }
        {
            add(c"Sec-Websocket-Extensions", DEFLATE_EXTENSION);
        }
        if !self.exec(path) {
            return false;
        }
        let status = self.parser.status_code;
        if status != SWITCHING_PROTOCOLS || !self.websocket {
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

    fn push(&mut self, data: *mut zval, opcode: i64, flags: i64, code: i64) -> bool {
        let object = self.object;
        if !self.websocket {
            // SAFETY: a plain store into Swoole's thread error.
            unsafe { swoole_rs_set_last_error(HANDSHAKE_FAILED) };
            warn(true, c"websocket handshake failed, cannot push data");
            // SAFETY: a plain read of Swoole's thread error.
            let last = unsafe { native::swoole_http_client_coro_last_error() };
            props::long(object, c"errCode", i64::from(last));
            props::string(
                object,
                c"errMsg",
                c"websocket handshake failed, cannot push data",
            );
            props::long(object, c"statusCode", CONNECT_FAILED);
            return false;
        }
        let socket = self.socket();
        let flags = c_int::from(flags.to_le_bytes()[0]);
        // SAFETY: a live socket, WebSocket state and data zval.
        let result = unsafe {
            native::swoole_http_client_coro_ws_push(
                socket,
                self.ws,
                data,
                opcode,
                flags,
                code,
                self.accept_websocket_compression,
            )
        };
        match result {
            0 => true,
            1 => {
                // SAFETY: a plain store into Swoole's thread error.
                unsafe { swoole_rs_set_last_error(PACK_FAILED) };
                // SAFETY: a plain read of Swoole's thread error.
                let last = unsafe { native::swoole_http_client_coro_last_error() };
                props::long(object, c"errCode", i64::from(last));
                props::string(object, c"errMsg", c"websocket frame pack failed");
                false
            }
            _ => {
                socket_error_properties(object, socket);
                props::long(object, c"statusCode", SERVER_RESET);
                self.close(true);
                false
            }
        }
    }

    fn reset(&mut self) {
        self.wait_response = false;
        self.completed = false;
        self.event_stream = false;
        self.compress = Compress::None;
        self.compression_error = false;
        self.inflate = None;
        if self.has_upload_files {
            props::null(self.object, c"uploadFiles");
        }
        if !self.download_file.is_null() {
            // SAFETY: the client's open download file, freed once.
            unsafe { native::swoole_http_client_coro_file_free(self.download_file) };
            self.download_file = ptr::null_mut();
            self.download_name = None;
            self.download_offset = 0;
            props::null(self.object, c"downloadFile");
            props::long(self.object, c"downloadOffset", 0);
        }
    }

    /// `close`: the socket's members must not be read after its close, which
    /// can free it (the close hook clears `socket`).
    fn close(&mut self, should_be_reset: bool) -> bool {
        let socket = self.socket();
        if socket.is_null() {
            return false;
        }
        if self.in_callback {
            // SAFETY: a live socket.
            unsafe { native::swoole_http_client_coro_close_wait(socket) };
            return true;
        }
        props::bool(self.object, c"connected", false);
        // SAFETY: as above.
        if !unsafe { native::swoole_http_client_coro_close(socket) } {
            socket_error_properties(self.object, socket);
            return false;
        }
        if should_be_reset {
            self.reset();
        }
        true
    }

    /// `decompress_response`: inflate a body chunk onto `body`.
    fn decompress(&mut self, input: &[u8]) -> bool {
        if input.is_empty() {
            return false;
        }
        if self.inflate.is_none() {
            if !matches!(self.compress, Compress::Gzip | Compress::Deflate) {
                // SAFETY: a warning with a small integer.
                unsafe { native::swoole_http_client_coro_inflate_warning(self.compress as c_int) };
                return false;
            }
            self.inflate = Some(Inflate::new(self.compress == Compress::Gzip));
        }
        let Some(output) = self
            .inflate
            .as_mut()
            .and_then(|inflate| inflate.decode(input))
        else {
            // SAFETY: as above.
            unsafe { native::swoole_http_client_coro_inflate_warning(-1) };
            return false;
        };
        self.body.extend_from_slice(output);
        true
    }

    fn add_header(&self, key: &[u8], data: &[u8]) {
        let headers = props::array(self.object, c"headers");
        let mut element = MaybeUninit::<zval>::uninit();
        // SAFETY: a writable zval takes the new string's reference.
        unsafe { value::set_str(element.as_mut_ptr(), string::copy(data)) };
        // SAFETY: a live array property takes the element.
        unsafe { table::add_or_merge(headers, key, element.as_mut_ptr()) };
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close(true);
        // SAFETY: the C++ objects the client owns, freed once (null is ignored
        // by each, as `delete` is).
        unsafe { native::swoole_http_client_coro_callable_free(self.write_func) };
        // SAFETY: as above.
        unsafe { native::swoole_http_client_coro_ws_free(self.ws) };
        // SAFETY: as above.
        unsafe { native::swoole_http_client_coro_resolve_free(self.resolve) };
        // SAFETY: as above.
        unsafe { native::swoole_http_client_coro_file_free(self.download_file) };
        // SAFETY: the conn the client allocated; the socket hook no longer runs
        // for it once the socket is closed or released below.
        let conn = unsafe { Box::from_raw(self.conn) };
        // SAFETY: the saved write buffer, freed once.
        unsafe { native::swoole_http_client_coro_buffer_free(conn.written) };
        let mut zsocket = conn.zsocket;
        // SAFETY: the conn's reference to its socket object, if any.
        unsafe { zval_ptr_dtor(&raw mut zsocket) };
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
    fn sizes(file: *mut zval) -> Option<usize> {
        // SAFETY: an element of the live `uploadFiles` array.
        if !unsafe { table::is_array(file) } {
            return None;
        }
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
    fn read(file: *mut zval) -> Option<Self> {
        // SAFETY: as above.
        if !unsafe { table::is_array(file) } {
            return None;
        }
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

    fn field(file: *mut zval, key: &[u8]) -> Option<*mut zval> {
        // SAFETY: an array element of `uploadFiles`.
        let found = unsafe { table::find(file, key) };
        (!found.is_null()).then_some(found)
    }

    /// `Z_STRVAL_P`/`Z_STRLEN_P`, which upstream assumes; a non-string is empty.
    fn text(value: *mut zval) -> &'a [u8] {
        // SAFETY: a live element, read while the array is.
        unsafe { table::string_bytes(value) }.unwrap_or_default()
    }

    /// `Z_LVAL_P`, which upstream assumes; a non-int is 0.
    fn number(value: *mut zval) -> i64 {
        // SAFETY: as above.
        if unsafe { type_of(value) } == swoole_rs_zend::sys::IS_LONG {
            // SAFETY: an int zval.
            unsafe { value::long_of(value) }
        } else {
            0
        }
    }
}

/// The client behind `parser.data`.
/// # Safety
/// `parser` is the client's own, executing inside `recv_response`.
unsafe fn parser_client<'a>(parser: *mut Parser) -> &'a mut Client {
    // SAFETY: per the caller, `data` is the client.
    let data = unsafe { (*parser).data };
    // SAFETY: as above.
    unsafe { &mut *data.cast::<Client>() }
}

unsafe extern "C" fn on_header_field(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the client's parser.
    let client = unsafe { parser_client(parser) };
    client.header_field = (at.cast(), length);
    0
}

unsafe extern "C" fn on_header_value(
    parser: *mut Parser,
    at: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: llhttp calls back with the client's parser.
    let client = unsafe { parser_client(parser) };
    let (field, field_length) = client.header_field;
    // SAFETY: llhttp's spans point into the buffer being parsed.
    let name = unsafe { std::slice::from_raw_parts(field, field_length) };
    // SAFETY: as above.
    let data = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
    let lowered;
    let name = if client.lowercase_header {
        lowered = name.to_ascii_lowercase();
        &lowered[..]
    } else {
        name
    };
    client.add_header(name, data);
    let status = client.parser.status_code;
    if status == SWITCHING_PROTOCOLS && name == b"upgrade" {
        if request::token_list_contains(data, b"websocket") != 0 {
            client.websocket = true;
        }
    } else if features() & 1 != 0
        && client.websocket
        // SAFETY: the client's WebSocket state.
        && unsafe { native::swoole_http_client_coro_ws_compression(client.ws) }
        && name == b"sec-websocket-extensions"
    {
        if request::contains_ignore_case(data, b"permessage-deflate") {
            client.accept_websocket_compression = true;
        }
    } else if name == b"set-cookie" {
        let cookies = props::array(client.object, c"cookies");
        let headers = props::array(client.object, c"set_cookie_headers");
        // SAFETY: the value's bytes and two live array properties.
        unsafe { swr_v1_phpclients_parse_set_cookies(at, length, cookies, headers) };
    } else if features() != 0 && name == b"content-encoding" {
        if let Some(method) = request::content_encoding(data, features()) {
            client.compress = method;
        }
    } else if name == b"transfer-encoding" && request::starts_with_ignore_case(data, b"chunked") {
        client.chunked = true;
    } else if name == b"connection" {
        client.connection_close = request::starts_with_ignore_case(data, b"close");
    } else if name == b"content-type" {
        client.event_stream = request::starts_with_ignore_case(data, b"text/event-stream");
    }
    0
}

unsafe extern "C" fn on_headers_complete(parser: *mut Parser) -> c_int {
    // SAFETY: llhttp calls back with the client's parser.
    let client = unsafe { parser_client(parser) };
    c_int::from(client.head || client.parser.status_code == NO_CONTENT)
}

unsafe extern "C" fn on_body(parser: *mut Parser, at: *const c_char, length: usize) -> c_int {
    // SAFETY: llhttp calls back with the client's parser.
    let client = unsafe { parser_client(parser) };
    // SAFETY: llhttp's span points into the buffer being parsed.
    let data = unsafe { std::slice::from_raw_parts(at.cast::<u8>(), length) };
    let decompress =
        client.body_decompression && !client.compression_error && client.compress != Compress::None;
    if !client.write_func.is_null() {
        let mut chunk = data;
        if decompress {
            if client.decompress(data) {
                if client.body.is_empty() {
                    return 0;
                }
                chunk = &client.body;
            } else {
                client.compression_error = true;
            }
        }
        let (callable, object) = (client.write_func, client.object);
        let (pointer, size) = (chunk.as_ptr(), chunk.len());
        client.in_callback = true;
        // SAFETY: the client's callable, its object and the chunk's bytes, which
        // live until the call returns (the body is cleared after).
        let result =
            unsafe { native::swoole_http_client_coro_call(callable, object, pointer.cast(), size) };
        // SAFETY: the callback may have reentered the client; read it afresh.
        let client = unsafe { parser_client(parser) };
        client.body.clear();
        client.in_callback = false;
        if result == 2 {
            client.bailout = true;
        }
        return if result == 0 { 0 } else { -1 };
    }
    if decompress {
        if !client.decompress(data) {
            client.compression_error = true;
            client.body.extend_from_slice(data);
        }
    } else {
        client.body.extend_from_slice(data);
    }
    if let Some(name) = &client.download_name {
        if client.body.is_empty() {
            return 0;
        }
        if client.download_file.is_null() {
            let path = c_bytes(name.bytes());
            // SAFETY: a NUL-terminated path; the file is the client's to free.
            client.download_file = unsafe {
                native::swoole_http_client_coro_file_open(path.as_ptr(), client.download_offset)
            };
            if client.download_file.is_null() {
                return -1;
            }
        }
        // SAFETY: the client's open file and its body bytes.
        let written = unsafe {
            native::swoole_http_client_coro_file_write(
                client.download_file,
                client.body.as_ptr().cast(),
                client.body.len(),
            )
        };
        if !written {
            return -1;
        }
        client.body.clear();
    }
    0
}

unsafe extern "C" fn on_message_complete(parser: *mut Parser) -> c_int {
    // SAFETY: llhttp calls back with the client's parser.
    let client = unsafe { parser_client(parser) };
    client.completed = true;
    if client.parser.upgrade != 0 && !client.websocket {
        client.parser.upgrade = 0;
        return llhttp::PAUSED;
    }
    props::long(
        client.object,
        c"statusCode",
        i64::from(client.parser.status_code),
    );
    if client.download_file.is_null() {
        props::bytes(client.object, c"body", &client.body);
    } else {
        client.download_name = None;
    }
    llhttp::PAUSED
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; zeroed data is no client.
    unsafe { ClientObject::create(&CLIENT, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees a client object.
    let data = unsafe { ClientObject::data(std) };
    let client = std::mem::replace(&mut data.client, ptr::null_mut());
    if !client.is_null() {
        // SAFETY: the client the constructor boxed, dropped once.
        drop(unsafe { Box::from_raw(client) });
    }
    // SAFETY: as above.
    unsafe { ClientObject::dtor(std) };
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ClientObject::install(entry, handlers, create, free);
}

/// MINIT, as `php_swoole_http_client_coro_minit` did: the class (and short
/// name), the `SensitiveParameter` on `setBasicAuth`'s password, the
/// properties, the exception class and the error-status constants.
/// # Safety
/// Called once, at MINIT of module `module`, with upstream's static method
/// table and Swoole's exception class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_http_client_minit(
    methods: *const zend_function_entry,
    exception: *mut zend_class_entry,
    shortname: bool,
    module: c_int,
    features: u32,
) {
    FEATURES.store(features, Ordering::Relaxed);
    // SAFETY: MINIT, with the caller's table.
    unsafe { CLIENT.register(c"Swoole\\Coroutine\\Http\\Client", methods, configure) };
    if shortname {
        // SAFETY: MINIT, after `register`.
        unsafe { CLIENT.alias(c"Co\\Http\\Client") };
    }
    let ce = CLIENT.entry();
    // SAFETY: MINIT, with the method the table declares.
    unsafe { swr_zend_sensitive_parameter(ce, c"setbasicauth".as_ptr(), 12, 1) };
    let public = ZEND_ACC_PUBLIC;
    // SAFETY: MINIT, with the class just registered (each declaration below too).
    unsafe { CLIENT.property_null(c"socket") };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"errCode", 0, public) };
    // SAFETY: as above.
    unsafe { declare_string(ce, c"errMsg", c"", public) };
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"connected", false, public) };
    // SAFETY: as above.
    unsafe { declare_string(ce, c"host", c"", public) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"port", 0, public) };
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"ssl", false, public) };
    for name in [
        c"setting",
        c"requestMethod",
        c"requestHeaders",
        c"requestBody",
        c"uploadFiles",
        c"downloadFile",
    ] {
        // SAFETY: as above.
        unsafe { CLIENT.property_null(name) };
    }
    // SAFETY: as above.
    unsafe { declare_long(ce, c"downloadOffset", 0, public) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"statusCode", 0, public) };
    for name in [c"headers", c"set_cookie_headers", c"cookies"] {
        // SAFETY: as above.
        unsafe { CLIENT.property_null(name) };
    }
    // SAFETY: as above.
    unsafe { declare_string(ce, c"body", c"", public) };
    // SAFETY: MINIT, extending Swoole's registered exception class.
    unsafe {
        EXCEPTION.register_child(
            c"Swoole\\Coroutine\\Http\\Client\\Exception",
            ptr::null(),
            exception,
            |_, _| {},
        );
    }
    if shortname {
        // SAFETY: as above.
        unsafe { EXCEPTION.alias(c"Co\\Http\\Client\\Exception") };
    }
    for (name, status) in [
        (c"SWOOLE_HTTP_CLIENT_ESTATUS_CONNECT_FAILED", CONNECT_FAILED),
        (
            c"SWOOLE_HTTP_CLIENT_ESTATUS_REQUEST_TIMEOUT",
            REQUEST_TIMEOUT,
        ),
        (c"SWOOLE_HTTP_CLIENT_ESTATUS_SERVER_RESET", SERVER_RESET),
        (c"SWOOLE_HTTP_CLIENT_ESTATUS_SEND_FAILED", SEND_FAILED),
    ] {
        // SAFETY: MINIT of `module`.
        unsafe { register_long_constant(name, status, module) };
    }
}

/// `$this`'s data.
fn this<'a>(call: &Call) -> &'a mut Data {
    // SAFETY: client methods are non-static, so `$this` is a client object.
    unsafe { ClientObject::data(call.this()) }
}

/// `http_client_coro_get_client`: the constructed client; a fatal error if not.
fn client<'a>(call: &Call) -> &'a mut Client {
    let client = this(call).client;
    if client.is_null() {
        // SAFETY: a fatal error; it does not return.
        unsafe { swoole_socket_coro_unconstructed() };
    }
    // SAFETY: a constructed client lives as long as its object.
    unsafe { &mut *client }
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
    if !this(call).client.is_null() {
        // SAFETY: from a handler, with `$this`.
        unsafe { throw_constructed_twice(object) };
        return call.set_bool(false);
    }
    props::bytes(object, c"host", host);
    props::long(object, c"port", port);
    props::bool(object, c"ssl", ssl);
    if host.is_empty() {
        // SAFETY: from a handler, with the registered exception class.
        unsafe { throw_exception(EXCEPTION.entry(), c"host is empty", i64::from(libc::EINVAL)) };
        return call.set_bool(false);
    }
    this(call).client = Box::into_raw(Client::new(object, host, port, ssl));
}

fn destruct(_call: &Call) {}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    let client = client(call);
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: an array argument.
    if unsafe { table::length(settings) } == 0 {
        return call.set_bool(false);
    }
    // SAFETY: `$this` declares `setting`; the settings are a live array.
    unsafe { swr_zend_property_merge(ce(), call.this(), c"setting".as_ptr(), 7, settings) };
    client.apply_setting(settings, true);
    call.set_bool(true);
}

fn get_defer(call: &Call) {
    call.set_bool(client(call).defer);
}

/// `setDefer(bool $defer = true): bool`.
fn set_defer(call: &Call) {
    let client = client(call);
    let Ok(defer) = call.arity(0, 1).and_then(|()| call.bool(1, true)) else {
        return call.set_bool(false);
    };
    client.defer = defer;
    call.set_bool(true);
}

/// `setMethod(string $method): bool`.
fn set_method(call: &Call) {
    let Ok(method) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    props::bytes(call.this(), c"requestMethod", method);
    call.set_bool(true);
}

/// `Z_PARAM_ARRAY_EX(.., 0, 1)` then a property update: the stored array is
/// the argument, separated.
fn set_array_property(call: &Call, name: &CStr) {
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: an array argument the frame owns.
    unsafe { swoole_rs_zend::sys::swr_zend_separate_array(array) };
    props::zval(call.this(), name, array);
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
    let client = client(call);
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
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    props::zval(call.this(), c"requestBody", call.zval(1));
    call.set_bool(true);
}

/// The upload entry `addFile` and `addData` append.
fn add_upload(call: &Call, entry: &[(&CStr, Upload<'_>)]) {
    let files = props::array(call.this(), c"uploadFiles");
    let mut file = MaybeUninit::<zval>::uninit();
    // SAFETY: a writable zval.
    unsafe { value::array_init(file.as_mut_ptr()) };
    for (key, item) in entry {
        match item {
            Upload::Text(text) => {
                // SAFETY: a live array; Zend copies the key and bytes.
                unsafe {
                    swoole_rs_zend::sys::add_assoc_stringl_ex(
                        file.as_mut_ptr(),
                        key.as_ptr(),
                        key.count_bytes(),
                        text.as_ptr().cast(),
                        text.len(),
                    );
                }
            }
            // SAFETY: as above.
            Upload::Number(number) => unsafe { value::add_long(file.as_mut_ptr(), key, *number) },
        }
    }
    // SAFETY: a live array property takes the entry.
    let added = unsafe { table::push(files, file.as_mut_ptr()) };
    call.set_bool(added);
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
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a NUL-terminated path and a writable stat buffer.
    if unsafe { libc::stat(c_path.as_ptr(), stat.as_mut_ptr()) } < 0 {
        let message = [&b"stat("[..], c_path.to_bytes(), b") failed"].concat();
        // SAFETY: a NUL-terminated message; errno is stat's.
        unsafe { native::swoole_http_client_coro_sys_error(c_bytes(&message).as_ptr()) };
        return call.set_bool(false);
    }
    // SAFETY: stat filled it.
    let size = unsafe { stat.assume_init() }.st_size;
    if size == 0 {
        let shown = filename.map_or(&b"(null)"[..], |filename| filename);
        let message = [&b"cannot send empty file["[..], shown, b"]"].concat();
        // SAFETY: as above.
        unsafe { native::swoole_http_client_coro_sys_error(c_bytes(&message).as_ptr()) };
        return call.set_bool(false);
    }
    if size <= offset {
        let message = format!("parameter $offset[{offset}] exceeds the file size");
        warn(false, &CString::new(message).unwrap_or_default());
        return call.set_bool(false);
    }
    if length > size - offset {
        let message = format!("parameter $length[{length}] exceeds the file size");
        // SAFETY: as above.
        unsafe {
            native::swoole_http_client_coro_sys_error(
                CString::new(message).unwrap_or_default().as_ptr(),
            );
        };
        return call.set_bool(false);
    }
    if length == 0 {
        length = size - offset;
    }
    let mime;
    if kind.is_empty() {
        // SAFETY: a NUL-terminated path; the type text is static.
        let text = unsafe { native::swoole_http_client_coro_mime(c_path.as_ptr()) };
        // SAFETY: as above.
        mime = unsafe { CStr::from_ptr(text) };
        kind = mime.to_bytes();
    }
    let filename = if let Some(filename) = filename.filter(|filename| !filename.is_empty()) {
        filename
    } else {
        let path = c_path.to_bytes();
        path.iter()
            .rposition(|&b| b == b'/')
            .map_or(path, |slash| &path[slash + 1..])
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
fn finish(client: &mut Client, call: &Call, result: bool) -> bool {
    let bailout = std::mem::take(&mut client.bailout);
    if !bailout {
        call.set_bool(result);
    }
    bailout
}

/// The path argument of `execute`, `get`, `upgrade`.
fn path_arg<'a>(call: &Call) -> Option<&'a [u8]> {
    let path = call.arity(1, 1).and_then(|()| call.string(1, b"")).ok();
    if path.is_none() {
        call.set_bool(false);
    }
    path
}

/// A method that can run PHP: its body, and whether it trapped a bailout.
macro_rules! calling {
    ($($name:ident => $body:ident;)*) => {$(
        /// A Zend handler body for a C++ trampoline.
        /// # Safety
        /// Called by the trampoline with Zend's live frame and return slot.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            frame: *mut swoole_rs_zend::sys::zend_execute_data,
            value: *mut zval,
        ) -> bool {
            // SAFETY: per the trampoline.
            $body(&unsafe { Call::new(frame, value) })
        }
    )*};
}

calling! {
    swr_v1_phpclients_http_client_execute => execute;
    swr_v1_phpclients_http_client_get => get;
    swr_v1_phpclients_http_client_post => post;
    swr_v1_phpclients_http_client_download => download;
    swr_v1_phpclients_http_client_upgrade => upgrade;
    swr_v1_phpclients_http_client_recv => recv;
}

fn execute(call: &Call) -> bool {
    let client = client(call);
    let Some(path) = path_arg(call) else {
        return false;
    };
    let result = client.exec(path);
    finish(client, call, result)
}

fn get(call: &Call) -> bool {
    let client = client(call);
    let Some(path) = path_arg(call) else {
        return false;
    };
    props::string(call.this(), c"requestMethod", c"GET");
    let result = client.exec(path);
    finish(client, call, result)
}

/// `post(string $path, mixed $data): bool`.
fn post(call: &Call) -> bool {
    let client = client(call);
    let Ok(path) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return false;
    };
    props::string(call.this(), c"requestMethod", c"POST");
    props::zval(call.this(), c"requestBody", call.zval(2));
    let result = client.exec(path);
    finish(client, call, result)
}

/// `download(string $path, string $file, int $offset = 0): bool`.
fn download(call: &Call) -> bool {
    let client = client(call);
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(3, 0)?)));
    let Ok((path, offset)) = parsed else {
        call.set_bool(false);
        return false;
    };
    props::zval(call.this(), c"downloadFile", call.zval(2));
    props::long(call.this(), c"downloadOffset", offset);
    let result = client.exec(path);
    finish(client, call, result)
}

fn upgrade(call: &Call) -> bool {
    let client = client(call);
    let Some(path) = path_arg(call) else {
        return false;
    };
    let result = client.upgrade(path);
    finish(client, call, result)
}

/// `recv(float $timeout = 0)`: a WebSocket frame, or the deferred response.
fn recv(call: &Call) -> bool {
    let client = client(call);
    if !client.is_available() {
        call.set_bool(false);
        return false;
    }
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return false;
    };
    let _hold = Hold::new(client.zsocket());
    if client.websocket {
        client.recv_websocket_frame(call, timeout);
        return false;
    }
    let result = client.recv_response(timeout);
    finish(client, call, result)
}

/// `push(mixed $data, int $opcode = TEXT, int $flags = FIN): bool`.
fn push(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let parsed = call.arity(1, 3).and_then(|()| call.long(2, OPCODE_TEXT));
    let Ok(opcode) = parsed else {
        return call.set_bool(false);
    };
    let flags = call.zval(3);
    // SAFETY: a passed argument (null when absent).
    let flags = if flags.is_null() || unsafe { table::is_null(flags) } {
        FLAG_FIN
    } else {
        // SAFETY: as above.
        unsafe { table::long(flags) }
    };
    let _hold = Hold::new(client.zsocket());
    let result = client.push(call.zval(1), opcode, flags & FLAGS_ALL, CLOSE_NORMAL);
    call.set_bool(result);
}

/// `close(): bool`.
fn close(call: &Call) {
    let client = client(call);
    let Some(_hold) = Hold::new(client.zsocket()) else {
        set_error_properties(call.this(), NO_CONNECTION, strerror(NO_CONNECTION));
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
    let client = client(call);
    let mut length = 0;
    // SAFETY: the live socket or the buffer its close left.
    let saved = unsafe { (*client.conn).written };
    // SAFETY: as above.
    let written =
        unsafe { native::swoole_http_client_coro_written(client.socket(), saved, &raw mut length) };
    if written.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: the buffer's `length` bytes.
    let data = unsafe { std::slice::from_raw_parts(written.cast::<u8>(), length) };
    match request::header_end(data) {
        Some(end) if end > 0 => {
            // SAFETY: the return slot takes the new string's reference.
            unsafe { call.set_string(string::copy(&data[..end])) };
        }
        _ => call.set_bool(false),
    }
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let socket = client.socket();
    // SAFETY: a connected socket.
    if !unsafe { swoole_socket_coro_getsockname(socket) } {
        socket_error_properties(call.this(), socket);
        return call.set_bool(false);
    }
    call.set_array();
    let mut port = 0;
    // SAFETY: as above, and a writable local.
    let address = unsafe { swoole_socket_coro_address(socket, &raw mut port) };
    // SAFETY: the socket's NUL-terminated address.
    set_endpoint(call, unsafe { CStr::from_ptr(address) }, port);
}

fn set_endpoint(call: &Call, address: &CStr, port: c_int) {
    call.set_array();
    // SAFETY: the return slot holds an array.
    unsafe { value::add_string(call.return_value(), c"address", address) };
    call.add_assoc_long(c"port", i64::from(port));
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let socket = client.socket();
    let mut address = [0 as c_char; 256];
    let mut port = 0;
    // SAFETY: a connected socket and writable locals of the sizes given.
    let found = unsafe {
        swoole_socket_coro_getpeername(socket, address.as_mut_ptr(), address.len(), &raw mut port)
    };
    if !found {
        socket_error_properties(call.this(), socket);
        return call.set_bool(false);
    }
    // SAFETY: the hook NUL-terminates what it writes.
    set_endpoint(call, unsafe { CStr::from_ptr(address.as_ptr()) }, port);
}

/// `getPeerCert(): string|false`.
fn get_peer_cert(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let socket = client.socket();
    // SAFETY: a connected socket.
    let cert = unsafe { native::swoole_http_client_coro_peer_cert(socket) };
    if cert.is_null() {
        socket_error_properties(call.this(), socket);
        return call.set_bool(false);
    }
    // SAFETY: the return slot takes the new string's reference.
    unsafe { call.set_string(cert) };
}

/// A string zval for the payload of `ping` and `disconnect`: the argument, or
/// the empty string.
fn payload(call: &Call, n: u32, string: Option<*mut zend_string>) -> zval {
    let mut payload = MaybeUninit::<zval>::uninit();
    match string {
        // SAFETY: a writable zval borrowing the frame's string (no reference).
        Some(string) => unsafe { value::set_str(payload.as_mut_ptr(), string) },
        // SAFETY: a writable zval.
        None => unsafe { swr_zval_empty_string(payload.as_mut_ptr()) },
    }
    let _ = (call, n);
    // SAFETY: initialised above.
    unsafe { payload.assume_init() }
}

/// `ping(string $data = ''): bool`.
fn ping(call: &Call) {
    let client = client(call);
    if !client.is_available() || !client.websocket {
        return call.set_bool(false);
    }
    let Ok(data) = call.arity(0, 1).and_then(|()| call.str(1)) else {
        return call.set_bool(false);
    };
    let mut data = payload(call, 1, data);
    let result = client.push(&raw mut data, OPCODE_PING, FLAG_FIN, CLOSE_NORMAL);
    call.set_bool(result);
}

/// `disconnect(int $code = 1000, string $reason = ''): bool`: push a close
/// frame, then close regardless.
fn disconnect(call: &Call) {
    let client = client(call);
    if !client.is_available() || !client.websocket {
        return call.set_bool(false);
    }
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, CLOSE_NORMAL)?, call.str(2)?)));
    let Ok((code, reason)) = parsed else {
        return call.set_bool(false);
    };
    let mut reason = payload(call, 2, reason);
    let result = client.push(&raw mut reason, OPCODE_CLOSE, FLAG_FIN, code);
    call.set_bool(result);
    client.close(true);
}

swoole_rs_zend::handlers! {
    zim_swoole_http_client_coro___construct => construct;
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
