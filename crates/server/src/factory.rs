//! `serv->factory_`: upstream's `Factory` and its `BaseFactory`, `ProcessFactory` and
//! `ThreadFactory`, as one record whose members dispatch on the server's mode. Thread
//! mode keeps the base factory's dispatch, finish, notify and end, as upstream's
//! `ThreadFactory : BaseFactory` did.
use core::ffi::{c_int, c_long, c_void};

use core::ptr;

use swoole_rs_corebase::DataHead;
use swoole_rs_process::workers::entry;
use swoole_rs_process::workers::factory_methods::{SendData, run};
use swoole_rs_process::workers::ops::{
    EVENT_CLOSE, RUN_PROCESS_DISPATCH, RUN_PROCESS_END, RUN_PROCESS_FACTORY_SHUTDOWN,
    RUN_PROCESS_FACTORY_START, RUN_PROCESS_FINISH,
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
    manager: *mut c_void,
}

/// A factory over `serv`, with thread mode's `manager` or null.
pub(crate) fn new(serv: *mut c_void, manager: *mut c_void) -> *mut c_void {
    Box::into_raw(Box::new(Factory {
        server: serv,
        manager,
    }))
    .cast()
}

/// `new ThreadFactory(serv)`: a factory whose manager holds `threads` threads, one
/// for each worker and the manager.
pub(crate) fn new_thread(serv: *mut c_void, threads: usize) -> *mut c_void {
    let manager = Box::into_raw(Box::new(ThreadManager::new(threads)));
    new(serv, manager.cast())
}

/// Thread mode's manager, of the thread factory at `factory`.
///
/// # Safety
/// `factory` is a live thread factory [`new_thread`] made.
pub(crate) unsafe fn manager<'a>(factory: *mut c_void) -> &'a ThreadManager {
    // SAFETY: as the caller guarantees.
    let manager = unsafe { (*factory.cast::<Factory>()).manager };
    // SAFETY: its manager, which lives as long as it.
    unsafe { &*manager.cast::<ThreadManager>() }
}

/// `delete factory_`: thread mode's joins its threads and frees its manager first,
/// as `~ThreadFactory` did.
///
/// # Safety
/// `factory` is a factory [`new`] made, not used again.
pub(crate) unsafe fn delete(factory: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let factory = unsafe { Box::from_raw(factory.cast::<Factory>()) };
    if !factory.manager.is_null() {
        // SAFETY: the manager `new_thread` made, freed once.
        let manager = unsafe { Box::from_raw(factory.manager.cast::<ThreadManager>()) };
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
    unsafe { run(serv, op, ptr::null_mut(), send, ptr::null(), arg) != 0 }
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
pub(crate) fn start(state: &ServerState) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => process(factory, RUN_PROCESS_FACTORY_START, ptr::null_mut(), 0),
        // SAFETY: the server's live thread factory.
        (factory, MODE_THREAD) => unsafe { entry::thread_factory_start(factory) },
        _ => true,
    }
}

/// `factory_->shutdown()`.
pub(crate) fn shutdown(state: &ServerState) -> bool {
    match of(state) {
        (factory, MODE_PROCESS) => {
            process(factory, RUN_PROCESS_FACTORY_SHUTDOWN, ptr::null_mut(), 0)
        }
        (_, MODE_THREAD) => {
            if let Some(manager) = thread_manager(state) {
                manager.join_all();
            }
            true
        }
        _ => true,
    }
}

/// Thread mode's manager, once the server's thread factory exists.
fn thread_manager(state: &ServerState) -> Option<&ThreadManager> {
    match of(state) {
        // SAFETY: the server's live thread factory.
        (factory, MODE_THREAD) if !factory.is_null() => Some(unsafe { manager(factory) }),
        _ => None,
    }
}

/// `Server::shutdown()` in thread mode: workers that exit from now on stay stopped.
pub(crate) fn stop_respawning(state: &ServerState) {
    if let Some(manager) = thread_manager(state) {
        manager.stop();
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
