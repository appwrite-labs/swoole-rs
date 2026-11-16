//! The server's `Worker` record, whose layout Rust owns (`#[repr(C)]`) so the
//! `sw_shm`-backed worker array every process maps is defined here. The C++
//! `swoole::Worker` the native test fixtures compile takes its fields from this
//! struct: `tools/process_records.py` writes them into
//! `native/process/swoole_rs_process_views.h`, and `tools/headers.sh --check`
//! fails when that header is stale. See #629.

use crate::process_pool::IpcMode;
use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU32, Ordering,
};
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

/// A native socket record this crate keeps for its owner, lent for `'a`. Only the
/// fields whose writers vouch for them are lent as one: a worker's pipe ends (the
/// unsafe [`Worker::set_pipes`], or copies of them), a hook's socket (the unsafe
/// [`crate::pool::PoolCall::set_socket`], or one of the former), and the stream
/// listener and connection the pool makes and frees itself. So the record is live
/// for `'a`, and crates/reactor takes one as its `Socket`.
#[derive(Clone, Copy)]
pub struct NativeSocket<'a> {
    raw: NonNull<c_void>,
    life: PhantomData<&'a c_void>,
}

impl NativeSocket<'_> {
    /// One of the vouched-for fields above, or `None` while it is null.
    pub(crate) fn lend(raw: *mut c_void) -> Option<Self> {
        Some(Self {
            raw: NonNull::new(raw)?,
            life: PhantomData,
        })
    }

    /// The record's address.
    #[must_use]
    pub fn as_non_null(self) -> NonNull<c_void> {
        self.raw
    }
}

/// An end of a worker's pipe.
#[derive(Clone, Copy)]
pub enum PipeEnd {
    /// `pipe_master`, the master's end.
    Master,
    /// `pipe_worker`, the worker's end.
    Worker,
    /// `pipe_current`, the end this process uses: one of the other two, or none.
    Current,
}

/// `swoole::Worker`'s fields, in upstream's order (`SWR_WORKER_RECORD_FIELDS`). The
/// pointer and atomic fields are opaque storage here.
///
/// `pool` and the pipe ends are private: `pool` is null or a live pool, and
/// `pipe_master`, `pipe_worker` and `pipe_current` are each null or a live native
/// socket, for as long as this process sends through the record or a copy of it.
/// Only [`Worker::join`] and [`Worker::set_pipes`], whose callers say so, write
/// them with anything but null or each other, so [`Worker::pool_ref`] and the sends
/// over it trust them. The records are shared memory, and each process owns its
/// own sockets at these addresses: a process that frees its pipes leaves the fields
/// to the others, and sends through the record no more.
#[repr(C)]
pub struct WorkerRecord {
    pub(crate) pid: AtomicI32,
    id: Cell<u32>,
    pool: Cell<*mut c_void>,
    queue: Cell<*mut c_void>,
    pub shared: Cell<bool>,
    /// In thread mode a user worker's thread writes these and `pipe_current` on its
    /// server record while another thread's `new Process` copies the record.
    pub redirect_stdout: AtomicBool,
    pub redirect_stdin: AtomicBool,
    pub redirect_stderr: AtomicBool,
    /// Written by the worker (busy, idle) while its dispatcher reads it.
    status: AtomicU8,
    kind: Cell<u8>,
    msgqueue_mode: Cell<u8>,
    pub child_process: Cell<bool>,
    /// Its worker's requests in flight, which the master's retirement resets.
    pub concurrency: AtomicU32,
    start_time: Cell<i64>,
    pub dispatch_count: AtomicI64,
    pub request_count: Cell<i64>,
    pub response_count: Cell<i64>,
    pub coroutine_num: Cell<usize>,
    pub lock: Cell<Option<&'static SharedMutex>>,
    pipe_object: Cell<*mut c_void>,
    pipe_master: Cell<*mut c_void>,
    pipe_worker: Cell<*mut c_void>,
    pipe_current: AtomicPtr<c_void>,
    ptr: Cell<*mut c_void>,
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
        let null = || Cell::new(core::ptr::null_mut());
        Self {
            pid: AtomicI32::new(0),
            id: Cell::new(0),
            pool: null(),
            queue: null(),
            shared: Cell::new(false),
            redirect_stdout: AtomicBool::new(false),
            redirect_stdin: AtomicBool::new(false),
            redirect_stderr: AtomicBool::new(false),
            status: AtomicU8::new(0),
            kind: Cell::new(0),
            msgqueue_mode: Cell::new(0),
            child_process: Cell::new(false),
            concurrency: AtomicU32::new(0),
            start_time: Cell::new(0),
            dispatch_count: AtomicI64::new(0),
            request_count: Cell::new(0),
            response_count: Cell::new(0),
            coroutine_num: Cell::new(0),
            lock: Cell::new(None),
            pipe_object: null(),
            pipe_master: null(),
            pipe_worker: null(),
            pipe_current: AtomicPtr::new(core::ptr::null_mut()),
            ptr: null(),
        }
    }

    /// The native pipe object that owns the worker's two ends, or null.
    #[must_use]
    pub fn pipe_object(&self) -> *mut c_void {
        self.pipe_object.get()
    }

    /// The worker's id (its slot in the worker pool).
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id.get()
    }

    /// The worker's kind (C++ `type`), `None` while unset.
    #[must_use]
    pub fn kind(&self) -> Option<WorkerType> {
        WorkerType::from_code(self.kind.get().into())
    }

    /// The worker's process (or thread) id.
    #[must_use]
    pub fn pid(&self) -> c_int {
        self.pid.load(Ordering::Relaxed)
    }

    /// Sets the worker's process (or thread) id.
    pub fn set_pid(&mut self, pid: c_int) {
        *self.pid.get_mut() = pid;
    }

    pub fn set_id(&mut self, id: u32) {
        self.id.set(id);
    }

    #[must_use]
    pub fn queue(&self) -> *mut c_void {
        self.queue.get()
    }

    pub fn set_queue(&mut self, queue: *mut c_void) {
        self.queue.set(queue);
    }

    #[must_use]
    pub fn msgqueue_mode(&self) -> u8 {
        self.msgqueue_mode.get()
    }

    pub fn set_msgqueue_mode(&mut self, mode: u8) {
        self.msgqueue_mode.set(mode);
    }

    pub fn set_status(&mut self, status: WorkerStatus) {
        *self.status.get_mut() = status as u8;
    }

    /// `worker->ptr = data`: what its owner keeps there.
    pub fn set_user_data(&mut self, data: *mut c_void) {
        self.ptr.set(data);
    }

    /// `worker->request_count++`, with the native unsigned wrapping.
    pub fn add_request_count(&mut self) {
        Worker::from(&*self).add_request_count();
    }

    /// `has_exceeded_max_request()`: this thread's request limit against the count.
    #[must_use]
    pub fn exceeded_max_request(&self) -> bool {
        Worker::from(self).exceeded_max_request()
    }
}

/// A live worker record: the only way to reach one's fields. Every field is a
/// `Cell` or an atomic, so the shared borrow [`Worker::record`] lends lets other
/// handles, and other processes in shared memory, write the record meanwhile.
/// Native code passes one as a `Worker *`.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Worker<'a> {
    raw: *mut WorkerRecord,
    life: core::marker::PhantomData<&'a WorkerRecord>,
}

impl<'a> From<&'a WorkerRecord> for Worker<'a> {
    /// The record a borrow keeps live for `'a`.
    fn from(record: &'a WorkerRecord) -> Self {
        Self {
            raw: core::ptr::from_ref(record).cast_mut(),
            life: core::marker::PhantomData,
        }
    }
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
    ($(#[$doc:meta])* $get:ident, $field:ident: $ty:ty) => {
        $(#[$doc])*
        #[must_use]
        pub fn $get(self) -> $ty {
            self.record().$field.get()
        }
    };
    ($(#[$doc:meta])* $get:ident, $vis:vis $set:ident, $field:ident: $ty:ty) => {
        field!($(#[$doc])* $get, $field: $ty);

        $(#[$doc])*
        $vis fn $set(self, value: $ty) {
            self.record().$field.set(value);
        }
    };
    ($(#[$doc:meta])* $get:ident, $vis:vis $set:ident, atomic $field:ident: $ty:ty) => {
        $(#[$doc])*
        #[must_use]
        pub fn $get(self) -> $ty {
            self.record().$field.load(Ordering::Relaxed)
        }

        $(#[$doc])*
        $vis fn $set(self, value: $ty) {
            self.record().$field.store(value, Ordering::Relaxed);
        }
    };
}

impl<'a> Worker<'a> {
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

    /// The record's address.
    #[must_use]
    pub fn raw(self) -> *mut WorkerRecord {
        self.raw
    }

    /// The record, for `'a`: its fields are cells and atomics, read and written
    /// one at a time.
    fn record(self) -> &'a WorkerRecord {
        // SAFETY: `new`'s contract (or the borrow `from` took): a live record for
        // `'a`, whose owner orders the writes made through its cells.
        unsafe { &*self.raw }
    }

    /// A copy of the whole record, a field at a time: the status, which its worker
    /// may be storing meanwhile, and the concurrency are loaded atomically.
    #[must_use]
    pub fn get(self) -> WorkerRecord {
        let r = self.record();
        WorkerRecord {
            pid: AtomicI32::new(r.pid.load(Ordering::Relaxed)),
            id: r.id.clone(),
            pool: r.pool.clone(),
            queue: r.queue.clone(),
            shared: r.shared.clone(),
            redirect_stdout: AtomicBool::new(r.redirect_stdout.load(Ordering::Relaxed)),
            redirect_stdin: AtomicBool::new(r.redirect_stdin.load(Ordering::Relaxed)),
            redirect_stderr: AtomicBool::new(r.redirect_stderr.load(Ordering::Relaxed)),
            status: AtomicU8::new(r.status.load(Ordering::Relaxed)),
            kind: r.kind.clone(),
            msgqueue_mode: r.msgqueue_mode.clone(),
            child_process: r.child_process.clone(),
            concurrency: AtomicU32::new(r.concurrency.load(Ordering::Relaxed)),
            start_time: r.start_time.clone(),
            dispatch_count: AtomicI64::new(r.dispatch_count.load(Ordering::Relaxed)),
            request_count: r.request_count.clone(),
            response_count: r.response_count.clone(),
            coroutine_num: r.coroutine_num.clone(),
            lock: r.lock.clone(),
            pipe_object: r.pipe_object.clone(),
            pipe_master: r.pipe_master.clone(),
            pipe_worker: r.pipe_worker.clone(),
            pipe_current: AtomicPtr::new(r.pipe_current.load(Ordering::Relaxed)),
            ptr: r.ptr.clone(),
        }
    }

    /// `*worker = record`, a field at a time: the record becomes a copy of
    /// another, whose pool and pipes (live while that one is sent through) it then
    /// shares.
    pub fn put(self, record: WorkerRecord) {
        let WorkerRecord {
            pid,
            id,
            pool,
            queue,
            shared,
            redirect_stdout,
            redirect_stdin,
            redirect_stderr,
            status,
            kind,
            msgqueue_mode,
            child_process,
            concurrency,
            start_time,
            dispatch_count,
            request_count,
            response_count,
            coroutine_num,
            lock,
            pipe_object,
            pipe_master,
            pipe_worker,
            pipe_current,
            ptr,
        } = record;
        let r = self.record();
        r.pid.store(pid.into_inner(), Ordering::Relaxed);
        r.id.set(id.into_inner());
        r.pool.set(pool.into_inner());
        r.queue.set(queue.into_inner());
        r.shared.set(shared.into_inner());
        r.redirect_stdout
            .store(redirect_stdout.into_inner(), Ordering::Relaxed);
        r.redirect_stdin
            .store(redirect_stdin.into_inner(), Ordering::Relaxed);
        r.redirect_stderr
            .store(redirect_stderr.into_inner(), Ordering::Relaxed);
        r.status.store(status.into_inner(), Ordering::Relaxed);
        r.kind.set(kind.into_inner());
        r.msgqueue_mode.set(msgqueue_mode.into_inner());
        r.child_process.set(child_process.into_inner());
        r.concurrency
            .store(concurrency.into_inner(), Ordering::Relaxed);
        r.start_time.set(start_time.into_inner());
        r.dispatch_count
            .store(dispatch_count.into_inner(), Ordering::Relaxed);
        r.request_count.set(request_count.into_inner());
        r.response_count.set(response_count.into_inner());
        r.coroutine_num.set(coroutine_num.into_inner());
        r.lock.set(lock.into_inner());
        r.pipe_object.set(pipe_object.into_inner());
        r.pipe_master.set(pipe_master.into_inner());
        r.pipe_worker.set(pipe_worker.into_inner());
        r.pipe_current
            .store(pipe_current.into_inner(), Ordering::Relaxed);
        r.ptr.set(ptr.into_inner());
    }

    field!(
        /// The worker's kind's code (C++ `type`), 0 while unset.
        kind_code, pub(crate) set_kind_code, kind: u8
    );

    /// The worker's kind (C++ `type`), `None` while unset.
    #[must_use]
    pub fn kind(self) -> Option<WorkerType> {
        WorkerType::from_code(self.kind_code().into())
    }

    /// Sets the worker's kind.
    pub fn set_kind(self, kind: WorkerType) {
        self.set_kind_code(kind.code());
    }
    field!(
        /// The worker's id (its slot in the worker pool, `WorkerId`).
        id, pub set_id, id: u32
    );
    field!(
        /// The worker's mutex.
        lock, pub set_lock, lock: Option<&'static SharedMutex>
    );
    field!(
        /// The worker's process (or thread) id.
        pid, pub set_pid, atomic pid: c_int
    );
    field!(
        /// The worker's message queue, or null.
        queue, pub set_queue, queue: *mut c_void
    );
    field!(
        /// The worker's `msgqueue_mode`.
        msgqueue_mode, pub set_msgqueue_mode, msgqueue_mode: u8
    );
    field!(
        /// The worker's pool (a native `ProcessPool`), or null: [`Worker::pool_ref`]'s.
        pool, write_pool, pool: *mut c_void
    );
    field!(
        /// Whether the record is a copy of a server's worker, whose pipe it shares.
        shared, pub set_shared, shared: bool
    );
    field!(
        /// Whether this process is the worker's forked child.
        child_process, pub set_child_process, child_process: bool
    );
    field!(
        /// Whether the worker's stdin goes to /dev/null when it starts.
        redirect_stdin, pub set_redirect_stdin, atomic redirect_stdin: bool
    );
    field!(
        /// Whether the worker's stdout goes to /dev/null when it starts.
        redirect_stdout, pub set_redirect_stdout, atomic redirect_stdout: bool
    );
    field!(
        /// Whether the worker's stderr goes to /dev/null when it starts.
        redirect_stderr, pub set_redirect_stderr, atomic redirect_stderr: bool
    );
    field!(
        /// `Worker::ptr`: what its owner keeps there (a user worker's process object).
        user_data, pub set_user_data, ptr: *mut c_void
    );
    field!(
        /// The master's end of the worker's pipe (a live native socket), or null.
        pipe_master, write_pipe_master, pipe_master: *mut c_void
    );
    field!(
        /// The worker's end of its pipe (a live native socket), or null.
        pipe_worker, write_pipe_worker, pipe_worker: *mut c_void
    );
    field!(
        /// The native pipe object that owns the worker's two ends, or null.
        pipe_object, write_pipe_object, pipe_object: *mut c_void
    );
    field!(
        /// The end of its pipe this process uses (a live native socket), or null.
        pipe_current, write_pipe_current, atomic pipe_current: *mut c_void
    );

    /// The worker's live pool, or `None` for a record no pool holds.
    #[must_use]
    pub fn pool_ref(self) -> Option<crate::process_pool::PoolRef<'a>> {
        let pool = self.pool().cast::<crate::process_pool::PoolRecord>();
        if pool.is_null() {
            return None;
        }
        // SAFETY: the record's invariant: a pool only `join` writes,
        // live for as long as the record is sent through, so for the handle's `'a`.
        Some(unsafe { crate::process_pool::PoolRef::new(pool) })
    }

    /// Makes the record one of `pool`'s workers, for this crate's pool operations, over the pool that
    /// owns the record's array or was lent it for as long as itself: `create`,
    /// `start_check` and the server's startup.
    pub(crate) fn join(self, pool: crate::process_pool::PoolRef<'_>) {
        self.write_pool(pool.raw().cast());
    }

    /// Gives the worker its pipe: the native pipe object that owns the two ends,
    /// the master's end and the worker's. The current end is left as it is.
    ///
    /// # Safety
    /// `master` and `end` are each null or a native socket that stays live for as
    /// long as this process sends through this record or a copy of it, or until the
    /// record's pipes are set or cleared again.
    pub unsafe fn set_pipes(self, object: *mut c_void, master: *mut c_void, end: *mut c_void) {
        self.write_pipe_object(object);
        self.write_pipe_master(master);
        self.write_pipe_worker(end);
    }

    /// The `end` of the worker's pipe, or `None` while it has none: live for the
    /// handle, as the record's invariant has it.
    #[must_use]
    pub fn pipe(self, end: PipeEnd) -> Option<NativeSocket<'a>> {
        NativeSocket::lend(match end {
            PipeEnd::Master => self.pipe_master(),
            PipeEnd::Worker => self.pipe_worker(),
            PipeEnd::Current => self.pipe_current(),
        })
    }

    /// Forgets the worker's pipe, which its owner is about to free.
    pub fn clear_pipes(self) {
        let null = core::ptr::null_mut();
        self.write_pipe_object(null);
        self.write_pipe_master(null);
        self.write_pipe_worker(null);
        self.write_pipe_current(null);
    }

    /// `pipe_current = pipe_master` (`master`) or `pipe_worker`: the end this
    /// process reads and writes.
    pub fn use_pipe(self, master: bool) {
        self.write_pipe_current(if master {
            self.pipe_master()
        } else {
            self.pipe_worker()
        });
    }

    field!(
        /// `worker->request_count`.
        request_count, pub set_request_count, request_count: i64
    );
    field!(
        /// `worker->response_count`.
        response_count, pub set_response_count, response_count: i64
    );
    field!(
        /// `worker->dispatch_count`, which dispatchers add to atomically.
        dispatch_count, pub set_dispatch_count, atomic dispatch_count: i64
    );
    field!(
        /// `worker->start_time`.
        start_time, pub set_start_time, start_time: i64
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

    fn status_slot(self) -> &'a AtomicU8 {
        &self.record().status
    }

    field!(
        /// `worker->coroutine_num`.
        coroutine_num, pub write_coroutine_num, coroutine_num: usize
    );

    /// `worker->concurrency`, now.
    #[must_use]
    pub fn concurrency_count(self) -> u32 {
        self.concurrency().load(Ordering::SeqCst)
    }

    /// `worker->concurrency++`, returning what it was.
    #[must_use]
    pub fn add_concurrency(self) -> u32 {
        self.concurrency().fetch_add(1, Ordering::SeqCst)
    }

    /// `worker->concurrency--`.
    pub fn sub_concurrency(self) {
        self.concurrency().fetch_sub(1, Ordering::SeqCst);
    }

    /// `worker->concurrency`.
    fn concurrency(self) -> &'a AtomicU32 {
        &self.record().concurrency
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
        self.record().dispatch_count.fetch_add(1, Ordering::SeqCst);
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
/// queue. Returns the copy's pipe fd when the pool talks over unix sockets and
/// has a worker `id`.
pub fn copy_for_process(
    pool: crate::process_pool::PoolRef<'_>,
    id: usize,
    copy: &mut WorkerRecord,
) -> Option<c_int> {
    const MSGQUEUE_BALANCE: u8 = 2;
    let pool = pool.record();
    // Copied by value, as upstream's `new Worker(...)` does.
    let mut worker = pool.worker(id).map(Worker::get)?;
    let null = core::ptr::null_mut();
    let mut pipe = None;
    if pool.ipc_mode == IpcMode::UnixSock {
        let current = if worker.id() == crate::methods::swoole_get_worker_id() {
            worker.pipe_worker.get()
        } else {
            worker.pipe_master.get()
        };
        *worker.pipe_current.get_mut() = current;
        worker.pipe_object.set(null);
        // SAFETY: a unix-socket pool's worker pipes are live sockets, and
        // `network::Socket` begins with its fd.
        pipe = Some(unsafe { current.cast::<c_int>().read() });
    }
    if pool.has_message_bus() {
        *worker.pipe_current.get_mut() = null;
        worker.pipe_object.set(null);
    }
    if pool.ipc_mode == IpcMode::MsgQueue && pool.on_message.is_none() {
        worker.queue.set(pool.queue());
        worker.msgqueue_mode.set(MSGQUEUE_BALANCE);
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
        let mut fds: [c_int; 2] = [7, 9];
        let mut workers: [WorkerRecord; 2] = unsafe { core::mem::zeroed() };
        workers[1].id.set(u32::MAX);
        *workers[1].pid.get_mut() = 41;
        workers[1].pipe_master.set((&raw mut fds[0]).cast());
        workers[1].pipe_worker.set((&raw mut fds[1]).cast());
        workers[1].pipe_object.set(core::ptr::dangling_mut());
        let mut pool: crate::process_pool::PoolRecord = unsafe { core::mem::zeroed() };
        pool.workers = workers.as_mut_ptr();
        pool.worker_num = 2;
        pool.ipc_mode = IpcMode::UnixSock;
        let mut copy: WorkerRecord = unsafe { core::mem::zeroed() };
        // A worker the pool doesn't have: no copy.
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 2, &mut copy),
            None
        );
        assert_eq!(*copy.pid.get_mut(), 0);
        // Another process's worker: the master's end of its pipe.
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            Some(7)
        );
        assert_eq!(
            (
                *copy.pid.get_mut(),
                *copy.pipe_current.get_mut(),
                copy.pipe_object.get()
            ),
            (41, workers[1].pipe_master.get(), core::ptr::null_mut())
        );
        // The message bus owns the pipe.
        crate::process_pool::PoolRef::from(&mut pool).set_message_bus(Some(Box::new(())));
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            Some(7)
        );
        assert!(copy.pipe_current.get_mut().is_null());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no msgget")]
    fn a_queue_pools_process_balances_over_the_pools_queue() {
        extern "C-unwind" fn on_message(
            _: crate::process_pool::PoolRef<'_>,
            _: &crate::process_pool::RecvData<'_>,
        ) {
        }
        let mut workers: [WorkerRecord; 2] = unsafe { core::mem::zeroed() };
        let mut pool: crate::process_pool::PoolRecord = unsafe { core::mem::zeroed() };
        pool.workers = workers.as_mut_ptr();
        pool.worker_num = 2;
        let mut copy: WorkerRecord = unsafe { core::mem::zeroed() };
        // Without onMessage, a message-queue pool's process balances over the pool's queue.
        pool.ipc_mode = IpcMode::MsgQueue;
        pool.queue = Some(Box::new(swoole_rs_ossupport::QueueRecord::new(
            libc::IPC_PRIVATE,
            false,
            0,
        )));
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            None
        );
        assert_eq!(
            (copy.queue.get(), copy.msgqueue_mode.get()),
            (pool.queue(), 2)
        );
        if let Some(queue) = pool.queue.as_deref_mut() {
            queue.destroy();
        }
        pool.on_message = Some(on_message);
        assert_eq!(
            copy_for_process(crate::process_pool::PoolRef::from(&mut pool), 1, &mut copy),
            None
        );
        assert!(copy.queue.get().is_null());
    }

    #[test]
    fn retirement_resets_counters_preserving_identity_and_clamps_underflow() {
        let mut record: WorkerRecord = unsafe { core::mem::zeroed() };
        *record.pid.get_mut() = 17;
        record.id.set(29);
        record.concurrency = AtomicU32::new(3);
        record.request_count.set(7);
        record.response_count.set(8);
        *record.dispatch_count.get_mut() = 8;
        unsafe { Worker::new(&raw mut record) }.count_dispatch();
        assert_eq!(record.dispatch_count.load(Ordering::Relaxed), 9);
        let total = core::sync::atomic::AtomicU32::new(7);
        unsafe { Worker::new(&raw mut record) }.reset_counters(&total);
        assert_eq!(
            (
                *record.pid.get_mut(),
                record.id.get(),
                total.load(core::sync::atomic::Ordering::SeqCst),
                *record.concurrency.get_mut()
            ),
            (17, 29, 4, 0)
        );
        assert_eq!(
            (
                record.request_count.get(),
                record.response_count.get(),
                record.dispatch_count.load(Ordering::Relaxed)
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
        *record.pid.get_mut() = 41;
        record.id.set(7);
        record.kind.set(3);
        record.msgqueue_mode.set(2);
        record.ptr.set(core::ptr::dangling_mut());
        let worker = unsafe { Worker::new(&raw mut record) };
        worker.set_status(WorkerStatus::Busy);
        let copy = worker.get();
        assert_eq!(
            (
                copy.pid(),
                copy.id.get(),
                copy.status.into_inner(),
                copy.kind.get(),
                copy.msgqueue_mode.get(),
                copy.ptr.get()
            ),
            (41, 7, 1, 3, 2, record.ptr.get())
        );
    }

    #[test]
    fn type_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_kind(WorkerType::Manager);
            assert_eq!(
                Worker::new(&raw const record).kind(),
                Some(WorkerType::Manager)
            );
            // independent of status
            Worker::new(&raw mut record).set_status(WorkerStatus::Busy);
            assert_eq!(
                Worker::new(&raw const record).kind(),
                Some(WorkerType::Manager)
            );
        }
    }

    #[test]
    fn pid_accessors_round_trip() {
        let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut record).set_pid(4242);
            assert_eq!(Worker::new(&raw const record).pid(), 4242);
            // independent of the one-byte fields that follow it
            Worker::new(&raw mut record).set_kind(WorkerType::Event);
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
        use crate::process_pool::{PoolRecord, PoolRef};
        let mut pools = [PoolRecord::empty(), PoolRecord::empty()];
        let [event_pool, task_pool] = pools.each_mut().map(|pool| unsafe { PoolRef::new(pool) });
        let view = unsafe {
            crate::server_workers::ServerWorkers::new(
                (event_pool, workers, count),
                (task_pool, 0),
                (core::ptr::null_mut(), 0),
            )
        };
        crate::server_workers::swr_v1_process_server_worker_by_pid(&view, None, pid)
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
