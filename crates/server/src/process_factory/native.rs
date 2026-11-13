//! The process-mode factory's native side: its reactor threads, barriers,
//! connections and message box, the manager's start and stop, forking event and user
//! workers, and the [`Pipe`] writes.
use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_memory::members::Shm;
use swoole_rs_process::methods::{
    ExitStatus, kill_all_workers, swoole_set_worker_id, swoole_set_worker_pid,
    swoole_set_worker_type, worker_report_error,
};
use swoole_rs_process::worker_record::{Worker, WorkerType};
use swoole_rs_reactor::wait_upstream::swoole_waitpid;

use super::{Pipe, worker_id};
use crate::base_factory::SendData;
use crate::reactor_ops::ThreadRecord;
use crate::{ConnectionRecord, view};

/// `SWR_WORKER_EVENT`, `SWR_WORKER_USER`: the registry's categories.
const REGISTRY_EVENT: c_int = 1;
const REGISTRY_USER: c_int = 2;
/// `SW_PIPE_MASTER`.
const PIPE_MASTER: c_int = 1;

/// `new ReactorThread[count]()`, each bus over the allocator of the server's.
pub(crate) fn new_reactor_threads(state: &crate::ServerState, count: u16) -> *mut c_void {
    ThreadRecord::new_array(count.into(), state.message_bus().allocator())
}

/// Initializes or destroys the reactor threads' and the manager's barriers.
fn barriers(serv: view::Server<'_>, init: bool) {
    let reactors = c_int::from(serv.config().reactor_num.get()) + 1;
    for (manager, count) in [(false, reactors), (true, 2)] {
        let barrier = crate::server_object::barrier(serv, manager);
        if init {
            swoole_rs_locks::barrier_init(barrier, manager, count);
        } else {
            swoole_rs_locks::barrier_destroy(barrier);
        }
    }
}

/// `Server::create_process_factory`: the factory, or null.
pub(crate) fn create(serv: view::Server<'_>) -> *mut c_void {
    let (state, config) = (serv.state(), serv.config());
    let threads = new_reactor_threads(state, config.reactor_num.get());
    let max = state.tables_ref().max_connection;
    let list = Shm::calloc(max as usize, size_of::<ConnectionRecord>());
    let pipes = config.worker_num.get() / u32::from(config.reactor_num.get());
    // SAFETY: the server's tables, which only setup and teardown write, while the
    // master is the server's only thread; nothing reads them while this lives.
    let tables = unsafe { &mut *state.tables() };
    tables.reactor_threads = threads;
    if list.is_null() {
        let size = size_of::<ConnectionRecord>();
        let m = format_args!("sw_shm_calloc({max}, {size}) for connection_list failed");
        log::sys_warning(c"Server::create_process_factory", m);
        return core::ptr::null_mut();
    }
    tables.connection_list = list;
    tables.reactor_pipe_num = u16::try_from(pipes & 0xffff).unwrap_or_default();
    barriers(serv, true);
    crate::factory::new(serv.raw(), None)
}

/// `Server::destroy_process_factory`.
pub(crate) fn destroy(serv: view::Server<'_>) {
    // SAFETY: as `create`'s, for its one statement.
    unsafe { &mut *serv.state().tables() }
        .connection_list
        .free();
    let (tables, config) = (serv.state().tables_ref(), serv.config());
    // SAFETY: the array `create` made, not used again.
    unsafe { ThreadRecord::delete_array(tables.reactor_threads, config.reactor_num.get().into()) };
    barriers(serv, false);
    let pool = crate::shared::ServerShared::of(serv).event_pool();
    if !pool.message_box().is_null() {
        // The event pool's box, which the pool forgets as it frees it.
        swoole_rs_process::methods::free_message_box(serv.event_pool_ref());
    }
}

/// `Server::create_worker_pipes`: a pipe per event worker, then the pipe buffers.
pub(crate) fn create_worker_pipes(serv: view::Server<'_>) -> bool {
    let state = serv.state();
    for id in 0..serv.config().worker_num.get() {
        let worker = serv.worker_record(worker_id(id.into()));
        if !worker.is_some_and(|worker| state.push_worker_pipe(worker)) {
            return false;
        }
    }
    serv.config().init_ipc_max_size();
    crate::master_ops::create_pipe_buffers(serv) >= 0
}

/// `ProcessFactory::start`: the workers' pipes, then the manager.
pub(crate) fn start(serv: view::Server<'_>) -> bool {
    create_worker_pipes(serv) && crate::manager_ops::start_manager_process(serv) == 0
}

/// `ProcessFactory::shutdown`: SIGTERM the manager and wait for it. False, as
/// upstream returns `SW_OK` from this bool function.
pub(crate) fn shutdown(serv: view::Server<'_>) -> bool {
    let pid = serv.counters().manager_pid.load(Relaxed);
    if swoole_rs_process::os::kill(pid, libc::SIGTERM) < 0 {
        log::sys_warning(
            c"ProcessFactory::shutdown",
            format_args!("kill({pid}) failed"),
        );
    }
    if swoole_waitpid(pid, None, 0) < 0 {
        log::sys_warning(
            c"ProcessFactory::shutdown",
            format_args!("waitpid({pid}) failed"),
        );
    }
    false
}

/// `Factory::kill_task_workers`: SIGTERM each task worker, then wait for each.
pub(crate) fn kill_task_workers(serv: view::Server<'_>) {
    let count = serv.config().task_worker_num.get();
    if count == 0 {
        return;
    }
    kill_all_workers(serv.task_pool_ref(), libc::SIGTERM);
    let pool = crate::shared::ServerShared::of(serv).task_pool();
    for index in 0..count as usize {
        let pid = pool.worker(index).map_or(0, Worker::pid);
        if swoole_waitpid(pid, None, 0) < 0 {
            log::sys_warning(
                c"Factory::kill_task_workers",
                format_args!("waitpid({pid}) failed"),
            );
        }
    }
}

/// Makes this forked child `worker`, of `kind`.
fn enter_child(worker: Worker<'_>, kind: WorkerType) {
    worker.set_pid(std::process::id().cast_signed());
    swoole_set_worker_id(worker.id());
    swoole_set_worker_pid(worker.pid());
    swoole_set_worker_type(kind as c_int);
    swoole_rs_process::worker_globals::set_current_worker(worker.raw());
}

/// `Factory::spawn_event_worker`: fork; the child runs the worker and exits.
pub(crate) fn spawn_event_worker(serv: view::Server<'_>, worker: Worker<'_>) -> c_int {
    let pid = swoole_rs_corebase::swoole_fork(0);
    if pid < 0 {
        let m = format_args!("failed to fork event worker");
        log::sys_warning(c"Factory::spawn_event_worker", m);
        return -1;
    }
    if pid > 0 {
        worker.set_pid(pid);
        serv.registry()
            .register(pid, worker.raw().cast(), REGISTRY_EVENT);
        return pid;
    }
    enter_child(worker, WorkerType::Event);
    if u64::from(serv.config().mode_) == crate::MODE_BASE {
        _ = serv.event_pool_ref().call_main_loop(worker);
    } else {
        _ = crate::event_worker_ops::start_event_worker(serv, worker);
    }
    // exit(3), as upstream.
    std::process::exit(0)
}

/// `Factory::spawn_user_worker`: fork; the child runs the user's process and exits.
pub(crate) fn spawn_user_worker(serv: view::Server<'_>, worker: Worker<'_>) -> c_int {
    let pid = swoole_rs_corebase::swoole_fork(0);
    // Both processes forget the worker's previous pid.
    if worker.pid() != 0 {
        _ = serv.registry().unregister(worker.pid());
    }
    if pid < 0 {
        let m = format_args!("failed to spawn the user worker");
        log::sys_warning(c"Factory::spawn_user_worker", m);
        return -1;
    }
    if pid == 0 {
        enter_child(worker, WorkerType::User);
        serv.state().callbacks.user_worker_start(serv, worker);
        std::process::exit(0)
    }
    // The worker is local memory, the server's copy shared memory: set both.
    worker.set_pid(pid);
    if let Some(shared) = serv.worker_record(worker_id(worker.id().into())) {
        shared.set_pid(pid);
    }
    serv.registry()
        .register(pid, worker.raw().cast(), REGISTRY_USER);
    pid
}

/// `Factory::check_worker_exit_status`: a worker that exited with an error is reported
/// and its error callback runs.
pub(crate) fn check_exit_status(
    serv: view::Server<'_>,
    worker: Option<Worker<'_>>,
    status: &ExitStatus,
) {
    if let Some(worker) = worker.filter(|_| status.status() != 0) {
        worker_report_error(worker, status);
        crate::event_worker_ops::call_worker_error_callback(serv, worker, status);
    }
}

/// [`Pipe`]'s writes of `send`.
pub(crate) fn send(serv: view::Server<'_>, to: Pipe, send: &mut SendData) -> bool {
    let state = serv.state();
    match to {
        Pipe::Worker(id) => {
            let pipe = serv.worker_record(id);
            let Some(pipe) = pipe.and_then(|worker| crate::objects::worker_pipe(worker, true))
            else {
                return false;
            };
            // This reactor thread's bus, and its copy of the worker's pipe.
            let thread = state
                .tables_ref()
                .reactor_threads
                .cast::<ThreadRecord>()
                .wrapping_add(usize::from(swoole_rs_corebase::thread_id()));
            // SAFETY: this reactor thread's live record.
            let bus = unsafe { &*thread }.message_bus();
            let Some(socket) = bus.pipe_socket(pipe) else {
                return false;
            };
            // SAFETY: the live pipe, and the data sent.
            unsafe {
                bus.rust()
                    .send(socket.as_ptr(), &mut send.info, send.data.cast())
            }
        }
        Pipe::Reactor(id) => {
            let Some(worker) = serv.worker_record(id) else {
                return false;
            };
            let bus = state.message_bus();
            // SAFETY: the live pipe, and the data sent.
            unsafe {
                bus.rust().send(
                    worker.pipe_worker().cast(),
                    &mut send.info,
                    send.data.cast(),
                )
            }
        }
        Pipe::Close(id) => {
            let Some(target) = serv.worker_record(id) else {
                return false;
            };
            let head = (&raw const send.info).cast::<u8>();
            // SAFETY: a worker of the live server and a head of the size passed.
            let sent = unsafe {
                swoole_rs_process::methods::send_pipe_message(
                    target.raw(),
                    head,
                    size_of::<DataHead>(),
                    PIPE_MASTER,
                )
            };
            sent > 0
        }
    }
}
