//! Server connections, sessions and lookup tables owned and operated by Rust.
use core::cell::UnsafeCell;
use core::ffi::{c_int, c_void};
use core::sync::atomic::AtomicU32;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_memory::members::Shm;

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

impl ConnectionRecord {
    /// The connection's session socket, or `None` before it has one or after it closes:
    /// closing frees the socket and nulls the field in one step, on the socket's thread.
    #[must_use]
    pub fn socket(&self) -> Option<swoole_rs_reactor::Socket<'_>> {
        // SAFETY: null, or the connection's live socket, as above.
        unsafe { swoole_rs_reactor::Socket::from_ptr(self.socket.cast()) }
    }
}

/// Server-private shared table loans and bounds. The arena owns the underlying shared mappings.
#[repr(C)]
pub struct ServerTables {
    /// `ConnectionRecord`s, `max_connection` of them.
    pub connection_list: Shm,
    /// `SessionRecord`s, `SW_SESSION_LIST_SIZE` of them.
    pub session_list: Shm,
    pub port_gs_list: Shm,
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
    /// The connection slots, exactly `max_connection` of them, or none before
    /// they're mapped. Index it with `get`, so every fd lookup is bounded by the
    /// table's own length.
    pub(crate) fn connections(&self) -> &[UnsafeCell<ConnectionRecord>] {
        let base = self.connection_list.as_ptr();
        if base.is_null() {
            return &[];
        }
        // SAFETY: every factory maps the table with `max_connection` zeroed records
        // and frees it only by nulling `connection_list`; `UnsafeCell` has the
        // record's layout and lets slots be written under the server's own
        // synchronization.
        unsafe { core::slice::from_raw_parts(base.cast(), self.max_connection as usize) }
    }

    /// The slot of `fd`, or `None` when `fd` is outside the table.
    #[expect(
        clippy::mut_from_ref,
        reason = "a slot of the connection table, written under the server's own synchronization"
    )]
    pub(crate) fn connection_at(&self, fd: c_int) -> Option<&mut ConnectionRecord> {
        // SAFETY: a slot of the live table; the server's own synchronization orders writes.
        Some(unsafe { &mut *self.connection_slot(fd)? })
    }

    /// The slot of `fd` as the table's own pointer, or `None` when `fd` is outside it.
    pub(crate) fn connection_slot(&self, fd: c_int) -> Option<*mut ConnectionRecord> {
        Some(self.connections().get(usize::try_from(fd).ok()?)?.get())
    }

    /// The session slots, or null before they're mapped.
    pub(crate) fn sessions(&self) -> *mut SessionRecord {
        self.session_list.as_ptr().cast()
    }

    /// The listen ports.
    pub(crate) fn ports(&self) -> &[crate::view::Port<'_>] {
        let PortList { items, count } = self.ports;
        if items.is_null() {
            return &[];
        }
        // SAFETY: the live buffer `ServerState::push_port` published with this length;
        // each item is a live, non-null port of the server (`Port` has a pointer's layout).
        unsafe { core::slice::from_raw_parts(items.cast(), count as usize) }
    }
}

impl Default for ServerTables {
    fn default() -> Self {
        Self {
            connection_list: Shm::NULL,
            session_list: Shm::NULL,
            port_gs_list: Shm::NULL,
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

/// `get_connection(fd)`: the fd's slot, active or not.
pub(crate) const BY_FD: u32 = 0;
/// `get_connection_by_session_id(id)`: the session's slot, unverified.
pub(crate) const BY_SESSION: u32 = 1;
/// `get_connection_verify_no_ssl(id)`: the session's active slot.
pub(crate) const VERIFIED: u32 = 2;
/// `get_connection_verify(id)`: as [`VERIFIED`], and TLS-ready.
pub(crate) const VERIFIED_TLS: u32 = 3;
/// `get_connection_for_iterator(fd)`: an open, TLS-ready slot.
pub(crate) const ITERATED: u32 = 4;

impl crate::ServerState {
    /// The session-table slot of `id`, by the original shared-table modulo rule,
    /// or `None` before the table is mapped.
    #[expect(
        clippy::mut_from_ref,
        reason = "a slot of the shared session table, written under the server's own synchronization"
    )]
    pub(crate) fn session(&self, id: libc::c_long) -> Option<&mut SessionRecord> {
        let sessions = self.tables_ref().sessions();
        let index = usize::try_from(id % 1_048_576).ok()?;
        if sessions.is_null() {
            return None;
        }
        // SAFETY: the mapped 1,048,576-slot table this state lends; `index` is inside it.
        unsafe { sessions.wrapping_add(index).as_mut() }
    }

    /// The connection slot `kind` (`BY_FD` .. `ITERATED`) finds for `key`.
    #[expect(
        clippy::mut_from_ref,
        reason = "a slot of the connection table, written under the server's own synchronization"
    )]
    pub(crate) fn connection(&self, key: libc::c_long, kind: u32) -> Option<&mut ConnectionRecord> {
        // SAFETY: null or a slot of the live table.
        unsafe { self.connection_ptr(key, kind).as_mut() }
    }

    /// The connection slot `kind` (`BY_FD` .. `ITERATED`) finds for `key`, or null.
    /// The pointer is the table's own, not one taken from a reference, so a
    /// later lookup of the slot (onClose's `exist()` during the close holding
    /// this one) leaves it valid.
    pub(crate) fn connection_ptr(&self, key: libc::c_long, kind: u32) -> *mut ConnectionRecord {
        self.lookup(key, kind).unwrap_or(core::ptr::null_mut())
    }

    fn lookup(&self, key: libc::c_long, kind: u32) -> Option<*mut ConnectionRecord> {
        let tables = self.tables_ref();
        let mut session_id = 0;
        let fd = if matches!(kind, BY_SESSION..=VERIFIED_TLS) {
            // Session ids are positive; a negative key names no session.
            let session = self.session(key)?;
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
        let slot = tables.connection_slot(fd)?;
        // SAFETY: a slot of the live table.
        let value = unsafe { &*slot };
        let found = match kind {
            BY_FD | BY_SESSION => true,
            _ if value.active == 0 => false,
            VERIFIED | VERIFIED_TLS if session_id != key || value.session_id != key => false,
            ITERATED if value.closed.load(core::sync::atomic::Ordering::Relaxed) != 0 => false,
            VERIFIED_TLS | ITERATED => value.ssl == 0 || value.ssl_ready != 0,
            _ => true,
        };
        found.then_some(slot)
    }

    /// Retire a dead base-mode worker's sessions, preserving IDs and reserved bits.
    pub(crate) fn retire_sessions(&self, worker_id: u32) {
        for id in 0..1_048_576 {
            if let Some(session) = self.session(id)
                && session.reactor_bits & 0xff == worker_id
            {
                session.fd = 0;
            }
        }
    }
}

/// [`ServerState::session`] for the C++ test fixtures.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_session(
    state: &crate::ServerState,
    id: libc::c_long,
) -> *mut SessionRecord {
    state
        .session(id)
        .map_or(core::ptr::null_mut(), core::ptr::from_mut)
}

/// [`ServerState::connection_ptr`] for the C++ test fixtures.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_connection(
    state: &crate::ServerState,
    key: libc::c_long,
    kind: u32,
) -> *mut ConnectionRecord {
    state.connection_ptr(key, kind)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::{free_state, swr_v1_server_state_new};

    /// A table field naming a test's own records, so several states can share them as
    /// processes share a mapping. Nothing frees it: the test owns `items`.
    pub(crate) fn naming<T>(items: &mut [T]) -> Shm {
        naming_at(items.as_mut_ptr())
    }

    /// [`naming`] at `items`, for a test that goes on reaching the records itself: it
    /// does so through `items` (or a `Vec::as_mut_ptr`, which takes no reference), since a
    /// fresh reference to the whole table would end the state's pointer into it.
    pub(crate) fn naming_at<T>(items: *mut T) -> Shm {
        // SAFETY: `Shm` is `repr(transparent)` over its payload pointer, and no code under
        // test frees these tables.
        unsafe { core::mem::transmute::<*mut c_void, Shm>(items.cast()) }
    }

    #[test]
    #[cfg_attr(miri, ignore = "a million-session table takes Miri over 15 minutes")]
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
        unsafe { (*(*state).tables()).session_list = naming(&mut sessions) };
        unsafe { &*state }.retire_sessions(257);
        assert!(sessions.iter().all(|slot| slot.fd == 23));
        unsafe { &*state }.retire_sessions(1);
        for (index, slot) in sessions.iter().enumerate() {
            let edge = index == 0 || index == 1_048_575;
            assert_eq!(slot.id, i64::try_from(index).unwrap());
            assert_eq!(
                slot.reactor_bits,
                if edge { 0x5a12_3401 } else { 0x5a12_3402 }
            );
            assert_eq!(slot.fd, if edge { 0 } else { 23 });
        }
        unsafe { free_state(state) };
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
        let (session_at, connection_at) = (
            (&raw mut sessions).cast::<SessionRecord>(),
            (&raw mut connections).cast::<ConnectionRecord>(),
        );
        unsafe {
            let tables = &mut *(*state).tables();
            tables.session_list = naming_at(session_at);
            tables.connection_list = naming_at(connection_at);
            tables.max_connection = 3;
            assert_eq!(swr_v1_server_session(&*state, 1), session_at.add(1));
            let expected = connection_at.add(2);
            assert_eq!(swr_v1_server_connection(&*state, 1, 2), expected);
            assert!(swr_v1_server_connection(&*state, 1_048_577, 2).is_null());
            assert!(swr_v1_server_connection(&*state, -1, 0).is_null());
            assert!(swr_v1_server_connection(&*state, -1, 1).is_null());
            assert!(swr_v1_server_connection(&*state, -1, 2).is_null());
            // fd == max_connection is one past the table.
            assert!(swr_v1_server_connection(&*state, 3, 0).is_null());
            (*expected).ssl = 1;
            assert!(swr_v1_server_connection(&*state, 1, 3).is_null());
            assert_eq!(swr_v1_server_connection(&*state, 1, 2), expected);
            (*expected).ssl_ready = 1;
            assert_eq!(swr_v1_server_connection(&*state, 1, 3), expected);
            (*expected)
                .closed
                .store(1, core::sync::atomic::Ordering::Relaxed);
            assert!(swr_v1_server_connection(&*state, 2, 4).is_null());
            (*expected)
                .closed
                .store(0, core::sync::atomic::Ordering::Relaxed);
            assert_eq!(swr_v1_server_connection(&*state, 2, 4), expected);
            (*expected).active = 0;
            assert!(swr_v1_server_connection(&*state, 1, 2).is_null());
            free_state(state);
        }
    }
}
