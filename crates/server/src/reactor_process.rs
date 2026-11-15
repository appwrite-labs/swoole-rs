//! Base-mode reactor processes (upstream's `src/server/reactor_process.cc`): every
//! worker accepts, reads and runs its callbacks itself. What only the hybrid links
//! runs as its steps; the reads from the server's message bus are in `native`.
use core::ffi::c_int;
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_process::worker_record::{Worker, WorkerType};
use swoole_rs_reactor::Events;
use swoole_rs_reactor::{Event, Running};

use crate::callbacks::{CallbackKind, ServerSlot};
use crate::hybrid::{Step, hybrid};
use crate::reactor_handle::Reactor;
use crate::view;

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) mod native;

/// `SW_FD_SESSION`, `SW_FD_STREAM_SERVER` and `SW_SSL_STATE_READY`.
const FD_SESSION: c_int = 0;
const FD_STREAM_SERVER: c_int = 1;
use swoole_rs_tls::SslState::SW_SSL_STATE_READY as SSL_READY;

/// `Socket::is_stream(type)`.
fn is_stream(port: view::Port<'_>) -> bool {
    matches!(port.config().r#type, 1 | 3 | 5)
}

/// Whether listening ports may use `SO_REUSEPORT` (`enable_reuse_port` on Linux).
fn reuse_port(config: &crate::ServerConfig) -> bool {
    cfg!(target_os = "linux") && config.enable_reuse_port.get()
}

/// `Server::start_reactor_processes()`: the ports, the event worker pool and its
/// workers, run by this process when there is one, else by the manager.
pub(crate) fn start_reactor_processes(serv: view::Server<'_>) -> bool {
    let config = serv.config();
    config.single_thread.set(true);
    // Listen on the TCP ports; with SO_REUSEPORT each worker listens itself.
    if config.have_stream_sock.get() {
        for (index, &port) in serv.ports().iter().enumerate() {
            if !is_stream(port) {
                continue;
            }
            if reuse_port(config) {
                hybrid().step(serv, Step::ClosePortSocket(index));
            } else if !hybrid().step(serv, Step::Listen(index)) {
                return false;
            }
        }
    }
    if !hybrid().step(serv, Step::CreateEventPool) {
        return false;
    }
    config.init_ipc_max_size();
    if !hybrid().step(serv, Step::CreatePipeBuffers) {
        return false;
    }
    let started = if serv.is_single_worker() {
        let worker = crate::shared::ServerShared::of(serv).event_pool().worker(0);
        let raw = worker.map_or(core::ptr::null_mut(), Worker::raw);
        swoole_rs_process::worker_globals::set_current_worker(raw);
        worker.is_some_and(|worker| main_loop(serv, worker))
    } else {
        hybrid().step(serv, Step::StartManager)
    };
    if started {
        hybrid().step(serv, Step::DestroyEventPool);
    }
    started
}

/// `Server::reactor_process_main_loop`: one base-mode worker, on this thread's
/// reactor, until it stops.
pub(crate) fn main_loop(serv: view::Server<'_>, worker: Worker<'_>) -> bool {
    use swoole_rs_process::methods as process;
    let id = worker.id();
    process::set_worker_type(WorkerType::Event);
    process::swoole_set_worker_id(id);
    process::swoole_set_worker_pid(std::process::id().cast_signed());
    crate::worker_methods::init_event_worker(serv, worker);
    if swoole_rs_corebase::thread_reactor().is_null() && !hybrid().step(serv, Step::EventInit) {
        return false;
    }
    hybrid().step(serv, Step::TimerReinit);
    hybrid().step(serv, Step::WorkerSignalInit);
    let slot = id as usize;
    if let Some(count) = serv.worker_connections(slot) {
        count.store(0, Relaxed);
    }
    let reactor = Reactor::thread();
    let config = serv.config();
    for (index, &port) in serv.ports().iter().enumerate() {
        if reuse_port(config) && is_stream(port) {
            if !hybrid().step(serv, Step::CreatePortSocket(index)) {
                hybrid().step(serv, Step::EventFree);
                return false;
            }
            if !hybrid().step(serv, Step::Listen(index)) {
                return false;
            }
        }
        if let Some(count) = serv.port_worker_connections(port, slot) {
            count.store(0, Relaxed);
        }
        if port
            .with_socket(|socket| reactor.add(socket, Events::READ))
            .is_none_or(|added| !added)
        {
            return false;
        }
    }
    let running = reactor.handle();
    running.set_id(u16::try_from(id).unwrap_or(u16::MAX));
    running.set_object(serv.raw());
    running.set_max_socket(serv.state().tables_ref().max_connection);
    running.set_close(Some(crate::reactor_entry::close_connection));
    reactor.set_handler(
        FD_STREAM_SERVER,
        Events::READ,
        crate::server_methods::accept_connection,
    );
    reactor.set_error_handler(on_close);
    hybrid().step(serv, Step::PipeHandler);
    crate::server_methods::store_listen_socket(serv);
    if !worker.pipe_worker().is_null() && !add_pipes(reactor, worker) {
        return false;
    }
    if config.task_worker_num.get() > 0 && config.task_ipc() == Some(crate::TaskIpcMode::UnixSock) {
        let pool = crate::shared::ServerShared::of(serv).task_pool();
        for index in 0..usize::try_from(pool.worker_num()).unwrap_or(0) {
            if let Some(pipe) = pool.worker(index).and_then(|w| view::worker_pipe(w, true)) {
                view::set_nonblock(pipe);
            }
        }
    }
    crate::reactor_thread::init_reactor(serv, reactor);
    if id == 0 {
        let counters = serv.counters();
        counters
            .master_pid
            .store(std::process::id().cast_signed(), Relaxed);
        let callbacks = &serv.state().callbacks;
        let on_start = callbacks.slot(CallbackKind::Start).is_set();
        if on_start && !counters.onstart_called.load(Relaxed) {
            counters.onstart_called.store(true, Relaxed);
            callbacks.call_server(ServerSlot::Start, serv);
        }
    }
    if !hybrid().step(serv, Step::MasterTimer) {
        hybrid().step(serv, Step::EventFree);
        return false;
    }
    hybrid().step(serv, Step::WorkerStart(id));
    // The heartbeat check.
    if config.heartbeat_check_interval.get() > 0 && !hybrid().step(serv, Step::HeartbeatTimer) {
        hybrid().step(serv, Step::EventFree);
        return false;
    }
    let waited = hybrid().step(serv, Step::ReactorWait);
    // Close every connection.
    crate::reactor_thread::each_connection(serv.state(), |conn| {
        crate::server_methods::close(serv, conn.session_id(), true);
    });
    hybrid().step(serv, Step::EventFree);
    hybrid().step(serv, Step::WorkerStop(id));
    waited
}

/// The worker's pipe ends, nonblocking, read by `reactor`: whether both were added.
fn add_pipes(reactor: Reactor, worker: Worker<'_>) -> bool {
    let pipes = (
        view::worker_pipe(worker, false),
        view::worker_pipe(worker, true),
    );
    let (Some(pipe_worker), Some(pipe_master)) = pipes else {
        return false;
    };
    view::set_nonblock(pipe_worker);
    view::set_nonblock(pipe_master);
    reactor.add(pipe_worker, Events::READ) && reactor.add(pipe_master, Events::READ)
}

/// `run` over the server of `reactor`, a reactor process's reactor: the one
/// [`main_loop`] runs, whose handlers and heartbeat timer are this module's.
pub(crate) fn with_server(reactor: Reactor, run: impl FnOnce(view::Server<'_>) -> c_int) -> c_int {
    // SAFETY: `main_loop` set the reactor's `ptr` to its live server before it added
    // the handlers and timer that call this, and the server outlives its loop.
    run(unsafe { crate::reactor_entry::server(reactor) })
}

/// `ReactorProcess_onClose`: the reactor's default error handler.
extern "C" fn on_close(reactor: Running<'_>, event: &mut Event) -> c_int {
    let reactor = Reactor::running(reactor);
    with_server(reactor, |serv| {
        crate::conn_io::status(close(serv, reactor, event))
    })
}

/// The peer closed (or errored) the connection of `event`: closed now when it already
/// left the reactor or its close is queued, else the worker hears of it.
pub(crate) fn close(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> bool {
    let Some(conn) = serv.at(event.fd()).filter(|conn| conn.is_active()) else {
        return false;
    };
    let Some(socket) = event.socket() else {
        return false;
    };
    if socket.read(|s| s.removed) {
        return crate::conn_io::close_connection(serv, reactor, Some(socket));
    }
    if !reactor.del(socket) {
        return false;
    }
    if conn.close_queued() {
        return crate::conn_io::close_connection(serv, reactor, Some(socket));
    }
    // The client closed the connection: it is no longer usable.
    conn.set_peer_closed(true);
    crate::server_methods::notify(serv, conn, ServerEvent::Close)
}

/// `ReactorProcess_onTimeout`: close the connections idle past the heartbeat, or
/// tell their worker.
pub(crate) fn heartbeat(serv: view::Server<'_>, reactor: Reactor) {
    let now = swoole_rs_corebase::microtime();
    crate::reactor_thread::each_connection(serv.state(), |conn| {
        if serv.healthy(now, conn) {
            return;
        }
        let ssl_pending = conn
            .socket()
            .is_some_and(|s| s.read(|s| !s.ssl().is_null() && s.tls.state != SSL_READY));
        if ssl_pending || serv.config().disable_notify.get() || conn.close_force() {
            crate::conn_io::close_connection(serv, reactor, conn.socket());
            return;
        }
        conn.set_close_force(true);
        close(
            serv,
            reactor,
            &conn.event(i16::from(conn.reactor_id()), FD_SESSION),
        );
    });
}

/// MSHUTDOWN after `exit()` or a fatal error in a single base-mode worker's callback:
/// the bailout left its loop before `start_reactor_processes` destroyed the event
/// worker pool and `Server::start` the server. That worker is the process that made
/// both, with no manager or other worker to share them, so this frees the pool and
/// what `destroy` would, without the shutdown callbacks upstream never runs on that
/// path. True when `server` is that server, which its owner then deletes; any other
/// server is left alone.
#[must_use]
pub fn release_abandoned(server: view::Server<'_>) -> bool {
    if server.raw() != swoole_rs_corebase::swr_v1_core_server()
        || !server.ran_its_single_worker_here()
    {
        return false;
    }
    hybrid().step(server, Step::DestroyEventPool);
    crate::startup::release_abandoned_base(server);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn_io::tests::{accepted, closed, session};
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls, fail};
    use Step::*;
    use core::ffi::c_long;
    use swoole_rs_process::worker_record::WorkerRecord;

    fn steps(steps: &[Step]) -> Vec<Call> {
        steps.iter().map(|&step| Call::Step(step)).collect()
    }

    /// The live server's configuration, which the test alone reads and writes.
    #[expect(
        clippy::mut_from_ref,
        reason = "each use is the test's only reference to the configuration"
    )]
    fn config(live: &LiveServer) -> &mut crate::ServerConfig {
        unsafe { &mut *live.state().config() }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn start_listens_then_the_manager_runs_the_workers() {
        let live = LiveServer::new();
        config(&live).mode_ = crate::Mode::Base;
        config(&live).have_stream_sock.set(true);
        config(&live).worker_num.set(2);
        calls();
        assert!(start_reactor_processes(live.server()));
        assert_eq!(
            calls(),
            steps(&[
                Listen(0),
                CreateEventPool,
                CreatePipeBuffers,
                StartManager,
                DestroyEventPool
            ])
        );
        assert!(config(&live).single_thread.get(), "a single thread");

        for (failing, ran) in [
            (Listen(0), &[Listen(0)][..]),
            (CreateEventPool, &[Listen(0), CreateEventPool]),
            (
                StartManager,
                &[Listen(0), CreateEventPool, CreatePipeBuffers, StartManager],
            ),
        ] {
            fail(Some(failing));
            assert!(!start_reactor_processes(live.server()), "{failing:?}");
            assert_eq!(calls(), steps(ran), "{failing:?}: the pool stays");
        }
        fail(None);

        config(&live).have_stream_sock.set(false);
        config(&live).worker_num.set(1);
        assert!(
            !start_reactor_processes(live.server()),
            "a single worker needs its pool's record"
        );
        assert_eq!(
            calls(),
            steps(&[CreateEventPool, CreatePipeBuffers]),
            "no listening or manager"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn main_loop_order_and_cleanup() {
        let live = LiveServer::new();
        // SAFETY: a plain record, for which zero is valid.
        let mut record: WorkerRecord = unsafe { core::mem::zeroed() };
        // SAFETY: a zeroed worker record, live for the test.
        let worker = unsafe { Worker::new(&raw mut record) };
        calls();
        assert!(main_loop(live.server(), worker));
        assert_eq!(
            calls(),
            steps(&[
                TimerReinit,
                WorkerSignalInit,
                PipeHandler,
                MasterTimer,
                WorkerStart(0),
                ReactorWait,
                EventFree,
                WorkerStop(0),
            ])
        );
        let reactor = live.reactor().handle();
        assert_eq!(reactor.id(), 0, "the worker's reactor");
        assert_eq!(reactor.object(), live.server().raw(), "it holds the server");
        assert_eq!(
            live.counters().master_pid.load(Relaxed),
            std::process::id().cast_signed(),
            "worker 0 is the master"
        );
        assert_eq!(
            swoole_rs_process::methods::worker_type(),
            Some(WorkerType::Event)
        );
        let fd = live.listener_fd;
        assert_eq!(live.counters().max_fd(), fd, "listen sockets stored");
        live.reactor().del(live.port_socket());

        fail(Some(MasterTimer));
        assert!(!main_loop(live.server(), worker));
        assert_eq!(calls()[3..], steps(&[MasterTimer, EventFree]), "no loop");
        live.reactor().del(live.port_socket());

        config(&live).heartbeat_check_interval.set(5);
        fail(Some(HeartbeatTimer));
        assert!(!main_loop(live.server(), worker));
        assert_eq!(
            calls()[3..],
            steps(&[MasterTimer, WorkerStart(0), HeartbeatTimer, EventFree]),
            "the worker started, no loop"
        );
        fail(None);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn close_notifies_the_worker_unless_the_close_is_queued() {
        let (live, _clients) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        let fd = conn.fd();
        assert!(close(serv, reactor, &conn.event(0, 0)));
        let listener = u16::try_from(live.listener_fd).unwrap();
        let notify = Call::Notify(c_long::from(fd), 0, 4, listener);
        assert_eq!(calls(), [notify], "the worker hears");
        assert!(conn.peer_closed(), "no longer usable");
        assert!(conn.is_active(), "the worker closes it");

        let conn = session(&live, 2);
        conn.set_close_queued(true);
        assert!(close(serv, reactor, &conn.event(0, 0)));
        assert_eq!(calls(), [], "a queued close closes now");
        assert!(closed(conn));
        assert!(!close(serv, reactor, &conn.event(0, 0)), "gone");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn close_closes_a_socket_already_out_of_the_reactor() {
        let (live, _clients) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        let conn = session(&live, 1);
        assert!(reactor.del(conn.socket().unwrap()));
        assert!(close(serv, reactor, &conn.event(0, 0)));
        assert_eq!(calls(), [], "nobody is told");
        assert!(closed(conn));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn heartbeat_closes_or_notifies_idle_connections() {
        let (live, _clients) = accepted();
        let (serv, reactor) = (live.server(), live.reactor());
        live.port().heartbeat_idle_time.set(1);
        let (idle, forced) = (session(&live, 1), session(&live, 2));
        idle.set_last_recv_time(1.0);
        forced.set_last_recv_time(1.0);
        forced.set_close_force(true);
        heartbeat(serv, reactor);
        let listener = u16::try_from(live.listener_fd).unwrap();
        let notify = Call::Notify(c_long::from(idle.fd()), 0, 4, listener);
        assert_eq!(calls(), [notify], "the idle one's worker hears");
        assert!(idle.close_force(), "and it closes by force");
        assert!(closed(forced), "a forced close closes now");

        idle.set_last_recv_time(swoole_rs_corebase::microtime());
        heartbeat(serv, reactor);
        assert_eq!(calls(), [], "healthy");
    }
}
