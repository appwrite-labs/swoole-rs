//! The reactor-thread ops (master.cc's `reactor_thread_call`) that are a log line
//! or a read or write of a record Rust already lays out: the server's config,
//! tables and callbacks, the reactor's fields and the context's connection,
//! socket, event and pipe message. Rust answers them before any native call, so
//! master.cc no longer has a case for them. Logs name the upstream function that
//! wrote them. Its effects over the reactor, the context's sockets and the
//! server's counters are Rust calls too.
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering::SeqCst;

use swoole_rs_buffer::cpp::{
    CppBuffer, new_buffer, swr_cpp_buffer_empty, swr_cpp_buffer_length, swr_cpp_buffer_pop,
    swr_cpp_buffer_typed_close, swr_cpp_buffer_typed_front,
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_buffer::string::delete_string;
use swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
use swoole_rs_corebase::{DataHead, log, microtime};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Event, Socket, SocketFields, SocketRecord, what_event_want};
use swoole_rs_timer::swoole_timer_del;
use swoole_rs_tls::upstream as tls;
use swoole_rs_tls::{OwnedTlsFields, OwnedTlsPrepared, swr_v1_tls_owner_view};

use crate::base_factory::SendData;
use crate::callbacks::{CB_BUFFER_EMPTY, CB_DISPATCH_FUNC};
use crate::conn_io as cn;
use crate::conn_io::{
    CLOSE_WAIT, LOW_WATERMARK, OUT_EMPTY, OVERFLOW_DRAINED, PEER_CLOSED, REMOVED, SEND_TIMER,
    SEND_WAIT,
};
use crate::ops::OP_PORT_DTLS;
use crate::ops::{
    LOG_CC_LINGER, LOG_NO_SESSION, LOG_RECVFROM, LOG_RT_CLOSED_BY_SERVER, LOG_RT_DISCARD,
    LOG_RT_SET_WRITE, OP_CC_SESSION_CLEAR, OP_CC_SSL_WANT, OP_CC_TIMER_DEL, OP_CC_VALID,
    OP_CERT_TASK, OP_CLIENT_CERT, OP_CONN_AT, OP_CONN_FD, OP_CONN_FORCE, OP_CONN_HANDSHAKING,
    OP_CONN_REACTOR, OP_CONN_READING, OP_CONN_SESSION, OP_HAVE_DGRAM, OP_MICROTIME, OP_PEER_CERT,
    OP_PIPE_FD, OP_PIPE_TYPE, OP_QUEUE_FULL, OP_QUEUE_SUB, OP_RD_WRITE_FACTS, OP_REACTOR_ID,
    OP_REACTOR_NUM, OP_RT_BUF_EMPTY, OP_RT_DISCARD_TIMEOUT, OP_RT_FRONT_CONN,
    OP_RT_FRONT_CONN_CLOSED, OP_RT_FRONT_STREAM, OP_RT_HAS_PIPE_COMMAND, OP_RT_TOUCH_SEND,
    OP_SERVER, OP_SESSION_CONN, OP_SET_WAITING_TIME, OP_SSL_READY, OP_SSL_VERIFY, OP_VERIFY_PEER,
    OP_WAIT_EXIT, OP_WAITING_TIME, OP_WORKER_NUM, OP_WR_CLEAR_OVERFLOW, OP_WR_QUEUED, OP_WR_STATE,
    OP_WR_TIMER_DEL, OP_WR_WAIT,
};
use crate::ops::{
    OP_CC_HTTP_OBJECT, OP_CC_LINGER, OP_CC_QUEUE_CLOSE, OP_CC_RECV_FREE, OP_CC_REDIS_FREE,
    OP_CC_REDIS_OBJECT, OP_CC_SSL_CLOSE, OP_CC_SSL_SHUTDOWN, OP_CC_WATCH_WRITE,
    OP_CC_WEBSOCKET_FREE, OP_CONN_UNREAD, OP_HEARTBEAT_TIMER_CLEAR, OP_KILL_SELF,
    OP_MASTER_REACTOR_ID, OP_NOTIFY_CONNECT, OP_PORT_DEL, OP_RD_UNREAD, OP_REACTOR_CLOSE,
    OP_REACTOR_DEL, OP_RT_DGRAM_BIND, OP_RT_POP, OP_RT_REMOVE_WRITE, OP_RT_SETUP, OP_SSL_ACCEPT,
    OP_WR_BUFFER_EMPTY, OP_WR_CLOSE, OP_WR_CLOSE_ERRNO, OP_WR_FRONT, OP_WR_NOTIFY_CLOSE,
    OP_WR_READ_ONLY,
};
use crate::port_config::{self, PortConfig};
use crate::reactor_entry as entry;
use crate::reactor_handle::Reactor;
use crate::records::ConnectionRecord;
use crate::view;
use crate::{ServerConfig, ServerState, ServerTables};

/// `PREQUEUE` in builds with `--fix-halfclose` (`SW_HALFCLOSE_PREQUEUE`), else nothing.
const BUILT_PREQUEUE: u64 = if cfg!(feature = "halfclose") {
    cn::PREQUEUE
} else {
    0
};

const SW_LOG_TRACE: c_int = 1;
const SW_LOG_NOTICE: c_int = 3;
const SW_LOG_WARNING: c_int = 4;
/// `SW_FD_SESSION`, `SW_FD_DGRAM_SERVER` and `SW_FD_PIPE`.
const FD_SESSION: c_int = 0;
#[cfg(all(feature = "upstream-symbols", not(test)))]
const FD_DGRAM_SERVER: c_int = 2;
const FD_PIPE: c_int = 3;
const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;
/// `SW_SERVER_EVENT_CLOSE`, `_CONNECT` and `_BUFFER_EMPTY`, which master.cc asserts.
const EVENT_CLOSE: c_int = 4;
const EVENT_CONNECT: c_int = 5;
const EVENT_BUFFER_EMPTY: c_int = 14;

/// The objects a reactor-thread op acts on: a live server, the reactor the op runs
/// under, and what the ops before it found. The entry points (`reactor_entry`) build
/// it. Each raw field is null or what the op setting it found, live while the context
/// is: one of the server's reactor threads, one of its connection slots, a worker's
/// message in this thread's bus, this thread's buffer. Its readers only test for null.
pub(crate) struct ReactorContext<'a> {
    pub(crate) serv: view::Server<'a>,
    pub(crate) reactor: Option<Reactor>,
    pub(crate) thread: *mut c_void,
    pub(crate) event: Option<&'a mut Event>,
    pub(crate) port: Option<view::Port<'a>>,
    pub(crate) sock: Option<Socket<'a>>,
    pub(crate) conn: *mut ConnectionRecord,
    /// The `PipeBuffer` read from a worker, which starts with its head.
    pub(crate) resp: *const DataHead,
    /// The datagram `OP_PACKET_INIT` reads into: this thread's buffer.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops read it")
    )]
    pub(crate) pkt: *mut c_void,
    /// The worker-pipe message at the front of the output buffer.
    pub(crate) front: *const DataHead,
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops read it")
    )]
    pub(crate) front_length: usize,
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
            front: core::ptr::null(),
            front_length: 0,
            task: SendData {
                info: DataHead::default(),
                data: core::ptr::null(),
            },
        }
    }

    /// The context's reactor thread, once an op has set it.
    pub(crate) fn thread_record(&self) -> Option<&'a mut ThreadRecord> {
        // SAFETY: null or one of the server's live reactor threads, as the struct
        // says, which only its own thread touches.
        unsafe { self.thread.cast::<ThreadRecord>().as_mut() }
    }

    /// The context's connection, once an op has set it.
    pub(crate) fn connection(&self) -> Option<&'a mut ConnectionRecord> {
        // SAFETY: null or one of the server's live connection slots, as the struct says.
        unsafe { self.conn.as_mut() }
    }

    /// The worker's message `OP_PIPE_READ` stored.
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
fn stream_event(kind: u8) -> bool {
    // RECV_DATA, SEND_DATA, SEND_FILE, CLOSE, CONNECT, PAUSE_RECV, RESUME_RECV,
    // BUFFER_FULL and BUFFER_EMPTY.
    matches!(kind, 0 | 2..=5 | 11..=14)
}

pub(crate) use swoole_rs_ossupport::pthread_self;

/// `f` over this thread's buffer (`sw_tg_buffer()`), which a datagram or a
/// certificate is read into.
pub(crate) fn with_thread_buffer<R>(f: impl FnOnce(&mut CppString) -> R) -> R {
    swoole_rs_corebase::lend_thread_buffer(|| {
        // SAFETY: the calling thread's live buffer, lent to `f` alone.
        f(unsafe { &mut *swoole_rs_corebase::thread_buffer().cast::<CppString>() })
    })
}

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

#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written on the reactor thread that owns them as master.cc did"
)]
impl<'c> Objects<'_, 'c> {
    /// The context's live server.
    fn server(&self) -> view::Server<'c> {
        self.ctx.serv
    }
    fn config(&self) -> &ServerConfig {
        self.state.config_ref()
    }
    fn tables(&self) -> &ServerTables {
        self.state.tables_ref()
    }
    fn conn(&self) -> Option<&'c mut ConnectionRecord> {
        self.ctx.connection()
    }
    fn conn_socket(&self) -> Option<Socket<'c>> {
        let conn: &'c ConnectionRecord = self.conn()?;
        conn.socket()
    }
    fn sock(&self) -> Option<Socket<'c>> {
        self.ctx.sock
    }
    /// `read` over the context's socket, or the default without one.
    fn read_sock<R: Default>(&self, read: impl FnOnce(&SocketFields) -> R) -> R {
        self.sock().map(|sock| sock.read(read)).unwrap_or_default()
    }
    fn event_socket(&self) -> Option<Socket<'_>> {
        self.event()?.socket()
    }
    fn event(&self) -> Option<&Event> {
        self.ctx.event.as_deref()
    }
    fn resp(&self) -> Option<&'c DataHead> {
        self.ctx.message()
    }
    fn front(&self) -> Option<&DataHead> {
        // SAFETY: null, or the live front message OP_RT_FRONT stored.
        unsafe { self.ctx.front.as_ref() }
    }
    /// The context's live reactor.
    fn handle(&self) -> Option<Reactor> {
        self.ctx.reactor
    }
    fn reactor(&self) -> Option<&mut swoole_rs_reactor::ReactorFields> {
        // The context's live reactor, which begins with its prepared state.
        let fields = swoole_rs_reactor::ReactorPrepared::of(self.handle()?.ptr()).fields;
        // SAFETY: as above; only this thread touches its reactor.
        unsafe { fields.as_mut() }
    }
    fn port(&self) -> Option<&'c PortConfig> {
        self.ctx.port.map(view::Port::config)
    }
    /// The TLS port's `SSLContext` view.
    fn ssl(&self) -> Option<OwnedTlsPrepared> {
        let context = self.port()?.ssl_context.as_deref()?;
        Some(swr_v1_tls_owner_view(context))
    }
    fn ssl_fields(&self) -> Option<&OwnedTlsFields> {
        // SAFETY: the owner's fields live as long as the port's reference.
        unsafe { self.ssl()?.fields.as_ref() }
    }
    /// `get_connection(fd)` (kind 0) or a verified session's (kind 2, 3).
    fn connection(&self, key: c_long, kind: u32) -> *mut ConnectionRecord {
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

/// `Socket::is_stream(type)` and `is_dgram(type)`.
fn is_stream(kind: c_int) -> bool {
    matches!(kind, 1 | 3 | 5)
}
fn is_dgram(kind: c_int) -> bool {
    matches!(kind, 2 | 4 | 6)
}

/// `ReactorThread_onRead`'s facts over the context's connection; sets the context's
/// port. A session socket that is a datagram is a DTLS port's (UDP ports take
/// their packets on the listening socket).
fn read_facts(o: &mut Objects<'_, '_>) -> u64 {
    let (Some(conn), Some(fd)) = (o.conn(), o.event().map(Event::fd)) else {
        return 0;
    };
    if conn.server_fd == 0 {
        return 0;
    }
    o.ctx.port = o.port_by_fd(fd);
    let process = o.mode() == crate::MODE_PROCESS;
    o.read_sock(|sock| {
        let ssl = !sock.ssl().is_null();
        cn::FOUND
            | BUILT_PREQUEUE
            | flag(sock.close_notified != 0, cn::CLOSE_NOTIFIED)
            | flag(conn.close_queued != 0, cn::CLOSE_QUEUED)
            | flag(ssl && sock.tls.want_read != 0, cn::SSL_WANT_READ)
            | flag(ssl && sock.tls.state != SSL_READY, cn::SSL_PENDING)
            | flag(is_dgram(sock.socket_type), cn::DTLS_PORT)
            | flag(sock.dtls != 0, cn::DTLS_SOCKET)
            | flag(process, cn::PROCESS)
    })
}

/// `ReactorThread_onWrite`'s facts; sets the context's port and connection.
fn write_facts(o: &mut Objects<'_, '_>) -> u64 {
    let Some(fd) = o.event().map(Event::fd) else {
        return 0;
    };
    o.ctx.port = o.port_by_fd(fd);
    if o.mode() == crate::MODE_PROCESS {
        debug_assert_eq!(
            Some(fd % c_int::from(o.config().reactor_num)),
            o.reactor().map(|reactor| c_int::from(reactor.id)),
            "a reactor thread writes only its own fds"
        );
    }
    let Some(conn) = o.state.connection(c_long::from(fd), 0) else {
        o.ctx.conn = core::ptr::null_mut();
        return 0;
    };
    o.ctx.conn = &raw mut *conn;
    if conn.active == 0 {
        return 0;
    }
    let pending = o.read_sock(|sock| !sock.ssl().is_null() && sock.tls.state != SSL_READY);
    cn::FOUND
        | BUILT_PREQUEUE
        | flag(conn.close_notify != 0, cn::CLOSE_NOTIFY)
        | flag(pending, cn::SSL_PENDING)
        | flag(o.config().disable_notify, cn::DISABLE_NOTIFY)
        | flag(conn.close_force != 0, cn::CLOSE_FORCE)
}

/// `ReactorThread_onClose`'s facts over the context's event: what decides whether
/// a clean FIN leaves the write side usable (native/halfclose/prequeue.md). Sets
/// the context's connection.
fn peer_close_facts(o: &mut Objects<'_, '_>) -> u64 {
    let Some(fd) = o.event().map(Event::fd) else {
        return 0;
    };
    let Some(conn) = o.state.connection(c_long::from(fd), 0) else {
        o.ctx.conn = core::ptr::null_mut();
        return 0;
    };
    o.ctx.conn = &raw mut *conn;
    if conn.active == 0 {
        return 0;
    }
    let config = o.config();
    let dispatch_func = o
        .state
        .callbacks
        .slot(CB_DISPATCH_FUNC)
        .is_some_and(crate::callbacks::CallbackSlot::is_set);
    let notified = o
        .event_socket()
        .is_some_and(|s| s.read(|s| s.close_notified != 0));
    cn::FOUND
        | BUILT_PREQUEUE
        | flag(config.disable_notify, cn::DISABLE_NOTIFY)
        | flag(conn.close_queued != 0, cn::CLOSE_QUEUED)
        | flag(notified, cn::CLOSE_NOTIFIED)
        | flag(o.mode() == crate::MODE_PROCESS, cn::PROCESS)
        | flag(matches!(config.dispatch_mode, 2 | 4 | 8), cn::HASH_DISPATCH)
        | flag(dispatch_func, cn::DISPATCH_FUNC)
        | flag(config.enable_coroutine, cn::COROUTINE)
        | flag(conn.ssl != 0, cn::CONN_SSL)
        | flag(conn.close_errno != 0, cn::CLOSE_ERRNO)
        | flag(conn.close_force != 0, cn::CLOSE_FORCE)
        | flag(conn.close_reset != 0, cn::CLOSE_RESET)
}

/// `Server::close_connection`'s facts, the socket's fd above bit 32; sets the
/// context's connection and port.
fn close_facts(o: &mut Objects<'_, '_>) -> u64 {
    let Some((object, fd)) = o
        .sock()
        .map(|sock| sock.read(|sock| (sock.object, sock.fd)))
    else {
        return 0;
    };
    // A session socket's object is its connection.
    o.ctx.conn = object.cast();
    o.ctx.port = o.port_by_fd(fd);
    let Some(conn) = o.conn() else {
        return 0;
    };
    let base = o.mode() == crate::MODE_BASE;
    o.read_sock(|sock| {
        let ssl = !sock.ssl().is_null();
        BUILT_PREQUEUE
            | flag(!conn.timer.is_null(), cn::TIMER)
            | flag(!conn.websocket_buffer.is_null(), cn::WEBSOCKET_BUFFER)
            | flag(sock.removed != 0, cn::REMOVED)
            | flag(base, cn::BASE)
            | flag(ssl, cn::SOCKET_SSL)
            | flag(sock.dtls != 0, cn::DTLS_SOCKET)
            | flag(!sock.recv_buffer.is_null(), cn::RECV_BUFFER)
            | flag(conn.close_force != 0, cn::CLOSE_FORCE)
            | flag(conn.close_reset != 0, cn::CLOSE_RESET)
            | flag(conn.peer_closed != 0, cn::PEER_CLOSED)
            | flag(conn.close_queued != 0, cn::CLOSE_QUEUED)
            | flag(sock.ssl_closed != 0, cn::SSL_CLOSED)
            | flag(
                ssl && sock.tls.state == SSL_READY && is_stream(sock.socket_type),
                cn::TLS_READY_STREAM,
            )
            | (u64::from(sock.fd.cast_unsigned()) << 32)
    })
}

/// Which facts [`facts`] builds.
#[derive(Clone, Copy)]
pub(crate) enum Facts {
    Read,
    Write,
    PeerClose,
    Close,
}

/// The facts an entry point passes its run.
pub(crate) fn facts(ctx: &mut ReactorContext<'_>, state: &ServerState, which: Facts) -> u64 {
    let mut o = Objects { ctx, state };
    match which {
        Facts::Read => read_facts(&mut o),
        Facts::Write => write_facts(&mut o),
        Facts::PeerClose => peer_close_facts(&mut o),
        Facts::Close => close_facts(&mut o),
    }
}

/// `Server::is_valid_connection`.
pub(crate) fn valid(conn: Option<&ConnectionRecord>) -> bool {
    let Some(conn) = conn.filter(|conn| conn.active != 0) else {
        return false;
    };
    conn.socket()
        .is_some_and(|socket| socket.read(|s| s.fd_type == FD_SESSION))
}

fn error(level: c_int, code: E, origin: &core::ffi::CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// Answers `op` when it is one of this module's, else `None`, as an op does when the
/// context lacks an object it reads. Always inlined: each caller's constant op folds
/// the match, so the hot runners' other ops cost nothing.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
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
        LOG_NO_SESSION => {
            let session = arg.cast_signed();
            let m = format_args!("force close connection failed, session#{session} does not exist");
            let origin = c"ReactorThread::close_connection";
            error(SW_LOG_TRACE, E::SW_ERROR_SESSION_NOT_EXIST, origin, m);
            0
        }
        LOG_RT_CLOSED_BY_SERVER => {
            let m = format_args!("Session#{} is closed by server", o.front()?.fd);
            let origin = c"ReactorThread_onPipeWrite";
            error(
                SW_LOG_NOTICE,
                E::SW_ERROR_SESSION_CLOSED_BY_SERVER,
                origin,
                m,
            );
            0
        }
        LOG_RT_DISCARD => {
            let front = o.front()?;
            let m = format_args!(
                "[1] ignore data[{} bytes] received from session#{}",
                front.len, front.fd
            );
            let origin = c"ReactorThread_onPipeWrite";
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SESSION_DISCARD_TIMEOUT_DATA,
                origin,
                m,
            );
            0
        }
        LOG_RT_SET_WRITE => {
            let m = format_args!("reactor->set({}) failed", o.event()?.fd());
            log::sys_warning(c"ReactorThread_onPipeWrite", m);
            0
        }
        LOG_CC_LINGER => {
            let m = format_args!("setsockopt(SO_LINGER) failed");
            log::sys_warning(c"Server::close_connection", m);
            0
        }
        OP_SSL_READY => {
            let conn = o.sock()?.read(|s| s.object.cast::<ConnectionRecord>());
            // SAFETY: a session socket's object is null or its live connection.
            unsafe { conn.as_mut() }?.ssl_ready = 1;
            0
        }
        OP_CLIENT_CERT => {
            // `client_cert_file`'s slot.
            let slot = o.ssl()?.strings[7];
            // SAFETY: a stable slot of the port's live owner.
            u64::from(!unsafe { &*slot }.bytes().is_empty())
        }
        OP_VERIFY_PEER => u64::from(o.ssl_fields()?.verify_peer != 0),
        OP_PEER_CERT => {
            let buffer = swoole_rs_corebase::thread_buffer();
            // SAFETY: a TLS socket's live record and this thread's live buffer.
            u64::from(unsafe { tls::socket_ssl_get_peer_certificate_string(o.sock()?, buffer) })
        }
        OP_SSL_VERIFY => {
            let allow_self_signed = o.ssl_fields()?.allow_self_signed != 0;
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
                    reactor_id: o.reactor()?.id.cast_signed(),
                    len: u32::try_from(len).unwrap_or(u32::MAX),
                    ..DataHead::default()
                },
                data: data.cast(),
            };
            0
        }
        OP_HAVE_DGRAM => u64::from(o.config().have_dgram_sock),
        // SAFETY: the datagram server's connection, whose object is its live port.
        OP_PORT_DTLS => u64::from(unsafe { port_config::of(o.conn()?.object) }.is_dtls()),
        OP_REACTOR_ID => u64::from(o.reactor()?.id),
        OP_REACTOR_NUM => u64::from(o.config().reactor_num),
        OP_WORKER_NUM => u64::from(o.config().worker_num),
        OP_CONN_AT | OP_CC_VALID => {
            let conn = o.state.connection(c_long::from(int(arg)), 0);
            if !valid(conn.as_deref()) {
                return Some(0);
            }
            if op == OP_CONN_AT {
                o.ctx.conn = conn.map_or(core::ptr::null_mut(), |conn| &raw mut *conn);
            }
            1
        }
        OP_CONN_FD => sign(o.conn()?.fd),
        OP_CONN_REACTOR => u64::from(o.conn()?.reactor_id),
        OP_CONN_SESSION => sign(o.conn()?.session_id),
        OP_CONN_READING => {
            let removed = o.conn_socket()?.read(|s| s.removed != 0);
            u64::from(o.conn()?.peer_closed == 0 && !removed)
        }
        OP_WAIT_EXIT => {
            o.reactor()?.wait_exit = true;
            0
        }
        OP_SESSION_CONN | OP_RT_FRONT_CONN => {
            // The front's fd is the session id.
            let (key, kind) = if op == OP_SESSION_CONN {
                (arg.cast_signed(), 2)
            } else {
                (o.front()?.fd, 3)
            };
            o.ctx.conn = o.connection(key, kind);
            u64::from(!o.ctx.conn.is_null())
        }
        OP_CONN_FORCE => u64::from(o.conn()?.close_force),
        OP_CONN_HANDSHAKING => u64::from(o.conn()?.ssl != 0 && o.conn()?.ssl_ready == 0),
        OP_PIPE_TYPE => u64::from(o.resp()?.kind),
        OP_PIPE_FD => sign(o.resp()?.fd),
        OP_MICROTIME => microtime().to_bits(),
        OP_QUEUE_FULL => {
            u64::from(o.conn()?.recv_queued_bytes.load(SeqCst) > o.config().max_queued_bytes)
        }
        OP_WAITING_TIME => u64::from(o.conn()?.waiting_time),
        OP_SET_WAITING_TIME => {
            o.conn()?.waiting_time = u16::try_from(arg & 0xffff).unwrap_or_default();
            0
        }
        OP_QUEUE_SUB => {
            let bytes = u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default();
            o.conn()?.recv_queued_bytes.fetch_sub(bytes, SeqCst);
            0
        }
        OP_RT_BUF_EMPTY => {
            let sock = o.event_socket()?;
            u64::from(sock.read(|s| out_buffer(s).is_none_or(|b| swr_cpp_buffer_empty(b))))
        }
        OP_RT_FRONT_STREAM => u64::from(stream_event(o.front()?.kind)),
        OP_RT_TOUCH_SEND => {
            o.conn()?.last_send_time = microtime();
            0
        }
        OP_RT_FRONT_CONN_CLOSED => u64::from(o.conn()?.closed.load(SeqCst)),
        OP_RT_DISCARD_TIMEOUT => u64::from(o.config().discard_timeout_request),
        OP_SERVER => u64::try_from(o.ctx.serv.raw().expose_provenance()).unwrap_or(0),
        OP_RT_HAS_PIPE_COMMAND => u64::from(!o.config().pipe_command.is_null()),
        OP_WR_WAIT => {
            let (close, send) = o.read_sock(|s| (s.output.close_wait, s.output.send_wait));
            (if close != 0 { CLOSE_WAIT } else { 0 }) | (if send != 0 { SEND_WAIT } else { 0 })
        }
        OP_WR_CLEAR_OVERFLOW => {
            o.conn()?.overflow = 0;
            0
        }
        OP_WR_QUEUED => {
            let length = o.read_sock(|s| out_buffer(s).map_or(0, |b| swr_cpp_buffer_length(b)));
            o.conn()?.send_queued_bytes = length;
            0
        }
        OP_WR_STATE => write_state(&o)?,
        OP_RD_WRITE_FACTS => write_facts(&mut o),
        OP_WR_TIMER_DEL => {
            let sock = o.sock()?;
            let timer = sock.read(|s| s.output.send_timer);
            // SAFETY: the socket's live send timer.
            unsafe { swoole_timer_del(timer.cast()) };
            sock.edit(|s| s.output.send_timer = core::ptr::null_mut());
            0
        }
        OP_CC_TIMER_DEL => {
            let conn = o.conn()?;
            // SAFETY: the connection's live timer.
            unsafe { swoole_timer_del(conn.timer.cast()) };
            conn.timer = core::ptr::null_mut();
            0
        }
        OP_CC_SSL_WANT => u64::from(o.read_sock(|s| s.tls.want_read != 0 || s.tls.want_write != 0)),
        OP_CC_SESSION_CLEAR => {
            if let Some(session) = state.session(o.conn()?.session_id) {
                session.fd = 0;
            }
            0
        }
        op => return effects(&mut o, op, arg),
    })
}

/// `ReactorThread`; server.cc asserts this layout.
#[repr(C)]
pub(crate) struct ThreadRecord {
    pub(crate) id: c_int,
    pub(crate) notify_pipe: *mut SocketRecord,
    pub(crate) dispatch_count: u64,
    pub(crate) pipe_command: *mut SocketRecord,
    pub(crate) heartbeat_timer: *mut c_void,
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
                id: 0,
                notify_pipe: core::ptr::null_mut(),
                dispatch_count: 0,
                pipe_command: core::ptr::null_mut(),
                heartbeat_timer: core::ptr::null_mut(),
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
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one arm per op, as master.cc's switch was"
)]
#[inline(always)]
fn effects(o: &mut Objects<'_, '_>, op: u32, arg: u64) -> Option<u64> {
    let reactor = o.handle();
    // `reactor->add/set/del(socket)`.
    let change = |socket, kind, events| reactor.map(|r| sign(r.change(socket, kind, events)));
    // `reactor->remove_read_event(socket)` (4) and `remove_write_event` (5).
    let interest = |socket, kind| reactor.map(|r| sign(r.interest(socket, kind, 0)));
    let port = || {
        let port = o.tables().ports().get(usize::try_from(arg).ok()?)?;
        Some(*port)
    };
    Some(match op {
        OP_REACTOR_DEL => change(o.sock()?, 2, 0)?,
        OP_PORT_DEL => {
            change(o.server().port_socket_at(usize::try_from(arg).ok()?)?, 2, 0);
            0
        }
        OP_RT_DGRAM_BIND => {
            // listen UDP port
            let port = port()?;
            // SAFETY: one of the server's live ports.
            let config = unsafe { port.config_mut() };
            let socket = o.server().port_socket_at(usize::try_from(arg).ok()?)?;
            let fd = socket.fd();
            // The connection table's slot for the listening fd.
            let serv_sock = o.state.connection(c_long::from(fd), 0)?;
            serv_sock.fd = fd;
            serv_sock.socket_type = config.r#type;
            serv_sock.object = port.raw();
            config.thread_id = pthread_self();
            change(socket, 0, SW_EVENT_READ)?
        }
        OP_CONN_UNREAD => interest(o.conn_socket()?, 4)?,
        OP_RD_UNREAD => interest(o.sock()?, 4)?,
        OP_RT_REMOVE_WRITE => interest(o.event_socket()?, 5)?,
        OP_WR_READ_ONLY => {
            change(o.sock()?, 1, SW_EVENT_READ);
            0
        }
        OP_REACTOR_CLOSE => {
            o.handle()?.close(o.conn_socket()?);
            0
        }
        OP_WR_CLOSE => sign(o.handle()?.close(o.sock()?)),
        OP_WR_CLOSE_ERRNO => {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "C++'s uint16_t close_errno"
            )]
            let errno = errno as u16;
            o.conn()?.close_errno = errno;
            let reactor = o.handle()?;
            sign(reactor.trigger_close(o.ctx.event.as_deref_mut()?))
        }
        OP_HEARTBEAT_TIMER_CLEAR => {
            let thread = o.ctx.thread_record()?;
            if !thread.heartbeat_timer.is_null() {
                // SAFETY: the thread's live heartbeat timer.
                unsafe { swoole_timer_del(thread.heartbeat_timer.cast()) };
                thread.heartbeat_timer = core::ptr::null_mut();
            }
            0
        }
        OP_KILL_SELF => {
            let pid = std::process::id().cast_signed();
            swoole_rs_process::os::kill(pid, libc::SIGTERM);
            0
        }
        OP_MASTER_REACTOR_ID => {
            // a special id for the master thread
            let id = o.config().reactor_num;
            o.reactor()?.id = id;
            swoole_rs_corebase::set_thread(swoole_rs_corebase::thread_kind(), id);
            0
        }
        OP_RT_SETUP => {
            let fields = o.reactor()?;
            fields.ptr = o.ctx.serv.raw();
            fields.id = u16::try_from(arg & 0xffff).unwrap_or_default();
            fields.wait_exit = false;
            fields.max_socket = o.tables().max_connection;
            fields.close = Some(crate::reactor_entry::close_connection);
            let handle = o.handle()?;
            handle.set_error_handler(entry::on_close);
            for (event, handler) in [
                (
                    SW_EVENT_READ,
                    entry::on_pipe_read as swoole_rs_reactor::Handler,
                ),
                (SW_EVENT_WRITE, entry::on_pipe_write),
            ] {
                handle.set_handler(FD_PIPE, event, handler);
            }
            0
        }
        OP_RT_POP => {
            o.event_socket()?
                .read(|s| out_buffer(s).map(|b| swr_cpp_buffer_pop(b)));
            0
        }
        OP_WR_FRONT => o.sock()?.read(|s| {
            let buffer = out_buffer(s).filter(|b| !swr_cpp_buffer_empty(b));
            buffer.map_or(0, |b| 1 + u64::from(swr_cpp_buffer_typed_front(b).kind))
        }),
        OP_CC_QUEUE_CLOSE => {
            let sock = o.sock()?;
            let mut buffer = sock.read(|s| s.output.out_buffer);
            if buffer.is_null() {
                buffer = new_buffer(SW_SEND_BUFFER_SIZE).cast();
                sock.edit(|s| s.output.out_buffer = buffer);
            }
            sock.read(|s| out_buffer(s).map(|b| swr_cpp_buffer_typed_close(b)));
            o.conn()?.close_queued = 1;
            0
        }
        OP_CC_WATCH_WRITE => {
            let events = o.sock()?.read(|s| what_event_want(s, SW_EVENT_WRITE));
            // arg: the socket is out of the reactor
            change(o.sock()?, u32::from(arg == 0), events)?
        }
        OP_CC_WEBSOCKET_FREE => {
            let conn = o.conn()?;
            // SAFETY: the connection's `new String`, not used again.
            unsafe { delete_string(conn.websocket_buffer.cast()) };
            conn.websocket_buffer = core::ptr::null_mut();
            0
        }
        OP_CC_RECV_FREE => {
            // free the reception memory buffer
            let sock = o.sock()?;
            let buffer = sock.read(|s| s.recv_buffer);
            // SAFETY: the socket's `new String`, not used again.
            unsafe { delete_string(buffer.cast()) };
            sock.edit(|s| s.recv_buffer = core::ptr::null_mut());
            0
        }
        OP_CC_HTTP_OBJECT => u64::from(o.port()?.open_http_protocol && !o.conn()?.object.is_null()),
        OP_CC_REDIS_OBJECT => {
            u64::from(o.port()?.open_redis_protocol && !o.conn()?.object.is_null())
        }
        OP_CC_REDIS_FREE => {
            // SAFETY: the redis request the connection owns, not used again.
            unsafe { swoole_rs_corebase::sw_free(o.conn()?.object) };
            0
        }
        OP_CC_LINGER => {
            let linger = libc::linger {
                l_onoff: 1,
                l_linger: 0,
            };
            let fd = o.conn_socket()?.fd();
            let ret =
                swoole_rs_corebase::sys::setsockopt(fd, libc::SOL_SOCKET, libc::SO_LINGER, &linger);
            u64::from(ret != 0)
        }
        OP_NOTIFY_CONNECT | OP_WR_NOTIFY_CLOSE | OP_WR_BUFFER_EMPTY => {
            let (conn, event) = match op {
                OP_NOTIFY_CONNECT => {
                    let conn = o.sock()?.read(|s| s.object).cast::<ConnectionRecord>();
                    // SAFETY: a session socket's object is null or its live connection.
                    (unsafe { conn.as_ref() }?, EVENT_CONNECT)
                }
                OP_WR_NOTIFY_CLOSE => (&*o.conn()?, EVENT_CLOSE),
                _ => {
                    let conn = o.conn()?;
                    conn.high_watermark = 0;
                    (&*conn, EVENT_BUFFER_EMPTY)
                }
            };
            crate::server_methods::notify(o.server(), conn, event);
            if op == OP_WR_NOTIFY_CLOSE {
                o.conn()?.close_notify = 0;
            }
            0
        }
        OP_SSL_ACCEPT => sign(o.sock().map_or(-1, tls::socket_ssl_accept)),
        OP_CC_SSL_SHUTDOWN => u64::from(o.sock().is_some_and(tls::socket_ssl_shutdown)),
        OP_CC_SSL_CLOSE => {
            if let Some(socket) = o.conn_socket() {
                let quiet = o.conn()?.peer_closed;
                socket.edit(|record| record.ssl_quiet_shutdown = quiet);
                tls::socket_ssl_close(socket);
            }
            0
        }
        op => return native(o, op, arg),
    })
}

/// The ops that call the timer, event loop and `Reactor::_close` under their
/// upstream symbols; else `None`. A reactor thread's own loop runs, and is freed,
/// here: master.cc no longer answers `OP_EVENT_WAIT` or `OP_EVENT_FREE`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[inline(always)]
fn native(o: &mut Objects<'_, '_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        OP_BASE_PASS, OP_SESSION_HANDLERS, OP_START_MASTER_THREAD, OP_STORE_LISTEN_SOCKET,
    };
    use crate::ops::{OP_CC_DTLS_FREE, OP_DTLS_ACCEPT, OP_RD_DTLS};
    use crate::ops::{
        OP_CC_FINISH, OP_EVENT_FREE, OP_EVENT_INIT, OP_EVENT_WAIT, OP_RESUME_READ,
        OP_WR_TIMER_DELAY,
    };
    use swoole_rs_reactor::event_api::swoole_event_wait;
    use swoole_rs_reactor::{swoole_event_free, swoole_event_init};
    use swoole_rs_timer::swoole_timer_delay;
    Some(match op {
        OP_DTLS_ACCEPT => {
            let (serv, reactor) = (o.ctx.serv, o.handle()?.ptr());
            let (single_thread, close) = (o.config().single_thread, o.reactor()?.close);
            let (port, pkt) = (o.conn()?.object, o.ctx.pkt.cast());
            // SAFETY: the context's live server and reactor, the DTLS port the packet
            // arrived on and the packet received, `arg` bytes.
            sign(unsafe {
                crate::dtls::on_packet(serv, single_thread, reactor, close, port, pkt, arg)
            })
        }
        OP_RD_DTLS => {
            let (sock, fd) = (o.sock()?, o.event()?.fd());
            // SAFETY: the context's live DTLS port; nothing else borrows its settings.
            let port = unsafe { o.ctx.port?.config_mut() };
            u64::from(crate::dtls::on_read(port, sock, fd))
        }
        OP_CC_DTLS_FREE => {
            // `Server::close_connection`'s `delete` of the socket's DTLS session.
            let fd = o.sock()?.fd();
            // SAFETY: the closing socket's live port; nothing else borrows its settings.
            let port = unsafe { o.ctx.port?.config_mut() };
            (crate::port_config::methods().dtls_free)(port, fd);
            0
        }
        OP_WR_TIMER_DELAY => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "C++'s implicit double to long"
            )]
            let delay = o.port()?.max_idle_time as c_long;
            let timer = o.sock()?.read(|s| s.output.send_timer);
            // SAFETY: the socket's live send timer.
            unsafe { swoole_timer_delay(timer.cast(), delay) };
            0
        }
        OP_RESUME_READ => {
            // The firing timer's reactor: this thread's.
            crate::reactor_handle::Reactor::thread().interest(o.sock()?, 2, 0);
            o.conn()?.timer = core::ptr::null_mut();
            0
        }
        OP_SESSION_HANDLERS => {
            let reactor = o.handle()?;
            for (fd_type, event, handler) in [
                (
                    FD_DGRAM_SERVER,
                    SW_EVENT_READ,
                    entry::on_packet_received as swoole_rs_reactor::Handler,
                ),
                (FD_SESSION, SW_EVENT_WRITE, entry::on_write),
                (FD_SESSION, SW_EVENT_READ, entry::on_read),
            ] {
                reactor.set_handler(fd_type, event, handler);
            }
            0
        }
        OP_BASE_PASS => {
            // SAFETY: the `SendData` (the context's task) the dispatch lends at arg.
            let send = unsafe { &*(arg as *const SendData) };
            crate::master_ops::base_pass(o.server(), send);
            0
        }
        OP_STORE_LISTEN_SOCKET => {
            crate::server_methods::store_listen_socket(o.server());
            0
        }
        OP_START_MASTER_THREAD => {
            let reactor = o.handle()?;
            sign(crate::server_methods::start_master_thread(
                o.ctx.serv, reactor,
            ))
        }
        OP_EVENT_INIT => {
            let ret = swoole_event_init(0);
            o.ctx.reactor = Some(crate::reactor_handle::Reactor::thread());
            sign(ret)
        }
        OP_EVENT_WAIT => sign(swoole_event_wait()),
        OP_EVENT_FREE => {
            swoole_event_free();
            0
        }
        OP_CC_FINISH => {
            let conn = o.conn()?;
            conn.active = 0;
            conn.closed.store(1, SeqCst);
            conn.socket = core::ptr::null_mut();
            conn.object = core::ptr::null_mut();
            let (reactor, sock) = (o.handle()?.ptr(), o.sock()?.as_ptr());
            // SAFETY: the closing socket, which nothing uses afterwards.
            sign(unsafe { swoole_rs_tls::reactor_write::reactor_close(reactor, sock) })
        }
        op => return crate::thread_ops::answer(o.ctx, o.state, op, arg),
    })
}

/// Outside the hybrid, whose unit tests link no C++, native code keeps these.
#[cfg(not(all(feature = "upstream-symbols", not(test))))]
#[inline(always)]
fn native(_: &mut Objects<'_, '_>, _: u32, _: u64) -> Option<u64> {
    None
}

/// `ReactorThread_onWrite`'s state after flushing.
fn write_state(o: &Objects<'_, '_>) -> Option<u64> {
    let conn = o.conn()?;
    let (out, buffer_size, timer, removed) = o.read_sock(|s| {
        let out = out_buffer(s).map(|b| (swr_cpp_buffer_length(b), swr_cpp_buffer_empty(b)));
        let output = &s.output;
        (out, output.buffer_size, output.send_timer, s.removed != 0)
    });
    let (length, out_empty) = out.unwrap_or((0, true));
    let buffer_empty = o
        .state
        .callbacks
        .slot(CB_BUFFER_EMPTY)
        .is_some_and(crate::callbacks::CallbackSlot::is_set);
    let low_watermark = o.port()?.buffer_low_watermark;
    let flag = |set: bool, bit: u64| if set { bit } else { 0 };
    Some(
        flag(conn.overflow != 0 && length < buffer_size, OVERFLOW_DRAINED)
            | flag(
                buffer_empty && conn.high_watermark != 0 && length <= low_watermark,
                LOW_WATERMARK,
            )
            | flag(!timer.is_null(), SEND_TIMER)
            | flag(out_empty, OUT_EMPTY)
            | flag(conn.peer_closed != 0, PEER_CLOSED)
            | flag(removed, REMOVED),
    )
}
