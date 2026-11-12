//! Thread mode's facts and effects (`crates/process/src/workers/thread.rs`'s `OP_*`)
//! over the server, pool, worker and port records Rust lays out, the factory's
//! threads ([`ThreadManager`]) and each thread's message bus: what workers.cc's
//! `ThreadContext::call` did.
use core::ffi::{c_int, c_void};
use core::ptr::null_mut;
use core::sync::atomic::Ordering;
use std::sync::Arc;

use swoole_rs_memory::members::Shm;
use swoole_rs_process::methods::{ExitStatus, start_check, swoole_get_worker_type};
use swoole_rs_process::server_workers::ServerWorkers;
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_process::workers::ops::RUN_CREATE_WORKER_PIPES;
use swoole_rs_process::workers::ops::{
    LOG_CALLOC_CONNECTIONS, OP_CALL_WORKER_ERROR, OP_CREATE_MESSAGE_BUS, OP_DESTROY_MESSAGE_BUS,
    OP_JOIN_THREAD, OP_NEW_THREAD_FACTORY, OP_REACTOR_THREAD_MAIN, OP_SELECT_WORKER,
    OP_SET_MASTER_THREAD_ID, OP_SPAWN_THREAD, OP_START_MASTER_THREAD, OP_THREAD_ENTER,
    OP_THREAD_EXIT, OP_THREAD_EXIT_STATUS, OP_THREAD_MANAGER, OP_THREAD_SUPPORTED,
    OP_WORKER_THREAD_START, RUN_CREATE_THREAD_FACTORY, RUN_DESTROY_THREAD_FACTORY, RUN_THREAD_BODY,
    RUN_THREAD_MAIN, RUN_THREAD_RELOAD, RUN_THREAD_TERMINATE_MANAGER,
};
use swoole_rs_process::workers::ops::{
    OP_ABORT, OP_ALL_CORE_WORKER_NUM, OP_ALL_WORKER_NUM, OP_CALLOC_CONNECTIONS,
    OP_CREATE_EVENT_WORKERS, OP_CREATE_USER_WORKERS, OP_CREATE_WORKER_PIPES,
    OP_DELETE_REACTOR_THREADS, OP_FREE_THREAD_CONNECTIONS, OP_GET_WORKER, OP_IS_MANAGER,
    OP_KILL_WORKER, OP_MANAGER_THREAD_ID, OP_NEW_REACTOR_THREADS, OP_ON_AFTER_RELOAD,
    OP_ON_BEFORE_RELOAD, OP_ON_MANAGER_START, OP_ON_MANAGER_STOP, OP_ON_USER_WORKER_START,
    OP_ON_WORKER_START, OP_ON_WORKER_STOP, OP_POOL_MAIN_LOOP, OP_PORT_ADD, OP_PORT_COUNT,
    OP_PORT_DGRAM, OP_PORT_LISTEN, OP_RUNNING, OP_SINGLE_REACTOR_PIPE, OP_STOPPING,
    OP_STORE_LISTEN_SOCKET, OP_TASK_START_CHECK, OP_TASK_WORKER_NUM, OP_TERMINATE_MANAGER_THREAD,
    OP_THREAD_REACTOR_NUM, OP_USER_WORKER_NUM, OP_WORKER_ID, OP_WORKER_NUM, OP_WORKER_TYPE,
};
use swoole_rs_process::workers::thread::{ThreadManager, worker_thread_start};

use crate::callbacks::{
    CB_AFTER_RELOAD, CB_BEFORE_RELOAD, CB_MANAGER_START, CB_MANAGER_STOP, CB_USER_WORKER_START,
};
use crate::objects::{Bus, Reactor};
use crate::port_config::methods;
use crate::reactor_ops::sign;
use crate::records::ConnectionRecord;
use crate::worker_methods::{create_event_workers, create_user_workers};
use crate::{factory, pool_native, server_methods, view};

/// What an operation runs over.
#[derive(Clone, Copy)]
struct Context<'a> {
    serv: view::Server<'a>,
    factory: *mut c_void,
    worker: *mut WorkerRecord,
}

impl<'a> Context<'a> {
    /// The context's live server.
    fn server(&self) -> view::Server<'a> {
        self.serv
    }
}

/// `SW_MANAGER`, `SW_EVENT_READ` and `Server::THREAD_WORKER`.
const SW_MANAGER: c_int = 3;
const SW_EVENT_READ: c_int = 1 << 9;
const THREAD_WORKER: u8 = 4;

/// A task pool's `onWorkerStart` and `onWorkerStop`, and `main_loop`, whose result
/// thread mode ignores.
/// The context's objects, each live when the op reading it runs.
#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written by the thread that owns them as workers.cc did"
)]
impl Context<'_> {
    fn state(&self) -> &crate::ServerState {
        self.server().state()
    }
    fn config(&self) -> &crate::ServerConfig {
        self.server().config()
    }
    fn tables(&self) -> &mut crate::records::ServerTables {
        // SAFETY: the server's tables, which live as long as it.
        unsafe { &mut *self.state().tables() }
    }
    fn workers(&self) -> ServerWorkers {
        self.server().workers()
    }
    /// `serv->get_all_worker_num()`.
    fn all_worker_num(&self) -> u64 {
        let workers = self.workers();
        u64::from(workers.event_count) + u64::from(workers.task_count) + workers.user_count as u64
    }
    /// The context's worker.
    fn worker(&self) -> Worker<'_> {
        // SAFETY: the ops reading it run with a live worker in the context.
        unsafe { Worker::new(self.worker) }
    }
    /// The thread factory's threads; none for another mode's factory, which
    /// [`Context::answer`] aborts on.
    fn manager(&self) -> Option<&ThreadManager> {
        // SAFETY: the ops reading it run with the context's live thread factory.
        unsafe { factory::manager(self.factory) }
    }
    /// The server's `(Server *)` callback `index`, when set.
    fn call_server(&self, index: u32) -> u64 {
        self.state().callbacks.call_server(index, self.server());
        0
    }
    /// Runs thread-mode `operation` with `arg` over this context.
    fn run(mut self, operation: u32, arg: u64) -> i64 {
        swoole_rs_process::workers::run_over(&mut self, operation, arg)
    }
}

/// `Server::reload_worker_threads(reload_all_workers)`.
pub(crate) fn reload_worker_threads(serv: view::Server<'_>, reload_all_workers: bool) -> bool {
    let arg = u64::from(reload_all_workers);
    over(serv, null_mut(), RUN_THREAD_RELOAD).run(RUN_THREAD_RELOAD, arg) != 0
}

/// A thread-mode `Server` method over the live server alone.
pub(crate) fn run_server(serv: view::Server<'_>, operation: u32) -> i64 {
    over(serv, null_mut(), operation).run(operation, 0)
}

/// The context of `operation` over `serv` and a `ThreadFactory` method's own
/// factory, `other`, or else the server's in thread mode.
fn over(serv: view::Server<'_>, other: *mut c_void, operation: u32) -> Context<'_> {
    let mut ctx = Context {
        serv,
        factory: null_mut(),
        worker: null_mut(),
    };
    ctx.factory = match operation {
        RUN_CREATE_THREAD_FACTORY | RUN_DESTROY_THREAD_FACTORY => null_mut(),
        _ if !other.is_null() => other,
        _ if u64::from(ctx.config().mode_) == crate::MODE_THREAD => ctx.config().factory_,
        _ => null_mut(),
    };
    ctx
}

/// Answers each thread-mode op over the context.
impl swoole_rs_process::workers::Answer for Context<'_> {
    fn answer(&mut self, op: u32, arg: u64) -> u64 {
        let ctx = self;
        let answer = server(ctx, op, arg);
        let answer = answer.or_else(|| worker(ctx, op, arg));
        let answer = answer.or_else(|| threads(ctx, op, arg));
        answer.unwrap_or_else(|| std::process::abort())
    }
}

/// A thread's body inside `worker_thread_start`: [`RUN_THREAD_MAIN`] over the context.
///
/// # Safety
/// `main` is `OP_WORKER_THREAD_START`'s live `(Context, u64)`, lent for the call.
unsafe extern "C" fn thread_main(main: *mut c_void) {
    // SAFETY: per the caller.
    let (ctx, arg) = unsafe { *main.cast::<(Context<'_>, u64)>() };
    ctx.run(RUN_THREAD_MAIN, arg);
}

/// The facts and effects on the factory's threads and the thread's own state.
fn threads(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    let slot = arg & u64::from(u32::MAX);
    Some(match op {
        OP_THREAD_MANAGER => (&raw const *ctx.manager()?).addr() as u64,
        OP_THREAD_SUPPORTED => u64::from(crate::THREAD_SUPPORTED.load(Ordering::Relaxed)),
        LOG_CALLOC_CONNECTIONS => {
            let max = ctx.tables().max_connection as usize;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "upstream prints the size as an int"
            )]
            let size = (max * size_of::<ConnectionRecord>()) as c_int;
            swoole_rs_corebase::log::sys_warning(
                c"Server::create_thread_factory",
                format_args!("calloc[2]({size}) failed"),
            );
            0
        }
        OP_NEW_THREAD_FACTORY => {
            let threads = usize::try_from(ctx.all_worker_num() + 1).ok()?;
            factory::new_thread(serv.raw(), threads).addr() as u64
        }
        OP_JOIN_THREAD => {
            // An uncaught `std::system_error` upstream: the process ends.
            if ctx.manager()?.thread(arg).join().is_err() {
                std::process::abort();
            }
            0
        }
        OP_SELECT_WORKER => {
            ctx.worker = core::ptr::with_exposed_provenance_mut(usize::try_from(arg).ok()?);
            0
        }
        OP_THREAD_EXIT_STATUS => {
            let thread = ctx.manager()?.thread(ctx.worker().id().into());
            i64::from(thread.exit_status()).cast_unsigned()
        }
        OP_CALL_WORKER_ERROR => {
            let worker = ctx.worker();
            let status = ctx.manager()?.thread(worker.id().into()).exit_status() << 8;
            let status = ExitStatus::new(worker.pid(), status);
            crate::event_worker_ops::call_worker_error_callback(ctx.server(), worker, &status);
            0
        }
        OP_SPAWN_THREAD => {
            let (serv, factory) = (
                serv.raw().expose_provenance(),
                ctx.factory.expose_provenance(),
            );
            let body = move || {
                let serv = core::ptr::with_exposed_provenance_mut(serv);
                let ctx = Context {
                    // SAFETY: the live server, which outlives the factory's threads.
                    serv: unsafe { view::Server::from_raw(serv) },
                    factory: core::ptr::with_exposed_provenance_mut(factory),
                    worker: null_mut(),
                };
                ctx.run(RUN_THREAD_BODY, arg);
            };
            // An uncaught `std::system_error` upstream: the process ends.
            if ctx.manager()?.thread(slot).start(body).is_err() {
                std::process::abort();
            }
            0
        }
        OP_THREAD_ENTER => {
            swoole_rs_corebase::swoole_thread_init(false);
            swoole_rs_corebase::set_thread(THREAD_WORKER, u16::try_from(arg).unwrap_or(u16::MAX));
            swoole_rs_ossupport::swr_v1_os_thread_native_id().cast_unsigned()
        }
        OP_THREAD_EXIT => {
            swoole_rs_corebase::swoole_thread_clean(false);
            0
        }
        OP_CREATE_MESSAGE_BUS => {
            let counter = core::ptr::from_ref(&ctx.server().counters().pipe_packet_msg_id);
            // SAFETY: the server's shared counters, mapped until `swr_v1_server_delete`,
            // after this thread has freed its bus (`OP_DESTROY_MESSAGE_BUS`).
            let bus = pool_native::new_bus(unsafe { &*counter });
            // SAFETY: the new bus, used by this thread alone.
            let handle = unsafe { Bus::of(bus) };
            handle.set_always_chunked();
            if !handle.alloc_buffer(ctx.config().ipc_max_size as usize) {
                std::alloc::handle_alloc_error(std::alloc::Layout::new::<u8>());
            }
            // SAFETY: a started server and its new bus.
            unsafe { server_methods::init_pipe_sockets(ctx.server(), bus) };
            swoole_rs_corebase::set_thread_message_bus(bus);
            0
        }
        OP_DESTROY_MESSAGE_BUS => {
            let bus = swoole_rs_corebase::thread_message_bus();
            // SAFETY: the bus `OP_CREATE_MESSAGE_BUS` made for this thread, freed once.
            unsafe { pool_native::free_bus(bus) };
            swoole_rs_corebase::set_thread_message_bus(null_mut());
            0
        }
        OP_WORKER_THREAD_START => {
            let thread = Arc::as_ptr(ctx.manager()?.thread(slot)).cast_mut().cast();
            let mut main = (*ctx, arg);
            // SAFETY: the slot's live thread, and `thread_main` over the context it lends.
            unsafe { worker_thread_start(thread, thread_main, (&raw mut main).cast()) };
            0
        }
        OP_REACTOR_THREAD_MAIN => {
            let id = c_int::try_from(arg).ok()?;
            crate::reactor_entry::reactor_thread_main_loop(serv, id);
            0
        }
        OP_SET_MASTER_THREAD_ID => {
            let id = u16::try_from(ctx.all_worker_num() + 1).unwrap_or(u16::MAX);
            swoole_rs_corebase::set_thread(swoole_rs_corebase::thread_kind(), id);
            Reactor::thread().fields().id = id;
            0
        }
        OP_START_MASTER_THREAD => {
            sign(server_methods::start_master_thread(serv, Reactor::thread()))
        }
        _ => return None,
    })
}

/// The facts and effects on the server, its tables and its ports.
fn server(ctx: &Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    let config = ctx.config();
    let port = || ctx.server().port_at(usize::try_from(arg).ok()?);
    Some(match op {
        OP_NEW_REACTOR_THREADS => {
            let threads = crate::factory_ops::new_reactor_threads(ctx.state(), config.reactor_num);
            ctx.tables().reactor_threads = threads;
            0
        }
        OP_DELETE_REACTOR_THREADS => {
            let threads = ctx.tables().reactor_threads;
            // SAFETY: the array `OP_NEW_REACTOR_THREADS` made, not used again.
            unsafe {
                crate::reactor_ops::ThreadRecord::delete_array(threads, config.reactor_num.into());
            }
            0
        }
        OP_MANAGER_THREAD_ID | OP_ALL_WORKER_NUM => ctx.all_worker_num(),
        OP_ALL_CORE_WORKER_NUM => u64::from(config.worker_num) + u64::from(config.task_worker_num),
        OP_WORKER_NUM => u64::from(config.worker_num),
        OP_TASK_WORKER_NUM => u64::from(config.task_worker_num),
        OP_USER_WORKER_NUM => ctx.workers().user_count as u64,
        OP_RUNNING => u64::from(config.running()),
        OP_STOPPING => u64::from(ctx.state().stopping.load(Ordering::Acquire)),
        OP_IS_MANAGER => u64::from(swoole_get_worker_type() == SW_MANAGER),
        OP_THREAD_REACTOR_NUM => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "`serv->reactor_num = serv->worker_num`, as upstream converts it"
            )]
            let reactor_num = config.worker_num as u16;
            // SAFETY: `create_thread_factory`, in `Server::create` before any thread
            // starts; `config` isn't read across the write.
            unsafe { serv.config_mut() }.reactor_num = reactor_num;
            0
        }
        OP_SINGLE_REACTOR_PIPE => {
            ctx.tables().reactor_pipe_num = 1;
            0
        }
        OP_CALLOC_CONNECTIONS => {
            // Private, as `sw_calloc`'s memory was.
            let tables = ctx.tables();
            let size = size_of::<ConnectionRecord>();
            tables.connection_list = Shm::calloc_private(tables.max_connection as usize, size);
            u64::from(!tables.connection_list.is_null())
        }
        OP_FREE_THREAD_CONNECTIONS => {
            ctx.tables().connection_list.free();
            0
        }
        OP_CREATE_WORKER_PIPES => {
            u64::from(crate::factory_ops::run_server(serv, RUN_CREATE_WORKER_PIPES) != 0)
        }
        OP_CREATE_EVENT_WORKERS => u64::from(create_event_workers(serv)),
        OP_CREATE_USER_WORKERS => {
            // SAFETY: `ThreadFactory::start`, before it starts a thread; `config`
            // isn't read across the call.
            u64::from(create_user_workers(serv, unsafe { serv.config_mut() }))
        }
        OP_TASK_START_CHECK => sign(start_check(ctx.server().task_pool_ref())),
        OP_STORE_LISTEN_SOCKET => {
            server_methods::store_listen_socket(ctx.server());
            0
        }
        OP_PORT_COUNT => ctx.server().ports().len() as u64,
        OP_PORT_DGRAM => u64::from(matches!(port()?.config().r#type, 2 | 4 | 6)),
        OP_PORT_LISTEN => sign((methods().listen)(port()?.raw())),
        OP_PORT_ADD => {
            let socket = ctx.server().port_socket_at(usize::try_from(arg).ok()?)?;
            Reactor::thread().add(socket, SW_EVENT_READ);
            0
        }
        OP_ON_MANAGER_START => ctx.call_server(CB_MANAGER_START),
        OP_ON_MANAGER_STOP => ctx.call_server(CB_MANAGER_STOP),
        OP_ON_BEFORE_RELOAD => ctx.call_server(CB_BEFORE_RELOAD),
        OP_ON_AFTER_RELOAD => ctx.call_server(CB_AFTER_RELOAD),
        OP_ABORT => std::process::abort(),
        _ => return None,
    })
}

/// The facts and effects on the context's worker and the thread manager.
fn worker(ctx: &mut Context<'_>, op: u32, arg: u64) -> Option<u64> {
    let (serv, worker) = (ctx.serv, ctx.worker);
    // The task pool's hook for `op`, over the pool and the context's worker. The hooks
    // are copied out first: the pool's own code, every task worker thread and PHP code
    // a hook runs (`$server->stats()`) reach the record while one runs.
    let pool_hook = |op| {
        let pool = ctx.workers().task_pool;
        // The borrow ends with the copy.
        let record = crate::shared::ServerShared::of(ctx.server()).task_pool();
        let (start, main_loop, stop) = (
            record.on_worker_start,
            record.main_loop,
            record.on_worker_stop,
        );
        let worker = ctx.worker();
        let pool = pool_native::pool_ref(pool);
        match (op, start, main_loop, stop) {
            (OP_ON_WORKER_START, Some(hook), ..) | (OP_ON_WORKER_STOP, .., Some(hook)) => {
                // SAFETY: the pool that holds the hook, and the worker it lends.
                unsafe { hook(pool, worker) };
            }
            // SAFETY: as above.
            (OP_POOL_MAIN_LOOP, _, Some(main_loop), _) => _ = unsafe { main_loop(pool, worker) },
            _ => {}
        }
        0
    };
    Some(match op {
        OP_WORKER_ID => u64::from(ctx.worker().id()),
        OP_WORKER_TYPE => u64::from(ctx.worker().kind()),
        OP_GET_WORKER => {
            let id = u16::try_from(arg).ok()?;
            ctx.worker = ctx.server().worker(id);
            ctx.worker.addr() as u64
        }
        OP_KILL_WORKER => {
            let id = c_int::try_from(arg.cast_signed()).ok()?;
            _ = crate::event_worker_ops::kill_worker(ctx.server(), id);
            0
        }
        OP_TERMINATE_MANAGER_THREAD => {
            // `ThreadFactory::terminate_manager_thread()` over the context's factory.
            over(serv, ctx.factory, RUN_THREAD_TERMINATE_MANAGER)
                .run(RUN_THREAD_TERMINATE_MANAGER, 0);
            0
        }
        OP_ON_USER_WORKER_START => {
            if let Some(slot) = ctx.state().callbacks.slot(CB_USER_WORKER_START) {
                // SAFETY: its `(Server *, Worker *)` slot, with the context's live worker.
                unsafe { slot.call(serv.raw(), worker.cast()) };
            }
            0
        }
        OP_ON_WORKER_START | OP_POOL_MAIN_LOOP | OP_ON_WORKER_STOP => pool_hook(op),
        _ => return None,
    })
}
