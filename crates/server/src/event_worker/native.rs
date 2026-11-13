//! The event worker's native side: the events it receives from the message bus, its
//! start and stop callbacks, its signals, the async stop and its exit wait, and the
//! process-mode main loop.
use crate::{DispatchMode, Mode};
use core::cell::Cell;
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::ServerEvent;

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_process::methods::{
    ExitStatus, push_stop, set_worker_type, swoole_get_worker_id, swoole_get_worker_pid,
    swoole_set_worker_id, worker_type,
};
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_globals::{
    current_worker, with_current, with_globals, worker_global_detach, worker_global_stopped,
};
use swoole_rs_process::worker_record::{
    Worker, WorkerRecord, WorkerStatus, WorkerType, swr_v1_worker_shutdown,
};
use swoole_rs_reactor::{Event, Running, Socket};

use super::{HANDLER_IGNORE, Wait};
use crate::callbacks::{
    CallbackKind, CallbackSlot, EventSlot, ReceiveSlot, ServerSlot, TaskSlot, WorkerSlot,
};
use crate::objects::{Bus, EVENT_READ, Reactor, set_nonblock, worker_pipe};
use crate::records::Lookup;
use crate::view;

/// `Reactor::PRIORITY_TRY_EXIT`, `PRIORITY_WORKER_CALLBACK`; `SW_FD_PIPE`, `SW_PIPE_MASTER`.
const PRIORITY_TRY_EXIT: c_int = 4;
const PRIORITY_WORKER_CALLBACK: c_int = 6;
const FD_PIPE: c_int = 3;
const PIPE_MASTER: c_int = 1;
/// `swGlobalHookType`'s worker and shutdown hooks.
const HOOK_BEFORE_WORKER_START: c_int = 2;
const HOOK_BEFORE_SERVER_SHUTDOWN: c_int = 6;
const HOOK_BEFORE_WORKER_STOP: c_int = 8;
/// `SW_LOG_WARNING` and the `SW_ERROR_*` the worker logs.
const WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;
const INVALID_PARAMS: c_int = 505;
const WRONG_OPERATION: c_int = 509;
const SESSION_DISCARD_TIMEOUT_DATA: c_int = 1007;
const SERVER_WORKER_EXIT_TIMEOUT: c_int = 9101;
const SERVER_WORKER_UNPROCESSED_DATA: c_int = 9103;

/// `serv->get_worker_message_bus()`.
fn bus(serv: view::Server<'_>) -> Bus {
    crate::master_ops::worker_message_bus(serv.state())
}

/// The event the bus just read, as its sender packed it: an `EventData`, its head and
/// the payload the head counts.
fn event_data(serv: view::Server<'_>) -> Packed<'_> {
    // SAFETY: called once the bus has read a whole event into its buffer, which only
    // this thread's next read replaces, after the callback it is lent to returns.
    unsafe { Packed::from_raw(bus(serv).buffer().cast()) }
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
fn unread(reactor: Reactor, pipe: Option<Socket<'_>>) {
    if let Some(sock) = pipe.filter(|s| s.read(|s| !s.removed)) {
        reactor.unread(sock);
    }
}

/// [`unread`] of `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the
/// thread bus's own end of `pipe`.
fn unread_in_bus(serv: view::Server<'_>, reactor: Reactor, pipe: Option<Socket<'_>>) {
    let unread = |pipe: Socket<'_>| unread(reactor, Some(pipe));
    crate::objects::in_worker_bus(serv.state(), serv.config(), pipe, unread);
}

/// Whether a connection's socket is still read: its peer hasn't closed and it is in
/// the loop.
fn readable(conn: crate::Connection<'_>) -> bool {
    let removed = conn.socket().is_none_or(|s| s.read(|s| s.removed));
    !conn.peer_closed() && !removed
}

/// `Server::worker_accept_event(info)`: the event whose head is `info` (its data, if
/// any, in the worker message bus), in this thread's worker.
pub(crate) fn accept_event(serv: view::Server<'_>, info: &DataHead) {
    _ = with_current(|&worker| accept(serv, worker, info));
}

fn accept(serv: view::Server<'_>, worker: Worker<'_>, info: &DataHead) {
    let (state, config) = (serv.state(), serv.config());
    let callbacks = &state.callbacks;
    worker.set_status(WorkerStatus::Busy);
    match info.event() {
        Some(ServerEvent::RecvData) => {
            let closed = received(serv, info);
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
                run_task(serv, worker, info, ReceiveSlot::Receive);
            }
        }
        Some(ServerEvent::RecvDgram) => run_task(serv, worker, info, ReceiveSlot::Packet),
        Some(ServerEvent::Close) => {
            free_client_cert(serv, info);
            _ = crate::factory::end(state, info.fd, 0);
        }
        Some(ServerEvent::Connect) => {
            if info.len > 0 {
                store_client_cert(serv, info);
            }
            callbacks.event(EventSlot::Connect, serv, info);
        }
        Some(ServerEvent::BufferFull) => callbacks.event(EventSlot::BufferFull, serv, info),
        Some(ServerEvent::BufferEmpty) => callbacks.event(EventSlot::BufferEmpty, serv, info),
        Some(ServerEvent::Finish) => _ = callbacks.task(TaskSlot::Finish, serv, event_data(serv)),
        Some(ServerEvent::PipeMessage) => callbacks.pipe_message(serv, event_data(serv)),
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

/// The connection received data is for, its queued bytes and dispatch time updated:
/// `None` without one, else whether the server closed it.
fn received(serv: view::Server<'_>, info: &DataHead) -> Option<bool> {
    let conn = serv.state().connection(info.fd, Lookup::VerifiedTls)?;
    if info.len > 0 {
        let length = u32::try_from(bus(serv).packet().length).unwrap_or(u32::MAX);
        conn.recv_queued_bytes().fetch_sub(length, Relaxed);
    }
    conn.set_last_dispatch_time(info.time);
    Some(conn.closed().load(Relaxed) != 0)
}

/// The receive or packet callback over the bus's packet; a request counted when it
/// succeeds.
fn run_task(serv: view::Server<'_>, worker: Worker<'_>, info: &DataHead, slot: ReceiveSlot) {
    let packet = bus(serv).packet();
    let mut recv = crate::reactor_entry::RecvData {
        info: *info,
        data: packet.data.cast(),
    };
    recv.info.len = u32::try_from(packet.length).unwrap_or(u32::MAX);
    if serv.state().callbacks.receive(slot, serv, &recv) == 0 {
        worker.add_request_count();
        serv.counters().request_count.fetch_add(1, Relaxed);
    }
}

/// The session's client certificate, sent with its connect event.
fn store_client_cert(serv: view::Server<'_>, info: &DataHead) {
    if let Some(conn) = serv.state().connection(info.fd, Lookup::Verified) {
        conn.set_ssl_client_cert(bus(serv).with_packet(client_cert).cast());
        conn.set_ssl_client_cert_pid(swoole_get_worker_pid());
    }
}

/// Frees the session's client certificate, if this worker stored it.
fn free_client_cert(serv: view::Server<'_>, info: &DataHead) {
    if let Some(conn) = serv.state().connection(info.fd, Lookup::Verified)
        && !conn.ssl_client_cert().is_null()
        && conn.ssl_client_cert_pid() == swoole_get_worker_pid()
    {
        // SAFETY: this worker's certificate, which `client_cert` made.
        let mut cert = unsafe { Box::from_raw(conn.ssl_client_cert().cast::<CppString>()) };
        cert.dispose();
        conn.set_ssl_client_cert(core::ptr::null_mut());
    }
}

/// `new String(data, length)`: the connection's client certificate.
fn client_cert(data: &[u8]) -> *mut CppString {
    let mut cert = Box::new(CppString::EMPTY);
    cert.alloc(data.len(), None);
    cert.append(data);
    Box::into_raw(cert)
}

/// `Worker_global_hook`: the worker start/stop and server shutdown hooks.
fn global_hook(serv: view::Server<'_>, worker: Worker<'_>, hook: c_int) {
    if !swoole_rs_corebase::swoole_isset_hook(hook) {
        return;
    }
    if hook == HOOK_BEFORE_SERVER_SHUTDOWN {
        serv.call_hook(view::ServerHook::BeforeShutdown);
        return;
    }
    let id = worker.id() as usize;
    let mut args: [*mut c_void; 2] = [serv.raw(), core::ptr::without_provenance_mut(id)];
    // SAFETY: the worker hooks take { server, worker id }.
    unsafe { swoole_rs_corebase::swoole_call_hook(hook, args.as_mut_ptr().cast()) };
}

/// `Server::worker_start_callback(worker)`.
pub(crate) fn worker_start_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    let config = serv.config();
    if swoole_rs_ossupport::swr_v1_os_is_root_user() {
        serv.state().set_isolation();
    }
    let me = worker.id();
    for id in (0..all_core_workers(config)).filter(|&id| id != me) {
        if worker_type() == Some(WorkerType::Event)
            && let Some(pipe) = other(serv, id).and_then(|w| worker_pipe(w, true))
        {
            set_nonblock(pipe);
        }
    }
    worker.set_status(WorkerStatus::Idle);
    if config.mode_ == Mode::Process {
        serv.state()
            .tables_ref()
            .session_list
            .protect(libc::PROT_READ);
    }
    call_worker_start_callback(serv, worker);
}

/// `Server::call_worker_start_callback(worker)`.
pub(crate) fn call_worker_start_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    #[cfg(target_os = "linux")]
    if serv.config().open_cpu_affinity.get() {
        set_cpu_affinity(serv.config(), worker.id());
    }
    global_hook(serv, worker, HOOK_BEFORE_WORKER_START);
    log::set_last_error(0);
    swoole_rs_error::swoole_clear_last_error_msg();
    let callbacks = &serv.state().callbacks;
    callbacks.worker(WorkerSlot::Start, serv, worker);
}

/// `Server::call_worker_stop_callback(worker)`.
pub(crate) fn call_worker_stop_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    global_hook(serv, worker, HOOK_BEFORE_WORKER_STOP);
    serv.state()
        .callbacks
        .worker(WorkerSlot::Stop, serv, worker);
    let bus = bus(serv);
    if !bus.is_empty() {
        log::error_log(
            WARNING,
            SERVER_WORKER_UNPROCESSED_DATA,
            c"Server::call_worker_stop_callback",
            format_args!("unprocessed data in the worker process buffer"),
        );
        bus.clear();
    }
    worker_global_stopped();
}

/// `Server::call_worker_error_callback(worker, status)`.
pub(crate) fn call_worker_error_callback(
    serv: view::Server<'_>,
    worker: Worker<'_>,
    status: &ExitStatus,
) {
    serv.state().callbacks.worker_error(serv, worker, status);
    if worker.kind() == WorkerType::Event.code() {
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
    let data = (&raw const head).cast();
    // SAFETY: one of the live server's workers, and the event.
    let sent = unsafe {
        swoole_rs_process::methods::send_pipe_message(
            worker.raw(),
            data,
            size_of::<DataHead>(),
            PIPE_MASTER,
        )
    };
    sent != -1
}

/// `Server::stop_async_worker(worker)`: stop reading, leave the pool and wait for the
/// reactor's remaining work before exiting.
pub fn stop_async_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    let config = serv.config();
    swr_v1_worker_shutdown(worker);
    if worker.kind() == WorkerType::Event.code() {
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
            if worker.id() == 0 && !pool.running {
                global_hook(serv, worker, HOOK_BEFORE_SERVER_SHUTDOWN);
                serv.state()
                    .callbacks
                    .call_server(ServerSlot::BeforeShutdown, serv);
            }
            unread(reactor, worker_pipe(worker, true));
            for port in serv.ports() {
                _ = port.with_socket(|sock| reactor.del(sock));
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
            if push_stop(serv.event_pool_ref(), pid, id) < 0 {
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
    swoole_rs_reactor::swr_v1_event_set_end_callback(PRIORITY_TRY_EXIT, try_to_exit);
    let now = swoole_rs_corebase::time_now();
    with_globals(|globals| globals.exit_time.store(now, Relaxed));
    try_exit(serv, reactor);
}

/// `Worker_reactor_try_to_exit`: the loop's end callback while the worker exits.
extern "C-unwind" fn try_to_exit() {
    let reactor = Reactor::thread();
    try_exit(server_of(reactor), reactor);
}

/// Once the reactor can exit, stop it; meanwhile call `onWorkerExit` once, then wait
/// at most `max_wait_time` since the stop.
fn try_exit(serv: view::Server<'_>, reactor: Reactor) {
    let callbacks = &serv.state().callbacks;
    let on_exit = callbacks
        .slot(CallbackKind::WorkerExit)
        .is_some_and(CallbackSlot::is_set);
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

/// The server of this thread's `reactor`, whose `ptr` is the server (or, in a task
/// worker, its pool, whose `ptr` is).
fn server_of(reactor: Reactor) -> view::Server<'static> {
    let mut serv = reactor.handle().object();
    if worker_type() == Some(WorkerType::Task) {
        // SAFETY: a task worker's reactor holds its live pool.
        serv = unsafe { swoole_rs_process::process_pool::PoolRef::new(serv.cast()) }.ptr();
    }
    // SAFETY: the worker's live server, per the above.
    unsafe { view::Server::from_raw(serv) }
}

/// `Worker_onPipeReceive`: a message from a reactor thread or the master (process mode).
extern "C" fn on_pipe_receive(reactor: Running<'_>, event: &mut Event) -> c_int {
    let serv = server_of(Reactor::running(reactor));
    let bus = bus(serv);
    if event.socket().is_some_and(|socket| bus.read(socket) > 0) {
        // SAFETY: the head of the whole event the bus just read, which only this
        // thread's next read replaces.
        accept_event(serv, unsafe { &*bus.buffer() });
        bus.pop();
    }
    0
}

/// `Server::worker_signal_init()`.
pub fn worker_signal_init(serv: view::Server<'_>) {
    if serv.config().mode_ == Mode::Thread {
        return;
    }
    for (signo, handler) in super::signals(HANDLER_IGNORE) {
        swr_workers_signal_set(signo, handler, worker_signal_handler);
    }
}

/// Installs `handler` (one of [`super::signals`]'s) for `signo`, `own` for the worker's
/// own; SIGRTMIN only where Linux names it.
pub(crate) fn swr_workers_signal_set(signo: c_int, handler: u64, own: extern "C" fn(c_int)) {
    use swoole_rs_reactor::signal_upstream::{self, Handler};
    #[cfg(target_os = "linux")]
    let signo = if signo == super::SIGNAL_RTMIN {
        libc::SIGRTMIN()
    } else {
        signo
    };
    #[cfg(not(target_os = "linux"))]
    if signo == super::SIGNAL_RTMIN {
        return;
    }
    let handler = match handler {
        super::HANDLER_NONE => Handler::NONE,
        HANDLER_IGNORE => Handler::IGNORE,
        _ => Handler::new(own),
    };
    _ = signal_upstream::set(signo, handler, false);
}

/// Whether `signo` is SIGRTMIN, which only Linux has.
fn is_sigrtmin(signo: c_int) -> bool {
    #[cfg(target_os = "linux")]
    return signo == libc::SIGRTMIN();
    #[cfg(not(target_os = "linux"))]
    {
        _ = signo;
        false
    }
}

/// `Server::worker_signal_handler(signo)`, over `SwooleG.server`.
pub(crate) extern "C" fn worker_signal_handler(signo: c_int) {
    // SAFETY: a worker's signal runs while its server lives; the view ends with it.
    let Some(serv) = (unsafe { view::Server::current() }) else {
        return;
    };
    let live = swoole_rs_corebase::swr_v1_core_running() && serv.state().running();
    _ = with_current(|&worker| {
        if !live {
            return;
        }
        if signo == libc::SIGTERM {
            // An event worker stops asynchronously; a task worker just shuts down.
            if swoole_rs_reactor::event_api::swoole_event_is_available() {
                stop_async_worker(serv, worker);
            } else {
                swr_v1_worker_shutdown(worker);
            }
        } else if (signo == libc::SIGWINCH || is_sigrtmin(signo))
            && let Some(logger) = swoole_rs_utilities::process_logger()
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

thread_local! {
    /// The worker whose coroutines `count_coroutines` counts: this thread's event worker.
    static COUNTED: Cell<*mut WorkerRecord> = const { Cell::new(core::ptr::null_mut()) };
}

/// `worker->coroutine_num = Coroutine::count()`, after each round of the loop.
extern "C-unwind" fn count_coroutines() {
    // SAFETY: the worker `start_event_worker` named, live while its loop runs.
    if let Some(worker) = unsafe { COUNTED.get().as_mut() } {
        worker.coroutine_num = swoole_rs_coroutine::swoole_coroutine_count();
    }
}

/// `Server::start_event_worker(worker)`: the process-mode worker's main loop.
pub(crate) fn start_event_worker(serv: view::Server<'_>, worker: Worker<'_>) -> c_int {
    let config = serv.config();
    swoole_set_worker_id(worker.id());
    set_worker_type(WorkerType::Event);
    crate::worker_methods::init_event_worker(serv, worker);
    if swoole_rs_reactor::swoole_event_init(0) < 0 {
        return -1;
    }
    worker_signal_init(serv);
    for id in 0..all_core_workers(config) {
        let Some(worker) = other(serv, id) else {
            continue;
        };
        for master in [true, false] {
            if let Some(pipe) = worker_pipe(worker, master) {
                pipe.edit(|pipe| pipe.output.buffer_size = u32::MAX);
            }
        }
    }
    let reactor = Reactor::thread();
    if let Some(pipe) = worker_pipe(worker, false) {
        set_nonblock(pipe);
        reactor.handle().set_object(serv.raw());
        reactor.add(pipe, EVENT_READ);
        reactor.set_handler(FD_PIPE, EVENT_READ, on_pipe_receive);
    }
    if matches!(
        config.dispatch(),
        Some(DispatchMode::CoConnLb | DispatchMode::CoReqLb)
    ) {
        COUNTED.set(worker.raw());
        let count = count_coroutines;
        swoole_rs_reactor::swr_v1_event_set_end_callback(PRIORITY_WORKER_CALLBACK, count);
    }
    worker_start_callback(serv, worker);
    let rv = reactor.wait();
    drain_worker_pipe(serv);
    swoole_rs_reactor::swoole_event_free();
    call_worker_stop_callback(serv, worker);
    rv
}

/// `Server::send_to_reactor_thread(head, sizeof(*head), head->fd)`: the event `head`
/// to its session's reactor pipe, through the event loop when there is one.
pub(crate) fn send_to_reactor_thread(serv: view::Server<'_>, head: &DataHead) -> isize {
    let (tables, config) = (serv.state().tables_ref(), serv.config());
    // `serv->get_reactor_pipe_socket(session_id, info->reactor_id)`.
    let pipes = c_long::from(tables.reactor_pipe_num);
    let id = c_long::from(head.reactor_id)
        + head.fd.checked_rem(pipes).unwrap_or(0) * c_long::from(config.reactor_num.get());
    let pipe = serv
        .worker_record(u16::try_from(id).unwrap_or(u16::MAX))
        .and_then(|worker| worker_pipe(worker, false));
    let Some(pipe) = pipe else {
        return -1;
    };
    let (info, size) = (core::ptr::from_ref(head).cast(), size_of::<DataHead>());
    if swoole_rs_reactor::event_api::swoole_event_is_available() {
        let write = swoole_rs_reactor::event_api::swoole_event_write;
        // SAFETY: the live pipe, and the event of `size` bytes it carries.
        unsafe { write(pipe.as_ptr().cast(), info, size) }
    } else {
        // SAFETY: as above.
        let event = unsafe { swoole_rs_reactor::Drain::from_raw(info, size) };
        swoole_rs_tls::upstream::send_sync_drain(pipe, event, 0)
    }
}

/// `Worker_set_cpu_affinity`.
#[cfg(target_os = "linux")]
fn set_cpu_affinity(config: &crate::ServerConfig, worker_id: u32) {
    use swoole_rs_process::affinity::AffinityError;
    const ORIGIN: &core::ffi::CStr = c"Worker_set_cpu_affinity";
    let available = config.cpu_affinity_available.get();
    let available = available.as_deref().map(Vec::as_slice);
    let outcome = swoole_rs_process::affinity::worker_affinity(
        worker_id,
        available,
        config.mode_ == Mode::Thread,
    );
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
