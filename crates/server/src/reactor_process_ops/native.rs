//! The base-mode reactor process's ops on native objects (its pool, reactor, pipes,
//! timer and the server's message bus), and the reactor callbacks that run its
//! operations: what workers.cc's `ReactorProcessContext::call` did.
use core::ffi::{c_int, c_long};
use core::sync::atomic::Ordering;

use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::process_pool::PoolRef;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::workers::ops::{
    OP_ADD_HEARTBEAT_TIMER, OP_ADD_PIPES, OP_BUS_POP, OP_BUS_READ, OP_CLOSE_NOTIFY,
    OP_COMMAND_REQUEST, OP_COMMAND_RESPONSE, OP_CREATE_PIPE_BUFFERS, OP_INFO_REACTOR_ID,
    OP_INFO_TYPE, OP_ON_FINISH, OP_ON_PIPE_MESSAGE, OP_POOL_CREATE, OP_PORT_ADD,
    OP_REACTOR_DEL_EVENT, OP_REACTOR_SETUP, OP_RUN_SINGLE_WORKER, OP_STOP_ASYNC,
    OP_TASK_PIPE_NONBLOCK, OP_TIMER_DETACHED, OP_TIMER_REINIT, RUN_REACTOR_PROCESS_CLOSE,
    RUN_REACTOR_PROCESS_MAIN, RUN_REACTOR_PROCESS_PIPE_READ, RUN_REACTOR_PROCESS_START,
    RUN_REACTOR_PROCESS_TIMEOUT,
};
use swoole_rs_reactor::{Event, Running};

use super::Context;
use crate::callbacks::TaskSlot;
use crate::objects::{Bus, EVENT_READ, Reactor, set_nonblock, worker_pipe};
use crate::reactor_ops::sign;
use crate::view;

/// `SW_IPC_UNIXSOCK`, `SW_WORKER`; `SW_FD_SESSION`, `SW_FD_STREAM_SERVER`, `SW_FD_PIPE`.
const IPC_UNIXSOCK: c_int = 1;
const SW_WORKER: u8 = 2;
const FD_SESSION: c_int = 0;
const FD_STREAM_SERVER: c_int = 1;
const FD_PIPE: c_int = 3;
const SW_ERR: u64 = u64::MAX;

/// The context's objects, each live when the op reading it runs.
impl Context<'_> {
    pub(super) fn state(&self) -> &crate::ServerState {
        self.server().state()
    }
    pub(super) fn config(&self) -> &crate::ServerConfig {
        self.server().config()
    }
    pub(super) fn counters(&self) -> &crate::ServerCounters {
        self.server().counters()
    }
    pub(super) fn workers(&self) -> swoole_rs_process::server_workers::ServerWorkers {
        self.server().workers()
    }
    /// `serv->ports`.
    pub(super) fn ports(&self) -> &[view::Port<'_>] {
        self.server().ports()
    }
    pub(super) fn callback_slot(&self, index: u32) -> Option<&crate::callbacks::CallbackSlot> {
        self.state().callbacks.slot(index)
    }
    pub(super) fn tables(&self) -> &crate::ServerTables {
        self.state().tables_ref()
    }
    pub(super) fn reactor_handle(&self) -> Reactor {
        self.reactor.unwrap_or_else(Reactor::thread)
    }
    /// The context's worker; the ops reading it run with one.
    pub(super) fn worker(&self) -> Worker<'_> {
        self.worker
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a reactor-process op without a worker"))
    }
    /// `serv->get_worker(id)`, or `None` past the last.
    fn other(&self, id: u64) -> Option<Worker<'_>> {
        self.server()
            .worker_record(u16::try_from(id).unwrap_or(u16::MAX))
    }
    /// The server's own `MessageBus`.
    pub(super) fn bus(&self) -> Bus {
        self.state().message_bus()
    }
    /// The event the bus just read, as its sender packed it: an `EventData`, its
    /// head and the payload the head counts.
    fn event_data(&self) -> Packed<'_> {
        // SAFETY: the ops reading it run once the bus has read a whole event into its
        // buffer, which only this thread's next read replaces.
        unsafe { Packed::from_raw(self.bus().buffer().cast()) }
    }
    /// The context's event, which the ops reading it run with.
    pub(super) fn event(&self) -> &Event {
        self.event
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a reactor-process op without an event"))
    }
    pub(super) fn event_socket(&self) -> Option<swoole_rs_reactor::Socket<'_>> {
        self.event().socket()
    }
    /// The connection `OP_CONN_AT` or `OP_EVENT_CONN_ACTIVE` selected, if any.
    pub(super) fn conn(&self) -> Option<crate::Connection<'_>> {
        self.conn
    }
}

/// The ops on native objects.
pub(super) fn native(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_POOL_CREATE => create_pool(ctx),
        OP_CREATE_PIPE_BUFFERS => sign(crate::master_ops::create_pipe_buffers(ctx.server())),
        OP_RUN_SINGLE_WORKER => {
            let worker = crate::shared::ServerShared::of(ctx.server())
                .event_pool()
                .worker(0);
            let raw = worker.map_or(core::ptr::null_mut(), Worker::raw);
            swoole_rs_process::worker_globals::set_current_worker(raw);
            let mut ctx = over(ctx.serv, worker);
            sign(run_over(&mut ctx, RUN_REACTOR_PROCESS_MAIN))
        }
        OP_BUS_READ => (ctx.bus().read(ctx.event_socket()?) as i64).cast_unsigned(),
        OP_INFO_TYPE => u64::from(ctx.bus().head().kind),
        OP_INFO_REACTOR_ID => sign(ctx.bus().head().reactor_id),
        OP_ON_PIPE_MESSAGE => {
            ctx.state()
                .callbacks
                .pipe_message(ctx.serv, ctx.event_data());
            0
        }
        OP_ON_FINISH => {
            let callbacks = &ctx.state().callbacks;
            let _ = callbacks.task(TaskSlot::Finish, ctx.serv, ctx.event_data());
            0
        }
        OP_STOP_ASYNC => {
            let worker = ctx.other(u64::from(ctx.reactor_handle().handle().id()))?;
            crate::event_worker_ops::stop_async_worker(ctx.server(), worker);
            0
        }
        OP_COMMAND_REQUEST => {
            // The worker this process runs.
            let me = swoole_rs_process::worker_globals::with_current(|me| me.id())?;
            let id = u16::try_from(me).unwrap_or(0);
            let pipe = worker_pipe(ctx.other(0)?, true);
            crate::commands::call_command_handler(ctx.server(), ctx.bus(), id, pipe);
            0
        }
        OP_COMMAND_RESPONSE => {
            let (bus, state) = (ctx.bus(), ctx.state());
            let fd = bus.head().fd;
            bus.with_packet(|reply| crate::commands::command_result(state, fd, reply));
            0
        }
        OP_BUS_POP => {
            ctx.bus().pop();
            0
        }
        OP_TIMER_DETACHED => u64::from(crate::objects::timer_detached()),
        OP_TIMER_REINIT => {
            crate::objects::timer_reinit(false);
            0
        }
        OP_PORT_ADD => {
            let port = *ctx.ports().get(usize::try_from(arg).ok()?)?;
            // The worker's slot of the port's counters.
            let id = ctx.worker().id() as usize;
            ctx.server()
                .port_worker_connections(port, id)?
                .store(0, Ordering::Relaxed);
            sign(port.with_socket(|socket| ctx.reactor_handle().add(socket, EVENT_READ))?)
        }
        OP_REACTOR_SETUP => {
            let reactor = ctx.reactor_handle();
            let running = reactor.handle();
            running.set_id(u16::try_from(ctx.worker().id()).unwrap_or(u16::MAX));
            running.set_object(ctx.serv.raw());
            running.set_max_socket(ctx.tables().max_connection);
            running.set_close(Some(crate::reactor_entry::close_connection));
            let accept = crate::server_methods::accept_connection;
            reactor.set_handler(FD_STREAM_SERVER, EVENT_READ, accept);
            reactor.set_error_handler(on_close);
            reactor.set_handler(FD_PIPE, EVENT_READ, on_pipe_read);
            0
        }
        OP_ADD_PIPES => {
            let worker = ctx.worker();
            let pipe_worker = worker_pipe(worker, false)?;
            let pipe_master = worker_pipe(worker, true)?;
            set_nonblock(pipe_worker);
            set_nonblock(pipe_master);
            let reactor = ctx.reactor_handle();
            if reactor.add(pipe_worker, EVENT_READ) < 0 {
                return Some(SW_ERR);
            }
            sign(reactor.add(pipe_master, EVENT_READ))
        }
        OP_TASK_PIPE_NONBLOCK => {
            let pool = crate::shared::ServerShared::of(ctx.server()).task_pool();
            let worker = pool.worker(usize::try_from(arg).ok()?)?;
            set_nonblock(worker_pipe(worker, true)?);
            0
        }
        OP_ADD_HEARTBEAT_TIMER => {
            let msec = c_long::from(ctx.config().heartbeat_check_interval.get()) * 1000;
            // `ReactorProcess_onTimeout`: the heartbeat check, on the reactor arming it,
            // which outlives its timers.
            let reactor = ctx.reactor_handle();
            let node = swoole_rs_timer::add_fn(msec, true, move || {
                run(reactor, None, RUN_REACTOR_PROCESS_TIMEOUT);
            });
            ctx.config().heartbeat_timer.store(node, Ordering::Relaxed);
            u64::from(!node.is_null())
        }
        OP_REACTOR_DEL_EVENT => sign(ctx.reactor_handle().del(ctx.event_socket()?)),
        OP_CLOSE_NOTIFY => {
            let conn = ctx.conn()?;
            let event = conn.event(i16::from(conn.reactor_id()), FD_SESSION);
            run(
                ctx.reactor_handle(),
                Some(&event),
                RUN_REACTOR_PROCESS_CLOSE,
            );
            0
        }
        _ => return None,
    })
}

/// `OP_POOL_CREATE`: the event worker pool over the server's workers.
fn create_pool(ctx: &Context<'_>) -> u64 {
    let config = ctx.config();
    let workers = ctx.server().workers();
    let pool = workers.event_pool;
    // SAFETY: the server's event pool, reset to zero as `*pool = {}` did.
    unsafe { pool.write_bytes(0, 1) };
    let pool_ref = ctx.server().event_pool_ref();
    if swoole_rs_process::methods::create(pool_ref, config.worker_num.get(), 0, IPC_UNIXSOCK) < 0 {
        return SW_ERR;
    }
    swoole_rs_process::methods::set_max_request(
        pool_ref,
        config.max_request.get(),
        config.max_request_grace.get(),
    );
    // SAFETY: as above.
    let record = unsafe { &mut *pool };
    record.ptr = ctx.serv.raw();
    record.max_wait_time = config.max_wait_time.get();
    record.use_msgqueue = false;
    record.main_loop = Some(main_loop);
    record.on_worker_not_found = Some(worker_not_found);
    let count = config.worker_num.get() as usize;
    // SAFETY: the pool's and the server's worker arrays, `count` records each.
    unsafe { core::ptr::copy_nonoverlapping(record.workers(), workers.event_workers, count) };
    // SAFETY: the server's `count` (`worker_num`) event worker records, which live
    // as long as the server and so its event pool.
    unsafe { record.set_workers(workers.event_workers) };
    // `create` set the pool's `worker_num` to `count`, so these are the server's records.
    for (id, index) in (0u32..).zip(0..count) {
        if let Some(worker) = record.worker(index) {
            worker.set_pool(pool.cast());
            worker.set_id(id);
            worker.set_kind(SW_WORKER);
        }
    }
    0
}

/// `pool->main_loop`: `Server::reactor_process_main_loop` over the pool's worker.
extern "C-unwind" fn main_loop(pool: PoolRef<'_>, worker: Worker<'_>) -> c_int {
    // SAFETY: a reactor process pool's `ptr` is its live server.
    let serv = unsafe { view::Server::from_raw(pool.ptr()) };
    let mut ctx = over(serv, Some(worker));
    c_int::try_from(run_over(&mut ctx, RUN_REACTOR_PROCESS_MAIN)).unwrap_or(-1)
}

/// `pool->onWorkerNotFound`: restart the event worker that exited. The pool lends
/// itself and the exit status for the call.
///
/// # Safety
/// As [`swoole_rs_process::process_pool::NotFoundFn`]: `pool` is a reactor process
/// pool, whose `ptr` is its live server.
unsafe extern "C-unwind" fn worker_not_found(pool: PoolRef<'_>, status: &ExitStatus) -> c_int {
    // SAFETY: as the caller promises, `ptr` is the pool's live server.
    let serv = unsafe { view::Server::from_raw(pool.ptr()) };
    crate::manager_ops::restart_worker_process(serv, status)
}

/// Runs `operation` over a context of `reactor`'s server and `event`.
fn run(reactor: Reactor, event: Option<&Event>, operation: u32) -> c_int {
    let mut ctx = Context {
        // SAFETY: a reactor process's reactor holds its live server.
        serv: unsafe { crate::reactor_entry::server(reactor) },
        worker: None,
        reactor: Some(reactor),
        event,
        conn: None,
        now: 0.0,
    };
    c_int::try_from(run_over(&mut ctx, operation)).unwrap_or(-1)
}

/// Runs `operation` over `ctx`, whose objects are live.
fn run_over(ctx: &mut Context<'_>, operation: u32) -> i64 {
    swoole_rs_process::workers::run_over(ctx, operation, 0)
}

/// The context over `serv` and `worker` alone.
fn over<'a>(serv: view::Server<'a>, worker: Option<Worker<'a>>) -> Context<'a> {
    Context {
        serv,
        worker,
        reactor: None,
        event: None,
        conn: None,
        now: 0.0,
    }
}

/// `Server::start_reactor_processes()`.
pub(crate) fn start_reactor_processes(serv: view::Server<'_>) -> c_int {
    let mut ctx = over(serv, None);
    c_int::try_from(run_over(&mut ctx, RUN_REACTOR_PROCESS_START)).unwrap_or(-1)
}

/// `ReactorProcess_onPipeRead`.
extern "C" fn on_pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    run(
        Reactor::running(reactor),
        Some(event),
        RUN_REACTOR_PROCESS_PIPE_READ,
    )
}

/// `ReactorProcess_onClose`: the reactor's default error handler.
extern "C" fn on_close(reactor: Running<'_>, event: &mut Event) -> c_int {
    run(
        Reactor::running(reactor),
        Some(event),
        RUN_REACTOR_PROCESS_CLOSE,
    )
}
