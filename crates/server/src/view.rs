//! A native `Server`'s Rust-owned records, read straight from its pointer by
//! the crates that bind it (phpserver), without a native call.
//!
//! `Server` has no virtual methods and `detail::ServerFields` is its first base,
//! so a `Server *` points at `ServerFields { state_, config_, tables_ }`
//! (`native/include/swoole_rs_server_view.h`). `ListenPort` likewise starts
//! with `detail::PortFields { port_config_ }`.
use crate::port_config::PortConfig;
use crate::{ConnectionRecord, PortCounters, ServerConfig, ServerCounters, ServerState};
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering;
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

/// `Server::is_started()`.
///
/// # Safety
/// As [`counters`].
pub unsafe fn is_started(serv: *mut c_void) -> bool {
    // SAFETY: per the caller.
    unsafe { counters(serv) }.start.load(Ordering::Relaxed) != 0
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
unsafe fn port<'a>(port: *mut c_void) -> &'a PortConfig {
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
