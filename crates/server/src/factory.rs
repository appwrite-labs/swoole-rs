//! `serv->factory_`: upstream's `Factory` and its `BaseFactory`, `ProcessFactory` and
//! `ThreadFactory`, as one record whose members dispatch on the server's mode. Thread
//! mode keeps the base factory's dispatch, finish, notify and end, as upstream's
//! `ThreadFactory : BaseFactory` did.
use core::ffi::{c_int, c_long, c_void};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::process;
use swoole_rs_corebase::reactor::Events;

use crate::base_factory::SendData;
use crate::thread_mode::ThreadManager;
use swoole_rs_corebase::DataHead;

use crate::state::ServerState;

/// [`crate::hybrid::Step`] over `serv`, in the master.
fn lifecycle_step(serv: crate::view::Server<'_>, step: crate::hybrid::Step) -> bool {
    use crate::hybrid::Step;
    use swoole_rs_corebase::reactor::signal_upstream::{self, Handler};
    /// `SW_FD_PIPE` and `SW_EVENT_READ`.
    const SW_FD_PIPE: c_int = 3;
    let state = serv.state();
    match step {
        Step::Daemon => {
            match swoole_rs_corebase::utilities::process_logger() {
                Some(logger) if logger.is_opened() => {
                    logger.redirect_stdout_and_stderr(true);
                }
                _ => swoole_rs_corebase::swoole_redirect_stdout_file(c"/dev/null"),
            }
            // The master, before it starts any thread.
            return swoole_rs_corebase::ossupport::swoole_daemon(0, 1) >= 0;
        }
        Step::FactoryStart => return start(serv),
        // In the master, after the process factory forks and before any reactor
        // thread, base-mode worker or worker thread starts.
        Step::InitProtocols => {
            for port in &state.ports {
                (crate::port_config::methods().init_protocol)(port.config());
            }
        }
        Step::StartProcess => return crate::thread_ops::start_reactor_threads(serv),
        Step::TimerReinit => {
            if swoole_rs_corebase::timer::thread_timer_detached() {
                swoole_rs_corebase::timer::thread_timer_reinit(false);
            }
        }
        Step::Ignore(signo) => _ = signal_upstream::set(signo, Handler::NONE, false),
        Step::Handle(signo) => {
            let handler = Handler::new(crate::server_methods::master_signal_handler);
            _ = signal_upstream::set(signo, handler, false);
        }
        Step::CommandHandler => {
            let handler: swoole_rs_corebase::reactor::Handler = accept_command_result;
            crate::reactor_handle::Reactor::thread().set_handler(SW_FD_PIPE, Events::READ, handler);
        }
        Step::MasterTimer => {
            let timer = crate::server_methods::master_timer();
            state.config_ref().master_timer.store(timer);
            return timer.is_some();
        }
        Step::EventFree => _ = swoole_rs_corebase::reactor::swoole_event_free(),
        Step::EventWait => return swoole_rs_corebase::reactor::event_api::swoole_event_wait() >= 0,
        Step::ReactorBarrier | Step::ManagerBarrier => {
            let manager = step == Step::ManagerBarrier;
            swoole_rs_corebase::locks::barrier_wait(crate::server_object::barrier(serv, manager));
        }
        Step::JoinReactorThreads => crate::reactor_thread::join_reactor_threads(serv),
        Step::FactoryShutdown => _ = shutdown(serv),
        Step::ReleasePipeBuffers => state.message_bus().free_buffer(),
        Step::ClosePorts => {
            for port in &state.ports {
                (crate::port_config::methods().close)(port.config());
            }
        }
        Step::ClearSignals => signal_upstream::clear(),
        Step::DestroyProcessFactory => crate::process_factory::native::destroy(serv),
        Step::DestroyTaskPool => {
            process::methods::destroy(crate::process_pool_native(), serv.task_pool_ref());
        }
        Step::DeleteFactory => delete(state),
        Step::Listen(index) | Step::ClosePortSocket(index) | Step::CreatePortSocket(index) => {
            let Some(port) = state.ports.get(index) else {
                return false;
            };
            let (methods, config) = (crate::port_config::methods(), port.config());
            return match step {
                Step::Listen(_) => (methods.listen)(config),
                Step::CreatePortSocket(_) => {
                    (methods.create_socket)(config, serv.config().enable_reuse_port.get())
                }
                _ => {
                    (methods.close_socket)(config);
                    true
                }
            };
        }
        Step::CreateEventPool => return crate::reactor_process::native::create_pool(serv),
        Step::DestroyEventPool => {
            process::methods::destroy(crate::process_pool_native(), serv.event_pool_ref());
        }
        Step::CreatePipeBuffers => return crate::master_ops::create_pipe_buffers(serv),
        Step::StartManager => return crate::manager::native::start_manager_process(serv),
        Step::EventInit => return swoole_rs_corebase::reactor::swoole_event_init(0) >= 0,
        Step::WorkerSignalInit => crate::event_worker::native::worker_signal_init(serv),
        Step::PipeHandler => crate::reactor_process::native::set_pipe_handler(),
        Step::WorkerStart(id) | Step::WorkerStop(id) => {
            let Some(worker) = serv.worker_record(u16::try_from(id).unwrap_or(u16::MAX)) else {
                return false;
            };
            if step == Step::WorkerStart(id) {
                crate::event_worker::native::worker_start_callback(serv, worker);
            } else {
                crate::event_worker::native::call_worker_stop_callback(serv, worker);
            }
        }
        Step::HeartbeatTimer => {
            let config = state.config_ref();
            let msec = core::ffi::c_long::from(config.heartbeat_check_interval.get()) * 1000;
            // `ReactorProcess_onTimeout` on the reactor arming it, which outlives its
            // timers.
            let reactor = crate::reactor_handle::Reactor::thread();
            let timer = swoole_rs_corebase::timer::add_fn(msec, true, move || {
                if let Some(serv) = reactor.server() {
                    crate::reactor_process::heartbeat(serv, reactor);
                }
            });
            config.heartbeat_timer.store(timer);
            return timer.is_some();
        }
        Step::ReactorWait => return crate::reactor_handle::Reactor::thread().wait(),
    }
    true
}

/// `Server::accept_command_result`: the master's command pipe is readable; a
/// whole reply goes to its waiting callback. The callback may resume a coroutine:
/// this frame owns nothing that needs a drop then.
///
/// Registered only on the master's reactor, whose `ptr` is the live server, for its
/// command pipe.
fn accept_command_result(
    reactor: swoole_rs_corebase::reactor::Running<'_>,
    event: &mut swoole_rs_corebase::reactor::Event,
) -> c_int {
    let Some(serv) = crate::reactor_handle::Reactor::running(reactor).server() else {
        return 0;
    };
    let state = serv.state();
    let bus = state.message_bus();
    let Some(sock) = event.socket() else {
        return 0;
    };
    if bus.read(*sock) <= Some(0) {
        return 0;
    }
    let request_id = bus.head().fd;
    bus.with_packet(|message| crate::commands::command_result(state, request_id, message));
    bus.pop();
    0
}
use crate::Mode;

/// What `factory_` holds once a factory is made: only whether it is set is read. The
/// factory's one member, thread mode's manager, is the state's
/// ([`ServerState::thread_manager`]); every other member dispatches on the mode.
static FACTORY: u8 = 0;

/// A server's factory: [`FACTORY`]'s address, for `factory_`.
pub(crate) fn new() -> *mut c_void {
    core::ptr::from_ref(&FACTORY).cast_mut().cast()
}

/// `new ThreadFactory(serv)`: a factory whose manager holds `threads` threads, one
/// for each worker and the manager.
pub(crate) fn new_thread(state: &ServerState, threads: usize) -> *mut c_void {
    *lock(&state.thread_manager) = Some(Arc::new(ThreadManager::new(threads)));
    new()
}

/// Thread mode's manager; none for another mode's factory, or before one is made.
pub(crate) fn manager(state: &ServerState) -> Option<Arc<ThreadManager>> {
    lock(&state.thread_manager).clone()
}

/// `delete factory_`: thread mode's joins its threads, which still reach the manager
/// as they exit, and then frees it, as `~ThreadFactory` did.
pub(crate) fn delete(state: &ServerState) {
    if let Some(manager) = manager(state) {
        manager.join_all();
    }
    drop(lock(&state.thread_manager).take());
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The server's factory, and its mode.
fn of(state: &ServerState) -> (*mut c_void, Mode) {
    let config = state.config_ref();
    (config.factory_.get(), config.mode_)
}

/// `factory_->start()`.
pub(crate) fn start(serv: crate::view::Server<'_>) -> bool {
    match of(serv.state()) {
        (_, Mode::Process) => crate::process_factory::native::start(serv),
        (_, Mode::Thread) => crate::thread_mode::native::factory_start(serv),
        _ => true,
    }
}

/// `factory_->shutdown()`.
pub(crate) fn shutdown(serv: crate::view::Server<'_>) -> bool {
    match of(serv.state()) {
        (_, Mode::Process) => crate::process_factory::native::shutdown(serv),
        (_, Mode::Thread) => {
            if let Some(manager) = manager(serv.state()) {
                manager.join_all();
            }
            true
        }
        _ => true,
    }
}

/// `factory_->dispatch(task)`.
pub(crate) fn dispatch(serv: crate::view::Server<'_>, task: &mut SendData) -> bool {
    if serv.config().mode_ == Mode::Process {
        crate::process_factory::dispatch(serv, task)
    } else {
        crate::base_factory::dispatch(serv, task, false)
    }
}

/// `factory_->finish(data)`.
pub(crate) fn finish(serv: crate::view::Server<'_>, send: &SendData) -> bool {
    if serv.config().mode_ == Mode::Process {
        crate::process_factory::finish(serv, send)
    } else {
        crate::base_factory::finish(serv, send)
    }
}

/// `factory_->notify(info)`.
pub(crate) fn notify(serv: crate::view::Server<'_>, head: &mut DataHead) -> bool {
    if serv.config().mode_ == Mode::Process {
        // A dispatch of the event with no data.
        let mut task = SendData::empty(*head);
        crate::process_factory::dispatch(serv, &mut task)
    } else {
        crate::base_factory::notify(serv, head)
    }
}

/// `factory_->end(session_id, flags)`.
pub(crate) fn end(serv: crate::view::Server<'_>, session_id: c_long, flags: c_int) -> bool {
    if serv.config().mode_ == Mode::Process {
        crate::process_factory::end(serv, session_id, flags)
    } else {
        crate::base_factory::end(serv, session_id, flags)
    }
}

/// The hybrid's side of [`crate::hybrid`].
pub(crate) struct Linked;

impl crate::hybrid::Hybrid for Linked {
    fn factory_end(&self, serv: crate::view::Server<'_>, session_id: c_long, flags: c_int) -> bool {
        end(serv, session_id, flags)
    }

    fn factory_notify(&self, serv: crate::view::Server<'_>, head: &mut DataHead) -> bool {
        notify(serv, head)
    }

    fn factory_dispatch(&self, serv: crate::view::Server<'_>, send: &mut SendData) -> bool {
        dispatch(serv, send)
    }

    fn base_pass(&self, serv: crate::view::Server<'_>, send: &SendData) {
        crate::master_ops::base_pass(serv, send);
    }

    fn accept_event(&self, serv: crate::view::Server<'_>, head: &mut DataHead) {
        crate::event_worker::native::accept_event(serv, head);
    }

    fn forward(&self, serv: crate::view::Server<'_>, send: &SendData) -> bool {
        crate::master_ops::forward(serv, send)
    }

    fn process_factory(&self, serv: crate::view::Server<'_>) -> *mut c_void {
        crate::process_factory::native::create(serv)
    }

    fn factory_new(&self, _: crate::view::Server<'_>) -> *mut c_void {
        new()
    }

    fn task_pool(
        &self,
        serv: crate::view::Server<'_>,
        ipc: process::process_pool::IpcMode,
        socket: Option<&core::ffi::CStr>,
    ) -> bool {
        use process::methods as pool;
        use process::process_pool::IpcMode;
        use process::worker_record::WorkerType;
        let config = serv.state().config_ref();
        // The server's task pool, reset to `{}` (no member has an initializer).
        serv.task_pool_ref().reset();
        let key = if matches!(ipc, IpcMode::MsgQueue) {
            swoole_rs_corebase::CCast::c_cast::<c_int>(config.message_queue_key.get())
        } else {
            0
        };
        let pool_ref = serv.task_pool_ref();
        let ipc = c_int::from(ipc.code());
        if !pool::create(
            crate::process_pool_native(),
            pool_ref,
            config.task_worker_num.get(),
            key,
            ipc,
        ) {
            return false;
        }
        let (max, grace) = (
            config.task_max_request.get(),
            config.task_max_request_grace.get(),
        );
        pool::set_max_request(crate::process_pool_native(), pool_ref, max, grace);
        pool::set_start_id(
            crate::process_pool_native(),
            pool_ref,
            config.worker_num.get().cast_signed(),
        );
        pool::set_type(crate::process_pool_native(), pool_ref, WorkerType::Task);
        socket.is_none_or(|file| pool::listen_unix(pool_ref, file, 2048))
    }

    fn task_workers(&self, serv: crate::view::Server<'_>) -> bool {
        crate::task_worker::native::init(serv)
    }

    fn step(&self, serv: crate::view::Server<'_>, step: crate::hybrid::Step) -> bool {
        lifecycle_step(serv, step)
    }

    fn dtls_accept(
        &self,
        serv: crate::view::Server<'_>,
        reactor: crate::reactor_handle::Reactor,
        port: crate::view::Port<'_>,
        peer: &swoole_rs_corebase::net_types::SocketAddress,
        data: &[u8],
    ) -> bool {
        let single_thread = serv.state().config_ref().single_thread.get();
        crate::dtls::on_packet(serv, single_thread, reactor, port, peer, data)
    }

    fn dtls_read(
        &self,
        _port: &crate::port_config::PortConfig,
        sock: swoole_rs_corebase::reactor::Socket<'_>,
        _fd: c_int,
    ) -> bool {
        crate::dtls::on_read(sock)
    }

    fn pipe_to_worker(
        &self,
        serv: crate::view::Server<'_>,
        worker_id: u32,
        message: &process::task_file::TaskBuffer,
    ) -> bool {
        /// `SW_PIPE_MASTER | SW_PIPE_NONBLOCK`.
        const PIPE_MASTER_NONBLOCK: c_int = 1 | 2;
        let Some(worker) = serv.worker_record(u16::try_from(worker_id).unwrap_or(u16::MAX)) else {
            return false;
        };
        let sent = process::methods::send_pipe_message(
            crate::process_pool_native(),
            worker,
            message.bytes(),
            PIPE_MASTER_NONBLOCK,
        );
        sent.is_some_and(|sent| sent > 0)
    }

    fn create_thread_factory(&self, serv: crate::view::Server<'_>) -> *mut c_void {
        crate::thread_mode::native::create_factory(serv)
    }

    fn start_worker_threads(&self, serv: crate::view::Server<'_>) -> bool {
        crate::thread_mode::native::start_worker_threads(serv)
    }

    fn stop_worker_threads(&self, serv: crate::view::Server<'_>) {
        crate::thread_mode::native::stop_worker_threads(serv);
    }

    fn stop_single_worker(&self, serv: crate::view::Server<'_>) {
        process::worker_globals::with_current(|worker| {
            crate::event_worker::native::stop_async_worker(serv, *worker);
        });
    }

    fn send_shutdown(&self, serv: crate::view::Server<'_>) -> bool {
        // A worker may run as a user that cannot signal the master.
        let head = DataHead {
            kind: ServerEvent::ShutdownSignal.code(),
            ..DataHead::default()
        };
        // `serv->get_reactor_pipe_socket(0, 0)`.
        let Some(pipe) = serv.worker_pipe(0, false) else {
            return false;
        };
        // The `SendData` upstream sends: the head, then a null payload pointer.
        let mut send = process::process_pool::head_view(&head).to_vec();
        send.resize(size_of::<DataHead>() + size_of::<usize>(), 0);
        swoole_rs_corebase::tls::upstream::send_async(pipe, &send).is_some_and(|sent| sent > 0)
    }

    fn thread0_shutdown(&self, serv: crate::view::Server<'_>) {
        let reactor = crate::reactor_handle::Reactor::thread();
        crate::reactor_entry::thread_shutdown(serv, reactor, 0);
    }

    fn before_shutdown(&self, serv: crate::view::Server<'_>) {
        serv.call_hook(crate::view::ServerHook::BeforeShutdown);
        let callbacks = &serv.state().callbacks;
        callbacks.call_server(crate::callbacks::ServerSlot::BeforeShutdown, serv);
    }

    fn reload(&self, serv: crate::view::Server<'_>, all_workers: bool) -> bool {
        crate::manager::native::reload(serv, all_workers)
    }

    fn command_send(
        &self,
        serv: crate::view::Server<'_>,
        route: process::command::CommandRoute,
        command_id: i32,
        request_id: i64,
        message: &[u8],
    ) -> bool {
        crate::commands::send(serv, route, command_id, request_id, message)
    }

    fn factory_finish(
        &self,
        serv: crate::view::Server<'_>,
        send: &mut crate::base_factory::SendData,
    ) -> bool {
        finish(serv, send)
    }

    fn process_send(
        &self,
        serv: crate::view::Server<'_>,
        to: crate::process_factory::Pipe,
        send: &mut SendData,
    ) -> bool {
        crate::process_factory::native::send(serv, to, send)
    }

    fn to_reactor_thread(&self, serv: crate::view::Server<'_>, head: &DataHead) -> Option<usize> {
        crate::event_worker::native::send_to_reactor_thread(serv, head)
    }
}
