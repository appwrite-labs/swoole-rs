//! Server startup's worker-array publication and ordered lookup. Native calls
//! enter here between the daemon/fork and factory/user callbacks, so no Rust
//! frame spans a fork hook, PHP bailout or whole-thread exit.
use crate::process_pool::PoolRecord;
use crate::worker_record::Worker;
use crate::worker_record::WorkerRecord;
use core::ffi::c_void;

/// Loans of the existing canonical arrays. Event/task records belong to their
/// shared pools; user records belong to the Server's shared allocation.
#[repr(C)]
pub struct ServerWorkers {
    pub event_pool: *mut PoolRecord,
    pub task_pool: *mut PoolRecord,
    pub event_workers: *mut WorkerRecord,
    pub user_workers: *mut WorkerRecord,
    pub event_count: u32,
    pub task_count: u32,
    pub user_count: usize,
}

/// Publish the event pool and initialize worker identities before the factory
/// starts. Pending user pointers are the original ordered configuration list;
/// lookup still uses the distinct shared user array after creation.
///
/// # Safety
/// `view` loans initialized pool/array records exclusively during startup.
/// `pending` loans `user_count` live worker pointers. `server` is an opaque loan.
pub unsafe fn swr_v1_process_server_workers_start(
    view: &ServerWorkers,
    server: *mut c_void,
    pending: *const *mut WorkerRecord,
) {
    // SAFETY: startup exclusively owns the event pool before factory start.
    let pool = unsafe { &mut *view.event_pool };
    pool.ptr = server;
    // The canonical event array, live for the pool's worker count.
    pool.workers = view.event_workers;
    pool.worker_num = view.event_count;
    // This original event pool uses its pipes rather than a message queue.
    pool.use_msgqueue = 0;
    for index in 0..view.event_count {
        // The array just published holds `event_count` records.
        let Some(worker) = pool.worker(index as usize) else {
            break;
        };
        worker.set_pool(view.event_pool.cast());
        worker.set_id(index);
        worker.set_kind(2);
    }
    let first_user = view.event_count.wrapping_add(view.task_count);
    for index in 0..view.user_count {
        // SAFETY: the pending configuration list loans this live worker pointer.
        let worker = unsafe { pending.wrapping_add(index).read() };
        let index = u32::try_from(index & u32::MAX as usize).unwrap_or_default();
        // SAFETY: startup owns the pending user worker identity; preserve u32 wrap.
        unsafe { Worker::new(worker) }.set_id(first_user.wrapping_add(index));
    }
}

/// Select a worker from the event, task or user category in its original order.
///
/// # Safety
/// `view` loans live pools and arrays with the stated counts. The returned
/// pointer remains a loan of the selected array, without extending its lifetime.
#[must_use]
pub unsafe fn swr_v1_process_server_worker(view: &ServerWorkers, id: u16) -> *mut WorkerRecord {
    let id = u32::from(id);
    if id < view.event_count {
        // SAFETY: the event pool is live.
        let published = unsafe { (*view.event_pool).workers };
        // The pool holds the event array from `start`; before it, `workers[id]` is the
        // array's, which `get_idle_worker_id` reads after `create`.
        let workers = if published.is_null() {
            view.event_workers
        } else {
            published
        };
        return workers.wrapping_add(id as usize);
    }
    let user_start = view.event_count.wrapping_add(view.task_count);
    if id < user_start {
        // SAFETY: the task pool is live and its canonical array has task_count slots.
        let workers = unsafe { (*view.task_pool).workers };
        return workers.wrapping_add(id.wrapping_sub(view.event_count) as usize);
    }
    let users = u32::try_from(view.user_count & u32::MAX as usize).unwrap_or_default();
    if id < user_start.wrapping_add(users) {
        return view
            .user_workers
            .wrapping_add(id.wrapping_sub(user_start) as usize);
    }
    core::ptr::null_mut()
}

/// Ordered Server PID lookup: event records first, then the task pool's live
/// registry, then user entries in the Server registry. PID zero/negative values
/// and direct record writes retain their original lookup behavior.
/// # Safety
/// `view` loans live canonical arrays and task pool storage; `registry` is the
/// Server's live registry. Lookup is serialized with registry mutation.
#[must_use]
pub unsafe fn swr_v1_process_server_worker_by_pid(
    view: &ServerWorkers,
    registry: *const crate::worker_registry::WorkerRegistry,
    pid: core::ffi::c_int,
) -> *mut WorkerRecord {
    // SAFETY: these are the canonical ordered event records and their live count:
    // the first with this pid, in array order, reading the pid slots raw writes reach.
    let worker = unsafe {
        crate::worker_registry::find_array(
            view.event_workers.cast(),
            view.event_count,
            size_of::<WorkerRecord>(),
            core::mem::offset_of!(WorkerRecord, pid),
            pid,
        )
    }
    .cast::<WorkerRecord>();
    if !worker.is_null() {
        return worker;
    }
    // SAFETY: the caller loans the initialized task pool, which keeps its registry.
    let task_pool = unsafe { &*view.task_pool };
    let worker = task_pool
        .with_registry(|tasks| tasks.lookup(pid))
        .unwrap_or(core::ptr::null_mut());
    if !worker.is_null() {
        return worker.cast();
    }
    // SAFETY: the caller retains the Server registry, and only user category matches.
    unsafe { (*registry).lookup_category(pid, crate::worker_registry::CATEGORY_USER) }.cast()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let tasks = Box::into_raw(Box::<WorkerRegistry>::default());
        task_pool.registry = tasks;
        let view = ServerWorkers {
            event_pool: &raw mut event_pool,
            task_pool: &raw mut task_pool,
            event_workers: workers,
            user_workers: &raw mut user,
            event_count: 2,
            task_count: 1,
            user_count: 1,
        };
        // Event pool workers remain null before publication: PID lookup has
        // always scanned the Server array directly, including direct PID writes.
        unsafe { Worker::new(workers).set_pid(-7) };
        unsafe { Worker::new(workers.add(1)).set_pid(-7) };
        unsafe {
            (*tasks).register(-7, (&raw mut task).cast(), 1);
            (*registry).register(-7, (&raw mut user).cast(), CATEGORY_USER);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, registry, -7),
                workers
            );
            Worker::new(workers).set_pid(5);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, registry, -7),
                workers.add(1)
            );
            Worker::new(workers.add(1)).set_pid(0);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, registry, 0),
                workers.add(1)
            );
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, registry, -7),
                &raw mut task
            );
            (*tasks).unregister(-7);
            assert_eq!(
                swr_v1_process_server_worker_by_pid(&view, registry, -7),
                &raw mut user
            );
            assert!(swr_v1_process_server_worker_by_pid(&view, registry, 99).is_null());
            drop(Box::from_raw(tasks));
            drop(Box::from_raw(registry));
        }
    }

    #[test]
    fn startup_publishes_arrays_and_lookup_preserves_aliases_and_categories() {
        let mut events = [worker(), worker()];
        let mut tasks = [worker()];
        let mut users = [worker(), worker(), worker()];
        // One pointer per array: each `as_mut_ptr` borrow would end the ones before.
        let (events_ptr, tasks_ptr, users_ptr) =
            (events.as_mut_ptr(), tasks.as_mut_ptr(), users.as_mut_ptr());
        let mut pending = [worker(), worker()];
        let pending_ptrs = [
            &raw mut pending[0],
            &raw mut pending[1],
            &raw mut pending[0],
        ];
        let mut event_pool = pool();
        let mut task_pool = pool();
        task_pool.workers = tasks_ptr;
        event_pool.reload_count = 91;
        let view = ServerWorkers {
            event_pool: &raw mut event_pool,
            task_pool: &raw mut task_pool,
            event_workers: events_ptr,
            user_workers: users_ptr,
            event_count: 2,
            task_count: 1,
            user_count: 3,
        };
        let mut server = 0_u8;
        unsafe {
            swr_v1_process_server_workers_start(
                &view,
                (&raw mut server).cast(),
                pending_ptrs.as_ptr(),
            );
        }
        assert_eq!(event_pool.workers, events_ptr);
        assert_eq!(event_pool.ptr, (&raw mut server).cast());
        assert_eq!(
            (
                event_pool.worker_num,
                event_pool.use_msgqueue,
                event_pool.reload_count
            ),
            (2, 0, 91)
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
            let found = unsafe { swr_v1_process_server_worker(&view, u16::try_from(id).unwrap()) };
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
        assert!(unsafe { swr_v1_process_server_worker(&view, 6) }.is_null());
        for (id, event) in events.iter().enumerate() {
            assert_eq!(
                unsafe { Worker::new(event).id() },
                u32::try_from(id).unwrap()
            );
            assert_eq!(unsafe { Worker::new(event).kind() }, 2);
        }
    }
}
