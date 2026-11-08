//! The base-mode reactor process's facts and effects
//! (`crates/process/src/workers/reactor_process.rs`'s `OP_*`) over the server,
//! port, worker and connection records Rust lays out. workers.cc's
//! `ReactorProcessContext::call` hands every op it has no case for to
//! [`swr_v1_reactor_process_call`].
use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering;

use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_id};
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::ops::{
    OP_ADD_MASTER_TIMER, OP_ALL_WORKER_NUM, OP_CLOSE_CONNECTION, OP_CLOSE_EVENT_SOCKET,
    OP_CLOSE_SESSION, OP_CONN_AT, OP_CONN_CLOSE_FORCE, OP_CONN_CLOSE_QUEUED, OP_CONN_HEALTHY,
    OP_CONN_SSL_PENDING, OP_DISABLE_NOTIFY, OP_EVENT_CONN_ACTIVE, OP_EVENT_SOCKET_REMOVED,
    OP_HAS_ON_START, OP_HAS_PIPE_WORKER, OP_HAS_REACTOR, OP_HAVE_STREAM_SOCK,
    OP_HEARTBEAT_INTERVAL, OP_HEARTBEAT_NOW, OP_INIT_IPC_MAX_SIZE, OP_INIT_REACTOR, OP_MAXFD,
    OP_MINFD, OP_NOTIFY_CLOSE, OP_ON_START, OP_ONSTART_CALLED, OP_POOL_DESTROY,
    OP_PORT_CLOSE_SOCKET, OP_PORT_COUNT, OP_PORT_CREATE_SOCKET, OP_PORT_LISTEN, OP_PORT_REUSE,
    OP_PORT_STREAM, OP_REACTOR_WAIT, OP_RESET_CONNECTION_NUM, OP_REUSE_PORT, OP_SET_CLOSE_FORCE,
    OP_SET_MASTER_PID, OP_SET_PEER_CLOSED, OP_SET_SINGLE_THREAD, OP_SINGLE_WORKER,
    OP_START_MANAGER, OP_STORE_LISTEN_SOCKET, OP_TASK_IPC_UNIXSOCK, OP_TASK_POOL_WORKER_NUM,
    OP_TASK_WORKER_NUM, OP_WORKER_ENTER, OP_WORKER_ID, OP_WORKER_SIGNAL_INIT,
    OP_WORKER_START_CALLBACK, OP_WORKER_STOP_CALLBACK,
};
use swoole_rs_reactor::SocketRecord;

use crate::ConnectionRecord;
use crate::callbacks::CB_START;
use crate::reactor_ops::sign;
use crate::view;

/// workers.cc's `WorkersNative<1>`.
#[repr(C)]
struct Context {
    serv: *mut c_void,
    pool: *mut c_void,
    worker: *mut WorkerRecord,
    reactor: *mut c_void,
    event: *mut Event,
    conn: *mut ConnectionRecord,
    now: f64,
}

/// `swoole::Event`.
#[repr(C)]
struct Event {
    fd: c_int,
    reactor_id: i16,
    fd_type: c_int,
    socket: *mut SocketRecord,
}

/// `Server::TASK_IPC_UNIXSOCK`, `SW_SERVER_EVENT_CLOSE`, `SW_SSL_STATE_READY`.
const TASK_IPC_UNIXSOCK: u8 = 1;
const EVENT_CLOSE: c_int = 4;
const SSL_READY: u32 = 1;

// SAFETY: crates/ports', native/reactor's and crates/process's bodies, under
// their upstream symbols.
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole10ListenPort6listenEv"]
    pub(crate) fn port_listen(port: *mut c_void) -> c_int;
    #[link_name = "_ZN6swoole10ListenPort13create_socketEv"]
    fn port_create_socket(port: *mut c_void) -> c_int;
    #[link_name = "_ZN6swoole10ListenPort12close_socketEv"]
    fn port_close_socket(port: *mut c_void);
    #[link_name = "_ZN6swoole7Reactor4waitEv"]
    fn reactor_wait(reactor: *mut c_void) -> c_int;
    #[link_name = "_ZN6swoole11ProcessPool7destroyEv"]
    fn pool_destroy(pool: *mut c_void);
}

/// Answers reactor-process op `op` with `arg` over the `WorkersNative<1>` at `context`.
///
/// # Safety
/// `context` is workers.cc's live `ReactorProcessContext`, whose objects the op reads are live.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_reactor_process_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    // SAFETY: per the caller.
    let answer = unsafe { server(ctx, op, arg) };
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { worker(ctx, op) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { connection(ctx, op, arg) });
    answer.unwrap_or_else(|| std::process::abort())
}

/// `serv->ports`, in listen order.
///
/// # Safety
/// `serv` is a live `Server`.
pub(crate) unsafe fn ports<'a>(serv: *mut c_void) -> &'a [*mut c_void] {
    // SAFETY: per the caller.
    let state = unsafe { view::state_ptr(serv) };
    // SAFETY: its state and tables live as long as it.
    let tables = unsafe { (*state).tables() };
    // SAFETY: as above.
    unsafe { tables.as_ref() }.map_or(&[], |tables| tables.ports())
}

/// `Socket::is_stream(type)`.
fn is_stream(kind: c_int) -> bool {
    matches!(kind, 1 | 3 | 5)
}

/// Whether listening ports may use `SO_REUSEPORT` (`enable_reuse_port` on Linux).
fn reuse_port(config: &crate::ServerConfig) -> bool {
    cfg!(target_os = "linux") && config.enable_reuse_port
}

/// The facts and effects on the server and its ports.
///
/// # Safety
/// As [`swr_v1_reactor_process_call`].
unsafe fn server(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: the context's live server.
    let config = unsafe { view::config(serv) };
    let port = || {
        // SAFETY: as above.
        let ports = unsafe { ports(serv) };
        usize::try_from(arg)
            .ok()
            .and_then(|i| ports.get(i).copied())
    };
    let stream = |port: *mut c_void| {
        // SAFETY: one of the server's live ports.
        is_stream(unsafe { view::port(port) }.r#type)
    };
    Some(match op {
        OP_SET_SINGLE_THREAD => {
            config.single_thread = true;
            0
        }
        OP_HAVE_STREAM_SOCK => u64::from(config.have_stream_sock),
        // SAFETY: the context's live server.
        OP_PORT_COUNT => unsafe { ports(serv) }.len() as u64,
        OP_PORT_STREAM => u64::from(stream(port()?)),
        OP_REUSE_PORT => u64::from(reuse_port(config)),
        OP_PORT_REUSE => u64::from(reuse_port(config) && stream(port()?)),
        OP_PORT_CLOSE_SOCKET => {
            let port = port()?;
            if reuse_port(config) {
                // SAFETY: one of the server's live ports.
                unsafe { port_close_socket(port) };
            }
            0
        }
        OP_PORT_CREATE_SOCKET => {
            let port = port()?;
            if !reuse_port(config) {
                return Some(0);
            }
            // SAFETY: as above.
            sign(unsafe { port_create_socket(port) })
        }
        // SAFETY: as above.
        OP_PORT_LISTEN => sign(unsafe { port_listen(port()?) }),
        OP_INIT_IPC_MAX_SIZE => {
            // SAFETY: the context's live server.
            unsafe { crate::worker_methods::init_ipc_max_size(serv) };
            0
        }
        OP_SINGLE_WORKER => u64::from(
            config.worker_num == 1
                && config.task_worker_num == 0
                && config.max_request == 0
                // SAFETY: the context's live server.
                && unsafe { view::user_worker_num(serv) } == 0,
        ),
        // SAFETY: as above.
        OP_START_MANAGER => sign(unsafe { entry::start_manager_process(serv) }),
        OP_POOL_DESTROY => {
            // SAFETY: the context's live server.
            let pool = unsafe { view::workers(serv) }.event_pool;
            // SAFETY: its live event worker pool.
            unsafe { pool_destroy(pool.cast()) };
            0
        }
        OP_ALL_WORKER_NUM => {
            // SAFETY: the context's live server.
            let workers = unsafe { view::workers(serv) };
            u64::from(workers.event_count)
                + u64::from(workers.task_count)
                + workers.user_count as u64
        }
        OP_HAS_REACTOR => u64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        OP_STORE_LISTEN_SOCKET => {
            // SAFETY: the context's live server.
            unsafe { crate::server_methods::store_listen_socket(serv) };
            0
        }
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        OP_TASK_IPC_UNIXSOCK => u64::from(config.task_ipc_mode == TASK_IPC_UNIXSOCK),
        OP_TASK_POOL_WORKER_NUM => {
            // SAFETY: the context's live server.
            let pool = unsafe { view::task_pool(serv) };
            // SAFETY: its live task pool.
            u64::from(unsafe { (*pool).worker_num })
        }
        OP_INIT_REACTOR => {
            // SAFETY: the context's live server and the running reactor.
            unsafe { crate::reactor_entry::init_reactor(serv, reactor(ctx)) };
            0
        }
        OP_SET_MASTER_PID => {
            // SAFETY: the live server's counters; a plain syscall.
            unsafe { view::counters_mut(serv) }.master_pid = unsafe { libc::getpid() };
            0
        }
        // SAFETY: the context's live server.
        OP_HAS_ON_START => u64::from(unsafe { view::has_callback(serv, CB_START) }),
        // SAFETY: the live server's counters.
        OP_ONSTART_CALLED => u64::from(unsafe { view::counters(serv) }.onstart_called),
        OP_ON_START => {
            // SAFETY: the live server's counters.
            unsafe { view::counters_mut(serv) }.onstart_called = true;
            // SAFETY: the context's live server.
            let state = unsafe { view::state_ptr(serv) };
            // SAFETY: its state, live as long as it.
            let state = unsafe { &*state };
            if let Some(slot) = state.callbacks.slot(CB_START) {
                // SAFETY: `onStart` takes the server.
                unsafe { slot.call_server(serv) };
            }
            0
        }
        OP_ADD_MASTER_TIMER => {
            let tick = crate::server_methods::master_timer_tick;
            // SAFETY: a persistent timer over the live server.
            let node =
                unsafe { crate::pool_native::swr_v1_timer_add_plain(1000, true, tick, serv) };
            config.master_timer = node;
            u64::from(!node.is_null())
        }
        OP_HEARTBEAT_INTERVAL => u64::from(config.heartbeat_check_interval),
        // SAFETY: the running reactor.
        OP_REACTOR_WAIT => sign(unsafe { reactor_wait(reactor(ctx)) }),
        // SAFETY: the live server's counters.
        OP_MINFD => sign(unsafe { view::counters(serv) }.min_fd),
        // SAFETY: as above.
        OP_MAXFD => sign(unsafe { view::counters(serv) }.max_fd),
        OP_DISABLE_NOTIFY => u64::from(config.disable_notify),
        _ => return None,
    })
}

/// The context's reactor, or the thread's.
fn reactor(ctx: &Context) -> *mut c_void {
    if ctx.reactor.is_null() {
        swoole_rs_corebase::thread_reactor()
    } else {
        ctx.reactor
    }
}

/// The effects on the context's worker.
///
/// # Safety
/// As [`swr_v1_reactor_process_call`].
unsafe fn worker(ctx: &Context, op: u32) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    // SAFETY: the ops below run with the context's live worker.
    let id = || unsafe { swr_v1_worker_id(worker) };
    match op {
        OP_WORKER_ENTER => {
            swoole_rs_process::methods::swoole_set_worker_type(SW_EVENT_WORKER);
            swoole_rs_process::methods::swoole_set_worker_id(id());
            // SAFETY: a plain syscall.
            swoole_rs_process::methods::swoole_set_worker_pid(unsafe { libc::getpid() });
            // SAFETY: the context's live server and worker.
            unsafe { crate::worker_methods::init_event_worker(serv, worker) };
        }
        // SAFETY: the context's live server.
        OP_WORKER_SIGNAL_INIT => unsafe { entry::worker_signal_init(serv) },
        OP_RESET_CONNECTION_NUM => {
            // SAFETY: the live server's counters, one slot per worker.
            let nums = unsafe { view::counters(serv) }.connection_nums;
            // SAFETY: the worker's slot.
            let slot = unsafe { &*nums.wrapping_add(id() as usize) };
            slot.store(0, Ordering::Relaxed);
        }
        // SAFETY: the context's live worker.
        OP_HAS_PIPE_WORKER => return Some(u64::from(!unsafe { (*worker).pipe_worker }.is_null())),
        OP_WORKER_ID => return Some(u64::from(id())),
        // SAFETY: the context's live server and worker.
        OP_WORKER_START_CALLBACK => unsafe { entry::worker_start_callback(serv, worker.cast()) },
        // SAFETY: as above.
        OP_WORKER_STOP_CALLBACK => unsafe { entry::worker_stop_callback(serv, worker.cast()) },
        _ => return None,
    }
    Some(0)
}

/// `swoole::WorkerType::SW_EVENT_WORKER`.
const SW_EVENT_WORKER: c_int = 2;

/// The connection the context selected (`OP_CONN_AT`, `OP_EVENT_CONN_ACTIVE`) and
/// the event being closed.
///
/// # Safety
/// As [`swr_v1_reactor_process_call`]; the connection ops run with the connection
/// selected, the event ops with the context's event.
unsafe fn connection(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    match op {
        OP_CONN_AT => {
            let fd = c_int::try_from(arg.cast_signed()).unwrap_or(-1);
            // SAFETY: the context's live server.
            let conn = unsafe { view::at(serv, fd) }.map_or(core::ptr::null_mut(), |c| &raw mut *c);
            // SAFETY: null or a live connection whose socket is null or live.
            let valid = unsafe { crate::reactor_ops::valid(conn) };
            ctx.conn = if valid { conn } else { core::ptr::null_mut() };
            return Some(u64::from(valid));
        }
        OP_EVENT_CONN_ACTIVE => {
            // SAFETY: the context's live event.
            let fd = unsafe { (*ctx.event).fd };
            // SAFETY: the context's live server.
            let conn = unsafe { view::at(serv, fd) };
            let active = conn.as_ref().is_some_and(|c| c.active != 0);
            ctx.conn = conn.map_or(core::ptr::null_mut(), |c| &raw mut *c);
            return Some(u64::from(active));
        }
        OP_EVENT_SOCKET_REMOVED => {
            // SAFETY: the context's live event.
            let socket = unsafe { (*ctx.event).socket };
            // SAFETY: its live socket.
            return Some(u64::from(unsafe { (*socket).removed }));
        }
        OP_CLOSE_EVENT_SOCKET => {
            // SAFETY: the context's live event.
            let socket = unsafe { (*ctx.event).socket };
            // SAFETY: the running reactor and the event's session socket.
            let closed = unsafe { crate::reactor_entry::close_connection(reactor(ctx), socket) };
            return Some(sign(closed));
        }
        OP_HEARTBEAT_NOW => {
            ctx.now = swoole_rs_corebase::microtime();
            return Some(0);
        }
        _ => {}
    }
    // SAFETY: null or the connection selected.
    let conn = unsafe { ctx.conn.as_mut() }?;
    let socket = conn.socket.cast::<SocketRecord>();
    Some(match op {
        OP_CLOSE_SESSION => {
            // SAFETY: the context's live server.
            unsafe { crate::server_methods::close(serv, conn.session_id, true) };
            0
        }
        OP_CONN_CLOSE_QUEUED => u64::from(conn.close_queued),
        OP_SET_PEER_CLOSED => {
            // the client has closed the connection and the connection is no longer available.
            conn.peer_closed = 1;
            0
        }
        OP_NOTIFY_CLOSE => {
            // SAFETY: the context's live server and one of its connections.
            u64::from(unsafe { crate::server_methods::notify(serv, conn, EVENT_CLOSE) })
        }
        // SAFETY: the context's live server and one of its connections.
        OP_CONN_HEALTHY => u64::from(unsafe { view::healthy(serv, ctx.now, conn) }),
        OP_CONN_SSL_PENDING => {
            // SAFETY: the valid connection's live socket.
            let socket = unsafe { &*socket };
            u64::from(!socket.ssl.is_null() && socket.tls.state != SSL_READY)
        }
        OP_CONN_CLOSE_FORCE => u64::from(conn.close_force),
        OP_CLOSE_CONNECTION => {
            // SAFETY: the running reactor and the connection's session socket.
            unsafe { crate::reactor_entry::close_connection(reactor(ctx), socket) };
            0
        }
        OP_SET_CLOSE_FORCE => {
            conn.close_force = 1;
            0
        }
        _ => return None,
    })
}
