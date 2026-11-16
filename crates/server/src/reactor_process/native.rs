//! The base-mode reactor process's native objects: its event worker pool and the
//! reads from the server's message bus.
use core::ffi::c_int;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::process_pool::{Hooks, IpcMode, PoolRef};
use swoole_rs_process::worker_record::Worker;
use swoole_rs_reactor::Events;
use swoole_rs_reactor::{Event, Running};

use crate::callbacks::TaskSlot;
use crate::objects::Bus;
use crate::reactor_handle::Reactor;
use crate::view;

/// `SW_FD_PIPE`, `Server::CLOSE_ACTIVELY`.
const FD_PIPE: c_int = 3;
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// The event worker pool over the server's workers.
pub(crate) fn create_pool(serv: view::Server<'_>) -> bool {
    let config = serv.config();
    let pool_ref = serv.event_pool_ref();
    // The server's event pool, reset as `*pool = {}` did.
    pool_ref.reset();
    let ipc = IpcMode::UnixSock.code().into();
    if !swoole_rs_process::methods::create(pool_ref, config.worker_num.get(), 0, ipc) {
        return false;
    }
    swoole_rs_process::methods::set_max_request(
        pool_ref,
        config.max_request.get(),
        config.max_request_grace.get(),
    );
    crate::shared::hold_server(serv);
    let hooks = Hooks {
        main_loop: Some(main_loop),
        on_worker_not_found: Some(worker_not_found),
        ..Hooks::default()
    };
    pool_ref.install(&hooks);
    pool_ref.set_max_wait_time(config.max_wait_time.get());
    pool_ref.set_use_msgqueue(false);
    // `create` set the pool's `worker_num` to the server's, so the pool's records
    // become the server's.
    serv.workers().adopt_event_workers();
    true
}

/// `pool->main_loop`: `Server::reactor_process_main_loop` over the pool's worker.
extern "C-unwind" fn main_loop(pool: PoolRef<'_>, worker: Worker<'_>) -> c_int {
    let Some(serv) = crate::shared::pool_server(pool) else {
        return -1;
    };
    if super::main_loop(serv, worker) {
        0
    } else {
        -1
    }
}

/// `pool->onWorkerNotFound`: restart the event worker that exited. The pool lends
/// itself and the exit status for the call.
extern "C-unwind" fn worker_not_found(pool: PoolRef<'_>, status: &ExitStatus) {
    if let Some(serv) = crate::shared::pool_server(pool) {
        crate::manager::native::restart_worker_process(serv, status);
    }
}

/// This thread's reactor reads its pipes with [`on_pipe_read`].
pub(crate) fn set_pipe_handler() {
    Reactor::thread().set_handler(FD_PIPE, Events::READ, on_pipe_read);
}

/// `ReactorProcess_onPipeRead`: a message from another worker or a task worker.
fn on_pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let reactor = Reactor::running(reactor);
    let Some(serv) = reactor.server() else {
        return -1;
    };
    let bus = serv.state().message_bus();
    if event
        .socket()
        .is_some_and(|socket| bus.read(socket) > Some(0))
        && dispatch(serv, reactor, bus)
    {
        bus.pop();
    }
    0
}

/// The message the bus just read, by its kind; false, leaving it on the bus, when
/// data comes from no known worker.
fn dispatch(serv: view::Server<'_>, reactor: Reactor, bus: Bus<'_>) -> bool {
    let state = serv.state();
    let info = bus.head();
    match info.event() {
        Some(ServerEvent::PipeMessage) => {
            _ = bus.with_event(|event| state.callbacks.pipe_message(serv, event));
        }
        Some(ServerEvent::Finish) => {
            _ = bus.with_event(|event| state.callbacks.task(TaskSlot::Finish, serv, event));
        }
        Some(ServerEvent::Shutdown) => {
            if let Some(worker) = serv.worker_record(reactor.handle().id()) {
                crate::event_worker::native::stop_async_worker(serv, worker);
            }
        }
        Some(ServerEvent::SendFile) => {
            bus.with_message(&info, |send| crate::factory::finish(serv, send));
        }
        Some(ServerEvent::SendData) => {
            let workers = serv.workers();
            let all = u64::from(workers.event_count())
                + u64::from(workers.task_count())
                + workers.user_count() as u64;
            let worker = info.reactor_id;
            if u64::try_from(worker).is_ok_and(|id| id < all) {
                let mut info = info;
                info.kind = ServerEvent::RecvData.code();
                bus.with_message(&info, |send| crate::factory::finish(serv, send));
            } else {
                let m = format_args!("invalid worker_id={worker}");
                swoole_rs_corebase::log::warning(c"ReactorProcess_onPipeRead", m);
                return false;
            }
        }
        Some(ServerEvent::Close | ServerEvent::CloseForward) => {
            _ = crate::factory::end(serv, info.fd, CLOSE_ACTIVELY);
        }
        Some(ServerEvent::CommandRequest) => {
            // The worker this process runs.
            let me = swoole_rs_process::worker_globals::with_current(|me| me.id());
            let pipe = serv.worker_pipe(0, true);
            if let Some(me) = me {
                let id = u16::try_from(me).unwrap_or(0);
                crate::commands::call_command_handler(serv, bus, id, pipe);
            }
        }
        Some(ServerEvent::CommandResponse) => {
            bus.with_packet(|reply| crate::commands::command_result(state, info.fd, reply));
        }
        _ => {}
    }
    true
}
