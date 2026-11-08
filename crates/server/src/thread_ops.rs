//! The reactor-thread ops over a thread's own state that were master.cc's: its
//! entry and setup, its message bus and worker pipes, the messages workers send
//! it, its timers and the server methods it calls. Each is a Rust call: the bus
//! is crates/protocols', the methods are this crate's and crates/process' and
//! crates/ports', reached under their upstream symbols.
use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::DataHead;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_reactor::{SocketRecord, reactor_upstream};

use crate::base_factory::SendData;
use crate::callbacks::{CB_FINISH, CB_PIPE_MESSAGE};
use crate::ops::{
    EVENT_SHUTDOWN, OP_CLOSE, OP_COMMAND_REQUEST, OP_COMMAND_RESPONSE, OP_CONN_HEALTHY,
    OP_DGRAM_BUFFER, OP_HEARTBEAT_ENTRY, OP_HEARTBEAT_TIMER, OP_INCOMING, OP_ON_FINISH,
    OP_ON_PIPE_MESSAGE, OP_PIPE_POP, OP_PIPE_READ, OP_PIPE_SEND, OP_PORT_LISTEN, OP_PORT_READ,
    OP_RD_ABORT, OP_REACTOR_THREAD_ENTRY, OP_REACTOR_THREAD_TYPE, OP_RESUME_TIMER,
    OP_RT_ADD_WORKER_PIPE, OP_RT_BUS_SETUP, OP_RT_CATCH_WAIT, OP_RT_FRONT, OP_RT_HEARTBEAT_TIMER,
    OP_RT_INIT_PIPE_SOCKETS, OP_RT_INIT_REACTOR, OP_RT_PIPE_COMMAND, OP_RT_SEND_FRONT,
    OP_RT_THREAD_WORKER, OP_SEND_CLOSE_FORCE, OP_STOP_THREAD_WORKER, OP_THREAD_CLEAN,
    OP_THREAD_ENTER, OP_THREAD_INIT, OP_THREAD_NOTIFY, OP_THREAD0_INIT, OP_WORKER_PIPE_UNREAD,
    OP_WORKER_START, OP_WORKER_STOP, RUN_HEARTBEAT_CHECK, RUN_HEARTBEAT_THREAD,
    RUN_RESUME_RECEIVING,
};
use crate::reactor_entry::{self as entry, run, thread};
use crate::reactor_ops::{ReactorContext, ThreadRecord, int, sign};
use crate::records::ConnectionRecord;
use crate::{ServerState, server_methods as methods, view};

/// `SW_EVENT_READ`, `Server::THREAD_REACTOR` and `SW_WAIT`.
const SW_EVENT_READ: c_int = 1 << 9;
const THREAD_REACTOR: u8 = 2;
const SW_WAIT: c_int = 2;

/// `swoole::PacketPtr`.
#[repr(C)]
struct BusPacket {
    length: usize,
    data: *const c_char,
}

// SAFETY: crates/protocols' bus, linked into the archive.
unsafe extern "C" {
    fn swr_v1_bus_set_buffer_size(bus: *mut c_void, size: usize);
    fn swr_v1_bus_buffer_size(bus: *mut c_void) -> usize;
    fn swr_v1_bus_set_always_chunked(bus: *mut c_void);
    fn swr_v1_bus_set_buffer(bus: *mut c_void, buffer: *mut c_void);
    fn swr_v1_bus_buffer(bus: *mut c_void) -> *mut DataHead;
    fn swr_v1_bus_packet(bus: *mut c_void) -> BusPacket;
    fn swr_v1_bus_pop(bus: *mut c_void);
    fn swr_v1_bus_count_ids(bus: *mut c_void, counter: *mut c_void);
    fn swr_v1_bus_init_pipe_socket(bus: *mut c_void, fd: c_int);
    fn swr_v1_bus_pipe_socket(bus: *mut c_void, fd: c_int) -> *mut SocketRecord;
}

// SAFETY: crates/process' and crates/ports' methods under their upstream symbols.
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole6Server26call_worker_start_callbackEPNS_6WorkerE"]
    fn call_worker_start_callback(serv: *mut c_void, worker: *mut WorkerRecord);
    #[link_name = "_ZN6swoole6Server25call_worker_stop_callbackEPNS_6WorkerE"]
    fn call_worker_stop_callback(serv: *mut c_void, worker: *mut WorkerRecord);
    #[link_name = "_ZN6swoole6Server17stop_async_workerEPNS_6WorkerE"]
    fn stop_async_worker(serv: *mut c_void, worker: *mut WorkerRecord);
    #[link_name = "_ZN6swoole10ListenPort6listenEv"]
    fn port_listen(port: *mut c_void) -> c_int;
}

/// The context's thread.
fn this<'a>(ctx: &ReactorContext) -> &'a mut ThreadRecord {
    // SAFETY: the ops reading it run with a live reactor thread in the context.
    unsafe { &mut *ctx.thread.cast::<ThreadRecord>() }
}

/// The thread's `MessageBus`.
fn bus_of(thread: &mut ThreadRecord) -> *mut c_void {
    (&raw mut thread.bus).cast()
}

fn socket<'a>(socket: *mut c_void) -> &'a mut SocketRecord {
    // SAFETY: every caller passes a live socket.
    unsafe { &mut *socket.cast::<SocketRecord>() }
}

/// `thread->message_bus.get_pipe_socket(sock)`.
fn pipe_socket(thread: &ThreadRecord, sock: *mut c_void) -> *mut SocketRecord {
    // SAFETY: the thread's live bus, and a live pipe socket.
    unsafe { swr_v1_bus_pipe_socket(thread.bus, socket(sock).fd) }
}

fn worker<'a>(serv: *mut c_void, id: impl TryInto<u16>) -> &'a mut WorkerRecord {
    let id = id.try_into().unwrap_or(u16::MAX);
    // SAFETY: a live server; every caller passes a worker id it has.
    let worker = unsafe { view::worker(serv, id) };
    // SAFETY: as above.
    unsafe { &mut *worker }
}

/// `swoole_timer_add(msec, persistent, callback, data)`.
fn timer_add(
    msec: c_long,
    persistent: bool,
    callback: unsafe extern "C" fn(*mut c_void),
    data: *mut c_void,
) -> *mut c_void {
    // SAFETY: a timer of this thread's.
    unsafe { crate::pool_native::swr_cpp_timer_add(msec, persistent, callback, data) }
}

/// `socket->send_sync(&head, sizeof(head))` of a bare event.
fn send_event(sock: *mut c_void, fd: c_long, kind: u32) {
    let head = DataHead {
        fd,
        kind: u8::try_from(kind).unwrap_or(u8::MAX),
        ..DataHead::default()
    };
    let data = (&raw const head).cast();
    // SAFETY: a live pipe socket, and the head's bytes.
    unsafe {
        swoole_rs_tls::upstream::socket_send_sync(sock.cast(), data, size_of::<DataHead>(), 0)
    };
}

/// Answers `op` when it is one of this module's, else `None`.
///
/// # Safety
/// As `reactor_ops::answer`.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
#[inline(always)]
pub(crate) unsafe fn answer(
    ctx: &mut ReactorContext,
    state: &ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    let (serv, reactor) = (ctx.serv, ctx.reactor);
    // SAFETY: a live server's config, borrowed only by the arm that reads it.
    let config = || unsafe { &mut *state.config() };
    let reactor_id = || {
        // SAFETY: the context's live reactor, which begins with its prepared state.
        let fields = unsafe { reactor_upstream::prepared(reactor) }.fields;
        // SAFETY: as above.
        unsafe { (*fields).id }
    };
    // `reactor->add(socket, SW_EVENT_READ)`.
    let add_read = |socket: *mut SocketRecord| {
        // SAFETY: the context's live reactor and a live socket.
        unsafe { reactor_upstream::reactor_change(reactor, socket, 0, SW_EVENT_READ) }
    };
    let (conn, resp) = (ctx.conn, ctx.resp);
    // SAFETY: the ops reading it run with a live connection.
    let conn_ref = || unsafe { &mut *conn };
    Some(match op {
        OP_THREAD_ENTER => {
            ctx.thread = thread(state, usize::try_from(arg).ok()?);
            let thread = this(ctx);
            thread.id = int(arg);
            swoole_rs_corebase::set_thread_message_bus(bus_of(thread));
            0
        }
        OP_REACTOR_THREAD_TYPE => {
            swoole_rs_corebase::set_thread(THREAD_REACTOR, u16::try_from(arg & 0xffff).ok()?);
            0
        }
        OP_WORKER_START | OP_WORKER_STOP => {
            let worker = worker(serv, this(ctx).id);
            if op == OP_WORKER_START {
                // SAFETY: the live server and this thread's worker.
                unsafe { call_worker_start_callback(serv, worker) };
            } else {
                // SAFETY: as above.
                unsafe { call_worker_stop_callback(serv, worker) };
            }
            0
        }
        OP_STOP_THREAD_WORKER => {
            // SAFETY: the live server and this reactor's worker.
            unsafe { stop_async_worker(serv, worker(serv, reactor_id())) };
            0
        }
        OP_THREAD_INIT | OP_THREAD0_INIT => {
            if op == OP_THREAD0_INIT {
                ctx.thread = thread(state, 0);
            }
            let id = u16::try_from(this(ctx).id).unwrap_or_default();
            // SAFETY: the server's live thread and its new reactor.
            let ret = unsafe { entry::thread_init(ctx.thread, serv, reactor, id) };
            sign(if op == OP_THREAD0_INIT { 0 } else { ret })
        }
        OP_THREAD_CLEAN => {
            // `message_bus.free_buffer()`.
            let thread = this(ctx);
            // SAFETY: the thread's live bus.
            let buffer = unsafe { swr_v1_bus_buffer(thread.bus) };
            // SAFETY: the bus's allocator, which allocated its buffer.
            if let Some(free) = unsafe { (*thread.allocator).free } {
                // SAFETY: as above.
                unsafe { free(buffer.cast()) };
            }
            // SAFETY: the thread's live bus.
            unsafe { swr_v1_bus_set_buffer(thread.bus, ptr::null_mut()) };
            0
        }
        OP_THREAD_NOTIFY => {
            let joining = thread(state, usize::try_from(arg).ok()?).cast::<ThreadRecord>();
            // SAFETY: one of the server's live threads.
            let notify = unsafe { (*joining).notify_pipe };
            if !notify.is_null() {
                send_event(notify.cast(), 0, EVENT_SHUTDOWN);
            }
            0
        }
        OP_REACTOR_THREAD_ENTRY => reactor_thread_entry as *const () as u64,
        OP_HEARTBEAT_ENTRY => heartbeat_main as *const () as u64,
        OP_HEARTBEAT_TIMER | OP_RT_HEARTBEAT_TIMER => {
            let msec = c_long::from(config().heartbeat_check_interval) * 1000;
            let timer = timer_add(msec, true, heartbeat_check, reactor);
            if op == OP_HEARTBEAT_TIMER {
                config().heartbeat_timer = timer;
            } else {
                this(ctx).heartbeat_timer = timer;
            }
            0
        }
        OP_RESUME_TIMER => {
            let msec = c_long::from(conn_ref().waiting_time);
            let timer = timer_add(msec, false, resume_data_receiving, ctx.sock.cast());
            conn_ref().timer = timer;
            u64::from(!timer.is_null())
        }

        OP_RT_INIT_REACTOR => {
            // SAFETY: the live server and this thread's reactor.
            unsafe { entry::init_reactor(serv, reactor) };
            0
        }
        OP_RT_INIT_PIPE_SOCKETS => {
            // SAFETY: the live server and this thread's bus.
            unsafe { methods::init_pipe_sockets(serv, bus_of(this(ctx))) };
            0
        }
        OP_RT_THREAD_WORKER => {
            let worker = worker(serv, reactor_id());
            // SAFETY: the live server and this thread's worker.
            unsafe { crate::worker_methods::init_event_worker(serv, worker) };
            sign(add_read(pipe_socket(this(ctx), worker.pipe_worker)));
            0
        }
        OP_RT_PIPE_COMMAND => {
            let pair = config()
                .pipe_command
                .cast::<swoole_rs_tls::socket_pair::SocketPair>();
            // SAFETY: the server's live command pipe.
            let fd = socket(unsafe { (*pair).worker_socket }.cast()).fd;
            let thread = this(ctx);
            // SAFETY: the thread's live bus.
            unsafe { swr_v1_bus_init_pipe_socket(thread.bus, fd) };
            // SAFETY: as above.
            thread.pipe_command = unsafe { swr_v1_bus_pipe_socket(thread.bus, fd) };
            socket(thread.pipe_command.cast()).output.buffer_size = u32::MAX;
            0
        }
        OP_RT_BUS_SETUP => {
            let thread = this(ctx);
            // SAFETY: a live server's shared counters.
            let gs = unsafe { &*config().gs.cast::<crate::shared::ServerCounters>() };
            let counter = gs.pipe_packet_msg_id.as_ptr().cast();
            // SAFETY: the thread's live bus, counting from the shared counter.
            unsafe { swr_v1_bus_count_ids(thread.bus, counter) };
            // SAFETY: as above.
            unsafe { swr_v1_bus_set_buffer_size(thread.bus, config().ipc_max_size as usize) };
            // SAFETY: as above.
            unsafe { swr_v1_bus_set_always_chunked(thread.bus) };
            // SAFETY: as above.
            let size = unsafe { swr_v1_bus_buffer_size(thread.bus) };
            // SAFETY: the bus's live allocator.
            let malloc = unsafe { (*thread.allocator).malloc };
            // SAFETY: as above.
            let buffer = malloc.map_or(ptr::null_mut(), |malloc| unsafe { malloc(size) });
            // SAFETY: the bus takes the buffer.
            unsafe { swr_v1_bus_set_buffer(thread.bus, buffer) };
            u64::from(!buffer.is_null())
        }
        OP_RT_ADD_WORKER_PIPE | OP_WORKER_PIPE_UNREAD => {
            let worker = worker(serv, arg);
            let thread = this(ctx);
            let sock = pipe_socket(thread, worker.pipe_master);
            if op == OP_WORKER_PIPE_UNREAD {
                // SAFETY: the context's live reactor and the pipe socket.
                unsafe { reactor_upstream::reactor_interest(reactor, sock, 4, 0) };
                return Some(0);
            }
            if add_read(sock) < 0 {
                return Some(sign(-1));
            }
            // Only synchronous sends go to the notify pipe: thread-safe, no separate memory.
            if thread.notify_pipe.is_null() {
                thread.notify_pipe = worker.pipe_worker.cast();
            }
            0
        }

        OP_PIPE_READ => {
            let thread = this(ctx);
            // SAFETY: the thread's live bus.
            ctx.resp = unsafe { swr_v1_bus_buffer(thread.bus) };
            // SAFETY: the context's live event.
            let sock = socket(unsafe { (*ctx.event).socket });
            let read = crate::pool_native::read_bus(thread.bus, thread.allocator, sock, true);
            sign(i64::try_from(read).unwrap_or(-1))
        }
        OP_PIPE_POP => {
            // SAFETY: the thread's live bus.
            unsafe { swr_v1_bus_pop(this(ctx).bus) };
            0
        }
        OP_COMMAND_REQUEST => {
            let thread = this(ctx);
            let (id, pipe) = (
                u16::try_from(thread.id).unwrap_or_default(),
                thread.pipe_command,
            );
            // SAFETY: the live server, this thread's bus holding the request and its pipe.
            unsafe { crate::commands::call_command_handler(serv, bus_of(thread), id, pipe.cast()) };
            0
        }
        OP_COMMAND_RESPONSE | OP_PIPE_SEND => {
            // SAFETY: the thread's live bus, holding a whole packet.
            let packet = unsafe { swr_v1_bus_packet(this(ctx).bus) };
            // SAFETY: the bus's buffer, which PIPE_READ stored.
            let mut info = unsafe { *resp };
            if op == OP_COMMAND_RESPONSE {
                use crate::commands::swr_v1_server_command_result as result;
                let (data, length) = (packet.data, packet.length);
                // SAFETY: the live server and the reply's bytes.
                unsafe { result(state, serv, info.fd, data, length) };
                return Some(0);
            }
            info.len = u32::try_from(packet.length).unwrap_or(u32::MAX);
            let send = SendData {
                info,
                data: packet.data,
            };
            // SAFETY: the live server and a send over the packet.
            unsafe { methods::send_to_connection(serv, (&raw const send).cast()) };
            0
        }
        OP_ON_FINISH | OP_ON_PIPE_MESSAGE => {
            let slot = if op == OP_ON_FINISH {
                CB_FINISH
            } else {
                CB_PIPE_MESSAGE
            };
            let slot = state.callbacks.slot(slot)?;
            // SAFETY: the slot's `(Server *, EventData *)` callback over the message.
            unsafe { slot.call(serv, resp.cast_mut().cast()) };
            0
        }

        OP_RT_FRONT => {
            // SAFETY: the context's live event.
            let buffer = socket(unsafe { (*ctx.event).socket }).output.out_buffer;
            // SAFETY: the socket's live, non-empty output buffer.
            let front = unsafe { swoole_rs_buffer::cpp::swr_cpp_buffer_typed_front(buffer.cast()) };
            ctx.front = front.data.cast();
            ctx.front_length = front.length as usize;
            0
        }
        OP_RT_SEND_FRONT => {
            // SAFETY: the context's live event.
            let sock = unsafe { (*ctx.event).socket }.cast();
            let (front, length) = (ctx.front.cast(), ctx.front_length);
            // SAFETY: the event's live socket and the front message's bytes.
            let sent = unsafe { swoole_rs_tls::upstream::socket_send(sock, front, length, 0) };
            sign(i64::try_from(sent).unwrap_or(-1))
        }
        OP_RT_CATCH_WAIT => {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            // SAFETY: the context's live event.
            let sock = socket(unsafe { (*ctx.event).socket });
            u64::from(sock.catch_error(errno) == SW_WAIT)
        }

        // SAFETY: the live server, this thread's reactor and a live connection.
        OP_INCOMING => sign(unsafe { methods::connection_incoming(serv, reactor, conn) }),
        // SAFETY: this thread's reactor and the connection's live socket.
        OP_CLOSE => sign(unsafe { entry::close_connection(reactor, conn_ref().socket.cast()) }),
        OP_CONN_HEALTHY => {
            // SAFETY: the live server and one of its connections.
            u64::from(unsafe { methods::is_healthy_connection(serv, f64::from_bits(arg), conn) })
        }
        OP_SEND_CLOSE_FORCE => {
            // `serv->get_reactor_pipe_socket(session_id, conn->reactor_id)`.
            let session = arg.cast_signed();
            // SAFETY: a live server's tables.
            let pipes = i64::from(unsafe { (*state.tables()).reactor_pipe_num });
            let index = session % pipes;
            let id = i64::from(conn_ref().reactor_id) + index * i64::from(config().reactor_num);
            // the session id, so the reactor verifies the connection before closing it
            send_event(
                worker(serv, id).pipe_worker,
                session,
                crate::ops::EVENT_CLOSE_FORCE,
            );
            0
        }
        OP_RD_ABORT => {
            let (port, sock) = (ctx.port, ctx.sock.cast());
            // SAFETY: the live server, this thread's reactor, the port and its socket.
            unsafe { methods::abort_connection(serv, reactor, port, sock) };
            0
        }
        OP_PORT_LISTEN | OP_PORT_READ => {
            // SAFETY: a live server's tables.
            let &port = unsafe { &*state.tables() }
                .ports()
                .get(usize::try_from(arg).ok()?)?;
            if op == OP_PORT_LISTEN {
                // SAFETY: one of the server's live ports.
                return Some(sign(unsafe { port_listen(port) }));
            }
            // SAFETY: as above.
            add_read(unsafe { crate::port_config::of(port) }.socket.cast());
            0
        }
        OP_DGRAM_BUFFER => {
            // support 64K packet
            let buffer = swoole_rs_corebase::thread_buffer().cast::<CppString>();
            // SAFETY: this thread's live buffer.
            let buffer = unsafe { &mut *buffer };
            buffer.extend(buffer.size.wrapping_mul(2));
            0
        }
        _ => return None,
    })
}

/// The server's state.
///
/// # Safety
/// `serv` is a live `Server`.
unsafe fn state_of<'a>(serv: *mut c_void) -> &'a ServerState {
    // SAFETY: per the caller.
    let state = unsafe { view::state_ptr(serv) };
    // SAFETY: the state lives as long as the server.
    unsafe { &*state }
}

/// What a reactor thread Rust starts runs.
unsafe extern "C" fn reactor_thread_entry(serv: *mut c_void, id: u64) {
    swoole_rs_corebase::swoole_thread_init(false);
    let id = c_int::try_from(id).unwrap_or_default();
    // SAFETY: the live server, on its new reactor thread.
    unsafe { entry::reactor_thread_main_loop(serv, id) };
    swoole_rs_corebase::swoole_thread_clean(false);
}

/// What the heartbeat thread Rust starts runs, its signals blocked.
unsafe extern "C" fn heartbeat_main(serv: *mut c_void, _: u64) {
    // SAFETY: the live server.
    let state = unsafe { state_of(serv) };
    // SAFETY: its live config.
    let reactors = unsafe { (*state.config()).reactor_num };
    // upstream sets THREAD_HEARTBEAT, then overwrites the type with this.
    let kind = (reactors.wrapping_add(1) & 0xff) as u8;
    swoole_rs_corebase::set_thread(kind, swoole_rs_corebase::thread_id());
    let mut ctx = ReactorContext::new(serv, ptr::null_mut());
    // SAFETY: the live server.
    unsafe { run(RUN_HEARTBEAT_THREAD, 0, &mut ctx) };
}

/// `Server::heartbeat_check`: the heartbeat timer of `reactor`.
unsafe extern "C" fn heartbeat_check(reactor: *mut c_void) {
    // SAFETY: the timer's live reactor, which begins with its prepared state.
    let fields = unsafe { reactor_upstream::prepared(reactor) }.fields;
    // SAFETY: as above.
    let fields = unsafe { &*fields };
    let mut ctx = ReactorContext::new(fields.ptr, reactor);
    // SAFETY: the reactor's live server.
    ctx.thread = thread(unsafe { state_of(fields.ptr) }, usize::from(fields.id));
    // SAFETY: as above.
    unsafe { run(RUN_HEARTBEAT_CHECK, 0, &mut ctx) };
}

/// `ReactorThread_resume_data_receiving`: the timer of a paused session socket.
unsafe extern "C" fn resume_data_receiving(sock: *mut c_void) {
    let sock = sock.cast::<SocketRecord>();
    // SAFETY: the timer's live session socket, whose object is its connection.
    let conn = unsafe { (*sock).object }.cast::<ConnectionRecord>();
    let mut ctx = ReactorContext::new(swoole_rs_corebase::swr_v1_core_server(), ptr::null_mut());
    ctx.sock = sock;
    ctx.conn = conn;
    // SAFETY: the live server.
    unsafe { run(RUN_RESUME_RECEIVING, 0, &mut ctx) };
}
