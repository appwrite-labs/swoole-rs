//! The reactor-thread ops over a thread's own state that were master.cc's: its
//! entry and setup, its message bus and worker pipes, the messages workers send
//! it, its timers and the server methods it calls. Each is a Rust call: the bus
//! is crates/bus', the methods are this crate's and crates/process' and
//! crates/ports', reached under their upstream symbols.
use core::ffi::{c_int, c_long, c_void};
use core::ptr;

use crate::objects::worker_pipe;
use crate::reactor_handle::Reactor;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};
use swoole_rs_process::worker_record::Worker;
use swoole_rs_reactor::Socket;
use swoole_rs_timer::swr_v1_timer_add_plain;

use crate::base_factory::SendData;
use crate::callbacks::{CB_FINISH, CB_PACKET, CB_PIPE_MESSAGE, CB_RECEIVE};
use crate::event_worker_ops::{
    call_worker_start_callback, call_worker_stop_callback, stop_async_worker,
};
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
use crate::reactor_ops::{ReactorContext, ThreadRecord, int, sign, with_thread_buffer};
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
fn out_front(ctx: &ReactorContext<'_>) -> Option<swoole_rs_buffer::TypedFront> {
    let front = swoole_rs_buffer::cpp::swr_cpp_buffer_typed_front;
    event_socket(ctx)?.read(|s| crate::reactor_ops::out_buffer(s).map(|b| front(b)))
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

/// The context's port settings.
fn port_config<'a>(ctx: &ReactorContext<'a>) -> Option<&'a crate::port_config::PortConfig> {
    ctx.port.map(view::Port::config)
}

/// The context event's socket.
fn event_socket<'c>(ctx: &'c ReactorContext<'_>) -> Option<Socket<'c>> {
    ctx.event.as_deref()?.socket()
}

/// `thread->message_bus.get_pipe_socket(sock)`.
fn pipe_socket<'b>(bus: &'b crate::objects::Bus, pipe: Option<Socket<'_>>) -> Option<Socket<'b>> {
    bus.pipe_socket(pipe?)
}

/// The server's worker `id`, if it has one.
fn worker(serv: view::Server<'_>, id: impl TryInto<u16>) -> Option<Worker<'_>> {
    serv.worker_record(id.try_into().unwrap_or(u16::MAX))
}

/// `socket->send_sync(&head, sizeof(head))` of a bare event.
fn send_event(sock: Option<Socket<'_>>, fd: c_long, kind: u32) {
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

/// Answers `op` when it is one of this module's, else `None`, as `reactor_ops::answer`.
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one arm per op, as master.cc's switch was"
)]
#[inline(always)]
pub(crate) fn answer(
    ctx: &mut ReactorContext<'_>,
    state: &ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    let (server, handle) = (ctx.serv, ctx.reactor);
    let (serv, reactor) = (server.raw(), handle.map_or(ptr::null_mut(), Reactor::ptr));
    let config = || state.config_ref();
    let reactor_id = || handle.map(|handle| handle.handle().id());
    // `reactor->add(socket, SW_EVENT_READ)`.
    let add_read = |socket: Option<Socket<'_>>| match (socket, handle) {
        (Some(socket), Some(handle)) => handle.change(socket, 0, SW_EVENT_READ),
        _ => -1,
    };
    let resp = ctx.resp;
    Some(match op {
        OP_REACTOR_BARRIER => {
            methods::reactor_barrier(server);
            0
        }
        OP_FORCE_CLOSE => {
            let conn = ctx.connection()?;
            conn.set_close_force(1);
            let mut event = conn.event(0, 0);
            handle?.trigger_close(&mut event);
            0
        }
        OP_PACKET_INIT => {
            let fd = event_fd(ctx)?;
            // The event's fd is a listening socket's, whose slot is live.
            let conn = state.connection(fd.into(), 0)?;
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
        OP_DESTROY_CALLBACK => {
            let running = handle?.handle();
            // SAFETY: the call takes the reactor, which outlives it.
            unsafe { swoole_rs_reactor::defer_destroy(running, close_connections, reactor) };
            0
        }
        OP_CLOSE_FORWARD => {
            factory::end(state, arg.cast_signed(), CLOSE_ACTIVELY);
            0
        }
        OP_WR_SENDFILE => sign(swoole_rs_tls::upstream::socket_handle_sendfile(ctx.sock?)),
        OP_WR_SEND => sign(swoole_rs_tls::upstream::socket_handle_send(ctx.sock?)),
        OP_PC_NOTIFY => {
            let mut head = DataHead {
                reactor_id: i16::try_from(reactor_id()?).unwrap_or_default(),
                fd: c_long::from(event_fd(ctx)?),
                kind: EVENT_CLOSE,
                ..DataHead::default()
            };
            // arg: peer_closed, the connection is no longer available
            ctx.connection()?.set_peer_closed(u8::from(arg != 0));
            ctx.sock?.edit(|s| s.close_notified = 1);
            u64::from(factory::notify(state, (&raw mut head).cast()))
        }
        OP_CC_COUNT => {
            use core::sync::atomic::Ordering::SeqCst;
            let gs = crate::shared::server_counters(state)?;
            let port = port_config(ctx)?.counters()?;
            gs.close_count.fetch_add(1, SeqCst);
            port.close_count.fetch_add(1, SeqCst);
            // arg: base mode, which counts connections per reactor
            if arg == 0 {
                gs.connection_num.fetch_sub(1, SeqCst);
                port.connection_num.fetch_sub(1, SeqCst);
            } else {
                let id = usize::from(reactor_id()?);
                for cells in [gs.cells(), port.cells()] {
                    // SAFETY: base mode's per-reactor counts, one per reactor.
                    unsafe { &*cells.wrapping_add(id) }.fetch_sub(1, SeqCst);
                }
            }
            0
        }
        OP_CC_HTTP_DESTROY => {
            (crate::port_config::methods().destroy_http_request)(ctx.conn);
            0
        }
        OP_THREAD_ENTER => {
            ctx.thread = thread(state, usize::try_from(arg).ok()?);
            ctx.thread_record()?.id.set(int(arg));
            swoole_rs_corebase::set_thread_message_bus(bus_of(ctx)?);
            0
        }
        OP_REACTOR_THREAD_TYPE => {
            swoole_rs_corebase::set_thread(THREAD_REACTOR, u16::try_from(arg & 0xffff).ok()?);
            0
        }
        OP_WORKER_START | OP_WORKER_STOP => {
            let worker = worker(server, ctx.thread_record()?.id.get())?;
            if op == OP_WORKER_START {
                call_worker_start_callback(server, worker);
            } else {
                call_worker_stop_callback(server, worker);
            }
            0
        }
        OP_STOP_THREAD_WORKER => {
            stop_async_worker(server, worker(server, reactor_id()?)?);
            0
        }
        OP_THREAD_INIT | OP_THREAD0_INIT => {
            if op == OP_THREAD0_INIT {
                ctx.thread = thread(state, 0);
            }
            let id = u16::try_from(ctx.thread_record()?.id.get()).unwrap_or_default();
            // `ReactorThread::init(serv, reactor, id)`, over the context's thread and
            // its new reactor.
            let mut init = ReactorContext::new(server, handle);
            init.thread = ctx.thread;
            let ret = entry::int(run(crate::ops::RUN_RT_INIT, u64::from(id), &mut init));
            sign(if op == OP_THREAD0_INIT { 0 } else { ret })
        }
        OP_THREAD_CLEAN => {
            // `message_bus.free_buffer()`.
            ctx.thread_record()?.message_bus().free_buffer();
            0
        }
        OP_THREAD_NOTIFY => {
            let joining = thread(state, usize::try_from(arg).ok()?).cast::<ThreadRecord>();
            // SAFETY: one of the server's live threads; only its field is read, as the
            // thread itself may hold its record meanwhile.
            let notify = unsafe { (*joining).notify_pipe.get() };
            // SAFETY: null or the thread's notify pipe, which it keeps until the server
            // joins it, after this notice.
            send_event(unsafe { Socket::from_ptr(notify) }, 0, EVENT_SHUTDOWN);
            0
        }
        OP_REACTOR_THREAD_ENTRY => reactor_thread_entry as *const () as u64,
        OP_HEARTBEAT_ENTRY => heartbeat_main as *const () as u64,
        OP_HEARTBEAT_TIMER | OP_RT_HEARTBEAT_TIMER => {
            let msec = c_long::from(config().heartbeat_check_interval.get()) * 1000;
            // SAFETY: `heartbeat_check` takes the reactor, which outlives its timers.
            let timer = unsafe { swr_v1_timer_add_plain(msec, true, heartbeat_check, reactor) };
            if op == OP_HEARTBEAT_TIMER {
                config()
                    .heartbeat_timer
                    .store(timer, core::sync::atomic::Ordering::Relaxed);
            } else {
                ctx.thread_record()?.heartbeat_timer.set(timer);
            }
            0
        }
        OP_RESUME_TIMER => {
            let msec = c_long::from(ctx.connection()?.waiting_time());
            // SAFETY: `resume_data_receiving` takes the connection's socket, which
            // deletes the timer before it goes.
            let timer = unsafe {
                swr_v1_timer_add_plain(
                    msec,
                    false,
                    resume_data_receiving,
                    ctx.sock?.as_ptr().cast(),
                )
            };
            ctx.connection()?.set_timer(timer);
            u64::from(!timer.is_null())
        }

        OP_RT_INIT_REACTOR => {
            entry::init_reactor(server, handle?);
            0
        }
        OP_RT_INIT_PIPE_SOCKETS => {
            // SAFETY: the live server and this thread's bus.
            unsafe { methods::init_pipe_sockets(server, bus_of(ctx)?) };
            0
        }
        OP_RT_THREAD_WORKER => {
            let worker = worker(server, reactor_id()?)?;
            crate::worker_methods::init_event_worker(server, worker);
            let bus = ctx.thread_record()?.message_bus();
            sign(add_read(pipe_socket(&bus, worker_pipe(worker, false))));
            0
        }
        OP_RT_PIPE_COMMAND => {
            // The server's live command pipe.
            let fd = config().command_pipe()?.worker_end()?.fd();
            let thread = ctx.thread_record()?;
            let bus = thread.message_bus();
            bus.init_pipe_socket(fd);
            thread.pipe_command.set(bus.pipe_fd(fd).cast());
            bus.rust()
                .pipe(fd)?
                .edit(|s| s.output.buffer_size = u32::MAX);
            0
        }
        OP_RT_BUS_SETUP => {
            let thread = ctx.thread_record()?;
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
                    .alloc_buffer(config().ipc_max_size() as usize),
            )
        }
        OP_RT_ADD_WORKER_PIPE | OP_WORKER_PIPE_UNREAD => {
            let Some(worker) = worker(server, arg) else {
                return Some(sign(-1));
            };
            let thread = ctx.thread_record()?;
            let bus = thread.message_bus();
            let sock = pipe_socket(&bus, worker_pipe(worker, true));
            if op == OP_WORKER_PIPE_UNREAD {
                handle?.interest(sock?, 4, 0);
                return Some(0);
            }
            if add_read(sock) < 0 {
                return Some(sign(-1));
            }
            // Only synchronous sends go to the notify pipe: thread-safe, no separate memory.
            if thread.notify_pipe.get().is_null() {
                thread.notify_pipe.set(worker.pipe_worker().cast());
            }
            0
        }

        OP_PIPE_READ => {
            let thread = ctx.thread_record()?;
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
            ctx.thread_record()?.message_bus().pop();
            0
        }
        OP_COMMAND_REQUEST => {
            let thread = ctx.thread_record()?;
            let id = u16::try_from(thread.id.get()).unwrap_or_default();
            // The bus's record of the command pipe (`thread.pipe_command`).
            let bus = thread.message_bus();
            let end = config()
                .command_pipe()
                .and_then(swoole_rs_tls::socket_pair::SocketPair::worker_end);
            let pipe = end.and_then(|end| bus.pipe_socket(end));
            crate::commands::call_command_handler(server, bus, id, pipe);
            0
        }
        OP_COMMAND_RESPONSE | OP_PIPE_SEND => {
            let bus = ctx.thread_record()?.message_bus();
            let packet = bus.packet();
            // The bus's buffer, which PIPE_READ stored.
            let mut info = *ctx.message()?;
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
            // SAFETY: a send over the bus's packet.
            unsafe { methods::send_to_connection(server, &send) };
            0
        }
        OP_ON_FINISH => {
            let slot = state.callbacks.slot(CB_FINISH)?;
            // SAFETY: the slot's `int (Server *, EventData *)` callback; its result is discarded.
            let _ = unsafe { slot.receive(serv, resp.cast_mut().cast()) };
            0
        }
        OP_ON_PIPE_MESSAGE => {
            let slot = state.callbacks.slot(CB_PIPE_MESSAGE)?;
            // SAFETY: the slot's `void (Server *, EventData *)` callback over the message.
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
            let sock = event_socket(ctx)?;
            let (front, length) = (ctx.front.cast(), ctx.front_length);
            // SAFETY: the event's live socket and the front message's bytes.
            let sent = unsafe { swoole_rs_tls::upstream::socket_send(sock, front, length, 0) };
            sign(i64::try_from(sent).unwrap_or(-1))
        }
        OP_RT_CATCH_WAIT => {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            u64::from(event_socket(ctx)?.catch_error(errno) == SW_WAIT)
        }

        OP_INCOMING => {
            let conn = ctx.connection()?;
            // SAFETY: the live server, this thread's reactor and a live connection.
            sign(unsafe { methods::connection_incoming(server, reactor, conn) })
        }
        OP_CLOSE => {
            let socket = ctx.connection()?.socket_ptr().cast();
            // SAFETY: this thread's reactor and the connection's live socket.
            sign(unsafe { entry::close_connection(reactor, socket) })
        }
        OP_CONN_HEALTHY => u64::from(server.healthy(f64::from_bits(arg), ctx.connection()?)),
        OP_SEND_CLOSE_FORCE => {
            // `serv->get_reactor_pipe_socket(session_id, conn->reactor_id)`.
            let session = arg.cast_signed();
            let pipes = i64::from(state.tables_ref().reactor_pipe_num);
            let index = session % pipes;
            let id = i64::from(ctx.connection()?.reactor_id())
                + index * i64::from(config().reactor_num.get());
            // the session id, so the reactor verifies the connection before closing it
            send_event(
                worker_pipe(worker(server, id)?, false),
                session,
                crate::ops::EVENT_CLOSE_FORCE,
            );
            0
        }
        OP_RD_ABORT => {
            let port = ctx.port.map_or(ptr::null_mut(), view::Port::raw);
            let sock = ctx
                .sock
                .map_or(ptr::null_mut(), |sock| sock.as_ptr().cast());
            // SAFETY: the live server, this thread's reactor, the port and its socket.
            unsafe { methods::abort_connection(serv, reactor, port, sock) };
            0
        }
        OP_PORT_LISTEN | OP_PORT_READ => {
            let port = state.tables_ref().ports().get(usize::try_from(arg).ok()?)?;
            if op == OP_PORT_LISTEN {
                return Some(sign((crate::port_config::methods().listen)(port.raw())));
            }
            add_read(server.port_socket_at(usize::try_from(arg).unwrap_or(usize::MAX)));
            0
        }
        OP_DGRAM_BUFFER => {
            // support 64K packet
            with_thread_buffer(|buffer| buffer.extend(buffer.size().wrapping_mul(2)));
            0
        }
        OP_RD_PORT_READ => {
            use core::sync::atomic::Ordering::SeqCst;
            let event = ptr::from_mut(ctx.event.as_deref_mut()?);
            let port = ctx.port?.raw();
            ctx.connection()?
                .set_last_recv_time(swoole_rs_corebase::microtime());
            // The event's socket, which the read may close.
            let sock = event_socket(ctx)?;
            let last_recv_bytes = sock.read(|s| s.total_recv_bytes);
            let port_config = port_config(ctx)?;
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
            let (total_recv_bytes, notified) =
                sock.read(|s| (s.total_recv_bytes, s.close_notified != 0));
            let received = total_recv_bytes.wrapping_sub(last_recv_bytes);
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
            let conn = ctx.connection()?;
            if conn.is_active() {
                let max = config().max_queued_bytes.get();
                let flag = |set: bool, bit: u64| if set { bit } else { 0 };
                result |= crate::conn_io::ACTIVE
                    | flag(notified, crate::conn_io::READ_NOTIFIED)
                    | flag(
                        max != 0 && conn.recv_queued_bytes().load(SeqCst) > max,
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
fn packet<'c>(ctx: &'c mut ReactorContext<'_>) -> Option<&'c mut DgramPacket> {
    // SAFETY: null, or this thread's buffer, which holds a packet head and its datagram.
    unsafe { ctx.pkt.cast::<DgramPacket>().as_mut() }
}

/// The destroy callback of a reactor thread's reactor: close its connections.
unsafe extern "C-unwind" fn close_connections(reactor: *mut c_void) {
    // SAFETY: the callback's live reactor, on its own thread: a reactor thread's
    // reactor, whose `ptr` is its live server.
    let mut ctx = unsafe { entry::reactor_context(reactor) };
    run(RUN_CLOSE_REACTOR_CONNECTIONS, 0, &mut ctx);
}

/// What a reactor thread Rust starts runs.
unsafe extern "C" fn reactor_thread_entry(serv: *mut c_void, id: u64) {
    swoole_rs_corebase::swoole_thread_init(false);
    let id = c_int::try_from(id).unwrap_or_default();
    // SAFETY: the live server, on its new reactor thread.
    entry::reactor_thread_main_loop(unsafe { view::Server::from_raw(serv) }, id);
    swoole_rs_corebase::swoole_thread_clean(false);
}

/// What the heartbeat thread Rust starts runs, its signals blocked.
unsafe extern "C" fn heartbeat_main(serv: *mut c_void, _: u64) {
    // Else a process-directed signal (the shutdown's SIGTERM) can land here rather
    // than on the master's signalfd, and its default action kills the master.
    swoole_rs_corebase::signal_block_all();
    // SAFETY: the live server.
    let server = unsafe { view::Server::from_raw(serv) };
    let reactors = server.state().config_ref().reactor_num.get();
    // upstream sets THREAD_HEARTBEAT, then overwrites the type with this.
    let kind = (reactors.wrapping_add(1) & 0xff) as u8;
    swoole_rs_corebase::set_thread(kind, swoole_rs_corebase::thread_id());
    run(
        RUN_HEARTBEAT_THREAD,
        0,
        &mut ReactorContext::new(server, None),
    );
}

/// `Server::heartbeat_check`: the heartbeat timer of `reactor`.
unsafe extern "C-unwind" fn heartbeat_check(reactor: *mut c_void) {
    // SAFETY: the timer's live reactor, on its own thread: a server reactor, whose
    // `ptr` is its live server.
    let mut ctx = unsafe { entry::reactor_context(reactor) };
    if let Some(reactor) = ctx.reactor {
        ctx.thread = thread(ctx.serv.state(), usize::from(reactor.handle().id()));
    }
    run(RUN_HEARTBEAT_CHECK, 0, &mut ctx);
}

/// `ReactorThread_resume_data_receiving`: the timer of a paused session socket.
unsafe extern "C-unwind" fn resume_data_receiving(sock: *mut c_void) {
    // SAFETY: the timer's live session socket, whose object is its connection.
    let Some(socket) = (unsafe { Socket::from_ptr(sock.cast()) }) else {
        return;
    };
    // SAFETY: the server whose socket's timer this is, live while its reactor runs
    // the timer; the view ends with it.
    let Some(serv) = (unsafe { view::Server::current() }) else {
        return;
    };
    let mut ctx = ReactorContext::new(serv, None);
    ctx.conn = socket.read(|s| s.object.cast::<ConnectionRecord>());
    ctx.sock = Some(socket);
    run(RUN_RESUME_RECEIVING, 0, &mut ctx);
}
