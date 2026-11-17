//! A live server for tests: the records an operation reads and writes, built by the
//! code that builds them in a running server, so a test checks what the operation
//! did to them rather than which ops it asked for.
use core::ffi::c_int;
use std::net::{SocketAddr, TcpListener};
use std::os::fd::IntoRawFd;
use swoole_rs_corebase::constants::SocketTypeCode;

use swoole_rs_corebase::reactor::{Socket, SocketRecord};

use crate::port_config::PortConfig;
use crate::records::tests::naming_at;
use crate::records::{ConnectionRecord, ServerTables, SessionRecord};
use crate::shared::{PortCounters, ServerCounters, ServerShared};
use crate::{Mode, ServerState, TestServer, view};
use std::rc::Rc;

/// Connection slots: fds below this are accepted. Miri's descriptors stay small, and it
/// takes ~5 ms a slot to set the table up.
pub(crate) const CONNECTIONS: usize = if cfg!(miri) { 256 } else { 4096 };
/// `SW_SESSION_LIST_SIZE`. Miri's session ids stay small too, and it retags the whole
/// table at each lookup: a minute a test with a million slots.
const SESSIONS: usize = if cfg!(miri) { CONNECTIONS } else { 1 << 20 };
/// `SW_SOCK_TCP` and `SW_FD_STREAM_SERVER`.
const SOCK_TCP: c_int = swoole_rs_corebase::constants::SocketType::SW_SOCK_TCP.raw();
const FD_STREAM_SERVER: c_int = 1;

/// A process-mode server with one reactor thread, its tables, shared counters and
/// pools, one TCP port listening on 127.0.0.1, and this thread's reactor.
pub(crate) struct LiveServer {
    server: TestServer,
    // `Rc`s, not `Box`es: moving a `Box` asserts it unique, which would leave the
    // state's pointers to it stale.
    shared: Rc<ServerShared>,
    port_counters: Rc<PortCounters>,
    listener: *mut SocketRecord,
    /// This thread's reactor, which the fixture made.
    reactor: Option<swoole_rs_corebase::reactor::OwnedReactor>,
    /// The listening socket's fd.
    pub listener_fd: c_int,
    /// The port's address.
    pub address: SocketAddr,
    // The tables the state names; only reached through it once it is built.
    _sessions: Vec<SessionRecord>,
    _connections: Vec<ConnectionRecord>,
}

/// `count` zeroed records: a table of free slots, zeroed in one allocation.
fn free_slots<T>(count: usize) -> Vec<T> {
    // SAFETY: the tables' records are plain data, and zero is a free slot.
    unsafe { Box::<[T]>::new_zeroed_slice(count).assume_init() }.into_vec()
}

impl LiveServer {
    pub(crate) fn new() -> Self {
        let mut sessions: Vec<SessionRecord> = free_slots(SESSIONS);
        let mut connections: Vec<ConnectionRecord> = free_slots(CONNECTIONS);
        let mut tables = ServerTables::default();
        *tables.session_list_mut() = unsafe { naming_at(sessions.as_mut_ptr(), SESSIONS) };
        *tables.connection_list_mut() = unsafe { naming_at(connections.as_mut_ptr(), CONNECTIONS) };
        // SAFETY: the table just named holds `CONNECTIONS` records.
        unsafe {
            tables.set_max_connection_unchecked(u32::try_from(CONNECTIONS).unwrap_or(u32::MAX));
        };
        let mut server = TestServer::with_tables(tables);
        // SAFETY: the shared record is plain data, for which zero is the state
        // `swr_v1_server_new`'s calloc leaves it in.
        let shared: Rc<ServerShared> = Rc::new(unsafe { core::mem::zeroed() });
        let port_counters = Rc::new(PortCounters::default());
        let config = server.config_mut();
        config.mode_ = Mode::Process;
        config.reactor_num.set(1);
        config.gs = unsafe { naming_at(Rc::as_ptr(&shared).cast_mut(), 1) };
        config.worker_registry = Some(Box::default());

        let listening = TcpListener::bind("127.0.0.1:0").unwrap();
        listening.set_nonblocking(true).unwrap();
        let address = listening.local_addr().unwrap();
        let fd = listening.into_raw_fd();
        let mut settings = PortConfig::boxed(c_int::MAX);
        let mut record = SocketRecord::with_fd(fd, FD_STREAM_SERVER);
        record.socket_type = SOCK_TCP;
        record.output.nonblock = true;
        record.object = (&raw mut *settings).cast();
        let listener = record.into_raw();
        settings.r#type = SOCK_TCP;
        // SAFETY: the record just boxed, which the port frees with the state.
        let socket = unsafe { Socket::from_ptr(listener) }.unwrap();
        settings.set_listening_socket(unsafe {
            swoole_rs_corebase::reactor::OwnedSocket::from_socket(socket)
        });
        settings
            .gs
            .set(Rc::as_ptr(&port_counters).cast_mut().cast());
        let state = server.state();
        let _ = state.ports.push(settings);
        // The listening fd's slot holds its port, as `Server::store_listen_socket` leaves it.
        let slot = state.hold_port(0, fd).unwrap();
        slot.set_fd(fd);
        slot.set_socket_type(SocketTypeCode(SOCK_TCP));

        // The thread's reactor, with corebase's socket I/O and without coroutine/client handlers.
        let reactor = swoole_rs_corebase::reactor::swr_v1_reactor_new(4096, 0);
        swoole_rs_corebase::set_thread_reactor(reactor.as_ptr());
        Self {
            server,
            shared,
            port_counters,
            listener,
            reactor: Some(reactor),
            listener_fd: fd,
            address,
            _sessions: sessions,
            _connections: connections,
        }
    }

    pub(crate) fn server(&self) -> view::Server<'_> {
        self.server.serv()
    }

    /// The state forgets the fixture's session table, and its connection table too
    /// when `connections`, for a test whose `Server::create` (and base factory) maps the
    /// server's own: their replacement would unmap the fixture's vectors as though a
    /// mapping's header preceded them.
    pub(crate) fn forget_tables(&mut self, connections: bool) {
        // A `Shm` unmaps only when told to: overwriting one forgets it.
        let tables = self.server.tables_mut();
        *tables.session_list_mut() = crate::table::Table::empty();
        if connections {
            *tables.connection_list_mut() = crate::table::Table::empty();
        }
    }

    pub(crate) fn state(&self) -> &ServerState {
        self.server.state()
    }

    /// The configuration, to write.
    pub(crate) fn config_mut(&mut self) -> &mut crate::ServerConfig {
        self.server.config_mut()
    }

    /// The tables, to write.
    pub(crate) fn tables_mut(&mut self) -> &mut crate::records::ServerTables {
        self.server.tables_mut()
    }

    pub(crate) fn counters(&self) -> &ServerCounters {
        &self.shared.counters
    }

    pub(crate) fn port_counters(&self) -> &PortCounters {
        &self.port_counters
    }

    /// The read event of the port's listening socket, as the reactor hands `accept` it.
    pub(crate) fn accept_event(&self) -> swoole_rs_corebase::reactor::Event {
        // SAFETY: the fixture's listener, live until the fixture drops.
        let listener = unsafe { swoole_rs_corebase::reactor::Socket::from_ptr(self.listener) }
            .map(swoole_rs_corebase::reactor::Heap::from_static);
        swoole_rs_corebase::reactor::Event::of(self.listener_fd, 0, FD_STREAM_SERVER, listener)
    }

    /// A client connected to the port, once the listener can accept it. Readiness
    /// shows only that some connection is pending, so the listener must have none
    /// before: accept each client before connecting the next.
    pub(crate) fn connect(&self) -> std::net::TcpStream {
        assert!(!self.acceptable(0), "a connection is already pending");
        let client = std::net::TcpStream::connect(self.address).unwrap();
        assert!(self.acceptable(5000), "acceptable");
        client
    }

    /// Whether the listener has a connection to accept within `timeout` milliseconds.
    fn acceptable(&self, timeout: c_int) -> bool {
        let mut ready = libc::pollfd {
            fd: self.listener_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one live pollfd.
        unsafe { libc::poll(&raw mut ready, 1, timeout) == 1 }
    }

    /// The listening socket's handle.
    pub(crate) fn port_socket(&self) -> swoole_rs_corebase::reactor::Heap<'static> {
        // SAFETY: the listener this fixture owns, which the port frees through its owner.
        swoole_rs_corebase::reactor::Heap::from_static(
            unsafe { swoole_rs_corebase::reactor::Socket::from_ptr(self.listener) }.unwrap(),
        )
    }

    /// The port's settings.
    pub(crate) fn port(&self) -> &PortConfig {
        self.state().ports.get(0).unwrap().config()
    }

    /// This thread's reactor, which the fixture made.
    pub(crate) fn reactor(&self) -> crate::reactor_handle::Reactor {
        let made = self.reactor.as_ref();
        let made = made.unwrap_or_else(|| panic!("the fixture holds its reactor until it drops"));
        crate::reactor_handle::Reactor::running(made.running())
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        swoole_rs_corebase::set_thread_reactor(core::ptr::null_mut());
        if let Some(reactor) = self.reactor.take() {
            swoole_rs_corebase::reactor::swr_v1_reactor_delete(reactor);
        }
        // SAFETY: the listener this fixture opened; its record goes with its port.
        unsafe { libc::close(self.listener_fd) };
        // The configuration's `gs` names the fixture's own record, which the state's drop
        // must not unmap.
        self.config_mut().gs = crate::table::Table::empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering::Relaxed;

    #[test]
    fn accepts_a_real_connection_into_the_tables_and_counters() {
        let live = LiveServer::new();
        let client = live.connect();
        let fd = live.listener_fd;
        let event = live.accept_event();
        // The batch ends at EAGAIN.
        crate::connection::accept(live.server(), live.reactor(), &event);

        let session = live.state().session(1).unwrap();
        let accepted = session.fd();
        assert!(
            accepted > 0 && accepted != fd,
            "session 1 holds the accepted fd"
        );
        assert_eq!(session.id(), 1, "the first session id");
        let conn = live.state().tables_ref().connection_at(accepted).unwrap();
        assert_eq!(conn.session_id(), 1, "the connection is bound to it");
        let gs = live.counters();
        assert_eq!(gs.session_round.load(Relaxed), 1, "the round moved on");
        assert_eq!(gs.accept_count.load(Relaxed), 1, "one accept");
        assert_eq!(gs.connection_num.load(Relaxed), 1, "one connection");
        assert_eq!(gs.max_fd.load(Relaxed), accepted, "the fd range grew");
        let port = live.port_counters();
        assert_eq!(
            (
                port.accept_count.load(Relaxed),
                port.connection_num.load(Relaxed)
            ),
            (1, 1),
            "the port counts it too"
        );
        // The peer's local address is the accepted socket's remote one.
        // SAFETY: the accepted fd, which the test closes once.
        let peer = unsafe {
            let mut addr: libc::sockaddr_in = core::mem::zeroed();
            let mut len = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>()).unwrap();
            libc::getpeername(accepted, (&raw mut addr).cast(), &raw mut len);
            libc::close(accepted);
            u16::from_be(addr.sin_port)
        };
        assert_eq!(
            peer,
            client.local_addr().unwrap().port(),
            "the client's connection"
        );
    }
}
