//! The process-mode factory's facts and effects (`crates/process/src/workers/factory.rs`'s
//! `OP_*`) over the server, worker, connection and send records Rust lays out, which
//! [`swr_v1_factory_call`] answers.
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicI64, Ordering};
use swoole_rs_process::worker_record::Worker;

use swoole_rs_corebase::{DataHead, log};
use swoole_rs_process::methods::{
    ExitStatus, kill_all_workers, swoole_get_worker_id, swoole_get_worker_type,
    swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type, worker_report_error,
};
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::worker_globals::thread_globals;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::ops::{
    LOG_ALLOC_CONNECTIONS, LOG_KILL_MANAGER, LOG_WAIT_MANAGER, LOG_WAIT_TASK_WORKER,
    OP_ALLOC_CONNECTIONS, OP_CALL_WORKER_ERROR, OP_CLEAR_CLOSE_ERRNO, OP_CONN_CLOSE_ACTIVELY,
    OP_CONN_CLOSE_FORCE, OP_CONN_CLOSE_RESET, OP_CONN_CLOSED, OP_CONN_CLOSING, OP_CONN_FD,
    OP_CONN_OVERFLOW, OP_CONN_PEER_CLOSED, OP_COUNT_DISPATCH, OP_CREATE_PIPE_BUFFERS,
    OP_CREATE_WORKER_PIPE, OP_CURRENT_WORKER_ID, OP_DELETE_REACTOR_THREADS, OP_DESTROY_BARRIERS,
    OP_DESTROY_MESSAGE_BOX, OP_DO_CLOSE_CALLBACK, OP_ENTER_CHILD, OP_EXIT, OP_EXIT_STATUS,
    OP_FINISH_CLOSE, OP_FORGET_USER_WORKER, OP_FORK, OP_FORWARD_CLOSE, OP_FREE_CONNECTIONS,
    OP_HAS_MESSAGE_BOX, OP_HASH_DISPATCH, OP_INIT_BARRIERS, OP_INIT_IPC_MAX_SIZE, OP_IS_WORKER,
    OP_KILL_MANAGER, OP_KILL_TASK_POOL, OP_MODE, OP_NEW_PROCESS_FACTORY, OP_NEW_REACTOR_THREADS,
    OP_ON_CLOSE, OP_ON_USER_WORKER_START, OP_OUTPUT_BUFFER_SIZE, OP_POOL_MAIN_LOOP,
    OP_REGISTER_USER_WORKER, OP_REGISTER_WORKER, OP_REPORT_ERROR, OP_SCHEDULE_CONN,
    OP_SCHEDULE_WORKER, OP_SEND_CONN_ACTIVE, OP_SEND_FD, OP_SEND_LEN, OP_SEND_TYPE, OP_SEND_YIELD,
    OP_SESSION_CONN, OP_SET_CLOSE_ACTIVELY, OP_SET_CLOSE_RESET, OP_SET_REACTOR_PIPE_NUM,
    OP_START_EVENT_WORKER, OP_START_MANAGER, OP_STREAM_EVENT, OP_TASK_WORKER_NUM, OP_TO_SESSION,
    OP_WAIT_MANAGER, OP_WAIT_TASK_WORKER, OP_WORKER_NUM, OP_WORKER_PID, OP_WRITE_TO_REACTOR,
    OP_WRITE_TO_WORKER,
};
use swoole_rs_reactor::SocketRecord;

use crate::base_factory::{SendData, is_stream_event, low16};
use crate::callbacks::{CB_CLOSE, CB_USER_WORKER_START, CallbackSlot};
use crate::reactor_ops::ThreadRecord;
use crate::records::{VERIFIED, VERIFIED_TLS};
use crate::{ConnectionRecord, ServerCounters, view};

/// The context crates/process's factory methods run over (its `FactoryContext`).
#[repr(C)]
struct Context {
    serv: *mut c_void,
    worker: *mut WorkerRecord,
    send: *mut SendData,
    status: *const ExitStatus,
    conn: *mut ConnectionRecord,
    fd: c_int,
}

impl Context {
    /// The context's live server.
    fn server(&self) -> view::Server<'_> {
        // SAFETY: the method that built the context lent its live server for the call.
        unsafe { view::Server::from_raw(self.serv) }
    }
}

const _: () = assert!(
    size_of::<Context>() == 48 && core::mem::offset_of!(Context, fd) == 40,
    "crates/process FactoryContext layout"
);

use swoole_rs_reactor::wait_upstream::swoole_waitpid;

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

/// A `Context` is only ever seen through [`swr_v1_factory_call`], whose caller keeps
/// its server, and whichever of its worker, send and status the op reads, live for
/// the call.
#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written under the server's own synchronization as workers.cc did"
)]
impl Context {
    fn state(&self) -> &crate::ServerState {
        self.server().state()
    }
    fn config(&self) -> &mut crate::ServerConfig {
        // SAFETY: the context's live server.
        unsafe { self.server().config_mut() }
    }
    fn tables(&self) -> &mut crate::ServerTables {
        // SAFETY: its tables, written by the master before its threads and workers start.
        unsafe { &mut *self.state().tables() }
    }
    fn workers(&self) -> swoole_rs_process::server_workers::ServerWorkers {
        self.server().workers()
    }
    fn task_pool(&self) -> *mut PoolRecord {
        self.server().task_pool()
    }
    /// Worker `id` of the server, which every caller passes with a worker id it has.
    fn worker_at(&self, id: u16) -> &mut WorkerRecord {
        let worker = self.server().worker(id);
        // SAFETY: one of its workers.
        unsafe { &mut *worker }
    }
    /// The data sent, for the ops that run with it.
    fn send(&self) -> &mut SendData {
        // SAFETY: the ops that read it run with the live data sent.
        unsafe { &mut *self.send }
    }
    fn slot(&self, index: u32) -> Option<&CallbackSlot> {
        self.state().callbacks.slot(index)
    }
}

fn signed(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// A worker id from an op's argument, as `Server::get_worker(uint16_t)` narrows it.
fn worker_id(arg: u64) -> u16 {
    u16::try_from(arg & 0xffff).unwrap_or_default()
}

/// Answers factory op `op` with `arg` over the context at `context`.
///
/// # Safety
/// `context` is crates/process's live `FactoryContext`, whose objects the op reads are live:
/// the ops after `OP_SEND_CONN_ACTIVE` or `OP_SESSION_CONN` run with its connection.
/// Only this makes a [`Context`], so the stages below take its proof with it.
pub(crate) unsafe extern "C-unwind" fn swr_v1_factory_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller; the stages below read the context under this proof.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    let answer = fact(ctx, op);
    let answer = answer.or_else(|| connection(ctx, op, arg));
    let answer = answer.or_else(|| worker(ctx, op, arg));
    let answer = answer.or_else(|| server(ctx, op, arg));
    let answer = answer.or_else(|| native(ctx, op, arg));
    answer.unwrap_or_else(|| std::process::abort())
}

/// The reactor threads, barriers, pipes and message box the factory makes and
/// frees, and its writes to the workers and reactor threads.
fn native(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    let (state, tables, config) = (ctx.state(), ctx.tables(), ctx.config());
    let barriers = |init: bool| {
        for (manager, count) in [(false, c_int::from(config.reactor_num) + 1), (true, 2)] {
            // SAFETY: the server's reactor-thread and (shared) manager barriers.
            let barrier = unsafe { crate::server_object::barrier(serv, manager) };
            if init {
                swoole_rs_locks::barrier_init(barrier, manager, count);
            } else {
                swoole_rs_locks::barrier_destroy(barrier);
            }
        }
        0
    };
    let send = || (ctx.send().info, ctx.send().data);
    Some(match op {
        OP_NEW_REACTOR_THREADS => {
            tables.reactor_threads = new_reactor_threads(state, config.reactor_num);
            0
        }
        OP_DELETE_REACTOR_THREADS => {
            // SAFETY: the array `OP_NEW_REACTOR_THREADS` made, not used again.
            unsafe {
                ThreadRecord::delete_array(tables.reactor_threads, config.reactor_num.into());
            }
            0
        }
        OP_INIT_BARRIERS => barriers(true),
        OP_DESTROY_BARRIERS => barriers(false),
        OP_NEW_PROCESS_FACTORY => {
            crate::factory::new(serv, core::ptr::null_mut()).expose_provenance() as u64
        }
        OP_DESTROY_MESSAGE_BOX => {
            // A shared mapping crates/server's new_message_box made.
            crate::pool_native::free_message_box(ctx.workers().event_pool);
            0
        }
        OP_CREATE_WORKER_PIPE => u64::from(state.push_worker_pipe(ctx.worker_at(worker_id(arg)))),
        OP_CREATE_PIPE_BUFFERS => signed(crate::master_ops::create_pipe_buffers(ctx.server())),
        OP_WRITE_TO_WORKER => {
            // This reactor thread's bus, and its copy of the worker's pipe.
            let thread = tables
                .reactor_threads
                .cast::<ThreadRecord>()
                .wrapping_add(usize::from(swoole_rs_corebase::thread_id()));
            // SAFETY: this reactor thread's live record.
            let bus = unsafe { &*thread }.message_bus();
            // The worker the factory scheduled.
            let pipe = ctx.worker_at(worker_id(arg)).pipe_master;
            // SAFETY: the worker's live pipe socket.
            let fd = unsafe { (*pipe.cast::<SocketRecord>()).fd };
            let socket = bus.pipe_fd(fd);
            let (mut info, data) = send();
            // SAFETY: the live pipe, and the data sent.
            u64::from(unsafe { bus.rust().send(socket.cast(), &mut info, data.cast()) })
        }
        OP_WRITE_TO_REACTOR => {
            let (mut info, data) = send();
            // SAFETY: the op runs with the session's connection selected.
            info.reactor_id = i16::from(unsafe { &*ctx.conn }.reactor_id);
            info.server_fd = low16(swoole_get_worker_id().cast_signed());
            // `serv->get_reactor_pipe_socket(info.fd, info.reactor_id)`.
            let pipes = i64::from(tables.reactor_pipe_num);
            let id = i64::from(info.reactor_id) + (info.fd % pipes) * i64::from(config.reactor_num);
            // The worker serving that reactor's pipe.
            let pipe = ctx.worker_at(worker_id(id.cast_unsigned())).pipe_worker;
            let bus = crate::master_ops::server_message_bus(state);
            // SAFETY: the live pipe, and the data sent.
            u64::from(unsafe { bus.rust().send(pipe.cast(), &mut info, data.cast()) })
        }
        OP_FINISH_CLOSE => u64::from(crate::factory::finish(state, ctx.send.cast())),
        _ => return None,
    })
}

/// `new ReactorThread[count]()`, each bus over the allocator of the server's.
pub(crate) fn new_reactor_threads(state: &crate::ServerState, count: u16) -> *mut c_void {
    ThreadRecord::new_array(
        count.into(),
        crate::master_ops::server_message_bus(state).allocator(),
    )
}

/// The facts read from the server config, the data sent and the worker globals.
fn fact(ctx: &Context, op: u32) -> Option<u64> {
    let config = ctx.config();
    let send = || &ctx.send().info;
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
fn connection(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    match op {
        OP_SCHEDULE_WORKER => {
            // The task's info.fd is the real fd, not the session id; dispatch converts it.
            let fd = ctx.send().info.fd;
            ctx.fd = c_int::from_ne_bytes(low32(fd.cast_unsigned()).to_ne_bytes());
            // SAFETY: the live server and the data sent.
            let worker =
                unsafe { crate::server_methods::schedule_worker(serv, ctx.fd, ctx.send.cast()) };
            return Some(signed(worker));
        }
        OP_SEND_CONN_ACTIVE => {
            ctx.conn = ctx.state().connection_ptr(ctx.fd.into(), 0);
            // SAFETY: null or a slot of the live connection table.
            return Some(u64::from(
                unsafe { ctx.conn.as_ref() }.is_some_and(|c| c.active != 0),
            ));
        }
        OP_SESSION_CONN => {
            let kind = if arg != 0 { VERIFIED } else { VERIFIED_TLS };
            let session = ctx.send().info.fd;
            ctx.conn = ctx.state().connection_ptr(session, kind);
            return Some(u64::from(!ctx.conn.is_null()));
        }
        _ => {}
    }
    // SAFETY: null or the connection selected above.
    let conn = unsafe { ctx.conn.as_mut() }?;
    let send = || &mut ctx.send().info;
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
            let set = ctx.slot(CB_CLOSE).is_some_and(CallbackSlot::is_set);
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
            if let Some(slot) = ctx.slot(CB_CLOSE) {
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
            let target = ctx.worker_at(worker_id(arg));
            let data = (&raw const ev).cast::<u8>();
            // SAFETY: a worker of the live server and a head of the size passed.
            let sent = unsafe {
                swoole_rs_process::methods::send_pipe_message(
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
fn worker(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    let registry = ctx
        .config()
        .worker_registry
        .cast::<swoole_rs_corebase::worker_registry::WorkerRegistry>();
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
        OP_WORKER_PID => return Some(signed(unsafe { Worker::new(worker) }.pid())),
        OP_REGISTER_WORKER => {
            // SAFETY: the context's live worker.
            unsafe { Worker::new(worker) }.set_pid(pid);
            // SAFETY: the server's live registry.
            unsafe { (*registry).register(pid, worker.cast(), REGISTRY_EVENT) };
        }
        OP_REGISTER_USER_WORKER => {
            // worker: local memory; user_workers: shared memory
            // SAFETY: the context's live worker.
            let record = unsafe { Worker::new(worker) };
            record.set_pid(pid);
            let id = worker_id(u64::from(record.id()));
            let shared = ctx.worker_at(id);
            // SAFETY: the worker's shared copy.
            unsafe { Worker::new(shared).set_pid(pid) };
            // SAFETY: the server's live registry.
            unsafe { (*registry).register(pid, worker.cast(), REGISTRY_USER) };
        }
        OP_FORGET_USER_WORKER => {
            // SAFETY: the server's registry.
            let _present = unsafe { (*registry).unregister(pid) };
        }
        OP_ENTER_CHILD => {
            let own = std::process::id().cast_signed();
            // SAFETY: the context's live worker.
            let record = unsafe { Worker::new(worker) };
            record.set_pid(own);
            swoole_set_worker_id(record.id());
            swoole_set_worker_pid(record.pid());
            swoole_set_worker_type(int);
            // SAFETY: this thread's worker globals.
            unsafe { (*thread_globals()).worker = worker };
        }
        OP_POOL_MAIN_LOOP => {
            type MainLoop =
                unsafe extern "C-unwind" fn(*mut PoolRecord, *mut WorkerRecord) -> c_int;
            let pool = ctx.workers().event_pool;
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
            if let Some(slot) = ctx.slot(CB_USER_WORKER_START) {
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
            let count = &raw mut ctx.worker_at(worker_id(arg)).dispatch_count;
            // SAFETY: a worker's counter, shared and updated only atomically.
            let count = unsafe { AtomicI64::from_ptr(count) };
            count.fetch_add(1, Ordering::SeqCst);
        }
        _ => return None,
    }
    Some(0)
}

/// The server's setup and teardown: connections, the manager and the task workers.
fn server(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    let (tables, config) = (ctx.tables(), ctx.config());
    // SAFETY: the server's shared counters, mapped for its life.
    let gs = unsafe { &*config.gs.cast::<ServerCounters>() };
    let task_worker_pid = || {
        // SAFETY: the server's live task pool.
        let pool = unsafe { &*ctx.task_pool() };
        let index = usize::try_from(arg).unwrap_or(usize::MAX);
        // SAFETY: a task worker of it; arg is below task_worker_num.
        unsafe { Worker::new(pool.workers.wrapping_add(index)).pid() }
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
            // SAFETY: the server's live event pool.
            u64::from(!unsafe { (*ctx.workers().event_pool).message_box }.is_null())
        }
        OP_INIT_IPC_MAX_SIZE => {
            // SAFETY: the live server.
            unsafe { crate::worker_methods::init_ipc_max_size(ctx.server()) };
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
            // SAFETY: the server's live task pool.
            unsafe { kill_all_workers(ctx.task_pool(), libc::SIGTERM) };
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
