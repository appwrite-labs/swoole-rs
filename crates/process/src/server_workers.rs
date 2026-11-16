//! Server startup's worker-array publication and ordered lookup. Native calls
//! enter here between the daemon/fork and factory/user callbacks, so no Rust
//! frame spans a fork hook, PHP bailout or whole-thread exit.
use crate::process_pool::{PoolObjectKey, PoolRef};
use crate::worker_record::Worker;
use crate::worker_record::WorkerRecord;
use core::ffi::c_void;

/// Loans of the existing canonical arrays. Event/task records belong to their
/// shared pools; user records belong to the Server's shared allocation. An array
/// not mapped yet is empty, while its count is still the configured one.
pub struct ServerWorkers<'a> {
    event_pool: PoolRef<'a>,
    task_pool: PoolRef<'a>,
    event_workers: &'a [WorkerRecord],
    user_workers: &'a [WorkerRecord],
    event_count: u32,
    task_count: u32,
    user_count: usize,
}

impl<'a> ServerWorkers<'a> {
    /// A server's worker arrays and counts: the event workers (the pool, its array and
    /// count), the task pool and its count, and the user workers' array and count.
    /// Each array is empty (not mapped yet) or holds exactly its count.
    #[must_use]
    #[inline]
    pub fn new(
        (event_pool, event_workers, event_count): (PoolRef<'a>, &'a [WorkerRecord], u32),
        (task_pool, task_count): (PoolRef<'a>, u32),
        (user_workers, user_count): (&'a [WorkerRecord], usize),
    ) -> Self {
        Self {
            event_pool,
            task_pool,
            event_workers,
            user_workers,
            event_count,
            task_count,
            user_count,
        }
    }

    /// How many event workers the server has.
    #[must_use]
    #[inline]
    pub fn event_count(&self) -> u32 {
        self.event_count
    }

    /// How many task workers the server has.
    #[must_use]
    #[inline]
    pub fn task_count(&self) -> u32 {
        self.task_count
    }

    /// How many user workers the server has.
    #[must_use]
    #[inline]
    pub fn user_count(&self) -> usize {
        self.user_count
    }

    /// Event worker `index`, or `None` past the last or before the array exists.
    fn event(&self, index: usize) -> Option<Worker<'a>> {
        Self::slot(self.event_workers, index)
    }

    /// User worker `index`, or `None` past the last or before the array exists.
    #[must_use]
    pub fn user(&self, index: usize) -> Option<Worker<'a>> {
        Self::slot(self.user_workers, index)
    }

    /// Record `index` of one of the arrays `new` was given, each empty or holding
    /// exactly its count.
    #[inline]
    fn slot(workers: &'a [WorkerRecord], index: usize) -> Option<Worker<'a>> {
        workers.get(index).map(Worker::from)
    }

    /// The event array as the pool holds it, and how many of its records the pool
    /// may count: null and `event_count` before it is mapped (the pool then has none).
    fn event_array(&self, count: u32) -> (*mut WorkerRecord, u32) {
        if self.event_workers.is_empty() {
            return (core::ptr::null_mut(), count);
        }
        let len = u32::try_from(self.event_workers.len()).unwrap_or(u32::MAX);
        (self.event_workers.as_ptr().cast_mut(), count.min(len))
    }

    /// `Server::get_worker(id)`: event workers, then task workers, then user workers;
    /// `None` past the last.
    #[must_use]
    pub fn get(&self, id: u16) -> Option<Worker<'a>> {
        let id = u32::from(id);
        if id < self.event_count {
            // The pool holds the event array from `start`; before it, `workers[id]` is
            // the array's, which `get_idle_worker_id` reads after `create`.
            return if self.event_pool.workers().is_null() {
                self.event(id as usize)
            } else {
                self.event_pool.worker(id as usize)
            };
        }
        let user_start = self.event_count.wrapping_add(self.task_count);
        if id < user_start {
            // The task pool's canonical array has task_count slots.
            let index = id.wrapping_sub(self.event_count) as usize;
            return self.task_pool.worker(index);
        }
        self.user(id.wrapping_sub(user_start) as usize)
    }

    /// Base mode's reactor process, after `create` gave the event pool records of
    /// its own: copies them into the server's event array, which the pool then uses,
    /// numbered and typed as event workers of the pool. Nothing when either array is
    /// missing.
    pub fn adopt_event_workers(&self) {
        let pool = self.event_pool;
        if pool.workers().is_null() || self.event_workers.is_empty() {
            return;
        }
        let (workers, count) = self.event_array(pool.worker_num().min(self.event_count));
        for index in 0..count as usize {
            if let Some((from, to)) = pool.worker(index).zip(self.event(index)) {
                to.put(from.get());
            }
        }
        // The server's array, lent for as long as the pool, of at least `count` records.
        pool.set_workers(workers);
        pool.set_worker_num(count);
        for (id, index) in (0u32..).zip(0..count as usize) {
            if let Some(worker) = pool.worker(index) {
                worker.join(pool);
                worker.set_id(id);
                worker.set_kind(crate::worker_record::WorkerType::Event);
            }
        }
    }
}

/// Publish the event pool and initialize worker identities before the factory
/// starts. `users` are the original ordered configuration list, of which the first
/// `user_count` are numbered; lookup still uses the distinct shared user array
/// after creation. The event pool holds `server` under `key` ([`PoolRef::hold`]).
pub fn swr_v1_process_server_workers_start<'w>(
    view: &ServerWorkers<'_>,
    (key, server): (&'static PoolObjectKey, *mut c_void),
    users: impl IntoIterator<Item = Worker<'w>>,
) {
    // Startup owns the event pool before the factory starts; a field at a time.
    let pool = view.event_pool;
    pool.hold(key, server);
    // The canonical event array, lent for as long as the pool, of at least the count.
    let (workers, count) = view.event_array(view.event_count);
    pool.set_workers(workers);
    pool.set_worker_num(count);
    // This original event pool uses its pipes rather than a message queue.
    pool.set_use_msgqueue(false);
    for index in 0..view.event_count {
        // The array just published holds `event_count` records.
        let Some(worker) = pool.worker(index as usize) else {
            break;
        };
        worker.join(pool);
        worker.set_id(index);
        worker.set_kind(crate::worker_record::WorkerType::Event);
    }
    let first_user = view.event_count.wrapping_add(view.task_count);
    for (index, worker) in users.into_iter().enumerate().take(view.user_count) {
        let index = u32::try_from(index & u32::MAX as usize).unwrap_or_default();
        // Startup owns the pending user worker identity; preserve u32 wrap.
        worker.set_id(first_user.wrapping_add(index));
    }
}

/// Select a worker from the event, task or user category in its original order:
/// its slot's address in the selected array, which nothing here reads, or null past
/// the last. The pointer is a loan of that array, without extending its lifetime.
#[must_use]
pub fn swr_v1_process_server_worker(view: &ServerWorkers<'_>, id: u16) -> *mut WorkerRecord {
    view.get(id).map_or(core::ptr::null_mut(), Worker::raw)
}

/// Ordered Server PID lookup: event records first, then the task pool's live
/// registry, then user entries in the Server registry. PID zero/negative values
/// and direct record writes retain their original lookup behavior. Lookup is
/// serialized with registry mutation by the registries' own locks.
#[must_use]
pub fn swr_v1_process_server_worker_by_pid(
    view: &ServerWorkers<'_>,
    registry: Option<&crate::worker_registry::WorkerRegistry>,
    pid: core::ffi::c_int,
) -> *mut WorkerRecord {
    // The canonical ordered event records: the first with this pid, in array order,
    // reading the pid slots raw writes reach.
    let event = (0..view.event_count as usize)
        .map_while(|index| view.event(index))
        .find(|worker| worker.pid() == pid);
    if let Some(worker) = event {
        return worker.raw();
    }
    // The task pool keeps its registry.
    let worker = view
        .task_pool
        .record()
        .with_registry(|tasks| tasks.lookup(pid))
        .unwrap_or(core::ptr::null_mut());
    if !worker.is_null() {
        return worker.cast();
    }
    // The Server's registry, in which only the user category matches.
    registry
        .map_or(core::ptr::null_mut(), |registry| {
            registry.lookup_category(pid, crate::worker_registry::CATEGORY_USER)
        })
        .cast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_pool::PoolRecord;

    fn worker() -> WorkerRecord {
        // Every WorkerRecord field is an integer, boolean or nullable raw pointer.
        unsafe { core::mem::zeroed() }
    }
    fn pool() -> PoolRecord {
        // Same all-zero valid representation as the native shared pool allocation.
        unsafe { core::mem::zeroed() }
    }

    #[test]
    fn pid_lookup_preserves_event_order_and_task_then_user_precedence() {
        use crate::worker_registry::*;
        let mut events = [worker(), worker()];
        // Every view of `events` comes from this pointer: a borrow of the array would end it.
        let workers = events.as_mut_ptr();
        let mut task = worker();
        let mut user = worker();
        let mut event_pool = pool();
        let mut task_pool = pool();
        let registry = Box::into_raw(Box::<WorkerRegistry>::default());
        task_pool.registry = Some(Box::default());
        let tasks = task_pool.registry();
        let view = unsafe {
            ServerWorkers::new(
                (
                    PoolRef::new(&raw mut event_pool),
                    core::slice::from_raw_parts(workers, 2),
                    2,
                ),
                (PoolRef::new(&raw mut task_pool), 1),
                (core::slice::from_raw_parts(&raw mut user, 1), 1),
            )
        };
        // Event pool workers remain null before publication: PID lookup has
        // always scanned the Server array directly, including direct PID writes.
        unsafe { Worker::new(workers).set_pid(-7) };
        unsafe { Worker::new(workers.add(1)).set_pid(-7) };
        unsafe {
            (*tasks).register(-7, (&raw mut task).cast(), 1);
            (*registry).register(-7, (&raw mut user).cast(), CATEGORY_USER);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, Some(&*registry), -7),
                workers
            );
            Worker::new(workers).set_pid(5);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, Some(&*registry), -7),
                workers.add(1)
            );
            Worker::new(workers.add(1)).set_pid(0);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, Some(&*registry), 0),
                workers.add(1)
            );
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, Some(&*registry), -7),
                &raw mut task
            );
            (*tasks).unregister(-7);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, Some(&*registry), -7),
                &raw mut user
            );
            assert!(swr_v1_process_server_worker_by_pid(&view, Some(&*registry), 99).is_null());
            drop(Box::from_raw(registry));
        }
    }

    #[test]
    fn startup_publishes_arrays_and_lookup_preserves_aliases_and_categories() {
        static SERVER: PoolObjectKey = PoolObjectKey::new();
        let mut events = [worker(), worker()];
        let mut tasks = [worker()];
        let mut users = [worker(), worker(), worker()];
        // One pointer per array: each `as_mut_ptr` borrow would end the ones before.
        let (events_ptr, tasks_ptr, users_ptr) =
            (events.as_mut_ptr(), tasks.as_mut_ptr(), users.as_mut_ptr());
        let mut pending = [worker(), worker()];
        let pending_ptrs: [*mut WorkerRecord; 3] = [
            &raw mut pending[0],
            &raw mut pending[1],
            &raw mut pending[0],
        ];
        let mut event_pool = pool();
        let mut task_pool = pool();
        task_pool.workers = tasks_ptr;
        event_pool.reload_count = 91;
        task_pool.worker_num = 1;
        let view = unsafe {
            ServerWorkers::new(
                (
                    PoolRef::new(&raw mut event_pool),
                    core::slice::from_raw_parts(events_ptr, 2),
                    2,
                ),
                (PoolRef::new(&raw mut task_pool), 1),
                (core::slice::from_raw_parts(users_ptr, 3), 3),
            )
        };
        let mut server = 0_u8;
        swr_v1_process_server_workers_start(
            &view,
            (&SERVER, (&raw mut server).cast()),
            pending_ptrs.map(|worker| unsafe { Worker::new(worker) }),
        );
        assert_eq!(event_pool.workers, events_ptr);
        assert_eq!(event_pool.ptr, (&raw mut server).cast());
        assert_eq!(
            (
                event_pool.worker_num,
                event_pool.use_msgqueue,
                event_pool.reload_count
            ),
            (2, false, 91)
        );
        assert_eq!(
            unsafe { Worker::new(&raw const pending[0]).id() },
            5,
            "duplicate pointer keeps last configured ID"
        );
        assert_eq!(unsafe { Worker::new(&raw const pending[1]).id() }, 4);
        let expected = [
            events_ptr,
            events_ptr.wrapping_add(1),
            tasks_ptr,
            users_ptr,
            users_ptr.wrapping_add(1),
            users_ptr.wrapping_add(2),
        ];
        for (id, expected) in expected.into_iter().enumerate() {
            let found = swr_v1_process_server_worker(&view, u16::try_from(id).unwrap());
            assert_eq!(found, expected);
            unsafe {
                Worker::new(found).set_pid(123);
            }
            assert_eq!(
                unsafe { Worker::new(expected).pid() },
                123,
                "lookup lends canonical mutable records"
            );
        }
        assert!(swr_v1_process_server_worker(&view, 6).is_null());
        for (id, event) in events.iter().enumerate() {
            assert_eq!(
                unsafe { Worker::new(event).id() },
                u32::try_from(id).unwrap()
            );
            assert_eq!(
                unsafe { Worker::new(event).kind() },
                Some(crate::worker_record::WorkerType::Event)
            );
        }
    }
}
