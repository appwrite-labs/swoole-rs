//! The reactor-thread ops over a thread's own state that were master.cc's: its
//! entry and setup, its message bus and worker pipes, the messages workers send
//! it, its timers and the server methods it calls. Each is a Rust call: the bus
//! is crates/bus', the methods are this crate's and crates/process' and
//! crates/ports', reached under their upstream symbols.
use core::ffi::{c_int, c_long, c_void};
use core::ptr;

use crate::objects::socket;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_process::workers::entry::{
    call_worker_start_callback, call_worker_stop_callback, stop_async_worker,
};
use swoole_rs_reactor::{Socket, SocketRecord, reactor_upstream};
use swoole_rs_timer::swr_v1_timer_add_plain;

use crate::base_factory::SendData;
use crate::callbacks::{CB_FINISH, CB_PACKET, CB_PIPE_MESSAGE, CB_RECEIVE};
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
use crate::ops::{
    OP_CC_COUNT, OP_CC_HTTP_DESTROY, OP_CLOSE_FORWARD, OP_DESTROY_CALLBACK, OP_FORCE_CLOSE,
    OP_PACKET_INIT, OP_PACKET_TASK, OP_PC_NOTIFY, OP_REACTOR_BARRIER, OP_RECVFROM, OP_WR_SEND,
    OP_WR_SENDFILE, RUN_CLOSE_REACTOR_CONNECTIONS,
};
use crate::ops::{OP_FACTORY_DISPATCH, OP_ON_PACKET, OP_ON_RECEIVE, OP_RD_PORT_READ};
use crate::reactor_entry::{self as entry, run, thread};
use crate::reactor_ops::{ReactorContext, ThreadRecord, int, sign};
use crate::records::ConnectionRecord;
use crate::{ServerState, factory, server_methods as methods, view};

/// `SW_EVENT_READ`, `Server::THREAD_REACTOR` and `SW_WAIT`.
const SW_EVENT_READ: c_int = 1 << 9;
const THREAD_REACTOR: u8 = 2;
const SW_WAIT: c_int = 2;
/// `SW_SERVER_EVENT_RECV_DGRAM`, `SW_SERVER_EVENT_CLOSE`, `SW_SOCK_UDP`
/// and `Server::CLOSE_ACTIVELY`.
const EVENT_RECV_DGRAM: u8 = 1;
const EVENT_CLOSE: u8 = 4;
const SOCK_UDP: c_int = 2;
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// `DgramPacket`: the datagram follows it.
#[repr(C)]
struct DgramPacket {
    socket_type: c_int,
    socket_addr: SocketAddress,
    length: u32,
}

/// The front entry of the context event socket's output buffer, if it has one.
fn out_front(ctx: &ReactorContext) -> Option<swoole_rs_buffer::TypedFront> {
    let front = swoole_rs_buffer::cpp::swr_cpp_buffer_typed_front;
    event_socket(ctx)?.read(|s| crate::reactor_ops::out_buffer(s).map(|b| front(b)))
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

/// The context event's socket.
fn event_socket<'a>(ctx: &ReactorContext) -> Option<Socket<'a>> {
    // SAFETY: the ops reading it run on a live event.
    socket(unsafe { (*ctx.event).socket_ptr() })
}

/// `thread->message_bus.get_pipe_socket(sock)`.
fn pipe_socket<'a>(thread: &ThreadRecord, sock: *mut c_void) -> Option<Socket<'a>> {
    socket(thread.message_bus().pipe_socket(socket(sock)?))
}

fn worker<'a>(serv: view::Server<'_>, id: impl TryInto<u16>) -> &'a mut WorkerRecord {
    let id = id.try_into().unwrap_or(u16::MAX);
    let worker = serv.worker(id);
    // SAFETY: as above.
    unsafe { &mut *worker }
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
    // SAFETY: the context's live server, as `reactor_ops::answer` requires.
    let server = unsafe { view::Server::from_raw(serv) };
    // SAFETY: a live server's config, borrowed only by the arm that reads it.
    let config = || unsafe { &mut *state.config() };
    // SAFETY: the context's live reactor, on its own thread.
    let handle = unsafe { crate::reactor_handle::Reactor::of(reactor) };
    let reactor_id = || handle.fields().id;
    // `reactor->add(socket, SW_EVENT_READ)`.
    let add_read =
        |socket: Option<Socket<'_>>| socket.map_or(-1, |s| handle.change(s, 0, SW_EVENT_READ));
    let (conn, resp) = (ctx.conn, ctx.resp);
    // SAFETY: the ops reading it run with a live connection.
    let conn_ref = || unsafe { &mut *conn };
    Some(match op {
        OP_REACTOR_BARRIER => {
            methods::reactor_barrier(server);
            0
        }
        OP_FORCE_CLOSE => {
            conn_ref().close_force = 1;
            // SAFETY: the connection's live socket, for this call.
            let mut event =
                unsafe { swoole_rs_reactor::Event::new(conn_ref().fd, 0, 0, conn_ref().socket) };
            // SAFETY: this thread's reactor and an event over a live connection.
            unsafe { reactor_upstream::trigger_close_event(reactor, (&raw mut event).cast()) };
            0
        }
        OP_PACKET_INIT => {
            // SAFETY: the context's live event.
            let fd = unsafe { (*ctx.event).fd() };
            // The event's fd is a listening socket's, whose slot is live.
            let conn = state.connection(fd.into(), 0)?;
            let kind = conn.socket_type;
            ctx.sock = conn.socket.cast();
            ctx.conn = conn;
            let buffer = swoole_rs_corebase::thread_buffer().cast::<CppString>();
            // SAFETY: this thread's live buffer.
            ctx.pkt = unsafe { (*buffer).as_mut_ptr() }.cast();
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
            let pkt = packet(ctx);
            pkt.socket_type = kind;
            pkt.socket_addr.kind = kind;
            0
        }
        OP_RECVFROM => {
            let buffer = swoole_rs_corebase::thread_buffer().cast::<CppString>();
            // SAFETY: this thread's live buffer.
            let room = unsafe { (*buffer).size() } - size_of::<DgramPacket>();
            let fd = socket(ctx.sock.cast())?.fd();
            let pkt = packet(ctx);
            let data = (&raw mut *pkt).wrapping_add(1).cast();
            let address = &mut pkt.socket_addr;
            address.len = libc::socklen_t::try_from(size_of::<AddressStorage>()).unwrap_or(0);
            let (storage, len) = ((&raw mut address.addr).cast(), &raw mut address.len);
            // SAFETY: the buffer's room after the packet head, and the address's storage.
            let n = unsafe { swoole_rs_reactor::fd_recvfrom(fd, data, room, 0, storage, len) };
            sign(i64::try_from(n).unwrap_or(-1))
        }
        OP_PACKET_TASK => {
            let pkt = packet(ctx);
            ctx.task.info.fd = if pkt.socket_type == SOCK_UDP {
                // SAFETY: a UDP datagram's peer is an inet4 address.
                let ip = unsafe { pkt.socket_addr.addr.inet_v4.sin_addr.s_addr };
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
            let len = size_of::<DgramPacket>() as u64 + arg;
            ctx.task.info.len = u32::try_from(len).unwrap_or(u32::MAX);
            ctx.task.data = ctx.pkt.cast();
            (&raw mut ctx.task).expose_provenance() as u64
        }
        OP_DESTROY_CALLBACK => {
            let queue = &raw mut handle.fields().destroy_callbacks;
            let push = swoole_rs_reactor::swr_v1_reactor_deferred_push;
            // SAFETY: the reactor's live destroy queue; the call takes the reactor, which
            // outlives it.
            if unsafe { push(queue, close_connections, reactor, None) } != 0 {
                std::alloc::handle_alloc_error(std::alloc::Layout::new::<usize>());
            }
            0
        }
        OP_CLOSE_FORWARD => {
            factory::end(state, arg.cast_signed(), CLOSE_ACTIVELY);
            0
        }
        OP_WR_SENDFILE => {
            // SAFETY: the context's live session socket.
            sign(unsafe { swoole_rs_tls::upstream::socket_handle_sendfile(ctx.sock) })
        }
        // SAFETY: as above.
        OP_WR_SEND => sign(unsafe { swoole_rs_tls::upstream::socket_handle_send(ctx.sock) }),
        OP_PC_NOTIFY => {
            let mut head = DataHead {
                reactor_id: i16::try_from(reactor_id()).unwrap_or_default(),
                // SAFETY: the context's live event.
                fd: c_long::from(unsafe { (*ctx.event).fd() }),
                kind: EVENT_CLOSE,
                ..DataHead::default()
            };
            // arg: peer_closed, the connection is no longer available
            conn_ref().peer_closed = u8::from(arg != 0);
            socket(ctx.sock.cast())?.edit(|s| s.close_notified = 1);
            u64::from(factory::notify(state, (&raw mut head).cast()))
        }
        OP_CC_COUNT => {
            use core::sync::atomic::Ordering::SeqCst;
            let gs = crate::shared::server_counters(state)?;
            // SAFETY: the context's live port, whose counters live as long as it.
            let port = unsafe { crate::port_config::of(ctx.port) }.counters()?;
            gs.close_count.fetch_add(1, SeqCst);
            port.close_count.fetch_add(1, SeqCst);
            // arg: base mode, which counts connections per reactor
            if arg == 0 {
                gs.connection_num.fetch_sub(1, SeqCst);
                port.connection_num.fetch_sub(1, SeqCst);
            } else {
                let id = usize::from(reactor_id());
                // SAFETY: base mode's per-reactor counts, one per reactor.
                unsafe { (*gs.cells().wrapping_add(id)).fetch_sub(1, SeqCst) };
                // SAFETY: as above.
                unsafe { (*port.cells().wrapping_add(id)).fetch_sub(1, SeqCst) };
            }
            0
        }
        OP_CC_HTTP_DESTROY => {
            (crate::port_config::methods().destroy_http_request)(conn);
            0
        }
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
            let worker: *mut WorkerRecord = worker(server, this(ctx).id);
            if op == OP_WORKER_START {
                // SAFETY: the live server and this thread's worker.
                unsafe { call_worker_start_callback(serv, worker.cast()) };
            } else {
                // SAFETY: as above.
                unsafe { call_worker_stop_callback(serv, worker.cast()) };
            }
            0
        }
        OP_STOP_THREAD_WORKER => {
            let worker: *mut WorkerRecord = worker(server, reactor_id());
            // SAFETY: the live server and this reactor's worker.
            unsafe { stop_async_worker(serv, worker.cast()) };
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
            this(ctx).message_bus().free_buffer();
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
            // SAFETY: `heartbeat_check` takes the reactor, which outlives its timers.
            let timer = unsafe { swr_v1_timer_add_plain(msec, true, heartbeat_check, reactor) };
            if op == OP_HEARTBEAT_TIMER {
                config().heartbeat_timer = timer;
            } else {
                this(ctx).heartbeat_timer = timer;
            }
            0
        }
        OP_RESUME_TIMER => {
            let msec = c_long::from(conn_ref().waiting_time);
            // SAFETY: `resume_data_receiving` takes the connection's socket, which
            // deletes the timer before it goes.
            let timer = unsafe {
                swr_v1_timer_add_plain(msec, false, resume_data_receiving, ctx.sock.cast())
            };
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
            unsafe { methods::init_pipe_sockets(server, bus_of(this(ctx))) };
            0
        }
        OP_RT_THREAD_WORKER => {
            let worker = worker(server, reactor_id());
            // SAFETY: the live server and this thread's worker.
            unsafe { crate::worker_methods::init_event_worker(server, worker) };
            sign(add_read(pipe_socket(this(ctx), worker.pipe_worker)));
            0
        }
        OP_RT_PIPE_COMMAND => {
            let pair = config()
                .pipe_command
                .cast::<swoole_rs_tls::socket_pair::SocketPair>();
            // SAFETY: the server's live command pipe.
            let fd = socket(unsafe { (*pair).worker_socket }.cast())?.fd();
            let thread = this(ctx);
            let bus = thread.message_bus();
            bus.init_pipe_socket(fd);
            thread.pipe_command = bus.pipe_fd(fd).cast();
            socket(thread.pipe_command.cast())?.edit(|s| s.output.buffer_size = u32::MAX);
            0
        }
        OP_RT_BUS_SETUP => {
            let thread = this(ctx);
            let gs = crate::shared::server_counters(state)?;
            // SAFETY: the shared counter outlives every reactor thread's bus.
            let counter = unsafe {
                &*gs.pipe_packet_msg_id
                    .as_ptr()
                    .cast::<core::sync::atomic::AtomicU64>()
            };
            thread.message_bus().rust().count_ids(counter);
            thread.message_bus().set_always_chunked();
            u64::from(
                thread
                    .message_bus()
                    .alloc_buffer(config().ipc_max_size as usize),
            )
        }
        OP_RT_ADD_WORKER_PIPE | OP_WORKER_PIPE_UNREAD => {
            let worker = worker(server, arg);
            let thread = this(ctx);
            let sock = pipe_socket(thread, worker.pipe_master);
            if op == OP_WORKER_PIPE_UNREAD {
                handle.interest(sock?, 4, 0);
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
            ctx.resp = thread.message_bus().buffer();
            let sock = event_socket(ctx)?;
            let read = crate::pool_native::read_bus(
                thread.message_bus().rust(),
                thread.allocator,
                sock,
                true,
            );
            sign(i64::try_from(read).unwrap_or(-1))
        }
        OP_PIPE_POP => {
            this(ctx).message_bus().pop();
            0
        }
        OP_COMMAND_REQUEST => {
            let thread = this(ctx);
            let id = u16::try_from(thread.id).unwrap_or_default();
            let pipe = socket(thread.pipe_command.cast());
            crate::commands::call_command_handler(server, thread.message_bus(), id, pipe);
            0
        }
        OP_COMMAND_RESPONSE | OP_PIPE_SEND => {
            let bus = this(ctx).message_bus();
            let packet = bus.packet();
            // SAFETY: the bus's buffer, which PIPE_READ stored.
            let mut info = unsafe { *resp };
            if op == OP_COMMAND_RESPONSE {
                let fd = info.fd;
                bus.with_packet(|reply| crate::commands::command_result(state, fd, reply));
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
            let front = out_front(ctx)?;
            ctx.front = front.data.cast();
            ctx.front_length = front.length as usize;
            0
        }
        OP_RT_SEND_FRONT => {
            let sock = event_socket(ctx)?.as_ptr();
            let (front, length) = (ctx.front.cast(), ctx.front_length);
            // SAFETY: the event's live socket and the front message's bytes.
            let sent = unsafe { swoole_rs_tls::upstream::socket_send(sock, front, length, 0) };
            sign(i64::try_from(sent).unwrap_or(-1))
        }
        OP_RT_CATCH_WAIT => {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            u64::from(event_socket(ctx)?.edit(|s| s.catch_error(errno)) == SW_WAIT)
        }

        // SAFETY: the live server, this thread's reactor and a live connection.
        OP_INCOMING => sign(unsafe { methods::connection_incoming(serv, reactor, conn) }),
        // SAFETY: this thread's reactor and the connection's live socket.
        OP_CLOSE => sign(unsafe { entry::close_connection(reactor, conn_ref().socket.cast()) }),
        OP_CONN_HEALTHY => u64::from(server.healthy(f64::from_bits(arg), conn_ref())),
        OP_SEND_CLOSE_FORCE => {
            // `serv->get_reactor_pipe_socket(session_id, conn->reactor_id)`.
            let session = arg.cast_signed();
            let pipes = i64::from(state.tables_ref().reactor_pipe_num);
            let index = session % pipes;
            let id = i64::from(conn_ref().reactor_id) + index * i64::from(config().reactor_num);
            // the session id, so the reactor verifies the connection before closing it
            send_event(
                worker(server, id).pipe_worker,
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
            let &port = state.tables_ref().ports().get(usize::try_from(arg).ok()?)?;
            if op == OP_PORT_LISTEN {
                return Some(sign((crate::port_config::methods().listen)(port)));
            }
            add_read(server.port_socket_at(usize::try_from(arg).unwrap_or(usize::MAX)));
            0
        }
        OP_DGRAM_BUFFER => {
            // support 64K packet
            let buffer = swoole_rs_corebase::thread_buffer().cast::<CppString>();
            // SAFETY: this thread's live buffer.
            let buffer = unsafe { &mut *buffer };
            buffer.extend(buffer.size().wrapping_mul(2));
            0
        }
        OP_RD_PORT_READ => {
            use core::sync::atomic::Ordering::SeqCst;
            let (event, port) = (ctx.event, ctx.port);
            conn_ref().last_recv_time = swoole_rs_corebase::microtime();
            // The event's socket, which the read may close.
            let sock = event_socket(ctx)?;
            let last_recv_bytes = sock.read(|s| s.total_recv_bytes);
            // SAFETY: the context's live port.
            let port_config = unsafe { crate::port_config::of(port) };
            let on_read = port_config.on_read;
            // SAFETY: `init_protocol` set `int (*onRead)(Reactor *, ListenPort *, Event *)`.
            let on_read: unsafe extern "C-unwind" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> c_int = unsafe { core::mem::transmute(on_read) };
            // SAFETY: this thread's reactor, the port and its event.
            let retval = unsafe { on_read(reactor, port, event.cast()) };
            // Upstream reads the socket after the read too.
            let received = sock
                .read(|s| s.total_recv_bytes)
                .wrapping_sub(last_recv_bytes);
            if received.cast_signed() > 0 {
                let received = received as u64;
                if let Some(gs) = port_config.counters() {
                    gs.total_recv_bytes.fetch_add(received, SeqCst);
                }
                if let Some(gs) = crate::shared::server_counters(state) {
                    gs.total_recv_bytes.fetch_add(received, SeqCst);
                }
            }
            let mut result = u64::from(retval.cast_unsigned());
            let conn = conn_ref();
            if conn.active != 0 {
                let max = config().max_queued_bytes;
                let flag = |set: bool, bit: u64| if set { bit } else { 0 };
                // An active connection's socket.
                let notified = sock.read(|s| s.close_notified != 0);
                result |= crate::conn_io::ACTIVE
                    | flag(notified, crate::conn_io::READ_NOTIFIED)
                    | flag(
                        max != 0 && conn.recv_queued_bytes.load(SeqCst) > max,
                        crate::conn_io::QUEUE_OVER,
                    );
            }
            result
        }
        OP_ON_RECEIVE | OP_ON_PACKET => {
            let callback = if op == OP_ON_PACKET {
                CB_PACKET
            } else {
                CB_RECEIVE
            };
            let slot = state.callbacks.slot(callback)?;
            // SAFETY: the slot's `int (Server *, RecvData *)` callback over the frame at arg.
            u64::from(unsafe { slot.receive(serv, arg as *mut c_void) } == 0)
        }
        OP_FACTORY_DISPATCH => u64::from(factory::dispatch(state, (&raw mut ctx.task).cast())),
        _ => return None,
    })
}

/// The datagram `OP_PACKET_INIT` points the context at.
fn packet<'a>(ctx: &ReactorContext) -> &'a mut DgramPacket {
    // SAFETY: this thread's buffer, which holds a packet head and its datagram.
    unsafe { &mut *ctx.pkt.cast::<DgramPacket>() }
}

/// The destroy callback of a reactor thread's reactor: close its connections.
unsafe extern "C-unwind" fn close_connections(reactor: *mut c_void) {
    // SAFETY: the callback's live reactor, on its own thread.
    let serv = unsafe { crate::reactor_handle::Reactor::of(reactor) }
        .fields()
        .ptr;
    let mut ctx = ReactorContext::new(serv, reactor);
    // SAFETY: the reactor's live server.
    unsafe { run(RUN_CLOSE_REACTOR_CONNECTIONS, 0, &mut ctx) };
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
    // Else a process-directed signal (the shutdown's SIGTERM) can land here rather
    // than on the master's signalfd, and its default action kills the master.
    swoole_rs_corebase::signal_block_all();
    // SAFETY: the live server.
    let state = unsafe { view::Server::from_raw(serv) }.state();
    let reactors = state.config_ref().reactor_num;
    // upstream sets THREAD_HEARTBEAT, then overwrites the type with this.
    let kind = (reactors.wrapping_add(1) & 0xff) as u8;
    swoole_rs_corebase::set_thread(kind, swoole_rs_corebase::thread_id());
    let mut ctx = ReactorContext::new(serv, ptr::null_mut());
    // SAFETY: the live server.
    unsafe { run(RUN_HEARTBEAT_THREAD, 0, &mut ctx) };
}

/// `Server::heartbeat_check`: the heartbeat timer of `reactor`.
unsafe extern "C-unwind" fn heartbeat_check(reactor: *mut c_void) {
    // SAFETY: the timer's live reactor, on its own thread.
    let handle = unsafe { crate::reactor_handle::Reactor::of(reactor) };
    let fields = handle.fields();
    let mut ctx = ReactorContext::new(fields.ptr, reactor);
    // SAFETY: the reactor's live server.
    ctx.thread = thread(
        unsafe { view::Server::from_raw(fields.ptr) }.state(),
        usize::from(fields.id),
    );
    // SAFETY: as above.
    unsafe { run(RUN_HEARTBEAT_CHECK, 0, &mut ctx) };
}

/// `ReactorThread_resume_data_receiving`: the timer of a paused session socket.
unsafe extern "C-unwind" fn resume_data_receiving(sock: *mut c_void) {
    // The timer's live session socket, whose object is its connection.
    let Some(conn) = socket(sock).map(|s| s.read(|s| s.object.cast::<ConnectionRecord>())) else {
        return;
    };
    let sock = sock.cast::<SocketRecord>();
    let mut ctx = ReactorContext::new(swoole_rs_corebase::swr_v1_core_server(), ptr::null_mut());
    ctx.sock = sock;
    ctx.conn = conn;
    // SAFETY: the live server.
    unsafe { run(RUN_RESUME_RECEIVING, 0, &mut ctx) };
}
