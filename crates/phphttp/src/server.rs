//! `Swoole\Http\Server` from upstream's `swoole_http_server.cc`: `onReceive`'s
//! routing and request parsing, the addresses cached for keep-alive sessions
//! until they close, and the requests queued past `worker_max_concurrency`.
//! The server, its connections and the worker stay C++ (`swr_http_host_*`),
//! and so does the handler's call (no Rust frame under PHP code).
use crate::{
    host::{self, Context},
    parse::{Key, key, swr_v1_phphttp_parse},
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
    /// The port speaks WebSocket and has `onMessage`.
    websocket: bool,
    http2: bool,
    /// The session's addresses may be cached.
    cache: bool,
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
    /// `swoole_event_defer`: run the queued request's `onRequest`, then
    /// release its objects.
    fn swr_http_host_defer(context: *mut Context);
    /// `ctx->onAfterResponse = nullptr`.
    fn swr_http_host_unhook(context: *mut Context);
    fn swr_http_host_send(context: *mut Context, data: *const c_char, length: usize) -> bool;
    fn swr_http_host_close(context: *mut Context) -> bool;
    /// `get_connection_verify_no_ssl` and its port's facts (null: closed).
    fn swr_http_host_receive(
        server: *mut c_void,
        session: i64,
        server_fd: c_int,
        out: *mut Receive,
    ) -> *mut c_void;
    /// A new context for the server's request, holding its data (`zdata`).
    fn swr_http_host_receive_context(
        server: *mut c_void,
        session: i64,
        request: *mut c_void,
        zdata: *mut *mut zval,
    ) -> *mut Context;
    /// `PHP_SWOOLE_HOOK_BEFORE_REQUEST`, when one is set.
    fn swr_http_host_before_request(context: *mut Context);
    /// The port's `onHandshake` (`handshake`) or `onRequest`, or null.
    fn swr_http_host_callback(
        server: *mut c_void,
        server_fd: c_int,
        handshake: bool,
    ) -> *mut c_void;
    /// `ctx->private_data_2 = callback`.
    fn swr_http_host_set_callback(context: *mut Context, callback: *mut c_void);
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

fn add(table: *mut HashTable, name: Key, string: *mut zend_string) {
    let mut value = new_zval();
    // SAFETY: a string the table takes over (or an interned one).
    unsafe { swr_zval_str(&raw mut value, string) };
    // SAFETY: the request's live `server` table and an interned key.
    unsafe { zend_hash_add_new(table, key(name), &raw mut value) };
}

fn add_long(table: *mut HashTable, name: Key, long: zend_long) {
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

/// `php_swoole_http_server_onReceive` up to the handler's call: route the
/// data to another protocol's `onReceive`, or parse it into a new context
/// (`*context`) and pick its handler. `REQUEST` leaves the call to the
/// trampoline; after `DONE` it only releases the context's objects. A
/// bailout (a handshake's `onOpen`) is raised last, over this frame only.
/// # Safety
/// `server` is the live server and `request` its `RecvData`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_receive(
    server: *mut c_void,
    request: *mut c_void,
    session: i64,
    server_fd: c_int,
    context: *mut *mut Context,
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
    // SAFETY: as above; the request's data moves into the context.
    let ctx = unsafe { swr_http_host_receive_context(server, session, request, &raw mut zdata) };
    // SAFETY: per the caller.
    unsafe { *context = ctx };
    let result = handle(server, conn, ctx, zdata, session, server_fd, &facts);
    let route = *result.as_ref().unwrap_or(&DONE);
    bailout::raise(result.map(|_| ()));
    route
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
        // SAFETY: a live context.
        unsafe { swr_http_host_send(ctx, BAD_REQUEST.as_ptr().cast(), BAD_REQUEST.len()) };
        // SAFETY: as above.
        unsafe { swr_http_host_close(ctx) };
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
    // SAFETY: a live context.
    unsafe { swr_http_host_before_request(ctx) };
    let handshake = facts.websocket_status == STATUS_CONNECTION;
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
    if handshake {
        websocket::upgrading(server, session);
        state.upgrade = 1;
    }
    // SAFETY: a live context and the port's callback.
    unsafe { swr_http_host_set_callback(ctx, callback) };
    Ok(REQUEST)
}

fn release([local, remote]: [*mut zend_string; 2]) {
    // SAFETY: the cache's references.
    unsafe { swr_zend_string_release(local) };
    // SAFETY: as above.
    unsafe { swr_zend_string_release(remote) };
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
    // SAFETY: a queued context, whose `onAfterResponse` C++ owns.
    unsafe { swr_http_host_unhook(ctx) };
    // SAFETY: the queued request's own references to its objects.
    unsafe { zval_ptr_dtor(state(ctx).request.zobject.ptr) };
    // SAFETY: as above.
    unsafe { zval_ptr_dtor(state(ctx).response.zobject.ptr) };
}

/// `swoole_http_server_onBeforeRequest` after it swaps the hooks: count the
/// request, or queue it past `worker_max_concurrency` (false: not now).
/// # Safety
/// `ctx` is a live server request's context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_server_admit(ctx: *mut Context) -> bool {
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_server_release() {
    let Some(worker) = worker() else {
        return;
    };
    if worker.shutdown {
        while let Some(ctx) = QUEUED.with_borrow_mut(VecDeque::pop_front) {
            // SAFETY: a queued context.
            unsafe {
                swr_http_host_send(
                    ctx,
                    SERVICE_UNAVAILABLE.as_ptr().cast(),
                    SERVICE_UNAVAILABLE.len(),
                )
            };
            // SAFETY: as above.
            unsafe { swr_http_host_close(ctx) };
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
