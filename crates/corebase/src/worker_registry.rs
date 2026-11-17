//! Always-on PID/category and detached-worker ownership shared by Server and `ProcessPool`.
//! Worker pointers are opaque loans: their records retain their own layout and lifetime
//! contracts. Signalling and wait effects run only after a PID snapshot is collected.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

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
pub struct WorkerRegistry<T = *mut c_void> {
    workers: RwLock<HashMap<c_int, Entry>>,
    detached: RwLock<HashSet<c_int>>,
    /// Append-only, so an entry stays where it is while the registry is borrowed:
    /// chunk `k` holds entries `2^k - 1 ..= 2^(k+1) - 2`, made with its first.
    pending_users: [OnceLock<Box<[OnceLock<T>]>>; CHUNKS],
    /// Held while one is added, which a lookup doesn't wait for.
    pending_add: Mutex<()>,
    /// How many `pending_users` holds, which every worker lookup reads.
    pending_count: AtomicUsize,
}

/// How many chunks the pending users may fill: `2^32 - 1` entries.
const CHUNKS: usize = 32;

/// The chunk entry `index` is in, and its place there.
fn chunk_of(index: usize) -> (usize, usize) {
    let at = index + 1;
    let chunk = at.ilog2() as usize;
    (chunk, at - (1 << chunk))
}

/// A read of `lock`; a panic while it was held leaves the data whole.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// A write of `lock`, as [`read`].
fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

impl<T> Default for WorkerRegistry<T> {
    fn default() -> Self {
        Self {
            workers: RwLock::default(),
            detached: RwLock::default(),
            pending_users: [const { OnceLock::new() }; CHUNKS],
            pending_add: Mutex::new(()),
            pending_count: AtomicUsize::new(0),
        }
    }
}

impl<T> WorkerRegistry<T> {
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
    pub fn pending_add(&self, worker: T) -> usize {
        let _adding = self
            .pending_add
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let index = self.pending_count.load(Ordering::Relaxed);
        let (chunk, at) = chunk_of(index);
        let Some(entries) = self.pending_users.get(chunk) else {
            crate::fatal("more pending user workers than a registry holds");
        };
        let entries = entries.get_or_init(|| (0..1 << chunk).map(|_| OnceLock::new()).collect());
        if entries
            .get(at)
            .is_none_or(|entry| entry.set(worker).is_err())
        {
            crate::fatal("a pending user worker's slot is taken");
        }
        self.pending_count.store(index + 1, Ordering::Release);
        index
    }

    /// How many pending user workers were added, without the lock.
    #[must_use]
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.pending_count.load(Ordering::Acquire)
    }

    /// The `index`th pending user worker, in the order they were added, or `None` past
    /// the last.
    #[must_use]
    pub fn pending(&self, index: usize) -> Option<&T> {
        let (chunk, at) = chunk_of(index);
        self.pending_users.get(chunk)?.get()?.get(at)?.get()
    }

    /// Terminate every worker of `category`: send `signo` to each, then wait for each.
    /// `kill(pid, signo)` sends the signal; `wait(pid)` reaps (it is `swoole_waitpid`,
    /// which services signals and timers across `EINTR`); `warn(pid)` is called when a
    /// wait reports failure. Workers stay registered; teardown frees the whole registry.
    pub fn terminate(
        &self,
        category: c_int,
        signo: c_int,
        kill: impl Fn(c_int, c_int) -> std::io::Result<()>,
        wait: impl Fn(c_int) -> c_int,
        warn: impl Fn(c_int),
    ) {
        let pids = self.pids(category);
        for &pid in &pids {
            _ = kill(pid, signo);
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
        let reg: WorkerRegistry = WorkerRegistry::default();
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

    #[expect(
        clippy::unnecessary_wraps,
        reason = "sys::kill's signature, which it stands in for"
    )]
    fn record_kill(pid: c_int, signo: c_int) -> std::io::Result<()> {
        KILLED.with(|k| k.borrow_mut().push((pid, signo)));
        Ok(())
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
        let reg: WorkerRegistry = WorkerRegistry::default();
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
        let reg: WorkerRegistry = WorkerRegistry::default();
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
        let reg: WorkerRegistry = WorkerRegistry::default();
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
        let reg: WorkerRegistry = WorkerRegistry::default();
        let (a, b) = (
            core::ptr::without_provenance_mut(1),
            core::ptr::without_provenance_mut(2),
        );
        assert_eq!(reg.pending_add(a), 0);
        assert_eq!(reg.pending_add(b), 1);
        assert_eq!(reg.pending(0), Some(&a));
        assert_eq!(reg.pending(1), Some(&b));
        assert_eq!(reg.pending(2), None);
        assert_eq!(reg.pending_count(), 2);
        for n in 2..100 {
            assert_eq!(reg.pending_add(core::ptr::without_provenance_mut(n + 1)), n);
        }
        assert!((0..100).all(|n| reg.pending(n).map(|p| p.addr()) == Some(n + 1)));
        assert_eq!(reg.pending(100), None);
    }

    #[test]
    fn user_lookup_cannot_return_an_event_worker_index_entry() {
        let registry: WorkerRegistry = WorkerRegistry::default();
        registry.register(30, core::ptr::without_provenance_mut(11), 1);
        registry.register(40, core::ptr::without_provenance_mut(13), CATEGORY_USER);
        assert!(registry.lookup_category(30, CATEGORY_USER).is_null());
        assert_eq!(registry.lookup_category(40, CATEGORY_USER).addr(), 13);
    }
}
