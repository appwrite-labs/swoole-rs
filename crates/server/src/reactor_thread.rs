//! Reactor threads: the receive path and dispatch to workers, the worker pipe
//! protocol, TLS readiness, heartbeats and the threads' lifecycle.
use crate::ops::{
    FIELD_HEARTBEAT_INTERVAL, LOG_RECVFROM, OP_CERT_TASK, OP_CLIENT_CERT, OP_DELAY_RECEIVE,
    OP_DGRAM_BUFFER, OP_DTLS_ACCEPT, OP_ERRNO, OP_EVENT_FREE, OP_EVENT_INIT, OP_EVENT_WAIT,
    OP_FACTORY_DISPATCH, OP_FIELD, OP_HAS_ON_CONNECT, OP_HAVE_DGRAM, OP_HEARTBEAT_ENTRY,
    OP_HEARTBEAT_TIMER, OP_MASTER_REACTOR_ID, OP_MODE, OP_NOTIFY_CONNECT, OP_PACKET_INIT,
    OP_PACKET_TASK, OP_PEER_CERT, OP_PORT_COUNT, OP_PORT_DGRAM, OP_PORT_DTLS, OP_PORT_LISTEN,
    OP_PORT_READ, OP_REACTOR_BARRIER, OP_REACTOR_DEL, OP_REACTOR_NUM, OP_REACTOR_THREAD_ENTRY,
    OP_REACTOR_THREAD_TYPE, OP_RECVFROM, OP_SERVER, OP_SESSION_HANDLERS, OP_SINGLE_THREAD,
    OP_SSL_ACCEPT, OP_SSL_READY, OP_SSL_VERIFY, OP_START_MASTER_THREAD, OP_STORE_LISTEN_SOCKET,
    OP_THREAD_CLEAN, OP_THREAD_ENTER, OP_THREAD_INIT, OP_THREAD_NOTIFY, OP_THREAD0_INIT,
    OP_VERIFY_PEER, OP_WORKER_START, OP_WORKER_STOP,
};
use core::ffi::{c_int, c_long};

use swoole_rs_corebase::{log, microtime};
use swoole_rs_error::ErrorCode;

use crate::reactor_handle::Reactor;
use crate::records::{Connection, Lookup};
use crate::{Host, MODE_BASE, MODE_PROCESS, MODE_THREAD, SW_ERR, SW_OK, ServerState, view};

/// `SW_LOG_TRACE`.
const SW_LOG_TRACE: c_int = 1;

/// `swReturnCode` values.
pub(crate) const SW_CONTINUE: i64 = 1;
pub(crate) const SW_WAIT: i64 = 2;
pub(crate) const SW_ERROR: i64 = 4;
pub(crate) const SW_READY: i64 = 5;
/// The heartbeat thread's `ServerState::start_thread` key; reactor threads use their ids.
const HEARTBEAT: u64 = u64::MAX;

/// `ReactorThread_verify_ssl_state` once the socket's TLS handshake is pending:
/// finish it, then announce the connection (with the client certificate when the
/// port asks for one).
pub(crate) fn verify_ssl<H: Host>(h: &mut H) -> crate::Effect<i64> {
    let code = h.signed(OP_SSL_ACCEPT)?;
    if code != SW_READY {
        return Ok(code);
    }
    h.get(OP_SSL_READY)?;
    let mut announced = false;
    if h.is(OP_CLIENT_CERT)? {
        if h.is(OP_PEER_CERT)? {
            if h.is(OP_VERIFY_PEER)? && !h.is(OP_SSL_VERIFY)? {
                return Ok(SW_ERROR);
            }
            h.get(OP_CERT_TASK)?;
            h.get(OP_FACTORY_DISPATCH)?;
            announced = true;
        } else if h.is(OP_VERIFY_PEER)? {
            return Ok(SW_ERROR);
        }
    }
    if !announced && h.is(OP_HAS_ON_CONNECT)? {
        h.get(OP_NOTIFY_CONNECT)?;
    }
    if h.is(OP_DELAY_RECEIVE)? && h.signed(OP_REACTOR_DEL)? < 0 {
        return Ok(SW_ERROR);
    }
    Ok(SW_READY)
}

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
        return crate::reactor_entry::close_socket(serv, reactor, conn.socket());
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
            crate::reactor_entry::close_socket(serv, reactor, conn.socket());
        }
    });
}

/// `Server::init_reactor`.
pub(crate) fn init_reactor<H: Host>(h: &mut H) -> crate::Effect<()> {
    // 64K datagrams need the larger thread buffer.
    if h.is(OP_HAVE_DGRAM)? {
        h.get(OP_DGRAM_BUFFER)?;
    }
    h.get(OP_SESSION_HANDLERS)?;
    Ok(())
}

/// `Server::start_reactor_threads`: listen, start the reactor threads (or set up the
/// single one) and the heartbeat, then run the master thread.
pub(crate) fn start_reactor_threads<H: Host>(h: &mut H) -> crate::Effect<i64> {
    if h.signed(OP_EVENT_INIT)? < 0 {
        return Ok(SW_ERR);
    }
    for port in 0..h.get(OP_PORT_COUNT)? {
        if h.op(OP_PORT_DGRAM, port)? != 0 {
            continue;
        }
        if h.op(OP_PORT_LISTEN, port)?.cast_signed() < 0 {
            h.get(OP_EVENT_FREE)?;
            return Ok(SW_ERR);
        }
        h.op(OP_PORT_READ, port)?;
    }
    h.get(OP_STORE_LISTEN_SOCKET)?;
    let single = h.is(OP_SINGLE_THREAD)?;
    if single {
        h.get(OP_THREAD0_INIT)?;
    } else {
        h.get(OP_MASTER_REACTOR_ID)?;
        let (entry, serv) = (h.get(OP_REACTOR_THREAD_ENTRY)?, h.get(OP_SERVER)?);
        for thread in 0..h.get(OP_REACTOR_NUM)? {
            h.state()?.start_thread(entry, serv, thread);
        }
    }
    if h.op(OP_FIELD, u64::from(FIELD_HEARTBEAT_INTERVAL))? >= 1 {
        if single {
            h.get(OP_HEARTBEAT_TIMER)?;
        } else {
            let (entry, serv) = (h.get(OP_HEARTBEAT_ENTRY)?, h.get(OP_SERVER)?);
            h.state()?.start_thread(entry, serv, HEARTBEAT);
        }
    }
    h.signed(OP_START_MASTER_THREAD)
}

/// `Server::reactor_thread_main_loop`.
pub(crate) fn main_loop<H: Host>(h: &mut H, thread: u64) -> crate::Effect<()> {
    h.op(OP_THREAD_ENTER, thread)?;
    if h.signed(OP_EVENT_INIT)? < 0 {
        return Ok(());
    }
    let mode = h.get(OP_MODE)?;
    if mode == MODE_THREAD {
        h.get(OP_WORKER_START)?;
    } else {
        h.op(OP_REACTOR_THREAD_TYPE, thread)?;
    }
    if h.signed(OP_THREAD_INIT)? < 0 {
        return Ok(());
    }
    // Every reactor thread is ready before the master runs.
    if mode == MODE_PROCESS {
        h.get(OP_REACTOR_BARRIER)?;
    }
    h.get(OP_EVENT_WAIT)?;
    if mode == MODE_THREAD {
        h.get(OP_WORKER_STOP)?;
    }
    h.get(OP_THREAD_CLEAN)?;
    Ok(())
}

/// `Server::join_reactor_thread`: tell each running reactor thread to shut down and join it.
pub(crate) fn join_reactor_threads<H: Host>(h: &mut H) -> crate::Effect<()> {
    if h.is(OP_SINGLE_THREAD)? {
        return Ok(());
    }
    if h.op(OP_FIELD, u64::from(FIELD_HEARTBEAT_INTERVAL))? > 0
        && let Some(heartbeat) = h.state()?.take_thread(HEARTBEAT)
    {
        heartbeat.join().unwrap_or_default();
    }
    for thread in 0..h.get(OP_REACTOR_NUM)? {
        let Some(joining) = h.state()?.take_thread(thread) else {
            continue;
        };
        h.op(OP_THREAD_NOTIFY, thread)?;
        joining.join().unwrap_or_default();
    }
    Ok(())
}

/// The reactor-thread operations of `crate::run`.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<i64> {
    use crate::ops::{
        RUN_INIT_REACTOR, RUN_JOIN_REACTOR_THREADS, RUN_PACKET_RECEIVED, RUN_REACTOR_THREAD_MAIN,
        RUN_START_REACTOR_THREADS,
    };
    Ok(match operation {
        RUN_PACKET_RECEIVED => packet_received(h)?,
        RUN_INIT_REACTOR => {
            init_reactor(h)?;
            SW_OK
        }
        RUN_START_REACTOR_THREADS => start_reactor_threads(h)?,
        RUN_REACTOR_THREAD_MAIN => {
            main_loop(h, arg)?;
            SW_OK
        }
        RUN_JOIN_REACTOR_THREADS => {
            join_reactor_threads(h)?;
            SW_OK
        }
        _ => crate::base_factory::run(h, operation, arg)?.unwrap_or(crate::SW_ERR),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    #[test]
    fn ssl_states() {
        let mut s = Script::with(&[(OP_SSL_ACCEPT, 2)]);
        assert_eq!(verify_ssl(&mut s).unwrap(), 2, "handshake waits");

        let mut s = Script::with(&[
            (OP_SSL_ACCEPT, 5),
            (OP_HAS_ON_CONNECT, 1),
            (OP_DELAY_RECEIVE, 1),
        ]);
        assert_eq!(verify_ssl(&mut s).unwrap(), SW_READY, "ready");
        assert_eq!(s.called(OP_NOTIFY_CONNECT), 1, "onConnect");
        assert_eq!(s.called(OP_REACTOR_DEL), 1, "delayed receive");

        let cert = |peer, verify, ok| {
            let mut s = Script::with(&[
                (OP_SSL_ACCEPT, 5),
                (OP_CLIENT_CERT, 1),
                (OP_PEER_CERT, peer),
                (OP_VERIFY_PEER, verify),
                (OP_SSL_VERIFY, ok),
                (OP_HAS_ON_CONNECT, 1),
            ]);
            let code = verify_ssl(&mut s).unwrap();
            (
                code,
                s.called(OP_CERT_TASK).min(s.called(OP_FACTORY_DISPATCH)),
                s.called(OP_NOTIFY_CONNECT),
            )
        };
        assert_eq!(cert(0, 1, 0), (SW_ERROR, 0, 0), "missing certificate");
        assert_eq!(cert(0, 0, 0), (SW_READY, 0, 1), "optional certificate");
        assert_eq!(cert(1, 1, 0), (SW_ERROR, 0, 0), "rejected certificate");
        assert_eq!(cert(1, 1, 1), (SW_READY, 1, 0), "verified certificate");
        assert_eq!(cert(1, 0, 0), (SW_READY, 1, 0), "unverified certificate");
    }

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

    static RAN: std::sync::Mutex<Vec<(usize, u64)>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "C" fn record(serv: *mut core::ffi::c_void, arg: u64) {
        RAN.lock().unwrap().push((serv.addr(), arg));
    }

    #[test]
    fn start_and_join_threads() {
        let entry = (record as *const ()).expose_provenance() as u64;
        let mut s = Script::with(&[
            (OP_PORT_COUNT, 2),
            (OP_REACTOR_NUM, 3),
            (OP_FIELD, 5),
            (OP_REACTOR_THREAD_ENTRY, entry),
            (OP_HEARTBEAT_ENTRY, entry),
            (OP_SERVER, 64),
        ])
        .queue(OP_PORT_DGRAM, &[1, 0]);
        assert_eq!(start_reactor_threads(&mut s).unwrap(), SW_OK, "master ran");
        assert_eq!(s.args(OP_PORT_LISTEN), vec![1], "stream port listens");
        join_reactor_threads(&mut s).unwrap();
        let mut ran = RAN.lock().unwrap().clone();
        ran.sort_unstable();
        assert_eq!(
            ran,
            vec![(64, 0), (64, 1), (64, 2), (64, HEARTBEAT)],
            "threads"
        );
        assert_eq!(s.args(OP_THREAD_NOTIFY), vec![0, 1, 2], "each told to stop");

        let mut s = Script::with(&[(OP_PORT_COUNT, 1), (OP_PORT_LISTEN, u64::MAX)]);
        assert_eq!(
            start_reactor_threads(&mut s).unwrap(),
            SW_ERR,
            "listen failed"
        );
        assert_eq!(s.called(OP_EVENT_FREE), 1, "loop freed");

        let mut s = Script::with(&[(OP_REACTOR_NUM, 2), (OP_FIELD, 1)]);
        join_reactor_threads(&mut s).unwrap();
        assert_eq!(s.called(OP_THREAD_NOTIFY), 0, "none started");
    }

    #[test]
    fn main_loop_runs_a_reactor_thread() {
        let mut s = Script::with(&[(OP_MODE, MODE_PROCESS)]);
        main_loop(&mut s, 2).unwrap();
        assert_eq!(s.args(OP_REACTOR_THREAD_TYPE), vec![2], "reactor thread");
        assert_eq!(s.called(OP_REACTOR_BARRIER), 1, "barrier");
        assert_eq!(s.called(OP_THREAD_CLEAN), 1, "cleaned");
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
        close(serv, reactor, 1);
        // The close itself runs conn_io's close path, whose counts only the hybrid
        // answers: here, only that the worker isn't told again shows.
        assert_eq!(handled(), [], "a forced close closes now");
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
}
