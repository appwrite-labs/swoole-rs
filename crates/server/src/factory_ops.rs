//! The process-mode factory's facts and effects (`crates/process/src/workers/factory.rs`'s
//! `OP_*`) over the server, worker, connection and send records Rust lays out.
//! process.cc's `FactoryContext::call` hands every op it has no case for to
//! [`swr_v1_factory_call`].
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicI64, Ordering};

use swoole_rs_corebase::worker_registry::{
    swr_v1_process_worker_registry_register as register,
    swr_v1_process_worker_registry_unregister as unregister,
};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_process::methods::{
    ExitStatus, kill_all_workers, swoole_get_worker_id, swoole_get_worker_type,
    swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type, worker_report_error,
};
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::worker_globals::swr_v1_worker_globals;
use swoole_rs_process::worker_record::{
    WorkerRecord, swr_v1_worker_id, swr_v1_worker_pid, swr_v1_worker_set_pid,
};
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::ops::{
    LOG_ALLOC_CONNECTIONS, LOG_KILL_MANAGER, LOG_WAIT_MANAGER, LOG_WAIT_TASK_WORKER,
    OP_ALLOC_CONNECTIONS, OP_CALL_WORKER_ERROR, OP_CLEAR_CLOSE_ERRNO, OP_CONN_CLOSE_ACTIVELY,
    OP_CONN_CLOSE_FORCE, OP_CONN_CLOSE_RESET, OP_CONN_CLOSED, OP_CONN_CLOSING, OP_CONN_FD,
    OP_CONN_OVERFLOW, OP_CONN_PEER_CLOSED, OP_COUNT_DISPATCH, OP_CURRENT_WORKER_ID,
    OP_DO_CLOSE_CALLBACK, OP_ENTER_CHILD, OP_EXIT, OP_EXIT_STATUS, OP_FORGET_USER_WORKER, OP_FORK,
    OP_FORWARD_CLOSE, OP_FREE_CONNECTIONS, OP_HAS_MESSAGE_BOX, OP_HASH_DISPATCH,
    OP_INIT_IPC_MAX_SIZE, OP_IS_WORKER, OP_KILL_MANAGER, OP_KILL_TASK_POOL, OP_MODE, OP_ON_CLOSE,
    OP_ON_USER_WORKER_START, OP_OUTPUT_BUFFER_SIZE, OP_POOL_MAIN_LOOP, OP_REGISTER_USER_WORKER,
    OP_REGISTER_WORKER, OP_REPORT_ERROR, OP_SCHEDULE_CONN, OP_SCHEDULE_WORKER, OP_SEND_CONN_ACTIVE,
    OP_SEND_FD, OP_SEND_LEN, OP_SEND_TYPE, OP_SEND_YIELD, OP_SESSION_CONN, OP_SET_CLOSE_ACTIVELY,
    OP_SET_CLOSE_RESET, OP_SET_REACTOR_PIPE_NUM, OP_START_EVENT_WORKER, OP_START_MANAGER,
    OP_STREAM_EVENT, OP_TASK_WORKER_NUM, OP_TO_SESSION, OP_WAIT_MANAGER, OP_WAIT_TASK_WORKER,
    OP_WORKER_NUM, OP_WORKER_PID,
};

use crate::base_factory::{SendData, is_stream_event, low16};
use crate::callbacks::{CB_CLOSE, CB_USER_WORKER_START, CallbackSlot};
use crate::records::swr_v1_server_connection;
use crate::{ConnectionRecord, ServerCounters, view};

/// process.cc's `WorkersNative<4>`.
#[repr(C)]
struct Context {
    serv: *mut c_void,
    worker: *mut WorkerRecord,
    send: *mut SendData,
    status: *const ExitStatus,
    conn: *mut ConnectionRecord,
    fd: c_int,
}

const _: () = assert!(
    size_of::<Context>() == 48 && core::mem::offset_of!(Context, fd) == 40,
    "process.cc FactoryContext layout"
);

// SAFETY: crates/reactor's body, under its upstream symbol.
unsafe extern "C-unwind" {
    #[link_name = "_Z14swoole_waitpidiPii"]
    fn swoole_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

/// `SWR_WORKER_EVENT`, `SWR_WORKER_USER`: the registry's categories.
const REGISTRY_EVENT: c_int = 1;
const REGISTRY_USER: c_int = 2;
/// `swoole::WorkerType::SW_EVENT_WORKER`.
const SW_EVENT_WORKER: c_int = 2;
/// `Server::DISPATCH_FDMOD`, `DISPATCH_IPMOD`, `DISPATCH_CO_CONN_LB`.
const DISPATCH_FDMOD: u8 = 2;
const DISPATCH_IPMOD: u8 = 4;
const DISPATCH_CO_CONN_LB: u8 = 8;
/// `SW_SERVER_EVENT_CLOSE`, `SW_PIPE_MASTER`.
const EVENT_CLOSE: u8 = 4;
const PIPE_MASTER: c_int = 1;
/// `swr_v1_server_connection`'s `get_connection_verify_no_ssl` and `get_connection_verify`.
const VERIFIED: u32 = 2;
const VERIFIED_TLS: u32 = 3;

/// The live server's state.
///
/// # Safety
/// `serv` is a live `Server`.
unsafe fn state<'a>(serv: *mut c_void) -> &'a crate::ServerState {
    // SAFETY: per the caller.
    let state = unsafe { view::state_ptr(serv) };
    // SAFETY: the state lives as long as the server.
    unsafe { &*state }
}

/// The live server's callback slot `index`, if it is one.
///
/// # Safety
/// `serv` is a live `Server`.
unsafe fn slot<'a>(serv: *mut c_void, index: u32) -> Option<&'a CallbackSlot> {
    // SAFETY: per the caller.
    unsafe { state(serv) }.callbacks.slot(index)
}

fn signed(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// A worker id from an op's argument, as `Server::get_worker(uint16_t)` narrows it.
fn worker_id(arg: u64) -> u16 {
    u16::try_from(arg & 0xffff).unwrap_or_default()
}

/// Answers factory op `op` with `arg` over the `WorkersNative<4>` at `context`.
///
/// # Safety
/// `context` is process.cc's live `FactoryContext`, whose objects the op reads are live.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_factory_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    // SAFETY: per the caller.
    let answer = unsafe { fact(ctx, op) };
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { connection(ctx, op, arg) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { worker(ctx, op, arg) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { server(ctx, op, arg) });
    answer.unwrap_or_else(|| std::process::abort())
}

/// The facts read from the server config, the data sent and the worker globals.
///
/// # Safety
/// As [`swr_v1_factory_call`].
unsafe fn fact(ctx: &Context, op: u32) -> Option<u64> {
    // SAFETY: the context's live server.
    let config = unsafe { view::config(ctx.serv) };
    let send = || {
        // SAFETY: the ops that read it run with the live data sent.
        unsafe { &(*ctx.send).info }
    };
    Some(match op {
        OP_MODE => u64::from(config.mode_),
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        OP_WORKER_NUM => u64::from(config.worker_num),
        OP_OUTPUT_BUFFER_SIZE => u64::from(config.output_buffer_size),
        OP_SEND_YIELD => u64::from(config.send_yield),
        OP_HASH_DISPATCH => u64::from(matches!(
            config.dispatch_mode,
            DISPATCH_FDMOD | DISPATCH_IPMOD | DISPATCH_CO_CONN_LB
        )),
        OP_SEND_TYPE => u64::from(send().kind),
        OP_SEND_FD => send().fd.cast_unsigned(),
        OP_SEND_LEN => u64::from(send().len),
        OP_STREAM_EVENT => u64::from(is_stream_event(send().kind)),
        OP_CURRENT_WORKER_ID => signed(swoole_get_worker_id().cast_signed()),
        OP_IS_WORKER => u64::from(swoole_get_worker_type() == SW_EVENT_WORKER),
        OP_EXIT_STATUS => {
            // SAFETY: the op runs with the live exit status.
            u64::from(unsafe { &*ctx.status }.status() != 0)
        }
        _ => return None,
    })
}

/// The connection the data sent is for: selected by `OP_SEND_CONN_ACTIVE` or
/// `OP_SESSION_CONN`, then read and updated by the ops after them.
///
/// # Safety
/// As [`swr_v1_factory_call`]; the ops after the selecting one run with its
/// connection.
unsafe fn connection(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    match op {
        OP_SCHEDULE_WORKER => {
            // The task's info.fd is the real fd, not the session id; dispatch converts it.
            // SAFETY: the op runs with the live data sent.
            let fd = unsafe { (*ctx.send).info.fd };
            ctx.fd = c_int::from_ne_bytes(low32(fd.cast_unsigned()).to_ne_bytes());
            // SAFETY: the live server and the data sent.
            let worker =
                unsafe { crate::server_methods::schedule_worker(serv, ctx.fd, ctx.send.cast()) };
            return Some(signed(worker));
        }
        OP_SEND_CONN_ACTIVE => {
            // SAFETY: the live server.
            let conn = unsafe { view::at(serv, ctx.fd) };
            ctx.conn = conn.map_or(core::ptr::null_mut(), |conn| &raw mut *conn);
            // SAFETY: null or a slot of the live connection table.
            return Some(u64::from(
                unsafe { ctx.conn.as_ref() }.is_some_and(|c| c.active != 0),
            ));
        }
        OP_SESSION_CONN => {
            let kind = if arg != 0 { VERIFIED } else { VERIFIED_TLS };
            // SAFETY: the op runs with the live data sent.
            let session = unsafe { (*ctx.send).info.fd };
            // SAFETY: the live server.
            let state = unsafe { view::state_ptr(serv) };
            // SAFETY: its live state and tables.
            ctx.conn = unsafe { swr_v1_server_connection(state, session, kind) };
            return Some(u64::from(!ctx.conn.is_null()));
        }
        _ => {}
    }
    // SAFETY: null or the connection selected above.
    let conn = unsafe { ctx.conn.as_mut() }?;
    let send = || {
        // SAFETY: the ops that read it run with the live data sent.
        unsafe { &mut (*ctx.send).info }
    };
    Some(match op {
        OP_CONN_CLOSED => u64::from(conn.closed.load(Ordering::Relaxed)),
        OP_CONN_CLOSE_FORCE => u64::from(conn.close_force),
        OP_CONN_PEER_CLOSED => u64::from(conn.peer_closed),
        OP_CONN_OVERFLOW => u64::from(conn.overflow),
        OP_CONN_CLOSE_ACTIVELY => u64::from(conn.close_actively),
        OP_CONN_CLOSING => u64::from(conn.closing),
        OP_CONN_CLOSE_RESET => u64::from(conn.close_reset),
        OP_CONN_FD => signed(conn.fd),
        OP_TO_SESSION => {
            // converted fd to session_id
            let info = send();
            info.fd = conn.session_id;
            info.server_fd = low16(conn.server_fd);
            0
        }
        OP_SCHEDULE_CONN => {
            // SAFETY: the live server.
            signed(unsafe {
                crate::server_methods::schedule_worker(serv, conn.fd, core::ptr::null_mut())
            })
        }
        OP_SET_CLOSE_RESET => {
            conn.close_reset = 1;
            0
        }
        OP_SET_CLOSE_ACTIVELY => {
            conn.close_actively = 1;
            0
        }
        OP_CLEAR_CLOSE_ERRNO => {
            conn.close_errno = 0;
            0
        }
        OP_DO_CLOSE_CALLBACK => {
            // `Server::if_do_close_callback`.
            // SAFETY: the live server.
            let set = unsafe { slot(serv, CB_CLOSE) }.is_some_and(CallbackSlot::is_set);
            let won = || {
                conn.closed
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            };
            u64::from(set && won())
        }
        OP_ON_CLOSE => {
            let mut info = DataHead {
                fd: send().fd,
                reactor_id: if conn.close_actively != 0 {
                    -1
                } else {
                    i16::from(conn.reactor_id)
                },
                server_fd: low16(conn.server_fd),
                ..DataHead::default()
            };
            conn.closing = 1;
            // SAFETY: the live server.
            if let Some(slot) = unsafe { slot(serv, CB_CLOSE) } {
                // SAFETY: onClose takes the server and the close event.
                unsafe { slot.call(serv, (&raw mut info).cast()) };
            }
            conn.closing = 0;
            0
        }
        OP_FORWARD_CLOSE => {
            let ev = DataHead {
                kind: EVENT_CLOSE,
                fd: send().fd,
                reactor_id: i16::from(conn.reactor_id),
                ..DataHead::default()
            };
            // SAFETY: the live server.
            let target = unsafe { view::worker(serv, worker_id(arg)) };
            let data = (&raw const ev).cast::<u8>();
            // SAFETY: a worker of the live server and a head of the size passed.
            let sent = unsafe {
                entry::send_to_worker_from_worker(
                    serv,
                    target,
                    data,
                    size_of::<DataHead>(),
                    PIPE_MASTER,
                )
            };
            signed(i64::try_from(sent).unwrap_or(-1))
        }
        _ => return None,
    })
}

/// The low 32 bits, as native's `int` conversion keeps them.
fn low32(value: u64) -> u32 {
    u32::try_from(value & u64::from(u32::MAX)).unwrap_or_default()
}

/// Forking, registering and running event and user workers.
///
/// # Safety
/// As [`swr_v1_factory_call`].
unsafe fn worker(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    // SAFETY: the live server's config.
    let registry = unsafe { view::config(serv) }.worker_registry.cast();
    // The pid or worker type in the argument, as native's `int` conversion keeps it.
    let int = c_int::from_ne_bytes(low32(arg).to_ne_bytes());
    let pid = int;
    match op {
        OP_FORK => return Some(signed(swoole_rs_corebase::swoole_fork(0))),
        OP_EXIT => {
            // SAFETY: exit(3), as upstream.
            unsafe { libc::exit(0) }
        }
        // SAFETY: the context's live worker.
        OP_WORKER_PID => return Some(signed(unsafe { swr_v1_worker_pid(worker) })),
        OP_REGISTER_WORKER => {
            // SAFETY: the context's live worker.
            unsafe { swr_v1_worker_set_pid(worker, pid) };
            // SAFETY: the server's live registry.
            unsafe { register(registry, pid, worker.cast(), REGISTRY_EVENT) };
        }
        OP_REGISTER_USER_WORKER => {
            // worker: local memory; user_workers: shared memory
            // SAFETY: the context's live worker.
            unsafe { swr_v1_worker_set_pid(worker, pid) };
            // SAFETY: as above.
            let id = worker_id(u64::from(unsafe { swr_v1_worker_id(worker) }));
            // SAFETY: the live server.
            let shared = unsafe { view::worker(serv, id) };
            // SAFETY: the worker's shared copy.
            unsafe { swr_v1_worker_set_pid(shared, pid) };
            // SAFETY: the server's live registry.
            unsafe { register(registry, pid, worker.cast(), REGISTRY_USER) };
        }
        OP_FORGET_USER_WORKER => {
            // SAFETY: the server's registry.
            let _present = unsafe { unregister(registry, pid) };
        }
        OP_ENTER_CHILD => {
            // SAFETY: a plain syscall.
            let own = unsafe { libc::getpid() };
            // SAFETY: the context's live worker.
            unsafe { swr_v1_worker_set_pid(worker, own) };
            // SAFETY: as above.
            swoole_set_worker_id(unsafe { swr_v1_worker_id(worker) });
            // SAFETY: as above.
            swoole_set_worker_pid(unsafe { swr_v1_worker_pid(worker) });
            swoole_set_worker_type(int);
            // SAFETY: this thread's worker globals.
            unsafe { (*swr_v1_worker_globals()).worker = worker };
        }
        OP_POOL_MAIN_LOOP => {
            type MainLoop =
                unsafe extern "C-unwind" fn(*mut PoolRecord, *mut WorkerRecord) -> c_int;
            // SAFETY: the live server.
            let pool = unsafe { view::workers(serv) }.event_pool;
            // SAFETY: its live event pool.
            let main_loop = unsafe { (*pool).main_loop };
            // SAFETY: the pool's `main_loop`, set before its workers start.
            let main_loop = unsafe { core::mem::transmute::<*const c_void, MainLoop>(main_loop) };
            // SAFETY: the pool and one of its workers.
            let _code = unsafe { main_loop(pool, worker) };
        }
        OP_START_EVENT_WORKER => {
            // SAFETY: the context's live server and worker.
            let _code = unsafe { entry::start_event_worker(serv, worker.cast()) };
        }
        OP_ON_USER_WORKER_START => {
            // SAFETY: the live server.
            if let Some(slot) = unsafe { slot(serv, CB_USER_WORKER_START) } {
                // SAFETY: onUserWorkerStart takes the server and the worker.
                unsafe { slot.call(serv, worker.cast()) };
            }
        }
        // SAFETY: the context's live worker and exit status.
        OP_REPORT_ERROR => unsafe { worker_report_error(worker, ctx.status) },
        OP_CALL_WORKER_ERROR => {
            // SAFETY: the context's live server, worker and exit status.
            unsafe { entry::call_worker_error_callback(serv, worker.cast(), ctx.status.cast()) };
        }
        OP_COUNT_DISPATCH => {
            // SAFETY: the live server.
            let target = unsafe { view::worker(serv, worker_id(arg)) };
            // SAFETY: a worker of the live server.
            let count = unsafe { &raw mut (*target).dispatch_count };
            // SAFETY: its counter, shared and updated only atomically.
            let count = unsafe { AtomicI64::from_ptr(count) };
            count.fetch_add(1, Ordering::SeqCst);
        }
        _ => return None,
    }
    Some(0)
}

/// The server's setup and teardown: connections, the manager and the task workers.
///
/// # Safety
/// As [`swr_v1_factory_call`].
unsafe fn server(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: the live server.
    let state = unsafe { state(serv) };
    // SAFETY: its table view, written by the master before workers start.
    let tables = unsafe { &mut *state.tables() };
    // SAFETY: the live server's config.
    let config = unsafe { view::config(serv) };
    // SAFETY: the server's shared counters, mapped for its life.
    let gs = unsafe { &*config.gs.cast::<ServerCounters>() };
    let task_worker_pid = || {
        // SAFETY: the live server.
        let pool = unsafe { view::task_pool(serv) };
        // SAFETY: its task pool.
        let pool = unsafe { &*pool };
        let index = usize::try_from(arg).unwrap_or(usize::MAX);
        // SAFETY: a task worker of it; arg is below task_worker_num.
        unsafe { swr_v1_worker_pid(pool.workers.wrapping_add(index)) }
    };
    let waitpid = |pid| {
        let mut status = 0;
        // SAFETY: a local status.
        signed(unsafe { swoole_waitpid(pid, &raw mut status, 0) })
    };
    Some(match op {
        OP_ALLOC_CONNECTIONS => {
            let size = size_of::<ConnectionRecord>();
            let list =
                swoole_rs_memory::members::sw_shm_calloc(tables.max_connection as usize, size);
            tables.connection_list = list.cast();
            u64::from(!list.is_null())
        }
        LOG_ALLOC_CONNECTIONS => {
            let (max, size) = (tables.max_connection, size_of::<ConnectionRecord>());
            let m = format_args!("sw_shm_calloc({max}, {size}) for connection_list failed");
            log::sys_warning(c"Server::create_process_factory", m);
            0
        }
        OP_FREE_CONNECTIONS => {
            swoole_rs_memory::members::sw_shm_free(tables.connection_list.cast());
            0
        }
        OP_SET_REACTOR_PIPE_NUM => {
            let num = config.worker_num / u32::from(config.reactor_num);
            tables.reactor_pipe_num = u16::try_from(num & 0xffff).unwrap_or_default();
            0
        }
        OP_HAS_MESSAGE_BOX => {
            // SAFETY: the live server.
            let pool = unsafe { view::workers(serv) }.event_pool;
            // SAFETY: its event pool.
            u64::from(!unsafe { (*pool).message_box }.is_null())
        }
        OP_INIT_IPC_MAX_SIZE => {
            // SAFETY: the live server.
            unsafe { crate::worker_methods::init_ipc_max_size(serv) };
            0
        }
        // SAFETY: as above.
        OP_START_MANAGER => signed(unsafe { entry::start_manager_process(serv) }),
        OP_KILL_MANAGER => {
            // SAFETY: kill has no memory preconditions.
            signed(unsafe { libc::kill(gs.manager_pid, libc::SIGTERM) })
        }
        LOG_KILL_MANAGER => {
            let pid = gs.manager_pid;
            log::sys_warning(
                c"ProcessFactory::shutdown",
                format_args!("kill({pid}) failed"),
            );
            0
        }
        OP_WAIT_MANAGER => waitpid(gs.manager_pid),
        LOG_WAIT_MANAGER => {
            let pid = gs.manager_pid;
            log::sys_warning(
                c"ProcessFactory::shutdown",
                format_args!("waitpid({pid}) failed"),
            );
            0
        }
        OP_KILL_TASK_POOL => {
            // SAFETY: the live server.
            let pool = unsafe { view::task_pool(serv) };
            // SAFETY: its task pool.
            unsafe { kill_all_workers(pool, libc::SIGTERM) };
            0
        }
        OP_WAIT_TASK_WORKER => waitpid(task_worker_pid()),
        LOG_WAIT_TASK_WORKER => {
            let pid = task_worker_pid();
            log::sys_warning(
                c"Factory::kill_task_workers",
                format_args!("waitpid({pid}) failed"),
            );
            0
        }
        _ => return None,
    })
}
