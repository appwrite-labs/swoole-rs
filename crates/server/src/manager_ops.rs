//! The manager's facts and effects (`crates/process/src/workers/manager.rs`'s
//! `OP_*`) over the server, pool and worker records Rust lays out, and the fork and
//! signal handler that run its operations: what workers.cc's `ManagerContext` did.
use core::ffi::{c_int, c_void};
use core::ptr::{null, null_mut};

use crate::factory_ops::run;
use swoole_rs_corebase::{DataHead, log, worker_registry as registry};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::methods::{self, ExitStatus, swoole_set_worker_type};
use swoole_rs_process::process_pool::{PoolRecord, PoolRef};
use swoole_rs_process::server_workers::ServerWorkers;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_process::workers::ops::{
    LOG_UNKNOWN_COMMAND, OP_ALL_WORKER_NUM, OP_ANY_WORKER_PID, OP_CHECK_EXIT_STATUS,
    OP_COMMAND_REPLY, OP_CREATE_EVENT_WORKERS, OP_CREATE_MESSAGE_BOX, OP_CREATE_USER_WORKERS,
    OP_DETACH_STOPPED, OP_DROP_RELOAD_TASK, OP_EVENT_WORKER_PID, OP_FORK_MANAGER, OP_HAS_COMMAND,
    OP_HAS_MESSAGE_BOX, OP_HAS_ON_WORKER_MESSAGE, OP_HAS_USER, OP_INFO_TYPE, OP_IS_MANAGER,
    OP_KILL_EVENT_WORKERS, OP_KILL_TASK_WORKERS, OP_KILL_USER_WORKERS, OP_MANAGER,
    OP_MANAGER_BARRIER, OP_MANAGER_INIT, OP_MANAGER_PID, OP_MAX_WAIT_TIME, OP_MESSAGE_TYPE,
    OP_MODE, OP_ON_AFTER_RELOAD, OP_ON_BEFORE_RELOAD, OP_ON_MANAGER_START, OP_ON_MANAGER_STOP,
    OP_ON_WORKER_MESSAGE, OP_POOL_RELOAD, OP_POP_MESSAGE, OP_READ_MESSAGE, OP_REAP_DETACHED,
    OP_REAP_WAIT, OP_RELOAD_ADD_EVENT_WORKERS, OP_RELOAD_ADD_TASK_WORKERS, OP_RELOAD_ASYNC,
    OP_RELOAD_INIT, OP_RELOAD_KILL_ALL, OP_RELOAD_TASK, OP_RELOAD_THREADS, OP_RELOAD_WARN,
    OP_REOPEN_LOGGER, OP_RESPAWN_EVENT_WORKER, OP_RESPAWN_TASK_WORKER, OP_RESPAWN_USER_WORKER,
    OP_RESTART_WORKER, OP_RUNNING, OP_SERVER_RELOAD, OP_SET_ISOLATION, OP_SET_MANAGER_PID,
    OP_SET_MANAGER_TYPE, OP_SET_READ_MESSAGE, OP_SET_RELOAD_INIT, OP_SET_RUNNING,
    OP_SIGNAL_DISPATCH, OP_SIGNAL_SET, OP_SPAWN_EVENT_WORKER, OP_SPAWN_TASK_WORKER,
    OP_SPAWN_USER_WORKER, OP_START_TASK_POOL, OP_STOP_MESSAGE_WORKER, OP_TASK_POOL_WORKER_NUM,
    OP_TASK_WORKER_NUM, OP_TASK_WORKER_PID, OP_UNREGISTER_PID, OP_USER_WORKER_NUM, OP_WAIT_ERROR,
    OP_WAIT_STATUS, OP_WORKER_BY_PID, OP_WORKER_NUM, OP_WORKER_REGISTRY, OP_WORKER_TYPE,
};
use swoole_rs_process::workers::ops::{
    RUN_CHECK_EXIT_STATUS, RUN_KILL_TASK_WORKERS, RUN_MANAGER_MAIN, RUN_MANAGER_SIGNAL,
    RUN_READ_WORKER_MESSAGE, RUN_RELOAD, RUN_RESTART_WORKER, RUN_SPAWN_EVENT_WORKER,
    RUN_SPAWN_USER_WORKER, RUN_START_MANAGER,
};

use crate::callbacks::{CB_AFTER_RELOAD, CB_BEFORE_RELOAD, CB_MANAGER_START, CB_MANAGER_STOP};
use crate::commands::CommandHandler;
use crate::{view, worker_methods};

/// `swoole::EventData`: `SW_IPC_MAX_SIZE` bytes, aligned for its head.
#[repr(C, align(8))]
struct EventData {
    info: DataHead,
    data: [u8; swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE],
}

/// `WorkerStopMessage`.
#[repr(C)]
#[derive(Clone, Copy)]
struct WorkerStopMessage {
    pid: libc::pid_t,
    worker_id: u16,
}

/// What an operation runs over.
struct Context<'a> {
    /// `None` for a signal that arrives before the server exists.
    serv: Option<view::Server<'a>>,
    /// The worker being restarted, or the message being read.
    worker: *mut WorkerRecord,
    task: *const EventData,
    handler: Option<CommandHandler>,
    wait_status: c_int,
    wait_error: c_int,
    msg: EventData,
    stop: WorkerStopMessage,
}

impl<'a> Context<'a> {
    /// The context's live server, which every op but `OP_MANAGER` runs with.
    fn server(&self) -> view::Server<'a> {
        self.serv.unwrap_or_else(|| std::process::abort())
    }
}

const _: () = assert!(
    size_of::<EventData>() == swoole_rs_corebase::constants::SW_IPC_MAX_SIZE as usize,
    "EventData layout"
);

/// The context's objects, each live when the op reading it runs.
#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written by the manager that owns them"
)]
impl<'a> Context<'a> {
    /// The context over `serv`.
    fn of(serv: Option<view::Server<'a>>) -> Self {
        Self {
            serv,
            worker: null_mut(),
            task: null(),
            handler: None,
            wait_status: 0,
            wait_error: 0,
            msg: EventData {
                info: DataHead::default(),
                data: [0; swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE],
            },
            stop: WorkerStopMessage {
                pid: 0,
                worker_id: 0,
            },
        }
    }
    fn state(&self) -> &crate::ServerState {
        self.server().state()
    }
    fn config(&self) -> &mut crate::ServerConfig {
        // SAFETY: the context's live server.
        unsafe { self.server().config_mut() }
    }
    fn counters(&self) -> &mut crate::ServerCounters {
        // SAFETY: the server's shared counters, which only the manager writes meanwhile.
        unsafe { self.server().counters_mut() }
    }
    fn workers(&self) -> ServerWorkers {
        self.server().workers()
    }
    /// The server's event worker pool.
    fn pool(&self) -> &mut PoolRecord {
        // SAFETY: the server's live event pool, which only the manager writes meanwhile.
        unsafe { &mut *self.workers().event_pool }
    }
    fn task_pool(&self) -> &mut PoolRecord {
        // SAFETY: the server's live task pool, which only the manager writes meanwhile.
        unsafe { &mut *self.workers().task_pool }
    }
    /// `serv->get_worker(id)`.
    fn worker_at(&self, id: u64) -> *mut WorkerRecord {
        let id = u16::try_from(id).unwrap_or(u16::MAX);
        self.server().worker(id)
    }
    /// The worker `OP_WORKER_BY_PID` found.
    fn worker(&self) -> &WorkerRecord {
        // SAFETY: the ops reading it run after it is found.
        unsafe { &*self.worker }
    }
    /// The command message's live head.
    fn task(&self) -> &EventData {
        // SAFETY: the ops reading it run with the message being read.
        unsafe { &*self.task }
    }
    /// The server's worker registry.
    fn registry(&self) -> &mut registry::WorkerRegistry {
        // SAFETY: the server's live registry, which only the manager writes.
        unsafe { self.server().registry_mut() }
    }
    /// The event pool's reload task, for the ops that run while the manager reloads.
    fn reload_task(&self) -> &mut swoole_rs_process::reload_task::ReloadTask {
        // SAFETY: the reload task the manager started.
        unsafe { &mut *self.pool().reload_task() }
    }
}

use swoole_rs_reactor::wait_upstream::swoole_waitpid;

/// `SWR_WORKER_EVENT`, `SWR_WORKER_USER`: the registry's categories.
const REGISTRY_EVENT: c_int = 1;
const REGISTRY_USER: c_int = 2;

/// `Factory::kill_event_workers()` (`origin` `kill_event_workers`) and
/// `kill_user_workers()`: SIGTERM each worker of `category`, then wait for each.
fn kill_workers(ctx: &Context<'_>, category: c_int, origin: &core::ffi::CStr) {
    let registry = ctx.registry();
    let kill = swoole_rs_process::os::kill;
    let wait = |pid| swoole_waitpid(pid, None, 0);
    let warn = |pid| log::sys_warning(origin, format_args!("waitpid({pid}) failed"));
    registry.terminate(category, libc::SIGTERM, kill, wait, warn);
}

/// `SW_MESSAGE_BOX_SIZE`, `swoole::WorkerType::SW_MANAGER`, `SW_LOG_ERROR`.
const MESSAGE_BOX_SIZE: usize = 65536;
const SW_MANAGER: c_int = 3;
const SW_LOG_ERROR: c_int = 5;

/// Answers each manager op over the context.
impl swoole_rs_process::workers::Answer for Context<'_> {
    fn answer(&mut self, op: u32, arg: u64) -> u64 {
        let ctx = self;
        if op == OP_MANAGER {
            // `serv ? serv->manager_ : 0`: a signal may arrive before the server exists.
            return ctx.serv.map_or(0, |serv| serv.config().manager_ as u64);
        }
        fact(ctx, op, arg)
            .or_else(|| effect(ctx, op, arg))
            .or_else(|| child(ctx, op, arg))
            .or_else(|| native(ctx, op, arg))
            .unwrap_or_else(|| std::process::abort())
    }
}

fn signed(value: c_int) -> u64 {
    i64::from(value).cast_unsigned()
}

fn index(arg: u64) -> usize {
    usize::try_from(arg).unwrap_or(usize::MAX)
}

/// A pid in the low word of `arg`.
fn pid_of(arg: u64) -> c_int {
    #[expect(clippy::cast_possible_truncation, reason = "the pid is the low word")]
    (arg as u32).cast_signed()
}

fn exit_status(exited: u64) -> ExitStatus {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the pid is the low word and the status the high word"
    )]
    ExitStatus::new(
        (exited as u32).cast_signed(),
        ((exited >> 32) as u32).cast_signed(),
    )
}

/// The facts read from the server, its pools and the context.
fn fact(ctx: &Context<'_>, op: u32, arg: u64) -> Option<u64> {
    // `running` is shared state, read and set before any `&mut ServerConfig` exists.
    if op == OP_RUNNING {
        return Some(u64::from(ctx.state().running()));
    }
    let config = ctx.config();
    let workers = ctx.workers();
    let (pool, task_pool) = (ctx.pool(), ctx.task_pool());
    let pid = |worker: *mut WorkerRecord| {
        // SAFETY: one of the server's live workers.
        signed(unsafe { &*worker }.pid())
    };
    Some(match op {
        OP_HAS_MESSAGE_BOX => u64::from(!pool.message_box().is_null()),
        OP_USER_WORKER_NUM => workers.user_count as u64,
        OP_MODE => u64::from(config.mode_),
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        OP_WORKER_NUM => u64::from(config.worker_num),
        OP_RELOAD_ASYNC => u64::from(config.reload_async),
        OP_MAX_WAIT_TIME => u64::from(config.max_wait_time),
        OP_EVENT_WORKER_PID => pid(workers.event_workers.wrapping_add(index(arg))),
        OP_TASK_POOL_WORKER_NUM => u64::from(task_pool.worker_num()),
        OP_TASK_WORKER_PID => signed(
            task_pool
                .worker(index(arg))
                .map_or(0, swoole_rs_process::worker_record::Worker::pid),
        ),
        OP_WORKER_REGISTRY => config.worker_registry as u64,
        OP_RELOAD_INIT => u64::from(pool.reload_init),
        OP_RELOAD_TASK => pool.reload_task() as u64,
        OP_WAIT_STATUS => signed(ctx.wait_status),
        OP_WAIT_ERROR => signed(ctx.wait_error),
        OP_READ_MESSAGE => u64::from(pool.read_message),
        OP_HAS_ON_WORKER_MESSAGE => u64::from(pool.on_worker_message.is_some()),
        OP_RELOAD_WARN => methods::reload_warn as *const () as u64,
        OP_IS_MANAGER => {
            let pid = std::process::id().cast_signed();
            u64::from(pid == ctx.counters().manager_pid)
        }
        OP_MANAGER_PID => signed(ctx.counters().manager_pid),
        OP_ALL_WORKER_NUM => {
            u64::from(config.worker_num)
                + u64::from(config.task_worker_num)
                + workers.user_count as u64
        }
        OP_ANY_WORKER_PID => pid(ctx.worker_at(arg)),
        OP_WORKER_TYPE => u64::from(ctx.worker().kind()),
        OP_INFO_TYPE => u64::from(ctx.task().info.kind),
        _ => return None,
    })
}

/// The effects on the server, its pools and the process.
fn effect(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    // `running` is shared state, read and set before any `&mut ServerConfig` exists.
    if op == OP_SET_RUNNING {
        ctx.state().set_running(arg != 0);
        return Some(0);
    }
    let serv = ctx.server();
    let config = ctx.config();
    let callback = |index| ctx.state().callbacks.call_server(index, ctx.server());
    let registry = ctx.registry();
    match op {
        OP_CREATE_EVENT_WORKERS => {
            let created = worker_methods::create_event_workers(ctx.server());
            return Some(u64::from(created));
        }
        OP_CREATE_MESSAGE_BOX => {
            return Some(signed(methods::create_message_box(
                ctx.server().event_pool_ref(),
                MESSAGE_BOX_SIZE,
            )));
        }
        OP_CREATE_USER_WORKERS => {
            let created = worker_methods::create_user_workers(ctx.server(), config);
            return Some(u64::from(created));
        }
        OP_SET_MANAGER_PID => ctx.counters().manager_pid = std::process::id().cast_signed(),
        OP_START_TASK_POOL => return Some(signed(methods::start(ctx.server().task_pool_ref()))),
        OP_SET_MANAGER_TYPE => swoole_set_worker_type(SW_MANAGER),
        OP_MANAGER_INIT => {
            config.manager_ = core::ptr::without_provenance_mut(index(arg));
            let pid = std::process::id().cast_signed();
            ctx.counters().manager_pid = pid;
            let event_pool = ctx.pool();
            event_pool.on_worker_message = Some(read_worker_message);
            event_pool.max_wait_time = config.max_wait_time;
            event_pool.master_pid = pid;
            ctx.task_pool().master_pid = pid;
        }
        OP_ON_MANAGER_START => callback(CB_MANAGER_START),
        OP_ON_MANAGER_STOP => callback(CB_MANAGER_STOP),
        OP_ON_BEFORE_RELOAD => callback(CB_BEFORE_RELOAD),
        OP_ON_AFTER_RELOAD => callback(CB_AFTER_RELOAD),
        OP_DROP_RELOAD_TASK => methods::drop_reload_task(ctx.server().event_pool_ref()),
        OP_KILL_TASK_WORKERS => _ = crate::factory_ops::run_server(serv, RUN_KILL_TASK_WORKERS),
        OP_SET_RELOAD_INIT => ctx.pool().reload_init = arg != 0,
        OP_SET_READ_MESSAGE => ctx.pool().read_message = arg != 0,
        OP_REAP_WAIT => {
            let waited = swoole_rs_process::reaper::wait_child(arg != 0);
            ctx.wait_status = waited.status;
            ctx.wait_error = waited.error;
            return Some(signed(waited.pid));
        }
        OP_SIGNAL_DISPATCH => swoole_rs_process::signal::swoole_signal_dispatch(),
        OP_RELOAD_ADD_EVENT_WORKERS | OP_RELOAD_ADD_TASK_WORKERS => {
            let (workers, count) = if op == OP_RELOAD_ADD_EVENT_WORKERS {
                (config.workers.as_ptr().cast(), config.worker_num)
            } else {
                (ctx.task_pool().workers(), config.task_worker_num)
            };
            // SAFETY: the server's `count` worker records.
            let workers: &[WorkerRecord] =
                unsafe { core::slice::from_raw_parts(workers, count as usize) };
            ctx.reload_task().add_workers(workers);
        }
        OP_RELOAD_KILL_ALL => {
            ctx.reload_task()
                .kill_all(libc::SIGTERM, methods::reload_warn);
        }
        OP_RESTART_WORKER => {
            _ = restart_worker_process(ctx.server(), &exit_status(arg));
        }
        OP_SERVER_RELOAD => _ = reload(ctx.server(), arg != 0),
        OP_REOPEN_LOGGER => {
            if let Some(logger) = swoole_rs_utilities::process_logger() {
                logger.reopen();
            }
        }
        OP_REAP_DETACHED => {
            return Some(u64::from(registry.reap_detached(pid_of(arg))));
        }
        OP_UNREGISTER_PID => {
            registry.unregister(pid_of(arg));
        }
        OP_HAS_COMMAND => {
            let id = i32::from(ctx.task().info.server_fd);
            let handler = crate::commands::handler(ctx.state(), id);
            let found = handler.is_some();
            if found {
                ctx.handler = handler;
            }
            return Some(u64::from(found));
        }
        LOG_UNKNOWN_COMMAND => {
            let id = ctx.task().info.server_fd;
            log::error_log(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_INVALID_COMMAND as c_int,
                c"Server::read_worker_message",
                format_args!("Unknown command[command_id={id}]"),
            );
        }
        OP_RELOAD_THREADS => {
            let reloaded = crate::thread_mode_ops::reload_worker_threads(ctx.server(), arg != 0);
            return Some(u64::from(reloaded));
        }
        OP_POOL_RELOAD => return Some(u64::from(methods::reload(ctx.server().event_pool_ref()))),
        _ => return None,
    }
    Some(0)
}

/// Finding, checking and replacing the server's children.
fn child(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.server();
    // `Factory::spawn_*_worker(worker)`: the child's pid, or -1.
    let spawn = |op, worker| {
        // SAFETY: the context's live server, and one of its workers.
        let pid = unsafe { run(serv, op, worker, null_mut(), null(), 0) };
        c_int::try_from(pid).unwrap_or(-1)
    };
    let pid = match op {
        OP_SPAWN_EVENT_WORKER => spawn(RUN_SPAWN_EVENT_WORKER, ctx.worker_at(arg).cast()),
        OP_SPAWN_USER_WORKER => {
            // The manager spawns the pending workers by index, below their count.
            let &worker = ctx.registry().pending().get(index(arg))?;
            spawn(RUN_SPAWN_USER_WORKER, worker.cast())
        }
        OP_WORKER_BY_PID => {
            ctx.worker = worker_methods::get_worker_by_pid(ctx.server(), pid_of(arg));
            return Some(u64::from(!ctx.worker.is_null()));
        }
        OP_CHECK_EXIT_STATUS => {
            let status = exit_status(arg);
            let (worker, status) = (ctx.worker, &raw const status);
            // SAFETY: the context's live server, the worker found and its exit status.
            unsafe { run(serv, RUN_CHECK_EXIT_STATUS, worker, null_mut(), status, 0) };
            return Some(0);
        }
        OP_KILL_EVENT_WORKERS => {
            kill_workers(ctx, REGISTRY_EVENT, c"Factory::kill_event_workers");
            return Some(0);
        }
        OP_KILL_USER_WORKERS => {
            kill_workers(ctx, REGISTRY_USER, c"Factory::kill_user_workers");
            return Some(0);
        }
        OP_SPAWN_TASK_WORKER | OP_RESPAWN_TASK_WORKER => {
            let worker = if op == OP_SPAWN_TASK_WORKER {
                ctx.worker_at(arg)
            } else {
                ctx.worker
            };
            // SAFETY: one of the task pool's workers.
            let pid = unsafe { methods::spawn(ctx.server().task_pool_ref(), worker) };
            if op == OP_SPAWN_TASK_WORKER {
                return Some(0);
            }
            pid
        }
        OP_RESPAWN_EVENT_WORKER => spawn(RUN_SPAWN_EVENT_WORKER, ctx.worker.cast()),
        OP_RESPAWN_USER_WORKER => spawn(RUN_SPAWN_USER_WORKER, ctx.worker.cast()),
        _ => return None,
    };
    Some(signed(pid))
}

/// The ops on native objects: the fork, the signals, the barrier, the message box
/// and the command reply.
fn native(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_FORK_MANAGER => {
            let serv = ctx.serv;
            signed(swoole_rs_corebase::fork_exec(|| {
                run_manager(serv, RUN_MANAGER_MAIN, 0);
            }))
        }
        OP_SIGNAL_SET => {
            crate::event_worker_ops::swr_workers_signal_set(arg, signal_handler);
            0
        }
        OP_MANAGER_BARRIER => {
            swoole_rs_locks::barrier_wait(crate::server_object::barrier(ctx.server(), true));
            0
        }
        OP_HAS_USER => u64::from(!ctx.state().text(1).is_empty()),
        OP_SET_ISOLATION => {
            ctx.state().set_isolation();
            0
        }
        OP_POP_MESSAGE => {
            let (msg, size) = ((&raw mut ctx.msg).cast::<u8>(), size_of::<EventData>());
            // SAFETY: the context's message, which the box's message overwrites.
            let msg = unsafe { core::slice::from_raw_parts_mut(msg, size) };
            signed(methods::pop_message(ctx.pool(), msg))
        }
        OP_MESSAGE_TYPE => u64::from(ctx.msg.info.kind),
        OP_ON_WORKER_MESSAGE => {
            let msg = (&raw mut ctx.msg).cast();
            let pool = ctx.pool();
            let on_message = pool.on_worker_message?;
            // SAFETY: the message popped from the pool.
            unsafe { on_message(pool.into(), msg) };
            0
        }
        OP_STOP_MESSAGE_WORKER => {
            // A stop message's payload is a `WorkerStopMessage`.
            let data = &ctx.msg.data;
            ctx.stop = WorkerStopMessage {
                pid: i32::from_ne_bytes([data[0], data[1], data[2], data[3]]),
                worker_id: u16::from_ne_bytes([data[4], data[5]]),
            };
            u64::from(ctx.stop.worker_id)
        }
        OP_DETACH_STOPPED => {
            ctx.registry().detach(ctx.stop.pid);
            0
        }
        OP_COMMAND_REPLY => {
            let task = ctx.task();
            let message = task
                .data
                .get(..task.info.len as usize)
                .unwrap_or(&task.data);
            let state = ctx.state();
            let sock = crate::commands::reply_socket(state, ctx.server())?;
            let bus = state.message_bus();
            let handler = ctx.handler.as_ref()?;
            let fd = task.info.fd;
            crate::commands::answer(handler, ctx.server(), message, (bus, sock), fd, 0);
            0
        }
        _ => return None,
    })
}

/// `pool->onWorkerMessage`: `Server::read_worker_message` over the pool's server
/// and the message it popped.
///
/// # Safety
/// `msg` is the live `EventData` the pool popped.
unsafe extern "C-unwind" fn read_worker_message(pool: PoolRef<'_>, msg: *mut c_void) {
    // SAFETY: the event worker pool's `ptr` is its live server.
    let serv = unsafe { view::Server::from_raw(pool.ptr()) };
    let mut ctx = Context::of(Some(serv));
    ctx.task = msg.cast_const().cast();
    run_over(&mut ctx, RUN_READ_WORKER_MESSAGE, 0);
}

/// `Manager_signal_handler`: the manager's signals over `SwooleG.server`.
extern "C" fn signal_handler(signo: c_int) {
    run_manager(
        // SAFETY: the manager's signal runs while its server lives; the view ends with it.
        unsafe { view::Server::current() },
        RUN_MANAGER_SIGNAL,
        i64::from(signo).cast_unsigned(),
    );
}

/// Runs manager `operation` with `arg` over `serv`.
fn run_manager(serv: Option<view::Server<'_>>, operation: u32, arg: u64) {
    run_over(&mut Context::of(serv), operation, arg);
}

/// Runs `operation` with `arg` over `ctx`, whose objects are live.
fn run_over(ctx: &mut Context<'_>, operation: u32, arg: u64) -> i64 {
    swoole_rs_process::workers::run_over(ctx, operation, arg)
}

/// `Server::start_manager_process()`.
#[must_use]
pub(crate) fn start_manager_process(serv: view::Server<'_>) -> c_int {
    let code = run_over(&mut Context::of(Some(serv)), RUN_START_MANAGER, 0);
    c_int::try_from(code).unwrap_or(-1)
}

/// `Server::restart_worker_process(exit_status)`.
#[must_use]
pub fn restart_worker_process(serv: view::Server<'_>, status: &ExitStatus) -> c_int {
    let (pid, status) = (
        status.pid().cast_unsigned(),
        status.status().cast_unsigned(),
    );
    let arg = u64::from(pid) | u64::from(status) << 32;
    let code = run_over(&mut Context::of(Some(serv)), RUN_RESTART_WORKER, arg);
    c_int::try_from(code).unwrap_or(-1)
}

/// `Server::reload(reload_all_workers)`.
#[must_use]
pub fn reload(serv: view::Server<'_>, reload_all_workers: bool) -> bool {
    let arg = u64::from(reload_all_workers);
    run_over(&mut Context::of(Some(serv)), RUN_RELOAD, arg) != 0
}
