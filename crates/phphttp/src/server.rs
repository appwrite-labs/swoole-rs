//! `Swoole\Http\Server` from upstream's `swoole_http_server.cc`: `onReceive`'s
//! routing and request parsing, the addresses cached for keep-alive sessions
//! until they close, and the requests queued past `worker_max_concurrency`.
//! The other protocols' `onReceive`, the port callbacks and the received data
//! stay C++, and so does the handler's call (no Rust frame under PHP code).
use crate::{
    context,
    host::{self, Context},
    parse::{Key, add_protocol, key, request_times, swr_v1_phphttp_parse},
    send::state,
    websocket,
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicU32, Ordering::SeqCst},
};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_server::{ConnectionRecord, view};
use swoole_rs_zend::sys::{
    HashTable, ZEND_ACC_NOT_SERIALIZABLE, swr_zend_http_uploads_rinit,
    swr_zend_http_uploads_rshutdown, swr_zend_string_copy, swr_zend_string_init,
    swr_zend_string_release, swr_zval_str, zend_class_entry, zend_hash_add_new, zend_long,
    zend_string, zval, zval_ptr_dtor,
};
use swoole_rs_zend::{Class, bailout, string, value};

/// `Swoole\Http\Server`: `Swoole\Server` with no methods of its own.
static SERVER: Class = Class::new();

/// `Swoole\Http\Server`, as C++ reads it; MINIT writes it.
#[unsafe(no_mangle)]
pub static mut swoole_http_server_ce: *mut zend_class_entry = ptr::null_mut();

unsafe extern "C" {
    static swoole_server_ce: *mut zend_class_entry;
}

/// `php_swoole_http_server_minit`, after `Swoole\Server`'s.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_minit() {
    // SAFETY: MINIT, after the parent class's registration.
    let parent = unsafe { swoole_server_ce };
    // SAFETY: MINIT, with no methods.
    unsafe {
        SERVER.register_child(c"Swoole\\Http\\Server", ptr::null(), parent, |entry, _| {
            entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
        });
    }
    // SAFETY: MINIT, before any thread reads it.
    unsafe { swoole_http_server_ce = SERVER.entry() };
}

/// `SW_HTTP_SERVICE_UNAVAILABLE_PACKET`.
const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\r\n";
/// `SW_HTTP_BAD_REQUEST_PACKET`.
const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\r\n";
/// llhttp's `HPE_OK`.
const HPE_OK: i32 = 0;
/// `WebSocket::STATUS_CONNECTION` and `STATUS_HANDSHAKE`.
const STATUS_CONNECTION: u8 = 1;
const STATUS_HANDSHAKE: u8 = 2;
/// `SW_ERROR_SESSION_NOT_EXIST` and `SW_LOG_TRACE`.
const SESSION_NOT_EXIST: c_int = 1005;
const LOG_TRACE: c_int = 1;

/// Where `onReceive` sends the data.
const CLOSED: c_int = 0;
const TCP: c_int = 1;
const WEBSOCKET: c_int = 2;
const HTTP2: c_int = 3;
const REQUEST: c_int = 4;
const DONE: c_int = 5;

/// The connection and its port, as `onReceive` routes by.
struct Receive {
    last_recv_time: f64,
    local_port: i64,
    remote_port: i64,
    loopback: c_int,
    websocket_status: u8,
    /// The port speaks HTTP and has `onRequest`.
    http: bool,
    /// The port speaks WebSocket and has `onMessage` (asked only when `http` is false).
    websocket: bool,
    http2: bool,
    /// The session's addresses may be cached.
    cache: bool,
    /// The server's `enable_coroutine`: handlers run in a new coroutine.
    coroutine: bool,
}

/// `SW_SERVER_CB_onRequest`, `onHandshake` and `onMessage`.
const ON_REQUEST: c_int = 4;
const ON_HANDSHAKE: c_int = 5;
const ON_MESSAGE: c_int = 8;
/// `PHP_SWOOLE_HOOK_BEFORE_REQUEST` (`SW_GLOBAL_HOOK_USER + 2`).
const HOOK_BEFORE_REQUEST: c_int = 26;
/// `SW_SOCK_TCP6`, `SW_MANAGER`, `SW_TASK_WORKER` and the server modes.
const SOCK_TCP6: c_int = 3;
const MANAGER: c_int = 3;
const TASK_WORKER: c_int = 4;
const MODE_BASE: u8 = 1;
const MODE_PROCESS: u8 = 2;
/// `Server::DISPATCH_FDMOD`.
const DISPATCH_FDMOD: u8 = 2;

// What stays C++: the TCP, WebSocket and HTTP/2 `onReceive`s and `onClose`
// (ext-src), the port callbacks and the received data (swoole_server.cc), the
// connection's addresses (master.cc) and its peer's port and loopback check
// (crates/clients).
unsafe extern "C" {
    #[link_name = "_Z27php_swoole_server_onReceivePN6swoole6ServerEPNS_8RecvDataE"]
    fn php_swoole_server_onReceive(server: *mut c_void, request: *mut c_void) -> c_int;
    #[link_name = "_Z26swoole_websocket_onMessagePN6swoole6ServerEPNS_8RecvDataE"]
    fn swoole_websocket_onMessage(server: *mut c_void, request: *mut c_void) -> c_int;
    #[link_name = "_Z29swoole_http2_server_onReceivePN6swoole6ServerEPNS_10ConnectionEPNS_8RecvDataE"]
    fn swoole_http2_server_onReceive(
        server: *mut c_void,
        conn: *mut ConnectionRecord,
        request: *mut c_void,
    ) -> c_int;
    #[link_name = "_Z25php_swoole_server_onClosePN6swoole6ServerEPNS_8DataHeadE"]
    fn php_swoole_server_onClose(server: *mut c_void, info: *mut DataHead);
    #[link_name = "_Z32php_swoole_server_isset_callbackPN6swoole6ServerEPNS_10ListenPortEi"]
    fn php_swoole_server_isset_callback(
        server: *mut c_void,
        port: *mut c_void,
        kind: c_int,
    ) -> bool;
    #[link_name = "_Z30php_swoole_server_get_callbackPN6swoole6ServerEii"]
    fn php_swoole_server_get_callback(
        server: *mut c_void,
        server_fd: c_int,
        kind: c_int,
    ) -> *mut c_void;
    #[link_name = "_Z24php_swoole_get_recv_dataPN6swoole6ServerEP12_zval_structPNS_8RecvDataE"]
    fn php_swoole_get_recv_data(server: *mut c_void, zdata: *mut zval, request: *mut c_void);
    #[link_name = "_ZN6swoole6Server14get_local_addrEPNS_10ConnectionE"]
    fn get_local_addr(server: *mut c_void, conn: *mut ConnectionRecord) -> *const c_char;
    #[link_name = "_ZN6swoole6Server15get_remote_addrEPNS_10ConnectionE"]
    fn get_remote_addr(server: *mut c_void, conn: *mut ConnectionRecord) -> *const c_char;
    fn swr_v1_clients_address_port(
        address: *const swoole_rs_corebase::net_types::SocketAddress,
    ) -> c_int;
    fn swr_v1_clients_address_loopback(
        address: *const swoole_rs_corebase::net_types::SocketAddress,
    ) -> bool;
    fn swr_v1_event_reactor() -> *mut c_void;
}

thread_local! {
    /// A keep-alive session's server and remote address, until it closes.
    static ADDRESSES: RefCell<HashMap<i64, [*mut zend_string; 2]>> = RefCell::new(HashMap::new());
    /// Requests waiting for the worker's concurrency to drop.
    static QUEUED: RefCell<VecDeque<*mut Context>> = const { RefCell::new(VecDeque::new()) };
}

fn new_zval() -> zval {
    // SAFETY: all zeroes is `IS_UNDEF`.
    unsafe { std::mem::zeroed() }
}

pub(crate) fn add(table: *mut HashTable, name: Key, string: *mut zend_string) {
    let mut value = new_zval();
    // SAFETY: a string the table takes over (or an interned one).
    unsafe { swr_zval_str(&raw mut value, string) };
    // SAFETY: the request's live `server` table and an interned key.
    unsafe { zend_hash_add_new(table, key(name), &raw mut value) };
}

pub(crate) fn add_long(table: *mut HashTable, name: Key, long: zend_long) {
    let mut value = new_zval();
    // SAFETY: writes a long into a local zval.
    unsafe { swoole_rs_zend::value::set_long(&raw mut value, long) };
    // SAFETY: as `add`.
    unsafe { zend_hash_add_new(table, key(name), &raw mut value) };
}

fn address(server: *mut c_void, conn: *mut ConnectionRecord, local: bool) -> *mut zend_string {
    let get = if local {
        get_local_addr
    } else {
        get_remote_addr
    };
    // SAFETY: the live server and connection the caller passed.
    let text = unsafe { get(server, conn) };
    // SAFETY: the server's NUL-terminated address text.
    let text = unsafe { CStr::from_ptr(text) };
    // SAFETY: a byte span of the address text.
    unsafe { swr_zend_string_init(text.as_ptr(), text.to_bytes().len()) }
}

/// `swoole_http_server_populate_ip_and_port`: `$request->server`'s ports and
/// addresses. `loopback` is 4 or 6 for a loopback peer, otherwise 0. With
/// `cache`, a session's addresses are made once and kept until the session
/// closes.
/// # Safety
/// `server` and `conn` are the live server and the request's connection;
/// `table` is the request's `server` array.
#[expect(clippy::too_many_arguments, reason = "upstream's request facts")]
unsafe fn addresses(
    server: *mut c_void,
    conn: *mut ConnectionRecord,
    table: *mut HashTable,
    session: i64,
    local_port: i64,
    remote_port: i64,
    loopback: c_int,
    cache: bool,
) {
    add_long(table, Key::ServerPort, local_port);
    add_long(table, Key::RemotePort, remote_port);
    if loopback != 0 {
        let name = if loopback == 6 {
            Key::LoopbackV6
        } else {
            Key::LoopbackV4
        };
        add(table, Key::ServerAddr, key(name));
        add(table, Key::RemoteAddr, key(name));
        return;
    }
    let [local, remote] = if cache {
        let [local, remote] = ADDRESSES.with_borrow_mut(|cached| {
            *cached
                .entry(session)
                .or_insert_with(|| [address(server, conn, true), address(server, conn, false)])
        });
        // SAFETY: cached strings, each referenced once more by the array.
        let local = unsafe { swr_zend_string_copy(local) };
        // SAFETY: as above.
        [local, unsafe { swr_zend_string_copy(remote) }]
    } else {
        [address(server, conn, true), address(server, conn, false)]
    };
    add(table, Key::ServerAddr, local);
    add(table, Key::RemoteAddr, remote);
}

/// `php_swoole_http_server_onReceive`: route the data to another protocol's
/// `onReceive`, or parse it into a new context and run its handler. A
/// bailout (a handshake's `onOpen`, the handler's `exit()`) is raised last.
/// # Safety
/// `server` is the live server and `request` its `RecvData`.
#[unsafe(export_name = "_Z32php_swoole_http_server_onReceivePN6swoole6ServerEPNS_8RecvDataE")]
pub(crate) unsafe extern "C" fn on_receive(server: *mut c_void, request: *mut c_void) -> c_int {
    // SAFETY: a `RecvData` begins with its `DataHead`.
    let info = unsafe { *request.cast::<DataHead>() };
    match receive(server, request, info.fd, c_int::from(info.server_fd)) {
        CLOSED => -1,
        // SAFETY: the live server and its request.
        TCP => unsafe { php_swoole_server_onReceive(server, request) },
        // SAFETY: as above.
        WEBSOCKET => unsafe { swoole_websocket_onMessage(server, request) },
        HTTP2 => {
            // SAFETY: the live server.
            let conn =
                unsafe { view::verified(server, info.fd) }.map_or(ptr::null_mut(), ptr::from_mut);
            // SAFETY: as above, with the session's connection.
            unsafe { swoole_http2_server_onReceive(server, conn, request) }
        }
        _ => 0,
    }
}

fn receive(server: *mut c_void, request: *mut c_void, session: i64, server_fd: c_int) -> c_int {
    // SAFETY: the live server.
    let Some(conn) = (unsafe { view::verified(server, session) }) else {
        host::log(
            LOG_TRACE,
            SESSION_NOT_EXIST,
            c"php_swoole_http_server_onReceive",
            format!("session[{session}] is closed"),
        );
        return CLOSED;
    };
    // SAFETY: the live server; a listening socket's record holds its port.
    let port = unsafe { view::port_by_server_fd(server, server_fd) };
    // SAFETY: the connection's live port.
    let config = unsafe { view::port(port) };
    // SAFETY: the live server and port.
    let isset = |kind| unsafe { php_swoole_server_isset_callback(server, port, kind) };
    // Only whether either is set routes the data, so onMessage is asked only without onRequest.
    let http = config.open_http_protocol && isset(ON_REQUEST);
    let websocket = !http && config.open_websocket_protocol && isset(ON_MESSAGE);
    let mut facts = Receive {
        last_recv_time: conn.last_recv_time,
        local_port: i64::from(conn.local_port),
        remote_port: 0,
        loopback: 0,
        websocket_status: conn.websocket_status,
        http,
        websocket,
        http2: conn.http2_stream != 0,
        cache: false,
        coroutine: false,
    };
    let route = route(&facts);
    if route != REQUEST {
        return route;
    }
    // SAFETY: the connection's own address record.
    facts.remote_port = i64::from(unsafe { swr_v1_clients_address_port(&raw const conn.info) });
    // SAFETY: as above.
    if unsafe { swr_v1_clients_address_loopback(&raw const conn.info) } {
        facts.loopback = if conn.info.kind == SOCK_TCP6 { 6 } else { 4 };
    }
    facts.cache = can_cache(server);
    facts.coroutine = enable_coroutine(server);
    let ctx = context::context_new(session);
    // SAFETY: a new context and the live server.
    unsafe { context::swr_v1_phphttp_context_init(ctx, server) };
    let state = state(ctx);
    state.before_request = 1;
    let zdata = &raw mut state.request.zdata;
    // SAFETY: the live server and its request; the data moves into the context.
    unsafe { php_swoole_get_recv_data(server, zdata, request) };
    crate::parse::init_request_parser(ctx);
    let conn = ptr::from_mut(conn);
    let result = handle(server, conn, ctx, zdata, session, server_fd, &facts).and_then(|route| {
        if route == REQUEST {
            // SAFETY: a live context; false leaves it queued.
            if !unsafe { context::before_request(ctx) } {
                return Ok(());
            }
            call(ctx, facts.coroutine)?;
        }
        release_objects(ctx);
        Ok(())
    });
    bailout::raise(result);
    DONE
}

/// `swoole_http_server_can_cache_session_addr`: a keep-alive session's
/// addresses stay cached until its close, so only when that close reaches
/// the same worker.
#[unsafe(export_name = "_Z41swoole_http_server_can_cache_session_addrPKN6swoole6ServerE")]
pub(crate) extern "C" fn can_cache(server: *mut c_void) -> bool {
    // SAFETY: the live server.
    let config = unsafe { view::config(server) };
    config.mode_ == MODE_BASE
        || (config.mode_ == MODE_PROCESS && config.dispatch_mode == DISPATCH_FDMOD)
}

/// `Server::is_enable_coroutine`.
fn enable_coroutine(server: *mut c_void) -> bool {
    // SAFETY: the live server.
    let config = unsafe { view::config(server) };
    match swoole_rs_process::methods::swoole_get_worker_type() {
        TASK_WORKER => config.task_enable_coroutine,
        MANAGER => false,
        _ => config.enable_coroutine,
    }
}

/// `http_server_process_request`: the handler with the request's objects.
fn call(ctx: *mut Context, coroutine: bool) -> Result<(), bailout::Bailout> {
    let state = state(ctx);
    let mut args = [state.request.zobject.value, state.response.zobject.value];
    // SAFETY: the handler and the request's two objects.
    match unsafe { swr_hsc_call(state.callback, args.as_mut_ptr(), coroutine) } {
        -1 => return Err(bailout::Bailout),
        0 => {
            host::notice("Swoole\\Http\\Server->onRequest handler error");
            context::send(ctx, SERVICE_UNAVAILABLE);
            context::close(ctx);
        }
        _ => {}
    }
    Ok(())
}

/// A queued request's turn (`swoole_event_defer`): run its handler, then
/// release its objects.
/// # Safety
/// `ctx` is a queued server request's live context.
unsafe extern "C-unwind" fn deferred(ctx: *mut c_void) {
    let ctx = ctx.cast::<Context>();
    let result = call(ctx, enable_coroutine(state(ctx).private_data));
    if result.is_ok() {
        release_objects(ctx);
    }
    bailout::raise(result);
}

/// Where `onReceive` sends data before any parsing.
fn route(facts: &Receive) -> c_int {
    if !facts.http && !facts.websocket {
        TCP
    } else if facts.websocket_status >= STATUS_HANDSHAKE {
        WEBSOCKET
    } else if facts.http2 {
        HTTP2
    } else {
        REQUEST
    }
}

fn notice(message: &str) {
    log::notice(format_args!("{message}"));
}

/// Parse the request, fill `$request->server` and pick the handler.
fn handle(
    server: *mut c_void,
    conn: *mut ConnectionRecord,
    ctx: *mut Context,
    zdata: *mut zval,
    session: i64,
    server_fd: c_int,
    facts: &Receive,
) -> Result<c_int, bailout::Bailout> {
    // SAFETY: the context's request data, a string.
    let data = unsafe { value::str_of(zdata) };
    // SAFETY: as above.
    let data = unsafe { string::bytes(data) };
    // SAFETY: a live context and its data.
    let parsed = unsafe { swr_v1_phphttp_parse(ctx, data.as_ptr().cast(), data.len()) };
    if state(ctx).parser.error != HPE_OK || state(ctx).completed == 0 {
        context::send(ctx, BAD_REQUEST);
        context::close(ctx);
        let parser = &state(ctx).parser;
        if parser.error == HPE_OK {
            notice("Incomplete HTTP request: parsed successfully but missing required components");
        } else {
            // SAFETY: llhttp's NUL-terminated reason for its error.
            let reason = unsafe { CStr::from_ptr(parser.reason) };
            notice(&format!(
                "Invalid HTTP request discarded: {} bytes unprocessed. Reason: {}",
                data.len() - parsed,
                reason.to_string_lossy()
            ));
        }
        return Ok(DONE);
    }
    let state = state(ctx);
    // SAFETY: the request's `server` array.
    let table = unsafe { value::table(state.request.zserver.ptr) };
    // SAFETY: the live server and connection and the request's array.
    unsafe {
        addresses(
            server,
            conn,
            table,
            session,
            facts.local_port,
            facts.remote_port,
            facts.loopback,
            state.keepalive != 0 && facts.cache,
        );
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (zend_long) cast"
    )]
    add_long(table, Key::MasterTime, facts.last_recv_time as zend_long);
    if swoole_rs_corebase::swoole_isset_hook(HOOK_BEFORE_REQUEST) {
        // SAFETY: the hook's context argument.
        unsafe { swoole_rs_corebase::swoole_call_hook(HOOK_BEFORE_REQUEST, ctx) };
    }
    // Parsing an upgrade request sets the connection's status.
    // SAFETY: the request's live connection.
    let handshake = unsafe { (*conn).websocket_status } == STATUS_CONNECTION;
    let kind = if handshake { ON_HANDSHAKE } else { ON_REQUEST };
    // SAFETY: the live server.
    let callback = unsafe { php_swoole_server_get_callback(server, server_fd, kind) };
    if callback.is_null() {
        if handshake {
            websocket::handshake_open(ctx)?;
        } else {
            websocket::request(ctx);
        }
        return Ok(DONE);
    }
    state.callback = callback;
    if handshake {
        websocket::upgrading(server, session);
        state.upgrade = 1;
    }
    Ok(REQUEST)
}

fn release([local, remote]: [*mut zend_string; 2]) {
    // SAFETY: the cache's references.
    unsafe { swr_zend_string_release(local) };
    // SAFETY: as above.
    unsafe { swr_zend_string_release(remote) };
}

/// `swr_h2s_connection_facts`: an HTTP/2 request's connection and server state.
#[repr(C)]
struct Http2Connection {
    last_recv_time: f64,
    local_port: i64,
    remote_port: i64,
    loopback: c_int,
    server_fd: c_int,
    cache: bool,
    static_handler: bool,
    unavailable: bool,
    coroutine: bool,
}

/// `swr_h2s_request`.
#[repr(C)]
struct Http2Request {
    ctx: *mut Context,
    default_ctx: *mut c_void,
    data: *mut c_void,
}

/// `SW_HTTP_VERSION_2`, `SW_HTTP_SERVICE_UNAVAILABLE` and
/// `SW_HTTP2_ERROR_INTERNAL_ERROR`.
const HTTP_VERSION_2: c_int = 3;
const SERVICE_UNAVAILABLE_STATUS: c_int = 503;
const HTTP2_INTERNAL_ERROR: u32 = 2;

unsafe extern "C" {
    /// `get_connection_by_session_id` (null: closed) and what onRequest reads.
    fn swr_h2s_connection(
        server: *mut c_void,
        fd: i64,
        out: *mut Http2Connection,
    ) -> *mut ConnectionRecord;
    /// `zend::function::call(callable, 2, args)`: 1 called, 0 failed, -1 bailed out.
    fn swr_hsc_call(callable: *mut c_void, args: *mut zval, coroutine: bool) -> c_int;
    /// The HTTP/2 server's (crates/phphttp2): the static handler, a whole
    /// response and a stream reset.
    fn swr_v1_phphttp2_server_static(server: *mut c_void, ctx: *mut Context) -> bool;
    fn swr_v1_phphttp2_server_respond(
        ctx: *mut Context,
        data: *const c_char,
        length: usize,
    ) -> bool;
    fn swr_v1_phphttp2_server_reset(ctx: *mut Context, stream_id: u32, code: u32);
}

/// `http2_server_onRequest`: a finished HTTP/2 request on a server's
/// connection, answered by the static handler or the port's `onRequest`.
/// A bailout in the handler is raised last, over this frame only.
/// # Safety
/// `request` is the session's `swr_h2s_request` for a live stream context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_http2_request(request: *mut c_void) {
    // SAFETY: per the caller.
    let ctx = unsafe { (*request.cast::<Http2Request>()).ctx };
    let result = http2_request(ctx);
    bailout::raise(result);
}

fn http2_request(ctx: *mut Context) -> Result<(), bailout::Bailout> {
    let (server, fd) = (state(ctx).private_data, state(ctx).fd);
    // SAFETY: plain data, filled below.
    let mut facts: Http2Connection = unsafe { std::mem::zeroed() };
    // SAFETY: the live server and a local output.
    let conn = unsafe { swr_h2s_connection(server, fd, &raw mut facts) };
    if conn.is_null() {
        release_objects(ctx);
        return Ok(());
    }
    state(ctx).request.version = HTTP_VERSION_2;
    // SAFETY: the live server and stream context.
    if facts.static_handler && unsafe { swr_v1_phphttp2_server_static(server, ctx) } {
        release_objects(ctx);
        return Ok(());
    }
    let state = state(ctx);
    // SAFETY: the request's `server` array.
    let table = unsafe { value::table(state.request.zserver.ptr) };
    // SAFETY: the live server and connection and the request's array.
    unsafe {
        addresses(
            server,
            conn,
            table,
            fd,
            facts.local_port,
            facts.remote_port,
            facts.loopback,
            state.keepalive != 0 && facts.cache,
        );
    }
    request_times(state);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (zend_long) cast"
    )]
    add_long(table, Key::MasterTime, facts.last_recv_time as zend_long);
    add_protocol(state, Key::Http2);
    // SAFETY: the live server.
    state.callback = unsafe { php_swoole_server_get_callback(server, facts.server_fd, ON_REQUEST) };
    if state.before_request != 0 {
        if facts.unavailable {
            state.response.status = SERVICE_UNAVAILABLE_STATUS;
            // SAFETY: a live stream context; an empty body.
            unsafe { swr_v1_phphttp2_server_respond(ctx, ptr::null(), 0) };
            release_objects(ctx);
            return Ok(());
        }
        // SAFETY: a live context.
        if !unsafe { context::before_request(ctx) } {
            return Ok(());
        }
    }
    let state = self::state(ctx);
    let mut args = [state.request.zobject.value, state.response.zobject.value];
    // SAFETY: the handler (or null) and the request's two objects.
    match unsafe { swr_hsc_call(state.callback, args.as_mut_ptr(), facts.coroutine) } {
        -1 => return Err(bailout::Bailout),
        0 => {
            let stream_id = self::state(ctx).stream_id;
            // SAFETY: the stream's live context.
            unsafe { swr_v1_phphttp2_server_reset(ctx, stream_id, HTTP2_INTERNAL_ERROR) };
            host::notice("Swoole\\Http\\Server->onRequest[v2] handler error");
        }
        _ => {}
    }
    release_objects(ctx);
    Ok(())
}

/// The request's own references to its objects, released once handled.
fn release_objects(ctx: *mut Context) {
    // SAFETY: the request's object, referenced by the request.
    unsafe { zval_ptr_dtor(state(ctx).request.zobject.ptr) };
    // SAFETY: as above.
    unsafe { zval_ptr_dtor(state(ctx).response.zobject.ptr) };
}

/// `php_swoole_http_server_onClose`: forget the session's addresses, then
/// the server's `onClose`.
/// # Safety
/// `server` is the live server and `info` the closed session's.
#[unsafe(export_name = "_Z30php_swoole_http_server_onClosePN6swoole6ServerEPNS_8DataHeadE")]
pub(crate) unsafe extern "C" fn on_close(server: *mut c_void, info: *mut DataHead) {
    // SAFETY: per the caller.
    let session = unsafe { (*info).fd };
    if let Some(addresses) = ADDRESSES.with_borrow_mut(|cached| cached.remove(&session)) {
        release(addresses);
    }
    // SAFETY: per the caller.
    unsafe { php_swoole_server_onClose(server, info) };
}

/// The counters of the worker serving requests.
struct Worker {
    /// `Worker::concurrency`.
    concurrency: *mut u32,
    /// `ServerGS::concurrency`, shared by every worker.
    server_concurrency: &'static AtomicU32,
    max_concurrency: u32,
    shutdown: bool,
}

/// `sw_server()` and `sw_worker()`'s counters; none when this process is no
/// server worker.
fn worker() -> Option<Worker> {
    let server = swoole_rs_corebase::swr_v1_core_server();
    let current = swoole_rs_process::worker_globals::swr_v1_worker_globals();
    // SAFETY: this thread's worker record.
    let current = unsafe { (*current).worker };
    if server.is_null() || current.is_null() {
        return None;
    }
    // SAFETY: the running server.
    let config = unsafe { view::config(server) };
    Some(Worker {
        // SAFETY: the current worker's live record.
        concurrency: unsafe { &raw mut (*current).concurrency },
        // SAFETY: the running server's shared counters.
        server_concurrency: &unsafe { view::counters(server) }.concurrency,
        max_concurrency: config.worker_max_concurrency,
        shutdown: swoole_rs_process::methods::worker_is_shutdown(current),
    })
}

/// Mark a queued request answered and release its objects.
fn abandon(ctx: *mut Context) {
    state(ctx).end = 1;
    state(ctx).after_response = 0;
    // SAFETY: the queued request's own references to its objects.
    unsafe { zval_ptr_dtor(state(ctx).request.zobject.ptr) };
    // SAFETY: as above.
    unsafe { zval_ptr_dtor(state(ctx).response.zobject.ptr) };
}

/// `swoole_http_server_onBeforeRequest` after it swaps the hooks: count the
/// request, or queue it past `worker_max_concurrency` (false: not now).
/// # Safety
/// `ctx` is a live server request's context.
pub(crate) unsafe fn admit(ctx: *mut Context) -> bool {
    let Some(worker) = worker().filter(|worker| !worker.shutdown) else {
        return false;
    };
    worker.server_concurrency.fetch_add(1, SeqCst);
    // SAFETY: this worker's own counter.
    let concurrency = unsafe { &mut *worker.concurrency };
    *concurrency += 1;
    if *concurrency > worker.max_concurrency {
        QUEUED.with_borrow_mut(|queued| queued.push_back(ctx));
        return false;
    }
    true
}

/// `swoole_http_server_onAfterResponse` after it clears the hook: uncount the
/// request and start the next queued one, or answer every queued one with
/// 503 once the worker is shutting down.
pub(crate) fn release_request() {
    let Some(worker) = worker() else {
        return;
    };
    if worker.shutdown {
        while let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
            context::send(ctx, SERVICE_UNAVAILABLE);
            context::close(ctx);
            abandon(ctx);
        }
        return;
    }
    worker.server_concurrency.fetch_sub(1, SeqCst);
    // SAFETY: this worker's own counter.
    let concurrency = unsafe { &mut *worker.concurrency };
    *concurrency = concurrency.wrapping_sub(1);
    if let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
        // SAFETY: no preconditions.
        let reactor = unsafe { swr_v1_event_reactor() };
        // SAFETY: the thread's live reactor, which begins with its prepared state.
        let fields = unsafe { (*reactor.cast::<swoole_rs_reactor::ReactorPrepared>()).fields };
        // SAFETY: a queued context, alive until its deferred request ends.
        unsafe { swoole_rs_reactor::defer(fields, deferred, ctx.cast()) };
    }
}

/// `php_swoole_http_server_rinit`: the uploaded files `is_uploaded_file`
/// and `move_uploaded_file` know.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_server_rinit() {
    // SAFETY: RINIT, on the request's thread.
    unsafe { swr_zend_http_uploads_rinit() };
}

/// `php_swoole_http_server_rshutdown`: forget the uploaded files, then drop
/// the cached addresses and the queued requests.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_server_rshutdown() {
    // SAFETY: RSHUTDOWN, on the request's thread.
    unsafe { swr_zend_http_uploads_rshutdown() };
    for addresses in ADDRESSES.with_borrow_mut(std::mem::take).into_values() {
        release(addresses);
    }
    while let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
        abandon(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http2_connection_matches_the_c_struct() {
        // `swr_h2s_connection_facts`, which C++ asserts is 40 bytes.
        assert_eq!(size_of::<Http2Connection>(), 40);
        assert_eq!(std::mem::offset_of!(Http2Connection, server_fd), 28);
        assert_eq!(std::mem::offset_of!(Http2Connection, coroutine), 35);
        assert_eq!(size_of::<Http2Request>(), 24);
    }

    fn facts(http: bool, websocket: bool, status: u8, http2: bool) -> Receive {
        Receive {
            last_recv_time: 0.0,
            local_port: 0,
            remote_port: 0,
            loopback: 0,
            websocket_status: status,
            http,
            websocket,
            http2,
            cache: false,
            coroutine: false,
        }
    }

    #[test]
    fn data_goes_where_upstream_sends_it() {
        assert_eq!(route(&facts(false, false, STATUS_HANDSHAKE, true)), TCP);
        assert_eq!(
            route(&facts(true, false, STATUS_HANDSHAKE, true)),
            WEBSOCKET
        );
        assert_eq!(route(&facts(false, true, 3, false)), WEBSOCKET);
        assert_eq!(route(&facts(true, false, STATUS_CONNECTION, true)), HTTP2);
        assert_eq!(route(&facts(true, true, STATUS_CONNECTION, false)), REQUEST);
        assert_eq!(route(&facts(true, false, 0, false)), REQUEST);
    }
}
