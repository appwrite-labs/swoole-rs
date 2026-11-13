//! The reactor-thread ops over a thread's own state that were master.cc's: its
//! entry and setup, its message bus and worker pipes, the messages workers send
//! it, its timers and the server methods it calls. Each is a Rust call: the bus
//! is crates/bus', the methods are this crate's and crates/process' and
//! crates/ports', reached under their upstream symbols.
use crate::reactor_thread::{HEARTBEAT, each_connection, send_event};
use crate::records::{Connection, Lookup};
use core::ffi::{c_int, c_long, c_void};
use core::ptr;

use crate::objects::worker_pipe;
use crate::reactor_handle::Reactor;
use crate::reactor_io::owns;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};
use swoole_rs_process::worker_record::Worker;
use swoole_rs_reactor::{Event, Running, Socket};

use crate::base_factory::SendData;
use crate::callbacks::TaskSlot;
use crate::event_worker_ops::{
    call_worker_start_callback, call_worker_stop_callback, stop_async_worker,
};
use crate::ops::OP_FACTORY_DISPATCH;
use crate::ops::{OP_PACKET_INIT, OP_PACKET_TASK, OP_RECVFROM};
use crate::reactor_entry::{self as entry, thread};
use crate::reactor_ops::{ReactorContext, ThreadRecord, sign};
use crate::{ServerState, factory, server_methods as methods, view};
use swoole_rs_buffer::string::with_thread_buffer;

/// `SW_EVENT_READ`, `Server::THREAD_REACTOR`, `SW_EVENT_WRITE` and `SW_FD_PIPE`.
const SW_EVENT_READ: c_int = 1 << 9;
const THREAD_REACTOR: u8 = 2;
const SW_EVENT_WRITE: c_int = 1 << 10;
const FD_PIPE: c_int = 3;
/// `SW_SERVER_EVENT_RECV_DGRAM`, `SW_SOCK_UDP` and `Server::CLOSE_ACTIVELY`.
const EVENT_RECV_DGRAM: u8 = 1;
const SOCK_UDP: c_int = 2;
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// `DgramPacket`: the datagram follows it.
#[repr(C)]
struct DgramPacket {
    socket_type: c_int,
    socket_addr: SocketAddress,
    length: u32,
}

/// The `MessageBus` of the context's thread, once an op has set it.
fn bus_of(ctx: &ReactorContext<'_>) -> Option<*mut c_void> {
    let thread = ctx.thread;
    (!thread.is_null()).then(|| thread.wrapping_byte_add(core::mem::offset_of!(ThreadRecord, bus)))
}

/// The context's event's fd.
fn event_fd(ctx: &ReactorContext<'_>) -> Option<c_int> {
    ctx.event.as_deref().map(swoole_rs_reactor::Event::fd)
}

/// `thread->message_bus.get_pipe_socket(sock)`.
fn pipe_socket<'b>(bus: &'b crate::objects::Bus, pipe: Option<Socket<'_>>) -> Option<Socket<'b>> {
    bus.pipe_socket(pipe?)
}

/// The server's worker `id`, if it has one.
fn worker(serv: view::Server<'_>, id: impl TryInto<u16>) -> Option<Worker<'_>> {
    serv.worker_record(id.try_into().unwrap_or(u16::MAX))
}

/// `Server::heartbeat_check`'s timer, which `reactor` arms for itself and runs on
/// its thread.
fn heartbeat_timer(state: &ServerState, reactor: Reactor) -> *mut c_void {
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
    handle.set_handler(FD_PIPE, SW_EVENT_READ, on_pipe_read);
    handle.set_handler(FD_PIPE, SW_EVENT_WRITE, entry::on_pipe_write);
    let reactors = config.reactor_num.get();
    // Listen on this thread's UDP ports.
    if config.have_dgram_sock {
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
            // The reactor thread that reads the datagram port.
            port.config()
                .thread_id
                .set(crate::reactor_ops::pthread_self());
            if handle.change(socket, 0, SW_EVENT_READ) < 0 {
                return Some(-1);
            }
        }
    }
    crate::reactor_thread::init_reactor(server, handle);
    let thread = ctx.thread_record()?;
    let bus = thread.message_bus();
    methods::init_pipe_sockets(server, bus);
    if u64::from(config.mode_) == crate::MODE_THREAD {
        let worker = worker(server, id)?;
        crate::worker_methods::init_event_worker(server, worker);
        if let Some(sock) = pipe_socket(&bus, worker_pipe(worker, false)) {
            let _ = handle.add(sock, SW_EVENT_READ);
        }
        if config.heartbeat_check_interval.get() > 0 {
            thread.heartbeat_timer.set(heartbeat_timer(state, handle));
        }
    }
    if !config.pipe_command.is_null() {
        // The server's live command pipe.
        let fd = config.command_pipe()?.worker_end()?.fd();
        bus.init_pipe_socket(fd);
        thread.pipe_command.set(bus.pipe_fd(fd).cast());
        bus.rust()
            .pipe(fd)?
            .edit(|s| s.output.buffer_size = u32::MAX);
    }
    let gs = crate::shared::server_counters(state)?;
    // SAFETY: the shared counter outlives every reactor thread's bus.
    let counter = unsafe {
        &*gs.pipe_packet_msg_id
            .as_ptr()
            .cast::<core::sync::atomic::AtomicU64>()
    };
    bus.rust().count_ids(counter);
    bus.set_always_chunked();
    if !bus.alloc_buffer(config.ipc_max_size() as usize) {
        return Some(-1);
    }
    for w in (0..config.worker_num.get()).filter(|&w| owns(i64::from(w), reactors, id)) {
        let Some(worker) = worker(server, w) else {
            return Some(-1);
        };
        let sock = pipe_socket(&bus, worker_pipe(worker, true));
        if sock.is_none_or(|sock| handle.add(sock, SW_EVENT_READ) < 0) {
            return Some(-1);
        }
        // Only synchronous sends go to the notify pipe: thread-safe, no separate memory.
        if thread.notify_pipe.get().is_null() {
            thread.notify_pipe.set(worker.pipe_worker().cast());
        }
    }
    Some(0)
}

/// Answers `op` when it is one of this module's, else `None`, as `reactor_ops::answer`.
#[inline(always)]
pub(crate) fn answer(
    ctx: &mut ReactorContext<'_>,
    state: &ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    Some(match op {
        OP_PACKET_INIT => {
            let fd = event_fd(ctx)?;
            // The event's fd is a listening socket's, whose slot is live.
            let conn = state.connection(fd.into(), Lookup::ByFd)?;
            let kind = conn.socket_type();
            // `ctx.sock` is already the event's socket, the port's listening socket
            // that `OP_STORE_LISTEN` stored in this slot.
            ctx.conn = conn.as_ptr();
            ctx.pkt = with_thread_buffer(swoole_rs_buffer::string::CppString::as_mut_ptr).cast();
            ctx.task = SendData {
                info: DataHead {
                    server_fd: u16::try_from(fd).unwrap_or(u16::MAX),
                    reactor_id: swoole_rs_corebase::thread_id().cast_signed(),
                    kind: EVENT_RECV_DGRAM,
                    time: swoole_rs_corebase::microtime(),
                    ..DataHead::default()
                },
                data: ptr::null(),
            };
            let pkt = packet(ctx)?;
            pkt.socket_type = kind;
            pkt.socket_addr.kind = kind;
            0
        }
        OP_RECVFROM => {
            let room = with_thread_buffer(|buffer| buffer.size()) - size_of::<DgramPacket>();
            let fd = ctx.sock?.fd();
            let pkt = packet(ctx)?;
            let data = (&raw mut *pkt).wrapping_add(1).cast();
            let address = &mut pkt.socket_addr;
            address.len = libc::socklen_t::try_from(size_of::<AddressStorage>()).unwrap_or(0);
            let (storage, len) = ((&raw mut address.addr).cast(), &raw mut address.len);
            // SAFETY: the buffer's room after the packet head, and the address's storage.
            let n = unsafe { swoole_rs_reactor::fd_recvfrom(fd, data, room, 0, storage, len) };
            sign(i64::try_from(n).unwrap_or(-1))
        }
        OP_PACKET_TASK => {
            let pkt = packet(ctx)?;
            let fd = if pkt.socket_type == SOCK_UDP {
                let ip = pkt.socket_addr.addr.v4().sin_addr.s_addr;
                c_long::from(ip.cast_signed())
            } else {
                let host =
                    swoole_rs_clients::address::swr_v1_clients_address_host(&pkt.socket_addr);
                let length = pkt.socket_addr.len as usize;
                if host.is_null() || length == 0 {
                    0
                } else {
                    // SAFETY: upstream hashes as many bytes of the text as the address is long.
                    let text = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) };
                    c_long::from(swoole_rs_crc32::crc32(text))
                }
            };
            pkt.length = u32::try_from(arg).unwrap_or(u32::MAX);
            ctx.task.info.fd = fd;
            let len = size_of::<DgramPacket>() as u64 + arg;
            ctx.task.info.len = u32::try_from(len).unwrap_or(u32::MAX);
            ctx.task.data = ctx.pkt.cast();
            (&raw mut ctx.task).expose_provenance() as u64
        }
        OP_FACTORY_DISPATCH => {
            u64::from(if u64::from(state.config_ref().mode_) == crate::MODE_BASE {
                // Base mode hands it to this process's worker as it does a connection's data.
                crate::base_factory::dispatch(ctx.serv, &mut ctx.task, true)
            } else {
                factory::dispatch(state, (&raw mut ctx.task).cast())
            })
        }
        _ => return None,
    })
}

/// The datagram `OP_PACKET_INIT` points the context at.
fn packet<'c>(ctx: &'c mut ReactorContext<'_>) -> Option<&'c mut DgramPacket> {
    // SAFETY: null, or this thread's buffer, which holds a packet head and its datagram.
    unsafe { ctx.pkt.cast::<DgramPacket>().as_mut() }
}

/// The destroy callback of a reactor thread's reactor: close its connections.
unsafe extern "C-unwind" fn close_connections(reactor: *mut c_void) {
    // SAFETY: the callback's live reactor, on its own thread.
    let reactor = unsafe { Reactor::of(reactor) };
    // SAFETY: a reactor thread's reactor, whose `ptr` is its live server.
    let serv = unsafe { entry::server(reactor) };
    crate::reactor_thread::close_reactor_connections(serv, reactor);
}

/// What a reactor thread Rust starts runs. As `state::ThreadEntry`, whose `void *`
/// a [`view::Server`] is laid out as: `ServerState::start_thread` calls it with the
/// live server, on its new reactor thread.
extern "C" fn reactor_thread_entry(server: view::Server<'_>, id: u64) {
    swoole_rs_corebase::swoole_thread_init(false);
    let id = c_int::try_from(id).unwrap_or_default();
    main_loop(server, u16::try_from(id).unwrap_or_default());
    swoole_rs_corebase::swoole_thread_clean(false);
}

/// What the heartbeat thread Rust starts runs, its signals blocked. Called with the
/// live server, as [`reactor_thread_entry`] is.
extern "C" fn heartbeat_main(server: view::Server<'_>, _: u64) {
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
pub(crate) fn start_reactor_threads(serv: view::Server<'_>) -> c_int {
    let state = serv.state();
    let config = state.config_ref();
    if swoole_rs_reactor::swoole_event_init(0) < 0 {
        return -1;
    }
    let reactor = Reactor::thread();
    for (index, port) in state.tables_ref().ports().iter().enumerate() {
        if matches!(port.config().r#type, 2 | 4 | 6) {
            continue;
        }
        if (crate::port_config::methods().listen)(port.config()) < 0 {
            swoole_rs_reactor::swoole_event_free();
            return -1;
        }
        if let Some(socket) = serv.port_socket_at(index) {
            reactor.change(socket, 0, SW_EVENT_READ);
        }
    }
    methods::store_listen_socket(serv);
    let single = config.single_thread.get();
    if single {
        // Thread 0's setup over the master's reactor; it runs the one loop.
        let mut init = ReactorContext::new(serv, Some(reactor));
        init.thread = thread(state, 0);
        if let Some(record) = init.thread_record() {
            let id = u16::try_from(record.id.get()).unwrap_or_default();
            let _ = init_thread(&mut init, state, id);
        }
    } else {
        // a special id for the master thread
        let id = config.reactor_num.get();
        reactor.handle().set_id(id);
        swoole_rs_corebase::set_thread(swoole_rs_corebase::thread_kind(), id);
        let entry = reactor_thread_entry as *const () as u64;
        let raw = serv.raw().expose_provenance() as u64;
        for id in 0..config.reactor_num.get() {
            state.start_thread(entry, raw, u64::from(id));
        }
    }
    if config.heartbeat_check_interval.get() >= 1 {
        if single {
            let timer = heartbeat_timer(state, reactor);
            config
                .heartbeat_timer
                .store(timer, core::sync::atomic::Ordering::Relaxed);
        } else {
            let raw = serv.raw().expose_provenance() as u64;
            state.start_thread(heartbeat_main as *const () as u64, raw, HEARTBEAT);
        }
    }
    methods::start_master_thread(serv, reactor)
}

/// `Server::reactor_thread_main_loop`: reactor thread `id`'s setup, its event loop
/// and its cleanup.
pub(crate) fn main_loop(serv: view::Server<'_>, id: u16) {
    let state = serv.state();
    let mut ctx = ReactorContext::new(serv, None);
    ctx.thread = thread(state, usize::from(id));
    let Some(record) = ctx.thread_record() else {
        return;
    };
    record.id.set(c_int::from(id));
    if let Some(bus) = bus_of(&ctx) {
        swoole_rs_corebase::set_thread_message_bus(bus);
    }
    if swoole_rs_reactor::swoole_event_init(0) < 0 {
        return;
    }
    ctx.reactor = Some(Reactor::thread());
    let mode = u64::from(state.config_ref().mode_);
    let worker = || worker(serv, id);
    if mode == crate::MODE_THREAD {
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
    if mode == crate::MODE_PROCESS {
        methods::reactor_barrier(serv);
    }
    #[expect(
        clippy::let_underscore_must_use,
        reason = "upstream's reactor thread ignores its loop's result: it cleans up either way"
    )]
    let _ = swoole_rs_reactor::event_api::swoole_event_wait();
    if mode == crate::MODE_THREAD
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
        send_event(
            worker_pipe(worker, false),
            session,
            crate::ops::EVENT_CLOSE_FORCE,
        );
    }
}

/// `ReactorThread_onPipeRead`: a worker's message.
extern "C" fn on_pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let reactor = Reactor::running(reactor);
    // SAFETY: the handler is registered only on server reactors.
    let serv = unsafe { entry::server(reactor) };
    let mut ctx = ReactorContext::new(serv, Some(reactor));
    ctx.thread = thread(serv.state(), usize::from(reactor.handle().id()));
    pipe_read(&mut ctx, event).unwrap_or(-1)
}

/// `ReactorThread_onPipeRead`: the messages workers send their reactor thread,
/// until the pipe is drained.
fn pipe_read(ctx: &mut ReactorContext<'_>, event: &Event) -> Option<c_int> {
    use crate::ops::{
        EVENT_CLOSE_FORCE, EVENT_CLOSE_FORWARD, EVENT_COMMAND_REQUEST, EVENT_COMMAND_RESPONSE,
        EVENT_FINISH, EVENT_INCOMING, EVENT_PIPE_MESSAGE, EVENT_SHUTDOWN_SIGNAL,
    };
    let (serv, reactor) = (ctx.serv, ctx.reactor?);
    let state = serv.state();
    let thread = ctx.thread_record()?;
    let bus = thread.message_bus();
    let sock = event.socket()?;
    ctx.resp = bus.buffer();
    loop {
        if crate::pool_native::read_bus(bus.rust(), thread.allocator, sock, true) <= 0 {
            return Some(0);
        }
        let head = *ctx.message()?;
        let session = head.fd;
        match u32::from(head.kind) {
            EVENT_INCOMING => {
                if let Some(conn) = state.connection(session, Lookup::Verified)
                    && methods::connection_incoming(serv, reactor, conn) < 0
                    && let Some(sock) = conn.socket()
                {
                    reactor.close(sock);
                }
            }
            EVENT_COMMAND_REQUEST => {
                let id = u16::try_from(thread.id.get()).unwrap_or_default();
                // The bus's record of the command pipe (`thread.pipe_command`).
                let end = state
                    .config_ref()
                    .command_pipe()
                    .and_then(swoole_rs_tls::socket_pair::SocketPair::worker_end);
                let pipe = end.and_then(|end| bus.pipe_socket(end));
                crate::commands::call_command_handler(serv, bus, id, pipe);
            }
            EVENT_COMMAND_RESPONSE => {
                bus.with_packet(|reply| crate::commands::command_result(state, session, reply));
            }
            crate::ops::EVENT_SHUTDOWN => shutdown(ctx)?,
            EVENT_SHUTDOWN_SIGNAL => {
                swoole_rs_process::os::kill(std::process::id().cast_signed(), libc::SIGTERM);
            }
            EVENT_FINISH | EVENT_PIPE_MESSAGE => {
                // SAFETY: the bus's buffer: its `EventData`, its head and the payload it
                // counts.
                let event =
                    unsafe { swoole_rs_process::task_file::Packed::from_raw(ctx.resp.cast()) };
                if u32::from(head.kind) == EVENT_FINISH {
                    let _ = state.callbacks.task(TaskSlot::Finish, serv, event);
                } else {
                    state.callbacks.pipe_message(serv, event);
                }
            }
            EVENT_CLOSE_FORCE => {
                crate::reactor_thread::close(serv, reactor, session);
            }
            EVENT_CLOSE_FORWARD => {
                factory::end(state, session, CLOSE_ACTIVELY);
            }
            _ => {
                let packet = bus.packet();
                let mut info = head;
                info.len = u32::try_from(packet.length).unwrap_or(u32::MAX);
                let send = SendData {
                    info,
                    data: packet.data,
                };
                // SAFETY: a send over the bus's packet.
                unsafe { methods::send_to_connection(serv, &send) };
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
    if config.have_dgram_sock {
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
    let timer = thread.heartbeat_timer.replace(ptr::null_mut());
    if !timer.is_null() {
        // SAFETY: the thread's live heartbeat timer.
        unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
    }
    // SAFETY: the call takes the reactor, which outlives it.
    unsafe { swoole_rs_reactor::defer_destroy(reactor.handle(), close_connections, reactor.ptr()) };
    if u64::from(config.mode_) == crate::MODE_THREAD {
        stop_async_worker(serv, worker(serv, id)?);
        return Some(());
    }
    let bus = thread.message_bus();
    for w in (0..config.worker_num.get()).filter(|&w| owns(i64::from(w), reactors, id)) {
        if let Some(sock) = pipe_socket(&bus, worker_pipe(worker(serv, w)?, true)) {
            reactor.interest(sock, 4, 0);
        }
    }
    each_connection(state, |conn| {
        let Some(sock) = conn.socket() else {
            return;
        };
        let reading = !conn.peer_closed() && !sock.read(|s| s.removed);
        if reading && owns(i64::from(conn.fd()), reactors, id) {
            reactor.interest(sock, 4, 0);
        }
    });
    reactor.handle().set_wait_exit(true);
    Some(())
}
