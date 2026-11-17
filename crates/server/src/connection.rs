//! Accepting connections and their idle and health checks.
use core::ffi::{c_int, c_long};
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::reactor::Events;

use swoole_rs_corebase::constants::{SW_ACCEPT_RETRY_TIME, SW_SEND_BUFFER_SIZE, SocketType};
use swoole_rs_corebase::reactor::{Event, Heap, OwnedSocket};
use swoole_rs_corebase::{CCast, log};
use swoole_rs_error::ErrorCode as E;

use crate::reactor_handle::Reactor;
use crate::records::Lookup;
use crate::{Connection, Mode, view};
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;

/// `SW_ACCEPT_MAX_COUNT`, `SW_MAX_CONNECTION`, `SW_SESSION_LIST_SIZE`; native asserts each.
const ACCEPT_MAX_COUNT: u32 = 64;
const MAX_CONNECTION: u32 = 100_000;
const SESSION_LIST_SIZE: u32 = 1024 * 1024;

use swoole_rs_corebase::sys::errno;

/// `Server::accept_connection`: up to `SW_ACCEPT_MAX_COUNT` sockets per readable
/// event on `reactor`, this thread's.
pub(crate) fn accept(serv: view::Server<'_>, reactor: Reactor, event: &Event) {
    let server_fd = event.fd();
    let Some(listener) = event.socket() else {
        return;
    };
    // A listening socket's object is its port.
    let Some(port) = serv.state().listed_port(listener.read(|l| l.object)) else {
        return;
    };
    for _ in 0..ACCEPT_MAX_COUNT {
        // The record `accepted` just made: the connection takes it, or `discard` frees it.
        let accepted = listener.edit(swoole_rs_corebase::reactor::SocketFields::accepted);
        let Some(sock) = accepted.map(OwnedSocket::new) else {
            if accept_failed(serv, errno()) {
                continue;
            }
            return;
        };
        let fd = sock.socket().fd();
        if i64::from(fd) >= i64::from(serv.max_connection()) {
            let m = format_args!("Too many connections [now: {fd}]");
            log::error_log(
                LOG_WARNING,
                E::SW_ERROR_SERVER_TOO_MANY_SOCKET as c_int,
                c"Server::accept_connection",
                m,
            );
            discard(serv, Some(port), sock);
            disable_accept(serv);
            return;
        }
        if port.config().ssl {
            if !(crate::port_config::methods().ssl_create)(port.config(), Some(sock.socket())) {
                discard(serv, Some(port), sock);
                return;
            }
        } else {
            sock.socket().edit(|s| s.set_ssl(None));
        }
        let conn = match add(serv, port, sock, server_fd) {
            Ok(conn) => conn,
            Err(sock) => return discard(serv, Some(port), sock),
        };
        let Some(sock) = conn.socket() else {
            return;
        };
        sock.edit(|s| s.chunk_size = SW_SEND_BUFFER_SIZE);
        let started = if serv.config().single_thread.get() {
            incoming(serv, reactor, port, conn)
        } else {
            to_reactor_thread(serv, conn).is_some()
        };
        if !started {
            abort(serv, reactor, Some(port), sock);
            return;
        }
    }
}

/// Whether the batch retries after `accept()` failed with `errno`. Out of
/// descriptors, accepting waits for the retry timer.
fn accept_failed(serv: view::Server<'_>, errno: c_int) -> bool {
    match errno {
        libc::EAGAIN => return false,
        libc::EINTR => return true,
        libc::EMFILE | libc::ENFILE => disable_accept(serv),
        _ => {}
    }
    log::sys_warning(
        c"Server::accept_connection",
        format_args!("accept() failed"),
    );
    false
}

/// `serv->send_to_reactor_thread()` of the connection's `SW_SERVER_EVENT_INCOMING`.
fn to_reactor_thread(serv: view::Server<'_>, conn: Connection<'_>) -> Option<usize> {
    let head = swoole_rs_corebase::DataHead {
        fd: conn.session_id(),
        reactor_id: conn.reactor_id().into(),
        kind: ServerEvent::Incoming.code(),
        server_fd: u16::try_from(conn.server_fd()).unwrap_or(u16::MAX),
        ..swoole_rs_corebase::DataHead::default()
    };
    crate::hybrid::hybrid().to_reactor_thread(serv, &head)
}

/// `Server::add_connection(port, sock, server_fd)`: the next free session slot after
/// `session_round` takes `sock`, and its fd's connection slot is set up and owns it.
///
/// # Errors
/// `sock`, which no connection took, when its fd has no slot or every session is taken.
pub fn add<'a>(
    serv: view::Server<'a>,
    port: view::Port<'_>,
    sock: OwnedSocket,
    server_fd: c_int,
) -> Result<Connection<'a>, OwnedSocket> {
    let state = serv.state();
    let config = serv.config();
    let ls = port.config();
    let gs = serv.counters();
    let fd = sock.socket().fd();
    let Some(conn) = state.tables_ref().connection_at(fd) else {
        return Err(sock);
    };
    // C++'s ReactorId (uint8_t).
    let reactor_id = if config.mode_ == Mode::Base {
        swoole_rs_corebase::process::methods::swoole_get_worker_id().c_cast::<u8>()
    } else {
        (fd % c_int::from(config.reactor_num.get())).c_cast::<u8>()
    };
    // The fd's last connection, closed on a reactor thread, published the slot with
    // its last write: acquire that before reusing it.
    conn.closed().load(Acquire);
    // Upstream zeroes the slot up to `closed`; other reactor threads read `active`.
    conn.active().store(false, Relaxed);
    state.tables_ref().reset_connection(fd);

    // `sw_spinlock`, with `SW_SPINLOCK_LOOP_N`.
    let cpus = u32::from(swoole_rs_corebase::swr_v1_core_cpu_num());
    swoole_rs_corebase::utilities::swr_v1_utils_spinlock(Some(&gs.spinlock), cpus, 1024);
    let mut session_id = gs.session_round.load(Relaxed);
    let mut found = false;
    for _ in 0..serv.max_connection() {
        session_id += 1;
        let Some(session) = state.session(session_id) else {
            continue;
        };
        if session.fd() == 0 {
            session.set_fd(fd);
            session.set_id(session_id);
            session.set_reactor_bits((session.reactor_bits() & !0xff) | u32::from(reactor_id));
            found = true;
            break;
        }
    }
    gs.spinlock.store(0, Release);
    if !found {
        let m = format_args!("no available session slot, fd={fd}");
        log::error_log(
            LOG_WARNING,
            E::SW_ERROR_SERVER_TOO_MANY_SOCKET as c_int,
            c"Server::add_connection",
            m,
        );
        return Err(sock);
    }
    gs.session_round.store(session_id, Relaxed);
    conn.set_session_id(session_id);
    let buffer_size = ls.socket_buffer_size.get().cast_unsigned();
    let socket = sock.socket();
    socket.edit(|s| {
        s.object = conn.as_ptr().cast();
        s.removed = true;
        s.output.buffer_size = buffer_size;
        s.timeouts.write = 0.0;
        s.timeouts.read = 0.0;
    });

    let listener = ls.listening_socket();
    let tcp = listener.is_some_and(|l| l.read(swoole_rs_corebase::reactor::SocketFields::is_tcp));
    if ls.open_tcp_nodelay.get() && tcp {
        if !socket.edit(|s| s.set_tcp_nodelay(1)) {
            log::sys_warning(
                c"Server::add_connection",
                format_args!("setsockopt(TCP_NODELAY) failed"),
            );
        }
        socket.edit(|s| s.enable_tcp_nodelay = true);
    }
    // Upstream sets the buffer sizes on the listening socket.
    let listen_fd = listener.map_or(-1, |listener| listener.fd());
    for (name, label, size) in [
        (
            libc::SO_RCVBUF,
            "SO_RCVBUF",
            ls.kernel_socket_recv_buffer_size.get(),
        ),
        (
            libc::SO_SNDBUF,
            "SO_SNDBUF",
            ls.kernel_socket_send_buffer_size.get(),
        ),
    ] {
        if size > 0
            && swoole_rs_corebase::sys::setsockopt(listen_fd, libc::SOL_SOCKET, name, &size)
                .is_err()
        {
            let m = format_args!("setsockopt({label}, {size}) failed");
            log::sys_warning(c"Server::add_connection", m);
        }
    }
    if !ls.ssl {
        socket.edit(|s| s.direct_send = true);
    }
    init(serv, conn, ls, sock, server_fd, reactor_id);

    state.locked.lock();
    if fd > gs.max_fd() {
        gs.max_fd.store(fd, Relaxed);
    } else if fd < gs.min_fd() {
        gs.min_fd.store(fd, Relaxed);
    }
    state.locked.unlock();

    gs.accept_count.fetch_add(1, Relaxed);
    let port_gs = ls.counters();
    if let Some(port_gs) = port_gs {
        port_gs.accept_count.fetch_add(1, Relaxed);
    }
    if config.mode_ == Mode::Base {
        // Base mode counts connections per worker; the reactor id is the accepting worker's.
        let at = usize::from(reactor_id);
        for cell in [
            serv.worker_connections(at),
            serv.port_worker_connections(port, at),
        ]
        .into_iter()
        .flatten()
        {
            cell.fetch_add(1, SeqCst);
        }
    } else {
        gs.connection_num.fetch_add(1, SeqCst);
        if let Some(port_gs) = port_gs {
            port_gs.connection_num.fetch_add(1, SeqCst);
        }
    }
    Ok(conn)
}

/// The connection's fields from its socket: the peer address, then the local one
/// `getsockname` writes over it in the socket.
fn init(
    serv: view::Server<'_>,
    conn: Connection<'_>,
    ls: &crate::port_config::PortConfig,
    sock: OwnedSocket,
    server_fd: c_int,
    reactor_id: u8,
) {
    let state = serv.state();
    let (ssl, local_port, local_addr_index) = sock.socket().edit(|sock| {
        conn.set_info(sock.info);
        sock.get_name();
        let info = &sock.info;
        let port = swoole_rs_clients::address::address_port(info);
        let v6 = info.kind != SocketType::SW_SOCK_TCP.raw();
        // The family's address bytes, as they are stored.
        let (inet_v6, inet_v4) = (
            info.addr.v6().sin6_addr.s6_addr,
            info.addr.v4().sin_addr.s_addr,
        );
        let inet_v4 = inet_v4.to_ne_bytes();
        let addr: &[u8] = if v6 { &inet_v6 } else { &inet_v4 };
        let index = crate::local_addr::index(state, v6, addr);
        (!sock.ssl().is_null(), port, index)
    });
    conn.set_fd(sock.socket().fd());
    conn.set_reactor_id(reactor_id);
    conn.set_server_fd(server_fd);
    let now = swoole_rs_corebase::microtime();
    conn.set_last_recv_time(now);
    conn.set_connect_time(now);
    conn.closed().store(0, Relaxed);
    conn.set_worker_id(-1);
    conn.set_socket_type(ls.r#type);
    // The slot is empty: `reset_connection` cleared it, and each close takes its socket.
    conn.hold_socket(sock);
    conn.set_ssl(ssl);
    conn.info().kind = conn.socket_type();
    conn.set_local_port(local_port);
    conn.set_local_addr_index(local_addr_index);
    // Last: a reactor thread that sees the slot active (`is_active`) reads the fields
    // above, which this publishes.
    conn.active().store(true, Release);
}

/// `Server::abort_connection(reactor, port, sock)` of a connection's socket: counted,
/// then the connection closed.
pub(crate) fn abort(
    serv: view::Server<'_>,
    reactor: Reactor,
    port: Option<view::Port<'_>>,
    sock: Heap<'_>,
) {
    count_abort(serv, port);
    crate::conn_io::close_connection(serv, reactor, Some(sock));
}

/// `Server::abort_connection` of an accepted socket no connection took: counted, then
/// freed.
pub(crate) fn discard(serv: view::Server<'_>, port: Option<view::Port<'_>>, sock: OwnedSocket) {
    count_abort(serv, port);
    swoole_rs_tls::upstream::socket_free(Some(sock));
}

fn count_abort(serv: view::Server<'_>, port: Option<view::Port<'_>>) {
    serv.counters().abort_count.fetch_add(1, SeqCst);
    if let Some(gs) = port.and_then(|port| port.config().counters()) {
        gs.abort_count.fetch_add(1, SeqCst);
    }
}

/// `Server::connection_incoming(reactor, conn)`, over `port`, the one `conn` arrived
/// on: the idle timer, reading, and `onConnect`; false when one failed.
pub(crate) fn incoming(
    serv: view::Server<'_>,
    reactor: Reactor,
    port: view::Port<'_>,
    conn: Connection<'_>,
) -> bool {
    let Some(sock) = conn.socket() else {
        return false;
    };
    let idle = port.config().max_idle_time.get();
    if idle > 0.0 {
        let timer = idle_timer(conn, idle);
        sock.edit(|s| {
            s.timeouts.read = idle;
            s.recv_timer = timer;
        });
    }
    if !sock.read(|s| s.ssl().is_null()) {
        return reactor.add(sock, Events::READ);
    }
    // Delayed receive waits for the resume command.
    if !serv.config().enable_delay_receive.get() && !reactor.add(sock, Events::READ) {
        return false;
    }
    if serv.has_callback(crate::callbacks::CallbackKind::Connect)
        && !crate::server_methods::notify(serv, conn, ServerEvent::Connect)
    {
        return false;
    }
    true
}

/// `swoole_timer_add(sec2msec(idle), true, Server::get_timeout_callback(...))`: the
/// persistent idle check of `conn`, on this thread's reactor, or none.
pub(crate) fn idle_timer(
    conn: Connection<'_>,
    idle: f64,
) -> Option<swoole_rs_corebase::timer::Held> {
    // sec2msec's (long) cast.
    let msec = (idle * 1000.0).c_cast::<c_long>();
    let fd = c_long::from(conn.fd());
    swoole_rs_corebase::timer::add_fn(msec, true, move || {
        // The server of the reactor that armed the timer: this thread's.
        let Some(reactor) = Reactor::current() else {
            return;
        };
        // A session socket's connection is its fd's slot, which deletes this timer
        // before it goes.
        if let Some(serv) = reactor.server()
            && let Some(conn) = serv.state().connection(fd, Lookup::ByFd)
        {
            idle_timeout(serv, reactor, conn);
        }
    })
}

/// The idle timer: closes a connection idle in both directions for `max_idle_time`.
pub(crate) fn idle_timeout(serv: view::Server<'_>, reactor: Reactor, conn: Connection<'_>) {
    if conn.protect() {
        return;
    }
    let idle = serv
        .port_by_server_fd(conn.server_fd())
        .map_or(0.0, |port| port.config().max_idle_time.get());
    let now = swoole_rs_corebase::reactor::steady_ms();
    let (received, sent) = conn
        .socket()
        .map_or_default(|s| s.read(|s| (s.last_received_time, s.last_sent_time)));
    if now - received < idle && now - sent < idle {
        return;
    }
    if serv.config().disable_notify.get() || conn.closed().load(Relaxed) != 0 || conn.close_force()
    {
        crate::conn_io::close_connection(serv, reactor, conn.socket());
        return;
    }
    conn.set_close_force(true);
    let mut event = conn.event(0, 0);
    reactor.trigger_close(&mut event);
}

/// `Server::disable_accept`: stop accepting on stream ports until the retry timer.
pub(crate) fn disable_accept(serv: view::Server<'_>) {
    /// `SW_ACCEPT_RETRY_TIME`, in milliseconds.
    #[expect(clippy::cast_possible_truncation, reason = "1000.0")]
    const RETRY_MSEC: c_long = (SW_ACCEPT_RETRY_TIME * 1000.0) as c_long;
    let timer = swoole_rs_corebase::timer::add_fn(RETRY_MSEC, false, || {
        // The server of the reactor that armed the timer: this thread's.
        if let Some(serv) = Reactor::current().and_then(Reactor::server) {
            enable_accept(serv);
        }
    });
    let tables = serv.state().tables_ref();
    tables.enable_accept_timer.store(timer);
    if timer.is_none() {
        return;
    }
    for_stream_ports(serv, |reactor, sock| reactor.del(*sock));
}

/// The accept retry timer: accept on stream ports again.
pub(crate) fn enable_accept(serv: view::Server<'_>) {
    for_stream_ports(serv, |reactor, sock| reactor.add(sock, Events::READ));
    let tables = serv.state().tables_ref();
    tables.enable_accept_timer.store(None);
}

/// `swoole_event_add`/`_del` on each stream port's listening socket.
fn for_stream_ports(serv: view::Server<'_>, change: impl Fn(Reactor, Heap<'_>) -> bool) {
    let reactor = Reactor::thread();
    for port in serv.ports() {
        if !SocketType::from_raw(port.config().r#type).is_some_and(SocketType::is_dgram) {
            port.with_socket(|sock| change(reactor, sock));
        }
    }
}

/// `Server::set_max_connection`: 0 means the default, bounded by the session list
/// and `SwooleG.max_sockets`.
pub fn set_max_connection(serv: view::Server<'_>, requested: u32) {
    let tables = serv.state().tables_ref();
    if tables.connections_mapped() {
        let m = format_args!("max_connection must be set before server create");
        log::warning(c"Server::set_max_connection", m);
        return;
    }
    let max_sockets = swoole_rs_corebase::max_sockets();
    let mut value = requested;
    if value == 0 {
        value = MAX_CONNECTION.min(max_sockets);
    } else if value > SESSION_LIST_SIZE {
        value = SESSION_LIST_SIZE;
        let m = format_args!(
            "max_connection is exceed the SW_SESSION_LIST_SIZE, it's reset to {SESSION_LIST_SIZE}"
        );
        log::warning(c"Server::set_max_connection", m);
    }
    if max_sockets > 0 && value > max_sockets {
        value = max_sockets;
        let m = format_args!("max_connection is exceed the maximum value, it's reset to {value}");
        log::warning(c"Server::set_max_connection", m);
    }
    tables.set_max_connection(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use std::io::{ErrorKind, Read};
    use std::net::TcpStream;

    const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;

    /// The read handler's status, which upstream's accept always returns `SW_OK`.
    fn accept_on(live: &LiveServer) -> c_int {
        let event = live.accept_event();
        accept(live.server(), live.reactor(), &event);
        OK
    }

    #[test]
    fn eagain_ends_an_empty_batch() {
        let live = LiveServer::new();
        assert_eq!(accept_on(&live), OK, "nothing to accept");
        assert_eq!(live.counters().accept_count.load(Relaxed), 0, "no accept");
    }

    #[test]
    fn too_many_connections_aborts_and_disables_accept() {
        let live = LiveServer::new();
        set_tables_max(&live, 1);
        let mut client = live.connect();
        assert_eq!(accept_on(&live), OK, "a full table is not fatal");
        let gs = live.counters();
        assert_eq!(gs.abort_count.load(Relaxed), 1, "aborted");
        assert_eq!(
            live.port_counters().abort_count.load(Relaxed),
            1,
            "on the port"
        );
        assert_eq!(gs.accept_count.load(Relaxed), 0, "no connection made");
        assert!(
            live.state().tables_ref().enable_accept_timer.is_some(),
            "accepting waits for the retry timer"
        );
        assert_closed(live, &mut client);
    }

    #[test]
    fn a_full_session_list_aborts() {
        let live = LiveServer::new();
        // Every session `max_connection` tries after the round is taken.
        for id in 1..=crate::fixture::CONNECTIONS {
            let id = c_long::try_from(id).unwrap();
            live.state().session(id).unwrap().set_fd(1);
        }
        let mut client = live.connect();
        assert_eq!(accept_on(&live), OK, "not fatal");
        let gs = live.counters();
        assert_eq!(gs.abort_count.load(Relaxed), 1, "aborted");
        assert_eq!(gs.session_round.load(Relaxed), 0, "the round stays");
        assert_eq!(gs.spinlock.load(Relaxed), 0, "the lock is released");
        assert_closed(live, &mut client);
    }

    #[test]
    fn session_ids_continue_from_the_round_and_skip_taken_slots() {
        let live = LiveServer::new();
        live.counters().session_round.store(41, Relaxed);
        live.state().session(42).unwrap().set_fd(1);
        let _client = live.connect();
        assert_eq!(accept_on(&live), OK);
        let gs = live.counters();
        assert_eq!(
            gs.session_round.load(Relaxed),
            43,
            "bound to the next free id"
        );
        let session = live.state().session(43).unwrap();
        let conn = live
            .state()
            .tables_ref()
            .connection_at(session.fd())
            .unwrap();
        assert_eq!(conn.session_id(), 43, "the connection has it");
        assert_eq!(conn.server_fd(), live.listener_fd, "from the listener");
        assert!(conn.is_active(), "active");
        conn.socket().unwrap().read(|s| {
            assert!(s.direct_send, "plain sockets send directly");
            assert_eq!(s.chunk_size, SW_SEND_BUFFER_SIZE, "chunked");
            assert_eq!(
                s.object,
                conn.as_ptr().cast(),
                "the socket names its connection"
            );
        });
    }

    #[test]
    fn single_thread_reads_inline_and_counts_the_fd_range() {
        let live = LiveServer::new();
        live.server().config().single_thread.set(true);
        let _first = live.connect();
        assert_eq!(accept_on(&live), OK, "EAGAIN ends the batch");
        let _second = live.connect();
        assert_eq!(accept_on(&live), OK, "EAGAIN ends the batch");
        let gs = live.counters();
        assert_eq!(gs.accept_count.load(Relaxed), 2, "both accepted");
        assert_eq!(gs.connection_num.load(Relaxed), 2, "both counted");
        let first = live.state().session(1).unwrap().fd();
        let second = live.state().session(2).unwrap().fd();
        assert_eq!(
            gs.max_fd.load(Relaxed),
            first.max(second),
            "maxfd grows to the highest fd"
        );
        let conn = live.state().tables_ref().connection_at(second).unwrap();
        assert!(reading(*conn.socket().unwrap()), "reading");
    }

    #[test]
    fn abort_closes_a_connection_through_the_reactor() {
        let live = LiveServer::new();
        live.server().config().single_thread.set(true);
        let mut client = live.connect();
        assert_eq!(accept_on(&live), OK);
        let fd = live.state().session(1).unwrap().fd();
        let sock = live
            .state()
            .tables_ref()
            .connection_at(fd)
            .unwrap()
            .socket()
            .unwrap();
        let port = live.server().port_by_server_fd(live.listener_fd);
        abort(live.server(), live.reactor(), port, sock);
        assert_eq!(live.counters().abort_count.load(Relaxed), 1, "counted");
        assert_eq!(
            live.port_counters().abort_count.load(Relaxed),
            1,
            "on the port"
        );
        assert_closed(live, &mut client);
    }

    #[test]
    fn delayed_receive_waits_for_the_resume() {
        let live = LiveServer::new();
        live.server().config().single_thread.set(true);
        live.server().config().enable_delay_receive.set(true);
        let _client = live.connect();
        assert_eq!(accept_on(&live), OK);
        let fd = live.state().session(1).unwrap().fd();
        let sock = live
            .state()
            .tables_ref()
            .connection_at(fd)
            .unwrap()
            .socket()
            .unwrap();
        assert!(!reading(*sock), "not read yet");
    }

    #[test]
    fn idle_timeout_closes_only_idle_connections() {
        let live = LiveServer::new();
        // `idle_timeout` compares it with milliseconds; a second outlasts Miri's slowdown.
        live.port().max_idle_time.set(1000.0);
        let _client = live.connect();
        live.server().config().single_thread.set(true);
        accept_on(&live);
        let fd = live.state().session(1).unwrap().fd();
        let conn = live.state().tables_ref().connection_at(fd).unwrap();
        let sock = conn.socket().unwrap();
        assert!(
            sock.read(|s| s.recv_timer).is_some(),
            "the idle timer is armed"
        );
        let now = swoole_rs_corebase::reactor::steady_ms();
        sock.edit(|s| {
            s.last_received_time = now;
            s.last_sent_time = now;
        });
        idle_timeout(live.server(), live.reactor(), conn);
        assert!(!conn.close_force(), "active");

        conn.set_protect(true);
        sock.edit(|s| s.last_received_time = 0.0);
        idle_timeout(live.server(), live.reactor(), conn);
        assert!(!conn.close_force(), "protected");

        conn.set_protect(false);
        idle_timeout(live.server(), live.reactor(), conn);
        assert!(conn.close_force(), "idle receive closes with notify");
    }

    #[test]
    fn accept_retry_reenables_stream_ports() {
        let live = LiveServer::new();
        let listener = live.port_socket();
        assert!(live.reactor().add(listener, Events::READ), "reading");
        disable_accept(live.server());
        let tables = live.state().tables_ref();
        assert!(tables.enable_accept_timer.is_some(), "armed");
        assert!(!reading(*listener), "no longer reading");
        enable_accept(live.server());
        assert!(!tables.enable_accept_timer.is_some(), "cleared");
        assert!(reading(*listener), "reading again");
    }

    #[test]
    fn eintr_retries_and_running_out_of_descriptors_disables_accept() {
        let live = LiveServer::new();
        let listener = live.port_socket();
        assert!(live.reactor().add(listener, Events::READ), "reading");
        let tables = live.state().tables_ref();
        let armed = || tables.enable_accept_timer.is_some();
        assert!(accept_failed(live.server(), libc::EINTR), "EINTR retries");
        assert!(!accept_failed(live.server(), libc::EAGAIN), "EAGAIN ends");
        assert!(
            !accept_failed(live.server(), libc::ECONNABORTED),
            "others end"
        );
        assert!(
            !armed() && reading(*listener),
            "none of them disables accept"
        );
        for errno in [libc::EMFILE, libc::ENFILE] {
            assert!(!accept_failed(live.server(), errno), "{errno} ends");
            assert!(armed(), "{errno} arms the retry timer");
            assert!(!reading(*listener), "{errno} stops reading");
            enable_accept(live.server());
        }
    }

    #[test]
    fn max_connection_bounds() {
        let live = LiveServer::new();
        let before = live.server().max_connection();
        set_max_connection(live.server(), 7);
        assert_eq!(live.server().max_connection(), before, "too late");

        // SAFETY: the fixture's tables, which no other thread reads.
        let tables = unsafe { &mut *live.state().tables() };
        let list = core::mem::take(tables.connection_list_mut());
        let run = |requested| {
            set_max_connection(live.server(), requested);
            live.server().max_connection()
        };
        let sockets = swoole_rs_corebase::max_sockets();
        let bound = |value: u32| {
            if sockets > 0 {
                value.min(sockets)
            } else {
                value
            }
        };
        assert_eq!(run(0), MAX_CONNECTION.min(sockets), "default");
        assert_eq!(run(5), bound(5), "as asked");
        assert_eq!(
            run(2_000_000),
            bound(SESSION_LIST_SIZE),
            "session list bound"
        );
        // SAFETY: as above.
        *unsafe { &mut *live.state().tables() }.connection_list_mut() = list;
    }

    /// Whether the reactor reads `sock`.
    fn reading(sock: swoole_rs_corebase::reactor::Socket<'_>) -> bool {
        sock.read(|s| !s.removed && s.events.intersects(Events::READ))
    }

    fn set_tables_max(live: &LiveServer, max: u32) {
        // SAFETY: the fixture's tables, which no other thread reads.
        unsafe { live.state().tables_ref().set_max_connection_unchecked(max) };
    }

    /// The peer sees the server close its end, once the reactor has run the close it
    /// deferred: here, as it goes.
    fn assert_closed(live: LiveServer, client: &mut TcpStream) {
        drop(live);
        client
            .set_read_timeout(Some(core::time::Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0];
        match client.read(&mut byte) {
            Ok(0) => {}
            Err(e) if e.kind() == ErrorKind::ConnectionReset => {}
            other => panic!("still open: {other:?}"),
        }
    }
}
