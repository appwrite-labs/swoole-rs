//! Always-on PID/category and detached-worker ownership shared by Server and `ProcessPool`.
//! Worker pointers are opaque loans: their records retain their own layout and lifetime
//! contracts. Signalling and wait effects run only after a PID snapshot is collected.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

// `category` is an opaque tag the registry only groups by: event workers (1) and
// user workers (2); task workers live in the task pool and are not registered.

/// User workers in the server registry.
pub const CATEGORY_USER: c_int = 2;

struct Entry {
    worker: *mut c_void,
    category: c_int,
}

/// pid -> (native `Worker*`, category). Read and written through `&`, each call
/// under its own lock: the master writes it, and its reactor threads look workers
/// up concurrently, so no caller holds `&mut` to it.
#[derive(Default)]
pub struct WorkerRegistry {
    workers: RwLock<HashMap<c_int, Entry>>,
    detached: RwLock<HashSet<c_int>>,
    pending_users: RwLock<Vec<*mut c_void>>,
    /// How many `pending_users` holds, which every worker lookup reads.
    pending_count: AtomicUsize,
}

/// A read of `lock`; a panic while it was held leaves the data whole.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// A write of `lock`, as [`read`].
fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

impl WorkerRegistry {
    #[must_use]
    pub fn pids(&self, category: c_int) -> Vec<c_int> {
        read(&self.workers)
            .iter()
            .filter(|(_, entry)| entry.category == category)
            .map(|(pid, _)| *pid)
            .collect()
    }

    /// Register (or overwrite) the worker at `pid` with its pointer and category.
    pub fn register(&self, pid: c_int, worker: *mut c_void, category: c_int) {
        write(&self.workers).insert(pid, Entry { worker, category });
    }

    /// Insert a worker without replacing an existing PID, matching `ProcessPool::add_worker`.
    pub fn insert(&self, pid: c_int, worker: *mut c_void, category: c_int) {
        write(&self.workers)
            .entry(pid)
            .or_insert(Entry { worker, category });
    }

    /// Remove the worker at `pid`. Returns whether one was present.
    pub fn unregister(&self, pid: c_int) -> bool {
        write(&self.workers).remove(&pid).is_some()
    }

    /// The pointer registered for `pid`, or null.
    #[must_use]
    pub fn lookup(&self, pid: c_int) -> *mut c_void {
        read(&self.workers)
            .get(&pid)
            .map_or(std::ptr::null_mut(), |entry| entry.worker)
    }

    /// The pointer registered for `pid` in `category` only, or null.
    #[must_use]
    pub fn lookup_category(&self, pid: c_int, category: c_int) -> *mut c_void {
        read(&self.workers)
            .get(&pid)
            .filter(|entry| entry.category == category)
            .map_or(std::ptr::null_mut(), |entry| entry.worker)
    }

    /// Detach the worker at `pid`: it has been replaced and exits on its own, so it
    /// leaves the lookup and its exit is later reaped by [`Self::reap_detached`].
    pub fn detach(&self, pid: c_int) {
        write(&self.workers).remove(&pid);
        write(&self.detached).insert(pid);
    }

    /// Reap a detached worker's exit. Returns false when `pid` was never detached.
    pub fn reap_detached(&self, pid: c_int) -> bool {
        write(&self.detached).remove(&pid)
    }

    /// Append one pending user worker before Server creation, preserving insertion
    /// order. The returned index is its original pre-start worker ID.
    pub fn pending_add(&self, worker: *mut c_void) -> usize {
        let mut pending = write(&self.pending_users);
        pending.push(worker);
        self.pending_count.store(pending.len(), Ordering::Release);
        pending.len() - 1
    }

    /// How many pending user workers were added, without the lock.
    #[must_use]
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.pending_count.load(Ordering::Acquire)
    }

    /// The pending user workers, in the order they were added.
    pub fn pending(&self) -> RwLockReadGuard<'_, Vec<*mut c_void>> {
        read(&self.pending_users)
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
        let pids = self.pids(category);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const CATEGORY_EVENT: c_int = 1;

    #[test]
    fn registers_looks_up_and_unregisters() {
        let reg = WorkerRegistry::default();
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
        let reg = WorkerRegistry::default();
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
        let reg = WorkerRegistry::default();
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
        let reg = WorkerRegistry::default();
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
        let reg = WorkerRegistry::default();
        let (a, b) = (
            core::ptr::without_provenance_mut(1),
            core::ptr::without_provenance_mut(2),
        );
        assert_eq!(reg.pending_add(a), 0);
        assert_eq!(reg.pending_add(b), 1);
        assert_eq!(*reg.pending(), [a, b]);
        assert_eq!(reg.pending_count(), 2);
    }

    #[test]
    fn user_lookup_cannot_return_an_event_worker_index_entry() {
        let registry = WorkerRegistry::default();
        registry.register(30, core::ptr::without_provenance_mut(11), 1);
        registry.register(40, core::ptr::without_provenance_mut(13), CATEGORY_USER);
        assert!(registry.lookup_category(30, CATEGORY_USER).is_null());
        assert_eq!(registry.lookup_category(40, CATEGORY_USER).addr(), 13);
    }
}
