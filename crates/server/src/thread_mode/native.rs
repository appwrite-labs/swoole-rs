//! Thread mode's native side: the thread factory and its connections, the worker,
//! user, task and manager threads' bodies, the manager thread's loop and the reloads
//! it walks.
use crate::Mode;
use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering;

use swoole_rs_corebase::{CCast, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_memory::members::Shm;
use swoole_rs_process::methods::{
    ExitStatus, start_check, swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type,
    worker_type,
};
use swoole_rs_process::worker_record::{Worker, WorkerStatus, WorkerType};

use super::{ThreadManager, Wake, worker_thread_start};
use crate::callbacks::ServerSlot;
use crate::objects::{Bus, Reactor};
use crate::reactor_ops::PoolBus;
use crate::records::ConnectionRecord;
use crate::worker_methods::{create_event_workers, create_user_workers};
use crate::{factory, pool_native, server_methods, view};

/// `SW_EVENT_READ` and `Server::THREAD_WORKER`.
const EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const THREAD_WORKER: u8 = 4;
/// `SW_LOG_WARNING`, `SW_LOG_ERROR`.
const WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;
const ERROR: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_ERROR as c_int;

/// The server's thread factory's manager; thread mode's server has one.
fn manager(serv: view::Server<'_>) -> std::sync::Arc<ThreadManager> {
    let manager = (serv.config().mode_ == Mode::Thread)
        .then(|| factory::manager(serv.state()))
        .flatten();
    manager.unwrap_or_else(|| swoole_rs_corebase::fatal("thread mode without its factory"))
}

/// `serv->get_all_worker_num()`.
fn all_worker_num(serv: view::Server<'_>) -> u32 {
    let workers = serv.workers();
    let users = u32::try_from(workers.user_count()).unwrap_or(u32::MAX);
    workers.event_count() + workers.task_count() + users
}

/// The manager thread's slot, after every worker's; the master's is next.
fn manager_thread_id(serv: view::Server<'_>) -> u32 {
    all_worker_num(serv)
}

/// The event and task workers: the threads `kill_worker` and reloads reach.
fn all_core_worker_num(serv: view::Server<'_>) -> u16 {
    let config = serv.config();
    let all = config.worker_num.get() + config.task_worker_num.get();
    u16::try_from(all).unwrap_or(u16::MAX)
}

/// `Server::create_thread_factory`: one reactor per worker over a private
/// connection list and one pipe each; null without thread support or memory.
pub(crate) fn create_factory(serv: view::Server<'_>) -> *mut c_void {
    const ORIGIN: &core::ffi::CStr = c"Server::create_thread_factory";
    let (state, config) = (serv.state(), serv.config());
    if !crate::THREAD_SUPPORTED.load(Ordering::Relaxed) {
        log::error_log(
            ERROR,
            E::SW_ERROR_OPERATION_NOT_SUPPORT as c_int,
            ORIGIN,
            format_args!("Thread support is not enabled, cannot create server with Mode::Thread"),
        );
        return core::ptr::null_mut();
    }
    config
        .reactor_num
        .set(config.worker_num.get().c_cast::<u16>());
    // Private, as `sw_calloc`'s memory was.
    let max = state.tables_ref().max_connection as usize;
    let list = Shm::calloc_private(max, size_of::<ConnectionRecord>());
    if list.is_null() {
        let size = (max * size_of::<ConnectionRecord>()).c_cast::<c_int>();
        log::sys_warning(ORIGIN, format_args!("calloc[2]({size}) failed"));
        return core::ptr::null_mut();
    }
    let threads =
        crate::process_factory::native::new_reactor_threads(state, config.reactor_num.get());
    // SAFETY: the server's tables, which only setup and teardown write, while the
    // master is the server's only thread; nothing reads them while this lives.
    let tables = unsafe { &mut *state.tables() };
    tables.connection_list = list;
    tables.install_reactor_threads(threads);
    tables.reactor_pipe_num = 1;
    factory::new_thread(state, all_worker_num(serv) as usize + 1)
}

/// `Server::destroy_thread_factory`: the connections and the reactor threads.
pub(crate) fn destroy_factory(serv: view::Server<'_>) {
    // SAFETY: as `create_factory`'s: the worker threads are joined, and nothing reads
    // the tables while this lives.
    let tables = unsafe { &mut *serv.state().tables() };
    tables.connection_list.free();
    drop(tables.take_reactor_threads());
}

/// `ThreadFactory::start`: the pipes and the event, task and user workers' records.
pub(crate) fn factory_start(serv: view::Server<'_>) -> bool {
    let config = serv.config();
    let user_workers = || {
        let workers = create_user_workers(serv);
        let created = !workers.is_null();
        // SAFETY: `ThreadFactory::start`, before it starts a thread; `config` isn't
        // read after the write.
        unsafe { serv.config_mut() }.user_workers = workers;
        created
    };
    crate::process_factory::native::create_worker_pipes(serv)
        && create_event_workers(serv)
        && (config.task_worker_num.get() == 0 || start_check(serv.task_pool_ref()))
        && (serv.user_worker_num() == 0 || user_workers())
}

/// `Server::start_worker_threads`: the task, event and user worker threads, the
/// manager thread, then the master thread's listeners and loop.
pub(crate) fn start_worker_threads(serv: view::Server<'_>) -> bool {
    let config = serv.config();
    let (workers, tasks) = (config.worker_num.get(), config.task_worker_num.get());
    for id in 0..tasks {
        spawn(serv, workers + id, WorkerType::Task);
    }
    for id in 0..workers {
        spawn(serv, id, WorkerType::Event);
    }
    let users = u32::try_from(serv.workers().user_count()).unwrap_or(u32::MAX);
    for id in 0..users {
        spawn(serv, tasks + workers + id, WorkerType::User);
    }
    spawn(serv, manager_thread_id(serv), WorkerType::Manager);
    if swoole_rs_reactor::swoole_event_init(0) < 0 {
        return false;
    }
    let reactor = Reactor::thread();
    for at in 0..serv.ports().len() {
        let Some(port) = serv.port_at(at) else {
            continue;
        };
        if matches!(port.config().r#type, 2 | 4 | 6) {
            continue;
        }
        if !(crate::port_config::methods().listen)(port.config()) {
            swoole_rs_reactor::swoole_event_free();
            return false;
        }
        if let Some(socket) = serv.port_socket_at(at) {
            reactor.add(socket, EVENT_READ);
        }
    }
    let id = u16::try_from(all_worker_num(serv) + 1).unwrap_or(u16::MAX);
    swoole_rs_corebase::set_thread(swoole_rs_corebase::thread_kind(), id);
    reactor.handle().set_id(id);
    server_methods::store_listen_socket(serv);
    server_methods::start_master_thread(serv, reactor)
}

/// `Server::stop_worker_threads`: the manager thread, then each worker.
pub(crate) fn stop_worker_threads(serv: view::Server<'_>) {
    // `ThreadFactory::terminate_manager_thread()`.
    let manager = &manager(serv);
    manager.push(Wake::Exit);
    // The manager thread may still be joining workers when the server stops:
    // reclaim it first, so it has surely exited.
    join(manager, manager_thread_id(serv));
    for id in 0..all_core_worker_num(serv) {
        _ = crate::event_worker::native::kill_worker(serv, id.into());
    }
}

/// Joins slot `id`'s thread; a failed join was an uncaught `std::system_error`.
fn join(manager: &ThreadManager, id: u32) {
    if manager.thread(id as usize).join().is_err() {
        swoole_rs_corebase::fatal("a worker thread's join failed");
    }
}

/// `ThreadFactory::spawn_*`: start slot `id`'s thread as a worker of `kind`.
fn spawn(serv: view::Server<'_>, id: u32, kind: WorkerType) {
    let address = serv.raw().expose_provenance();
    let body = move || {
        let serv = core::ptr::with_exposed_provenance_mut(address);
        // SAFETY: the live server, which outlives the factory's threads.
        body(unsafe { view::Server::from_raw(serv) }, id, kind);
    };
    // An uncaught `std::system_error` upstream: the process ends.
    if manager(serv).thread(id as usize).start(body).is_err() {
        swoole_rs_corebase::fatal("a worker thread can't start");
    }
}

/// A thread's body: enter the thread as its worker (or the manager), run its main
/// inside the server's `worker_thread_start`, then leave, telling the manager
/// thread when a worker exited.
fn body(serv: view::Server<'_>, id: u32, kind: WorkerType) {
    let slot = u16::try_from(id).unwrap_or(u16::MAX);
    swoole_rs_corebase::swoole_thread_init(false);
    swoole_rs_corebase::set_thread(THREAD_WORKER, slot);
    let pid = swoole_rs_ossupport::swr_v1_os_thread_native_id().c_cast::<i32>();
    swoole_set_worker_type(kind as c_int);
    swoole_set_worker_id(id);
    swoole_set_worker_pid(pid);
    // The thread's own bus, which outlives its use as the thread's below.
    let bus = matches!(kind, WorkerType::Task | WorkerType::User).then(|| Box::new(new_bus(serv)));
    if let Some(bus) = &bus {
        init_bus(serv, bus);
        swoole_rs_corebase::set_thread_message_bus(
            core::ptr::from_ref::<PoolBus>(bus).cast_mut().cast(),
        );
    }
    let worker = (kind != WorkerType::Manager).then(|| {
        let worker = serv.worker_record(slot);
        let worker =
            worker.unwrap_or_else(|| swoole_rs_corebase::fatal("no worker for this thread"));
        worker.set_kind(kind.code());
        worker.set_pid(pid);
        if kind == WorkerType::Task {
            worker.set_status(WorkerStatus::Idle);
        }
        swoole_rs_process::worker_globals::set_current_worker(worker.raw());
        worker
    });
    if worker.is_none() {
        swoole_rs_timer::swoole_timer_create(true);
    }
    let mut main = Main { serv, slot, worker };
    let factory = manager(serv);
    let thread = factory.thread(id as usize);
    // `serv->worker_thread_start(thread, fn)`: the registered starter runs
    // `thread_main` over the `Main` it is lent, which outlives the call.
    match worker_thread_start() {
        Some(start) => start(thread, thread_main, (&raw mut main).cast()),
        None => run_main(&main),
    }
    if bus.is_some() {
        swoole_rs_corebase::set_thread_message_bus(core::ptr::null_mut());
    }
    drop(bus);
    if worker.is_some() {
        factory.push(Wake::Exited(slot));
    } else {
        if serv.state().running() {
            let m = format_args!("Fatal Error: manager thread exits abnormally");
            log::warning(c"ThreadFactory::spawn_manager_thread", m);
        }
        if swoole_rs_timer::swoole_timer_is_available() {
            swoole_rs_timer::swoole_timer_free();
        }
    }
    swoole_rs_corebase::swoole_thread_clean(false);
}

/// The task and user workers' own message bus.
fn new_bus(serv: view::Server<'_>) -> PoolBus {
    let counter = serv.state().msg_ids.get().copied();
    pool_native::new_bus(
        counter
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a server without its shared counters")),
    )
}

/// Readies the thread's new bus, used by this thread alone, at the address it keeps.
fn init_bus(serv: view::Server<'_>, bus: &PoolBus) {
    let handle = Bus::over(bus);
    handle.set_always_chunked();
    if !handle.alloc_buffer(serv.config().ipc_max_size() as usize) {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<u8>());
    }
    server_methods::init_pipe_sockets(serv, handle);
}

/// What a thread runs inside `worker_thread_start`: its server, slot and worker.
struct Main<'a> {
    serv: view::Server<'a>,
    slot: u16,
    /// `None` for the manager.
    worker: Option<Worker<'a>>,
}

/// [`run_main`] as the `fn(void *)` a registered starter runs.
///
/// # Safety
/// `main` is [`body`]'s live `Main`, lent for the call.
unsafe extern "C" fn thread_main(main: *mut c_void) {
    // SAFETY: per the caller.
    run_main(unsafe { &*main.cast::<Main<'_>>() });
}

/// A thread's main inside `worker_thread_start`.
fn run_main(main: &Main<'_>) {
    let Main { serv, slot, worker } = main;
    let serv = *serv;
    let Some(worker) = *worker else {
        let callbacks = &serv.state().callbacks;
        callbacks.call_server(ServerSlot::ManagerStart, serv);
        wait(serv, &manager(serv));
        callbacks.call_server(ServerSlot::ManagerStop, serv);
        return;
    };
    // The task pool copies a hook out before it runs it: the pool's own code, every
    // task worker thread and PHP code a hook runs (`$server->stats()`) reach the
    // record meanwhile.
    let pool = serv.task_pool_ref();
    match worker.kind() {
        kind if kind == WorkerType::Event.code() => crate::thread_ops::main_loop(serv, *slot),
        kind if kind == WorkerType::Task.code() => {
            pool.call_on_worker_start(worker);
            _ = pool.call_main_loop(worker);
            pool.call_on_worker_stop(worker);
        }
        _ => serv.state().callbacks.user_worker_start(serv, worker),
    }
}

/// `ThreadFactory::wait`: the manager thread's loop.
fn wait(serv: view::Server<'_>, manager: &ThreadManager) {
    loop {
        match manager.next(swoole_rs_timer::swoole_timer_get_next_msec()) {
            Some(Wake::Reload) => {
                reload_next(serv, manager);
                continue;
            }
            Some(Wake::Exit) => break,
            Some(Wake::Exited(slot)) => restart(serv, manager, slot),
            None => {}
        }
        if swoole_rs_timer::swoole_timer_is_available() {
            swoole_rs_timer::swoole_timer_select();
        }
        if serv.state().running() && manager.reloading() {
            reload_next(serv, manager);
        }
    }
}

/// Rejoin an exited worker thread, report an abnormal exit, and respawn it.
fn restart(serv: view::Server<'_>, manager: &ThreadManager, slot: u16) {
    let Some(worker) = serv.worker_record(slot) else {
        swoole_rs_corebase::fatal("an exited thread without its worker");
    };
    let id = worker.id();
    let exit_status = manager.thread(id as usize).exit_status();
    if exit_status != 0 {
        let status = ExitStatus::new(worker.pid(), exit_status << 8);
        crate::event_worker::native::call_worker_error_callback(serv, worker, &status);
    }
    join(manager, id);
    // A worker that exits once the server is stopping stays down: the master is
    // joining every thread, and a new one could block it forever (a user process
    // re-run waits on input its first run consumed).
    if serv.state().stopping.load(Ordering::Acquire) {
        return;
    }
    let kind = [WorkerType::Event, WorkerType::Task, WorkerType::User]
        .into_iter()
        .find(|kind| kind.code() == worker.kind());
    let Some(kind) = kind else {
        swoole_rs_corebase::fatal("an exited thread of an unknown worker type");
    };
    spawn(serv, id, kind);
}

/// `ThreadFactory::do_reload`: ask the next worker to stop.
fn reload_next(serv: view::Server<'_>, manager: &ThreadManager) {
    let callbacks = &serv.state().callbacks;
    let events = u16::try_from(serv.config().worker_num.get()).unwrap_or(u16::MAX);
    manager.reload_next(
        (all_core_worker_num(serv), events),
        || callbacks.call_server(ServerSlot::BeforeReload, serv),
        |id| _ = crate::event_worker::native::kill_worker(serv, id.into()),
        || callbacks.call_server(ServerSlot::AfterReload, serv),
    );
}

/// `Server::reload_worker_threads(reload_all_workers)`: start a reload, in the
/// manager thread or by asking it.
pub(crate) fn reload_worker_threads(serv: view::Server<'_>, reload_all_workers: bool) -> bool {
    let which = if reload_all_workers { "all" } else { "task" };
    if serv.config().task_worker_num.get() == 0 && !reload_all_workers {
        log::error_log(
            WARNING,
            E::SW_ERROR_OPERATION_NOT_SUPPORT as c_int,
            c"ThreadFactory::reload",
            format_args!("Cannot reload {which} workers, task workers are not started"),
        );
        return false;
    }
    let manager = &manager(serv);
    // One reload at a time.
    if !manager.begin_reload(reload_all_workers) {
        log::set_last_error(E::SW_ERROR_OPERATION_NOT_SUPPORT as c_int);
        return false;
    }
    if worker_type() == Some(WorkerType::Manager) {
        log::info(format_args!("Server is reloading {which} workers now"));
        reload_next(serv, manager);
    } else {
        log::info(format_args!(
            "Send a notification to the manager process to prepare for restarting {which} \
             worker processes."
        ));
        manager.push(Wake::Reload);
    }
    true
}
