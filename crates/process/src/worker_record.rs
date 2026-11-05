//! The server's `Worker` record, whose layout Rust owns (`#[repr(C)]`) so the
//! `sw_shm`-backed worker array every process maps is defined here, and the scalar
//! process fields (pid/status/type/id) move behind accessors one at a time. The C++
//! `swoole::Worker` keeps its methods and pointer/atomic fields; a `static_assert`
//! in the patch locks `sizeof`/`offsetof` to this layout, so a mismatch fails the
//! build rather than corrupting shared memory. See #629.

use std::ffi::{c_int, c_void};

/// Byte-exact mirror of `swoole::Worker` (`include/swoole_process_pool.h`). Field order,
/// types and padding match the pinned C++ struct; the pointer and atomic fields are
/// opaque storage here (owned by C++ for now). Only the scalar process fields are
/// migrated behind the accessors below.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct WorkerRecord {
    pid: c_int,
    id: u32,
    pool: *mut c_void,
    queue: *mut c_void,
    shared: bool,
    redirect_stdout: bool,
    redirect_stdin: bool,
    redirect_stderr: bool,
    status: u8,
    kind: u8,
    msgqueue_mode: u8,
    child_process: u8,
    concurrency: u32,
    start_time: i64,
    dispatch_count: i64,
    request_count: i64,
    response_count: i64,
    coroutine_num: usize,
    lock: *mut c_void,
    pipe_object: *mut c_void,
    pipe_master: *mut c_void,
    pipe_worker: *mut c_void,
    pipe_current: *mut c_void,
    ptr: *mut c_void,
}

/// The worker's status (`SW_WORKER_IDLE`/`BUSY`/…).
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord` in its backing memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_status(worker: *const WorkerRecord) -> u8 {
    // SAFETY: the caller lends a live worker record. Reading the field through the
    // pointer makes no reference to the whole record, which other processes write
    // in shared memory.
    unsafe { (*worker).status }
}

/// Set the worker's status.
///
/// # Safety
/// `worker` points to a live, writable `Worker`/`WorkerRecord`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_set_status(worker: *mut WorkerRecord, status: u8) {
    // SAFETY: the caller lends a live, writable worker record; as above, only the
    // field is written, with no reference to the whole record.
    unsafe { (*worker).status = status };
}

/// The worker's kind (C++ `type`: `SW_EVENT_WORKER`/`SW_TASK_WORKER`/`SW_USER_WORKER`/…).
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord` in its backing memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_type(worker: *const WorkerRecord) -> u8 {
    // SAFETY: the caller lends a live worker record; only the field is read, with no
    // reference to the whole record (other processes write it in shared memory).
    unsafe { (*worker).kind }
}

/// Set the worker's kind (C++ `type`).
///
/// # Safety
/// `worker` points to a live, writable `Worker`/`WorkerRecord`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_set_type(worker: *mut WorkerRecord, kind: u8) {
    // SAFETY: the caller lends a live, writable worker record; only the field is written.
    unsafe { (*worker).kind = kind };
}

/// The worker's id (its slot in the worker pool, `WorkerId`).
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord` in its backing memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_id(worker: *const WorkerRecord) -> u32 {
    // SAFETY: the caller lends a live worker record; only the field is read, with no
    // reference to the whole record (other processes write it in shared memory).
    unsafe { (*worker).id }
}

/// Set the worker's id.
///
/// # Safety
/// `worker` points to a live, writable `Worker`/`WorkerRecord`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_set_id(worker: *mut WorkerRecord, id: u32) {
    // SAFETY: the caller lends a live, writable worker record; only the field is written.
    unsafe { (*worker).id = id };
}

/// The worker's process id.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord` in its backing memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_pid(worker: *const WorkerRecord) -> c_int {
    // SAFETY: the caller lends a live worker record; only the field is read, with no
    // reference to the whole record (other processes write it in shared memory).
    unsafe { (*worker).pid }
}

/// Set the worker's process id.
///
/// # Safety
/// `worker` points to a live, writable `Worker`/`WorkerRecord`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_set_pid(worker: *mut WorkerRecord, pid: c_int) {
    // SAFETY: the caller lends a live, writable worker record; only the field is written.
    unsafe { (*worker).pid = pid };
}

/// The master's end of the worker's pipe (a native socket), or null.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
pub(crate) unsafe fn pipe_master(worker: *const WorkerRecord) -> *mut c_void {
    // SAFETY: the caller lends a live worker record; only the field is read.
    unsafe { (*worker).pipe_master }
}

/// Point the worker at its pool.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
pub(crate) unsafe fn set_pool(worker: *mut WorkerRecord, pool: *mut c_void) {
    // SAFETY: the caller lends a live worker record; only the field is written.
    unsafe { (*worker).pool = pool };
}

/// Increments the worker's request counter with the original unsigned wrapping.
///
/// # Safety
/// The caller owns this worker's counter update, as in its worker event loop.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_worker_add_request_count(worker: *mut WorkerRecord) {
    // SAFETY: the worker loop owns the original non-atomic counter update.
    let count = unsafe { (*worker).request_count };
    // SAFETY: the same worker loop owns this write; wrapping preserves the native word bits.
    unsafe { (*worker).request_count = count.wrapping_add(1) };
}

/// Initializes the current worker's counters and local running state.
///
/// # Safety
/// `worker` is a writable initialized worker record owned by this worker loop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_init(worker: *mut WorkerRecord) {
    // SAFETY: time's null output argument requests the current native timestamp.
    let now = unsafe { libc::time(core::ptr::null_mut()) };
    // SAFETY: worker initialization owns its start timestamp update.
    unsafe { (*worker).start_time = now };
    // SAFETY: worker initialization owns its request counter reset.
    unsafe { (*worker).request_count = 0 };
    // SAFETY: the caller owns the worker's lifecycle/status transition.
    unsafe { swr_v1_worker_set_status(worker, 2) };
    crate::worker_globals::swr_v1_worker_global_set(3, 1);
    crate::worker_globals::swr_v1_worker_global_set(4, 0);
}

/// Marks the current worker exited and publishes its local shutdown state.
///
/// # Safety
/// `worker` is the writable current worker record for this thread/process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_shutdown(worker: *mut WorkerRecord) {
    // SAFETY: the caller owns the current worker's status transition.
    unsafe { swr_v1_worker_set_status(worker, 3) };
    crate::worker_globals::swr_v1_worker_global_set(4, 1);
}

/// Checks the current thread's request limit against the shared worker count.
///
/// # Safety
/// `worker` is a readable live worker record; its owning loop synchronizes the count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_exceeded_max_requests(worker: *const WorkerRecord) -> bool {
    let limit = crate::worker_globals::swr_v1_worker_global_get(5).cast_unsigned();
    if limit == 0 {
        return false;
    }
    // SAFETY: the caller lends the loop's initialized counter; preserve native unsigned word bits.
    let count = unsafe { (*worker).request_count }.cast_unsigned();
    count >= limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::align_of;

    // The exact offsets/size are locked C++-side: the patch's `static_assert`s compare
    // `swoole::Worker` against the `swr_v1_worker_record` mirror in swoole_rs_process.h,
    // which has these same `#[repr(C)]` fields. Here just sanity-check the Rust record's
    // alignment and that the accessors read/write the right field.
    #[test]
    fn record_is_pointer_aligned() {
        assert_eq!(align_of::<WorkerRecord>(), 8);
    }

    #[test]
    fn status_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            swr_v1_worker_set_status(&raw mut record, 2);
            assert_eq!(swr_v1_worker_status(&raw const record), 2);
        }
    }

    #[test]
    fn type_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            swr_v1_worker_set_type(&raw mut record, 3);
            assert_eq!(swr_v1_worker_type(&raw const record), 3);
            // independent of status
            swr_v1_worker_set_status(&raw mut record, 1);
            assert_eq!(swr_v1_worker_type(&raw const record), 3);
        }
    }

    #[test]
    fn pid_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            swr_v1_worker_set_pid(&raw mut record, 4242);
            assert_eq!(swr_v1_worker_pid(&raw const record), 4242);
            // independent of the one-byte fields that follow it
            swr_v1_worker_set_type(&raw mut record, 2);
            swr_v1_worker_set_status(&raw mut record, 1);
            assert_eq!(swr_v1_worker_pid(&raw const record), 4242);
        }
    }

    #[test]
    fn id_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            swr_v1_worker_set_id(&raw mut record, 7);
            swr_v1_worker_set_pid(&raw mut record, 4242);
            assert_eq!(swr_v1_worker_id(&raw const record), 7);
            // id and pid are adjacent words; neither disturbs the other
            assert_eq!(swr_v1_worker_pid(&raw const record), 4242);
        }
    }
}
