//! The server's worker lookup, owned in Rust behind an opaque handle. It maps a
//! worker's pid to its native `Worker*` and category, replacing
//! `Server::get_worker_by_pid`'s event-worker scan and user-map branch and the
//! `kill_event_workers` / `kill_user_workers` loops. Native registers a worker when
//! it spawns and unregisters it when it is reaped or detached; fork, signalling and
//! every `Worker` field stay native. A `ProcessPool` (the task pool, or a
//! `Swoole\Process\Pool`) keeps its own registry of its workers. Each registry also
//! holds the detached set: pids of workers that asked to be replaced before exiting,
//! which the manager reaps without restarting.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};

// `category` is an opaque tag the registry only groups by; its values are the
// `SWR_WORKER_*` constants in the C header (event workers and user workers; task
// workers live in the task pool and are not registered).

/// User workers in the server registry (`SWR_WORKER_USER`).
pub(crate) const CATEGORY_USER: c_int = 2;

struct Entry {
    worker: *mut c_void,
    category: c_int,
}

/// pid -> (native `Worker*`, category).
#[derive(Default)]
pub struct WorkerRegistry {
    workers: HashMap<c_int, Entry>,
    detached: HashSet<c_int>,
}

impl WorkerRegistry {
    pub(crate) fn pids(&self, category: c_int) -> impl Iterator<Item = c_int> + '_ {
        self.workers
            .iter()
            .filter(move |(_, entry)| entry.category == category)
            .map(|(pid, _)| *pid)
    }
}

/// Create an empty registry. Native frees it with [`swr_v1_process_worker_registry_free`].
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_worker_registry_new() -> *mut WorkerRegistry {
    Box::into_raw(Box::<WorkerRegistry>::default())
}

/// Free a registry.
///
/// # Safety
/// `registry` is null or a live handle from `_new`, not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_free(registry: *mut WorkerRegistry) {
    if registry.is_null() {
        return;
    }
    // SAFETY: the caller lends a live handle previously leaked by `_new`.
    drop(unsafe { Box::from_raw(registry) });
}

/// Register (or overwrite) the worker at `pid` with its native pointer and category
/// (`SWR_WORKER_EVENT` or `SWR_WORKER_USER`).
///
/// # Safety
/// `registry` is a live handle; `worker` is an opaque pointer the registry only stores.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_register(
    registry: *mut WorkerRegistry,
    pid: c_int,
    worker: *mut c_void,
    category: c_int,
) {
    // SAFETY: the caller lends a live registry handle.
    let registry = unsafe { &mut *registry };
    registry.workers.insert(pid, Entry { worker, category });
}

/// Remove the worker at `pid`. Returns whether one was present.
///
/// # Safety
/// `registry` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_unregister(
    registry: *mut WorkerRegistry,
    pid: c_int,
) -> bool {
    // SAFETY: the caller lends a live registry handle.
    let registry = unsafe { &mut *registry };
    registry.workers.remove(&pid).is_some()
}

/// The native pointer registered for `pid`, or null.
///
/// # Safety
/// `registry` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_lookup(
    registry: *const WorkerRegistry,
    pid: c_int,
) -> *mut c_void {
    // SAFETY: the caller lends a live registry handle.
    let registry = unsafe { &*registry };
    registry
        .workers
        .get(&pid)
        .map_or(std::ptr::null_mut(), |entry| entry.worker)
}

/// Detach the worker at `pid`: it has been replaced and exits on its own, so it
/// leaves the lookup and its exit is later reaped by
/// [`swr_v1_process_worker_registry_reap_detached`].
///
/// # Safety
/// `registry` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_detach(
    registry: *mut WorkerRegistry,
    pid: c_int,
) {
    // SAFETY: the caller lends a live registry handle.
    let registry = unsafe { &mut *registry };
    registry.workers.remove(&pid);
    registry.detached.insert(pid);
}

/// Reap a detached worker's exit. Returns false when `pid` was never detached.
///
/// # Safety
/// `registry` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_reap_detached(
    registry: *mut WorkerRegistry,
    pid: c_int,
) -> bool {
    // SAFETY: the caller lends a live registry handle.
    let registry = unsafe { &mut *registry };
    registry.detached.remove(&pid)
}

/// Terminate every worker of `category`: send `signo` to each, then wait for each.
/// `kill_fn(pid, signo)` sends the signal; `wait_fn(pid, &status, 0)` reaps (it is
/// `swoole_waitpid`, which services signals and timers across `EINTR`); `warn_fn(pid)`
/// is called when a wait reports failure. Workers stay registered; teardown frees the
/// whole registry.
///
/// # Safety
/// `registry` is a live handle; the three callbacks are valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_terminate(
    registry: *const WorkerRegistry,
    category: c_int,
    signo: c_int,
    kill_fn: extern "C" fn(c_int, c_int) -> c_int,
    wait_fn: extern "C" fn(c_int, *mut c_int, c_int) -> c_int,
    warn_fn: extern "C" fn(c_int),
) {
    // SAFETY: the caller lends a live registry handle.
    let registry = unsafe { &*registry };
    let pids: Vec<c_int> = registry.pids(category).collect();
    for &pid in &pids {
        kill_fn(pid, signo);
    }
    for &pid in &pids {
        let mut status: c_int = 0;
        if wait_fn(pid, &raw mut status, 0) < 0 {
            warn_fn(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const CATEGORY_EVENT: c_int = 1;

    fn registry() -> *mut WorkerRegistry {
        swr_v1_process_worker_registry_new()
    }

    #[test]
    fn registers_looks_up_and_unregisters() {
        let reg = registry();
        let mut worker_a = 0u8;
        let a = std::ptr::from_mut(&mut worker_a).cast::<c_void>();
        unsafe {
            swr_v1_process_worker_registry_register(reg, 100, a, CATEGORY_EVENT);
            assert_eq!(swr_v1_process_worker_registry_lookup(reg, 100), a);
            assert!(swr_v1_process_worker_registry_lookup(reg, 999).is_null());
            assert!(swr_v1_process_worker_registry_unregister(reg, 100));
            assert!(!swr_v1_process_worker_registry_unregister(reg, 100));
            assert!(swr_v1_process_worker_registry_lookup(reg, 100).is_null());
            swr_v1_process_worker_registry_free(reg);
        }
    }

    thread_local! {
        static KILLED: RefCell<Vec<(c_int, c_int)>> = const { RefCell::new(Vec::new()) };
        static WAITED: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
        static WARNED: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
    }

    extern "C" fn record_kill(pid: c_int, signo: c_int) -> c_int {
        KILLED.with(|k| k.borrow_mut().push((pid, signo)));
        0
    }
    extern "C" fn record_wait_ok(pid: c_int, _status: *mut c_int, _options: c_int) -> c_int {
        WAITED.with(|w| w.borrow_mut().push(pid));
        pid
    }
    extern "C" fn record_wait_fail(pid: c_int, _status: *mut c_int, _options: c_int) -> c_int {
        WAITED.with(|w| w.borrow_mut().push(pid));
        -1
    }
    extern "C" fn record_warn(pid: c_int) {
        WARNED.with(|w| w.borrow_mut().push(pid));
    }

    #[test]
    fn terminate_hits_only_its_category_kill_all_then_wait_all() {
        let reg = registry();
        let mut w = 0u8;
        let p = std::ptr::from_mut(&mut w).cast::<c_void>();
        unsafe {
            swr_v1_process_worker_registry_register(reg, 10, p, CATEGORY_EVENT);
            swr_v1_process_worker_registry_register(reg, 11, p, CATEGORY_EVENT);
            swr_v1_process_worker_registry_register(reg, 20, p, CATEGORY_USER);
            swr_v1_process_worker_registry_terminate(
                reg,
                CATEGORY_EVENT,
                15,
                record_kill,
                record_wait_ok,
                record_warn,
            );
            swr_v1_process_worker_registry_free(reg);
        }
        let mut killed = KILLED.with(|k| k.borrow().clone());
        let mut waited = WAITED.with(|w| w.borrow().clone());
        killed.sort_unstable();
        waited.sort_unstable();
        assert_eq!(killed, vec![(10, 15), (11, 15)]);
        assert_eq!(waited, vec![10, 11]);
        assert!(WARNED.with(|w| w.borrow().is_empty()));
    }

    #[test]
    fn terminate_warns_on_wait_failure() {
        let reg = registry();
        let mut w = 0u8;
        let p = std::ptr::from_mut(&mut w).cast::<c_void>();
        WARNED.with(|w| w.borrow_mut().clear());
        unsafe {
            swr_v1_process_worker_registry_register(reg, 30, p, CATEGORY_USER);
            swr_v1_process_worker_registry_terminate(
                reg,
                CATEGORY_USER,
                15,
                record_kill,
                record_wait_fail,
                record_warn,
            );
            swr_v1_process_worker_registry_free(reg);
        }
        assert_eq!(WARNED.with(|w| w.borrow().clone()), vec![30]);
    }

    #[test]
    fn detached_workers_leave_the_lookup_and_are_reaped_once() {
        let reg = registry();
        let mut w = 0u8;
        let p = std::ptr::from_mut(&mut w).cast::<c_void>();
        unsafe {
            swr_v1_process_worker_registry_register(reg, 40, p, CATEGORY_EVENT);
            swr_v1_process_worker_registry_detach(reg, 40);
            assert!(swr_v1_process_worker_registry_lookup(reg, 40).is_null());
            assert!(!swr_v1_process_worker_registry_reap_detached(reg, 41));
            assert!(swr_v1_process_worker_registry_reap_detached(reg, 40));
            assert!(!swr_v1_process_worker_registry_reap_detached(reg, 40));
            swr_v1_process_worker_registry_free(reg);
        }
    }

    #[test]
    fn free_handles_null() {
        unsafe { swr_v1_process_worker_registry_free(std::ptr::null_mut()) };
    }
}
