//! Server connections, sessions and lookup tables owned and operated by Rust.
use core::cell::Cell;
use core::ffi::{c_int, c_void};
use core::sync::atomic::{
    AtomicBool, AtomicU32,
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

/// A connection-table slot's record.
pub struct ConnectionRecord {
    /// Read by every reactor thread (`is_valid_connection` over the fd range).
    pub active: AtomicBool,
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
    /// address, a pipe end's pipe with [`NOT_PORT`] set, or a session's HTTP request
    /// with [`REQUEST`] set too. Private: only this module's typed setters write it.
    object: Cell<*mut c_void>,
    /// The session socket, `None` before the connection has one or after it closes: a
    /// record live until the unsafe free that ends the connection.
    pub socket: Cell<Option<swoole_rs_reactor::Heap<'static>>>,
    pub connect_time: Cell<f64>,
    pub last_recv_time: Cell<f64>,
    pub last_send_time: Cell<f64>,
    pub last_dispatch_time: Cell<f64>,
    pub uid: Cell<u32>,
    pub websocket_status: Cell<WebSocketStatus>,
    /// The WebSocket message being reassembled: a box only the connection's reactor
    /// makes, takes and frees.
    pub(crate) websocket_buffer: Cell<Option<Box<swoole_rs_corebase::buffer::string::CppString>>>,
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
        self.active.load(Acquire)
    }

    /// Upstream's `memset` of the slot from `socket_type` up to `closed`: every field
    /// but `active`, `closed` and `server_fd` back to its zero value.
    fn clear(&self) {
        self.socket_type.set(0);
        self.fd.set(0);
        self.worker_id.set(0);
        self.session_id.set(0);
        for flag in [
            &self.ssl,
            &self.ssl_ready,
            &self.overflow,
            &self.high_watermark,
            &self.http_upgrade,
            &self.http2_stream,
            &self.websocket_compression,
            &self.close_actively,
            &self.close_queued,
            &self.closing,
            &self.close_reset,
            &self.peer_closed,
            &self.protect,
            &self.close_notify,
            &self.close_force,
        ] {
            flag.set(false);
        }
        self.reactor_id.set(0);
        self.close_errno.set(0);
        self.recv_queued_bytes.store(0, Relaxed);
        self.send_queued_bytes.set(0);
        self.waiting_time.set(0);
        self.local_port.set(0);
        self.local_addr_index.set(0);
        self.timer.set(None);
        self.info.set(SocketAddress::default());
        self.object.set(core::ptr::null_mut());
        self.socket.set(None);
        for time in [
            &self.connect_time,
            &self.last_recv_time,
            &self.last_send_time,
            &self.last_dispatch_time,
        ] {
            time.set(0.0);
        }
        self.uid.set(0);
        self.websocket_status.set(WebSocketStatus::None);
        self.websocket_buffer.set(None);
        self.ssl_client_cert.set(core::ptr::null_mut());
        self.ssl_client_cert_pid.set(0);
        self.lock.store(0, Relaxed);
    }
}

/// The top address bit: set on every slot object but a port.
const NOT_PORT: usize = 1 << (usize::BITS - 1);
/// The low address bit, set beside [`NOT_PORT`] on an HTTP request
/// (`(address >> 1) | NOT_PORT | REQUEST`) and on nothing else.
const REQUEST: usize = 1;

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
    /// Resets the slot for a new connection: every field but `active`, `closed` and
    /// `server_fd` back to zero, as `Server::add_connection`'s `memset`.
    pub(crate) fn clear(self) {
        self.record.clear();
    }

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
    field!(socket, set_socket: Option<swoole_rs_reactor::Heap<'static>>);
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
    pub fn take_websocket_buffer(
        self,
    ) -> Option<Box<swoole_rs_corebase::buffer::string::CppString>> {
        self.record.websocket_buffer.take()
    }

    /// The connection reassembles `buffer`, which must hold none (one it held is
    /// freed).
    pub fn set_websocket_buffer(self, buffer: Box<swoole_rs_corebase::buffer::string::CppString>) {
        if let Some(mut held) = self.record.websocket_buffer.replace(Some(buffer)) {
            held.dispose();
        }
    }

    /// `OP_STORE_CLIENT_CERT`: `cert` is the connection's client certificate, which
    /// only this worker reads and frees.
    pub fn store_client_cert(self, cert: swoole_rs_corebase::buffer::string::CppString) {
        self.set_ssl_client_cert(Box::into_raw(Box::new(cert)).cast());
        self.set_ssl_client_cert_pid(swoole_rs_process::methods::swoole_get_worker_pid());
    }

    /// `OP_FREE_CLIENT_CERT`: the certificate this worker stored, taken from the
    /// connection, or `None` when this worker stored none.
    #[must_use]
    pub fn take_client_cert(self) -> Option<Box<swoole_rs_corebase::buffer::string::CppString>> {
        let cert = self
            .ssl_client_cert()
            .cast::<swoole_rs_corebase::buffer::string::CppString>();
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
        // SAFETY: a positive object is a port: everything else is null or has
        // `NOT_PORT` set, and only `hold_port` stores one, from the port list of the
        // server whose table holds the slot. The server frees its ports only with that table, so
        // the port lives as long as the slot's borrow.
        unsafe { raw.cast::<crate::port_config::PortConfig>().as_ref() }
            .map(crate::view::Port::from)
    }

    /// The HTTP request the session holds, which the slot no longer names, or `None`.
    #[must_use]
    #[inline]
    pub fn take_http_request(self) -> Option<Box<swoole_rs_http_ffi::HttpRequest>> {
        const TAG: usize = NOT_PORT | REQUEST;
        let raw = self.object();
        if raw.addr() & TAG != TAG {
            return None;
        }
        self.clear_object();
        let request = raw.map_addr(|a| (a & !TAG) << 1).cast();
        // SAFETY: only `hold_http_request` sets `REQUEST`, on the box it leaked, and
        // this take clears it, so each box comes back once.
        Some(unsafe { Box::from_raw(request) })
    }

    /// The session holds `request` until [`Self::take_http_request`].
    #[inline]
    pub fn hold_http_request(self, request: Box<swoole_rs_http_ffi::HttpRequest>) {
        const {
            assert!(
                align_of::<swoole_rs_http_ffi::HttpRequest>() >= 4,
                "the tag takes the low bit after the shift"
            );
        }
        let raw = Box::into_raw(request).cast::<c_void>();
        self.set_object(raw.map_addr(|a| (a >> 1) | NOT_PORT | REQUEST));
    }

    /// `Server::store_pipe_fd`: a pipe end's slot names its pipe, which nothing reads
    /// back as a port or a request. An address with the top bit set, which no
    /// user-space pointer has, is not kept; a pipe is 2-aligned, so its low bit is
    /// clear.
    pub(crate) fn set_pipe(self, pipe: *mut c_void) {
        let kept = pipe.addr().cast_signed() >= 0;
        self.set_object(if kept {
            pipe.map_addr(|a| (a | NOT_PORT) & !REQUEST)
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

    field!(atomic active: AtomicBool);
    field!(atomic recv_queued_bytes: AtomicU32);
    field!(atomic lock: AtomicU32);
    field!(atomic closed: AtomicU32);

    /// Whether the slot holds a connection (`active`), whose fields `add` published.
    #[must_use]
    pub fn is_active(self) -> bool {
        self.active().load(Acquire)
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
            .cast::<swoole_rs_corebase::buffer::string::CppString>();
        // SAFETY: null, or the certificate this worker made, which only this worker's
        // `OP_FREE_CLIENT_CERT` frees, and not during `f`, which gets a borrow.
        unsafe { cert.as_ref() }.map(|cert| f(cert.view()))
    }
}

/// Server-private shared table loans and bounds. The arena owns the underlying shared mappings.
///
/// Setup writes the tables through `&`: every field it writes is a cell (a [`Shm`] is
/// one), written while the master is the server's only thread. Each [`Shm`] is mapped
/// once through `&`, and only [`Self::unmap`], through `&mut`, unmaps them, so nothing
/// lent out of one outlives its mapping. The two lists are private, so that the tables
/// bound the slots they lend.
pub struct ServerTables {
    /// `ConnectionRecord`s, `max_connection` of them.
    connection_list: Shm,
    /// `SessionRecord`s, `SW_SESSION_LIST_SIZE` of them.
    session_list: Shm,
    pub port_gs_list: Shm,
    /// `WorkerRecord`s, `worker_num` of them.
    pub(crate) workers: Shm,
    /// A `WorkerRecord` per added user worker.
    pub(crate) user_workers: Shm,
    /// Each event worker's result slot, which its tasks' results fill.
    pub(crate) task_results: Shm,
    /// Base mode's server connection counts (`gs->connection_nums`): an `AtomicU32`
    /// per worker.
    pub(crate) connection_nums: Shm,
    /// How many records `connection_list` maps. Private: [`Self::set_max_connection`]
    /// changes it only while the list is unmapped, so [`Self::connections`] never
    /// reaches past the mapping.
    max_connection: Cell<u32>,
    /// Armed when accepting runs out of fds and cleared by its callback, at runtime.
    pub enable_accept_timer: swoole_rs_corebase::timer_key::AtomicHeld,
    pub reactor_pipe_num: Cell<u16>,
    /// `ReactorThread[]`: the array [`Self::install_reactor_threads`] gave, until
    /// [`Self::take_reactor_threads`].
    reactor_threads: core::cell::OnceCell<Box<[ThreadRecord]>>,
    /// `&Server::message_bus`, whose `MessageBus` starts with its Rust bus: the
    /// one `swr_v1_server_new` made, which the tables own.
    #[cfg(feature = "upstream-symbols")]
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
        let unmapped = !self.connections_mapped();
        if unmapped {
            self.max_connection.set(value);
        }
        unmapped
    }

    /// Whether a factory has mapped the connection list.
    pub(crate) fn connections_mapped(&self) -> bool {
        !self.connection_list.is_null()
    }

    /// Whether `Server::create` has mapped the session list.
    pub(crate) fn sessions_mapped(&self) -> bool {
        !self.session_list.is_null()
    }

    /// `connection_list = list`: a factory's `max_connection` zeroed records. A list
    /// already mapped stays, as its slots may be lent, and `list` is unmapped.
    pub(crate) fn map_connections(&self, list: Shm) {
        self.connection_list.fill(list);
    }

    /// `session_list = list`: `SW_SESSION_LIST_SIZE` zeroed records, as
    /// [`Self::map_connections`].
    pub(crate) fn map_sessions(&self, list: Shm) {
        self.session_list.fill(list);
    }

    /// `session_list`'s `SharedMemory::protect(flags)`.
    #[cfg_attr(
        not(feature = "upstream-symbols"),
        expect(dead_code, reason = "only the hybrid's event workers protect it")
    )]
    pub(crate) fn protect_sessions(&self, flags: c_int) -> std::io::Result<()> {
        self.session_list.protect(flags)
    }

    /// `sw_shm_free` of each table: exclusive access means nothing lent out of one is
    /// still borrowed.
    pub(crate) fn unmap(&mut self) {
        for table in [
            &mut self.connection_list,
            &mut self.session_list,
            &mut self.port_gs_list,
            &mut self.workers,
            &mut self.user_workers,
            &mut self.task_results,
            &mut self.connection_nums,
        ] {
            table.free();
        }
    }

    /// The connection slots, exactly `max_connection` of them, or none before
    /// they're mapped. Index it with `get`, so every fd lookup is bounded by the
    /// table's own length.
    pub(crate) fn connections(&self) -> &[ConnectionRecord] {
        let base = self.connection_list.as_ptr();
        if base.is_null() {
            return &[];
        }
        // SAFETY: every factory maps the table with `max_connection` zeroed records,
        // and only `unmap`, through `&mut self`, unmaps it; the record's cells
        // and atomics let slots be written under the server's own synchronization.
        unsafe { core::slice::from_raw_parts(base.cast(), self.max_connection() as usize) }
    }

    /// The slot of `fd`, or `None` when `fd` is outside the table.
    pub(crate) fn connection_at(&self, fd: c_int) -> Option<Connection<'_>> {
        Some(self.connections().get(usize::try_from(fd).ok()?)?.into())
    }

    /// Resets `fd`'s slot for reuse: every field but `active`, `closed` and
    /// `server_fd` back to zero, as upstream's `memset` up to `closed`.
    pub(crate) fn reset_connection(&self, fd: c_int) {
        if let Some(conn) = self.connection_at(fd) {
            conn.clear();
        }
    }

    /// Zeroes all of `fd`'s slot, as a DTLS session that fails its setup leaves it.
    /// It writes through the table's own pointer: the padding between the cells is no
    /// cell, so a slot's shared borrow can't write it. Borrow the slot again afterwards.
    #[cfg(any(test, feature = "upstream-symbols"))]
    pub(crate) fn clear_connection(&self, fd: c_int) {
        let Some(index) = usize::try_from(fd)
            .ok()
            .filter(|&i| i < self.connections().len())
        else {
            return;
        };
        let record = self.connection_list.as_ptr().cast::<ConnectionRecord>();
        // SAFETY: a slot of the mapped table, which `connections` bounds, written
        // through the mapping's own pointer.
        unsafe { record.wrapping_add(index).write_bytes(0, 1) };
    }

    /// Session slot `index` (below `SW_SESSION_LIST_SIZE`), or none before the table is
    /// mapped. Only that slot is borrowed: a unit test maps a smaller table.
    pub(crate) fn session_slot(&self, index: usize) -> Option<&SessionRecord> {
        let base = self.session_list.as_ptr().cast::<SessionRecord>();
        if base.is_null() {
            return None;
        }
        // SAFETY: a factory maps the table with 1,048,576 zeroed records, which the
        // modulo keeps `index` within, and only `unmap`, through `&mut self`,
        // unmaps it; the record's fields are cells.
        Some(unsafe { &*base.wrapping_add(index) })
    }
}

#[cfg_attr(
    not(feature = "upstream-symbols"),
    expect(dead_code, reason = "only the hybrid's factories make reactor threads")
)]
impl ServerTables {
    /// `reactor_threads = threads`: the tables keep the array until
    /// [`Self::take_reactor_threads`]. A factory makes one per create, which its
    /// destroy takes, so a second is a bug, which aborts.
    pub(crate) fn install_reactor_threads(&self, threads: Box<[ThreadRecord]>) {
        if u16::try_from(threads.len()).is_err() {
            swoole_rs_corebase::fatal("more reactor threads than a reactor id counts");
        }
        if self.reactor_threads.set(threads).is_err() {
            swoole_rs_corebase::fatal("a server's reactor threads made twice");
        }
    }

    /// `delete[] reactor_threads`'s array, to drop: the tables forget it. Exclusive
    /// access means no record lent by [`Self::reactor_thread`] is still borrowed.
    pub(crate) fn take_reactor_threads(&mut self) -> Option<Box<[ThreadRecord]>> {
        self.reactor_threads.take()
    }

    /// `serv->get_thread(id)`: reactor thread `id`'s record, or `None` past the last
    /// or before a factory made them.
    pub(crate) fn reactor_thread(&self, id: usize) -> Option<&ThreadRecord> {
        self.reactor_threads.get()?.get(id)
    }
}

impl Default for ServerTables {
    fn default() -> Self {
        Self {
            connection_list: Shm::empty(),
            session_list: Shm::empty(),
            port_gs_list: Shm::empty(),
            workers: Shm::empty(),
            user_workers: Shm::empty(),
            task_results: Shm::empty(),
            connection_nums: Shm::empty(),
            max_connection: Cell::new(0),
            enable_accept_timer: swoole_rs_corebase::timer_key::AtomicHeld::new(),
            reactor_pipe_num: Cell::new(0),
            reactor_threads: core::cell::OnceCell::new(),
            #[cfg(feature = "upstream-symbols")]
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

        /// The connection list, to name a test's own records or forget them.
        pub(crate) fn connection_list_mut(&mut self) -> &mut Shm {
            &mut self.connection_list
        }

        /// The session list, as [`Self::connection_list_mut`].
        pub(crate) fn session_list_mut(&mut self) -> &mut Shm {
            &mut self.session_list
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
    fn clearing_a_slot_keeps_only_active_closed_and_server_fd() {
        // SAFETY: a connection record is plain data; zero is a closed one.
        let record: ConnectionRecord = unsafe { core::mem::zeroed() };
        let conn = Connection::from(&record);
        record.active.store(true, Relaxed);
        record.closed.store(1, Relaxed);
        conn.set_server_fd(3);
        conn.set_fd(4);
        conn.set_session_id(5);
        conn.set_close_force(true);
        conn.set_last_recv_time(1.5);
        conn.set_uid(6);
        conn.set_websocket_status(WebSocketStatus::Active);
        conn.set_ssl_client_cert_pid(7);
        record.lock.store(1, Relaxed);
        conn.clear();
        assert!(conn.is_active());
        assert_eq!(record.closed.load(Relaxed), 1);
        assert_eq!(conn.server_fd(), 3);
        assert_eq!((conn.fd(), conn.session_id(), conn.uid()), (0, 0, 0));
        assert!(!conn.close_force());
        assert_eq!(conn.last_recv_time().to_bits(), 0);
        assert_eq!(conn.websocket_status(), WebSocketStatus::None);
        assert_eq!(conn.ssl_client_cert_pid(), 0);
        assert_eq!(record.lock.load(Relaxed), 0);
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
        *server.tables_mut().session_list_mut() = naming(&mut sessions);
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
        connections[2].active.store(true, Relaxed);
        connections[2].session_id.set(1);
        let (session_at, connection_at) = (
            (&raw mut sessions).cast::<SessionRecord>(),
            (&raw mut connections).cast::<ConnectionRecord>(),
        );
        let tables = server.tables_mut();
        *tables.session_list_mut() = naming_at(session_at);
        *tables.connection_list_mut() = naming_at(connection_at);
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
            (*expected).active.store(false, Relaxed);
            assert!(
                state
                    .connection(1, Lookup::Verified)
                    .map_or(core::ptr::null_mut(), Connection::as_ptr)
                    .is_null()
            );
        }
    }

    /// A list is mapped once through `&` and unmapped only through `&mut`: a second
    /// mapping leaves the first, so a slot lent from it stays usable. Mapping by
    /// replacing unmapped it under the lent slot, which then reached freed memory.
    #[test]
    fn a_lent_slot_outlives_a_second_mapping() {
        let mut tables = ServerTables::default();
        assert!(tables.set_max_connection(2));
        let connections = || Shm::calloc_private(2, size_of::<ConnectionRecord>());
        let sessions = || Shm::calloc_private(2, size_of::<SessionRecord>());
        tables.map_connections(connections());
        tables.map_sessions(sessions());
        let (slot, session) = (&tables.connections()[1], tables.session_slot(1).unwrap());
        tables.map_connections(connections());
        tables.map_sessions(sessions());
        slot.fd.set(7);
        session.fd.set(9);
        assert_eq!(tables.connections()[1].fd.get(), 7, "the first list stays");
        assert_eq!(
            tables.session_slot(1).unwrap().fd.get(),
            9,
            "so do the sessions"
        );
        assert!(!tables.set_max_connection(3), "fixed while mapped");
        tables.unmap();
        assert!(tables.connections().is_empty(), "unmapped");
        assert!(tables.session_slot(1).is_none(), "sessions unmapped");
        assert!(tables.set_max_connection(3), "free to change again");
    }

    /// A reset keeps a slot's `active` and `closed` and zeroes what lies between; a
    /// clear zeroes all of it; neither reaches outside the table.
    #[test]
    fn reset_and_clear_zero_their_part_of_one_slot() {
        let mut server = TestServer::new();
        // SAFETY: a connection record is plain data; zero is a closed one.
        let mut connections: [ConnectionRecord; 3] = unsafe { core::mem::zeroed() };
        let at = (&raw mut connections).cast::<ConnectionRecord>();
        *server.tables_mut().connection_list_mut() = naming_at(at);
        let tables = server.state().tables_ref();
        // SAFETY: the table above holds 3 records.
        unsafe { tables.set_max_connection_unchecked(3) };
        let fill = |slot: &ConnectionRecord| {
            slot.active.store(true, Relaxed);
            slot.fd.set(7);
            slot.reactor_id.set(2);
            slot.closed.store(1, Relaxed);
        };
        let read = |index: usize| {
            // SAFETY: a record of the table, read afresh after each write.
            let slot = unsafe { &*at.add(index) };
            (
                slot.active.load(Relaxed),
                slot.fd.get(),
                slot.reactor_id.get(),
                slot.closed.load(Relaxed),
            )
        };
        for index in 0..3 {
            // SAFETY: as above.
            fill(unsafe { &*at.add(index) });
        }
        tables.reset_connection(1);
        tables.clear_connection(2);
        tables.reset_connection(3);
        tables.clear_connection(-1);
        assert_eq!(read(0), (true, 7, 2, 1), "untouched");
        assert_eq!(read(1), (true, 0, 0, 1), "reset");
        assert_eq!(read(2), (false, 0, 0, 0), "cleared");
    }
}
