//! Server connections, sessions and lookup tables owned and operated by Rust.
use core::cell::Cell;
use core::ffi::{c_int, c_void};
use core::sync::atomic::{
    AtomicU8, AtomicU32,
    Ordering::{Acquire, Relaxed},
};
use swoole_rs_corebase::WebSocketStatus;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_memory::members::Shm;

use crate::reactor_ops::ThreadRecord;

/// A shared session-table slot; reactor bits preserve the native reserved bits.
pub struct SessionRecord {
    pub id: Cell<libc::c_long>,
    pub fd: Cell<c_int>,
    pub reactor_bits: Cell<u32>,
}

/// The wire connection record; native code holds a borrowed view of these fields.
#[repr(C)]
pub struct ConnectionRecord {
    /// Read by every reactor thread (`is_valid_connection` over the fd range).
    pub active: AtomicU8,
    pub socket_type: Cell<c_int>,
    pub fd: Cell<c_int>,
    pub worker_id: Cell<c_int>,
    pub session_id: Cell<libc::c_long>,
    pub ssl: Cell<bool>,
    pub ssl_ready: Cell<bool>,
    pub overflow: Cell<bool>,
    pub high_watermark: Cell<bool>,
    pub http_upgrade: Cell<bool>,
    pub http2_stream: Cell<bool>,
    pub websocket_compression: Cell<bool>,
    pub close_actively: Cell<bool>,
    pub close_queued: Cell<bool>,
    pub closing: Cell<bool>,
    pub close_reset: Cell<bool>,
    pub peer_closed: Cell<bool>,
    pub protect: Cell<bool>,
    pub close_notify: Cell<bool>,
    pub close_force: Cell<bool>,
    pub reactor_id: Cell<u8>,
    pub close_errno: Cell<u16>,
    pub recv_queued_bytes: AtomicU32,
    pub send_queued_bytes: Cell<u32>,
    pub waiting_time: Cell<u16>,
    pub local_port: Cell<u16>,
    pub local_addr_index: Cell<u16>,
    pub timer: Cell<Option<swoole_rs_corebase::timer_key::Held>>,
    pub info: Cell<SocketAddress>,
    /// Null, a listening slot's port ([`ServerState::hold_port`]) as its plain
    /// address, or anything else (a session's protocol state, a pipe) [`encode`]d
    /// negative. Private: only this module's typed setters write it.
    object: Cell<*mut c_void>,
    /// The session socket, `None` before the connection has one or after it closes: a
    /// record live until the unsafe free that ends the connection.
    pub socket: Cell<Option<swoole_rs_reactor::Socket<'static>>>,
    pub connect_time: Cell<f64>,
    pub last_recv_time: Cell<f64>,
    pub last_send_time: Cell<f64>,
    pub last_dispatch_time: Cell<f64>,
    pub uid: Cell<u32>,
    pub websocket_status: Cell<WebSocketStatus>,
    /// The WebSocket message being reassembled: a box only the connection's reactor
    /// makes, takes and frees.
    pub(crate) websocket_buffer: Cell<Option<Box<swoole_rs_buffer::string::CppString>>>,
    /// Null, or the `CppString` box the worker `ssl_client_cert_pid` stored with
    /// [`Connection::store_client_cert`]. Private with the pid, so nothing else names a
    /// certificate here.
    pub(crate) ssl_client_cert: Cell<*mut c_void>,
    pub(crate) ssl_client_cert_pid: Cell<c_int>,
    pub lock: AtomicU32,
    pub closed: AtomicU32,
    pub server_fd: Cell<c_int>,
}

impl ConnectionRecord {
    /// Whether the slot holds a connection (`active`), whose fields `add` published.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.load(Acquire) != 0
    }
}

/// The top address bit: set on every slot object but a port.
const NOT_PORT: usize = 1 << (usize::BITS - 1);

/// A 2-aligned `object` that isn't a port, as `(address >> 1) | NOT_PORT`: negative,
/// so [`Connection::port`] reads it as none with the sign test that replaces its null
/// test, and [`decode`] gives back every bit of the address.
fn encode<T>(object: *mut T) -> *mut c_void {
    const { assert!(align_of::<T>() >= 2, "the encoding drops the lowest bit") };
    object.cast::<c_void>().map_addr(|a| (a >> 1) | NOT_PORT)
}

/// The address [`encode`] stored, or `None` for null or a port.
fn decode<T>(object: *mut c_void) -> Option<*mut T> {
    (object.addr().cast_signed() < 0).then(|| object.map_addr(|a| a << 1).cast())
}

/// A slot of a shared table, borrowed for `'a` and read and written one field at a
/// time: every field is a cell or an atomic, so two handles to one slot share it and
/// never make a `&mut` (#3554). The server's own synchronization orders the writes,
/// as it orders native code's.
#[repr(transparent)]
pub struct Slot<'a, T> {
    record: &'a T,
}

/// A connection-table slot.
pub type Connection<'a> = Slot<'a, ConnectionRecord>;
/// A session-table slot.
pub type Session<'a> = Slot<'a, SessionRecord>;

impl<T> Clone for Slot<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Slot<'_, T> {}

impl<'a, T> From<&'a T> for Slot<'a, T> {
    fn from(record: &'a T) -> Self {
        Self { record }
    }
}

impl<'a, T> From<&'a mut T> for Slot<'a, T> {
    /// The record a unique borrow keeps live for `'a`.
    fn from(record: &'a mut T) -> Self {
        Self { record }
    }
}

impl<T> Slot<'_, T> {
    /// The table's own pointer to the slot.
    #[must_use]
    pub fn as_ptr(self) -> *mut T {
        core::ptr::from_ref(self.record).cast_mut()
    }
}

/// A getter and a setter for a cell field, or a getter for an atomic one.
macro_rules! field {
    (@[$($vis:tt)*] $get:ident = $field:ident, $set:ident: $ty:ty) => {
        #[must_use]
        $($vis)* fn $get(self) -> $ty {
            self.record.$field.get()
        }

        $($vis)* fn $set(self, value: $ty) {
            self.record.$field.set(value);
        }
    };
    (private $field:ident, $set:ident: $ty:ty) => {
        field!(@[] $field = $field, $set: $ty);
    };
    ($field:ident, $set:ident: $ty:ty) => {
        field!(@[pub] $field = $field, $set: $ty);
    };
    (atomic $field:ident: $ty:ty) => {
        #[must_use]
        pub fn $field(self) -> &'a $ty {
            &self.record.$field
        }
    };
}

impl Session<'_> {
    field!(id, set_id: libc::c_long);
    field!(fd, set_fd: c_int);
    field!(reactor_bits, set_reactor_bits: u32);
}

impl<'a> Connection<'a> {
    field!(socket_type, set_socket_type: c_int);
    field!(fd, set_fd: c_int);
    field!(worker_id, set_worker_id: c_int);
    field!(session_id, set_session_id: libc::c_long);
    field!(ssl, set_ssl: bool);
    field!(ssl_ready, set_ssl_ready: bool);
    field!(overflow, set_overflow: bool);
    field!(high_watermark, set_high_watermark: bool);
    field!(http_upgrade, set_http_upgrade: bool);
    field!(http2_stream, set_http2_stream: bool);
    field!(websocket_compression, set_websocket_compression: bool);
    field!(close_actively, set_close_actively: bool);
    field!(close_queued, set_close_queued: bool);
    field!(closing, set_closing: bool);
    field!(close_reset, set_close_reset: bool);
    field!(peer_closed, set_peer_closed: bool);
    field!(protect, set_protect: bool);
    field!(close_notify, set_close_notify: bool);
    field!(close_force, set_close_force: bool);
    field!(reactor_id, set_reactor_id: u8);
    field!(close_errno, set_close_errno: u16);
    field!(send_queued_bytes, set_send_queued_bytes: u32);
    field!(waiting_time, set_waiting_time: u16);
    field!(local_port, set_local_port: u16);
    field!(local_addr_index, set_local_addr_index: u16);
    field!(timer, set_timer: Option<swoole_rs_corebase::timer_key::Held>);
    field!(info, set_info: SocketAddress);
    field!(private object, set_object: *mut c_void);
    field!(socket, set_socket: Option<swoole_rs_reactor::Socket<'static>>);
    field!(connect_time, set_connect_time: f64);
    field!(last_recv_time, set_last_recv_time: f64);
    field!(last_send_time, set_last_send_time: f64);
    field!(last_dispatch_time, set_last_dispatch_time: f64);
    field!(uid, set_uid: u32);
    field!(websocket_status, set_websocket_status: WebSocketStatus);
    field!(private ssl_client_cert, set_ssl_client_cert: *mut c_void);
    field!(private ssl_client_cert_pid, set_ssl_client_cert_pid: c_int);

    /// The WebSocket message being reassembled, taken out of the connection.
    #[must_use]
    pub fn take_websocket_buffer(self) -> Option<Box<swoole_rs_buffer::string::CppString>> {
        self.record.websocket_buffer.take()
    }

    /// The connection reassembles `buffer`, which must hold none (one it held is
    /// freed).
    pub fn set_websocket_buffer(self, buffer: Box<swoole_rs_buffer::string::CppString>) {
        if let Some(mut held) = self.record.websocket_buffer.replace(Some(buffer)) {
            held.dispose();
        }
    }

    /// `OP_STORE_CLIENT_CERT`: `cert` is the connection's client certificate, which
    /// only this worker reads and frees.
    pub fn store_client_cert(self, cert: swoole_rs_buffer::string::CppString) {
        self.set_ssl_client_cert(Box::into_raw(Box::new(cert)).cast());
        self.set_ssl_client_cert_pid(swoole_rs_process::methods::swoole_get_worker_pid());
    }

    /// `OP_FREE_CLIENT_CERT`: the certificate this worker stored, taken from the
    /// connection, or `None` when this worker stored none.
    #[must_use]
    pub fn take_client_cert(self) -> Option<Box<swoole_rs_buffer::string::CppString>> {
        let cert = self
            .ssl_client_cert()
            .cast::<swoole_rs_buffer::string::CppString>();
        if cert.is_null()
            || self.ssl_client_cert_pid() != swoole_rs_process::methods::swoole_get_worker_pid()
        {
            return None;
        }
        self.set_ssl_client_cert(core::ptr::null_mut());
        // SAFETY: the box `store_client_cert` made in this worker, which the
        // connection named until just now.
        Some(unsafe { Box::from_raw(cert) })
    }
    field!(server_fd, set_server_fd: c_int);

    /// `(ListenPort *) conn->object`: the port a listening slot holds
    /// ([`ServerState::hold_port`]), or `None` on any other slot.
    #[must_use]
    #[inline]
    pub fn port(self) -> Option<crate::view::Port<'a>> {
        let raw = self.object();
        if raw.addr().cast_signed() <= 0 {
            return None;
        }
        // SAFETY: a positive object is a port: everything else is null or `encode`d
        // negative, and only `hold_port` stores one, from the port list of the server
        // whose table holds the slot. The server frees its ports only with that table, so
        // the port lives as long as the slot's borrow.
        unsafe { raw.cast::<crate::port_config::PortConfig>().as_ref() }
            .map(crate::view::Port::from)
    }

    /// The HTTP request a session holds, or `None`: an HTTP port's session holds
    /// nothing else.
    #[must_use]
    #[inline]
    pub fn http_request(self) -> Option<*mut swoole_rs_http_ffi::HttpRequest> {
        decode(self.object())
    }

    /// The session holds `request`, whose address comes back; the slot keeps it until
    /// [`Self::take_http_request`].
    #[inline]
    #[must_use]
    pub fn set_http_request(
        self,
        request: Box<swoole_rs_http_ffi::HttpRequest>,
    ) -> *mut swoole_rs_http_ffi::HttpRequest {
        let request = Box::into_raw(request);
        self.set_object(encode(request));
        request
    }

    /// The HTTP request the session held, which the slot no longer names: the box
    /// [`Self::set_http_request`] took, in the process that stored it.
    #[must_use]
    #[inline]
    pub fn take_http_request(self) -> Option<*mut swoole_rs_http_ffi::HttpRequest> {
        let request = self.http_request();
        self.clear_object();
        request
    }

    /// The redis request state a session holds, or `None`: a redis port's session
    /// holds nothing else.
    #[must_use]
    pub fn resp_request(self) -> Option<*mut swoole_rs_protocols::RespRequest> {
        decode(self.object())
    }

    /// The session holds `request`, whose address comes back; the slot keeps it until
    /// [`Self::take_resp_request`].
    #[must_use]
    pub fn set_resp_request(
        self,
        request: Box<swoole_rs_protocols::RespRequest>,
    ) -> *mut swoole_rs_protocols::RespRequest {
        let request = Box::into_raw(request);
        self.set_object(encode(request));
        request
    }

    /// The redis request state the session held, which the slot no longer names: the
    /// box [`Self::set_resp_request`] took, in the process that stored it.
    #[must_use]
    pub fn take_resp_request(self) -> Option<*mut swoole_rs_protocols::RespRequest> {
        let request = self.resp_request();
        self.clear_object();
        request
    }

    /// `Server::store_pipe_fd`: a pipe end's slot names its pipe, which nothing reads
    /// back as a port. An address with the top bit set, which no user-space pointer
    /// has, is not kept.
    pub(crate) fn set_pipe(self, pipe: *mut c_void) {
        let kept = pipe.addr().cast_signed() >= 0;
        self.set_object(if kept {
            pipe.map_addr(|a| a | NOT_PORT)
        } else {
            core::ptr::null_mut()
        });
    }

    /// The pipe [`Self::set_pipe`] stored, for tests.
    #[cfg(test)]
    pub(crate) fn pipe(self) -> Option<*mut c_void> {
        let raw = self.object();
        (raw.addr().cast_signed() < 0).then(|| raw.map_addr(|a| a & !NOT_PORT))
    }

    /// `conn->object = nullptr`.
    pub fn clear_object(self) {
        self.set_object(core::ptr::null_mut());
    }

    field!(atomic active: AtomicU8);
    field!(atomic recv_queued_bytes: AtomicU32);
    field!(atomic lock: AtomicU32);
    field!(atomic closed: AtomicU32);

    /// Whether the slot holds a connection (`active`), whose fields `add` published.
    #[must_use]
    pub fn is_active(self) -> bool {
        self.active().load(Acquire) != 0
    }

    /// A reactor event over the connection's socket, as the reactor would deliver it.
    #[must_use]
    pub fn event(self, reactor_id: i16, fd_type: c_int) -> swoole_rs_reactor::Event {
        swoole_rs_reactor::Event::of(self.fd(), reactor_id, fd_type, self.socket())
    }

    /// `f` over the client certificate this worker stored for the connection
    /// (`OP_STORE_CLIENT_CERT`), or `None` when this worker stored none.
    pub fn with_client_cert<R>(self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        if self.ssl_client_cert_pid() != swoole_rs_process::methods::swoole_get_worker_pid() {
            return None;
        }
        let cert = self
            .ssl_client_cert()
            .cast::<swoole_rs_buffer::string::CppString>();
        // SAFETY: null, or the certificate this worker made, which only this worker's
        // `OP_FREE_CLIENT_CERT` frees, and not during `f`, which gets a borrow.
        unsafe { cert.as_ref() }.map(|cert| f(cert.view()))
    }
}

/// Server-private shared table loans and bounds. The arena owns the underlying shared mappings.
///
/// Setup and teardown write the tables through `&`: every field they write is a cell
/// (a [`Shm`] is one), written while the master is the server's only thread.
pub struct ServerTables {
    /// `ConnectionRecord`s, `max_connection` of them.
    pub connection_list: Shm,
    /// `SessionRecord`s, `SW_SESSION_LIST_SIZE` of them.
    pub session_list: Shm,
    pub port_gs_list: Shm,
    /// How many records `connection_list` maps. Private: [`Self::set_max_connection`]
    /// changes it only while the list is unmapped, so [`Self::connections`] never
    /// reaches past the mapping.
    max_connection: Cell<u32>,
    /// Armed when accepting runs out of fds and cleared by its callback, at runtime.
    pub enable_accept_timer: swoole_rs_corebase::timer_key::AtomicHeld,
    pub reactor_pipe_num: Cell<u16>,
    /// How many records `reactor_threads` holds. Private with it: only [`Self::install_reactor_threads`] and
    /// [`Self::take_reactor_threads`] write the pair.
    reactor_thread_num: Cell<u16>,
    /// `ReactorThread[]`: null, or the array `install_reactor_threads` leaked.
    reactor_threads: Cell<*mut c_void>,
    /// `&Server::message_bus`, whose `MessageBus` starts with its Rust bus: the
    /// one `swr_v1_server_new` made, which the tables own.
    pub(crate) message_bus: Option<Box<crate::reactor_ops::PoolBus>>,
}

impl ServerTables {
    /// `max_connection`: how many connection slots the server has, or will have.
    #[must_use]
    pub fn max_connection(&self) -> u32 {
        self.max_connection.get()
    }

    /// `max_connection = value`, while the connection list is unmapped: whether it
    /// changed.
    pub(crate) fn set_max_connection(&self, value: u32) -> bool {
        let unmapped = self.connection_list.is_null();
        if unmapped {
            self.max_connection.set(value);
        }
        unmapped
    }

    /// The connection slots, exactly `max_connection` of them, or none before
    /// they're mapped. Index it with `get`, so every fd lookup is bounded by the
    /// table's own length.
    pub(crate) fn connections(&self) -> &[ConnectionRecord] {
        let base = self.connection_list.as_ptr();
        if base.is_null() {
            return &[];
        }
        // SAFETY: every factory maps the table with `max_connection` zeroed records
        // and frees it only by nulling `connection_list`; the record's cells and
        // atomics let slots be written under the server's own synchronization.
        unsafe { core::slice::from_raw_parts(base.cast(), self.max_connection() as usize) }
    }

    /// The slot of `fd`, or `None` when `fd` is outside the table.
    pub(crate) fn connection_at(&self, fd: c_int) -> Option<Connection<'_>> {
        Some(self.connections().get(usize::try_from(fd).ok()?)?.into())
    }

    /// Zeroes `fd`'s slot from after `active` up to `closed`, as upstream does on
    /// reuse. It writes through the table's own pointer: the padding between the
    /// cells is no cell, so a slot's shared borrow can't write it. Borrow the slot
    /// again afterwards.
    pub(crate) fn reset_connection(&self, fd: c_int) {
        let Some(index) = usize::try_from(fd)
            .ok()
            .filter(|&i| i < self.connections().len())
        else {
            return;
        };
        let start = core::mem::offset_of!(ConnectionRecord, socket_type);
        let record = self.connection_list.as_ptr().cast::<ConnectionRecord>();
        let bytes = record.wrapping_add(index).cast::<u8>().wrapping_add(start);
        // SAFETY: a slot of the mapped table, which `connections` bounds, written
        // through the mapping's own pointer.
        unsafe { bytes.write_bytes(0, core::mem::offset_of!(ConnectionRecord, closed) - start) };
    }

    /// Session slot `index` (below `SW_SESSION_LIST_SIZE`), or none before the table is
    /// mapped. Only that slot is borrowed: a unit test maps a smaller table.
    pub(crate) fn session_slot(&self, index: usize) -> Option<&SessionRecord> {
        let base = self.session_list.as_ptr().cast::<SessionRecord>();
        if base.is_null() {
            return None;
        }
        // SAFETY: a factory maps the table with 1,048,576 zeroed records, which the
        // modulo keeps `index` within, and frees it only by nulling `session_list`;
        // the record's fields are cells.
        Some(unsafe { &*base.wrapping_add(index) })
    }
}

#[cfg_attr(
    not(feature = "upstream-symbols"),
    expect(dead_code, reason = "only the hybrid's factories make reactor threads")
)]
impl ServerTables {
    /// `reactor_threads = threads`: the tables keep the array until
    /// [`Self::take_reactor_threads`]. One already kept is leaked, so a record read
    /// from it stays valid.
    pub(crate) fn install_reactor_threads(&self, threads: Box<[ThreadRecord]>) {
        let Ok(count) = u16::try_from(threads.len()) else {
            swoole_rs_corebase::fatal("more reactor threads than a reactor id counts");
        };
        self.reactor_threads.set(Box::into_raw(threads).cast());
        self.reactor_thread_num.set(count);
    }

    /// `delete[] reactor_threads`'s array, to drop: the tables forget it. Exclusive
    /// access means no record lent by [`Self::reactor_thread`] is still borrowed.
    pub(crate) fn take_reactor_threads(&mut self) -> Option<Box<[ThreadRecord]>> {
        let threads = self.reactor_threads.replace(core::ptr::null_mut());
        let count = usize::from(self.reactor_thread_num.take());
        if threads.is_null() {
            return None;
        }
        let threads = core::ptr::slice_from_raw_parts_mut(threads.cast::<ThreadRecord>(), count);
        // SAFETY: the box of `count` records `install_reactor_threads` leaked, which
        // the tables no longer name; only those two methods write the pair.
        Some(unsafe { Box::from_raw(threads) })
    }

    /// `serv->get_thread(id)`: reactor thread `id`'s record, or `None` past the last
    /// or before a factory made them.
    pub(crate) fn reactor_thread(&self, id: usize) -> Option<&ThreadRecord> {
        if id >= usize::from(self.reactor_thread_num.get()) {
            return None;
        }
        // SAFETY: within the live array of `reactor_thread_num` records
        // `install_reactor_threads` leaked, which is freed only through `&mut self`
        // (an install leaks the array it replaces).
        Some(unsafe {
            &*self
                .reactor_threads
                .get()
                .cast::<ThreadRecord>()
                .wrapping_add(id)
        })
    }
}

impl Default for ServerTables {
    fn default() -> Self {
        Self {
            connection_list: Shm::empty(),
            session_list: Shm::empty(),
            port_gs_list: Shm::empty(),
            max_connection: Cell::new(0),
            enable_accept_timer: swoole_rs_corebase::timer_key::AtomicHeld::new(),
            reactor_pipe_num: Cell::new(0),
            reactor_thread_num: Cell::new(0),
            reactor_threads: Cell::new(core::ptr::null_mut()),
            message_bus: None,
        }
    }
}

/// Which of upstream's `get_connection*` finds a connection slot for a key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lookup {
    /// `get_connection(fd)`: the fd's slot, active or not.
    ByFd,
    /// `get_connection_by_session_id(id)`: the session's slot, unverified.
    BySession,
    /// `get_connection_verify_no_ssl(id)`: the session's active slot.
    Verified,
    /// `get_connection_verify(id)`: as [`Lookup::Verified`], and TLS-ready.
    VerifiedTls,
    /// `get_connection_for_iterator(fd)`: an open, TLS-ready slot.
    Iterated,
}

impl crate::ServerState {
    /// `connection_list[fd].object = ports[index]`: the listening slot of `fd` holds
    /// the server's own port `index`, the one kind of slot object [`Connection::port`]
    /// reads. The slot, or `None` when either is out of range.
    pub(crate) fn hold_port(&self, index: usize, fd: c_int) -> Option<Connection<'_>> {
        let port = self.ports.get(index)?.raw();
        if port.addr().cast_signed() < 0 {
            swoole_rs_corebase::fatal("a port above the user address range");
        }
        let slot = self.tables_ref().connection_at(fd)?;
        slot.set_object(port);
        Some(slot)
    }

    /// The session-table slot of `id`, by the original shared-table modulo rule,
    /// or `None` before the table is mapped.
    pub(crate) fn session(&self, id: libc::c_long) -> Option<Session<'_>> {
        let index = usize::try_from(id % 1_048_576).ok()?;
        Some(self.tables_ref().session_slot(index)?.into())
    }

    /// The connection slot `kind` finds for `key`.
    pub(crate) fn connection(&self, key: libc::c_long, kind: Lookup) -> Option<Connection<'_>> {
        self.lookup(key, kind)
    }

    fn lookup(&self, key: libc::c_long, kind: Lookup) -> Option<Connection<'_>> {
        let tables = self.tables_ref();
        let mut session_id = 0;
        let fd = if matches!(
            kind,
            Lookup::BySession | Lookup::Verified | Lookup::VerifiedTls
        ) {
            // Session ids are positive; a negative key names no session.
            let session = self.session(key)?;
            session_id = session.id();
            session.fd()
        } else {
            // Native's parameter is int for fd-based lookups; preserve its low bits.
            c_int::from_ne_bytes(
                u32::try_from(key.cast_unsigned() & u64::from(u32::MAX))
                    .unwrap_or_default()
                    .to_ne_bytes(),
            )
        };
        let value = tables.connection_at(fd)?;
        let found = match kind {
            Lookup::ByFd | Lookup::BySession => true,
            _ if !value.is_active() => false,
            Lookup::Verified | Lookup::VerifiedTls
                if session_id != key || value.session_id() != key =>
            {
                false
            }
            Lookup::Iterated if value.closed().load(Relaxed) != 0 => false,
            Lookup::VerifiedTls | Lookup::Iterated => !value.ssl() || value.ssl_ready(),
            Lookup::Verified => true,
        };
        found.then_some(value)
    }

    /// Retire a dead base-mode worker's sessions, preserving IDs and reserved bits.
    pub(crate) fn retire_sessions(&self, worker_id: u32) {
        for id in 0..1_048_576 {
            if let Some(session) = self.session(id)
                && session.reactor_bits() & 0xff == worker_id
            {
                session.set_fd(0);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::TestServer;

    impl ServerTables {
        /// `max_connection = value` over a test's own tables, mapped or not.
        ///
        /// # Safety
        /// A mapped `connection_list` holds at least `value` records.
        pub(crate) unsafe fn set_max_connection_unchecked(&self, value: u32) {
            self.max_connection.set(value);
        }
    }

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
        let mut server = TestServer::new();
        let mut sessions: Vec<_> = (0..1_048_576)
            .map(|id| SessionRecord {
                id: Cell::new(id),
                fd: Cell::new(23),
                reactor_bits: Cell::new(0x5a12_3402),
            })
            .collect();
        sessions[0].reactor_bits.set(0x5a12_3401);
        sessions[1_048_575].reactor_bits.set(0x5a12_3401);
        server.tables_mut().session_list = naming(&mut sessions);
        server.state().retire_sessions(257);
        assert!(sessions.iter().all(|slot| slot.fd.get() == 23));
        server.state().retire_sessions(1);
        for (index, slot) in sessions.iter().enumerate() {
            let edge = index == 0 || index == 1_048_575;
            assert_eq!(slot.id.get(), i64::try_from(index).unwrap());
            assert_eq!(
                slot.reactor_bits.get(),
                if edge { 0x5a12_3401 } else { 0x5a12_3402 }
            );
            assert_eq!(slot.fd.get(), if edge { 0 } else { 23 });
        }
    }

    #[test]
    fn sessions_validate_reused_slots_tls_and_iteration() {
        let mut server = TestServer::new();
        let mut sessions = [
            SessionRecord {
                id: Cell::new(0),
                fd: Cell::new(0),
                reactor_bits: Cell::new(0),
            },
            SessionRecord {
                id: Cell::new(1),
                fd: Cell::new(2),
                reactor_bits: Cell::new(7),
            },
        ];
        let mut connections: [ConnectionRecord; 3] = unsafe { core::mem::zeroed() };
        connections[2].active.store(1, Relaxed);
        connections[2].session_id.set(1);
        let (session_at, connection_at) = (
            (&raw mut sessions).cast::<SessionRecord>(),
            (&raw mut connections).cast::<ConnectionRecord>(),
        );
        let tables = server.tables_mut();
        tables.session_list = naming_at(session_at);
        tables.connection_list = naming_at(connection_at);
        let state = server.state();
        unsafe {
            state.tables_ref().set_max_connection_unchecked(3);
            assert_eq!(
                state
                    .session(1)
                    .map_or(core::ptr::null_mut(), Session::as_ptr),
                session_at.add(1)
            );
            let expected = connection_at.add(2);
            assert_eq!(
                state
                    .connection(1, Lookup::Verified)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr),
                expected
            );
            assert!(
                state
                    .connection(1_048_577, Lookup::Verified)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            assert!(
                state
                    .connection(-1, Lookup::ByFd)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            assert!(
                state
                    .connection(-1, Lookup::BySession)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            assert!(
                state
                    .connection(-1, Lookup::Verified)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            // fd == max_connection is one past the table.
            assert!(
                state
                    .connection(3, Lookup::ByFd)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            (*expected).ssl.set(true);
            assert!(
                state
                    .connection(1, Lookup::VerifiedTls)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            assert_eq!(
                state
                    .connection(1, Lookup::Verified)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr),
                expected
            );
            (*expected).ssl_ready.set(true);
            assert_eq!(
                state
                    .connection(1, Lookup::VerifiedTls)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr),
                expected
            );
            (*expected)
                .closed
                .store(1, core::sync::atomic::Ordering::Relaxed);
            assert!(
                state
                    .connection(2, Lookup::Iterated)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
            (*expected)
                .closed
                .store(0, core::sync::atomic::Ordering::Relaxed);
            assert_eq!(
                state
                    .connection(2, Lookup::Iterated)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr),
                expected
            );
            (*expected).active.store(0, Relaxed);
            assert!(
                state
                    .connection(1, Lookup::Verified)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
        }
    }
}
