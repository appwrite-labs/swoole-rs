//! Thread mode's facts and effects (`crates/process/src/workers/thread.rs`'s `OP_*`)
//! over the server, pool, worker and port records Rust lays out. process.cc's
//! `ThreadContext::call` hands every op it has no case for to
//! [`swr_v1_thread_mode_call`].
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::{sw_calloc, sw_free};
use swoole_rs_process::methods::{start_check, swoole_get_worker_type};
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_id, swr_v1_worker_type};
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::factory_methods::create_worker_pipes;
use swoole_rs_process::workers::ops::{
    OP_ABORT, OP_ALL_CORE_WORKER_NUM, OP_ALL_WORKER_NUM, OP_CALLOC_CONNECTIONS,
    OP_CREATE_EVENT_WORKERS, OP_CREATE_USER_WORKERS, OP_CREATE_WORKER_PIPES,
    OP_FREE_THREAD_CONNECTIONS, OP_GET_WORKER, OP_IS_MANAGER, OP_KILL_WORKER, OP_MANAGER_THREAD_ID,
    OP_ON_AFTER_RELOAD, OP_ON_BEFORE_RELOAD, OP_ON_MANAGER_START, OP_ON_MANAGER_STOP,
    OP_ON_USER_WORKER_START, OP_ON_WORKER_START, OP_ON_WORKER_STOP, OP_POOL_MAIN_LOOP, OP_PORT_ADD,
    OP_PORT_COUNT, OP_PORT_DGRAM, OP_PORT_LISTEN, OP_RUNNING, OP_SINGLE_REACTOR_PIPE,
    OP_STORE_LISTEN_SOCKET, OP_TASK_START_CHECK, OP_TASK_WORKER_NUM, OP_TERMINATE_MANAGER_THREAD,
    OP_THREAD_REACTOR_NUM, OP_USER_WORKER_NUM, OP_WORKER_ID, OP_WORKER_NUM, OP_WORKER_TYPE,
};
use swoole_rs_reactor::{SocketRecord, reactor_upstream};

use crate::callbacks::{
    CB_AFTER_RELOAD, CB_BEFORE_RELOAD, CB_MANAGER_START, CB_MANAGER_STOP, CB_USER_WORKER_START,
};
use crate::reactor_ops::sign;
use crate::reactor_process_ops::{port_listen, ports};
use crate::records::ConnectionRecord;
use crate::worker_methods::{create_event_workers, create_user_workers};
use crate::{server_methods, view};

/// process.cc's `WorkersNative<5>`.
#[repr(C)]
struct Context {
    serv: *mut c_void,
    factory: *mut c_void,
    worker: *mut WorkerRecord,
}

/// `SW_MANAGER` and `SW_EVENT_READ`.
const SW_MANAGER: c_int = 3;
const SW_EVENT_READ: c_int = 1 << 9;

/// A task pool's `onWorkerStart` and `onWorkerStop`, and `main_loop`, whose result
/// thread mode ignores.
type PoolHook = unsafe extern "C-unwind" fn(*mut PoolRecord, *mut WorkerRecord);

/// Answers thread-mode op `op` with `arg` over the `WorkersNative<5>` at `context`.
///
/// # Safety
/// `context` is process.cc's live `ThreadContext`, whose objects the op reads are live.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_thread_mode_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    // SAFETY: per the caller.
    let answer = unsafe { server(ctx, op, arg) };
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { worker(ctx, op, arg) });
    answer.unwrap_or_else(|| std::process::abort())
}

/// The facts and effects on the server, its tables and its ports.
///
/// # Safety
/// As [`swr_v1_thread_mode_call`].
unsafe fn server(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: the context's live server.
    let config = unsafe { view::config(serv) };
    let tables = || {
        // SAFETY: as above.
        let state = unsafe { view::state_ptr(serv) };
        // SAFETY: its state, which lives as long as it.
        let tables = unsafe { (*state).tables() };
        // SAFETY: its tables, which live as long as it.
        unsafe { &mut *tables }
    };
    let port = || {
        // SAFETY: as above.
        let ports = unsafe { ports(serv) };
        usize::try_from(arg)
            .ok()
            .and_then(|i| ports.get(i).copied())
    };
    let call_server = |index| {
        // SAFETY: as above.
        let state = unsafe { view::state_ptr(serv) };
        // SAFETY: its state, whose `(Server *)` slot `index` is.
        if let Some(slot) = unsafe { &(*state).callbacks }.slot(index) {
            // SAFETY: as above.
            unsafe { slot.call_server(serv) };
        }
        0
    };
    Some(match op {
        OP_MANAGER_THREAD_ID | OP_ALL_WORKER_NUM => {
            // SAFETY: as above.
            let workers = unsafe { view::workers(serv) };
            u64::from(workers.event_count)
                + u64::from(workers.task_count)
                + workers.user_count as u64
        }
        OP_ALL_CORE_WORKER_NUM => u64::from(config.worker_num) + u64::from(config.task_worker_num),
        OP_WORKER_NUM => u64::from(config.worker_num),
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        // SAFETY: as above.
        OP_USER_WORKER_NUM => (unsafe { view::user_worker_num(serv) }) as u64,
        OP_RUNNING => u64::from(config.running),
        OP_IS_MANAGER => u64::from(swoole_get_worker_type() == SW_MANAGER),
        OP_THREAD_REACTOR_NUM => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "`serv->reactor_num = serv->worker_num`, as upstream converts it"
            )]
            let reactor_num = config.worker_num as u16;
            config.reactor_num = reactor_num;
            0
        }
        OP_SINGLE_REACTOR_PIPE => {
            tables().reactor_pipe_num = 1;
            0
        }
        OP_CALLOC_CONNECTIONS => {
            let tables = tables();
            let list = sw_calloc(
                tables.max_connection as usize,
                size_of::<ConnectionRecord>(),
            );
            tables.connection_list = list.cast();
            u64::from(!list.is_null())
        }
        OP_FREE_THREAD_CONNECTIONS => {
            // SAFETY: the list OP_CALLOC_CONNECTIONS allocated, or null.
            unsafe { sw_free(tables().connection_list.cast()) };
            0
        }
        // SAFETY: the live server.
        OP_CREATE_WORKER_PIPES => u64::from(unsafe { create_worker_pipes(serv) }),
        // SAFETY: as above.
        OP_CREATE_EVENT_WORKERS => u64::from(unsafe { create_event_workers(serv) }),
        // SAFETY: as above.
        OP_CREATE_USER_WORKERS => u64::from(unsafe { create_user_workers(serv) }),
        OP_TASK_START_CHECK => {
            // SAFETY: the live server.
            let pool = unsafe { view::task_pool(serv) };
            // SAFETY: its live task pool.
            sign(unsafe { start_check(pool) })
        }
        OP_STORE_LISTEN_SOCKET => {
            // SAFETY: the live server.
            unsafe { server_methods::store_listen_socket(serv) };
            0
        }
        // SAFETY: as above.
        OP_PORT_COUNT => unsafe { ports(serv) }.len() as u64,
        // SAFETY: one of the server's live ports.
        OP_PORT_DGRAM => u64::from(matches!(unsafe { view::port(port()?) }.r#type, 2 | 4 | 6)),
        // SAFETY: as above.
        OP_PORT_LISTEN => sign(unsafe { port_listen(port()?) }),
        OP_PORT_ADD => {
            // SAFETY: as above.
            let socket = unsafe { view::port(port()?) }.socket.cast::<SocketRecord>();
            let reactor = swoole_rs_corebase::thread_reactor();
            // SAFETY: this thread's live reactor and the port's live socket.
            unsafe { reactor_upstream::reactor_change(reactor, socket, 0, SW_EVENT_READ) };
            0
        }
        OP_ON_MANAGER_START => call_server(CB_MANAGER_START),
        OP_ON_MANAGER_STOP => call_server(CB_MANAGER_STOP),
        OP_ON_BEFORE_RELOAD => call_server(CB_BEFORE_RELOAD),
        OP_ON_AFTER_RELOAD => call_server(CB_AFTER_RELOAD),
        OP_ABORT => std::process::abort(),
        _ => return None,
    })
}

/// The facts and effects on the context's worker and the thread manager.
///
/// # Safety
/// As [`swr_v1_thread_mode_call`].
unsafe fn worker(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    let pool_hook = |hook: fn(&PoolRecord) -> *const c_void| {
        // SAFETY: the live server's task pool.
        let pool = unsafe { view::task_pool(serv) };
        // SAFETY: as above.
        let hook = hook(unsafe { &*pool });
        // SAFETY: the pool's hook is null or a `(ProcessPool *, Worker *)` function.
        let hook = unsafe { core::mem::transmute::<*const c_void, Option<PoolHook>>(hook) };
        if let Some(hook) = hook {
            // SAFETY: the pool and the context's live worker, as the hook takes them.
            unsafe { hook(pool, worker) };
        }
        0
    };
    Some(match op {
        // SAFETY: the context's live worker.
        OP_WORKER_ID => u64::from(unsafe { swr_v1_worker_id(worker) }),
        // SAFETY: as above.
        OP_WORKER_TYPE => u64::from(unsafe { swr_v1_worker_type(worker) }),
        OP_GET_WORKER => {
            let id = u16::try_from(arg).ok()?;
            // SAFETY: the live server.
            ctx.worker = unsafe { view::worker(serv, id) };
            ctx.worker.addr() as u64
        }
        OP_KILL_WORKER => {
            let id = c_int::try_from(arg.cast_signed()).ok()?;
            // SAFETY: as above.
            let _ = unsafe { entry::kill_worker(serv, id) };
            0
        }
        OP_TERMINATE_MANAGER_THREAD => {
            // SAFETY: the context's live ThreadFactory.
            unsafe { entry::terminate_manager_thread(ctx.factory) };
            0
        }
        OP_ON_USER_WORKER_START => {
            // SAFETY: the live server's state.
            let state = unsafe { view::state_ptr(serv) };
            // SAFETY: its `(Server *, Worker *)` slot.
            if let Some(slot) = unsafe { &(*state).callbacks }.slot(CB_USER_WORKER_START) {
                // SAFETY: as above, with the context's live worker.
                unsafe { slot.call(serv, worker.cast()) };
            }
            0
        }
        OP_ON_WORKER_START => pool_hook(|pool| pool.on_worker_start),
        OP_POOL_MAIN_LOOP => pool_hook(|pool| pool.main_loop),
        OP_ON_WORKER_STOP => pool_hook(|pool| pool.on_worker_stop),
        _ => return None,
    })
}
