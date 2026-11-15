//! The reactor-thread ops over a thread's own state that were master.cc's: its
//! entry and setup, its message bus and worker pipes, the messages workers send
//! it, its timers and the server methods it calls. Each is a Rust call: the bus
//! is crates/bus', the methods are this crate's and crates/process' and
//! crates/ports', reached under their upstream symbols.
use crate::Mode;
use crate::reactor_thread::{HEARTBEAT, each_connection, send_event};
use crate::records::{Connection, Lookup};
use core::ffi::{c_int, c_long};
use core::ptr;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_reactor::Events;

use crate::objects::worker_pipe;
use crate::reactor_handle::Reactor;
use crate::reactor_io::owns;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_reactor::{Event, Running, Socket};

use crate::callbacks::TaskSlot;
use crate::event_worker::native::{
    call_worker_start_callback, call_worker_stop_callback, stop_async_worker,
};
use crate::reactor_entry as entry;
use crate::reactor_ops::ReactorContext;
use crate::{ServerState, factory, server_methods as methods, view};

/// `SW_EVENT_READ`, `Server::THREAD_REACTOR`, `SW_EVENT_WRITE` and `SW_FD_PIPE`.
const THREAD_REACTOR: u8 = 2;
const FD_PIPE: c_int = 3;
/// `Server::CLOSE_ACTIVELY`.
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// `thread->message_bus.get_pipe_socket(sock)`.
fn pipe_socket<'b>(
    bus: &'b crate::objects::Bus<'_>,
    pipe: Option<Socket<'_>>,
) -> Option<Socket<'b>> {
    bus.pipe_socket(pipe?)
}

/// The server's worker `id`, if it has one.
fn worker(serv: view::Server<'_>, id: impl TryInto<u16>) -> Option<Worker<'_>> {
    serv.worker_record(id.try_into().unwrap_or(u16::MAX))
}

/// `Server::heartbeat_check`'s timer, which `reactor` arms for itself and runs on
/// its thread.
fn heartbeat_timer(state: &ServerState, reactor: Reactor) -> Option<swoole_rs_timer::Held> {
    let msec = c_long::from(state.config_ref().heartbeat_check_interval.get()) * 1000;
    swoole_rs_timer::add_fn(msec, true, move || {
        // SAFETY: a server reactor whose `ptr` is its live server, which outlives its
        // timers.
        let serv = unsafe { entry::server(reactor) };
        crate::reactor_thread::heartbeat_check(serv, reactor);
    })
}

/// `ReactorThread::init`: thread `id`'s reactor handlers, its UDP ports, worker
/// pipes and message bus; `None` when the context lacks its reactor or thread.
fn init_thread(ctx: &mut ReactorContext<'_>, state: &ServerState, id: u16) -> Option<c_int> {
    let (server, handle) = (ctx.serv, ctx.reactor?);
    let (config, tables) = (state.config_ref(), state.tables_ref());
    let reactor = handle.handle();
    reactor.set_object(server.raw());
    reactor.set_id(id);
    reactor.set_wait_exit(false);
    reactor.set_max_socket(tables.max_connection);
    reactor.set_close(Some(entry::close_connection));
    handle.set_error_handler(entry::on_close);
    handle.set_handler(FD_PIPE, Events::READ, on_pipe_read);
    handle.set_handler(FD_PIPE, Events::WRITE, entry::on_pipe_write);
    let reactors = config.reactor_num.get();
    // Listen on this thread's UDP ports.
    if config.have_dgram_sock.get() {
        for (index, port) in tables.ports().iter().enumerate() {
            let dgram = matches!(port.config().r#type, 2 | 4 | 6);
            if !dgram || !owns(i64::from(port.fd()), reactors, id) {
                continue;
            }
            let socket = server.port_socket_at(index)?;
            let fd = socket.fd();
            // The connection table's slot for the listening fd.
            let serv_sock = state.connection(c_long::from(fd), Lookup::ByFd)?;
            serv_sock.set_fd(fd);
            serv_sock.set_socket_type(port.config().r#type);
            serv_sock.set_object(port.raw());
            serv_sock.listen(*port);
            // The reactor thread that reads the datagram port.
            port.config()
                .thread_id
                .set(crate::reactor_ops::pthread_self());
            if !handle.change(socket, swoole_rs_reactor::ChangeOp::Add, Events::READ) {
                return Some(-1);
            }
        }
    }
    crate::reactor_thread::init_reactor(server, handle);
    let thread = ctx.thread_record()?;
    let bus = thread.message_bus();
    methods::init_pipe_sockets(server, bus);
    if config.mode_ == Mode::Thread {
        let worker = worker(server, id)?;
        crate::worker_methods::init_event_worker(server, worker);
        if let Some(sock) = pipe_socket(&bus, worker_pipe(worker, false)) {
            let _ = handle.add(sock, Events::READ);
        }
        if config.heartbeat_check_interval.get() > 0 {
            thread.heartbeat_timer.set(heartbeat_timer(state, handle));
        }
    }
    if let Some(pipe) = config.command_pipe() {
        // The server's live command pipe.
        let fd = pipe.worker_end()?.fd();
        bus.init_pipe_socket(fd);
        thread.pipe_command.set(bus.pipe_fd(fd).cast());
        bus.rust()
            .pipe(fd)?
            .edit(|s| s.output.buffer_size = u32::MAX);
    }
    bus.rust().count_ids(state.msg_ids.get()?);
    bus.set_always_chunked();
    if !bus.alloc_buffer(config.ipc_max_size() as usize) {
        return Some(-1);
    }
    for w in (0..config.worker_num.get()).filter(|&w| owns(i64::from(w), reactors, id)) {
        let Some(worker) = worker(server, w) else {
            return Some(-1);
        };
        let sock = pipe_socket(&bus, worker_pipe(worker, true));
        if sock.is_none_or(|sock| !handle.add(sock, Events::READ)) {
            return Some(-1);
        }
        // Only synchronous sends go to the notify pipe: thread-safe, no separate memory.
        if thread.notify_pipe.get().is_null() {
            thread.notify_pipe.set(worker.pipe_worker().cast());
        }
    }
    Some(0)
}

/// The destroy callback of a reactor thread's reactor: close its connections.
struct CloseConnections(Reactor);

impl swoole_rs_reactor::event_api::Deferred for CloseConnections {
    fn run(self) {
        // SAFETY: a reactor thread's reactor, which runs its destroy callbacks on its
        // own thread while it lives, and whose `ptr` is its live server.
        let serv = unsafe { entry::server(self.0) };
        crate::reactor_thread::close_reactor_connections(serv, self.0);
    }

    /// Queued after the destroy pass, it closes nothing, as the native callback did.
    fn dispose(self) {}
}

/// What a reactor thread Rust starts runs: `ServerState::start_thread` calls it with
/// the live server, on its new reactor thread.
fn reactor_thread_entry(server: view::Server<'_>, id: u64) {
    swoole_rs_corebase::swoole_thread_init(false);
    let id = c_int::try_from(id).unwrap_or_default();
    main_loop(server, u16::try_from(id).unwrap_or_default());
    swoole_rs_corebase::swoole_thread_clean(false);
}

/// What the heartbeat thread Rust starts runs, its signals blocked. Called with the
/// live server, as [`reactor_thread_entry`] is.
fn heartbeat_main(server: view::Server<'_>, _: u64) {
    // Else a process-directed signal (the shutdown's SIGTERM) can land here rather
    // than on the master's signalfd, and its default action kills the master.
    swoole_rs_corebase::signal_block_all();
    let reactors = server.state().config_ref().reactor_num.get();
    // upstream sets THREAD_HEARTBEAT, then overwrites the type with this.
    let kind = (reactors.wrapping_add(1) & 0xff) as u8;
    swoole_rs_corebase::set_thread(kind, swoole_rs_corebase::thread_id());
    heartbeat_thread(server);
}

/// `Server::start_reactor_threads`, in the master: listen, start the reactor
/// threads (or set up the single one) and the heartbeat, then run the master thread.
pub(crate) fn start_reactor_threads(serv: view::Server<'_>) -> bool {
    let state = serv.state();
    let config = state.config_ref();
    if swoole_rs_reactor::swoole_event_init(0) < 0 {
        return false;
    }
    let reactor = Reactor::thread();
    for (index, port) in state.tables_ref().ports().iter().enumerate() {
        if matches!(port.config().r#type, 2 | 4 | 6) {
            continue;
        }
        if !(crate::port_config::methods().listen)(port.config()) {
            swoole_rs_reactor::swoole_event_free();
            return false;
        }
        if let Some(socket) = serv.port_socket_at(index) {
            reactor.change(socket, swoole_rs_reactor::ChangeOp::Add, Events::READ);
        }
    }
    methods::store_listen_socket(serv);
    let single = config.single_thread.get();
    if single {
        // Thread 0's setup over the master's reactor; it runs the one loop.
        let mut init = ReactorContext::new(serv, Some(reactor));
        init.thread = state.tables_ref().reactor_thread(0);
        if let Some(record) = init.thread_record() {
            let id = u16::try_from(record.id.get()).unwrap_or_default();
            let _ = init_thread(&mut init, state, id);
        }
    } else {
        // a special id for the master thread
        let id = config.reactor_num.get();
        reactor.handle().set_id(id);
        swoole_rs_corebase::set_thread(swoole_rs_corebase::thread_kind(), id);
        for id in 0..config.reactor_num.get() {
            ServerState::start_thread(serv, reactor_thread_entry, u64::from(id));
        }
    }
    if config.heartbeat_check_interval.get() >= 1 {
        if single {
            let timer = heartbeat_timer(state, reactor);
            config.heartbeat_timer.store(timer);
        } else {
            ServerState::start_thread(serv, heartbeat_main, HEARTBEAT);
        }
    }
    methods::start_master_thread(serv, reactor)
}

/// `Server::reactor_thread_main_loop`: reactor thread `id`'s setup, its event loop
/// and its cleanup.
pub(crate) fn main_loop(serv: view::Server<'_>, id: u16) {
    let state = serv.state();
    let mut ctx = ReactorContext::new(serv, None);
    ctx.thread = state.tables_ref().reactor_thread(usize::from(id));
    let Some(record) = ctx.thread_record() else {
        return;
    };
    record.id.set(c_int::from(id));
    swoole_rs_corebase::set_thread_message_bus(ptr::from_ref(&record.bus).cast_mut().cast());
    if swoole_rs_reactor::swoole_event_init(0) < 0 {
        return;
    }
    ctx.reactor = Some(Reactor::thread());
    let mode = state.config_ref().mode_;
    let worker = || worker(serv, id);
    if mode == Mode::Thread {
        let Some(worker) = worker() else {
            return;
        };
        call_worker_start_callback(serv, worker);
    } else {
        swoole_rs_corebase::set_thread(THREAD_REACTOR, id);
    }
    if init_thread(&mut ctx, state, id).unwrap_or(-1) < 0 {
        return;
    }
    // Every reactor thread is ready before the master runs.
    if mode == Mode::Process {
        methods::reactor_barrier(serv);
    }
    #[expect(
        clippy::let_underscore_must_use,
        reason = "upstream's reactor thread ignores its loop's result: it cleans up either way"
    )]
    let _ = swoole_rs_reactor::event_api::swoole_event_wait();
    if mode == Mode::Thread
        && let Some(worker) = worker()
    {
        call_worker_stop_callback(serv, worker);
    }
    record.message_bus().free_buffer();
}

/// The heartbeat thread: every interval, ask reactor threads to force-close
/// connections idle past their port's heartbeat time.
fn heartbeat_thread(serv: view::Server<'_>) {
    let state = serv.state();
    while state.running() {
        let now = swoole_rs_corebase::microtime();
        each_connection(state, |conn| {
            let session = conn.session_id();
            if session > 0 && !serv.healthy(now, conn) {
                send_close_force(serv, conn, session);
            }
        });
        let interval = state.config_ref().heartbeat_check_interval.get();
        std::thread::sleep(core::time::Duration::from_secs(u64::from(interval)));
    }
}

/// `serv->get_reactor_pipe_socket(session, conn->reactor_id)` told to close
/// `session`: the reactor verifies the connection before closing it.
fn send_close_force(serv: view::Server<'_>, conn: Connection<'_>, session: c_long) {
    let state = serv.state();
    let pipes = i64::from(state.tables_ref().reactor_pipe_num);
    let index = session.checked_rem(pipes).unwrap_or_default();
    let reactors = i64::from(state.config_ref().reactor_num.get());
    let id = i64::from(conn.reactor_id()) + index * reactors;
    if let Some(worker) = worker(serv, id) {
        send_event(worker_pipe(worker, false), session, ServerEvent::CloseForce);
    }
}

/// `ReactorThread_onPipeRead`: a worker's message.
extern "C" fn on_pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let reactor = Reactor::running(reactor);
    // SAFETY: the handler is registered only on server reactors.
    let serv = unsafe { entry::server(reactor) };
    let mut ctx = ReactorContext::new(serv, Some(reactor));
    ctx.thread = serv
        .state()
        .tables_ref()
        .reactor_thread(usize::from(reactor.handle().id()));
    pipe_read(&mut ctx, event).unwrap_or(-1)
}

/// `ReactorThread_onPipeRead`: the messages workers send their reactor thread,
/// until the pipe is drained.
fn pipe_read(ctx: &mut ReactorContext<'_>, event: &Event) -> Option<c_int> {
    let (serv, reactor) = (ctx.serv, ctx.reactor?);
    let state = serv.state();
    let thread = ctx.thread_record()?;
    let bus = thread.message_bus();
    let sock = event.socket()?;
    loop {
        if bus.read_dgram(sock) <= 0 {
            return Some(0);
        }
        let head = bus.head();
        let session = head.fd;
        match head.event() {
            Some(ServerEvent::Incoming) => {
                if let Some(conn) = state.connection(session, Lookup::Verified)
                    && !methods::connection_incoming(serv, reactor, conn)
                    && let Some(sock) = conn.socket()
                {
                    reactor.close(sock);
                }
            }
            Some(ServerEvent::CommandRequest) => {
                let id = u16::try_from(thread.id.get()).unwrap_or_default();
                // The bus's record of the command pipe (`thread.pipe_command`).
                let end = state
                    .config_ref()
                    .command_pipe()
                    .and_then(swoole_rs_tls::socket_pair::SocketPair::worker_end);
                let pipe = end.and_then(|end| bus.pipe_socket(end));
                crate::commands::call_command_handler(serv, bus, id, pipe);
            }
            Some(ServerEvent::CommandResponse) => {
                bus.with_packet(|reply| crate::commands::command_result(state, session, reply));
            }
            Some(ServerEvent::Shutdown) => shutdown(ctx)?,
            Some(ServerEvent::ShutdownSignal) => {
                swoole_rs_process::os::kill(std::process::id().cast_signed(), libc::SIGTERM);
            }
            Some(ServerEvent::Finish | ServerEvent::PipeMessage) => {
                // SAFETY: the bus's buffer: its `EventData`, its head and the payload it
                // counts.
                let event =
                    unsafe { swoole_rs_process::task_file::Packed::from_raw(bus.buffer().cast()) };
                if head.event() == Some(ServerEvent::Finish) {
                    let _ = state.callbacks.task(TaskSlot::Finish, serv, event);
                } else {
                    state.callbacks.pipe_message(serv, event);
                }
            }
            Some(ServerEvent::CloseForce) => {
                crate::reactor_thread::close(serv, reactor, session);
            }
            Some(ServerEvent::CloseForward) => {
                factory::end(serv, session, CLOSE_ACTIVELY);
            }
            _ => {
                bus.with_packet(|data| {
                    let mut info = head;
                    info.len = u32::try_from(data.len()).unwrap_or(u32::MAX);
                    methods::send_event_to_connection(serv, &info, data);
                });
            }
        }
        bus.pop();
    }
}

/// `ReactorThread::shutdown`: stop reading this reactor's datagram ports, worker
/// pipes and connections, and let it exit once its writes drain.
pub(crate) fn shutdown(ctx: &ReactorContext<'_>) -> Option<()> {
    let (serv, reactor) = (ctx.serv, ctx.reactor?);
    let state = serv.state();
    let config = state.config_ref();
    let (reactors, id) = (config.reactor_num.get(), reactor.handle().id());
    if config.have_dgram_sock.get() {
        for (index, port) in state.tables_ref().ports().iter().enumerate() {
            let dgram = matches!(port.config().r#type, 2 | 4 | 6);
            if !dgram || !owns(i64::from(port.fd()), reactors, id) {
                continue;
            }
            if let Some(socket) = serv
                .port_socket_at(index)
                .filter(|s| !s.read(|s| s.removed))
            {
                reactor.del(socket);
            }
        }
    }
    let thread = ctx.thread_record()?;
    if let Some(timer) = thread.heartbeat_timer.take() {
        swoole_rs_timer::del_id(timer);
    }
    swoole_rs_reactor::defer_destroy_value(reactor.handle(), CloseConnections(reactor));
    if config.mode_ == Mode::Thread {
        stop_async_worker(serv, worker(serv, id)?);
        return Some(());
    }
    let bus = thread.message_bus();
    for w in (0..config.worker_num.get()).filter(|&w| owns(i64::from(w), reactors, id)) {
        if let Some(sock) = pipe_socket(&bus, worker_pipe(worker(serv, w)?, true)) {
            reactor.interest(sock, swoole_rs_reactor::Interest::RemoveRead);
        }
    }
    each_connection(state, |conn| {
        let Some(sock) = conn.socket() else {
            return;
        };
        let reading = !conn.peer_closed() && !sock.read(|s| s.removed);
        if reading && owns(i64::from(conn.fd()), reactors, id) {
            reactor.interest(sock, swoole_rs_reactor::Interest::RemoveRead);
        }
    });
    reactor.handle().set_wait_exit(true);
    Some(())
}
