//! `Swoole\Http\Server` from upstream's `swoole_http_server.cc`: `onReceive`'s
//! routing and request parsing, the addresses cached for keep-alive sessions
//! until they close, and the requests queued past `worker_max_concurrency`.
//! The other protocols' `onReceive` and the received data are crates/phpserver's;
//! handlers and port callbacks run through `swoole_rs_zend::host::function_call`,
//! which traps a bailout before it reaches a Rust frame.
use crate::{
    context::{self, Ctx},
    host,
    parse::{Key, add_protocol, add_server, key, parse_request, request_times},
    state::State,
    websocket,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    ffi::{CStr, c_int, c_void},
    mem::ManuallyDrop,
    ptr,
    sync::atomic::{AtomicU32, Ordering::SeqCst},
};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_memory::flat::Table;
use swoole_rs_phpserver::{
    RecvData, php_swoole_get_recv_data, php_swoole_server_get_callback,
    php_swoole_server_isset_callback, php_swoole_server_on_close, php_swoole_server_on_receive,
};
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_server::{ConnectionRecord, view};
use swoole_rs_zend::sys::{
    ZEND_ACC_NOT_SERIALIZABLE, swr_zend_http_uploads_rinit, swr_zend_http_uploads_rshutdown,
    zend_class_entry, zend_long, zval,
};
use swoole_rs_zend::{Class, Owned, ZStr, ZString, bailout};

/// `Swoole\Http\Server`: `Swoole\Server` with no methods of its own.
pub(crate) static SERVER: Class = Class::new();

/// `Swoole\Http\Server`, once MINIT registered it.
#[must_use]
pub fn server_class() -> *mut zend_class_entry {
    SERVER.entry()
}

/// `php_swoole_http_server_minit`, after `Swoole\Server`'s.
pub fn server_minit(args: &MinitArgs) {
    let minit = &args.minit;
    let parent = minit.parent(swoole_rs_phpserver::server_class_ref());
    minit.register_child(&SERVER, c"Swoole\\Http\\Server", &[], parent, |entry, _| {
        entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    });
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

/// `http_server::SW_HTTP_EXT_FLAG_UPLOAD_PREPROCESSED`.
const UPLOAD_PREPROCESSED: u16 = 1;

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
    /// The connection's fd, when the session's addresses may be cached.
    cache: Option<usize>,
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

// The connection's addresses and its peer's port and loopback check.
use swoole_rs_clients::address::{
    swr_v1_clients_address_host, swr_v1_clients_address_loopback, swr_v1_clients_address_port,
};

/// A keep-alive session's server and remote address, until it closes, by its
/// connection's fd. The session check drops a reused fd's entry. RSHUTDOWN
/// releases them; a thread's exit, after the request heap is gone, must not.
type Addresses = ManuallyDrop<Table<(i64, [ZString; 2])>>;

php_local! {
    static ADDRESSES: RefCell<Addresses> = RefCell::new(ManuallyDrop::new(Table::new()));
    /// Requests waiting for the worker's concurrency to drop.
    static QUEUED: RefCell<VecDeque<Ctx>> = RefCell::new(VecDeque::new());
}

/// An `IS_UNDEF` zval.
pub(crate) fn new_zval() -> zval {
    swoole_rs_zend::UNDEF
}

fn address(server: view::Server<'_>, conn: &ConnectionRecord, local: bool) -> ZString {
    let text = if local {
        // SAFETY: the live server and one of its connections.
        unsafe { swoole_rs_server::local_addr::get_local_addr(server.raw(), conn) }
    } else {
        swr_v1_clients_address_host(&conn.info)
    };
    // SAFETY: the server's NUL-terminated address text.
    ZString::new(unsafe { CStr::from_ptr(text) }.to_bytes())
}

/// `swoole_http_server_populate_ip_and_port`: `$request->server`'s ports and
/// addresses. `loopback` is 4 or 6 for a loopback peer, otherwise 0. With
/// `cache`, the connection's fd, a session's addresses are made once and kept
/// until the session closes. `server` and `conn` are the live server and the
/// request's connection; `state` the request's context.
#[expect(clippy::too_many_arguments, reason = "upstream's request facts")]
fn addresses(
    server: view::Server<'_>,
    conn: &ConnectionRecord,
    state: &mut State,
    session: i64,
    local_port: i64,
    remote_port: i64,
    loopback: c_int,
    cache: Option<usize>,
) {
    add_server(state, Key::ServerPort, Owned::long(local_port));
    add_server(state, Key::RemotePort, Owned::long(remote_port));
    if loopback != 0 {
        let name = if loopback == 6 {
            Key::LoopbackV6
        } else {
            Key::LoopbackV4
        };
        if let Some(name) = key(name) {
            add_server(state, Key::ServerAddr, Owned::string(name.to_owned()));
            add_server(state, Key::RemoteAddr, Owned::string(name.to_owned()));
        }
        return;
    }
    let cached = cache.and_then(|fd| cached(server, conn, fd, session));
    let [local, remote] =
        cached.unwrap_or_else(|| [address(server, conn, true), address(server, conn, false)]);
    add_server(state, Key::ServerAddr, Owned::string(local));
    add_server(state, Key::RemoteAddr, Owned::string(remote));
}

/// The session's cached addresses, each referenced once more for the caller;
/// `None` when the cache can't grow.
fn cached(
    server: view::Server<'_>,
    conn: &ConnectionRecord,
    fd: usize,
    session: i64,
) -> Option<[ZString; 2]> {
    ADDRESSES.with_borrow_mut(|cached| {
        if let Some((owner, addresses)) = cached.get(fd)
            && *owner == session
        {
            return Some(addresses.each_ref().map(|address| ZStr::to_owned(address)));
        }
        let addresses = [address(server, conn, true), address(server, conn, false)];
        let copies = addresses.each_ref().map(|address| ZStr::to_owned(address));
        cached.insert(fd, (session, addresses)).ok().map(|_| copies)
    })
}

/// `php_swoole_http_server_onReceive`: route the data to another protocol's
/// `onReceive`, or parse it into a new context and run its handler. A
/// bailout (a handshake's `onOpen`, the handler's `exit()`) is raised last.
#[must_use]
pub fn http_server_on_receive(server: view::Server<'_>, request: &RecvData) -> c_int {
    let info = request.info();
    match receive(server, request) {
        CLOSED => -1,
        TCP => php_swoole_server_on_receive(server, request),
        WEBSOCKET => websocket::on_message(server, request),
        HTTP2 => {
            // `receive` verified the session.
            let (Some(conn), Some(http2)) = (server.verified(info.fd), crate::http2::server())
            else {
                return -1;
            };
            (http2.on_receive)(server, conn, request)
        }
        _ => 0,
    }
}

fn receive(server: view::Server<'_>, request: &RecvData) -> c_int {
    let info = request.info();
    let (session, server_fd) = (info.fd, c_int::from(info.server_fd));
    let Some(conn) = server.verified(session) else {
        host::log(
            LOG_TRACE,
            SESSION_NOT_EXIST,
            c"php_swoole_http_server_onReceive",
            format!("session[{session}] is closed"),
        );
        return CLOSED;
    };
    // A live connection's listening port.
    let Some(port) = server.port_by_server_fd(server_fd) else {
        return CLOSED;
    };
    let config = port.config();
    let isset = |kind| php_swoole_server_isset_callback(server, Some(port), kind);
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
        cache: None,
        coroutine: false,
    };
    let route = route(&facts);
    if route != REQUEST {
        return route;
    }
    facts.remote_port = i64::from(swr_v1_clients_address_port(&conn.info));
    if swr_v1_clients_address_loopback(&conn.info) {
        facts.loopback = if conn.info.kind == SOCK_TCP6 { 6 } else { 4 };
    }
    facts.cache = can_cache_session_addr(server)
        .then(|| usize::try_from(conn.fd).ok())
        .flatten();
    facts.coroutine = enable_coroutine(server);
    let mut ctx = context::context_new(session);
    context::init(ctx.state(), server);
    let state = ctx.state();
    state.before_request = 1;
    state.upload_preprocessed = u8::from(info.ext_flags & UPLOAD_PREPROCESSED != 0);
    let zdata = state.request.zdata.as_ptr();
    // SAFETY: the request's own zval, which nothing else borrows; the data moves into it.
    php_swoole_get_recv_data(server, unsafe { &mut *zdata }, request);
    crate::parse::init_request_parser(ctx);
    let conn = ptr::from_mut(conn);
    let result = handle(server, conn, ctx, session, server_fd, &facts).and_then(|route| {
        if route == REQUEST {
            // False leaves it queued, with its objects.
            if !context::before_request(ctx) {
                return Ok(());
            }
            call(ctx, facts.coroutine)?;
        }
        context::release_objects(ctx)
    });
    bailout::raise(result);
    DONE
}

/// `swoole_http_server_can_cache_session_addr`: a keep-alive session's
/// addresses stay cached until its close, so only when that close reaches
/// the same worker.
#[must_use]
pub fn can_cache_session_addr(server: view::Server<'_>) -> bool {
    let config = server.config();
    config.mode_ == MODE_BASE
        || (config.mode_ == MODE_PROCESS && config.dispatch_mode == DISPATCH_FDMOD)
}

/// `Server::is_enable_coroutine`.
fn enable_coroutine(server: view::Server<'_>) -> bool {
    let config = server.config();
    match swoole_rs_process::methods::swoole_get_worker_type() {
        TASK_WORKER => config.task_enable_coroutine,
        MANAGER => false,
        _ => config.enable_coroutine,
    }
}

pub(crate) use swoole_rs_zend::host::callable::Callable;

/// `zend::function::call(callable, args.len(), args, nullptr, coroutine)`:
/// whether the handler was called.
/// # Errors
/// It bailed out.
pub(crate) fn call_handler(
    callable: *mut Callable,
    args: &mut [zval],
    coroutine: bool,
) -> Result<bool, bailout::Bailout> {
    // SAFETY: a live handle.
    let cache = unsafe { swoole_rs_zend::host::callable::cache(callable) };
    // SAFETY: a resolved cache and the two arguments; the coroutine leaf and
    // the reporter run inside C traps.
    let called = unsafe {
        swoole_rs_zend::host::function_call(
            cache,
            u32::try_from(args.len()).unwrap_or(0),
            args.as_mut_ptr(),
            std::ptr::null_mut(),
            coroutine,
            swoole_rs_phpcoroutines::create_coroutine,
            swoole_rs_zend::sys::swr_zend_callable_warning,
        )
    };
    called.map_err(|bailout| {
        let _relayed = bailout::relay_to_c(Err(bailout));
        bailout::Bailout
    })
}

/// The port callback `kind` with `args`: `None` when it is unset, otherwise
/// whether it was called.
/// # Errors
/// It bailed out.
pub(crate) fn call_port(
    server: view::Server<'_>,
    server_fd: c_int,
    kind: c_int,
    args: &mut [zval],
) -> Result<Option<bool>, bailout::Bailout> {
    let callback = php_swoole_server_get_callback(server, server_fd, kind);
    if callback.is_null() {
        return Ok(None);
    }
    call_handler(callback.cast(), args, enable_coroutine(server)).map(Some)
}

/// `http_server_process_request`: the handler with the request's objects.
fn call(mut ctx: Ctx, coroutine: bool) -> Result<(), bailout::Bailout> {
    let state = ctx.state();
    let mut args = [state.request.zobject.value, state.response.zobject.value];
    match call_handler(state.callback.cast(), &mut args, coroutine) {
        Err(bailout) => return Err(bailout),
        Ok(false) => {
            host::notice("Swoole\\Http\\Server->onRequest handler error");
            context::send(ctx, SERVICE_UNAVAILABLE);
            context::close(ctx);
        }
        Ok(true) => {}
    }
    Ok(())
}

/// A queued request's turn (`swoole_event_defer`): run its handler, then
/// release its objects.
/// # Safety
/// `ctx` is a queued server request's live context.
unsafe extern "C-unwind" fn deferred(ctx: *mut c_void) {
    let Some(ctx) = std::ptr::NonNull::new(ctx) else {
        return;
    };
    // SAFETY: per the caller.
    let mut ctx = unsafe { Ctx::of(ctx.cast()) };
    let result = call(ctx, enable_coroutine(context::server(ctx.state())))
        .and_then(|()| context::release_objects(ctx));
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
    server: view::Server<'_>,
    conn: *mut ConnectionRecord,
    mut ctx: Ctx,
    session: i64,
    server_fd: c_int,
    facts: &Receive,
) -> Result<c_int, bailout::Bailout> {
    // The request data `receive` moved into the context, a string; this
    // frame's reference keeps it while the parser fills the context.
    let zdata = ctx.state().request.zdata.copy();
    let data = zdata.bytes().unwrap_or_default();
    // The parser `receive` initialised.
    let parsed = parse_request(ctx.state(), data);
    if ctx.state().parser.error != HPE_OK || ctx.state().completed == 0 {
        context::send(ctx, BAD_REQUEST);
        context::close(ctx);
        let parser = &ctx.state().parser;
        if parser.error == HPE_OK {
            notice("Incomplete HTTP request: parsed successfully but missing required components");
        } else {
            notice(&format!(
                "Invalid HTTP request discarded: {} bytes unprocessed. Reason: {}",
                data.len() - parsed,
                parser.reason()
            ));
        }
        return Ok(DONE);
    }
    let state = ctx.state();
    let cache = facts.cache.filter(|_| state.keepalive != 0);
    addresses(
        server,
        // SAFETY: the request's live connection, read before the hooks run.
        unsafe { &*conn },
        state,
        session,
        facts.local_port,
        facts.remote_port,
        facts.loopback,
        cache,
    );
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (zend_long) cast"
    )]
    let master_time = Owned::long(facts.last_recv_time as zend_long);
    add_server(state, Key::MasterTime, master_time);
    // The hooks may reach the context: no borrow of it is held across them.
    if swoole_rs_corebase::swoole_isset_hook(HOOK_BEFORE_REQUEST) {
        // SAFETY: the hook's context argument.
        unsafe { swoole_rs_corebase::swoole_call_hook(HOOK_BEFORE_REQUEST, ctx.raw()) };
    }
    // Parsing an upgrade request sets the connection's status.
    // SAFETY: the request's live connection.
    let handshake = unsafe { (*conn).websocket_status } == STATUS_CONNECTION;
    let kind = if handshake { ON_HANDSHAKE } else { ON_REQUEST };
    let callback = php_swoole_server_get_callback(server, server_fd, kind);
    if callback.is_null() {
        if handshake {
            websocket::handshake_open(ctx)?;
        } else {
            websocket::request(ctx);
        }
        return Ok(DONE);
    }
    let state = ctx.state();
    state.callback = callback;
    if handshake {
        websocket::upgrading(server, session);
        state.upgrade = 1;
    }
    Ok(REQUEST)
}

/// An HTTP/2 request's connection and server state.
struct Http2Connection {
    last_recv_time: f64,
    local_port: i64,
    remote_port: i64,
    loopback: c_int,
    server_fd: c_int,
    cache: Option<usize>,
    static_handler: bool,
    unavailable: bool,
    coroutine: bool,
}

/// The session's connection (none: closed) and what onRequest reads of it
/// and the server.
fn http2_connection(
    server: view::Server<'_>,
    fd: i64,
) -> Option<(&ConnectionRecord, Http2Connection)> {
    let conn = server.by_session(fd)?;
    let remote_port = i64::from(swr_v1_clients_address_port(&conn.info));
    let loopback = if swr_v1_clients_address_loopback(&conn.info) {
        if conn.info.kind == SOCK_TCP6 { 6 } else { 4 }
    } else {
        0
    };
    let counters = server.counters();
    let facts = Http2Connection {
        last_recv_time: conn.last_recv_time,
        local_port: i64::from(conn.local_port),
        remote_port,
        loopback,
        server_fd: conn.server_fd,
        cache: can_cache_session_addr(server)
            .then(|| usize::try_from(conn.fd).ok())
            .flatten(),
        static_handler: server.config().enable_static_handler,
        unavailable: counters.concurrency.load(SeqCst) >= counters.max_concurrency,
        coroutine: enable_coroutine(server),
    };
    Some((conn, facts))
}

/// `SW_HTTP_VERSION_2`, `SW_HTTP_SERVICE_UNAVAILABLE` and
/// `SW_HTTP2_ERROR_INTERNAL_ERROR`.
const HTTP_VERSION_2: c_int = 3;
const SERVICE_UNAVAILABLE_STATUS: c_int = 503;
const HTTP2_INTERNAL_ERROR: u32 = 2;

/// `http2_server_onRequest`: a finished HTTP/2 request on a server's
/// connection (the session's handler), answered by the static handler or the
/// port's `onRequest`.
/// # Errors
/// The handler bailed out.
pub fn http2_request(mut ctx: Ctx) -> Result<(), bailout::Bailout> {
    let (server, fd) = (context::server(ctx.state()), ctx.state().fd);
    let Some((conn, facts)) = http2_connection(server, fd) else {
        return context::release_objects(ctx);
    };
    ctx.state().request.version = HTTP_VERSION_2;
    let Some(http2) = crate::http2::server() else {
        return context::release_objects(ctx);
    };
    if facts.static_handler && (http2.static_file)(server.raw(), ctx)? {
        return context::release_objects(ctx);
    }
    let state = ctx.state();
    let cache = facts.cache.filter(|_| state.keepalive != 0);
    addresses(
        server,
        conn,
        state,
        fd,
        facts.local_port,
        facts.remote_port,
        facts.loopback,
        cache,
    );
    request_times(state);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (zend_long) cast"
    )]
    let master_time = Owned::long(facts.last_recv_time as zend_long);
    add_server(state, Key::MasterTime, master_time);
    add_protocol(state, Key::Http2);
    state.callback = php_swoole_server_get_callback(server, facts.server_fd, ON_REQUEST);
    if state.before_request != 0 {
        if facts.unavailable {
            state.response.status = SERVICE_UNAVAILABLE_STATUS;
            (http2.respond)(ctx, &[]);
            return context::release_objects(ctx);
        }
        if !context::before_request(ctx) {
            return Ok(());
        }
    }
    let state = ctx.state();
    let mut args = [state.request.zobject.value, state.response.zobject.value];
    match call_handler(state.callback.cast(), &mut args, facts.coroutine) {
        Err(bailout) => return Err(bailout),
        Ok(false) => {
            let stream_id = ctx.state().stream_id;
            (http2.reset)(ctx, stream_id, HTTP2_INTERNAL_ERROR);
            host::notice("Swoole\\Http\\Server->onRequest[v2] handler error");
        }
        Ok(true) => {}
    }
    context::release_objects(ctx)
}

/// `php_swoole_http_server_onClose`: forget the session's addresses, then
/// the server's `onClose`.
pub fn http_server_on_close(server: view::Server<'_>, info: &DataHead) {
    let session = info.fd;
    if let Some(conn) = server.by_session(session)
        && let Ok(fd) = usize::try_from(conn.fd)
    {
        ADDRESSES.with_borrow_mut(|cached| {
            if cached.get(fd).is_some_and(|(owner, _)| *owner == session) {
                cached.remove(fd);
            }
        });
    }
    php_swoole_server_on_close(server, info);
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
    let current = swoole_rs_process::worker_globals::thread_globals();
    // SAFETY: this thread's worker record.
    let current = unsafe { (*current).worker };
    if server.is_null() || current.is_null() {
        return None;
    }
    // SAFETY: `server` is the live server here.
    let server = unsafe { view::Server::from_raw(server) };
    Some(Worker {
        // SAFETY: the current worker's live record.
        concurrency: unsafe { &raw mut (*current).concurrency },
        server_concurrency: &server.counters().concurrency,
        max_concurrency: server.config().worker_max_concurrency,
        shutdown: swoole_rs_process::methods::worker_is_shutdown(current),
    })
}

/// Mark a queued request answered and release its objects, deferring a
/// bailout.
fn abandon(mut ctx: Ctx) {
    ctx.state().end = 1;
    ctx.state().after_response = 0;
    bailout::defer(context::release_objects(ctx));
}

/// `swoole_http_server_onBeforeRequest` after it swaps the hooks: count the
/// request, or queue it past `worker_max_concurrency` (false: not now).
pub(crate) fn admit(ctx: Ctx) -> bool {
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
        let reactor = swoole_rs_reactor::event_api::swr_v1_event_reactor();
        // SAFETY: the thread's live reactor, which begins with its prepared state.
        let fields = unsafe { (*reactor.cast::<swoole_rs_reactor::ReactorPrepared>()).fields };
        // SAFETY: a queued context, alive until its deferred request ends.
        unsafe { swoole_rs_reactor::defer(fields, deferred, ctx.raw().cast(), None) };
    }
}

/// `php_swoole_http_server_rinit`: the uploaded files `is_uploaded_file`
/// and `move_uploaded_file` know.
pub extern "C" fn server_rinit() {
    // SAFETY: RINIT, on the request's thread.
    unsafe { swr_zend_http_uploads_rinit() };
}

/// `php_swoole_http_server_rshutdown`: forget the uploaded files, then drop
/// the cached addresses and the queued requests. A bailout their release
/// deferred is raised last, into the hook's `zend_try`.
pub extern "C" fn server_rshutdown() {
    // SAFETY: RSHUTDOWN, on the request's thread.
    unsafe { swr_zend_http_uploads_rshutdown() };
    let addresses = ADDRESSES
        .with_borrow_mut(|cached| std::mem::replace(cached, ManuallyDrop::new(Table::new())));
    drop(ManuallyDrop::into_inner(addresses));
    while let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
        abandon(ctx);
    }
    bailout::finish();
}

#[cfg(test)]
mod tests {
    use super::*;

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
            cache: None,
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
