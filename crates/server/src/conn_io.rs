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
use swoole_rs_reactor::Events;

use swoole_rs_buffer::EntryKind;
use swoole_rs_buffer::cpp::{
    new_buffer, swr_cpp_buffer_empty, swr_cpp_buffer_length, swr_cpp_buffer_typed_close,
    swr_cpp_buffer_typed_front,
};
use swoole_rs_buffer::string::delete_string;
use swoole_rs_buffer::string::with_thread_buffer;
use swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
use swoole_rs_corebase::{CCast, DataHead, microtime};
use swoole_rs_reactor::{Event, Socket, what_event_want};
use swoole_rs_timer::{del_id, delay_held};
use swoole_rs_tls::Accept;
use swoole_rs_tls::swr_v1_tls_owner_view;
use swoole_rs_tls::upstream as tls;

use crate::base_factory::SendData;
use crate::callbacks::CallbackKind;
use crate::hybrid::hybrid;
use crate::port_config::{PortConfig, ReadEvent};
use crate::reactor_handle::Reactor;
use crate::reactor_ops::{is_dgram, out_buffer, valid};
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
/// port read, and pause reading while the workers hold too many of its bytes.
pub(crate) fn read(serv: view::Server<'_>, reactor: Reactor, event: &mut Event) -> c_int {
    read_as(BUILT_PREQUEUE, serv, reactor, event)
}

/// [`read`], as a build with `prequeue` (the `halfclose` feature) or without.
fn read_as(prequeue: bool, serv: view::Server<'_>, reactor: Reactor, event: &mut Event) -> c_int {
    let state = serv.state();
    let fd = event.fd();
    let conn = state.connection(c_long::from(fd), Lookup::ByFd);
    // The server closed the connection (and the fd may be reused) before the event.
    let Some(conn) = conn.filter(|conn| conn.server_fd() != 0) else {
        return OK;
    };
    if prequeue {
        let Some((notified, want_read)) = event
            .socket()
            .map(|s| s.read(|s| (s.close_notified, !s.ssl().is_null() && s.tls.want_read)))
        else {
            return OK;
        };
        if notified {
            return OK;
        }
        // Once a close is queued, a read only drives a TLS shutdown that wants one.
        if conn.close_queued() {
            return if want_read {
                status(write(serv, reactor, event))
            } else {
                OK
            };
        }
    }
    // The port reads the event through its pointer while the socket is borrowed.
    let raw = core::ptr::from_mut(event);
    let Some(sock) = event.socket() else {
        return OK;
    };
    let port = serv.port_by_fd(fd);
    if let Some(config) = port.map(view::Port::config)
        && sock.read(|s| is_dgram(s.socket_type))
        && hybrid().dtls_read(config, sock, fd)
    {
        crate::connection::abort(serv, reactor, port, sock);
        return OK;
    }
    match verify_ssl(serv, reactor, port, sock) {
        Handshake::Error => {
            crate::connection::abort(serv, reactor, port, sock);
            return OK;
        }
        Handshake::Ready if sock.read(|s| s.dtls) => return OK,
        Handshake::Wait => return OK,
        Handshake::Ready | Handshake::Continue => {}
    }
    let Some(port) = port else {
        return ERR;
    };
    conn.set_last_recv_time(microtime());
    let last_recv_bytes = sock.read(|s| s.total_recv_bytes);
    let Some(on_read) = port.config().on_read.get() else {
        return ERR;
    };
    // SAFETY: the reactor's event, which no reference is used through during the read.
    let event = unsafe { &mut *raw };
    // A readable event on the live connection of `fd`, of this server's port `port`.
    let retval = on_read(reactor.handle(), port, ReadEvent::new(serv, event));
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
        // The active connection's socket: the event's.
        if let Some(own) = conn.socket()
            && resume_timer(serv, conn, own)
        {
            reactor.interest(sock, swoole_rs_reactor::Interest::RemoveRead);
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
        .and_then(|config| config.with_ssl_fields(|_, f| (f.verify_peer, f.allow_self_signed)))
        .unwrap_or_default();
    let peer =
        || with_thread_buffer(|buffer| tls::socket_ssl_peer_certificate_string(sock, buffer));
    let verified = || tls::socket_ssl_verify(sock, allow_self_signed);
    let wanted = config.is_some_and(client_cert_file);
    match certificate(wanted, verify_peer, peer, verified) {
        Certificate::Rejected => return Handshake::Error,
        Certificate::Dispatched => {
            // The peer's certificate filled this thread's buffer.
            let (len, data) = with_thread_buffer(|buffer| (buffer.len(), buffer.as_ptr()));
            let info = DataHead {
                fd: sock.fd().into(),
                kind: ServerEvent::Connect.code(),
                reactor_id: reactor.handle().id().cast_signed(),
                len: u32::try_from(len).unwrap_or(u32::MAX),
                ..DataHead::default()
            };
            // SAFETY: the thread's buffer, which lives as long as the thread and holds
            // the certificate's `len` bytes; the dispatch reads them before anything on
            // this thread fills the buffer again.
            let mut task = unsafe { SendData::from_raw(info, data.cast()) };
            hybrid().factory_dispatch(serv, &mut task);
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
    let Some(context) = port.ssl_context.get() else {
        return false;
    };
    // `client_cert_file`'s slot.
    let slot = swr_v1_tls_owner_view(&context).strings[7];
    // SAFETY: a stable slot of the port's live owner.
    !unsafe { &*slot }.bytes().is_empty()
}

/// Arms `conn`'s resume timer for its paused `socket`, after its waiting time.
///
/// The connection's own socket, which deletes this timer before it goes.
fn resume_timer(serv: view::Server<'_>, conn: Connection<'_>, socket: Socket<'static>) -> bool {
    let msec = c_long::from(conn.waiting_time());
    let raw = serv.raw();
    let timer = swoole_rs_timer::add_fn(msec, false, move || {
        // SAFETY: the server whose reactor armed the timer, which outlives the
        // timers of its sockets.
        let serv = unsafe { view::Server::from_raw(raw) };
        resume_data_receiving(serv, socket);
    });
    conn.set_timer(timer);
    timer.is_some()
}

/// `ReactorThread_resume_data_receiving`, the timer of a paused session socket: back
/// off (doubling up to 1024ms) while the workers still hold too many bytes, then
/// read again.
fn resume_data_receiving(serv: view::Server<'_>, socket: Socket<'static>) {
    let state = serv.state();
    let Some(conn) = state.connection(c_long::from(socket.fd()), Lookup::ByFd) else {
        return;
    };
    if conn.recv_queued_bytes().load(SeqCst) > state.config_ref().max_queued_bytes.get() {
        let waiting = conn.waiting_time();
        if waiting != MAX_WAITING_TIME {
            conn.set_waiting_time(waiting.saturating_mul(2));
        }
        if resume_timer(serv, conn, socket) {
            return;
        }
    }
    // The firing timer's reactor: this thread's.
    Reactor::thread().interest(socket, swoole_rs_reactor::Interest::AddRead);
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
use swoole_rs_tls::SslState::SW_SSL_STATE_READY as SSL_READY;

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
        if handshaking(sock) {
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
            EntryKind::Close => return reactor.close(sock) >= 0,
            EntryKind::Sendfile => tls::socket_handle_sendfile(sock),
            EntryKind::Data => {
                let sent = tls::socket_handle_send(sock);
                if sent {
                    conn.set_send_queued_bytes(output(sock).0);
                }
                sent
            }
        };
        if !sent {
            let (close_wait, send_wait) = sock.read(|s| (s.output.close_wait, s.output.send_wait));
            if close_wait {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                conn.set_close_errno(errno.c_cast::<u16>());
                return reactor.trigger_close(event) >= 0;
            } else if send_wait {
                break;
            }
        }
    }
    if conn.overflow() && output(sock).0 < sock.read(|s| s.output.buffer_size) {
        conn.set_overflow(false);
    }
    let buffer_empty = state.callbacks.slot(CallbackKind::BufferEmpty).is_set();
    let low_watermark = port.map_or(0, |port| port.buffer_low_watermark.get());
    if buffer_empty && conn.high_watermark() && output(sock).0 <= low_watermark {
        conn.set_high_watermark(false);
        crate::server_methods::notify(serv, conn, ServerEvent::BufferEmpty);
    }
    if let Some(timer) = sock.read(|s| s.output.send_timer) {
        if output(sock).1 {
            del_id(timer);
            sock.edit(|s| s.output.send_timer = None);
        } else if let Some(port) = port {
            delay_held(timer, port.max_idle_time.get().c_cast::<c_long>());
        }
    }
    if !conn.peer_closed() && !sock.read(|s| s.removed) && output(sock).1 {
        // A notified close no longer reads: it only drains.
        if BUILT_PREQUEUE {
            reactor.interest(sock, swoole_rs_reactor::Interest::RemoveWrite);
        } else {
            reactor.change(sock, swoole_rs_reactor::ChangeOp::Set, Events::READ);
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
    if !reactor.del(sock) {
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
        || !config.hash_dispatch()
        || dispatch_func
        || config.enable_coroutine.get()
        || conn.ssl()
        || conn.close_errno() != 0
        || conn.close_force()
        || conn.close_reset()
}

/// `Server::close_connection`: release what the connection holds, count the close,
/// clear its session and move the max fd down past it, then close the socket.
pub(crate) fn close_connection(
    serv: view::Server<'_>,
    reactor: Reactor,
    sock: Option<Socket<'_>>,
) -> bool {
    let state = serv.state();
    let Some(sock) = sock else {
        return false;
    };
    let fd = sock.fd();
    // A session socket's connection is its fd's slot.
    let Some(conn) = state.connection(c_long::from(fd), Lookup::ByFd) else {
        return false;
    };
    let port = serv.port_by_fd(fd).map(view::Port::config);
    if let Some(timer) = conn.timer() {
        del_id(timer);
        conn.set_timer(None);
    }
    let removed = sock.read(|s| s.removed);
    // A TLS close notify that cannot be sent now goes behind the queued output.
    if BUILT_PREQUEUE && owes_close_notify(conn, sock) {
        if !conn.close_queued() {
            queue_close(conn, sock);
        }
        let events = sock.read(|s| what_event_want(s, Events::WRITE));
        let op = if removed {
            swoole_rs_reactor::ChangeOp::Add
        } else {
            swoole_rs_reactor::ChangeOp::Set
        };
        if reactor.change(sock, op, events) {
            return true;
        }
    }
    let websocket = conn.websocket_buffer();
    if !websocket.is_null() {
        // SAFETY: the connection's `new String`, not used again.
        unsafe { delete_string(websocket.cast()) };
        conn.set_websocket_buffer(core::ptr::null_mut());
    }
    if !removed && !reactor.del(sock) {
        return false;
    }
    count_close(state, port, reactor);
    let (ssl, dtls, recv_buffer) = sock.read(|s| (!s.ssl().is_null(), s.dtls, s.recv_buffer));
    if ssl {
        sock.edit(|s| s.ssl_quiet_shutdown = conn.peer_closed());
        // SAFETY: handles to a session live only for the operation that took them,
        // and native code closes it between operations.
        unsafe { tls::socket_ssl_close(sock) };
    }
    if let Some(port) = port.filter(|_| dtls) {
        (crate::port_config::methods().dtls_free)(port, fd);
    }
    if !recv_buffer.is_null() {
        // SAFETY: the socket's `new String`, not used again.
        unsafe { delete_string(recv_buffer.cast()) };
        sock.edit(|s| s.recv_buffer = core::ptr::null_mut());
    }
    if let Some(port) = port {
        if port.open_http_protocol.get() && !conn.object().is_null() {
            (crate::port_config::methods().destroy_http_request)(conn);
        }
        if port.open_redis_protocol.get() && !conn.object().is_null() {
            // SAFETY: the redis request the connection owns, not used again.
            unsafe { swoole_rs_corebase::sw_free(conn.object()) };
        }
    }
    if let Some(session) = state.session(conn.session_id()) {
        session.set_fd(0);
    }
    lower_max_fd(state, fd);
    conn.active().store(0, Relaxed);
    conn.set_socket(None);
    conn.set_object(core::ptr::null_mut());
    // The slot's last write before close(fd): an accept that gets the fd back
    // acquires it (OP_CONN_RESET) before reusing the slot.
    conn.closed().store(1, Release);
    // SAFETY: the closing socket, which nothing uses afterwards.
    unsafe { swoole_rs_tls::reactor_write::reactor_close(reactor.ptr(), sock.as_ptr()) };
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
    sock.read(|s| out_buffer(s).map(|b| swr_cpp_buffer_typed_close(b)));
    conn.set_close_queued(true);
}

/// The close's counts: the server's and the port's closes and connections, which
/// base mode counts per reactor.
fn count_close(state: &crate::ServerState, port: Option<&PortConfig>, reactor: Reactor) {
    let gs = crate::shared::server_counters(state);
    let (Some(gs), Some(port)) = (gs, port.and_then(PortConfig::counters)) else {
        return;
    };
    gs.close_count.fetch_add(1, SeqCst);
    port.close_count.fetch_add(1, SeqCst);
    if state.config_ref().mode_ == Mode::Base {
        let id = usize::from(reactor.handle().id());
        for cells in [gs.cells(), port.cells()] {
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
    state.lock();
    if fd == gs.max_fd() {
        let mut find = fd - 1;
        while !valid(state.connection(c_long::from(find), Lookup::ByFd)) && find > gs.min_fd() {
            find -= 1;
        }
        gs.max_fd.store(find, Relaxed);
    }
    state.unlock();
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
            // SAFETY: the fixture's listener, live for the event.
            let event = unsafe { Event::new(live.listener_fd, 0, 1, live.listener().cast()) };
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
            assert!(
                live.reactor()
                    .change(sock, swoole_rs_reactor::ChangeOp::Add, Events::READ)
            );
        }
        conn
    }

    /// Edits the state's configuration, which the test alone touches.
    fn configure(live: &LiveServer, edit: impl FnOnce(&mut crate::ServerConfig)) {
        // SAFETY: the fixture's state, on this thread only.
        edit(unsafe { &mut *live.state().config() });
    }

    pub(crate) fn closed(conn: Connection<'_>) -> bool {
        !conn.is_active() && conn.closed().load(Relaxed) == 1 && conn.socket().is_none()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
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
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
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
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn peer_close_closes_when_nobody_is_told() {
        let (live, _clients) = accepted();
        configure(&live, |c| c.disable_notify.set(true));
        let conn = session(&live, 1);
        let event = conn.event(0, 0);
        assert!(peer_close(live.server(), live.reactor(), &event));
        assert!(closed(conn));
        assert_eq!(calls(), []);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn prequeue_keeps_a_synchronous_process_response() {
        let (live, _clients) = accepted();
        let (state, conn) = (live.state(), session(&live, 1));
        assert_eq!(state.config_ref().dispatch_mode.get(), 2, "fixed by fd");
        state.config_ref().enable_coroutine.set(false);
        assert!(!fin_ends(true, state, conn), "owed its response");
        assert!(fin_ends(false, state, conn), "upstream");
        state.config_ref().dispatch_mode.set(1);
        assert!(fin_ends(true, state, conn), "round robin");
        state.config_ref().dispatch_mode.set(2);
        state.config_ref().enable_coroutine.set(true);
        assert!(fin_ends(true, state, conn), "a coroutine worker");
        state.config_ref().enable_coroutine.set(false);
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
        configure(&live, |c| c.mode_ = Mode::Base);
        assert!(fin_ends(true, state, conn), "base mode");
    }

    /// `bytes` queued on the connection's socket.
    pub(crate) fn queue(conn: Connection<'_>, bytes: &[u8]) {
        let buffer = swoole_rs_buffer::cpp::new_buffer(0);
        buffer.append(bytes);
        conn.socket()
            .unwrap()
            .edit(|s| s.output.out_buffer = Some(buffer));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn write_flushes_the_output_then_reads_again() {
        use std::io::Read;
        let (live, [mut client, _]) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        queue(conn, b"queued");
        let sock = conn.socket().unwrap();
        reactor.change(
            sock,
            swoole_rs_reactor::ChangeOp::Set,
            Events::READ | Events::WRITE,
        );
        assert!(write(serv, reactor, &mut conn.event(0, 0)));
        let mut got = [0; 6];
        client.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"queued");
        assert_eq!(output(sock), (0, true));
        assert_eq!(conn.send_queued_bytes(), 0);
        let events = sock.read(|s| s.events);
        assert!(!events.intersects(Events::WRITE), "reads only");
        assert_eq!(calls(), []);
        conn.active().store(0, Relaxed);
        assert!(!write(serv, reactor, &mut conn.event(0, 0)), "gone");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
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
        assert_eq!(output(conn.socket().unwrap()).0, 4, "nothing flushed");

        configure(&live, |c| c.disable_notify.set(true));
        let conn = session(&live, 2);
        conn.set_close_force(true);
        assert!(write(serv, reactor, &mut conn.event(0, 0)));
        assert!(closed(conn), "a forced close nobody is told of");
    }

    std::thread_local! {
        static READS: core::cell::RefCell<Vec<c_int>> = const { core::cell::RefCell::new(Vec::new()) };
    }

    /// The port's `onRead`: receives 5 bytes and returns 42.
    fn on_read(_: swoole_rs_reactor::Running<'_>, _: view::Port<'_>, read: ReadEvent<'_>) -> c_int {
        let (_, event) = read.into_parts();
        READS.with_borrow_mut(|reads| reads.push(event.fd()));
        let sock = event.socket().unwrap();
        sock.edit(|s| s.total_recv_bytes += 5);
        42
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
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn read_lets_the_port_read() {
        let (live, _clients) = reading();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        assert_eq!(read(serv, reactor, &mut conn.event(0, 0)), 42);
        assert_eq!(reads(), [conn.fd()]);
        assert!(conn.last_recv_time() > 0.0, "stamped");
        assert_eq!(live.counters().total_recv_bytes.load(SeqCst), 5);
        assert_eq!(live.port_counters().total_recv_bytes.load(SeqCst), 5);
        assert_eq!(conn.waiting_time(), 0, "no queue limit");
        conn.set_server_fd(0);
        assert_eq!(read(serv, reactor, &mut conn.event(0, 0)), OK);
        assert_eq!(reads(), [], "a stale event");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn read_fails_on_a_port_without_on_read() {
        let (live, _clients) = reading();
        live.port().on_read.set(None);
        let conn = session(&live, 1);
        assert_eq!(
            read(live.server(), live.reactor(), &mut conn.event(0, 0)),
            ERR
        );
        assert_eq!(reads(), [], "nothing read");
        assert_eq!(live.counters().total_recv_bytes.load(SeqCst), 0);
        assert_eq!(live.port_counters().total_recv_bytes.load(SeqCst), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn read_pauses_a_process_connection_over_its_queue() {
        let (live, _clients) = reading();
        let (serv, reactor) = (live.server(), live.reactor());
        live.state().config_ref().max_queued_bytes.set(8);
        let conn = session(&live, 1);
        conn.recv_queued_bytes().store(9, SeqCst);
        assert_eq!(read(serv, reactor, &mut conn.event(0, 0)), 42);
        assert_eq!(conn.waiting_time(), 1);
        assert!(conn.timer().is_some(), "resumes on a timer");
        let events = conn.socket().unwrap().read(|s| s.events);
        assert!(!events.intersects(Events::READ), "paused");

        let conn = session(&live, 2);
        conn.recv_queued_bytes().store(8, SeqCst);
        read(serv, reactor, &mut conn.event(0, 0));
        assert_eq!(conn.waiting_time(), 0, "at the limit");
        conn.recv_queued_bytes().store(9, SeqCst);
        configure(&live, |c| c.mode_ = Mode::Base);
        read(serv, reactor, &mut conn.event(0, 0));
        assert_eq!(conn.waiting_time(), 0, "base mode");
        assert_eq!(reads().len(), 3);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn read_aborts_a_failed_dtls_handshake() {
        let (live, _clients) = reading();
        let conn = session(&live, 1);
        let fd = conn.fd();
        conn.socket().unwrap().edit(|s| s.socket_type = 2);
        let event = &mut conn.event(0, 0);
        assert_eq!(read(live.server(), live.reactor(), event), OK);
        assert_eq!(calls(), [Call::DtlsRead(fd)]);
        assert_eq!(reads(), [], "not read");
        assert_eq!(live.counters().abort_count.load(SeqCst), 1);
        assert_eq!(live.port_counters().abort_count.load(SeqCst), 1);
    }

    // tests/modules/halfclose: a read event cached across a queued close.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn prequeue_reads_stop_at_a_queued_or_notified_close() {
        let (live, _clients) = reading();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        conn.set_close_queued(true);
        assert_eq!(read_as(false, serv, reactor, &mut conn.event(0, 0)), 42);
        assert_eq!(reads(), [conn.fd()], "upstream reads");
        assert_eq!(read_as(true, serv, reactor, &mut conn.event(0, 0)), OK);
        assert_eq!(reads(), [], "a queued plain close is not read");
        conn.set_close_queued(false);
        conn.socket().unwrap().edit(|s| s.close_notified = true);
        assert_eq!(read_as(true, serv, reactor, &mut conn.event(0, 0)), OK);
        assert_eq!(reads(), [], "notified is terminal");
        assert_eq!(read_as(false, serv, reactor, &mut conn.event(0, 0)), 42);
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
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_plain_connection_needs_no_handshake() {
        let (live, _clients) = accepted();
        let sock = session(&live, 1).socket().unwrap();
        let port = live.server().port_by_fd(sock.fd());
        let handshake = verify_ssl(live.server(), live.reactor(), port, sock);
        assert_eq!(handshake, Handshake::Continue);
    }
}
