//! The server's calls into what only the hybrid links (the factories, the event and
//! thread-mode workers), behind one seam. Callers build in every configuration, and
//! only [`hybrid`] says which side answers: the hybrid's ([`crate::factory::Linked`]),
//! the unit tests' recorder, or, in a build that links none of it, the defaults below.
use core::ffi::{c_int, c_long};
use swoole_rs_corebase::DataHead;
use swoole_rs_process::task_file::TaskBuffer;

use swoole_rs_reactor::Socket;

use crate::commands::CommandSend;
use crate::port_config::PortConfig;
use crate::view::Server;

/// A step of the server's start, master thread or destroy that runs what only the
/// hybrid links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The logger's output, or stdout, redirected, then `swoole_daemon(0, 1)`.
    Daemon,
    /// `factory_->start()`.
    FactoryStart,
    /// `port->init_protocol()` of each port.
    InitProtocols,
    /// `start_reactor_processes()`.
    StartBase,
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
}

/// What the hybrid links. Each default is a build without it: the reactor threads
/// take a new connection, and the rest is fatal, as no such server can start.
pub(crate) trait Hybrid: Sync {
    /// `factory_->end(session_id, flags)`.
    fn factory_end(&self, _serv: Server<'_>, _session_id: c_long, _flags: c_int) -> bool {
        missing()
    }

    /// `factory_->notify(head)`.
    fn factory_notify(&self, _serv: Server<'_>, _head: &mut DataHead) -> bool {
        missing()
    }

    /// `worker->send_pipe_message(message, EventData::size(), SW_PIPE_MASTER |
    /// SW_PIPE_NONBLOCK)` to worker `worker_id`.
    fn pipe_to_worker(&self, _serv: Server<'_>, _worker_id: u32, _message: &TaskBuffer) -> bool {
        missing()
    }

    /// Thread mode's `Server` method `operation`.
    fn thread_mode(&self, _serv: Server<'_>, _operation: u32) -> i64 {
        missing()
    }

    /// `serv->stop_async_worker(sw_worker())` over base mode's single worker.
    fn stop_single_worker(&self, _serv: Server<'_>) {
        missing()
    }

    /// A worker's `SW_SERVER_EVENT_SHUTDOWN_SIGNAL` to the master, through the first
    /// reactor pipe: whether it went.
    fn send_shutdown(&self, _serv: Server<'_>) -> bool {
        missing()
    }

    /// `ReactorThread::shutdown` of the server's first reactor thread, over this
    /// thread's reactor.
    fn thread0_shutdown(&self, _serv: Server<'_>) {
        missing()
    }

    /// `SW_GLOBAL_HOOK_BEFORE_SERVER_SHUTDOWN`, then `onBeforeShutdown`, each when set.
    fn before_shutdown(&self, _serv: Server<'_>) {
        missing()
    }

    /// `Server::reload(reload_all_workers)`.
    fn reload(&self, _serv: Server<'_>, _all_workers: bool) -> bool {
        missing()
    }

    /// `Server::command`'s request to another process: whether it went.
    fn command_send(&self, _serv: Server<'_>, _request: &CommandSend<'_>) -> bool {
        missing()
    }

    /// `factory_->finish(send)`.
    fn factory_finish(&self, _serv: Server<'_>, _send: &mut crate::base_factory::SendData) -> bool {
        missing()
    }

    /// `factory_->dispatch(send)`.
    fn factory_dispatch(
        &self,
        _serv: Server<'_>,
        _send: &mut crate::base_factory::SendData,
    ) -> bool {
        missing()
    }

    /// A DTLS port's read: `sock`'s next datagram to the session on `fd`; whether its
    /// pending handshake failed.
    fn dtls_read(&self, _port: &PortConfig, _sock: Socket<'_>, _fd: c_int) -> bool {
        missing()
    }

    /// Base mode's message bus takes `send`, then this worker accepts the event.
    fn base_pass(&self, _serv: Server<'_>, _send: &crate::base_factory::SendData) {
        missing()
    }

    /// `worker_accept_event(head)` in this process.
    fn accept_event(&self, _serv: Server<'_>, _head: &mut DataHead) {
        missing()
    }

    /// The message bus writes `send` to the pipe of the worker that owns its session:
    /// whether it went.
    fn forward(&self, _serv: Server<'_>, _send: &crate::base_factory::SendData) -> bool {
        missing()
    }

    /// `Server::accept_dtls_connection` of the datagram `pkt`, `length` bytes, that
    /// arrived on the DTLS port `port` of `reactor`, then its handshake.
    fn dtls_accept(
        &self,
        _serv: Server<'_>,
        _reactor: crate::reactor_handle::Reactor,
        _port: *mut core::ffi::c_void,
        _pkt: &mut crate::reactor_entry::DgramPacket,
        _length: u32,
    ) -> c_int {
        missing()
    }

    /// `new ProcessFactory(serv)`, or null.
    fn process_factory(&self, _serv: Server<'_>) -> *mut core::ffi::c_void {
        missing()
    }

    /// `new BaseFactory(serv)` over the tables `create_base_factory` allocated.
    fn factory_new(&self, _serv: Server<'_>) -> *mut core::ffi::c_void {
        missing()
    }

    /// The task pool of `task_worker_num` workers over `ipc`, its requests and worker
    /// ids set, listening on `socket` when given: whether it was made.
    fn task_pool(
        &self,
        _serv: Server<'_>,
        _ipc: swoole_rs_process::process_pool::IpcMode,
        _socket: Option<&core::ffi::CStr>,
    ) -> bool {
        missing()
    }

    /// Each event worker's task notify pipe, then `init_task_workers`: whether all
    /// were made.
    fn task_workers(&self, _serv: Server<'_>) -> bool {
        missing()
    }

    /// Runs `step`: negative when it failed, else 0 or what it returns.
    fn step(&self, _serv: Server<'_>, _step: Step) -> c_int {
        missing()
    }

    /// `serv->send_to_reactor_thread(head)`: 0, the reactor threads keeping it, without
    /// the event workers.
    fn to_reactor_thread(&self, _serv: Server<'_>, _head: &DataHead) -> isize {
        0
    }
}

fn missing() -> ! {
    swoole_rs_corebase::fatal("no native server ops without upstream-symbols")
}

/// A build that links none of the hybrid.
#[cfg(not(any(test, feature = "upstream-symbols")))]
struct Unlinked;

#[cfg(not(any(test, feature = "upstream-symbols")))]
impl Hybrid for Unlinked {}

/// The side this build links.
pub(crate) fn hybrid() -> &'static dyn Hybrid {
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    let hybrid = &crate::factory::Linked;
    #[cfg(test)]
    let hybrid = &tests::Recorder;
    #[cfg(not(any(test, feature = "upstream-symbols")))]
    let hybrid = &Unlinked;
    hybrid
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
        ThreadMode(u32),
        StopSingleWorker,
        SendShutdown,
        Thread0Shutdown,
        BeforeShutdown,
        Reload(bool),
        /// The action, process, command, request and message length.
        CommandSend(i32, u32, i32, i64, usize),
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
        Step(Step),
    }

    thread_local! {
        static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
        static FAILING: core::cell::Cell<Option<Step>> = const { core::cell::Cell::new(None) };
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
            ))
        }

        fn pipe_to_worker(&self, _: Server<'_>, worker_id: u32, message: &TaskBuffer) -> bool {
            let sent = message.bytes().len();
            hear(Call::Pipe(worker_id, message.head().kind, sent))
        }

        fn thread_mode(&self, _: Server<'_>, operation: u32) -> i64 {
            hear(Call::ThreadMode(operation));
            0
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

        fn to_reactor_thread(&self, _: Server<'_>, head: &DataHead) -> isize {
            hear(Call::ToReactorThread(head.fd, head.reactor_id, head.kind));
            size_of::<DataHead>().cast_signed()
        }

        fn factory_dispatch(
            &self,
            _: Server<'_>,
            send: &mut crate::base_factory::SendData,
        ) -> bool {
            hear(Call::Dispatch(send.info.fd, send.info.kind, send.info.len))
        }

        fn base_pass(&self, _: Server<'_>, send: &crate::base_factory::SendData) {
            hear(Call::BasePass(send.info.fd, send.info.kind, send.info.len));
        }

        fn accept_event(&self, _: Server<'_>, head: &mut DataHead) {
            hear(Call::AcceptEvent(head.fd, head.kind, head.server_fd));
        }

        fn forward(&self, _: Server<'_>, send: &crate::base_factory::SendData) -> bool {
            hear(Call::Forward(
                send.info.fd,
                send.info.kind,
                send.info.reactor_id,
            ))
        }

        fn dtls_accept(
            &self,
            _: Server<'_>,
            _: crate::reactor_handle::Reactor,
            _: *mut core::ffi::c_void,
            _: &mut crate::reactor_entry::DgramPacket,
            length: u32,
        ) -> c_int {
            hear(Call::DtlsAccept(length));
            0
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
            ipc: swoole_rs_process::process_pool::IpcMode,
            socket: Option<&core::ffi::CStr>,
        ) -> bool {
            hear(Call::TaskPool(ipc.code(), socket.is_some()))
        }

        fn task_workers(&self, _: Server<'_>) -> bool {
            hear(Call::TaskWorkers)
        }

        fn step(&self, _: Server<'_>, step: Step) -> c_int {
            hear(Call::Step(step));
            if FAILING.get() == Some(step) { -1 } else { 0 }
        }

        /// A failed handshake.
        fn dtls_read(&self, _: &PortConfig, _: Socket<'_>, fd: c_int) -> bool {
            hear(Call::DtlsRead(fd))
        }

        fn factory_finish(&self, _: Server<'_>, send: &mut crate::base_factory::SendData) -> bool {
            let info = &send.info;
            // SAFETY: a sendfile task: its 16-byte head, then its name and NUL.
            let task =
                unsafe { core::slice::from_raw_parts(send.data.cast::<u8>(), info.len as usize) };
            let word = |at: usize| u64::from_ne_bytes(task[at..at + 8].try_into().unwrap());
            let mut name = [0; 8];
            let given = &task[16..task.len().min(24)];
            name[..given.len()].copy_from_slice(given);
            let (offset, length) = (word(0).cast_signed(), usize::try_from(word(8)).unwrap());
            hear(Call::Finish(
                info.fd, info.kind, info.len, offset, length, name,
            ))
        }

        fn command_send(&self, _: Server<'_>, c: &CommandSend<'_>) -> bool {
            let (action, process, command) = (c.action, c.process_id, c.command_id);
            hear(Call::CommandSend(
                action,
                process,
                command,
                c.request_id,
                c.message.len(),
            ))
        }
    }
}
