//! The server's `Worker` record, whose layout Rust owns (`#[repr(C)]`) so the
//! `sw_shm`-backed worker array every process maps is defined here, and the scalar
//! process fields (pid/status/type/id) move behind accessors one at a time. The C++
//! `swoole::Worker` keeps its methods and pointer/atomic fields; a `static_assert`
//! in the patch locks `sizeof`/`offsetof` to this layout, so a mismatch fails the
//! build rather than corrupting shared memory. See #629.

use std::ffi::{c_int, c_void};

/// Byte-exact mirror of `swoole::Worker` (`native/process/swoole_rs_process_pool.h`). Field order,
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
    pub lock: *mut c_void,
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

    /// Makes the record `pool`'s worker `id`, of `kind`.
    pub fn place(&mut self, pool: *mut c_void, id: u32, kind: u8) {
        self.pool = pool;
        self.id = id;
        self.kind = kind;
    }

    /// The worker's id (its slot in the worker pool).
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The worker's kind (C++ `type`).
    #[must_use]
    pub fn kind(&self) -> u8 {
        self.kind
    }

    /// The worker's process (or thread) id.
    #[must_use]
    pub fn pid(&self) -> c_int {
        self.pid
    }

    /// Sets the worker's process (or thread) id.
    pub fn set_pid(&mut self, pid: c_int) {
        self.pid = pid;
    }

    /// `SW_WORKER_IDLE`, `SW_WORKER_BUSY`, ...
    pub fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    #[must_use]
    pub fn queue(&self) -> *mut c_void {
        self.queue
    }

    pub fn set_queue(&mut self, queue: *mut c_void) {
        self.queue = queue;
    }

    #[must_use]
    pub fn msgqueue_mode(&self) -> u8 {
        self.msgqueue_mode
    }

    pub fn set_msgqueue_mode(&mut self, mode: u8) {
        self.msgqueue_mode = mode;
    }

    pub fn set_status(&mut self, status: u8) {
        self.status = status;
    }

    /// `worker->ptr = data`: what its owner keeps there.
    pub fn set_user_data(&mut self, data: *mut c_void) {
        self.ptr = data;
    }

    /// `worker->request_count++`, with the native unsigned wrapping.
    pub fn add_request_count(&mut self) {
        self.request_count = self.request_count.wrapping_add(1);
    }

    /// `has_exceeded_max_request()`: this thread's request limit against the count.
    #[must_use]
    pub fn exceeded_max_request(&self) -> bool {
        let limit = crate::worker_globals::swr_v1_worker_global_get(5).cast_unsigned();
        limit != 0 && self.request_count.cast_unsigned() >= limit
    }
}

/// First event worker with this pid, preserving array order and public raw writes.
/// # Safety
/// `workers` is null or lends `count` initialized canonical Worker records.
pub unsafe fn swr_v1_worker_array_lookup(
    workers: *mut WorkerRecord,
    count: u32,
    pid: c_int,
) -> *mut WorkerRecord {
    // SAFETY: the caller lends the complete initialized canonical array and its PID slots.
    unsafe {
        crate::worker_registry::find_array(
            workers.cast(),
            count,
            core::mem::size_of::<WorkerRecord>(),
            core::mem::offset_of!(WorkerRecord, pid),
            pid,
        )
    }
    .cast()
}

/// A live worker record: the only way to reach one's fields. Each read and write
/// goes through the pointer to that one field and makes no reference to the whole
/// record, which other processes write in shared memory. Native code passes one
/// as a `Worker *`.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Worker<'a> {
    raw: *mut WorkerRecord,
    life: core::marker::PhantomData<&'a WorkerRecord>,
}

impl<'a> From<&'a mut WorkerRecord> for Worker<'a> {
    /// The record a unique borrow keeps live for `'a`.
    fn from(record: &'a mut WorkerRecord) -> Self {
        Self {
            raw: record,
            life: core::marker::PhantomData,
        }
    }
}

macro_rules! field {
    ($(#[$doc:meta])* $get:ident, $set:ident, $field:ident: $ty:ty) => {
        $(#[$doc])*
        #[must_use]
        pub fn $get(self) -> $ty {
            // SAFETY: `new`'s contract: a live record; only the field is read.
            unsafe { (*self.raw).$field }
        }

        $(#[$doc])*
        pub fn $set(self, value: $ty) {
            // SAFETY: `new`'s contract: a live record whose owner orders this write;
            // only the field is written.
            unsafe { (*self.raw).$field = value };
        }
    };
}

impl Worker<'_> {
    /// The record at `raw`.
    ///
    /// # Safety
    /// `raw` points to a live `Worker`/`WorkerRecord` for `'a`, and the process
    /// protocol that owns it orders the field writes made through the handle
    /// (a worker loop owns its counters; the master owns pid and status).
    #[must_use]
    pub unsafe fn new(raw: *const WorkerRecord) -> Self {
        Self {
            raw: raw.cast_mut(),
            life: core::marker::PhantomData,
        }
    }

    /// The record at `raw`, or `None` when it is null.
    ///
    /// # Safety
    /// As [`Worker::new`], when `raw` is not null.
    #[must_use]
    pub unsafe fn from_raw(raw: *const WorkerRecord) -> Option<Self> {
        // SAFETY: as the caller guarantees.
        (!raw.is_null()).then(|| unsafe { Self::new(raw) })
    }

    /// The record's address.
    #[must_use]
    pub fn raw(self) -> *mut WorkerRecord {
        self.raw
    }

    field!(
        /// The worker's status (`SW_WORKER_IDLE`/`BUSY`/…).
        status, set_status, status: u8
    );
    field!(
        /// The worker's kind (C++ `type`: `SW_EVENT_WORKER`/`SW_TASK_WORKER`/…).
        kind, set_kind, kind: u8
    );
    field!(
        /// The worker's id (its slot in the worker pool, `WorkerId`).
        id, set_id, id: u32
    );
    field!(
        /// The worker's process (or thread) id.
        pid, set_pid, pid: c_int
    );
    field!(
        /// The worker's message queue, or null.
        queue, set_queue, queue: *mut c_void
    );
    field!(
        /// The worker's `msgqueue_mode`.
        msgqueue_mode, set_msgqueue_mode, msgqueue_mode: u8
    );
    field!(
        /// The worker's pool (a native `ProcessPool`).
        pool, set_pool, pool: *mut c_void
    );
    field!(
        /// `Worker::ptr`: what its owner keeps there (a user worker's process object).
        user_data, set_user_data, ptr: *mut c_void
    );
    field!(
        /// The master's end of the worker's pipe (a native socket), or null.
        pipe_master, set_pipe_master, pipe_master: *mut c_void
    );
    field!(
        /// The worker's end of its pipe (a native socket), or null.
        pipe_worker, set_pipe_worker, pipe_worker: *mut c_void
    );
    field!(
        /// `worker->request_count`.
        request_count, set_request_count, request_count: i64
    );
    field!(
        /// `worker->response_count`.
        response_count, set_response_count, response_count: i64
    );
    field!(
        /// `worker->dispatch_count`.
        dispatch_count, set_dispatch_count, dispatch_count: i64
    );
    field!(
        /// `worker->start_time`.
        start_time, set_start_time, start_time: i64
    );

    /// `Server::stats()`'s worker counters: request, response and dispatch
    /// counts, then the concurrency.
    #[must_use]
    pub fn counters(self) -> [i64; 4] {
        // SAFETY: `new`'s contract: a live record; only the field is read.
        let concurrency = unsafe { (*self.raw).concurrency };
        [
            self.request_count(),
            self.response_count(),
            self.dispatch_count(),
            i64::from(concurrency),
        ]
    }

    /// `worker->request_count++`, with the native unsigned wrapping.
    pub fn add_request_count(self) {
        self.set_request_count(self.request_count().wrapping_add(1));
    }

    /// `worker->response_count++`, with the native unsigned wrapping.
    pub fn add_response_count(self) {
        self.set_response_count(self.response_count().wrapping_add(1));
    }

    /// `Worker::init()`: starts the current worker's clock, request count and
    /// running state.
    pub fn init(self) {
        self.set_start_time(swoole_rs_corebase::time_now());
        self.set_request_count(0);
        self.set_status(2);
        crate::worker_globals::swr_v1_worker_global_set(3, 1);
        crate::worker_globals::swr_v1_worker_global_set(4, 0);
    }

    /// `Worker::shutdown()`: marks the current worker exited.
    pub fn shutdown(self) {
        self.set_status(3);
        crate::worker_globals::swr_v1_worker_global_set(4, 1);
    }

    /// `has_exceeded_max_request()`: this thread's request limit against the count.
    #[must_use]
    pub fn exceeded_max_request(self) -> bool {
        let limit = crate::worker_globals::swr_v1_worker_global_get(5).cast_unsigned();
        limit != 0 && self.request_count().cast_unsigned() >= limit
    }

    /// Resets one retired worker's counters and removes its concurrency from the
    /// server's shared `total`. The CAS and underflow clamp keep the native reset
    /// protocol.
    pub fn reset_counters(self, total: &core::sync::atomic::AtomicU32) {
        use core::sync::atomic::{AtomicU32, Ordering};
        let slot = self
            .raw
            .wrapping_byte_add(core::mem::offset_of!(WorkerRecord, concurrency))
            .cast::<u32>();
        // SAFETY: `new`'s contract: a live record whose concurrency is an aligned
        // u32 that native workers access atomically.
        let concurrency = unsafe { AtomicU32::from_ptr(slot) };
        let value = concurrency.load(Ordering::SeqCst);
        if value > 0
            && concurrency
                .compare_exchange(value, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            total.fetch_sub(value, Ordering::SeqCst);
            if total.load(Ordering::SeqCst).cast_signed() < 0 {
                total.store(0, Ordering::SeqCst);
            }
        }
        self.set_request_count(0);
        self.set_response_count(0);
        self.set_dispatch_count(0);
    }
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

/// `Worker::init()` for the native symbol.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker4initEv")
)]
pub extern "C" fn swr_v1_worker_init(worker: Worker<'_>) {
    worker.init();
}

/// `Worker::shutdown()` for the native symbol.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Worker8shutdownEv")
)]
pub extern "C" fn swr_v1_worker_shutdown(worker: Worker<'_>) {
    worker.shutdown();
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
        let total = core::sync::atomic::AtomicU32::new(7);
        unsafe { Worker::new(&raw mut record) }.reset_counters(&total);
        assert_eq!(
            (
                record.pid,
                record.id,
                total.load(core::sync::atomic::Ordering::SeqCst),
                record.concurrency
            ),
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
        total.store(3, core::sync::atomic::Ordering::SeqCst);
        unsafe { Worker::new(&raw mut record) }.reset_counters(&total);
        assert_eq!(
            (
                record.concurrency,
                total.load(core::sync::atomic::Ordering::SeqCst)
            ),
            (0, 0)
        );
        total.store(13, core::sync::atomic::Ordering::SeqCst);
        unsafe { Worker::new(&raw mut record) }.reset_counters(&total);
        assert_eq!(total.load(core::sync::atomic::Ordering::SeqCst), 13);
    }

    #[test]
    fn record_is_pointer_aligned() {
        assert_eq!(align_of::<WorkerRecord>(), 8);
    }

    #[test]
    fn status_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_status(2);
            assert_eq!(Worker::new(&raw const record).status(), 2);
        }
    }

    #[test]
    fn type_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_kind(3);
            assert_eq!(Worker::new(&raw const record).kind(), 3);
            // independent of status
            Worker::new(&raw mut record).set_status(1);
            assert_eq!(Worker::new(&raw const record).kind(), 3);
        }
    }

    #[test]
    fn pid_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_pid(4242);
            assert_eq!(Worker::new(&raw const record).pid(), 4242);
            // independent of the one-byte fields that follow it
            Worker::new(&raw mut record).set_kind(2);
            Worker::new(&raw mut record).set_status(1);
            assert_eq!(Worker::new(&raw const record).pid(), 4242);
        }
    }

    #[test]
    fn id_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_id(7);
            Worker::new(&raw mut record).set_pid(4242);
            assert_eq!(Worker::new(&raw const record).id(), 7);
            // id and pid are adjacent words; neither disturbs the other
            assert_eq!(Worker::new(&raw const record).pid(), 4242);
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
