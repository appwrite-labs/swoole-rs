//! `Swoole\Coroutine\Http\Server`: registration, the object and upstream's
//! `swoole::coroutine::HttpServer` (handlers, connections, settings),
//! `__construct`, `set`, `handle`, `start`, `shutdown`, the per-connection
//! request loop (`onAccept`), HTTP/2 on a connection and its request
//! dispatch, over the coroutine sockets through `phpruntime::co`.
use crate::{
    context::{self, CompressionTypes, Ctx},
    host,
    parse::{Key, add_protocol, add_server, parse_request},
    server::{Callable, NO_THEN, call_handler},
    websocket,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    ffi::{CString, c_int},
    ptr,
    rc::Rc,
    sync::atomic::Ordering::Relaxed,
};
use swoole_rs_buffer::string::CppString as Buffer;
use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
use swoole_rs_hooks::{co_socket::Co, packet::recv_packet};
use swoole_rs_phpclients::{get_socket, is_socket_closed, php_swoole_socket_set_protocol};
use swoole_rs_phpruntime::co;
use swoole_rs_phpstubs::http_server_coro;
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_zend::{
    Array, Call, Custom, CustomClass, Key as ArrayKey, Minit, Owned, OwnedArray, Props, Value,
    ZString, bailout, handlers, method_table, string,
    sys::{self as z, ZEND_ACC_FINAL, ZEND_ACC_PUBLIC, zend_long, zval},
};

static SERVER: CustomClass<Data> = CustomClass::new();
/// `SW_HAVE_COMPRESSION`.
static COMPRESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `struct HttpServerObject { HttpServer *server; zend_object std; }`: the
/// server `__construct` makes, dropped with the object.
struct Data(OnceCell<HttpServer>);

impl Custom for Data {
    fn class() -> &'static CustomClass<Self> {
        &SERVER
    }

    fn new() -> Self {
        Self(OnceCell::new())
    }
}

/// Upstream's `swoole::coroutine::HttpServer`. Its connections' coroutines
/// share it, so every field that changes is a cell.
struct HttpServer {
    /// The listening coroutine socket.
    socket: Co,
    /// The same socket as crates/phpclients' handle, for its accept and its
    /// length callable.
    listener: swoole_rs_phpclients::Co,
    /// `handlers`, by pattern, in the order they were first set. Upstream's
    /// `std::unordered_map` leaves the order among overlapping prefixes to
    /// the standard library's hashing; registration order is ours.
    handlers: RefCell<Vec<(Vec<u8>, Rc<Callable>)>>,
    /// `default_handler`: the `/` handler.
    default_handler: RefCell<Option<Rc<Callable>>>,
    running: Cell<bool>,
    /// The connections, by the coroutine serving each.
    clients: Owned,
    parse_cookie: Cell<bool>,
    parse_post: Cell<bool>,
    parse_files: Cell<bool>,
    compression: Cell<bool>,
    compression_level: Cell<i8>,
    compression_min_length: Cell<u32>,
    /// `http_compression_types`, once set.
    compression_types: RefCell<Option<CompressionTypes>>,
    /// `WebSocketSettings` (`websocket::` bits): `start` always applies them,
    /// so the contexts see a server's.
    websocket_settings: Cell<u8>,
    upload_tmp_dir: RefCell<Rc<CString>>,
}

/// `SW_Z_BEST_SPEED` and `SW_COMPRESSION_MIN_LENGTH_DEFAULT`.
const Z_BEST_SPEED: i8 = 1;
const COMPRESSION_MIN_LENGTH_DEFAULT: u32 = 20;

impl HttpServer {
    fn new(socket: Co) -> Self {
        Self {
            socket,
            // The listening socket, which the server frees in its drop.
            listener: swoole_rs_phpclients::Co::from(socket),
            handlers: RefCell::new(Vec::new()),
            default_handler: RefCell::new(None),
            running: Cell::new(true),
            clients: OwnedArray::default().into_owned(),
            parse_cookie: Cell::new(true),
            parse_post: Cell::new(true),
            parse_files: Cell::new(false),
            compression: Cell::new(true),
            compression_level: Cell::new(Z_BEST_SPEED),
            compression_min_length: Cell::new(COMPRESSION_MIN_LENGTH_DEFAULT),
            compression_types: RefCell::new(None),
            #[expect(clippy::cast_possible_truncation, reason = "the bit fits")]
            websocket_settings: Cell::new(websocket::IN_SERVER as u8),
            upload_tmp_dir: RefCell::new(Rc::new(c"/tmp".to_owned())),
        }
    }

    /// `WebSocket::apply_setting(hs->websocket_settings, vht, true)`, the
    /// settings a coroutine server's contexts read.
    #[expect(clippy::cast_possible_truncation, reason = "the bits fit")]
    fn apply_websocket(&self, table: &Array) {
        let mut bits = self.websocket_settings.get();
        let mut flag = |key: &[u8], bit: u8| {
            if let Some(found) = table.get(key) {
                if found.truthy() {
                    bits |= bit;
                } else {
                    bits &= !bit;
                }
            }
        };
        if websocket::ZLIB.load(Relaxed) {
            flag(b"websocket_compression", websocket::SETTING_COMPRESSION);
        }
        flag(
            b"open_websocket_close_frame",
            websocket::OPEN_CLOSE_FRAME as u8,
        );
        flag(
            b"open_websocket_ping_frame",
            websocket::OPEN_PING_FRAME as u8,
        );
        flag(
            b"open_websocket_pong_frame",
            websocket::OPEN_PONG_FRAME as u8,
        );
        self.websocket_settings
            .set(bits | websocket::IN_SERVER as u8);
    }

    fn clients(&self) -> *mut z::HashTable {
        self.clients.array().map_or(ptr::null_mut(), Array::as_ptr)
    }

    /// `set_handler`: `handle` replaces the pattern's handler, if any.
    fn set_handler(&self, pattern: &[u8], handle: Rc<Callable>) -> Result<(), bailout::Bailout> {
        let old_default =
            (pattern == b"/").then(|| self.default_handler.replace(Some(handle.clone())));
        let mut handlers = self.handlers.borrow_mut();
        let mut old = None;
        if let Some(entry) = handlers.iter_mut().find(|(known, _)| known == pattern) {
            old = Some(std::mem::replace(&mut entry.1, handle));
        } else {
            handlers.push((pattern.to_vec(), handle));
        }
        drop(handlers);
        // The handler this one replaces, released once no borrow is live.
        drop((old_default, old));
        bailout::outcome(())
    }

    /// `get_handler`: the callable for the request's path, if any.
    fn handler(&self, mut context: Ctx) -> Option<Rc<Callable>> {
        let path = crate::parse::request_path(&context.state().request);
        let default = self.default_handler.borrow().clone();
        let is_default =
            |handle: &Rc<Callable>| default.as_ref().is_some_and(|d| Rc::ptr_eq(d, handle));
        let found = self
            .handlers
            .borrow()
            .iter()
            .filter(|(_, handle)| !is_default(handle))
            .find(|(pattern, _)| swoole_rs_httpbindings::handler_matches(path, pattern))
            .map(|(_, handle)| handle.clone());
        found.or(default)
    }

    /// A flag's or a number's setting (`SWR_HTTPBINDINGS_SETTING_*`).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's uint8_t level and uint32_t minimum"
    )]
    fn apply(&self, kind: u32, value: i64) {
        match kind {
            SETTING_COOKIE => self.parse_cookie.set(value != 0),
            SETTING_POST => self.parse_post.set(value != 0),
            SETTING_FILES => self.parse_files.set(value != 0),
            SETTING_COMPRESSION => self.compression.set(value != 0),
            SETTING_LEVEL => self.compression_level.set(value as i8),
            SETTING_MINIMUM => self.compression_min_length.set(value as u32),
            _ => {}
        }
    }

    /// `create_context`: the settings, the socket and the parser.
    fn context(&self, socket: Co, zconn: &Value) -> Ctx {
        let mut context = context::context_new(i64::from(socket.sock_fd()));
        crate::parse::init_request_parser(context);
        if let Some(response) = context.state().response.zobject.props() {
            response.value(c"socket", zconn);
        }
        context::bind(context.state(), zconn);
        let state = context.state();
        state
            .compression_types
            .clone_from(&self.compression_types.borrow());
        state.upload_tmp_dir = Some(self.upload_tmp_dir.borrow().clone());
        state.websocket_settings = self.websocket_settings.get();
        state.parse_body = u8::from(self.parse_post.get());
        state.parse_cookie = u8::from(self.parse_cookie.get());
        state.parse_files = u8::from(self.parse_files.get());
        if COMPRESSION.load(Relaxed) {
            state.enable_compression = u8::from(self.compression.get());
            state.compression_level = self.compression_level.get();
            state.compression_min_length = self.compression_min_length.get();
        }
        context
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        // Releasing a handler defers what it raises.
        drop(self.default_handler.take());
        drop(self.handlers.take());
        // Each connection frees its own copy (`Co::accept`); the listener owns the original.
        bailout::defer(self.listener.free_length_callable());
        // SAFETY: the listening socket `__construct` made, freed once.
        bailout::defer(unsafe { co::free(self.socket.this()) });
    }
}

/// A text setting (`SWR_HTTPBINDINGS_SETTING_*`): `http_compression_types`
/// (reset by `None`, then one type per call) or `upload_tmp_dir`.
fn text_setting(server: &HttpServer, kind: u32, text: Option<&[u8]>) {
    if kind != SETTING_TYPES {
        *server.upload_tmp_dir.borrow_mut() = context::directory(text.unwrap_or_default());
        return;
    }
    let mut types = server.compression_types.borrow_mut();
    match text {
        None => *types = Some(Rc::default()),
        Some(kind) => {
            if let Some(types) = types.as_mut() {
                Rc::make_mut(types).insert(kind.into());
            }
        }
    }
}

/// HTTP statuses the loop answers with, and llhttp's codes it checks.
const BAD_REQUEST: c_int = 400;
const NOT_FOUND: c_int = 404;
/// `SW_HTTP2_ERROR_INTERNAL_ERROR`.
const HTTP2_INTERNAL_ERROR: u32 = 2;
const ENTITY_TOO_LARGE: c_int = 413;
const HPE_OK: i32 = 0;
const SW_ERR: c_int = -1;
const HPE_PAUSED_H2_UPGRADE: i32 = 23;
/// `SW_BUFFER_SIZE_BIG`.
const BUFFER_SIZE_BIG: usize = 65536;
/// `SW_HTTP2_PRI_STRING`.
const HTTP2_PRI: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// `EINVAL`.
const EINVAL: zend_long = 22;

/// The addresses `onAccept` puts in each request's `$server`: the strings
/// are references the loop releases.
struct Peer {
    remote_addr: ZString,
    server_addr: ZString,
    remote_port: c_int,
    server_port: c_int,
}

/// The socket's error code and message.
fn socket_error(socket: Co) -> (c_int, CString) {
    (socket.err_code(), socket.err_message().unwrap_or_default())
}

/// `get_addr()` and `get_port()`: the network socket's address.
fn address(socket: Co) -> (ZString, c_int) {
    let address = socket.with_network(|network| {
        network.read(|record| {
            let info = &record.info;
            let host = with_host(info, |host| ZString::new(host.to_bytes()));
            (host, swr_v1_clients_address_port(info))
        })
    });
    address.unwrap_or_else(|| (ZString::new(b""), 0))
}

/// `SW_EVENT_READ`.
const EVENT_READ: u32 = 1 << 9;
/// `SW_HTTP2_FRAME_HEADER_SIZE`.
const HTTP2_FRAME_HEADER_SIZE: u8 = 9;

/// `zend::assign_zend_string_by_val(&request.zdata, buffer->pop(SW_BUFFER_SIZE_BIG), length)`:
/// the request's `length` bytes, in a zend string the buffer allocated,
/// become `zdata`; the buffer starts a new allocation.
fn request_data(mut context: Ctx, buffer: &mut Buffer, length: usize) {
    let data = buffer.pop(BUFFER_SIZE_BIG);
    let string = string::from_val(data);
    // SAFETY: the zend string allocator's unshared string, holding `length`
    // bytes and room for the NUL; its one reference moves into `zdata`.
    let string = unsafe { string::ZBuf::from_raw(string, length) };
    context.state().request.zdata = Owned::string(string.into_zstring());
}

/// HTTP/2 on the connection: the server's SETTINGS frame, then frames by
/// their length (`swoole::http2::get_frame_length`).
fn http2_begin(socket: Co) {
    let settings = swoole_rs_http2::sw_rs_http2_default_settings();
    let mut frame = [0u8; 45];
    let length = swoole_rs_http2::sw_rs_http2_pack_settings(&mut frame, &settings, true);
    let frame = frame.get(..length).unwrap_or(&frame);
    socket.with_network(|network| swoole_rs_tls::upstream::send(network, frame, 0));
    socket.set_open_length_check(true);
    let mut protocol = socket.protocol();
    protocol.package_length_size = HTTP2_FRAME_HEADER_SIZE;
    protocol.package_length_offset = 0;
    protocol.package_body_offset = 0;
    protocol.get_package_length = Some(swoole_rs_ports::http_routes::exports::http2_length);
    socket.set_protocol(protocol);
}

/// `recv_wait`: the socket waits for a request.
fn set_recv_wait(socket: Co, waiting: bool) {
    socket.with_network(|network| network.edit(|record| record.recv_wait = u8::from(waiting)));
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_http_server_coro___construct, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server___construct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_http_server_coro_set, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_set, z::ZEND_ACC_PUBLIC;
        c"handle" => zim_swoole_http_server_coro_handle, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_handle, z::ZEND_ACC_PUBLIC;
        c"onAccept" => zim_swoole_http_server_coro_onAccept, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_onAccept, z::ZEND_ACC_PRIVATE;
        c"start" => zim_swoole_http_server_coro_start, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_start, z::ZEND_ACC_PUBLIC;
        c"shutdown" => zim_swoole_http_server_coro_shutdown, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_shutdown, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: the class, as `php_swoole_http_server_coro_minit` registered it.
pub fn server_coro_minit(args: &MinitArgs) {
    let minit = &args.minit;
    if let Err(error) = register(minit, args.shortname, args.compression) {
        error.defer();
        swoole_rs_zend::bailout::finish();
    }
}

fn register(minit: &Minit, short_name: bool, compression: bool) -> Result<(), bailout::Bailout> {
    COMPRESSION.store(compression, Relaxed);
    // What releasing the handlers or the socket defers is raised as the
    // object is freed.
    minit.register_object::<Data>(
        c"Swoole\\Coroutine\\Http\\Server",
        &METHODS.0,
        ZEND_ACC_FINAL,
    );
    if short_name {
        minit.alias(&SERVER, c"Co\\Http\\Server")?;
    }
    for (name, value) in [(c"fd", -1), (c"port", -1)] {
        if name == c"port" {
            // Upstream's order.
            minit.property_null(&SERVER, c"host", ZEND_ACC_PUBLIC);
        }
        minit.property_long(&SERVER, name, value, ZEND_ACC_PUBLIC);
    }
    minit.property_bool(&SERVER, c"ssl", false, ZEND_ACC_PUBLIC);
    minit.property_null(&SERVER, c"settings", ZEND_ACC_PUBLIC);
    minit.property_long(&SERVER, c"errCode", 0, ZEND_ACC_PUBLIC);
    minit.property_string(&SERVER, c"errMsg", c"", ZEND_ACC_PUBLIC);
    Ok(())
}

/// `$this`'s server, once constructed. The frame holds `$this` for the
/// call, and the connections' coroutines run inside `onAccept` calls.
fn server_of(call: &Call) -> Option<&HttpServer> {
    call.custom::<Data>()?.0.get()
}

/// `http_server_coro_set_error`: the socket's error into `errCode`/`errMsg`.
fn set_error(props: Props<'_>, socket: Co) -> zend_long {
    let (code, message) = socket_error(socket);
    props.long(c"errCode", zend_long::from(code));
    props.bytes(c"errMsg", message.to_bytes());
    zend_long::from(code)
}

fn exception(call: &Call, code: zend_long, message: &str) {
    let message = std::ffi::CString::new(message).unwrap_or_default();
    call.throw(swoole_rs_phpswoole::exception(), &message, code);
}

fn construct(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call.arity(1, 4).and_then(|()| {
            Ok((
                call.string(1, b"")?,
                call.long(2, 0)?,
                call.bool(3, false)?,
                call.bool(4, false)?,
            ))
        });
        let (host, port, ssl, reuse_port) = parsed.ok()?;
        let (data, props) = (call.custom::<Data>()?, call.props(Some(&SERVER))?);
        if data.0.get().is_some() {
            call.throw_constructed_twice();
            return None;
        }
        props.bytes(c"host", host);
        props.bool(c"ssl", ssl);
        if host.is_empty() {
            exception(call, EINVAL, "host is empty");
            return None;
        }
        let kind = swoole_rs_reactor::host_type(host);
        let Ok(socket) = co::make(-1, 0, kind.socket_type, 0, false) else {
            bailout::Bailout.defer();
            return None;
        };
        // The server owns the new socket until it drops.
        if data.0.set(HttpServer::new(socket)).is_err() {
            return None;
        }
        if reuse_port {
            set_reuse_port(socket.sock_fd());
        }
        #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
        if !socket.bind(host, port as c_int) {
            let code = set_error(props, socket);
            #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) port")]
            let port = port as i32;
            exception(
                call,
                code,
                &format!(
                    "bind({}:{port}) failed",
                    String::from_utf8_lossy(nul_terminated(host))
                ),
            );
            return None;
        }
        if ssl {
            // ssl_check_context runs once the user has set the protocols.
            // SAFETY: nothing else borrows `settings` while one key is added.
            if let Some(settings) = unsafe { props.array(c"settings") } {
                settings.insert_bool(b"open_ssl", true);
            }
        }
        if ssl {
            let _enabled = socket.enable_ssl();
        }
        if !socket.listen(0) {
            let code = set_error(props, socket);
            exception(call, code, "listen() failed");
            return None;
        }
        let fd = socket.sock_fd();
        let (_, bound) = address(socket);
        props.long(c"fd", zend_long::from(fd));
        props.long(c"port", zend_long::from(bound));
        Some(())
    };
    if run().is_none() {
        call.set_bool(false);
    }
}

/// `SWR_HTTPBINDINGS_SETTING_*`.
const SETTING_LOOKUP: u32 = 1;
const SETTING_COOKIE: u32 = 2;
const SETTING_POST: u32 = 3;
const SETTING_FILES: u32 = 4;
const SETTING_COMPRESSION: u32 = 5;
const SETTING_LEVEL: u32 = 6;
const SETTING_MINIMUM: u32 = 7;
const SETTING_TYPES: u32 = 8;
const SETTING_WEBSOCKET: u32 = 9;
const SETTING_UPLOAD: u32 = 10;
const SETTING_DONE: u32 = 11;
/// `SW_ERROR_SSL_BAD_CLIENT`.
const SSL_BAD_CLIENT: c_int = 1015;
/// `SW_ACCEPT_RETRY_TIME`.
const ACCEPT_RETRY_TIME: c_int = 1;

/// `start`'s `onAccept`: the callable and its cache, which outlive the
/// accept loop.
struct Accept {
    callback: Owned,
    cache: z::zend_fcall_info_cache,
}

/// `sw_zend_is_callable_at_frame("onAccept")` on `$this` at `start`'s frame;
/// an `E_CORE_ERROR` when it is not.
fn accept_callable(call: &Call) -> Result<Option<Box<Accept>>, bailout::Bailout> {
    // SAFETY: plain data (an undefined name), filled below.
    let mut accept: Box<Accept> = Box::new(unsafe { std::mem::zeroed() });
    accept.callback = Owned::string(ZString::new(b"onAccept"));
    let mut callable = false;
    // SAFETY: the name, `$this`, this method's frame and local outputs; the
    // shim traps what Zend raises.
    bailout::trapped(unsafe {
        z::swr_zend_try_facade_callable_frame(
            accept.callback.as_ptr(),
            call.this(),
            call.frame(),
            0,
            &raw mut accept.cache,
            ptr::null_mut(),
            &raw mut callable,
        )
    })?;
    if callable {
        return Ok(Some(accept));
    }
    // `zend_get_callable_name_ex` of a method name on `$this`: `Class::onAccept`.
    let class = call.class_name().unwrap_or_default().to_string_lossy();
    host::set_last_error(host::PHP_FATAL_ERROR);
    let message =
        CString::new(format!("function '{class}::onAccept' is not callable")).unwrap_or_default();
    swoole_rs_zend::report(z::E_CORE_ERROR.cast_signed(), &message)?;
    Ok(None)
}

/// `start`: apply the settings, then accept connections, each served by
/// `onAccept` in its own coroutine, until `shutdown`.
fn start(call: &Call) {
    match run_start(call) {
        Ok(Some(done)) => call.set_bool(done),
        Ok(None) => {}
        Err(bailout) => bailout.defer(),
    }
}

fn run_start(call: &Call) -> Result<Option<bool>, bailout::Bailout> {
    use swoole_rs_httpbindings::{
        AcceptErrors, Action, SettingsCursor, accept_loop, swr_v1_httpbindings_settings,
    };
    let (Some(server), Some(props)) = (server_of(call), call.props(Some(&SERVER))) else {
        return Ok(None);
    };
    let socket = server.socket;
    let Some(mut accept) = accept_callable(call)? else {
        return Ok(None);
    };
    // SAFETY: `settings` is only read from here on.
    let Some(settings) = (unsafe { props.array(c"settings") }) else {
        return Ok(None);
    };
    // SAFETY: a live socket and the settings array.
    if !unsafe { php_swoole_socket_set_protocol(socket.this(), settings.as_ptr()) } {
        props.long(c"errCode", EINVAL);
        #[expect(clippy::cast_possible_truncation, reason = "EINVAL")]
        let message = swoole_rs_phpruntime::strerror(EINVAL as c_int);
        props.bytes(c"errMsg", message.to_bytes());
        server.running.set(false);
        return Ok(Some(false));
    }
    let Some(table) = settings.array() else {
        return Ok(None);
    };
    let mut cursor = SettingsCursor::default();
    let mut found = 0;
    let mut value = None;
    let compression = u32::from(COMPRESSION.load(Relaxed));
    loop {
        let mut action = Action::default();
        if swr_v1_httpbindings_settings(Some(&mut cursor), compression, found, Some(&mut action))
            != 0
        {
            return Ok(None);
        }
        match action.kind {
            SETTING_LOOKUP => {
                value = table.get(action.text);
                found = u32::from(value.is_some());
            }
            SETTING_DONE => break,
            SETTING_WEBSOCKET => server.apply_websocket(table),
            kind => match value
                .map(|value| setting(server, kind, value))
                .transpose()?
            {
                Some(Some(value)) => server.apply(kind, value),
                _ => return Ok(None),
            },
        }
    }
    server.running.set(true);
    let errors = AcceptErrors {
        per_process: libc::EMFILE,
        system: libc::ENFILE,
        timeout: libc::ETIMEDOUT,
        bad_ssl: SSL_BAD_CLIENT,
        canceled: libc::ECANCELED,
    };
    let mut start = Start {
        props,
        server,
        socket,
        accept: &mut accept,
        bailed: false,
    };
    let accepted = accept_loop(&mut start, &errors);
    if start.bailed {
        return Err(bailout::Bailout);
    }
    Ok(Some(accepted))
}

/// One `start` setting (`SWR_HTTPBINDINGS_SETTING_*`) from its value: a
/// flag's or a number's value for [`HttpServer::apply`], or `None` (no
/// return value); an `E_ERROR` bails out.
fn setting(server: &HttpServer, kind: u32, value: &Value) -> Result<Option<i64>, bailout::Bailout> {
    let fatal = |message: String| {
        host::set_last_error(host::PHP_FATAL_ERROR);
        let message = CString::new(message).unwrap_or_default();
        swoole_rs_zend::report(swoole_rs_zend::E_ERROR, &message)
    };
    let compression = COMPRESSION.load(Relaxed);
    let result = match kind {
        SETTING_COOKIE | SETTING_POST | SETTING_FILES | SETTING_COMPRESSION => {
            Some(i64::from(value.truthy()))
        }
        SETTING_LEVEL if compression => Some(i64::from(
            swoole_rs_httpbindings::swr_v1_httpbindings_server_level(value.long()),
        )),
        SETTING_MINIMUM if compression => Some(value.long()),
        SETTING_TYPES if compression => {
            text_setting(server, kind, None);
            let Some(types) = value.array() else {
                fatal("http_compression_types must be array".into())?;
                return Ok(None);
            };
            for (_, kind) in types.entries() {
                let kind = kind.string();
                if bailout::pending() {
                    break;
                }
                if !kind.bytes().is_empty() {
                    text_setting(server, SETTING_TYPES, Some(kind.bytes()));
                }
            }
            Some(0)
        }
        SETTING_UPLOAD => {
            let dir = value.string();
            if bailout::pending() {
                return Err(bailout::Bailout);
            }
            let text = dir.bytes();
            let created = swoole_rs_phpserver::php_swoole_create_dir(text);
            if created {
                text_setting(server, kind, Some(text));
            }
            let message = format!(
                "Unable to create upload_tmp_dir[{}]",
                String::from_utf8_lossy(nul_terminated(text))
            );
            drop(dir);
            if !created {
                fatal(message)?;
                return Ok(None);
            }
            Some(0)
        }
        _ => None,
    };
    if bailout::pending() {
        return Err(bailout::Bailout);
    }
    Ok(result)
}

/// The `start` frame the accept loop drives.
struct Start<'a> {
    props: Props<'a>,
    server: &'a HttpServer,
    socket: Co,
    accept: &'a mut Accept,
    /// Creating an `onAccept` coroutine bailed out (#913): the loop stops and
    /// `start` re-raises it once the Rust frames are done.
    bailed: bool,
}

impl swoole_rs_httpbindings::AcceptHost for Start<'_> {
    fn running(&mut self) -> bool {
        !self.bailed && self.server.running.get()
    }

    /// Accept a connection and serve it with `onAccept` in a new coroutine: its
    /// id, or 0.
    fn accept(&mut self) -> Option<i64> {
        // The listener's accept, in `start`'s coroutine, gives the connection its own
        // copy of the listener's length callable, which the connection's object frees.
        let conn = self.server.listener.accept(0.0)?;
        // The loop's reference to the connection object.
        let Ok(zsocket) = swoole_rs_phpclients::init_socket_object(conn) else {
            self.bailed = true;
            return Some(0);
        };
        let mut cid = 0;
        // A warning's error handler can exit(): start() re-raises it (#913).
        // SAFETY: the resolved cache, the connection object and the callable's
        // name; the shim traps what the new coroutine raises.
        self.bailed = bailout::trapped(unsafe {
            z::swr_zend_try_facade_coroutine(
                Some(swoole_rs_phpcoroutines::create_coroutine),
                &raw mut self.accept.cache,
                1,
                zsocket.as_ptr(),
                self.accept.callback.as_ptr(),
                &raw mut cid,
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        })
        .is_err();
        if self.bailed {
            // As upstream, whose bailout jumps past the release: start() raises
            // it and the request's shutdown frees the object.
            let _kept = zsocket.into_zval();
            return Some(0);
        }
        Some(cid)
    }

    fn error(&mut self) -> (c_int, CString) {
        socket_error(self.socket)
    }

    fn sleep(&mut self) {
        // A coroutine runs `start`.
        swoole_rs_hooks::stdio::swoole_coroutine_sleep(ACCEPT_RETRY_TIME);
    }

    fn set_error(&mut self) {
        set_error(self.props, self.socket);
    }

    fn fail(&mut self, warning: &str) {
        host::fatal(warning);
        // Keep-alive connections stop once the listener cannot accept.
        self.server.running.set(false);
    }
}

/// `%s` of a ZPP string: up to its first NUL.
fn nul_terminated(text: &[u8]) -> &[u8] {
    text.split(|&b| b == 0).next().unwrap_or(text)
}

/// `SO_REUSEPORT`, as `set_reuse_port()`; a failure is ignored.
fn set_reuse_port(fd: c_int) {
    let enable: c_int = 1;
    swoole_rs_corebase::sys::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, &enable);
}

fn handle(call: &Call) {
    let Ok((pattern, callable)) = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.value(2))))
    else {
        return;
    };
    let (Some(server), Some(callable)) = (server_of(call), callable) else {
        return;
    };
    let result = Callable::new(callable).and_then(|handle| {
        let Some(handle) = handle else {
            return Ok(false);
        };
        server.set_handler(pattern, handle)?;
        Ok(true)
    });
    match result {
        Ok(set) => call.set_bool(set),
        Err(bailout) => bailout.defer(),
    }
}

fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        call.set_bool(false);
        return;
    };
    if settings.count() == 0 {
        call.set_bool(false);
        return;
    }
    if let (Some(props), Ok(settings)) = (call.props(Some(&SERVER)), call.array(1)) {
        props.merge(c"settings", settings);
    }
    call.set_bool(true);
}

fn shutdown(call: &Call) {
    let Some(server) = server_of(call) else {
        return;
    };
    server.running.set(false);
    server.socket.cancel(EVENT_READ);
    // Connections waiting for their next request stop waiting.
    let clients = server.clients();
    let mut waiting: Vec<(z::zend_ulong, Co)> = Vec::new();
    if let Some(connections) = server.clients.array() {
        connections.for_each(|key, connection| {
            let ArrayKey::Index(index) = key else { return };
            let Some(socket) = get_socket(connection).map(swoole_rs_phpclients::Co::handle) else {
                return;
            };
            let waiting_now =
                socket.with_network(|network| network.read(|record| record.recv_wait != 0));
            if waiting_now == Some(true) {
                waiting.push((index, socket));
            }
        });
    }
    for (index, socket) in waiting {
        // The array's connection objects keep their sockets live.
        socket.cancel(EVENT_READ);
        // SAFETY: the live array and one of its keys.
        unsafe { z::zend_hash_index_del(clients, index) };
    }
}

/// The context's request and response objects, borrowed as upstream's
/// handler arguments (released by the caller).
fn objects(mut context: Ctx) -> [zval; 2] {
    let state = context.state();
    [state.request.zobject.value, state.response.zobject.value]
}

/// One connection's state across its requests.
struct Connection<'a> {
    /// The server object, whose `onAccept` call holds it.
    this: &'a Value,
    server: &'a HttpServer,
    socket: Co,
    /// The connection object, held for the loop.
    zconn: &'a Value,
    buffer: *mut Buffer,
}

impl Connection<'_> {
    #[expect(
        clippy::mut_from_ref,
        reason = "the socket owns the buffer; no two borrows overlap"
    )]
    fn buffer(&self) -> &mut Buffer {
        // SAFETY: the connection socket's read buffer, live for the loop and
        // only touched by the socket through calls that take it.
        unsafe { &mut *self.buffer }
    }

    fn data(&self, from: usize, to: usize) -> &[u8] {
        self.buffer().view().get(from..to).unwrap_or_default()
    }

    fn package_max_length(&self) -> usize {
        self.socket.protocol().package_max_length as usize
    }
}

/// Where the loop goes next: read more, or parse what the buffer holds.
enum Step {
    Recv,
    Parse,
}

fn on_accept(call: &Call) {
    if call.arity(1, 1).and_then(|()| call.object(1)).is_err() {
        call.set_null();
        return;
    }
    let (Some(this), Some(server)) = (call.this_value(), server_of(call)) else {
        call.set_null();
        return;
    };
    // The connection object, held for the loop.
    let Some(held) = call.value(1).map(Value::dereferenced) else {
        call.set_null();
        return;
    };
    let zconn: &Value = &held;
    // The server's array keeps the object, and so its socket.
    let Some(socket) = get_socket(zconn) else {
        call.set_null();
        return;
    };
    // The request allocator first.
    socket.use_zend_strings();
    let socket = socket.handle();
    let buffer = socket.buffer(false);
    let handshake = !socket.tls_owner().is_null() && !socket.ssl_handshake();
    if handshake {
        call.set_null();
        return;
    }
    let cid = swoole_rs_phpcoroutines::get_cid();
    // A reference for the server's array, which stays taken when the array
    // already holds the key, as upstream's.
    let mut kept = zconn.copy().into_zval();
    // SAFETY: the server's live array, which copies the connection's zval in.
    unsafe { z::zend_hash_index_add(server.clients(), cid.cast_unsigned(), &raw mut kept) };
    let (remote_addr, remote_port) = address(socket);
    // The network socket's address becomes the local one.
    let _named = socket.getsockname();
    let (server_addr, _) = address(socket);
    let peer = Peer {
        remote_addr,
        server_addr,
        remote_port,
        server_port: address(server.socket).1,
    };
    let conn = Connection {
        this,
        server,
        socket,
        zconn,
        buffer,
    };
    let result = serve(&conn, &peer);
    drop(peer);
    if result.is_ok() {
        // SAFETY: the server's live array.
        unsafe { z::zend_hash_index_del(server.clients(), cid.cast_unsigned()) };
    }
    bailout::defer(result);
}

/// `$request->server`'s addresses and ports of the connection.
fn add_peer(mut context: Ctx, peer: &Peer) {
    let state = context.state();
    // The array takes a reference of its own to each of the loop's strings.
    let remote_addr = Owned::string(peer.remote_addr.clone());
    add_server(state, Key::RemoteAddr, remote_addr);
    add_server(state, Key::RemotePort, Owned::long(peer.remote_port.into()));
    let server_addr = Owned::string(peer.server_addr.clone());
    add_server(state, Key::ServerAddr, server_addr);
    add_server(state, Key::ServerPort, Owned::long(peer.server_port.into()));
}

/// The request loop: parse each request on the connection and run its handler.
fn serve(conn: &Connection, peer: &Peer) -> Result<(), bailout::Bailout> {
    let mut current: Option<Ctx> = None;
    let mut header_completed = false;
    let mut crlf_offset = 0;
    let mut total = 0;
    let mut step = Step::Recv;
    loop {
        if matches!(step, Step::Recv) {
            set_recv_wait(conn.socket, true);
            let buffer = conn.buffer();
            let length = buffer.len();
            let received = conn.socket.recv(buffer.spare());
            if received <= 0 {
                break;
            }
            // SAFETY: the receive wrote `received` bytes into the free space.
            unsafe { buffer.set_len(length + received.cast_unsigned()) };
        }
        step = Step::Recv;
        let mut context =
            *current.get_or_insert_with(|| conn.server.context(conn.socket, conn.zconn));
        let max = conn.package_max_length();
        if !header_completed {
            let length = conn.buffer().len();
            let Some(pos) = find_crlf2(conn.data(crlf_offset, length)) else {
                if length == conn.buffer().size() {
                    context.state().response.status = ENTITY_TOO_LARGE;
                    break;
                }
                crlf_offset = length.saturating_sub(4);
                continue;
            };
            let header_length = crlf_offset + pos + 4;
            header_completed = true;
            crlf_offset = 0;
            // The header first, so `max - header_length` cannot underflow.
            if header_length > max {
                context.state().response.status = ENTITY_TOO_LARGE;
                break;
            }
            // The parser `context` initialised.
            let parsed = parse_request(context.state(), conn.data(0, header_length));
            if parsed != header_length {
                context.state().response.status = BAD_REQUEST;
                break;
            }
            let buffer = conn.buffer();
            buffer.set_offset(buffer.offset() + i64::try_from(header_length).unwrap_or(i64::MAX));
            let content_length = context.state().parser.content_length;
            if content_length > (max - header_length) as u64 {
                context.state().response.status = ENTITY_TOO_LARGE;
                break;
            }
            total = header_length + usize::try_from(content_length).unwrap_or(usize::MAX);
            if total > conn.buffer().size() {
                conn.buffer().extend(total);
            }
            // Once per request, so the interim response is not repeated.
            let state = context.state();
            if state.completed == 0 && state.request.version == 101 && expects_continue(context) {
                const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
                let _sent = conn.socket.send_all(CONTINUE);
            }
        }
        if context.state().completed == 0 {
            let chunked = context.state().recv_chunked != 0;
            if !chunked && conn.buffer().len() < total {
                continue;
            }
            let offset = usize::try_from(conn.buffer().offset()).unwrap_or(0);
            let mut length = conn.buffer().len() - offset;
            if chunked {
                // Body callbacks never see bytes the request limit rejects.
                length = length.min(max - offset);
            }
            // `offset + length` lies within the buffer.
            let parsed = parse_request(context.state(), conn.data(offset, offset + length));
            let buffer = conn.buffer();
            buffer.set_offset(buffer.offset() + i64::try_from(parsed).unwrap_or(i64::MAX));
            let error = context.state().parser.error;
            if error != HPE_OK && error != HPE_PAUSED_H2_UPGRADE {
                context.state().response.status = BAD_REQUEST;
                break;
            }
            if chunked {
                if context.state().completed == 0 {
                    if conn.buffer().len() >= max {
                        context.state().response.status = ENTITY_TOO_LARGE;
                        break;
                    }
                    if conn.buffer().len() == conn.buffer().size() {
                        let size = (conn.buffer().size() + BUFFER_SIZE_BIG).min(max);
                        conn.buffer().extend(size);
                    }
                    continue;
                }
                // llhttp pauses at the end of this request, before pipelined bytes.
                total = usize::try_from(conn.buffer().offset()).unwrap_or(0);
            }
        }
        add_peer(context, peer);
        if context.state().parser.error == HPE_PAUSED_H2_UPGRADE
            && conn.buffer().len() >= HTTP2_PRI.len()
            && conn.data(0, HTTP2_PRI.len()) == HTTP2_PRI
        {
            conn.buffer()
                .set_offset(i64::try_from(HTTP2_PRI.len()).unwrap_or(0));
            return http2(conn, context);
        }
        request_data(context, conn.buffer(), total);
        let callable = conn.server.handler(context);
        let mut args = objects(context);
        let state = context.state();
        let keep_alive = state.parser.should_keep_alive() && state.websocket == 0;
        set_recv_wait(conn.socket, false);
        if let Some(callable) = callable {
            if !call_handler(&callable, &mut args, false, NO_THEN)? {
                host::notice("handler error");
            }
        } else {
            context.state().response.status = NOT_FOUND;
        }
        let released = context::release_objects(context);
        current = None;
        released?;
        let running = conn.server.running.get();
        if !running || !keep_alive || is_socket_closed(conn.zconn) {
            break;
        }
        header_completed = false;
        if !conn.buffer().is_empty() {
            step = Step::Parse;
        }
    }
    current.map_or(Ok(()), context::release_objects)
}

/// `swoole_strnpos(.., "\r\n\r\n")`.
/// `recv_http2_frame`: the connection speaks HTTP/2 from here on. Its session
/// takes the HTTP/1 request's context as its default and handles each frame
/// until the peer goes or a frame fails (or a handler bailed out, which
/// `onAccept` re-raises).
fn http2(conn: &Connection, mut context: Ctx) -> Result<(), bailout::Bailout> {
    http2_begin(conn.socket);
    let fd = context.state().fd;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's uint32_t package_max_length"
    )]
    let max_body_size = conn.package_max_length() as u32;
    if let Some(server) = crate::http2::server() {
        // The context outlives the session, freed below; the session's
        // handler holds the server object.
        let this = conn.this.copy();
        (server.session_new)(
            fd,
            context,
            max_body_size,
            Box::new(move |ctx, http1| {
                let Some(server) = this.custom::<Data>().and_then(|data| data.0.get()) else {
                    return Ok(());
                };
                http2_request(ctx, http1, server)
            }),
        );
        loop {
            // The frame stays in the read buffer.
            let retval = recv_packet(conn.socket, 0.0);
            if retval <= 0 || (server.session_receive)(fd, conn.buffer().view()) == SW_ERR {
                break;
            }
            if bailout::pending() {
                break;
            }
        }
        (server.session_free)(fd);
    }
    context.state().detached = 1;
    context::release_objects(context)
}

fn find_crlf2(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

/// `Expect: 100-continue` (case-insensitive) among the request headers.
fn expects_continue(mut context: Ctx) -> bool {
    let header = context.state().request.zheader.get();
    header
        .and_then(Value::array)
        .and_then(|header| header.find(b"expect"))
        .and_then(Value::bytes)
        .is_some_and(|text| text.eq_ignore_ascii_case(b"100-continue"))
}

handlers! {
    zim_swoole_http_server_coro___construct => construct;
    zim_swoole_http_server_coro_set => set;
    zim_swoole_http_server_coro_handle => handle;
    zim_swoole_http_server_coro_onAccept => on_accept;
    zim_swoole_http_server_coro_shutdown => shutdown;
    zim_swoole_http_server_coro_start => start;
}

/// `http2_server_onRequest`: an HTTP/2 stream's request, with the HTTP/1
/// request's `$server` and `SERVER_PROTOCOL` `HTTP/2`.
fn http2_request(
    mut context: Ctx,
    mut http1: Ctx,
    server: &HttpServer,
) -> Result<(), bailout::Bailout> {
    let source = http1.state().request.zserver.get().and_then(Value::array);
    if let (Some(source), Some(target)) = (source, context.state().request.zserver.array_mut()) {
        // `http_server_add_server_array(ht, key, value)`: each entry with a
        // reference of its own.
        source.for_each(|key, entry| {
            if let ArrayKey::Name(key) = key {
                target.add_new(&ZString::new(key), entry.copy());
            }
        });
    }
    add_protocol(context.state(), Key::Http2);
    let callable = server.handler(context);
    let mut args = objects(context);
    let mut result = Ok(());
    if let Some(callable) = callable {
        match call_handler(&callable, &mut args, true, NO_THEN) {
            Err(bailout) => result = Err(bailout),
            Ok(false) => {
                let stream_id = context.state().stream_id;
                if let Some(server) = crate::http2::server() {
                    (server.reset)(context, stream_id, HTTP2_INTERNAL_ERROR);
                }
                host::notice("Swoole\\Http\\Server->onRequest[v2] handler error");
            }
            Ok(true) => {}
        }
    } else {
        context.state().response.status = NOT_FOUND;
    }
    result.and_then(|()| context::release_objects(context))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_ends_at_the_first_blank_line() {
        assert_eq!(find_crlf2(b"GET / HTTP/1.1\r\n\r\nbody"), Some(14), "end");
        assert_eq!(find_crlf2(b"GET / HTTP/1.1\r\n"), None, "incomplete");
    }
}
