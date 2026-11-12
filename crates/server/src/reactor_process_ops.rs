//! The base-mode reactor process's facts and effects
//! (`crates/process/src/workers/reactor_process.rs`'s `OP_*`) over the server,
//! port, worker and connection records Rust lays out.
use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering;

use swoole_rs_process::methods::destroy as pool_destroy;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::workers::ops::{
    OP_ADD_MASTER_TIMER, OP_ALL_WORKER_NUM, OP_CLOSE_CONNECTION, OP_CLOSE_EVENT_SOCKET,
    OP_CLOSE_SESSION, OP_CONN_AT, OP_CONN_CLOSE_FORCE, OP_CONN_CLOSE_QUEUED, OP_CONN_HEALTHY,
    OP_CONN_SSL_PENDING, OP_DISABLE_NOTIFY, OP_END_ACTIVELY, OP_EVENT_CONN_ACTIVE,
    OP_EVENT_SOCKET_REMOVED, OP_FINISH_SEND_DATA, OP_FINISH_SEND_FILE, OP_HAS_ON_START,
    OP_HAS_PIPE_WORKER, OP_HAS_REACTOR, OP_HAVE_STREAM_SOCK, OP_HEARTBEAT_INTERVAL,
    OP_HEARTBEAT_NOW, OP_INIT_IPC_MAX_SIZE, OP_INIT_REACTOR, OP_MAXFD, OP_MINFD, OP_NOTIFY_CLOSE,
    OP_ON_START, OP_ONSTART_CALLED, OP_POOL_DESTROY, OP_PORT_CLOSE_SOCKET, OP_PORT_COUNT,
    OP_PORT_CREATE_SOCKET, OP_PORT_LISTEN, OP_PORT_REUSE, OP_PORT_STREAM, OP_REACTOR_WAIT,
    OP_RESET_CONNECTION_NUM, OP_REUSE_PORT, OP_SET_CLOSE_FORCE, OP_SET_MASTER_PID,
    OP_SET_PEER_CLOSED, OP_SET_SINGLE_THREAD, OP_SINGLE_WORKER, OP_START_MANAGER,
    OP_STORE_LISTEN_SOCKET, OP_TASK_IPC_UNIXSOCK, OP_TASK_POOL_WORKER_NUM, OP_TASK_WORKER_NUM,
    OP_WORKER_ENTER, OP_WORKER_ID, OP_WORKER_SIGNAL_INIT, OP_WORKER_START_CALLBACK,
    OP_WORKER_STOP_CALLBACK,
};
use swoole_rs_reactor::{Event, SocketRecord};

use crate::ConnectionRecord;
use crate::callbacks::CB_START;
use crate::port_config::methods;
use crate::reactor_handle::Reactor;
use crate::reactor_ops::sign;
use crate::view;

mod native;
pub(crate) use native::start_reactor_processes;

/// What an operation runs over.
struct Context<'a> {
    serv: view::Server<'a>,
    worker: Option<Worker<'a>>,
    reactor: Option<Reactor>,
    event: Option<&'a Event>,
    conn: *mut ConnectionRecord,
    now: f64,
}

impl<'a> Context<'a> {
    /// The context's live server.
    fn server(&self) -> view::Server<'a> {
        self.serv
    }
}

/// `Server::TASK_IPC_UNIXSOCK`, `SW_SERVER_EVENT_CLOSE`, `SW_SSL_STATE_READY`.
const TASK_IPC_UNIXSOCK: u8 = 1;
const EVENT_CLOSE: c_int = 4;
const SSL_READY: u32 = 1;

/// Answers each reactor-process op over the context.
impl swoole_rs_process::workers::Answer for Context<'_> {
    fn answer(&mut self, op: u32, arg: u64) -> u64 {
        let ctx = self;
        native::native(ctx, op, arg)
            .or_else(|| server(ctx, op, arg))
            .or_else(|| worker(ctx, op))
            .or_else(|| connection(ctx, op, arg))
            .or_else(|| pipe(ctx, op))
            .unwrap_or_else(|| std::process::abort())
    }
}

/// The factory's work on the message the server's bus just read from the pipe.
fn pipe(ctx: &Context<'_>, op: u32) -> Option<u64> {
    use crate::base_factory::SendData;
    /// `SW_SERVER_EVENT_RECV_DATA`, `Server::CLOSE_ACTIVELY`.
    const EVENT_RECV_DATA: u8 = 0;
    const CLOSE_ACTIVELY: c_int = 1 << 2;
    let (state, bus) = (ctx.state(), ctx.bus());
    let info = bus.head();
    let mut send = match op {
        OP_FINISH_SEND_FILE => SendData {
            info,
            // The bus's `PipeBuffer`: the message's head, then its data.
            data: bus.buffer().wrapping_add(1).cast(),
        },
        OP_FINISH_SEND_DATA => {
            let packet = bus.packet();
            let mut info = info;
            info.kind = EVENT_RECV_DATA;
            info.len = u32::try_from(packet.length).unwrap_or(u32::MAX);
            SendData {
                info,
                data: packet.data,
            }
        }
        OP_END_ACTIVELY => {
            crate::factory::end(state, info.fd, CLOSE_ACTIVELY);
            return Some(0);
        }
        _ => return None,
    };
    crate::factory::finish(state, (&raw mut send).cast());
    Some(0)
}

/// MSHUTDOWN after `exit()` or a fatal error in a single base-mode worker's callback:
/// the bailout left its loop before `start_reactor_processes` destroyed the event
/// worker pool and `Server::start` the server. That worker is the process that made
/// both, with no manager or other worker to share them, so this frees the pool and
/// what `destroy` would, without the shutdown callbacks upstream never runs on that
/// path. True when `server` is that server, which its owner then deletes; any other
/// server is left alone.
#[must_use]
pub fn release_abandoned(server: view::Server<'_>) -> bool {
    if server.raw() != swoole_rs_corebase::swr_v1_core_server()
        || !server.ran_its_single_worker_here()
    {
        return false;
    }
    // The live event pool only this process uses; destroy frees each part once.
    pool_destroy(server.event_pool_ref());
    crate::master_ops::release_abandoned_base(server);
    true
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
fn server(ctx: &Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    let config = ctx.config();
    let port = || {
        usize::try_from(arg)
            .ok()
            .and_then(|i| ctx.ports().get(i).copied())
    };
    let stream = |port: crate::view::Port<'_>| is_stream(port.config().r#type);
    Some(match op {
        OP_SET_SINGLE_THREAD => {
            // SAFETY: `create_reactor_processes`, in the master before it forks or
            // starts a thread, and `config` isn't read across the write.
            unsafe { serv.config_mut() }.single_thread = true;
            0
        }
        OP_HAVE_STREAM_SOCK => u64::from(config.have_stream_sock),
        OP_PORT_COUNT => ctx.ports().len() as u64,
        OP_PORT_STREAM => u64::from(stream(port()?)),
        OP_REUSE_PORT => u64::from(reuse_port(config)),
        OP_PORT_REUSE => u64::from(reuse_port(config) && stream(port()?)),
        OP_PORT_CLOSE_SOCKET => {
            let port = port()?;
            if reuse_port(config) {
                (methods().close_socket)(port.raw());
            }
            0
        }
        OP_PORT_CREATE_SOCKET => {
            let port = port()?;
            if !reuse_port(config) {
                return Some(0);
            }
            sign((methods().create_socket)(port.raw()))
        }
        OP_PORT_LISTEN => sign((methods().listen)(port()?.raw())),
        OP_INIT_IPC_MAX_SIZE => {
            config.init_ipc_max_size();
            0
        }
        OP_SINGLE_WORKER => u64::from(ctx.server().is_single_worker()),
        OP_START_MANAGER => sign(crate::manager_ops::start_manager_process(ctx.server())),
        OP_POOL_DESTROY => {
            pool_destroy(ctx.server().event_pool_ref());
            0
        }
        OP_ALL_WORKER_NUM => {
            let workers = ctx.workers();
            u64::from(workers.event_count)
                + u64::from(workers.task_count)
                + workers.user_count as u64
        }
        OP_HAS_REACTOR => u64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        OP_STORE_LISTEN_SOCKET => {
            crate::server_methods::store_listen_socket(ctx.server());
            0
        }
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        OP_TASK_IPC_UNIXSOCK => u64::from(config.task_ipc_mode == TASK_IPC_UNIXSOCK),
        OP_TASK_POOL_WORKER_NUM => u64::from(
            crate::shared::ServerShared::of(ctx.server())
                .task_pool()
                .worker_num(),
        ),
        OP_INIT_REACTOR => {
            crate::reactor_entry::init_reactor(serv, ctx.reactor_handle());
            0
        }
        OP_SET_MASTER_PID => {
            let pid = std::process::id().cast_signed();
            ctx.counters().master_pid.store(pid, Ordering::Relaxed);
            0
        }
        OP_HAS_ON_START => u64::from(
            ctx.callback_slot(CB_START)
                .is_some_and(crate::callbacks::CallbackSlot::is_set),
        ),
        OP_ONSTART_CALLED => u64::from(ctx.counters().onstart_called.load(Ordering::Relaxed)),
        OP_ON_START => {
            ctx.counters().onstart_called.store(true, Ordering::Relaxed);
            ctx.server()
                .state()
                .callbacks
                .call_server(CB_START, ctx.server());
            0
        }
        OP_ADD_MASTER_TIMER => {
            let tick = crate::server_methods::master_timer_tick;
            // SAFETY: `tick` takes the server, which outlives the master's timer.
            let node =
                unsafe { swoole_rs_timer::swr_v1_timer_add_plain(1000, true, tick, serv.raw()) };
            config.master_timer.store(node, Ordering::Relaxed);
            u64::from(!node.is_null())
        }
        OP_HEARTBEAT_INTERVAL => u64::from(config.heartbeat_check_interval),
        OP_REACTOR_WAIT => sign(ctx.reactor_handle().wait()),
        OP_MINFD => sign(ctx.counters().min_fd()),
        OP_MAXFD => sign(ctx.counters().max_fd()),
        OP_DISABLE_NOTIFY => u64::from(config.disable_notify),
        _ => return None,
    })
}

/// The context's reactor, or the thread's.
fn reactor(ctx: &Context<'_>) -> *mut c_void {
    ctx.reactor.unwrap_or_else(Reactor::thread).ptr()
}

/// The effects on the context's worker.
fn worker(ctx: &Context<'_>, op: u32) -> Option<u64> {
    match op {
        OP_WORKER_ENTER => {
            swoole_rs_process::methods::swoole_set_worker_type(SW_EVENT_WORKER);
            swoole_rs_process::methods::swoole_set_worker_id(ctx.worker().id());
            swoole_rs_process::methods::swoole_set_worker_pid(std::process::id().cast_signed());
            crate::worker_methods::init_event_worker(ctx.server(), ctx.worker());
        }
        OP_WORKER_SIGNAL_INIT => crate::event_worker_ops::worker_signal_init(ctx.server()),
        OP_RESET_CONNECTION_NUM => {
            let id = ctx.worker().id() as usize;
            // SAFETY: the live server's counters' slot for each worker.
            let slot = unsafe { &*ctx.counters().cells().wrapping_add(id) };
            slot.store(0, Ordering::Relaxed);
        }
        OP_HAS_PIPE_WORKER => return Some(u64::from(!ctx.worker().pipe_worker().is_null())),
        OP_WORKER_ID => return Some(u64::from(ctx.worker().id())),
        OP_WORKER_START_CALLBACK => {
            crate::event_worker_ops::worker_start_callback(ctx.server(), ctx.worker());
        }
        OP_WORKER_STOP_CALLBACK => {
            crate::event_worker_ops::call_worker_stop_callback(ctx.server(), ctx.worker());
        }
        _ => return None,
    }
    Some(0)
}

/// `swoole::WorkerType::SW_EVENT_WORKER`.
const SW_EVENT_WORKER: c_int = 2;

/// The connection the context selected (`OP_CONN_AT`, `OP_EVENT_CONN_ACTIVE`) and
/// the event being closed.
fn connection(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let server = ctx.server();
    match op {
        OP_CONN_AT => {
            let fd = c_int::try_from(arg.cast_signed()).unwrap_or(-1);
            let conn = server
                .at(fd)
                .filter(|conn| crate::reactor_ops::valid(Some(*conn)));
            let valid = conn.is_some();
            ctx.conn = conn.map_or(core::ptr::null_mut(), crate::Connection::as_ptr);
            return Some(u64::from(valid));
        }
        OP_EVENT_CONN_ACTIVE => {
            let fd = ctx.event().fd();
            let conn = server.at(fd);
            let active = conn.is_some_and(crate::Connection::is_active);
            ctx.conn = conn.map_or(core::ptr::null_mut(), crate::Connection::as_ptr);
            return Some(u64::from(active));
        }
        OP_EVENT_SOCKET_REMOVED => {
            let socket = ctx.event().socket()?;
            return Some(u64::from(socket.read(|s| s.removed)));
        }
        OP_CLOSE_EVENT_SOCKET => {
            // SAFETY: the running reactor and the event's session socket.
            let closed = unsafe {
                crate::reactor_entry::close_connection(
                    reactor(ctx),
                    ctx.event().socket_ptr().cast(),
                )
            };
            return Some(sign(closed));
        }
        OP_HEARTBEAT_NOW => {
            ctx.now = swoole_rs_corebase::microtime();
            return Some(0);
        }
        _ => {}
    }
    let conn = ctx.conn()?;
    let socket = conn.socket_ptr().cast::<SocketRecord>();
    Some(match op {
        OP_CLOSE_SESSION => {
            crate::server_methods::close(server, conn.session_id(), true);
            0
        }
        OP_CONN_CLOSE_QUEUED => u64::from(conn.close_queued()),
        OP_SET_PEER_CLOSED => {
            // the client has closed the connection and the connection is no longer available.
            conn.set_peer_closed(1);
            0
        }
        OP_NOTIFY_CLOSE => u64::from(crate::server_methods::notify(server, conn, EVENT_CLOSE)),
        OP_CONN_HEALTHY => u64::from(server.healthy(ctx.now, conn)),
        OP_CONN_SSL_PENDING => {
            let socket = conn.socket()?;
            u64::from(socket.read(|s| !s.ssl().is_null() && s.tls.state != SSL_READY))
        }
        OP_CONN_CLOSE_FORCE => u64::from(conn.close_force()),
        OP_CLOSE_CONNECTION => {
            // SAFETY: the running reactor and the connection's session socket.
            unsafe { crate::reactor_entry::close_connection(reactor(ctx), socket) };
            0
        }
        OP_SET_CLOSE_FORCE => {
            conn.set_close_force(1);
            0
        }
        _ => return None,
    })
}
