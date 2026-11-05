//! `Swoole\Http\Server` from upstream's `swoole_http_server.cc`: `onReceive`'s
//! routing and request parsing, the addresses cached for keep-alive sessions
//! until they close, and the requests queued past `worker_max_concurrency`.
//! The server, its connections and the worker stay C++ (`swr_http_host_*`),
//! and so does the handler's call (no Rust frame under PHP code).
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
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicU32, Ordering::SeqCst},
};
use swoole_rs_zend::sys::{
    HashTable, swr_zend_string_copy, swr_zend_string_init, swr_zend_string_release, swr_zval_str,
    zend_hash_add_new, zend_long, zend_string, zval, zval_ptr_dtor,
};
use swoole_rs_zend::{bailout, string, value};

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

/// What `swr_v1_phphttp_server_receive` tells its trampoline to do
/// (`SWR_PHPHTTP_RECEIVE_*`).
const CLOSED: c_int = 0;
const TCP: c_int = 1;
const WEBSOCKET: c_int = 2;
const HTTP2: c_int = 3;
const REQUEST: c_int = 4;
const DONE: c_int = 5;

/// `swr_http_receive`: the connection and its port, as `onReceive` routes by.
#[repr(C)]
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

/// `swr_http_worker`: the counters of the worker serving requests.
#[repr(C)]
struct Worker {
    /// `Worker::concurrency`.
    concurrency: *mut u32,
    /// `ServerGS::concurrency`, shared by every worker.
    server_concurrency: *mut u32,
    max_concurrency: u32,
    shutdown: bool,
}

unsafe extern "C" {
    /// `Server::get_local_addr` (`local`) or `get_remote_addr`.
    fn swr_http_host_address(server: *mut c_void, conn: *mut c_void, local: bool) -> *const c_char;
    /// False when this process is no server worker.
    fn swr_http_host_worker(worker: *mut Worker) -> bool;
    /// `swoole_event_defer` of `swr_v1_phphttp_server_deferred`.
    fn swr_http_host_defer(context: *mut Context);
    /// `get_connection_verify_no_ssl` and the facts `route` reads (null: closed).
    fn swr_http_host_receive(
        server: *mut c_void,
        session: i64,
        server_fd: c_int,
        out: *mut Receive,
    ) -> *mut c_void;
    /// A new context for the server's request, holding its data (`zdata`), and the
    /// rest of the connection's facts.
    fn swr_http_host_receive_context(
        server: *mut c_void,
        conn: *mut c_void,
        session: i64,
        request: *mut c_void,
        zdata: *mut *mut zval,
        out: *mut Receive,
    ) -> *mut Context;
    /// `PHP_SWOOLE_HOOK_BEFORE_REQUEST`, when one is set, then the connection's
    /// `websocket_status` (parsing an upgrade request sets it).
    fn swr_http_host_before_request(context: *mut Context, conn: *mut c_void) -> u8;
    /// The port's `onHandshake` (`handshake`) or `onRequest`, or null.
    fn swr_http_host_callback(
        server: *mut c_void,
        server_fd: c_int,
        handshake: bool,
    ) -> *mut c_void;
    /// `swoole_notice("%s", message)`.
    fn swr_http_host_notice(message: *const c_char);
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

fn address(server: *mut c_void, conn: *mut c_void, local: bool) -> *mut zend_string {
    // SAFETY: the live server and connection the caller passed.
    let text = unsafe { swr_http_host_address(server, conn, local) };
    // SAFETY: the server's NUL-terminated address text.
    let text = unsafe { CStr::from_ptr(text) };
    // SAFETY: a byte span of the address text.
    unsafe { swr_zend_string_init(text.as_ptr(), text.to_bytes().len()) }
}

/// `swoole_http_server_populate_ip_and_port`: `$request->server`'s ports and
/// addresses. `loopback` is 4 or 6 for a loopback peer, otherwise 0. With
/// `cache`, a session's addresses are made once and kept until
/// `swr_v1_phphttp_server_close`.
/// # Safety
/// `server` and `conn` are the live server and the request's connection;
/// `table` is the request's `server` array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_addresses(
    server: *mut c_void,
    conn: *mut c_void,
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
/// bailout (a handshake's `onOpen`, the handler's `exit()`) is raised last,
/// over this frame only.
/// # Safety
/// `server` is the live server and `request` its `RecvData`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_receive(
    server: *mut c_void,
    request: *mut c_void,
    session: i64,
    server_fd: c_int,
) -> c_int {
    let mut facts = Receive {
        last_recv_time: 0.0,
        local_port: 0,
        remote_port: 0,
        loopback: 0,
        websocket_status: 0,
        http: false,
        websocket: false,
        http2: false,
        cache: false,
        coroutine: false,
    };
    // SAFETY: the live server; fills a local struct.
    let conn = unsafe { swr_http_host_receive(server, session, server_fd, &raw mut facts) };
    if conn.is_null() {
        host::log(
            LOG_TRACE,
            SESSION_NOT_EXIST,
            c"php_swoole_http_server_onReceive",
            format!("session[{session}] is closed"),
        );
        return CLOSED;
    }
    let route = route(&facts);
    if route != REQUEST {
        return route;
    }
    let mut zdata = ptr::null_mut();
    // SAFETY: as above, with the live connection; the request's data moves into the context.
    let ctx = unsafe {
        swr_http_host_receive_context(
            server,
            conn,
            session,
            request,
            &raw mut zdata,
            &raw mut facts,
        )
    };
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

/// A queued request's turn (`swr_http_host_defer`): run its handler, then
/// release its objects.
/// # Safety
/// `ctx` is a queued server request's live context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_deferred(ctx: *mut Context, coroutine: bool) {
    let result = call(ctx, coroutine);
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
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: a NUL-terminated message, printed through "%s".
    unsafe { swr_http_host_notice(message.as_ptr()) };
}

/// Parse the request, fill `$request->server` and pick the handler.
fn handle(
    server: *mut c_void,
    conn: *mut c_void,
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
        swr_v1_phphttp_server_addresses(
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
    // SAFETY: a live context and its connection.
    let handshake = unsafe { swr_http_host_before_request(ctx, conn) } == STATUS_CONNECTION;
    // SAFETY: the live server.
    let callback = unsafe { swr_http_host_callback(server, server_fd, handshake) };
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
    fn swr_h2s_connection(server: *mut c_void, fd: i64, out: *mut Http2Connection) -> *mut c_void;
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
        swr_v1_phphttp_server_addresses(
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
    state.callback = unsafe { swr_http_host_callback(server, facts.server_fd, false) };
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

/// `php_swoole_http_server_onClose`: forget the session's addresses.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_server_close(session: i64) {
    if let Some(addresses) = ADDRESSES.with_borrow_mut(|cached| cached.remove(&session)) {
        release(addresses);
    }
}

fn worker() -> Option<Worker> {
    let mut worker = Worker {
        concurrency: std::ptr::null_mut(),
        server_concurrency: std::ptr::null_mut(),
        max_concurrency: 0,
        shutdown: false,
    };
    // SAFETY: fills a local struct.
    unsafe { swr_http_host_worker(&raw mut worker) }.then_some(worker)
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
    // SAFETY: `ServerGS::concurrency`, in shared memory and only touched atomically.
    unsafe { AtomicU32::from_ptr(worker.server_concurrency) }.fetch_add(1, SeqCst);
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
    // SAFETY: as in `swr_v1_phphttp_server_admit`.
    unsafe { AtomicU32::from_ptr(worker.server_concurrency) }.fetch_sub(1, SeqCst);
    // SAFETY: this worker's own counter.
    let concurrency = unsafe { &mut *worker.concurrency };
    *concurrency = concurrency.wrapping_sub(1);
    if let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
        // SAFETY: a queued context, alive until its deferred request ends.
        unsafe { swr_http_host_defer(ctx) };
    }
}

/// `php_swoole_http_server_rshutdown`: drop the cached addresses and the
/// queued requests.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_server_rshutdown() {
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
    fn worker_matches_the_c_struct() {
        // `struct swr_http_worker { uint32_t *, uint32_t *, uint32_t, bool }`
        assert_eq!(size_of::<Worker>(), 24);
        assert_eq!(std::mem::offset_of!(Worker, max_concurrency), 16);
        assert_eq!(std::mem::offset_of!(Worker, shutdown), 20);
    }

    #[test]
    fn http2_connection_matches_the_c_struct() {
        // `swr_h2s_connection_facts`, which C++ asserts is 40 bytes.
        assert_eq!(size_of::<Http2Connection>(), 40);
        assert_eq!(std::mem::offset_of!(Http2Connection, server_fd), 28);
        assert_eq!(std::mem::offset_of!(Http2Connection, coroutine), 35);
        assert_eq!(size_of::<Http2Request>(), 24);
    }

    #[test]
    fn receive_matches_the_c_struct() {
        // `swr_http_receive`, which C++ asserts is 40 bytes.
        assert_eq!(size_of::<Receive>(), 40);
        assert_eq!(std::mem::offset_of!(Receive, loopback), 24);
        assert_eq!(std::mem::offset_of!(Receive, websocket_status), 28);
        assert_eq!(std::mem::offset_of!(Receive, cache), 32);
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
