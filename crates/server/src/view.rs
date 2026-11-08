//! A native `Server`'s Rust-owned records, read straight from its pointer by
//! the crates that bind it (phpserver), without a native call.
//!
//! `Server` has no virtual methods and `detail::ServerFields` is its first base,
//! so a `Server *` points at `ServerFields { state_, config_, tables_ }`
//! (`native/include/swoole_rs_server_view.h`). `ListenPort` likewise starts
//! with `detail::PortFields { port_config_ }`. `ServerGS` puts the task and
//! event worker pools right after the shared counters.
use crate::callbacks::CallbackSlot;
use crate::port_config::PortConfig;
use crate::{ConnectionRecord, PortCounters, ServerConfig, ServerCounters, ServerState};
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering;
use std::ffi::CString;
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::server_workers::{ServerWorkers, swr_v1_process_server_worker};
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_reactor::SocketRecord;

/// `ServerFields`, the head of every `Server`.
#[repr(C)]
struct Fields {
    state: *const ServerState,
    config: *mut ServerConfig,
}

/// # Safety
/// `serv` is a live `Server`.
unsafe fn fields<'a>(serv: *mut c_void) -> &'a Fields {
    // SAFETY: the head of a live Server, per the module comment.
    unsafe { &*serv.cast::<Fields>() }
}

/// The server's Rust-owned state, as a pointer.
///
/// # Safety
/// `serv` is a live `Server`.
pub(crate) unsafe fn state_ptr(serv: *mut c_void) -> *const ServerState {
    // SAFETY: per the caller.
    unsafe { fields(serv) }.state
}

/// The server's Rust-owned state.
///
/// # Safety
/// `serv` is a live `Server`.
unsafe fn state<'a>(serv: *mut c_void) -> &'a ServerState {
    // SAFETY: per the caller.
    let state = unsafe { fields(serv) }.state;
    // SAFETY: the state lives as long as the server.
    unsafe { &*state }
}

/// `serv->onX = f`: callback slot `index` (`callbacks::CB_*`) now calls `invoke`, a
/// Rust `extern "C"` fn taking a null `ctx` and then the member's own arguments.
///
/// # Safety
/// `serv` is a live `Server` whose slot no other thread reads meanwhile, and
/// `invoke` has the slot's signature.
pub unsafe fn set_callback(serv: *mut c_void, index: u32, invoke: *const c_void) {
    // SAFETY: per the caller.
    if let Some(slot) = unsafe { state(serv) }.callbacks.slot(index) {
        slot.set(invoke);
    }
}

/// `serv->onX != nullptr` of callback slot `index`.
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe fn has_callback(serv: *mut c_void, index: u32) -> bool {
    // SAFETY: per the caller.
    unsafe { state(serv) }
        .callbacks
        .slot(index)
        .is_some_and(CallbackSlot::is_set)
}

/// The server's configuration.
///
/// # Safety
/// `serv` is a live `Server`; native code and other threads write the fields
/// returned only under the server's own synchronization.
pub unsafe fn config<'a>(serv: *mut c_void) -> &'a mut ServerConfig {
    // SAFETY: per the caller.
    let config = unsafe { fields(serv) }.config;
    // SAFETY: the config allocation lives as long as the server.
    unsafe { &mut *config }
}

/// `upload_tmp_dir = dir`.
///
/// # Safety
/// `serv` is a live `Server`, not started.
pub unsafe fn set_upload_tmp_dir(serv: *mut c_void, dir: &[u8]) {
    // SAFETY: per the caller.
    unsafe { state(serv) }.set_upload_tmp_dir(dir);
}

/// `url_rewrite_rules`: add one rule.
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe fn add_rewrite_rule(serv: *mut c_void, pattern: &[u8], replacement: &[u8]) {
    // SAFETY: per the caller.
    crate::static_config::add_rewrite_rule(unsafe { state(serv) }, pattern, replacement);
}

/// The static handler's collection `kind` (0 locations, 1 index files): whether
/// it is set, and a copy of its values.
///
/// # Safety
/// `serv` is a live `Server`.
#[must_use]
pub unsafe fn static_strings(serv: *mut c_void, kind: u32) -> (bool, Vec<Vec<u8>>) {
    // SAFETY: per the caller.
    crate::static_config::strings(unsafe { state(serv) }, kind)
}

/// `apply_rewrite_rules` on a static context: true when a rule applied.
///
/// # Safety
/// `serv` is a live `Server` and `context` a live context no one else uses meanwhile.
pub unsafe fn static_rewrite(
    serv: *mut c_void,
    context: *mut swoole_rs_http::HttpStaticContext,
) -> bool {
    // SAFETY: per the caller.
    let state = unsafe { state(serv) };
    // SAFETY: the live state and the caller's context.
    unsafe { crate::swr_v1_server_static_rewrite(state, context) }
}

/// `document_root`, copied.
///
/// # Safety
/// `serv` is a live `Server`.
#[must_use]
pub unsafe fn document_root(serv: *mut c_void) -> Vec<u8> {
    // SAFETY: per the caller.
    unsafe { state(serv) }
        .text(crate::state::TEXT_DOCUMENT_ROOT)
        .to_vec()
}

/// `upload_tmp_dir.c_str()`: the directory up to its first NUL.
///
/// # Safety
/// `serv` is a live `Server`.
#[must_use]
pub unsafe fn upload_tmp_dir(serv: *mut c_void) -> CString {
    // SAFETY: per the caller.
    let dir = unsafe { state(serv) }.upload_tmp_dir();
    let dir = dir.split(|&byte| byte == 0).next().unwrap_or_default();
    CString::new(dir).unwrap_or_default()
}

/// `http_compression_types` lists `kind`; `None` asks whether it lists any.
///
/// # Safety
/// `serv` is a live `Server`.
#[must_use]
pub unsafe fn compression_type(serv: *mut c_void, kind: Option<&[u8]>) -> bool {
    // SAFETY: per the caller.
    let state = unsafe { fields(serv) }.state;
    let op = |operation, data: &[u8]| {
        // SAFETY: the live state and a byte span (non-null even when empty).
        unsafe {
            crate::swr_v1_server_static_config(state, 2, operation, data.as_ptr(), data.len())
        }
    };
    let types = op(1, &[]) != 0;
    match kind {
        Some(kind) if types => op(5, kind) != op(0, &[]),
        _ => types,
    }
}

/// The shared counters (`serv->gs`).
///
/// # Safety
/// As [`config`], once the server has its shared memory.
pub unsafe fn counters<'a>(serv: *mut c_void) -> &'a ServerCounters {
    // SAFETY: per the caller.
    let gs = unsafe { config(serv) }.gs.cast::<ServerCounters>();
    // SAFETY: `ServerGS` starts with the counters, mapped for the server's life.
    unsafe { &*gs }
}

/// The shared counters, for the settings `Server::set()` writes before the
/// server starts.
///
/// # Safety
/// As [`counters`], with no concurrent reader of the fields written.
pub unsafe fn counters_mut<'a>(serv: *mut c_void) -> &'a mut ServerCounters {
    // SAFETY: per the caller.
    let gs = unsafe { config(serv) }.gs.cast::<ServerCounters>();
    // SAFETY: `ServerGS` starts with the counters, mapped for the server's life.
    unsafe { &mut *gs }
}

/// `Server::is_started()`.
///
/// # Safety
/// As [`counters`].
pub unsafe fn is_started(serv: *mut c_void) -> bool {
    // SAFETY: per the caller.
    unsafe { counters(serv) }.start.load(Ordering::Relaxed) != 0
}

/// `Server::is_worker_thread()`: thread mode, on a worker thread.
///
/// # Safety
/// As [`config`].
pub unsafe fn is_worker_thread(serv: *mut c_void) -> bool {
    /// `Server::MODE_THREAD` and `THREAD_WORKER`.
    const MODE_THREAD: u8 = 3;
    const THREAD_WORKER: u8 = 4;
    // SAFETY: per the caller.
    unsafe { config(serv) }.mode_ == MODE_THREAD
        && swoole_rs_corebase::thread_kind() == THREAD_WORKER
}

/// `Server::get_connection_num()`: the per-worker cells when there are any.
///
/// # Safety
/// As [`counters`].
pub unsafe fn connection_num(serv: *mut c_void) -> usize {
    // SAFETY: per the caller.
    let gs = unsafe { counters(serv) };
    if gs.connection_nums.is_null() {
        return gs.connection_num.load(Ordering::Relaxed) as usize;
    }
    // SAFETY: per the caller.
    let workers = unsafe { config(serv) }.worker_num as usize;
    (0..workers)
        .map(|i| {
            let cell = gs.connection_nums.wrapping_add(i);
            // SAFETY: one cell per event worker, allocated with the counters.
            let cell = unsafe { &*cell };
            cell.load(Ordering::Relaxed) as usize
        })
        .fold(0, usize::wrapping_add)
}

/// `swr_v1_server_connection(state, key, kind)`.
///
/// # Safety
/// As [`config`]; the connection is the server's, under its own synchronization.
unsafe fn connection<'a>(
    serv: *mut c_void,
    key: c_long,
    kind: u32,
) -> Option<&'a mut ConnectionRecord> {
    // SAFETY: per the caller.
    let state = unsafe { fields(serv) }.state;
    // SAFETY: the server's live state and tables.
    let conn = unsafe { crate::swr_v1_server_connection(state, key, kind) };
    // SAFETY: null, or a slot of the live connection table.
    unsafe { conn.as_mut() }
}

/// `Server::get_connection_verify(session_id)`.
///
/// # Safety
/// As [`connection`].
pub unsafe fn verified<'a>(serv: *mut c_void, session_id: i64) -> Option<&'a mut ConnectionRecord> {
    // SAFETY: per the caller.
    unsafe { connection(serv, session_id, 2) }
}

/// `Server::get_connection_by_session_id(session_id)`: the session's slot,
/// unverified.
///
/// # Safety
/// As [`connection`].
pub unsafe fn by_session<'a>(
    serv: *mut c_void,
    session_id: i64,
) -> Option<&'a mut ConnectionRecord> {
    // SAFETY: per the caller.
    unsafe { connection(serv, session_id, 1) }
}

/// `Server::get_connection_for_iterator(fd)`.
///
/// # Safety
/// As [`connection`].
pub unsafe fn iterated<'a>(serv: *mut c_void, fd: c_int) -> Option<&'a mut ConnectionRecord> {
    // SAFETY: per the caller.
    unsafe { connection(serv, c_long::from(fd), 4) }
}

/// A `ListenPort`'s settings.
///
/// # Safety
/// `port` is a live `ListenPort`.
pub unsafe fn port<'a>(port: *mut c_void) -> &'a PortConfig {
    // SAFETY: the head of a live port, per the module comment.
    let config = unsafe { *port.cast::<*const PortConfig>() };
    // SAFETY: its settings live as long as it.
    unsafe { &*config }
}

/// `ListenPort::get_fd()`.
///
/// # Safety
/// As [`port`].
pub unsafe fn port_fd(port: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let socket = unsafe { self::port(port) }.socket.cast::<SocketRecord>();
    // SAFETY: null, or the port's live listening socket.
    unsafe { socket.as_ref() }.map_or(-1, |socket| socket.fd)
}

/// `ListenPort::get_connection_num()` of one of `serv`'s ports.
///
/// # Safety
/// As [`port`] and [`counters`].
pub unsafe fn port_connection_num(serv: *mut c_void, port: *mut c_void) -> usize {
    // SAFETY: per the caller.
    let gs = unsafe { self::port(port) }.gs;
    // SAFETY: the port's mapped counters.
    let cells = unsafe { &*gs.cast::<PortCounters>() }.connection_nums;
    let workers = if cells.is_null() {
        0
    } else {
        // SAFETY: per the caller.
        unsafe { config(serv) }.worker_num
    };
    // SAFETY: the port's live counters, with one cell per event worker.
    unsafe { crate::swr_v1_server_port_connections(gs, workers) }
}

/// `Server::get_max_connection()`.
///
/// # Safety
/// As [`config`].
pub unsafe fn max_connection(serv: *mut c_void) -> u32 {
    // SAFETY: per the caller.
    let tables = unsafe { state(serv) }.tables();
    // SAFETY: the tables live as long as the server.
    unsafe { &*tables }.max_connection
}

/// `serv->get_task_worker_pool()`.
///
/// # Safety
/// As [`counters`].
pub unsafe fn task_pool(serv: *mut c_void) -> *mut PoolRecord {
    // SAFETY: per the caller.
    let gs = unsafe { config(serv) }.gs.cast::<u8>();
    let offset = size_of::<ServerCounters>().next_multiple_of(align_of::<PoolRecord>());
    gs.wrapping_add(offset).cast()
}

/// `Server::get_user_worker_num()`: the user workers added so far.
///
/// # Safety
/// As [`config`].
pub unsafe fn user_worker_num(serv: *mut c_void) -> usize {
    // SAFETY: per the caller.
    let registry = unsafe { config(serv) }.worker_registry;
    // SAFETY: the server's live registry, configured only by the master.
    unsafe {
        swoole_rs_corebase::worker_registry::swr_v1_process_worker_registry_pending_count(
            registry.cast(),
        )
    }
}

/// The server's worker arrays and counts.
///
/// # Safety
/// As [`counters`].
pub unsafe fn workers(serv: *mut c_void) -> ServerWorkers {
    // SAFETY: per the caller.
    let task_pool = unsafe { task_pool(serv) };
    // SAFETY: per the caller.
    let config = unsafe { config(serv) };
    ServerWorkers {
        event_pool: task_pool.wrapping_add(1),
        task_pool,
        event_workers: config.workers.cast(),
        user_workers: config.user_workers.cast(),
        event_count: config.worker_num,
        task_count: config.task_worker_num,
        // SAFETY: per the caller.
        user_count: unsafe { user_worker_num(serv) },
    }
}

/// `Server::get_worker(id)`: event workers, then task workers, then user
/// workers; null past the last.
///
/// # Safety
/// As [`counters`].
pub unsafe fn worker(serv: *mut c_void, id: u16) -> *mut WorkerRecord {
    // SAFETY: per the caller.
    let view = unsafe { workers(serv) };
    // SAFETY: the server's live pools and worker arrays, with their counts.
    unsafe { swr_v1_process_server_worker(&raw const view, id) }
}

/// `Server::get_connection(fd)`.
///
/// # Safety
/// As [`connection`].
pub unsafe fn at<'a>(serv: *mut c_void, fd: c_int) -> Option<&'a mut ConnectionRecord> {
    // SAFETY: per the caller.
    unsafe { connection(serv, c_long::from(fd), 0) }
}

/// `Server::get_server_socket(fd)`: the listening socket at `fd`, or null.
///
/// # Safety
/// As [`connection`].
pub unsafe fn server_socket(serv: *mut c_void, fd: c_long) -> *mut c_void {
    // SAFETY: per the caller.
    unsafe { connection(serv, fd, 0) }.map_or(core::ptr::null_mut(), |conn| conn.socket)
}

/// A `network::Socket`'s fd and `swSocketType`.
///
/// # Safety
/// `socket` is a live socket.
pub unsafe fn socket_fd(socket: *mut c_void) -> (c_int, c_int) {
    // SAFETY: per the caller.
    let socket = unsafe { &*socket.cast::<SocketRecord>() };
    (socket.fd, socket.socket_type)
}

/// `Server::get_port_by_fd(fd)`: the `ListenPort` a connection arrived on, or
/// null.
///
/// # Safety
/// As [`connection`].
pub unsafe fn port_by_fd(serv: *mut c_void, fd: c_int) -> *mut c_void {
    // SAFETY: per the caller.
    let Some(server_fd) = (unsafe { at(serv, fd) }).map(|conn| conn.server_fd) else {
        return core::ptr::null_mut();
    };
    // SAFETY: per the caller.
    unsafe { port_by_server_fd(serv, server_fd) }
}

/// `Server::get_port_by_server_fd(server_fd)`: the `ListenPort` listening on
/// `server_fd`, or null.
///
/// # Safety
/// As [`connection`].
pub unsafe fn port_by_server_fd(serv: *mut c_void, server_fd: c_int) -> *mut c_void {
    // SAFETY: per the caller; a listening socket's record holds its port.
    unsafe { at(serv, server_fd) }.map_or(core::ptr::null_mut(), |listen| listen.object)
}

/// Whether one of `serv`'s ports speaks WebSocket.
///
/// # Safety
/// `port` is a live `ListenPort`.
pub unsafe fn port_websocket(port: *mut c_void) -> bool {
    // SAFETY: per the caller.
    unsafe { self::port(port) }.open_websocket_protocol
}

/// `Server::is_healthy_connection(now, conn)`: protected, never received, or
/// received within its port's heartbeat idle time.
///
/// # Safety
/// As [`connection`]; `conn` is one of `serv`'s connections.
pub unsafe fn healthy(serv: *mut c_void, now: f64, conn: &ConnectionRecord) -> bool {
    if conn.protect != 0 || conn.last_recv_time == 0.0 {
        return true;
    }
    // SAFETY: per the caller (`get_port_by_session_id`).
    let Some(fd) = (unsafe { connection(serv, conn.session_id, 1) }).map(|conn| conn.fd) else {
        return true;
    };
    // SAFETY: per the caller.
    let port = unsafe { port_by_fd(serv, fd) };
    if port.is_null() {
        return true;
    }
    // SAFETY: the connection's live port.
    let idle = unsafe { self::port(port) }.heartbeat_idle_time;
    idle == 0 || conn.last_recv_time > now - f64::from(idle)
}

/// `serv->ports[index]`: the `ListenPort`s in listen order, null past the last
/// (index 0 is `Server::get_primary_port()`).
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe fn port_at(serv: *mut c_void, index: usize) -> *mut c_void {
    // SAFETY: per the caller.
    let tables = unsafe { state(serv) }.tables();
    if tables.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the live server's tables.
    let ports = unsafe { &*tables }.ports();
    ports.get(index).copied().unwrap_or(core::ptr::null_mut())
}

/// `Server::get_port(port)`: the first port listening on `port` (any at 0),
/// or null.
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe fn port_number(serv: *mut c_void, port: c_int) -> *mut c_void {
    // SAFETY: per the caller.
    unsafe { find_port(serv, |config| port == 0 || config.port == port) }
}

/// The first of `serv->ports` whose settings `wanted` accepts, or null.
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe fn find_port(serv: *mut c_void, wanted: impl Fn(&PortConfig) -> bool) -> *mut c_void {
    // SAFETY: per the caller.
    let tables = unsafe { state(serv) }.tables();
    if tables.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the live server's tables, whose ports are live.
    unsafe { &*tables }
        .ports()
        .iter()
        .copied()
        // SAFETY: as above.
        .find(|&ls| wanted(unsafe { port(ls) }))
        .unwrap_or(core::ptr::null_mut())
}

/// The connection socket's `recv_buffer` (a `swoole::String *`), or null.
///
/// # Safety
/// `conn` is a live connection whose `socket` is null or live.
pub unsafe fn recv_buffer(conn: &ConnectionRecord) -> *mut c_void {
    let socket = conn.socket.cast::<SocketRecord>();
    if socket.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: per the caller.
    unsafe { &*socket }.recv_buffer
}
