//! Reactor threads: the datagram path and dispatch to workers, the worker pipe
//! protocol, heartbeats and the threads' lifecycle.
use crate::ops::{
    LOG_RECVFROM, OP_DTLS_ACCEPT, OP_ERRNO, OP_FACTORY_DISPATCH, OP_MODE, OP_PACKET_INIT,
    OP_PACKET_TASK, OP_PORT_DTLS, OP_RECVFROM,
};
use core::ffi::{c_int, c_long};

use swoole_rs_corebase::{DataHead, log, microtime};
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::Socket;

use crate::reactor_handle::Reactor;
use crate::reactor_ops::ThreadRecord;
use crate::records::{Connection, Lookup};
use crate::{Host, MODE_BASE, SW_ERR, SW_OK, ServerState, view};

/// `SW_LOG_TRACE`, `SW_FD_SESSION`, `SW_FD_DGRAM_SERVER`, `SW_EVENT_READ` and
/// `SW_EVENT_WRITE`.
const SW_LOG_TRACE: c_int = 1;
const FD_SESSION: c_int = 0;
const FD_DGRAM_SERVER: c_int = 2;
const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;

/// The heartbeat thread's `ServerState::start_thread` key; reactor threads use their ids.
pub(crate) const HEARTBEAT: u64 = u64::MAX;

/// `ReactorThread_onPacketReceived`: dispatch datagrams until the socket would block.
pub(crate) fn packet_received<H: Host>(h: &mut H) -> crate::Effect<i64> {
    h.get(OP_PACKET_INIT)?;
    loop {
        let n = h.signed(OP_RECVFROM)?;
        if n <= 0 {
            if h.get(OP_ERRNO)? == u64::from(libc::EAGAIN.cast_unsigned()) {
                return Ok(SW_OK);
            }
            h.get(LOG_RECVFROM)?;
            return Ok(SW_ERR);
        }
        let length = n.cast_unsigned();
        if h.is(OP_PORT_DTLS)? {
            return Ok(h.op(OP_DTLS_ACCEPT, length)?.cast_signed());
        }
        let task = h.op(OP_PACKET_TASK, length)?;
        // Base mode hands it to this process's worker as it does a connection's data.
        let dispatched = if h.get(OP_MODE)? == MODE_BASE {
            let task = core::ptr::with_exposed_provenance_mut(usize::try_from(task).unwrap_or(0));
            crate::base_factory::dispatch(h, task, true)?
        } else {
            h.get(OP_FACTORY_DISPATCH)? != 0
        };
        if !dispatched {
            return Ok(SW_ERR);
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
    if serv.state().config_ref().disable_notify || conn.close_force() || handshaking {
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
    if serv.state().config_ref().have_dgram_sock {
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
        let thread = crate::reactor_entry::thread(state, usize::from(id)).cast::<ThreadRecord>();
        if !thread.is_null() {
            // SAFETY: one of the server's live threads; only its field is read, as the
            // thread itself may hold its record meanwhile.
            let notify = unsafe { (*thread).notify_pipe.get() };
            // SAFETY: null or the thread's notify pipe, which it keeps until the server
            // joins it, after this notice.
            let pipe = unsafe { Socket::from_ptr(notify) };
            send_event(pipe, 0, crate::ops::EVENT_SHUTDOWN);
        }
        joining.join().unwrap_or_default();
    }
}

/// `socket->send_sync(&head, sizeof(head))` of a bare event.
pub(crate) fn send_event(sock: Option<Socket<'_>>, fd: c_long, kind: u32) {
    let head = DataHead {
        fd,
        kind: u8::try_from(kind).unwrap_or(u8::MAX),
        ..DataHead::default()
    };
    let data = (&raw const head).cast();
    if let Some(sock) = sock {
        // SAFETY: a live pipe socket, and the head's bytes.
        let _ = unsafe {
            swoole_rs_tls::upstream::socket_send_sync(sock, data, size_of::<DataHead>(), 0)
        };
    }
}

/// The reactor-thread operations of `crate::run`.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<i64> {
    Ok(match operation {
        crate::ops::RUN_PACKET_RECEIVED => packet_received(h)?,
        _ => crate::base_factory::run(h, operation, arg)?.unwrap_or(crate::SW_ERR),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    #[test]
    fn datagrams_until_eagain() {
        let mut s = Script::with(&[
            (OP_ERRNO, libc::EAGAIN.cast_unsigned().into()),
            (OP_FACTORY_DISPATCH, 1),
        ])
        .queue(OP_RECVFROM, &[10, 20, u64::MAX]);
        assert_eq!(packet_received(&mut s).unwrap(), SW_OK, "drained");
        assert_eq!(s.args(OP_PACKET_TASK), vec![10, 20], "each datagram");
        assert_eq!(s.called(OP_FACTORY_DISPATCH), 2, "through the factory");

        let mut s = Script::with(&[(OP_ERRNO, 5)]).queue(OP_RECVFROM, &[u64::MAX]);
        assert_eq!(packet_received(&mut s).unwrap(), SW_ERR, "recvfrom failed");
        assert_eq!(s.called(LOG_RECVFROM), 1, "warned");

        let mut s = Script::with(&[(OP_FACTORY_DISPATCH, 0), (OP_RECVFROM, 3)]);
        assert_eq!(packet_received(&mut s).unwrap(), SW_ERR, "dispatch failed");

        // Base mode dispatches the task itself; with no worker here, through the bus.
        let mut task = crate::base_factory::SendData {
            info: swoole_rs_corebase::DataHead {
                kind: 1, // SW_SERVER_EVENT_RECV_DGRAM
                ..swoole_rs_corebase::DataHead::default()
            },
            data: core::ptr::null(),
        };
        let address = (&raw mut task).expose_provenance() as u64;
        let mut s = Script::with(&[
            (OP_ERRNO, libc::EAGAIN.cast_unsigned().into()),
            (OP_MODE, MODE_BASE),
            (OP_PACKET_TASK, address),
        ])
        .queue(OP_RECVFROM, &[5, u64::MAX]);
        assert_eq!(packet_received(&mut s).unwrap(), SW_OK);
        assert_eq!(s.called(OP_FACTORY_DISPATCH), 0, "not the factory");
        assert_eq!(s.called(crate::ops::OP_BASE_PASS), 1, "the worker's bus");
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
        live.reactor()
            .change(conn.socket().unwrap(), 0, SW_EVENT_READ);
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

    unsafe extern "C" fn joined(_: *mut core::ffi::c_void, arg: u64) {
        JOINED.lock().unwrap().push(arg);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn join_waits_for_each_reactor_thread_and_the_heartbeat() {
        let live = crate::fixture::LiveServer::new();
        let (serv, state) = (live.server(), live.state());
        let config = state.config_ref();
        let entry = (joined as *const ()).expose_provenance() as u64;
        let raw = serv.raw().expose_provenance() as u64;
        for id in 0..u64::from(config.reactor_num.get()) {
            state.start_thread(entry, raw, id);
        }
        state.start_thread(entry, raw, HEARTBEAT);
        config.heartbeat_check_interval.set(1);
        join_reactor_threads(serv);
        let mut ran = JOINED.lock().unwrap().clone();
        ran.sort_unstable();
        assert_eq!(ran, vec![0, HEARTBEAT], "each thread ran and was joined");
        assert!(state.take_thread(0).is_none() && state.take_thread(HEARTBEAT).is_none());
    }
}
