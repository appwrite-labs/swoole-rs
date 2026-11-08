//! The `Factory`, `ProcessFactory` and `Server` methods of `native/process/workers.cc`
//! that were one operation of [`super::factory`], defined under their upstream
//! (Itanium) symbols when the hybrid links this crate (`upstream-symbols`). Native
//! code registers its factory call once; each method fills the context the way its
//! C++ forward did and runs over it.
use core::ffi::{c_int, c_long, c_void};
use core::ptr;
use std::sync::OnceLock;

use super::ops::{
    EVENT_CLOSE, RUN_CHECK_EXIT_STATUS, RUN_CREATE_PROCESS_FACTORY, RUN_CREATE_WORKER_PIPES,
    RUN_DESTROY_PROCESS_FACTORY, RUN_KILL_TASK_WORKERS, RUN_PROCESS_DISPATCH, RUN_PROCESS_END,
    RUN_PROCESS_FACTORY_SHUTDOWN, RUN_PROCESS_FACTORY_START, RUN_PROCESS_FINISH,
    RUN_SPAWN_EVENT_WORKER, RUN_SPAWN_USER_WORKER,
};
use super::{WorkersCall, swr_v1_workers_run};
use swoole_rs_corebase::DataHead;

/// workers.cc's `FactoryContext` (`WorkersNative<4>`), which asserts this layout.
#[repr(C)]
pub struct FactoryContext {
    serv: *mut c_void,
    worker: *mut c_void,
    send: *mut SendData,
    status: *const c_void,
    conn: *mut c_void,
    fd: c_int,
}

const _: () = assert!(
    size_of::<FactoryContext>() == 48 && core::mem::offset_of!(FactoryContext, fd) == 40,
    "workers.cc FactoryContext layout"
);

/// `swoole::SendData`.
#[repr(C)]
pub struct SendData {
    pub(crate) info: DataHead,
    pub(crate) data: *const c_void,
}

static CALL: OnceLock<WorkersCall> = OnceLock::new();

/// Registers workers.cc's `FactoryContext::call`, which the methods below run over.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_workers_set_factory_call(call: WorkersCall) {
    // A second registration is the same function; keeping the first is equivalent.
    CALL.get_or_init(|| call);
}

/// `Factory::server_`: the one member after the vtable pointer (workers.cc asserts
/// `sizeof(Factory)`).
///
/// # Safety
/// `factory` is a live `Factory`.
pub(crate) unsafe fn server_of(factory: *const c_void) -> *mut c_void {
    let server = factory.cast::<*mut c_void>().wrapping_add(1);
    // SAFETY: as the caller guarantees; the Itanium layout puts the vptr first.
    unsafe { *server }
}

/// [`run`] over the server of `factory`.
///
/// # Safety
/// `factory` is a live `Factory`; the others are as [`run`] requires.
unsafe fn on(
    factory: *const c_void,
    worker: *mut c_void,
    send: *mut SendData,
    status: *const c_void,
    op: u32,
    arg: u64,
) -> i64 {
    // SAFETY: as the caller guarantees.
    let serv = unsafe { server_of(factory) };
    // SAFETY: as the caller guarantees.
    unsafe { run(serv, worker, send, status, op, arg) }
}

/// Runs `op` over a context of `serv` and the others.
///
/// # Safety
/// `serv` is a live `Server`; the others are what `op` reads, live for the call.
unsafe fn run(
    serv: *mut c_void,
    worker: *mut c_void,
    send: *mut SendData,
    status: *const c_void,
    op: u32,
    arg: u64,
) -> i64 {
    let Some(&call) = CALL.get() else {
        // workers.cc registers before any Server exists.
        std::process::abort()
    };
    let mut ctx = FactoryContext {
        serv,
        worker,
        send,
        status,
        conn: ptr::null_mut(),
        fd: 0,
    };
    // SAFETY: as the caller guarantees; the context lives for the call.
    unsafe { swr_v1_workers_run(op, arg, (&raw mut ctx).cast(), call) }
}

/// A method's `pid_t` from the operation's result.
fn pid(value: i64) -> c_int {
    c_int::try_from(value).unwrap_or(-1)
}

/// `Factory::kill_task_workers()`.
///
/// # Safety
/// `factory` is a live `Factory`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Factory17kill_task_workersEv")
)]
pub unsafe extern "C" fn kill_task_workers(factory: *const c_void) {
    // SAFETY: as the caller guarantees.
    unsafe {
        on(
            factory,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            RUN_KILL_TASK_WORKERS,
            0,
        )
    };
}

/// `Factory::spawn_event_worker(worker)`.
///
/// # Safety
/// `factory` is a live `Factory` and `worker` one of its server's workers.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Factory18spawn_event_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn spawn_event_worker(factory: *const c_void, worker: *mut c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    pid(unsafe {
        on(
            factory,
            worker,
            ptr::null_mut(),
            ptr::null(),
            RUN_SPAWN_EVENT_WORKER,
            0,
        )
    })
}

/// `Factory::spawn_user_worker(worker)`.
///
/// # Safety
/// `factory` is a live `Factory` and `worker` one of its server's user workers.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Factory17spawn_user_workerEPNS_6WorkerE")
)]
pub unsafe extern "C" fn spawn_user_worker(factory: *const c_void, worker: *mut c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    pid(unsafe {
        on(
            factory,
            worker,
            ptr::null_mut(),
            ptr::null(),
            RUN_SPAWN_USER_WORKER,
            0,
        )
    })
}

/// `Factory::check_worker_exit_status(worker, exit_status)`.
///
/// # Safety
/// `factory` is a live `Factory`, `worker` one of its workers and `status` its exit status.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZNK6swoole7Factory24check_worker_exit_statusEPNS_6WorkerERKNS_10ExitStatusE"
    )
)]
pub unsafe extern "C" fn check_worker_exit_status(
    factory: *const c_void,
    worker: *mut c_void,
    status: *const c_void,
) {
    // SAFETY: as the caller guarantees.
    unsafe {
        on(
            factory,
            worker,
            ptr::null_mut(),
            status,
            RUN_CHECK_EXIT_STATUS,
            0,
        )
    };
}

/// `ProcessFactory::start()`.
///
/// # Safety
/// `factory` is a live `ProcessFactory`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole14ProcessFactory5startEv")
)]
pub unsafe extern "C" fn start(factory: *mut c_void) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        on(
            factory,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            RUN_PROCESS_FACTORY_START,
            0,
        ) != 0
    }
}

/// `ProcessFactory::shutdown()`.
///
/// # Safety
/// `factory` is a live `ProcessFactory`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole14ProcessFactory8shutdownEv")
)]
pub unsafe extern "C" fn shutdown(factory: *mut c_void) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        on(
            factory,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            RUN_PROCESS_FACTORY_SHUTDOWN,
            0,
        ) != 0
    }
}

/// `ProcessFactory::dispatch(task)`: to the worker the server schedules.
///
/// # Safety
/// `factory` is a live `ProcessFactory` and `task` a live `SendData`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole14ProcessFactory8dispatchEPNS_8SendDataE")
)]
pub unsafe extern "C" fn dispatch(factory: *mut c_void, task: *mut SendData) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        on(
            factory,
            ptr::null_mut(),
            task,
            ptr::null(),
            RUN_PROCESS_DISPATCH,
            0,
        ) != 0
    }
}

/// `ProcessFactory::notify(ev)`: a dispatch of the event with no data.
///
/// # Safety
/// `factory` is a live `ProcessFactory` and `ev` a live `DataHead`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole14ProcessFactory6notifyEPNS_8DataHeadE")
)]
pub unsafe extern "C" fn notify(factory: *mut c_void, ev: *const DataHead) -> bool {
    let mut task = SendData {
        // SAFETY: as the caller guarantees.
        info: unsafe { *ev },
        data: ptr::null(),
    };
    // SAFETY: as the caller guarantees; `task` lives for the call.
    unsafe { dispatch(factory, &raw mut task) }
}

/// `ProcessFactory::finish(resp)`: to the client, through its reactor.
///
/// # Safety
/// `factory` is a live `ProcessFactory` and `resp` a live `SendData`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole14ProcessFactory6finishEPNS_8SendDataE")
)]
pub unsafe extern "C" fn finish(factory: *mut c_void, resp: *mut SendData) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        on(
            factory,
            ptr::null_mut(),
            resp,
            ptr::null(),
            RUN_PROCESS_FINISH,
            0,
        ) != 0
    }
}

/// `ProcessFactory::end(session_id, flags)`: a close of the session.
///
/// # Safety
/// `factory` is a live `ProcessFactory`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole14ProcessFactory3endEli")
)]
pub unsafe extern "C" fn end(factory: *mut c_void, session_id: c_long, flags: c_int) -> bool {
    let mut close = SendData {
        info: DataHead {
            fd: session_id,
            kind: u8::try_from(EVENT_CLOSE).unwrap_or_default(),
            ..DataHead::default()
        },
        data: ptr::null(),
    };
    // SAFETY: as the caller guarantees; `close` lives for the call.
    unsafe {
        on(
            factory,
            ptr::null_mut(),
            &raw mut close,
            ptr::null(),
            RUN_PROCESS_END,
            u64::from(flags.cast_unsigned()),
        ) != 0
    }
}

/// `Server::create_process_factory()`: the factory, or null.
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe extern "C" fn create_process_factory(serv: *mut c_void) -> *mut c_void {
    // SAFETY: as the caller guarantees.
    let created = unsafe {
        run(
            serv,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            RUN_CREATE_PROCESS_FACTORY,
            0,
        )
    };
    ptr::with_exposed_provenance_mut(usize::try_from(created).unwrap_or(0))
}

/// `Server::destroy_process_factory()`.
///
/// # Safety
/// `serv` is a live `Server` whose process factory was created.
pub unsafe extern "C" fn destroy_process_factory(serv: *mut c_void) {
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            serv,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            RUN_DESTROY_PROCESS_FACTORY,
            0,
        )
    };
}

/// `Server::create_worker_pipes()`.
///
/// # Safety
/// `serv` is a live `Server`.
pub unsafe extern "C" fn create_worker_pipes(serv: *mut c_void) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            serv,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
            RUN_CREATE_WORKER_PIPES,
            0,
        ) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The context each native op saw: server, worker, status, and the send's fd and type.
    type Seen = (usize, usize, usize, i64, u8);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn fake(context: *mut c_void, _op: u32, _arg: u64) -> u64 {
        // SAFETY: the methods pass their own live context.
        let ctx = unsafe { &*context.cast::<FactoryContext>() };
        let (fd, kind) = if ctx.send.is_null() {
            (0, 0)
        } else {
            // SAFETY: a non-null send is live for the call.
            let info = unsafe { &(*ctx.send).info };
            (info.fd, info.kind)
        };
        let seen = (
            ctx.serv.addr(),
            ctx.worker.addr(),
            ctx.status.addr(),
            fd,
            kind,
        );
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(seen);
        0
    }

    fn last() -> Seen {
        let seen = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *seen.last().unwrap_or(&(0, 0, 0, 0, 0))
    }

    #[test]
    fn methods_fill_the_context_their_forward_did() {
        swr_v1_workers_set_factory_call(fake);
        // A `Factory`: the vtable pointer, then `server_`.
        let factory: [usize; 2] = [0, 0x10];
        let this = (&raw const factory).cast::<c_void>().cast_mut();
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            spawn_event_worker(this, at(0x20));
            assert_eq!(
                last(),
                (0x10, 0x20, 0, 0, 0),
                "the factory's server and the worker"
            );
            check_worker_exit_status(this, at(0x20), at(0x40));
            assert_eq!(last().2, 0x40, "the exit status");
            end(this, 9, 6);
            assert_eq!((last().3, last().4), (9, 4), "a close of session 9");
            let head = DataHead {
                fd: 3,
                kind: 5,
                ..DataHead::default()
            };
            notify(this, &raw const head);
            assert_eq!((last().3, last().4), (3, 5), "notify dispatches a copy");
            create_worker_pipes(at(0x30));
            assert_eq!(last().0, 0x30, "server methods take the server");
        }
    }
}
