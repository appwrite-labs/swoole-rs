//! The server's `Worker` record, whose layout Rust owns (`#[repr(C)]`) so the
//! `sw_shm`-backed worker array every process maps is defined here. The C++
//! `swoole::Worker` the native test fixtures compile takes its fields from this
//! struct: `tools/process_records.py` writes them into
//! `native/process/swoole_rs_process_views.h`, and `tools/headers.sh --check`
//! fails when that header is stale. See #629.

use crate::process_pool::IpcMode;
use core::sync::atomic::{AtomicI64, AtomicU8, AtomicU32, Ordering};
use std::ffi::{c_int, c_void};
use swoole_rs_locks::SharedMutex;

/// `swWorkerStatus`. A new record's status byte is 0, which is none of them.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerStatus {
    Busy = 1,
    Idle = 2,
    Exit = 3,
}

impl WorkerStatus {
    /// The status a status byte holds, `None` for a new record's 0.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Busy),
            2 => Some(Self::Idle),
            3 => Some(Self::Exit),
            _ => None,
        }
    }

    /// `SW_WORKER_BUSY` while `busy`, else `SW_WORKER_IDLE`.
    #[must_use]
    pub const fn busy(busy: bool) -> Self {
        if busy { Self::Busy } else { Self::Idle }
    }
}

/// `swWorkerType`: what the current process or thread is
/// ([`crate::methods::worker_type`]); a worker record's kind holds its code.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerType {
    Master = 1,
    /// `SW_WORKER` and `SW_EVENT_WORKER`.
    Event = 2,
    Manager = 3,
    Task = 4,
    User = 5,
}

impl WorkerType {
    /// The type `code` names, `None` for any other.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            1 => Some(Self::Master),
            2 => Some(Self::Event),
            3 => Some(Self::Manager),
            4 => Some(Self::Task),
            5 => Some(Self::User),
            _ => None,
        }
    }

    /// The C++ enum's value.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// `swoole::Worker`'s fields, in upstream's order (`SWR_WORKER_RECORD_FIELDS`). The
/// pointer and atomic fields are opaque storage here.
#[repr(C)]
pub struct WorkerRecord {
    pub(crate) pid: c_int,
    id: u32,
    pool: *mut c_void,
    queue: *mut c_void,
    pub shared: bool,
    pub redirect_stdout: bool,
    pub redirect_stdin: bool,
    pub redirect_stderr: bool,
    /// Written by the worker (busy, idle) while its dispatcher reads it.
    status: AtomicU8,
    kind: u8,
    msgqueue_mode: u8,
    pub child_process: bool,
    /// Its worker's requests in flight, which the master's retirement resets.
    pub concurrency: AtomicU32,
    start_time: i64,
    pub dispatch_count: i64,
    pub request_count: i64,
    pub response_count: i64,
    pub coroutine_num: usize,
    pub lock: Option<&'static SharedMutex>,
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
        Box::new(Self::empty())
    }

    /// `Worker()` by value, for an owner that boxes it itself: a zeroed record.
    #[must_use]
    pub fn empty() -> Self {
        let null = core::ptr::null_mut();
        Self {
            pid: 0,
            id: 0,
            pool: null,
            queue: null,
            shared: false,
            redirect_stdout: false,
            redirect_stdin: false,
            redirect_stderr: false,
            status: AtomicU8::new(0),
            kind: 0,
            msgqueue_mode: 0,
            child_process: false,
            concurrency: AtomicU32::new(0),
            start_time: 0,
            dispatch_count: 0,
            request_count: 0,
            response_count: 0,
            coroutine_num: 0,
            lock: None,
            pipe_object: null,
            pipe_master: null,
            pipe_worker: null,
            pipe_current: null,
            ptr: null,
        }
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

    pub fn set_status(&mut self, status: WorkerStatus) {
        *self.status.get_mut() = status as u8;
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
        (!raw.is_null()).then_some(Self {
            raw: raw.cast_mut(),
            life: core::marker::PhantomData,
        })
    }

    /// The record's address.
    #[must_use]
    pub fn raw(self) -> *mut WorkerRecord {
        self.raw
    }

    /// A copy of the whole record. Its bytes are copied around the status, which
    /// its worker may be storing meanwhile, and the status is loaded atomically.
    #[must_use]
    pub fn get(self) -> WorkerRecord {
        let mut copy = WorkerRecord::empty();
        let (from, to) = (self.raw.cast::<u8>(), (&raw mut copy).cast::<u8>());
        let status = core::mem::offset_of!(WorkerRecord, status);
        let after = status + size_of::<AtomicU8>();
        // SAFETY: `new`'s contract: a live record, whose bytes before the status are
        // copied into `copy`'s, a separate record of the same layout.
        unsafe { to.copy_from_nonoverlapping(from, status) };
        // SAFETY: as above, the bytes after the status, to the record's end.
        unsafe {
            to.wrapping_add(after).copy_from_nonoverlapping(
                from.wrapping_add(after),
                size_of::<WorkerRecord>() - after,
            );
        };
        *copy.status.get_mut() = self.status_slot().load(Ordering::Relaxed);
        copy
    }

    field!(
        /// The worker's kind (C++ `type`: `SW_EVENT_WORKER`/`SW_TASK_WORKER`/…).
        kind, set_kind, kind: u8
    );
    field!(
        /// The worker's id (its slot in the worker pool, `WorkerId`).
        id, set_id, id: u32
    );
    field!(
        /// The worker's mutex.
        lock, set_lock, lock: Option<&'static SharedMutex>
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
        /// Whether the worker's stdin goes to /dev/null when it starts.
        redirect_stdin, set_redirect_stdin, redirect_stdin: bool
    );
    field!(
        /// Whether the worker's stdout goes to /dev/null when it starts.
        redirect_stdout, set_redirect_stdout, redirect_stdout: bool
    );
    field!(
        /// Whether the worker's stderr goes to /dev/null when it starts.
        redirect_stderr, set_redirect_stderr, redirect_stderr: bool
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
        /// The native pipe object that owns the worker's two ends, or null.
        pipe_object, set_pipe_object, pipe_object: *mut c_void
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
        [
            self.request_count(),
            self.response_count(),
            self.dispatch_count(),
            i64::from(self.concurrency().load(Ordering::Relaxed)),
        ]
    }

    /// The worker's status, `None` before it has one. Relaxed: a dispatcher only
    /// picks an idle worker by it, and the message it then sends synchronizes.
    #[must_use]
    pub fn status(self) -> Option<WorkerStatus> {
        WorkerStatus::from_byte(self.status_slot().load(Ordering::Relaxed))
    }

    /// Whether the worker is `SW_WORKER_IDLE`.
    #[must_use]
    pub fn is_idle(self) -> bool {
        self.status() == Some(WorkerStatus::Idle)
    }

    /// Sets the worker's status, which its dispatcher reads concurrently.
    pub fn set_status(self, status: WorkerStatus) {
        self.status_slot().store(status as u8, Ordering::Relaxed);
    }

    fn status_slot(&self) -> &AtomicU8 {
        // SAFETY: `new`'s contract: a live record, for longer than this borrow; the
        // reference covers only the atomic field.
        unsafe { &(*self.raw).status }
    }

    /// `worker->coroutine_num`.
    #[must_use]
    pub fn coroutine_num(self) -> usize {
        // SAFETY: `new`'s contract: a live record, for longer than this read.
        unsafe { (*self.raw).coroutine_num }
    }

    /// `worker->concurrency`, now.
    #[must_use]
    pub fn concurrency_count(self) -> u32 {
        self.concurrency().load(Ordering::SeqCst)
    }

    /// `worker->concurrency`.
    fn concurrency(&self) -> &AtomicU32 {
        // SAFETY: `new`'s contract: a live record, for longer than this borrow; the
        // reference covers only the atomic field.
        unsafe { &(*self.raw).concurrency }
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
        self.set_status(WorkerStatus::Idle);
        crate::worker_globals::swr_v1_worker_global_set(3, 1);
        crate::worker_globals::swr_v1_worker_global_set(4, 0);
    }

    /// `Worker::shutdown()`: marks the current worker exited.
    pub fn shutdown(self) {
        self.set_status(WorkerStatus::Exit);
        crate::worker_globals::swr_v1_worker_global_set(4, 1);
    }

    /// `has_exceeded_max_request()`: this thread's request limit against the count.
    #[must_use]
    pub fn exceeded_max_request(self) -> bool {
        let limit = crate::worker_globals::swr_v1_worker_global_get(5).cast_unsigned();
        limit != 0 && self.request_count().cast_unsigned() >= limit
    }

    /// `sw_atomic_fetch_add(&worker->dispatch_count, 1)`: one more dispatch to this worker.
    pub fn count_dispatch(self) {
        let count = self
            .raw
            .wrapping_byte_add(core::mem::offset_of!(WorkerRecord, dispatch_count))
            .cast::<i64>();
        // SAFETY: `new`'s contract: a live record, whose dispatch counter is shared
        // and updated only atomically.
        unsafe { AtomicI64::from_ptr(count) }.fetch_add(1, Ordering::SeqCst);
    }

    /// Resets one retired worker's counters and removes its concurrency from the
    /// server's shared `total`. The CAS and underflow clamp keep the native reset
    /// protocol.
    pub fn reset_counters(self, total: &AtomicU32) {
        let concurrency = self.concurrency();
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
/// sockets, or for an `id` the pool has no worker for.
///
pub fn copy_for_process(
    pool: crate::process_pool::PoolRef<'_>,
    id: usize,
    copy: &mut WorkerRecord,
) -> c_int {
    const MSGQUEUE_BALANCE: u8 = 2;
    let pool = pool.record();
    // Copied by value, as upstream's `new Worker(...)` does.
    let Some(mut worker) = pool.worker(id).map(Worker::get) else {
        return -1;
    };
    let mut pipe = -1;
    if pool.ipc_mode == IpcMode::UnixSock {
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
    if pool.ipc_mode == IpcMode::MsgQueue && pool.on_message.is_none() {
        worker.queue = pool.queue;
        worker.msgqueue_mode = MSGQUEUE_BALANCE;
    }
    *copy = worker;
    pipe
}

/// `Worker::init()`.
pub fn swr_v1_worker_init(worker: Worker<'_>) {
    worker.init();
}

/// `Worker::shutdown()`.
pub fn swr_v1_worker_shutdown(worker: Worker<'_>) {
    worker.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::align_of;

    // The C++ view takes its fields from this struct (tools/process_records.py). Here
    // just sanity-check the Rust record's alignment and that the accessors read/write
    // the right field.
    #[test]
    fn a_pool_process_copies_the_worker_without_the_pipes_it_may_not_close() {
        extern "C-unwind" fn on_message(
            _: crate::process_pool::PoolRef<'_>,
            _: &crate::process_pool::RecvData,
        ) {
        }
        let mut fds: [c_int; 2] = [7, 9];
        let mut workers: [WorkerRecord; 2] = unsafe { core::mem::zeroed() };
        workers[1].id = u32::MAX;
        workers[1].pid = 41;
        workers[1].pipe_master = (&raw mut fds[0]).cast();
        workers[1].pipe_worker = (&raw mut fds[1]).cast();
        workers[1].pipe_object = core::ptr::dangling_mut();
        let mut pool: crate::process_pool::PoolRecord = unsafe { core::mem::zeroed() };
        pool.workers = workers.as_mut_ptr();
        pool.worker_num = 2;
        pool.ipc_mode = IpcMode::UnixSock;
        let mut copy: WorkerRecord = unsafe { core::mem::zeroed() };
        // A worker the pool doesn't have: no copy.
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 2, &mut copy),
            -1
        );
        assert_eq!(copy.pid, 0);
        // Another process's worker: the master's end of its pipe.
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            7
        );
        assert_eq!(
            (copy.pid, copy.pipe_current, copy.pipe_object),
            (41, workers[1].pipe_master, core::ptr::null_mut())
        );
        // The message bus owns the pipe.
        pool.message_bus = core::ptr::dangling_mut();
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            7
        );
        assert!(copy.pipe_current.is_null());
        // Without onMessage, a message-queue pool's process balances over the pool's queue.
        pool.ipc_mode = IpcMode::MsgQueue;
        pool.queue = 3 as *mut c_void;
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            -1
        );
        assert_eq!((copy.queue, copy.msgqueue_mode), (pool.queue, 2));
        pool.on_message = Some(on_message);
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            -1
        );
        assert!(copy.queue.is_null());
    }

    #[test]
    fn retirement_resets_counters_preserving_identity_and_clamps_underflow() {
        let mut record: WorkerRecord = unsafe { core::mem::zeroed() };
        record.pid = 17;
        record.id = 29;
        record.concurrency = AtomicU32::new(3);
        record.request_count = 7;
        record.response_count = 8;
        record.dispatch_count = 8;
        unsafe { Worker::new(&raw mut record) }.count_dispatch();
        assert_eq!(record.dispatch_count, 9);
        let total = core::sync::atomic::AtomicU32::new(7);
        unsafe { Worker::new(&raw mut record) }.reset_counters(&total);
        assert_eq!(
            (
                record.pid,
                record.id,
                total.load(core::sync::atomic::Ordering::SeqCst),
                *record.concurrency.get_mut()
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
        record.concurrency = AtomicU32::new(9);
        total.store(3, core::sync::atomic::Ordering::SeqCst);
        unsafe { Worker::new(&raw mut record) }.reset_counters(&total);
        assert_eq!(
            (
                *record.concurrency.get_mut(),
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
            assert_eq!(Worker::new(&raw const record).status(), None, "new");
            Worker::new(&raw mut record).set_status(WorkerStatus::Idle);
            assert_eq!(
                Worker::new(&raw const record).status(),
                Some(WorkerStatus::Idle)
            );
            assert!(Worker::new(&raw const record).is_idle());
        }
    }

    #[test]
    fn get_copies_every_field_around_the_status() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        record.pid = 41;
        record.id = 7;
        record.kind = 3;
        record.msgqueue_mode = 2;
        record.ptr = core::ptr::dangling_mut();
        let worker = unsafe { Worker::new(&raw mut record) };
        worker.set_status(WorkerStatus::Busy);
        let copy = worker.get();
        assert_eq!(
            (
                copy.pid,
                copy.id,
                copy.status.into_inner(),
                copy.kind,
                copy.msgqueue_mode,
                copy.ptr
            ),
            (41, 7, 1, 3, 2, record.ptr)
        );
    }

    #[test]
    fn type_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_kind(3);
            assert_eq!(Worker::new(&raw const record).kind(), 3);
            // independent of status
            Worker::new(&raw mut record).set_status(WorkerStatus::Busy);
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
            Worker::new(&raw mut record).set_status(WorkerStatus::Busy);
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

    /// `server_workers`' event-worker lookup: the first record in the array with this pid.
    unsafe fn swr_v1_worker_array_lookup(
        workers: *mut WorkerRecord,
        count: u32,
        pid: c_int,
    ) -> *mut WorkerRecord {
        unsafe {
            crate::worker_registry::find_array(
                workers.cast(),
                count,
                size_of::<WorkerRecord>(),
                core::mem::offset_of!(WorkerRecord, pid),
                pid,
            )
        }
        .cast()
    }

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
