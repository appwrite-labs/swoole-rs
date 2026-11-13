//! The server's calls into what only the hybrid links (the factories, the event and
//! thread-mode workers), behind one seam. Callers build in every configuration, and
//! only [`hybrid`] says which side answers: the hybrid's ([`crate::factory::Linked`]),
//! the unit tests' recorder, or, in a build that links none of it, the defaults below.
use core::ffi::{c_int, c_long};
use swoole_rs_corebase::DataHead;
use swoole_rs_process::task_file::TaskBuffer;

use crate::commands::CommandSend;
use crate::view::Server;

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
    }

    thread_local! {
        static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
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
