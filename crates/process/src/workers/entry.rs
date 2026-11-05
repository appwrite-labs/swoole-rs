//! The `Server` methods of the deleted worker-side files (`src/server/manager.cc`),
//! defined under their upstream (Itanium) symbols when the hybrid links this crate
//! (`upstream-symbols`). Each one enters native code with its operation and objects;
//! process.cc's `workers_enter` builds the context the C++ forward built and runs
//! the operation over it.
use core::ffi::{c_int, c_void};
use core::ptr;
use std::sync::OnceLock;

use super::ops::{RUN_READ_WORKER_MESSAGE, RUN_RELOAD, RUN_RESTART_WORKER, RUN_START_MANAGER};

/// process.cc's `workers_enter(operation, object, other, arg)`.
pub type WorkersEnter = unsafe extern "C-unwind" fn(
    operation: u32,
    object: *mut c_void,
    other: *mut c_void,
    arg: u64,
) -> i64;

static ENTER: OnceLock<WorkersEnter> = OnceLock::new();

/// Registers process.cc's `workers_enter`, which the methods below enter.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_workers_set_enter(enter: WorkersEnter) {
    // A second registration is the same function; keeping the first is equivalent.
    ENTER.get_or_init(|| enter);
}

/// Runs `operation` with `arg` over the context native code builds from `object` and `other`.
///
/// # Safety
/// `object` and `other` are what `operation`'s context takes, live for the call.
unsafe fn enter(operation: u32, object: *mut c_void, other: *mut c_void, arg: u64) -> i64 {
    let Some(&enter) = ENTER.get() else {
        // process.cc registers before any Server exists.
        std::process::abort()
    };
    // SAFETY: as the caller guarantees.
    unsafe { enter(operation, object, other, arg) }
}

/// `Server::start_manager_process()`.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server21start_manager_processEv")
)]
pub unsafe extern "C" fn start_manager_process(serv: *mut c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    let result = unsafe { enter(RUN_START_MANAGER, serv, ptr::null_mut(), 0) };
    c_int::try_from(result).unwrap_or(-1)
}

/// `Server::restart_worker_process(exit_status)`: the new worker's pid, or -1.
///
/// # Safety
/// `serv` is a live `Server` and `exit_status` a live `ExitStatus`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server22restart_worker_processERKNS_10ExitStatusE")
)]
pub unsafe extern "C" fn restart_worker_process(
    serv: *mut c_void,
    exit_status: *const c_void,
) -> c_int {
    // SAFETY: as the caller guarantees.
    let pid = unsafe { enter(RUN_RESTART_WORKER, serv, exit_status.cast_mut(), 0) };
    c_int::try_from(pid).unwrap_or(-1)
}

/// `Server::read_worker_message(pool, msg)`, the manager's `onWorkerMessage`.
///
/// # Safety
/// `pool` is the server's live event worker pool and `msg` a live message.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server19read_worker_messageEPNS_11ProcessPoolEPNS_9EventDataE"
    )
)]
pub unsafe extern "C" fn read_worker_message(pool: *mut c_void, msg: *mut c_void) {
    // SAFETY: as the caller guarantees.
    unsafe { enter(RUN_READ_WORKER_MESSAGE, pool, msg, 0) };
}

/// `Server::reload(reload_all_workers)`.
///
/// # Safety
/// `serv` is a live `Server`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server6reloadEb")
)]
pub unsafe extern "C" fn reload(serv: *const c_void, reload_all_workers: bool) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        enter(
            RUN_RELOAD,
            serv.cast_mut(),
            ptr::null_mut(),
            u64::from(reload_all_workers),
        ) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Each entry: operation, object, other, arg.
    static SEEN: Mutex<Vec<(u32, usize, usize, u64)>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn fake(
        operation: u32,
        object: *mut c_void,
        other: *mut c_void,
        arg: u64,
    ) -> i64 {
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((operation, object.addr(), other.addr(), arg));
        if operation == RUN_RESTART_WORKER {
            1234
        } else {
            1
        }
    }

    #[test]
    fn methods_enter_with_their_operation_and_objects() {
        swr_v1_workers_set_enter(fake);
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        // SAFETY: the fake only records the pointers.
        unsafe {
            assert_eq!(start_manager_process(at(0x10)), 1);
            assert_eq!(restart_worker_process(at(0x10), at(0x20)), 1234);
            read_worker_message(at(0x30), at(0x40));
            assert!(reload(at(0x10), true));
        }
        let seen = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            *seen,
            [
                (RUN_START_MANAGER, 0x10, 0, 0),
                (RUN_RESTART_WORKER, 0x10, 0x20, 0),
                (RUN_READ_WORKER_MESSAGE, 0x30, 0x40, 0),
                (RUN_RELOAD, 0x10, 0, 1),
            ]
        );
    }
}
