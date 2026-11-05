//! Server startup's worker-array publication and ordered lookup. Native calls
//! enter here between the daemon/fork and factory/user callbacks, so no Rust
//! frame spans a fork hook, PHP bailout or whole-thread exit.
use crate::process_pool::PoolRecord;
use crate::worker_record::{WorkerRecord, set_pool, swr_v1_worker_set_id, swr_v1_worker_set_type};
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_server_workers_start(
    view: *const ServerWorkers,
    server: *mut c_void,
    pending: *const *mut WorkerRecord,
) {
    // SAFETY: the native startup call loans the view for this leaf operation.
    let view = unsafe { &*view };
    let pool = view.event_pool;
    // SAFETY: startup exclusively owns these pool fields before factory start.
    unsafe { (*pool).ptr = server };
    // SAFETY: the canonical event array is live for the pool's worker count.
    unsafe { (*pool).workers = view.event_workers };
    // SAFETY: the caller owns the pool's unpublished worker count.
    unsafe { (*pool).worker_num = view.event_count };
    // SAFETY: this original event pool uses its pipes rather than a message queue.
    unsafe { (*pool).use_msgqueue = 0 };
    for index in 0..view.event_count {
        let worker = view.event_workers.wrapping_add(index as usize);
        // SAFETY: the index is inside the live canonical event array.
        unsafe { set_pool(worker, pool.cast()) };
        // SAFETY: startup owns this worker's identity before factory publication.
        unsafe { swr_v1_worker_set_id(worker, index) };
        // SAFETY: same exclusively owned event record, SW_WORKER is 2.
        unsafe { swr_v1_worker_set_type(worker, 2) };
    }
    let first_user = view.event_count.wrapping_add(view.task_count);
    for index in 0..view.user_count {
        // SAFETY: the pending configuration list loans this live worker pointer.
        let worker = unsafe { pending.wrapping_add(index).read() };
        let index = u32::try_from(index & u32::MAX as usize).unwrap_or_default();
        // SAFETY: startup owns the pending user worker identity; preserve u32 wrap.
        unsafe { swr_v1_worker_set_id(worker, first_user.wrapping_add(index)) };
    }
}

/// Select a worker from the event, task or user category in its original order.
///
/// # Safety
/// `view` loans live pools and arrays with the stated counts. The returned
/// pointer remains a loan of the selected array, without extending its lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_server_worker(
    view: *const ServerWorkers,
    id: u16,
) -> *mut WorkerRecord {
    // SAFETY: this leaf call loans the view and its canonical arrays.
    let view = unsafe { &*view };
    let id = u32::from(id);
    if id < view.event_count {
        // SAFETY: the event pool is live and its workers have been published.
        let workers = unsafe { (*view.event_pool).workers };
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

/// Initialize worker resources, preserving the original copy-before-create order
/// for user workers. Resource constructors stay native leaves; any C++ exception
/// is captured there and rethrown only after this operation returns.
/// # Safety
/// `view` and `pending` loan the initialized arrays described by the view. For user
/// creation the destination has `user_count` writable records, disjoint from
/// pending originals. `create` initializes one worker and returns false after
/// capturing an exception, without unwinding, bailing out or exiting the thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_server_workers_create(
    view: *const ServerWorkers,
    pending: *const *mut WorkerRecord,
    users: bool,
    context: *mut c_void,
    create: unsafe extern "C" fn(*mut c_void, *mut WorkerRecord) -> bool,
) -> bool {
    // SAFETY: the native creator lends this view for the whole operation.
    let view = unsafe { &*view };
    let count = if users {
        view.user_count
    } else {
        view.event_count as usize
    };
    for index in 0..count {
        let worker = if users {
            // SAFETY: the immutable pending list has user_count live worker pointers.
            let original = unsafe { pending.wrapping_add(index).read() };
            // SAFETY: each original and its distinct shared destination are valid
            // initialized WorkerRecord storage; this is the original byte copy.
            unsafe {
                core::ptr::copy_nonoverlapping(original, view.user_workers.wrapping_add(index), 1);
            };
            original
        } else {
            // SAFETY: the event pool's initialized array has event_count records.
            let workers = unsafe { (*view.event_pool).workers };
            workers.wrapping_add(index)
        };
        // SAFETY: the native leaf catches C++ exceptions and returns without
        // crossing this Rust frame; this worker remains live for the operation.
        if !unsafe { create(context, worker) } {
            return false;
        }
        if users {
            // SAFETY: resource creation succeeded, and the original user record
            // remains exclusively loaned. Preserve SW_USER_WORKER after copying.
            unsafe { swr_v1_worker_set_type(worker, 5) };
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_record::{
        swr_v1_worker_id, swr_v1_worker_pid, swr_v1_worker_set_pid, swr_v1_worker_type,
    };

    fn worker() -> WorkerRecord {
        // Every WorkerRecord field is an integer, boolean or nullable raw pointer.
        unsafe { core::mem::zeroed() }
    }
    fn pool() -> PoolRecord {
        // Same all-zero valid representation as the native shared pool allocation.
        unsafe { core::mem::zeroed() }
    }

    unsafe extern "C" fn create(context: *mut c_void, worker: *mut WorkerRecord) -> bool {
        let calls = unsafe { &mut *context.cast::<usize>() };
        *calls += 1;
        unsafe {
            swr_v1_worker_set_pid(worker, 99);
        }
        *calls != 2
    }

    #[test]
    fn startup_publishes_arrays_and_lookup_preserves_aliases_and_categories() {
        let mut events = [worker(), worker()];
        let mut tasks = [worker()];
        let mut users = [worker(), worker(), worker()];
        let mut pending = [worker(), worker()];
        let pending_ptrs = [
            &raw mut pending[0],
            &raw mut pending[1],
            &raw mut pending[0],
        ];
        let mut event_pool = pool();
        let mut task_pool = pool();
        task_pool.workers = tasks.as_mut_ptr();
        event_pool.reload_count = 91;
        let view = ServerWorkers {
            event_pool: &raw mut event_pool,
            task_pool: &raw mut task_pool,
            event_workers: events.as_mut_ptr(),
            user_workers: users.as_mut_ptr(),
            event_count: 2,
            task_count: 1,
            user_count: 3,
        };
        let mut server = 0_u8;
        unsafe {
            swr_v1_process_server_workers_start(
                &raw const view,
                (&raw mut server).cast(),
                pending_ptrs.as_ptr(),
            );
        }
        assert_eq!(event_pool.workers, events.as_mut_ptr());
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
            unsafe { swr_v1_worker_id(&raw const pending[0]) },
            5,
            "duplicate pointer keeps last configured ID"
        );
        assert_eq!(unsafe { swr_v1_worker_id(&raw const pending[1]) }, 4);
        let expected = [
            events.as_mut_ptr(),
            events.as_mut_ptr().wrapping_add(1),
            tasks.as_mut_ptr(),
            users.as_mut_ptr(),
            users.as_mut_ptr().wrapping_add(1),
            users.as_mut_ptr().wrapping_add(2),
        ];
        for (id, expected) in expected.into_iter().enumerate() {
            let found = unsafe {
                swr_v1_process_server_worker(&raw const view, u16::try_from(id).unwrap())
            };
            assert_eq!(found, expected);
            unsafe {
                swr_v1_worker_set_pid(found, 123);
            }
            assert_eq!(
                unsafe { swr_v1_worker_pid(expected) },
                123,
                "lookup lends canonical mutable records"
            );
        }
        assert!(unsafe { swr_v1_process_server_worker(&raw const view, 6) }.is_null());
        for (id, event) in events.iter().enumerate() {
            assert_eq!(
                unsafe { swr_v1_worker_id(event) },
                u32::try_from(id).unwrap()
            );
            assert_eq!(unsafe { swr_v1_worker_type(event) }, 2);
        }
    }

    #[test]
    fn resource_failure_preserves_copy_before_create_and_stops_at_the_failed_worker() {
        let mut pending = [worker(), worker(), worker()];
        let mut shared = [worker(), worker(), worker()];
        for (index, record) in pending.iter_mut().enumerate() {
            unsafe {
                swr_v1_worker_set_pid(record, i32::try_from(index).unwrap() + 10);
            }
            unsafe {
                swr_v1_worker_set_type(record, 7);
            }
        }
        let pointers = [
            &raw mut pending[0],
            &raw mut pending[1],
            &raw mut pending[2],
        ];
        let view = ServerWorkers {
            event_pool: core::ptr::null_mut(),
            task_pool: core::ptr::null_mut(),
            event_workers: core::ptr::null_mut(),
            user_workers: shared.as_mut_ptr(),
            event_count: 0,
            task_count: 0,
            user_count: 3,
        };
        let mut calls = 0_usize;
        assert!(!unsafe {
            swr_v1_process_server_workers_create(
                &raw const view,
                pointers.as_ptr(),
                true,
                (&raw mut calls).cast(),
                create,
            )
        });
        assert_eq!(calls, 2);
        assert_eq!(unsafe { swr_v1_worker_pid(&raw const shared[0]) }, 10);
        assert_eq!(
            unsafe { swr_v1_worker_type(&raw const shared[0]) },
            7,
            "shared copy precedes original type update"
        );
        assert_eq!(unsafe { swr_v1_worker_type(&raw const pending[0]) }, 5);
        assert_eq!(
            unsafe { swr_v1_worker_pid(&raw const shared[1]) },
            11,
            "failed worker was copied before constructor attempt"
        );
        assert_eq!(
            unsafe { swr_v1_worker_type(&raw const pending[1]) },
            7,
            "failed creation must not publish user type"
        );
        assert_eq!(
            unsafe { swr_v1_worker_pid(&raw const shared[2]) },
            0,
            "no later worker copy after failure"
        );
        assert_eq!(unsafe { swr_v1_worker_pid(&raw const pending[2]) }, 12);
    }
}
