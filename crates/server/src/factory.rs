//! `serv->factory_`: upstream's `Factory` and its `BaseFactory`, `ProcessFactory` and
//! `ThreadFactory`, as one record whose members dispatch on the server's mode. Thread
//! mode keeps the base factory's dispatch, finish, notify and end, as upstream's
//! `ThreadFactory : BaseFactory` did.
use core::ffi::{c_int, c_long, c_void};

use core::ptr;

use crate::base_factory::SendData;
use crate::factory_ops::run;
use swoole_rs_corebase::DataHead;
use swoole_rs_process::workers::ops::{
    EVENT_CLOSE, RUN_PROCESS_DISPATCH, RUN_PROCESS_END, RUN_PROCESS_FACTORY_SHUTDOWN,
    RUN_PROCESS_FACTORY_START, RUN_PROCESS_FINISH, RUN_THREAD_FACTORY_START,
};
use swoole_rs_process::workers::thread::ThreadManager;

use crate::ops::{RUN_BASE_DISPATCH, RUN_BASE_END, RUN_BASE_FINISH, RUN_BASE_NOTIFY};
use crate::server_methods::{end_arg, on_base};
use crate::state::ServerState;
use crate::{MODE_PROCESS, MODE_THREAD};

/// `swoole::Factory` (upstream's `swoole_server.h`): its server, then thread mode's manager state
/// (crates/process/src/workers/thread.rs), null in the other modes.
#[repr(C)]
pub(crate) struct Factory {
    server: *mut c_void,
    manager: Option<Box<ThreadManager>>,
}

/// A factory over `serv`, with thread mode's `manager` or none.
pub(crate) fn new(serv: *mut c_void, manager: Option<Box<ThreadManager>>) -> *mut c_void {
    Box::into_raw(Box::new(Factory {
        server: serv,
        manager,
    }))
    .cast()
}

/// `new ThreadFactory(serv)`: a factory whose manager holds `threads` threads, one
/// for each worker and the manager.
pub(crate) fn new_thread(serv: *mut c_void, threads: usize) -> *mut c_void {
    new(serv, Some(Box::new(ThreadManager::new(threads))))
}

/// Thread mode's manager of the factory at `factory`; none for another mode's.
///
/// # Safety
/// `factory` is a live factory [`new`] made.
pub(crate) unsafe fn manager<'a>(factory: *mut c_void) -> Option<&'a ThreadManager> {
    // SAFETY: as the caller guarantees.
    let factory = unsafe { &*factory.cast::<Factory>() };
    factory.manager.as_deref()
}

/// `delete factory_`: thread mode's joins its threads and frees its manager first,
/// as `~ThreadFactory` did.
///
/// # Safety
/// `factory` is a factory [`new`] made, not used again.
pub(crate) unsafe fn delete(factory: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let factory = unsafe { Box::from_raw(factory.cast::<Factory>()) };
    if let Some(manager) = factory.manager {
        manager.join_all();
    }
}

/// The server's factory, and its mode.
fn of(state: &ServerState) -> (*mut c_void, u64) {
    let config = state.config_ref();
    (config.factory_, u64::from(config.mode_))
}

/// `ProcessFactory`'s member `op` over the server of `factory`, sending `send`.
fn process(factory: *mut c_void, op: u32, send: *mut SendData, arg: u64) -> bool {
    // SAFETY: the server's live process factory, whose server and send are live.
    let serv = unsafe { (*factory.cast::<Factory>()).server };
    // SAFETY: as above.
    let serv = unsafe { crate::view::Server::from_raw(serv) };
    // SAFETY: as above: null or the live send, which nothing else borrows during the op.
    let send = unsafe { send.as_mut() };
    run(serv, op, None, send, None, arg) != 0
}

/// `BaseFactory`'s member `op` over `factory` with `arg`, lending `send`.
fn base(factory: *mut c_void, op: u32, send: *mut c_void, arg: u64) -> bool {
    // SAFETY: the server's live factory, and the record the op takes.
    unsafe { on_base(factory, send, op, arg) }
}

/// A pointer member argument, as the op's `uint64_t`.
fn addr(p: *mut c_void) -> u64 {
    p.expose_provenance() as u64
}

/// `factory_->start()`.
pub(crate) fn start(serv: crate::view::Server<'_>) -> bool {
    match of(serv.state()) {
        (factory, MODE_PROCESS) => process(factory, RUN_PROCESS_FACTORY_START, ptr::null_mut(), 0),
        (_, MODE_THREAD) => crate::thread_mode_ops::run_server(serv, RUN_THREAD_FACTORY_START) != 0,
        _ => true,
    }
}

/// `factory_->shutdown()`.
pub(crate) fn shutdown(state: &ServerState) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => {
            process(factory, RUN_PROCESS_FACTORY_SHUTDOWN, ptr::null_mut(), 0)
        }
        (factory, MODE_THREAD) => {
            // SAFETY: the server's live factory.
            if let Some(manager) = unsafe { manager(factory) } {
                manager.join_all();
            }
            true
        }
        _ => true,
    }
}

/// `factory_->dispatch(task)`.
pub(crate) fn dispatch(state: &ServerState, task: *mut c_void) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => process(factory, RUN_PROCESS_DISPATCH, task.cast(), 0),
        (factory, _) => base(factory, RUN_BASE_DISPATCH, task, addr(task)),
    }
}

/// `factory_->finish(data)`.
pub(crate) fn finish(state: &ServerState, data: *mut c_void) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => process(factory, RUN_PROCESS_FINISH, data.cast(), 0),
        (factory, _) => base(factory, RUN_BASE_FINISH, data, addr(data)),
    }
}

/// `factory_->notify(info)`.
pub(crate) fn notify(state: &ServerState, info: *mut c_void) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => {
            // A dispatch of the event with no data.
            let mut task = SendData {
                // SAFETY: the live event the caller hears.
                info: unsafe { *info.cast::<DataHead>() },
                data: ptr::null(),
            };
            process(factory, RUN_PROCESS_DISPATCH, &raw mut task, 0)
        }
        (factory, _) => base(factory, RUN_BASE_NOTIFY, ptr::null_mut(), addr(info)),
    }
}

/// `factory_->end(session_id, flags)`.
pub(crate) fn end(state: &ServerState, session_id: c_long, flags: c_int) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => {
            // A close of the session.
            let mut close = SendData {
                info: DataHead {
                    fd: session_id,
                    kind: u8::try_from(EVENT_CLOSE).unwrap_or_default(),
                    ..DataHead::default()
                },
                data: ptr::null(),
            };
            let flags = u64::from(flags.cast_unsigned());
            process(factory, RUN_PROCESS_END, &raw mut close, flags)
        }
        (factory, _) => base(
            factory,
            RUN_BASE_END,
            ptr::null_mut(),
            end_arg(session_id, flags),
        ),
    }
}

/// The hybrid's side of [`crate::hybrid`].
pub(crate) struct Linked;

impl crate::hybrid::Hybrid for Linked {
    fn factory_end(&self, serv: crate::view::Server<'_>, session_id: c_long, flags: c_int) -> bool {
        end(serv.state(), session_id, flags)
    }

    fn factory_notify(&self, serv: crate::view::Server<'_>, head: &mut DataHead) -> bool {
        notify(serv.state(), ptr::from_mut(head).cast())
    }

    fn pipe_to_worker(
        &self,
        serv: crate::view::Server<'_>,
        worker_id: u32,
        message: &swoole_rs_process::task_file::TaskBuffer,
    ) -> bool {
        /// `SW_PIPE_MASTER | SW_PIPE_NONBLOCK`.
        const PIPE_MASTER_NONBLOCK: c_int = 1 | 2;
        let worker = serv.worker(u16::try_from(worker_id).unwrap_or(u16::MAX));
        let bytes = message.bytes();
        // SAFETY: one of the server's workers, and the message's bytes, which live
        // for the call.
        let sent = unsafe {
            swoole_rs_process::methods::send_pipe_message(
                worker,
                bytes.as_ptr(),
                bytes.len(),
                PIPE_MASTER_NONBLOCK,
            )
        };
        sent > 0
    }

    fn thread_mode(&self, serv: crate::view::Server<'_>, operation: u32) -> i64 {
        crate::thread_mode_ops::run_server(serv, operation)
    }

    fn stop_single_worker(&self, serv: crate::view::Server<'_>) {
        swoole_rs_process::worker_globals::with_current(|worker| {
            crate::event_worker_ops::stop_async_worker(serv, *worker);
        });
    }

    fn send_shutdown(&self, serv: crate::view::Server<'_>) -> bool {
        /// `SW_SERVER_EVENT_SHUTDOWN_SIGNAL`.
        const EVENT_SHUTDOWN_SIGNAL: u8 = 19;
        // A worker may run as a user that cannot signal the master.
        let send = SendData {
            info: DataHead {
                kind: EVENT_SHUTDOWN_SIGNAL,
                ..DataHead::default()
            },
            data: ptr::null(),
        };
        // `serv->get_reactor_pipe_socket(0, 0)`.
        let Some(pipe) = serv.worker_pipe(0, false) else {
            return false;
        };
        let data = (&raw const send).cast();
        // SAFETY: the worker's live pipe socket and the send's bytes.
        let sent = unsafe {
            swoole_rs_tls::upstream::socket_send_async(pipe, data, size_of::<SendData>())
        };
        sent > 0
    }

    fn thread0_shutdown(&self, serv: crate::view::Server<'_>) {
        let reactor = crate::reactor_handle::Reactor::thread();
        crate::reactor_entry::thread_shutdown(serv, reactor, 0);
    }

    fn before_shutdown(&self, serv: crate::view::Server<'_>) {
        serv.call_hook(crate::view::ServerHook::BeforeShutdown);
        let callbacks = &serv.state().callbacks;
        callbacks.call_server(crate::callbacks::CB_BEFORE_SHUTDOWN, serv);
    }

    fn reload(&self, serv: crate::view::Server<'_>, all_workers: bool) -> bool {
        crate::manager_ops::reload(serv, all_workers)
    }

    fn command_send(
        &self,
        serv: crate::view::Server<'_>,
        c: &crate::commands::CommandSend<'_>,
    ) -> bool {
        crate::commands::send(serv, c)
    }

    fn factory_finish(
        &self,
        serv: crate::view::Server<'_>,
        send: &mut crate::base_factory::SendData,
    ) -> bool {
        finish(serv.state(), (&raw mut *send).cast())
    }

    fn to_reactor_thread(&self, serv: crate::view::Server<'_>, head: &DataHead) -> isize {
        crate::event_worker_ops::send_to_reactor_thread(serv, head)
    }
}
