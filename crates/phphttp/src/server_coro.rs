//! `Swoole\Coroutine\Http\Server`: registration, the object and upstream's
//! `swoole::coroutine::HttpServer` (handlers, connections, settings),
//! `__construct`, `set`, `handle`, `start`, `shutdown`, the per-connection
//! request loop (`onAccept`), HTTP/2 on a connection and its request
//! dispatch. C++ keeps the coroutine sockets and the settings the contexts
//! copy into C++ fields, and answers through `swr_hsc_*`
//! (`ext-src/swoole_http_server_coro.cc`).
use crate::{
    host::{self, Context},
    parse::{find, swr_v1_phphttp_parse},
    send,
};
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};
use swoole_rs_httpbindings::{Text, swr_v1_httpbindings_handler};
use swoole_rs_phpstubs::http_server_coro;
use swoole_rs_zend::{
    Call, Class, Object, bailout, declare_bool, declare_long, declare_property_null,
    declare_string, handlers, method_table,
    string::bytes,
    sys::{
        self as z, IS_STRING, ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC,
        swr_zend_property_merge, zend_class_entry, zend_long, zend_object, zend_object_handlers,
        zval, zval_ptr_dtor,
    },
    throw_constructed_twice, throw_exception, type_of, unset_property_deny,
    value::{self, update_bool, update_long},
};

static SERVER: Class = Class::new();
static EXCEPTION: AtomicPtr<zend_class_entry> = AtomicPtr::new(ptr::null_mut());
/// `SW_HAVE_COMPRESSION`.
static COMPRESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `struct HttpServerObject { HttpServer *server; zend_object std; }`.
type Holder = Object<*mut HttpServer>;

/// Upstream's `swoole::coroutine::HttpServer`. Its connections' coroutines
/// share it, so every field that changes is a cell.
struct HttpServer {
    /// The listening coroutine socket (C++'s).
    socket: *mut c_void,
    /// What the contexts copy into C++ fields (`HttpServerSettings`).
    settings: *mut c_void,
    /// `handle()`'s patterns, in registration order, and their callables.
    handlers: RefCell<Vec<Handler>>,
    default_handler: Cell<*mut c_void>,
    running: Cell<bool>,
    /// The connections, by the coroutine serving each.
    clients: UnsafeCell<zval>,
    parse_cookie: Cell<bool>,
    parse_post: Cell<bool>,
    parse_files: Cell<bool>,
    compression: Cell<bool>,
    compression_level: Cell<i8>,
    compression_min_length: Cell<u32>,
}

/// A pattern and its callable (a `zend::Callable`).
type Handler = (Box<[u8]>, *mut c_void);

/// `SW_Z_BEST_SPEED` and `SW_COMPRESSION_MIN_LENGTH_DEFAULT`.
const Z_BEST_SPEED: i8 = 1;
const COMPRESSION_MIN_LENGTH_DEFAULT: u32 = 20;

impl HttpServer {
    fn new(socket: *mut c_void, settings: *mut c_void) -> Self {
        // SAFETY: a zval is plain data, initialised below.
        let mut clients = unsafe { std::mem::zeroed::<zval>() };
        // SAFETY: an empty slot takes a new array.
        unsafe { value::array_init(&raw mut clients) };
        Self {
            socket,
            settings,
            handlers: RefCell::new(Vec::new()),
            default_handler: Cell::new(ptr::null_mut()),
            running: Cell::new(true),
            clients: UnsafeCell::new(clients),
            parse_cookie: Cell::new(true),
            parse_post: Cell::new(true),
            parse_files: Cell::new(false),
            compression: Cell::new(true),
            compression_level: Cell::new(Z_BEST_SPEED),
            compression_min_length: Cell::new(COMPRESSION_MIN_LENGTH_DEFAULT),
        }
    }

    fn clients(&self) -> *mut z::HashTable {
        // SAFETY: the server's array, live until drop.
        unsafe { value::table(self.clients.get()) }
    }

    /// `set_handler`: false if `callable` is not callable.
    fn set_handler(&self, pattern: &[u8], callable: *mut zval) -> Result<bool, bailout::Bailout> {
        let mut bailed = false;
        // SAFETY: a passed value; C++ traps what making the callable raises.
        let handler = unsafe { swr_hsc_callable(callable, &raw mut bailed) };
        if bailed {
            return Err(bailout::Bailout);
        }
        if handler.is_null() {
            return Ok(false);
        }
        let mut handlers = self.handlers.borrow_mut();
        if let Some(entry) = handlers.iter_mut().find(|(known, _)| **known == *pattern) {
            // SAFETY: the callable this pattern had, freed once.
            unsafe { swr_hsc_callable_free(entry.1) };
            entry.1 = handler;
        } else {
            handlers.push((pattern.into(), handler));
        }
        if pattern == b"/" {
            self.default_handler.set(handler);
        }
        Ok(true)
    }

    /// `get_handler`: the first other handler whose pattern starts the path
    /// (ignoring case), else the default.
    fn handler(&self, context: *mut Context) -> *mut c_void {
        let request = &send::state(context).request;
        let path = Text {
            data: request.path.cast(),
            length: request.path_len as usize,
        };
        select(&self.handlers.borrow(), self.default_handler.get(), path)
    }

    /// A flag's or a number's setting (`SWR_HTTPBINDINGS_SETTING_*`), as
    /// `swr_hsc_setting` read it.
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
    fn context(&self, socket: *mut c_void, zconn: *mut zval) -> *mut Context {
        // SAFETY: the live settings, socket and connection object.
        let context = unsafe { swr_hsc_context(self.settings, socket, zconn) };
        let state = send::state(context);
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
        // SAFETY: the server's own array, released once.
        unsafe { zval_ptr_dtor(self.clients.get()) };
        for (_, handler) in self.handlers.get_mut().drain(..) {
            // SAFETY: each callable `set_handler` made, freed once.
            unsafe { swr_hsc_callable_free(handler) };
        }
        // SAFETY: the socket and settings `swr_hsc_new` made, freed once.
        unsafe { swr_hsc_delete(self.socket, self.settings) };
    }
}

/// `get_handler`'s scan over `handlers`.
fn select(handlers: &[Handler], default: *mut c_void, path: Text) -> *mut c_void {
    for (pattern, handler) in handlers {
        if *handler == default {
            continue;
        }
        let pattern = Text {
            data: pattern.as_ptr(),
            length: pattern.len(),
        };
        // SAFETY: the request's path and the pattern, for the call.
        if unsafe { swr_v1_httpbindings_handler(path, pattern) } == 1 {
            return *handler;
        }
    }
    default
}

/// The live server a method's or a connection's pointer names.
fn server<'a>(server: *const HttpServer) -> &'a HttpServer {
    // SAFETY: the object's server, which outlives its methods and the
    // connections it serves; only cells change.
    unsafe { &*server }
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

/// `swoole::String`, which C++ owns (`swr_hsc_buffer` asserts the layout).
#[repr(C)]
struct Buffer {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut u8,
    allocator: *const c_void,
}

/// The addresses `onAccept` puts in each request's `$server`
/// (`swr_hsc_peer`): the strings are references the loop releases.
#[repr(C)]
struct Peer {
    remote_addr: *mut z::zend_string,
    server_addr: *mut z::zend_string,
    remote_port: c_int,
    server_port: c_int,
}

unsafe extern "C" {
    fn swr_hsc_new(host: *const c_char, length: usize, settings: *mut *mut c_void) -> *mut c_void;
    fn swr_hsc_delete(socket: *mut c_void, settings: *mut c_void);
    fn swr_hsc_callable(callable: *mut zval, bailed: *mut bool) -> *mut c_void;
    fn swr_hsc_callable_free(callable: *mut c_void);
    fn swr_hsc_call(callable: *mut c_void, args: *mut zval, coroutine: bool) -> c_int;
    fn swr_hsc_cancel(socket: *mut c_void);
    fn swr_hsc_recv_waiting(socket: *mut c_void) -> bool;
    fn swr_hsc_cid() -> i64;
    fn swr_hsc_bind(
        socket: *mut c_void,
        host: *const c_char,
        length: usize,
        port: zend_long,
        reuse_port: bool,
    ) -> bool;
    fn swr_hsc_listen(socket: *mut c_void, ssl: bool) -> bool;
    fn swr_hsc_socket_info(socket: *mut c_void, fd: *mut c_int, port: *mut c_int);
    fn swr_hsc_error(socket: *mut c_void, message: *mut *const c_char) -> c_int;
    fn swr_hsc_connection(connection: *mut zval) -> *mut c_void;
    fn swr_hsc_connection_buffer(socket: *mut c_void) -> *mut Buffer;
    fn swr_hsc_ssl_handshake(socket: *mut c_void) -> bool;
    fn swr_hsc_peer(listener: *mut c_void, socket: *mut c_void, peer: *mut Peer);
    fn swr_hsc_recv(socket: *mut c_void, buffer: *mut Buffer) -> isize;
    fn swr_hsc_recv_wait(socket: *mut c_void, waiting: bool);
    fn swr_hsc_package_max_length(socket: *mut c_void) -> usize;
    fn swr_hsc_extend(buffer: *mut Buffer, size: usize);
    fn swr_hsc_send_all(socket: *mut c_void, data: *const c_char, length: usize);
    fn swr_hsc_context(
        settings: *mut c_void,
        socket: *mut c_void,
        connection: *mut zval,
    ) -> *mut Context;
    fn swr_hsc_add_server(context: *mut Context, peer: *mut Peer);
    fn swr_hsc_http2_begin(socket: *mut c_void);
    fn swr_hsc_recv_packet(socket: *mut c_void, retval: *mut isize) -> *const c_char;
    /// The HTTP/2 sessions' request handler, C++ (a `swr_h2s_request`).
    fn swr_hsc_http2_request(request: *mut c_void);
    fn swr_hsc_request_data(context: *mut Context, buffer: *mut Buffer, length: usize);
    fn swr_hsc_closed(connection: *mut zval) -> bool;
    /// The HTTP/2 server (crates/phphttp2): a stream's reset, and a
    /// connection's session. `receive` leaves a bailout pending.
    fn swr_v1_phphttp2_server_reset(ctx: *mut c_void, stream_id: u32, code: u32);
    fn swr_v1_phphttp2_session_new(
        fd: i64,
        default_ctx: *mut c_void,
        max_body_size: u32,
        is_coro: bool,
        handle: unsafe extern "C" fn(*mut c_void),
        data: *mut c_void,
    );
    fn swr_v1_phphttp2_session_receive(fd: i64, buf: *const c_char) -> c_int;
    fn swr_v1_phphttp2_session_free(fd: i64);
    fn swr_hsc_accept_callable(
        object: *mut zval,
        frame: *mut z::zend_execute_data,
        out: *mut *mut c_void,
    ) -> c_int;
    fn swr_hsc_accept_free(callable: *mut c_void);
    fn swr_hsc_set_protocol(socket: *mut c_void, settings: *mut zval) -> bool;
    fn swr_hsc_strerror(code: c_int) -> *const c_char;
    fn swr_hsc_accept_errors(errors: *mut swoole_rs_httpbindings::AcceptErrors);
    fn swr_hsc_accept(
        socket: *mut c_void,
        callable: *mut c_void,
        connected: *mut bool,
        bailed: *mut bool,
    ) -> i64;
    fn swr_hsc_sleep();
    fn swr_hsc_setting(
        settings: *mut c_void,
        kind: u32,
        table: *mut z::HashTable,
        value: *mut zval,
        result: *mut i64,
    ) -> c_int;
    fn llhttp_should_keep_alive(parser: *const crate::state::Llhttp) -> c_int;
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

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this (final) class; a null
    // server is the unconstructed object.
    unsafe { Holder::create(&SERVER, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let server = unsafe { *Holder::data(std) };
    if !server.is_null() {
        // SAFETY: the server `__construct` boxed, dropped once.
        drop(unsafe { Box::from_raw(server) });
    }
    // SAFETY: as above.
    unsafe { Holder::dtor(std) };
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE | ZEND_ACC_FINAL;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Holder::install(entry, handlers, create, free);
}

/// MINIT: the class, as `php_swoole_http_server_coro_minit` registered it.
/// # Safety
/// Called once, at MINIT, with Swoole's exception class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_coro_minit(
    exception: *mut zend_class_entry,
    short_name: bool,
    compression: bool,
) -> *mut zend_class_entry {
    EXCEPTION.store(exception, Relaxed);
    COMPRESSION.store(compression, Relaxed);
    // SAFETY: MINIT, with a static table.
    unsafe {
        SERVER.register(
            c"Swoole\\Coroutine\\Http\\Server",
            METHODS.as_ptr(),
            configure,
        );
    }
    if short_name {
        // SAFETY: MINIT, after registration.
        unsafe { SERVER.alias(c"Co\\Http\\Server") };
    }
    let ce = SERVER.entry();
    for (name, value) in [(c"fd", -1), (c"port", -1)] {
        if name == c"port" {
            // SAFETY: MINIT, with the class just registered; upstream's order.
            unsafe { declare_property_null(ce, c"host", ZEND_ACC_PUBLIC) };
        }
        // SAFETY: as above.
        unsafe { declare_long(ce, name, value, ZEND_ACC_PUBLIC) };
    }
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"ssl", false, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_property_null(ce, c"settings", ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"errCode", 0, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_string(ce, c"errMsg", c"", ZEND_ACC_PUBLIC) };
    ce
}

fn server_of(call: &Call) -> *mut HttpServer {
    // SAFETY: the methods are non-static, so `$this` is one of ours.
    unsafe { *Holder::data(call.this()) }
}

/// `http_server_coro_set_error`: the socket's error into `errCode`/`errMsg`.
fn set_error(object: *mut zend_object, socket: *mut c_void) -> zend_long {
    let mut message = ptr::null();
    // SAFETY: a live socket and a local output.
    let code = unsafe { swr_hsc_error(socket, &raw mut message) };
    // SAFETY: a live object of the class.
    unsafe { update_long(SERVER.entry(), object, c"errCode", zend_long::from(code)) };
    // SAFETY: as above; the socket's message is NUL-terminated.
    let message = unsafe { CStr::from_ptr(message) };
    // SAFETY: as above.
    unsafe { value::update_string(SERVER.entry(), object, c"errMsg", message) };
    zend_long::from(code)
}

fn exception(code: zend_long, message: &str) {
    let message = std::ffi::CString::new(message).unwrap_or_default();
    // SAFETY: Swoole's exception class, registered at MINIT.
    unsafe { throw_exception(EXCEPTION.load(Relaxed), &message, code) };
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
        let object = call.this();
        if !server_of(call).is_null() {
            // SAFETY: a live object.
            unsafe { throw_constructed_twice(object) };
            return None;
        }
        // SAFETY: a live object of the class; Zend copies the host.
        unsafe {
            z::zend_update_property_stringl(
                SERVER.entry(),
                object,
                c"host".as_ptr(),
                4,
                host.as_ptr().cast(),
                host.len(),
            );
        }
        // SAFETY: as above.
        unsafe { update_bool(SERVER.entry(), object, c"ssl", ssl) };
        if host.is_empty() {
            exception(EINVAL, "host is empty");
            return None;
        }
        let mut settings = ptr::null_mut();
        // SAFETY: the host's bytes, which C++ copies, and a local output.
        let socket = unsafe { swr_hsc_new(host.as_ptr().cast(), host.len(), &raw mut settings) };
        let server = Box::into_raw(Box::new(HttpServer::new(socket, settings)));
        // SAFETY: as above.
        unsafe { *Holder::data(object) = server };
        // SAFETY: a live socket and the host's bytes.
        if !unsafe { swr_hsc_bind(socket, host.as_ptr().cast(), host.len(), port, reuse_port) } {
            let code = set_error(object, socket);
            #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) port")]
            let port = port as i32;
            exception(
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
            // SAFETY: a live object; the shim converts `settings` to an array.
            let settings = unsafe {
                z::swr_zend_property_array(SERVER.entry(), object, c"settings".as_ptr(), 8)
            };
            // SAFETY: the live settings array.
            unsafe { z::add_assoc_bool_ex(settings, c"open_ssl".as_ptr(), 8, true) };
        }
        // SAFETY: a live socket.
        if !unsafe { swr_hsc_listen(socket, ssl) } {
            let code = set_error(object, socket);
            exception(code, "listen() failed");
            return None;
        }
        let (mut fd, mut bound) = (0, 0);
        // SAFETY: a live socket and local outputs.
        unsafe { swr_hsc_socket_info(socket, &raw mut fd, &raw mut bound) };
        // SAFETY: a live object of the class.
        unsafe { update_long(SERVER.entry(), object, c"fd", zend_long::from(fd)) };
        // SAFETY: as above.
        unsafe { update_long(SERVER.entry(), object, c"port", zend_long::from(bound)) };
        Some(())
    };
    if run().is_none() {
        call.set_bool(false);
    }
}

/// `swr_hsc_setting`/`swr_hsc_accept_callable` results: done, `false`, no
/// return value, bailed out.
const DONE: c_int = 0;
const REFUSED: c_int = 1;
const BAILED: c_int = -1;
/// `SWR_HTTPBINDINGS_SETTING_*`.
const SETTING_LOOKUP: u32 = 1;
const SETTING_COOKIE: u32 = 2;
const SETTING_POST: u32 = 3;
const SETTING_FILES: u32 = 4;
const SETTING_COMPRESSION: u32 = 5;
const SETTING_LEVEL: u32 = 6;
const SETTING_MINIMUM: u32 = 7;
const SETTING_DONE: u32 = 11;

/// `start`'s `onAccept` callable and its cache (C++'s), released on drop.
struct Accept(*mut c_void);

impl Drop for Accept {
    fn drop(&mut self) {
        // SAFETY: the callable `swr_hsc_accept_callable` made, freed once.
        unsafe { swr_hsc_accept_free(self.0) };
    }
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
        AcceptHost, Action, SettingsCursor, swr_v1_httpbindings_accept_loop,
        swr_v1_httpbindings_settings,
    };
    let object = call.this();
    let server = server(server_of(call));
    let socket = server.socket;
    let mut callable = ptr::null_mut();
    // SAFETY: `$this`, this method's frame and a local output; C++ traps the
    // E_CORE_ERROR of an uncallable `onAccept`.
    match unsafe { swr_hsc_accept_callable(call.this_zval(), call.frame(), &raw mut callable) } {
        BAILED => return Err(bailout::Bailout),
        DONE => {}
        _ => return Ok(None),
    }
    let accept = Accept(callable);
    // SAFETY: a live object; the shim converts `settings` to an array.
    let settings =
        unsafe { z::swr_zend_property_array(SERVER.entry(), object, c"settings".as_ptr(), 8) };
    // SAFETY: a live socket and the settings array.
    if !unsafe { swr_hsc_set_protocol(socket, settings) } {
        // SAFETY: a live object of the class.
        unsafe { update_long(SERVER.entry(), object, c"errCode", EINVAL) };
        // SAFETY: Swoole's static message for a code.
        let message = unsafe { swr_hsc_strerror(22) };
        // SAFETY: as above, NUL-terminated.
        let message = unsafe { CStr::from_ptr(message) };
        // SAFETY: a live object of the class.
        unsafe { value::update_string(SERVER.entry(), object, c"errMsg", message) };
        server.running.set(false);
        return Ok(Some(false));
    }
    // SAFETY: the live settings array.
    let table = unsafe { value::table(settings) };
    let mut cursor = SettingsCursor::default();
    let mut found = 0;
    let mut value = ptr::null_mut();
    let compression = u32::from(COMPRESSION.load(Relaxed));
    loop {
        let mut action = Action::default();
        // SAFETY: a local cursor and output.
        if unsafe {
            swr_v1_httpbindings_settings(&raw mut cursor, compression, found, &raw mut action)
        } != 0
        {
            return Ok(None);
        }
        match action.kind {
            SETTING_LOOKUP => {
                // SAFETY: a live table and the action's key bytes.
                value = unsafe {
                    z::zend_hash_str_find(table, action.text.data.cast(), action.text.length)
                };
                // SAFETY: a found entry is live.
                found = u32::from(!value.is_null() && unsafe { type_of(value) } != z::IS_NULL);
            }
            SETTING_DONE => break,
            kind => {
                let mut result = 0;
                // SAFETY: the live settings, the settings array, the value just
                // found and a local output.
                match unsafe {
                    swr_hsc_setting(server.settings, kind, table, value, &raw mut result)
                } {
                    DONE => server.apply(kind, result),
                    BAILED => return Err(bailout::Bailout),
                    REFUSED => return Ok(Some(false)),
                    _ => return Ok(None),
                }
            }
        }
    }
    server.running.set(true);
    // SAFETY: a zeroed policy table C++ fills with the platform's errno values.
    let mut errors = unsafe { std::mem::zeroed() };
    // SAFETY: a local output.
    unsafe { swr_hsc_accept_errors(&raw mut errors) };
    let mut start = Start {
        object,
        server,
        socket,
        accept: accept.0,
        bailed: Cell::new(false),
    };
    let host = AcceptHost {
        data: (&raw mut start).cast(),
        running: start_running,
        accept: start_accept,
        error: start_error,
        sleep: start_sleep,
        set_error: start_set_error,
        fail: start_fail,
    };
    // SAFETY: the host below and the local policy table.
    let accepted = unsafe { swr_v1_httpbindings_accept_loop(&raw const host, &raw const errors) };
    if start.bailed.get() {
        return Err(bailout::Bailout);
    }
    Ok(Some(accepted == 1))
}

/// The `start` frame the accept loop's host callbacks read (`AcceptHost::data`).
struct Start<'a> {
    object: *mut zend_object,
    server: &'a HttpServer,
    socket: *mut c_void,
    accept: *mut c_void,
    /// Creating an `onAccept` coroutine bailed out (#913): the loop stops and
    /// `start` re-raises it once the Rust frames are done.
    bailed: Cell<bool>,
}

fn start_of<'a>(data: *mut c_void) -> &'a Start<'a> {
    // SAFETY: `run_start` passes its live `Start` for the loop.
    unsafe { &*data.cast::<Start>() }
}

unsafe extern "C" fn start_running(data: *mut c_void) -> bool {
    let start = start_of(data);
    !start.bailed.get() && start.server.running.get()
}

unsafe extern "C" fn start_accept(data: *mut c_void, connected: *mut bool) -> i64 {
    let start = start_of(data);
    let mut bailed = false;
    // SAFETY: a live listener, the accept callable and the loop's outputs.
    let cid = unsafe { swr_hsc_accept(start.socket, start.accept, connected, &raw mut bailed) };
    start.bailed.set(bailed);
    cid
}

unsafe extern "C" fn start_error(data: *mut c_void, message: *mut *const c_char) -> c_int {
    // SAFETY: a live socket and the loop's output.
    unsafe { swr_hsc_error(start_of(data).socket, message) }
}

unsafe extern "C" fn start_sleep(_: *mut c_void) {
    // SAFETY: a coroutine runs `start`.
    unsafe { swr_hsc_sleep() };
}

unsafe extern "C" fn start_set_error(data: *mut c_void) {
    let start = start_of(data);
    set_error(start.object, start.socket);
}

unsafe extern "C" fn start_fail(data: *mut c_void, warning: swoole_rs_httpbindings::Text) {
    // SAFETY: the loop lends the warning's UTF-8 bytes for the call.
    let warning = unsafe { std::slice::from_raw_parts(warning.data, warning.length) };
    host::fatal(std::str::from_utf8(warning).unwrap_or_default());
    // Keep-alive connections stop once the listener cannot accept.
    start_of(data).server.running.set(false);
}

/// `%s` of a ZPP string: up to its first NUL.
fn nul_terminated(text: &[u8]) -> &[u8] {
    text.iter()
        .position(|&b| b == 0)
        .map_or(text, |nul| &text[..nul])
}

fn handle(call: &Call) {
    let server = server_of(call);
    let Ok((pattern, callable)) = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.zval(2))))
    else {
        return;
    };
    if server.is_null() {
        return;
    }
    match self::server(server).set_handler(pattern, callable) {
        Ok(set) => call.set_bool(set),
        Err(bailout) => bailout.defer(),
    }
}

fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: a passed array.
    if unsafe { value::count(settings) } == 0 {
        call.set_bool(false);
        return;
    }
    // SAFETY: a live object of the class and the passed array.
    unsafe {
        swr_zend_property_merge(
            SERVER.entry(),
            call.this(),
            c"settings".as_ptr(),
            8,
            settings,
        );
    }
    call.set_bool(true);
}

fn shutdown(call: &Call) {
    let server = server_of(call);
    if server.is_null() {
        return;
    }
    let server = self::server(server);
    server.running.set(false);
    // SAFETY: its live listening socket.
    unsafe { swr_hsc_cancel(server.socket) };
    // Connections waiting for their next request stop waiting.
    let clients = server.clients();
    let mut waiting: Vec<(z::zend_ulong, *mut c_void)> = Vec::new();
    // SAFETY: a live array; the callback only reads.
    unsafe { z::swr_zend_hash_foreach_kv(clients, Some(collect), (&raw mut waiting).cast()) };
    for (index, socket) in waiting {
        // SAFETY: a live connection socket.
        unsafe { swr_hsc_cancel(socket) };
        // SAFETY: the live array and one of its keys.
        unsafe { z::zend_hash_index_del(clients, index) };
    }
}

/// `shutdown`'s scan: the connections whose socket waits for a request.
unsafe extern "C" fn collect(
    _: *mut z::zend_string,
    index: z::zend_ulong,
    connection: *mut zval,
    out: *mut c_void,
) {
    // SAFETY: a socket object in the server's array.
    let socket = unsafe { swr_hsc_connection(connection) };
    // SAFETY: as above.
    if unsafe { swr_hsc_recv_waiting(socket) } {
        // SAFETY: `shutdown`'s vector, borrowed for the scan.
        unsafe { (*out.cast::<Vec<(z::zend_ulong, *mut c_void)>>()).push((index, socket)) };
    }
}

/// The context's request and response objects, borrowed as upstream's
/// handler arguments (released by the caller).
fn objects(context: *mut Context) -> [zval; 2] {
    let state = send::state(context);
    // SAFETY: the context's live request object.
    let request = unsafe { *state.request.zobject.ptr };
    // SAFETY: the context's live response object.
    let response = unsafe { *state.response.zobject.ptr };
    [request, response]
}

/// One connection's state across its requests.
struct Connection<'a> {
    server: &'a HttpServer,
    socket: *mut c_void,
    zconn: *mut zval,
    buffer: *mut Buffer,
}

impl Connection<'_> {
    #[expect(
        clippy::mut_from_ref,
        reason = "C++ owns the buffer; no two borrows overlap"
    )]
    fn buffer(&self) -> &mut Buffer {
        // SAFETY: the connection socket's read buffer, live for the loop and
        // only touched by C++ through calls that take it.
        unsafe { &mut *self.buffer }
    }

    fn data(&self, from: usize, to: usize) -> &[u8] {
        let buffer = self.buffer();
        // SAFETY: `from..to` lies within the buffer's `length` bytes.
        let start = unsafe { buffer.str.add(from) };
        // SAFETY: as above.
        unsafe { std::slice::from_raw_parts(start, to - from) }
    }

    fn package_max_length(&self) -> usize {
        // SAFETY: a live socket.
        unsafe { swr_hsc_package_max_length(self.socket) }
    }

    fn extend(&self, size: usize) {
        // SAFETY: the live buffer.
        unsafe { swr_hsc_extend(self.buffer, size) };
    }
}

fn parse(context: *mut Context, data: *const u8, length: usize) -> usize {
    // SAFETY: a live context and `length` readable bytes.
    unsafe { swr_v1_phphttp_parse(context, data.cast(), length) }
}

/// Where the loop goes next: read more, or parse what the buffer holds.
enum Step {
    Recv,
    Parse,
}

fn on_accept(call: &Call) {
    let Ok(zconn) = call.arity(1, 1).and_then(|()| call.object(1)) else {
        call.set_null();
        return;
    };
    let server = server(server_of(call));
    // SAFETY: a coroutine socket object.
    let socket = unsafe { swr_hsc_connection(zconn) };
    // SAFETY: as above; sets the request allocator first.
    let buffer = unsafe { swr_hsc_connection_buffer(socket) };
    // SAFETY: a live socket.
    if !unsafe { swr_hsc_ssl_handshake(socket) } {
        call.set_null();
        return;
    }
    // SAFETY: a coroutine runs this method.
    let cid = unsafe { swr_hsc_cid() };
    // SAFETY: the server's live array takes a reference to the connection.
    unsafe { z::swr_zval_try_addref(zconn) };
    // SAFETY: as above.
    unsafe { z::zend_hash_index_add(server.clients(), cid.cast_unsigned(), zconn) };
    let mut peer = Peer {
        remote_addr: ptr::null_mut(),
        server_addr: ptr::null_mut(),
        remote_port: 0,
        server_port: 0,
    };
    // SAFETY: the live listener and socket.
    unsafe { swr_hsc_peer(server.socket, socket, &raw mut peer) };
    let conn = Connection {
        server,
        socket,
        zconn,
        buffer,
    };
    let result = serve(&conn, &mut peer);
    // SAFETY: the address strings `swr_hsc_peer` made.
    unsafe { z::swr_zend_string_release(peer.remote_addr) };
    // SAFETY: as above.
    unsafe { z::swr_zend_string_release(peer.server_addr) };
    if result.is_ok() {
        // SAFETY: the server's live array.
        unsafe { z::zend_hash_index_del(server.clients(), cid.cast_unsigned()) };
    }
    bailout::defer(result);
}

/// Release a context the loop still holds: its request and response objects.
fn release(context: *mut Context) {
    let state = send::state(context);
    let (request, response) = (state.request.zobject.ptr, state.response.zobject.ptr);
    // SAFETY: the context's live objects, released once.
    unsafe { zval_ptr_dtor(request) };
    // SAFETY: as above.
    unsafe { zval_ptr_dtor(response) };
}

/// The request loop: parse each request on the connection and run its handler.
fn serve(conn: &Connection, peer: &mut Peer) -> Result<(), bailout::Bailout> {
    let mut context: *mut Context = ptr::null_mut();
    let mut header_completed = false;
    let mut crlf_offset = 0;
    let mut total = 0;
    let mut step = Step::Recv;
    loop {
        if matches!(step, Step::Recv) {
            // SAFETY: a live socket and its buffer.
            if unsafe { swr_hsc_recv(conn.socket, conn.buffer) } <= 0 {
                break;
            }
        }
        step = Step::Recv;
        if context.is_null() {
            context = conn.server.context(conn.socket, conn.zconn);
        }
        let max = conn.package_max_length();
        if !header_completed {
            let length = conn.buffer().length;
            let Some(pos) = find_crlf2(conn.data(crlf_offset, length)) else {
                if length == conn.buffer().size {
                    send::state(context).response.status = ENTITY_TOO_LARGE;
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
                send::state(context).response.status = ENTITY_TOO_LARGE;
                break;
            }
            if parse(context, conn.buffer().str, header_length) != header_length {
                send::state(context).response.status = BAD_REQUEST;
                break;
            }
            conn.buffer().offset += i64::try_from(header_length).unwrap_or(i64::MAX);
            let content_length = send::state(context).parser.content_length;
            if content_length > (max - header_length) as u64 {
                send::state(context).response.status = ENTITY_TOO_LARGE;
                break;
            }
            total = header_length + usize::try_from(content_length).unwrap_or(usize::MAX);
            if total > conn.buffer().size {
                conn.extend(total);
            }
            // Once per request, so the interim response is not repeated.
            let state = send::state(context);
            if state.completed == 0 && state.request.version == 101 && expects_continue(context) {
                const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
                // SAFETY: a live socket and the interim response.
                unsafe { swr_hsc_send_all(conn.socket, CONTINUE.as_ptr().cast(), CONTINUE.len()) };
            }
        }
        if send::state(context).completed == 0 {
            let chunked = send::state(context).recv_chunked != 0;
            if !chunked && conn.buffer().length < total {
                continue;
            }
            let offset = usize::try_from(conn.buffer().offset).unwrap_or(0);
            let mut length = conn.buffer().length - offset;
            if chunked {
                // Body callbacks never see bytes the request limit rejects.
                length = length.min(max - offset);
            }
            // SAFETY: `offset + length` lies within the buffer.
            let parsed = parse(context, unsafe { conn.buffer().str.add(offset) }, length);
            conn.buffer().offset += i64::try_from(parsed).unwrap_or(i64::MAX);
            let error = send::state(context).parser.error;
            if error != HPE_OK && error != HPE_PAUSED_H2_UPGRADE {
                send::state(context).response.status = BAD_REQUEST;
                break;
            }
            if chunked {
                if send::state(context).completed == 0 {
                    if conn.buffer().length >= max {
                        send::state(context).response.status = ENTITY_TOO_LARGE;
                        break;
                    }
                    if conn.buffer().length == conn.buffer().size {
                        conn.extend((conn.buffer().size + BUFFER_SIZE_BIG).min(max));
                    }
                    continue;
                }
                // llhttp pauses at the end of this request, before pipelined bytes.
                total = usize::try_from(conn.buffer().offset).unwrap_or(0);
            }
        }
        // SAFETY: a live context and the connection's addresses.
        unsafe { swr_hsc_add_server(context, peer) };
        if send::state(context).parser.error == HPE_PAUSED_H2_UPGRADE
            && conn.buffer().length >= HTTP2_PRI.len()
            && conn.data(0, HTTP2_PRI.len()) == HTTP2_PRI
        {
            conn.buffer().offset = i64::try_from(HTTP2_PRI.len()).unwrap_or(0);
            http2(conn, context);
            context = ptr::null_mut();
            break;
        }
        // SAFETY: a live context and the buffer holding its `total` bytes.
        unsafe { swr_hsc_request_data(context, conn.buffer, total) };
        let callable = conn.server.handler(context);
        let mut args = objects(context);
        let state = send::state(context);
        // SAFETY: the context's llhttp parser.
        let keep_alive = unsafe { llhttp_should_keep_alive(&raw const state.parser) } != 0
            && state.websocket == 0;
        // SAFETY: a live socket.
        unsafe { swr_hsc_recv_wait(conn.socket, false) };
        if callable.is_null() {
            send::state(context).response.status = NOT_FOUND;
        } else {
            // SAFETY: the server's live handler and the arguments.
            match unsafe { swr_hsc_call(callable, args.as_mut_ptr(), false) } {
                -1 => return Err(bailout::Bailout),
                0 => host::notice("handler error"),
                _ => {}
            }
        }
        // SAFETY: the context's objects, released once.
        unsafe { zval_ptr_dtor(&raw mut args[0]) };
        // SAFETY: as above.
        unsafe { zval_ptr_dtor(&raw mut args[1]) };
        context = ptr::null_mut();
        let running = conn.server.running.get();
        // SAFETY: a live connection object.
        if !running || !keep_alive || unsafe { swr_hsc_closed(conn.zconn) } {
            break;
        }
        header_completed = false;
        if conn.buffer().length > 0 {
            step = Step::Parse;
        }
    }
    if !context.is_null() {
        release(context);
    }
    Ok(())
}

/// `swoole_strnpos(.., "\r\n\r\n")`.
/// `recv_http2_frame`: the connection speaks HTTP/2 from here on. Its session
/// takes the HTTP/1 request's context as its default and handles each frame
/// until the peer goes or a frame fails (or a handler bailed out, which
/// `onAccept` re-raises).
fn http2(conn: &Connection, context: *mut Context) {
    // SAFETY: a live socket.
    unsafe { swr_hsc_http2_begin(conn.socket) };
    let fd = send::state(context).fd;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's uint32_t package_max_length"
    )]
    let max_body_size = conn.package_max_length() as u32;
    // SAFETY: the context outlives the session, freed below; the handler
    // gets the server, which outlives the connection.
    unsafe {
        swr_v1_phphttp2_session_new(
            fd,
            context,
            max_body_size,
            true,
            swr_hsc_http2_request,
            ptr::from_ref(conn.server).cast_mut().cast(),
        );
    };
    loop {
        let mut retval = 0;
        // SAFETY: a live socket; the frame stays in its read buffer.
        let frame = unsafe { swr_hsc_recv_packet(conn.socket, &raw mut retval) };
        // SAFETY: a whole frame.
        if retval <= 0 || unsafe { swr_v1_phphttp2_session_receive(fd, frame) } == SW_ERR {
            break;
        }
        if bailout::pending() {
            break;
        }
    }
    // SAFETY: the session made above.
    unsafe { swr_v1_phphttp2_session_free(fd) };
    send::state(context).detached = 1;
    release(context);
}

fn find_crlf2(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

/// `Expect: 100-continue` (case-insensitive) among the request headers.
fn expects_continue(context: *mut Context) -> bool {
    let expect = find(send::state(context).request.zheader.ptr, b"expect");
    // SAFETY: a live header value.
    !expect.is_null() && unsafe { type_of(expect) } == IS_STRING && {
        // SAFETY: a string zval.
        let string = unsafe { value::str_of(expect) };
        // SAFETY: a live string.
        let text = unsafe { bytes(string) };
        text.eq_ignore_ascii_case(b"100-continue")
    }
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
/// # Safety
/// The stream's live context, the session's default context and the server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_coro_http2_request(
    context: *mut c_void,
    http1: *mut c_void,
    server: *mut c_void,
) {
    let result = http2_request(context, http1, self::server(server.cast()));
    bailout::raise(result);
}

fn http2_request(
    context: *mut Context,
    http1: *mut Context,
    server: &HttpServer,
) -> Result<(), bailout::Bailout> {
    let target = send::state(context).request.zserver.ptr;
    let source = send::state(http1).request.zserver.ptr;
    // SAFETY: a live array.
    let table = unsafe { value::table(target) };
    // SAFETY: a live array.
    let source = unsafe { value::table(source) };
    // SAFETY: both contexts' live `$server` arrays; each entry is added with
    // its own reference.
    unsafe { z::swr_zend_hash_foreach_kv(source, Some(copy_entry), table.cast()) };
    // SAFETY: Zend interns the name once; the array keeps it.
    let key = unsafe { z::swr_zend_http_intern(c"server_protocol".as_ptr(), 15) };
    // SAFETY: as above.
    let http2 = unsafe { z::swr_zend_http_intern(c"HTTP/2".as_ptr(), 6) };
    // SAFETY: a zval is plain data.
    let mut protocol = unsafe { std::mem::zeroed::<zval>() };
    // SAFETY: an empty slot takes the interned string.
    unsafe { z::swr_zval_str(&raw mut protocol, http2) };
    // SAFETY: the live array, the interned key and value.
    unsafe { z::zend_hash_add_new(table, key, &raw mut protocol) };
    let callable = server.handler(context);
    let mut args = objects(context);
    let mut result = Ok(());
    if callable.is_null() {
        send::state(context).response.status = NOT_FOUND;
    } else {
        // SAFETY: the server's live handler and the arguments.
        match unsafe { swr_hsc_call(callable, args.as_mut_ptr(), true) } {
            -1 => result = Err(bailout::Bailout),
            0 => {
                let stream_id = send::state(context).stream_id;
                // SAFETY: the stream's live context.
                unsafe { swr_v1_phphttp2_server_reset(context, stream_id, HTTP2_INTERNAL_ERROR) };
                host::notice("Swoole\\Http\\Server->onRequest[v2] handler error");
            }
            _ => {}
        }
    }
    if result.is_ok() {
        // SAFETY: the context's objects, released once.
        unsafe { zval_ptr_dtor(&raw mut args[0]) };
        // SAFETY: as above.
        unsafe { zval_ptr_dtor(&raw mut args[1]) };
    }
    result
}

/// `http_server_add_server_array(ht, key, value)` and the string's addref.
unsafe extern "C" fn copy_entry(
    key: *mut z::zend_string,
    _: z::zend_ulong,
    entry: *mut zval,
    table: *mut c_void,
) {
    // SAFETY: a live array entry and the target table.
    unsafe { z::zend_hash_add_new(table.cast(), key, entry) };
    // SAFETY: as above.
    if unsafe { type_of(entry) } == IS_STRING {
        // SAFETY: a refcounted or interned string.
        unsafe { z::swr_zval_try_addref(entry) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_ends_at_the_first_blank_line() {
        assert_eq!(find_crlf2(b"GET / HTTP/1.1\r\n\r\nbody"), Some(14), "end");
        assert_eq!(find_crlf2(b"GET / HTTP/1.1\r\n"), None, "incomplete");
    }

    #[test]
    fn a_handler_matches_a_prefix_of_the_path_ignoring_case() {
        let mut slots = [0_u8; 3];
        let [root, api, stop] = [0, 1, 2].map(|i| (&raw mut slots[i]).cast::<c_void>());
        let handlers: Vec<Handler> = vec![
            ((*b"/").into(), root),
            ((*b"/api").into(), api),
            ((*b"/stop").into(), stop),
        ];
        let path = |path: &'static [u8]| Text {
            data: path.as_ptr(),
            length: path.len(),
        };
        assert_eq!(select(&handlers, root, path(b"/API/users")), api);
        assert_eq!(select(&handlers, root, path(b"/stop")), stop);
        assert_eq!(select(&handlers, root, path(b"/other")), root, "default");
        assert!(select(&handlers[1..], ptr::null_mut(), path(b"/x")).is_null());
    }

    #[test]
    fn std_follows_the_server_pointer() {
        assert_eq!(Holder::OFFSET, size_of::<*mut c_void>(), "HttpServerObject");
    }
}
