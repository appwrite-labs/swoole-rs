//! Accepting connections and their idle and health checks.
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};

use swoole_rs_corebase::constants::{SW_ACCEPT_RETRY_TIME, SW_SEND_BUFFER_SIZE};
use swoole_rs_corebase::{CCast, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Event, Socket};

use crate::reactor_handle::{EVENT_READ, Reactor};
use crate::records::ConnectionRecord;
use crate::{Connection, MODE_BASE, view};

/// `SW_ACCEPT_MAX_COUNT`, `SW_MAX_CONNECTION`, `SW_SESSION_LIST_SIZE`; native asserts each.
const ACCEPT_MAX_COUNT: u32 = 64;
const MAX_CONNECTION: u32 = 100_000;
const SESSION_LIST_SIZE: u32 = 1024 * 1024;
const OK: c_int = 0;
const ERR: c_int = -1;
/// `SW_SOCK_TCP`, `SW_SOCK_TCP6`.
const SOCK_TCP: c_int = 1;
const SOCK_TCP6: c_int = 3;
/// `SW_SERVER_EVENT_CONNECT`.
const EVENT_CONNECT: c_int = 5;
/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Server::accept_connection`: up to `SW_ACCEPT_MAX_COUNT` sockets per readable
/// event on `reactor`, this thread's.
pub(crate) fn accept(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> c_int {
    let server_fd = event.fd();
    let Some(listener) = event.socket() else {
        return OK;
    };
    // SAFETY: a listening socket's object is its port, which outlives its reactor.
    let Some(port) = (unsafe { view::Port::from_raw(listener.read(|l| l.object)) }) else {
        return OK;
    };
    for _ in 0..ACCEPT_MAX_COUNT {
        let accepted = listener.edit(swoole_rs_reactor::SocketFields::accept);
        // SAFETY: null, or the socket `accept` just made, which nothing else holds
        // until a connection takes it or `abort` frees it.
        let Some(sock) = (unsafe { Socket::from_ptr(accepted) }) else {
            match errno() {
                libc::EAGAIN => return OK,
                libc::EINTR => continue,
                libc::EMFILE | libc::ENFILE => disable_accept(serv),
                _ => {}
            }
            log::sys_warning(
                c"Server::accept_connection",
                format_args!("accept() failed"),
            );
            return OK;
        };
        if i64::from(sock.fd()) >= i64::from(serv.max_connection()) {
            let m = format_args!("Too many connections [now: {}]", sock.fd());
            log::error_log(
                LOG_WARNING,
                E::SW_ERROR_SERVER_TOO_MANY_SOCKET as c_int,
                c"Server::accept_connection",
                m,
            );
            abort(serv, reactor, Some(port), sock);
            disable_accept(serv);
            return OK;
        }
        if port.config().ssl != 0 {
            if !(crate::port_config::methods().ssl_create)(port.raw(), Some(sock)) {
                abort(serv, reactor, Some(port), sock);
                return OK;
            }
        } else {
            sock.edit(|s| {
                s.take_ssl();
            });
        }
        let Some(conn) = add(serv, port, sock, server_fd) else {
            abort(serv, reactor, Some(port), sock);
            return OK;
        };
        sock.edit(|s| s.chunk_size = SW_SEND_BUFFER_SIZE);
        let started = if serv.config().single_thread.get() {
            incoming(serv, reactor, port, conn) >= 0
        } else {
            to_reactor_thread(serv, conn) >= 0
        };
        if !started {
            abort(serv, reactor, Some(port), sock);
            return OK;
        }
    }
    OK
}

/// `serv->send_to_reactor_thread()` of the connection's `SW_SERVER_EVENT_INCOMING`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn to_reactor_thread(serv: view::Server<'_>, conn: Connection<'_>) -> isize {
    /// `SW_SERVER_EVENT_INCOMING`.
    const EVENT_INCOMING: u8 = 15;
    let head = swoole_rs_corebase::DataHead {
        fd: conn.session_id(),
        reactor_id: conn.reactor_id().into(),
        kind: EVENT_INCOMING,
        server_fd: u16::try_from(conn.server_fd()).unwrap_or(u16::MAX),
        ..swoole_rs_corebase::DataHead::default()
    };
    crate::event_worker_ops::send_to_reactor_thread(serv, &head)
}

/// Without the event workers, the reactor threads take the connection.
#[cfg(not(all(feature = "upstream-symbols", not(test))))]
fn to_reactor_thread(_: view::Server<'_>, _: Connection<'_>) -> isize {
    0
}

/// `Server::add_connection(port, sock, server_fd)`: the next free session slot after
/// `session_round` takes `sock`, and its fd's connection slot is set up; `None` when
/// every slot is taken.
pub fn add<'a>(
    serv: view::Server<'a>,
    port: view::Port<'_>,
    sock: Socket<'_>,
    server_fd: c_int,
) -> Option<Connection<'a>> {
    let state = serv.state();
    let config = serv.config();
    let ls = port.config();
    let gs = serv.counters();
    let fd = sock.fd();
    let conn = state.tables_ref().connection_at(fd)?;
    // C++'s ReactorId (uint8_t).
    let reactor_id = if u64::from(config.mode_) == MODE_BASE {
        swoole_rs_process::methods::swoole_get_worker_id().c_cast::<u8>()
    } else {
        (fd % c_int::from(config.reactor_num.get())).c_cast::<u8>()
    };
    // The fd's last connection, closed on a reactor thread, published the slot with
    // its last write: acquire that before reusing it.
    conn.closed().load(Acquire);
    // Upstream zeroes the slot up to `closed`; other reactor threads read `active`.
    conn.active().store(0, Relaxed);
    let start = core::mem::offset_of!(ConnectionRecord, socket_type);
    let bytes = conn.as_ptr().cast::<u8>().wrapping_add(start);
    // SAFETY: the same slot, from after `active` up to `closed`.
    unsafe { bytes.write_bytes(0, core::mem::offset_of!(ConnectionRecord, closed) - start) };

    // `sw_spinlock`, with `SW_SPINLOCK_LOOP_N`.
    let cpus = u32::from(swoole_rs_corebase::swr_v1_core_cpu_num());
    swoole_rs_utilities::swr_v1_utils_spinlock(Some(&gs.spinlock), cpus, 1024);
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
        return None;
    }
    gs.session_round.store(session_id, Relaxed);
    conn.set_session_id(session_id);
    let buffer_size = ls.socket_buffer_size.get().cast_unsigned();
    sock.edit(|s| {
        s.object = conn.as_ptr().cast();
        s.removed = true;
        s.output.buffer_size = buffer_size;
        s.timeouts.write = 0.0;
        s.timeouts.read = 0.0;
    });

    let listener = ls.listening_socket();
    let tcp = listener.is_some_and(|l| matches!(l.read(|l| l.socket_type), SOCK_TCP | SOCK_TCP6));
    if ls.open_tcp_nodelay.get() && tcp {
        if !sock.edit(|s| s.set_tcp_nodelay(1)) {
            log::sys_warning(
                c"Server::add_connection",
                format_args!("setsockopt(TCP_NODELAY) failed"),
            );
        }
        sock.edit(|s| s.enable_tcp_nodelay = true);
    }
    // Upstream sets the buffer sizes on the listening socket.
    let listen_fd = listener.map_or(-1, Socket::fd);
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
            && swoole_rs_corebase::sys::setsockopt(listen_fd, libc::SOL_SOCKET, name, &size) != 0
        {
            let m = format_args!("setsockopt({label}, {size}) failed");
            log::sys_warning(c"Server::add_connection", m);
        }
    }
    init(serv, conn, ls, sock, server_fd, reactor_id);
    if ls.ssl == 0 {
        sock.edit(|s| s.direct_send = true);
    }

    state.lock();
    if fd > gs.max_fd() {
        gs.max_fd.store(fd, Relaxed);
    } else if fd < gs.min_fd() {
        gs.min_fd.store(fd, Relaxed);
    }
    state.unlock();

    gs.accept_count.fetch_add(1, Relaxed);
    let port_gs = ls.counters();
    if let Some(port_gs) = port_gs {
        port_gs.accept_count.fetch_add(1, Relaxed);
    }
    if u64::from(config.mode_) == MODE_BASE {
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
    Some(conn)
}

/// The connection's fields from its socket: the peer address, then the local one
/// `getsockname` writes over it in the socket.
fn init(
    serv: view::Server<'_>,
    conn: Connection<'_>,
    ls: &crate::port_config::PortConfig,
    sock: Socket<'_>,
    server_fd: c_int,
    reactor_id: u8,
) {
    let state = serv.state();
    let (ssl, local_port, local_addr_index) = sock.edit(|sock| {
        conn.set_info(sock.info);
        sock.get_name();
        let info = &sock.info;
        let port = swoole_rs_clients::address::swr_v1_clients_address_port(info);
        let v6 = info.kind != SOCK_TCP;
        // The address field by offset: raw borrows of the union's fields are safe.
        let addr = if v6 {
            let at = core::mem::offset_of!(libc::sockaddr_in6, sin6_addr);
            (&raw const info.addr.inet_v6).cast::<u8>().wrapping_add(at)
        } else {
            let at = core::mem::offset_of!(libc::sockaddr_in, sin_addr);
            (&raw const info.addr.inet_v4).cast::<u8>().wrapping_add(at)
        };
        // SAFETY: the address of the family, in the socket's live record.
        let addr = unsafe { core::slice::from_raw_parts(addr, if v6 { 16 } else { 4 }) };
        let index = crate::local_addr::index(state, v6, addr);
        (!sock.ssl().is_null(), port, index)
    });
    conn.set_fd(sock.fd());
    conn.set_reactor_id(reactor_id);
    conn.set_server_fd(server_fd);
    let now = swoole_rs_corebase::microtime();
    conn.set_last_recv_time(now);
    conn.set_connect_time(now);
    conn.active().store(1, Relaxed);
    conn.closed().store(0, Relaxed);
    conn.set_worker_id(-1);
    conn.set_socket_type(ls.r#type);
    conn.set_socket(sock.as_ptr().cast());
    conn.set_ssl(ssl);
    conn.info().kind = conn.socket_type();
    conn.set_local_port(u16::try_from(local_port).unwrap_or_default());
    conn.set_local_addr_index(local_addr_index);
}

/// `Server::abort_connection(reactor, port, sock)`: counted, then closed through the
/// reactor when it is a connection's, else freed.
pub(crate) fn abort(
    serv: view::Server<'_>,
    reactor: Reactor,
    port: Option<view::Port<'_>>,
    sock: Socket<'_>,
) {
    serv.counters().abort_count.fetch_add(1, SeqCst);
    if let Some(gs) = port.and_then(|port| port.config().counters()) {
        gs.abort_count.fetch_add(1, SeqCst);
    }
    if sock.read(|s| !s.object.is_null()) {
        reactor.close(sock);
    } else {
        // SAFETY: an accepted socket no connection took, which nothing uses afterwards.
        unsafe { swoole_rs_tls::upstream::socket_free(sock.as_ptr()) };
    }
}

/// `Server::connection_incoming(reactor, conn)`, over `port`, the one `conn` arrived
/// on: the idle timer, reading, and `onConnect`.
pub(crate) fn incoming(
    serv: view::Server<'_>,
    reactor: Reactor,
    port: view::Port<'_>,
    conn: Connection<'_>,
) -> c_int {
    let Some(sock) = conn.socket() else {
        return ERR;
    };
    let idle = port.config().max_idle_time.get();
    if idle > 0.0 {
        // sec2msec's (long) cast.
        let msec = (idle * 1000.0).c_cast::<c_long>();
        // SAFETY: `idle_tick` takes the live connection, which deletes the timer
        // before it goes.
        let node = unsafe {
            swoole_rs_timer::swr_v1_timer_add_plain(msec, true, idle_tick, conn.as_ptr().cast())
        };
        sock.edit(|s| {
            s.timeouts.read = idle;
            s.recv_timer = node;
        });
    }
    if !sock.read(|s| s.ssl().is_null()) {
        return reactor.add(sock, EVENT_READ);
    }
    // Delayed receive waits for the resume command.
    if !serv.config().enable_delay_receive.get() && reactor.add(sock, EVENT_READ) < 0 {
        return ERR;
    }
    if serv.has_callback(crate::callbacks::CB_CONNECT)
        && !crate::server_methods::notify(serv, conn, EVENT_CONNECT)
    {
        return ERR;
    }
    OK
}

/// `Server::get_timeout_callback`'s timer: the idle check of `conn`, a live
/// connection of `SwooleG.server` on this thread's reactor, which added the timer.
///
/// # Safety
/// As the timer's contract: `conn` is that connection.
pub(crate) unsafe extern "C-unwind" fn idle_tick(conn: *mut c_void) {
    // SAFETY: the connection's server, live for the tick per the caller.
    let Some(serv) = (unsafe { view::Server::current() }) else {
        return;
    };
    // SAFETY: as the caller guarantees.
    if let Some(conn) = unsafe { Connection::from_ptr(conn.cast()) } {
        idle_timeout(serv, Reactor::thread(), conn);
    }
}

/// The idle timer: closes a connection idle in both directions for `max_idle_time`.
pub(crate) fn idle_timeout(serv: view::Server<'_>, reactor: Reactor, conn: Connection<'_>) {
    if conn.protect() {
        return;
    }
    let idle = serv
        .port_by_server_fd(conn.server_fd())
        .map_or(0.0, |port| port.config().max_idle_time.get());
    let now = swoole_rs_reactor::steady_ms();
    let (received, sent) = conn
        .socket()
        .map(|s| s.read(|s| (s.last_received_time, s.last_sent_time)))
        .unwrap_or_default();
    if now - received < idle && now - sent < idle {
        return;
    }
    if serv.config().disable_notify || conn.closed().load(Relaxed) != 0 || conn.close_force() {
        crate::reactor_entry::close_socket(serv, reactor, conn.socket());
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
    let raw = serv.raw();
    let timer = swoole_rs_timer::add_fn(RETRY_MSEC, false, move || {
        // SAFETY: the server that armed this one-shot timer, on its own thread; it
        // outlives its timers, which `clear_timer` deletes before it goes.
        enable_accept(unsafe { view::Server::from_raw(raw) });
    });
    let tables = serv.state().tables_ref();
    tables.enable_accept_timer.store(timer, Relaxed);
    if timer.is_null() {
        return;
    }
    for_stream_ports(serv, Reactor::del);
}

/// The accept retry timer: accept on stream ports again.
pub(crate) fn enable_accept(serv: view::Server<'_>) {
    for_stream_ports(serv, |reactor, sock| reactor.add(sock, EVENT_READ));
    let tables = serv.state().tables_ref();
    tables
        .enable_accept_timer
        .store(core::ptr::null_mut(), Relaxed);
}

/// `swoole_event_add`/`_del` on each stream port's listening socket.
fn for_stream_ports(serv: view::Server<'_>, change: impl Fn(Reactor, Socket<'_>) -> c_int) {
    let reactor = Reactor::thread();
    for port in serv.ports() {
        if !crate::reactor_ops::is_dgram(port.config().r#type) {
            port.with_socket(|sock| change(reactor, sock));
        }
    }
}

/// `Server::set_max_connection`: 0 means the default, bounded by the session list
/// and `SwooleG.max_sockets`.
pub fn set_max_connection(serv: view::Server<'_>, requested: u32) {
    let state = serv.state();
    if !state.tables_ref().connection_list.is_null() {
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
    // SAFETY: before `Server::create` (no connection list yet), so no thread or
    // process reads the tables, and the reference ends with the write.
    unsafe { &mut *state.tables() }.max_connection = value;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use std::io::{ErrorKind, Read};
    use std::net::TcpStream;

    fn accept_on(live: &LiveServer) -> c_int {
        // SAFETY: the listener, live for the event's life.
        let event = unsafe { Event::new(live.listener_fd, 0, 1, live.listener().cast()) };
        accept(live.server(), live.reactor(), &event)
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn eagain_ends_an_empty_batch() {
        let live = LiveServer::new();
        assert_eq!(accept_on(&live), OK, "nothing to accept");
        assert_eq!(live.counters().accept_count.load(Relaxed), 0, "no accept");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
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
            !live
                .state()
                .tables_ref()
                .enable_accept_timer
                .load(Relaxed)
                .is_null(),
            "accepting waits for the retry timer"
        );
        assert_closed(live, &mut client);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
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
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
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
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn single_thread_reads_inline_and_counts_the_fd_range() {
        let live = LiveServer::new();
        live.server().config().single_thread.set(true);
        let _first = live.connect();
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
        assert!(reading(conn.socket().unwrap()), "reading");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
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
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
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
        assert!(!reading(sock), "not read yet");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn idle_timeout_closes_only_idle_connections() {
        let live = LiveServer::new();
        live.port().max_idle_time.set(5.0);
        let _client = live.connect();
        live.server().config().single_thread.set(true);
        accept_on(&live);
        let fd = live.state().session(1).unwrap().fd();
        let conn = live.state().tables_ref().connection_at(fd).unwrap();
        let sock = conn.socket().unwrap();
        assert!(
            !sock.read(|s| s.recv_timer).is_null(),
            "the idle timer is armed"
        );
        let now = swoole_rs_reactor::steady_ms();
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
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn accept_retry_reenables_stream_ports() {
        let live = LiveServer::new();
        let listener = live.port_socket();
        assert_eq!(live.reactor().add(listener, EVENT_READ), OK, "reading");
        disable_accept(live.server());
        let tables = live.state().tables_ref();
        assert!(!tables.enable_accept_timer.load(Relaxed).is_null(), "armed");
        assert!(!reading(listener), "no longer reading");
        enable_accept(live.server());
        assert!(
            tables.enable_accept_timer.load(Relaxed).is_null(),
            "cleared"
        );
        assert!(reading(listener), "reading again");
    }

    #[test]
    fn max_connection_bounds() {
        let live = LiveServer::new();
        let before = live.server().max_connection();
        set_max_connection(live.server(), 7);
        assert_eq!(live.server().max_connection(), before, "too late");

        // SAFETY: the fixture's tables, which no other thread reads.
        let tables = unsafe { &mut *live.state().tables() };
        let list = core::mem::take(&mut tables.connection_list);
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
        unsafe { &mut *live.state().tables() }.connection_list = list;
    }

    /// Whether the reactor reads `sock`.
    fn reading(sock: Socket<'_>) -> bool {
        sock.read(|s| !s.removed && s.events & EVENT_READ != 0)
    }

    fn set_tables_max(live: &LiveServer, max: u32) {
        // SAFETY: the fixture's tables, which no other thread reads.
        unsafe { &mut *live.state().tables() }.max_connection = max;
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
