//! Reactor threads: the datagram path and dispatch to workers, the worker pipe
//! protocol, heartbeats and the threads' lifecycle.
use core::ffi::{c_int, c_long};
#[cfg(any(test, feature = "upstream-symbols"))]
use swoole_rs_corebase::ServerEvent;
#[cfg(test)]
use swoole_rs_corebase::constants::SocketTypeCode;
use swoole_rs_corebase::reactor::ErrorHandler::ConnectionDefault;

use swoole_rs_corebase::reactor::Event;
#[cfg(any(test, feature = "upstream-symbols"))]
use swoole_rs_corebase::reactor::Heap;
use swoole_rs_corebase::{CellExt, DataHead, log, microtime};
#[cfg(any(test, feature = "upstream-symbols"))]
use swoole_rs_error::ErrorCode;

use crate::base_factory::SendData;
use crate::hybrid::hybrid;
use crate::reactor_entry::DgramPacket;
use crate::reactor_handle::Reactor;
use crate::records::{Connection, Lookup};
use crate::{Mode, ServerState, view};
#[cfg(any(test, feature = "upstream-symbols"))]
use swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE;

/// `SW_LOG_TRACE`, `SW_FD_SESSION`, `SW_FD_DGRAM_SERVER`, `SW_EVENT_READ` and
/// `SW_EVENT_WRITE`.
const FD_SESSION: c_int = 0;
const FD_DGRAM_SERVER: c_int = 2;

/// The heartbeat thread's `ServerState::start_thread` key; reactor threads use their ids.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) const HEARTBEAT: u64 = u64::MAX;

thread_local! {
    /// This thread's datagram buffer, the size of its `sw_tg_buffer()`: taken out while
    /// a read dispatches from it, so a nested read makes its own.
    static DATAGRAMS: core::cell::Cell<Option<Box<[u8]>>> = const { core::cell::Cell::new(None) };
}

/// `ReactorThread_onPacketReceived`: dispatch the datagrams on the event's socket, a
/// UDP port's, until it would block. Each is read into this thread's datagram buffer;
/// false when one fails.
pub(crate) fn packet_received(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> bool {
    let allocate = || {
        let size = swoole_rs_corebase::buffer::string::with_thread_buffer(|b| b.size());
        vec![0; size].into_boxed_slice()
    };
    DATAGRAMS.with(|cell| {
        cell.with_taken(|buffer| {
            let buffer = buffer.get_or_insert_with(allocate);
            receive_packets(serv, reactor, event.fd(), buffer)
        })
    })
}

/// [`packet_received`] from `fd` into `buffer`: each datagram's packet head, then the
/// datagram in the remaining room.
fn receive_packets(serv: view::Server<'_>, reactor: Reactor, fd: c_int, buffer: &mut [u8]) -> bool {
    use crate::reactor_entry::DGRAM_HEAD as HEAD;
    if buffer.len() < HEAD {
        return false;
    }
    let state = serv.state();
    // The event's fd is a listening socket's, whose slot holds its port.
    let Some(conn) = state.connection(fd.into(), Lookup::ByFd) else {
        return false;
    };
    let kind = conn.socket_type();
    let Some(port) = conn.port() else {
        return false;
    };
    let dtls = port.config().ssl && port.config().is_dtls();
    let info = DataHead {
        server_fd: crate::base_factory::low16(fd),
        reactor_id: swoole_rs_corebase::thread_id().cast_signed(),
        kind: swoole_rs_corebase::ServerEvent::RecvDgram.code(),
        time: microtime(),
        ..DataHead::default()
    };
    let base = state.config_ref().mode_ == Mode::Base;
    // The complete native head, its padding zero, precedes each datagram.
    let mut pkt = DgramPacket::default();
    loop {
        let (header, payload) = buffer.split_at_mut(HEAD);
        pkt.socket_type = kind;
        pkt.socket_addr.kind = kind.0;
        let mut room = swoole_rs_corebase::reactor::Fill::new(payload);
        // A DTLS datagram goes to the handshake, as the bytes it received.
        let received = room.spare(|spare| match spare.recvfrom(fd, 0, &mut pkt.socket_addr) {
            Some(1..) if dtls => {
                Err(hybrid().dtls_accept(serv, reactor, port, &pkt.socket_addr, spare.written()))
            }
            n => Ok(n),
        });
        let n = match received {
            Ok(n) => n,
            Err(accepted) => return accepted,
        };
        let Some(length @ 1..) = n.and_then(|n| u32::try_from(n).ok()) else {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN) {
                return true;
            }
            log::sys_warning(
                c"ReactorThread_onPacketReceived",
                format_args!("recvfrom({fd}) failed"),
            );
            return false;
        };
        pkt.length = length;
        header.copy_from_slice(&pkt.to_bytes());
        let info = DataHead {
            fd: pkt.sender(),
            ..info
        };
        // The packet head and the `length` bytes just received behind it.
        let Some(packet) = buffer.get(..HEAD + length as usize) else {
            return false;
        };
        let mut task = SendData::new(info, packet);
        let dispatched = if base {
            // Base mode hands it to this process's worker as it does a connection's data.
            crate::base_factory::dispatch(serv, &mut task, true)
        } else {
            hybrid().factory_dispatch(serv, &mut task)
        };
        if !dispatched {
            return false;
        }
    }
}

/// `Server::foreach_connection`: each valid connection from the lowest fd to
/// the highest, read afresh each step.
pub(crate) fn each_connection(state: &ServerState, mut each: impl FnMut(Connection<'_>)) {
    let Some(gs) = crate::shared::server_counters(state) else {
        return;
    };
    let bounds = state.fd_bounds(gs);
    let mut fd = bounds.min_fd();
    while fd <= bounds.max_fd() {
        let conn = state.connection(c_long::from(fd), Lookup::ByFd);
        if let Some(conn) = conn.filter(|_| crate::reactor_ops::valid(conn)) {
            each(conn);
        }
        fd = fd.saturating_add(1);
    }
}

/// `ReactorThread::close_connection`: close `session` now when nobody is told,
/// else notify the worker through the close event.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn close(serv: view::Server<'_>, reactor: Reactor, session: c_long) {
    let Some(conn) = serv.state().connection(session, Lookup::Verified) else {
        let m = format_args!("force close connection failed, session#{session} does not exist");
        let code = ErrorCode::SW_ERROR_SESSION_NOT_EXIST as c_int;
        log::error_log(SW_LOG_TRACE, code, c"ReactorThread::close_connection", m);
        return;
    };
    // A TLS connection that never finished its handshake has nobody to notify.
    let handshaking = conn.ssl() && !conn.ssl_ready();
    if serv.state().config_ref().disable_notify.get() || conn.close_force() || handshaking {
        crate::conn_io::close_connection(serv, reactor, conn.socket());
        return;
    }
    conn.set_close_force(true);
    reactor.trigger_close(&mut conn.event(0, 0));
}

/// `Server::heartbeat_check`: close `reactor`'s connections idle past their port's
/// heartbeat time.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn heartbeat_check(serv: view::Server<'_>, reactor: Reactor) {
    let (now, id) = (microtime(), reactor.handle().id());
    each_connection(serv.state(), |conn| {
        let session = conn.session_id();
        if session > 0 && u16::from(conn.reactor_id()) == id && !serv.healthy(now, conn) {
            close(serv, reactor, session);
        }
    });
}

/// The destroy callback `ReactorThread::shutdown` adds: close each connection
/// `reactor` owns.
#[cfg_attr(
    not(feature = "upstream-symbols"),
    expect(
        dead_code,
        reason = "its caller is the reactor threads' destroy callback"
    )
)]
pub(crate) fn close_reactor_connections(serv: view::Server<'_>, reactor: Reactor) {
    let id = reactor.handle().id();
    each_connection(serv.state(), |conn| {
        if u16::from(conn.reactor_id()) == id {
            crate::conn_io::close_connection(serv, reactor, conn.socket());
        }
    });
}

/// `Server::init_reactor`: a reactor's session and datagram handlers.
pub(crate) fn init_reactor(serv: view::Server<'_>, reactor: Reactor) {
    use crate::reactor_entry::{on_packet_received, on_read, on_write};
    // 64K datagrams need the larger thread buffer.
    if serv.state().config_ref().have_dgram_sock.get() {
        swoole_rs_corebase::buffer::string::with_thread_buffer(|b| {
            b.extend(b.size().wrapping_mul(2));
        });
    }
    reactor.set_handlers(
        FD_DGRAM_SERVER,
        Some(on_packet_received),
        None,
        ConnectionDefault,
    );
    reactor.set_handlers(FD_SESSION, Some(on_read), Some(on_write), ConnectionDefault);
}

/// `Server::join_reactor_thread`: tell each running reactor thread to shut down,
/// and join it.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn join_reactor_threads(serv: view::Server<'_>) {
    let state = serv.state();
    let config = state.config_ref();
    if config.single_thread.get() {
        return;
    }
    if config.heartbeat_check_interval.get() > 0
        && let Some(heartbeat) = state.take_thread(HEARTBEAT)
    {
        heartbeat.join().unwrap_or_default();
    }
    for id in 0..config.reactor_num.get() {
        let Some(joining) = state.take_thread(u64::from(id)) else {
            continue;
        };
        // Only the field is read: the thread itself holds its record meanwhile.
        if let Some(thread) = state.tables_ref().reactor_thread(usize::from(id)) {
            let pipe = thread
                .notify_worker
                .get()
                .and_then(|worker| serv.worker_pipe(worker, false));
            send_event(pipe, 0, ServerEvent::Shutdown);
        }
        joining.join().unwrap_or_default();
    }
}

/// `socket->send_sync(&head, sizeof(head))` of a bare event.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn send_event(sock: Option<Heap<'_>>, fd: c_long, kind: ServerEvent) {
    let head = DataHead {
        fd,
        kind: kind.code(),
        ..DataHead::default()
    };
    if let Some(sock) = sock {
        let data = swoole_rs_corebase::process::process_pool::head_view(&head);
        let data = swoole_rs_corebase::reactor::Drain::new(&data);
        _ = swoole_rs_corebase::tls::upstream::send_sync_drain(*sock, data, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_corebase::reactor::Events;

    /// A UDP socket listed as a datagram port of `live`, as `store_listen_socket`
    /// leaves one, and a client that sends to it.
    fn udp_port(live: &crate::fixture::LiveServer) -> (std::net::UdpSocket, std::net::UdpSocket) {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&server);
        let slot = live.state().hold_port(0, fd).unwrap();
        slot.set_fd(fd);
        slot.set_socket_type(SocketTypeCode(2)); // SW_SOCK_UDP
        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        client.connect(server.local_addr().unwrap()).unwrap();
        (server, client)
    }

    #[test]
    fn datagram_headers_preserve_unknown_socket_codes() {
        let mut head = [0; crate::reactor_entry::DGRAM_HEAD];
        head[..size_of::<c_int>()].copy_from_slice(&(-1_i32).to_ne_bytes());
        let packet = DgramPacket::from_bytes(&head);
        assert_eq!(packet.socket_type.known(), None);
        assert_eq!(packet.to_bytes(), head);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no datagram sockets")]
    fn datagram_buffer_rejects_a_short_head() {
        let live = crate::fixture::LiveServer::new();
        let (serv, reactor) = (live.server(), live.reactor());
        let (server, _client) = udp_port(&live);
        let fd = std::os::fd::AsRawFd::as_raw_fd(&server);
        let mut short = [0; size_of::<DgramPacket>() - 1];
        assert!(!receive_packets(serv, reactor, fd, &mut short));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no datagram sockets")]
    fn datagrams_until_eagain() {
        use crate::hybrid::tests::{Call, calls};
        let live = crate::fixture::LiveServer::new();
        let (serv, reactor) = (live.server(), live.reactor());
        let (server, client) = udp_port(&live);
        let fd = std::os::fd::AsRawFd::as_raw_fd(&server);
        let mut buffer = vec![0; 8192];
        // Each pass drains what has arrived; loopback may deliver a datagram late.
        let mut received = |count: usize| {
            let mut heard = Vec::new();
            for _ in 0..1000 {
                assert!(receive_packets(serv, reactor, fd, &mut buffer), "drained");
                heard.extend(calls());
                if heard.len() >= count {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            heard
        };
        drop(calls());
        client.send(b"one").unwrap();
        client.send(b"three").unwrap();
        // An IPv4 sender's session is its address.
        let sender = c_long::from(u32::from_ne_bytes([127, 0, 0, 1]).cast_signed());
        let head = u32::try_from(size_of::<DgramPacket>()).unwrap();
        let each = [
            Call::Dispatch(
                sender,
                swoole_rs_corebase::ServerEvent::RecvDgram.code(),
                head + 3,
            ),
            Call::Dispatch(
                sender,
                swoole_rs_corebase::ServerEvent::RecvDgram.code(),
                head + 5,
            ),
        ];
        assert_eq!(received(2), each, "each datagram, through the factory");

        // Base mode hands it to this worker; with none here, through the bus.
        // SAFETY: the fixture's state, on this thread only.
        unsafe { (*live.state().config()).mode_ = Mode::Base };
        client.send(b"two").unwrap();
        assert_eq!(
            received(1),
            [Call::BasePass(
                sender,
                swoole_rs_corebase::ServerEvent::RecvDgram.code(),
                head + 3
            )]
        );

        let no_port = receive_packets(serv, reactor, -1, &mut buffer);
        assert!(!no_port, "no port");
        assert_eq!(calls(), []);
    }

    /// The live server with session 1 accepted. The reactor records the fds its
    /// error handler (the worker told) is given.
    fn accepted() -> (crate::fixture::LiveServer, std::net::TcpStream) {
        let live = crate::fixture::LiveServer::new();
        let client = live.connect();
        let event = live.accept_event();
        crate::connection::accept(live.server(), live.reactor(), &event);
        live.reactor().set_error_handler(record_error);
        drop(handled());
        (live, client)
    }

    std::thread_local! {
        static HANDLED: core::cell::RefCell<Vec<(&'static str, c_int)>> =
            const { core::cell::RefCell::new(Vec::new()) };
    }

    /// What the reactor's handlers were given since the last call.
    fn handled() -> Vec<(&'static str, c_int)> {
        HANDLED.with_borrow_mut(core::mem::take)
    }

    fn record_error(
        _: swoole_rs_corebase::reactor::Running<'_>,
        event: &mut swoole_rs_corebase::reactor::Event,
    ) -> c_int {
        HANDLED.with_borrow_mut(|h| h.push(("told", event.fd())));
        0
    }

    fn session(live: &crate::fixture::LiveServer) -> Connection<'_> {
        live.state().connection(1, Lookup::Verified).unwrap()
    }

    #[test]
    fn close_tells_the_worker_unless_nobody_is_told() {
        let (live, _client) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let fd = session(&live).fd();
        close(serv, reactor, 9);
        assert_eq!(handled(), []);
        close(serv, reactor, 1);
        assert_eq!(handled(), [("told", fd)], "the worker is told");
        assert!(session(&live).close_force());
        let conn = session(&live);
        live.reactor().change(
            conn.socket().unwrap(),
            swoole_rs_corebase::reactor::ChangeOp::Add,
            Events::READ,
        );
        close(serv, reactor, 1);
        assert_eq!(handled(), [], "a forced close closes now");
        assert!(!conn.is_active() && conn.socket().is_none());
    }

    #[test]
    fn heartbeat_closes_only_this_reactors_stale_sessions() {
        let (live, _client) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live);
        let fd = conn.fd();
        serv.port_at(0).unwrap().config().heartbeat_idle_time.set(1);
        heartbeat_check(serv, reactor);
        assert_eq!(handled(), [], "a session that never received is healthy");
        conn.set_last_recv_time(1.0);
        conn.set_reactor_id(conn.reactor_id().wrapping_add(1));
        heartbeat_check(serv, reactor);
        assert_eq!(handled(), [], "another reactor's session");
        conn.set_reactor_id(conn.reactor_id().wrapping_sub(1));
        heartbeat_check(serv, reactor);
        assert_eq!(handled(), [("told", fd)], "the stale session");
    }

    #[test]
    fn init_reactor_sets_the_session_handlers() {
        use swoole_rs_corebase::reactor::reactor_upstream::isset_handler as isset;
        let live = crate::fixture::LiveServer::new();
        let running = live.reactor().handle();
        assert!(!isset(running, FD_SESSION, Events::READ));
        init_reactor(live.server(), live.reactor());
        for (fd_type, event) in [
            (FD_SESSION, Events::READ),
            (FD_SESSION, Events::WRITE),
            (FD_DGRAM_SERVER, Events::READ),
        ] {
            assert!(isset(running, fd_type, event), "{fd_type} {event}");
        }
    }

    static JOINED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

    fn joined(arg: u64) -> impl FnOnce() + Send + 'static {
        move || JOINED.lock().unwrap().push(arg)
    }

    #[test]
    fn join_waits_for_each_reactor_thread_and_the_heartbeat() {
        let live = crate::fixture::LiveServer::new();
        let (serv, state) = (live.server(), live.state());
        let config = state.config_ref();
        for id in 0..u64::from(config.reactor_num.get()) {
            ServerState::start_thread(serv, id, joined(id));
        }
        ServerState::start_thread(serv, HEARTBEAT, joined(HEARTBEAT));
        config.heartbeat_check_interval.set(1);
        join_reactor_threads(serv);
        let mut ran = JOINED.lock().unwrap().clone();
        ran.sort_unstable();
        assert_eq!(ran, vec![0, HEARTBEAT], "each thread ran and was joined");
        assert!(state.take_thread(0).is_none() && state.take_thread(HEARTBEAT).is_none());
    }
}
