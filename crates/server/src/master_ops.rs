//! The ops of master.cc's runner (`Server::rust_call`) that are a log line or a
//! read or write of a record Rust already lays out: the server's config, tables
//! and shared counters, and the context's connection, socket and port. Rust
//! answers them before any native call, so master.cc no longer has a case for
//! them. Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_char, c_int};
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{SocketRecord, reactor_upstream};

use crate::callbacks::{
    CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CLOSE, CB_CONNECT, CB_RECEIVE, CB_TASK,
};
use crate::ops::{
    LOG_ACCEPT_FAILED, LOG_ALLOC_PORT_GS, LOG_ALLOC_SESSIONS, LOG_ALLOC_TASK_RESULTS,
    LOG_ALLOC_WORKERS, LOG_BASE_ALLOC_CONNECTIONS, LOG_BASE_ALLOC_NUMS, LOG_BASE_ALLOC_PORT_NUMS,
    LOG_CALLBACK_DISPATCH, LOG_CALLBACK_REQUIRED, LOG_FDS_START, LOG_FILE_EMPTY, LOG_HOST_TOO_LONG,
    LOG_INVALID_PORT, LOG_INVALID_SESSION, LOG_LC_ONLY_ONE, LOG_LISTEN_PID, LOG_MANAGER_EXIT,
    LOG_MAX_CONNECTION_CREATED, LOG_MAX_CONNECTION_MINIMUM, LOG_MAX_CONNECTION_SESSIONS,
    LOG_NO_IDLE_WORKER, LOG_NO_SESSION_SLOT, LOG_NODELAY, LOG_NOT_REGULAR, LOG_PORT_CREATED,
    LOG_RCVBUF, LOG_REACTOR_NUM, LOG_SC_CLOSED_BY_CLIENT, LOG_SC_CONN_OVERFLOW,
    LOG_SC_NO_SESSION_DATA, LOG_SC_NO_SESSION_EVENT, LOG_SC_SOCKET_OVERFLOW, LOG_SEND_IN_MASTER,
    LOG_SENDFILE_NAME, LOG_SENDWAIT, LOG_SHUTDOWN_FAILED, LOG_SNDBUF, LOG_ST_ROOT_MISSING,
    LOG_ST_ROOT_TOO_LONG, LOG_STAT, LOG_TASK_POOL, LOG_TASK_WORKER_NUM, LOG_TOO_MANY_CONNECTIONS,
    LOG_TOO_MANY_PORTS, LOG_WORKER_NUM, OP_ACCEPT, OP_BASE_REACTORS, OP_CHUNK_SIZE,
    OP_CLEAR_MAX_QUEUED, OP_CLEAR_SEND_YIELD, OP_CONN_CLOSING, OP_CONN_PROTECT, OP_CONN_RESET,
    OP_CONN_UID, OP_CONN_WORKER, OP_COUNT_ACCEPT, OP_DGRAM_PORT, OP_DIRECT_SEND, OP_EVENT_FD,
    OP_FD_CONN, OP_HAS_PIPE_COMMAND, OP_HOST_MAXSIZE, OP_HOST_SIZE, OP_IS_CREATED, OP_IS_MASTER,
    OP_KILL_TERM, OP_KILL_WINCH, OP_LAST_RECEIVED, OP_LAST_SENT, OP_NEW_PORT_TYPE,
    OP_NODELAY_ENABLED, OP_PIN_WORKER, OP_PORT_IDLE_TIME, OP_PORT_NODELAY, OP_PORT_NUMBER,
    OP_PORT_PUSH, OP_PORT_RCVBUF, OP_PORT_SNDBUF, OP_PORT_SSL, OP_PORT_TYPE, OP_REACTOR_ADD_READ,
    OP_SC_ADD, OP_SC_ADD_READ, OP_SC_ADD_WRITE, OP_SC_ADVANCE, OP_SC_DEL, OP_SC_HAS_SEND_TIMER,
    OP_SC_HIGH_WATERMARK, OP_SC_OVERFLOW, OP_SC_REMOVE_READ, OP_SC_SEND, OP_SC_SET,
    OP_SC_TOUCH_SEND, OP_SC_WRITABLE, OP_SC_YIELD_ERROR, OP_SEND_SYNC, OP_SEND_TIMEOUT,
    OP_SEND_TIMEOUT_MIN, OP_SENDFILE_NAME, OP_SENDFILE_NAME_LEN, OP_SENDFILE_NAME_MAX,
    OP_SENDFILE_OFFSET, OP_SESSION_BIND, OP_SESSION_CLAIM, OP_SET_DISABLE_NOTIFY,
    OP_SET_MASTER_PID, OP_SET_NODELAY, OP_SET_RCVBUF, OP_SET_SNDBUF, OP_SIGNAL_FD, OP_SIGRTMIN,
    OP_SOCK_FD, OP_SOCK_SSL, OP_SOCKET_CONN, OP_SOCKET_NOBUFS, OP_SPIN_LOCK, OP_SPIN_UNLOCK,
    OP_SSL_CLEAR, OP_ST_ABORT_COUNT, OP_ST_ROOT_PATH, OP_ST_SOCK_OBJECT, OP_ST_TIMER,
    OP_ST_W_CONCURRENCY, OP_ST_W_CONCURRENCY_CAS, OP_ST_W_RESET_COUNTS, OP_STATE, OP_STREAM_PORT,
    OP_TASKING, OP_TASKING_RESET, OP_VERIFY_CONN, OP_WARNING, OP_WARNING_RESET, OP_WARNING_TIME,
    OP_WORKER_CONCURRENCY, OP_WORKER_COROUTINES, OP_WRONG_OPERATION,
};
use crate::ops::{
    OP_AFTER_CREATE, OP_AFTER_SHUTDOWN, OP_ALLOC_CONNECTION_NUMS, OP_ALLOC_PORT_CONNECTION_NUMS,
    OP_ALLOC_PORT_GS, OP_ALLOC_SESSIONS, OP_ALLOC_TASK_RESULTS, OP_ALLOC_WORKERS,
    OP_FREE_USER_WORKERS, OP_LC_BEFORE_START, OP_MASTER_REACTOR, OP_MASTER_WORKER,
    OP_SIGNAL_ACTIVE,
};
use crate::port_config::PortConfig;
use crate::records::{ConnectionRecord, SessionRecord};
use crate::server_methods::NativeContext;
use crate::shared::{PortCounters, ServerCounters};
use crate::{MODE_BASE, MODE_PROCESS, MODE_THREAD, ServerState};
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::hook;
use swoole_rs_memory::members::{sw_shm_calloc, sw_shm_free};
use swoole_rs_process::worker_record::WorkerRecord;

const SW_LOG_TRACE: c_int = 1;
const SW_LOG_NOTICE: c_int = 3;
const SW_LOG_WARNING: c_int = 4;
const SW_LOG_ERROR: c_int = 5;
const SW_SOCK_TCP: c_int = 1;
const SW_SOCK_TCP6: c_int = 3;
/// `SW_MASTER`, the master's worker type.
const SW_MASTER: c_int = 1;
const SW_EVENT_READ: c_int = 1 << 9;
/// `SW_FD_STREAM_SERVER`.
const SW_FD_STREAM_SERVER: c_int = 1;
/// `SW_GLOBAL_HOOK_BEFORE_SERVER_START`, `_AFTER_SERVER_SHUTDOWN` and `_AFTER_SERVER_CREATE`.
const HOOK_BEFORE_SERVER_START: c_int = 0;
const HOOK_AFTER_SERVER_SHUTDOWN: c_int = 7;
const HOOK_AFTER_SERVER_CREATE: c_int = 11;
const SW_EVENT_WRITE: c_int = 1 << 10;
/// `sizeof(Session)`, `sizeof(sw_atomic_t)` and `sizeof(SendfileTask)`.
const SESSION_SIZE: usize = size_of::<SessionRecord>();
const ATOMIC_SIZE: usize = 4;
const SENDFILE_TASK_SIZE: usize = 16;
// master.cc asserts the native sizes of the arrays allocated here.
const _: () = assert!(
    SESSION_SIZE == 16 && size_of::<PortCounters>() == 80,
    "master.cc sizeof(Session), sizeof(ServerPortGS)"
);
const IPC_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;

/// `SW_HOST_MAXSIZE`: `sizeof(sockaddr_un::sun_path)`.
fn host_maxsize() -> usize {
    // SAFETY: a plain C struct, for which zero is valid.
    let addr: libc::sockaddr_un = unsafe { core::mem::zeroed() };
    size_of_val(&addr.sun_path)
}

fn error(level: c_int, code: E, origin: &CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// A NUL-terminated native string, or "" for null.
///
/// # Safety
/// `text` is null or a live NUL-terminated string.
unsafe fn text<'a>(text: *const c_char) -> std::borrow::Cow<'a, str> {
    if text.is_null() {
        return "".into();
    }
    // SAFETY: per the caller.
    unsafe { CStr::from_ptr(text) }.to_string_lossy()
}

/// `if (swoole_isset_hook(kind)) swoole_call_hook(kind, serv)`.
fn call_hook(kind: c_int, serv: *mut core::ffi::c_void) {
    if hook::swoole_isset_hook(kind) {
        // SAFETY: the server hooks take the server.
        unsafe { hook::swoole_call_hook(kind, serv) };
    }
}

/// A `ListenPort`'s settings.
///
/// # Safety
/// `port` is a live `ListenPort`.
unsafe fn port_config<'a>(port: *mut core::ffi::c_void) -> &'a mut PortConfig {
    // SAFETY: a `ListenPort` starts with its settings pointer (shared.rs).
    let config = unsafe { *port.cast::<*mut PortConfig>() };
    // SAFETY: the port's settings live as long as it.
    unsafe { &mut *config }
}

/// `sw_shm_calloc(count, size)` as a typed pointer.
fn shm_calloc<T>(count: usize, size: usize) -> *mut T {
    sw_shm_calloc(count, size).cast()
}

fn sign(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// The context's objects, each null when the op doesn't use it.
struct Objects<'a> {
    ctx: &'a mut NativeContext,
    state: &'a ServerState,
}

#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written under the server's own synchronization as master.cc did"
)]
impl Objects<'_> {
    fn config(&self) -> &mut crate::ServerConfig {
        // SAFETY: a live server's config, written only under its own synchronization.
        unsafe { &mut *self.state.config() }
    }
    fn tables(&self) -> &mut crate::ServerTables {
        // SAFETY: as `config`.
        unsafe { &mut *self.state.tables() }
    }
    fn gs(&self) -> &mut ServerCounters {
        let gs = self.config().gs.cast::<ServerCounters>();
        // SAFETY: the server's shared counters, mapped for its life.
        unsafe { &mut *gs }
    }
    fn conn(&self) -> &mut ConnectionRecord {
        // SAFETY: the ops reading it run with a live connection in the context.
        unsafe { &mut *self.ctx.conn.cast::<ConnectionRecord>() }
    }
    fn sock(&self) -> &mut SocketRecord {
        // SAFETY: the ops reading it run with a live socket in the context.
        unsafe { &mut *self.ctx.sock.cast::<SocketRecord>() }
    }
    fn conn_socket(&self) -> &SocketRecord {
        // SAFETY: a live connection's socket, which the ops reading it have.
        unsafe { &*self.conn().socket.cast::<SocketRecord>() }
    }
    fn port(&self) -> &mut PortConfig {
        // SAFETY: a `ListenPort` starts with its settings pointer (shared.rs).
        let config = unsafe { *self.ctx.port.cast::<*mut PortConfig>() };
        // SAFETY: the port's settings live as long as it.
        unsafe { &mut *config }
    }
    fn port_gs(&self) -> &PortCounters {
        // SAFETY: the port's slot in `port_gs_list`, once the server is created.
        unsafe { &*self.port().gs.cast::<PortCounters>() }
    }
    fn worker(&self) -> &mut WorkerRecord {
        // SAFETY: the ops reading it run with a live worker as the context's object.
        unsafe { &mut *self.ctx.object.cast::<WorkerRecord>() }
    }
    fn data(&self) -> std::borrow::Cow<'_, str> {
        // SAFETY: the ops reading it pass a NUL-terminated string.
        unsafe { text(self.ctx.data.cast()) }
    }
    /// `SendfileTask::filename` in the context's buffer.
    fn filename(&self) -> std::borrow::Cow<'_, str> {
        // SAFETY: the buffer is a `SendfileTask`, whose name follows its 16-byte head.
        unsafe { text(self.ctx.buffer.wrapping_add(SENDFILE_TASK_SIZE)) }
    }
}

/// Answers `op` when it is one of this module's, else `None`.
///
/// # Safety
/// `ctx` is master.cc's live context for the running operation and `state`
/// its server's; the objects each op reads are live, as for master.cc's case.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
pub(crate) unsafe fn answer(
    ctx: *mut NativeContext,
    state: *const ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    if ctx.is_null() || state.is_null() {
        return None;
    }
    // SAFETY: per the caller; nothing else touches the context during the op.
    let ctx = unsafe { &mut *ctx };
    // SAFETY: per the caller.
    let state = unsafe { &*state };
    if state.config().is_null() || state.tables().is_null() {
        return None;
    }
    let o = Objects { ctx, state };
    let cpus = c_int::from(swoole_rs_corebase::swr_v1_core_cpu_num());
    Some(match op {
        // Logs.
        LOG_LC_ONLY_ONE => {
            let m = format_args!("can only start one server");
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_ONLY_START_ONE,
                c"Server::start",
                m,
            );
            0
        }
        LOG_ACCEPT_FAILED => {
            log::sys_warning(
                c"Server::accept_connection",
                format_args!("accept() failed"),
            );
            0
        }
        LOG_TOO_MANY_CONNECTIONS => {
            let m = format_args!("Too many connections [now: {}]", o.sock().fd);
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_TOO_MANY_SOCKET,
                c"Server::accept_connection",
                m,
            );
            0
        }
        LOG_NO_SESSION_SLOT => {
            let m = format_args!("no available session slot, fd={}", o.sock().fd);
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_TOO_MANY_SOCKET,
                c"Server::add_connection",
                m,
            );
            0
        }
        LOG_NODELAY => {
            let m = format_args!("setsockopt(TCP_NODELAY) failed");
            log::sys_warning(c"Server::add_connection", m);
            0
        }
        LOG_RCVBUF => {
            let size = o.port().kernel_socket_recv_buffer_size;
            let m = format_args!("setsockopt(SO_RCVBUF, {size}) failed");
            log::sys_warning(c"Server::add_connection", m);
            0
        }
        LOG_SNDBUF => {
            let size = o.port().kernel_socket_send_buffer_size;
            let m = format_args!("setsockopt(SO_SNDBUF, {size}) failed");
            log::sys_warning(c"Server::add_connection", m);
            0
        }
        LOG_MAX_CONNECTION_CREATED => {
            let m = format_args!("max_connection must be set before server create");
            log::warning(c"Server::set_max_connection", m);
            0
        }
        LOG_MAX_CONNECTION_SESSIONS => {
            let m = format_args!(
                "max_connection is exceed the SW_SESSION_LIST_SIZE, it's reset to 1048576"
            );
            log::warning(c"Server::set_max_connection", m);
            0
        }
        LOG_SHUTDOWN_FAILED => {
            let pid = arg as libc::pid_t;
            let m = format_args!("failed to shutdown, kill({pid}, SIGTERM) failed");
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SYSTEM_CALL_FAIL,
                c"Server::shutdown",
                m,
            );
            0
        }
        LOG_MANAGER_EXIT => {
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (int) arg")]
            let status = arg as c_int;
            let signal = swoole_rs_process::signal::swoole_signal_to_str(libc::WTERMSIG(status));
            // SAFETY: the static text `swoole_signal_to_str` just wrote.
            let signal = unsafe { text(signal) };
            let exit = libc::WEXITSTATUS(status);
            let m =
                format_args!("Fatal Error: manager process exit. status={exit}, signal=[{signal}]");
            log::warning(c"Server::signal_handler_child_exit", m);
            0
        }
        LOG_NO_IDLE_WORKER => {
            let m = if arg != 0 {
                format_args!("No idle task worker is available")
            } else {
                format_args!("No idle worker is available")
            };
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_NO_IDLE_WORKER,
                c"Server::timer_callback",
                m,
            );
            0
        }
        LOG_ALLOC_SESSIONS => {
            let m = format_args!("sw_shm_calloc(1048576, {SESSION_SIZE}) for session_list failed");
            log::sys_warning(c"Server::create", m);
            0
        }
        LOG_ALLOC_PORT_GS => {
            let (ports, size) = (o.tables().ports.count, size_of::<PortCounters>());
            let m =
                format_args!("sw_shm_calloc({ports}, {size}) for port_connection_num_array failed");
            log::sys_warning(c"Server::create", m);
            0
        }
        LOG_ALLOC_WORKERS => {
            let (workers, size) = (
                o.config().worker_num.cast_signed(),
                size_of::<swoole_rs_process::worker_record::WorkerRecord>(),
            );
            let m = format_args!("sw_shm_calloc({workers}, {size}) for workers failed");
            log::sys_warning(c"Server::create", m);
            0
        }
        LOG_MAX_CONNECTION_MINIMUM => {
            #[expect(clippy::cast_possible_truncation, reason = "the low half of arg")]
            let (minimum, reset) = (arg >> 32, arg as u32);
            let m =
                format_args!("max_connection must be bigger than {minimum}, it's reset to {reset}");
            log::warning(c"Server::create", m);
            0
        }
        LOG_REACTOR_NUM => {
            let (now, max) = (o.config().reactor_num, cpus * 4);
            let m =
                format_args!("reactor_num == {now}, Too many threads, reset to max value {max}");
            log::warning(c"Server::create", m);
            0
        }
        LOG_WORKER_NUM => {
            let (now, max) = (o.config().worker_num.cast_signed(), cpus * 1000);
            let m =
                format_args!("worker_num == {now}, Too many processes, reset to max value {max}");
            log::warning(c"Server::create", m);
            0
        }
        LOG_TASK_WORKER_NUM => {
            let (now, max) = (o.config().task_worker_num.cast_signed(), cpus * 1000);
            let m = format_args!(
                "task_worker_num == {now}, Too many processes, reset to max value {max}"
            );
            log::warning(c"Server::create", m);
            0
        }
        LOG_TASK_POOL => {
            let m = format_args!("[Master] create task_workers failed");
            log::warning(c"Server::create_task_workers", m);
            0
        }
        LOG_ALLOC_TASK_RESULTS => {
            let (workers, size) = (
                o.config().worker_num.cast_signed(),
                swoole_rs_corebase::constants::SW_IPC_MAX_SIZE,
            );
            let m = format_args!("sw_shm_calloc({workers}, {size}) for task_result failed");
            log::warning(c"Server::create_task_workers", m);
            0
        }
        LOG_CALLBACK_DISPATCH => {
            let event = match u32::try_from(arg).unwrap_or(u32::MAX) {
                CB_CONNECT => "onConnect",
                CB_CLOSE => "onClose",
                CB_BUFFER_FULL => "onBufferFull",
                CB_BUFFER_EMPTY => "onBufferEmpty",
                _ => return Some(0),
            };
            let mode = o.config().dispatch_mode;
            let m = format_args!("cannot set '{event}' event when using dispatch_mode={mode}");
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_INVALID_CALLBACK,
                c"Server::start_check",
                m,
            );
            0
        }
        LOG_CALLBACK_REQUIRED => {
            let event = match u32::try_from(arg).unwrap_or(u32::MAX) {
                CB_TASK => "onTask",
                CB_RECEIVE => "onReceive",
                _ => "onPacket",
            };
            let m = format_args!("require '{event}' callback");
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_INVALID_CALLBACK,
                c"Server::start_check",
                m,
            );
            0
        }
        LOG_SENDWAIT => {
            let (length, session) = (o.ctx.length.cast_signed(), arg.cast_signed());
            let m =
                format_args!("send {length} byte failed, because session#{session} is not exists");
            error(
                SW_LOG_TRACE,
                E::SW_ERROR_SESSION_NOT_EXIST,
                c"Server::sendwait",
                m,
            );
            0
        }
        LOG_INVALID_SESSION => {
            let m = format_args!("invalid fd[{}]", o.ctx.session_id);
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SESSION_INVALID_ID,
                c"Server::sendfile",
                m,
            );
            0
        }
        LOG_SEND_IN_MASTER => {
            let m = format_args!("can't send data to the connections in master process");
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_SEND_IN_MASTER,
                c"Server::sendfile",
                m,
            );
            0
        }
        LOG_SENDFILE_NAME => {
            // `%.8s`: at most 8 bytes of the name.
            // SAFETY: sendfile's name is a NUL-terminated string.
            let name = unsafe { CStr::from_ptr(o.ctx.data.cast()) }.to_bytes();
            let head = String::from_utf8_lossy(&name[..name.len().min(8)]);
            let (length, max) = (o.ctx.length, IPC_BUFFER_SIZE - SENDFILE_TASK_SIZE - 1);
            let m = format_args!(
                "sendfile name[{head}...] length {length} is exceed the max name len {max}"
            );
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_NAME_TOO_LONG,
                c"Server::sendfile",
                m,
            );
            0
        }
        LOG_STAT => {
            let m = format_args!("stat({}) failed", o.filename());
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SYSTEM_CALL_FAIL,
                c"Server::sendfile",
                m,
            );
            0
        }
        LOG_NOT_REGULAR => {
            let m = format_args!("the path[{}] given is not a regular file", o.filename());
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_IS_NOT_REGULAR_FILE,
                c"Server::sendfile",
                m,
            );
            0
        }
        LOG_FILE_EMPTY => {
            let m = format_args!("file[offset={}] is empty", o.ctx.offset);
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SYSTEM_CALL_FAIL,
                c"Server::sendfile",
                m,
            );
            0
        }
        LOG_PORT_CREATED => {
            let m = format_args!("must add port before server is created");
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_WRONG_OPERATION,
                c"Server::add_port",
                m,
            );
            0
        }
        LOG_TOO_MANY_PORTS => {
            let m = format_args!("up to 60000 listening ports are allowed");
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_TOO_MANY_LISTEN_PORT,
                c"Server::add_port",
                m,
            );
            0
        }
        LOG_INVALID_PORT => {
            let m = format_args!("invalid port [{}]", o.ctx.number);
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_INVALID_LISTEN_PORT,
                c"Server::add_port",
                m,
            );
            0
        }
        LOG_HOST_TOO_LONG => {
            // SAFETY: add_port's host is a NUL-terminated string.
            let host = unsafe { text(o.ctx.host) };
            let m = format_args!(
                "address '{host}' exceeds the limit of {} characters",
                host_maxsize() - 1
            );
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_NAME_TOO_LONG,
                c"Server::add_port",
                m,
            );
            0
        }
        LOG_LISTEN_PID => {
            log::warning(
                c"Server::add_systemd_socket",
                format_args!("invalid LISTEN_PID"),
            );
            0
        }
        LOG_FDS_START => {
            log::warning(
                c"Server::add_systemd_socket",
                format_args!("invalid LISTEN_FDS_START"),
            );
            0
        }
        LOG_SC_NO_SESSION_DATA => {
            let (length, session) = (o.ctx.length.cast_signed(), o.ctx.session_id);
            let m = format_args!("send {length} byte failed, session#{session} does not exist");
            error(
                SW_LOG_TRACE,
                E::SW_ERROR_SESSION_NOT_EXIST,
                c"Server::send_to_connection",
                m,
            );
            0
        }
        LOG_SC_NO_SESSION_EVENT => {
            // SAFETY: send_to_connection's context holds its send data.
            let kind = unsafe {
                (*o.ctx.send.cast::<crate::base_factory::SendData>())
                    .info
                    .kind
            };
            let m = format_args!(
                "send event[{kind}] failed, session#{} does not exist",
                o.ctx.session_id
            );
            error(
                SW_LOG_TRACE,
                E::SW_ERROR_SESSION_NOT_EXIST,
                c"Server::send_to_connection",
                m,
            );
            0
        }
        LOG_SC_SOCKET_OVERFLOW => {
            let m = format_args!("socket#{} output buffer overflow", o.conn().fd);
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_OUTPUT_BUFFER_OVERFLOW,
                c"Server::send_to_connection",
                m,
            );
            0
        }
        LOG_SC_CLOSED_BY_CLIENT => {
            let m = format_args!("socket#{} is closed by client", o.conn().fd);
            error(
                SW_LOG_NOTICE,
                E::SW_ERROR_SESSION_CLOSED_BY_CLIENT,
                c"Server::send_to_connection",
                m,
            );
            0
        }
        LOG_SC_CONN_OVERFLOW => {
            let m = format_args!("connection#{} output buffer overflow", o.conn().fd);
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_OUTPUT_BUFFER_OVERFLOW,
                c"Server::send_to_connection",
                m,
            );
            0
        }
        LOG_ST_ROOT_TOO_LONG => {
            let m = format_args!(
                "The length of document_root must be less than {}",
                libc::PATH_MAX
            );
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_NAME_TOO_LONG,
                c"Server::set_document_root",
                m,
            );
            0
        }
        LOG_ST_ROOT_MISSING => {
            let m = format_args!("document_root[{}] does not exist", o.data());
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_DIR_NOT_EXIST,
                c"Server::set_document_root",
                m,
            );
            0
        }
        LOG_BASE_ALLOC_CONNECTIONS => {
            let size = o.tables().max_connection as usize * size_of::<ConnectionRecord>();
            log::sys_warning(
                c"Server::create_base_factory",
                format_args!("calloc[2]({size}) failed"),
            );
            0
        }
        LOG_BASE_ALLOC_NUMS | LOG_BASE_ALLOC_PORT_NUMS => {
            let size = o.config().worker_num as usize * ATOMIC_SIZE;
            let what = if op == LOG_BASE_ALLOC_NUMS {
                "gs"
            } else {
                "port"
            };
            let m = format_args!("sw_shm_calloc({size}) for {what}->connection_nums failed");
            log::sys_warning(c"Server::create_base_factory", m);
            0
        }

        // The server's own records.
        OP_SIGRTMIN => sigrtmin(),
        OP_SIGNAL_FD => sign(swoole_rs_corebase::swr_v1_core_signal_fd()),
        OP_WRONG_OPERATION => {
            log::set_last_error(E::SW_ERROR_WRONG_OPERATION as c_int);
            0
        }
        // The task pool's are native.
        OP_WARNING if arg == 0 => u64::from(o.config().scheduler_warning),
        OP_WARNING_TIME if arg == 0 => sign(o.config().warning_time),
        OP_WARNING_RESET if arg & 1 == 0 => {
            o.config().scheduler_warning = false;
            o.config().warning_time = (arg >> 1).cast_signed() as libc::time_t;
            0
        }
        OP_SET_DISABLE_NOTIFY => {
            o.config().disable_notify = true;
            0
        }
        OP_CLEAR_SEND_YIELD => {
            o.config().send_yield = false;
            0
        }
        OP_CLEAR_MAX_QUEUED => {
            o.config().max_queued_bytes = 0;
            0
        }
        OP_SEND_TIMEOUT => o.config().send_timeout.to_bits(),
        OP_SEND_TIMEOUT_MIN => {
            o.config().send_timeout = 0.001;
            0
        }
        OP_HAS_PIPE_COMMAND => u64::from(!o.config().pipe_command.is_null()),
        OP_STREAM_PORT => {
            o.config().have_stream_sock = true;
            0
        }
        OP_BASE_REACTORS => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "C++'s uint16_t reactor_num"
            )]
            let reactors = o.config().worker_num as u16;
            o.config().reactor_num = reactors;
            0
        }
        OP_ST_TIMER => u64::from(match arg {
            0 => !o.config().master_timer.is_null(),
            1 => !o.config().heartbeat_timer.is_null(),
            _ => !o.tables().enable_accept_timer.is_null(),
        }),
        OP_SET_MASTER_PID => {
            // SAFETY: getpid has no preconditions.
            o.gs().master_pid = unsafe { libc::getpid() };
            0
        }
        OP_TASKING => sign(o.gs().tasking_num.load(SeqCst).cast_signed()),
        OP_TASKING_RESET => {
            o.gs().tasking_num.store(0, SeqCst);
            0
        }
        OP_KILL_TERM | OP_KILL_WINCH => {
            let pid = arg as libc::pid_t;
            let signal = if op == OP_KILL_TERM {
                libc::SIGTERM
            } else {
                libc::SIGWINCH
            };
            // SAFETY: kill has no memory preconditions.
            sign(unsafe { libc::kill(pid, signal) })
        }

        // The context's port.
        OP_PORT_SSL => u64::from(o.port().ssl),
        OP_PORT_IDLE_TIME => o.port().max_idle_time.to_bits(),
        OP_PORT_RCVBUF => sign(o.port().kernel_socket_recv_buffer_size),
        OP_PORT_SNDBUF => sign(o.port().kernel_socket_send_buffer_size),
        OP_PORT_NODELAY => {
            let port = o.port();
            let tcp = || {
                // SAFETY: a port with its listening socket.
                let socket = unsafe { &*port.socket.cast::<SocketRecord>() };
                matches!(socket.socket_type, SW_SOCK_TCP | SW_SOCK_TCP6)
            };
            u64::from(port.open_tcp_nodelay && tcp())
        }
        OP_NEW_PORT_TYPE => sign(o.port().r#type),
        OP_PORT_PUSH => {
            // SAFETY: the context's new port, which the server owns from now on;
            // ports are only added before the server starts.
            unsafe { o.state.push_port(o.ctx.port) };
            0
        }
        OP_ST_ABORT_COUNT => {
            o.gs().abort_count.fetch_add(1, SeqCst);
            o.port_gs().abort_count.fetch_add(1, SeqCst);
            0
        }
        OP_COUNT_ACCEPT => {
            o.gs().accept_count.fetch_add(1, Relaxed);
            o.port_gs().accept_count.fetch_add(1, Relaxed);
            if u64::from(o.config().mode_) == MODE_BASE {
                let at = usize::from(o.ctx.reactor_id);
                // Base mode has one connection-count cell per worker, and the
                // reactor id is the accepting worker's.
                let cells = [o.gs().connection_nums, o.port_gs().connection_nums];
                for cell in cells.map(|cells| cells.wrapping_add(at)) {
                    // SAFETY: a cell of the counters' per-worker array, as above.
                    unsafe { &*cell }.fetch_add(1, SeqCst);
                }
            } else {
                o.gs().connection_num.fetch_add(1, SeqCst);
                o.port_gs().connection_num.fetch_add(1, SeqCst);
            }
            0
        }

        // The context's socket.
        OP_SOCK_FD => sign(o.sock().fd),
        OP_SSL_CLEAR => {
            o.sock().ssl = core::ptr::null_mut();
            0
        }
        OP_CHUNK_SIZE => {
            o.sock().chunk_size = swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
            0
        }
        OP_NODELAY_ENABLED => {
            o.sock().enable_tcp_nodelay = true;
            0
        }
        OP_DIRECT_SEND => {
            o.sock().direct_send = 1;
            0
        }
        OP_ST_SOCK_OBJECT => u64::from(!o.sock().object.is_null()),
        OP_SC_HAS_SEND_TIMER => u64::from(!o.sock().output.send_timer.is_null()),
        OP_EVENT_FD => sign(o.ctx.server_fd),
        OP_CONN_RESET => {
            let fd = o.sock().fd;
            let conn = o
                .tables()
                .connection_list
                .wrapping_add(usize::try_from(fd).unwrap_or(0));
            o.ctx.conn = conn.cast();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "C++'s ReactorId (uint8_t)"
            )]
            let reactor_id = if u64::from(o.config().mode_) == MODE_BASE {
                swoole_rs_process::methods::swoole_get_worker_id() as u8
            } else {
                (fd % c_int::from(o.config().reactor_num)) as u8
            };
            o.ctx.reactor_id = reactor_id;
            // SAFETY: the connection slot of a live fd; upstream zeroes it up to `closed`.
            unsafe {
                conn.cast::<u8>()
                    .write_bytes(0, core::mem::offset_of!(ConnectionRecord, closed));
            }
            u64::from(reactor_id)
        }
        OP_SESSION_CLAIM => {
            // SAFETY: a session id of the server's table.
            let session = unsafe { crate::swr_v1_server_session(state, arg.cast_signed()) };
            // SAFETY: the session's slot, in the live shared table.
            let session = unsafe { &mut *session };
            if session.fd != 0 {
                return Some(0);
            }
            session.fd = o.sock().fd;
            session.id = arg.cast_signed();
            session.reactor_bits = (session.reactor_bits & !0xff) | u32::from(o.ctx.reactor_id);
            1
        }
        OP_SESSION_BIND => {
            o.gs().session_round = arg.cast_signed();
            o.conn().session_id = arg.cast_signed();
            let buffer_size = o.port().socket_buffer_size.cast_unsigned();
            let conn = o.ctx.conn;
            let sock = o.sock();
            sock.object = conn;
            sock.removed = 1;
            sock.output.buffer_size = buffer_size;
            sock.timeouts.write = 0.0;
            sock.timeouts.read = 0.0;
            0
        }

        // The context's connection.
        OP_SOCK_SSL => u64::from(!o.conn_socket().ssl.is_null()),
        OP_LAST_RECEIVED => o.conn_socket().last_received_time.to_bits(),
        OP_LAST_SENT => o.conn_socket().last_sent_time.to_bits(),
        OP_CONN_PROTECT => u64::from(o.conn().protect),
        OP_CONN_CLOSING => {
            u64::from(o.conn().closed.load(Relaxed) != 0 || o.conn().close_force != 0)
        }
        OP_CONN_UID => u64::from(o.conn().uid),
        OP_CONN_WORKER => sign(o.conn().worker_id),
        OP_PIN_WORKER => {
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (int) arg")]
            let worker = arg as c_int;
            o.conn().worker_id = worker;
            0
        }
        OP_SC_OVERFLOW => {
            o.conn().overflow = 1;
            0
        }
        OP_SC_HIGH_WATERMARK => {
            o.conn().high_watermark = 1;
            0
        }
        OP_FD_CONN | OP_VERIFY_CONN | OP_SOCKET_CONN => {
            // OP_SOCKET_CONN: `get_connection(int)` of the session id, as master.cc did.
            #[expect(clippy::cast_possible_truncation, reason = "C++'s int parameter")]
            let fd = arg as c_int;
            let (key, kind) = match op {
                OP_FD_CONN => (libc::c_long::from(o.ctx.server_fd), 0),
                OP_VERIFY_CONN => (arg.cast_signed(), 3),
                _ => (libc::c_long::from(fd), 0),
            };
            // SAFETY: the server's live tables.
            let conn = unsafe { crate::swr_v1_server_connection(state, key, kind) };
            o.ctx.conn = conn.cast();
            if op != OP_SOCKET_CONN {
                return Some(u64::from(!conn.is_null()));
            }
            o.ctx.session_id = arg.cast_signed();
            // SAFETY: null, or a slot of the live connection table.
            if unsafe { conn.as_ref() }.is_none_or(|conn| conn.socket.is_null()) {
                return Some(0);
            }
            if u64::from(o.config().mode_) == MODE_PROCESS {
                3
            } else {
                1
            }
        }

        // The context's own fields.
        OP_PORT_TYPE => sign(o.ctx.socket_type),
        OP_PORT_NUMBER => sign(o.ctx.number),
        OP_HOST_SIZE => {
            // SAFETY: add_port's host is a NUL-terminated string.
            unsafe { CStr::from_ptr(o.ctx.host) }.count_bytes() as u64 + 1
        }
        OP_HOST_MAXSIZE => host_maxsize() as u64,
        OP_SENDFILE_NAME_LEN => u64::from(o.ctx.length),
        OP_SENDFILE_NAME_MAX => (IPC_BUFFER_SIZE - SENDFILE_TASK_SIZE - 1) as u64,
        OP_SENDFILE_OFFSET => sign(o.ctx.offset),
        OP_ST_ROOT_PATH => o.ctx.data as u64,
        OP_SC_ADVANCE => {
            #[expect(clippy::cast_possible_truncation, reason = "a length within the data")]
            let step = arg as u32;
            o.ctx.data = o.ctx.data.cast::<u8>().wrapping_add(step as usize).cast();
            o.ctx.length = o.ctx.length.wrapping_sub(step);
            0
        }

        // Effects through the Rust definitions of the reactor's and socket's methods.
        OP_ACCEPT => {
            // SAFETY: the accepting event's listening socket.
            let listener = unsafe { (*o.ctx.event.cast::<swoole_rs_reactor::Event>()).socket };
            // SAFETY: a live listening socket record.
            let socket = unsafe { swoole_rs_reactor::swr_v1_socket_accept_new(listener.cast()) };
            o.ctx.sock = socket.cast();
            u64::from(!socket.is_null())
        }
        OP_SET_NODELAY => {
            // SAFETY: the accepted socket.
            u64::from(unsafe { swoole_rs_reactor::swr_v1_socket_set_tcp_nodelay(o.sock(), 1) })
        }
        OP_SET_RCVBUF | OP_SET_SNDBUF => {
            let port = o.port();
            let (name, size) = if op == OP_SET_RCVBUF {
                (libc::SO_RCVBUF, port.kernel_socket_recv_buffer_size)
            } else {
                (libc::SO_SNDBUF, port.kernel_socket_send_buffer_size)
            };
            // SAFETY: a port with its listening socket.
            let fd = unsafe { (*port.socket.cast::<SocketRecord>()).fd };
            let length = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or(4);
            // SAFETY: `size` is a readable int of `length` bytes.
            sign(unsafe {
                libc::setsockopt(fd, libc::SOL_SOCKET, name, (&raw const size).cast(), length)
            })
        }
        OP_DGRAM_PORT => {
            let port = o.port();
            let socket = port.socket;
            // SAFETY: the new port's socket.
            let fd = unsafe { (*socket.cast::<SocketRecord>()).fd };
            let size = port.socket_buffer_size.cast_unsigned();
            swoole_rs_reactor::swr_v1_socket_buffer_size(fd, true, true, size);
            let config = o.config();
            config.have_dgram_sock = true;
            config.dgram_port_num = config.dgram_port_num.wrapping_add(1);
            match arg {
                2 => config.udp_socket_ipv4 = socket,
                4 => config.udp_socket_ipv6 = socket,
                _ => config.dgram_socket = socket,
            }
            0
        }
        OP_REACTOR_ADD_READ => {
            let socket = o.conn().socket.cast();
            // SAFETY: the context's live reactor and the connection's socket.
            sign(unsafe {
                reactor_upstream::reactor_change(o.ctx.reactor, socket, 0, SW_EVENT_READ)
            })
        }
        OP_SC_SET => {
            let read = if arg & 1 != 0 { SW_EVENT_READ } else { 0 };
            let write = if arg & 2 != 0 { SW_EVENT_WRITE } else { 0 };
            // SAFETY: the context's live reactor and socket.
            sign(unsafe {
                reactor_upstream::reactor_change(o.ctx.reactor, o.sock(), 1, read | write)
            })
        }
        OP_SC_DEL => {
            // SAFETY: the context's live reactor and socket.
            sign(unsafe { reactor_upstream::reactor_change(o.ctx.reactor, o.sock(), 2, 0) })
        }
        OP_SC_ADD => {
            // SAFETY: the context's live reactor and socket.
            sign(unsafe {
                reactor_upstream::reactor_change(o.ctx.reactor, o.sock(), 0, SW_EVENT_READ)
            })
        }
        OP_SC_REMOVE_READ | OP_SC_ADD_READ | OP_SC_ADD_WRITE => {
            let kind = match op {
                OP_SC_REMOVE_READ => 4,
                OP_SC_ADD_READ => 2,
                _ => 3,
            };
            // SAFETY: the context's live reactor and socket.
            let result =
                unsafe { reactor_upstream::reactor_interest(o.ctx.reactor, o.sock(), kind, 0) };
            if op == OP_SC_ADD_WRITE {
                0
            } else {
                sign(result)
            }
        }
        OP_SC_WRITABLE => u64::from(o.sock().events & SW_EVENT_WRITE != 0),
        OP_SC_SEND => {
            let (data, length) = (o.ctx.data, o.ctx.length as usize);
            // SAFETY: the context's socket and the data it sends.
            let sent =
                unsafe { swoole_rs_tls::upstream::socket_send(o.sock(), data.cast(), length, 0) };
            sign(sent as i64)
        }
        OP_SEND_SYNC => {
            let (data, length) = (o.ctx.data, o.ctx.length as usize);
            let socket = o.conn().socket.cast();
            // SAFETY: the connection's socket and the data it sends.
            let sent = unsafe {
                swoole_rs_tls::upstream::socket_send_sync(socket, data.cast(), length, 0)
            };
            u64::from(usize::try_from(sent) == Ok(length))
        }
        OP_SOCKET_NOBUFS => {
            let socket = o.conn().socket.cast();
            // SAFETY: the connection's live socket.
            u64::from(unsafe { swoole_rs_reactor::swr_v1_socket_has_kernel_nobufs(socket) })
        }
        OP_SC_TOUCH_SEND => {
            o.conn().last_send_time = swoole_rs_corebase::microtime();
            0
        }

        // The server's state and the worker records.
        OP_STATE => core::ptr::from_ref(state).expose_provenance() as u64,
        OP_IS_CREATED => u64::from(!o.config().factory_.is_null()),
        OP_IS_MASTER => {
            u64::from(swoole_rs_process::methods::swoole_get_worker_type() == SW_MASTER)
        }
        OP_SC_YIELD_ERROR => {
            log::set_last_error(E::SW_ERROR_OUTPUT_SEND_YIELD as c_int);
            0
        }
        OP_SPIN_LOCK => {
            let cpus = u32::from(swoole_rs_corebase::swr_v1_core_cpu_num());
            // SAFETY: the shared counters' aligned lock word, which every holder takes
            // this way (`sw_spinlock`, with `SW_SPINLOCK_LOOP_N`).
            unsafe {
                swoole_rs_utilities::swr_v1_utils_spinlock(o.gs().spinlock.as_ptr(), cpus, 1024);
            };
            0
        }
        OP_SPIN_UNLOCK => {
            // `sw_spinlock_release`: a release store of 0.
            o.gs()
                .spinlock
                .store(0, core::sync::atomic::Ordering::Release);
            0
        }
        OP_WORKER_COROUTINES | OP_WORKER_CONCURRENCY => {
            let at = usize::try_from(arg).unwrap_or(usize::MAX);
            let worker = o.config().workers.cast::<WorkerRecord>().wrapping_add(at);
            // SAFETY: an event worker of the server's array, which the caller indexes.
            let worker = unsafe { &*worker };
            if op == OP_WORKER_COROUTINES {
                worker.coroutine_num as u64
            } else {
                u64::from(worker.concurrency)
            }
        }
        OP_ST_W_CONCURRENCY => u64::from(o.worker().concurrency),
        OP_ST_W_CONCURRENCY_CAS => {
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (uint32_t) arg")]
            let expected = arg as u32;
            // SAFETY: the worker's aligned `sw_atomic_t`, shared with other processes.
            let cell =
                unsafe { core::sync::atomic::AtomicU32::from_ptr(&raw mut o.worker().concurrency) };
            // `sw_atomic_value_cmp_set`: the value before, swapped or not.
            let before = cell.compare_exchange(expected, 0, SeqCst, SeqCst);
            u64::from(before.unwrap_or_else(|value| value))
        }
        OP_ST_W_RESET_COUNTS => {
            let worker = o.worker();
            worker.request_count = 0;
            worker.response_count = 0;
            worker.dispatch_count = 0;
            0
        }
        OP_SENDFILE_NAME => {
            // `swoole_strlcpy(req->filename, data, SW_IPC_BUFFER_SIZE - sizeof(*req))`.
            let name = o.ctx.buffer.wrapping_add(SENDFILE_TASK_SIZE);
            // SAFETY: sendfile's NUL-terminated name.
            let source = unsafe { CStr::from_ptr(o.ctx.data.cast()) }.to_bytes();
            let length = source.len().min(IPC_BUFFER_SIZE - SENDFILE_TASK_SIZE - 1);
            // SAFETY: the task buffer holds `SW_IPC_BUFFER_SIZE` bytes after its head.
            unsafe { core::ptr::copy_nonoverlapping(source.as_ptr(), name.cast(), length) };
            // SAFETY: as above; `length` leaves room for the NUL.
            unsafe { name.wrapping_add(length).write(0) };
            name.expose_provenance() as u64
        }
        // Lifecycle effects over process-wide state Rust owns: hooks, the
        // worker globals, the master reactor and the server's shared allocations.
        OP_LC_BEFORE_START => {
            call_hook(HOOK_BEFORE_SERVER_START, o.ctx.serv);
            0
        }
        OP_AFTER_CREATE => {
            call_hook(HOOK_AFTER_SERVER_CREATE, o.ctx.serv);
            0
        }
        OP_AFTER_SHUTDOWN => {
            call_hook(HOOK_AFTER_SERVER_SHUTDOWN, o.ctx.serv);
            0
        }
        OP_SIGNAL_ACTIVE => {
            u64::from(swoole_rs_corebase::swr_v1_core_running() && o.config().running)
        }
        OP_MASTER_WORKER => {
            swoole_rs_process::methods::swoole_set_worker_type(SW_MASTER);
            if arg == MODE_THREAD {
                #[expect(clippy::cast_possible_truncation, reason = "C++'s pid_t parameter")]
                let tid = swoole_rs_ossupport::swr_v1_os_thread_native_id() as c_int;
                swoole_rs_process::methods::swoole_set_worker_pid(tid);
            } else if arg == MODE_PROCESS {
                // SAFETY: getpid has no preconditions.
                swoole_rs_process::methods::swoole_set_worker_pid(unsafe { libc::getpid() });
            }
            0
        }
        OP_MASTER_REACTOR => {
            // SAFETY: the master's live reactor.
            let fields = unsafe { reactor_upstream::prepared(o.ctx.reactor) }.fields;
            // SAFETY: as above.
            unsafe { (*fields).ptr = o.ctx.serv };
            let accept: swoole_rs_reactor::Handler = crate::server_methods::accept_connection;
            // SAFETY: as above.
            unsafe {
                reactor_upstream::reactor_set_handler(
                    o.ctx.reactor,
                    SW_FD_STREAM_SERVER,
                    SW_EVENT_READ,
                    Some(accept),
                );
            };
            0
        }
        OP_FREE_USER_WORKERS => {
            let workers = core::mem::replace(&mut o.config().user_workers, core::ptr::null_mut());
            if !workers.is_null() {
                sw_shm_free(workers);
            }
            0
        }
        OP_ALLOC_SESSIONS => {
            let list = shm_calloc(SW_SESSION_LIST_SIZE as usize, SESSION_SIZE);
            o.tables().session_list = list;
            u64::from(!list.is_null())
        }
        OP_ALLOC_PORT_GS => {
            let ports = o.tables().ports();
            let list = shm_calloc::<PortCounters>(ports.len(), size_of::<PortCounters>());
            o.tables().port_gs_list = list.cast();
            if list.is_null() {
                return Some(0);
            }
            for (index, &port) in ports.iter().enumerate() {
                // SAFETY: one of the server's live ports.
                unsafe { port_config(port) }.gs = list.wrapping_add(index).cast();
            }
            1
        }
        OP_ALLOC_WORKERS => {
            let workers = shm_calloc(o.config().worker_num as usize, size_of::<WorkerRecord>());
            o.config().workers = workers;
            u64::from(!workers.is_null())
        }
        OP_ALLOC_TASK_RESULTS => {
            let size = swoole_rs_corebase::constants::SW_IPC_MAX_SIZE as usize;
            let results = shm_calloc(o.config().worker_num as usize, size);
            o.config().task_results = results;
            u64::from(!results.is_null())
        }
        OP_ALLOC_CONNECTION_NUMS => {
            let nums = shm_calloc(o.config().worker_num as usize, ATOMIC_SIZE);
            o.gs().connection_nums = nums;
            u64::from(!nums.is_null())
        }
        OP_ALLOC_PORT_CONNECTION_NUMS => {
            let port = o.tables().ports()[usize::try_from(arg).unwrap_or(usize::MAX)];
            // SAFETY: one of the server's live ports.
            let gs = unsafe { port_config(port) }.gs.cast::<PortCounters>();
            // SAFETY: the port's slot in `port_gs_list`, allocated at create.
            let gs = unsafe { &mut *gs };
            gs.connection_nums = shm_calloc(o.config().worker_num as usize, ATOMIC_SIZE);
            u64::from(!gs.connection_nums.is_null())
        }
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn sigrtmin() -> u64 {
    sign(libc::SIGRTMIN())
}

#[cfg(not(target_os = "linux"))]
fn sigrtmin() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> NativeContext {
        // SAFETY: every NativeContext field is a pointer or integer; zero is valid.
        unsafe { core::mem::zeroed() }
    }

    #[test]
    fn unknown_ops_and_missing_contexts_fall_through_to_native() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state and a zeroed context.
        unsafe {
            assert_eq!(
                answer(&raw mut ctx, state, crate::ops::OP_PORT_COUNT, 0),
                None
            );
            assert_eq!(
                answer(core::ptr::null_mut(), state, OP_SEND_TIMEOUT, 0),
                None
            );
            crate::swr_v1_server_state_free(state);
        }
    }

    #[test]
    fn config_ops_read_and_write_the_config() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state owns its config; the ops below touch only it.
        unsafe {
            let config = &mut *(*state).config();
            config.send_timeout = 2.5;
            config.worker_num = 6;
            assert_eq!(
                answer(&raw mut ctx, state, OP_SEND_TIMEOUT, 0),
                Some(2.5f64.to_bits())
            );
            answer(&raw mut ctx, state, OP_SEND_TIMEOUT_MIN, 0);
            assert_eq!(config.send_timeout, 0.001);
            answer(&raw mut ctx, state, OP_CLEAR_SEND_YIELD, 0);
            assert!(!config.send_yield);
            answer(&raw mut ctx, state, OP_BASE_REACTORS, 0);
            assert_eq!(config.reactor_num, 6);
            config.scheduler_warning = true;
            answer(&raw mut ctx, state, OP_WARNING_RESET, 7 << 1);
            assert!(!config.scheduler_warning);
            assert_eq!(answer(&raw mut ctx, state, OP_WARNING_TIME, 0), Some(7));
            // The task pool's warning stays native.
            assert_eq!(answer(&raw mut ctx, state, OP_WARNING, 1), None);
            crate::swr_v1_server_state_free(state);
        }
    }

    #[test]
    fn host_size_counts_the_nul_and_advance_moves_the_data() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        ctx.host = c"0.0.0.0".as_ptr();
        let data = [0u8; 8];
        ctx.data = data.as_ptr().cast();
        ctx.length = 8;
        // SAFETY: the context's host and data are live above.
        unsafe {
            assert_eq!(answer(&raw mut ctx, state, OP_HOST_SIZE, 0), Some(8));
            answer(&raw mut ctx, state, OP_SC_ADVANCE, 3);
            crate::swr_v1_server_state_free(state);
        }
        assert_eq!(ctx.length, 5);
        assert_eq!(ctx.data, data[3..].as_ptr().cast());
    }

    #[test]
    fn concurrency_swap_returns_the_value_before_like_sw_atomic_value_cmp_set() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: every WorkerRecord field is a pointer or integer; zero is valid.
        let mut worker: WorkerRecord = unsafe { core::mem::zeroed() };
        worker.concurrency = 3;
        ctx.object = (&raw mut worker).cast();
        // SAFETY: the context's worker is live above.
        unsafe {
            assert_eq!(
                answer(&raw mut ctx, state, OP_ST_W_CONCURRENCY_CAS, 2),
                Some(3)
            );
            assert_eq!((*ctx.object.cast::<WorkerRecord>()).concurrency, 3);
            assert_eq!(
                answer(&raw mut ctx, state, OP_ST_W_CONCURRENCY_CAS, 3),
                Some(3)
            );
            assert_eq!((*ctx.object.cast::<WorkerRecord>()).concurrency, 0);
            crate::swr_v1_server_state_free(state);
        }
    }

    #[test]
    fn sendfile_name_is_copied_after_the_task_head() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        let mut buffer = [0xffu8; 64];
        ctx.buffer = buffer.as_mut_ptr().cast();
        ctx.data = c"/tmp/a".as_ptr().cast();
        // SAFETY: the context's buffer and name are live above.
        let name = unsafe { answer(&raw mut ctx, state, OP_SENDFILE_NAME, 0) };
        // SAFETY: created above and not used again.
        unsafe { crate::swr_v1_server_state_free(state) };
        assert_eq!(name, Some(buffer.as_ptr().wrapping_add(16).addr() as u64));
        assert_eq!(&buffer[16..23], b"/tmp/a\0");
    }

    #[test]
    fn allocation_ops_fill_the_config_tables_and_counters() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        let mut gs = ServerCounters::default();
        // SAFETY: a fresh state; every pointer the ops write is checked and freed below.
        unsafe {
            let config = &mut *(*state).config();
            config.worker_num = 3;
            config.gs = (&raw mut gs).cast();
            assert_eq!(answer(&raw mut ctx, state, OP_SIGNAL_ACTIVE, 0), Some(0));
            for op in [
                OP_ALLOC_WORKERS,
                OP_ALLOC_TASK_RESULTS,
                OP_ALLOC_CONNECTION_NUMS,
                OP_ALLOC_SESSIONS,
            ] {
                assert_eq!(answer(&raw mut ctx, state, op, 0), Some(1), "op {op}");
            }
            assert_eq!((*gs.connection_nums.add(2)).load(Relaxed), 0, "zeroed");
            let tables = &mut *(*state).tables();
            assert_eq!((*tables.session_list.add(5)).fd, 0, "zeroed");
            config.user_workers = sw_shm_calloc(1, 8);
            answer(&raw mut ctx, state, OP_FREE_USER_WORKERS, 0);
            assert!(config.user_workers.is_null());
            answer(&raw mut ctx, state, OP_FREE_USER_WORKERS, 0);
            for allocation in [
                config.workers,
                config.task_results,
                gs.connection_nums.cast(),
                tables.session_list.cast(),
            ] {
                sw_shm_free(allocation);
            }
            tables.session_list = core::ptr::null_mut();
            crate::swr_v1_server_state_free(state);
        }
    }
}
