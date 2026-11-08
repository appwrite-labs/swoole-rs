//! The reactor-thread ops (master.cc's `reactor_thread_call`) that are a log line
//! or a read or write of a record Rust already lays out: the server's config,
//! tables and callbacks, the reactor's fields and the context's connection,
//! socket, event and pipe message. Rust answers them before any native call, so
//! master.cc no longer has a case for them. Logs name the upstream function that
//! wrote them.
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering::SeqCst;

use swoole_rs_buffer::cpp::{CppBuffer, swr_cpp_buffer_empty, swr_cpp_buffer_length};
use swoole_rs_corebase::{DataHead, log, microtime};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Event, SocketRecord, reactor_upstream};

use crate::callbacks::CB_BUFFER_EMPTY;
use crate::conn_io::{
    CLOSE_WAIT, LOW_WATERMARK, OUT_EMPTY, OVERFLOW_DRAINED, PEER_CLOSED, REMOVED, SEND_TIMER,
    SEND_WAIT,
};
use crate::ops::{
    LOG_CC_LINGER, LOG_NO_SESSION, LOG_RECVFROM, LOG_RT_CLOSED_BY_SERVER, LOG_RT_DISCARD,
    LOG_RT_SET_WRITE, OP_CC_SESSION_CLEAR, OP_CC_SSL_WANT, OP_CC_TIMER_DEL, OP_CC_VALID,
    OP_CONN_AT, OP_CONN_FD, OP_CONN_FORCE, OP_CONN_HANDSHAKING, OP_CONN_REACTOR, OP_CONN_READING,
    OP_CONN_SESSION, OP_HAVE_DGRAM, OP_MICROTIME, OP_PIPE_FD, OP_PIPE_TYPE, OP_QUEUE_FULL,
    OP_QUEUE_SUB, OP_REACTOR_ID, OP_REACTOR_NUM, OP_RT_BUF_EMPTY, OP_RT_DISCARD_TIMEOUT,
    OP_RT_FRONT_CONN, OP_RT_FRONT_CONN_CLOSED, OP_RT_FRONT_STREAM, OP_RT_HAS_PIPE_COMMAND,
    OP_RT_TOUCH_SEND, OP_SERVER, OP_SESSION_CONN, OP_SET_WAITING_TIME, OP_SSL_READY, OP_WAIT_EXIT,
    OP_WAITING_TIME, OP_WORKER_NUM, OP_WR_CLEAR_OVERFLOW, OP_WR_QUEUED, OP_WR_STATE,
    OP_WR_TIMER_DEL, OP_WR_WAIT,
};
use crate::port_config::PortConfig;
use crate::records::{ConnectionRecord, swr_v1_server_connection, swr_v1_server_session};
use crate::{ServerConfig, ServerState};

const SW_LOG_TRACE: c_int = 1;
const SW_LOG_NOTICE: c_int = 3;
const SW_LOG_WARNING: c_int = 4;
/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;

unsafe extern "C-unwind" {
    /// crates/timer's `swoole_timer_del`.
    #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
    fn swoole_timer_del(node: *mut c_void) -> bool;
}

/// master.cc's `ReactorThreadContext` up to its task, which Rust doesn't read;
/// master.cc asserts this layout.
#[repr(C)]
pub(crate) struct ReactorContext {
    serv: *mut c_void,
    reactor: *mut c_void,
    thread: *mut c_void,
    event: *mut Event,
    port: *mut c_void,
    sock: *mut SocketRecord,
    conn: *mut ConnectionRecord,
    timer: *mut c_void,
    rdata: *const c_void,
    /// The `PipeBuffer` read from a worker, which starts with its head.
    resp: *const DataHead,
    pkt: *mut c_void,
    /// The worker-pipe message at the front of the output buffer.
    front: *const DataHead,
    front_length: usize,
}

const _: () = assert!(
    core::mem::offset_of!(ReactorContext, front_length) == 96,
    "master.cc ReactorThreadContext layout"
);

/// The low 32 bits of `arg`, as native code passes a C `int`.
fn int(arg: u64) -> c_int {
    u32::try_from(arg & u64::from(u32::MAX))
        .unwrap_or_default()
        .cast_signed()
}

fn sign(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// `Server::is_stream_event`.
fn stream_event(kind: u8) -> bool {
    // RECV_DATA, SEND_DATA, SEND_FILE, CLOSE, CONNECT, PAUSE_RECV, RESUME_RECV,
    // BUFFER_FULL and BUFFER_EMPTY.
    matches!(kind, 0 | 2..=5 | 11..=14)
}

/// `Buffer::empty(buffer)`.
///
/// # Safety
/// `buffer` is null or a live `Buffer`.
unsafe fn empty(buffer: *mut c_void) -> bool {
    // SAFETY: per the caller.
    buffer.is_null() || unsafe { swr_cpp_buffer_empty(buffer.cast::<CppBuffer>()) }
}

/// `buffer ? buffer->length() : 0`.
///
/// # Safety
/// As `empty`.
unsafe fn length(buffer: *mut c_void) -> u32 {
    if buffer.is_null() {
        return 0;
    }
    // SAFETY: per the caller.
    unsafe { swr_cpp_buffer_length(buffer.cast::<CppBuffer>()) }
}

/// The context's objects, each live when the op reading it runs.
struct Objects<'a> {
    ctx: &'a mut ReactorContext,
    state: &'a ServerState,
}

#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written on the reactor thread that owns them as master.cc did"
)]
impl Objects<'_> {
    fn config(&self) -> &ServerConfig {
        // SAFETY: a live server's config.
        unsafe { &*self.state.config() }
    }
    fn conn(&self) -> &mut ConnectionRecord {
        // SAFETY: the ops reading it run with a live connection in the context.
        unsafe { &mut *self.ctx.conn }
    }
    fn conn_socket(&self) -> &SocketRecord {
        // SAFETY: a live connection's socket.
        unsafe { &*self.conn().socket.cast::<SocketRecord>() }
    }
    fn sock(&self) -> &mut SocketRecord {
        // SAFETY: the ops reading it run with a live socket in the context.
        unsafe { &mut *self.ctx.sock }
    }
    fn event(&self) -> &Event {
        // SAFETY: the ops reading it run on a live event.
        unsafe { &*self.ctx.event }
    }
    fn front(&self) -> &DataHead {
        // SAFETY: OP_RT_FRONT stored the live front message first.
        unsafe { &*self.ctx.front }
    }
    fn reactor(&self) -> &mut swoole_rs_reactor::ReactorFields {
        // SAFETY: the context's live reactor, which begins with its prepared state.
        let fields = unsafe { reactor_upstream::prepared(self.ctx.reactor) }.fields;
        // SAFETY: as above; only this thread touches its reactor.
        unsafe { &mut *fields }
    }
    fn port(&self) -> &PortConfig {
        // SAFETY: the context's live port.
        unsafe { crate::port_config::of(self.ctx.port) }
    }
    /// `get_connection(fd)` (kind 0) or a verified session's (kind 2, 3).
    fn connection(&self, key: c_long, kind: u32) -> *mut ConnectionRecord {
        // SAFETY: the server's live tables.
        unsafe { swr_v1_server_connection(self.state, key, kind) }
    }
}

/// `Server::is_valid_connection`.
///
/// # Safety
/// `conn` is null or a live connection whose socket is null or live.
unsafe fn valid(conn: *const ConnectionRecord) -> bool {
    // SAFETY: per the caller.
    let Some(conn) = (unsafe { conn.as_ref() }) else {
        return false;
    };
    // SAFETY: per the caller.
    let socket = unsafe { conn.socket.cast::<SocketRecord>().as_ref() };
    conn.active != 0 && socket.is_some_and(|socket| socket.fd_type == FD_SESSION)
}

fn error(level: c_int, code: E, origin: &core::ffi::CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// Answers `op` when it is one of this module's, else `None`. Always inlined: each
/// caller's constant op folds the match, so the hot runners' other ops cost nothing.
///
/// # Safety
/// `ctx` is master.cc's live context for the running operation and `state`
/// its server's; the objects each op reads are live, as for master.cc's case.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
#[inline(always)]
pub(crate) unsafe fn answer(
    ctx: *mut ReactorContext,
    state: *const ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    // SAFETY: per the caller; nothing else touches the context during the op.
    let ctx = unsafe { &mut *ctx };
    // SAFETY: per the caller.
    let state = unsafe { &*state };
    if state.config().is_null() || state.tables().is_null() {
        return None;
    }
    let o = Objects { ctx, state };
    Some(match op {
        LOG_RECVFROM => {
            let m = format_args!("recvfrom({}) failed", o.event().fd);
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
            let m = format_args!("Session#{} is closed by server", o.front().fd);
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
            let front = o.front();
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
            let m = format_args!("reactor->set({}) failed", o.event().fd);
            log::sys_warning(c"ReactorThread_onPipeWrite", m);
            0
        }
        LOG_CC_LINGER => {
            let m = format_args!("setsockopt(SO_LINGER) failed");
            log::sys_warning(c"Server::close_connection", m);
            0
        }
        OP_SSL_READY => {
            // SAFETY: a session socket's object is its live connection.
            unsafe { (*o.sock().object.cast::<ConnectionRecord>()).ssl_ready = 1 };
            0
        }
        OP_HAVE_DGRAM => u64::from(o.config().have_dgram_sock),
        OP_REACTOR_ID => u64::from(o.reactor().id),
        OP_REACTOR_NUM => u64::from(o.config().reactor_num),
        OP_WORKER_NUM => u64::from(o.config().worker_num),
        OP_CONN_AT | OP_CC_VALID => {
            let conn = o.connection(c_long::from(int(arg)), 0);
            // SAFETY: a table slot, whose socket is null or live.
            if !unsafe { valid(conn) } {
                return Some(0);
            }
            if op == OP_CONN_AT {
                o.ctx.conn = conn;
            }
            1
        }
        OP_CONN_FD => sign(o.conn().fd),
        OP_CONN_REACTOR => u64::from(o.conn().reactor_id),
        OP_CONN_SESSION => sign(o.conn().session_id),
        OP_CONN_READING => u64::from(o.conn().peer_closed == 0 && o.conn_socket().removed == 0),
        OP_WAIT_EXIT => {
            o.reactor().wait_exit = true;
            0
        }
        OP_SESSION_CONN | OP_RT_FRONT_CONN => {
            // The front's fd is the session id.
            let (key, kind) = if op == OP_SESSION_CONN {
                (arg.cast_signed(), 2)
            } else {
                (o.front().fd, 3)
            };
            o.ctx.conn = o.connection(key, kind);
            u64::from(!o.ctx.conn.is_null())
        }
        OP_CONN_FORCE => u64::from(o.conn().close_force),
        OP_CONN_HANDSHAKING => u64::from(o.conn().ssl != 0 && o.conn().ssl_ready == 0),
        // SAFETY: OP_PIPE_READ stored the live message first.
        OP_PIPE_TYPE => u64::from(unsafe { (*o.ctx.resp).kind }),
        // SAFETY: as above.
        OP_PIPE_FD => sign(unsafe { (*o.ctx.resp).fd }),
        OP_MICROTIME => microtime().to_bits(),
        OP_QUEUE_FULL => {
            u64::from(o.conn().recv_queued_bytes.load(SeqCst) > o.config().max_queued_bytes)
        }
        OP_WAITING_TIME => u64::from(o.conn().waiting_time),
        OP_SET_WAITING_TIME => {
            o.conn().waiting_time = u16::try_from(arg & 0xffff).unwrap_or_default();
            0
        }
        OP_QUEUE_SUB => {
            let bytes = u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default();
            o.conn().recv_queued_bytes.fetch_sub(bytes, SeqCst);
            0
        }
        OP_RT_BUF_EMPTY => {
            // SAFETY: the event's live socket.
            let socket = unsafe { &*o.event().socket.cast::<SocketRecord>() };
            // SAFETY: its output buffer is null or live.
            u64::from(unsafe { empty(socket.output.out_buffer) })
        }
        OP_RT_FRONT_STREAM => u64::from(stream_event(o.front().kind)),
        OP_RT_TOUCH_SEND => {
            o.conn().last_send_time = microtime();
            0
        }
        OP_RT_FRONT_CONN_CLOSED => u64::from(o.conn().closed.load(SeqCst)),
        OP_RT_DISCARD_TIMEOUT => u64::from(o.config().discard_timeout_request),
        OP_SERVER => u64::try_from(o.ctx.serv.expose_provenance()).unwrap_or(0),
        OP_RT_HAS_PIPE_COMMAND => u64::from(!o.config().pipe_command.is_null()),
        OP_WR_WAIT => {
            let output = &o.sock().output;
            (if output.close_wait != 0 {
                CLOSE_WAIT
            } else {
                0
            }) | (if output.send_wait != 0 { SEND_WAIT } else { 0 })
        }
        OP_WR_CLEAR_OVERFLOW => {
            o.conn().overflow = 0;
            0
        }
        OP_WR_QUEUED => {
            // SAFETY: the socket's output buffer is null or live.
            o.conn().send_queued_bytes = unsafe { length(o.sock().output.out_buffer) };
            0
        }
        OP_WR_STATE => write_state(&o),
        OP_WR_TIMER_DEL => {
            let output = &mut o.sock().output;
            // SAFETY: the socket's live send timer.
            unsafe { swoole_timer_del(output.send_timer) };
            output.send_timer = core::ptr::null_mut();
            0
        }
        OP_CC_TIMER_DEL => {
            let conn = o.conn();
            // SAFETY: the connection's live timer.
            unsafe { swoole_timer_del(conn.timer) };
            conn.timer = core::ptr::null_mut();
            0
        }
        OP_CC_SSL_WANT => {
            let tls = o.sock().tls;
            u64::from(tls.want_read != 0 || tls.want_write != 0)
        }
        OP_CC_SESSION_CLEAR => {
            // SAFETY: the server's live session table.
            let session = unsafe { swr_v1_server_session(state, o.conn().session_id) };
            // SAFETY: a live connection's slot in it.
            unsafe { (*session).fd = 0 };
            0
        }
        _ => return None,
    })
}

/// `ReactorThread_onWrite`'s state after flushing.
fn write_state(o: &Objects<'_>) -> u64 {
    let (sock, conn) = (o.sock(), o.conn());
    // SAFETY: the socket's output buffer is null or live.
    let length = unsafe { length(sock.output.out_buffer) };
    let buffer_empty = o
        .state
        .callbacks
        .slot(CB_BUFFER_EMPTY)
        .is_some_and(crate::callbacks::CallbackSlot::is_set);
    let flag = |set: bool, bit: u64| if set { bit } else { 0 };
    flag(conn.overflow != 0 && length < sock.output.buffer_size, OVERFLOW_DRAINED)
        | flag(
            buffer_empty && conn.high_watermark != 0 && length <= o.port().buffer_low_watermark,
            LOW_WATERMARK,
        )
        | flag(!sock.output.send_timer.is_null(), SEND_TIMER)
        // SAFETY: as above.
        | flag(unsafe { empty(sock.output.out_buffer) }, OUT_EMPTY)
        | flag(conn.peer_closed != 0, PEER_CLOSED)
        | flag(sock.removed != 0, REMOVED)
}
