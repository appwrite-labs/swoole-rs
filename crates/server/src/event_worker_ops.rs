//! The event worker's facts and effects (`crates/process/src/workers/event_worker.rs`'s
//! `OP_*`) over the server, worker and connection records Rust lays out. process.cc's
//! `WorkerContext::call` hands every op it has no case for to
//! [`swr_v1_event_worker_call`].
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::DataHead;
#[cfg(target_os = "linux")]
use swoole_rs_corebase::log;
use swoole_rs_process::methods::{
    push_message, swoole_get_worker_id, swoole_get_worker_type, swoole_set_worker_id,
    swoole_set_worker_type,
};
use swoole_rs_process::worker_globals::{swr_v1_worker_global_detach, swr_v1_worker_globals};
use swoole_rs_process::worker_record::{
    WorkerRecord, swr_v1_worker_exceeded_max_requests, swr_v1_worker_id, swr_v1_worker_set_status,
    swr_v1_worker_shutdown, swr_v1_worker_type,
};
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::ops::{
    HOOK_BEFORE_SERVER_SHUTDOWN, HOOK_BEFORE_WORKER_START, OP_ABORT_WORKER, OP_ALL_CORE_WORKER_NUM,
    OP_CALL_WORKER_START, OP_CLEAR_TIMER, OP_CLOSE_CONNECTION, OP_CO_DISPATCH, OP_CONN_AT,
    OP_CONN_REACTOR, OP_CONN_READABLE, OP_CPU_AFFINITY, OP_CURRENT_WORKER_ID, OP_DETACH_WORKER,
    OP_DISABLE_NOTIFY, OP_DISCARD_TIMEOUT_REQUEST, OP_DRAIN_PIPES, OP_EXCEEDED_MAX_REQUEST,
    OP_GLOBAL_HOOK, OP_HAS_ON_WORKER_EXIT, OP_HAS_REACTOR, OP_INFO_FD, OP_INFO_LEN, OP_INFO_TYPE,
    OP_IS_EVENT_WORKER, OP_MAX_WAIT_TIME, OP_MAXFD, OP_MINFD, OP_MODE, OP_ON_BEFORE_SHUTDOWN,
    OP_ON_BUFFER_EMPTY, OP_ON_BUFFER_FULL, OP_ON_CONNECT, OP_ON_WORKER_EXIT, OP_ON_WORKER_START,
    OP_ON_WORKER_STOP, OP_POOL_RUNNING, OP_PUSH_STOP_MESSAGE, OP_REACTOR_NUM, OP_RELOAD_ASYNC,
    OP_REOPEN_LOGGER, OP_RESET_WORKER_COUNTER, OP_SIGNAL_LIVE, OP_SIGNAL_SET, OP_STOP_ASYNC,
    OP_WORKER_ENTER, OP_WORKER_EXISTS, OP_WORKER_ID, OP_WORKER_NUM, OP_WORKER_SHUTDOWN,
    OP_WORKER_SIGNAL_INIT, OP_WORKER_START_CALLBACK, OP_WORKER_STATUS, OP_WORKER_STOP_CALLBACK,
    OP_WORKER_TYPE,
};
use swoole_rs_reactor::SocketRecord;

use crate::ConnectionRecord;
use crate::callbacks::{
    CB_BEFORE_SHUTDOWN, CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CONNECT, CB_WORKER_EXIT,
    CB_WORKER_START, CB_WORKER_STOP, CallbackSlot,
};
use crate::view;

/// process.cc's `WorkersNative<0>`.
#[repr(C)]
struct Context {
    serv: *mut c_void,
    worker: *mut WorkerRecord,
    info: *mut DataHead,
    reactor: *mut c_void,
    conn: *mut ConnectionRecord,
    status: *const c_void,
    socket: *mut c_void,
}

/// `swGlobalHookType`'s worker and shutdown hooks.
const SW_GLOBAL_HOOK_BEFORE_WORKER_START: c_int = 2;
const SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN: c_int = 6;
const SW_GLOBAL_HOOK_BEFORE_WORKER_STOP: c_int = 8;
/// `swoole::WorkerType::SW_EVENT_WORKER`, `SW_WORKER_MESSAGE_STOP`.
const SW_EVENT_WORKER: c_int = 2;
const SW_WORKER_MESSAGE_STOP: u8 = 1;
/// `Server::DISPATCH_CO_CONN_LB`, `DISPATCH_CO_REQ_LB`.
const DISPATCH_CO_CONN_LB: u8 = 8;
const DISPATCH_CO_REQ_LB: u8 = 9;

/// `WorkerStopMessage`.
#[repr(C)]
struct WorkerStopMessage {
    pid: libc::pid_t,
    worker_id: u16,
}

/// Answers event-worker op `op` with `arg` over the `WorkersNative<0>` at `context`.
///
/// # Safety
/// `context` is process.cc's live `WorkerContext`, whose objects the op reads are live.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_event_worker_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    // SAFETY: per the caller.
    let answer = unsafe { fact(ctx, op, arg) };
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { callback(ctx, op) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { effect(ctx, op, arg) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { connection(ctx, op, arg) });
    answer.unwrap_or_else(|| std::process::abort())
}

/// The facts read from the server, the worker, the event head and the globals.
///
/// # Safety
/// As [`swr_v1_event_worker_call`].
unsafe fn fact(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    // SAFETY: the context's live server.
    let config = unsafe { view::config(serv) };
    let info = || {
        // SAFETY: the ops that read it run with the event's live head.
        unsafe { &*ctx.info }
    };
    Some(match op {
        OP_MODE => u64::from(config.mode_),
        OP_SIGNAL_LIVE => u64::from(
            swoole_rs_corebase::swr_v1_core_running()
                && !serv.is_null()
                && !worker.is_null()
                && config.running,
        ),
        OP_EXCEEDED_MAX_REQUEST => {
            // SAFETY: the context's live worker.
            u64::from(unsafe { swr_v1_worker_exceeded_max_requests(worker) })
        }
        OP_INFO_TYPE => u64::from(info().kind),
        OP_INFO_FD => info().fd.cast_unsigned(),
        OP_INFO_LEN => u64::from(info().len),
        OP_DISABLE_NOTIFY => u64::from(config.disable_notify),
        OP_DISCARD_TIMEOUT_REQUEST => u64::from(config.discard_timeout_request),
        // SAFETY: the context's live worker.
        OP_WORKER_ID => u64::from(unsafe { swr_v1_worker_id(worker) }),
        // SAFETY: the context's live worker.
        OP_WORKER_TYPE => u64::from(unsafe { swr_v1_worker_type(worker) }),
        OP_ALL_CORE_WORKER_NUM => u64::from(config.worker_num) + u64::from(config.task_worker_num),
        OP_WORKER_NUM => u64::from(config.worker_num),
        OP_REACTOR_NUM => u64::from(config.reactor_num),
        OP_RELOAD_ASYNC => u64::from(config.reload_async),
        OP_MAX_WAIT_TIME => u64::from(config.max_wait_time),
        OP_CO_DISPATCH => u64::from(
            config.dispatch_mode == DISPATCH_CO_CONN_LB
                || config.dispatch_mode == DISPATCH_CO_REQ_LB,
        ),
        OP_CURRENT_WORKER_ID => u64::from(swoole_get_worker_id()),
        OP_WORKER_EXISTS => {
            let id = u16::try_from(arg.cast_signed()).ok();
            // SAFETY: the live server.
            u64::from(id.is_some_and(|id| !unsafe { view::worker(serv, id) }.is_null()))
        }
        OP_IS_EVENT_WORKER => u64::from(swoole_get_worker_type() == SW_EVENT_WORKER),
        OP_POOL_RUNNING => {
            // SAFETY: the live server.
            let pool = unsafe { view::workers(serv) }.event_pool;
            // SAFETY: its live event worker pool.
            u64::from(unsafe { (*pool).running })
        }
        // SAFETY: the live server's counters.
        OP_MINFD => i64::from(unsafe { view::counters(serv) }.min_fd).cast_unsigned(),
        // SAFETY: as above.
        OP_MAXFD => i64::from(unsafe { view::counters(serv) }.max_fd).cast_unsigned(),
        OP_HAS_REACTOR => u64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        _ => return None,
    })
}

/// The server's PHP-facing callbacks.
///
/// # Safety
/// As [`swr_v1_event_worker_call`].
unsafe fn callback(ctx: &Context, op: u32) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: the context's live server.
    let state = unsafe { view::state_ptr(serv) };
    // SAFETY: its state, live as long as it.
    let callbacks = unsafe { &(*state).callbacks };
    let (index, data) = match op {
        OP_ON_CONNECT => (CB_CONNECT, ctx.info.cast()),
        OP_ON_BUFFER_FULL => (CB_BUFFER_FULL, ctx.info.cast()),
        OP_ON_BUFFER_EMPTY => (CB_BUFFER_EMPTY, ctx.info.cast()),
        OP_ON_WORKER_START => (CB_WORKER_START, ctx.worker.cast()),
        OP_ON_WORKER_STOP => (CB_WORKER_STOP, ctx.worker.cast()),
        OP_ON_WORKER_EXIT => {
            // SAFETY: this thread's worker globals.
            let current = unsafe { (*swr_v1_worker_globals()).worker };
            (CB_WORKER_EXIT, current.cast())
        }
        OP_ON_BEFORE_SHUTDOWN => (CB_BEFORE_SHUTDOWN, core::ptr::null_mut()),
        OP_HAS_ON_WORKER_EXIT => {
            let set = callbacks
                .slot(CB_WORKER_EXIT)
                .is_some_and(CallbackSlot::is_set);
            return Some(u64::from(set));
        }
        _ => return None,
    };
    let slot = callbacks.slot(index)?;
    if op == OP_ON_BEFORE_SHUTDOWN {
        // SAFETY: `onBeforeShutdown` takes the server.
        unsafe { slot.call_server(serv) };
    } else {
        // SAFETY: the slot takes the server and the event head or worker.
        unsafe { slot.call(serv, data) };
    }
    Some(0)
}

/// The effects on the worker, the server and the process.
///
/// # Safety
/// As [`swr_v1_event_worker_call`].
unsafe fn effect(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    match op {
        // SAFETY: Rust's signal handler for the event worker.
        OP_SIGNAL_SET => unsafe { swr_workers_signal_set(arg, entry::worker_signal_handler) },
        // SAFETY: the context's live server and worker.
        OP_STOP_ASYNC => unsafe { entry::stop_async_worker(serv, worker.cast()) },
        // SAFETY: the context's live worker.
        OP_WORKER_SHUTDOWN => unsafe { swr_v1_worker_shutdown(worker) },
        // SAFETY: as above.
        OP_WORKER_STATUS => unsafe { swr_v1_worker_set_status(worker, u8::from(arg != 0)) },
        OP_REOPEN_LOGGER => {
            if let Some(logger) = swoole_rs_utilities::process_logger() {
                // SAFETY: the process logger, live while the process runs.
                unsafe { swoole_rs_utilities::swr_v1_logger_reopen(logger) };
            }
        }
        // SAFETY: the context's live server and worker.
        OP_CALL_WORKER_START => unsafe { entry::call_worker_start_callback(serv, worker.cast()) },
        // SAFETY: as above.
        OP_WORKER_START_CALLBACK => unsafe { entry::worker_start_callback(serv, worker.cast()) },
        // SAFETY: as above.
        OP_WORKER_STOP_CALLBACK => unsafe { entry::worker_stop_callback(serv, worker.cast()) },
        // SAFETY: the context's live server.
        OP_WORKER_SIGNAL_INIT => unsafe { entry::worker_signal_init(serv) },
        // SAFETY: as above.
        OP_DRAIN_PIPES => unsafe { entry::drain_worker_pipe(serv) },
        // SAFETY: as above.
        OP_CLEAR_TIMER => unsafe { crate::server_methods::clear_timer(serv) },
        // SAFETY: the context's live server and worker.
        OP_ABORT_WORKER => unsafe { crate::worker_methods::abort_worker(serv, worker) },
        // SAFETY: as above.
        OP_RESET_WORKER_COUNTER => unsafe {
            crate::worker_methods::reset_worker_counter(serv, worker);
        },
        // SAFETY: the context's live worker.
        OP_DETACH_WORKER => unsafe { swr_v1_worker_global_detach(worker) },
        OP_WORKER_ENTER => {
            // SAFETY: as above.
            swoole_set_worker_id(unsafe { swr_v1_worker_id(worker) });
            swoole_set_worker_type(SW_EVENT_WORKER);
            // SAFETY: the context's live server and worker.
            unsafe { crate::worker_methods::init_event_worker(serv, worker) };
        }
        OP_CPU_AFFINITY => {
            // SAFETY: the context's live server.
            let config = unsafe { view::config(serv) };
            #[cfg(target_os = "linux")]
            if config.open_cpu_affinity {
                // SAFETY: the context's live worker.
                set_cpu_affinity(config, unsafe { swr_v1_worker_id(worker) });
            }
            #[cfg(not(target_os = "linux"))]
            let _ = config;
        }
        // SAFETY: the context's live server and worker.
        OP_GLOBAL_HOOK => unsafe { global_hook(serv, worker, arg) },
        OP_PUSH_STOP_MESSAGE => {
            let msg = WorkerStopMessage {
                // SAFETY: a plain syscall.
                pid: unsafe { libc::getpid() },
                // SAFETY: the context's live worker.
                worker_id: u16::try_from(unsafe { swr_v1_worker_id(worker) }).unwrap_or(u16::MAX),
            };
            // SAFETY: the context's live server.
            let pool = unsafe { view::workers(serv) }.event_pool;
            let data = (&raw const msg).cast::<u8>();
            // SAFETY: its live event pool and a message of the size passed.
            let sent =
                unsafe { push_message(pool, SW_WORKER_MESSAGE_STOP, data, size_of_val(&msg)) };
            return Some(i64::from(sent).cast_unsigned());
        }
        _ => return None,
    }
    Some(0)
}

/// `Server::foreach_connection`'s selected connection.
///
/// # Safety
/// As [`swr_v1_event_worker_call`]; the ops after `OP_CONN_AT` run with the
/// connection it selected.
unsafe fn connection(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    if op == OP_CONN_AT {
        let fd = c_int::try_from(arg.cast_signed()).unwrap_or(-1);
        // SAFETY: the context's live server.
        let conn = unsafe { view::at(ctx.serv, fd) };
        let conn = conn.map_or(core::ptr::null_mut(), |conn| &raw mut *conn);
        // SAFETY: null or a live connection whose socket is null or live.
        let valid = unsafe { crate::reactor_ops::valid(conn) };
        ctx.conn = if valid { conn } else { core::ptr::null_mut() };
        return Some(u64::from(valid));
    }
    // SAFETY: null or the connection OP_CONN_AT selected.
    let conn = unsafe { ctx.conn.as_ref() }?;
    Some(match op {
        OP_CONN_READABLE => {
            // SAFETY: the valid connection's live socket.
            let socket = unsafe { &*conn.socket.cast::<SocketRecord>() };
            u64::from(conn.peer_closed == 0 && socket.removed == 0)
        }
        OP_CONN_REACTOR => u64::from(conn.reactor_id),
        OP_CLOSE_CONNECTION => {
            let reactor = swoole_rs_corebase::thread_reactor();
            // SAFETY: the thread's live reactor and the connection's session socket.
            unsafe { crate::reactor_entry::close_connection(reactor, conn.socket.cast()) };
            0
        }
        _ => return None,
    })
}

/// `OP_SIGNAL_SET`: the signal number (-1 for SIGRTMIN) in the low word and the
/// handler in the high word: 0 none, 1 `SIG_IGN`, 2 `own`.
///
/// # Safety
/// `own` is a signal handler nothing unloads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_workers_signal_set(arg: u64, own: unsafe extern "C" fn(c_int)) {
    // SAFETY: crates/reactor's body, under its upstream symbol.
    unsafe extern "C-unwind" {
        #[link_name = "_Z17swoole_signal_setiPFviEb"]
        fn swoole_signal_set(signo: c_int, handler: usize, safety: bool) -> usize;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the low word holds the signal number"
    )]
    let signo = (arg as u32).cast_signed();
    #[cfg(target_os = "linux")]
    let signo = if signo == -1 { libc::SIGRTMIN() } else { signo };
    #[cfg(not(target_os = "linux"))]
    if signo == -1 {
        return;
    }
    let handler = match arg >> 32 {
        0 => 0,
        1 => libc::SIG_IGN,
        _ => own as usize,
    };
    // SAFETY: a valid signal number and a handler per the caller.
    let _previous = unsafe { swoole_signal_set(signo, handler, false) };
}

/// `Worker_global_hook`: the worker start/stop and server shutdown hooks.
///
/// # Safety
/// `serv` and `worker` are live.
unsafe fn global_hook(serv: *mut c_void, worker: *mut WorkerRecord, hook: u64) {
    let id = match hook {
        HOOK_BEFORE_WORKER_START => SW_GLOBAL_HOOK_BEFORE_WORKER_START,
        HOOK_BEFORE_SERVER_SHUTDOWN => SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN,
        _ => SW_GLOBAL_HOOK_BEFORE_WORKER_STOP,
    };
    if !swoole_rs_corebase::swoole_isset_hook(id) {
        return;
    }
    if hook == HOOK_BEFORE_SERVER_SHUTDOWN {
        // SAFETY: the shutdown hooks take the server.
        unsafe { swoole_rs_corebase::swoole_call_hook(id, serv) };
        return;
    }
    // SAFETY: per the caller.
    let worker_id = unsafe { swr_v1_worker_id(worker) } as usize;
    let mut args: [*mut c_void; 2] = [serv, core::ptr::without_provenance_mut(worker_id)];
    // SAFETY: the worker hooks take { server, worker id }.
    unsafe { swoole_rs_corebase::swoole_call_hook(id, args.as_mut_ptr().cast()) };
}

/// `Worker_set_cpu_affinity`.
#[cfg(target_os = "linux")]
fn set_cpu_affinity(config: &crate::ServerConfig, worker_id: u32) {
    const ORIGIN: &core::ffi::CStr = c"Worker_set_cpu_affinity";
    /// `Server::MODE_THREAD`.
    const MODE_THREAD: u8 = 3;
    // SAFETY: the server's live CPU list and its length.
    let outcome = unsafe {
        swoole_rs_process::affinity::swr_v1_process_worker_affinity(
            worker_id,
            config.cpu_affinity_available,
            config.cpu_affinity_available_num,
            config.mode_ == MODE_THREAD,
        )
    };
    match outcome.status {
        1 => log::sys_warning_errno(
            outcome.error,
            ORIGIN,
            format_args!("get CPU affinity for worker#{worker_id} failed"),
        ),
        2 => log::warning(
            ORIGIN,
            format_args!("no available CPU for worker#{worker_id}"),
        ),
        3 => log::sys_warning_errno(
            outcome.error,
            ORIGIN,
            format_args!("set CPU affinity for worker#{worker_id} failed"),
        ),
        _ => {}
    }
}
