//! A connection's read, write and close paths (`src/server/reactor_thread.cc`):
//! `ReactorThread_onRead`, `ReactorThread_onWrite`, `ReactorThread_onClose` and
//! `Server::close_connection`.
//!
//! `--fix-halfclose` (M15) builds have the `halfclose` feature (upstream's
//! `SW_HALFCLOSE_PREQUEUE`): a synchronous process worker keeps its ordered response
//! after a clean FIN, `close_notified` is the terminal read state, and a TLS close notify
//! still owed is queued behind the output (`native/halfclose/prequeue.md`).
use crate::Mode;
use core::ffi::{c_int, c_long};
use core::sync::atomic::Ordering::{Relaxed, Release, SeqCst};
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::reactor::Events;

use swoole_rs_corebase::buffer::EntryKind;
use swoole_rs_corebase::buffer::cpp::{
    new_buffer, swr_cpp_buffer_empty, swr_cpp_buffer_length, swr_cpp_buffer_typed_close,
    swr_cpp_buffer_typed_front,
};
use swoole_rs_corebase::buffer::string::with_thread_buffer;
use swoole_rs_corebase::constants::{SW_SEND_BUFFER_SIZE, SocketType};
use swoole_rs_corebase::reactor::{Event, Heap, Socket, what_event_want};
use swoole_rs_corebase::timer::{del_id, delay_held};
use swoole_rs_corebase::tls::Accept;
use swoole_rs_corebase::tls::upstream as tls;
use swoole_rs_corebase::{CCast, DataHead, microtime};

use crate::base_factory::SendData;
use crate::callbacks::CallbackKind;
use crate::hybrid::hybrid;
use crate::port_config::{PortConfig, ReadEvent};
use crate::reactor_handle::Reactor;
use crate::reactor_ops::{out_buffer, valid};
use crate::records::{Connection, Lookup};
use crate::view;

/// `swReturnCode`'s handshake states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handshake {
    Continue,
    Wait,
    Error,
    Ready,
}

/// The longest resume back-off, in milliseconds.
const MAX_WAITING_TIME: u16 = 1024;

/// `ReactorThread_onRead`: finish a pending TLS (or DTLS) handshake, then let the
/// port read, and pause reading while the workers hold too many of its bytes; false
/// where upstream returns `SW_ERR`.
pub(crate) fn read(serv: view::Server<'_>, reactor: Reactor, event: &mut Event) -> bool {
    read_as(BUILT_PREQUEUE, serv, reactor, event)
}

/// [`read`], as a build with `prequeue` (the `halfclose` feature) or without.
fn read_as(prequeue: bool, serv: view::Server<'_>, reactor: Reactor, event: &mut Event) -> bool {
    let state = serv.state();
    let fd = event.fd();
    let conn = state.connection(c_long::from(fd), Lookup::ByFd);
    // The server closed the connection (and the fd may be reused) before the event.
    let Some(conn) = conn.filter(|conn| conn.server_fd() != 0) else {
        return true;
    };
    if prequeue {
        let Some((notified, want_read)) = event
            .socket()
            .map(|s| s.read(|s| (s.close_notified, !s.ssl().is_null() && s.tls.want_read)))
        else {
            return true;
        };
        if notified {
            return true;
        }
        // Once a close is queued, a read only drives a TLS shutdown that wants one.
        if conn.close_queued() {
            return !want_read || write(serv, reactor, event);
        }
    }
    let Some(sock) = event.socket() else {
        return true;
    };
    // The slot found above is `fd`'s, so its listener names the port.
    let port = serv.port_by_server_fd(conn.server_fd());
    if let Some(config) = port.map(view::Port::config)
        && sock.read(|s| SocketType::from_raw(s.socket_type).is_some_and(SocketType::is_dgram))
        && hybrid().dtls_read(config, *sock, fd)
    {
        crate::connection::abort(serv, reactor, port, sock);
        return true;
    }
    match verify_ssl(serv, reactor, port, *sock) {
        Handshake::Error => {
            crate::connection::abort(serv, reactor, port, sock);
            return true;
        }
        Handshake::Ready if sock.read(|s| s.dtls) => return true,
        Handshake::Wait => return true,
        Handshake::Ready | Handshake::Continue => {}
    }
    let Some(port) = port else {
        return false;
    };
    conn.set_last_recv_time(microtime());
    let last_recv_bytes = sock.read(|s| s.total_recv_bytes);
    let Some(on_read) = port.config().on_read.get() else {
        return false;
    };
    // A readable event on the live connection of `fd`, of this server's port `port`.
    let retval = on_read(reactor.handle(), port, ReadEvent::new(serv, event, conn));
    // Upstream reads the socket after the read too.
    let (total_recv_bytes, notified) = sock.read(|s| (s.total_recv_bytes, s.close_notified));
    let received = total_recv_bytes.wrapping_sub(last_recv_bytes);
    if received.cast_signed() > 0 {
        let received = received as u64;
        if let Some(gs) = port.config().counters() {
            gs.total_recv_bytes.fetch_add(received, SeqCst);
        }
        if let Some(gs) = crate::shared::server_counters(state) {
            gs.total_recv_bytes.fetch_add(received, SeqCst);
        }
    }
    if !conn.is_active() || prequeue && notified {
        return retval;
    }
    let config = state.config_ref();
    let max = config.max_queued_bytes.get();
    let process = config.mode_ == Mode::Process;
    if process && max != 0 && conn.recv_queued_bytes().load(SeqCst) > max {
        conn.set_waiting_time(1);
        if resume_timer(conn) {
            reactor.interest(sock, swoole_rs_corebase::reactor::Interest::RemoveRead);
        }
    }
    retval
}

/// `ReactorThread_verify_ssl_state`: finish a pending TLS handshake, then announce
/// the connection (with the client certificate when the port asks for one).
fn verify_ssl(
    serv: view::Server<'_>,
    reactor: Reactor,
    port: Option<view::Port<'_>>,
    sock: Socket<'_>,
) -> Handshake {
    if !handshaking(sock) {
        return Handshake::Continue;
    }
    match tls::socket_ssl_accept(sock) {
        Accept::Ready => {}
        Accept::Wait => return Handshake::Wait,
        Accept::Error => return Handshake::Error,
    }
    let state = serv.state();
    let Some(conn) = state.connection(c_long::from(sock.fd()), Lookup::ByFd) else {
        return Handshake::Error;
    };
    conn.set_ssl_ready(true);
    let config = port.map(view::Port::config);
    let (verify_peer, allow_self_signed) = config
        .and_then(|config| config.tls.settings.get())
        .map_or_default(|settings| (settings.verify_peer(), settings.allow_self_signed()));
    let peer =
        || with_thread_buffer(|buffer| tls::socket_ssl_peer_certificate_string(sock, buffer));
    let verified = || tls::socket_ssl_verify(sock, allow_self_signed);
    let wanted = config.is_some_and(client_cert_file);
    match certificate(wanted, verify_peer, peer, verified) {
        Certificate::Rejected => return Handshake::Error,
        Certificate::Dispatched => {
            // The peer's certificate filled this thread's buffer: a copy, which the
            // dispatch reads while the buffer is free to lend again.
            let cert = with_thread_buffer(|buffer| buffer.view().to_vec());
            let info = DataHead {
                fd: sock.fd().into(),
                kind: ServerEvent::Connect.code(),
                reactor_id: reactor.handle().id().cast_signed(),
                ..DataHead::default()
            };
            hybrid().factory_dispatch(serv, &mut SendData::new(info, &cert));
        }
        Certificate::None => {
            let on_connect = state.callbacks.slot(CallbackKind::Connect);
            if on_connect.is_set() {
                crate::server_methods::notify(serv, conn, ServerEvent::Connect);
            }
        }
    }
    if state.config_ref().enable_delay_receive.get() && !reactor.del(sock) {
        return Handshake::Error;
    }
    Handshake::Ready
}

/// What a finished handshake does with the client's certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Certificate {
    /// None wanted, or an optional one missing: `onConnect` announces the connection.
    None,
    /// The connect event carries it to the workers.
    Dispatched,
    Rejected,
}

/// The client certificate's fate when the port `wanted` one (it has a client
/// certificate file): the `peer` sent one, and it `verified` when `verify_peer`.
fn certificate(
    wanted: bool,
    verify_peer: bool,
    peer: impl FnOnce() -> bool,
    verified: impl FnOnce() -> bool,
) -> Certificate {
    if !wanted {
        Certificate::None
    } else if peer() {
        if !verify_peer || verified() {
            Certificate::Dispatched
        } else {
            Certificate::Rejected
        }
    } else if verify_peer {
        Certificate::Rejected
    } else {
        Certificate::None
    }
}

/// Whether the TLS port has a client certificate file.
fn client_cert_file(port: &PortConfig) -> bool {
    port.tls
        .settings
        .get()
        .is_some_and(|settings| settings.has_client_cert())
}

/// Arms `conn`'s resume timer for its paused `socket`, after its waiting time.
///
/// The connection's own socket, which deletes this timer before it goes.
fn resume_timer(conn: Connection<'_>) -> bool {
    let (msec, fd) = (c_long::from(conn.waiting_time()), c_long::from(conn.fd()));
    let timer = swoole_rs_corebase::timer::add_fn(msec, false, move || {
        // The server of the reactor that armed the timer: this thread's.
        if let Some(serv) = Reactor::current().and_then(Reactor::server) {
            resume_data_receiving(serv, fd);
        }
    });
    conn.set_timer(timer);
    timer.is_some()
}

/// `ReactorThread_resume_data_receiving`, the timer of a paused session socket: back
/// off (doubling up to 1024ms) while the workers still hold too many bytes, then
/// read again. The connection's close deletes the timer, so `fd`'s slot is its own.
fn resume_data_receiving(serv: view::Server<'_>, fd: c_long) {
    let state = serv.state();
    let Some((conn, socket)) = state
        .connection(fd, Lookup::ByFd)
        .and_then(|conn| Some((conn, conn.socket()?)))
    else {
        return;
    };
    if conn.recv_queued_bytes().load(SeqCst) > state.config_ref().max_queued_bytes.get() {
        let waiting = conn.waiting_time();
        if waiting != MAX_WAITING_TIME {
            conn.set_waiting_time(waiting.saturating_mul(2));
        }
        if resume_timer(conn) {
            return;
        }
    }
    // The firing timer's reactor: this thread's.
    Reactor::thread().interest(socket, swoole_rs_corebase::reactor::Interest::AddRead);
    conn.set_timer(None);
}

/// Built with the `halfclose` feature.
const BUILT_PREQUEUE: bool = cfg!(feature = "halfclose");

/// `SW_OK` and `SW_ERR` as the reactor's handlers return them.
const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;

/// A handler's success as the reactor's `SW_OK`/`SW_ERR`.
pub(crate) const fn status(ok: bool) -> c_int {
    if ok { OK } else { ERR }
}
/// `SW_EVENT_READ`, `SW_EVENT_WRITE` and `SW_SSL_STATE_READY`.
use swoole_rs_corebase::tls::SslState::SW_SSL_STATE_READY as SSL_READY;

/// Whether the socket has TLS whose handshake is not finished.
fn handshaking(sock: Socket<'_>) -> bool {
    sock.read(|s| !s.ssl().is_null() && s.tls.state != SSL_READY)
}

/// The socket's queued output, in bytes, and whether none is queued.
fn output(sock: Socket<'_>) -> (u32, bool) {
    sock.read(|s| {
        out_buffer(s).map_or((0, true), |b| {
            (swr_cpp_buffer_length(b), swr_cpp_buffer_empty(b))
        })
    })
}

/// `ReactorThread_onWrite`: notify a pending close, or flush the output buffer, then
/// report the drained buffer and stop watching for writes once it is empty.
pub(crate) fn write(serv: view::Server<'_>, reactor: Reactor, event: &mut Event) -> bool {
    let state = serv.state();
    let config = state.config_ref();
    let fd = event.fd();
    let port = serv.port_by_fd(fd).map(view::Port::config);
    if config.mode_ == Mode::Process {
        debug_assert_eq!(
            fd % c_int::from(config.reactor_num.get()),
            c_int::from(reactor.handle().id()),
            "a reactor thread writes only its own fds"
        );
    }
    let conn = state.connection(c_long::from(fd), Lookup::ByFd);
    // A session's socket is the event's.
    let Some((conn, sock)) = conn
        .filter(|conn| conn.is_active())
        .and_then(|conn| Some((conn, conn.socket()?)))
    else {
        return false;
    };
    if conn.close_notify() {
        if handshaking(*sock) {
            return close_connection(serv, reactor, Some(sock));
        }
        crate::server_methods::notify(serv, conn, ServerEvent::Close);
        conn.set_close_notify(false);
        return true;
    } else if config.disable_notify.get() && conn.close_force() {
        return close_connection(serv, reactor, Some(sock));
    }
    while let Some(kind) = sock.read(|s| {
        let buffer = out_buffer(s).filter(|b| !swr_cpp_buffer_empty(b))?;
        Some(swr_cpp_buffer_typed_front(buffer).kind)
    }) {
        let sent = match kind {
            EntryKind::Close => return close_connection(serv, reactor, Some(sock)),
            EntryKind::Sendfile => tls::socket_handle_sendfile(*sock),
            EntryKind::Data => {
                let sent = tls::socket_handle_send(*sock);
                if sent {
                    conn.set_send_queued_bytes(output(*sock).0);
                }
                sent
            }
        };
        if !sent {
            let (close_wait, send_wait) = sock.read(|s| (s.output.close_wait, s.output.send_wait));
            if close_wait {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                conn.set_close_errno(errno.c_cast::<u16>());
                return reactor.trigger_close(event);
            } else if send_wait {
                break;
            }
        }
    }
    let queued = output(*sock).0;
    if conn.overflow() && queued < sock.read(|s| s.output.buffer_size) {
        conn.set_overflow(false);
    }
    let buffer_empty = state.callbacks.slot(CallbackKind::BufferEmpty).is_set();
    let low_watermark = port.map_or(0, |port| port.buffer_low_watermark.get());
    if buffer_empty && conn.high_watermark() && queued <= low_watermark {
        conn.set_high_watermark(false);
        crate::server_methods::notify(serv, conn, ServerEvent::BufferEmpty);
    }
    // Read after the notification, which may queue more.
    let (timer, removed, empty) = sock.read(|s| {
        let empty = out_buffer(s).is_none_or(swr_cpp_buffer_empty);
        (s.output.send_timer, s.removed, empty)
    });
    if let Some(timer) = timer {
        if empty {
            del_id(timer);
            sock.edit(|s| s.output.send_timer = None);
        } else if let Some(port) = port {
            delay_held(Some(timer), port.max_idle_time.get().c_cast::<c_long>());
        }
    }
    if !conn.peer_closed() && !removed && empty {
        // A notified close no longer reads: it only drains.
        if BUILT_PREQUEUE {
            reactor.interest(sock, swoole_rs_corebase::reactor::Interest::RemoveWrite);
        } else {
            reactor.change(
                sock,
                swoole_rs_corebase::reactor::ChangeOp::Set,
                Events::READ,
            );
        }
    }
    true
}

/// `ReactorThread_onClose`: the peer closed. Close now when nobody is told or a
/// close is already queued, else mark the connection and notify the worker once.
pub(crate) fn peer_close(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> bool {
    let state = serv.state();
    let fd = event.fd();
    let conn = state.connection(c_long::from(fd), Lookup::ByFd);
    let (Some(conn), Some(sock)) = (conn.filter(|conn| conn.is_active()), event.socket()) else {
        return false;
    };
    if state.config_ref().disable_notify.get() {
        return close_connection(serv, reactor, Some(sock));
    }
    if !reactor.del(*sock) {
        return false;
    }
    if conn.close_queued() {
        return close_connection(serv, reactor, Some(sock));
    }
    if sock.read(|s| s.close_notified) {
        return true;
    }
    let mut head = DataHead {
        reactor_id: i16::try_from(reactor.handle().id()).unwrap_or_default(),
        fd: c_long::from(fd),
        kind: ServerEvent::Close.code(),
        ..DataHead::default()
    };
    // The connection is no longer available.
    conn.set_peer_closed(fin_ends(BUILT_PREQUEUE, state, conn));
    sock.edit(|s| s.close_notified = true);
    crate::hybrid::hybrid().factory_notify(serv, &mut head)
}

/// Whether the peer's FIN ends the connection. A synchronous process worker with
/// fixed-worker dispatch still owes its ordered response after a clean FIN; other
/// modes, TLS and fatal closes are terminal (always, without `prequeue`).
fn fin_ends(prequeue: bool, state: &crate::ServerState, conn: Connection<'_>) -> bool {
    let config = state.config_ref();
    let dispatch_func = state.callbacks.slot(CallbackKind::DispatchFunc).is_set();
    !prequeue
        || config.mode_ != Mode::Process
        || !config.dispatch_mode.get().is_hash()
        || dispatch_func
        || config.enable_coroutine.get()
        || conn.ssl()
        || conn.close_errno() != 0
        || conn.close_force()
        || conn.close_reset()
}

/// `Server::close_connection`: release what the connection holds, count the close,
/// clear its session and move the max fd down past it, then take the socket from the
/// connection and close it. False unless `sock` is its connection's.
pub(crate) fn close_connection(
    serv: view::Server<'_>,
    reactor: Reactor,
    sock: Option<Heap<'_>>,
) -> bool {
    let state = serv.state();
    let Some(sock) = sock else {
        return false;
    };
    let fd = sock.fd();
    // A session socket's connection is its fd's slot, which owns it.
    let Some(conn) = state
        .connection(c_long::from(fd), Lookup::ByFd)
        .filter(|conn| {
            conn.socket()
                .is_some_and(|own| own.as_ptr() == sock.as_ptr())
        })
    else {
        return false;
    };
    let port = serv.port_by_fd(fd).map(view::Port::config);
    if let Some(timer) = conn.timer() {
        del_id(timer);
        conn.set_timer(None);
    }
    let removed = sock.read(|s| s.removed);
    // A TLS close notify that cannot be sent now goes behind the queued output.
    if BUILT_PREQUEUE && owes_close_notify(conn, *sock) {
        if !conn.close_queued() {
            queue_close(conn, *sock);
        }
        let events = sock.read(|s| what_event_want(s, Events::WRITE));
        let op = if removed {
            swoole_rs_corebase::reactor::ChangeOp::Add
        } else {
            swoole_rs_corebase::reactor::ChangeOp::Set
        };
        if reactor.change(sock, op, events) {
            return true;
        }
    }
    if let Some(mut websocket) = conn.take_websocket_buffer() {
        websocket.dispose();
    }
    if !removed && !reactor.del(*sock) {
        return false;
    }
    count_close(state, port, reactor);
    let ssl = sock.read(|s| !s.ssl().is_null());
    if ssl {
        sock.edit(|s| s.ssl_quiet_shutdown = conn.peer_closed());
        tls::socket_ssl_close(*sock);
    }
    if let Some(mut recv_buffer) = sock.edit(|s| s.recv_buffer.take()) {
        recv_buffer.dispose();
    }
    if let Some(port) = port
        && port.open_http_protocol.get()
    {
        (crate::port_config::methods().destroy_http_request)(conn);
    }
    if let Some(session) = state.session(conn.session_id()) {
        session.set_fd(0);
    }
    lower_max_fd(state, fd);
    conn.active().store(false, Relaxed);
    conn.clear_object();
    // The slot's last write, after the socket leaves it and before close(fd): an
    // accept that gets the fd back acquires it before reusing the slot.
    conn.free_socket(|| conn.closed().store(1, Release));
    true
}

/// Whether a clean close of a ready TLS stream owes a close notify that its
/// shutdown could not send now.
fn owes_close_notify(conn: Connection<'_>, sock: Socket<'_>) -> bool {
    let (ready_stream, ssl_closed) = sock.read(|s| {
        let stream = matches!(s.socket_type, 1 | 3 | 5);
        (
            !s.ssl().is_null() && s.tls.state == SSL_READY && stream,
            s.ssl_closed,
        )
    });
    ready_stream
        && !(conn.peer_closed() || conn.close_force() || conn.close_reset() || ssl_closed)
        && !tls::socket_ssl_shutdown(sock)
        && sock.read(|s| s.tls.want_read || s.tls.want_write)
}

/// Queues a close behind the socket's output.
fn queue_close(conn: Connection<'_>, sock: Socket<'_>) {
    if sock.read(|s| s.output.out_buffer.is_none()) {
        let buffer = Some(new_buffer(SW_SEND_BUFFER_SIZE));
        sock.edit(|s| s.output.out_buffer = buffer);
    }
    sock.read(|s| out_buffer(s).map(swr_cpp_buffer_typed_close));
    conn.set_close_queued(true);
}

/// The close's counts: the server's and the port's closes and connections, which
/// base mode counts per reactor.
fn count_close(state: &crate::ServerState, port: Option<&PortConfig>, reactor: Reactor) {
    let gs = crate::shared::server_counters(state);
    let (Some(gs), Some(config)) = (gs, port) else {
        return;
    };
    let Some(port) = config.counters() else {
        return;
    };
    gs.close_count.fetch_add(1, SeqCst);
    port.close_count.fetch_add(1, SeqCst);
    if state.config_ref().mode_ == Mode::Base {
        let id = usize::from(reactor.handle().id());
        let server = state.tables_ref().connection_nums.records();
        for cells in [server, config.cells()] {
            if let Some(cell) = cells.get(id) {
                cell.fetch_sub(1, SeqCst);
            }
        }
    } else {
        gs.connection_num.fetch_sub(1, SeqCst);
        port.connection_num.fetch_sub(1, SeqCst);
    }
}

/// The connection list's max fd moves down past `fd`, if it was `fd`.
fn lower_max_fd(state: &crate::ServerState, fd: c_int) {
    let Some(gs) = crate::shared::server_counters(state) else {
        return;
    };
    state.locked.lock();
    if fd == gs.max_fd() {
        let mut find = fd - 1;
        while !valid(state.connection(c_long::from(find), Lookup::ByFd)) && find > gs.min_fd() {
            find -= 1;
        }
        gs.max_fd.store(find, Relaxed);
    }
    state.locked.unlock();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls};

    /// The live server with sessions 1 and 2 accepted, and their clients.
    pub(crate) fn accepted() -> (LiveServer, [std::net::TcpStream; 2]) {
        let live = LiveServer::new();
        let clients = [(); 2].map(|()| {
            let client = live.connect();
            let event = live.accept_event();
            crate::connection::accept(live.server(), live.reactor(), &event);
            client
        });
        drop(calls());
        (live, clients)
    }

    /// Session `id`, which its reactor thread reads.
    pub(crate) fn session(live: &LiveServer, id: c_long) -> Connection<'_> {
        let conn = live.state().connection(id, Lookup::Verified).unwrap();
        let sock = conn.socket().unwrap();
        if sock.read(|s| s.removed) {
            assert!(live.reactor().change(
                sock,
                swoole_rs_corebase::reactor::ChangeOp::Add,
                Events::READ
            ));
        }
        conn
    }

    pub(crate) fn closed(conn: Connection<'_>) -> bool {
        !conn.is_active() && conn.closed().load(Relaxed) == 1 && conn.socket().is_none()
    }

    #[test]
    fn close_releases_counts_and_moves_the_max_fd() {
        let (live, _clients) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let (mut one, mut two) = (session(&live, 1), session(&live, 2));
        if one.fd() > two.fd() {
            core::mem::swap(&mut one, &mut two);
        }
        let gs = live.counters();
        gs.max_fd.store(two.fd(), Relaxed);
        gs.min_fd.store(0, Relaxed);
        let (gs_before, port_before) = (
            gs.connection_num.load(SeqCst),
            live.port_counters().connection_num.load(SeqCst),
        );
        let session_id = two.session_id();
        assert!(close_connection(serv, reactor, two.socket()));
        assert!(closed(two));
        assert_eq!(gs.max_fd(), one.fd(), "down to the open session");
        let session = live.state().session(session_id).unwrap();
        assert_eq!(session.fd(), 0, "the session is clear");
        gs.max_fd.store(one.fd() + 1, Relaxed);
        assert!(close_connection(serv, reactor, one.socket()));
        assert!(closed(one));
        assert_eq!(gs.max_fd(), one.fd() + 1, "not the max fd");
        assert_eq!(gs.close_count.load(SeqCst), 2);
        assert_eq!(live.port_counters().close_count.load(SeqCst), 2);
        assert_eq!(gs.connection_num.load(SeqCst), gs_before - 2);
        assert_eq!(
            live.port_counters().connection_num.load(SeqCst),
            port_before - 2
        );
        assert!(!close_connection(serv, reactor, None));
        assert_eq!(calls(), [], "nobody is told");
    }

    #[test]
    fn peer_close_notifies_once() {
        let (live, _clients) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        let fd = conn.fd();
        let event = conn.event(0, 0);
        assert!(peer_close(serv, reactor, &event));
        let close = Call::Notify(c_long::from(fd), 0, ServerEvent::Close.code(), 0);
        assert_eq!(calls(), [close]);
        assert!(conn.peer_closed(), "upstream: the connection is gone");
        assert!(conn.socket().unwrap().read(|s| s.close_notified));
        assert!(conn.is_active(), "the worker closes it");

        let conn = session(&live, 2);
        conn.set_close_queued(true);
        assert!(peer_close(serv, reactor, &conn.event(0, 0)));
        assert_eq!(calls(), [], "a queued close closes now");
        assert!(closed(conn));
        assert!(!peer_close(serv, reactor, &conn.event(0, 0)), "gone");
    }

    #[test]
    fn peer_close_closes_when_nobody_is_told() {
        let (live, _clients) = accepted();
        live.server().config().disable_notify.set(true);
        let conn = session(&live, 1);
        let event = conn.event(0, 0);
        assert!(peer_close(live.server(), live.reactor(), &event));
        assert!(closed(conn));
        assert_eq!(calls(), []);
    }

    #[test]
    fn prequeue_keeps_a_synchronous_process_response() {
        let (mut live, _clients) = accepted();
        let (state, conn) = (live.state(), session(&live, 1));
        let config = state.config_ref();
        assert_eq!(
            config.dispatch_mode.get(),
            crate::DispatchMode::FD_MOD,
            "fixed by fd"
        );
        config.enable_coroutine.set(false);
        assert!(!fin_ends(true, state, conn), "owed its response");
        assert!(fin_ends(false, state, conn), "upstream");
        config.dispatch_mode.set(crate::DispatchMode::ROUND);
        assert!(fin_ends(true, state, conn), "round robin");
        config.dispatch_mode.set(crate::DispatchMode::FD_MOD);
        config.enable_coroutine.set(true);
        assert!(fin_ends(true, state, conn), "a coroutine worker");
        config.enable_coroutine.set(false);
        let terminal: [fn(Connection<'_>, bool); 4] = [
            |c, on| c.set_ssl(on),
            |c, on| c.set_close_errno(u16::from(on)),
            |c, on| c.set_close_force(on),
            |c, on| c.set_close_reset(on),
        ];
        for set in terminal {
            set(conn, true);
            assert!(fin_ends(true, state, conn));
            set(conn, false);
        }
        live.config_mut().mode_ = Mode::Base;
        let (state, conn) = (live.state(), session(&live, 1));
        assert!(fin_ends(true, state, conn), "base mode");
    }

    /// `bytes` queued on the connection's socket.
    pub(crate) fn queue(conn: Connection<'_>, bytes: &[u8]) {
        let buffer = swoole_rs_corebase::buffer::cpp::new_buffer(0);
        buffer.append(bytes);
        conn.socket()
            .unwrap()
            .edit(|s| s.output.out_buffer = Some(buffer));
    }

    #[test]
    fn write_flushes_the_output_then_reads_again() {
        use std::io::Read;
        let (live, [mut client, _]) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        queue(conn, b"queued");
        let sock = conn.socket().unwrap();
        reactor.change(
            sock,
            swoole_rs_corebase::reactor::ChangeOp::Set,
            Events::READ | Events::WRITE,
        );
        assert!(write(serv, reactor, &mut conn.event(0, 0)));
        let mut got = [0; 6];
        client.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"queued");
        assert_eq!(output(*sock), (0, true));
        assert_eq!(conn.send_queued_bytes(), 0);
        let events = sock.read(|s| s.events);
        assert!(!events.intersects(Events::WRITE), "reads only");
        assert_eq!(calls(), []);
        conn.active().store(false, Relaxed);
        assert!(!write(serv, reactor, &mut conn.event(0, 0)), "gone");
    }

    #[test]
    fn write_notifies_or_closes_a_pending_close() {
        let (live, _clients) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        conn.set_close_notify(true);
        queue(conn, b"kept");
        assert!(write(serv, reactor, &mut conn.event(0, 0)));
        let close = Call::Notify(
            c_long::from(conn.fd()),
            0,
            ServerEvent::Close.code(),
            conn.server_fd().c_cast(),
        );
        assert_eq!(calls(), [close]);
        assert!(!conn.close_notify());
        assert_eq!(output(*conn.socket().unwrap()).0, 4, "nothing flushed");

        live.server().config().disable_notify.set(true);
        let conn = session(&live, 2);
        conn.set_close_force(true);
        assert!(write(serv, reactor, &mut conn.event(0, 0)));
        assert!(closed(conn), "a forced close nobody is told of");
    }

    std::thread_local! {
        static READS: core::cell::RefCell<Vec<c_int>> = const { core::cell::RefCell::new(Vec::new()) };
    }

    /// The port's `onRead`: receives 5 bytes and fails, so its result is told apart.
    fn on_read(
        _: swoole_rs_corebase::reactor::Running<'_>,
        _: view::Port<'_>,
        read: ReadEvent<'_>,
    ) -> bool {
        let (_, event, _) = read.into_parts();
        READS.with_borrow_mut(|reads| reads.push(event.fd()));
        let sock = event.socket().unwrap();
        sock.edit(|s| s.total_recv_bytes += 5);
        false
    }

    /// The fds the port read since the last call.
    fn reads() -> Vec<c_int> {
        READS.with_borrow_mut(core::mem::take)
    }

    /// The live server with sessions 1 and 2 accepted, whose port reads with [`on_read`].
    fn reading() -> (LiveServer, [std::net::TcpStream; 2]) {
        let (live, clients) = accepted();
        live.port().on_read.set(Some(on_read));
        drop(reads());
        (live, clients)
    }

    #[test]
    fn read_lets_the_port_read() {
        let (live, _clients) = reading();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        assert!(
            !read(serv, reactor, &mut conn.event(0, 0)),
            "the port's result"
        );
        assert_eq!(reads(), [conn.fd()]);
        assert!(conn.last_recv_time() > 0.0, "stamped");
        assert_eq!(live.counters().total_recv_bytes.load(SeqCst), 5);
        assert_eq!(live.port_counters().total_recv_bytes.load(SeqCst), 5);
        assert_eq!(conn.waiting_time(), 0, "no queue limit");
        conn.set_server_fd(0);
        assert!(read(serv, reactor, &mut conn.event(0, 0)));
        assert_eq!(reads(), [], "a stale event");
    }

    #[test]
    fn read_fails_on_a_port_without_on_read() {
        let (live, _clients) = reading();
        live.port().on_read.set(None);
        let conn = session(&live, 1);
        assert!(!read(live.server(), live.reactor(), &mut conn.event(0, 0)));
        assert_eq!(reads(), [], "nothing read");
        assert_eq!(live.counters().total_recv_bytes.load(SeqCst), 0);
        assert_eq!(live.port_counters().total_recv_bytes.load(SeqCst), 0);
    }

    #[test]
    fn read_pauses_a_process_connection_over_its_queue() {
        let (mut live, _clients) = reading();
        let (serv, reactor) = (live.server(), live.reactor());
        live.state().config_ref().max_queued_bytes.set(8);
        let conn = session(&live, 1);
        conn.recv_queued_bytes().store(9, SeqCst);
        assert!(
            !read(serv, reactor, &mut conn.event(0, 0)),
            "the port's result"
        );
        assert_eq!(conn.waiting_time(), 1);
        assert!(conn.timer().is_some(), "resumes on a timer");
        let events = conn.socket().unwrap().read(|s| s.events);
        assert!(!events.intersects(Events::READ), "paused");

        let conn = session(&live, 2);
        conn.recv_queued_bytes().store(8, SeqCst);
        read(serv, reactor, &mut conn.event(0, 0));
        assert_eq!(conn.waiting_time(), 0, "at the limit");
        conn.recv_queued_bytes().store(9, SeqCst);
        live.config_mut().mode_ = Mode::Base;
        let (serv, conn) = (live.server(), session(&live, 2));
        read(serv, reactor, &mut conn.event(0, 0));
        assert_eq!(conn.waiting_time(), 0, "base mode");
        assert_eq!(reads().len(), 3);
    }

    #[test]
    fn read_aborts_a_failed_dtls_handshake() {
        let (live, _clients) = reading();
        let conn = session(&live, 1);
        let fd = conn.fd();
        conn.socket().unwrap().edit(|s| s.socket_type = 2);
        let event = &mut conn.event(0, 0);
        assert!(read(live.server(), live.reactor(), event));
        assert_eq!(calls(), [Call::DtlsRead(fd)]);
        assert_eq!(reads(), [], "not read");
        assert_eq!(live.counters().abort_count.load(SeqCst), 1);
        assert_eq!(live.port_counters().abort_count.load(SeqCst), 1);
    }

    // tests/modules/halfclose: a read event cached across a queued close.
    #[test]
    fn prequeue_reads_stop_at_a_queued_or_notified_close() {
        let (live, _clients) = reading();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        conn.set_close_queued(true);
        assert!(
            !read_as(false, serv, reactor, &mut conn.event(0, 0)),
            "the port's result"
        );
        assert_eq!(reads(), [conn.fd()], "upstream reads");
        assert!(read_as(true, serv, reactor, &mut conn.event(0, 0)));
        assert_eq!(reads(), [], "a queued plain close is not read");
        conn.set_close_queued(false);
        conn.socket().unwrap().edit(|s| s.close_notified = true);
        assert!(read_as(true, serv, reactor, &mut conn.event(0, 0)));
        assert_eq!(reads(), [], "notified is terminal");
        assert!(
            !read_as(false, serv, reactor, &mut conn.event(0, 0)),
            "the port's result"
        );
    }

    #[test]
    fn a_client_certificate_is_dispatched_or_rejected() {
        let fate = |wanted, verify_peer, peer, verified| {
            certificate(wanted, verify_peer, || peer, || verified)
        };
        assert_eq!(
            fate(false, true, false, false),
            Certificate::None,
            "none wanted"
        );
        assert_eq!(
            fate(true, true, false, false),
            Certificate::Rejected,
            "missing"
        );
        assert_eq!(
            fate(true, false, false, false),
            Certificate::None,
            "optional"
        );
        assert_eq!(
            fate(true, true, true, false),
            Certificate::Rejected,
            "rejected"
        );
        assert_eq!(
            fate(true, true, true, true),
            Certificate::Dispatched,
            "verified"
        );
        assert_eq!(
            fate(true, false, true, false),
            Certificate::Dispatched,
            "unverified"
        );
    }

    #[test]
    fn a_plain_connection_needs_no_handshake() {
        let (live, _clients) = accepted();
        let sock = session(&live, 1).socket().unwrap();
        let port = live.server().port_by_fd(sock.fd());
        let handshake = verify_ssl(live.server(), live.reactor(), port, *sock);
        assert_eq!(handshake, Handshake::Continue);
    }
}
