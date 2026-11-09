//! The manager's facts and effects (`crates/process/src/workers/manager.rs`'s
//! `OP_*`) over the server, pool and worker records Rust lays out. workers.cc's
//! `ManagerContext::call` hands every op it has no case for to
//! [`swr_v1_manager_call`].
use core::ffi::{c_int, c_void};
use core::ptr::{null, null_mut};

use swoole_rs_corebase::{DataHead, log, worker_registry as registry};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::methods::{self, ExitStatus, swoole_set_worker_type};
use swoole_rs_process::reload_task::{
    swr_v1_process_reload_task_add_workers, swr_v1_process_reload_task_kill_all,
};
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_pid, swr_v1_worker_type};
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::factory_methods::run;
use swoole_rs_process::workers::ops::{
    LOG_UNKNOWN_COMMAND, OP_ALL_WORKER_NUM, OP_ANY_WORKER_PID, OP_CHECK_EXIT_STATUS,
    OP_CREATE_EVENT_WORKERS, OP_CREATE_MESSAGE_BOX, OP_CREATE_USER_WORKERS, OP_DROP_RELOAD_TASK,
    OP_EVENT_WORKER_PID, OP_HAS_COMMAND, OP_HAS_MESSAGE_BOX, OP_HAS_ON_WORKER_MESSAGE,
    OP_INFO_TYPE, OP_IS_MANAGER, OP_KILL_EVENT_WORKERS, OP_KILL_TASK_WORKERS, OP_KILL_USER_WORKERS,
    OP_MANAGER, OP_MANAGER_INIT, OP_MANAGER_PID, OP_MAX_WAIT_TIME, OP_MODE, OP_ON_AFTER_RELOAD,
    OP_ON_BEFORE_RELOAD, OP_ON_MANAGER_START, OP_ON_MANAGER_STOP, OP_POOL_RELOAD, OP_READ_MESSAGE,
    OP_REAP_DETACHED, OP_REAP_WAIT, OP_RELOAD_ADD_EVENT_WORKERS, OP_RELOAD_ADD_TASK_WORKERS,
    OP_RELOAD_ASYNC, OP_RELOAD_INIT, OP_RELOAD_KILL_ALL, OP_RELOAD_TASK, OP_RELOAD_THREADS,
    OP_RELOAD_WARN, OP_REOPEN_LOGGER, OP_RESPAWN_EVENT_WORKER, OP_RESPAWN_TASK_WORKER,
    OP_RESPAWN_USER_WORKER, OP_RESTART_WORKER, OP_RUNNING, OP_SERVER_RELOAD, OP_SET_MANAGER_PID,
    OP_SET_MANAGER_TYPE, OP_SET_READ_MESSAGE, OP_SET_RELOAD_INIT, OP_SET_RUNNING,
    OP_SIGNAL_DISPATCH, OP_SPAWN_EVENT_WORKER, OP_SPAWN_TASK_WORKER, OP_SPAWN_USER_WORKER,
    OP_START_TASK_POOL, OP_TASK_POOL_WORKER_NUM, OP_TASK_WORKER_NUM, OP_TASK_WORKER_PID,
    OP_UNREGISTER_PID, OP_USER_WORKER_NUM, OP_WAIT_ERROR, OP_WAIT_STATUS, OP_WORKER_BY_PID,
    OP_WORKER_NUM, OP_WORKER_REGISTRY, OP_WORKER_TYPE,
};
use swoole_rs_process::workers::ops::{
    RUN_CHECK_EXIT_STATUS, RUN_KILL_TASK_WORKERS, RUN_SPAWN_EVENT_WORKER, RUN_SPAWN_USER_WORKER,
};

use crate::callbacks::{CB_AFTER_RELOAD, CB_BEFORE_RELOAD, CB_MANAGER_START, CB_MANAGER_STOP};
use crate::commands::{CommandFn, swr_v1_server_command_handler};
use crate::{view, worker_methods};

/// workers.cc's `WorkersNative<3>`, up to the fields only native code reads.
#[repr(C)]
struct Context {
    serv: *mut c_void,
    worker: *mut WorkerRecord,
    task: *const DataHead,
    handler: CommandFn,
    wait_status: c_int,
    wait_error: c_int,
}

// SAFETY: crates/reactor's body, under its upstream symbol.
unsafe extern "C-unwind" {
    #[link_name = "_Z14swoole_waitpidiPii"]
    fn swoole_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

/// `SWR_WORKER_EVENT`, `SWR_WORKER_USER`: the registry's categories.
const REGISTRY_EVENT: c_int = 1;
const REGISTRY_USER: c_int = 2;

/// `Factory::kill_event_workers()` (`origin` `kill_event_workers`) and
/// `kill_user_workers()`: SIGTERM each worker of `category`, then wait for each.
fn kill_workers(serv: *mut c_void, category: c_int, origin: &core::ffi::CStr) {
    // SAFETY: the live server's config.
    let registry = unsafe { view::config(serv) }.worker_registry;
    // SAFETY: the server's live registry.
    let registry = unsafe { &*registry.cast::<registry::WorkerRegistry>() };
    // SAFETY: kill(2) has no memory preconditions.
    let kill = |pid, signo| unsafe { libc::kill(pid, signo) };
    let wait = |pid| {
        let mut status = 0;
        // SAFETY: a local status.
        unsafe { swoole_waitpid(pid, &raw mut status, 0) }
    };
    let warn = |pid| log::sys_warning(origin, format_args!("waitpid({pid}) failed"));
    registry.terminate(category, libc::SIGTERM, kill, wait, warn);
}

/// `SW_MESSAGE_BOX_SIZE`, `swoole::WorkerType::SW_MANAGER`, `SW_LOG_ERROR`.
const MESSAGE_BOX_SIZE: usize = 65536;
const SW_MANAGER: c_int = 3;
const SW_LOG_ERROR: c_int = 5;

/// Answers manager op `op` with `arg` over the `WorkersNative<3>` at `context`.
///
/// # Safety
/// `context` is workers.cc's live `ManagerContext`, whose objects the op reads are live.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_manager_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    if op == OP_MANAGER {
        // `serv ? serv->manager_ : 0`: a signal may arrive before the server exists.
        return if ctx.serv.is_null() {
            0
        } else {
            // SAFETY: the context's live server.
            unsafe { view::config(ctx.serv) }.manager_ as u64
        };
    }
    // SAFETY: per the caller.
    let answer = unsafe { fact(ctx, op, arg) };
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { effect(ctx, op, arg) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { child(ctx, op, arg) });
    answer.unwrap_or_else(|| std::process::abort())
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
///
/// # Safety
/// As [`swr_v1_manager_call`].
unsafe fn fact(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: the context's live server.
    let config = unsafe { view::config(serv) };
    // SAFETY: as above.
    let workers = unsafe { view::workers(serv) };
    // SAFETY: the server's live event pool.
    let pool = unsafe { &*workers.event_pool };
    // SAFETY: the server's live task pool.
    let task_pool = unsafe { &*workers.task_pool };
    Some(match op {
        OP_HAS_MESSAGE_BOX => u64::from(!pool.message_box.is_null()),
        // SAFETY: the context's live server.
        OP_USER_WORKER_NUM => workers.user_count as u64,
        OP_MODE => u64::from(config.mode_),
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        OP_WORKER_NUM => u64::from(config.worker_num),
        OP_RELOAD_ASYNC => u64::from(config.reload_async),
        OP_MAX_WAIT_TIME => u64::from(config.max_wait_time),
        OP_EVENT_WORKER_PID => {
            let worker = config
                .workers
                .cast::<WorkerRecord>()
                .wrapping_add(index(arg));
            // SAFETY: one of the server's event workers.
            signed(unsafe { swr_v1_worker_pid(worker) })
        }
        OP_TASK_POOL_WORKER_NUM => u64::from(task_pool.worker_num),
        OP_TASK_WORKER_PID => {
            // SAFETY: one of the task pool's workers.
            signed(unsafe { swr_v1_worker_pid(task_pool.workers.wrapping_add(index(arg))) })
        }
        OP_WORKER_REGISTRY => config.worker_registry as u64,
        OP_RUNNING => u64::from(config.running),
        OP_RELOAD_INIT => u64::from(pool.reload_init),
        OP_RELOAD_TASK => pool.reload_task as u64,
        OP_WAIT_STATUS => signed(ctx.wait_status),
        OP_WAIT_ERROR => signed(ctx.wait_error),
        OP_READ_MESSAGE => u64::from(pool.read_message),
        OP_HAS_ON_WORKER_MESSAGE => u64::from(!pool.on_worker_message.is_null()),
        OP_RELOAD_WARN => methods::reload_warn as *const () as u64,
        OP_IS_MANAGER => {
            let pid = std::process::id().cast_signed();
            // SAFETY: the server's live shared counters.
            u64::from(pid == unsafe { view::counters(serv) }.manager_pid)
        }
        // SAFETY: the context's live server.
        OP_MANAGER_PID => signed(unsafe { view::counters(serv) }.manager_pid),
        OP_ALL_WORKER_NUM => {
            u64::from(config.worker_num)
                + u64::from(config.task_worker_num)
                + workers.user_count as u64
        }
        OP_ANY_WORKER_PID => {
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            // SAFETY: the context's live server.
            let worker = unsafe { view::worker(serv, id) };
            // SAFETY: the id names one of its workers.
            signed(unsafe { swr_v1_worker_pid(worker) })
        }
        // SAFETY: the worker OP_WORKER_BY_PID found.
        OP_WORKER_TYPE => u64::from(unsafe { swr_v1_worker_type(ctx.worker) }),
        // SAFETY: the command message's live head.
        OP_INFO_TYPE => u64::from(unsafe { (*ctx.task).kind }),
        _ => return None,
    })
}

/// The effects on the server, its pools and the process.
///
/// # Safety
/// As [`swr_v1_manager_call`].
unsafe fn effect(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: the context's live server.
    let config = unsafe { view::config(serv) };
    // SAFETY: as above.
    let workers = unsafe { view::workers(serv) };
    let (pool, task_pool) = (workers.event_pool, workers.task_pool);
    // A borrow per use: the pool methods below take the pool by pointer.
    // SAFETY: the server's live event pool, which only the manager writes meanwhile.
    let event_pool = || unsafe { &mut *pool };
    // SAFETY: the context's live server.
    let state = unsafe { view::state_ptr(serv) };
    // SAFETY: its state, live as long as it.
    let callbacks = unsafe { &(*state).callbacks };
    let callback = |index| {
        if let Some(slot) = callbacks.slot(index) {
            // SAFETY: the manager and reload callbacks take the server.
            unsafe { slot.call_server(serv) };
        }
    };
    let registry = config.worker_registry.cast::<registry::WorkerRegistry>();
    match op {
        OP_CREATE_EVENT_WORKERS => {
            // SAFETY: the context's live server.
            let created = unsafe { worker_methods::create_event_workers(serv) };
            return Some(u64::from(created));
        }
        OP_CREATE_MESSAGE_BOX => {
            // SAFETY: the server's live event pool.
            return Some(signed(unsafe {
                methods::create_message_box(pool, MESSAGE_BOX_SIZE)
            }));
        }
        OP_CREATE_USER_WORKERS => {
            // SAFETY: the context's live server.
            let created = unsafe { worker_methods::create_user_workers(serv) };
            return Some(u64::from(created));
        }
        OP_SET_MANAGER_PID => {
            let pid = std::process::id().cast_signed();
            // SAFETY: the server's shared counters, which only the manager writes meanwhile.
            unsafe { view::counters_mut(serv) }.manager_pid = pid;
        }
        // SAFETY: the server's live task pool.
        OP_START_TASK_POOL => return Some(signed(unsafe { methods::start(task_pool) })),
        OP_SET_MANAGER_TYPE => swoole_set_worker_type(SW_MANAGER),
        OP_MANAGER_INIT => {
            config.manager_ = core::ptr::without_provenance_mut(index(arg));
            event_pool().on_worker_message = entry::read_worker_message as *const c_void;
            event_pool().max_wait_time = config.max_wait_time;
            let pid = std::process::id().cast_signed();
            // SAFETY: the server's shared counters.
            unsafe { view::counters_mut(serv) }.manager_pid = pid;
            event_pool().master_pid = pid;
            // SAFETY: the server's live task pool, distinct from the event pool.
            unsafe { (*task_pool).master_pid = pid };
        }
        OP_ON_MANAGER_START => callback(CB_MANAGER_START),
        OP_ON_MANAGER_STOP => callback(CB_MANAGER_STOP),
        OP_ON_BEFORE_RELOAD => callback(CB_BEFORE_RELOAD),
        OP_ON_AFTER_RELOAD => callback(CB_AFTER_RELOAD),
        // SAFETY: the server's live event pool.
        OP_DROP_RELOAD_TASK => unsafe { methods::drop_reload_task(pool) },
        OP_KILL_TASK_WORKERS => {
            // SAFETY: the context's live server.
            unsafe {
                run(
                    serv,
                    RUN_KILL_TASK_WORKERS,
                    null_mut(),
                    null_mut(),
                    null(),
                    0,
                )
            };
        }
        OP_SET_RUNNING => config.running = arg != 0,
        OP_SET_RELOAD_INIT => event_pool().reload_init = arg != 0,
        OP_SET_READ_MESSAGE => event_pool().read_message = arg != 0,
        OP_REAP_WAIT => {
            // SAFETY: the context's own status slots.
            let pid = unsafe {
                swoole_rs_process::process_pool::swr_v1_process_reap_wait(
                    core::ptr::null_mut(),
                    arg != 0,
                    &raw mut ctx.wait_status,
                    &raw mut ctx.wait_error,
                )
            };
            return Some(signed(pid));
        }
        OP_SIGNAL_DISPATCH => swoole_rs_process::signal::swoole_signal_dispatch(),
        // SAFETY: the reload task the manager started and the server's event workers.
        OP_RELOAD_ADD_EVENT_WORKERS => unsafe {
            swr_v1_process_reload_task_add_workers(
                event_pool().reload_task,
                config.workers.cast(),
                config.worker_num as usize,
            );
        },
        OP_RELOAD_ADD_TASK_WORKERS => {
            // SAFETY: the server's live task pool.
            let task_workers = unsafe { (*task_pool).workers };
            // SAFETY: the reload task the manager started and the task pool's workers.
            unsafe {
                swr_v1_process_reload_task_add_workers(
                    event_pool().reload_task,
                    task_workers,
                    config.task_worker_num as usize,
                );
            }
        }
        // SAFETY: the reload task the manager started.
        OP_RELOAD_KILL_ALL => unsafe {
            swr_v1_process_reload_task_kill_all(
                event_pool().reload_task,
                libc::SIGTERM,
                methods::reload_warn,
            );
        },
        OP_RESTART_WORKER => {
            let status = exit_status(arg);
            // SAFETY: the context's live server and the exited child's status.
            unsafe { entry::restart_worker_process(serv, (&raw const status).cast()) };
        }
        // SAFETY: the context's live server.
        OP_SERVER_RELOAD => _ = unsafe { entry::reload(serv, arg != 0) },
        OP_REOPEN_LOGGER => {
            if let Some(logger) = swoole_rs_utilities::process_logger() {
                // SAFETY: the process logger, live while the process runs.
                unsafe { swoole_rs_utilities::swr_v1_logger_reopen(logger) };
            }
        }
        OP_REAP_DETACHED => {
            // SAFETY: the server's live registry.
            return Some(u64::from(unsafe {
                registry::swr_v1_process_worker_registry_reap_detached(registry, pid_of(arg))
            }));
        }
        OP_UNREGISTER_PID => {
            // SAFETY: the server's live registry.
            unsafe { registry::swr_v1_process_worker_registry_unregister(registry, pid_of(arg)) };
        }
        OP_HAS_COMMAND => {
            // SAFETY: the command message's live head.
            let id = i32::from(unsafe { (*ctx.task).server_fd });
            // SAFETY: the server's live state and the context's handler slot.
            let found = unsafe { swr_v1_server_command_handler(state, id, &raw mut ctx.handler) };
            return Some(u64::from(found));
        }
        LOG_UNKNOWN_COMMAND => {
            // SAFETY: the command message's live head.
            let id = unsafe { (*ctx.task).server_fd };
            log::error_log(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_INVALID_COMMAND as c_int,
                c"Server::read_worker_message",
                format_args!("Unknown command[command_id={id}]"),
            );
        }
        OP_RELOAD_THREADS => {
            // SAFETY: the context's live server.
            let reloaded = unsafe { entry::reload_worker_threads(serv, arg != 0) };
            return Some(u64::from(reloaded));
        }
        // SAFETY: the server's live event pool.
        OP_POOL_RELOAD => return Some(u64::from(unsafe { methods::reload(pool) })),
        _ => return None,
    }
    Some(0)
}

/// Finding, checking and replacing the server's children.
///
/// # Safety
/// As [`swr_v1_manager_call`].
unsafe fn child(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // `Factory::spawn_*_worker(worker)`: the child's pid, or -1.
    let spawn = |op, worker| {
        // SAFETY: the context's live server, and one of its workers.
        let pid = unsafe { run(serv, op, worker, null_mut(), null(), 0) };
        c_int::try_from(pid).unwrap_or(-1)
    };
    let pid = match op {
        OP_SPAWN_EVENT_WORKER => {
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            // SAFETY: the context's live server.
            let worker = unsafe { view::worker(serv, id) };
            spawn(RUN_SPAWN_EVENT_WORKER, worker.cast())
        }
        OP_SPAWN_USER_WORKER => {
            // SAFETY: the context's live server.
            let registry = unsafe { view::config(serv) }.worker_registry;
            // SAFETY: the server's live registry.
            let pending =
                unsafe { registry::swr_v1_process_worker_registry_pending_data(registry.cast()) };
            // SAFETY: the manager spawns the pending workers by index, below their count.
            let slot = unsafe { pending.add(index(arg)) };
            // SAFETY: as above.
            let worker = unsafe { *slot };
            spawn(RUN_SPAWN_USER_WORKER, worker)
        }
        OP_WORKER_BY_PID => {
            // SAFETY: the context's live server.
            ctx.worker = unsafe { worker_methods::get_worker_by_pid(serv, pid_of(arg)) };
            return Some(u64::from(!ctx.worker.is_null()));
        }
        OP_CHECK_EXIT_STATUS => {
            let status = exit_status(arg);
            let (worker, status) = (ctx.worker.cast(), (&raw const status).cast());
            // SAFETY: the context's live server, the worker found and its exit status.
            unsafe { run(serv, RUN_CHECK_EXIT_STATUS, worker, null_mut(), status, 0) };
            return Some(0);
        }
        OP_KILL_EVENT_WORKERS => {
            kill_workers(serv, REGISTRY_EVENT, c"Factory::kill_event_workers");
            return Some(0);
        }
        OP_KILL_USER_WORKERS => {
            kill_workers(serv, REGISTRY_USER, c"Factory::kill_user_workers");
            return Some(0);
        }
        OP_SPAWN_TASK_WORKER => {
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            // SAFETY: the context's live server.
            let pool = unsafe { view::task_pool(serv) };
            // SAFETY: as above.
            let worker = unsafe { view::worker(serv, id) };
            // SAFETY: the live task pool and one of its workers.
            unsafe { methods::spawn(pool, worker) };
            return Some(0);
        }
        OP_RESPAWN_TASK_WORKER => {
            // SAFETY: the context's live server.
            let pool = unsafe { view::task_pool(serv) };
            // SAFETY: the live task pool and the worker found.
            unsafe { methods::spawn(pool, ctx.worker) }
        }
        OP_RESPAWN_EVENT_WORKER => spawn(RUN_SPAWN_EVENT_WORKER, ctx.worker.cast()),
        OP_RESPAWN_USER_WORKER => spawn(RUN_SPAWN_USER_WORKER, ctx.worker.cast()),
        _ => return None,
    };
    Some(signed(pid))
}
