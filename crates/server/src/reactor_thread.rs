//! Reactor threads: the datagram path and dispatch to workers, the worker pipe
//! protocol, heartbeats and the threads' lifecycle.
use core::ffi::{c_int, c_long, c_void};
use swoole_rs_corebase::ServerEvent;

use swoole_rs_corebase::net_types::AddressStorage;
use swoole_rs_corebase::{DataHead, log, microtime};
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::{Event, Socket};

use crate::base_factory::SendData;
use crate::hybrid::hybrid;
use crate::reactor_entry::DgramPacket;
use crate::reactor_handle::Reactor;
use crate::records::{Connection, Lookup};
use crate::{Mode, ServerState, view};

/// `SW_LOG_TRACE`, `SW_FD_SESSION`, `SW_FD_DGRAM_SERVER`, `SW_EVENT_READ` and
/// `SW_EVENT_WRITE`.
const SW_LOG_TRACE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as c_int;
const FD_SESSION: c_int = 0;
const FD_DGRAM_SERVER: c_int = 2;
const SW_EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const SW_EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;

/// `SW_OK` and `SW_ERR`.
const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;

/// The heartbeat thread's `ServerState::start_thread` key; reactor threads use their ids.
pub(crate) const HEARTBEAT: u64 = u64::MAX;

/// `ReactorThread_onPacketReceived`: dispatch the datagrams on the event's socket, a
/// UDP port's, until it would block. Each is read into this thread's buffer.
pub(crate) fn packet_received(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> c_int {
    let (buffer, size) =
        swoole_rs_buffer::string::with_thread_buffer(|b| (b.as_mut_ptr().cast(), b.size()));
    receive_packets(serv, reactor, event.fd(), buffer, size)
}

/// [`packet_received`] from `fd` into the `size` bytes at `buffer`, which are aligned
/// for a [`DgramPacket`] and live, and which nothing else borrows, for the call.
fn receive_packets(
    serv: view::Server<'_>,
    reactor: Reactor,
    fd: c_int,
    buffer: *mut c_void,
    size: usize,
) -> c_int {
    let state = serv.state();
    // The event's fd is a listening socket's, whose slot holds its port.
    let Some(conn) = state.connection(fd.into(), Lookup::ByFd) else {
        return SW_ERR;
    };
    let kind = conn.socket_type();
    let Some(port) = state.listed_port(conn.object()) else {
        return SW_ERR;
    };
    let dtls = port.config().is_dtls();
    let head = size_of::<DgramPacket>();
    let info = DataHead {
        server_fd: crate::base_factory::low16(fd),
        reactor_id: swoole_rs_corebase::thread_id().cast_signed(),
        kind: swoole_rs_corebase::ServerEvent::RecvDgram.code(),
        time: microtime(),
        ..DataHead::default()
    };
    // SAFETY: no payload yet (`len` 0); each packet's length is set below.
    let mut task = unsafe { SendData::from_raw(info, buffer.cast_const().cast()) };
    let base = state.config_ref().mode_ == Mode::Base;
    loop {
        // SAFETY: the caller's buffer, which holds a packet head and its datagram.
        let pkt = unsafe { &mut *buffer.cast::<DgramPacket>() };
        pkt.socket_type = kind;
        let address = &mut pkt.socket_addr;
        address.kind = kind;
        address.len = libc::socklen_t::try_from(size_of::<AddressStorage>()).unwrap_or(0);
        let data = buffer.wrapping_byte_add(head);
        // SAFETY: the buffer's room after the packet head, which nothing else reaches
        // during the call.
        let n = unsafe { swoole_rs_reactor::Fill::from_raw(data, size - head) }
            .recvfrom(fd, 0, address);
        let Ok(length @ 1..) = u32::try_from(n) else {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN) {
                return SW_OK;
            }
            log::sys_warning(
                c"ReactorThread_onPacketReceived",
                format_args!("recvfrom({fd}) failed"),
            );
            return SW_ERR;
        };
        if dtls {
            let datagram = data.cast::<u8>();
            // SAFETY: the `length` bytes just received into the buffer's room after the
            // packet head, which nothing writes during the call.
            let datagram = unsafe { core::slice::from_raw_parts(datagram, length as usize) };
            return hybrid().dtls_accept(serv, reactor, port, &pkt.socket_addr, datagram);
        }
        pkt.length = length;
        task.set_fd(pkt.sender());
        // SAFETY: the packet head and the `length` bytes just received behind it, in the
        // caller's buffer, which nothing writes until the dispatch returns.
        unsafe { task.set_len(u32::try_from(head).unwrap_or(0).saturating_add(length)) };
        let dispatched = if base {
            // Base mode hands it to this process's worker as it does a connection's data.
            crate::base_factory::dispatch(serv, &mut task, true)
        } else {
            hybrid().factory_dispatch(serv, &mut task)
        };
        if !dispatched {
            return SW_ERR;
        }
    }
}

/// `Server::foreach_connection`: each valid connection from the shared lowest fd to
/// the highest, read afresh each step.
pub(crate) fn each_connection(state: &ServerState, mut each: impl FnMut(Connection<'_>)) {
    let Some(gs) = crate::shared::server_counters(state) else {
        return;
    };
    let mut fd = gs.min_fd();
    while fd <= gs.max_fd() {
        let conn = state.connection(c_long::from(fd), Lookup::ByFd);
        if let Some(conn) = conn.filter(|_| crate::reactor_ops::valid(conn)) {
            each(conn);
        }
        fd = fd.saturating_add(1);
    }
}

/// `ReactorThread::close_connection`: close `session` now when nobody is told,
/// else notify the worker through the close event.
pub(crate) fn close(serv: view::Server<'_>, reactor: Reactor, session: c_long) -> c_int {
    let Some(conn) = serv.state().connection(session, Lookup::Verified) else {
        let m = format_args!("force close connection failed, session#{session} does not exist");
        let code = ErrorCode::SW_ERROR_SESSION_NOT_EXIST as c_int;
        log::error_log(SW_LOG_TRACE, code, c"ReactorThread::close_connection", m);
        return 0;
    };
    // A TLS connection that never finished its handshake has nobody to notify.
    let handshaking = conn.ssl() && !conn.ssl_ready();
    if serv.state().config_ref().disable_notify.get() || conn.close_force() || handshaking {
        return crate::conn_io::close_connection(serv, reactor, conn.socket());
    }
    conn.set_close_force(true);
    reactor.trigger_close(&mut conn.event(0, 0));
    0
}

/// `Server::heartbeat_check`: close `reactor`'s connections idle past their port's
/// heartbeat time.
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
    not(all(feature = "upstream-symbols", not(test))),
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
        swoole_rs_buffer::string::with_thread_buffer(|b| b.extend(b.size().wrapping_mul(2)));
    }
    reactor.set_handler(FD_DGRAM_SERVER, SW_EVENT_READ, on_packet_received);
    reactor.set_handler(FD_SESSION, SW_EVENT_WRITE, on_write);
    reactor.set_handler(FD_SESSION, SW_EVENT_READ, on_read);
}

/// `Server::join_reactor_thread`: tell each running reactor thread to shut down,
/// and join it.
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
            let notify = thread.notify_pipe.get();
            // SAFETY: null or the thread's notify pipe, which it keeps until the server
            // joins it, after this notice.
            let pipe = unsafe { Socket::from_ptr(notify) };
            send_event(pipe, 0, ServerEvent::Shutdown);
        }
        joining.join().unwrap_or_default();
    }
}

/// `socket->send_sync(&head, sizeof(head))` of a bare event.
pub(crate) fn send_event(sock: Option<Socket<'_>>, fd: c_long, kind: ServerEvent) {
    let head = DataHead {
        fd,
        kind: kind.code(),
        ..DataHead::default()
    };
    if let Some(sock) = sock {
        let data = swoole_rs_process::process_pool::head_view(&head);
        let data = swoole_rs_reactor::Drain::new(data);
        _ = swoole_rs_tls::upstream::send_sync_drain(sock, data, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UDP socket listed as a datagram port of `live`, as `store_listen_socket`
    /// leaves one, and a client that sends to it.
    fn udp_port(live: &crate::fixture::LiveServer) -> (std::net::UdpSocket, std::net::UdpSocket) {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&server);
        let slot = live.state().tables_ref().connection_at(fd).unwrap();
        slot.set_fd(fd);
        slot.set_socket_type(2); // SW_SOCK_UDP
        slot.set_object(core::ptr::from_ref(live.port()).cast_mut().cast());
        slot.listen(live.server().port_at(0).unwrap());
        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        client.connect(server.local_addr().unwrap()).unwrap();
        (server, client)
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn datagrams_until_eagain() {
        use crate::hybrid::tests::{Call, calls};
        let live = crate::fixture::LiveServer::new();
        let (serv, reactor) = (live.server(), live.reactor());
        let (server, client) = udp_port(&live);
        let fd = std::os::fd::AsRawFd::as_raw_fd(&server);
        let mut buffer = vec![0_u64; 1024];
        let (at, size) = (buffer.as_mut_ptr().cast(), buffer.len() * 8);
        // Each pass drains what has arrived; loopback may deliver a datagram late.
        let received = |count: usize| {
            let mut heard = Vec::new();
            for _ in 0..1000 {
                assert_eq!(
                    receive_packets(serv, reactor, fd, at, size),
                    SW_OK,
                    "drained"
                );
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

        let no_port = receive_packets(serv, reactor, -1, at, size);
        assert_eq!(no_port, SW_ERR, "no port");
        assert_eq!(calls(), []);
    }

    /// The live server with session 1 accepted. The reactor records the fds its
    /// error handler (the worker told) is given.
    fn accepted() -> (crate::fixture::LiveServer, std::net::TcpStream) {
        let live = crate::fixture::LiveServer::new();
        let client = live.connect();
        // SAFETY: the fixture's listener, live for the event.
        let event = unsafe {
            swoole_rs_reactor::Event::new(live.listener_fd, 0, 1, live.listener().cast())
        };
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

    extern "C" fn record_error(
        _: swoole_rs_reactor::Running<'_>,
        event: &mut swoole_rs_reactor::Event,
    ) -> c_int {
        HANDLED.with_borrow_mut(|h| h.push(("told", event.fd())));
        0
    }

    fn session(live: &crate::fixture::LiveServer) -> Connection<'_> {
        live.state().connection(1, Lookup::Verified).unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn close_tells_the_worker_unless_nobody_is_told() {
        let (live, _client) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let fd = session(&live).fd();
        assert_eq!(close(serv, reactor, 9), 0, "a missing session");
        assert_eq!(handled(), []);
        assert_eq!(close(serv, reactor, 1), 0);
        assert_eq!(handled(), [("told", fd)], "the worker is told");
        assert!(session(&live).close_force());
        let conn = session(&live);
        live.reactor().change(
            conn.socket().unwrap(),
            swoole_rs_reactor::ChangeOp::Add,
            SW_EVENT_READ,
        );
        assert_eq!(close(serv, reactor, 1), 0);
        assert_eq!(handled(), [], "a forced close closes now");
        assert!(!conn.is_active() && conn.socket().is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
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
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn init_reactor_sets_the_session_handlers() {
        use swoole_rs_reactor::reactor_upstream::reactor_isset_handler as isset;
        let live = crate::fixture::LiveServer::new();
        let running = live.reactor().handle();
        assert!(!isset(running, FD_SESSION, SW_EVENT_READ));
        init_reactor(live.server(), live.reactor());
        for (fd_type, event) in [
            (FD_SESSION, SW_EVENT_READ),
            (FD_SESSION, SW_EVENT_WRITE),
            (FD_DGRAM_SERVER, SW_EVENT_READ),
        ] {
            assert!(isset(running, fd_type, event), "{fd_type} {event}");
        }
    }

    static JOINED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

    fn joined(_: view::Server<'_>, arg: u64) {
        JOINED.lock().unwrap().push(arg);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn join_waits_for_each_reactor_thread_and_the_heartbeat() {
        let live = crate::fixture::LiveServer::new();
        let (serv, state) = (live.server(), live.state());
        let config = state.config_ref();
        for id in 0..u64::from(config.reactor_num.get()) {
            ServerState::start_thread(serv, joined, id);
        }
        ServerState::start_thread(serv, joined, HEARTBEAT);
        config.heartbeat_check_interval.set(1);
        join_reactor_threads(serv);
        let mut ran = JOINED.lock().unwrap().clone();
        ran.sort_unstable();
        assert_eq!(ran, vec![0, HEARTBEAT], "each thread ran and was joined");
        assert!(state.take_thread(0).is_none() && state.take_thread(HEARTBEAT).is_none());
    }
}
