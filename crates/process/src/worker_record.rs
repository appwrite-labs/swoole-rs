//! The server's `Worker` record, whose layout Rust owns (`#[repr(C)]`) so the
//! `sw_shm`-backed worker array every process maps is defined here, and the scalar
//! process fields (pid/status/type/id) move behind accessors one at a time. The C++
//! `swoole::Worker` keeps its methods and pointer/atomic fields; a `static_assert`
//! in the patch locks `sizeof`/`offsetof` to this layout, so a mismatch fails the
//! build rather than corrupting shared memory. See #629.

use std::ffi::{c_int, c_void};

/// Byte-exact mirror of `swoole::Worker` (`include/swoole_server.h`). Field order,
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
    pub shared: bool,
    pub redirect_stdout: bool,
    pub redirect_stdin: bool,
    pub redirect_stderr: bool,
    status: u8,
    kind: u8,
    msgqueue_mode: u8,
    pub child_process: u8,
    pub concurrency: u32,
    start_time: i64,
    pub dispatch_count: i64,
    pub request_count: i64,
    pub response_count: i64,
    pub coroutine_num: usize,
    lock: *mut c_void,
    pub pipe_object: *mut c_void,
    pub pipe_master: *mut c_void,
    pub pipe_worker: *mut c_void,
    pub pipe_current: *mut c_void,
    ptr: *mut c_void,
}

impl WorkerRecord {
    /// `new Worker()`: a zeroed record (no pipe, no queue, id 0, pid 0).
    #[must_use]
    pub fn boxed() -> Box<Self> {
        // SAFETY: every field is an integer, a bool or a raw pointer, for which
        // all-zero is a valid value.
        Box::new(unsafe { core::mem::zeroed() })
    }
}

/// First event worker with this pid, preserving array order and public raw writes.
/// # Safety
/// `workers` is null or lends `count` initialized canonical Worker records.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_array_lookup(
    workers: *mut WorkerRecord,
    count: u32,
    pid: c_int,
) -> *mut WorkerRecord {
    // SAFETY: the caller lends the complete initialized canonical array and its PID slots.
    unsafe {
        crate::worker_registry::swr_v1_process_worker_registry_find_array(
            workers.cast(),
            count,
            core::mem::size_of::<WorkerRecord>(),
            core::mem::offset_of!(WorkerRecord, pid),
            pid,
        )
    }
    .cast()
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

/// The worker's end of its pipe (a native socket), or null.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
pub(crate) unsafe fn pipe_worker(worker: *const WorkerRecord) -> *mut c_void {
    // SAFETY: the caller lends a live worker record; only the field is read.
    unsafe { (*worker).pipe_worker }
}

/// The worker's pool (a native `ProcessPool`).
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
pub(crate) unsafe fn pool_of(worker: *const WorkerRecord) -> *mut c_void {
    // SAFETY: the caller lends a live worker record; only the field is read.
    unsafe { (*worker).pool }
}

/// `Worker::ptr`: what its owner keeps there (a user worker's process object).
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
#[must_use]
pub unsafe fn user_data(worker: *const WorkerRecord) -> *mut c_void {
    // SAFETY: the caller lends a live worker record; only the field is read.
    unsafe { (*worker).ptr }
}

/// `Swoole\Process\Pool::getProcess()`'s worker: `copy` becomes a copy of the
/// pool's worker `id`, separated from shared memory. PHP may not close a pipe
/// it shares with the pool, nor touch one the message bus owns; without
/// `onMessage`, a message-queue pool's process pushes and pops the pool's
/// queue. Returns the copy's pipe fd, or -1 unless the pool talks over unix
/// sockets.
///
/// # Safety
/// `pool` is a created pool with a worker `id`; `copy` is a writable record.
pub unsafe fn copy_for_process(
    pool: *const crate::process_pool::PoolRecord,
    id: usize,
    copy: *mut WorkerRecord,
) -> c_int {
    const IPC_UNIXSOCK: u8 = 1;
    const IPC_MSGQUEUE: u8 = 2;
    const MSGQUEUE_BALANCE: u8 = 2;
    // SAFETY: the caller lends a created pool.
    let pool = unsafe { &*pool };
    // SAFETY: the pool's worker `id`, copied by value as upstream's `new Worker(...)` does.
    let mut worker = unsafe { pool.workers.wrapping_add(id).read() };
    let mut pipe = -1;
    if pool.ipc_mode == IPC_UNIXSOCK {
        worker.pipe_current = if worker.id == crate::methods::swoole_get_worker_id() {
            worker.pipe_worker
        } else {
            worker.pipe_master
        };
        worker.pipe_object = core::ptr::null_mut();
        // SAFETY: a unix-socket pool's worker pipes are live sockets, and
        // `network::Socket` begins with its fd.
        pipe = unsafe { worker.pipe_current.cast::<c_int>().read() };
    }
    if !pool.message_bus.is_null() {
        worker.pipe_current = core::ptr::null_mut();
        worker.pipe_object = core::ptr::null_mut();
    }
    if pool.ipc_mode == IPC_MSGQUEUE && pool.on_message.is_null() {
        worker.queue = pool.queue;
        worker.msgqueue_mode = MSGQUEUE_BALANCE;
    }
    // SAFETY: as the caller guarantees.
    unsafe { copy.write(worker) };
    pipe
}

/// The worker's message queue, or null.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
#[must_use]
pub unsafe fn queue(worker: *const WorkerRecord) -> *mut c_void {
    // SAFETY: the caller lends a live worker record; only the field is read.
    unsafe { (*worker).queue }
}

/// Give the worker a message queue, or null.
///
/// # Safety
/// `worker` points to a live, writable `Worker`/`WorkerRecord`.
pub unsafe fn set_queue(worker: *mut WorkerRecord, queue: *mut c_void) {
    // SAFETY: the caller lends a live, writable worker record; only the field is written.
    unsafe { (*worker).queue = queue };
}

/// The worker's `msgqueue_mode`.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
#[must_use]
pub unsafe fn msgqueue_mode(worker: *const WorkerRecord) -> u8 {
    // SAFETY: the caller lends a live worker record; only the field is read.
    unsafe { (*worker).msgqueue_mode }
}

/// Set the worker's `msgqueue_mode`.
///
/// # Safety
/// `worker` points to a live, writable `Worker`/`WorkerRecord`.
pub unsafe fn set_msgqueue_mode(worker: *mut WorkerRecord, mode: u8) {
    // SAFETY: the caller lends a live, writable worker record; only the field is written.
    unsafe { (*worker).msgqueue_mode = mode };
}

/// Point the worker at its pool.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
pub(crate) unsafe fn set_pool(worker: *mut WorkerRecord, pool: *mut c_void) {
    // SAFETY: the caller lends a live worker record; only the field is written.
    unsafe { (*worker).pool = pool };
}

/// `Server::stats()`'s worker counters: request, response and dispatch
/// counts, then the concurrency.
///
/// # Safety
/// `worker` points to a live `Worker`/`WorkerRecord`.
#[must_use]
pub unsafe fn counters(worker: *const WorkerRecord) -> [i64; 4] {
    // SAFETY: the caller lends a live worker record; only the field is read.
    let request = unsafe { (*worker).request_count };
    // SAFETY: as above.
    let response = unsafe { (*worker).response_count };
    // SAFETY: as above.
    let dispatch = unsafe { (*worker).dispatch_count };
    // SAFETY: as above.
    let concurrency = unsafe { (*worker).concurrency };
    [request, response, dispatch, i64::from(concurrency)]
}

/// Increments the worker's request counter with the original unsigned wrapping.
///
/// # Safety
/// The caller owns this worker's counter update, as in its worker event loop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_add_request_count(worker: *mut WorkerRecord) {
    // SAFETY: the worker loop owns the original non-atomic counter update.
    let count = unsafe { (*worker).request_count };
    // SAFETY: the same worker loop owns this write; wrapping preserves the native word bits.
    unsafe { (*worker).request_count = count.wrapping_add(1) };
}

/// `worker->response_count++`.
///
/// # Safety
/// The caller owns this worker's counter update, as in its worker event loop.
pub unsafe fn add_response_count(worker: *mut WorkerRecord) {
    // SAFETY: as the caller guarantees; only the field is read.
    let count = unsafe { (*worker).response_count };
    // SAFETY: the same worker loop owns this write.
    unsafe { (*worker).response_count = count.wrapping_add(1) };
}

/// Initializes the current worker's counters and local running state.
///
/// # Safety
/// `worker` is a writable initialized worker record owned by this worker loop.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker4initEv")
)]
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker8shutdownEv")
)]
pub unsafe extern "C" fn swr_v1_worker_shutdown(worker: *mut WorkerRecord) {
    // SAFETY: the caller owns the current worker's status transition.
    unsafe { swr_v1_worker_set_status(worker, 3) };
    crate::worker_globals::swr_v1_worker_global_set(4, 1);
}

/// Checks the current thread's request limit against the shared worker count.
///
/// # Safety
/// `worker` is a readable live worker record; its owning loop synchronizes the count.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Worker24has_exceeded_max_requestEv")
)]
#[must_use]
pub unsafe extern "C" fn swr_v1_worker_exceeded_max_requests(worker: *const WorkerRecord) -> bool {
    let limit = crate::worker_globals::swr_v1_worker_global_get(5).cast_unsigned();
    if limit == 0 {
        return false;
    }
    // SAFETY: the caller lends the loop's initialized counter; preserve native unsigned word bits.
    let count = unsafe { (*worker).request_count }.cast_unsigned();
    count >= limit
}

/// Reset one retired worker's counters and remove its concurrency from the
/// Server's canonical shared counter. The CAS and underflow clamp preserve the
/// existing reset protocol, without per-field native callbacks.
/// # Safety
/// `worker` loans a live canonical record under the native retirement protocol;
/// its concurrency and `total` are aligned atomic u32 slots. The caller owns the
/// three request/response/dispatch counter resets, as the native method did.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_worker_reset_counters(worker: *mut WorkerRecord, total: *mut u32) {
    use core::sync::atomic::{AtomicU32, Ordering};
    // SAFETY: the caller lends this aligned canonical worker concurrency slot.
    let slot = unsafe { &raw mut (*worker).concurrency };
    // SAFETY: concurrency is an aligned u32 accessed atomically by native workers.
    let concurrency = unsafe { AtomicU32::from_ptr(slot) };
    let value = concurrency.load(Ordering::SeqCst);
    if value > 0
        && concurrency
            .compare_exchange(value, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        // SAFETY: the caller lends the aligned live shared Server concurrency slot.
        let total = unsafe { AtomicU32::from_ptr(total) };
        total.fetch_sub(value, Ordering::SeqCst);
        if total.load(Ordering::SeqCst).cast_signed() < 0 {
            total.store(0, Ordering::SeqCst);
        }
    }
    // SAFETY: the retirement protocol owns this worker's request-counter reset.
    unsafe { (*worker).request_count = 0 };
    // SAFETY: the same protocol owns this response-counter reset.
    unsafe { (*worker).response_count = 0 };
    // SAFETY: the same protocol owns this dispatch-counter reset.
    unsafe { (*worker).dispatch_count = 0 };
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
    fn a_pool_process_copies_the_worker_without_the_pipes_it_may_not_close() {
        let mut fds = [7 as c_int, 9];
        let mut workers: [WorkerRecord; 2] = unsafe { core::mem::zeroed() };
        workers[1].id = u32::MAX;
        workers[1].pid = 41;
        workers[1].pipe_master = (&raw mut fds[0]).cast();
        workers[1].pipe_worker = (&raw mut fds[1]).cast();
        workers[1].pipe_object = core::ptr::dangling_mut();
        let mut pool: crate::process_pool::PoolRecord = unsafe { core::mem::zeroed() };
        pool.workers = workers.as_mut_ptr();
        pool.ipc_mode = 1;
        let mut copy: WorkerRecord = unsafe { core::mem::zeroed() };
        // Another process's worker: the master's end of its pipe.
        assert_eq!(
            unsafe { copy_for_process(&raw const pool, 1, &raw mut copy) },
            7
        );
        assert_eq!(
            (copy.pid, copy.pipe_current, copy.pipe_object),
            (41, workers[1].pipe_master, core::ptr::null_mut())
        );
        // The message bus owns the pipe.
        pool.message_bus = core::ptr::dangling_mut();
        assert_eq!(
            unsafe { copy_for_process(&raw const pool, 1, &raw mut copy) },
            7
        );
        assert!(copy.pipe_current.is_null());
        // Without onMessage, a message-queue pool's process balances over the pool's queue.
        pool.ipc_mode = 2;
        pool.queue = 3 as *mut c_void;
        assert_eq!(
            unsafe { copy_for_process(&raw const pool, 1, &raw mut copy) },
            -1
        );
        assert_eq!((copy.queue, copy.msgqueue_mode), (pool.queue, 2));
        pool.on_message = 4 as *const c_void;
        assert_eq!(
            unsafe { copy_for_process(&raw const pool, 1, &raw mut copy) },
            -1
        );
        assert!(copy.queue.is_null());
    }

    #[test]
    fn retirement_resets_counters_preserving_identity_and_clamps_underflow() {
        let mut record: WorkerRecord = unsafe { core::mem::zeroed() };
        record.pid = 17;
        record.id = 29;
        record.concurrency = 3;
        record.request_count = 7;
        record.response_count = 8;
        record.dispatch_count = 9;
        let mut total = 7;
        unsafe { swr_v1_worker_reset_counters(&raw mut record, &raw mut total) };
        assert_eq!(
            (record.pid, record.id, total, record.concurrency),
            (17, 29, 4, 0)
        );
        assert_eq!(
            (
                record.request_count,
                record.response_count,
                record.dispatch_count
            ),
            (0, 0, 0)
        );
        record.concurrency = 9;
        total = 3;
        unsafe { swr_v1_worker_reset_counters(&raw mut record, &raw mut total) };
        assert_eq!((record.concurrency, total), (0, 0));
        total = 13;
        unsafe { swr_v1_worker_reset_counters(&raw mut record, &raw mut total) };
        assert_eq!(total, 13);
    }

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

#[cfg(test)]
mod public_pid_array_tests {
    use super::*;
    #[test]
    fn public_pid_writes_and_duplicates_keep_first_array_match_including_zero() {
        let mut workers: [WorkerRecord; 3] = unsafe { core::mem::zeroed() };
        let first = workers.as_mut_ptr();
        unsafe {
            assert_eq!(swr_v1_worker_array_lookup(first, 3, 0), first);
            first.cast::<c_int>().write(9);
            first.add(1).cast::<c_int>().write(9);
            assert_eq!(swr_v1_worker_array_lookup(first, 3, 9), first);
            first.cast::<c_int>().write(4);
            assert_eq!(swr_v1_worker_array_lookup(first, 3, 9), first.add(1));
            assert_eq!(swr_v1_worker_array_lookup(first, 3, 0), first.add(2));
            assert!(swr_v1_worker_array_lookup(first, 3, 77).is_null());
            assert!(swr_v1_worker_array_lookup(core::ptr::null_mut(), 0, 0).is_null());
        }
    }
}
