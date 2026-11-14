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
    ffi::c_int,
    mem::ManuallyDrop,
    sync::atomic::{AtomicU32, Ordering::SeqCst},
};
use swoole_rs_corebase::{CCast, HookType, swoole_call_hook, swoole_isset_hook};
use swoole_rs_corebase::{DataHead, WebSocketStatus, log};
use swoole_rs_memory::flat::Table;
use swoole_rs_phpserver::PortCallback;
use swoole_rs_phpserver::{
    RecvData, php_swoole_get_recv_data, php_swoole_server_get_callback,
    php_swoole_server_isset_callback, php_swoole_server_on_close, php_swoole_server_on_receive,
};
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_process::worker_record::WorkerType;
use swoole_rs_server::{Connection, DispatchMode, Mode, view};
use swoole_rs_zend::sys::{
    ZEND_ACC_NOT_SERIALIZABLE, swr_zend_http_uploads_rinit, swr_zend_http_uploads_rshutdown,
    zend_long, zval,
};
use swoole_rs_zend::{Class, Owned, ZStr, ZString, bailout};

/// `Swoole\Http\Server`: `Swoole\Server` with no methods of its own.
pub(crate) static SERVER: Class = Class::new();

/// `Swoole\Http\Server`, once MINIT registered it.
#[must_use]
pub fn server_class() -> &'static Class {
    &SERVER
}

/// `php_swoole_http_server_minit`, after `Swoole\Server`'s.
pub fn server_minit(args: &MinitArgs) {
    let minit = args.minit;
    let parent = minit.parent(swoole_rs_phpserver::server_class_ref());
    minit.register_child(
        &SERVER,
        c"Swoole\\Http\\Server",
        swoole_rs_zend::arginfo::Table::NONE,
        parent,
        |entry, _| {
            entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
        },
    );
}

/// `SW_HTTP_SERVICE_UNAVAILABLE_PACKET`.
const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\r\n";
/// `SW_HTTP_BAD_REQUEST_PACKET`.
const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\r\n";
/// llhttp's `HPE_OK`.
const HPE_OK: i32 = 0;
/// `SW_ERROR_SESSION_NOT_EXIST` and `SW_LOG_TRACE`.
const SESSION_NOT_EXIST: c_int = 1005;
const LOG_TRACE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as c_int;

/// `http_server::SW_HTTP_EXT_FLAG_UPLOAD_PREPROCESSED`.
const UPLOAD_PREPROCESSED: u16 = 1;

/// Where `onReceive` sends the data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// The session or its port is gone.
    Closed,
    /// The server's plain `onReceive`.
    Tcp,
    /// The WebSocket `onMessage`.
    WebSocket,
    /// The HTTP/2 server.
    Http2,
    /// A parsed request, for its handler.
    Request,
    /// Handled here.
    Done,
}

/// The connection and its port, as `onReceive` routes by.
struct Receive {
    last_recv_time: f64,
    local_port: i64,
    remote_port: i64,
    loopback: c_int,
    websocket_status: WebSocketStatus,
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

/// `SW_SOCK_TCP6`.
const SOCK_TCP6: c_int = 3;

// The connection's addresses and its peer's port and loopback check.
use swoole_rs_clients::address::{
    swr_v1_clients_address_loopback, swr_v1_clients_address_port, with_host,
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

fn address(server: view::Server<'_>, conn: Connection<'_>, local: bool) -> ZString {
    if local {
        let text = swoole_rs_server::local_addr::get_local_addr(server, conn);
        return ZString::new(text.to_bytes());
    }
    with_host(&conn.info(), |text| ZString::new(text.to_bytes()))
}

/// `swoole_http_server_populate_ip_and_port`: `$request->server`'s ports and
/// addresses. `loopback` is 4 or 6 for a loopback peer, otherwise 0. With
/// `cache`, the connection's fd, a session's addresses are made once and kept
/// until the session closes. `server` and `conn` are the live server and the
/// request's connection; `state` the request's context.
#[expect(clippy::too_many_arguments, reason = "upstream's request facts")]
fn addresses(
    server: view::Server<'_>,
    conn: Connection<'_>,
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
    conn: Connection<'_>,
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
pub fn http_server_on_receive(server: view::Server<'_>, request: &RecvData) -> bool {
    let info = request.info();
    match receive(server, request) {
        Route::Closed => false,
        Route::Tcp => {
            php_swoole_server_on_receive(server, request);
            true
        }
        Route::WebSocket => websocket::on_message(server, request),
        Route::Http2 => {
            // `receive` verified the session.
            let (Some(conn), Some(http2)) = (server.verified(info.fd), crate::http2::server())
            else {
                return false;
            };
            (http2.on_receive)(server, conn, request)
        }
        Route::Request | Route::Done => true,
    }
}

fn receive(server: view::Server<'_>, request: &RecvData) -> Route {
    let info = request.info();
    let (session, server_fd) = (info.fd, c_int::from(info.server_fd));
    let Some(conn) = server.verified(session) else {
        host::log(
            LOG_TRACE,
            SESSION_NOT_EXIST,
            c"php_swoole_http_server_onReceive",
            format!("session[{session}] is closed"),
        );
        return Route::Closed;
    };
    // A live connection's listening port.
    let Some(port) = server.port_by_server_fd(server_fd) else {
        return Route::Closed;
    };
    let config = port.config();
    let isset = |kind| php_swoole_server_isset_callback(server, Some(port), kind);
    // Only whether either is set routes the data, so onMessage is asked only without onRequest.
    let http = config.open_http_protocol.get() && isset(PortCallback::Request);
    let websocket = !http && config.open_websocket_protocol.get() && isset(PortCallback::Message);
    let mut facts = Receive {
        last_recv_time: conn.last_recv_time(),
        local_port: i64::from(conn.local_port()),
        remote_port: 0,
        loopback: 0,
        websocket_status: conn.websocket_status(),
        http,
        websocket,
        http2: conn.http2_stream(),
        cache: None,
        coroutine: false,
    };
    let route = route(&facts);
    if route != Route::Request {
        return route;
    }
    facts.remote_port = i64::from(swr_v1_clients_address_port(&conn.info()));
    if swr_v1_clients_address_loopback(&conn.info()) {
        facts.loopback = if conn.info().kind == SOCK_TCP6 { 6 } else { 4 };
    }
    facts.cache = can_cache_session_addr(server)
        .then(|| usize::try_from(conn.fd()).ok())
        .flatten();
    facts.coroutine = enable_coroutine(server);
    let mut ctx = context::context_new(session);
    context::init(ctx.state(), server);
    let state = ctx.state();
    state.before_request = true;
    state.upload_preprocessed = info.ext_flags & UPLOAD_PREPROCESSED != 0;
    // The data moves into the request's own zval.
    state.request.zdata = php_swoole_get_recv_data(server, request);
    crate::parse::init_request_parser(ctx);
    let result = handle(server, ctx, session, server_fd, &facts).and_then(|route| {
        if route != Route::Request {
            return context::release_objects(ctx);
        }
        // False leaves it queued, with its objects.
        if !context::before_request(ctx) {
            return Ok(());
        }
        call(ctx, facts.coroutine)
    });
    bailout::raise(result);
    Route::Done
}

/// `swoole_http_server_can_cache_session_addr`: a keep-alive session's
/// addresses stay cached until its close, so only when that close reaches
/// the same worker.
#[must_use]
pub fn can_cache_session_addr(server: view::Server<'_>) -> bool {
    let config = server.config();
    config.mode_ == Mode::Base
        || (config.mode_ == Mode::Process && config.dispatch() == Some(DispatchMode::FdMod))
}

/// `Server::is_enable_coroutine`.
fn enable_coroutine(server: view::Server<'_>) -> bool {
    let config = server.config();
    match swoole_rs_process::methods::worker_type() {
        Some(WorkerType::Task) => config.task_enable_coroutine.get(),
        Some(WorkerType::Manager) => false,
        _ => config.enable_coroutine.get(),
    }
}

pub(crate) use swoole_rs_zend::host::{NO_THEN, Then, callable::Callable};

/// `zend::function::call(callable, args.len(), args, nullptr, coroutine)`:
/// whether the handler was called; in a coroutine, then `then`
/// (`swoole_rs_zend::host::Then`).
/// # Errors
/// It bailed out.
pub(crate) fn call_handler(
    callable: &Callable,
    args: &mut [zval],
    coroutine: bool,
    then: Then,
) -> Result<bool, bailout::Bailout> {
    // SAFETY: the resolved cache of a callable the caller holds through the
    // call, and the arguments; the coroutine leaf, `then` and the reporter run
    // inside C traps.
    let called = unsafe {
        swoole_rs_zend::host::function_call(
            callable.cache_ptr(),
            u32::try_from(args.len()).unwrap_or(0),
            args.as_mut_ptr(),
            std::ptr::null_mut(),
            coroutine,
            swoole_rs_phpcoroutines::create_coroutine,
            then,
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
    kind: PortCallback,
    args: &mut [zval],
) -> Result<Option<bool>, bailout::Bailout> {
    let Some(callback) = php_swoole_server_get_callback(server, server_fd, kind) else {
        return Ok(None);
    };
    call_handler(&callback, args, enable_coroutine(server), NO_THEN).map(Some)
}

/// `http_server_process_request`: the handler with the request's objects,
/// then the context's references to them released; in a coroutine, inside
/// the call's own trap.
fn call(mut ctx: Ctx, coroutine: bool) -> Result<(), bailout::Bailout> {
    let state = ctx.state();
    // Its own reference: the handler may replace the server's.
    let callback = state.callback.clone();
    let mut args = [
        state.request.zobject.copy_value(),
        state.response.zobject.copy_value(),
    ];
    let mut release = context::Release::new(ctx);
    let called = callback.map_or(Ok(false), |callback| {
        call_handler(&callback, &mut args, coroutine, release.then())
    });
    match called {
        // The objects stay held, as upstream's jump skipped their release.
        Err(bailout) if !release.begun() => return Err(bailout),
        Err(bailout) => bailout.defer(),
        Ok(false) => {
            host::notice("Swoole\\Http\\Server->onRequest handler error");
            context::send(ctx, SERVICE_UNAVAILABLE);
            context::close(ctx);
        }
        Ok(true) => {}
    }
    release.finish()
}

/// A queued request's turn (`swoole_event_defer`): run its handler, then
/// release its objects. The context is a queued server request's, alive until
/// its deferred request ends.
struct Turn(Ctx);

impl swoole_rs_reactor::event_api::Deferred for Turn {
    fn run(self) {
        let mut ctx = self.0;
        bailout::raise(call(ctx, enable_coroutine(context::server(ctx.state()))));
    }
    fn dispose(self) {}
}

/// Where `onReceive` sends data before any parsing.
fn route(facts: &Receive) -> Route {
    if !facts.http && !facts.websocket {
        Route::Tcp
    } else if facts.websocket_status >= WebSocketStatus::Handshake {
        Route::WebSocket
    } else if facts.http2 {
        Route::Http2
    } else {
        Route::Request
    }
}

fn notice(message: &str) {
    log::notice(format_args!("{message}"));
}

/// Parse the request, fill `$request->server` and pick the handler. The
/// session's connection is looked up at each use: parsing an upgrade and the
/// hooks write it.
fn handle(
    server: view::Server<'_>,
    mut ctx: Ctx,
    session: i64,
    server_fd: c_int,
    facts: &Receive,
) -> Result<Route, bailout::Bailout> {
    // The request data `receive` moved into the context, a string; this
    // frame's reference keeps it while the parser fills the context.
    let zdata = ctx.state().request.zdata.copy();
    let data = zdata.bytes().unwrap_or_default();
    // The parser `receive` initialised.
    let parsed = parse_request(ctx.state(), data);
    if ctx.state().parser.error != HPE_OK || !ctx.state().completed {
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
        return Ok(Route::Done);
    }
    let state = ctx.state();
    let cache = facts.cache.filter(|_| state.keepalive);
    if let Some(conn) = server.by_session(session) {
        addresses(
            server,
            conn,
            state,
            session,
            facts.local_port,
            facts.remote_port,
            facts.loopback,
            cache,
        );
    }
    let master_time = Owned::long(facts.last_recv_time.c_cast::<zend_long>());
    add_server(state, Key::MasterTime, master_time);
    // The hooks may reach the context: no borrow of it is held across them.
    if swoole_isset_hook(HookType::BEFORE_REQUEST) {
        // SAFETY: the hook's context argument.
        unsafe { swoole_call_hook(HookType::BEFORE_REQUEST, ctx.raw()) };
    }
    // Parsing an upgrade request sets the connection's status.
    let handshake = server
        .by_session(session)
        .is_some_and(|conn| conn.websocket_status() == WebSocketStatus::Connection);
    let kind = if handshake {
        PortCallback::Handshake
    } else {
        PortCallback::Request
    };
    let Some(callback) = php_swoole_server_get_callback(server, server_fd, kind) else {
        if handshake {
            websocket::handshake_open(ctx)?;
        } else {
            websocket::request(ctx);
        }
        return Ok(Route::Done);
    };
    let state = ctx.state();
    state.callback = Some(callback);
    if handshake {
        websocket::upgrading(server, session);
        state.upgrade = true;
    }
    Ok(Route::Request)
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
) -> Option<(Connection<'_>, Http2Connection)> {
    let conn = server.by_session(fd)?;
    let remote_port = i64::from(swr_v1_clients_address_port(&conn.info()));
    let loopback = if swr_v1_clients_address_loopback(&conn.info()) {
        if conn.info().kind == SOCK_TCP6 { 6 } else { 4 }
    } else {
        0
    };
    let counters = server.counters();
    let facts = Http2Connection {
        last_recv_time: conn.last_recv_time(),
        local_port: i64::from(conn.local_port()),
        remote_port,
        loopback,
        server_fd: conn.server_fd(),
        cache: can_cache_session_addr(server)
            .then(|| usize::try_from(conn.fd()).ok())
            .flatten(),
        static_handler: server.config().enable_static_handler.get(),
        unavailable: counters.concurrency.load(SeqCst) >= counters.max_concurrency.load(SeqCst),
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
    if facts.static_handler && (http2.static_file)(server, ctx)? {
        return context::release_objects(ctx);
    }
    let state = ctx.state();
    let cache = facts.cache.filter(|_| state.keepalive);
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
    let master_time = Owned::long(facts.last_recv_time.c_cast::<zend_long>());
    add_server(state, Key::MasterTime, master_time);
    add_protocol(state, Key::Http2);
    state.callback = php_swoole_server_get_callback(server, facts.server_fd, PortCallback::Request);
    if state.before_request {
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
    let callback = state.callback.clone();
    let mut args = [
        state.request.zobject.copy_value(),
        state.response.zobject.copy_value(),
    ];
    let mut release = context::Release::new(ctx);
    let called = callback.map_or(Ok(false), |callback| {
        call_handler(&callback, &mut args, facts.coroutine, release.then())
    });
    match called {
        // As in `call`.
        Err(bailout) if !release.begun() => return Err(bailout),
        Err(bailout) => bailout.defer(),
        Ok(false) => {
            let stream_id = ctx.state().stream_id;
            (http2.reset)(ctx, stream_id, HTTP2_INTERNAL_ERROR);
            host::notice("Swoole\\Http\\Server->onRequest[v2] handler error");
        }
        Ok(true) => {}
    }
    release.finish()
}

/// `php_swoole_http_server_onClose`: forget the session's addresses, then
/// the server's `onClose`.
pub fn http_server_on_close(server: view::Server<'_>, info: &DataHead) {
    let session = info.fd;
    if let Some(conn) = server.by_session(session)
        && let Ok(fd) = usize::try_from(conn.fd())
    {
        ADDRESSES.with_borrow_mut(|cached| {
            if cached.get(fd).is_some_and(|(owner, _)| *owner == session) {
                cached.remove(fd);
            }
        });
    }
    php_swoole_server_on_close(server, info);
}

/// The counters of the worker serving requests, for one admission or release.
struct Worker {
    /// `Worker::concurrency`, which the master resets as it retires the worker.
    concurrency: &'static AtomicU32,
    /// `ServerGS::concurrency`, shared by every worker.
    server_concurrency: &'static AtomicU32,
    max_concurrency: u32,
    shutdown: bool,
}

/// `sw_server()` and `sw_worker()`'s counters; none when this process is no
/// server worker.
fn worker() -> Option<Worker> {
    // SAFETY: a worker's server, which outlives the worker's requests.
    let server = unsafe { view::Server::<'static>::current() }?;
    let current = swoole_rs_process::worker_globals::current_worker();
    if current.is_null() {
        return None;
    }
    Some(Worker {
        // SAFETY: the current worker's live record; the borrow covers only its
        // atomic counter.
        concurrency: unsafe { &(*current).concurrency },
        server_concurrency: &server.counters().concurrency,
        max_concurrency: server.config().worker_max_concurrency.get(),
        shutdown: swoole_rs_process::methods::worker_is_shutdown(),
    })
}

/// Mark a queued request answered and release its objects, deferring a
/// bailout.
fn abandon(mut ctx: Ctx) {
    ctx.state().end = true;
    ctx.state().after_response = false;
    bailout::defer(context::release_objects(ctx));
}

/// `swoole_http_server_onBeforeRequest` after it swaps the hooks: count the
/// request, or queue it past `worker_max_concurrency` (false: not now).
pub(crate) fn admit(ctx: Ctx) -> bool {
    let Some(worker) = worker().filter(|worker| !worker.shutdown) else {
        return false;
    };
    worker.server_concurrency.fetch_add(1, SeqCst);
    if worker.concurrency.fetch_add(1, SeqCst) >= worker.max_concurrency {
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
    worker.concurrency.fetch_sub(1, SeqCst);
    if let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
        // Upstream dereferences `sw_reactor()` here unchecked.
        if swoole_rs_reactor::event_api::swoole_event_defer_value(Turn(ctx)).is_err() {
            swoole_rs_corebase::fatal("no reactor to defer the queued request to");
        }
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

    fn facts(http: bool, websocket: bool, status: WebSocketStatus, http2: bool) -> Receive {
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
        assert_eq!(
            route(&facts(false, false, WebSocketStatus::Handshake, true)),
            Route::Tcp
        );
        assert_eq!(
            route(&facts(true, false, WebSocketStatus::Handshake, true)),
            Route::WebSocket
        );
        assert_eq!(
            route(&facts(false, true, WebSocketStatus::Active, false)),
            Route::WebSocket
        );
        assert_eq!(
            route(&facts(true, false, WebSocketStatus::Connection, true)),
            Route::Http2
        );
        assert_eq!(
            route(&facts(true, true, WebSocketStatus::Connection, false)),
            Route::Request
        );
        assert_eq!(
            route(&facts(true, false, WebSocketStatus::None, false)),
            Route::Request
        );
    }
}
