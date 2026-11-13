//! The reactor-thread ops (master.cc's `reactor_thread_call`) that are a log line
//! or a read or write of a record Rust already lays out: the server's config,
//! tables and callbacks, the reactor's fields and the context's connection,
//! socket, event and pipe message. Rust answers them before any native call, so
//! master.cc no longer has a case for them. Logs name the upstream function that
//! wrote them. Its effects over the reactor, the context's sockets and the
//! server's counters are Rust calls too.
use crate::records::Lookup;
use core::cell::Cell;
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering::SeqCst;

use swoole_rs_buffer::cpp::CppBuffer;
use swoole_rs_buffer::string::with_thread_buffer;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_reactor::{Event, Socket, SocketFields, SocketRecord};
use swoole_rs_tls::upstream as tls;
use swoole_rs_tls::{OwnedTlsFields, OwnedTlsPrepared, swr_v1_tls_owner_view};

use crate::base_factory::SendData;
use crate::conn_io as cn;
use crate::ops::OP_PORT_DTLS;
use crate::ops::{
    LOG_RECVFROM, OP_CERT_TASK, OP_CLIENT_CERT, OP_CONN_AT, OP_CONN_FD, OP_CONN_REACTOR,
    OP_HAVE_DGRAM, OP_PEER_CERT, OP_QUEUE_SUB, OP_RD_WRITE, OP_REACTOR_ID, OP_REACTOR_NUM,
    OP_SERVER, OP_SESSION_CONN, OP_SET_WAITING_TIME, OP_SSL_READY, OP_SSL_VERIFY, OP_VERIFY_PEER,
    OP_WORKER_NUM,
};
use crate::ops::{
    OP_CONN_UNREAD, OP_NOTIFY_CONNECT, OP_PORT_DEL, OP_RD_UNREAD, OP_REACTOR_DEL, OP_SSL_ACCEPT,
};
use crate::port_config::{self, PortConfig};
use crate::reactor_handle::Reactor;
use crate::records::{Connection, ConnectionRecord};
use crate::view;
use crate::{ServerConfig, ServerState};

/// `PREQUEUE` in builds with `--fix-halfclose` (`SW_HALFCLOSE_PREQUEUE`), else nothing.
const BUILT_PREQUEUE: u64 = if cfg!(feature = "halfclose") {
    cn::PREQUEUE
} else {
    0
};

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;
/// `SW_SERVER_EVENT_CONNECT`.
const EVENT_CONNECT: c_int = 5;

/// The objects a reactor-thread op acts on: a live server, the reactor the op runs
/// under, and what the ops before it found. The entry points (`reactor_entry`) build
/// it. Each raw field is null or what the op setting it found, live while the context
/// is: one of the server's reactor threads, one of its connection slots, a worker's
/// message in this thread's bus, this thread's buffer. Its readers only test for null.
pub(crate) struct ReactorContext<'a> {
    pub(crate) serv: view::Server<'a>,
    pub(crate) reactor: Option<Reactor>,
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops read it")
    )]
    pub(crate) thread: *mut c_void,
    pub(crate) event: Option<&'a mut Event>,
    pub(crate) port: Option<view::Port<'a>>,
    pub(crate) sock: Option<Socket<'a>>,
    pub(crate) conn: *mut ConnectionRecord,
    /// The `PipeBuffer` read from a worker, which starts with its head.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops read it")
    )]
    pub(crate) resp: *const DataHead,
    /// The datagram `OP_PACKET_INIT` reads into: this thread's buffer.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops read it")
    )]
    pub(crate) pkt: *mut c_void,
    pub(crate) task: SendData,
}

impl<'a> ReactorContext<'a> {
    /// A context over `serv` and `reactor` with nothing else set.
    pub(crate) fn new(serv: view::Server<'a>, reactor: Option<Reactor>) -> Self {
        let null = core::ptr::null_mut();
        Self {
            serv,
            reactor,
            thread: null,
            event: None,
            port: None,
            sock: None,
            conn: null.cast(),
            resp: core::ptr::null(),
            pkt: null,
            task: SendData {
                info: DataHead::default(),
                data: core::ptr::null(),
            },
        }
    }

    /// The context's reactor thread, once an op has set it.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops call it")
    )]
    pub(crate) fn thread_record(&self) -> Option<&'a ThreadRecord> {
        // SAFETY: null or one of the server's live reactor threads, as the struct
        // says, which only its own thread touches.
        unsafe { self.thread.cast::<ThreadRecord>().as_ref() }
    }

    /// The context's connection, once an op has set it.
    pub(crate) fn connection(&self) -> Option<Connection<'a>> {
        // SAFETY: null or one of the server's live connection slots, as the struct says.
        unsafe { Connection::from_ptr(self.conn) }
    }

    /// The worker's message the pipe read stored.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops call it")
    )]
    pub(crate) fn message(&self) -> Option<&'a DataHead> {
        // SAFETY: null or the head of this thread's bus buffer, as the struct says.
        unsafe { self.resp.as_ref() }
    }
}

/// The low 32 bits of `arg`, as native code passes a C `int`.
pub(crate) fn int(arg: u64) -> c_int {
    u32::try_from(arg & u64::from(u32::MAX))
        .unwrap_or_default()
        .cast_signed()
}

pub(crate) fn sign(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// `Server::is_stream_event`.
pub(crate) fn stream_event(kind: u8) -> bool {
    // RECV_DATA, SEND_DATA, SEND_FILE, CLOSE, CONNECT, PAUSE_RECV, RESUME_RECV,
    // BUFFER_FULL and BUFFER_EMPTY.
    matches!(kind, 0 | 2..=5 | 11..=14)
}

pub(crate) use swoole_rs_ossupport::pthread_self;

/// A socket's output buffer, or `None` without one.
pub(crate) fn out_buffer(socket: &SocketFields) -> Option<&CppBuffer> {
    // SAFETY: a socket's output buffer is null or the live `Buffer` it owns.
    unsafe { socket.output.out_buffer.cast::<CppBuffer>().as_ref() }
}

/// The context's objects, each `None` until an op sets it.
struct Objects<'a, 'c> {
    ctx: &'a mut ReactorContext<'c>,
    state: &'a ServerState,
}

impl<'c> Objects<'_, 'c> {
    /// The context's live server.
    fn server(&self) -> view::Server<'c> {
        self.ctx.serv
    }
    fn config(&self) -> &ServerConfig {
        self.state.config_ref()
    }
    fn conn(&self) -> Option<Connection<'c>> {
        self.ctx.connection()
    }
    fn conn_socket(&self) -> Option<Socket<'c>> {
        self.conn()?.socket()
    }
    fn sock(&self) -> Option<Socket<'c>> {
        self.ctx.sock
    }
    /// The connection of the context's socket, which the ops reading it run over a
    /// session socket for.
    fn sock_conn(&self) -> Option<Connection<'c>> {
        let conn = self.sock()?.read(|s| s.object.cast::<ConnectionRecord>());
        // SAFETY: a session socket's object is null or its live connection.
        unsafe { Connection::from_ptr(conn) }
    }
    /// `read` over the context's socket, or the default without one.
    fn read_sock<R: Default>(&self, read: impl FnOnce(&SocketFields) -> R) -> R {
        self.sock().map(|sock| sock.read(read)).unwrap_or_default()
    }
    fn event(&self) -> Option<&Event> {
        self.ctx.event.as_deref()
    }
    /// The context's live reactor.
    fn handle(&self) -> Option<Reactor> {
        self.ctx.reactor
    }
    /// The context's live reactor, which only this thread touches.
    fn reactor(&self) -> Option<swoole_rs_reactor::Running<'static>> {
        self.ctx.reactor.map(Reactor::handle)
    }
    fn port(&self) -> Option<&'c PortConfig> {
        self.ctx.port.map(view::Port::config)
    }
    /// The TLS port's `SSLContext` view.
    fn ssl(&self) -> Option<OwnedTlsPrepared> {
        let context = self.port()?.ssl_context.as_deref()?;
        Some(swr_v1_tls_owner_view(context))
    }
    fn ssl_fields(&self) -> Option<&'c OwnedTlsFields> {
        self.port()?.ssl_fields()
    }
    /// `get_connection(fd)` or a verified session's.
    fn connection(&self, key: c_long, kind: Lookup) -> *mut ConnectionRecord {
        self.state.connection_ptr(key, kind)
    }
    /// `get_port_by_fd(fd)`: the port whose listening socket accepted `fd`.
    fn port_by_fd(&self, fd: c_int) -> Option<view::Port<'c>> {
        self.server().port_by_fd(fd)
    }
    fn mode(&self) -> u64 {
        u64::from(self.config().mode_)
    }
}

/// `bit` when `set`.
fn flag(set: bool, bit: u64) -> u64 {
    if set { bit } else { 0 }
}

/// `SW_SSL_STATE_READY`.
const SSL_READY: u32 = 1;

/// `Socket::is_dgram(type)`.
pub(crate) fn is_dgram(kind: c_int) -> bool {
    matches!(kind, 2 | 4 | 6)
}

/// `ReactorThread_onRead`'s facts over the context's connection; sets the context's
/// port. A session socket that is a datagram is a DTLS port's (UDP ports take
/// their packets on the listening socket).
fn read_facts(o: &mut Objects<'_, '_>) -> u64 {
    let (Some(conn), Some(fd)) = (o.conn(), o.event().map(Event::fd)) else {
        return 0;
    };
    if conn.server_fd() == 0 {
        return 0;
    }
    o.ctx.port = o.port_by_fd(fd);
    let process = o.mode() == crate::MODE_PROCESS;
    o.read_sock(|sock| {
        let ssl = !sock.ssl().is_null();
        cn::FOUND
            | BUILT_PREQUEUE
            | flag(sock.close_notified, cn::CLOSE_NOTIFIED)
            | flag(conn.close_queued(), cn::CLOSE_QUEUED)
            | flag(ssl && sock.tls.want_read, cn::SSL_WANT_READ)
            | flag(ssl && sock.tls.state != SSL_READY, cn::SSL_PENDING)
            | flag(is_dgram(sock.socket_type), cn::DTLS_PORT)
            | flag(sock.dtls, cn::DTLS_SOCKET)
            | flag(process, cn::PROCESS)
    })
}

/// The facts the read's entry passes its run.
#[inline(always)]
pub(crate) fn facts(ctx: &mut ReactorContext<'_>, state: &ServerState) -> u64 {
    read_facts(&mut Objects { ctx, state })
}

/// `Server::is_valid_connection`.
pub(crate) fn valid(conn: Option<Connection<'_>>) -> bool {
    let Some(conn) = conn.filter(|conn| conn.is_active()) else {
        return false;
    };
    conn.socket()
        .is_some_and(|socket| socket.read(|s| s.fd_type == FD_SESSION))
}

/// Answers `op` when it is one of this module's, else `None`, as an op does when the
/// context lacks an object it reads. Always inlined: each caller's constant op folds
/// the match, so the hot runners' other ops cost nothing.
#[inline(always)]
pub(crate) fn answer(
    ctx: &mut ReactorContext<'_>,
    state: &ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    let mut o = Objects { ctx, state };
    Some(match op {
        LOG_RECVFROM => {
            let m = format_args!("recvfrom({}) failed", o.event()?.fd());
            log::sys_warning(c"ReactorThread_onPacketReceived", m);
            0
        }
        OP_SSL_READY => {
            o.sock_conn()?.set_ssl_ready(true);
            0
        }
        OP_CLIENT_CERT => {
            // `client_cert_file`'s slot.
            let slot = o.ssl()?.strings[7];
            // SAFETY: a stable slot of the port's live owner.
            u64::from(!unsafe { &*slot }.bytes().is_empty())
        }
        OP_VERIFY_PEER => u64::from(o.ssl_fields()?.verify_peer),
        OP_PEER_CERT => {
            let buffer = swoole_rs_corebase::thread_buffer();
            // SAFETY: a TLS socket's live record and this thread's live buffer.
            u64::from(unsafe { tls::socket_ssl_get_peer_certificate_string(o.sock()?, buffer) })
        }
        OP_SSL_VERIFY => {
            let allow_self_signed = o.ssl_fields()?.allow_self_signed;
            u64::from(
                o.sock()
                    .is_some_and(|sock| tls::socket_ssl_verify(sock, allow_self_signed)),
            )
        }
        OP_CERT_TASK => {
            // OP_PEER_CERT filled this thread's live buffer.
            let (len, data) = with_thread_buffer(|buffer| (buffer.len(), buffer.as_ptr()));
            o.ctx.task = SendData {
                info: DataHead {
                    fd: o.sock()?.fd().into(),
                    kind: u8::try_from(EVENT_CONNECT).unwrap_or_default(),
                    reactor_id: o.reactor()?.id().cast_signed(),
                    len: u32::try_from(len).unwrap_or(u32::MAX),
                    ..DataHead::default()
                },
                data: data.cast(),
            };
            0
        }
        OP_HAVE_DGRAM => u64::from(o.config().have_dgram_sock),
        // SAFETY: the datagram server's connection, whose object is its live port.
        OP_PORT_DTLS => u64::from(unsafe { port_config::of(o.conn()?.object()) }.is_dtls()),
        OP_REACTOR_ID => u64::from(o.reactor()?.id()),
        OP_REACTOR_NUM => u64::from(o.config().reactor_num.get()),
        OP_WORKER_NUM => u64::from(o.config().worker_num.get()),
        OP_CONN_AT => {
            let conn = o.state.connection(c_long::from(int(arg)), Lookup::ByFd);
            if !valid(conn) {
                return Some(0);
            }
            o.ctx.conn = conn.map_or(core::ptr::null_mut(), Connection::as_ptr);
            1
        }
        OP_CONN_FD => sign(o.conn()?.fd()),
        OP_CONN_REACTOR => u64::from(o.conn()?.reactor_id()),
        OP_SESSION_CONN => {
            o.ctx.conn = o.connection(arg.cast_signed(), Lookup::Verified);
            u64::from(!o.ctx.conn.is_null())
        }
        OP_SET_WAITING_TIME => {
            o.conn()?
                .set_waiting_time(u16::try_from(arg & 0xffff).unwrap_or_default());
            0
        }
        OP_QUEUE_SUB => {
            let bytes = u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default();
            o.conn()?.recv_queued_bytes().fetch_sub(bytes, SeqCst);
            0
        }
        OP_SERVER => u64::try_from(o.ctx.serv.raw().expose_provenance()).unwrap_or(0),
        OP_RD_WRITE => {
            let (serv, reactor) = (o.server(), o.handle()?);
            sign(crate::conn_io::write(
                serv,
                reactor,
                o.ctx.event.as_deref_mut()?,
            ))
        }
        op => return effects(&mut o, op, arg),
    })
}

/// `ReactorThread`; server.cc asserts this layout. Its own thread writes it, so the
/// fields it sets are cells behind a shared reference.
#[repr(C)]
pub(crate) struct ThreadRecord {
    pub(crate) id: Cell<c_int>,
    pub(crate) notify_pipe: Cell<*mut SocketRecord>,
    pub(crate) dispatch_count: Cell<u64>,
    pub(crate) pipe_command: Cell<*mut SocketRecord>,
    pub(crate) heartbeat_timer: Cell<*mut c_void>,
    /// `message_bus`: its Rust bus, then the allocator of its buffer and packets.
    pub(crate) bus: *mut c_void,
    pub(crate) allocator: *const swoole_rs_corebase::CoreAllocator,
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ThreadRecord {
    /// `message_bus`.
    pub(crate) fn message_bus(&self) -> crate::objects::Bus {
        // SAFETY: a live thread's bus and allocator, made together.
        unsafe { crate::objects::Bus::from_parts(self.bus, self.allocator) }
    }
}

const _: () = assert!(
    core::mem::offset_of!(ThreadRecord, heartbeat_timer) == 32
        && core::mem::offset_of!(ThreadRecord, bus) == 40
        && size_of::<ThreadRecord>() == 56,
    "server.cc ReactorThread layout"
);

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ThreadRecord {
    /// `new ReactorThread[count]()`: each with a fresh bus over `allocator`.
    pub(crate) fn new_array(
        count: usize,
        allocator: *const swoole_rs_corebase::CoreAllocator,
    ) -> *mut c_void {
        let size = swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize;
        let threads: Box<[Self]> = (0..count)
            .map(|_| Self {
                id: Cell::new(0),
                notify_pipe: Cell::new(core::ptr::null_mut()),
                dispatch_count: Cell::new(0),
                pipe_command: Cell::new(core::ptr::null_mut()),
                heartbeat_timer: Cell::new(core::ptr::null_mut()),
                bus: Box::into_raw(swoole_rs_bus::MessageBus::create(size)).cast(),
                allocator,
            })
            .collect();
        Box::into_raw(threads).cast()
    }

    /// `delete[] threads`: the `count` threads [`Self::new_array`] made, and their buses.
    ///
    /// # Safety
    /// `threads` is [`Self::new_array`]'s array of `count`, not used again.
    pub(crate) unsafe fn delete_array(threads: *mut c_void, count: usize) {
        let threads = core::ptr::slice_from_raw_parts_mut(threads.cast::<Self>(), count);
        // SAFETY: as the caller guarantees.
        let threads = unsafe { Box::from_raw(threads) };
        for thread in &threads {
            // SAFETY: the thread's own bus, `new_array` boxed, freed once.
            drop(unsafe { Box::from_raw(thread.bus.cast::<swoole_rs_bus::MessageBus>()) });
        }
    }
}

/// The ops over the context's reactor, sockets, connection, port and thread
/// and the server's counters; else `None`. Inlined, as [`answer`] is.
#[inline(always)]
fn effects(o: &mut Objects<'_, '_>, op: u32, arg: u64) -> Option<u64> {
    let reactor = o.handle();
    // `reactor->add/set/del(socket)`.
    let change = |socket, kind, events| reactor.map(|r| sign(r.change(socket, kind, events)));
    // `reactor->remove_read_event(socket)` (4) and `remove_write_event` (5).
    let interest = |socket, kind| reactor.map(|r| sign(r.interest(socket, kind, 0)));
    Some(match op {
        OP_REACTOR_DEL => change(o.sock()?, 2, 0)?,
        OP_PORT_DEL => {
            change(o.server().port_socket_at(usize::try_from(arg).ok()?)?, 2, 0);
            0
        }
        OP_CONN_UNREAD => interest(o.conn_socket()?, 4)?,
        OP_RD_UNREAD => interest(o.sock()?, 4)?,
        OP_NOTIFY_CONNECT => {
            crate::server_methods::notify(o.server(), o.sock_conn()?, EVENT_CONNECT);
            0
        }
        OP_SSL_ACCEPT => sign(o.sock().map_or(-1, tls::socket_ssl_accept)),
        op => return native(o, op, arg),
    })
}

/// The ops that call DTLS and the event loop under their upstream symbols; else
/// `None`. A reactor thread's own loop runs, and is freed,
/// here: master.cc no longer answers `OP_EVENT_WAIT` or `OP_EVENT_FREE`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[inline(always)]
fn native(o: &mut Objects<'_, '_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{OP_BASE_PASS, OP_DTLS_ACCEPT, OP_EVENT_WAIT, OP_RD_DTLS};
    use swoole_rs_reactor::event_api::swoole_event_wait;
    Some(match op {
        OP_DTLS_ACCEPT => {
            let (serv, reactor) = (o.ctx.serv, o.handle()?);
            let (single_thread, close) = (o.config().single_thread.get(), o.reactor()?.close());
            let (port, pkt) = (o.conn()?.object(), o.ctx.pkt.cast());
            // SAFETY: the context's live server and reactor, the DTLS port the packet
            // arrived on and the packet received, `arg` bytes.
            sign(unsafe {
                crate::dtls::on_packet(serv, single_thread, reactor, close, port, pkt, arg)
            })
        }
        OP_RD_DTLS => {
            let (sock, fd) = (o.sock()?, o.event()?.fd());
            u64::from(crate::dtls::on_read(o.ctx.port?.config(), sock, fd))
        }
        OP_BASE_PASS => {
            // SAFETY: the `SendData` (the context's task) the dispatch lends at arg.
            let send = unsafe { &*(arg as *const SendData) };
            crate::master_ops::base_pass(o.server(), send);
            0
        }
        OP_EVENT_WAIT => sign(swoole_event_wait()),
        op => return crate::thread_ops::answer(o.ctx, o.state, op, arg),
    })
}

/// Outside the hybrid, whose unit tests link no C++, native code keeps these.
#[cfg(not(all(feature = "upstream-symbols", not(test))))]
#[inline(always)]
fn native(_: &mut Objects<'_, '_>, _: u32, _: u64) -> Option<u64> {
    None
}
