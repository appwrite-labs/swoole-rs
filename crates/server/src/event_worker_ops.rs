//! The event worker's facts and effects (`crates/process/src/workers/event_worker.rs`'s
//! `OP_*`) over the server, worker and connection records Rust lays out.
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::DataHead;
#[cfg(target_os = "linux")]
use swoole_rs_corebase::log;
use swoole_rs_process::methods::{
    push_message, swoole_get_worker_id, swoole_get_worker_type, swoole_set_worker_id,
    swoole_set_worker_type,
};
use swoole_rs_process::worker_globals::{thread_globals, worker_global_detach};
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_shutdown};
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::ops::{
    HOOK_BEFORE_SERVER_SHUTDOWN, HOOK_BEFORE_WORKER_START, OP_ABORT_WORKER, OP_ALL_CORE_WORKER_NUM,
    OP_CALL_WORKER_START, OP_CLEAR_TIMER, OP_CLOSE_CONNECTION, OP_CO_DISPATCH, OP_CONN_AT,
    OP_CONN_REACTOR, OP_CONN_READABLE, OP_CPU_AFFINITY, OP_CURRENT_WORKER_ID, OP_DETACH_WORKER,
    OP_DISABLE_NOTIFY, OP_DISCARD_TIMEOUT_REQUEST, OP_DRAIN_PIPES, OP_EXCEEDED_MAX_REQUEST,
    OP_FACTORY_END, OP_GLOBAL_HOOK, OP_HAS_ON_WORKER_EXIT, OP_HAS_REACTOR, OP_INFO_FD, OP_INFO_LEN,
    OP_INFO_TYPE, OP_IS_EVENT_WORKER, OP_MAX_WAIT_TIME, OP_MAXFD, OP_MINFD, OP_MODE,
    OP_ON_BEFORE_SHUTDOWN, OP_ON_BUFFER_EMPTY, OP_ON_BUFFER_FULL, OP_ON_CONNECT, OP_ON_WORKER_EXIT,
    OP_ON_WORKER_START, OP_ON_WORKER_STOP, OP_POOL_RUNNING, OP_PUSH_STOP_MESSAGE, OP_REACTOR_NUM,
    OP_RELOAD_ASYNC, OP_REOPEN_LOGGER, OP_RESET_WORKER_COUNTER, OP_SIGNAL_LIVE, OP_SIGNAL_SET,
    OP_STOP_ASYNC, OP_WORKER_ENTER, OP_WORKER_EXISTS, OP_WORKER_ID, OP_WORKER_NUM,
    OP_WORKER_SHUTDOWN, OP_WORKER_SIGNAL_INIT, OP_WORKER_START_CALLBACK, OP_WORKER_STATUS,
    OP_WORKER_STOP_CALLBACK, OP_WORKER_TYPE,
};

use crate::ConnectionRecord;
use crate::callbacks::{
    CB_BEFORE_SHUTDOWN, CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CONNECT, CB_WORKER_EXIT,
    CB_WORKER_START, CB_WORKER_STOP, CallbackSlot,
};
use crate::view;

mod native;
pub(crate) use native::enter;

/// What an operation runs over.
struct Context {
    serv: *mut c_void,
    worker: *mut WorkerRecord,
    info: *mut DataHead,
    reactor: *mut c_void,
    conn: *mut ConnectionRecord,
    status: *const c_void,
    socket: *mut c_void,
}

impl Context {
    /// The context's live server.
    fn server(&self) -> view::Server<'_> {
        // SAFETY: the method that built the context lent its live server for the call.
        unsafe { view::Server::from_raw(self.serv) }
    }
}

/// `swGlobalHookType`'s worker and shutdown hooks.
const SW_GLOBAL_HOOK_BEFORE_WORKER_START: c_int = 2;
const SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN: c_int = 6;
const SW_GLOBAL_HOOK_BEFORE_WORKER_STOP: c_int = 8;
/// `swoole::WorkerType::SW_EVENT_WORKER`, `SW_WORKER_MESSAGE_STOP`.
const SW_EVENT_WORKER: c_int = 2;
const SW_WORKER_MESSAGE_STOP: u8 = 1;
/// `SW_WORKER_BUSY`, `SW_WORKER_IDLE`.
const SW_WORKER_BUSY: u8 = 1;
const SW_WORKER_IDLE: u8 = 2;
/// `Server::DISPATCH_CO_CONN_LB`, `DISPATCH_CO_REQ_LB`.
const DISPATCH_CO_CONN_LB: u8 = 8;
const DISPATCH_CO_REQ_LB: u8 = 9;

/// `WorkerStopMessage`.
#[repr(C)]
struct WorkerStopMessage {
    pid: libc::pid_t,
    worker_id: u16,
}

/// Answers each event-worker op over the context.
impl swoole_rs_process::workers::Answer for Context {
    #[inline(always)]
    fn answer(&mut self, op: u32, arg: u64) -> u64 {
        // Plain branches, not `or_else` closures, so each group inlines and an op
        // known at the call folds to its arm.
        let ctx = self;
        if let Some(value) = native::native(ctx, op, arg) {
            return value;
        }
        if let Some(value) = fact(ctx, op, arg) {
            return value;
        }
        if let Some(value) = callback(ctx, op) {
            return value;
        }
        if let Some(value) = effect(ctx, op, arg) {
            return value;
        }
        connection(ctx, op, arg).unwrap_or_else(|| std::process::abort())
    }
}

/// The facts read from the server, the worker, the event head and the globals.
#[inline(always)]
fn fact(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let config = ctx.config();
    Some(match op {
        OP_FACTORY_END => u64::from(crate::factory::end(ctx.state(), ctx.info().fd, 0)),
        OP_MODE => u64::from(config.mode_),
        OP_SIGNAL_LIVE => u64::from(
            swoole_rs_corebase::swr_v1_core_running() && !ctx.worker.is_null() && config.running,
        ),
        OP_EXCEEDED_MAX_REQUEST => u64::from(ctx.worker().exceeded_max_request()),
        OP_INFO_TYPE => u64::from(ctx.info().kind),
        OP_INFO_FD => ctx.info().fd.cast_unsigned(),
        OP_INFO_LEN => u64::from(ctx.info().len),
        OP_DISABLE_NOTIFY => u64::from(config.disable_notify),
        OP_DISCARD_TIMEOUT_REQUEST => u64::from(config.discard_timeout_request),
        OP_WORKER_ID => u64::from(ctx.worker().id()),
        OP_WORKER_TYPE => u64::from(ctx.worker().kind()),
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
            u64::from(id.is_some_and(|id| !ctx.server().worker(id).is_null()))
        }
        OP_IS_EVENT_WORKER => u64::from(swoole_get_worker_type() == SW_EVENT_WORKER),
        OP_POOL_RUNNING => {
            let pool = ctx.server().workers().event_pool;
            // SAFETY: its live event worker pool.
            u64::from(unsafe { (*pool).running })
        }
        OP_MINFD => i64::from(ctx.counters().min_fd).cast_unsigned(),
        OP_MAXFD => i64::from(ctx.counters().max_fd).cast_unsigned(),
        OP_HAS_REACTOR => u64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        _ => return None,
    })
}

/// The server's PHP-facing callbacks.
#[inline(always)]
fn callback(ctx: &Context, op: u32) -> Option<u64> {
    let callbacks = &ctx.state().callbacks;
    let (index, data) = match op {
        OP_ON_CONNECT => (CB_CONNECT, ctx.info.cast()),
        OP_ON_BUFFER_FULL => (CB_BUFFER_FULL, ctx.info.cast()),
        OP_ON_BUFFER_EMPTY => (CB_BUFFER_EMPTY, ctx.info.cast()),
        OP_ON_WORKER_START => (CB_WORKER_START, ctx.worker.cast()),
        OP_ON_WORKER_STOP => (CB_WORKER_STOP, ctx.worker.cast()),
        OP_ON_WORKER_EXIT => {
            // SAFETY: this thread's worker globals.
            let current = unsafe { (*thread_globals()).worker };
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
        callbacks.call_server(index, ctx.server());
    } else {
        // SAFETY: the slot takes the server and the event head or worker.
        unsafe { slot.call(ctx.serv, data) };
    }
    Some(0)
}

/// The effects on the worker, the server and the process.
#[inline(always)]
fn effect(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    match op {
        // SAFETY: Rust's signal handler for the event worker.
        OP_SIGNAL_SET => unsafe { swr_workers_signal_set(arg, entry::worker_signal_handler) },
        // SAFETY: the context's live server and worker.
        OP_STOP_ASYNC => unsafe { entry::stop_async_worker(serv, worker.cast()) },
        OP_WORKER_SHUTDOWN => swr_v1_worker_shutdown(ctx.worker().into()),
        OP_WORKER_STATUS => ctx.worker().set_status(if arg == 0 {
            SW_WORKER_IDLE
        } else {
            SW_WORKER_BUSY
        }),
        OP_REOPEN_LOGGER => {
            if let Some(logger) = swoole_rs_utilities::process_logger() {
                logger.reopen();
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
        OP_CLEAR_TIMER => crate::server_methods::clear_timer(ctx.server()),
        OP_ABORT_WORKER => crate::worker_methods::abort_worker(ctx.server(), ctx.worker().into()),
        OP_RESET_WORKER_COUNTER => {
            crate::worker_methods::reset_worker_counter(ctx.server(), ctx.worker().into());
        }
        // SAFETY: the context's live worker.
        OP_DETACH_WORKER => unsafe { worker_global_detach(worker) },
        OP_WORKER_ENTER => {
            swoole_set_worker_id(ctx.worker().id());
            swoole_set_worker_type(SW_EVENT_WORKER);
            // SAFETY: the context's live server and worker.
            unsafe { crate::worker_methods::init_event_worker(ctx.server(), worker) };
        }
        OP_CPU_AFFINITY =>
        {
            #[cfg(target_os = "linux")]
            if ctx.config().open_cpu_affinity {
                set_cpu_affinity(ctx.config(), ctx.worker().id());
            }
        }
        OP_GLOBAL_HOOK => global_hook(ctx, arg),
        OP_PUSH_STOP_MESSAGE => {
            let msg = WorkerStopMessage {
                pid: std::process::id().cast_signed(),
                worker_id: u16::try_from(ctx.worker().id()).unwrap_or(u16::MAX),
            };
            let pool = ctx.server().workers().event_pool;
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
#[inline(always)]
fn connection(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    if op == OP_CONN_AT {
        let fd = c_int::try_from(arg.cast_signed()).unwrap_or(-1);
        let conn = ctx.server().at(fd);
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
            let removed =
                crate::objects::socket(conn.socket).is_none_or(|s| s.read(|s| s.removed != 0));
            u64::from(conn.peer_closed == 0 && !removed)
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
pub(crate) unsafe extern "C" fn swr_workers_signal_set(arg: u64, own: unsafe extern "C" fn(c_int)) {
    use swoole_rs_reactor::signal_upstream::swoole_signal_set;
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
    _ = unsafe { swoole_signal_set(signo, handler, false) };
}

/// `Worker_global_hook`: the worker start/stop and server shutdown hooks.
fn global_hook(ctx: &Context, hook: u64) {
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
        unsafe { swoole_rs_corebase::swoole_call_hook(id, ctx.serv) };
        return;
    }
    let worker_id = ctx.worker().id() as usize;
    let mut args: [*mut c_void; 2] = [ctx.serv, core::ptr::without_provenance_mut(worker_id)];
    // SAFETY: the worker hooks take { server, worker id }.
    unsafe { swoole_rs_corebase::swoole_call_hook(id, args.as_mut_ptr().cast()) };
}

/// `Worker_set_cpu_affinity`.
#[cfg(target_os = "linux")]
fn set_cpu_affinity(config: &crate::ServerConfig, worker_id: u32) {
    const ORIGIN: &core::ffi::CStr = c"Worker_set_cpu_affinity";
    /// `Server::MODE_THREAD`.
    const MODE_THREAD: u8 = 3;
    let available = (!config.cpu_affinity_available.is_null()).then(|| {
        let len = usize::try_from(config.cpu_affinity_available_num).unwrap_or(0);
        // SAFETY: the server's live CPU list and its length.
        unsafe { core::slice::from_raw_parts(config.cpu_affinity_available, len) }
    });
    let outcome = swoole_rs_process::affinity::worker_affinity(
        worker_id,
        available,
        config.mode_ == MODE_THREAD,
    );
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
