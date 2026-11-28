//! Server operations implemented by the runtime and recorded by unit tests.
use core::ffi::{c_int, c_long};
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::process::command::CommandRoute;
use swoole_rs_corebase::process::task_file::TaskBuffer;

use swoole_rs_corebase::reactor::Socket;

use crate::port_config::PortConfig;
use crate::view::Server;

/// A step in server startup, the master thread, or teardown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The logger's output, or stdout, redirected, then `swoole_daemon(0, 1)`.
    Daemon,
    /// `factory_->start()`.
    FactoryStart,
    /// `port->init_protocol()` of each port.
    InitProtocols,
    /// `start_reactor_threads()`.
    StartProcess,
    /// `swoole_timer_reinit()` when the timer is detached.
    TimerReinit,
    /// `swoole_signal_set(signo, nullptr)`.
    Ignore(c_int),
    /// `swoole_signal_set(signo, master_signal_handler)`.
    Handle(c_int),
    /// The master reactor's `SW_FD_PIPE` reads are command results.
    CommandHandler,
    /// The master's timer: fails when it can't be added.
    MasterTimer,
    /// `swoole_event_free()`.
    EventFree,
    /// `swoole_event_wait()`.
    EventWait,
    /// `reactor_thread_barrier.wait()`.
    ReactorBarrier,
    /// `gs->manager_barrier.wait()`.
    ManagerBarrier,
    /// `join_reactor_thread()`.
    JoinReactorThreads,
    /// `factory_->shutdown()`.
    FactoryShutdown,
    /// The message bus frees its buffer.
    ReleasePipeBuffers,
    /// `port->close()` of each port.
    ClosePorts,
    /// `swoole_signal_clear()`.
    ClearSignals,
    /// `destroy_process_factory()`.
    DestroyProcessFactory,
    /// `gs->task_workers.destroy()`.
    DestroyTaskPool,
    /// `delete factory_`.
    DeleteFactory,
    /// `serv->ports[index]->listen()`.
    Listen(usize),
    /// `serv->ports[index]->close_socket()`, before each worker listens itself.
    ClosePortSocket(usize),
    /// `serv->ports[index]->create_socket()`, in the worker that listens on it.
    CreatePortSocket(usize),
    /// The base-mode event worker pool over the server's workers.
    CreateEventPool,
    /// `gs->event_workers.destroy()`.
    DestroyEventPool,
    /// `create_pipe_buffers()`.
    CreatePipeBuffers,
    /// `start_manager_process()`.
    StartManager,
    /// `swoole_event_init(0)`.
    EventInit,
    /// `worker_signal_init()`.
    WorkerSignalInit,
    /// The thread's reactor reads its worker pipes from the message bus.
    PipeHandler,
    /// `worker_start_callback(worker)` for event worker `id`.
    WorkerStart(u32),
    /// `worker_stop_callback(worker)` for event worker `id`.
    WorkerStop(u32),
    /// The heartbeat check's timer on this thread's reactor.
    HeartbeatTimer,
    /// `reactor->wait()` on this thread's reactor.
    ReactorWait,
}

/// The factory and worker operations a server delegates to its runtime.
pub(crate) trait Hybrid: Sync {
    /// `factory_->end(session_id, flags)`.
    fn factory_end(&self, serv: Server<'_>, session_id: c_long, flags: c_int) -> bool;

    /// `factory_->notify(head)`.
    fn factory_notify(&self, serv: Server<'_>, head: &mut DataHead) -> bool;

    /// `worker->send_pipe_message(message, EventData::size(), SW_PIPE_MASTER |
    /// SW_PIPE_NONBLOCK)` to worker `worker_id`.
    fn pipe_to_worker(&self, serv: Server<'_>, worker_id: u32, message: &TaskBuffer) -> bool;

    /// `Server::create_thread_factory`: the factory, null when it failed.
    fn create_thread_factory(&self, serv: Server<'_>) -> *mut core::ffi::c_void;

    /// `Server::start_worker_threads`: whether the master thread's loop ran.
    fn start_worker_threads(&self, serv: Server<'_>) -> bool;

    /// `serv->stop_async_worker(sw_worker())` over base mode's single worker.
    fn stop_single_worker(&self, serv: Server<'_>);

    /// A worker's `SW_SERVER_EVENT_SHUTDOWN_SIGNAL` to the master, through the first
    /// reactor pipe: whether it went.
    fn send_shutdown(&self, serv: Server<'_>) -> bool;

    /// `ReactorThread::shutdown` of the server's first reactor thread, over this
    /// thread's reactor.
    fn thread0_shutdown(&self, serv: Server<'_>);

    /// `SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN`, then `onBeforeShutdown`, each when set.
    fn before_shutdown(&self, serv: Server<'_>);

    /// `Server::reload(reload_all_workers)`.
    fn reload(&self, serv: Server<'_>, all_workers: bool) -> bool;

    /// `Server::command`'s request to another process: whether it went.
    fn command_send(
        &self,
        serv: Server<'_>,
        route: CommandRoute,
        command_id: i32,
        request_id: i64,
        message: &[u8],
    ) -> bool;

    /// `factory_->finish(send)`.
    fn factory_finish(&self, serv: Server<'_>, send: &mut crate::base_factory::SendData) -> bool;

    /// `factory_->dispatch(send)`.
    fn factory_dispatch(&self, serv: Server<'_>, send: &mut crate::base_factory::SendData) -> bool;

    /// A DTLS port's read: `sock`'s next datagram to the session on `fd`; whether its
    /// pending handshake failed.
    fn dtls_read(&self, port: &PortConfig, sock: Socket<'_>, fd: c_int) -> bool;

    /// Base mode's message bus takes `send`, then this worker accepts the event.
    fn base_pass(&self, serv: Server<'_>, send: &crate::base_factory::SendData);

    /// `worker_accept_event(head)` in this process.
    fn accept_event(&self, serv: Server<'_>, head: &mut DataHead);

    /// The message bus writes `send` to the pipe of the worker that owns its session:
    /// whether it went.
    fn forward(&self, serv: Server<'_>, send: &crate::base_factory::SendData) -> bool;

    /// `Server::accept_dtls_connection` of the datagram `data` from `peer` that
    /// arrived on the DTLS port `port` of `reactor`, then its handshake.
    fn dtls_accept(
        &self,
        serv: Server<'_>,
        reactor: crate::reactor_handle::Reactor,
        port: crate::view::Port<'_>,
        peer: &swoole_rs_corebase::net_types::SocketAddress,
        data: &[u8],
    ) -> bool;

    /// `new ProcessFactory(serv)`, or null.
    fn process_factory(&self, serv: Server<'_>) -> *mut core::ffi::c_void;

    /// `new BaseFactory(serv)` over the tables `create_base_factory` allocated.
    fn factory_new(&self, serv: Server<'_>) -> *mut core::ffi::c_void;

    /// The task pool of `task_worker_num` workers over `ipc`, its requests and worker
    /// ids set, listening on `socket` when given: whether it was made.
    fn task_pool(
        &self,
        serv: Server<'_>,
        ipc: swoole_rs_corebase::process::process_pool::IpcMode,
        socket: Option<&core::ffi::CStr>,
    ) -> bool;

    /// Each event worker's task notify pipe, then `init_task_workers`: whether all
    /// were made.
    fn task_workers(&self, serv: Server<'_>) -> bool;

    /// Process mode's write of `send` to `to`: whether it went.
    fn process_send(
        &self,
        serv: Server<'_>,
        to: crate::process_factory::Pipe,
        send: &mut crate::base_factory::SendData,
    ) -> bool;

    /// Runs `step`: whether it succeeded.
    fn step(&self, serv: Server<'_>, step: Step) -> bool;

    /// `serv->send_to_reactor_thread(head)`: send to the reactor thread that owns the session.
    fn to_reactor_thread(&self, serv: Server<'_>, head: &DataHead) -> Option<usize>;
}
/// The side this build links.
pub(crate) fn hybrid() -> &'static dyn Hybrid {
    let linked: &'static dyn Hybrid = &crate::factory::Linked;
    #[cfg(test)]
    let linked = tests::recorder(linked);
    linked
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A call the unit tests' side heard.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Call {
        End(c_long, c_int),
        /// The head's fd, reactor, kind and listening fd.
        Notify(c_long, i16, u8, u16),
        /// The worker, the message's kind and the bytes sent: the head and its payload.
        Pipe(u32, u8, usize),
        StartWorkerThreads,
        StopSingleWorker,
        SendShutdown,
        Thread0Shutdown,
        BeforeShutdown,
        Reload(bool),
        /// The target, process, command, request and message length.
        CommandSend(
            swoole_rs_corebase::process::command::Target,
            u32,
            i32,
            i64,
            usize,
        ),
        /// The head's session, reactor and kind.
        ToReactorThread(c_long, i16, u8),
        /// The send's session, kind and length, and its task's offset, length and the
        /// first 8 bytes of its name.
        Finish(c_long, u8, u32, i64, usize, [u8; 8]),
        /// The send's fd, kind and length.
        Dispatch(c_long, u8, u32),
        /// The session's fd.
        DtlsRead(c_int),
        /// The send's fd, kind and length.
        BasePass(c_long, u8, u32),
        /// The head's fd, kind and listening fd.
        AcceptEvent(c_long, u8, u16),
        /// The send's session, kind and reactor.
        Forward(c_long, u8, i16),
        /// The datagram's length.
        DtlsAccept(u32),
        ProcessFactory,
        FactoryNew,
        /// The IPC mode's code and whether the pool listens.
        TaskPool(u8, bool),
        TaskWorkers,
        /// Where process mode wrote, and the send's fd and kind.
        Process(crate::process_factory::Pipe, c_long, u8),
        Step(Step),
    }

    thread_local! {
        pub(crate) static BASE_PAYLOAD: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        pub(crate) static FAIL_NOTIFY: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
        static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
        static FAILING: core::cell::Cell<Option<Step>> = const { core::cell::Cell::new(None) };
        static ON_STEP: RefCell<Option<StepCheck>> = const { RefCell::new(None) };
    }

    /// A check a test runs on the server as each step is made.
    pub(crate) type StepCheck = Box<dyn Fn(Server<'_>, Step)>;

    /// What this thread's tests run as each step is made, until the next call.
    pub(crate) fn on_step(check: Option<StepCheck>) {
        ON_STEP.set(check);
    }

    /// The step this thread's tests make fail, until the next call.
    pub(crate) fn fail(step: Option<Step>) {
        FAILING.set(step);
    }

    /// The calls this thread's tests made, oldest first, which it forgets.
    pub(crate) fn calls() -> Vec<Call> {
        CALLS.with_borrow_mut(core::mem::take)
    }

    fn hear(call: Call) -> bool {
        CALLS.with_borrow_mut(|calls| calls.push(call));
        true
    }

    /// The unit tests' side in place of `linked`: it records each call and succeeds,
    /// since the tests run no live server for the linked side to act on.
    pub(crate) fn recorder(_linked: &'static dyn Hybrid) -> &'static dyn Hybrid {
        &Recorder
    }

    /// Records each call and succeeds.
    pub(crate) struct Recorder;

    impl Hybrid for Recorder {
        fn factory_end(&self, _: Server<'_>, session_id: c_long, flags: c_int) -> bool {
            hear(Call::End(session_id, flags))
        }

        fn factory_notify(&self, _: Server<'_>, head: &mut DataHead) -> bool {
            hear(Call::Notify(
                head.fd,
                head.reactor_id,
                head.kind,
                head.server_fd,
            )) && !FAIL_NOTIFY.replace(false)
        }

        fn pipe_to_worker(&self, _: Server<'_>, worker_id: u32, message: &TaskBuffer) -> bool {
            let sent = message.packed().bytes().len();
            hear(Call::Pipe(worker_id, message.head().kind, sent))
        }

        fn create_thread_factory(&self, _: Server<'_>) -> *mut core::ffi::c_void {
            core::ptr::null_mut()
        }

        fn start_worker_threads(&self, _: Server<'_>) -> bool {
            hear(Call::StartWorkerThreads)
        }

        fn stop_single_worker(&self, _: Server<'_>) {
            hear(Call::StopSingleWorker);
        }

        fn send_shutdown(&self, _: Server<'_>) -> bool {
            hear(Call::SendShutdown)
        }

        fn thread0_shutdown(&self, _: Server<'_>) {
            hear(Call::Thread0Shutdown);
        }

        fn before_shutdown(&self, _: Server<'_>) {
            hear(Call::BeforeShutdown);
        }

        fn reload(&self, _: Server<'_>, all_workers: bool) -> bool {
            hear(Call::Reload(all_workers))
        }

        fn to_reactor_thread(&self, _: Server<'_>, head: &DataHead) -> Option<usize> {
            hear(Call::ToReactorThread(head.fd, head.reactor_id, head.kind));
            Some(size_of::<DataHead>())
        }

        fn factory_dispatch(
            &self,
            _: Server<'_>,
            send: &mut crate::base_factory::SendData,
        ) -> bool {
            let info = send.info();
            hear(Call::Dispatch(info.fd, info.kind, info.len))
        }

        fn base_pass(&self, _: Server<'_>, send: &crate::base_factory::SendData) {
            let info = send.info();
            BASE_PAYLOAD.replace(send.payload().to_vec());
            hear(Call::BasePass(info.fd, info.kind, info.len));
        }

        fn accept_event(&self, _: Server<'_>, head: &mut DataHead) {
            hear(Call::AcceptEvent(head.fd, head.kind, head.server_fd));
        }

        fn forward(&self, _: Server<'_>, send: &crate::base_factory::SendData) -> bool {
            let info = send.info();
            hear(Call::Forward(info.fd, info.kind, info.reactor_id))
        }

        fn dtls_accept(
            &self,
            _: Server<'_>,
            _: crate::reactor_handle::Reactor,
            _: crate::view::Port<'_>,
            _: &swoole_rs_corebase::net_types::SocketAddress,
            data: &[u8],
        ) -> bool {
            hear(Call::DtlsAccept(u32::try_from(data.len()).unwrap()));
            true
        }

        fn process_factory(&self, _: Server<'_>) -> *mut core::ffi::c_void {
            hear(Call::ProcessFactory);
            core::ptr::without_provenance_mut(0x70)
        }

        fn factory_new(&self, _: Server<'_>) -> *mut core::ffi::c_void {
            hear(Call::FactoryNew);
            core::ptr::without_provenance_mut(0x70)
        }

        fn task_pool(
            &self,
            _: Server<'_>,
            ipc: swoole_rs_corebase::process::process_pool::IpcMode,
            socket: Option<&core::ffi::CStr>,
        ) -> bool {
            hear(Call::TaskPool(ipc.code(), socket.is_some()))
        }

        fn task_workers(&self, _: Server<'_>) -> bool {
            hear(Call::TaskWorkers)
        }

        fn process_send(
            &self,
            _: Server<'_>,
            to: crate::process_factory::Pipe,
            send: &mut crate::base_factory::SendData,
        ) -> bool {
            hear(Call::Process(to, send.info().fd, send.info().kind))
        }

        fn step(&self, serv: Server<'_>, step: Step) -> bool {
            ON_STEP.with_borrow(|check| check.as_ref().map(|check| check(serv, step)));
            hear(Call::Step(step));
            FAILING.get() != Some(step)
        }

        /// A failed handshake.
        fn dtls_read(&self, _: &PortConfig, _: Socket<'_>, fd: c_int) -> bool {
            hear(Call::DtlsRead(fd))
        }

        fn factory_finish(&self, _: Server<'_>, send: &mut crate::base_factory::SendData) -> bool {
            let (info, task) = (send.info(), send.payload());
            let word = |at: usize| u64::from_ne_bytes(task[at..at + 8].try_into().unwrap());
            let mut name = [0; 8];
            let given = &task[16..task.len().min(24)];
            name[..given.len()].copy_from_slice(given);
            let (offset, length) = (word(0).cast_signed(), usize::try_from(word(8)).unwrap());
            hear(Call::Finish(
                info.fd, info.kind, info.len, offset, length, name,
            ))
        }

        fn command_send(
            &self,
            _: Server<'_>,
            route: CommandRoute,
            command_id: i32,
            request_id: i64,
            message: &[u8],
        ) -> bool {
            hear(Call::CommandSend(
                route.target,
                route.process_id,
                command_id,
                request_id,
                message.len(),
            ))
        }
    }
}
