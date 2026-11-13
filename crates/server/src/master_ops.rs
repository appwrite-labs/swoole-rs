//! The ops of master.cc's runner (`Server::rust_call`) that are a log line or a
//! read or write of a record Rust already lays out: the server's config, tables
//! and shared counters, and the context's connection, socket and port. Rust
//! answers them before any native call, so master.cc no longer has a case for
//! them. Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_int};
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::CCast;
use swoole_rs_process::worker_record::WorkerType;

use swoole_rs_buffer::cpp::swr_cpp_buffer_empty;
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Socket, SocketFields, SocketRecord};

use crate::callbacks::{
    CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CLOSE, CB_CONNECT, CB_RECEIVE, CB_TASK,
};
use crate::hybrid::hybrid;
use crate::ops::OP_PIPE_COMMAND_ADD;
use crate::ops::{
    LOG_ALLOC_PORT_GS, LOG_ALLOC_SESSIONS, LOG_ALLOC_TASK_RESULTS, LOG_ALLOC_WORKERS,
    LOG_BASE_ALLOC_CONNECTIONS, LOG_BASE_ALLOC_NUMS, LOG_BASE_ALLOC_PORT_NUMS,
    LOG_CALLBACK_DISPATCH, LOG_CALLBACK_REQUIRED, LOG_LC_ONLY_ONE, LOG_MAX_CONNECTION_MINIMUM,
    LOG_REACTOR_NUM, LOG_TASK_POOL, LOG_TASK_WORKER_NUM, LOG_WORKER_NUM, OP_BASE_REACTORS,
    OP_CLEAR_MAX_QUEUED, OP_CLEAR_SEND_YIELD, OP_CONN_CLOSING, OP_HAS_PIPE_COMMAND, OP_IS_CREATED,
    OP_PORT_IDLE_TIME, OP_SEND_TIMEOUT, OP_SEND_TIMEOUT_MIN, OP_SET_DISABLE_NOTIFY,
    OP_SET_MASTER_PID, OP_SIGNAL_FD, OP_SIGRTMIN, OP_STATE,
};
use crate::ops::{
    OP_AFTER_CREATE, OP_AFTER_SHUTDOWN, OP_ALLOC_CONNECTION_NUMS, OP_ALLOC_PORT_CONNECTION_NUMS,
    OP_ALLOC_PORT_GS, OP_ALLOC_SESSIONS, OP_ALLOC_TASK_RESULTS, OP_ALLOC_WORKERS,
    OP_CREATE_FACTORY, OP_DESTROY_FACTORY, OP_DESTROY_WORKER, OP_FREE_USER_WORKERS,
    OP_HTTP_DEFAULTS, OP_INIT_LOCAL_ADDRS, OP_LC_BEFORE_START, OP_LC_CHECK, OP_LC_PREPARE_WORKERS,
    OP_LC_START_THREAD, OP_MASTER_REACTOR, OP_MASTER_WORKER,
};
use crate::ops::{OP_ALLOC_CONNECTIONS, OP_PORT_MAX_LENGTH, OP_PORT_MIN_LENGTH};
use crate::ops::{OP_MASTER_THREAD, OP_OUT_EMPTY};
use crate::port_config::PortConfig;
#[cfg(all(feature = "upstream-symbols", not(test)))]
use crate::port_config::methods;
use crate::records::{ConnectionRecord, SessionRecord};
use crate::server_methods::NativeContext;
use crate::shared::{PortCounters, ServerCounters};
use crate::view::ServerHook;
use crate::{MODE_BASE, MODE_PROCESS, MODE_THREAD, ServerState};
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_memory::members::Shm;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_process::workers::ops::{
    RUN_CREATE_THREAD_FACTORY, RUN_DESTROY_THREAD_FACTORY, RUN_START_WORKER_THREADS,
};

const SW_LOG_WARNING: c_int = 4;
const SW_LOG_ERROR: c_int = 5;
const SW_EVENT_READ: c_int = 1 << 9;
/// `SW_FD_STREAM_SERVER`.
const SW_FD_STREAM_SERVER: c_int = 1;
/// `sizeof(Session)` and `sizeof(sw_atomic_t)`.
const SESSION_SIZE: usize = size_of::<SessionRecord>();
const ATOMIC_SIZE: usize = 4;
// The native sizes of the arrays allocated here.
const _: () = assert!(
    SESSION_SIZE == 16 && size_of::<PortCounters>() == 80,
    "server.cc sizeof(Session), sizeof(ServerPortGS)"
);
/// `SW_BUFFER_MIN_SIZE`.
const BUFFER_MIN_SIZE: u32 = 65536;

fn error(level: c_int, code: E, origin: &CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// `if (swoole_isset_hook(hook)) swoole_call_hook(hook, serv)`, over the context's server.
fn call_hook(o: &Objects<'_>, hook: ServerHook) {
    if let Some(serv) = o.server {
        serv.call_hook(hook);
    }
}

fn sign(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// The context's objects, each null when the op doesn't use it.
struct Objects<'a> {
    ctx: &'a mut NativeContext<'a>,
    /// The context's server, which the ops reading it run with.
    server: Option<crate::view::Server<'a>>,
    state: &'a ServerState,
}

impl<'a> Objects<'a> {
    /// The context's live server.
    fn server(&self) -> crate::view::Server<'a> {
        self.server
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a master op without a server"))
    }
    fn config(&self) -> &crate::ServerConfig {
        self.state.config_ref()
    }
    fn tables(&self) -> &crate::ServerTables {
        self.state.tables_ref()
    }
    fn gs(&self) -> &ServerCounters {
        let gs = self.config().gs.as_ptr().cast::<ServerCounters>();
        // SAFETY: the server's shared counters, mapped for its life.
        unsafe { &*gs }
    }
    /// The context's connection, which the ops reading it run with.
    fn conn(&self) -> crate::Connection<'a> {
        self.ctx
            .conn
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a master op without a connection"))
    }
    fn port(&self) -> &PortConfig {
        // SAFETY: the ops reading it run with a live port in the context, which no
        // op writes while it reads.
        unsafe { &*self.ctx.port.cast::<PortConfig>() }
    }
    /// `serv->ports[index]`, if there is one.
    fn port_at(&self, index: u64) -> Option<crate::view::Port<'_>> {
        self.tables()
            .ports()
            .get(usize::try_from(index).ok()?)
            .copied()
    }
    /// The context's live reactor.
    fn reactor(&self) -> crate::reactor_handle::Reactor {
        // SAFETY: the ops reading it run on the context's live reactor, on its thread.
        unsafe { crate::reactor_handle::Reactor::of(self.ctx.reactor) }
    }
    /// The live socket native code passes as `arg`.
    fn arg_socket<'s>(arg: u64) -> Option<Socket<'s>> {
        // SAFETY: the ops reading it pass a live socket.
        unsafe { Socket::from_ptr(arg as *mut SocketRecord) }
    }
}

/// Answers `op` when it is one of this module's, else `None`.
///
/// # Safety
/// `ctx` is the live context for the running operation, `server` its server (the
/// ops reading it run with one) and `state` that server's; the objects each op
/// reads are live, as for master.cc's case, and the server is at the point of its
/// life where it makes `op`: [`configure`]'s ops only while the master is the
/// server's only thread. The [`Objects`] made from them carry this proof to each
/// stage below.
#[inline(always)]
pub(crate) unsafe fn answer<'a>(
    ctx: *mut NativeContext<'a>,
    server: Option<crate::view::Server<'a>>,
    state: &'a ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: per the caller; nothing else touches the context during the op.
    let ctx = unsafe { &mut *ctx };
    let mut o = Objects { ctx, server, state };
    connection(&mut o, op, arg).or_else(|| ops(&mut o, op, arg))
}

/// The ops each connection makes as it is closed (`Server::close`,
/// `base_factory::end`), answered where they are made: inlined into a call with a
/// constant op, each folds to its arm, where [`ops`]' match and the stages it falls
/// through to cost about 40 instructions an op (#2748).
#[inline(always)]
fn connection(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_PORT_IDLE_TIME => o.port().max_idle_time.get().to_bits(),
        OP_OUT_EMPTY => {
            let socket = Objects::arg_socket(arg)?;
            let empty = |s: &SocketFields| {
                crate::reactor_ops::out_buffer(s).is_none_or(|b| swr_cpp_buffer_empty(b))
            };
            u64::from(socket.read(empty))
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_ON_CLOSE => {
            // SAFETY: the close event `base_factory::end` lends at `arg` for the op.
            let event = unsafe { &*(arg as *const swoole_rs_corebase::DataHead) };
            // SAFETY: the context's live server (`answer`'s contract), read as it was
            // before the slot took a handle: `o.server` would test for one.
            let serv = unsafe { crate::view::Server::from_raw(o.ctx.serv) };
            let callbacks = &o.state.callbacks;
            callbacks.event(crate::callbacks::EventSlot::Close, serv, event);
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
            // A live session socket of this thread's reactor.
            let socket = Objects::arg_socket(arg)?;
            socket.read(|s| {
                crate::reactor_ops::out_buffer(s)
                    .map(|b| swoole_rs_buffer::cpp::swr_cpp_buffer_typed_close(b))
            });
            // Only the half-close drain stops reading (`SW_HALFCLOSE_DRAIN`); upstream reads on.
            u64::from(
                !cfg!(feature = "halfclose")
                    || crate::reactor_handle::Reactor::thread().interest(socket, 4, 0) == 0,
            )
        }
        _ => return None,
    })
}

/// The ops that write the configuration and tables, each of which runs only while
/// the master is the server's only thread: `Server::create` (`OP_ALLOC_SESSIONS`,
/// `OP_ALLOC_PORT_GS`, `OP_HTTP_DEFAULTS`, `OP_ALLOC_WORKERS`, `OP_CREATE_FACTORY`,
/// `OP_BASE_REACTORS`, `OP_ALLOC_CONNECTIONS`, `OP_ALLOC_CONNECTION_NUMS`,
/// `OP_ALLOC_PORT_CONNECTION_NUMS`, `OP_ALLOC_TASK_RESULTS`, `OP_TASK_PIPE`), `add_port` before it and `add_command` before the start (`OP_DGRAM_PORT`,
/// `OP_STREAM_PORT`), `start_check` (`OP_SET_DISABLE_NOTIFY`,
/// `OP_CLEAR_SEND_YIELD`, `OP_CLEAR_MAX_QUEUED`, `OP_SEND_TIMEOUT_MIN`), and `destroy`
/// once the factory has joined its threads (`OP_DESTROY_FACTORY` in base mode, `OP_FREE_USER_WORKERS`,
/// `OP_DESTROY_TASK_WORKERS`, `OP_FREE_SERVER`); else `None`.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per op, as master.cc's switch was"
)]
fn configure(o: &Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    // SAFETY: no other thread runs (above), and each arm's write through it is its
    // last config access, so no other reference to the config is used meanwhile.
    let config = || unsafe { &mut *o.state.config() };
    // SAFETY: as `config`, for the tables.
    let tables = || unsafe { &mut *o.state.tables() };
    let gs = || o.config().gs.as_ptr().cast::<ServerCounters>();
    // SAFETY: as `config`, for the counters `swr_v1_server_new` mapped; and before
    // the server forks or after its workers are reaped, no process shares them.
    let counters = || unsafe { &mut *gs() };
    let port_gs = |index| Some(o.port_at(index)?.config().gs.get().cast::<PortCounters>());
    // SAFETY: as `counters`, for a port's slot in `port_gs_list`: null, or mapped.
    let port_counters = |index| unsafe { port_gs(index)?.as_mut() };
    let mode = || u64::from(o.config().mode_);
    Some(match op {
        OP_SET_DISABLE_NOTIFY => {
            config().disable_notify = true;
            0
        }
        OP_CLEAR_SEND_YIELD => {
            o.config().send_yield.set(false);
            0
        }
        OP_CLEAR_MAX_QUEUED => {
            o.config().max_queued_bytes.set(0);
            0
        }
        OP_SEND_TIMEOUT_MIN => {
            o.config().send_timeout.set(0.001);
            0
        }
        OP_BASE_REACTORS => {
            let reactors = o.config().worker_num.get().c_cast::<u16>();
            o.config().reactor_num.set(reactors);
            0
        }
        OP_HTTP_DEFAULTS => {
            // `locations` (0) and `http_compression_types` (2): count, enabled, enable, reset.
            let strings = |kind, op| crate::static_config::config(o.state, kind, op, &[]);
            if o.config().enable_static_handler.get() && strings(0, 1) == 0 {
                strings(0, 3);
                strings(0, 2);
            }
            if strings(2, 1) != 0 && strings(2, 0) != 0 {
                o.config().http_compression.set(true);
            }
            0
        }
        OP_ALLOC_SESSIONS => {
            let list = Shm::calloc(SW_SESSION_LIST_SIZE as usize, SESSION_SIZE);
            let ready = !list.is_null();
            tables().session_list = list;
            u64::from(ready)
        }
        OP_ALLOC_PORT_GS => {
            let count = o.tables().ports().len();
            let shm = Shm::calloc(count, size_of::<PortCounters>());
            let list = shm.as_ptr().cast::<PortCounters>();
            tables().port_gs_list = shm;
            if list.is_null() {
                return Some(0);
            }
            for index in 0..count {
                o.port_at(index as u64)?
                    .set_gs(list.wrapping_add(index).cast());
            }
            1
        }
        OP_ALLOC_CONNECTIONS => {
            // Private, as `sw_calloc`'s memory was: each base worker indexes its own fds.
            let size = size_of::<ConnectionRecord>();
            let list = Shm::calloc_private(o.tables().max_connection as usize, size);
            let ready = !list.is_null();
            tables().connection_list = list;
            u64::from(ready)
        }
        OP_ALLOC_CONNECTION_NUMS => {
            let cells = Shm::calloc(o.config().worker_num.get() as usize, ATOMIC_SIZE);
            let ready = !cells.is_null();
            counters().connection_nums = cells;
            u64::from(ready)
        }
        OP_ALLOC_WORKERS => {
            let workers = Shm::calloc(
                o.config().worker_num.get() as usize,
                size_of::<WorkerRecord>(),
            );
            let ready = !workers.is_null();
            config().workers = workers;
            u64::from(ready)
        }
        OP_ALLOC_TASK_RESULTS => {
            let size = swoole_rs_corebase::constants::SW_IPC_MAX_SIZE as usize;
            let results = Shm::calloc(o.config().worker_num.get() as usize, size);
            let ready = !results.is_null();
            config().task_results = results;
            u64::from(ready)
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        OP_CREATE_FACTORY if mode() == MODE_PROCESS => {
            use swoole_rs_process::workers::ops::RUN_CREATE_PROCESS_FACTORY;
            let made = crate::factory_ops::run_server(o.server(), RUN_CREATE_PROCESS_FACTORY);
            let factory =
                core::ptr::with_exposed_provenance_mut(usize::try_from(made).unwrap_or(0));
            config().factory_ = factory;
            u64::from(!factory.is_null())
        }
        OP_CREATE_FACTORY if mode() != MODE_PROCESS => {
            let factory = if mode() == MODE_BASE {
                crate::server_methods::create_base_factory(o.server())
            } else {
                let factory = hybrid().thread_mode(o.server(), RUN_CREATE_THREAD_FACTORY);
                core::ptr::with_exposed_provenance_mut(usize::try_from(factory).unwrap_or(0))
            };
            config().factory_ = factory;
            u64::from(!factory.is_null())
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_TASK_PIPE => {
            let ready = o.state.push_task_pipe();
            if !ready {
                config().task_results.free();
            }
            u64::from(ready)
        }
        // `Server::destroy_base_factory()`: the connection list and the connection counts.
        OP_DESTROY_FACTORY if arg == MODE_BASE => {
            tables().connection_list.free();
            counters().connection_nums.free();
            for index in 0..o.tables().ports().len() {
                if let Some(gs) = port_counters(index as u64) {
                    gs.connection_nums.free();
                }
            }
            0
        }
        OP_ALLOC_PORT_CONNECTION_NUMS => {
            let Some(gs) = port_counters(arg) else {
                return Some(0);
            };
            let cells = Shm::calloc(o.config().worker_num.get() as usize, ATOMIC_SIZE);
            let ready = !cells.is_null();
            gs.connection_nums = cells;
            u64::from(ready)
        }
        OP_FREE_USER_WORKERS => {
            config().user_workers.free();
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_DESTROY_TASK_WORKERS => {
            // `Server::destroy_task_workers()`: free the task results, then destroy the task pool.
            config().task_results.free();
            swoole_rs_process::methods::destroy(o.server().task_pool_ref());
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_FREE_SERVER => {
            let lists = tables();
            lists.session_list.free();
            lists.port_gs_list.free();
            let config = config();
            config.workers.free();
            let factory = core::mem::replace(&mut config.factory_, core::ptr::null_mut());
            if !factory.is_null() {
                // SAFETY: `delete factory_`: nothing names the factory afterwards.
                unsafe { crate::factory::delete(factory) };
            }
            swoole_rs_corebase::swr_v1_core_set_server(core::ptr::null_mut());
            0
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
    if let Some(done) = configure(o, op, arg) {
        return Some(done);
    }
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
                o.config().worker_num.get().cast_signed(),
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
            let (now, max) = (o.config().reactor_num.get(), cpus * 4);
            let m =
                format_args!("reactor_num == {now}, Too many threads, reset to max value {max}");
            log::warning(c"Server::create", m);
            0
        }
        LOG_WORKER_NUM => {
            let (now, max) = (o.config().worker_num.get().cast_signed(), cpus * 1000);
            let m =
                format_args!("worker_num == {now}, Too many processes, reset to max value {max}");
            log::warning(c"Server::create", m);
            0
        }
        LOG_TASK_WORKER_NUM => {
            let (now, max) = (o.config().task_worker_num.get().cast_signed(), cpus * 1000);
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
                o.config().worker_num.get().cast_signed(),
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
            let mode = o.config().dispatch_mode.get();
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
        LOG_BASE_ALLOC_CONNECTIONS => {
            let size = o.tables().max_connection as usize * size_of::<ConnectionRecord>();
            log::sys_warning(
                c"Server::create_base_factory",
                format_args!("calloc[2]({size}) failed"),
            );
            0
        }
        LOG_BASE_ALLOC_NUMS | LOG_BASE_ALLOC_PORT_NUMS => {
            let size = o.config().worker_num.get() as usize * ATOMIC_SIZE;
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
        OP_SEND_TIMEOUT => o.config().send_timeout.get().to_bits(),
        OP_HAS_PIPE_COMMAND => u64::from(!o.config().pipe_command.is_null()),
        OP_SET_MASTER_PID => {
            o.gs()
                .master_pid
                .store(std::process::id().cast_signed(), Relaxed);
            0
        }

        OP_CONN_CLOSING => {
            u64::from(o.conn().closed().load(Relaxed) != 0 || o.conn().close_force())
        }

        // The server's state and the worker records.
        OP_STATE => core::ptr::from_ref(state).expose_provenance() as u64,
        OP_IS_CREATED => u64::from(!o.config().factory_.is_null()),
        // Lifecycle effects over process-wide state Rust owns: hooks, the
        // worker globals, the master reactor and the server's shared allocations.
        OP_LC_BEFORE_START => {
            call_hook(o, ServerHook::BeforeStart);
            0
        }
        OP_AFTER_CREATE => {
            call_hook(o, ServerHook::AfterCreate);
            0
        }
        OP_AFTER_SHUTDOWN => {
            call_hook(o, ServerHook::AfterShutdown);
            0
        }
        OP_MASTER_WORKER => {
            swoole_rs_process::methods::set_worker_type(WorkerType::Master);
            if arg == MODE_THREAD {
                let tid = swoole_rs_ossupport::swr_v1_os_thread_native_id().c_cast::<c_int>();
                swoole_rs_process::methods::swoole_set_worker_pid(tid);
            } else if arg == MODE_PROCESS {
                swoole_rs_process::methods::swoole_set_worker_pid(std::process::id().cast_signed());
            }
            0
        }
        OP_MASTER_REACTOR => {
            let reactor = o.reactor();
            reactor.handle().set_object(o.ctx.serv);
            let accept: swoole_rs_reactor::Handler = crate::server_methods::accept_connection;
            reactor.set_handler(SW_FD_STREAM_SERVER, SW_EVENT_READ, accept);
            0
        }
        OP_INIT_LOCAL_ADDRS => u64::from(crate::local_addr::init(o.state)),
        op => return forwarded(o, op, arg),
    })
}

/// The ops over the server's tables and ports that master.cc made, and the
/// `Server` methods crates/server defines that it forwarded to; then [`lifecycle`].
fn forwarded(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_PORT_MAX_LENGTH | OP_PORT_MIN_LENGTH => {
            let port = o.port_at(arg)?;
            if op == OP_PORT_MIN_LENGTH {
                port.set_package_max_length(BUFFER_MIN_SIZE);
            }
            u64::from(port.config().protocol.get().package_max_length)
        }
        op => return lifecycle(o, op, arg),
    })
}

/// The ops that call a `Server` method crates/server or crates/process defines,
/// or else `None`. `ProcessPool`'s methods and the process factory are
/// [`factory_ops`]', which only the hybrid builds: their runners link the C++
/// event loop and timer, and the server's fault fixture and unit tests link no C++.
fn lifecycle(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_LC_CHECK => sign(crate::server_methods::start_check(o.server())),
        OP_LC_PREPARE_WORKERS => {
            crate::worker_methods::workers_start(o.server());
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_LC_START_BASE => sign(crate::reactor_process_ops::start_reactor_processes(
            o.server(),
        )),
        OP_LC_START_THREAD => {
            let started = hybrid().thread_mode(o.server(), RUN_START_WORKER_THREADS);
            sign(started.c_cast::<c_int>())
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_INIT_TASK_WORKERS => {
            u64::from(crate::task_worker_ops::init_task_workers(o.server()))
        }
        OP_DESTROY_WORKER => {
            // `get_worker(i)` of an event worker, which shutdown indexes.
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            if let Some(worker) = o.server().worker_record(id) {
                crate::worker_methods::destroy_worker(worker);
            }
            0
        }
        OP_DESTROY_FACTORY if arg == MODE_THREAD => {
            hybrid().thread_mode(o.server(), RUN_DESTROY_THREAD_FACTORY);
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
    Some(match op {
        OP_MASTER_THREAD => {
            swoole_rs_corebase::set_thread(THREAD_MASTER, arg.c_cast::<u16>());
            0
        }
        op => return sockets(o, op, arg),
    })
}

/// `DgramPacket` up to its address.
#[repr(C)]
pub(crate) struct DgramPacket {
    socket_type: c_int,
    pub(crate) socket_addr: SocketAddress,
}

/// The ops over the context's reactor, sockets and pipes, the server's
/// workers and the dispatch address, which Rust lays out; else `None`.
fn sockets(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        // `ctx->reactor->add` of the command pipe's master end.
        OP_PIPE_COMMAND_ADD => {
            if let Some(master) = o
                .config()
                .command_pipe()
                .and_then(swoole_rs_tls::socket_pair::SocketPair::master_end)
            {
                o.reactor().change(master, 0, SW_EVENT_READ);
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
        OP_SIGNAL_HANDLE, OP_SIGNAL_IGNORE, OP_TIMER_DETACHED, OP_TIMER_REINIT,
    };
    /// `SW_FD_PIPE`.
    const SW_FD_PIPE: c_int = 3;
    use swoole_rs_error::swoole_clear_last_error_msg;
    use swoole_rs_reactor::event_api::swoole_event_wait;
    use swoole_rs_reactor::signal_upstream::{self, Handler};
    use swoole_rs_reactor::swoole_event_free;
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
        OP_TIMER_DETACHED => u64::from(crate::objects::timer_detached()),
        OP_TIMER_REINIT => {
            crate::objects::timer_reinit(false);
            0
        }
        OP_SIGNAL_IGNORE | OP_SIGNAL_HANDLE => {
            let handler = if op == OP_SIGNAL_HANDLE {
                let handler = crate::server_methods::master_signal_handler as *const () as usize;
                // SAFETY: the master's handler, which runs while `SwooleG.server` is live.
                unsafe { Handler::from_raw(handler) }
            } else {
                Handler::NONE
            };
            let signo = arg.c_cast::<c_int>();
            _ = signal_upstream::set(signo, handler, false);
            0
        }
        OP_MARK_SHUTDOWN => {
            signal_upstream::clear();
            o.gs().start.store(0, core::sync::atomic::Ordering::SeqCst);
            o.gs()
                .shutdown
                .store(1, core::sync::atomic::Ordering::SeqCst);
            0
        }
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
        OP_LC_DAEMON, OP_LC_FACTORY_START, OP_LC_INIT_PROTOCOLS, OP_LC_START_PROCESS,
        OP_MASTER_START, OP_MASTER_TIMER, OP_MAX_SOCKETS, OP_STORE_LISTEN,
    };
    use swoole_rs_clients::address::assign;
    use swoole_rs_corebase::swoole_redirect_stdout_file;
    use swoole_rs_ossupport::swoole_daemon;
    use swoole_rs_utilities::{process_logger, swr_v1_logger_is_opened, swr_v1_logger_redirect};
    Some(match op {
        OP_LC_DAEMON => {
            match process_logger() {
                Some(logger) if swr_v1_logger_is_opened(logger) => {
                    swr_v1_logger_redirect(logger, true);
                }
                _ => swoole_redirect_stdout_file(c"/dev/null"),
            }
            // The master, before it starts any thread.
            sign(swoole_daemon(0, 1))
        }
        OP_LC_FACTORY_START => u64::from(crate::factory::start(o.server())),
        // In the master, after the process factory forks and before any reactor
        // thread, base-mode worker or worker thread starts.
        OP_LC_INIT_PROTOCOLS => {
            for port in o.tables().ports() {
                (crate::port_config::methods().init_protocol)(port.config());
            }
            0
        }
        // The master starts its reactor threads.
        OP_LC_START_PROCESS => sign(crate::thread_ops::start_reactor_threads(o.server())),
        OP_MAX_SOCKETS => u64::from(swoole_rs_corebase::max_sockets()),
        OP_MASTER_TIMER => {
            let node = crate::server_methods::master_timer(o.server());
            o.config().master_timer.store(node, Relaxed);
            u64::from(!node.is_null())
        }
        OP_MASTER_START => {
            o.state.callbacks.call_server(CB_START, o.server());
            0
        }
        OP_STORE_LISTEN => {
            let index = usize::try_from(arg).unwrap_or(usize::MAX);
            let &listen = o.tables().ports().get(index)?;
            let (ls, port) = (listen.raw(), listen.config());
            let socket = o.server().port_socket_at(index)?;
            let fd = socket.fd();
            let conn = o.tables().connection_at(fd)?;
            conn.set_fd(fd);
            conn.set_socket(port.socket_ptr().cast());
            conn.set_socket_type(port.r#type);
            conn.set_object(ls);
            let mut info = conn.info();
            info.kind = port.r#type;
            let error = assign(
                &mut info.addr,
                &mut info.len,
                port.r#type,
                port.host.bytes_with_nul(),
                port.port.get(),
                true,
            );
            conn.set_info(info);
            if error != 0 {
                log::set_last_error(error);
            }
            socket.edit(|socket| socket.object = ls);
            sign(fd)
        }
        op => return closing(o, op, arg),
    })
}

/// The ops over a connection's close and the server's shutdown, which
/// call crates/server's, crates/process's and crates/ports' methods under their
/// upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn closing(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::callbacks::CB_SHUTDOWN;
    use crate::ops::{OP_JOIN_REACTOR_THREADS, OP_NOTIFY_CONNECT, OP_ON_SHUTDOWN, OP_PORT_CLOSE};
    /// `SW_SERVER_EVENT_CONNECT`.
    const EVENT_CONNECT: c_int = 5;
    Some(match op {
        OP_NOTIFY_CONNECT => u64::from(crate::server_methods::notify(
            o.server(),
            o.conn(),
            EVENT_CONNECT,
        )),
        OP_ON_SHUTDOWN => {
            o.state.callbacks.call_server(CB_SHUTDOWN, o.server());
            0
        }
        OP_JOIN_REACTOR_THREADS => {
            crate::reactor_thread::join_reactor_threads(o.server());
            0
        }
        OP_PORT_CLOSE => {
            let port = o.tables().ports().get(usize::try_from(arg).ok()?)?;
            (methods().close)(*port);
            0
        }
        op => return sends(o, op, arg),
    })
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
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn base_pass(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) {
    let bus = worker_message_bus(serv.state());
    bus.rust().pass(&send.info, send.data);
    let mut head = bus.head();
    crate::event_worker_ops::accept_event(serv, &mut head);
}

/// The send ops that call a `Server` or `Socket` method crates/server,
/// crates/process or crates/tls defines; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn sends(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{OP_ACCEPT_EVENT, OP_BASE_PASS, OP_FORWARD, OP_SEND_HERE};
    use swoole_rs_corebase::DataHead;
    // The `SendData` native code lends at arg.
    let lent = || {
        // SAFETY: as above.
        unsafe { &*(arg as *const crate::base_factory::SendData) }
    };
    Some(match op {
        OP_SEND_HERE => {
            // SAFETY: the live `SendData` the send path lends.
            let send = unsafe { &*o.ctx.send.cast::<crate::base_factory::SendData>() };
            // SAFETY: its data.
            sign(unsafe { crate::server_methods::send_to_connection(o.server(), send) })
        }
        OP_ACCEPT_EVENT => {
            // SAFETY: the `DataHead` native code lends at arg.
            let info = unsafe { &mut *(arg as *mut DataHead) };
            crate::event_worker_ops::accept_event(o.server(), info);
            0
        }
        OP_BASE_PASS => {
            base_pass(o.server(), lent());
            0
        }
        OP_FORWARD => forward(o, lent()),
        op => return factory_ops(o, op, arg),
    })
}

/// `OP_FORWARD`: the worker message bus writes `send` to the pipe of the worker
/// that owns its session.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn forward(o: &Objects<'_>, send: &crate::base_factory::SendData) -> u64 {
    let Some(session) = o.state.session(send.info.fd) else {
        return 0;
    };
    let id = session.reactor_bits() & 0xff;
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
        Some(pipe)
    };
    let Some(socket) = socket else {
        return 0;
    };
    let mut info = send.info;
    // SAFETY: a live pipe socket and the send it writes.
    u64::from(unsafe {
        bus.rust()
            .send(socket.as_ptr(), &mut info, send.data.cast())
    })
}

/// The ops over the server's factory and task pool: the process factory's and
/// `ProcessPool`'s methods (crates/process), the factory's virtual members and
/// the task notify pipes; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn factory_ops(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::factory_ops as process;
    use crate::ops::{
        OP_FACTORY_SHUTDOWN, OP_NEW_BASE_FACTORY, OP_TASK_POOL_CREATE, OP_TASK_POOL_LISTEN,
        OP_TASK_POOL_SETUP,
    };
    use swoole_rs_process::methods as pool;
    use swoole_rs_process::workers::ops::RUN_DESTROY_PROCESS_FACTORY;
    // crates/process' `ProcessPool::listen(socket_file, backlog)`.
    use swoole_rs_process::methods::listen_unix as pool_listen;
    let serv = o.ctx.serv;
    let config = o.config();
    Some(match op {
        OP_DESTROY_FACTORY => {
            // The live server, whose process factory shutdown destroys.
            process::run_server(o.server(), RUN_DESTROY_PROCESS_FACTORY);
            0
        }
        OP_TASK_POOL_CREATE => {
            let task_pool = o.server().task_pool();
            // SAFETY: the server's task pool, reset to `{}` (no member has an initializer).
            unsafe { task_pool.write_bytes(0, 1) };
            let key = if arg == u64::from(swoole_rs_process::process_pool::IpcMode::MsgQueue.code())
            {
                config.message_queue_key.get().c_cast::<c_int>()
            } else {
                0
            };
            #[expect(clippy::cast_possible_truncation, reason = "an swIPCMode")]
            let created = pool::create(
                o.server().task_pool_ref(),
                config.task_worker_num.get(),
                key,
                arg as c_int,
            );
            u64::from(created >= 0)
        }
        OP_TASK_POOL_SETUP => {
            let task_pool = o.server().task_pool_ref();
            pool::set_max_request(
                task_pool,
                config.task_max_request.get(),
                config.task_max_request_grace.get(),
            );
            pool::set_start_id(task_pool, config.worker_num.get().cast_signed());
            pool::set_type(task_pool, WorkerType::Task.code().into());
            0
        }
        OP_TASK_POOL_LISTEN => {
            // SAFETY: the NUL-terminated path `arg` lends.
            let file = unsafe { CStr::from_ptr(ptr_of(arg)) };
            sign(pool_listen(o.server().task_pool_ref(), file, 2048))
        }
        OP_FACTORY_SHUTDOWN => {
            crate::factory::shutdown(o.state);
            0
        }
        OP_NEW_BASE_FACTORY => {
            o.ctx.object = crate::factory::new(serv, None);
            0
        }
        op => return objects(o, op),
    })
}

/// The ops over the server's timers, barriers and pipe buffers; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn objects(o: &mut Objects<'_>, op: u32) -> Option<u64> {
    use crate::ops::{OP_MANAGER_BARRIER, OP_REACTOR_BARRIER, OP_RELEASE_PIPE_BUFFERS};
    use crate::server_object::barrier;
    Some(match op {
        OP_REACTOR_BARRIER | OP_MANAGER_BARRIER => {
            swoole_rs_locks::barrier_wait(barrier(o.server(), op == OP_MANAGER_BARRIER));
            0
        }
        OP_RELEASE_PIPE_BUFFERS => {
            release_pipe_buffers(o.state);
            0
        }
        _ => return None,
    })
}

/// `Server::create_pipe_buffers()`: the bus's buffer, `ipc_max_size` bytes.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn create_pipe_buffers(serv: crate::view::Server<'_>) -> c_int {
    let state = serv.state();
    let size = state.config_ref().ipc_max_size();
    if state.message_bus().alloc_buffer(size as usize) {
        0
    } else {
        -1
    }
}

/// `MessageBus::free_buffer()` of the server's bus.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn release_pipe_buffers(state: &ServerState) {
    state.message_bus().free_buffer();
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
    let serv = reactor.object();
    // SAFETY: the master reactor's live server.
    let state = unsafe { crate::view::Server::from_raw(serv) }.state();
    let bus = state.message_bus();
    let Some(sock) = event.socket() else {
        return 0;
    };
    if bus.read(sock) <= 0 {
        return 0;
    }
    let request_id = bus.head().fd;
    bus.with_packet(|message| crate::commands::command_result(state, request_id, message));
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

/// What `Server::destroy` frees of a base-mode server whose single worker bailed out
/// of its loop: the base factory's connection list and counts, the server's lists and
/// its factory. It runs none of the shutdown (callbacks, hooks, signals, ports), only
/// marks the server shut down, so `swr_v1_server_delete` frees the rest.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn release_abandoned_base(serv: crate::view::Server<'_>) {
    let mut ctx = NativeContext::new(serv.raw());
    let mut o = Objects {
        ctx: &mut ctx,
        server: Some(serv),
        state: serv.state(),
    };
    o.gs()
        .shutdown
        .store(1, core::sync::atomic::Ordering::SeqCst);
    ops(&mut o, OP_DESTROY_FACTORY, MODE_BASE);
    ops(&mut o, crate::ops::OP_FREE_SERVER, 0);
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

    fn context<'a>() -> NativeContext<'a> {
        NativeContext::new(core::ptr::null_mut())
    }

    #[test]
    fn unknown_ops_and_missing_contexts_fall_through_to_native() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state and a zeroed context.
        unsafe {
            assert_eq!(
                answer(&raw mut ctx, None, &*state, crate::ops::OP_PORT_COUNT, 0),
                None
            );
            assert_eq!(
                answer(core::ptr::null_mut(), None, &*state, OP_SEND_TIMEOUT, 0),
                None
            );
            crate::state::free_state(state);
        }
    }

    #[test]
    fn config_ops_read_and_write_the_config() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state owns its config; the ops below touch only it. The test
        // reads it through the state's pointer, as the ops do, so no reference of its
        // own outlives an op.
        unsafe {
            let config = (*state).config();
            (*config).send_timeout.set(2.5);
            (*config).worker_num.set(6);
            assert_eq!(
                answer(&raw mut ctx, None, &*state, OP_SEND_TIMEOUT, 0),
                Some(2.5f64.to_bits())
            );
            answer(&raw mut ctx, None, &*state, OP_SEND_TIMEOUT_MIN, 0);
            assert_eq!((*config).send_timeout.get(), 0.001);
            answer(&raw mut ctx, None, &*state, OP_CLEAR_SEND_YIELD, 0);
            assert!(!(*config).send_yield.get());
            answer(&raw mut ctx, None, &*state, OP_BASE_REACTORS, 0);
            assert_eq!((*config).reactor_num.get(), 6);
            crate::state::free_state(state);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn allocation_ops_fill_the_config_tables_and_counters() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state; every pointer the ops write is checked and freed below.
        unsafe {
            let config = &mut *(*state).config();
            config.worker_num.set(3);
            config.gs = Shm::calloc(1, size_of::<ServerCounters>());
            let gs = config.gs.as_ptr().cast::<ServerCounters>();
            gs.write(ServerCounters::default());
            for op in [
                OP_ALLOC_WORKERS,
                OP_ALLOC_TASK_RESULTS,
                OP_ALLOC_CONNECTION_NUMS,
                OP_ALLOC_SESSIONS,
            ] {
                assert_eq!(
                    answer(&raw mut ctx, None, &*state, op, 0),
                    Some(1),
                    "op {op}"
                );
            }
            let nums = (*gs).cells();
            assert_eq!((*nums.add(2)).load(Relaxed), 0, "zeroed");
            let tables = &mut *(*state).tables();
            assert_eq!((*tables.session_slot(5).unwrap().get()).fd, 0, "zeroed");
            config.user_workers = Shm::calloc(1, 8);
            answer(&raw mut ctx, None, &*state, OP_FREE_USER_WORKERS, 0);
            assert!(config.user_workers.is_null());
            answer(&raw mut ctx, None, &*state, OP_FREE_USER_WORKERS, 0);
            config.task_results.free();
            config.workers.free();
            (*gs).connection_nums.free();
            tables.session_list.free();
            config.gs.free();
            crate::state::free_state(state);
        }
    }
}
