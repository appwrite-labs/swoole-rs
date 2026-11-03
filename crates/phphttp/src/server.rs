//! `Swoole\Http\Server`'s per-worker state from upstream's
//! `swoole_http_server.cc`: the addresses cached for keep-alive sessions until
//! they close, and the requests queued past `worker_max_concurrency`. The
//! server, its connections and the worker stay C++ (`swr_http_host_*`).
use crate::{
    host::Context,
    parse::{Key, key},
    send::state,
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{CStr, c_char, c_int, c_void},
    sync::atomic::{AtomicU32, Ordering::SeqCst},
};
use swoole_rs_zend::sys::{
    HashTable, swr_zend_string_copy, swr_zend_string_init, swr_zend_string_release, swr_zval_str,
    zend_hash_add_new, zend_long, zend_string, zval, zval_ptr_dtor,
};

/// `SW_HTTP_SERVICE_UNAVAILABLE_PACKET`.
const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\r\n";

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
}
