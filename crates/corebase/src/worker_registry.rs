//! Always-on PID/category and detached-worker ownership shared by Server and `ProcessPool`.
//! Worker pointers are opaque loans: their records retain their own layout and lifetime
//! contracts. Signalling and wait effects run only after a PID snapshot is collected.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};

// `category` is an opaque tag the registry only groups by: event workers (1) and
// user workers (2); task workers live in the task pool and are not registered.

/// User workers in the server registry.
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
    pending_users: Vec<*mut c_void>,
}

impl WorkerRegistry {
    pub fn pids(&self, category: c_int) -> impl Iterator<Item = c_int> + '_ {
        self.workers
            .iter()
            .filter(move |(_, entry)| entry.category == category)
            .map(|(pid, _)| *pid)
    }

    /// Register (or overwrite) the worker at `pid` with its pointer and category.
    pub fn register(&mut self, pid: c_int, worker: *mut c_void, category: c_int) {
        self.workers.insert(pid, Entry { worker, category });
    }

    /// Insert a worker without replacing an existing PID, matching `ProcessPool::add_worker`.
    pub fn insert(&mut self, pid: c_int, worker: *mut c_void, category: c_int) {
        self.workers
            .entry(pid)
            .or_insert(Entry { worker, category });
    }

    /// Remove the worker at `pid`. Returns whether one was present.
    pub fn unregister(&mut self, pid: c_int) -> bool {
        self.workers.remove(&pid).is_some()
    }

    /// The pointer registered for `pid`, or null.
    #[must_use]
    pub fn lookup(&self, pid: c_int) -> *mut c_void {
        self.workers
            .get(&pid)
            .map_or(std::ptr::null_mut(), |entry| entry.worker)
    }

    /// The pointer registered for `pid` in `category` only, or null.
    #[must_use]
    pub fn lookup_category(&self, pid: c_int, category: c_int) -> *mut c_void {
        self.workers
            .get(&pid)
            .filter(|entry| entry.category == category)
            .map_or(std::ptr::null_mut(), |entry| entry.worker)
    }

    /// Detach the worker at `pid`: it has been replaced and exits on its own, so it
    /// leaves the lookup and its exit is later reaped by [`Self::reap_detached`].
    pub fn detach(&mut self, pid: c_int) {
        self.workers.remove(&pid);
        self.detached.insert(pid);
    }

    /// Reap a detached worker's exit. Returns false when `pid` was never detached.
    pub fn reap_detached(&mut self, pid: c_int) -> bool {
        self.detached.remove(&pid)
    }

    /// Append one pending user worker before Server creation, preserving insertion
    /// order. The returned index is its original pre-start worker ID.
    pub fn pending_add(&mut self, worker: *mut c_void) -> usize {
        self.pending_users.push(worker);
        self.pending_users.len() - 1
    }

    /// The pending user workers, in the order they were added.
    #[must_use]
    pub fn pending(&self) -> &[*mut c_void] {
        &self.pending_users
    }

    /// Terminate every worker of `category`: send `signo` to each, then wait for each.
    /// `kill(pid, signo)` sends the signal; `wait(pid)` reaps (it is `swoole_waitpid`,
    /// which services signals and timers across `EINTR`); `warn(pid)` is called when a
    /// wait reports failure. Workers stay registered; teardown frees the whole registry.
    pub fn terminate(
        &self,
        category: c_int,
        signo: c_int,
        kill: impl Fn(c_int, c_int) -> c_int,
        wait: impl Fn(c_int) -> c_int,
        warn: impl Fn(c_int),
    ) {
        let pids: Vec<c_int> = self.pids(category).collect();
        for &pid in &pids {
            kill(pid, signo);
        }
        for &pid in &pids {
            if wait(pid) < 0 {
                warn(pid);
            }
        }
    }
}

/// Scan a caller-owned record array in its original order, including direct PID writes.
/// This accepts both the original native Worker and its canonical Rust layout, without
/// per-record callbacks or a cached PID index that could miss an earlier duplicate.
/// # Safety
/// `array` is null or a live array of `count` records, each `stride` bytes, with an
/// initialized `c_int` PID at `pid_offset`. Reads follow the caller's synchronization.
pub unsafe fn find_array(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const CATEGORY_EVENT: c_int = 1;

    #[test]
    fn registers_looks_up_and_unregisters() {
        let mut reg = WorkerRegistry::default();
        let mut worker_a = 0u8;
        let a = std::ptr::from_mut(&mut worker_a).cast::<c_void>();
        reg.register(100, a, CATEGORY_EVENT);
        let mut worker_b = 1_u8;
        let b = std::ptr::from_mut(&mut worker_b).cast::<c_void>();
        reg.insert(100, b, CATEGORY_USER);
        assert_eq!(reg.lookup(100), a);
        assert_eq!(reg.lookup_category(100, CATEGORY_EVENT), a);
        assert!(reg.lookup(999).is_null());
        assert!(reg.unregister(100));
        assert!(!reg.unregister(100));
        assert!(reg.lookup(100).is_null());
    }

    #[test]
    fn array_lookup_reads_unaligned_slots_and_keeps_first_match_after_raw_writes() {
        let mut bytes = [0_u8; 26];
        let array = bytes.as_mut_ptr().cast::<c_void>();
        // The first record's PID, written through `array`: a write to `bytes` would end it.
        let pid = array.cast::<u8>().wrapping_add(3).cast::<[u8; 4]>();
        unsafe {
            assert_eq!(find_array(array, 2, 13, 3, 0), array);
            pid.write(9_i32.to_ne_bytes());
            assert_eq!(
                find_array(array, 2, 13, 3, 0),
                array.cast::<u8>().add(13).cast()
            );
            pid.write(0_i32.to_ne_bytes());
            assert_eq!(find_array(array, 2, 13, 3, 0), array);
            assert!(find_array(array, 2, 13, 3, 7).is_null());
            assert!(find_array(core::ptr::null_mut(), 2, 13, 3, 0).is_null());
        }
    }

    thread_local! {
        static KILLED: RefCell<Vec<(c_int, c_int)>> = const { RefCell::new(Vec::new()) };
        static WAITED: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
        static WARNED: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
    }

    fn record_kill(pid: c_int, signo: c_int) -> c_int {
        KILLED.with(|k| k.borrow_mut().push((pid, signo)));
        0
    }
    fn record_wait_ok(pid: c_int) -> c_int {
        WAITED.with(|w| w.borrow_mut().push(pid));
        pid
    }
    fn record_wait_fail(pid: c_int) -> c_int {
        WAITED.with(|w| w.borrow_mut().push(pid));
        -1
    }
    fn record_warn(pid: c_int) {
        WARNED.with(|w| w.borrow_mut().push(pid));
    }

    #[test]
    fn terminate_hits_only_its_category_kill_all_then_wait_all() {
        let mut reg = WorkerRegistry::default();
        let mut w = 0u8;
        let p = std::ptr::from_mut(&mut w).cast::<c_void>();
        reg.register(10, p, CATEGORY_EVENT);
        reg.register(11, p, CATEGORY_EVENT);
        reg.register(20, p, CATEGORY_USER);
        reg.terminate(CATEGORY_EVENT, 15, record_kill, record_wait_ok, record_warn);
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
        let mut reg = WorkerRegistry::default();
        let mut w = 0u8;
        let p = std::ptr::from_mut(&mut w).cast::<c_void>();
        WARNED.with(|w| w.borrow_mut().clear());
        reg.register(30, p, CATEGORY_USER);
        reg.terminate(
            CATEGORY_USER,
            15,
            record_kill,
            record_wait_fail,
            record_warn,
        );
        assert_eq!(WARNED.with(|w| w.borrow().clone()), vec![30]);
    }

    #[test]
    fn detached_workers_leave_the_lookup_and_are_reaped_once() {
        let mut reg = WorkerRegistry::default();
        let mut w = 0u8;
        let p = std::ptr::from_mut(&mut w).cast::<c_void>();
        reg.register(40, p, CATEGORY_EVENT);
        reg.detach(40);
        assert!(reg.lookup(40).is_null());
        assert!(!reg.reap_detached(41));
        assert!(reg.reap_detached(40));
        assert!(!reg.reap_detached(40));
    }

    #[test]
    fn pending_workers_keep_their_order_and_indexes() {
        let mut reg = WorkerRegistry::default();
        let (a, b) = (
            core::ptr::without_provenance_mut(1),
            core::ptr::without_provenance_mut(2),
        );
        assert_eq!(reg.pending_add(a), 0);
        assert_eq!(reg.pending_add(b), 1);
        assert_eq!(reg.pending(), [a, b]);
    }

    #[test]
    fn user_lookup_cannot_return_an_event_worker_index_entry() {
        let mut registry = WorkerRegistry::default();
        registry.register(30, core::ptr::without_provenance_mut(11), 1);
        registry.register(40, core::ptr::without_provenance_mut(13), CATEGORY_USER);
        assert!(registry.lookup_category(30, CATEGORY_USER).is_null());
        assert_eq!(registry.lookup_category(40, CATEGORY_USER).addr(), 13);
    }
}
