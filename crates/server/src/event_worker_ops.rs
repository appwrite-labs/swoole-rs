//! The event worker's facts and effects (`crates/process/src/workers/event_worker.rs`'s
//! `OP_*`) over the server, worker and connection records Rust lays out.
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::DataHead;
#[cfg(target_os = "linux")]
use swoole_rs_corebase::log;
use swoole_rs_process::methods::{
    push_stop, swoole_get_worker_id, swoole_get_worker_type, swoole_set_worker_id,
    swoole_set_worker_type,
};
use swoole_rs_process::worker_globals::worker_global_detach;
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_shutdown};
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
pub(crate) use native::{
    accept_event, call_worker_error_callback, call_worker_start_callback,
    call_worker_stop_callback, send_to_reactor_thread, start_event_worker, worker_signal_handler,
    worker_start_callback,
};
pub use native::{
    clean_worker_connections, drain_worker_pipe, kill_worker, stop_async_worker, worker_signal_init,
};

/// What an operation runs over.
struct Context<'a> {
    serv: view::Server<'a>,
    worker: *mut WorkerRecord,
    info: *mut DataHead,
    reactor: crate::reactor_handle::Reactor,
    conn: *mut ConnectionRecord,
    status: *const c_void,
    socket: *mut c_void,
}

impl<'a> Context<'a> {
    /// The context's live server.
    fn server(&self) -> view::Server<'a> {
        self.serv
    }
}

/// `swGlobalHookType`'s worker and shutdown hooks.
const SW_GLOBAL_HOOK_BEFORE_WORKER_START: c_int = 2;
const SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN: c_int = 6;
const SW_GLOBAL_HOOK_BEFORE_WORKER_STOP: c_int = 8;
/// `swoole::WorkerType::SW_EVENT_WORKER`.
const SW_EVENT_WORKER: c_int = 2;
/// `SW_WORKER_BUSY`, `SW_WORKER_IDLE`.
const SW_WORKER_BUSY: u8 = 1;
const SW_WORKER_IDLE: u8 = 2;
/// `Server::DISPATCH_CO_CONN_LB`, `DISPATCH_CO_REQ_LB`.
const DISPATCH_CO_CONN_LB: u8 = 8;
const DISPATCH_CO_REQ_LB: u8 = 9;

/// Answers each event-worker op over the context.
impl swoole_rs_process::workers::Answer for Context<'_> {
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
fn fact(ctx: &Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let config = ctx.config();
    Some(match op {
        OP_FACTORY_END => u64::from(crate::factory::end(ctx.state(), ctx.info().fd, 0)),
        OP_MODE => u64::from(config.mode_),
        OP_SIGNAL_LIVE => u64::from(
            swoole_rs_corebase::swr_v1_core_running()
                && !ctx.worker.is_null()
                && ctx.state().running(),
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
        OP_POOL_RUNNING => u64::from(
            crate::shared::ServerShared::of(ctx.server())
                .event_pool()
                .running,
        ),
        OP_MINFD => i64::from(ctx.counters().min_fd()).cast_unsigned(),
        OP_MAXFD => i64::from(ctx.counters().max_fd()).cast_unsigned(),
        OP_HAS_REACTOR => u64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        _ => return None,
    })
}

/// The server's PHP-facing callbacks.
#[inline(always)]
fn callback(ctx: &Context<'_>, op: u32) -> Option<u64> {
    let callbacks = &ctx.state().callbacks;
    let (index, data) = match op {
        OP_ON_CONNECT => (CB_CONNECT, ctx.info.cast()),
        OP_ON_BUFFER_FULL => (CB_BUFFER_FULL, ctx.info.cast()),
        OP_ON_BUFFER_EMPTY => (CB_BUFFER_EMPTY, ctx.info.cast()),
        OP_ON_WORKER_START => (CB_WORKER_START, ctx.worker.cast()),
        OP_ON_WORKER_STOP => (CB_WORKER_STOP, ctx.worker.cast()),
        OP_ON_WORKER_EXIT => {
            let current = swoole_rs_process::worker_globals::current_worker();
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
        unsafe { slot.call(ctx.serv.raw(), data) };
    }
    Some(0)
}

/// The effects on the worker, the server and the process.
#[inline(always)]
fn effect(ctx: &Context<'_>, op: u32, arg: u64) -> Option<u64> {
    match op {
        OP_SIGNAL_SET => swr_workers_signal_set(arg, worker_signal_handler),
        OP_STOP_ASYNC => stop_async_worker(ctx.server(), ctx.worker().into()),
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
        OP_CALL_WORKER_START => call_worker_start_callback(ctx.server(), ctx.worker().into()),
        OP_WORKER_START_CALLBACK => worker_start_callback(ctx.server(), ctx.worker().into()),
        OP_WORKER_STOP_CALLBACK => {
            call_worker_stop_callback(ctx.server(), ctx.worker().into());
        }
        OP_WORKER_SIGNAL_INIT => worker_signal_init(ctx.server()),
        OP_DRAIN_PIPES => drain_worker_pipe(ctx.server()),
        OP_CLEAR_TIMER => crate::server_methods::clear_timer(ctx.server()),
        OP_ABORT_WORKER => crate::worker_methods::abort_worker(ctx.server(), ctx.worker().into()),
        OP_RESET_WORKER_COUNTER => {
            crate::worker_methods::reset_worker_counter(ctx.server(), ctx.worker().into());
        }
        OP_DETACH_WORKER => worker_global_detach(ctx.worker().into()),
        OP_WORKER_ENTER => {
            swoole_set_worker_id(ctx.worker().id());
            swoole_set_worker_type(SW_EVENT_WORKER);
            crate::worker_methods::init_event_worker(ctx.server(), ctx.worker().into());
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
            let sent = push_stop(
                ctx.server().event_pool_ref(),
                std::process::id().cast_signed(),
                u16::try_from(ctx.worker().id()).unwrap_or(u16::MAX),
            );
            return Some(i64::from(sent).cast_unsigned());
        }
        _ => return None,
    }
    Some(0)
}

/// `Server::foreach_connection`'s selected connection.
#[inline(always)]
fn connection(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    if op == OP_CONN_AT {
        let fd = c_int::try_from(arg.cast_signed()).unwrap_or(-1);
        let conn = ctx
            .server()
            .at(fd)
            .filter(|conn| crate::reactor_ops::valid(Some(conn)));
        let valid = conn.is_some();
        ctx.conn = conn.map_or(core::ptr::null_mut(), |conn| &raw mut *conn);
        return Some(u64::from(valid));
    }
    let conn = ctx.conn()?;
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
pub(crate) fn swr_workers_signal_set(arg: u64, own: extern "C" fn(c_int)) {
    use swoole_rs_reactor::signal_upstream::{self, Handler};
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
        0 => Handler::NONE,
        1 => Handler::IGNORE,
        _ => Handler::new(own),
    };
    _ = signal_upstream::set(signo, handler, false);
}

/// `Worker_global_hook`: the worker start/stop and server shutdown hooks.
fn global_hook(ctx: &Context<'_>, hook: u64) {
    let id = match hook {
        HOOK_BEFORE_WORKER_START => SW_GLOBAL_HOOK_BEFORE_WORKER_START,
        HOOK_BEFORE_SERVER_SHUTDOWN => SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN,
        _ => SW_GLOBAL_HOOK_BEFORE_WORKER_STOP,
    };
    if !swoole_rs_corebase::swoole_isset_hook(id) {
        return;
    }
    if hook == HOOK_BEFORE_SERVER_SHUTDOWN {
        ctx.serv.call_hook(view::ServerHook::BeforeShutdown);
        return;
    }
    let worker_id = ctx.worker().id() as usize;
    let mut args: [*mut c_void; 2] = [ctx.serv.raw(), core::ptr::without_provenance_mut(worker_id)];
    // SAFETY: the worker hooks take { server, worker id }.
    unsafe { swoole_rs_corebase::swoole_call_hook(id, args.as_mut_ptr().cast()) };
}

/// `Worker_set_cpu_affinity`.
#[cfg(target_os = "linux")]
fn set_cpu_affinity(config: &crate::ServerConfig, worker_id: u32) {
    const ORIGIN: &core::ffi::CStr = c"Worker_set_cpu_affinity";
    /// `Server::MODE_THREAD`.
    const MODE_THREAD: u8 = 3;
    let available = &config.cpu_affinity_available;
    let available = (!available.is_empty()).then_some(available.as_slice());
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
