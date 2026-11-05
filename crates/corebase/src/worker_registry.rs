//! Always-on PID/category and detached-worker ownership shared by Server and `ProcessPool`.
//! Worker pointers are opaque loans: their records retain their own layout and lifetime
//! contracts. Signalling and wait effects run only after a PID snapshot is collected.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};

// `category` is an opaque tag the registry only groups by; its values are the
// `SWR_WORKER_*` constants in the C header (event workers and user workers; task
// workers live in the task pool and are not registered).

/// User workers in the server registry (`SWR_WORKER_USER`).
/// cbindgen:ignore
pub const CATEGORY_USER: c_int = 2;

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
    pub fn pids(&self, category: c_int) -> impl Iterator<Item = c_int> + '_ {
        self.workers
            .iter()
            .filter(move |(_, entry)| entry.category == category)
            .map(|(pid, _)| *pid)
    }
}

/// Scan a caller-owned record array in its original order, including direct PID writes.
/// This accepts both the original native Worker and its canonical Rust layout, without
/// per-record callbacks or a cached PID index that could miss an earlier duplicate.
/// # Safety
/// `array` is null or a live array of `count` records, each `stride` bytes, with an
/// initialized `c_int` PID at `pid_offset`. Reads follow the caller's synchronization.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_find_array(
    array: *mut c_void,
    count: u32,
    stride: usize,
    pid_offset: usize,
    pid: c_int,
) -> *mut c_void {
    if array.is_null() {
        return core::ptr::null_mut();
    }
    for index in 0..usize::try_from(count).unwrap_or_default() {
        let record = array.cast::<u8>().wrapping_add(index.wrapping_mul(stride));
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "Byte-strided native records may be unaligned; the only access uses read_unaligned"
        )]
        let slot = record.wrapping_add(pid_offset).cast::<c_int>();
        // SAFETY: the caller lends this record's initialized PID bytes; no whole-record borrow.
        if unsafe { slot.read_unaligned() } == pid {
            return record.cast();
        }
    }
    core::ptr::null_mut()
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

/// Insert a worker without replacing an existing PID, matching `ProcessPool::add_worker`.
/// # Safety
/// `registry` is live; `worker` is an opaque native loan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_insert(
    registry: *mut WorkerRegistry,
    pid: c_int,
    worker: *mut c_void,
    category: c_int,
) {
    // SAFETY: the caller lends the registry exclusively for this leaf mutation.
    let registry = unsafe { &mut *registry };
    registry
        .workers
        .entry(pid)
        .or_insert(Entry { worker, category });
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

/// Look up a registered worker of the specified category only.
/// # Safety
/// `registry` is a live handle; returned pointers remain native loans.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_lookup_category(
    registry: *const WorkerRegistry,
    pid: c_int,
    category: c_int,
) -> *mut c_void {
    // SAFETY: the caller lends a live registry for this leaf query.
    let registry = unsafe { &*registry };
    registry
        .workers
        .get(&pid)
        .filter(|entry| entry.category == category)
        .map_or(core::ptr::null_mut(), |entry| entry.worker)
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

/// A detached native read view of a Rust worker-pointer vector. Capacity is opaque deallocation metadata.
#[repr(C)]
#[derive(Default)]
pub struct WorkerSnapshot {
    pub data: *mut *mut c_void,
    pub length: usize,
    pub capacity: usize,
}

/// Collect opaque worker pointers before native signalling, waiting or allocation effects can reenter.
/// Returns false on allocation failure, leaving `output` untouched.
/// # Safety
/// `registry` is null or live and synchronized. `output` lends writable disjoint
/// storage; on success its buffer is freed once by `_snapshot_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_snapshot(
    registry: *const WorkerRegistry,
    category: c_int,
    output: *mut WorkerSnapshot,
) -> bool {
    let mut values = Vec::new();
    if !registry.is_null() {
        // SAFETY: the caller lends a live registry for this leaf collection.
        let registry = unsafe { &*registry };
        if values.try_reserve_exact(registry.workers.len()).is_err() {
            return false;
        }
        values.extend(
            registry
                .workers
                .values()
                .filter(|entry| entry.category == category)
                .map(|entry| entry.worker),
        );
    }
    let snapshot = WorkerSnapshot {
        data: if values.capacity() == 0 {
            core::ptr::null_mut()
        } else {
            values.as_mut_ptr()
        },
        length: values.len(),
        capacity: values.capacity(),
    };
    // SAFETY: the caller lends disjoint writable output storage.
    unsafe { output.write(snapshot) };
    core::mem::forget(values);
    true
}

/// Release a successful worker snapshot; its slots become empty so cleanup is repeatable.
/// # Safety
/// `snapshot` lends live initialized slots from `_snapshot`; its buffer metadata
/// has not been modified or freed separately.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_worker_registry_snapshot_free(
    snapshot: *mut WorkerSnapshot,
) {
    // SAFETY: the caller lends initialized snapshot slots for their final release.
    let value = unsafe { snapshot.read() };
    if value.capacity != 0 {
        // SAFETY: this is the exact pointer/length/capacity of the forgotten Rust vector.
        drop(unsafe { Vec::from_raw_parts(value.data, value.length, value.capacity) });
    }
    // SAFETY: the caller lends these slots exclusively until cleanup returns.
    unsafe { snapshot.write(WorkerSnapshot::default()) };
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
            let mut worker_b = 1_u8;
            let b = std::ptr::from_mut(&mut worker_b).cast::<c_void>();
            swr_v1_process_worker_registry_insert(reg, 100, b, CATEGORY_USER);
            assert_eq!(swr_v1_process_worker_registry_lookup(reg, 100), a);
            assert_eq!(
                swr_v1_process_worker_registry_lookup_category(reg, 100, CATEGORY_EVENT),
                a
            );
            assert!(swr_v1_process_worker_registry_lookup(reg, 999).is_null());
            assert!(swr_v1_process_worker_registry_unregister(reg, 100));
            assert!(!swr_v1_process_worker_registry_unregister(reg, 100));
            assert!(swr_v1_process_worker_registry_lookup(reg, 100).is_null());
            swr_v1_process_worker_registry_free(reg);
        }
    }

    #[test]
    fn array_lookup_reads_unaligned_slots_and_keeps_first_match_after_raw_writes() {
        let mut bytes = [0_u8; 26];
        let array = bytes.as_mut_ptr().cast::<c_void>();
        unsafe {
            assert_eq!(
                swr_v1_process_worker_registry_find_array(array, 2, 13, 3, 0),
                array
            );
            bytes[3..7].copy_from_slice(&9_i32.to_ne_bytes());
            assert_eq!(
                swr_v1_process_worker_registry_find_array(array, 2, 13, 3, 0),
                array.cast::<u8>().add(13).cast()
            );
            bytes[3..7].copy_from_slice(&0_i32.to_ne_bytes());
            assert_eq!(
                swr_v1_process_worker_registry_find_array(array, 2, 13, 3, 0),
                array
            );
            assert!(swr_v1_process_worker_registry_find_array(array, 2, 13, 3, 7).is_null());
            assert!(
                swr_v1_process_worker_registry_find_array(core::ptr::null_mut(), 2, 13, 3, 0)
                    .is_null()
            );
        }
    }

    #[test]
    fn native_pointer_snapshot_outlives_registry_mutation_and_destruction() {
        let reg = registry();
        let mut first = 1_u8;
        let mut second = 2_u8;
        let a = core::ptr::from_mut(&mut first).cast();
        let b = core::ptr::from_mut(&mut second).cast();
        let mut snapshot = WorkerSnapshot::default();
        unsafe {
            swr_v1_process_worker_registry_register(reg, 10, a, CATEGORY_USER);
            swr_v1_process_worker_registry_register(reg, 20, b, CATEGORY_EVENT);
            assert!(swr_v1_process_worker_registry_snapshot(
                reg,
                CATEGORY_USER,
                &raw mut snapshot
            ));
            swr_v1_process_worker_registry_unregister(reg, 10);
            swr_v1_process_worker_registry_free(reg);
            assert_eq!(snapshot.length, 1);
            assert_eq!(*snapshot.data, a);
            swr_v1_process_worker_registry_snapshot_free(&raw mut snapshot);
            assert!(snapshot.data.is_null());
            assert_eq!((snapshot.length, snapshot.capacity), (0, 0));
            swr_v1_process_worker_registry_snapshot_free(&raw mut snapshot);
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

#[cfg(test)]
mod category_compat_tests {
    use super::*;
    #[test]
    fn user_lookup_cannot_return_an_event_worker_index_entry() {
        let mut registry = WorkerRegistry::default();
        unsafe {
            swr_v1_process_worker_registry_register(
                &raw mut registry,
                30,
                core::ptr::without_provenance_mut(11),
                1,
            );
            swr_v1_process_worker_registry_register(
                &raw mut registry,
                40,
                core::ptr::without_provenance_mut(13),
                CATEGORY_USER,
            );
            assert!(
                swr_v1_process_worker_registry_lookup_category(
                    &raw const registry,
                    30,
                    CATEGORY_USER
                )
                .is_null()
            );
            assert_eq!(
                swr_v1_process_worker_registry_lookup_category(
                    &raw const registry,
                    40,
                    CATEGORY_USER
                )
                .addr(),
                13
            );
        }
    }
}
