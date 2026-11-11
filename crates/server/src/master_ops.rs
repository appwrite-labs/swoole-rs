//! The ops of master.cc's runner (`Server::rust_call`) that are a log line or a
//! read or write of a record Rust already lays out: the server's config, tables
//! and shared counters, and the context's connection, socket and port. Rust
//! answers them before any native call, so master.cc no longer has a case for
//! them. Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_char, c_int};
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

use swoole_rs_buffer::cpp::{
    CppBuffer, new_buffer, swr_cpp_buffer_append, swr_cpp_buffer_empty, swr_cpp_buffer_length,
    swr_cpp_buffer_typed_close,
};
use swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Socket, SocketRecord, reactor_upstream};

use crate::base_factory::SendData;
use crate::callbacks::CallbackSlot;
use crate::callbacks::{
    CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CLOSE, CB_CONNECT, CB_RECEIVE, CB_TASK,
};
use crate::ops::{
    LOG_ACCEPT_FAILED, LOG_ALLOC_PORT_GS, LOG_ALLOC_SESSIONS, LOG_ALLOC_TASK_RESULTS,
    LOG_ALLOC_WORKERS, LOG_BASE_ALLOC_CONNECTIONS, LOG_BASE_ALLOC_NUMS, LOG_BASE_ALLOC_PORT_NUMS,
    LOG_CALLBACK_DISPATCH, LOG_CALLBACK_REQUIRED, LOG_FDS_START, LOG_FILE_EMPTY, LOG_HOST_TOO_LONG,
    LOG_INVALID_PORT, LOG_INVALID_SESSION, LOG_LC_ONLY_ONE, LOG_LISTEN_PID, LOG_MANAGER_EXIT,
    LOG_MAX_CONNECTION_CREATED, LOG_MAX_CONNECTION_MINIMUM, LOG_MAX_CONNECTION_SESSIONS,
    LOG_MAX_CONNECTION_SOCKETS, LOG_NO_IDLE_WORKER, LOG_NO_SESSION_SLOT, LOG_NODELAY,
    LOG_NOT_REGULAR, LOG_PORT_CREATED, LOG_RCVBUF, LOG_REACTOR_NUM, LOG_SC_CLOSED_BY_CLIENT,
    LOG_SC_CONN_OVERFLOW, LOG_SC_NO_SESSION_DATA, LOG_SC_NO_SESSION_EVENT, LOG_SC_SOCKET_OVERFLOW,
    LOG_SEND_IN_MASTER, LOG_SENDFILE_NAME, LOG_SENDWAIT, LOG_SHUTDOWN_FAILED, LOG_SNDBUF,
    LOG_ST_ROOT_MISSING, LOG_ST_ROOT_TOO_LONG, LOG_STAT, LOG_TASK_POOL, LOG_TASK_WORKER_NUM,
    LOG_TOO_MANY_CONNECTIONS, LOG_TOO_MANY_PORTS, LOG_WORKER_NUM, OP_ACCEPT, OP_BASE_REACTORS,
    OP_CHUNK_SIZE, OP_CLEAR_MAX_QUEUED, OP_CLEAR_SEND_YIELD, OP_CONN_CLOSING, OP_CONN_PROTECT,
    OP_CONN_RESET, OP_CONN_UID, OP_CONN_WORKER, OP_COUNT_ACCEPT, OP_DGRAM_PORT, OP_DIRECT_SEND,
    OP_EVENT_FD, OP_FD_CONN, OP_HAS_PIPE_COMMAND, OP_HOST_MAXSIZE, OP_HOST_SIZE, OP_IS_CREATED,
    OP_IS_MASTER, OP_KILL_TERM, OP_KILL_WINCH, OP_LAST_RECEIVED, OP_LAST_SENT, OP_NEW_PORT_TYPE,
    OP_NODELAY_ENABLED, OP_PIN_WORKER, OP_PIPE_NOBUFS, OP_PORT_IDLE_TIME, OP_PORT_NODELAY,
    OP_PORT_NUMBER, OP_PORT_PUSH, OP_PORT_RCVBUF, OP_PORT_SNDBUF, OP_PORT_SSL, OP_PORT_TYPE,
    OP_REACTOR_ADD_READ, OP_SC_ADD, OP_SC_ADD_READ, OP_SC_ADD_WRITE, OP_SC_ADVANCE, OP_SC_DEL,
    OP_SC_FLAGS, OP_SC_HAS_SEND_TIMER, OP_SC_HIGH_WATERMARK, OP_SC_OVERFLOW, OP_SC_REMOVE_READ,
    OP_SC_SEND, OP_SC_SET, OP_SC_TOUCH_SEND, OP_SC_VERIFY, OP_SC_WRITABLE, OP_SC_YIELD_ERROR,
    OP_SEND_SYNC, OP_SEND_TIMEOUT, OP_SEND_TIMEOUT_MIN, OP_SENDFILE_NAME, OP_SENDFILE_NAME_LEN,
    OP_SENDFILE_NAME_MAX, OP_SENDFILE_OFFSET, OP_SESSION_BIND, OP_SESSION_CLAIM,
    OP_SET_DISABLE_NOTIFY, OP_SET_MASTER_PID, OP_SET_NODELAY, OP_SET_RCVBUF, OP_SET_SNDBUF,
    OP_SIGNAL_FD, OP_SIGRTMIN, OP_SOCK_FD, OP_SOCK_SSL, OP_SOCKET_CONN, OP_SOCKET_NOBUFS,
    OP_SPIN_LOCK, OP_SPIN_UNLOCK, OP_SSL_CLEAR, OP_ST_ABORT_COUNT, OP_ST_ROOT_PATH,
    OP_ST_SOCK_OBJECT, OP_ST_TIMER, OP_ST_W_CONCURRENCY, OP_ST_W_CONCURRENCY_CAS,
    OP_ST_W_RESET_COUNTS, OP_STATE, OP_STREAM_PORT, OP_TASKING, OP_TASKING_RESET, OP_VERIFY_CONN,
    OP_WARNING, OP_WARNING_RESET, OP_WARNING_TIME, OP_WORKER_CONCURRENCY, OP_WORKER_COROUTINES,
    OP_WRONG_OPERATION,
};
use crate::ops::{
    OP_ABORT, OP_ALLOC_CONNECTIONS, OP_PORT_MAX_LENGTH, OP_PORT_MIN_LENGTH, OP_ST_PIPE_BIND,
};
use crate::ops::{
    OP_AFTER_CREATE, OP_AFTER_SHUTDOWN, OP_ALLOC_CONNECTION_NUMS, OP_ALLOC_PORT_CONNECTION_NUMS,
    OP_ALLOC_PORT_GS, OP_ALLOC_SESSIONS, OP_ALLOC_TASK_RESULTS, OP_ALLOC_WORKERS,
    OP_CREATE_FACTORY, OP_DESTROY_FACTORY, OP_DESTROY_WORKER, OP_FREE_USER_WORKERS,
    OP_HTTP_DEFAULTS, OP_INIT_LOCAL_ADDRS, OP_INIT_TASK_WORKERS, OP_LC_BEFORE_START, OP_LC_CHECK,
    OP_LC_PREPARE_WORKERS, OP_LC_START_BASE, OP_LC_START_THREAD, OP_MASTER_REACTOR,
    OP_MASTER_WORKER, OP_SIGNAL_ACTIVE, OP_SIGNAL_READ_MESSAGE, OP_STOP_SINGLE_WORKER,
    OP_STOP_WORKER_THREADS,
};
use crate::ops::{
    OP_IP_KEY, OP_PIPE_COMMAND_ADD, OP_PIPE_COMMAND_DEL, OP_SC_CLOSE, OP_ST_PIPE_FD,
    OP_ST_REACTOR_CLOSE, OP_WORKER_IDLE,
};
use crate::ops::{
    OP_IS_MASTER_THREAD, OP_MASTER_THREAD, OP_NOW_MS, OP_OUT_EMPTY, OP_PORT_EVENT_ADD,
    OP_PORT_EVENT_DEL, OP_PORT_REACTOR_DEL, OP_REACTOR_STOP, OP_REACTOR_WAIT_EXIT, OP_SC_APPEND,
    OP_SC_ENSURE_BUFFER, OP_SC_OUT_FULL, OP_SC_OUT_HIGH, OP_SC_QUEUE_CLOSE,
};
use crate::port_config::PortConfig;
#[cfg(all(feature = "upstream-symbols", not(test)))]
use crate::port_config::methods;
use crate::records::{ConnectionRecord, SessionRecord};
use crate::send;
use crate::server_methods::NativeContext;
use crate::shared::{PortCounters, ServerCounters};
use crate::{MODE_BASE, MODE_PROCESS, MODE_THREAD, ServerState};
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::hook;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_memory::members::{sw_shm_calloc, sw_shm_free};
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_process::workers::entry;
use swoole_rs_tls::socket_pair::SocketPair;
use swoole_rs_utilities::misc::swoole_hash_php;

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
/// `SW_SERVER_EVENT_CLOSE`.
const EVENT_CLOSE: u8 = 4;
/// `sizeof(Session)`, `sizeof(sw_atomic_t)` and `sizeof(SendfileTask)`.
const SESSION_SIZE: usize = size_of::<SessionRecord>();
const ATOMIC_SIZE: usize = 4;
const SENDFILE_TASK_SIZE: usize = 16;
// The native sizes of the arrays allocated here.
const _: () = assert!(
    SESSION_SIZE == 16 && size_of::<PortCounters>() == 80,
    "server.cc sizeof(Session), sizeof(ServerPortGS)"
);
/// `SW_BUFFER_MIN_SIZE`.
const BUFFER_MIN_SIZE: u32 = 65536;

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
    /// The context's live server.
    fn server(&self) -> crate::view::Server<'_> {
        // SAFETY: the method runner lends a context whose server is live.
        unsafe { crate::view::Server::from_raw(self.ctx.serv) }
    }
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
    fn sock(&self) -> Option<Socket<'_>> {
        // SAFETY: the ops reading it run with a live socket in the context.
        unsafe { Socket::from_ptr(self.ctx.sock.cast()) }
    }
    fn conn_socket(&self) -> Option<Socket<'_>> {
        // SAFETY: a live connection's socket, which the ops reading it have.
        unsafe { Socket::from_ptr(self.conn().socket.cast()) }
    }
    /// `edit` over the context's socket, or the default without one.
    fn edit_sock<R: Default>(&self, edit: impl FnOnce(&mut SocketRecord) -> R) -> R {
        self.sock().map(|sock| sock.edit(edit)).unwrap_or_default()
    }
    /// `read` over the context connection's socket, or the default without one.
    fn read_conn_socket<R: Default>(&self, read: impl FnOnce(&SocketRecord) -> R) -> R {
        self.conn_socket()
            .map(|sock| sock.read(read))
            .unwrap_or_default()
    }
    /// Whether the context port listens on TCP.
    fn port_is_tcp(&self) -> bool {
        let kind = self
            .port_socket()
            .map(|socket| socket.read(|s| s.socket_type));
        matches!(kind, Some(SW_SOCK_TCP | SW_SOCK_TCP6))
    }
    /// `OP_ACCEPT`: the accepting event's listening socket accepts into the context's
    /// socket; whether it did.
    fn accept(&mut self) -> bool {
        // SAFETY: the accepting event.
        let listener = unsafe { (*self.ctx.event.cast::<swoole_rs_reactor::Event>()).socket };
        // SAFETY: its live listening socket.
        let listener = unsafe { Socket::from_ptr(listener.cast()) };
        let socket = listener.map_or(core::ptr::null_mut(), |l| l.edit(SocketRecord::accept));
        self.ctx.sock = socket.cast();
        !socket.is_null()
    }
    /// The context port's listening fd, or -1 without one.
    fn port_fd(&self) -> c_int {
        self.port_socket().map_or(-1, Socket::fd)
    }
    /// `Reactor::change`'s `op` of `socket` with `events`, or -1 without one.
    fn change(&self, socket: Option<Socket<'_>>, op: u32, events: c_int) -> c_int {
        socket.map_or(-1, |socket| self.reactor().change(socket, op, events))
    }
    /// The context port's listening socket.
    fn port_socket(&self) -> Option<Socket<'_>> {
        // SAFETY: the ops reading it run with a live port, which owns its socket.
        unsafe { Socket::from_ptr(self.port().socket.cast()) }
    }
    fn port(&self) -> &mut PortConfig {
        // SAFETY: the ops reading it run with a live port in the context.
        unsafe { port_config(self.ctx.port) }
    }
    /// The settings of `serv->ports[index]`, if there is one.
    fn port_at(&self, index: u64) -> Option<&mut PortConfig> {
        let &port = self.tables().ports().get(usize::try_from(index).ok()?)?;
        // SAFETY: one of the server's live ports.
        Some(unsafe { port_config(port) })
    }
    fn port_gs(&self) -> &PortCounters {
        // SAFETY: the port's slot in `port_gs_list`, once the server is created.
        unsafe { &*self.port().gs.cast::<PortCounters>() }
    }
    /// The context's live reactor.
    fn reactor(&self) -> crate::reactor_handle::Reactor {
        // SAFETY: the ops reading it run on the context's live reactor, on its thread.
        unsafe { crate::reactor_handle::Reactor::of(self.ctx.reactor) }
    }
    /// `serv->get_task_worker_pool()`.
    fn task_pool(&self) -> *mut PoolRecord {
        self.server().task_pool()
    }
    /// The server's live task pool.
    fn task_pool_record(&self) -> &mut PoolRecord {
        // SAFETY: the pools live in the server's shared counters for its life.
        unsafe { &mut *self.task_pool() }
    }
    /// `serv->get_event_worker_pool()`, right after the task pool.
    fn event_pool(&self) -> &mut PoolRecord {
        // SAFETY: as `task_pool`; the server's live event pool.
        unsafe { &mut *self.task_pool().wrapping_add(1) }
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
/// `ctx` is the live context for the running operation and `state`
/// its server's; the objects each op reads are live, as for master.cc's case,
/// and the server is at the point of its life where it makes `op`. The
/// [`Objects`] made from them carry this proof to each stage below.
#[inline(always)]
pub(crate) unsafe fn answer(
    ctx: *mut NativeContext,
    state: &ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: per the caller; nothing else touches the context during the op.
    let ctx = unsafe { &mut *ctx };
    let mut o = Objects { ctx, state };
    connection(&mut o, op, arg).or_else(|| ops(&mut o, op, arg))
}

/// The ops each connection makes as it is accepted (`connection::accept`, `add`
/// and `incoming`) and closed (`Server::close`, `base_factory::end`), answered
/// where they are made: inlined into a call with a constant op, each folds to
/// its arm, where [`ops`]' match and the stages it falls through to cost about
/// 40 instructions an op, ~29 ops a connection (#2748).
#[expect(clippy::too_many_lines, reason = "one arm per op, as in `ops`")]
#[inline(always)]
fn connection(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    let state = o.state;
    Some(match op {
        OP_PORT_SSL => u64::from(o.port().ssl),
        OP_PORT_IDLE_TIME => o.port().max_idle_time.to_bits(),
        OP_PORT_RCVBUF => sign(o.port().kernel_socket_recv_buffer_size),
        OP_PORT_SNDBUF => sign(o.port().kernel_socket_send_buffer_size),
        OP_PORT_NODELAY => u64::from(o.port().open_tcp_nodelay && o.port_is_tcp()),
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
        OP_SOCK_FD => sign(o.edit_sock(|s| s.fd)),
        OP_SSL_CLEAR => {
            o.edit_sock(|s| s.ssl = core::ptr::null_mut());
            0
        }
        OP_CHUNK_SIZE => {
            let size = swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
            o.edit_sock(|s| s.chunk_size = size);
            0
        }
        OP_NODELAY_ENABLED => {
            o.edit_sock(|s| s.enable_tcp_nodelay = true);
            0
        }
        OP_DIRECT_SEND => {
            o.edit_sock(|s| s.direct_send = 1);
            0
        }
        OP_EVENT_FD => sign(o.ctx.server_fd),
        OP_CONN_RESET => {
            let fd = o.edit_sock(|s| s.fd);
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
            let session = state.session(arg.cast_signed())?;
            if session.fd != 0 {
                return Some(0);
            }
            session.fd = o.edit_sock(|s| s.fd);
            session.id = arg.cast_signed();
            session.reactor_bits = (session.reactor_bits & !0xff) | u32::from(o.ctx.reactor_id);
            1
        }
        OP_SESSION_BIND => {
            o.gs().session_round = arg.cast_signed();
            o.conn().session_id = arg.cast_signed();
            let buffer_size = o.port().socket_buffer_size.cast_unsigned();
            let conn = o.ctx.conn;
            o.edit_sock(|sock| {
                sock.object = conn;
                sock.removed = 1;
                sock.output.buffer_size = buffer_size;
                sock.timeouts.write = 0.0;
                sock.timeouts.read = 0.0;
            });
            0
        }
        OP_SOCK_SSL => u64::from(o.read_conn_socket(|s| !s.ssl.is_null())),
        OP_ACCEPT => u64::from(o.accept()),
        OP_SET_NODELAY => u64::from(o.edit_sock(|s| s.set_tcp_nodelay(1))),
        OP_REACTOR_ADD_READ => sign(o.change(o.conn_socket(), 0, SW_EVENT_READ)),
        OP_SPIN_LOCK => {
            let cpus = u32::from(swoole_rs_corebase::swr_v1_core_cpu_num());
            // The shared counters' lock word, which every holder takes this way
            // (`sw_spinlock`, with `SW_SPINLOCK_LOOP_N`).
            swoole_rs_utilities::swr_v1_utils_spinlock(Some(&o.gs().spinlock), cpus, 1024);
            0
        }
        OP_SPIN_UNLOCK => {
            // `sw_spinlock_release`: a release store of 0.
            o.gs()
                .spinlock
                .store(0, core::sync::atomic::Ordering::Release);
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_CONN_INIT => {
            let conn = o.conn();
            let (fd, ssl, accepted, sock) = o.sock()?.edit(|sock| {
                let accepted = sock.info;
                sock.get_name();
                (sock.fd, !sock.ssl.is_null(), accepted, sock.info)
            });
            conn.fd = fd;
            conn.reactor_id = o.ctx.reactor_id;
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (int) arg")]
            let server_fd = arg as c_int;
            conn.server_fd = server_fd;
            let now = swoole_rs_corebase::microtime();
            conn.last_recv_time = now;
            conn.connect_time = now;
            conn.active = 1;
            conn.closed.store(0, Relaxed);
            conn.worker_id = -1;
            conn.socket_type = o.port().r#type;
            conn.socket = o.ctx.sock;
            conn.ssl = u8::from(ssl);
            conn.info = accepted;
            conn.info.kind = conn.socket_type;
            let info = &sock;
            let port = swoole_rs_clients::address::swr_v1_clients_address_port(info);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "C++'s uint16_t local_port"
            )]
            let port = port as u16;
            conn.local_port = port;
            let v6 = sock.kind != SW_SOCK_TCP;
            let addr = if v6 {
                // SAFETY: an inet6 address's field.
                unsafe { (&raw const sock.addr.inet_v6.sin6_addr).cast::<u8>() }
            } else {
                // SAFETY: an inet4 address's field.
                unsafe { (&raw const sock.addr.inet_v4.sin_addr).cast::<u8>() }
            };
            // SAFETY: the live state and an address of the family.
            conn.local_addr_index = unsafe {
                crate::local_addr::swr_v1_server_local_addr_index(Some(o.state), v6, addr)
            };
            0
        }
        OP_OUT_EMPTY => {
            // SAFETY: native code passes a live socket.
            let socket = unsafe { Socket::from_ptr(arg as *mut SocketRecord) }?;
            let buffer = socket.read(|s| s.output.out_buffer);
            // SAFETY: null or the socket's live buffer (`Buffer::empty`).
            u64::from(buffer.is_null() || swr_cpp_buffer_empty(unsafe { &*buffer.cast() }))
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_ON_CLOSE => {
            let event = arg as *mut core::ffi::c_void;
            let slot = o.state.callbacks.slot(CB_CLOSE)?;
            // SAFETY: onClose's slot, the live server and the close event.
            unsafe { slot.call(o.ctx.serv, event) };
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_CLOSE_SOCKET => {
            let reactor = swoole_rs_corebase::thread_reactor();
            let socket = arg as *mut SocketRecord;
            // SAFETY: this thread's live reactor and a session socket of it.
            sign(unsafe { crate::reactor_entry::close_connection(reactor, socket) })
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_QUEUE_CLOSE => {
            // SAFETY: a live session socket of this thread's reactor.
            let socket = unsafe { Socket::from_ptr(arg as *mut SocketRecord) }?;
            let out = socket.read(|s| s.output.out_buffer);
            // SAFETY: its live output buffer.
            swr_cpp_buffer_typed_close(unsafe { &*out.cast() });
            // Only the half-close drain stops reading (`SW_HALFCLOSE_DRAIN`); upstream reads on.
            u64::from(
                !cfg!(feature = "halfclose")
                    || crate::reactor_handle::Reactor::thread().interest(socket, 4, 0) == 0,
            )
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_FACTORY_END => {
            u64::from(crate::factory::end(o.state, o.ctx.session_id, o.ctx.number))
        }
        _ => return None,
    })
}

/// The other ops of master.cc's switch; then [`forwarded`].
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
fn ops(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    let state = o.state;
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
            let m = format_args!("Too many connections [now: {}]", o.edit_sock(|s| s.fd));
            error(
                SW_LOG_WARNING,
                E::SW_ERROR_SERVER_TOO_MANY_SOCKET,
                c"Server::accept_connection",
                m,
            );
            0
        }
        LOG_NO_SESSION_SLOT => {
            let m = format_args!("no available session slot, fd={}", o.edit_sock(|s| s.fd));
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
        LOG_MAX_CONNECTION_SOCKETS => {
            let m = format_args!("max_connection is exceed the maximum value, it's reset to {arg}");
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
            let head = String::from_utf8_lossy(name.get(..8).unwrap_or(name));
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
        // arg 1 (bit 0 for the reset) names the task pool's.
        OP_WARNING if arg == 0 => u64::from(o.config().scheduler_warning),
        OP_WARNING_TIME if arg == 0 => sign(o.config().warning_time),
        OP_WARNING_RESET if arg & 1 == 0 => {
            o.config().scheduler_warning = false;
            o.config().warning_time = (arg >> 1).cast_signed() as libc::time_t;
            0
        }
        OP_WARNING => u64::from(o.task_pool_record().scheduler_warning),
        OP_WARNING_TIME => sign(o.task_pool_record().warning_time),
        OP_WARNING_RESET => {
            let pool = o.task_pool_record();
            pool.scheduler_warning = 0;
            pool.warning_time = (arg >> 1).cast_signed();
            0
        }
        OP_SIGNAL_READ_MESSAGE => {
            o.event_pool().read_message = true;
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
            o.gs().master_pid = std::process::id().cast_signed();
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

        // The context's socket.
        OP_ST_SOCK_OBJECT => u64::from(o.edit_sock(|s| !s.object.is_null())),
        OP_SC_HAS_SEND_TIMER => u64::from(o.edit_sock(|s| !s.output.send_timer.is_null())),

        // The context's connection.
        OP_LAST_RECEIVED => o.read_conn_socket(|s| s.last_received_time.to_bits()),
        OP_LAST_SENT => o.read_conn_socket(|s| s.last_sent_time.to_bits()),
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
            let conn = state.connection(key, kind);
            let missing = conn.as_ref().is_none_or(|conn| conn.socket.is_null());
            o.ctx.conn = conn.map_or(core::ptr::null_mut(), |conn| (&raw mut *conn).cast());
            if op != OP_SOCKET_CONN {
                return Some(u64::from(!o.ctx.conn.is_null()));
            }
            o.ctx.session_id = arg.cast_signed();
            if missing {
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
        OP_SET_RCVBUF | OP_SET_SNDBUF => {
            let port = o.port();
            let (name, size) = if op == OP_SET_RCVBUF {
                (libc::SO_RCVBUF, port.kernel_socket_recv_buffer_size)
            } else {
                (libc::SO_SNDBUF, port.kernel_socket_send_buffer_size)
            };
            let fd = o.port_fd();
            let length = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or(4);
            // SAFETY: `size` is a readable int of `length` bytes.
            sign(unsafe {
                libc::setsockopt(fd, libc::SOL_SOCKET, name, (&raw const size).cast(), length)
            })
        }
        OP_DGRAM_PORT => {
            let fd = o.port_fd();
            let port = o.port();
            let socket = port.socket;
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
        OP_SC_SET => {
            let read = if arg & 1 != 0 { SW_EVENT_READ } else { 0 };
            let write = if arg & 2 != 0 { SW_EVENT_WRITE } else { 0 };
            sign(o.change(o.sock(), 1, read | write))
        }
        OP_SC_DEL => sign(o.change(o.sock(), 2, 0)),
        OP_SC_ADD => sign(o.change(o.sock(), 0, SW_EVENT_READ)),
        OP_SC_REMOVE_READ | OP_SC_ADD_READ | OP_SC_ADD_WRITE => {
            let kind = match op {
                OP_SC_REMOVE_READ => 4,
                OP_SC_ADD_READ => 2,
                _ => 3,
            };
            let result = o
                .sock()
                .map_or(-1, |sock| o.reactor().interest(sock, kind, 0));
            if op == OP_SC_ADD_WRITE {
                0
            } else {
                sign(result)
            }
        }
        OP_SC_WRITABLE => u64::from(o.edit_sock(|s| s.events) & SW_EVENT_WRITE != 0),
        OP_SC_SEND => {
            let (data, length) = (o.ctx.data, o.ctx.length as usize);
            let sock = o.ctx.sock.cast();
            // SAFETY: the context's live socket and the data it sends.
            let sent =
                unsafe { swoole_rs_tls::upstream::socket_send(sock, data.cast(), length, 0) };
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
        OP_SC_FLAGS => send_flags(o),
        OP_SC_VERIFY => verify(o),
        OP_PIPE_NOBUFS => {
            // `serv->get_reactor_pipe_socket(session_id, conn->reactor_id)`.
            let pipes = i64::from(o.tables().reactor_pipe_num);
            let index = o.ctx.session_id % pipes;
            let id = i64::from(o.conn().reactor_id) + index * i64::from(o.config().reactor_num);
            let id = u16::try_from(id).unwrap_or(u16::MAX);
            let pipe = o.server().worker_pipe(id, false);
            u64::from(pipe.is_some_and(|pipe| pipe.edit(SocketRecord::has_kernel_nobufs)))
        }
        OP_SOCKET_NOBUFS => {
            let socket = o.conn_socket();
            u64::from(socket.is_some_and(|socket| socket.edit(SocketRecord::has_kernel_nobufs)))
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
                swoole_rs_process::methods::swoole_set_worker_pid(std::process::id().cast_signed());
            }
            0
        }
        OP_MASTER_REACTOR => {
            let reactor = o.reactor();
            reactor.fields().ptr = o.ctx.serv;
            let accept: swoole_rs_reactor::Handler = crate::server_methods::accept_connection;
            reactor.set_handler(SW_FD_STREAM_SERVER, SW_EVENT_READ, accept);
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
            for index in 0..ports.len() {
                o.port_at(index as u64)?.gs = list.wrapping_add(index).cast();
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
        OP_INIT_LOCAL_ADDRS => u64::from(crate::local_addr::init(o.state)),
        op => return forwarded(o, op, arg),
    })
}

/// The ops over the server's tables and ports that master.cc made, and the
/// `Server` methods crates/server defines that it forwarded to; then [`lifecycle`].
fn forwarded(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_ALLOC_CONNECTIONS => {
            let size = size_of::<ConnectionRecord>();
            let list = swoole_rs_corebase::sw_calloc(o.tables().max_connection as usize, size);
            o.tables().connection_list = list.cast();
            u64::from(!list.is_null())
        }
        OP_PORT_MAX_LENGTH | OP_PORT_MIN_LENGTH => {
            let protocol = &mut o.port_at(arg)?.protocol;
            if op == OP_PORT_MIN_LENGTH {
                protocol.package_max_length = BUFFER_MIN_SIZE;
            }
            u64::from(protocol.package_max_length)
        }
        OP_ST_PIPE_BIND => {
            let fd = isize::try_from(arg).unwrap_or(isize::MAX);
            let conn = o.tables().connection_list.wrapping_offset(fd);
            // SAFETY: the pipe's fd indexes the live connection list, as in master.cc.
            unsafe { (*conn).object = o.ctx.object };
            0
        }
        OP_ABORT => {
            let ctx = &o.ctx;
            // SAFETY: the context's live reactor, port and accepted socket.
            unsafe {
                crate::server_methods::abort_connection(ctx.serv, ctx.reactor, ctx.port, ctx.sock);
            };
            0
        }
        op => return lifecycle(o, op, arg),
    })
}

/// The ops that call a `Server` method crates/server or crates/process defines,
/// or else `None`. `ProcessPool`'s methods and the process factory are
/// [`factory_ops`]', which only the hybrid builds: their runners link the C++
/// event loop and timer, and the server's fault fixture and unit tests link no C++.
fn lifecycle(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    let serv = o.ctx.serv;
    Some(match op {
        OP_LC_CHECK => sign(crate::server_methods::start_check(o.server())),
        OP_LC_PREPARE_WORKERS => {
            crate::worker_methods::workers_start(o.server());
            0
        }
        // SAFETY: per the caller, for this and each call below.
        OP_LC_START_BASE => sign(unsafe { entry::start_reactor_processes(serv) }),
        // SAFETY: as above.
        OP_LC_START_THREAD => sign(unsafe { entry::start_worker_threads(serv) }),
        OP_STOP_WORKER_THREADS => {
            // SAFETY: as above.
            unsafe { entry::stop_worker_threads(serv) };
            0
        }
        OP_STOP_SINGLE_WORKER => {
            o.event_pool().running = false;
            // SAFETY: this thread's worker globals.
            let worker = unsafe { (*swoole_rs_process::worker_globals::thread_globals()).worker };
            // SAFETY: as above; base mode's single worker, which this process runs.
            unsafe { entry::stop_async_worker(serv, worker.cast()) };
            0
        }
        OP_CREATE_FACTORY if u64::from(o.config().mode_) != MODE_PROCESS => {
            let factory = if u64::from(o.config().mode_) == MODE_BASE {
                crate::server_methods::create_base_factory(o.server())
            } else {
                // SAFETY: as above.
                unsafe { entry::create_thread_factory(serv) }
            };
            o.config().factory_ = factory;
            u64::from(!factory.is_null())
        }
        // SAFETY: as above.
        OP_INIT_TASK_WORKERS => u64::from(unsafe { entry::init_task_workers(serv) }),
        OP_DESTROY_WORKER => {
            let at = usize::try_from(arg).unwrap_or(usize::MAX);
            let worker = o.config().workers.cast::<WorkerRecord>().wrapping_add(at);
            // SAFETY: an event worker of the server's array, which shutdown indexes.
            let worker = unsafe { &mut *worker };
            // SAFETY: a worker `create_worker` set up; shutdown holds no lock of it.
            unsafe { crate::worker_methods::destroy_worker(worker) };
            0
        }
        OP_DESTROY_FACTORY if arg == MODE_BASE => destroy_base_factory(o),
        OP_DESTROY_FACTORY if arg == MODE_THREAD => {
            // SAFETY: as above.
            unsafe { entry::destroy_thread_factory(serv) };
            0
        }
        OP_ALLOC_CONNECTION_NUMS => {
            let nums = shm_calloc(o.config().worker_num as usize, ATOMIC_SIZE);
            o.gs().connection_nums = nums;
            u64::from(!nums.is_null())
        }
        OP_ALLOC_PORT_CONNECTION_NUMS => {
            let Some(gs) = o.port_at(arg).and_then(|port| port.counters()) else {
                return Some(0);
            };
            gs.connection_nums = shm_calloc(o.config().worker_num as usize, ATOMIC_SIZE);
            u64::from(!gs.connection_nums.is_null())
        }
        OP_HTTP_DEFAULTS => {
            // `locations` (0) and `http_compression_types` (2): count, enabled, enable, reset.
            let strings = |kind, op| {
                // SAFETY: the live state; these operations lend no bytes.
                unsafe {
                    crate::swr_v1_server_static_config(o.state, kind, op, core::ptr::null(), 0)
                }
            };
            if o.config().enable_static_handler && strings(0, 1) == 0 {
                strings(0, 3);
                strings(0, 2);
            }
            if strings(2, 1) != 0 && strings(2, 0) != 0 {
                o.config().http_compression = true;
            }
            0
        }
        op => return effects(o, op, arg),
    })
}

/// The ops over the thread's globals and reactor, the server's ports and
/// the context socket's output buffer, which Rust lays out; else `None`.
fn effects(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    /// `THREAD_MASTER`.
    const THREAD_MASTER: u8 = 1;
    let reactor = crate::reactor_handle::Reactor::thread;
    let listener = || o.server().port_socket_at(usize::try_from(arg).ok()?);
    // The send path's output buffer, which `OP_SC_ENSURE_BUFFER` made before any op reads it.
    let out = || {
        let out = o.sock().map(|sock| sock.read(|s| s.output.out_buffer));
        // SAFETY: as above, the socket's live buffer.
        unsafe { &*out.unwrap_or(core::ptr::null_mut()).cast::<CppBuffer>() }
    };
    Some(match op {
        OP_IS_MASTER_THREAD => u64::from(swoole_rs_corebase::thread_kind() == THREAD_MASTER),
        OP_MASTER_THREAD => {
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (uint16_t) arg")]
            swoole_rs_corebase::set_thread(THREAD_MASTER, arg as u16);
            0
        }
        // `swoole_event_add`/`_del` and `SwooleTG.reactor->del` on a listening socket.
        OP_PORT_EVENT_ADD | OP_PORT_EVENT_DEL | OP_PORT_REACTOR_DEL => {
            let (kind, events) = if op == OP_PORT_EVENT_ADD {
                (0, SW_EVENT_READ)
            } else {
                (2, 0)
            };
            reactor().change(listener()?, kind, events);
            0
        }
        OP_REACTOR_WAIT_EXIT => {
            reactor().fields().wait_exit = true;
            0
        }
        OP_REACTOR_STOP => {
            reactor().fields().running = false;
            0
        }
        OP_NOW_MS => swoole_rs_reactor::steady_ms().to_bits(),
        OP_SC_ENSURE_BUFFER => {
            let sock = o.sock()?;
            if sock.read(|s| s.output.out_buffer.is_null()) {
                let buffer = new_buffer(SW_SEND_BUFFER_SIZE).cast();
                sock.edit(|s| s.output.out_buffer = buffer);
            }
            0
        }
        OP_SC_OUT_FULL => {
            let size = o.sock()?.read(|s| s.output.buffer_size);
            u64::from(swr_cpp_buffer_length(out()) >= size)
        }
        OP_SC_OUT_HIGH => u64::from(swr_cpp_buffer_length(out()) >= o.port().buffer_high_watermark),
        OP_SC_APPEND => {
            // SAFETY: the context lends `length` bytes at `data`.
            unsafe { swr_cpp_buffer_append(out(), o.ctx.data.cast(), o.ctx.length) };
            o.conn().send_queued_bytes = swr_cpp_buffer_length(out());
            0
        }
        OP_SC_QUEUE_CLOSE => {
            swr_cpp_buffer_typed_close(out());
            o.conn().close_queued = 1;
            0
        }
        op => return sockets(o, op, arg),
    })
}

/// `OP_SC_VERIFY`: selects the send's connection (`get_connection_verify`, or
/// `get_connection_verify_no_ssl` for a close), its reactor, port and socket, and
/// returns every fact the send reads, or 0 when there is none.
fn verify(o: &mut Objects<'_>) -> u64 {
    // SAFETY: the send path lends its live `SendData`.
    let send = unsafe { &*o.ctx.send.cast::<SendData>() };
    let kind = if send.info.kind == EVENT_CLOSE { 2 } else { 3 };
    o.ctx.conn = o.state.connection_ptr(o.ctx.session_id, kind).cast();
    // SAFETY: null, or a slot of the live connection table.
    let Some(conn) = (unsafe { o.ctx.conn.cast::<ConnectionRecord>().as_ref() }) else {
        return 0;
    };
    o.ctx.reactor = swoole_rs_corebase::thread_reactor();
    o.ctx.port = o.server().port_by_server_fd(conn.server_fd);
    o.ctx.sock = conn.socket;
    let config = o.config();
    debug_assert!(
        config.single_thread
            || i64::from(conn.fd) % i64::from(config.reactor_num)
                == i64::from(swoole_rs_corebase::thread_id()),
        "a connection is sent to on its own reactor thread"
    );
    let Some(sock) = o.sock() else {
        return 0;
    };
    let (out, direct_send) = sock.read(|s| (s.output.out_buffer, s.direct_send != 0));
    // SAFETY: null or the socket's live buffer (`Buffer::empty`).
    let out_empty = out.is_null() || swr_cpp_buffer_empty(unsafe { &*out.cast() });
    let bit = |set: bool, bit| if set { bit } else { 0 };
    send_flags(o)
        | send::FOUND
        | bit(cfg!(feature = "halfclose"), send::HALFCLOSE)
        | bit(config.send_yield, send::SEND_YIELD)
        | bit(out_empty, send::OUT_EMPTY)
        | bit(direct_send, send::DIRECT_SEND)
        | (u64::from(o.ctx.length) << 32)
}

/// `OP_SC_FLAGS`: the connection's flags, then its socket's and the server's buffer callbacks.
fn send_flags(o: &Objects<'_>) -> u64 {
    let conn = o.conn();
    let (removed, notified, events) = o.sock().map_or((false, false, 0), |sock| {
        sock.read(|s| (s.removed != 0, s.close_notified != 0, s.events))
    });
    let callback = |index| {
        o.state
            .callbacks
            .slot(index)
            .is_some_and(CallbackSlot::is_set)
    };
    [
        (conn.overflow != 0, send::OVERFLOW),
        (conn.close_reset != 0, send::CLOSE_RESET),
        (conn.close_force != 0, send::CLOSE_FORCE),
        (conn.peer_closed != 0, send::PEER_CLOSED),
        (conn.closed.load(Relaxed) != 0, send::CLOSED),
        (conn.close_queued != 0, send::CLOSE_QUEUED),
        (conn.high_watermark != 0, send::HIGH_WATERMARK),
        (removed, send::REMOVED),
        (
            cfg!(feature = "halfclose") && notified,
            send::CLOSE_NOTIFIED,
        ),
        (events & SW_EVENT_READ != 0, send::EVENT_READ),
        (events & SW_EVENT_WRITE != 0, send::EVENT_WRITE),
        (callback(CB_BUFFER_FULL), send::ON_BUFFER_FULL),
        (callback(CB_BUFFER_EMPTY), send::ON_BUFFER_EMPTY),
    ]
    .into_iter()
    .filter_map(|(set, bit)| set.then_some(bit))
    .fold(0, |flags, bit| flags | bit)
}

/// `DgramPacket` up to its address.
#[repr(C)]
struct DgramPacket {
    socket_type: c_int,
    socket_addr: SocketAddress,
}

/// The ops over the context's reactor, sockets and pipes, the server's
/// workers and the dispatch address, which Rust lays out; else `None`.
fn sockets(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    /// `SW_WORKER_IDLE`.
    const WORKER_IDLE: u8 = 2;
    Some(match op {
        OP_WORKER_IDLE => {
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (uint16_t) id")]
            let id = arg as u16;
            let worker = o.server().worker_record(id);
            u64::from(worker.is_some_and(|worker| worker.status() == WORKER_IDLE))
        }
        OP_IP_KEY => {
            let fd = libc::c_long::from(o.ctx.server_fd);
            let addr = if let Some(conn) = o.state.connection(fd, 0) {
                &conn.info
            } else {
                // SAFETY: no connection: the context sends a datagram.
                let packet = unsafe { (*o.ctx.send.cast::<crate::base_factory::SendData>()).data };
                #[expect(
                    clippy::cast_ptr_alignment,
                    reason = "the payload is a message bus buffer, aligned for its `DgramPacket`"
                )]
                let packet = packet.cast::<DgramPacket>();
                // SAFETY: the datagram's payload starts with its `DgramPacket`.
                unsafe { &(*packet).socket_addr }
            };
            if swoole_rs_reactor::type_is(addr.kind, swoole_rs_reactor::CLASS_INET4) {
                // SAFETY: an inet4 address.
                u64::from(u32::from_be(unsafe { addr.addr.inet_v4.sin_addr.s_addr }))
            } else {
                let v6 = (&raw const addr.addr).cast::<c_char>();
                // SAFETY: the address's storage holds a whole `sockaddr_in6`.
                let hash = unsafe { swoole_hash_php(v6, size_of::<libc::sockaddr_in6>()) };
                #[expect(clippy::cast_possible_truncation, reason = "C++'s (uint32_t)")]
                u64::from(hash as u32)
            }
        }
        OP_SC_CLOSE | OP_ST_REACTOR_CLOSE => {
            o.reactor().close(o.sock()?);
            0
        }
        OP_ST_PIPE_FD => {
            // SAFETY: the context's live `UnixSocket`.
            let pair = unsafe { &*o.ctx.object.cast::<SocketPair>() };
            let end = if arg == 0 {
                pair.master_socket
            } else {
                pair.worker_socket
            };
            // SAFETY: a ready pair's live end.
            sign(unsafe { Socket::from_ptr(end) }?.fd())
        }
        // `ctx->reactor->add` and `SwooleTG.reactor->del` of the command pipe's master end.
        OP_PIPE_COMMAND_ADD | OP_PIPE_COMMAND_DEL => {
            let pipe = o.config().pipe_command.cast::<SocketPair>();
            // SAFETY: null or the server's live command pipe.
            if let Some(pipe) = unsafe { pipe.as_ref() } {
                let (reactor, kind, events) = if op == OP_PIPE_COMMAND_ADD {
                    (o.ctx.reactor.cast_const(), 0, SW_EVENT_READ)
                } else {
                    (swoole_rs_corebase::thread_reactor().cast_const(), 2, 0)
                };
                // SAFETY: a live reactor and the pipe's live end.
                unsafe {
                    reactor_upstream::reactor_change(reactor, pipe.master_socket, kind, events)
                };
            }
            0
        }
        op => return native(o, op, arg),
    })
}

/// The ops that call the event loop, timer, signal and error functions under
/// their upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn native(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        OP_COMMAND_HANDLER, OP_EVENT_FREE, OP_EVENT_WAIT, OP_LC_CLEAR_ERROR, OP_MARK_SHUTDOWN,
        OP_SIGNAL_HANDLE, OP_SIGNAL_IGNORE, OP_SSL_CREATE, OP_ST_SOCK_FREE, OP_ST_TIMER_DEL,
        OP_TIMER_DETACHED, OP_TIMER_REINIT,
    };
    use swoole_rs_timer::{owner_reactor, owner_reinit, swoole_timer_del};
    /// `SW_FD_PIPE`.
    const SW_FD_PIPE: c_int = 3;
    use swoole_rs_error::swoole_clear_last_error_msg;
    use swoole_rs_reactor::event_api::swoole_event_wait;
    use swoole_rs_reactor::signal_upstream::{swoole_signal_clear, swoole_signal_set};
    use swoole_rs_reactor::swoole_event_free;
    // `SwooleTG.timer`.
    let timer = || {
        // SAFETY: the address of this thread's slot.
        unsafe { *swoole_rs_corebase::swr_v1_core_timer_slot() }
    };
    Some(match op {
        OP_LC_CLEAR_ERROR => {
            log::set_last_error(0);
            swoole_clear_last_error_msg();
            0
        }
        OP_EVENT_FREE => {
            swoole_event_free();
            0
        }
        OP_EVENT_WAIT => sign(swoole_event_wait()),
        OP_TIMER_DETACHED => {
            let timer = timer();
            // SAFETY: this thread's live timer.
            u64::from(
                !timer.is_null()
                    && owner_reactor(unsafe { &*timer.cast::<swoole_rs_timer::Owner>() }).is_null(),
            )
        }
        OP_TIMER_REINIT => {
            // SAFETY: this thread's live timer.
            owner_reinit(unsafe { &*timer().cast::<swoole_rs_timer::Owner>() }, false);
            0
        }
        OP_ST_TIMER_DEL => {
            let node = match arg {
                0 => &mut o.config().master_timer,
                1 => &mut o.config().heartbeat_timer,
                _ => &mut o.tables().enable_accept_timer,
            };
            // SAFETY: null or one of this thread's live timer nodes.
            unsafe { swoole_timer_del((*node).cast()) };
            *node = core::ptr::null_mut();
            0
        }
        OP_SIGNAL_IGNORE | OP_SIGNAL_HANDLE => {
            let handler = if op == OP_SIGNAL_HANDLE {
                crate::server_methods::master_signal_handler as *const () as usize
            } else {
                0
            };
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (int) arg")]
            let signo = arg as c_int;
            // SAFETY: a handler nothing unloads, or none.
            _ = unsafe { swoole_signal_set(signo, handler, false) };
            0
        }
        OP_MARK_SHUTDOWN => {
            // SAFETY: the master, which owns the signal handlers.
            unsafe { swoole_signal_clear() };
            o.gs().start.store(0, SeqCst);
            o.gs().shutdown.store(1, SeqCst);
            0
        }
        OP_ST_SOCK_FREE => {
            // SAFETY: the context's socket, which nothing uses afterwards;
            // its body links the timer and C++ buffer.
            unsafe { swoole_rs_tls::upstream::socket_free(o.ctx.sock.cast()) };
            0
        }
        OP_SSL_CREATE => u64::from((methods().ssl_create)(o.ctx.port, o.ctx.sock)),
        OP_COMMAND_HANDLER => {
            let handler: swoole_rs_reactor::Handler = accept_command_result;
            o.reactor().set_handler(SW_FD_PIPE, SW_EVENT_READ, handler);
            0
        }
        op => return startup(o, op, arg),
    })
}

/// The startup ops over the server's factory, timer, callbacks, listening sockets
/// and environment, under their upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn startup(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::callbacks::CB_START;
    use crate::ops::{
        OP_ENV_FDS_START, OP_ENV_LISTEN_PID, OP_LC_DAEMON, OP_LC_FACTORY_START,
        OP_LC_START_PROCESS, OP_MASTER_START, OP_MASTER_TIMER, OP_MAX_SOCKETS, OP_ST_FINISH,
        OP_ST_INIT_PIPE, OP_STORE_LISTEN, OP_SYSTEMD_FDS,
    };
    use swoole_rs_clients::address::{Resolve, swr_v1_clients_address_assign};
    use swoole_rs_corebase::{swoole_get_systemd_listen_fds, swoole_redirect_stdout_file};
    use swoole_rs_ossupport::swoole_daemon;
    use swoole_rs_utilities::{process_logger, swr_v1_logger_is_opened, swr_v1_logger_redirect};
    let serv = o.ctx.serv;
    Some(match op {
        OP_LC_DAEMON => {
            match process_logger() {
                Some(logger) if swr_v1_logger_is_opened(logger) => {
                    swr_v1_logger_redirect(logger, true);
                }
                // SAFETY: a NUL-terminated path.
                _ => unsafe { swoole_redirect_stdout_file(c"/dev/null".as_ptr()) },
            }
            // The master, before it starts any thread.
            sign(swoole_daemon(0, 1))
        }
        OP_LC_FACTORY_START => u64::from(crate::factory::start(o.state)),
        OP_ST_FINISH => u64::from(crate::factory::finish(o.state, o.ctx.send)),
        // SAFETY: the master, starting its reactor threads.
        OP_LC_START_PROCESS => sign(unsafe { crate::reactor_entry::start_reactor_threads(serv) }),
        OP_MAX_SOCKETS => u64::from(swoole_rs_corebase::max_sockets()),
        OP_MASTER_TIMER => {
            let tick = crate::server_methods::master_timer_tick;
            // SAFETY: `tick` takes the server, which outlives the master's timer.
            let node = unsafe { swoole_rs_timer::swr_v1_timer_add_plain(1000, true, tick, serv) };
            o.config().master_timer = node;
            u64::from(!node.is_null())
        }
        OP_MASTER_START => {
            if let Some(slot) = o.state.callbacks.slot(CB_START) {
                // SAFETY: `onStart` takes the server.
                unsafe { slot.call_server(serv) };
            }
            0
        }
        OP_STORE_LISTEN => {
            let index = usize::try_from(arg).unwrap_or(usize::MAX);
            let &ls = o.tables().ports().get(index)?;
            // SAFETY: one of the server's live ports.
            let port = unsafe { port_config(ls) };
            let socket = o.server().port_socket_at(index)?;
            let fd = socket.fd();
            let at = isize::try_from(fd).unwrap_or(isize::MAX);
            // SAFETY: the listening socket's fd indexes the live connection list.
            let conn = unsafe { &mut *o.tables().connection_list.wrapping_offset(at) };
            conn.fd = fd;
            conn.socket = port.socket.cast();
            conn.socket_type = port.r#type;
            conn.object = ls;
            conn.info.kind = port.r#type;
            let host = port.host();
            let info = &mut conn.info;
            let resolve: Resolve = swoole_rs_corebase::swr_v1_clients_gethostbyname;
            // SAFETY: the connection's address storage and the port's host.
            let error = unsafe {
                swr_v1_clients_address_assign(
                    (&raw mut info.addr).cast(),
                    &raw mut info.len,
                    port.r#type,
                    host.data,
                    host.length,
                    port.port,
                    Some(resolve),
                )
            };
            if error != 0 {
                log::set_last_error(error);
            }
            socket.edit(|socket| socket.object = ls);
            sign(fd)
        }
        OP_ENV_LISTEN_PID | OP_ENV_FDS_START => {
            let name = if op == OP_ENV_LISTEN_PID {
                c"LISTEN_PID"
            } else {
                c"LISTEN_FDS_START"
            };
            swoole_rs_corebase::get_env(name).map_or(i64::MIN.cast_unsigned(), sign)
        }
        // SAFETY: reads the environment.
        OP_SYSTEMD_FDS => sign(swoole_get_systemd_listen_fds()),
        OP_ST_INIT_PIPE => {
            // SAFETY: the context's live `MessageBus`.
            let bus = unsafe { crate::objects::Bus::of(o.ctx.object) };
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            // SAFETY: the context's live server; `arg` is one of its worker ids.
            let serv = unsafe { crate::view::Server::from_raw(serv) };
            bus.init_pipe_socket(serv.worker_pipe(id, true)?.fd());
            bus.init_pipe_socket(serv.worker_pipe(id, false)?.fd());
            0
        }
        op => return closing(o, op, arg),
    })
}

/// The ops over a connection's close and the server's shutdown, which
/// call crates/server's, crates/process's and crates/ports' methods under their
/// upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn closing(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::callbacks::{CB_BEFORE_SHUTDOWN, CB_DISPATCH_FUNC, CB_SHUTDOWN};
    use crate::ops::{
        OP_ACCEPT_TIMER, OP_BEFORE_SHUTDOWN, OP_CLOSE_CONNECTION, OP_DISPATCH_FUNC, OP_FORCE_CLOSE,
        OP_FORCED_EXIT, OP_FREE_SERVER, OP_JOIN_REACTOR_THREADS, OP_NOTIFY_CONNECT, OP_ON_SHUTDOWN,
        OP_PORT_CLOSE, OP_SIGNAL_RELOAD, OP_SIGNAL_REOPEN, OP_THREAD0_SHUTDOWN,
    };
    /// `SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN`.
    const HOOK_BEFORE_SERVER_SHUTDOWN: c_int = 6;
    /// `Reactor::EXIT_CONDITION_FORCED_TERMINATION`.
    const EXIT_CONDITION_FORCED_TERMINATION: c_int = 8;
    /// `SW_SERVER_EVENT_CONNECT`.
    const EVENT_CONNECT: c_int = 5;
    /// `SW_ACCEPT_RETRY_TIME`, in milliseconds.
    #[expect(clippy::cast_possible_truncation, reason = "1000.0")]
    const ACCEPT_RETRY_MSEC: libc::c_long =
        (swoole_rs_corebase::constants::SW_ACCEPT_RETRY_TIME * 1000.0) as libc::c_long;
    let serv = o.ctx.serv;
    let slot = |index| o.state.callbacks.slot(index);
    let reopen_logger = || {
        if let Some(logger) = swoole_rs_utilities::process_logger() {
            logger.reopen();
        }
    };
    Some(match op {
        OP_NOTIFY_CONNECT => u64::from(crate::server_methods::notify(
            o.server(),
            o.conn(),
            EVENT_CONNECT,
        )),
        OP_CLOSE_CONNECTION => {
            let socket = o.conn().socket.cast();
            // SAFETY: the context's live reactor and the connection's socket.
            unsafe { crate::reactor_entry::close_connection(o.ctx.reactor, socket) };
            0
        }
        OP_FORCE_CLOSE => {
            let conn = o.conn();
            conn.close_force = 1;
            let mut event = swoole_rs_reactor::Event {
                fd: conn.fd,
                reactor_id: 0,
                fd_type: 0,
                socket: conn.socket,
            };
            // SAFETY: the context's live reactor and an event over its live connection.
            unsafe {
                reactor_upstream::trigger_close_event(o.ctx.reactor, (&raw mut event).cast())
            };
            0
        }
        OP_ACCEPT_TIMER => {
            let retry = crate::server_methods::enable_accept;
            // SAFETY: `retry` takes the server, which outlives this one-shot timer.
            let timer = unsafe {
                swoole_rs_timer::swr_v1_timer_add_plain(ACCEPT_RETRY_MSEC, false, retry, serv)
            };
            o.tables().enable_accept_timer = timer;
            u64::from(!timer.is_null())
        }
        OP_FORCED_EXIT => {
            FORCED_EXIT_SERVER.store(serv, Relaxed);
            FORCED_EXIT_TIME.store(crate::shared::unix_now(), Relaxed);
            swoole_rs_reactor::swr_v1_event_set_exit_condition(
                EXIT_CONDITION_FORCED_TERMINATION,
                forced_exit,
            );
            0
        }
        OP_THREAD0_SHUTDOWN => {
            let reactor = swoole_rs_corebase::thread_reactor();
            let thread = crate::reactor_entry::thread(o.state, 0);
            // SAFETY: the server's first reactor thread and this thread's reactor.
            unsafe { crate::reactor_entry::thread_shutdown(thread, reactor) };
            0
        }
        OP_BEFORE_SHUTDOWN => {
            call_hook(HOOK_BEFORE_SERVER_SHUTDOWN, serv);
            // SAFETY: onBeforeShutdown's slot and the live server.
            unsafe { slot(CB_BEFORE_SHUTDOWN)?.call_server(serv) };
            0
        }
        OP_SIGNAL_RELOAD => {
            // SAFETY: the live server, in its master.
            let _ = unsafe { swoole_rs_process::workers::entry::reload(serv, arg != 0) };
            reopen_logger();
            0
        }
        OP_SIGNAL_REOPEN => {
            reopen_logger();
            0
        }
        OP_ON_SHUTDOWN => {
            // SAFETY: onShutdown's slot and the live server.
            unsafe { slot(CB_SHUTDOWN)?.call_server(serv) };
            0
        }
        OP_FREE_SERVER => {
            let tables = o.tables();
            let config = o.config();
            sw_shm_free(core::mem::replace(&mut tables.session_list, core::ptr::null_mut()).cast());
            sw_shm_free(core::mem::replace(
                &mut tables.port_gs_list,
                core::ptr::null_mut(),
            ));
            sw_shm_free(core::mem::replace(&mut config.workers, core::ptr::null_mut()).cast());
            let factory = core::mem::replace(&mut config.factory_, core::ptr::null_mut());
            if !factory.is_null() {
                // SAFETY: `delete factory_`: nothing names the factory afterwards.
                unsafe { crate::factory::delete(factory) };
            }
            swoole_rs_corebase::swr_v1_core_set_server(core::ptr::null_mut());
            0
        }
        OP_JOIN_REACTOR_THREADS => {
            // SAFETY: the live server, in its master.
            unsafe { crate::reactor_entry::join_reactor_thread(serv) };
            0
        }
        OP_PORT_CLOSE => {
            let &port = o.tables().ports().get(usize::try_from(arg).ok()?)?;
            (methods().close)(port);
            0
        }
        OP_DISPATCH_FUNC => {
            let fd = libc::c_long::from(o.ctx.server_fd);
            let conn = o.state.connection_ptr(fd, 0);
            let dispatch = slot(CB_DISPATCH_FUNC)?;
            // SAFETY: dispatch_func's slot, the live server, connection and data.
            sign(unsafe { dispatch.dispatch(serv, conn.cast(), o.ctx.send) })
        }
        op => return sends(o, op, arg),
    })
}

/// The server and the time `Server::stop_master_thread`'s forced exit started.
#[cfg(all(feature = "upstream-symbols", not(test)))]
static FORCED_EXIT_SERVER: core::sync::atomic::AtomicPtr<core::ffi::c_void> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
#[cfg(all(feature = "upstream-symbols", not(test)))]
static FORCED_EXIT_TIME: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

/// The forced-termination exit condition: past `max_wait_time`, stop the loop.
#[cfg(all(feature = "upstream-symbols", not(test)))]
extern "C-unwind" fn forced_exit(_: &mut usize) -> bool {
    let serv = FORCED_EXIT_SERVER.load(Relaxed);
    // SAFETY: the server that is shutting down, live until its loop exits.
    let state = unsafe { crate::view::Server::from_raw(serv) }.state();
    let max_wait_time = state.config_ref().max_wait_time;
    let now = crate::shared::unix_now();
    if now - FORCED_EXIT_TIME.load(Relaxed) > libc::time_t::from(max_wait_time) {
        error(
            SW_LOG_WARNING,
            E::SW_ERROR_SERVER_WORKER_EXIT_TIMEOUT,
            c"Server::stop_master_thread",
            format_args!("graceful shutdown failed, forced termination"),
        );
        crate::reactor_handle::Reactor::thread().fields().running = false;
    }
    true
}

/// `serv->get_worker_message_bus()`: this thread's in thread mode, else the
/// server's.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn worker_message_bus(state: &ServerState) -> crate::objects::Bus {
    let thread_mode = u64::from(state.config_ref().mode_) == MODE_THREAD;
    let bus = if thread_mode {
        swoole_rs_corebase::thread_message_bus()
    } else {
        state.tables_ref().message_bus
    };
    // SAFETY: the thread's or the server's live `MessageBus`.
    unsafe { crate::objects::Bus::of(bus) }
}

/// `OP_BASE_PASS`: the worker message bus takes `send`, then `worker_accept_event`
/// of it.
///
/// # Safety
/// `serv` is a live server and `state` its own.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) unsafe fn base_pass(serv: *mut core::ffi::c_void, state: &ServerState, send: &SendData) {
    let bus = worker_message_bus(state);
    bus.rust().pass(&send.info, send.data);
    let event = bus.buffer();
    // SAFETY: the live server and the bus's buffer, which now holds the event.
    unsafe { entry::worker_accept_event(serv, event.cast()) };
}

/// `SendfileTask`'s head: its offset and length, then its name.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[repr(C)]
struct SendfileTask {
    offset: libc::off_t,
    length: usize,
}

/// The send ops that call a `Server` or `Socket` method crates/server,
/// crates/process or crates/tls defines; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn sends(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        OP_ACCEPT_EVENT, OP_BASE_PASS, OP_FEEDBACK_SEND, OP_FEEDBACK_THREAD, OP_FORWARD,
        OP_SC_NOTIFY_FULL, OP_SC_SENDFILE, OP_SEND_HERE, OP_SEND_INCOMING, OP_SEND_SHUTDOWN,
        OP_SENDFILE_FINISH,
    };
    use core::ffi::c_void;
    use swoole_rs_corebase::DataHead;
    use swoole_rs_tls::upstream::{
        socket_send_async as send_async, socket_sendfile_async as sendfile_async,
    };
    /// `SW_SERVER_EVENT_SEND_FILE`, `_BUFFER_FULL`, `_INCOMING` and `_SHUTDOWN_SIGNAL`.
    const EVENT_SEND_FILE: u8 = 3;
    const EVENT_BUFFER_FULL: c_int = 13;
    const EVENT_INCOMING: u8 = 15;
    const EVENT_SHUTDOWN_SIGNAL: u8 = 19;
    let serv = o.ctx.serv;
    // The connection's session and reactor in a head of `kind` (C++'s uint8_t).
    #[expect(clippy::cast_possible_truncation, reason = "C++'s (uint8_t) arg")]
    let head = |kind: u64| {
        let conn = o.conn();
        DataHead {
            fd: conn.session_id,
            reactor_id: conn.reactor_id.into(),
            kind: kind as u8,
            ..DataHead::default()
        }
    };
    // `serv->send_to_reactor_thread(head, sizeof(*head), head->fd)`.
    let to_reactor_thread = |head: &DataHead| {
        let size = size_of::<DataHead>();
        // SAFETY: the live server and a head that outlives the call.
        unsafe { entry::send_to_reactor_thread(serv, (&raw const *head).cast(), size, head.fd) }
    };
    // The `SendData` native code lends at arg.
    let lent = || {
        // SAFETY: as above.
        unsafe { &*(arg as *const SendData) }
    };
    Some(match op {
        OP_SEND_INCOMING => {
            let head = DataHead {
                server_fd: u16::try_from(o.ctx.server_fd).unwrap_or(u16::MAX),
                ..head(u64::from(EVENT_INCOMING))
            };
            (to_reactor_thread(&head) as i64).cast_unsigned()
        }
        OP_FEEDBACK_THREAD => u64::from(to_reactor_thread(&head(arg)) > 0),
        OP_FEEDBACK_SEND => {
            let send = SendData {
                info: head(arg),
                data: core::ptr::null(),
            };
            // SAFETY: the live server and the send it delivers.
            let sent = unsafe {
                crate::server_methods::send_to_connection(serv, (&raw const send).cast())
            };
            u64::from(i64::from(sent) == crate::SW_OK)
        }
        OP_SEND_HERE => {
            // SAFETY: the live server and the context's send.
            sign(unsafe { crate::server_methods::send_to_connection(serv, o.ctx.send) })
        }
        OP_SC_NOTIFY_FULL => {
            crate::server_methods::notify(o.server(), o.conn(), EVENT_BUFFER_FULL);
            o.conn().high_watermark = 1;
            0
        }
        OP_ACCEPT_EVENT => {
            // SAFETY: the live server and the `DataHead` native code lends at arg.
            unsafe { entry::worker_accept_event(serv, arg as *mut c_void) };
            0
        }
        OP_BASE_PASS => {
            // SAFETY: the live server and its state.
            unsafe { base_pass(serv, o.state, lent()) };
            0
        }
        OP_FORWARD => forward(o, lent()),
        OP_SC_SENDFILE => {
            let task = o.ctx.data.cast::<SendfileTask>();
            // SAFETY: the send's `SendfileTask`, whose name follows its head.
            let SendfileTask { offset, length } = unsafe { task.read() };
            let name = task.wrapping_add(1).cast::<c_char>();
            // SAFETY: the connection's live socket and the task's name.
            sign(unsafe { sendfile_async(o.conn().socket.cast(), name, offset, length) })
        }
        OP_SEND_SHUTDOWN => {
            // A worker may run as a user that cannot signal the master.
            let send = SendData {
                info: DataHead {
                    kind: EVENT_SHUTDOWN_SIGNAL,
                    ..DataHead::default()
                },
                data: core::ptr::null(),
            };
            // `serv->get_reactor_pipe_socket(0, 0)`.
            let pipe = o.server().worker_pipe(0, false)?;
            let data = (&raw const send).cast();
            // SAFETY: the worker's live pipe socket and the send's bytes.
            u64::from(unsafe { send_async(pipe.as_ptr().cast(), data, size_of::<SendData>()) } > 0)
        }
        OP_SENDFILE_FINISH => {
            let task = o.ctx.buffer.cast::<SendfileTask>();
            // SAFETY: `Server::sendfile`'s buffer, which starts with its task.
            unsafe { (*task).offset = o.ctx.offset };
            let head = u32::try_from(size_of::<SendfileTask>()).unwrap_or(0);
            let mut send = SendData {
                info: DataHead {
                    fd: o.ctx.session_id,
                    kind: EVENT_SEND_FILE,
                    len: head + o.ctx.length + 1,
                    ..DataHead::default()
                },
                data: o.ctx.buffer,
            };
            u64::from(factory_finish(o, &mut send))
        }
        op => return factory_ops(o, op, arg),
    })
}

/// `OP_FORWARD`: the worker message bus writes `send` to the pipe of the worker
/// that owns its session.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn forward(o: &Objects<'_>, send: &SendData) -> u64 {
    let Some(session) = o.state.session(send.info.fd) else {
        return 0;
    };
    let id = session.reactor_bits & 0xff;
    let Some(pipe) = o
        .server()
        .worker_pipe(u16::try_from(id).unwrap_or(u16::MAX), true)
    else {
        return 0;
    };
    let bus = worker_message_bus(o.state);
    let socket = if u64::from(o.config().mode_) == MODE_THREAD {
        bus.pipe_socket(pipe)
    } else {
        pipe.as_ptr().cast()
    };
    let mut info = send.info;
    // SAFETY: a live pipe socket and the send it writes.
    u64::from(unsafe { bus.rust().send(socket.cast(), &mut info, send.data.cast()) })
}

/// `serv->factory_->finish(send)`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn factory_finish(o: &Objects<'_>, send: &mut SendData) -> bool {
    crate::factory::finish(o.state, (&raw mut *send).cast())
}

/// The ops over the server's factory and task pool: the process factory's and
/// `ProcessPool`'s methods (crates/process), the factory's virtual members and
/// the task notify pipes; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn factory_ops(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        OP_DESTROY_TASK_WORKERS, OP_FACTORY_NOTIFY, OP_FACTORY_SHUTDOWN, OP_NEW_BASE_FACTORY,
        OP_PIPE_TO_WORKER, OP_TASK_PIPE, OP_TASK_POOL_CREATE, OP_TASK_POOL_LISTEN,
        OP_TASK_POOL_SETUP,
    };
    use core::ptr::{null, null_mut};
    use swoole_rs_process::workers::ops::{
        RUN_CREATE_PROCESS_FACTORY, RUN_DESTROY_PROCESS_FACTORY,
    };
    use swoole_rs_process::{methods as pool, workers::factory_methods as process};
    /// `SW_IPC_MSGQUEUE`, `SW_TASK_WORKER` and `SW_PIPE_MASTER | SW_PIPE_NONBLOCK`.
    const IPC_MSGQUEUE: u64 = 2;
    const SW_TASK_WORKER: c_int = 4;
    const PIPE_MASTER_NONBLOCK: c_int = 1 | 2;
    // crates/process' `ProcessPool::listen(socket_file, backlog)`.
    use swoole_rs_process::methods::listen_unix as pool_listen;
    let serv = o.ctx.serv;
    let config = o.config();
    Some(match op {
        OP_CREATE_FACTORY => {
            // SAFETY: the live server, at startup.
            let made = unsafe {
                process::run(
                    serv,
                    RUN_CREATE_PROCESS_FACTORY,
                    null_mut(),
                    null_mut(),
                    null(),
                    0,
                )
            };
            config.factory_ =
                core::ptr::with_exposed_provenance_mut(usize::try_from(made).unwrap_or(0));
            u64::from(!config.factory_.is_null())
        }
        OP_DESTROY_FACTORY => {
            // SAFETY: the live server, whose process factory shutdown destroys.
            unsafe {
                process::run(
                    serv,
                    RUN_DESTROY_PROCESS_FACTORY,
                    null_mut(),
                    null_mut(),
                    null(),
                    0,
                )
            };
            0
        }
        OP_DESTROY_TASK_WORKERS => {
            // SAFETY: as above.
            unsafe { crate::worker_methods::destroy_task_workers(o.server()) };
            0
        }
        OP_TASK_POOL_CREATE => {
            let task_pool = o.task_pool();
            // SAFETY: the server's task pool, reset to `{}` (no member has an initializer).
            unsafe { task_pool.write_bytes(0, 1) };
            #[expect(clippy::cast_possible_truncation, reason = "C++'s key_t conversion")]
            let key = if arg == IPC_MSGQUEUE {
                config.message_queue_key as c_int
            } else {
                0
            };
            #[expect(clippy::cast_possible_truncation, reason = "an swIPCMode")]
            // SAFETY: the reset pool.
            let created =
                unsafe { pool::create(task_pool, config.task_worker_num, key, arg as c_int) };
            u64::from(created >= 0)
        }
        OP_TASK_POOL_SETUP => {
            let task_pool = o.task_pool();
            // SAFETY: the created task pool, for each call.
            unsafe {
                pool::set_max_request(
                    task_pool,
                    config.task_max_request,
                    config.task_max_request_grace,
                );
            };
            // SAFETY: as above.
            unsafe { pool::set_start_id(task_pool, config.worker_num.cast_signed()) };
            // SAFETY: as above.
            unsafe { pool::set_type(task_pool, SW_TASK_WORKER) };
            0
        }
        OP_TASK_POOL_LISTEN => {
            // SAFETY: the created task pool and the NUL-terminated path `arg` lends.
            sign(unsafe { pool_listen(o.task_pool().cast(), ptr_of(arg), 2048) })
        }
        OP_TASK_PIPE => {
            let ready = o.state.push_task_pipe();
            if !ready {
                sw_shm_free(config.task_results);
            }
            u64::from(ready)
        }
        OP_FACTORY_SHUTDOWN => {
            crate::factory::shutdown(o.state);
            0
        }
        OP_NEW_BASE_FACTORY => {
            o.ctx.object = crate::factory::new(serv, core::ptr::null_mut());
            0
        }
        OP_FACTORY_NOTIFY => u64::from(crate::factory::notify(o.state, o.ctx.object)),
        OP_PIPE_TO_WORKER => {
            let worker = o
                .server()
                .worker(u16::try_from(o.ctx.number).unwrap_or(u16::MAX));
            // SAFETY: a live worker and the message `send_pipe_message` lends.
            let sent = unsafe {
                swoole_rs_process::methods::send_pipe_message(
                    worker,
                    o.ctx.object.cast(),
                    o.ctx.length as usize,
                    PIPE_MASTER_NONBLOCK,
                )
            };
            u64::from(sent > 0)
        }
        op => return objects(o, op, arg),
    })
}

/// `swoole::EventData`: `SW_IPC_MAX_SIZE` bytes.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[repr(C)]
struct EventData {
    info: swoole_rs_corebase::DataHead,
    data: [u8; IPC_BUFFER_SIZE],
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
const _: () = assert!(
    size_of::<EventData>() == swoole_rs_corebase::constants::SW_IPC_MAX_SIZE as usize,
    "sizeof(EventData)"
);

/// The ops over the server's ports, timers, barriers, command pipe and message
/// bus, which build or call a native `ListenPort`, `Barrier` or `UnixSocket`;
/// else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
fn objects(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        EVENT_COMMAND_REQUEST, OP_ADD_RECV_TIMER, OP_MANAGER_BARRIER, OP_PORT_IMPORT, OP_PORT_NEW,
        OP_PORT_SOCKET, OP_REACTOR_BARRIER, OP_RELEASE_PIPE_BUFFERS, OP_SC_IDLE_TIMER,
        OP_ST_CMD_SEND, OP_ST_PIPE_COMMAND_NEW,
    };
    use swoole_rs_process::command::{TO_MANAGER, TO_REACTOR_THREAD, TO_TASK_WORKER};
    /// `SW_SOCK_SSL`.
    const SOCK_SSL: c_int = 1 << 9;
    use crate::server_object::{barrier, port_delete, port_new};
    use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
    let serv = o.ctx.serv;
    Some(match op {
        // `swoole_timer_add(sec2msec(port->max_idle_time), true, get_timeout_callback(...))`
        // for the connection's reads, or the context socket's sends.
        OP_ADD_RECV_TIMER | OP_SC_IDLE_TIMER => {
            let idle = o.port().max_idle_time;
            let sock = if op == OP_ADD_RECV_TIMER {
                o.conn_socket()
            } else {
                o.sock()
            }?;
            let tick = crate::server_methods::idle_timeout_tick;
            #[expect(clippy::cast_possible_truncation, reason = "sec2msec's (long) cast")]
            let msec = (idle * 1000.0) as libc::c_long;
            // SAFETY: `tick` takes the live connection, which deletes the timer
            // before it goes.
            let node =
                unsafe { swoole_rs_timer::swr_v1_timer_add_plain(msec, true, tick, o.ctx.conn) };
            let now = (op != OP_ADD_RECV_TIMER).then(|| swoole_rs_reactor::steady_ms());
            sock.edit(|sock| {
                if let Some(now) = now {
                    sock.timeouts.write = idle;
                    sock.last_sent_time = now;
                    sock.output.send_timer = node;
                } else {
                    sock.timeouts.read = idle;
                    sock.recv_timer = node;
                }
            });
            0
        }
        OP_REACTOR_BARRIER | OP_MANAGER_BARRIER => {
            // SAFETY: the live server's barrier, initialized at startup.
            swoole_rs_locks::barrier_wait(unsafe { barrier(serv, op == OP_MANAGER_BARRIER) });
            0
        }
        OP_RELEASE_PIPE_BUFFERS => {
            release_pipe_buffers(o.state);
            0
        }
        OP_PORT_NEW => {
            let ls = port_new(serv);
            o.ctx.port = ls;
            let port = o.port();
            port.r#type = o.ctx.socket_type;
            port.port = o.ctx.number;
            // SAFETY: the NUL-terminated host the caller lends.
            let host = unsafe { CStr::from_ptr(o.ctx.host) };
            // SAFETY: the new port's own slot.
            let set = unsafe {
                swoole_rs_corebase::swr_v1_core_string_set(
                    &raw const *port.host,
                    host.as_ptr(),
                    host.count_bytes(),
                )
            };
            if set < 0 {
                // `throw std::bad_alloc()`, which nothing catches.
                std::process::abort();
            }
            if arg != 0 {
                port.r#type = o.ctx.socket_type & !SOCK_SSL;
                port.ssl = 1;
                (methods().ssl_context_init)(ls);
            }
            0
        }
        OP_PORT_SOCKET => {
            if (methods().create_socket)(o.ctx.port) < 0 {
                log::set_last_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
                // SAFETY: the new port, deleted once.
                unsafe { port_delete(o.ctx.port) };
                o.ctx.port = core::ptr::null_mut();
                return Some(0);
            }
            1
        }
        OP_PORT_IMPORT => {
            let ls = port_new(serv);
            #[expect(clippy::cast_possible_truncation, reason = "C++'s (int) arg")]
            let fd = arg as c_int;
            if !(methods().import)(ls, fd) {
                // SAFETY: the new port, deleted once.
                unsafe { port_delete(ls) };
                return Some(0);
            }
            o.ctx.port = ls;
            // The record `import` made: O_NONBLOCK and O_CLOEXEC.
            o.port_socket()?
                .edit(|socket| socket_set_fd_option(socket, 1, 1));
            1
        }
        OP_ST_CMD_SEND => {
            // SAFETY: the `CommandSend` `arg` lends.
            let c = unsafe { &*ptr_of::<crate::commands::CommandSend>(arg) };
            let process_id = u16::try_from(c.process_id).unwrap_or(u16::MAX);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "C++'s conversions into DataHead's fields"
            )]
            let head = |info: &mut swoole_rs_corebase::DataHead| {
                info.kind = EVENT_COMMAND_REQUEST as u8;
                info.fd = c.request_id as libc::c_long;
                info.server_fd = c.command_id as u16;
            };
            if c.action == TO_TASK_WORKER || c.action == TO_MANAGER {
                // SAFETY: an all-zero `EventData` is valid.
                let mut buf: EventData = unsafe { core::mem::zeroed() };
                let sent = if c.action == TO_TASK_WORKER {
                    // SAFETY: a writable `EventData` and the message's bytes.
                    unsafe {
                        swoole_rs_process::task_file::task_pack(
                            (&raw mut buf).cast(),
                            c.message.cast(),
                            c.length,
                        )
                    }
                } else {
                    let length = c.length.min(IPC_BUFFER_SIZE);
                    buf.info.len = u32::try_from(length).unwrap_or(u32::MAX);
                    // SAFETY: the message's bytes, within the buffer.
                    unsafe {
                        core::ptr::copy_nonoverlapping(c.message, buf.data.as_mut_ptr(), length);
                    };
                    true
                };
                head(&mut buf.info);
                if !sent {
                    return Some(0);
                }
                let sent = if c.action == TO_TASK_WORKER {
                    let mut dst = c_int::from(process_id);
                    let (gs, pool) = (o.gs(), o.task_pool());
                    swoole_rs_process::task_sync::count_task(
                        &gs.tasking_num,
                        &gs.task_count,
                        || {
                            // SAFETY: the live task pool and the packed task.
                            let sent = unsafe {
                                swoole_rs_process::methods::dispatch(
                                    pool,
                                    (&raw mut buf).cast(),
                                    &raw mut dst,
                                    false,
                                )
                            };
                            Ok(sent == 0)
                        },
                    )
                    .unwrap_or(false)
                } else {
                    let pool = core::ptr::from_mut(o.event_pool());
                    // SAFETY: the live event pool and the message.
                    unsafe {
                        swoole_rs_process::methods::push_event(pool, (&raw const buf).cast()) >= 0
                    }
                };
                return Some(u64::from(sent));
            }
            #[expect(clippy::cast_possible_truncation, reason = "DataHead's fields")]
            let mut task = SendData {
                info: swoole_rs_corebase::DataHead {
                    reactor_id: process_id.cast_signed(),
                    len: c.length as u32,
                    ..Default::default()
                },
                data: c.message.cast(),
            };
            head(&mut task.info);
            // SAFETY: the live server and one of its workers.
            let worker = unsafe { crate::view::Server::from_raw(serv) }.worker(process_id);
            // SAFETY: as above.
            let worker = unsafe { &*worker };
            let pipe = if c.action == TO_REACTOR_THREAD {
                worker.pipe_worker
            } else {
                worker.pipe_master
            };
            let bus = server_message_bus(o.state);
            // SAFETY: a worker pipe and the message.
            u64::from(unsafe {
                bus.rust()
                    .send(pipe.cast(), &mut task.info, task.data.cast())
            })
        }
        OP_ST_PIPE_COMMAND_NEW => {
            use core::mem::MaybeUninit;
            use swoole_rs_tls::socket_pair::{drop, unix_socket_new};
            let mut storage = Box::new(MaybeUninit::<SocketPair>::uninit());
            unix_socket_new(&mut storage, false, libc::SOCK_DGRAM);
            // SAFETY: the constructor initialised it.
            let mut built = unsafe { storage.assume_init() };
            if built.master_socket.is_null() || built.worker_socket.is_null() {
                drop(&mut built);
                return Some(0);
            }
            o.config().pipe_command = Box::into_raw(built).cast();
            1
        }
        _ => return None,
    })
}

/// The server's own `MessageBus`: its Rust bus, then its allocator.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn server_message_bus(state: &ServerState) -> crate::objects::Bus {
    let bus = state.tables_ref().message_bus;
    // SAFETY: the server's live `MessageBus`.
    unsafe { crate::objects::Bus::of(bus) }
}

/// `Server::create_pipe_buffers()`: the bus's buffer, `ipc_max_size` bytes.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn create_pipe_buffers(serv: crate::view::Server<'_>) -> c_int {
    let state = serv.state();
    let size = state.config_ref().ipc_max_size;
    if server_message_bus(state).alloc_buffer(size as usize) {
        0
    } else {
        -1
    }
}

/// `MessageBus::free_buffer()` of the server's bus.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn release_pipe_buffers(state: &ServerState) {
    server_message_bus(state).free_buffer();
}

/// `Server::accept_command_result`: the master's command pipe is readable; a
/// whole reply goes to its waiting callback. The callback may resume a coroutine:
/// this frame owns nothing that needs a drop then.
///
/// Registered only on the master's reactor, whose `ptr` is the live server, for its
/// command pipe.
#[cfg(all(feature = "upstream-symbols", not(test)))]
extern "C" fn accept_command_result(
    reactor: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    let serv = crate::reactor_handle::Reactor::running(reactor)
        .fields()
        .ptr;
    // SAFETY: the master reactor's live server.
    let state = unsafe { crate::view::Server::from_raw(serv) }.state();
    let bus = server_message_bus(state);
    // SAFETY: the event's live socket.
    let Some(sock) = (unsafe { Socket::from_ptr(event.socket.cast()) }) else {
        return 0;
    };
    if bus.read(sock) <= 0 {
        return 0;
    }
    let packet = bus.packet();
    let request_id = bus.head().fd;
    // SAFETY: the live state and server, and the packet's bytes.
    unsafe {
        crate::commands::swr_v1_server_command_result(
            state,
            serv,
            request_id,
            packet.data,
            packet.length,
        );
    };
    bus.pop();
    0
}

/// A pointer an op passes in its argument.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn ptr_of<T>(arg: u64) -> *const T {
    core::ptr::with_exposed_provenance(usize::try_from(arg).unwrap_or(0))
}

/// Outside the hybrid, whose unit tests link no C++, native code keeps these.
#[cfg(not(all(feature = "upstream-symbols", not(test))))]
fn native(_: &mut Objects<'_>, _: u32, _: u64) -> Option<u64> {
    None
}

/// `Server::destroy_base_factory()`: the connection list and the connection counts.
fn destroy_base_factory(o: &Objects<'_>) -> u64 {
    let tables = o.tables();
    // SAFETY: the base factory's `sw_calloc`ed connection list, freed once.
    unsafe { swoole_rs_corebase::sw_free(tables.connection_list.cast()) };
    sw_shm_free(o.gs().connection_nums.cast());
    for index in 0..tables.ports().len() {
        if let Some(gs) = o.port_at(index as u64).and_then(|port| port.counters()) {
            sw_shm_free(gs.connection_nums.cast());
        }
    }
    o.gs().connection_nums = core::ptr::null_mut();
    0
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
                answer(&raw mut ctx, &*state, crate::ops::OP_PORT_COUNT, 0),
                None
            );
            assert_eq!(
                answer(core::ptr::null_mut(), &*state, OP_SEND_TIMEOUT, 0),
                None
            );
            crate::swr_v1_server_state_free(state);
        }
    }

    #[test]
    fn pipe_fd_and_datagram_ip_key_read_their_records() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: plain records, for which zero is valid.
        let (mut master, mut worker): (SocketRecord, SocketRecord) =
            unsafe { (core::mem::zeroed(), core::mem::zeroed()) };
        master.fd = 7;
        worker.fd = 8;
        let mut pair = SocketPair {
            blocking: false,
            timeout: 0.0,
            socks: [8, 7],
            master_socket: &raw mut master,
            worker_socket: &raw mut worker,
        };
        ctx.object = (&raw mut pair).cast();
        // SAFETY: as above.
        let mut packet: DgramPacket = unsafe { core::mem::zeroed() };
        packet.socket_addr.kind = 2;
        packet.socket_addr.addr.inet_v4.sin_addr.s_addr = 0x0a00_0001_u32.to_be();
        let mut send = crate::base_factory::SendData {
            // SAFETY: as above.
            info: unsafe { core::mem::zeroed() },
            data: (&raw const packet).cast(),
        };
        ctx.send = (&raw mut send).cast();
        ctx.server_fd = -1;
        // SAFETY: the context's records are live above; no connection has fd -1.
        unsafe {
            assert_eq!(answer(&raw mut ctx, &*state, OP_ST_PIPE_FD, 0), Some(7));
            assert_eq!(answer(&raw mut ctx, &*state, OP_ST_PIPE_FD, 1), Some(8));
            assert_eq!(
                answer(&raw mut ctx, &*state, OP_IP_KEY, 0),
                Some(0x0a00_0001)
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
                answer(&raw mut ctx, &*state, OP_SEND_TIMEOUT, 0),
                Some(2.5f64.to_bits())
            );
            answer(&raw mut ctx, &*state, OP_SEND_TIMEOUT_MIN, 0);
            assert_eq!(config.send_timeout, 0.001);
            answer(&raw mut ctx, &*state, OP_CLEAR_SEND_YIELD, 0);
            assert!(!config.send_yield);
            answer(&raw mut ctx, &*state, OP_BASE_REACTORS, 0);
            assert_eq!(config.reactor_num, 6);
            config.scheduler_warning = true;
            answer(&raw mut ctx, &*state, OP_WARNING_RESET, 7 << 1);
            assert!(!config.scheduler_warning);
            assert_eq!(answer(&raw mut ctx, &*state, OP_WARNING_TIME, 0), Some(7));
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
            assert_eq!(answer(&raw mut ctx, &*state, OP_HOST_SIZE, 0), Some(8));
            answer(&raw mut ctx, &*state, OP_SC_ADVANCE, 3);
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
                answer(&raw mut ctx, &*state, OP_ST_W_CONCURRENCY_CAS, 2),
                Some(3)
            );
            assert_eq!((*ctx.object.cast::<WorkerRecord>()).concurrency, 3);
            assert_eq!(
                answer(&raw mut ctx, &*state, OP_ST_W_CONCURRENCY_CAS, 3),
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
        let name = unsafe { answer(&raw mut ctx, &*state, OP_SENDFILE_NAME, 0) };
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
            assert_eq!(answer(&raw mut ctx, &*state, OP_SIGNAL_ACTIVE, 0), Some(0));
            for op in [
                OP_ALLOC_WORKERS,
                OP_ALLOC_TASK_RESULTS,
                OP_ALLOC_CONNECTION_NUMS,
                OP_ALLOC_SESSIONS,
            ] {
                assert_eq!(answer(&raw mut ctx, &*state, op, 0), Some(1), "op {op}");
            }
            assert_eq!((*gs.connection_nums.add(2)).load(Relaxed), 0, "zeroed");
            let tables = &mut *(*state).tables();
            assert_eq!((*tables.session_list.add(5)).fd, 0, "zeroed");
            config.user_workers = sw_shm_calloc(1, 8);
            answer(&raw mut ctx, &*state, OP_FREE_USER_WORKERS, 0);
            assert!(config.user_workers.is_null());
            answer(&raw mut ctx, &*state, OP_FREE_USER_WORKERS, 0);
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

    #[test]
    fn send_buffer_ops_queue_on_the_sockets_output() {
        unsafe extern "C" {
            #[link_name = "_ZdlPv"]
            fn operator_delete(storage: *mut core::ffi::c_void);
        }
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: plain records, for which zero is valid.
        let mut sock: SocketRecord = unsafe { core::mem::zeroed() };
        // SAFETY: as above.
        let mut conn: ConnectionRecord = unsafe { core::mem::zeroed() };
        let config = crate::port_config::swr_v1_port_config_new(0);
        // A `ListenPort` starts with its settings pointer.
        let mut port = config;
        sock.output.buffer_size = 4;
        // SAFETY: the settings allocated above.
        unsafe { (*config).buffer_high_watermark = 2 };
        let data = *b"abc";
        ctx.sock = (&raw mut sock).cast();
        ctx.conn = (&raw mut conn).cast();
        ctx.port = (&raw mut port).cast();
        ctx.data = data.as_ptr().cast();
        ctx.length = 3;
        let socket = (&raw const sock).addr() as u64;
        // SAFETY: the context's records are live above.
        unsafe {
            assert_eq!(answer(&raw mut ctx, &*state, OP_OUT_EMPTY, socket), Some(1));
            answer(&raw mut ctx, &*state, OP_SC_ENSURE_BUFFER, 0);
            assert!(!sock.output.out_buffer.is_null());
            assert_eq!(answer(&raw mut ctx, &*state, OP_OUT_EMPTY, socket), Some(1));
            answer(&raw mut ctx, &*state, OP_SC_APPEND, 0);
            assert_eq!(conn.send_queued_bytes, 3);
            assert_eq!(answer(&raw mut ctx, &*state, OP_OUT_EMPTY, socket), Some(0));
            assert_eq!(answer(&raw mut ctx, &*state, OP_SC_OUT_FULL, 0), Some(0));
            assert_eq!(answer(&raw mut ctx, &*state, OP_SC_OUT_HIGH, 0), Some(1));
            answer(&raw mut ctx, &*state, OP_SC_QUEUE_CLOSE, 0);
            assert_eq!(conn.close_queued, 1);
            let buffer = sock.output.out_buffer.cast::<CppBuffer>();
            assert_eq!(swoole_rs_buffer::cpp::swr_cpp_buffer_count(&*buffer), 2);
            swoole_rs_buffer::cpp::swr_cpp_buffer_drop(&mut *buffer);
            operator_delete(buffer.cast());
            crate::port_config::swr_v1_port_config_free(config);
            crate::swr_v1_server_state_free(state);
        }
    }
}
