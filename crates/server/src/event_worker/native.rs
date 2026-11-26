//! The event worker's native side: the events it receives from the message bus, its
//! start and stop callbacks, its signals, the async stop and its exit wait, and the
//! process-mode main loop.
use crate::{DispatchMode, Mode};
use core::ffi::{c_int, c_long};
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::process;
use swoole_rs_corebase::process::pool::PIPE_MASTER;
use swoole_rs_corebase::reactor::ErrorHandler::ConnectionDefault;
use swoole_rs_corebase::reactor::Events;
use swoole_rs_corebase::reactor::SocketFields;

use process::methods::{
    ExitStatus, push_stop, set_worker_type, swoole_get_worker_id, swoole_set_worker_id, worker_type,
};
use process::worker_globals::{
    current_worker, with_current, with_globals, worker_global_detach, worker_global_stopped,
};
use process::worker_record::{Worker, WorkerStatus, WorkerType, swr_v1_worker_shutdown};
use swoole_rs_corebase::reactor::{Event, Heap, Running};
use swoole_rs_corebase::{DataHead, log};

use super::{SignalHandler, Wait};
use crate::callbacks::{CallbackKind, EventSlot, ReceiveSlot, ServerSlot, TaskSlot, WorkerSlot};
use crate::objects::{Bus, Reactor, set_nonblock, worker_pipe};
use crate::records::Lookup;
use crate::view;
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as WARNING;

/// `SW_FD_PIPE`.
const FD_PIPE: c_int = swoole_rs_corebase::constants::FdType::SW_FD_PIPE as c_int;
/// `SW_LOG_WARNING` and the `SW_ERROR_*` the worker logs.
const INVALID_PARAMS: c_int = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as c_int;
const WRONG_OPERATION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int;
const SESSION_DISCARD_TIMEOUT_DATA: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SESSION_DISCARD_TIMEOUT_DATA as c_int;
const SERVER_WORKER_EXIT_TIMEOUT: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SERVER_WORKER_EXIT_TIMEOUT as c_int;
const SERVER_WORKER_UNPROCESSED_DATA: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SERVER_WORKER_UNPROCESSED_DATA as c_int;

/// `f` over `serv->get_worker_message_bus()`.
#[inline]
fn with_bus<R>(serv: view::Server<'_>, f: impl FnOnce(Bus<'_>) -> R) -> R {
    crate::master_ops::with_worker_message_bus(serv.state(), f)
}

/// `serv->get_worker(id)`, or `None` past the last worker.
fn other(serv: view::Server<'_>, id: u32) -> Option<Worker<'_>> {
    serv.worker_record(u16::try_from(id).unwrap_or(u16::MAX))
}

/// Every event and task worker: `worker_num + task_worker_num`.
fn all_core_workers(config: &crate::ServerConfig) -> u32 {
    config.worker_num.get() + config.task_worker_num.get()
}

/// `reactor->remove_read_event(pipe)` when `pipe` is still in the loop.
fn unread(reactor: Reactor, pipe: Option<Heap<'_>>) {
    if let Some(sock) = pipe.filter(|s| s.read(|s| !s.has_left_reactor())) {
        reactor.unread(sock);
    }
}

/// [`unread`] of `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the
/// thread bus's own end of `pipe`.
fn unread_in_bus(serv: view::Server<'_>, reactor: Reactor, pipe: Option<Heap<'_>>) {
    let unread = |pipe: Heap<'_>| unread(reactor, Some(pipe));
    crate::objects::in_worker_bus(serv.state(), serv.config(), pipe, unread);
}

/// Whether a connection's socket is still read: its peer hasn't closed and it is in
/// the loop.
fn readable(conn: crate::Connection<'_>) -> bool {
    let removed = conn
        .socket()
        .is_none_or(|s| s.read(SocketFields::has_left_reactor));
    !conn.peer_closed() && !removed
}

/// Where an event's data is: in the worker message bus, or passed with the event
/// (base mode's `MessageBus::pass`).
#[derive(Clone, Copy)]
pub(crate) enum Payload<'a> {
    Bus,
    Passed(&'a [u8]),
}

impl Payload<'_> {
    /// `f` over the event's data.
    fn with<R>(self, serv: view::Server<'_>, f: impl FnOnce(&[u8]) -> R) -> R {
        match self {
            Self::Bus => with_bus(serv, |bus| bus.with_packet(f)),
            Self::Passed(data) => f(data),
        }
    }

    /// `f` over the event under the head `info`, as a receive callback takes it: the
    /// bus's packet, which `f` may take, or the data passed with the event.
    #[inline(always)]
    fn receive(
        self,
        serv: view::Server<'_>,
        info: &DataHead,
        f: impl FnOnce(&mut crate::reactor_entry::RecvData<'_>),
    ) {
        match self {
            Self::Bus => {
                let mut packet = with_bus(serv, |bus| bus.rust().take_packet());
                f(&mut packet.receive(*info));
                with_bus(serv, |bus| bus.rust().restore(packet));
            }
            Self::Passed(data) => f(&mut crate::reactor_entry::RecvData::new(*info, data)),
        }
    }
}

/// `Server::worker_accept_event(info)`: the event whose head is `info` (its data, if
/// any, at `payload`), in this thread's worker.
pub(crate) fn accept_event(serv: view::Server<'_>, info: &DataHead, payload: Payload<'_>) {
    _ = with_current(|&worker| accept(serv, worker, info, payload));
}

fn accept(serv: view::Server<'_>, worker: Worker<'_>, info: &DataHead, payload: Payload<'_>) {
    let (state, config) = (serv.state(), serv.config());
    let callbacks = &state.callbacks;
    worker.set_status(WorkerStatus::Busy);
    match info.event() {
        Some(ServerEvent::RecvData) => payload.receive(serv, info, |recv| {
            let closed = received(serv, recv.info());
            let (notify, timeout) = (
                config.disable_notify.get(),
                config.discard_timeout_request.get(),
            );
            if super::discard(closed, notify, timeout) {
                let (len, session) = (info.len, info.fd);
                log::error_log(
                    WARNING,
                    SESSION_DISCARD_TIMEOUT_DATA,
                    c"Worker_discard_data",
                    format_args!("[2] ignore data[{len} bytes] received from session#{session}"),
                );
            } else {
                run_task(serv, worker, recv, ReceiveSlot::Receive);
            }
        }),
        Some(ServerEvent::RecvDgram) => payload.receive(serv, info, |recv| {
            run_task(serv, worker, recv, ReceiveSlot::Packet);
        }),
        Some(ServerEvent::Close) => {
            free_client_cert(serv, info);
            _ = crate::factory::end(serv, info.fd, 0);
        }
        Some(ServerEvent::Connect) => {
            if info.len > 0 {
                store_client_cert(serv, info, payload);
            }
            callbacks.event(EventSlot::Connect, serv, info);
        }
        Some(ServerEvent::BufferFull) => callbacks.event(EventSlot::BufferFull, serv, info),
        Some(ServerEvent::BufferEmpty) => callbacks.event(EventSlot::BufferEmpty, serv, info),
        Some(ServerEvent::Finish) => {
            _ = with_bus(serv, |bus| {
                bus.with_event(|event| callbacks.task(TaskSlot::Finish, serv, event))
            });
        }
        Some(ServerEvent::PipeMessage) => {
            _ = with_bus(serv, |bus| {
                bus.with_event(|event| callbacks.pipe_message(serv, event))
            });
        }
        Some(ServerEvent::CommandRequest) => {
            // The server's live command pipe, a `UnixSocket`.
            if let Some(pipe) = config.command_pipe() {
                let id = u16::try_from(worker.id()).unwrap_or(0);
                let bus = state.message_bus();
                crate::commands::call_command_handler(serv, bus, id, pipe.worker_end());
            }
        }
        Some(ServerEvent::Shutdown) => stop_async_worker(serv, worker),
        _ => log::warning(
            c"Server::worker_accept_event",
            format_args!("[Worker] error event[type={}]", info.kind),
        ),
    }
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return;
    }
    worker.set_status(WorkerStatus::Idle);
    // The maximum number of requests: the worker exits.
    if worker.exceeded_max_request() {
        if config.mode_ == Mode::Thread {
            // This reactor thread's reactor, and its own record.
            let reactor = Reactor::thread();
            let id = usize::from(reactor.handle().id());
            crate::reactor_entry::thread_shutdown(serv, reactor, id);
        } else {
            stop_async_worker(serv, worker);
        }
    }
}

/// The connection received data (whose head is `info`) is for, its queued bytes (less
/// the `len` received) and dispatch time updated: `None` without one, else whether
/// the server closed it.
fn received(serv: view::Server<'_>, info: &DataHead) -> Option<bool> {
    let conn = serv.state().connection(info.fd, Lookup::VerifiedTls)?;
    if info.len > 0 {
        conn.recv_queued_bytes().fetch_sub(info.len, Relaxed);
    }
    conn.set_last_dispatch_time(info.time);
    Some(conn.closed().load(Relaxed) != 0)
}

/// The receive or packet callback over `recv`; a request counted when it succeeds.
fn run_task(
    serv: view::Server<'_>,
    worker: Worker<'_>,
    recv: &mut crate::reactor_entry::RecvData<'_>,
    slot: ReceiveSlot,
) {
    let result = serv.state().callbacks.receive(slot, serv, recv);
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return;
    }
    if result.is_ok() {
        worker.add_request_count();
        serv.counters().request_count.fetch_add(1, Relaxed);
    }
}

/// The session's client certificate, sent with its connect event.
fn store_client_cert(serv: view::Server<'_>, info: &DataHead, payload: Payload<'_>) {
    if let Some(conn) = serv.state().connection(info.fd, Lookup::Verified) {
        payload.with(serv, |cert| conn.store_client_cert(cert));
    }
}

/// Frees the session's client certificate, if this worker stored it.
fn free_client_cert(serv: view::Server<'_>, info: &DataHead) {
    if let Some(conn) = serv.state().connection(info.fd, Lookup::Verified) {
        conn.free_client_cert();
    }
}

/// `Server::worker_start_callback(worker)`.
pub(crate) fn worker_start_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    let config = serv.config();
    if swoole_rs_corebase::ossupport::swr_v1_os_is_root_user() {
        serv.state().set_isolation();
    }
    let me = worker.id();
    for id in (0..all_core_workers(config)).filter(|&id| id != me) {
        if worker_type() == Some(WorkerType::Event)
            && let Some(pipe) = other(serv, id).and_then(|w| worker_pipe(w, true))
        {
            set_nonblock(*pipe);
        }
    }
    worker.set_status(WorkerStatus::Idle);
    if config.mode_ == Mode::Process {
        // Upstream ignores whether the session list became read-only.
        _ = serv.state().tables_ref().protect_sessions(libc::PROT_READ);
    }
    call_worker_start_callback(serv, worker);
}

/// `Server::call_worker_start_callback(worker)`.
pub(crate) fn call_worker_start_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    #[cfg(target_os = "linux")]
    if serv.config().open_cpu_affinity.get() {
        set_cpu_affinity(serv.config(), worker.id());
    }
    serv.call_hook(view::ServerHook::BeforeWorkerStart(worker.id()));
    log::set_last_error(0);
    swoole_rs_error::swoole_clear_last_error_msg();
    let callbacks = &serv.state().callbacks;
    callbacks.worker(WorkerSlot::Start, serv, worker);
}

/// `Server::call_worker_stop_callback(worker)`.
pub(crate) fn call_worker_stop_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    serv.call_hook(view::ServerHook::BeforeWorkerStop(worker.id()));
    serv.state()
        .callbacks
        .worker(WorkerSlot::Stop, serv, worker);
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return;
    }
    with_bus(serv, |bus| {
        if !bus.is_empty() {
            log::error_log(
                WARNING,
                SERVER_WORKER_UNPROCESSED_DATA,
                c"Server::call_worker_stop_callback",
                format_args!("unprocessed data in the worker process buffer"),
            );
            bus.clear();
        }
    });
    worker_global_stopped();
}

/// `Server::call_worker_error_callback(worker, status)`.
pub(crate) fn call_worker_error_callback(
    serv: view::Server<'_>,
    worker: Worker<'_>,
    status: &ExitStatus,
) {
    serv.state().callbacks.worker_error(serv, worker, status);
    if worker.kind() == Some(WorkerType::Event) {
        crate::worker_methods::abort_worker(serv, worker);
    }
}

/// `Server::kill_worker(worker_id)`: ask a worker (-1 for this thread's) to stop.
#[must_use]
pub fn kill_worker(serv: view::Server<'_>, worker_id: c_int) -> bool {
    let id = if worker_id < 0 {
        if current_worker().is_null() {
            log::error_log(
                WARNING,
                WRONG_OPERATION,
                c"Server::kill_worker",
                format_args!("kill worker in non worker process requires specifying an id"),
            );
            return false;
        }
        swoole_get_worker_id()
    } else {
        worker_id.cast_unsigned()
    };
    let Some(worker) = other(serv, id).filter(|_| u16::try_from(id).is_ok()) else {
        log::error_log(
            WARNING,
            INVALID_PARAMS,
            c"Server::kill_worker",
            format_args!("the worker_id[{}] is invalid", id.cast_signed()),
        );
        return false;
    };
    let head = DataHead {
        kind: ServerEvent::Shutdown.code(),
        ..DataHead::default()
    };
    let data = process::process_pool::head_view(&head);
    process::methods::send_pipe_message(crate::process_pool_native(), worker, &data, PIPE_MASTER)
        .is_some()
}

/// `Server::stop_async_worker(worker)`: stop reading, leave the pool and wait for the
/// reactor's remaining work before exiting.
pub fn stop_async_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    let config = serv.config();
    swr_v1_worker_shutdown(worker);
    if worker.kind() == Some(WorkerType::Event) {
        crate::worker_methods::reset_worker_counter(serv, worker);
    }
    let reactor = Reactor::thread();
    // Forced termination.
    if !config.reload_async.get() {
        reactor.handle().set_running(false);
        return;
    }
    // The worker is already shutting down.
    if reactor.handle().wait_exit() {
        return;
    }
    // Separated from the event worker pool.
    worker_global_detach(worker);
    unread_in_bus(serv, reactor, worker_pipe(worker, false));
    let event_worker = worker_type() == Some(WorkerType::Event);
    match config.mode_ {
        Mode::Base if event_worker => {
            let pool = crate::shared::ServerShared::of(serv).event_pool();
            if worker.id() == 0 && !pool.running() {
                serv.call_hook(view::ServerHook::BeforeShutdown);
                serv.state()
                    .callbacks
                    .call_server(ServerSlot::BeforeShutdown, serv);
            }
            unread(reactor, worker_pipe(worker, true));
            for port in serv.ports() {
                _ = port.with_socket(|sock| reactor.del(*sock));
            }
            crate::reactor_thread::each_connection(serv.state(), |conn| {
                if readable(conn)
                    && let Some(sock) = conn.socket()
                {
                    reactor.unread(sock);
                }
            });
            crate::control::clear_timer(serv);
        }
        Mode::Base => {}
        Mode::Process => {
            let pid = std::process::id().cast_signed();
            let id = u16::try_from(worker.id()).unwrap_or(u16::MAX);
            if !push_stop(crate::process_pool_native(), serv.event_pool_ref(), pid, id) {
                let m = format_args!("failed to push WORKER_STOP message");
                log::sys_warning(c"Server::stop_async_worker", m);
            }
        }
        Mode::Thread if event_worker => {
            // Thread mode forwards through the master pipes, which may be watched for
            // writes: stop reading them before the thread exits.
            let me = reactor.handle().id();
            let pipes = super::reactor_pipes(config.worker_num.get(), config.reactor_num.get(), me);
            for id in pipes {
                if let Some(worker) = other(serv, id) {
                    unread_in_bus(serv, reactor, worker_pipe(worker, true));
                }
            }
            // Only reads stop; sends may still be in flight until the reactor is
            // destroyed.
            crate::reactor_thread::each_connection(serv.state(), |conn| {
                if u16::from(conn.reactor_id()) == me
                    && readable(conn)
                    && let Some(sock) = conn.socket()
                {
                    reactor.unread(sock);
                }
            });
        }
        Mode::Thread => {}
    }
    reactor.handle().set_wait_exit(true);
    swoole_rs_corebase::reactor::swr_v1_event_set_end_callback(
        swoole_rs_corebase::reactor::Priority::TryExit,
        try_to_exit,
    );
    let now = swoole_rs_corebase::time_now();
    with_globals(|globals| globals.exit_time.store(now, Relaxed));
    try_exit(serv, reactor);
}

/// `Worker_reactor_try_to_exit`: the loop's end callback while the worker exits.
fn try_to_exit() {
    let reactor = Reactor::thread();
    if let Some(serv) = reactor.server() {
        try_exit(serv, reactor);
    }
}

/// Once the reactor can exit, stop it; meanwhile call `onWorkerExit` once, then wait
/// at most `max_wait_time` since the stop.
fn try_exit(serv: view::Server<'_>, reactor: Reactor) {
    let callbacks = &serv.state().callbacks;
    let on_exit = callbacks.slot(CallbackKind::WorkerExit).is_set();
    let mut called = false;
    loop {
        if reactor.if_exit() {
            reactor.handle().set_running(false);
        } else {
            if !called && on_exit {
                called = true;
                // This thread's worker, as `sw_worker()`; without one there is none.
                _ = with_current(|&worker| callbacks.worker(WorkerSlot::Exit, serv, worker));
                continue;
            }
            let exit_time = with_globals(|globals| globals.exit_time.load(Relaxed));
            let now = swoole_rs_corebase::time_now();
            let max_wait = serv.config().max_wait_time.get();
            let handle = reactor.handle();
            match super::exit_wait(now, exit_time, max_wait, handle.timeout_msec()) {
                Wait::Stop => {
                    log::error_log(
                        WARNING,
                        SERVER_WORKER_EXIT_TIMEOUT,
                        c"Worker_reactor_try_to_exit",
                        format_args!("worker exit timeout, forced termination"),
                    );
                    handle.set_running(false);
                }
                Wait::Timeout(limit) => handle.set_timeout_msec(limit),
                Wait::Keep => {}
            }
        }
        break;
    }
}

/// `Worker_onPipeReceive`: a message from a reactor thread or the master (process mode).
fn on_pipe_receive(reactor: Running<'_>, event: &mut Event) -> bool {
    // Registered only on a worker's reactor, which holds its server.
    let Some(serv) = Reactor::running(reactor).server() else {
        return false;
    };
    with_bus(serv, |bus| {
        if event
            .socket()
            .is_some_and(|socket| bus.read(*socket) > Some(0))
        {
            accept_event(serv, &bus.rust().head(), Payload::Bus);
            bus.pop();
        }
    });
    true
}

/// `Server::worker_signal_init()`.
pub fn worker_signal_init(serv: view::Server<'_>) {
    if serv.config().mode_ == Mode::Thread {
        return;
    }
    for (signo, handler) in super::signals(SignalHandler::Ignore) {
        swr_workers_signal_set(signo, handler, worker_signal_handler);
    }
}

/// Installs `handler` (one of [`super::signals`]'s) for `signo`, `own` for the worker's
/// own.
pub(crate) fn swr_workers_signal_set(
    signo: c_int,
    handler: SignalHandler,
    own: extern "C" fn(c_int),
) {
    use swoole_rs_corebase::process::signal_upstream::{self, Handler};
    let handler = match handler {
        SignalHandler::None => Handler::NONE,
        SignalHandler::Ignore => Handler::IGNORE,
        SignalHandler::Own => Handler::new(own),
    };
    _ = signal_upstream::set(signo, handler, false);
}

/// `Server::worker_signal_handler(signo)`, over `SwooleG.server`.
pub(crate) extern "C" fn worker_signal_handler(signo: c_int) {
    crate::server_methods::on_signal(crate::server_methods::SignalBody::Worker, signo);
}

/// [`worker_signal_handler`]'s body over the server.
pub(crate) fn signal(serv: view::Server<'_>, signo: c_int) {
    let live = swoole_rs_corebase::swr_v1_core_running() && serv.state().running();
    _ = with_current(|&worker| {
        if !live {
            return;
        }
        if signo == libc::SIGTERM {
            // An event worker stops asynchronously; a task worker just shuts down.
            if swoole_rs_corebase::reactor::event_api::swoole_event_is_available() {
                stop_async_worker(serv, worker);
            } else {
                swr_v1_worker_shutdown(worker);
            }
        } else if (signo == libc::SIGWINCH || super::sigrtmin() == Some(signo))
            && let Some(logger) = swoole_rs_corebase::utilities::process_logger()
        {
            logger.reopen();
        }
    });
}

/// `Server::drain_worker_pipe()`: each worker's pipes, when this thread has a reactor.
pub fn drain_worker_pipe(serv: view::Server<'_>) {
    if swoole_rs_corebase::thread_reactor().is_null() {
        return;
    }
    for id in 0..all_core_workers(serv.config()) {
        let Some(worker) = other(serv, id) else {
            continue;
        };
        for master in [false, true] {
            if let Some(pipe) = worker_pipe(worker, master) {
                Reactor::thread().drain(pipe);
            }
        }
    }
}

/// `Server::clean_worker_connections(worker)`: close the connections the worker's
/// reactor holds (all of them in base mode).
pub fn clean_worker_connections(serv: view::Server<'_>, worker: Worker<'_>) {
    let reactor = Reactor::thread();
    reactor.handle().set_destroyed(true);
    let mode = serv.config().mode_;
    if mode != Mode::Base && mode != Mode::Thread {
        return;
    }
    crate::reactor_thread::each_connection(serv.state(), |conn| {
        if mode == Mode::Base || u32::from(conn.reactor_id()) == worker.id() {
            crate::conn_io::close_connection(serv, reactor, conn.socket());
        }
    });
}

/// `worker->coroutine_num = Coroutine::count()`, after each round of the loop, of the
/// worker `start_event_worker` runs: the process's current one (`enter_child`).
fn count_coroutines() {
    let count = swoole_rs_corebase::coroutine::coroutine_count();
    _ = with_current(|worker| worker.write_coroutine_num(count));
}

/// `Server::start_event_worker(worker)`: the process-mode worker's main loop; whether
/// it ran and ended without an error.
pub(crate) fn start_event_worker(serv: view::Server<'_>, worker: Worker<'_>) -> bool {
    let config = serv.config();
    swoole_set_worker_id(worker.id());
    set_worker_type(WorkerType::Event);
    crate::worker_methods::init_event_worker(serv, worker);
    if swoole_rs_corebase::reactor::swoole_event_init(0).is_err() {
        return false;
    }
    worker_signal_init(serv);
    for id in 0..all_core_workers(config) {
        let Some(worker) = other(serv, id) else {
            continue;
        };
        for master in [true, false] {
            if let Some(pipe) = worker_pipe(worker, master) {
                pipe.edit(|pipe| pipe.limit_output(u32::MAX));
            }
        }
    }
    let reactor = Reactor::thread();
    if let Some(pipe) = worker_pipe(worker, false) {
        set_nonblock(*pipe);
        reactor.hold_server(serv);
        reactor.add(pipe, Events::READ);
        reactor.set_handlers(FD_PIPE, Some(on_pipe_receive), None, ConnectionDefault);
    }
    if matches!(
        config.dispatch_mode.get(),
        DispatchMode::CO_CONN_LB | DispatchMode::CO_REQ_LB
    ) {
        let count = count_coroutines;
        swoole_rs_corebase::reactor::swr_v1_event_set_end_callback(
            swoole_rs_corebase::reactor::Priority::WorkerCallback,
            count,
        );
    }
    worker_start_callback(serv, worker);
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return false;
    }
    let rv = reactor.wait();
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return false;
    }
    drain_worker_pipe(serv);
    swoole_rs_corebase::reactor::swoole_event_free();
    call_worker_stop_callback(serv, worker);
    rv
}

/// `Server::send_to_reactor_thread(head, sizeof(*head), head->fd)`: the event `head`
/// to its session's reactor pipe, through the event loop when there is one: the bytes
/// written, or `None` when it failed.
pub(crate) fn send_to_reactor_thread(serv: view::Server<'_>, head: &DataHead) -> Option<usize> {
    let (tables, config) = (serv.state().tables_ref(), serv.config());
    // `serv->get_reactor_pipe_socket(session_id, info->reactor_id)`.
    let pipes = c_long::from(tables.reactor_pipe_num.get());
    let id = c_long::from(head.reactor_id)
        + head.fd.checked_rem(pipes).unwrap_or(0) * c_long::from(config.reactor_num.get());
    let pipe = serv
        .worker_record(u16::try_from(id).unwrap_or(u16::MAX))
        .and_then(|worker| worker_pipe(worker, false));
    let pipe = pipe?;
    let info = process::process_pool::head_view(head);
    if swoole_rs_corebase::reactor::event_api::swoole_event_is_available() {
        swoole_rs_corebase::reactor::event_api::event_write(pipe, &info)
    } else {
        let event = swoole_rs_corebase::reactor::Drain::new(&info);
        swoole_rs_corebase::tls::upstream::send_sync_drain(*pipe, event, 0)
    }
}

/// `Worker_set_cpu_affinity`.
#[cfg(target_os = "linux")]
fn set_cpu_affinity(config: &crate::ServerConfig, worker_id: u32) {
    use process::affinity::AffinityError;
    const ORIGIN: &core::ffi::CStr = c"Worker_set_cpu_affinity";
    let available = config.cpu_affinity_available.get();
    let available = available.as_deref().map(Vec::as_slice);
    let outcome =
        process::affinity::worker_affinity(worker_id, available, config.mode_ == Mode::Thread);
    match outcome {
        Err(AffinityError::Get(error)) => log::sys_warning_errno(
            error,
            ORIGIN,
            format_args!("get CPU affinity for worker#{worker_id} failed"),
        ),
        Err(AffinityError::NoCpu) => log::warning(
            ORIGIN,
            format_args!("no available CPU for worker#{worker_id}"),
        ),
        Err(AffinityError::Set(error)) => log::sys_warning_errno(
            error,
            ORIGIN,
            format_args!("set CPU affinity for worker#{worker_id} failed"),
        ),
        Ok(()) => {}
    }
}
