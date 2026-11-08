//! Server connections, sessions and lookup tables owned and operated by Rust.
use core::ffi::{c_int, c_void};
use core::sync::atomic::AtomicU32;
use swoole_rs_corebase::net_types::SocketAddress;

/// A shared session-table slot; reactor bits preserve the native reserved bits.
#[repr(C)]
pub struct SessionRecord {
    pub id: libc::c_long,
    pub fd: c_int,
    pub reactor_bits: u32,
}

/// The wire connection record; native code holds a borrowed view of these fields.
#[repr(C)]
pub struct ConnectionRecord {
    pub active: u8,
    pub socket_type: c_int,
    pub fd: c_int,
    pub worker_id: c_int,
    pub session_id: libc::c_long,
    pub ssl: u8,
    pub ssl_ready: u8,
    pub overflow: u8,
    pub high_watermark: u8,
    pub http_upgrade: u8,
    pub http2_stream: u8,
    pub websocket_compression: u8,
    pub close_actively: u8,
    pub close_queued: u8,
    pub closing: u8,
    pub close_reset: u8,
    pub peer_closed: u8,
    pub protect: u8,
    pub close_notify: u8,
    pub close_force: u8,
    pub reactor_id: u8,
    pub close_errno: u16,
    pub recv_queued_bytes: AtomicU32,
    pub send_queued_bytes: u32,
    pub waiting_time: u16,
    pub local_port: u16,
    pub local_addr_index: u16,
    pub timer: *mut c_void,
    pub info: SocketAddress,
    pub object: *mut c_void,
    pub socket: *mut c_void,
    pub connect_time: f64,
    pub last_recv_time: f64,
    pub last_send_time: f64,
    pub last_dispatch_time: f64,
    pub uid: u32,
    pub websocket_status: u8,
    pub websocket_buffer: *mut c_void,
    pub ssl_client_cert: *mut c_void,
    pub ssl_client_cert_pid: c_int,
    pub lock: AtomicU32,
    pub closed: AtomicU32,
    pub server_fd: c_int,
}

/// Server-private shared table loans and bounds. The arena owns the underlying shared mappings.
#[repr(C)]
pub struct ServerTables {
    pub connection_list: *mut ConnectionRecord,
    pub session_list: *mut SessionRecord,
    pub port_gs_list: *mut c_void,
    pub max_connection: u32,
    pub enable_accept_timer: *mut c_void,
    pub reactor_pipe_num: u16,
    pub reactor_threads: *mut c_void,
    /// `Server::ports`.
    pub ports: PortList,
    /// `&Server::message_bus`, whose `MessageBus` starts with its Rust bus.
    pub message_bus: *mut c_void,
}

/// The `ListenPort`s in listen order: a loan of `ServerState`'s list.
#[repr(C)]
pub struct PortList {
    pub items: *const *mut c_void,
    pub count: u32,
}

impl ServerTables {
    /// The listen ports.
    pub(crate) fn ports(&self) -> &[*mut c_void] {
        let PortList { items, count } = self.ports;
        if items.is_null() {
            return &[];
        }
        // SAFETY: the live buffer `ServerState::push_port` published with this length.
        unsafe { core::slice::from_raw_parts(items, count as usize) }
    }
}

impl Default for ServerTables {
    fn default() -> Self {
        Self {
            connection_list: core::ptr::null_mut(),
            session_list: core::ptr::null_mut(),
            port_gs_list: core::ptr::null_mut(),
            max_connection: 0,
            enable_accept_timer: core::ptr::null_mut(),
            reactor_pipe_num: 0,
            reactor_threads: core::ptr::null_mut(),
            ports: PortList {
                items: core::ptr::null(),
                count: 0,
            },
            message_bus: core::ptr::null_mut(),
        }
    }
}

/// Borrows the private table fields for the original native Server view.
///
/// # Safety
/// `state` is live, and the view is no longer used after `state_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_tables_get(
    state: *mut crate::ServerState,
) -> *mut ServerTables {
    // SAFETY: the caller lends a live state whose table-view allocation is still owned.
    unsafe { &*state }.tables()
}

/// Borrows a session-table slot through the original shared-table modulo rule.
///
/// # Safety
/// `state` owns initialized live tables and `id` is in the server's session domain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_session(
    state: *const crate::ServerState,
    id: libc::c_long,
) -> *mut SessionRecord {
    // SAFETY: the caller lends a live state.
    let tables = unsafe { &*state }.tables();
    // SAFETY: the state owns its table-view allocation.
    let sessions = unsafe { (*tables).session_list };
    sessions.wrapping_offset(isize::try_from(id % 1_048_576).unwrap_or_default())
}

/// Lookup kind: 0 fd, 1 session fd, 2 verified session, 3 verified TLS session, 4 fd iterator.
///
/// # Safety
/// `state` owns initialized tables. Slot contents, pointer loans and atomic fields
/// follow the server's native synchronization/lifetime protocol for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_connection(
    state: *const crate::ServerState,
    key: libc::c_long,
    kind: u32,
) -> *mut ConnectionRecord {
    // SAFETY: the caller lends a live state.
    let tables = unsafe { &*state }.tables();
    // SAFETY: the state owns the stable table-view allocation for this call.
    let tables = unsafe { &*tables };
    let mut session_id = 0;
    let fd = if matches!(kind, 1..=3) {
        let slot = tables
            .session_list
            .wrapping_offset(isize::try_from(key % 1_048_576).unwrap_or_default());
        // SAFETY: the requested session is in the initialized shared table.
        let session = unsafe { &*slot };
        session_id = session.id;
        session.fd
    } else {
        // Native's parameter is int for fd-based lookups; preserve its low bits.
        c_int::from_ne_bytes(
            u32::try_from(key.cast_unsigned() & u64::from(u32::MAX))
                .unwrap_or_default()
                .to_ne_bytes(),
        )
    };
    if fd.cast_unsigned() > tables.max_connection {
        return core::ptr::null_mut();
    }
    let connection = tables
        .connection_list
        .wrapping_offset(isize::try_from(fd).unwrap_or_default());
    if kind == 0 || kind == 1 {
        return connection;
    }
    if connection.is_null() {
        return connection;
    }
    // SAFETY: the native table bounds/session protocol lends this live connection record.
    let value = unsafe { &*connection };
    if value.active == 0 {
        return core::ptr::null_mut();
    }
    if matches!(kind, 2 | 3) && (session_id != key || value.session_id != key) {
        return core::ptr::null_mut();
    }
    if kind == 4 && value.closed.load(core::sync::atomic::Ordering::Relaxed) != 0 {
        return core::ptr::null_mut();
    }
    if matches!(kind, 3 | 4) && value.ssl != 0 && value.ssl_ready == 0 {
        return core::ptr::null_mut();
    }
    connection
}

/// Retire a dead base-mode worker's sessions, preserving IDs and reserved bits.
/// The table traversal stays entirely in Rust instead of crossing the native
/// boundary for each slot and worker-ID comparison.
/// # Safety
/// `state` loans its mapped complete 1,048,576-slot session table; the native
/// worker-retirement protocol synchronizes these per-field accesses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_sessions_retire(
    state: *const crate::ServerState,
    worker_id: u32,
) {
    // SAFETY: the caller retains the live state and its table-view allocation.
    let tables = unsafe { &*state }.tables();
    // SAFETY: the native retirement call loans the complete mapped session table.
    let sessions = unsafe { (*tables).session_list };
    for index in 0..1_048_576 {
        let session = sessions.wrapping_add(index);
        // SAFETY: this is one initialized slot's native 8-bit reactor-ID field.
        let reactor = unsafe { (*session).reactor_bits } & 0xff;
        if reactor == worker_id {
            // SAFETY: retire exactly this worker's FD while retaining all other
            // slot fields; no whole-record borrow overlaps native session activity.
            unsafe { (*session).fd = 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{swr_v1_server_state_free, swr_v1_server_state_new};

    #[test]
    fn retirement_scans_table_edges_without_truncating_worker_identity() {
        let state = swr_v1_server_state_new();
        let mut sessions: Vec<_> = (0..1_048_576)
            .map(|id| SessionRecord {
                id,
                fd: 23,
                reactor_bits: 0x5a12_3402,
            })
            .collect();
        sessions[0].reactor_bits = 0x5a12_3401;
        sessions[1_048_575].reactor_bits = 0x5a12_3401;
        unsafe {
            (*swr_v1_server_tables_get(state)).session_list = sessions.as_mut_ptr();
            swr_v1_server_sessions_retire(state, 257);
        }
        assert!(sessions.iter().all(|slot| slot.fd == 23));
        unsafe { swr_v1_server_sessions_retire(state, 1) };
        for (index, slot) in sessions.iter().enumerate() {
            let edge = index == 0 || index == 1_048_575;
            assert_eq!(slot.id, i64::try_from(index).unwrap());
            assert_eq!(
                slot.reactor_bits,
                if edge { 0x5a12_3401 } else { 0x5a12_3402 }
            );
            assert_eq!(slot.fd, if edge { 0 } else { 23 });
        }
        unsafe { swr_v1_server_state_free(state) };
    }

    #[test]
    fn sessions_validate_reused_slots_tls_and_iteration() {
        let state = swr_v1_server_state_new();
        let mut sessions = [
            SessionRecord {
                id: 0,
                fd: 0,
                reactor_bits: 0,
            },
            SessionRecord {
                id: 1,
                fd: 2,
                reactor_bits: 7,
            },
        ];
        let mut connections: [ConnectionRecord; 3] = unsafe { core::mem::zeroed() };
        connections[2].active = 1;
        connections[2].session_id = 1;
        unsafe {
            let tables = &mut *swr_v1_server_tables_get(state);
            tables.session_list = sessions.as_mut_ptr();
            tables.connection_list = connections.as_mut_ptr();
            tables.max_connection = 2;
            assert_eq!(
                swr_v1_server_session(state, 1),
                sessions.as_mut_ptr().add(1)
            );
            let expected = connections.as_mut_ptr().add(2);
            assert_eq!(swr_v1_server_connection(state, 1, 2), expected);
            assert!(swr_v1_server_connection(state, 1_048_577, 2).is_null());
            assert!(swr_v1_server_connection(state, -1, 0).is_null());
            assert!(swr_v1_server_connection(state, 3, 0).is_null());
            connections[2].ssl = 1;
            assert!(swr_v1_server_connection(state, 1, 3).is_null());
            assert_eq!(swr_v1_server_connection(state, 1, 2), expected);
            connections[2].ssl_ready = 1;
            assert_eq!(swr_v1_server_connection(state, 1, 3), expected);
            connections[2]
                .closed
                .store(1, core::sync::atomic::Ordering::Relaxed);
            assert!(swr_v1_server_connection(state, 2, 4).is_null());
            connections[2]
                .closed
                .store(0, core::sync::atomic::Ordering::Relaxed);
            assert_eq!(swr_v1_server_connection(state, 2, 4), expected);
            (*expected).active = 0;
            assert!(swr_v1_server_connection(state, 1, 2).is_null());
            swr_v1_server_state_free(state);
        }
    }
}
