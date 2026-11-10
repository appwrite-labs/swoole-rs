//! The base-mode reactor process's ops on native objects (its pool, reactor, pipes,
//! timer and the server's message bus), and the reactor callbacks that run its
//! operations: what workers.cc's `ReactorProcessContext::call` did.
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering;

use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::ops::{
    OP_ADD_HEARTBEAT_TIMER, OP_ADD_PIPES, OP_BUS_POP, OP_BUS_READ, OP_CLOSE_NOTIFY,
    OP_COMMAND_REQUEST, OP_COMMAND_RESPONSE, OP_CREATE_PIPE_BUFFERS, OP_INFO_REACTOR_ID,
    OP_INFO_TYPE, OP_ON_FINISH, OP_ON_PIPE_MESSAGE, OP_POOL_CREATE, OP_PORT_ADD,
    OP_REACTOR_DEL_EVENT, OP_REACTOR_SETUP, OP_RUN_SINGLE_WORKER, OP_STOP_ASYNC,
    OP_TASK_PIPE_NONBLOCK, OP_TIMER_DETACHED, OP_TIMER_REINIT, RUN_REACTOR_PROCESS_CLOSE,
    RUN_REACTOR_PROCESS_MAIN, RUN_REACTOR_PROCESS_PIPE_READ, RUN_REACTOR_PROCESS_TIMEOUT,
};
use swoole_rs_reactor::Event;

use super::Context;
use crate::callbacks::{CB_FINISH, CB_PIPE_MESSAGE};
use crate::objects::{Bus, EVENT_READ, Reactor, set_nonblock, socket};
use crate::reactor_ops::sign;
use crate::view;
use swoole_rs_timer::{swr_v1_timer_owner_reactor, swr_v1_timer_owner_reinit};

/// `SW_IPC_UNIXSOCK`, `SW_WORKER`; `SW_FD_SESSION`, `SW_FD_STREAM_SERVER`, `SW_FD_PIPE`.
const IPC_UNIXSOCK: c_int = 1;
const SW_WORKER: u8 = 2;
const FD_SESSION: c_int = 0;
const FD_STREAM_SERVER: c_int = 1;
const FD_PIPE: c_int = 3;
const SW_ERR: u64 = u64::MAX;

/// The context's objects, each live when the op reading it runs.
#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written by the process that owns them as workers.cc did"
)]
impl Context {
    pub(super) fn state(&self) -> &crate::ServerState {
        self.server().state()
    }
    pub(super) fn config(&self) -> &mut crate::ServerConfig {
        // SAFETY: as above.
        unsafe { self.server().config_mut() }
    }
    pub(super) fn counters(&self) -> &mut crate::ServerCounters {
        // SAFETY: the context's live server.
        unsafe { self.server().counters_mut() }
    }
    pub(super) fn workers(&self) -> swoole_rs_process::server_workers::ServerWorkers {
        self.server().workers()
    }
    /// `serv->ports`.
    pub(super) fn ports(&self) -> &[*mut c_void] {
        self.server().ports()
    }
    pub(super) fn callback_slot(&self, index: u32) -> Option<&crate::callbacks::CallbackSlot> {
        self.state().callbacks.slot(index)
    }
    pub(super) fn tables(&self) -> &crate::ServerTables {
        self.state().tables_ref()
    }
    pub(super) fn reactor_handle(&self) -> Reactor {
        // SAFETY: the context's (or the thread's) live reactor, on this thread.
        unsafe { Reactor::of(super::reactor(self)) }
    }
    pub(super) fn worker_record(&self) -> &WorkerRecord {
        // SAFETY: the ops reading it run with the context's live worker.
        unsafe { &*self.worker }
    }
    /// `serv->get_worker(id)`.
    fn other(&self, id: u64) -> *mut WorkerRecord {
        self.server().worker(u16::try_from(id).unwrap_or(u16::MAX))
    }
    /// The server's own `MessageBus`.
    pub(super) fn bus(&self) -> Bus {
        // SAFETY: the live server's bus, which this process alone reads.
        unsafe { Bus::of(self.tables().message_bus) }
    }
    fn callback(&self, index: u32) {
        if let Some(slot) = self.state().callbacks.slot(index) {
            // SAFETY: the slot takes the server and the bus's `EventData`.
            unsafe { slot.call(self.serv, self.bus().buffer().cast()) };
        }
    }
    pub(super) fn event_socket(&self) -> *mut c_void {
        // SAFETY: the context's live event.
        unsafe { (*self.event).socket }.cast()
    }
}

/// One of the server's ports.
pub(super) fn port_config<'a>(port: *mut c_void) -> &'a crate::port_config::PortConfig {
    // SAFETY: the context hands out only its live server's ports.
    unsafe { view::port(port) }
}

/// This thread's timer, or null.
fn timer() -> *mut c_void {
    // SAFETY: this thread's timer slot.
    unsafe { *swoole_rs_corebase::swr_v1_core_timer_slot() }
}

/// The ops on native objects.
pub(super) fn native(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_POOL_CREATE => create_pool(ctx),
        OP_CREATE_PIPE_BUFFERS => sign(crate::master_ops::create_pipe_buffers(ctx.server())),
        OP_RUN_SINGLE_WORKER => {
            let pool = ctx.server().workers().event_pool;
            // SAFETY: its live event pool, which has a worker.
            let worker = unsafe { (*pool).workers };
            let globals = swoole_rs_process::worker_globals::thread_globals();
            // SAFETY: this thread's worker globals.
            unsafe { (*globals).worker = worker };
            // SAFETY: the pool and its first worker.
            sign(unsafe { entry::reactor_process_main_loop(pool.cast(), worker.cast()) })
        }
        OP_BUS_READ => (ctx.bus().read(socket(ctx.event_socket())?) as i64).cast_unsigned(),
        OP_INFO_TYPE => u64::from(ctx.bus().head().kind),
        OP_INFO_REACTOR_ID => sign(ctx.bus().head().reactor_id),
        OP_ON_PIPE_MESSAGE => {
            ctx.callback(CB_PIPE_MESSAGE);
            0
        }
        OP_ON_FINISH => {
            ctx.callback(CB_FINISH);
            0
        }
        OP_STOP_ASYNC => {
            let worker = ctx.other(u64::from(ctx.reactor_handle().fields().id));
            // SAFETY: the live server and one of its workers.
            unsafe { entry::stop_async_worker(ctx.serv, worker.cast()) };
            0
        }
        OP_COMMAND_REQUEST => {
            // SAFETY: this thread's worker globals, which name its live worker.
            let me = unsafe { (*swoole_rs_process::worker_globals::thread_globals()).worker };
            // SAFETY: as above.
            let id = u16::try_from(unsafe { &*me }.id()).unwrap_or(0);
            // SAFETY: worker 0, live.
            let pipe = unsafe { (*ctx.other(0)).pipe_master };
            let bus = ctx.tables().message_bus;
            // SAFETY: the live server, its bus holding the request, and the pipe.
            unsafe { crate::commands::call_command_handler(ctx.server(), bus, id, pipe) };
            0
        }
        OP_COMMAND_RESPONSE => {
            let (fd, packet) = (ctx.bus().head().fd, ctx.bus().packet());
            let result = crate::commands::swr_v1_server_command_result;
            // SAFETY: the live server and the packet's bytes.
            unsafe { result(ctx.state(), ctx.serv, fd, packet.data, packet.length) };
            0
        }
        OP_BUS_POP => {
            ctx.bus().pop();
            0
        }
        OP_TIMER_DETACHED => {
            let timer = timer();
            // SAFETY: this thread's live timer.
            u64::from(
                !timer.is_null() && unsafe { swr_v1_timer_owner_reactor(timer.cast()) }.is_null(),
            )
        }
        OP_TIMER_REINIT => {
            // SAFETY: this thread's live timer.
            unsafe { swr_v1_timer_owner_reinit(timer().cast(), false) };
            0
        }
        OP_PORT_ADD => {
            let port = *ctx.ports().get(usize::try_from(arg).ok()?)?;
            // SAFETY: one of its live ports.
            let port = unsafe { view::port(port) };
            let gs = port.counters()?;
            let id = ctx.worker_record().id() as usize;
            // SAFETY: the worker's slot of them.
            unsafe { &*gs.connection_nums.wrapping_add(id) }.store(0, Ordering::Relaxed);
            sign(ctx.reactor_handle().add(socket(port.socket)?, EVENT_READ))
        }
        OP_REACTOR_SETUP => {
            let reactor = ctx.reactor_handle();
            let fields = reactor.fields();
            fields.id = u16::try_from(ctx.worker_record().id()).unwrap_or(u16::MAX);
            fields.ptr = ctx.serv;
            fields.max_socket = ctx.tables().max_connection;
            fields.close = Some(crate::reactor_entry::close_connection);
            let accept = crate::server_methods::accept_connection;
            reactor.set_handler(FD_STREAM_SERVER, EVENT_READ, accept);
            reactor.set_error_handler(on_close);
            reactor.set_handler(FD_PIPE, EVENT_READ, on_pipe_read);
            0
        }
        OP_ADD_PIPES => {
            let worker = ctx.worker_record();
            let pipe_worker = socket(worker.pipe_worker)?;
            let pipe_master = socket(worker.pipe_master)?;
            set_nonblock(pipe_worker);
            set_nonblock(pipe_master);
            let reactor = ctx.reactor_handle();
            if reactor.add(pipe_worker, EVENT_READ) < 0 {
                return Some(SW_ERR);
            }
            sign(reactor.add(pipe_master, EVENT_READ))
        }
        OP_TASK_PIPE_NONBLOCK => {
            let pool = ctx.server().task_pool();
            // SAFETY: its live task pool, whose worker `arg` is.
            let worker = unsafe { (*pool).workers }.wrapping_add(usize::try_from(arg).ok()?);
            // SAFETY: as above.
            set_nonblock(socket(unsafe { (*worker).pipe_master })?);
            0
        }
        OP_ADD_HEARTBEAT_TIMER => {
            let msec = c_long::from(ctx.config().heartbeat_check_interval) * 1000;
            let reactor = ctx.reactor_handle().ptr();
            // SAFETY: `on_timeout` takes the reactor, which outlives its timers.
            let node =
                unsafe { swoole_rs_timer::swr_v1_timer_add_plain(msec, true, on_timeout, reactor) };
            ctx.config().heartbeat_timer = node;
            u64::from(!node.is_null())
        }
        OP_REACTOR_DEL_EVENT => sign(ctx.reactor_handle().del(socket(ctx.event_socket())?)),
        OP_CLOSE_NOTIFY => {
            // SAFETY: the connection the context selected.
            let conn = unsafe { &*ctx.conn };
            let mut event = Event {
                fd: conn.fd,
                reactor_id: i16::from(conn.reactor_id),
                fd_type: FD_SESSION,
                socket: conn.socket,
            };
            on_close(ctx.reactor_handle().ptr(), (&raw mut event).cast());
            0
        }
        _ => return None,
    })
}

/// `OP_POOL_CREATE`: the event worker pool over the server's workers.
fn create_pool(ctx: &Context) -> u64 {
    let config = ctx.config();
    let workers = ctx.server().workers();
    let pool = workers.event_pool;
    // SAFETY: the server's event pool, reset to zero as `*pool = {}` did.
    unsafe { pool.write_bytes(0, 1) };
    let create = swoole_rs_process::methods::create;
    // SAFETY: as above.
    if unsafe { create(pool, config.worker_num, 0, IPC_UNIXSOCK) } < 0 {
        return SW_ERR;
    }
    let set_max_request = swoole_rs_process::methods::set_max_request;
    // SAFETY: the pool just created.
    unsafe { set_max_request(pool, config.max_request, config.max_request_grace) };
    // SAFETY: as above.
    let record = unsafe { &mut *pool };
    record.ptr = ctx.serv;
    record.max_wait_time = config.max_wait_time;
    record.use_msgqueue = 0;
    record.main_loop = entry::reactor_process_main_loop as *const c_void;
    record.on_worker_not_found = worker_not_found as *const c_void;
    let count = config.worker_num as usize;
    // SAFETY: the pool's and the server's worker arrays, `count` records each.
    unsafe { core::ptr::copy_nonoverlapping(record.workers, workers.event_workers, count) };
    record.workers = workers.event_workers;
    // SAFETY: as above.
    let workers = unsafe { core::slice::from_raw_parts_mut(record.workers, count) };
    for (id, worker) in (0u32..).zip(workers) {
        worker.place(pool.cast(), id, SW_WORKER);
    }
    0
}

/// `pool->onWorkerNotFound`: restart the event worker that exited.
extern "C" fn worker_not_found(pool: *mut PoolRecord, status: *const ExitStatus) -> c_int {
    // SAFETY: the live pool.
    let serv = unsafe { (*pool).ptr };
    // SAFETY: its server and the worker's exit status.
    unsafe { entry::restart_worker_process(serv, status.cast()) }
}

/// Runs `operation` over a context of `reactor`'s server and `event`.
fn run(reactor: *mut c_void, event: *mut c_void, operation: u32) -> c_int {
    // SAFETY: the reactor running this callback.
    let reactor = unsafe { Reactor::of(reactor) };
    let mut ctx = Context {
        // A reactor process's reactor holds its server.
        serv: reactor.fields().ptr,
        worker: core::ptr::null_mut(),
        reactor: reactor.ptr(),
        event: event.cast(),
        conn: core::ptr::null_mut(),
        now: 0.0,
    };
    c_int::try_from(run_over(&mut ctx, operation)).unwrap_or(-1)
}

/// Runs `operation` over `ctx`, whose objects are live.
fn run_over(ctx: &mut Context, operation: u32) -> i64 {
    swoole_rs_process::workers::run_over(ctx, operation, 0)
}

/// `Server::start_reactor_processes` over `object`, the server, and
/// `Server::reactor_process_main_loop` over `object`, a pool, and its `worker`
/// (crates/process's entry.rs).
///
/// # Safety
/// The objects are what `operation`'s method took, live for the call.
pub(crate) unsafe fn enter(
    operation: u32,
    object: *mut c_void,
    worker: *mut c_void,
    _extra: *mut c_void,
    _arg: u64,
) -> i64 {
    let serv = if operation == RUN_REACTOR_PROCESS_MAIN {
        // SAFETY: as the caller guarantees: a reactor process pool's `ptr` is its server.
        unsafe { (*object.cast::<PoolRecord>()).ptr }
    } else {
        object
    };
    let mut ctx = Context {
        serv,
        worker: worker.cast(),
        reactor: core::ptr::null_mut(),
        event: core::ptr::null_mut(),
        conn: core::ptr::null_mut(),
        now: 0.0,
    };
    run_over(&mut ctx, operation)
}

/// `ReactorProcess_onPipeRead`.
extern "C" fn on_pipe_read(reactor: *mut c_void, event: *mut c_void) -> c_int {
    run(reactor, event, RUN_REACTOR_PROCESS_PIPE_READ)
}

/// `ReactorProcess_onClose`: the reactor's default error handler.
extern "C" fn on_close(reactor: *mut c_void, event: *mut c_void) -> c_int {
    run(reactor, event, RUN_REACTOR_PROCESS_CLOSE)
}

/// `ReactorProcess_onTimeout`: the heartbeat check.
extern "C-unwind" fn on_timeout(reactor: *mut c_void) {
    run(reactor, core::ptr::null_mut(), RUN_REACTOR_PROCESS_TIMEOUT);
}
