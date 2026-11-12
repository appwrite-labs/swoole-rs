//! The process-mode factory's facts and effects (`crates/process/src/workers/factory.rs`'s
//! `OP_*`) over the server, worker, connection and send records Rust lays out, which
//! [`run`]'s context answers.
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicI64, Ordering};
use swoole_rs_memory::members::Shm;
use swoole_rs_process::worker_record::Worker;

use swoole_rs_corebase::{DataHead, log};
use swoole_rs_process::methods::{
    ExitStatus, kill_all_workers, swoole_get_worker_id, swoole_get_worker_type,
    swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type, worker_report_error,
};
use swoole_rs_process::worker_record::WorkerRecord;
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

use crate::base_factory::{SendData, is_stream_event, low16};
use crate::callbacks::{CB_CLOSE, CB_USER_WORKER_START, CallbackSlot};
use crate::reactor_ops::ThreadRecord;
use crate::records::{VERIFIED, VERIFIED_TLS};
use crate::{ConnectionRecord, view};

/// What a factory operation runs over: the server, and the worker, send and exit
/// status it reads.
struct Context<'a> {
    serv: view::Server<'a>,
    worker: Option<Worker<'a>>,
    send: Option<&'a mut SendData>,
    status: Option<&'a ExitStatus>,
    conn: *mut ConnectionRecord,
    fd: c_int,
}

impl<'a> Context<'a> {
    fn server(&self) -> view::Server<'a> {
        self.serv
    }
}

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

/// Only [`run`] makes a `Context`, from the worker, send and status its caller lends.
/// An op that reads one its caller didn't lend aborts, as a null dereference would.
#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written under the server's own synchronization as workers.cc did"
)]
impl<'a> Context<'a> {
    fn state(&self) -> &'a crate::ServerState {
        self.serv.state()
    }
    fn config(&self) -> &'a crate::ServerConfig {
        self.serv.config()
    }
    fn tables(&self) -> &'a crate::ServerTables {
        self.state().tables_ref()
    }
    fn workers(&self) -> swoole_rs_process::server_workers::ServerWorkers {
        self.server().workers()
    }
    /// Worker `id` of the server, or `None` past the last.
    fn worker_at(&self, id: u16) -> Option<&mut WorkerRecord> {
        // SAFETY: null past the last, else one of the server's live workers.
        unsafe { self.server().worker(id).as_mut() }
    }
    /// The data sent, for the ops that run with it.
    fn send(&self) -> &SendData {
        self.send
            .as_deref()
            .unwrap_or_else(|| std::process::abort())
    }
    /// The data sent, to write.
    fn send_mut(&mut self) -> &mut SendData {
        self.send
            .as_deref_mut()
            .unwrap_or_else(|| std::process::abort())
    }
    /// The data sent, for the native-shaped calls that take it, or null.
    fn send_ptr(&mut self) -> *mut SendData {
        self.send
            .as_deref_mut()
            .map_or(core::ptr::null_mut(), core::ptr::from_mut)
    }
    /// The context's worker, for the ops that run with one.
    fn worker(&self) -> Worker<'a> {
        self.worker.unwrap_or_else(|| std::process::abort())
    }
    /// The context's worker, for the native-shaped calls that take it, or null.
    fn worker_ptr(&self) -> *mut WorkerRecord {
        self.worker.map_or(core::ptr::null_mut(), Worker::raw)
    }
    /// The exit status, for the ops that run with one.
    fn status(&self) -> &ExitStatus {
        self.status.unwrap_or_else(|| std::process::abort())
    }
    /// The connection selected by `OP_SEND_CONN_ACTIVE` or `OP_SESSION_CONN`, if any.
    fn conn(&self) -> Option<&'a mut ConnectionRecord> {
        // SAFETY: null or a slot of the live connection table.
        unsafe { self.conn.as_mut() }
    }
    /// The server's worker registry.
    fn registry(&self) -> &mut swoole_rs_corebase::worker_registry::WorkerRegistry {
        // SAFETY: the server's live registry, which only the manager writes.
        unsafe { self.server().registry_mut() }
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

/// Runs factory operation `op` (a `RUN_*` of crates/process's `workers::ops`) with
/// `arg` over `serv` and the worker, send and exit status it reads.
pub(crate) fn run<'a>(
    serv: view::Server<'a>,
    op: u32,
    worker: Option<Worker<'a>>,
    send: Option<&'a mut SendData>,
    status: Option<&'a ExitStatus>,
    arg: u64,
) -> i64 {
    let mut ctx = Context {
        serv,
        worker,
        send,
        status,
        conn: core::ptr::null_mut(),
        fd: 0,
    };
    swoole_rs_process::workers::run_over(&mut ctx, op, arg)
}

/// [`run`] of an op that reads the server alone.
pub(crate) fn run_server(serv: view::Server<'_>, op: u32) -> i64 {
    run(serv, op, None, None, None, 0)
}

/// Each factory op over the objects [`run`]'s caller lent: the ops after
/// `OP_SEND_CONN_ACTIVE` or `OP_SESSION_CONN` run with its connection.
impl swoole_rs_process::workers::Answer for Context<'_> {
    fn answer(&mut self, op: u32, arg: u64) -> u64 {
        let answer = fact(self, op);
        let answer = answer.or_else(|| connection(self, op, arg));
        let answer = answer.or_else(|| worker(self, op, arg));
        let answer = answer.or_else(|| server(self, op, arg));
        let answer = answer.or_else(|| native(self, op, arg));
        answer.unwrap_or_else(|| std::process::abort())
    }
}

/// The reactor threads, barriers, pipes and message box the factory makes and
/// frees, and its writes to the workers and reactor threads.
fn native(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv.raw();
    let (state, tables, config) = (ctx.state(), ctx.tables(), ctx.config());
    let barriers = |init: bool| {
        for (manager, count) in [(false, c_int::from(config.reactor_num) + 1), (true, 2)] {
            let barrier = crate::server_object::barrier(ctx.server(), manager);
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
        OP_DELETE_REACTOR_THREADS => {
            // SAFETY: the array `OP_NEW_REACTOR_THREADS` made, not used again.
            unsafe {
                ThreadRecord::delete_array(tables.reactor_threads, config.reactor_num.into());
            }
            0
        }
        OP_INIT_BARRIERS => barriers(true),
        OP_DESTROY_BARRIERS => barriers(false),
        OP_NEW_PROCESS_FACTORY => crate::factory::new(serv, None).expose_provenance() as u64,
        OP_DESTROY_MESSAGE_BOX => {
            // The event pool's box, which the pool forgets as it frees it.
            swoole_rs_process::methods::free_message_box(ctx.server().event_pool_ref());
            0
        }
        OP_CREATE_WORKER_PIPE => u64::from(
            ctx.worker_at(worker_id(arg))
                .is_some_and(|worker| state.push_worker_pipe(worker)),
        ),
        OP_CREATE_PIPE_BUFFERS => signed(crate::master_ops::create_pipe_buffers(ctx.server())),
        OP_WRITE_TO_WORKER => {
            // The worker the factory scheduled, and its pipe, if the server has them.
            let pipe = ctx.worker_at(worker_id(arg));
            let Some(pipe) = pipe.and_then(|worker| crate::objects::socket(worker.pipe_master))
            else {
                return Some(0);
            };
            // This reactor thread's bus, and its copy of the worker's pipe.
            let thread = tables
                .reactor_threads
                .cast::<ThreadRecord>()
                .wrapping_add(usize::from(swoole_rs_corebase::thread_id()));
            // SAFETY: this reactor thread's live record.
            let bus = unsafe { &*thread }.message_bus();
            let socket = bus.pipe_socket(pipe);
            let (mut info, data) = send();
            // SAFETY: the live pipe, and the data sent.
            u64::from(unsafe { bus.rust().send(socket.cast(), &mut info, data.cast()) })
        }
        OP_WRITE_TO_REACTOR => {
            let (mut info, data) = send();
            // The op runs with the session's connection selected.
            info.reactor_id = i16::from(ctx.conn()?.reactor_id);
            info.server_fd = low16(swoole_get_worker_id().cast_signed());
            // `serv->get_reactor_pipe_socket(info.fd, info.reactor_id)`.
            let pipes = i64::from(tables.reactor_pipe_num);
            let id = i64::from(info.reactor_id) + (info.fd % pipes) * i64::from(config.reactor_num);
            // The worker serving that reactor's pipe, if the server has it.
            let Some(worker) = ctx.worker_at(worker_id(id.cast_unsigned())) else {
                return Some(0);
            };
            let pipe = worker.pipe_worker;
            let bus = state.message_bus();
            // SAFETY: the live pipe, and the data sent.
            u64::from(unsafe { bus.rust().send(pipe.cast(), &mut info, data.cast()) })
        }
        OP_FINISH_CLOSE => u64::from(crate::factory::finish(state, ctx.send_ptr().cast())),
        _ => return None,
    })
}

/// `new ReactorThread[count]()`, each bus over the allocator of the server's.
pub(crate) fn new_reactor_threads(state: &crate::ServerState, count: u16) -> *mut c_void {
    ThreadRecord::new_array(count.into(), state.message_bus().allocator())
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
        OP_EXIT_STATUS => u64::from(ctx.status().status() != 0),
        _ => return None,
    })
}

/// The connection the data sent is for: selected by `OP_SEND_CONN_ACTIVE` or
/// `OP_SESSION_CONN`, then read and updated by the ops after them.
fn connection(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv.raw();
    match op {
        OP_SCHEDULE_WORKER => {
            // The task's info.fd is the real fd, not the session id; dispatch converts it.
            let fd = ctx.send().info.fd;
            ctx.fd = c_int::from_ne_bytes(low32(fd.cast_unsigned()).to_ne_bytes());
            let send = ctx.send_ptr().cast();
            // SAFETY: the live server and the data sent.
            let worker = unsafe { crate::server_methods::schedule_worker(serv, ctx.fd, send) };
            return Some(signed(worker));
        }
        OP_SEND_CONN_ACTIVE => {
            ctx.conn = ctx.state().connection_ptr(ctx.fd.into(), 0);
            return Some(u64::from(ctx.conn().is_some_and(|c| c.is_active())));
        }
        OP_SESSION_CONN => {
            let kind = if arg != 0 { VERIFIED } else { VERIFIED_TLS };
            let session = ctx.send().info.fd;
            ctx.conn = ctx.state().connection_ptr(session, kind);
            return Some(u64::from(!ctx.conn.is_null()));
        }
        OP_ON_CLOSE => {
            let call = |info: &mut DataHead| {
                if let Some(slot) = ctx.slot(CB_CLOSE) {
                    // SAFETY: onClose takes the server and the close event.
                    unsafe { slot.call(serv, core::ptr::from_mut(info).cast()) };
                }
            };
            let fd = ctx.send().info.fd;
            return crate::base_factory::on_close(|| ctx.conn(), fd, call).map(|()| 0);
        }
        _ => {}
    }
    let conn = ctx.conn()?;
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
            let info = &mut ctx.send_mut().info;
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
        OP_FORWARD_CLOSE => {
            let ev = DataHead {
                kind: EVENT_CLOSE,
                fd: ctx.send().info.fd,
                reactor_id: i16::from(conn.reactor_id),
                ..DataHead::default()
            };
            let Some(target) = ctx.worker_at(worker_id(arg)) else {
                return Some(signed(-1));
            };
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
    let (serv, worker) = (ctx.serv.raw(), ctx.worker_ptr());
    // The pid or worker type in the argument, as native's `int` conversion keeps it.
    let int = c_int::from_ne_bytes(low32(arg).to_ne_bytes());
    let pid = int;
    match op {
        OP_FORK => return Some(signed(swoole_rs_corebase::swoole_fork(0))),
        OP_EXIT => {
            // exit(3), as upstream.
            std::process::exit(0)
        }
        OP_WORKER_PID => return Some(signed(ctx.worker().pid())),
        OP_REGISTER_WORKER => {
            ctx.worker().set_pid(pid);
            ctx.registry().register(pid, worker.cast(), REGISTRY_EVENT);
        }
        OP_REGISTER_USER_WORKER => {
            // worker: local memory; user_workers: shared memory
            let record = ctx.worker();
            record.set_pid(pid);
            let id = worker_id(u64::from(record.id()));
            // The worker's shared copy.
            if let Some(shared) = ctx.server().worker_record(id) {
                shared.set_pid(pid);
            }
            ctx.registry().register(pid, worker.cast(), REGISTRY_USER);
        }
        OP_FORGET_USER_WORKER => {
            let _present = ctx.registry().unregister(pid);
        }
        OP_ENTER_CHILD => {
            let own = std::process::id().cast_signed();
            let record = ctx.worker();
            record.set_pid(own);
            swoole_set_worker_id(record.id());
            swoole_set_worker_pid(record.pid());
            swoole_set_worker_type(int);
            swoole_rs_process::worker_globals::set_current_worker(worker);
        }
        OP_POOL_MAIN_LOOP => {
            let pool = ctx.workers().event_pool;
            // SAFETY: its live event pool.
            let pool = unsafe { &mut *pool };
            if let Some(main_loop) = pool.main_loop {
                // SAFETY: the pool that holds the loop, and the context's worker of it.
                let _code = unsafe { main_loop(pool.into(), ctx.worker()) };
            }
        }
        OP_START_EVENT_WORKER => {
            let _code = crate::event_worker_ops::start_event_worker(ctx.serv, ctx.worker());
        }
        OP_ON_USER_WORKER_START => {
            if let Some(slot) = ctx.slot(CB_USER_WORKER_START) {
                // SAFETY: onUserWorkerStart takes the server and the worker.
                unsafe { slot.call(serv, worker.cast()) };
            }
        }
        OP_REPORT_ERROR => {
            worker_report_error(ctx.worker(), ctx.status());
        }
        OP_CALL_WORKER_ERROR => {
            let (worker, status) = (ctx.worker(), ctx.status());
            crate::event_worker_ops::call_worker_error_callback(ctx.serv, worker, status);
        }
        OP_COUNT_DISPATCH => {
            if let Some(worker) = ctx.worker_at(worker_id(arg)) {
                // SAFETY: a worker's counter, shared and updated only atomically.
                let count = unsafe { AtomicI64::from_ptr(&raw mut worker.dispatch_count) };
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
        _ => return None,
    }
    Some(0)
}

/// The server's setup and teardown: connections, the manager and the task workers.
fn server(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (tables, config) = (ctx.tables(), ctx.config());
    // SAFETY: only the arms below that `create_process_factory` and
    // `destroy_process_factory` run call it, while the master is the server's only
    // thread, and each as its arm's last tables access.
    let tables_mut = || unsafe { &mut *ctx.state().tables() };
    let gs = ctx.server().counters();
    let manager_pid = || gs.manager_pid.load(Ordering::Relaxed);
    let task_worker_pid = || {
        let pool = crate::shared::ServerShared::of(ctx.server()).task_pool();
        let index = usize::try_from(arg).unwrap_or(usize::MAX);
        pool.worker(index).map_or(0, Worker::pid)
    };
    let waitpid = |pid| signed(swoole_waitpid(pid, None, 0));
    Some(match op {
        OP_NEW_REACTOR_THREADS => {
            tables_mut().reactor_threads = new_reactor_threads(ctx.state(), config.reactor_num);
            0
        }
        OP_ALLOC_CONNECTIONS => {
            let size = size_of::<ConnectionRecord>();
            let list = Shm::calloc(tables.max_connection as usize, size);
            let ready = !list.is_null();
            tables_mut().connection_list = list;
            u64::from(ready)
        }
        LOG_ALLOC_CONNECTIONS => {
            let (max, size) = (tables.max_connection, size_of::<ConnectionRecord>());
            let m = format_args!("sw_shm_calloc({max}, {size}) for connection_list failed");
            log::sys_warning(c"Server::create_process_factory", m);
            0
        }
        OP_FREE_CONNECTIONS => {
            tables_mut().connection_list.free();
            0
        }
        OP_SET_REACTOR_PIPE_NUM => {
            let num = config.worker_num / u32::from(config.reactor_num);
            tables_mut().reactor_pipe_num = u16::try_from(num & 0xffff).unwrap_or_default();
            0
        }
        OP_HAS_MESSAGE_BOX => {
            let pool = crate::shared::ServerShared::of(ctx.server()).event_pool();
            u64::from(!pool.message_box().is_null())
        }
        OP_INIT_IPC_MAX_SIZE => {
            ctx.config().init_ipc_max_size();
            0
        }
        OP_START_MANAGER => signed(crate::manager_ops::start_manager_process(ctx.server())),
        OP_KILL_MANAGER => signed(swoole_rs_process::os::kill(manager_pid(), libc::SIGTERM)),
        LOG_KILL_MANAGER => {
            let pid = manager_pid();
            log::sys_warning(
                c"ProcessFactory::shutdown",
                format_args!("kill({pid}) failed"),
            );
            0
        }
        OP_WAIT_MANAGER => waitpid(manager_pid()),
        LOG_WAIT_MANAGER => {
            let pid = manager_pid();
            log::sys_warning(
                c"ProcessFactory::shutdown",
                format_args!("waitpid({pid}) failed"),
            );
            0
        }
        OP_KILL_TASK_POOL => {
            kill_all_workers(ctx.server().task_pool_ref(), libc::SIGTERM);
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
