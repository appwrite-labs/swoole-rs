//! A live server for tests: the records an operation reads and writes, built by the
//! code that builds them in a running server, so a test checks what the operation
//! did to them rather than which ops it asked for.
use core::ffi::c_int;
use std::net::{SocketAddr, TcpListener};
use std::os::fd::IntoRawFd;

use swoole_rs_reactor::{Socket, SocketRecord};

use crate::port_config::PortConfig;
use crate::records::tests::naming_at;
use crate::records::{ConnectionRecord, ServerTables, SessionRecord};
use crate::shared::{PortCounters, ServerCounters, ServerShared};
use crate::view::Fields;
use crate::{Mode, ServerState, view};

/// `SW_SESSION_LIST_SIZE`.
const SESSIONS: usize = 1 << 20;
/// Connection slots: fds below this are accepted.
pub(crate) const CONNECTIONS: usize = 4096;
/// `SW_SOCK_TCP` and `SW_FD_STREAM_SERVER`.
const SOCK_TCP: c_int = 1;
const FD_STREAM_SERVER: c_int = 1;

/// A process-mode server with one reactor thread, its tables, shared counters and
/// pools, one TCP port listening on 127.0.0.1, and this thread's reactor.
pub(crate) struct LiveServer {
    head: Box<Fields>,
    state: Box<ServerState>,
    shared: Box<ServerShared>,
    port_counters: Box<PortCounters>,
    port: *mut PortConfig,
    listener: *mut SocketRecord,
    /// This thread's reactor, which the fixture made.
    reactor: *mut core::ffi::c_void,
    /// The listening socket's fd.
    pub listener_fd: c_int,
    /// The port's address.
    pub address: SocketAddr,
    // The tables the state names; only reached through it once it is built.
    _sessions: Vec<SessionRecord>,
    _connections: Vec<ConnectionRecord>,
}

/// `count` zeroed records: a table of free slots.
fn free_slots<T>(count: usize) -> Vec<T> {
    // SAFETY: the tables' records are plain data, and zero is a free slot.
    (0..count).map(|_| unsafe { core::mem::zeroed() }).collect()
}

impl LiveServer {
    pub(crate) fn new() -> Self {
        let mut sessions: Vec<SessionRecord> = free_slots(SESSIONS);
        let mut connections: Vec<ConnectionRecord> = free_slots(CONNECTIONS);
        let mut tables = ServerTables::default();
        tables.session_list = naming_at(sessions.as_mut_ptr());
        tables.connection_list = naming_at(connections.as_mut_ptr());
        tables.max_connection = u32::try_from(CONNECTIONS).unwrap_or(u32::MAX);
        let state = Box::new(ServerState::with_tables(tables));
        // SAFETY: the shared record is plain data, for which zero is the state
        // `swr_v1_server_new`'s calloc leaves it in.
        let mut shared: Box<ServerShared> = Box::new(unsafe { core::mem::zeroed() });
        let mut port_counters = Box::new(PortCounters::default());
        // SAFETY: the state's own configuration, which nothing else reads yet.
        let config = unsafe { &mut *state.config() };
        config.mode_ = Mode::Process;
        config.reactor_num.set(1);
        config.gs = naming_at(&raw mut *shared);
        config.worker_registry = Some(Box::default());

        let listening = TcpListener::bind("127.0.0.1:0").unwrap();
        listening.set_nonblocking(true).unwrap();
        let address = listening.local_addr().unwrap();
        let fd = listening.into_raw_fd();
        let port = Box::into_raw(PortConfig::boxed(c_int::MAX));
        let mut record = SocketRecord::with_fd(fd, FD_STREAM_SERVER);
        record.socket_type = SOCK_TCP;
        record.output.nonblock = true;
        record.object = port.cast();
        let listener = record.into_raw();
        // SAFETY: the port just boxed, which only this fixture holds.
        let settings = unsafe { &mut *port };
        settings.r#type = SOCK_TCP;
        // SAFETY: the record just boxed, which the port frees with the state.
        let socket = unsafe { Socket::from_ptr(listener) }.unwrap();
        settings.set_listening_socket(socket);
        settings.gs.set((&raw mut *port_counters).cast());
        // SAFETY: the port boxed above, listed until the fixture frees it with the state;
        // no other thread reads the list.
        let listed = unsafe { state.push_port(Box::from_raw(port)) };
        // The listening fd's slot holds its port, as `Server::store_listen_socket` leaves it.
        let slot = state.tables_ref().connection_at(fd).unwrap();
        slot.set_fd(fd);
        slot.set_socket(Some(socket));
        slot.set_socket_type(SOCK_TCP);
        slot.set_object(port.cast());
        slot.listen(listed);

        let head = Box::new(Fields::of(&state));
        // The thread's reactor, without `swoole_event_init`'s coroutine and client handlers.
        let reactor = swoole_rs_reactor::swr_v1_reactor_new(4096, 0);
        swoole_rs_corebase::set_thread_reactor(reactor);
        Self {
            head,
            state,
            shared,
            port_counters,
            port,
            listener,
            reactor,
            listener_fd: fd,
            address,
            _sessions: sessions,
            _connections: connections,
        }
    }

    pub(crate) fn server(&self) -> view::Server<'_> {
        // SAFETY: the head of a live server, which this fixture owns for the borrow.
        unsafe { view::Server::from_raw((&raw const *self.head).cast_mut().cast()) }
    }

    pub(crate) fn state(&self) -> &ServerState {
        &self.state
    }

    pub(crate) fn counters(&self) -> &ServerCounters {
        &self.shared.counters
    }

    pub(crate) fn port_counters(&self) -> &PortCounters {
        &self.port_counters
    }

    /// The port's listening socket.
    pub(crate) fn listener(&self) -> *mut SocketRecord {
        self.listener
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
    pub(crate) fn port_socket(&self) -> swoole_rs_reactor::Socket<'_> {
        // SAFETY: the listener this fixture owns for the borrow.
        unsafe { swoole_rs_reactor::Socket::from_ptr(self.listener) }.unwrap()
    }

    /// The port's settings.
    pub(crate) fn port(&self) -> &PortConfig {
        // SAFETY: the port this fixture boxed and owns for the borrow.
        unsafe { &*self.port }
    }

    /// This thread's reactor, which the fixture made.
    pub(crate) fn reactor(&self) -> crate::reactor_handle::Reactor {
        // SAFETY: the reactor this fixture made, live until it drops, on this thread.
        unsafe { crate::reactor_handle::Reactor::of(self.reactor) }
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        swoole_rs_corebase::set_thread_reactor(core::ptr::null_mut());
        // SAFETY: the reactor `new` made, which nothing uses afterwards.
        unsafe { swoole_rs_reactor::swr_v1_reactor_delete(self.reactor) };
        // SAFETY: the listener this fixture opened and boxed, used by nothing after it.
        unsafe {
            libc::close(self.listener_fd);
            SocketRecord::free(self.listener);
        }
        // SAFETY: the port this fixture boxed; the state that lists it drops with it.
        drop(unsafe { Box::from_raw(self.port) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering::Relaxed;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn accepts_a_real_connection_into_the_tables_and_counters() {
        let live = LiveServer::new();
        let client = live.connect();
        let listener = live.listener();
        let fd = live.listener_fd;
        // SAFETY: as above, for the event's life.
        let event =
            unsafe { swoole_rs_reactor::Event::new(fd, 0, FD_STREAM_SERVER, listener.cast()) };
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
