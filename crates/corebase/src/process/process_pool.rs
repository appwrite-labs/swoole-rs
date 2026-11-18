//! `ProcessPool`'s lifecycle (once `src/os/process_pool.cc`): create, start, spawn,
//! stop, detach, reload, the manager's wait and shutdown, destroy and the
//! worker-side entry into the loops of [`crate::process::pool`]. Rust reads and writes
//! the pool's fields through [`PoolRecord`] (the C++ layout mirror follows it).
//! The pool owns its registry, IPC, reload, pipes and message bus; the native
//! owners are type-erased to avoid a dependency cycle. `crates/server/src/pool_native.rs`
//! constructs them and supplies the effects and callbacks through [`PoolNative`].
//!
//! A callback that can reach PHP runs through [`PoolNative::callback`] inside
//! the bailout trap; when it bails out, the export returns [`Bailed`] and its
//! native caller raises the bailout again once the Rust frames are gone.

use crate::BoxCellExt;
use crate::CCast;
use crate::DataHead;
use crate::memory::{ChannelInitError, SharedChannel};
use crate::process::task_file::Packed;
use crate::process::worker_record::{Worker, WorkerType};
use core::cell::Cell;
use std::ffi::{c_int, c_long, c_void};
use std::ptr;
use std::sync::atomic::AtomicU32;

use crate::process::armed_timer::ArmedTimer;
use crate::process::os::{getpid, kill};
use crate::process::pool::{
    Bailed, PoolCall, PoolIpc, PoolLoop, pool_ipc_free, pool_ipc_new, pool_ipc_prepare, pool_run,
    swr_v1_process_pool_ipc_listener,
};
use crate::process::reaper::{self, Reap, ReloadStart, Waited, wait_child};
use crate::process::reload_task::{ReloadTask, Warn};
use crate::process::worker_record::WorkerRecord;
use crate::process::worker_registry::WorkerRegistry;

/// The native pipe owners, kept alive at stable addresses until the pool closes.
type Pipes = Vec<Box<dyn std::any::Any>>;
/// A pool's boxed owner, one pointer wide in the record: replaced whole, and lent
/// to one operation at a time by taking it out ([`BoxCellExt::with_box`]).
type Owner<T> = core::cell::Cell<Option<Box<T>>>;

/// `swoole::ProcessPool`'s fields, in order. Rust owns this layout; the C++ view
/// takes its fields from here (`SWR_POOL_RECORD_FIELDS`, `tools/process_records.py`).
/// Every field but the three owners the record lends in place (`registry`, `queue`
/// and `message_box`) is a cell or an atomic, so a handle writes it through
/// `&PoolRecord` while other loans of the record are live.
#[repr(C)]
pub struct PoolRecord {
    pub running: Cell<bool>,
    pub reload_init: Cell<bool>,
    pub read_message: Cell<bool>,
    pub started: Cell<bool>,
    pub schedule_by_sysvmsg: Cell<bool>,
    pub is_async: Cell<bool>,
    pub ipc_mode: Cell<IpcMode>,
    pub protocol_type: Cell<c_int>,
    pub master_pid: Cell<c_int>,
    pub max_wait_time: Cell<u32>,
    pub reload_count: Cell<u64>,
    pub reload_last_time: Cell<i64>,
    pub kind: Cell<u8>,
    pub start_id: Cell<u16>,
    pub use_msgqueue: Cell<bool>,
    pub use_socket: Cell<bool>,
    pub max_packet_size: Cell<u32>,
    pub msgqueue_key: Cell<c_int>,
    pub(crate) worker_num: Cell<u32>,
    pub max_request: Cell<u32>,
    pub max_request_grace: Cell<u32>,
    pub scheduler_warning: Cell<bool>,
    pub warning_time: Cell<i64>,
    pub(crate) on_start: Cell<Option<PoolFn>>,
    pub(crate) on_shutdown: Cell<Option<PoolFn>>,
    pub(crate) on_before_reload: Cell<Option<PoolFn>>,
    pub(crate) on_after_reload: Cell<Option<PoolFn>>,
    pub(crate) on_task: Cell<Option<TaskFn>>,
    pub(crate) on_worker_start: Cell<Option<WorkerFn>>,
    pub(crate) on_message: Cell<Option<MessageFn>>,
    pub(crate) on_worker_exit: Cell<Option<WorkerFn>>,
    pub(crate) on_worker_stop: Cell<Option<WorkerFn>>,
    pub(crate) on_worker_error: Cell<Option<ErrorFn>>,
    pub(crate) on_worker_message: Cell<Option<EventFn>>,
    pub(crate) on_worker_not_found: Cell<Option<NotFoundFn>>,
    pub(crate) main_loop: Cell<Option<MainLoopFn>>,
    /// The round robin's next worker, which other processes update atomically.
    pub round_id: AtomicU32,
    pub(crate) workers: Cell<*mut WorkerRecord>,
    pipes: Owner<Pipes>,
    // The records and resource owners are private: outside code uses the handle's
    // operations. The hooks are installed together ([`PoolRef::install`]), and `ptr`
    // is written under a key ([`PoolRef::hold`]).
    /// The registry `create` made, which `destroy` (or dropping the record) frees.
    pub(crate) registry: Option<Box<WorkerRegistry>>,
    /// A message-queue pool's queue, which goes as the registry does.
    pub(crate) queue: Option<Box<crate::ossupport::QueueRecord>>,
    pub(crate) ipc: Cell<*mut PoolIpc>,
    pub(crate) message_box: SharedChannel,
    message_bus: Owner<Box<dyn std::any::Any>>,
    reload_task: Owner<ReloadTask>,
    pub(crate) ptr: Cell<*mut c_void>,
    /// The `max_wait_time` timer `wait` arms to SIGKILL the workers still
    /// running. It stays armed after `wait` returns, which reaps only the first,
    /// and goes with the pool: `destroy`, or dropping the record, deletes it.
    pub kill_timer: Cell<Option<ArmedTimer>>,
    /// The key `ptr` is held under ([`PoolRef::hold`]), or null for none. A raw
    /// pointer, compared and never read, so a zeroed record holds nothing.
    pub(crate) ptr_key: Cell<*const PoolObjectKey>,
}

/// A kind of object a pool's `ptr` holds ([`PoolRef::hold`]), named by the address of a
/// `static`: a crate keeps its key private, so what a pool holds under it is what that
/// crate gave it.
#[derive(Default)]
pub struct PoolObjectKey(
    #[expect(
        dead_code,
        reason = "a byte gives each key's static an address of its own"
    )]
    u8,
);

impl PoolObjectKey {
    #[must_use]
    pub const fn new() -> Self {
        Self(0)
    }
}

impl PoolRecord {
    #[must_use]
    pub fn has_pipes(&self) -> bool {
        self.pipes.with_box(|_| ()).is_some()
    }

    #[must_use]
    pub fn has_message_bus(&self) -> bool {
        self.message_bus.with_box(|_| ()).is_some()
    }

    /// A pool before `create`, as C's `ProcessPool pool = {}`: every field zero,
    /// null or `None`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            running: Cell::new(false),
            reload_init: Cell::new(false),
            read_message: Cell::new(false),
            started: Cell::new(false),
            schedule_by_sysvmsg: Cell::new(false),
            is_async: Cell::new(false),
            ipc_mode: Cell::new(IpcMode::None),
            protocol_type: Cell::new(0),
            master_pid: Cell::new(0),
            max_wait_time: Cell::new(0),
            reload_count: Cell::new(0),
            reload_last_time: Cell::new(0),
            kind: Cell::new(0),
            start_id: Cell::new(0),
            use_msgqueue: Cell::new(false),
            use_socket: Cell::new(false),
            max_packet_size: Cell::new(0),
            msgqueue_key: Cell::new(0),
            worker_num: Cell::new(0),
            max_request: Cell::new(0),
            max_request_grace: Cell::new(0),
            scheduler_warning: Cell::new(false),
            warning_time: Cell::new(0),
            on_start: Cell::new(None),
            on_shutdown: Cell::new(None),
            on_before_reload: Cell::new(None),
            on_after_reload: Cell::new(None),
            on_task: Cell::new(None),
            on_worker_start: Cell::new(None),
            on_message: Cell::new(None),
            on_worker_exit: Cell::new(None),
            on_worker_stop: Cell::new(None),
            on_worker_error: Cell::new(None),
            on_worker_message: Cell::new(None),
            on_worker_not_found: Cell::new(None),
            main_loop: Cell::new(None),
            round_id: AtomicU32::new(0),
            workers: Cell::new(ptr::null_mut()),
            pipes: Owner::new(None),
            registry: None,
            queue: None,
            ipc: Cell::new(ptr::null_mut()),
            message_box: SharedChannel::none(),
            message_bus: Owner::new(None),
            reload_task: Owner::new(None),
            ptr: Cell::new(ptr::null_mut()),
            kill_timer: Cell::new(None),
            ptr_key: Cell::new(ptr::null()),
        }
    }

    /// The pool's worker count: how many records `workers` holds.
    #[must_use]
    pub fn worker_num(&self) -> u32 {
        self.worker_num.get()
    }

    /// The pool's `worker_num` worker records, or null before `create`.
    #[must_use]
    pub fn workers(&self) -> *mut WorkerRecord {
        self.workers.get()
    }

    /// The `SysV` message queue of a message-queue pool, or null.
    #[must_use]
    pub fn queue(&self) -> *mut c_void {
        self.queue.as_deref().map_or(ptr::null_mut(), |queue| {
            ptr::from_ref(queue).cast_mut().cast()
        })
    }

    /// The pool's IPC state, or null before `create`.
    #[must_use]
    pub fn ipc(&self) -> *mut PoolIpc {
        self.ipc.get()
    }

    /// The pool's message box (a shared-memory channel), or null.
    #[must_use]
    pub fn message_box(&self) -> *mut c_void {
        self.message_box.as_ptr()
    }

    /// The pool's message box, which the workers push to and the master pops from.
    pub(crate) fn message_box_channel(&self) -> Option<&crate::memory::channel::Record> {
        self.message_box.get()
    }

    /// The pool's worker at `index`, or `None` past `worker_num`.
    #[must_use]
    pub fn worker(&self, index: usize) -> Option<Worker<'_>> {
        if self.workers.get().is_null() || index >= self.worker_num.get() as usize {
            return None;
        }
        // SAFETY: `workers` holds the pool's `worker_num` records, live as long as the
        // pool (the field is crate-private, and only `PoolRef::set_workers` publishes it).
        Some(unsafe { Worker::from(&*self.workers.get().wrapping_add(index)) })
    }

    /// The pool's worker at `raw`, or `None` when `raw` is none of its records.
    #[must_use]
    pub fn own(&self, raw: *const WorkerRecord) -> Option<Worker<'_>> {
        self.worker(self.index_of(raw)?)
    }

    /// The index of the pool's worker at `raw`, or `None` when `raw` is none of its
    /// records.
    #[must_use]
    pub fn index_of(&self, raw: *const WorkerRecord) -> Option<usize> {
        let offset = raw.addr().checked_sub(self.workers.get().addr())?;
        let index = offset / size_of::<WorkerRecord>();
        let own = !self.workers.get().is_null() && index < self.worker_num.get() as usize;
        (own && offset % size_of::<WorkerRecord>() == 0).then_some(index)
    }

    /// `f` over the pool's IPC state; `None` before `create`.
    pub(crate) fn with_ipc<R>(&self, f: impl FnOnce(&mut PoolIpc) -> R) -> Option<R> {
        // SAFETY: `ipc` is crate-private: null, or the state `create` made until
        // `destroy` frees it. Only these calls borrow it, none across a callback, so
        // nothing else borrows it while `f` runs.
        unsafe { self.ipc.get().as_mut() }.map(f)
    }

    /// The pool's stream listener, if any.
    #[must_use]
    pub fn ipc_listener(&self) -> Option<crate::process::pool::Connection> {
        self.with_ipc(|ipc| swr_v1_process_pool_ipc_listener(ipc).cloned())
            .flatten()
    }

    /// The stream connection being served, if any.
    #[must_use]
    pub fn ipc_connection(&self) -> Option<crate::process::pool::Connection> {
        self.with_ipc(|ipc| crate::process::pool::pool_ipc_connection(ipc))
            .flatten()
    }

    /// The registry `create` made, or null.
    #[must_use]
    pub fn registry(&self) -> *mut WorkerRegistry {
        self.registry
            .as_deref()
            .map_or(ptr::null_mut(), |registry| {
                ptr::from_ref(registry).cast_mut()
            })
    }

    /// `f` over the registry `create` made; `None` before it. No `f` reaches
    /// `destroy`, which frees it.
    pub(crate) fn with_registry<R>(&self, f: impl FnOnce(&WorkerRegistry) -> R) -> Option<R> {
        self.registry.as_deref().map(f)
    }
}

/// [`PoolRef::reset`]: each owner field, then each cell field, takes its value in
/// [`PoolRecord::empty`], forgetting the old one. The pattern names every field, so
/// a field added to the record and not here fails to compile.
macro_rules! reset {
    ($pool:ident; $($cell:ident),* ; $($owner:ident),*) => {{
        let PoolRecord { $($cell,)* $($owner,)* round_id } = PoolRecord::empty();
        $(_ = core::mem::ManuallyDrop::new($pool.replace(|record| &mut record.$owner, $owner));)*
        let record = $pool.record();
        $(_ = core::mem::ManuallyDrop::new(record.$cell.replace($cell.into_inner()));)*
        record.round_id.store(round_id.into_inner(), core::sync::atomic::Ordering::Relaxed);
    }};
}

/// A live pool, as the pool passes one to its callbacks. Like [`Worker`], it reads
/// and writes the record's cells a field at a time, as the pool's own code and
/// signal handlers do while a callback runs.
///
/// Any record is a pool the operations over this handle can run on: a zeroed one,
/// or one `create` made. The fields they trust (`worker_num` and the pointers from
/// `workers` on) are crate-private, written only by this crate's operations and the
/// `unsafe` setters, so safe code can't forge them, and the operations check each
/// pointer for null before using it.
#[derive(Clone, Copy)]
pub struct PoolRef<'a> {
    raw: *mut PoolRecord,
    live: core::marker::PhantomData<&'a PoolRecord>,
}

impl<'a> From<&'a mut PoolRecord> for PoolRef<'a> {
    /// The record a unique borrow keeps live for `'a`.
    fn from(record: &'a mut PoolRecord) -> Self {
        Self {
            raw: record,
            live: core::marker::PhantomData,
        }
    }
}

impl<'a> From<&'a core::cell::UnsafeCell<PoolRecord>> for PoolRef<'a> {
    /// The record a cell keeps live for `'a`: its owner reaches it only through
    /// handles like this one, which read and write a field at a time.
    fn from(record: &'a core::cell::UnsafeCell<PoolRecord>) -> Self {
        Self {
            raw: record.get(),
            live: core::marker::PhantomData,
        }
    }
}

impl<'a> PoolRef<'a> {
    /// Keeps a native pipe owner until this pool closes. False before its pipe
    /// list is created; then `pipe` drops immediately.
    #[must_use]
    pub fn keep_pipe(self, pipe: Box<dyn std::any::Any>) -> bool {
        self.record()
            .pipes
            .with_box(|pipes| pipes.push(pipe))
            .is_some()
    }

    fn set_pipes(self, pipes: Option<Box<Pipes>>) {
        drop(self.record().pipes.replace(pipes));
    }

    /// Whether the pool owns its native message bus.
    #[must_use]
    pub fn has_message_bus(self) -> bool {
        self.record().has_message_bus()
    }

    /// Lends the native bus to one operation, taken out of the pool meanwhile: no
    /// reference to it escapes, and the pool has no bus to drop until it returns.
    pub fn with_message_bus<R>(self, f: impl FnOnce(&dyn std::any::Any) -> R) -> Option<R> {
        self.record().message_bus.with_box(|bus| f(bus.as_ref()))
    }

    pub(crate) fn set_message_bus(self, bus: Option<Box<dyn std::any::Any>>) {
        drop(self.record().message_bus.replace(bus.map(Box::new)));
    }

    /// One operation on the reload in progress, with no borrow across callbacks:
    /// the task is out of the pool until `f` returns, so nothing else reaches it.
    pub fn with_reload_task<R>(self, f: impl FnOnce(&mut ReloadTask) -> R) -> Option<R> {
        self.record().reload_task.with_box(f)
    }

    /// Arms the reload's timeout timer, then gives it to the task to cancel on
    /// drop; nothing while no reload is in progress. The task is out of the pool
    /// while `arm` runs. The timer keeps no pointer into the task: when it fires,
    /// it reaches the task through the pool ([`Self::with_reload_task`]).
    pub fn arm_reload_timer(self, arm: impl FnOnce() -> Option<ArmedTimer>) {
        self.record()
            .reload_task
            .with_box(|task| task.set_timer(arm()));
    }

    fn set_reload_task(self, task: Option<Box<ReloadTask>>) {
        drop(self.record().reload_task.replace(task));
    }

    /// The pool's worker at `raw` ([`PoolRecord::own`]), or `None` when `raw` is
    /// none of its records.
    #[must_use]
    pub fn worker_of(self, raw: *const WorkerRecord) -> Option<Worker<'a>> {
        // The pool's records live as long as it does.
        self.record().own(raw)
    }

    /// The index of `worker` among the pool's workers, or `None` when it is none of
    /// them: [`PoolRef::worker`] gives it back.
    #[must_use]
    pub fn index_of(self, worker: Worker<'_>) -> Option<usize> {
        self.record().index_of(worker.raw())
    }

    /// The pool at `raw`.
    ///
    /// # Safety
    /// `raw` is a live pool for `'a`.
    #[must_use]
    pub unsafe fn new(raw: *mut PoolRecord) -> Self {
        Self {
            raw,
            live: core::marker::PhantomData,
        }
    }

    /// The pool's address.
    #[must_use]
    pub fn raw(self) -> *mut PoolRecord {
        self.raw
    }

    /// Resets the pool to [`PoolRecord::empty`], as C's `*pool = {}`: what it held
    /// before is forgotten, not freed. An empty pool's fields are valid values for
    /// every handle that reads them.
    pub fn reset(self) {
        reset!(
            self;
            running, reload_init, read_message, started, schedule_by_sysvmsg, is_async,
            ipc_mode, protocol_type, master_pid, max_wait_time, reload_count,
            reload_last_time, kind, start_id, use_msgqueue, use_socket, max_packet_size,
            msgqueue_key, worker_num, max_request, max_request_grace, scheduler_warning,
            warning_time, on_start, on_shutdown, on_before_reload, on_after_reload, on_task,
            on_worker_start, on_message, on_worker_exit, on_worker_stop, on_worker_error,
            on_worker_message, on_worker_not_found, main_loop, workers, pipes, ipc,
            message_bus, reload_task, ptr, kill_timer, ptr_key;
            registry, queue, message_box
        );
    }

    /// Puts `value` in the owner field `field` projects from the record (its
    /// registry, queue or message box, which the record lends in place), and
    /// returns what was there. `field` is a plain fn, which cannot reach the pool to
    /// borrow it again. Every other field is a cell, written through `&PoolRecord`.
    fn replace<T>(self, field: fn(&mut PoolRecord) -> &mut T, value: T) -> T {
        // SAFETY: `new`'s contract: a live pool for `'a`, borrowed only for the
        // projection and the swap. The owner fields are written only by `create`,
        // `destroy` and `reset`, which no loan of them (`with_registry`,
        // `with_queue`, the message box's) spans.
        core::mem::replace(field(unsafe { &mut *self.raw }), value)
    }

    /// Sets the pool's registry.
    pub(crate) fn set_registry(self, registry: Option<Box<WorkerRegistry>>) {
        drop(self.replace(|record| &mut record.registry, registry));
    }

    /// Sets the pool's message queue.
    pub(crate) fn set_queue(self, queue: Option<Box<crate::ossupport::QueueRecord>>) {
        drop(self.replace(|record| &mut record.queue, queue));
    }

    /// Sets the pool's message box.
    pub(crate) fn set_message_box(self, message_box: SharedChannel) {
        _ = self.replace(|record| &mut record.message_box, message_box);
    }
}

impl<'a> PoolRef<'a> {
    /// The pool's message box (a shared-memory channel), or null.
    #[must_use]
    pub fn message_box(self) -> *mut c_void {
        self.record().message_box()
    }

    /// Takes the pool's message box out, leaving none.
    pub(crate) fn take_message_box(self) -> SharedChannel {
        self.replace(|record| &mut record.message_box, SharedChannel::none())
    }

    /// The record, for this crate's operations, which read it in place.
    pub(crate) fn record(self) -> &'a PoolRecord {
        // SAFETY: `new`'s contract: a live pool for `'a`.
        unsafe { &*self.raw }
    }

    /// The pool's message queue's message and byte counts (`msgctl(IPC_STAT)`),
    /// or `None` for a pool without one or a failed stat.
    #[must_use]
    pub fn queue_stat(self) -> Option<(usize, usize)> {
        self.with_queue(crate::ossupport::QueueRecord::stat)?
    }

    /// The pool's message queue's id (`msgget`'s), 0 without one.
    #[must_use]
    pub fn queue_id(self) -> c_int {
        self.with_queue(crate::ossupport::QueueRecord::id)
            .unwrap_or(0)
    }

    /// `queue->pop(node, size)` into a `QueueNode` (a `long` type, then room for
    /// `size` payload bytes), and `errno` right after it; -1 when it failed or there
    /// is no queue.
    pub(crate) fn queue_pop(self, node: &mut [u8], size: usize) -> (isize, c_int) {
        let node = node.get_mut(..size_of::<c_long>().saturating_add(size));
        let result = self.with_queue(|queue| queue.pop(node?)).flatten();
        let length = result.map_or(-1, CCast::c_cast::<isize>);
        (length, crate::sys::errno())
    }

    /// `queue->push(node)` of a `QueueNode`, a `long` type and then its payload;
    /// false without a queue.
    pub(crate) fn queue_push(self, node: &[u8]) -> bool {
        self.with_queue(|queue| queue.push(node)).unwrap_or(false)
    }

    /// `f` over the pool's message queue; `None` without one. Only this type's queue
    /// reads borrow it, and none of them reaches `destroy`, which frees it.
    fn with_queue<R>(self, f: impl FnOnce(&crate::ossupport::QueueRecord) -> R) -> Option<R> {
        self.record().queue.as_deref().map(f)
    }

    /// The registry `create` made, or null.
    #[must_use]
    pub fn registry(self) -> *mut WorkerRegistry {
        self.record().registry()
    }

    /// The `SysV` message queue of a message-queue pool, or null.
    #[must_use]
    pub fn queue(self) -> *mut c_void {
        self.record().queue()
    }

    /// The pool's worker at `index`, or `None` past `worker_num`.
    #[must_use]
    pub fn worker(self, index: usize) -> Option<Worker<'a>> {
        self.record().worker(index)
    }

    /// The pool's stream listener, which its IPC owns until `destroy` frees it, or
    /// `None` without one.
    #[must_use]
    pub fn ipc_listener(self) -> Option<crate::process::pool::Connection> {
        self.record().ipc_listener()
    }

    /// The stream connection being served, which the worker loop frees once it is
    /// answered, or `None` without one.
    #[must_use]
    pub fn ipc_connection(self) -> Option<crate::process::pool::Connection> {
        self.record().ipc_connection()
    }

    /// The event loop's receive buffer, taken out until [`Self::restore_ipc_packet`];
    /// empty before the loop allocates it, or while it is taken.
    #[must_use]
    pub fn take_ipc_packet(self) -> Vec<u8> {
        self.record()
            .with_ipc(crate::process::pool::pool_ipc_take_packet)
            .unwrap_or_default()
    }

    /// Give back the buffer [`Self::take_ipc_packet`] took.
    pub fn restore_ipc_packet(self, packet: Vec<u8>) {
        self.record()
            .with_ipc(|ipc| crate::process::pool::pool_ipc_restore_packet(ipc, packet));
    }
}

/// `swoole::RecvData`: a message's head and its bytes, for one callback: lent by
/// their holder, or a packet the callback owns and may take
/// ([`RecvData::take_packet`]), freed with it otherwise.
pub struct RecvData<'a> {
    info: DataHead,
    payload: Payload<'a>,
}

enum Payload<'a> {
    Lent(&'a [u8]),
    Buffer(&'a mut crate::buffer::string::CppString),
    Owned(crate::buffer::string::OwnedString),
}

impl<'a> RecvData<'a> {
    /// A message of `data` (at most `u32::MAX` bytes of it), its head's `len` their
    /// length.
    #[must_use]
    #[inline]
    pub fn new(info: DataHead, data: &'a [u8]) -> Self {
        let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let data = data.get(..len as usize).unwrap_or(data);
        Self {
            info: DataHead { len, ..info },
            payload: Payload::Lent(data),
        }
    }

    /// A message of `info` as it is, over at most its `len` bytes of `data`.
    #[must_use]
    #[inline]
    pub fn within(info: DataHead, data: &'a [u8]) -> Self {
        let data = data.get(..info.len as usize).unwrap_or(data);
        Self {
            info,
            payload: Payload::Lent(data),
        }
    }

    /// The complete receive buffer, whose allocation a local worker may take.
    #[must_use]
    pub fn buffer(info: DataHead, buffer: &'a mut crate::buffer::string::CppString) -> Self {
        Self {
            info,
            payload: Payload::Buffer(buffer),
        }
    }

    /// A message of `packet`, which the callback owns, its head's `len` the packet's
    /// length (at most `u32::MAX`).
    #[must_use]
    pub fn owned(info: DataHead, packet: crate::buffer::string::OwnedString) -> Self {
        let len = u32::try_from(packet.len()).unwrap_or(u32::MAX);
        Self {
            info: DataHead { len, ..info },
            payload: Payload::Owned(packet),
        }
    }

    #[must_use]
    #[inline]
    pub fn info(&self) -> &DataHead {
        &self.info
    }

    /// The message's bytes, at most its head's `len`; none once
    /// [`RecvData::take_packet`] took them.
    #[must_use]
    #[inline]
    pub fn payload(&self) -> &[u8] {
        match &self.payload {
            // `new` keeps no more than `len`.
            Payload::Lent(data) => data,
            Payload::Buffer(buffer) => buffer
                .view()
                .get(..self.info.len as usize)
                .unwrap_or(buffer.view()),
            Payload::Owned(packet) => {
                let bytes = packet.view();
                bytes.get(..self.info.len as usize).unwrap_or(bytes)
            }
        }
    }

    /// The head's `flags`.
    #[inline]
    pub fn set_flags(&mut self, flags: u8) {
        self.info.flags = flags;
    }

    /// `move_packet()`: the packet this message owns, now the caller's, or `None`
    /// when its bytes are lent.
    pub fn take_packet(&mut self) -> Option<crate::buffer::string::OwnedString> {
        if let Payload::Buffer(buffer) = &mut self.payload {
            let packet = buffer.pop_owned(buffer.size())?;
            self.payload = Payload::Lent(&[]);
            return Some(packet);
        }
        match core::mem::replace(&mut self.payload, Payload::Lent(&[])) {
            Payload::Owned(packet) => Some(packet),
            lent => {
                self.payload = lent;
                None
            }
        }
    }
}

/// The pool's callbacks. Each takes the live pool and the objects of the event,
/// which the pool lends for the call.
///
/// A hook runs only through the pool that holds it ([`PoolRef::call_on_start`] and
/// the calls beside it), and only its owner installs it ([`PoolRef::install`]). So a
/// hook whose other arguments are handles and references is a safe fn: the pool it
/// is given is the one it was installed on.
pub type PoolFn = fn(PoolRef<'_>);
/// `onWorkerStart`, `onWorkerStop`, `onWorkerExit`.
pub type WorkerFn = fn(PoolRef<'_>, Worker<'_>);
/// `main_loop`: the worker's loop, and its exit code.
pub type MainLoopFn = fn(PoolRef<'_>, Worker<'_>) -> c_int;
/// `onTask`: the task the worker read.
pub type TaskFn = fn(PoolRef<'_>, Worker<'_>, Packed<'_>) -> c_int;
/// `onMessage`: the message, lent for the call.
pub type MessageFn = fn(PoolRef<'_>, &mut RecvData<'_>);
/// `onWorkerMessage`: the `EventData` the master popped, as its bytes.
pub type EventFn = fn(PoolRef<'_>, &[u8; crate::constants::SW_IPC_MAX_SIZE as usize]);
/// `onWorkerError`.
pub type ErrorFn = fn(PoolRef<'_>, Worker<'_>, &crate::process::methods::ExitStatus);
/// `onWorkerNotFound`.
pub type NotFoundFn = fn(PoolRef<'_>, &crate::process::methods::ExitStatus);

/// What a pool's owner installs ([`PoolRef::install`]): its hooks. `None` leaves what
/// the pool has.
#[derive(Clone, Copy, Default)]
pub struct Hooks {
    pub on_start: Option<PoolFn>,
    pub on_shutdown: Option<PoolFn>,
    pub on_before_reload: Option<PoolFn>,
    pub on_after_reload: Option<PoolFn>,
    pub on_task: Option<TaskFn>,
    pub on_worker_start: Option<WorkerFn>,
    pub on_message: Option<MessageFn>,
    pub on_worker_exit: Option<WorkerFn>,
    pub on_worker_stop: Option<WorkerFn>,
    pub on_worker_error: Option<ErrorFn>,
    pub on_worker_message: Option<EventFn>,
    pub on_worker_not_found: Option<NotFoundFn>,
    pub main_loop: Option<MainLoopFn>,
}

impl PoolRef<'_> {
    /// Installs `hooks` on the pool. A hook takes handles and what the pool lends
    /// it, so it runs soundly on any pool.
    pub fn install(self, hooks: &Hooks) {
        // One write per field named: the pool's own code and its signal handlers
        // read the record meanwhile.
        if hooks.on_start.is_some() {
            self.set_on_start(hooks.on_start);
        }
        if hooks.on_shutdown.is_some() {
            self.set_on_shutdown(hooks.on_shutdown);
        }
        if hooks.on_before_reload.is_some() {
            self.set_on_before_reload(hooks.on_before_reload);
        }
        if hooks.on_after_reload.is_some() {
            self.set_on_after_reload(hooks.on_after_reload);
        }
        if hooks.on_task.is_some() {
            self.set_on_task(hooks.on_task);
        }
        if hooks.on_worker_start.is_some() {
            self.set_on_worker_start(hooks.on_worker_start);
        }
        if hooks.on_message.is_some() {
            self.set_on_message(hooks.on_message);
        }
        if hooks.on_worker_exit.is_some() {
            self.set_on_worker_exit(hooks.on_worker_exit);
        }
        if hooks.on_worker_stop.is_some() {
            self.set_on_worker_stop(hooks.on_worker_stop);
        }
        if hooks.on_worker_error.is_some() {
            self.set_on_worker_error(hooks.on_worker_error);
        }
        if hooks.on_worker_message.is_some() {
            self.set_on_worker_message(hooks.on_worker_message);
        }
        if hooks.on_worker_not_found.is_some() {
            self.set_on_worker_not_found(hooks.on_worker_not_found);
        }
        if hooks.main_loop.is_some() {
            self.set_main_loop(hooks.main_loop);
        }
    }

    /// `pool->ptr = object`, held under `key`: [`Self::object_of`] with `key` returns
    /// it until the next `hold` or [`Self::reset`]. The object first, then its key, so
    /// a reader that sees the key sees the object.
    pub fn hold(self, key: &'static PoolObjectKey, object: *mut c_void) {
        self.set_ptr(object);
        self.set_ptr_key(key);
    }

    /// `pool->ptr`, when the pool holds it under `key` ([`Self::hold`]).
    #[inline]
    #[must_use]
    pub fn object_of(self, key: &'static PoolObjectKey) -> Option<*mut c_void> {
        ptr::eq(self.ptr_key(), key).then(|| self.ptr())
    }

    /// `pool->main_loop = nullptr`: the pool's workers run no loop of its own.
    pub fn clear_main_loop(self) {
        self.set_main_loop(None);
    }

    /// `hook(pool)` when the pool has it.
    fn call_pool(self, hook: Option<PoolFn>) {
        if let Some(hook) = hook {
            hook(self);
        }
    }

    /// `hook(pool, worker)` when the pool has it.
    fn call_worker(self, hook: Option<WorkerFn>, worker: Worker<'_>) {
        if let Some(hook) = hook {
            hook(self, worker);
        }
    }

    /// `onStart(pool)`; nothing without one.
    pub fn call_on_start(self) {
        self.call_pool(self.on_start());
    }

    /// `onShutdown(pool)`; nothing without one.
    pub fn call_on_shutdown(self) {
        self.call_pool(self.on_shutdown());
    }

    /// `onBeforeReload(pool)`; nothing without one.
    pub fn call_on_before_reload(self) {
        self.call_pool(self.on_before_reload());
    }

    /// `onAfterReload(pool)`; nothing without one.
    pub fn call_on_after_reload(self) {
        self.call_pool(self.on_after_reload());
    }

    /// `onWorkerStart(pool, worker)`; nothing without one.
    pub fn call_on_worker_start(self, worker: Worker<'_>) {
        self.call_worker(self.on_worker_start(), worker);
    }

    /// `onWorkerStop(pool, worker)`; nothing without one.
    pub fn call_on_worker_stop(self, worker: Worker<'_>) {
        self.call_worker(self.on_worker_stop(), worker);
    }

    /// `onMessage(pool, message)`; nothing without one.
    pub fn call_on_message(self, message: &mut RecvData<'_>) {
        if let Some(hook) = self.on_message() {
            hook(self, message);
        }
    }

    /// `onWorkerExit(pool, worker)`; nothing without one.
    pub fn call_on_worker_exit(self, worker: Worker<'_>) {
        self.call_worker(self.on_worker_exit(), worker);
    }

    /// `main_loop(pool, worker)`: its exit code, or `None` without one.
    #[expect(
        clippy::must_use_candidate,
        reason = "callers ignore the result where upstream's do"
    )]
    pub fn call_main_loop(self, worker: Worker<'_>) -> Option<c_int> {
        Some(self.main_loop()?(self, worker))
    }

    /// Whether the pool has a `main_loop`.
    #[must_use]
    pub fn has_main_loop(self) -> bool {
        self.main_loop().is_some()
    }

    /// `onTask(pool, worker, task)`: its result, or `None` without one.
    #[expect(
        clippy::must_use_candidate,
        reason = "callers ignore the result where upstream's do"
    )]
    pub fn call_on_task(self, worker: Worker<'_>, task: Packed<'_>) -> Option<c_int> {
        Some(self.on_task()?(self, worker, task))
    }

    /// `onWorkerError(pool, worker, status)`; nothing without one.
    pub fn call_on_worker_error(
        self,
        worker: Worker<'_>,
        status: &crate::process::methods::ExitStatus,
    ) {
        if let Some(hook) = self.on_worker_error() {
            hook(self, worker, status);
        }
    }

    /// `onWorkerMessage(pool, message)`: whether the pool has one.
    #[expect(
        clippy::must_use_candidate,
        reason = "the master's loop reads it; the native op does not"
    )]
    pub fn call_on_worker_message(
        self,
        message: &[u8; crate::constants::SW_IPC_MAX_SIZE as usize],
    ) -> bool {
        let Some(hook) = self.on_worker_message() else {
            return false;
        };
        hook(self, message);
        true
    }

    /// `onWorkerNotFound(pool, status)`; nothing without one.
    pub fn call_on_worker_not_found(self, status: &crate::process::methods::ExitStatus) {
        if let Some(hook) = self.on_worker_not_found() {
            hook(self, status);
        }
    }
}

/// Failure of a pool's pipe or message-bus operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeError {
    MissingWorker,
    Open,
    Closed,
    Buffer,
    Register,
    StopMessage,
}

/// What every export runs over (`crates/server/src/pool_native.rs`): the pool's
/// native objects, which this crate can't depend on, and its callbacks. Each takes
/// a [`PoolCall`] this crate filled over live objects.
pub trait PoolNative: Sync {
    /// One effect on the call's pool and worker.
    fn call(&self, call: &PoolCall, op: Op, arg: u64);
    /// `swoole_event_init`: this thread's event loop, run for the call's pool;
    /// false when it can't be made.
    fn event_init(&self, call: &PoolCall) -> bool;
    /// `swoole_event_is_available()`.
    fn event_available(&self) -> bool;
    /// `swoole_event_wait()`: false when the loop failed.
    fn event_wait(&self) -> bool;
    /// `swoole_waitpid(pid)`: false when it failed.
    fn waitpid(&self, pid: c_int) -> bool;
    /// Create and retain the worker's datagram pipe.
    /// # Errors
    /// Missing worker, pipe creation failure, or closed pool ownership.
    fn new_pipe(&self, call: &PoolCall) -> Result<(), PipeError>;
    /// The master pipe's kernel send-buffer size, retaining getsockopt's errno.
    /// # Errors
    /// The descriptor or getsockopt failure, with its captured errno.
    fn send_buffer_size(&self, call: &PoolCall) -> std::io::Result<c_int>;
    /// Allocate the pool's message-bus buffer.
    /// # Errors
    /// Missing message bus or failed allocation.
    fn bus_buffer(&self, call: &PoolCall, size: usize) -> Result<(), PipeError>;
    /// Register the worker pipe for reads.
    /// # Errors
    /// Missing pipe or failed reactor registration.
    fn add_pipe(&self, call: &PoolCall) -> Result<(), PipeError>;
    /// Publish the worker's stop message to the pool.
    /// # Errors
    /// The stop message could not be queued.
    fn push_stop(&self, call: &PoolCall) -> Result<(), PipeError>;
    /// `count` workers from the global memory pool, or null.
    fn alloc_workers(&self, count: u32) -> *mut WorkerRecord;
    /// A new native message bus, owned by the pool, or None on allocation failure.
    fn new_message_bus(&self) -> Option<Box<dyn std::any::Any>>;
    /// The pool's `callback`; it runs inside the trap.
    fn callback(&self, call: &mut PoolCall, callback: Callback);
    /// `sizeof(EventData)`.
    fn event_size(&self) -> usize;
    /// `ProcessPool::is_worker_running(worker)`.
    fn running(&self, call: &PoolCall) -> bool;
    /// Run the expired timers and dispatch the pending signals.
    fn end(&self);
    /// The stream listener's `accept()`: the connection, set with
    /// [`PoolCall::set_connection`].
    fn accept(&self, call: &mut PoolCall);
    /// `socket->recv_sync(data, length, flags)` on the call's connection.
    fn recv(&self, call: &mut PoolCall);
    /// `socket->send_sync(data, length)` on the call's connection.
    fn send(&self, call: &mut PoolCall);
    /// `socket->free()` of the call's connection (or the listener), which is not
    /// used again.
    fn close(&self, call: &mut PoolCall);
    /// The worker's pipe, with the next timer as its read timeout: `read_sync`.
    fn pipe_read(&self, call: &mut PoolCall);
    /// `pipe_worker->wait_event(next timer, SW_EVENT_READ)`.
    fn pipe_wait(&self, call: &mut PoolCall);
    /// `message_bus->read(pipe_worker)`.
    fn bus_read(&self, call: &mut PoolCall);
    /// `onMessage` with the bus's packet, then `message_bus->pop()`.
    fn bus_deliver(&self, call: &mut PoolCall);
    /// `message_bus->write(worker->pipe_master, {head, message})` over `pool`'s
    /// bus, which it has, `head.len` set to the message's length: whether the whole
    /// message went. False for a worker without a pipe.
    fn bus_write(
        &self,
        pool: PoolRef<'_>,
        worker: Worker<'_>,
        head: &mut crate::DataHead,
        message: &[u8],
    ) -> bool;
    /// `onTask(pool, worker, data)`.
    fn task(&self, call: &mut PoolCall);
    /// `onMessage` with `length` bytes at `data`.
    fn message(&self, call: &mut PoolCall);
    /// `worker->shutdown()`.
    fn shutdown(&self, call: &PoolCall);
    /// `socket` (a worker's pipe): `swoole_event_write` when the event loop is
    /// available and `flags` asks for it, else `send_sync`.
    fn pipe_write(&self, call: &mut PoolCall);
    /// Report a worker loop's warning.
    fn warn(&self, call: &PoolCall, warning: crate::process::pool::Warning);
}

/// `swIPCMode`: how a pool's master reaches its workers.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcMode {
    None = 0,
    UnixSock = 1,
    MsgQueue = 2,
    Socket = 3,
}

impl IpcMode {
    /// The mode `code` names; `create` takes any other as `None`.
    #[must_use]
    pub const fn from_code(code: i64) -> Self {
        match code {
            1 => Self::UnixSock,
            2 => Self::MsgQueue,
            3 => Self::Socket,
            _ => Self::None,
        }
    }

    /// The C++ enum's value.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// `swoole::ProtocolType`: how a pool's workers receive, so which loop they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Protocol {
    Task = 1,
    Stream = 2,
    Message = 3,
}

impl Protocol {
    /// The protocol of a native `ProtocolType` code.
    #[must_use]
    pub const fn from_code(code: c_int) -> Option<Self> {
        match code {
            1 => Some(Self::Task),
            2 => Some(Self::Stream),
            3 => Some(Self::Message),
            _ => None,
        }
    }

    /// The native `ProtocolType` code.
    #[must_use]
    pub const fn code(self) -> c_int {
        self as c_int
    }
}

/// `Op::MainLoop`'s `run_async`, after the protocols' own codes.
const MAIN_ASYNC: u64 = 4;

/// `swoole_config.h`; native asserts each.
pub const SW_MESSAGE_BOX_SIZE: u64 = 65536;
pub const SW_INPUT_BUFFER_SIZE: u32 = 2 * 1024 * 1024;
pub const SW_WORKER_MAX_WAIT_TIME: u32 = 3;
/// `SW_MSGMAX`: a `SysV` message's fixed receive limit.
pub const SW_MSGMAX: u32 = 65536;

/// `set_max_packet_size`'s value: a `SysV` message queue keeps its fixed receive
/// limit and ignores `max_package_size`.
#[must_use]
pub const fn packet_limit(use_msgqueue: bool, size: u32) -> u32 {
    if use_msgqueue { SW_MSGMAX } else { size }
}

/// `max_package_size`'s `swoole::clamp<int64_t>(v, 1, UINT32_MAX)`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the last arm's value is within 1..=u32::MAX"
)]
pub const fn max_package_size(value: i64) -> u32 {
    if value < 1 {
        1
    } else if value > u32::MAX as i64 {
        u32::MAX
    } else {
        value as u32
    }
}
#[cfg(target_os = "macos")]
pub const SW_IPC_MAX_SIZE: u64 = 2048;
#[cfg(not(target_os = "macos"))]
pub const SW_IPC_MAX_SIZE: u64 = 8192;
#[cfg(target_os = "linux")]
pub const SW_IPC_BUFFER_MAX_SIZE: i64 = 64 * 1024;

/// `SW_WORKER_MESSAGE_STOP`, and where an `EventData` keeps its type and data.
const WORKER_MESSAGE_STOP: u8 = 1;
const EVENT_LEN: usize = 16;
const EVENT_TYPE: usize = 22;
const EVENT_DATA: usize = 40;
const HEAD_SIZE: u32 = 40;
const _: () = assert!(
    size_of::<DataHead>() == EVENT_DATA,
    "DataHead heads an EventData"
);

/// `head`'s bytes, in its C layout: each field's native bytes at its offset.
#[must_use]
#[inline]
pub fn head_view(head: &DataHead) -> [u8; EVENT_DATA] {
    use core::mem::offset_of;
    // Every byte of a `DataHead` is a field's: its fields end where the next begins.
    const _: () = assert!(
        core::mem::offset_of!(DataHead, time) + size_of::<f64>() == EVENT_DATA
            && core::mem::offset_of!(DataHead, msg_id) == 8
            && core::mem::offset_of!(DataHead, len) == 16
            && core::mem::offset_of!(DataHead, reactor_id) == 20
            && core::mem::offset_of!(DataHead, kind) == 22
            && core::mem::offset_of!(DataHead, flags) == 23
            && core::mem::offset_of!(DataHead, server_fd) == 24
            && core::mem::offset_of!(DataHead, ext_flags) == 26
            && core::mem::offset_of!(DataHead, reserved) == 28
            && core::mem::offset_of!(DataHead, time) == 32,
        "DataHead has no padding"
    );
    let fields: [(usize, &[u8]); 10] = [
        (offset_of!(DataHead, fd), &head.fd.to_ne_bytes()),
        (offset_of!(DataHead, msg_id), &head.msg_id.to_ne_bytes()),
        (offset_of!(DataHead, len), &head.len.to_ne_bytes()),
        (
            offset_of!(DataHead, reactor_id),
            &head.reactor_id.to_ne_bytes(),
        ),
        (offset_of!(DataHead, kind), &[head.kind]),
        (offset_of!(DataHead, flags), &[head.flags]),
        (
            offset_of!(DataHead, server_fd),
            &head.server_fd.to_ne_bytes(),
        ),
        (
            offset_of!(DataHead, ext_flags),
            &head.ext_flags.to_ne_bytes(),
        ),
        (offset_of!(DataHead, reserved), &head.reserved.to_ne_bytes()),
        (offset_of!(DataHead, time), &head.time.to_ne_bytes()),
    ];
    let mut bytes = [0; EVENT_DATA];
    for (at, field) in fields {
        if let Some(to) = bytes.get_mut(at..at + field.len()) {
            to.copy_from_slice(field);
        }
    }
    bytes
}

/// A pool effect, which `crates/server/src/pool_native.rs` answers; the process-wide
/// worker and signal effects run here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    #[cfg(test)]
    HaveMsgqueue,
    AllocWorkers,
    NewMessageBox,
    #[cfg(test)]
    NewPipe,
    MainLoop,
    NewMessageBus,
    #[cfg(test)]
    Sndbuf,
    #[cfg(test)]
    BusBuffer,
    WorkerType,
    UnlimitPipes,
    #[cfg(test)]
    Fork,
    WorkerShutdown,
    #[cfg(test)]
    EventAvailable,
    RemovePipe,
    ExitCallback,
    ReopenLogger,
    #[cfg(test)]
    IsMaster,
    ReloadTimer,
    DontRestart,
    TimerInit,
    TimerFree,
    #[cfg(test)]
    AddPipe,
    PipeHandler,
    #[cfg(test)]
    EventWait,
    CurrentWorker,
    #[cfg(test)]
    PushStop,
    SignalDispatch,
    TimerSelect,
    OnWorkerNotFound,
    WorkerError,
    OnAfterReload,
    KillTimer,
    #[cfg(test)]
    Waitpid,
    FreeMessageBox,
    FreeWorkers,
    WorkerInit,
    #[cfg(test)]
    EventInit,
    BoxPush,
    /// A warning, which [`Native`] logs itself.
    Log(Log),
}

/// The warnings a pool operation logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Log {
    NoSysvmsg,
    Alloc,
    NeedsUnixsock,
    BusExists,
    MustListen,
    UnknownChild,
    WaitFailed,
    KillFailed,
    WaitpidFailed,
}

/// One of the pool's callbacks, or its worker loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callback {
    Start,
    BeforeReload,
    Shutdown,
    WorkerStart,
    MainLoop,
    WorkerStop,
}

/// What a pool operation asks of native code.
pub(crate) trait PoolHost {
    fn op(&mut self, worker: *mut c_void, op: Op, arg: u64);
    /// The platform has System V message queues.
    fn have_msgqueue(&mut self) -> bool;
    /// This thread's event loop for the pool's worker: false when it can't be made.
    fn event_init(&mut self) -> bool;
    fn event_available(&mut self) -> bool;
    /// `swoole_event_wait()`: false when the loop failed.
    fn event_wait(&mut self) -> bool;
    /// This process is the pool's master.
    fn is_master(&mut self) -> bool;
    /// `swoole_fork(0)` for `worker`: 0 in the child, the child's pid in the parent,
    /// or `None` when it failed.
    fn fork(&mut self, worker: *mut WorkerRecord) -> Option<c_int>;
    /// `swoole_waitpid(pid)`: false when it failed.
    fn waitpid(&mut self, pid: c_int) -> bool;
    fn new_pipe(&mut self, worker: *mut WorkerRecord) -> Result<(), PipeError>;
    #[cfg(target_os = "linux")]
    fn send_buffer_size(&mut self, worker: *mut WorkerRecord) -> std::io::Result<c_int>;
    fn bus_buffer(&mut self, size: usize) -> Result<(), PipeError>;
    fn add_pipe(&mut self, worker: *mut WorkerRecord) -> Result<(), PipeError>;
    fn push_stop(&mut self, worker: *mut WorkerRecord) -> Result<(), PipeError>;
    /// Run `callback` inside the trap: its result, or [`Bailed`].
    fn callback(&mut self, callback: Callback, worker: *mut c_void) -> Result<isize, Bailed>;
    /// Run the protocol's worker loop: `DONE` or `BAILED`.
    fn run_loop(&mut self, pool_loop: &PoolLoop<'_>, protocol: Protocol) -> Result<(), Bailed>;
    /// The native side, which tests leave out.
    fn native(&self) -> Option<&dyn PoolNative>;
    fn warn(&self) -> Warn;
    fn event_size(&self) -> usize;
    /// The stream listener's free.
    fn close(&self, listener: crate::process::pool::Connection);
    /// `Channel::make(size, sizeof(EventData), SW_CHAN_LOCK | SW_CHAN_SHM)`: the
    /// pool's new message box, or null.
    fn new_box(&mut self, size: u64) -> SharedChannel;
    /// `message_box->push(message)`, under the box's lock: false when it fails.
    fn push_box(&mut self, message: &[u8]) -> bool;
    /// Frees the pool's message box, if it has one, and forgets it.
    fn free_box(&mut self);
    /// `count` workers for `create`, or null.
    fn alloc_workers(&mut self, count: u32) -> *mut WorkerRecord;
    /// A new native message bus, owned by the pool, or None on allocation failure.
    fn new_message_bus(&mut self) -> Option<Box<dyn std::any::Any>>;
    /// `sw_worker()`: this process's worker.
    fn current_worker(&mut self) -> *mut WorkerRecord;
}

pub(crate) struct Native<'a> {
    native: &'a dyn PoolNative,
    pool: PoolRef<'a>,
}

impl<'a> Native<'a> {
    /// A call over the pool and `worker`, when it is one of the pool's records.
    fn call(&self, worker: *const c_void) -> PoolCall<'a> {
        PoolCall::new(self.pool, self.pool.worker_of(worker.cast()))
    }
}

impl PoolHost for Native<'_> {
    fn op(&mut self, worker: *mut c_void, op: Op, arg: u64) {
        if let Op::Log(kind) = op {
            log(kind, arg);
            return;
        }
        if effect(op, arg) {
            return;
        }
        // A native effect on the live pool this export runs over.
        self.native.call(&self.call(worker), op, arg);
    }

    /// The message queue is Rust's on every platform (crates/corebase/src/ossupport).
    fn have_msgqueue(&mut self) -> bool {
        true
    }

    fn event_init(&mut self) -> bool {
        self.native.event_init(&PoolCall::new(self.pool, None))
    }

    fn event_available(&mut self) -> bool {
        self.native.event_available()
    }

    fn event_wait(&mut self) -> bool {
        self.native.event_wait()
    }

    fn is_master(&mut self) -> bool {
        crate::process::methods::worker_type() == Some(WorkerType::Master)
    }

    /// crates/corebase's fork, with its process bookkeeping.
    fn fork(&mut self, _: *mut WorkerRecord) -> Option<c_int> {
        let pid = crate::swoole_fork(0);
        (pid >= 0).then_some(pid)
    }

    fn waitpid(&mut self, pid: c_int) -> bool {
        self.native.waitpid(pid)
    }

    fn new_pipe(&mut self, worker: *mut WorkerRecord) -> Result<(), PipeError> {
        self.native.new_pipe(&self.call(worker.cast()))
    }

    #[cfg(target_os = "linux")]
    fn send_buffer_size(&mut self, worker: *mut WorkerRecord) -> std::io::Result<c_int> {
        self.native.send_buffer_size(&self.call(worker.cast()))
    }

    fn bus_buffer(&mut self, size: usize) -> Result<(), PipeError> {
        self.native
            .bus_buffer(&PoolCall::new(self.pool, None), size)
    }

    fn add_pipe(&mut self, worker: *mut WorkerRecord) -> Result<(), PipeError> {
        self.native.add_pipe(&self.call(worker.cast()))
    }

    fn push_stop(&mut self, worker: *mut WorkerRecord) -> Result<(), PipeError> {
        self.native.push_stop(&self.call(worker.cast()))
    }

    fn callback(&mut self, callback: Callback, worker: *mut c_void) -> Result<isize, Bailed> {
        let mut call = self.call(worker);
        crate::process::pool::trapped(|| self.native.callback(&mut call, callback))?;
        Ok(call.result)
    }

    fn run_loop(&mut self, pool_loop: &PoolLoop<'_>, protocol: Protocol) -> Result<(), Bailed> {
        pool_run(self.native, pool_loop, protocol)
    }

    fn native(&self) -> Option<&dyn PoolNative> {
        Some(self.native)
    }

    fn warn(&self) -> Warn {
        crate::process::methods::reload_warn
    }

    fn event_size(&self) -> usize {
        self.native.event_size()
    }

    fn close(&self, listener: crate::process::pool::Connection) {
        let mut call = PoolCall::new(self.pool, None);
        call.connection = Some(listener);
        self.native.close(&mut call);
    }

    fn new_box(&mut self, size: u64) -> SharedChannel {
        let size = usize::try_from(size).unwrap_or(usize::MAX);
        new_message_box(size, self.native.event_size())
    }

    fn push_box(&mut self, message: &[u8]) -> bool {
        self.pool
            .record()
            .message_box_channel()
            .is_some_and(|channel| channel.push(message))
    }

    fn free_box(&mut self) {
        self.pool.take_message_box().free();
    }

    fn alloc_workers(&mut self, count: u32) -> *mut WorkerRecord {
        self.native.alloc_workers(count)
    }

    fn new_message_bus(&mut self) -> Option<Box<dyn std::any::Any>> {
        self.native.new_message_bus()
    }

    fn current_worker(&mut self) -> *mut WorkerRecord {
        crate::process::worker_globals::current_worker()
    }
}

/// `Channel::make(size, maxlen, SW_CHAN_LOCK | SW_CHAN_SHM)`: a channel in fresh
/// shared memory, or none.
fn new_message_box(size: usize, maxlen: usize) -> SharedChannel {
    use crate::log;
    /// `SW_CHAN_LOCK`.
    const LOCK: c_int = 2;
    match SharedChannel::make(size, maxlen, LOCK) {
        Ok(channel) => return channel,
        Err(ChannelInitError::NoMemory) => {
            log::warning(c"Channel::make", format_args!("alloc({size}) failed"));
        }
        Err(ChannelInitError::Pipe) => {
            log::sys_warning(c"Channel::make", format_args!("pipe() failed"));
            log::warning(c"Channel::make", format_args!("notify_fd init failed"));
        }
        Err(ChannelInitError::Mutex) => log::warning(
            c"Channel::make",
            format_args!("pthread_mutex_init() failed"),
        ),
        Err(ChannelInitError::Invalid) => {}
    }
    SharedChannel::none()
}

/// The `OP_*` effects that are calls into the process-wide worker and signal state:
/// `None` for the remaining effects supplied by the pool's caller, including its
/// timer, event-loop and wait operations.
fn effect(op: Op, arg: u64) -> bool {
    match op {
        Op::WorkerType => crate::process::methods::swoole_set_worker_type(arg.c_cast::<c_int>()),
        Op::SignalDispatch => crate::process::signal::swoole_signal_dispatch(),
        _ => return false,
    }
    true
}

/// `LOG_*`'s warning, under the `ProcessPool` method upstream logged it from.
fn log(kind: Log, arg: u64) {
    /// `SW_LOG_WARNING`, `SW_ERROR_OPERATION_NOT_SUPPORT`, `SW_ERROR_WRONG_OPERATION`.
    use crate::log::LogLevel::SW_LOG_WARNING as WARNING;
    use crate::log::{error_log, sys_warning, warning};
    const NOT_SUPPORT: c_int = swoole_rs_error::ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT as c_int;
    const WRONG_OPERATION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int;
    let pid = arg.c_cast::<c_int>();
    match kind {
        Log::NoSysvmsg => warning(
            c"ProcessPool::create",
            format_args!("current platform does not support `sysvmsg`"),
        ),
        Log::Alloc => sys_warning(
            if arg == 0 {
                c"ProcessPool::create"
            } else {
                c"ProcessPool::create_message_bus"
            },
            format_args!("malloc[1] failed"),
        ),
        Log::NeedsUnixsock => error_log(
            WARNING,
            NOT_SUPPORT,
            if arg == 0 {
                c"ProcessPool::create_message_bus"
            } else {
                c"ProcessPool::run_with_message_protocol"
            },
            format_args!("not support, ipc_mode must be SW_IPC_UNIXSOCK"),
        ),
        Log::BusExists => error_log(
            WARNING,
            WRONG_OPERATION,
            c"ProcessPool::create_message_bus",
            format_args!("the message bus has been created"),
        ),
        Log::MustListen => error_log(
            WARNING,
            WRONG_OPERATION,
            c"ProcessPool::start_check",
            format_args!("must first listen to an tcp port"),
        ),
        Log::UnknownChild => warning(
            c"ProcessPool::wait_detached_worker",
            format_args!("received an exit signal from an unknown child process[pid={pid}]"),
        ),
        Log::WaitFailed => sys_warning(c"ProcessPool::wait", format_args!("wait() failed")),
        Log::KillFailed => sys_warning(
            c"ProcessPool::wait",
            format_args!("kill({pid}, SIGTERM) failed"),
        ),
        Log::WaitpidFailed => {
            sys_warning(c"ProcessPool::wait", format_args!("waitpid({pid}) failed"));
        }
    }
}

/// One operation over a pool: its record and the native side.
pub(crate) struct Pool<'a, H> {
    h: H,
    p: PoolRef<'a>,
}

/// [`PoolRef`]'s field reads, and [`Pool`]'s over its handle. A read copies the one
/// field out, so no reference to the record outlives it.
macro_rules! getters {
    (@get $vis:vis $field:ident: $ty:ty) => {
        #[doc = concat!("The record's `", stringify!($field), "`.")]
        #[must_use]
        $vis fn $field(self) -> $ty {
            self.record().$field.get()
        }
    };
    (
        $($vis:vis $field:ident: $ty:ty),* $(,)? ;
        $($only_vis:vis $only:ident: $only_ty:ty),* $(,)?
    ) => {
        impl PoolRef<'_> {
            $(getters!(@get $vis $field: $ty);)*
            $(getters!(@get $only_vis $only: $only_ty);)*
        }
        impl<H> Pool<'_, H> {
            $(
                fn $field(&self) -> $ty {
                    self.p.$field()
                }
            )*
        }
    };
}

/// [`PoolRef`]'s field writes, and [`Pool`]'s over its handle: X/// `&PoolRecord`, dropping the old value. The fields the pool's operations trust are
/// written only from this crate.
macro_rules! setters {
    (@set $vis:vis $field:ident / $set:ident: $ty:ty) => {
        #[doc = concat!("Sets the record's `", stringify!($field), "`.")]
        $vis fn $set(self, value: $ty) {
            _ = self.record().$field.replace(value);
        }
    };
    (
        $($vis:vis $field:ident / $set:ident: $ty:ty),* ;
        $($only_vis:vis $only:ident / $only_set:ident: $only_ty:ty),* $(,)?
    ) => {
        impl PoolRef<'_> {
            $(setters!(@set $vis $field / $set: $ty);)*
            $(setters!(@set $only_vis $only / $only_set: $only_ty);)*
        }
        impl<H> Pool<'_, H> {
            $(
                fn $set(&mut self, value: $ty) {
                    self.p.$set(value);
                }
            )*
        }
    };
}

getters!(
    pub running: bool,
    pub reload_init: bool,
    pub read_message: bool,
    pub schedule_by_sysvmsg: bool,
    pub is_async: bool,
    pub ipc_mode: IpcMode,
    pub master_pid: c_int,
    pub max_wait_time: u32,
    pub reload_count: u64,
    pub kind: u8,
    pub start_id: u16,
    pub use_msgqueue: bool,
    pub use_socket: bool,
    pub max_packet_size: u32,
    pub worker_num: u32,
    // The hooks only this type's `call_*` run, on the pool that holds them.
    pub(crate) on_start: Option<PoolFn>,
    pub(crate) on_shutdown: Option<PoolFn>,
    pub(crate) on_before_reload: Option<PoolFn>,
    pub(crate) on_after_reload: Option<PoolFn>,
    pub(crate) on_worker_start: Option<WorkerFn>,
    pub(crate) on_worker_stop: Option<WorkerFn>,
    pub(crate) on_worker_exit: Option<WorkerFn>,
    pub(crate) on_worker_not_found: Option<NotFoundFn>,
    pub(crate) main_loop: Option<MainLoopFn>,
    pub(crate) on_message: Option<MessageFn>,
    pub workers: *mut WorkerRecord,
    pub ipc: *mut PoolIpc,
    ;
    ptr: *mut c_void,
    ptr_key: *const PoolObjectKey,
    pub started: bool,
    pub protocol_type: c_int,
    pub max_request: u32,
    pub max_request_grace: u32,
    pub scheduler_warning: bool,
    pub warning_time: i64,
    pub reload_last_time: i64,
    pub(crate) on_task: Option<TaskFn>,
    pub(crate) on_worker_error: Option<ErrorFn>,
    pub(crate) on_worker_message: Option<EventFn>,
);

setters!(
    pub running / set_running: bool,
    pub reload_init / set_reload_init: bool,
    pub read_message / set_read_message: bool,
    pub started / set_started: bool,
    pub ipc_mode / set_ipc_mode: IpcMode,
    pub protocol_type / set_protocol_type: c_int,
    pub master_pid / set_master_pid: c_int,
    pub max_wait_time / set_max_wait_time: u32,
    pub reload_count / set_reload_count: u64,
    pub reload_last_time / set_reload_last_time: i64,
    pub kind / set_kind: u8,
    pub start_id / set_start_id: u16,
    pub use_msgqueue / set_use_msgqueue: bool,
    pub use_socket / set_use_socket: bool,
    pub max_packet_size / set_max_packet_size: u32,
    pub msgqueue_key / set_msgqueue_key: c_int,
    pub(crate) worker_num / set_worker_num: u32,
    pub max_request / set_max_request: u32,
    pub max_request_grace / set_max_request_grace: u32,
    pub scheduler_warning / set_scheduler_warning: bool,
    pub(crate) workers / set_workers: *mut WorkerRecord,
    pub(crate) ipc / set_ipc: *mut PoolIpc,
    pub kill_timer / set_kill_timer: Option<ArmedTimer>
    ;
    pub is_async / set_is_async: bool,
    pub warning_time / set_warning_time: i64,
    pub schedule_by_sysvmsg / set_schedule_by_sysvmsg: bool,
    // The object `hold` writes under its key, and the hooks, which `install` writes
    // together.
    ptr / set_ptr: *mut c_void,
    ptr_key / set_ptr_key: *const PoolObjectKey,
    on_start / set_on_start: Option<PoolFn>,
    on_shutdown / set_on_shutdown: Option<PoolFn>,
    on_before_reload / set_on_before_reload: Option<PoolFn>,
    on_after_reload / set_on_after_reload: Option<PoolFn>,
    on_worker_error / set_on_worker_error: Option<ErrorFn>,
    on_message / set_on_message: Option<MessageFn>,
    on_worker_exit / set_on_worker_exit: Option<WorkerFn>,
    on_task / set_on_task: Option<TaskFn>,
    on_worker_start / set_on_worker_start: Option<WorkerFn>,
    on_worker_stop / set_on_worker_stop: Option<WorkerFn>,
    on_worker_message / set_on_worker_message: Option<EventFn>,
    on_worker_not_found / set_on_worker_not_found: Option<NotFoundFn>,
    main_loop / set_main_loop: Option<MainLoopFn>,
);

fn exited(pid: c_int, status: c_int) -> u64 {
    u64::from(pid.cast_unsigned()) | (u64::from(status.cast_unsigned()) << 32)
}

impl<H: PoolHost> Pool<'_, H> {
    fn op(&mut self, op: Op, arg: u64) {
        self.h.op(ptr::null_mut(), op, arg);
    }

    fn on(&mut self, worker: *mut WorkerRecord, op: Op, arg: u64) {
        self.h.op(worker.cast(), op, arg);
    }

    fn worker(&self, index: u32) -> *mut WorkerRecord {
        self.workers().wrapping_add(index as usize)
    }

    /// The worker at `index`, which must be below `worker_num`.
    fn worker_at(&self, index: u32) -> Worker<'_> {
        let worker = self.record().worker(index as usize);
        assert!(worker.is_some(), "worker {index} out of range");
        worker.unwrap_or_else(|| unreachable!())
    }

    /// The live pool this runs over.
    fn record(&self) -> &PoolRecord {
        self.p.record()
    }

    /// `f` over the pool's registry ([`PoolRecord::with_registry`]).
    fn with_registry<R>(&self, f: impl FnOnce(&WorkerRegistry) -> R) -> Option<R> {
        self.record().with_registry(f)
    }

    /// `f` over the pool's IPC state ([`PoolRecord::with_ipc`]).
    fn with_ipc<R>(&self, f: impl FnOnce(&mut PoolIpc) -> R) -> Option<R> {
        self.record().with_ipc(f)
    }

    /// `get_worker_by_pid`: the pool's worker at `pid`, or null.
    fn worker_by_pid(&self, pid: c_int) -> *mut WorkerRecord {
        self.with_registry(|registry| registry.lookup(pid))
            .map_or(ptr::null_mut(), <*mut c_void>::cast)
    }

    /// `ProcessPool::create`.
    fn create(&mut self, worker_num: u32, key: c_int, mode: u8) -> bool {
        let mode = IpcMode::from_code(mode.into());
        if mode == IpcMode::MsgQueue && !self.h.have_msgqueue() {
            self.op(Op::Log(Log::NoSysvmsg), 0);
            return false;
        }
        // The count goes with the records, so `worker_num` never counts past `workers`.
        let workers = self.h.alloc_workers(worker_num);
        if workers.is_null() {
            self.op(Op::Log(Log::Alloc), 0);
            return false;
        }
        self.set_workers(workers);
        self.set_worker_num(worker_num);
        if !self.create_message_box(SW_MESSAGE_BOX_SIZE) {
            return false;
        }
        match mode {
            IpcMode::MsgQueue => {
                self.set_use_msgqueue(true);
                self.set_msgqueue_key(key);
                let Some(queue) = crate::ossupport::QueueRecord::open(key) else {
                    return false;
                };
                self.p.set_queue(Some(queue));
            }
            IpcMode::UnixSock => {
                self.p.set_pipes(Some(Box::default()));
                for index in 0..worker_num {
                    if self.h.new_pipe(self.worker(index)).is_err() {
                        // Forget the workers' views before dropping their pipe owners.
                        for made in 0..index {
                            self.worker_at(made).clear_pipes();
                        }
                        self.p.set_pipes(None);
                        return false;
                    }
                }
            }
            IpcMode::Socket => self.set_use_socket(true),
            IpcMode::None => {}
        }
        self.p.set_registry(Some(Box::default()));
        self.set_ipc(pool_ipc_new(mode == IpcMode::Socket));
        self.set_ipc_mode(mode);
        self.set_protocol(Protocol::Task);
        self.set_max_packet_size(packet_limit(self.use_msgqueue(), SW_INPUT_BUFFER_SIZE));
        self.set_max_wait_time(SW_WORKER_MAX_WAIT_TIME);
        for index in 0..worker_num {
            self.worker_at(index).join(self.p);
        }
        true
    }

    /// `ProcessPool::create_message_box`.
    pub(crate) fn create_message_box(&mut self, size: u64) -> bool {
        let message_box = self.h.new_box(size);
        if message_box.is_none() {
            return false;
        }
        self.p.set_message_box(message_box);
        true
    }

    /// `ProcessPool::create_message_bus`.
    pub(crate) fn create_message_bus(&mut self) -> bool {
        if self.ipc_mode() != IpcMode::UnixSock {
            self.op(Op::Log(Log::NeedsUnixsock), 0);
            return false;
        }
        if self.p.has_message_bus() {
            self.op(Op::Log(Log::BusExists), 0);
            return false;
        }
        let Some(bus) = self.h.new_message_bus() else {
            self.op(Op::Log(Log::Alloc), 1);
            return false;
        };
        self.p.set_message_bus(Some(bus));
        #[cfg(target_os = "linux")]
        let size = {
            // The largest unix dgram the pipes carry.
            let sndbuf = self
                .h
                .send_buffer_size(self.worker(0))
                .map_or(SW_IPC_MAX_SIZE.cast_signed(), i64::from);
            // Negative sizes retain the same fallback as a failed getsockopt.
            let sndbuf = if sndbuf < 0 {
                SW_IPC_MAX_SIZE.cast_signed()
            } else {
                sndbuf
            };
            sndbuf.min(SW_IPC_BUFFER_MAX_SIZE).cast_unsigned()
        };
        #[cfg(not(target_os = "linux"))]
        let size = SW_IPC_MAX_SIZE;
        self.h.bus_buffer(size.c_cast::<usize>()).is_ok()
    }

    /// `ProcessPool::set_protocol`: how the workers receive (`crate::process::pool`'s loops).
    ///
    /// - `SW_PROTOCOL_TASK`: each `EventData` arrives whole, in one message; a
    ///   payload larger than its data field travels in a temporary file whose path
    ///   is sent instead, with `SW_TASK_TMPFILE` in `info.ext_flags`
    ///   (`Server::task_pack`).
    /// - `SW_PROTOCOL_MESSAGE`: an `EventData` may arrive in slices sharing one
    ///   `info.msg_id`, the first flagged `SW_EVENT_DATA_CHUNK | SW_EVENT_DATA_BEGIN`
    ///   and the last `SW_EVENT_DATA_CHUNK | SW_EVENT_DATA_END`; the message bus
    ///   joins them and `onMessage` runs once per whole message
    ///   (`MessageBus::write`/`read`).
    /// - `SW_PROTOCOL_STREAM`: a 4-byte payload length in network byte order, then
    ///   the payload; the receiver reads the length, allocates, then reads that
    ///   many bytes.
    pub(crate) fn set_protocol(&mut self, protocol: Protocol) {
        self.op(Op::MainLoop, u64::from(protocol.code().cast_unsigned()));
        self.set_protocol_type(protocol.code());
    }

    /// `ProcessPool::start_check`.
    pub(crate) fn start_check(&mut self) -> bool {
        if self.ipc_mode() == IpcMode::Socket
            && self
                .with_ipc(|ipc| swr_v1_process_pool_ipc_listener(ipc).is_none())
                .unwrap_or(true)
        {
            self.op(Op::Log(Log::MustListen), 0);
            return false;
        }
        self.set_running(true);
        self.set_started(true);
        self.set_master_pid(getpid());
        self.op(Op::WorkerType, WorkerType::Master.code().into());
        if self.is_async() {
            self.op(Op::MainLoop, MAIN_ASYNC);
        }
        for index in 0..self.worker_num() {
            let worker = self.worker_at(index);
            let id = u32::from(self.start_id()) + index;
            worker.join(self.p);
            worker.set_id(id);
            worker.set_kind_code(self.kind());
            self.on(worker.raw(), Op::UnlimitPipes, 0);
        }
        true
    }

    /// `ProcessPool::start`.
    pub(crate) fn start(&mut self) -> Result<bool, Bailed> {
        if !self.start_check() {
            return Ok(false);
        }
        if self.on_start().is_some() {
            self.h.callback(Callback::Start, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            if self.spawn(self.worker(index))?.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `ProcessPool::spawn`: fork `worker`, which runs its loop and exits; the
    /// child's pid. `None` when the fork failed or `worker` is none of the pool's
    /// ([`PoolRecord::own`]).
    pub(crate) fn spawn(&mut self, worker: *mut WorkerRecord) -> Result<Option<c_int>, Bailed> {
        let Some(old) = self.record().own(worker).map(Worker::pid) else {
            return Ok(None);
        };
        self.with_registry(|registry| registry.unregister(old));
        let pid = self.h.fork(worker).unwrap_or(-1);
        if pid == 0 {
            let Err(bailed) = self.child(worker);
            return Err(bailed);
        }
        if let Some(worker) = self.record().own(worker) {
            worker.set_pid(pid);
        }
        self.with_registry(|registry| {
            registry.register(pid, worker.cast(), 0);
        });
        Ok((pid > 0).then_some(pid))
    }

    /// The forked worker: its callbacks and loop, then `exit`.
    fn child(&mut self, worker: *mut WorkerRecord) -> Result<!, Bailed> {
        self.on(worker, Op::WorkerInit, 0);
        if crate::process::pool::bailout_pending() {
            return Err(Bailed);
        }
        if self.is_async() && !self.h.event_init() {
            if crate::process::pool::bailout_pending() {
                return Err(Bailed);
            }
            // exit(3), as upstream.
            std::process::exit(254);
        }
        if self.on_worker_start().is_some() {
            self.h.callback(Callback::WorkerStart, worker.cast())?;
        }
        let mut code = 0;
        if self.main_loop().is_some() {
            code =
                c_int::try_from(self.h.callback(Callback::MainLoop, worker.cast())?).unwrap_or(-1);
        }
        if self.on_worker_stop().is_some() {
            self.h.callback(Callback::WorkerStop, worker.cast())?;
        }
        // exit(3), as upstream.
        std::process::exit(code)
    }

    /// `ProcessPool::stop`.
    pub(crate) fn stop(&mut self, worker: *mut WorkerRecord) {
        self.on(worker, Op::WorkerShutdown, 0);
        if !self.h.event_available() {
            return;
        }
        self.on(worker, Op::RemovePipe, 1);
        if self.on_worker_exit().is_some() {
            self.on(worker, Op::ExitCallback, 0);
        }
    }

    /// `ProcessPool::detach`.
    pub(crate) fn detach(&mut self) -> bool {
        if !self.running() || self.p.message_box().is_null() {
            return false;
        }
        let worker = self.h.current_worker();
        self.on(worker, Op::WorkerShutdown, 0);
        if self.is_async() {
            self.on(worker, Op::RemovePipe, 0);
        }
        if self.h.push_stop(worker).is_err() {
            return false;
        }
        self.set_running(false);
        true
    }

    /// `Swoole\Process\Pool`'s signal handler (upstream's
    /// `process_pool_signal_handler`), in `worker` or (null) the master.
    pub(crate) fn signal(&mut self, signo: c_int, worker: *mut WorkerRecord) -> Result<(), Bailed> {
        match signo {
            libc::SIGTERM => {
                self.set_running(false);
                if !worker.is_null() {
                    self.stop(worker);
                }
            }
            libc::SIGUSR1 | libc::SIGUSR2 => {
                self.reload()?;
            }
            libc::SIGIO => self.set_read_message(true),
            libc::SIGWINCH => self.reopen_logger(),
            _ if crate::process::is_sigrtmin(signo) => self.reopen_logger(),
            _ => {}
        }
        Ok(())
    }

    /// `ProcessPool::kill_all_workers`.
    pub(crate) fn kill_all_workers(&mut self, signo: c_int) {
        for index in 0..self.worker_num() {
            _ = kill(self.worker_at(index).pid(), signo);
        }
    }

    /// `ProcessPool::reopen_logger`.
    pub(crate) fn reopen_logger(&mut self) {
        self.op(Op::ReopenLogger, 0);
        if self.h.is_master() {
            self.kill_all_workers(libc::SIGWINCH);
        }
    }

    /// `ProcessPool::shutdown`.
    pub(crate) fn shutdown(&mut self) -> bool {
        if self.h.is_master() {
            self.set_running(false);
            true
        } else {
            kill(self.master_pid(), libc::SIGTERM).is_ok()
        }
    }

    /// `ProcessPool::reload`.
    pub(crate) fn reload(&mut self) -> Result<bool, Bailed> {
        if self.p.with_reload_task(|_| ()).is_some() {
            return Ok(false);
        }
        if self.on_before_reload().is_some() {
            self.h.callback(Callback::BeforeReload, ptr::null_mut())?;
        }
        self.p.set_reload_task(Some(Box::default()));
        if self.max_wait_time() != 0 {
            self.op(Op::ReloadTimer, 0);
        }
        self.set_reload_count(self.reload_count() + 1);
        self.set_reload_init(true);
        self.set_reload_last_time(crate::time_now());
        Ok(true)
    }

    /// `ProcessPool::drop_reload_task`: free the reload, and its timer with it.
    pub(crate) fn drop_reload_task(&mut self) {
        self.p.set_reload_task(None);
    }

    /// `ProcessPool::at_worker_enter`.
    fn at_worker_enter(&mut self, worker: *mut WorkerRecord) {
        self.on(worker, Op::DontRestart, 0);
        if self.ipc_mode() == IpcMode::UnixSock {
            self.op(Op::TimerInit, 0);
        }
    }

    /// `ProcessPool::run_with_*_protocol` and `run_async`: the worker's loop; false
    /// where upstream returns `SW_ERR`, as for a `worker` that is none of the pool's.
    fn main(
        &mut self,
        worker: *mut WorkerRecord,
        protocol: Option<Protocol>,
    ) -> Result<bool, Bailed> {
        let Some(protocol) = protocol else {
            return Ok(self.run_async(worker));
        };
        if protocol == Protocol::Message {
            if self.ipc_mode() != IpcMode::UnixSock {
                self.op(Op::Log(Log::NeedsUnixsock), 1);
                return Ok(false);
            }
            // The loop reads the bus: without one (it couldn't be made) there's no loop.
            if !self.p.has_message_bus() && !self.create_message_bus() {
                return Ok(false);
            }
        }
        if self.ipc().is_null() {
            // A pool `create` never made.
            return Ok(false);
        }
        let Some(own) = self.p.worker_of(worker) else {
            // None of the pool's workers.
            return Ok(false);
        };
        let worker_id = own.id();
        let ipc_mode = if self.use_msgqueue() {
            IpcMode::MsgQueue
        } else if self.use_socket() {
            IpcMode::Socket
        } else {
            IpcMode::UnixSock
        };
        let pool_loop = PoolLoop {
            pool: self.p,
            worker: own,
            worker_id,
            ipc_mode,
            schedule_by_sysvmsg: self.schedule_by_sysvmsg(),
            max_packet_size: self.max_packet_size(),
            event_size: self.h.event_size(),
        };
        self.at_worker_enter(worker);
        self.h.run_loop(&pool_loop, protocol)?;
        self.op(Op::TimerFree, 0);
        Ok(true)
    }

    /// `ProcessPool::run_async`: false when the event loop failed.
    fn run_async(&mut self, worker: *mut WorkerRecord) -> bool {
        if self.ipc_mode() == IpcMode::UnixSock && self.on_message().is_some() {
            let _registered = self.h.add_pipe(worker);
            if self.p.has_message_bus() {
                self.op(Op::PipeHandler, 1);
            } else {
                let size = self.max_packet_size();
                self.with_ipc(|ipc| pool_ipc_prepare(ipc, size));
                self.op(Op::PipeHandler, 0);
            }
        }
        self.h.event_wait()
    }

    /// `ProcessPool::wait`'s per-iteration service: signals, timers, then the
    /// workers' messages (a stopped worker is replaced and its pid detached).
    fn service(&mut self) {
        self.op(Op::SignalDispatch, 0);
        if crate::process::pool::bailout_pending() {
            return;
        }
        self.op(Op::TimerSelect, 0);
        if crate::process::pool::bailout_pending() {
            return;
        }
        if !self.read_message() {
            return;
        }
        // An EventData's bytes, which each pop overwrites.
        let mut event = [0; crate::constants::SW_IPC_MAX_SIZE as usize];
        while crate::process::methods::pop_message(self.p, &mut event)
            .is_some_and(|length| length > 0)
        {
            if crate::process::pool::bailout_pending() {
                return;
            }
            if !self.running() {
                continue;
            }
            if event[EVENT_TYPE] != WORKER_MESSAGE_STOP && self.p.call_on_worker_message(&event) {
                continue;
            }
            // The WorkerStopMessage's pid, at the start of its data.
            let Some(&pid) = event.get(EVENT_DATA..).and_then(<[u8]>::first_chunk) else {
                continue;
            };
            let pid = c_int::from_ne_bytes(pid);
            let worker = self.worker_by_pid(pid);
            if worker.is_null() {
                continue;
            }
            // Leave the callback bailout pending for the enclosing native boundary.
            if self.spawn(worker).is_err() {
                crate::process::pool::defer_bailout();
                return;
            }
            self.with_registry(|registry| registry.detach(pid));
        }
        self.set_read_message(false);
    }

    /// `ProcessPool::wait`'s reap: restart the exited worker. False when `pid`
    /// was no worker of the pool's.
    fn reap(&mut self, pid: c_int, status: c_int) -> bool {
        let worker = self.worker_by_pid(pid);
        if worker.is_null() {
            if self.on_worker_not_found().is_some() {
                self.op(Op::OnWorkerNotFound, exited(pid, status));
            } else if !self
                .with_registry(|registry| registry.reap_detached(pid))
                .unwrap_or(false)
            {
                self.op(Op::Log(Log::UnknownChild), u64::from(pid.cast_unsigned()));
            }
            return false;
        }
        if !libc::WIFEXITED(status) {
            self.on(worker, Op::WorkerError, exited(pid, status));
        }
        // Leave the callback bailout pending for the enclosing native boundary.
        if crate::process::pool::bailout_pending() {
            return false;
        }
        if self.spawn(worker).is_err() {
            crate::process::pool::defer_bailout();
            return false;
        }
        true
    }

    /// `ProcessPool::wait`: reap and restart workers until the pool stops, then
    /// stop them all.
    pub(crate) fn wait(&mut self) -> Result<(), Bailed> {
        reaper::run(self)?;

        self.set_running(false);
        self.drop_reload_task();
        if self.on_shutdown().is_some() {
            self.h.callback(Callback::Shutdown, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            let pid = self.worker_at(index).pid();
            if kill(pid, libc::SIGTERM).is_err() {
                self.op(Op::Log(Log::KillFailed), u64::from(pid.cast_unsigned()));
            }
        }
        if self.max_wait_time() != 0 {
            self.op(Op::KillTimer, 0);
        }
        // Upstream waits for the first worker only.
        if self.worker_num() > 0 {
            let pid = self.worker_at(0).pid();
            if !self.h.waitpid(pid) {
                self.op(Op::Log(Log::WaitpidFailed), u64::from(pid.cast_unsigned()));
            }
        }
        self.set_started(false);
        Ok(())
    }

    /// `ProcessPool::push_message(msg)`: the `EventData` with `head` and `data` into
    /// the message box, then wake the master.
    fn push_event(&mut self, head: &DataHead, data: &[u8]) -> bool {
        if self.p.message_box().is_null() || data.len() > self.h.event_size() - EVENT_DATA {
            return false;
        }
        let head = DataHead {
            len: u32::try_from(data.len()).unwrap_or(u32::MAX),
            ..*head
        };
        let message = [&head_view(&head)[..], data].concat();
        self.h.push_box(&message) && kill(self.master_pid(), libc::SIGIO).is_ok()
    }

    /// `ProcessPool::push_message(type, data, length)`.
    fn push(&mut self, kind: u8, data: &[u8]) -> bool {
        if self.p.message_box().is_null() {
            return false;
        }
        assert!(
            data.len() < self.h.event_size() - EVENT_DATA,
            "the message fits in EventData"
        );
        let head = DataHead {
            kind,
            ..DataHead::default()
        };
        self.push_event(&head, data)
    }
    /// `ProcessPool::dispatch`/`dispatch_sync(EventData *)`: to the stream
    /// listener, or `worker`'s pipe or queue (scheduled when None). An `event`
    /// shorter than its head's length is refused.
    fn dispatch(&mut self, event: &[u8], worker: Option<u32>, blocking: bool) -> bool {
        let Some(&len) = event.get(EVENT_LEN..).and_then(<[u8]>::first_chunk) else {
            return false;
        };
        let size = u32::from_ne_bytes(len).saturating_add(HEAD_SIZE);
        let Some(event) = event.get(..size as usize) else {
            return false;
        };
        if self.use_socket() {
            let send = |ipc: Option<&PoolIpc>| {
                crate::process::pool::swr_v1_process_pool_ipc_send(ipc, event, blocking)
            };
            let sent = self
                .with_ipc(|ipc| send(Some(ipc)))
                .unwrap_or_else(|| send(None));
            return sent;
        }
        let worker = worker.unwrap_or_else(|| {
            if self.schedule_by_sysvmsg() {
                0
            } else {
                self.schedule()
            }
        });
        let Some(native) = self.h.native() else {
            return false;
        };
        let workers = |index| {
            u32::try_from(index)
                .ok()
                .filter(|&index| index < self.worker_num())
                .map(|index| self.worker_at(index))
        };
        crate::process::pool::pool_dispatch(
            native,
            self.p,
            workers,
            self.start_id(),
            self.use_msgqueue(),
            worker,
            event,
            blocking,
        )
    }

    /// `ProcessPool::schedule`: the next idle worker, round robin; the last one
    /// tried, with the scheduler warning set, when all are busy.
    fn schedule(&mut self) -> u32 {
        let round = &self.record().round_id;
        let (target, warning) =
            crate::process::pool::pool_schedule(self.worker_num(), round, |target| {
                crate::process::pool::idle(self.worker_at(target))
            });
        if warning {
            self.set_scheduler_warning(true);
        }
        target
    }

    /// `ProcessPool::destroy`.
    pub(crate) fn destroy(&mut self) {
        self.set_kill_timer(None);
        self.p.set_pipes(None);
        self.p.set_queue(None);
        let ipc = self.ipc();
        if !ipc.is_null() {
            // SAFETY: the pool's IPC state, the box `pool_ipc_new` leaked (the field is
            // crate-private), taken back once: the field is cleared next.
            let ipc = unsafe { Box::from_raw(ipc) };
            // The native close frees its listener.
            pool_ipc_free(*ipc, |listener| self.h.close(listener));
        }
        self.set_ipc(ptr::null_mut());
        self.p.set_registry(None);
        self.h.free_box();
        self.p.set_message_bus(None);
        self.op(Op::FreeWorkers, 0);
    }
}

impl<H: PoolHost> Reap for Pool<'_, H> {
    fn leave_when_stopped(&self) -> bool {
        true
    }

    fn skip_no_child(&self) -> bool {
        false
    }

    fn running(&mut self) -> bool {
        Pool::running(self)
    }

    fn reload_requested(&mut self) -> bool {
        self.reload_init()
    }

    fn with_reload<R>(&mut self, f: impl FnOnce(&mut ReloadTask) -> R) -> Option<R> {
        self.p.with_reload_task(f)
    }

    fn wait(&mut self, nonblock: bool) -> Waited {
        wait_child(nonblock)
    }

    fn service(&mut self) {
        Pool::service(self);
    }

    fn wait_failed(&mut self) {
        self.op(Op::Log(Log::WaitFailed), 0);
    }

    fn start_reload(&mut self) -> ReloadStart {
        if !self.reload_init() {
            return ReloadStart::None;
        }
        self.set_reload_init(false);
        let workers: Vec<_> = (0..self.worker_num() as usize)
            .filter_map(|i| self.record().worker(i))
            .map(|worker| (worker.pid(), worker.id()))
            .collect();
        self.with_reload(|task| {
            for (pid, id) in workers {
                task.add(pid, id);
            }
        });
        ReloadStart::KillOne
    }

    fn reap(&mut self, pid: c_int, status: c_int) -> bool {
        Pool::reap(self, pid, status)
    }

    fn finish_reload(&mut self) {
        self.drop_reload_task();
        if self.on_after_reload().is_some() {
            self.op(Op::OnAfterReload, 0);
        }
    }

    fn warn(&self) -> Warn {
        self.h.warn()
    }
}

/// Run `f` over the pool with the native side.
pub(crate) fn with<T>(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    f: impl FnOnce(&mut Pool<'_, Native<'_>>) -> T,
) -> T {
    let mut pool = Pool {
        h: Native { native, pool },
        p: pool,
    };
    f(&mut pool)
}

/// `ProcessPool::create(worker_num, key, mode)`.
pub fn swr_v1_process_pool_create(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker_num: u32,
    key: c_int,
    mode: u8,
) -> bool {
    with(native, pool, |p| p.create(worker_num, key, mode))
}

/// `ProcessPool::push_message(type, data, length)`.
pub fn swr_v1_process_pool_push(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    kind: u8,
    data: &[u8],
) -> bool {
    with(native, pool, |p| p.push(kind, data))
}

/// `ProcessPool::push_message(EventData *)` of the event with `head` and `data`.
pub fn swr_v1_process_pool_push_event(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    head: &DataHead,
    data: &[u8],
) -> bool {
    with(native, pool, |p| p.push_event(head, data))
}

/// `ProcessPool::dispatch` (`blocking`: `dispatch_sync`) of the `EventData` at the
/// start of `event`.
pub fn swr_v1_process_pool_dispatch_event(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    event: &[u8],
    worker: Option<u32>,
    blocking: bool,
) -> bool {
    with(native, pool, |p| p.dispatch(event, worker, blocking))
}

/// `swoole_get_worker_symbol`: the log's mark for a worker type (`swWorkerType`).
#[must_use]
pub extern "C" fn swr_v1_worker_symbol(kind: c_int) -> std::ffi::c_char {
    let symbol = match WorkerType::from_code(kind.into()) {
        Some(WorkerType::Master) => b'#',
        Some(WorkerType::Manager) => b'$',
        Some(WorkerType::Event) => b'*',
        Some(WorkerType::Task) => b'^',
        Some(WorkerType::User) => b'@',
        None => b'%',
    };
    std::ffi::c_char::from_ne_bytes([symbol])
}

/// `ProcessPool::run_with_*_protocol`, or `run_async` for no `protocol`:
/// `worker`'s loop in `pool`; false where upstream returns `SW_ERR`, as when
/// `worker` is none of the pool's ([`PoolRef::worker_of`]).
///
/// # Errors
/// [`Bailed`] when a callback bailed out; the caller raises it.
pub fn pool_run_main(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker: Worker<'_>,
    protocol: Option<Protocol>,
) -> Result<bool, Bailed> {
    match pool.worker_of(worker.raw()) {
        Some(worker) => with(native, pool, |p| p.main(worker.raw(), protocol)),
        None => Ok(false),
    }
}

impl<H: PoolHost> Pool<'_, H> {
    /// `ProcessPool::set_type(type)`: the pool's and each worker's.
    pub(crate) fn set_type(&mut self, kind: WorkerType) {
        self.set_kind(kind.code());
        for index in 0..self.worker_num() {
            self.worker_at(index).set_kind(kind);
        }
    }

    /// `ProcessPool::set_start_id(start_id)`: the workers numbered from it.
    pub(crate) fn number_from(&mut self, start_id: u16) {
        self.set_start_id(start_id);
        for index in 0..self.worker_num() {
            self.worker_at(index).set_id(u32::from(start_id) + index);
        }
    }

    /// `ProcessPool::set_max_request(max_request, max_request_grace)`.
    pub(crate) fn set_max_requests(&mut self, max: u32, grace: u32) {
        self.set_max_request(max);
        self.set_max_request_grace(grace);
    }

    /// `ProcessPool::free_message_box`.
    pub(crate) fn free_message_box(&mut self) {
        self.h.free_box();
    }
}

#[cfg(test)]
#[path = "../../../../tests/modules/process/pool_owners.rs"]
mod owner_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn worker_can_take_the_receive_allocation_once() {
        use crate::buffer::string::OwnedString;
        let mut buffer = OwnedString::default();
        buffer.alloc(64, None);
        buffer.append(b"request");
        buffer.set_offset(7);
        let original = buffer.view().as_ptr();
        let info = DataHead {
            len: 7,
            ..DataHead::default()
        };
        let mut recv = RecvData::buffer(info, &mut buffer);
        assert_eq!(recv.payload(), b"request");
        let packet = recv.take_packet().unwrap();
        assert_eq!(packet.view().as_ptr(), original);
        assert_eq!(packet.view(), b"request");
        assert_eq!(recv.payload(), b"");
        assert!(recv.take_packet().is_none());
        assert!(buffer.is_empty());
        buffer.append(b"next");
        assert_eq!(buffer.view(), b"next");
    }

    /// core-tests' `base.clamp`, over `max_package_size`'s clamp.
    #[test]
    fn base_clamp() {
        const _: () = assert!(max_package_size(-1) == 1, "clamp must be a constant");
        assert_eq!(max_package_size(0), 1);
        assert_eq!(max_package_size(1), 1);
        assert_eq!(max_package_size(1024), 1024);
        assert_eq!(max_package_size(u32::MAX.into()), u32::MAX);
        assert_eq!(max_package_size(i64::from(u32::MAX) + 1), u32::MAX);
        assert_eq!(u32::MAX.clamp(0, 1024), 1024);
    }

    #[test]
    fn a_message_queue_ignores_the_packet_size() {
        assert_eq!(packet_limit(true, 1), SW_MSGMAX);
        assert_eq!(packet_limit(false, 1), 1);
    }

    #[test]
    fn the_record_has_process_pools_layout() {
        assert_eq!(offset_of!(PoolRecord, protocol_type), 8);
        assert_eq!(offset_of!(PoolRecord, reload_count), 24);
        assert_eq!(offset_of!(PoolRecord, kind), 40);
        assert_eq!(offset_of!(PoolRecord, start_id), 42);
        assert_eq!(offset_of!(PoolRecord, max_packet_size), 48);
        assert_eq!(offset_of!(PoolRecord, warning_time), 72);
        assert_eq!(offset_of!(PoolRecord, on_start), 80);
        assert_eq!(offset_of!(PoolRecord, round_id), 184);
        assert_eq!(offset_of!(PoolRecord, workers), 192);
        assert_eq!(offset_of!(PoolRecord, ptr), 256);
        assert_eq!(offset_of!(PoolRecord, kill_timer), 264);
        assert_eq!(offset_of!(PoolRecord, ptr_key), 288);
        assert_eq!(size_of::<PoolRecord>(), 296);
    }

    /// A scripted op's pointer answer.
    fn pointer<T>(address: i64) -> *mut T {
        ptr::with_exposed_provenance_mut(usize::try_from(address).unwrap_or(0))
    }

    /// A scripted native side over a pool record allocated here.
    #[derive(Default)]
    struct Script {
        answers: std::collections::HashMap<Op, i64>,
        log: Vec<(Op, u64)>,
        callbacks: Vec<Callback>,
        bail: Option<Callback>,
        /// The fixture's record, where `Op::KillTimer` arms the kill timer.
        record: Option<std::ptr::NonNull<PoolRecord>>,
    }

    impl Script {
        /// Logs `op` and answers it from the script: 1 unless it says otherwise.
        fn ask(&mut self, op: Op, arg: u64) -> i64 {
            self.log.push((op, arg));
            if op == Op::KillTimer
                && let Some(mut record) = self.record
            {
                let timer =
                    ArmedTimer::new(1, KILL_TIMER_ID, crate::process::armed_timer::tests::record);
                // SAFETY: the fixture's record, which outlives its pool.
                unsafe { record.as_mut() }.kill_timer.set(Some(timer));
            }
            self.answers.get(&op).copied().unwrap_or(1)
        }
    }

    impl PoolHost for Script {
        fn op(&mut self, _: *mut c_void, op: Op, arg: u64) {
            self.ask(op, arg);
        }
        fn have_msgqueue(&mut self) -> bool {
            self.ask(Op::HaveMsgqueue, 0) != 0
        }
        fn event_init(&mut self) -> bool {
            self.ask(Op::EventInit, 0) >= 0
        }
        fn event_available(&mut self) -> bool {
            self.ask(Op::EventAvailable, 0) != 0
        }
        fn event_wait(&mut self) -> bool {
            self.ask(Op::EventWait, 0) >= 0
        }
        fn is_master(&mut self) -> bool {
            self.ask(Op::IsMaster, 0) != 0
        }
        fn fork(&mut self, _: *mut WorkerRecord) -> Option<c_int> {
            c_int::try_from(self.ask(Op::Fork, 0))
                .ok()
                .filter(|pid| *pid >= 0)
        }
        fn waitpid(&mut self, pid: c_int) -> bool {
            self.ask(Op::Waitpid, u64::from(pid.cast_unsigned())) >= 0
        }
        fn new_pipe(&mut self, _: *mut WorkerRecord) -> Result<(), PipeError> {
            (self.ask(Op::NewPipe, 0) != 0)
                .then_some(())
                .ok_or(PipeError::Open)
        }
        #[cfg(target_os = "linux")]
        fn send_buffer_size(&mut self, _: *mut WorkerRecord) -> std::io::Result<c_int> {
            let size = self.ask(Op::Sndbuf, 0);
            if size < 0 {
                Err(std::io::Error::from_raw_os_error(libc::EBADF))
            } else {
                Ok(size.c_cast::<c_int>())
            }
        }
        fn bus_buffer(&mut self, size: usize) -> Result<(), PipeError> {
            (self.ask(Op::BusBuffer, size as u64) != 0)
                .then_some(())
                .ok_or(PipeError::Buffer)
        }
        fn add_pipe(&mut self, _: *mut WorkerRecord) -> Result<(), PipeError> {
            (self.ask(Op::AddPipe, 0) >= 0)
                .then_some(())
                .ok_or(PipeError::Register)
        }
        fn push_stop(&mut self, _: *mut WorkerRecord) -> Result<(), PipeError> {
            (self.ask(Op::PushStop, 0) >= 0)
                .then_some(())
                .ok_or(PipeError::StopMessage)
        }
        fn callback(&mut self, cb: Callback, _: *mut c_void) -> Result<isize, Bailed> {
            self.callbacks.push(cb);
            if self.bail == Some(cb) {
                Err(Bailed)
            } else {
                Ok(0)
            }
        }
        fn run_loop(&mut self, _: &PoolLoop<'_>, _: Protocol) -> Result<(), Bailed> {
            Ok(())
        }
        fn native(&self) -> Option<&dyn PoolNative> {
            None
        }
        fn warn(&self) -> Warn {
            warn
        }
        fn event_size(&self) -> usize {
            8192
        }
        fn close(&self, _: crate::process::pool::Connection) {}
        fn new_box(&mut self, size: u64) -> SharedChannel {
            if self.ask(Op::NewMessageBox, size) == 0 {
                return SharedChannel::none();
            }
            SharedChannel::make(64, 16, 0).unwrap_or_default()
        }
        fn push_box(&mut self, message: &[u8]) -> bool {
            self.ask(Op::BoxPush, message.len() as u64) >= 0
        }
        fn free_box(&mut self) {
            if let Some(mut record) = self.record
                // SAFETY: the fixture's record, which outlives its pool.
                && !unsafe { record.as_ref() }.message_box.is_none()
            {
                self.ask(Op::FreeMessageBox, 0);
                // SAFETY: as above.
                unsafe { record.as_mut() }.message_box.free();
            }
        }
        fn alloc_workers(&mut self, count: u32) -> *mut WorkerRecord {
            pointer(self.ask(Op::AllocWorkers, count.into()))
        }
        fn new_message_bus(&mut self) -> Option<Box<dyn std::any::Any>> {
            (self.ask(Op::NewMessageBus, 0) != 0)
                .then(|| -> Box<dyn std::any::Any> { Box::new(()) })
        }
        fn current_worker(&mut self) -> *mut WorkerRecord {
            pointer(self.ask(Op::CurrentWorker, 0))
        }
    }

    fn close(_: crate::process::pool::Connection) {}

    fn warn(_: crate::process::reload_task::KillWarning, _: c_int, _: u32) {}

    struct Fixture {
        record: Box<PoolRecord>,
        workers: Vec<WorkerRecord>,
    }

    impl Fixture {
        fn new(count: usize) -> Self {
            // SAFETY: both are plain data; all-zero is a valid empty pool and worker.
            let record = Box::new(unsafe { std::mem::zeroed::<PoolRecord>() });
            let workers = (0..count)
                // SAFETY: as above.
                .map(|_| unsafe { std::mem::zeroed::<WorkerRecord>() })
                .collect();
            Self { record, workers }
        }

        fn pool(&mut self, mut script: Script) -> Pool<'_, Script> {
            self.record.workers.set(self.workers.as_mut_ptr());
            // `create` takes the workers `Op::AllocWorkers` answers.
            let workers = self.workers.as_mut_ptr().expose_provenance().cast_signed() as i64;
            script.answers.entry(Op::AllocWorkers).or_insert(workers);
            self.record
                .worker_num
                .set(u32::try_from(self.workers.len()).unwrap());
            // The script and the pool share one raw pointer, so neither
            // invalidates the other.
            let p = &raw mut *self.record;
            script.record = std::ptr::NonNull::new(p);
            // SAFETY: the fixture's record, live as long as the fixture.
            Pool {
                h: script,
                p: unsafe { PoolRef::new(p) },
            }
        }
    }

    fn record<'a>(pool: &'a mut Pool<'_, Script>) -> &'a mut PoolRecord {
        // SAFETY: the fixture's record.
        unsafe { &mut *pool.p.raw() }
    }

    fn ops(pool: &Pool<'_, Script>) -> Vec<Op> {
        pool.h.log.iter().map(|(op, _)| *op).collect()
    }

    /// The id of the kill timer the script's `Op::KillTimer` arms.
    const KILL_TIMER_ID: std::ffi::c_long = 41;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no process spawning or kill")]
    fn the_kill_timer_stays_armed_for_workers_still_stopping_until_the_pool_goes() {
        // Two workers: `wait` SIGTERMs both and reaps only the first (the script's
        // `Op::Waitpid`), so the second may still be stopping when it returns.
        let _children = crate::process::wait::tests::CHILDREN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut children: Vec<_> = (0..2)
            .map(|_| {
                std::process::Command::new("sleep")
                    .arg("30")
                    .spawn()
                    .unwrap()
            })
            .collect();
        let mut fixture = Fixture::new(2);
        for (worker, child) in fixture.workers.iter_mut().zip(&children) {
            worker.set_pid(c_int::try_from(child.id()).unwrap());
        }
        let mut pool = fixture.pool(Script::default());
        record(&mut pool).max_wait_time.set(3);
        assert!(matches!(pool.wait(), Ok(())));
        for child in &mut children {
            child.wait().unwrap();
        }
        assert!(ops(&pool).contains(&Op::KillTimer));
        assert!(record(&mut pool).kill_timer.get_mut().is_some());
        assert_eq!(crate::process::armed_timer::tests::cancelled(), []);
        drop(pool);
        drop(fixture);
        assert_eq!(
            crate::process::armed_timer::tests::cancelled(),
            [KILL_TIMER_ID]
        );
    }

    #[test]
    fn destroying_the_pool_deletes_an_armed_kill_timer() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        let timer = ArmedTimer::new(1, KILL_TIMER_ID, crate::process::armed_timer::tests::record);
        record(&mut pool).kill_timer.set(Some(timer));
        pool.destroy();
        assert!(record(&mut pool).kill_timer.get_mut().is_none());
        assert_eq!(
            crate::process::armed_timer::tests::cancelled(),
            [KILL_TIMER_ID]
        );
    }

    #[test]
    fn create_refuses_a_queue_without_sysvmsg() {
        let mut fixture = Fixture::new(0);
        let mut script = Script::default();
        script.answers.insert(Op::HaveMsgqueue, 0);
        let mut pool = fixture.pool(script);
        assert!(!pool.create(2, 0, IpcMode::MsgQueue.code()));
        assert_eq!(ops(&pool), [Op::HaveMsgqueue, Op::Log(Log::NoSysvmsg)]);
    }

    #[test]
    fn create_makes_a_pipe_per_worker_and_sets_the_defaults() {
        struct PipeOwner(std::rc::Rc<std::cell::Cell<usize>>);
        impl Drop for PipeOwner {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let mut fixture = Fixture::new(3);
        let mut pool = fixture.pool(Script::default());
        assert!(pool.create(3, 0, IpcMode::UnixSock.code()));
        assert_eq!(
            ops(&pool),
            [
                Op::AllocWorkers,
                Op::NewMessageBox,
                Op::NewPipe,
                Op::NewPipe,
                Op::NewPipe,
                Op::MainLoop
            ]
        );
        assert_eq!(pool.ipc_mode(), IpcMode::UnixSock);
        assert_eq!(record(&mut pool).protocol_type.get(), Protocol::Task.code());
        assert_eq!(pool.max_packet_size(), SW_INPUT_BUFFER_SIZE);
        assert_eq!(pool.max_wait_time(), SW_WORKER_MAX_WAIT_TIME);
        assert!(!pool.p.registry().is_null());
        let dropped = std::rc::Rc::new(std::cell::Cell::new(0));
        for _ in 0..3 {
            assert!(pool.p.keep_pipe(Box::new(PipeOwner(dropped.clone()))));
        }
        assert_eq!(dropped.get(), 0);
        pool.destroy();
        assert_eq!(dropped.get(), 3, "the pool drops each pipe owner once");
        assert!(!pool.p.keep_pipe(Box::new(PipeOwner(dropped.clone()))));
        assert_eq!(dropped.get(), 4, "a closed pool retains no new owner");
        assert!(pool.p.registry().is_null() && pool.ipc().is_null());
    }

    #[test]
    fn create_stops_at_the_first_pipe_that_fails() {
        let mut fixture = Fixture::new(3);
        let mut script = Script::default();
        script.answers.insert(Op::NewPipe, 0);
        let mut pool = fixture.pool(script);
        assert!(!pool.create(3, 0, IpcMode::UnixSock.code()));
        assert_eq!(
            pool.h
                .log
                .iter()
                .filter(|(op, _)| *op == Op::NewPipe)
                .count(),
            1
        );
    }

    #[test]
    fn an_unknown_ipc_mode_is_none() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        assert!(pool.create(1, 0, 9));
        assert_eq!(pool.ipc_mode(), IpcMode::None);
        pool.destroy();
    }

    #[test]
    fn start_check_numbers_the_workers_from_start_id() {
        let mut fixture = Fixture::new(2);
        let mut pool = fixture.pool(Script::default());
        pool.set_start_id(5);
        pool.set_kind(4);
        assert!(pool.start_check());
        assert!(pool.running() && record(&mut pool).started.get());
        assert_eq!(pool.master_pid(), getpid());
        // SAFETY: the fixture's records.
        unsafe {
            assert_eq!(Worker::from(&*(pool.worker(1))).id(), 6);
            assert_eq!(
                Worker::from(&*(pool.worker(1))).kind(),
                Some(WorkerType::Task)
            );
        }
    }

    #[test]
    fn a_socket_pool_must_listen_first() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IpcMode::Socket);
        pool.set_ipc(pool_ipc_new(true));
        assert!(!pool.start_check());
        assert_eq!(ops(&pool), [Op::Log(Log::MustListen)]);
        pool.destroy();
    }

    #[test]
    fn a_bailout_in_on_start_spawns_nothing() {
        fn on_start(_: PoolRef<'_>) {}
        let mut fixture = Fixture::new(2);
        let script = Script {
            bail: Some(Callback::Start),
            ..Script::default()
        };
        let mut pool = fixture.pool(script);
        record(&mut pool).on_start.set(Some(on_start));
        assert!(pool.start().is_err());
        assert!(!ops(&pool).contains(&Op::Fork));
    }

    #[test]
    fn spawn_registers_the_new_pid() {
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(Op::Fork, 4321);
        let mut pool = fixture.pool(script);
        pool.p.set_registry(Some(Box::default()));
        let worker = pool.worker(0);
        assert_eq!(pool.spawn(worker).ok(), Some(Some(4321)));
        assert_eq!(pool.worker_by_pid(4321), worker);
        // A respawn forgets the old pid.
        pool.h.answers.insert(Op::Fork, 4322);
        assert_eq!(pool.spawn(worker).ok(), Some(Some(4322)));
        assert!(pool.worker_by_pid(4321).is_null());
        pool.destroy();
    }

    #[test]
    fn reload_once_at_a_time() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        pool.set_max_wait_time(3);
        assert_eq!(pool.reload().ok(), Some(true));
        assert_eq!(pool.reload().ok(), Some(false));
        assert!(pool.reload_init());
        assert_eq!(pool.reload_count(), 1);
        assert_eq!(ops(&pool), [Op::ReloadTimer]);
        pool.drop_reload_task();
        assert!(pool.with_reload(|_| ()).is_none());
    }

    #[test]
    fn detach_needs_a_running_pool_with_a_message_box() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        assert!(!pool.detach());
        pool.set_running(true);
        pool.p
            .set_message_box(SharedChannel::make(64, 16, 0).expect("a box"));
        record(&mut pool).is_async.set(true);
        assert!(pool.detach());
        assert!(!pool.running());
        assert_eq!(
            ops(&pool),
            [
                Op::CurrentWorker,
                Op::WorkerShutdown,
                Op::RemovePipe,
                Op::PushStop
            ]
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn message_bus_keeps_send_buffer_fallback_and_allocation_failure() {
        for (size, expected) in [
            (-1, SW_IPC_MAX_SIZE),
            (0, 0),
            (8192, 8192),
            (131_072, 65536),
        ] {
            for allocated in [0, 1] {
                let mut fixture = Fixture::new(1);
                let mut script = Script::default();
                script.answers.insert(Op::Sndbuf, size);
                script.answers.insert(Op::BusBuffer, allocated);
                let mut pool = fixture.pool(script);
                pool.set_ipc_mode(IpcMode::UnixSock);
                assert_eq!(pool.create_message_bus(), allocated != 0);
                assert_eq!(
                    pool.h.log,
                    [
                        (Op::NewMessageBus, 0),
                        (Op::Sndbuf, 0),
                        (Op::BusBuffer, expected)
                    ]
                );
                assert!(
                    pool.p.has_message_bus(),
                    "the bus remains owned if its buffer fails"
                );
                pool.destroy();
            }
        }
    }

    #[test]
    fn failed_stop_message_keeps_the_pool_running() {
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(Op::PushStop, -1);
        let mut pool = fixture.pool(script);
        pool.set_running(true);
        pool.p
            .set_message_box(SharedChannel::make(64, 16, 0).expect("a box"));
        assert!(!pool.detach());
        assert!(pool.running());
        assert_eq!(
            ops(&pool),
            [Op::CurrentWorker, Op::WorkerShutdown, Op::PushStop]
        );
        pool.set_running(false);
        pool.destroy();
    }

    #[test]
    fn async_loop_still_waits_after_pipe_registration_failure() {
        fn on_message(_: PoolRef<'_>, _: &mut RecvData<'_>) {}
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(Op::AddPipe, -1);
        script.answers.insert(Op::EventWait, -1);
        let mut pool = fixture.pool(script);
        pool.set_ipc_mode(IpcMode::UnixSock);
        record(&mut pool).on_message.set(Some(on_message));
        pool.p.set_message_bus(Some(Box::new(())));
        assert!(!pool.run_async(pool.worker(0)), "the loop's failure");
        assert_eq!(
            pool.h.log,
            [(Op::AddPipe, 0), (Op::PipeHandler, 1), (Op::EventWait, 0)]
        );
        pool.destroy();
    }

    #[test]
    fn stop_outside_an_event_loop_only_shuts_down() {
        fn on_worker_exit(_: PoolRef<'_>, _: Worker<'_>) {}
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(Op::EventAvailable, 0);
        let mut pool = fixture.pool(script);
        record(&mut pool).on_worker_exit.set(Some(on_worker_exit));
        pool.stop(pool.worker(0));
        assert_eq!(ops(&pool), [Op::WorkerShutdown, Op::EventAvailable]);
    }

    #[test]
    fn the_message_protocol_needs_unix_sockets() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IpcMode::MsgQueue);
        assert!(matches!(
            pool.main(pool.worker(0), Some(Protocol::Message)),
            Ok(false)
        ));
        assert_eq!(pool.h.log, [(Op::Log(Log::NeedsUnixsock), 1)]);
    }

    #[test]
    fn each_worker_type_has_its_log_symbol() {
        let symbols: String = (0..=6)
            .map(|kind| char::from(u8::from_ne_bytes(swr_v1_worker_symbol(kind).to_ne_bytes())))
            .collect();
        assert_eq!(symbols, "%#*$^@%");
    }

    #[test]
    fn push_message_needs_a_message_box_then_wakes_the_master() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        assert!(!pool.push(1, b"ab"));
        record(&mut pool).message_box = SharedChannel::make(64, 16, 0).expect("a box");
        pool.h.answers.insert(Op::BoxPush, -1);
        assert!(!pool.push(1, b"ab"));
        assert_eq!(ops(&pool), [Op::BoxPush]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no Unix sockets")]
    fn a_socket_pool_sends_a_dispatch_length_prefixed() {
        use std::io::Read;
        let path = std::env::temp_dir().join(format!("swr-pool-sync-{}.sock", std::process::id()));
        std::fs::remove_file(&path).unwrap_or_default();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let file = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        record(&mut pool).use_socket.set(true);
        let ipc = pool_ipc_new(true);
        // SAFETY: a live handle.
        crate::process::pool::swr_v1_process_pool_ipc_address(unsafe { &mut *ipc }, &file, 0);
        record(&mut pool).ipc.set(ipc);
        let mut event = [0u8; 64];
        event[EVENT_LEN] = 3;
        event[HEAD_SIZE as usize..][..3].copy_from_slice(b"abc");
        // Blocking, then over a stream (upstream's async dispatch).
        for blocking in [true, false] {
            assert!(pool.dispatch(&event, None, blocking));
            let mut received = Vec::new();
            listener
                .accept()
                .unwrap()
                .0
                .read_to_end(&mut received)
                .unwrap();
            assert_eq!(received[..4], 43u32.to_be_bytes());
            assert_eq!(received[4..], event[..43]);
        }
        assert_eq!(ops(&pool), Vec::<Op>::new());
        std::fs::remove_file(&path).unwrap();
        record(&mut pool).ipc.set(ptr::null_mut());
        // SAFETY: the handle made above; it has no listener to close.
        pool_ipc_free(*unsafe { Box::from_raw(ipc) }, close);
        // A pool whose listener is gone fails.
        let ipc = pool_ipc_new(true);
        // SAFETY: as above.
        crate::process::pool::swr_v1_process_pool_ipc_address(unsafe { &mut *ipc }, &file, 0);
        record(&mut pool).ipc.set(ipc);
        for blocking in [true, false] {
            assert!(!pool.dispatch(&event, None, blocking));
        }
        record(&mut pool).ipc.set(ptr::null_mut());
        // SAFETY: as above.
        pool_ipc_free(*unsafe { Box::from_raw(ipc) }, close);
    }

    #[test]
    fn signals_stop_reload_and_read_messages() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_running(true);
        assert!(pool.signal(libc::SIGIO, ptr::null_mut()).is_ok());
        assert!(pool.read_message());
        assert!(pool.signal(libc::SIGUSR1, ptr::null_mut()).is_ok());
        assert!(pool.reload_init());
        pool.drop_reload_task();
        assert!(pool.signal(libc::SIGTERM, pool.worker(0)).is_ok());
        assert!(!pool.running());
        assert!(ops(&pool).contains(&Op::WorkerShutdown));
    }

    #[test]
    fn an_unknown_child_is_reported() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        pool.p.set_registry(Some(Box::default()));
        assert!(!pool.reap(99, 0));
        assert_eq!(pool.h.log, [(Op::Log(Log::UnknownChild), 99)]);
        pool.destroy();
    }
}
