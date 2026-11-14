//! `ProcessPool`'s lifecycle (once `src/os/process_pool.cc`): create, start, spawn,
//! stop, detach, reload, the manager's wait and shutdown, destroy and the
//! worker-side entry into the loops of [`crate::pool`]. Rust reads and writes
//! the pool's fields in place through [`PoolRecord`], `ProcessPool`'s layout
//! (the C++ view takes its fields from it), and owns its registry, IPC state and
//! reload. `crates/server/src/pool_native.rs` keeps the objects (queue, pipes,
//! channel, message bus, timers, reactor, fork) and the callbacks, and does each
//! effect when this crate asks through [`PoolNative::call`].
//!
//! A callback that can reach PHP runs through [`PoolNative::callback`] inside
//! the bailout trap; when it bails out, the export returns [`RAISE`] and its
//! native caller raises the bailout again once the Rust frames are gone.

use crate::task_file::Packed;
use crate::worker_record::Worker;
use std::ffi::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::AtomicU32;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::DataHead;
use swoole_rs_memory::ChannelInitError;

use crate::armed_timer::ArmedTimer;
use crate::os::{getpid, kill};
use crate::pool::{
    Bailed, PoolCall, PoolIpc, PoolLoop, pool_ipc_free, pool_ipc_new, pool_ipc_prepare, pool_run,
    swr_v1_process_pool_ipc_listener,
};
use crate::reaper::{self, Reap, ReloadStart, Waited, wait_child};
use crate::reload_task::{ReloadTask, Warn};
use crate::worker_record::WorkerRecord;
use crate::worker_registry::WorkerRegistry;

/// `swoole::ProcessPool`'s fields, in order. Rust owns this layout; the C++ view
/// takes its fields from here (`SWR_POOL_RECORD_FIELDS`, `tools/process_records.py`).
#[repr(C)]
pub struct PoolRecord {
    pub running: bool,
    pub reload_init: bool,
    pub read_message: bool,
    pub started: bool,
    pub schedule_by_sysvmsg: bool,
    pub is_async: bool,
    pub ipc_mode: IpcMode,
    pub protocol_type: c_int,
    pub master_pid: c_int,
    pub max_wait_time: u32,
    pub reload_count: u64,
    pub reload_last_time: i64,
    pub kind: u8,
    pub start_id: u16,
    pub use_msgqueue: bool,
    pub use_socket: bool,
    pub max_packet_size: u32,
    pub msgqueue_key: c_int,
    pub(crate) worker_num: u32,
    pub max_request: u32,
    pub max_request_grace: u32,
    pub scheduler_warning: bool,
    pub warning_time: i64,
    pub(crate) on_start: Option<PoolFn>,
    pub(crate) on_shutdown: Option<PoolFn>,
    pub(crate) on_before_reload: Option<PoolFn>,
    pub(crate) on_after_reload: Option<PoolFn>,
    pub(crate) on_task: Option<TaskFn>,
    pub(crate) on_worker_start: Option<WorkerFn>,
    pub(crate) on_message: Option<MessageFn>,
    pub(crate) on_worker_exit: Option<WorkerFn>,
    pub(crate) on_worker_stop: Option<WorkerFn>,
    pub(crate) on_worker_error: Option<ErrorFn>,
    pub(crate) on_worker_message: Option<EventFn>,
    pub(crate) on_worker_not_found: Option<NotFoundFn>,
    pub(crate) main_loop: Option<MainLoopFn>,
    /// The round robin's next worker, which other processes update atomically.
    pub round_id: AtomicU32,
    pub(crate) workers: *mut WorkerRecord,
    pub(crate) pipes: *mut c_void,
    // `worker_num` and the pointers from `workers` on are crate-private, like
    // `registry`: the pool's operations trust them, so safe code elsewhere reads
    // them through the getters below and writes them only through the `unsafe`
    // setters. So are the hooks above and `ptr`, which the hooks read: their owner
    // installs them together ([`PoolRef::install`]).
    /// The registry `create` made, which `destroy` (or dropping the record) frees.
    pub(crate) registry: Option<Box<WorkerRegistry>>,
    /// A message-queue pool's queue, which goes as the registry does.
    pub(crate) queue: Option<Box<swoole_rs_ossupport::QueueRecord>>,
    pub(crate) ipc: *mut PoolIpc,
    pub(crate) message_box: *mut c_void,
    pub(crate) message_bus: *mut c_void,
    pub(crate) reload_task: *mut ReloadTask,
    pub(crate) ptr: *mut c_void,
    /// The `max_wait_time` timer `wait` arms to SIGKILL the workers still
    /// running. It stays armed after `wait` returns, which reaps only the first,
    /// and goes with the pool: `destroy`, or dropping the record, deletes it.
    pub kill_timer: Option<ArmedTimer>,
}

impl PoolRecord {
    /// A pool before `create`, as C's `ProcessPool pool = {}`: every field zero,
    /// null or `None`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            running: false,
            reload_init: false,
            read_message: false,
            started: false,
            schedule_by_sysvmsg: false,
            is_async: false,
            ipc_mode: IpcMode::None,
            protocol_type: 0,
            master_pid: 0,
            max_wait_time: 0,
            reload_count: 0,
            reload_last_time: 0,
            kind: 0,
            start_id: 0,
            use_msgqueue: false,
            use_socket: false,
            max_packet_size: 0,
            msgqueue_key: 0,
            worker_num: 0,
            max_request: 0,
            max_request_grace: 0,
            scheduler_warning: false,
            warning_time: 0,
            on_start: None,
            on_shutdown: None,
            on_before_reload: None,
            on_after_reload: None,
            on_task: None,
            on_worker_start: None,
            on_message: None,
            on_worker_exit: None,
            on_worker_stop: None,
            on_worker_error: None,
            on_worker_message: None,
            on_worker_not_found: None,
            main_loop: None,
            round_id: AtomicU32::new(0),
            workers: ptr::null_mut(),
            pipes: ptr::null_mut(),
            registry: None,
            queue: None,
            ipc: ptr::null_mut(),
            message_box: ptr::null_mut(),
            message_bus: ptr::null_mut(),
            reload_task: ptr::null_mut(),
            ptr: ptr::null_mut(),
            kill_timer: None,
        }
    }

    /// The pool's worker count: how many records `workers` holds.
    #[must_use]
    pub fn worker_num(&self) -> u32 {
        self.worker_num
    }

    /// The pool's `worker_num` worker records, or null before `create`.
    #[must_use]
    pub fn workers(&self) -> *mut WorkerRecord {
        self.workers
    }

    /// The native pipes `create` made for a unix-socket pool, or null.
    #[must_use]
    pub fn pipes(&self) -> *mut c_void {
        self.pipes
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
        self.ipc
    }

    /// The pool's message box (a shared-memory channel), or null.
    #[must_use]
    pub fn message_box(&self) -> *mut c_void {
        self.message_box
    }

    /// The pool's message box, which the workers push to and the master pops from.
    pub(crate) fn message_box_channel(&self) -> Option<&swoole_rs_memory::channel::Record> {
        // SAFETY: `message_box` is crate-private: null, or the channel `new_box` made
        // in shared memory until `free_box` forgets and frees it. Its SW_CHAN_LOCK
        // mutex serializes the processes that use it.
        unsafe {
            self.message_box
                .cast::<swoole_rs_memory::channel::Record>()
                .as_ref()
        }
    }

    /// The pool's message bus, or null.
    #[must_use]
    pub fn message_bus(&self) -> *mut c_void {
        self.message_bus
    }

    /// The reload in progress, or null.
    #[must_use]
    pub fn reload_task(&self) -> *mut ReloadTask {
        self.reload_task
    }

    /// The pool's worker at `index`, or `None` past `worker_num`.
    #[must_use]
    pub fn worker(&self, index: usize) -> Option<Worker<'_>> {
        if self.workers.is_null() || index >= self.worker_num as usize {
            return None;
        }
        // SAFETY: `workers` holds the pool's `worker_num` records, live as long as the
        // pool (the field is crate-private; `set_workers` states it).
        Some(unsafe { Worker::new(self.workers.wrapping_add(index)) })
    }

    /// The pool's worker at `raw`, or `None` when `raw` is none of its records.
    #[must_use]
    pub fn own(&self, raw: *const WorkerRecord) -> Option<Worker<'_>> {
        let offset = raw.addr().checked_sub(self.workers.addr())?;
        if offset % size_of::<WorkerRecord>() != 0 {
            return None;
        }
        self.worker(offset / size_of::<WorkerRecord>())
    }

    /// Sets `workers`.
    ///
    /// # Safety
    /// `workers` holds the pool's `worker_num` records, live as long as the pool.
    pub unsafe fn set_workers(&mut self, workers: *mut WorkerRecord) {
        self.workers = workers;
    }

    /// `f` over the pool's IPC state; `None` before `create`.
    pub(crate) fn with_ipc<R>(&self, f: impl FnOnce(&mut PoolIpc) -> R) -> Option<R> {
        // SAFETY: `ipc` is crate-private: null, or the state `create` made until
        // `destroy` frees it. Only these calls borrow it, none across a callback, so
        // nothing else borrows it while `f` runs.
        unsafe { self.ipc.as_mut() }.map(f)
    }

    /// The pool's stream listener (a native socket), or null.
    #[must_use]
    pub fn ipc_listener(&self) -> *mut c_void {
        self.with_ipc(|ipc| swr_v1_process_pool_ipc_listener(ipc))
            .unwrap_or(ptr::null_mut())
    }

    /// The stream connection being served (a native socket), or null.
    #[must_use]
    pub fn ipc_connection(&self) -> *mut c_void {
        self.with_ipc(|ipc| crate::pool::pool_ipc_connection(ipc))
            .unwrap_or(ptr::null_mut())
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

/// A live pool, as the pool passes one to its callbacks. Like [`Worker`], it reads
/// and writes through the pointer and makes no lasting reference to the record,
/// which the pool's own code and signal handlers write while a callback runs.
///
/// Any record is a pool the operations over this handle can run on: a zeroed one,
/// or one `create` made. The fields they trust (`worker_num` and the pointers from
/// `workers` on) are crate-private, written only by this crate's operations and the
/// `unsafe` setters, so safe code can't forge them, and the operations check each
/// pointer for null before using it.
#[derive(Clone, Copy)]
#[repr(transparent)]
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
    /// The pool's worker at `raw` ([`PoolRecord::own`]), or `None` when `raw` is
    /// none of its records.
    #[must_use]
    pub fn worker_of(self, raw: *const WorkerRecord) -> Option<Worker<'a>> {
        // The pool's records live as long as it does (`set_workers`).
        self.record().own(raw)
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
}

impl<'a> PoolRef<'a> {
    /// The record, for this crate's operations, which read it in place.
    pub(crate) fn record(self) -> &'a PoolRecord {
        // SAFETY: `new`'s contract: a live pool for `'a`.
        unsafe { &*self.raw }
    }

    /// The pool's message queue's message and byte counts (`msgctl(IPC_STAT)`),
    /// or `None` for a pool without one or a failed stat.
    #[must_use]
    pub fn queue_stat(self) -> Option<(usize, usize)> {
        self.with_queue(swoole_rs_ossupport::QueueRecord::stat)?
    }

    /// The pool's message queue's id (`msgget`'s), 0 without one.
    #[must_use]
    pub fn queue_id(self) -> c_int {
        self.with_queue(swoole_rs_ossupport::QueueRecord::id)
            .unwrap_or(0)
    }

    /// `queue->pop(node, size)` into a `QueueNode` held in words, and `errno` right
    /// after it; -1 when it failed or there is no queue.
    pub(crate) fn queue_pop(self, node: &mut [u64], size: usize) -> (isize, c_int) {
        let result = self
            .with_queue(|queue| queue.pop_words(node, size))
            .flatten();
        let length = result.map_or(-1, CCast::c_cast::<isize>);
        (length, swoole_rs_corebase::sys::errno())
    }

    /// `queue->push(node, length)` of a `QueueNode` held in words; false without a
    /// queue.
    pub(crate) fn queue_push(self, node: &[u64], length: usize) -> bool {
        self.with_queue(|queue| queue.push_words(node, length))
            .unwrap_or(false)
    }

    /// `f` over the pool's message queue; `None` without one. Only this type's queue
    /// reads borrow it, and none of them reaches `destroy`, which frees it.
    fn with_queue<R>(self, f: impl FnOnce(&swoole_rs_ossupport::QueueRecord) -> R) -> Option<R> {
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

    /// The pool's stream listener (a native socket), or null.
    #[must_use]
    pub fn ipc_listener(self) -> *mut c_void {
        self.record().ipc_listener()
    }

    /// The stream connection being served (a native socket), or null.
    #[must_use]
    pub fn ipc_connection(self) -> *mut c_void {
        self.record().ipc_connection()
    }

    /// The worker loop's receive buffer, or null before the loop allocates it.
    #[must_use]
    pub fn ipc_packet(self) -> *mut c_char {
        self.record()
            .with_ipc(crate::pool::pool_ipc_packet)
            .unwrap_or(ptr::null_mut())
    }
}

/// `swoole::RecvData`: a message's head and its data.
#[repr(C)]
pub struct RecvData {
    pub info: swoole_rs_corebase::DataHead,
    pub data: *const core::ffi::c_char,
}

/// The pool's callbacks, in `ProcessPool`'s C layout (`swoole_rs_process_views.h`).
/// Each takes the live pool and the objects of the event, which the pool lends for
/// the call. `C-unwind`, as a PHP bailout longjmps over them.
///
/// A hook runs only through the pool that holds it ([`PoolRef::call_on_start`] and
/// the calls beside it), and only its owner installs it ([`PoolRef::install`]), with
/// the `ptr` it reads. So a hook whose other arguments are handles and references is
/// a safe fn: the pool it is given is the one it was installed on.
pub type PoolFn = extern "C-unwind" fn(PoolRef<'_>);
/// `onWorkerStart`, `onWorkerStop`, `onWorkerExit`.
pub type WorkerFn = extern "C-unwind" fn(PoolRef<'_>, Worker<'_>);
/// `main_loop`: the worker's loop, and its exit code.
pub type MainLoopFn = extern "C-unwind" fn(PoolRef<'_>, Worker<'_>) -> c_int;
/// `onTask`: the task the worker read.
pub type TaskFn = extern "C-unwind" fn(PoolRef<'_>, Worker<'_>, Packed<'_>) -> c_int;
/// `onMessage`.
///
/// # Safety
/// The pool is the one that holds the callback, and `data` points at `info.len` bytes.
pub type MessageFn = unsafe extern "C-unwind" fn(PoolRef<'_>, &RecvData);
/// `onWorkerMessage`.
///
/// # Safety
/// The pool is the one that holds the callback, and the pointer is an `EventData`
/// the master popped.
pub type EventFn = unsafe extern "C-unwind" fn(PoolRef<'_>, *mut c_void);
/// `onWorkerError`.
pub type ErrorFn = extern "C-unwind" fn(PoolRef<'_>, Worker<'_>, &crate::methods::ExitStatus);
/// `onWorkerNotFound`.
pub type NotFoundFn = extern "C-unwind" fn(PoolRef<'_>, &crate::methods::ExitStatus);

/// What a pool's owner installs ([`PoolRef::install`]): the object its hooks read
/// as the pool's `ptr`, and the hooks. `None` leaves what the pool has.
#[derive(Clone, Copy, Default)]
pub struct Hooks {
    pub ptr: Option<*mut c_void>,
    pub on_start: Option<PoolFn>,
    pub on_shutdown: Option<PoolFn>,
    pub on_task: Option<TaskFn>,
    pub on_worker_start: Option<WorkerFn>,
    pub on_message: Option<MessageFn>,
    pub on_worker_exit: Option<WorkerFn>,
    pub on_worker_stop: Option<WorkerFn>,
    pub on_worker_message: Option<EventFn>,
    pub on_worker_not_found: Option<NotFoundFn>,
    pub main_loop: Option<MainLoopFn>,
}

impl PoolRef<'_> {
    /// Installs `hooks`' object and hooks on the pool.
    ///
    /// # Safety
    /// Every hook the pool then holds is sound to run on this pool with what the
    /// pool lends it: what a hook reads through the pool's `ptr` is what `ptr` then
    /// is, and stays live while the pool can run the hook.
    pub unsafe fn install(self, hooks: &Hooks) {
        // One write per field named: the pool's own code and its signal handlers
        // read the record meanwhile.
        if let Some(ptr) = hooks.ptr {
            self.set_ptr(ptr);
        }
        if hooks.on_start.is_some() {
            self.set_on_start(hooks.on_start);
        }
        if hooks.on_shutdown.is_some() {
            self.set_on_shutdown(hooks.on_shutdown);
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
    pub fn call_on_worker_error(self, worker: Worker<'_>, status: &crate::methods::ExitStatus) {
        if let Some(hook) = self.on_worker_error() {
            hook(self, worker, status);
        }
    }

    /// `onWorkerNotFound(pool, status)`; nothing without one.
    pub fn call_on_worker_not_found(self, status: &crate::methods::ExitStatus) {
        if let Some(hook) = self.on_worker_not_found() {
            hook(self, status);
        }
    }
}

/// What every export runs over (`crates/server/src/pool_native.rs`): the pool's
/// native objects, which this crate can't depend on, and its callbacks. Each takes
/// a [`PoolCall`] this crate filled over live objects.
pub trait PoolNative: Sync {
    /// One effect on the call's pool and worker.
    fn call(&self, call: &PoolCall, op: Op, arg: u64) -> i64;
    /// The pool's `callback`; it runs inside the trap.
    fn callback(&self, call: &mut PoolCall, callback: Callback);
    /// `sizeof(EventData)`.
    fn event_size(&self) -> usize;
    /// `ProcessPool::is_worker_running(worker)`.
    fn running(&self, call: &PoolCall) -> bool;
    /// Run the expired timers and dispatch the pending signals.
    fn end(&self);
    /// The stream listener's `accept()`: the connection in `socket`.
    fn accept(&self, call: &mut PoolCall);
    /// `socket->recv_sync(data, length, flags)`.
    fn recv(&self, call: &mut PoolCall);
    /// `socket->send_sync(data, length)`.
    fn send(&self, call: &mut PoolCall);
    /// `socket->free()` of the call's socket, which is not used again.
    fn close(&self, call: &PoolCall);
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
        head: &mut swoole_rs_corebase::DataHead,
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
    fn warn(&self, call: &PoolCall, warning: crate::pool::Warning);
}

/// An export's result when a callback bailed out: the caller raises it.
pub const RAISE: i64 = i64::MIN;

const SW_OK: i64 = swoole_rs_corebase::constants::ResultCode::SW_OK as i64;
const SW_ERR: i64 = swoole_rs_corebase::constants::ResultCode::SW_ERR as i64;

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

/// `swWorkerType`.
const SW_MASTER: u64 = 1;

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

/// `head`'s bytes, in its C layout.
#[must_use]
pub fn head_view(head: &DataHead) -> &[u8; EVENT_DATA] {
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
    // SAFETY: a live head of `EVENT_DATA` initialized bytes (no padding, checked
    // above), borrowed as long as the head; bytes have no alignment.
    unsafe { &*ptr::from_ref(head).cast() }
}

/// A pool effect, which `crates/server/src/pool_native.rs` answers; the process-wide
/// worker and signal effects run here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    HaveMsgqueue,
    AllocWorkers,
    NewMessageBox,
    NewPipes,
    NewPipe,
    MainLoop,
    NewMessageBus,
    Sndbuf,
    BusBuffer,
    WorkerType,
    UnlimitPipes,
    Fork,
    WorkerShutdown,
    EventAvailable,
    RemovePipe,
    ExitCallback,
    ReopenLogger,
    IsMaster,
    ReloadTimer,
    DontRestart,
    TimerInit,
    TimerFree,
    AddPipe,
    PipeHandler,
    EventWait,
    CurrentWorker,
    PushStop,
    SignalDispatch,
    TimerSelect,
    PopMessage,
    OnWorkerMessage,
    OnWorkerNotFound,
    WorkerError,
    OnAfterReload,
    KillTimer,
    Waitpid,
    FreePipes,
    FreeMessageBox,
    FreeMessageBus,
    FreeWorkers,
    WorkerInit,
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

/// One `ProcessPool` method, with its arguments, which [`run_op`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    CreateMessageBox(u64),
    CreateMessageBus,
    SetProtocol(Protocol),
    StartCheck,
    Start,
    Spawn,
    Stop,
    Detach,
    KillAll(c_int),
    ReopenLogger,
    Shutdown,
    Reload,
    DropReloadTask,
    /// `run_with_*_protocol`, or `run_async` for none.
    Main(Option<Protocol>),
    Wait,
    Destroy,
    SetType(u8),
    SetStartId(u16),
    SetMaxRequest {
        max: u32,
        grace: u32,
    },
    FreeMessageBox,
    Signal(c_int),
}

/// What a pool operation asks of native code.
pub(crate) trait PoolHost {
    fn op(&mut self, worker: *mut c_void, op: Op, arg: u64) -> i64;
    /// Run `callback` inside the trap: its result, or [`Bailed`].
    fn callback(&mut self, callback: Callback, worker: *mut c_void) -> Result<isize, Bailed>;
    /// Run the protocol's worker loop: `DONE` or `BAILED`.
    fn run_loop(&mut self, pool_loop: &PoolLoop<'_>, protocol: Protocol) -> Result<(), Bailed>;
    /// The native side, which tests leave out.
    fn native(&self) -> Option<&dyn PoolNative>;
    fn warn(&self) -> Warn;
    fn event_size(&self) -> usize;
    /// The native socket free.
    fn close(&self, socket: *mut c_void);
    /// `Channel::make(size, sizeof(EventData), SW_CHAN_LOCK | SW_CHAN_SHM)`: the
    /// pool's new message box, or null.
    fn new_box(&mut self, size: u64) -> *mut c_void;
    /// `message_box->push(message)`, under the box's lock: 0, or -1.
    fn push_box(&mut self, message: &[u8]) -> i64;
    /// Frees the pool's message box, if it has one, and forgets it.
    fn free_box(&mut self);
}

struct Native<'a> {
    native: &'a dyn PoolNative,
    pool: PoolRef<'a>,
}

impl PoolHost for Native<'_> {
    fn op(&mut self, worker: *mut c_void, op: Op, arg: u64) -> i64 {
        if let Op::Log(kind) = op {
            log(kind, arg);
            return 0;
        }
        #[cfg(feature = "upstream-symbols")]
        if let Some(result) = effect(op, arg) {
            return result;
        }
        // A native effect on the live pool this export runs over.
        self.native
            .call(&PoolCall::new(self.pool.raw().cast(), worker), op, arg)
    }

    fn callback(&mut self, callback: Callback, worker: *mut c_void) -> Result<isize, Bailed> {
        let mut call = PoolCall::new(self.pool.raw().cast(), worker);
        crate::pool::trapped(|| self.native.callback(&mut call, callback))?;
        Ok(call.result)
    }

    fn run_loop(&mut self, pool_loop: &PoolLoop<'_>, protocol: Protocol) -> Result<(), Bailed> {
        pool_run(self.native, pool_loop, protocol)
    }

    fn native(&self) -> Option<&dyn PoolNative> {
        Some(self.native)
    }

    fn warn(&self) -> Warn {
        crate::methods::reload_warn
    }

    fn event_size(&self) -> usize {
        self.native.event_size()
    }

    fn close(&self, socket: *mut c_void) {
        let mut call = PoolCall::new(self.pool.raw().cast(), ptr::null_mut());
        call.socket = socket;
        self.native.close(&call);
    }

    fn new_box(&mut self, size: u64) -> *mut c_void {
        let size = usize::try_from(size).unwrap_or(usize::MAX);
        new_message_box(size, self.native.event_size())
    }

    fn push_box(&mut self, message: &[u8]) -> i64 {
        self.pool
            .record()
            .message_box_channel()
            .map_or(
                SW_ERR,
                |channel| if channel.push(message) { SW_OK } else { SW_ERR },
            )
    }

    fn free_box(&mut self) {
        let channel = self
            .pool
            .message_box()
            .cast::<swoole_rs_memory::channel::Record>();
        if channel.is_null() {
            return;
        }
        // The field is null from here on, so nothing in this process reaches the box again.
        self.pool.set_message_box(ptr::null_mut());
        // SAFETY: the box `new_box` made (the field is crate-private), which the pool
        // has just forgotten.
        unsafe { swoole_rs_memory::swr_v1_memory_channel_close(channel) };
        // SAFETY: the mapping `new_box` made it in, freed once.
        unsafe { swoole_rs_memory::members::sw_shm_free(channel.cast()) };
    }
}

/// `Channel::make(size, maxlen, SW_CHAN_LOCK | SW_CHAN_SHM)`: a channel in fresh
/// shared memory, or null.
fn new_message_box(size: usize, maxlen: usize) -> *mut c_void {
    use swoole_rs_corebase::log;
    use swoole_rs_memory::members::Shm;
    /// `SW_CHAN_LOCK | SW_CHAN_SHM`.
    const FLAGS: c_int = 2 | 8;
    let total = swoole_rs_memory::swr_v1_memory_channel_bytes(size, maxlen);
    if total == 0 {
        return ptr::null_mut();
    }
    let mut memory = Shm::malloc(total);
    if memory.is_null() {
        log::warning(c"Channel::make", format_args!("alloc({size}) failed"));
        return ptr::null_mut();
    }
    let record = memory.as_ptr().cast();
    // SAFETY: fresh shared memory of the size the record needs.
    match unsafe { swoole_rs_memory::swr_v1_memory_channel_init(record, size, maxlen, FLAGS) } {
        Ok(()) => return memory.into_raw(),
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
    memory.free();
    ptr::null_mut()
}

/// The `OP_*` effects that are calls into the process-wide worker and signal state:
/// `None` for the rest, which native code does (the timer, event loop and wait
/// effects too, since crates/reactor and crates/timer depend on this crate).
#[cfg(feature = "upstream-symbols")]
fn effect(op: Op, arg: u64) -> Option<i64> {
    use crate::methods::{swoole_get_worker_type, swoole_set_worker_type};
    let int = arg.c_cast::<c_int>();
    Some(match op {
        Op::WorkerType => {
            swoole_set_worker_type(int);
            0
        }
        Op::IsMaster => i64::from(i64::from(swoole_get_worker_type()) == SW_MASTER.cast_signed()),
        Op::CurrentWorker => {
            let worker = crate::worker_globals::current_worker();
            worker.expose_provenance().cast_signed() as i64
        }
        Op::SignalDispatch => {
            crate::signal::swoole_signal_dispatch();
            0
        }
        // crates/corebase's fork, with its process bookkeeping.
        Op::Fork => i64::from(swoole_rs_corebase::swoole_fork(0)),
        _ => return None,
    })
}

/// `LOG_*`'s warning, under the `ProcessPool` method upstream logged it from.
fn log(kind: Log, arg: u64) {
    use swoole_rs_corebase::log::{error_log, sys_warning, warning};
    /// `SW_LOG_WARNING`, `SW_ERROR_OPERATION_NOT_SUPPORT`, `SW_ERROR_WRONG_OPERATION`.
    const WARNING: c_int = 4;
    const NOT_SUPPORT: c_int = 507;
    const WRONG_OPERATION: c_int = 509;
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
struct Pool<'a, H> {
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
            // SAFETY: a live pool (the handle's contract); only the field is read.
            unsafe { (*self.raw).$field }
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

/// [`PoolRef`]'s field writes, and [`Pool`]'s over its handle. The fields the pool's
/// operations trust are written only from this crate.
macro_rules! setters {
    (@set $vis:vis $field:ident / $set:ident: $ty:ty) => {
        #[doc = concat!("Sets the record's `", stringify!($field), "`.")]
        $vis fn $set(self, value: $ty) {
            // SAFETY: a live pool (the handle's contract); only the field is written.
            unsafe { (*self.raw).$field = value };
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
    // The `unsafe fn`s, whose caller says the pool holds them.
    pub on_message: Option<MessageFn>,
    pub on_worker_message: Option<EventFn>,
    pub workers: *mut WorkerRecord,
    pub pipes: *mut c_void,
    pub ipc: *mut PoolIpc,
    pub message_box: *mut c_void,
    pub message_bus: *mut c_void,
    pub reload_task: *mut ReloadTask,
    ;
    pub ptr: *mut c_void,
    pub started: bool,
    pub protocol_type: c_int,
    pub max_request: u32,
    pub max_request_grace: u32,
    pub scheduler_warning: bool,
    pub warning_time: i64,
    pub(crate) on_task: Option<TaskFn>,
    pub(crate) on_worker_error: Option<ErrorFn>,
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
    pub(crate) pipes / set_pipes: *mut c_void,
    pub(crate) registry / set_registry: Option<Box<WorkerRegistry>>,
    pub(crate) queue / set_queue: Option<Box<swoole_rs_ossupport::QueueRecord>>,
    pub(crate) ipc / set_ipc: *mut PoolIpc,
    pub(crate) message_box / set_message_box: *mut c_void,
    pub(crate) message_bus / set_message_bus: *mut c_void,
    pub(crate) reload_task / set_reload_task: *mut ReloadTask,
    pub kill_timer / set_kill_timer: Option<ArmedTimer>
    ;
    pub is_async / set_is_async: bool,
    pub warning_time / set_warning_time: i64,
    pub schedule_by_sysvmsg / set_schedule_by_sysvmsg: bool,
    // The object the hooks read and the hooks, which `install` writes together (and
    // the server's startup, for its event pool).
    pub(crate) ptr / set_ptr: *mut c_void,
    on_start / set_on_start: Option<PoolFn>,
    on_shutdown / set_on_shutdown: Option<PoolFn>,
    on_message / set_on_message: Option<MessageFn>,
    on_worker_exit / set_on_worker_exit: Option<WorkerFn>,
    on_task / set_on_task: Option<TaskFn>,
    on_worker_start / set_on_worker_start: Option<WorkerFn>,
    on_worker_stop / set_on_worker_stop: Option<WorkerFn>,
    on_worker_message / set_on_worker_message: Option<EventFn>,
    on_worker_not_found / set_on_worker_not_found: Option<NotFoundFn>,
    main_loop / set_main_loop: Option<MainLoopFn>,
);

/// A native op's pointer result.
fn pointer<T>(address: i64) -> *mut T {
    ptr::with_exposed_provenance_mut(usize::try_from(address).unwrap_or(0))
}

fn exited(pid: c_int, status: c_int) -> u64 {
    u64::from(pid.cast_unsigned()) | (u64::from(status.cast_unsigned()) << 32)
}

impl<H: PoolHost> Pool<'_, H> {
    fn op(&mut self, op: Op, arg: u64) -> i64 {
        self.h.op(ptr::null_mut(), op, arg)
    }

    fn on(&mut self, worker: *mut WorkerRecord, op: Op, arg: u64) -> i64 {
        self.h.op(worker.cast(), op, arg)
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
    fn create(&mut self, worker_num: u32, key: c_int, mode: u8) -> i64 {
        let mode = IpcMode::from_code(mode.into());
        if mode == IpcMode::MsgQueue && self.op(Op::HaveMsgqueue, 0) == 0 {
            self.op(Op::Log(Log::NoSysvmsg), 0);
            return SW_ERR;
        }
        // The count goes with the records, so `worker_num` never counts past `workers`.
        let workers = pointer::<WorkerRecord>(self.op(Op::AllocWorkers, worker_num.into()));
        if workers.is_null() {
            self.op(Op::Log(Log::Alloc), 0);
            return SW_ERR;
        }
        self.set_workers(workers);
        self.set_worker_num(worker_num);
        if self.create_message_box(SW_MESSAGE_BOX_SIZE) < 0 {
            return SW_ERR;
        }
        match mode {
            IpcMode::MsgQueue => {
                self.set_use_msgqueue(true);
                self.set_msgqueue_key(key);
                let Some(queue) = swoole_rs_ossupport::QueueRecord::open(key) else {
                    return SW_ERR;
                };
                self.set_queue(Some(queue));
            }
            IpcMode::UnixSock => {
                let pipes = pointer(self.op(Op::NewPipes, 0));
                self.set_pipes(pipes);
                for index in 0..worker_num {
                    if self.on(self.worker(index), Op::NewPipe, 0) == 0 {
                        // The failed pipe freed the list, and with it the pipes of
                        // the workers before this one.
                        for made in 0..index {
                            self.worker_at(made).clear_pipes();
                        }
                        self.set_pipes(ptr::null_mut());
                        return SW_ERR;
                    }
                }
            }
            IpcMode::Socket => self.set_use_socket(true),
            IpcMode::None => {}
        }
        self.set_registry(Some(Box::default()));
        self.set_ipc(pool_ipc_new(mode == IpcMode::Socket));
        self.set_ipc_mode(mode);
        self.set_protocol(Protocol::Task);
        self.set_max_packet_size(packet_limit(self.use_msgqueue(), SW_INPUT_BUFFER_SIZE));
        self.set_max_wait_time(SW_WORKER_MAX_WAIT_TIME);
        for index in 0..worker_num {
            self.worker_at(index).join(self.p);
        }
        SW_OK
    }

    /// `ProcessPool::create_message_box`.
    fn create_message_box(&mut self, size: u64) -> i64 {
        let message_box = self.h.new_box(size);
        if message_box.is_null() {
            return SW_ERR;
        }
        self.set_message_box(message_box);
        SW_OK
    }

    /// `ProcessPool::create_message_bus`.
    fn create_message_bus(&mut self) -> i64 {
        if self.ipc_mode() != IpcMode::UnixSock {
            self.op(Op::Log(Log::NeedsUnixsock), 0);
            return SW_ERR;
        }
        if !self.message_bus().is_null() {
            self.op(Op::Log(Log::BusExists), 0);
            return SW_ERR;
        }
        let bus = pointer::<c_void>(self.op(Op::NewMessageBus, 0));
        if bus.is_null() {
            self.op(Op::Log(Log::Alloc), 1);
            return SW_ERR;
        }
        self.set_message_bus(bus);
        #[cfg(target_os = "linux")]
        let size = {
            // The largest unix dgram the pipes carry.
            let sndbuf = self.on(self.worker(0), Op::Sndbuf, 0);
            let sndbuf = if sndbuf < 0 {
                SW_IPC_MAX_SIZE.cast_signed()
            } else {
                sndbuf
            };
            sndbuf.min(SW_IPC_BUFFER_MAX_SIZE).cast_unsigned()
        };
        #[cfg(not(target_os = "linux"))]
        let size = SW_IPC_MAX_SIZE;
        if self.op(Op::BusBuffer, size) == 0 {
            SW_ERR
        } else {
            SW_OK
        }
    }

    /// `ProcessPool::set_protocol`: how the workers receive (`crate::pool`'s loops).
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
    fn set_protocol(&mut self, protocol: Protocol) {
        self.op(Op::MainLoop, u64::from(protocol.code().cast_unsigned()));
        self.set_protocol_type(protocol.code());
    }

    /// `ProcessPool::start_check`.
    fn start_check(&mut self) -> i64 {
        if self.ipc_mode() == IpcMode::Socket
            && self
                .with_ipc(|ipc| swr_v1_process_pool_ipc_listener(ipc).is_null())
                .unwrap_or(true)
        {
            self.op(Op::Log(Log::MustListen), 0);
            return SW_ERR;
        }
        self.set_running(true);
        self.set_started(true);
        self.set_master_pid(getpid());
        self.op(Op::WorkerType, SW_MASTER);
        if self.is_async() {
            self.op(Op::MainLoop, MAIN_ASYNC);
        }
        for index in 0..self.worker_num() {
            let worker = self.worker_at(index);
            let id = u32::from(self.start_id()) + index;
            worker.join(self.p);
            worker.set_id(id);
            worker.set_kind(self.kind());
            self.on(worker.raw(), Op::UnlimitPipes, 0);
        }
        SW_OK
    }

    /// `ProcessPool::start`.
    fn start(&mut self) -> Result<i64, Bailed> {
        if self.start_check() < 0 {
            return Ok(SW_ERR);
        }
        if self.on_start().is_some() {
            self.h.callback(Callback::Start, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            if self.spawn(self.worker(index))? < 0 {
                return Ok(SW_ERR);
            }
        }
        Ok(SW_OK)
    }

    /// `ProcessPool::spawn`: fork `worker`, which runs its loop and exits.
    /// -1 for a `worker` that is none of the pool's ([`PoolRecord::own`]).
    fn spawn(&mut self, worker: *mut WorkerRecord) -> Result<i64, Bailed> {
        let Some(old) = self.record().own(worker).map(Worker::pid) else {
            return Ok(SW_ERR);
        };
        self.with_registry(|registry| registry.unregister(old));
        let pid = self.on(worker, Op::Fork, 0);
        if pid == 0 {
            self.child(worker)?;
            return Ok(0);
        }
        let pid = c_int::try_from(pid).unwrap_or(-1);
        if let Some(worker) = self.record().own(worker) {
            worker.set_pid(pid);
        }
        self.with_registry(|registry| {
            registry.register(pid, worker.cast(), 0);
        });
        Ok(pid.into())
    }

    /// The forked worker: its callbacks and loop, then `exit`.
    fn child(&mut self, worker: *mut WorkerRecord) -> Result<(), Bailed> {
        self.on(worker, Op::WorkerInit, 0);
        if self.is_async() && self.op(Op::EventInit, 0) < 0 {
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
    fn stop(&mut self, worker: *mut WorkerRecord) {
        self.on(worker, Op::WorkerShutdown, 0);
        if self.op(Op::EventAvailable, 0) == 0 {
            return;
        }
        self.on(worker, Op::RemovePipe, 1);
        if self.on_worker_exit().is_some() {
            self.on(worker, Op::ExitCallback, 0);
        }
    }

    /// `ProcessPool::detach`.
    fn detach(&mut self) -> bool {
        if !self.running() || self.message_box().is_null() {
            return false;
        }
        let worker = pointer(self.op(Op::CurrentWorker, 0));
        self.on(worker, Op::WorkerShutdown, 0);
        if self.is_async() {
            self.on(worker, Op::RemovePipe, 0);
        }
        if self.on(worker, Op::PushStop, 0) < 0 {
            return false;
        }
        self.set_running(false);
        true
    }

    /// `Swoole\Process\Pool`'s signal handler (upstream's
    /// `process_pool_signal_handler`), in `worker` or (null) the master.
    fn signal(&mut self, signo: c_int, worker: *mut WorkerRecord) -> Result<(), Bailed> {
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
            _ if crate::is_sigrtmin(signo) => self.reopen_logger(),
            _ => {}
        }
        Ok(())
    }

    /// `ProcessPool::kill_all_workers`.
    fn kill_all_workers(&mut self, signo: c_int) {
        for index in 0..self.worker_num() {
            kill(self.worker_at(index).pid(), signo);
        }
    }

    /// `ProcessPool::reopen_logger`.
    fn reopen_logger(&mut self) {
        self.op(Op::ReopenLogger, 0);
        if self.op(Op::IsMaster, 0) != 0 {
            self.kill_all_workers(libc::SIGWINCH);
        }
    }

    /// `ProcessPool::shutdown`.
    fn shutdown(&mut self) -> bool {
        if self.op(Op::IsMaster, 0) != 0 {
            self.set_running(false);
            true
        } else {
            kill(self.master_pid(), libc::SIGTERM) == 0
        }
    }

    /// `ProcessPool::reload`.
    fn reload(&mut self) -> Result<bool, Bailed> {
        if !self.reload_task().is_null() {
            return Ok(false);
        }
        if self.on_before_reload().is_some() {
            self.h.callback(Callback::BeforeReload, ptr::null_mut())?;
        }
        self.set_reload_task(Box::into_raw(Box::<ReloadTask>::default()));
        if self.max_wait_time() != 0 {
            self.op(Op::ReloadTimer, 0);
        }
        self.set_reload_count(self.reload_count() + 1);
        self.set_reload_init(true);
        self.set_reload_last_time(swoole_rs_corebase::time_now());
        Ok(true)
    }

    /// `ProcessPool::drop_reload_task`: free the reload, and its timer with it.
    fn drop_reload_task(&mut self) {
        let task = self.reload_task();
        self.set_reload_task(ptr::null_mut());
        if !task.is_null() {
            // SAFETY: the pool's reload, boxed by `reload` and freed once.
            drop(unsafe { Box::from_raw(task) });
        }
    }

    /// `ProcessPool::at_worker_enter`.
    fn at_worker_enter(&mut self, worker: *mut WorkerRecord) {
        self.on(worker, Op::DontRestart, 0);
        if self.ipc_mode() == IpcMode::UnixSock {
            self.op(Op::TimerInit, 0);
        }
    }

    /// `ProcessPool::run_with_*_protocol` and `run_async`: the worker's loop; -1 for
    /// a `worker` that is none of the pool's.
    fn main(&mut self, worker: *mut WorkerRecord, protocol: Option<Protocol>) -> i64 {
        let Some(protocol) = protocol else {
            return self.run_async(worker);
        };
        if protocol == Protocol::Message {
            if self.ipc_mode() != IpcMode::UnixSock {
                self.op(Op::Log(Log::NeedsUnixsock), 1);
                return SW_ERR;
            }
            // The loop reads the bus: without one (it couldn't be made) there's no loop.
            if self.message_bus().is_null() && self.create_message_bus() < 0 {
                return SW_ERR;
            }
        }
        if self.ipc().is_null() {
            // A pool `create` never made.
            return SW_ERR;
        }
        let Some(own) = self.p.worker_of(worker) else {
            // None of the pool's workers.
            return SW_ERR;
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
        if self.h.run_loop(&pool_loop, protocol).is_err() {
            return RAISE;
        }
        self.op(Op::TimerFree, 0);
        SW_OK
    }

    /// `ProcessPool::run_async`.
    fn run_async(&mut self, worker: *mut WorkerRecord) -> i64 {
        if self.ipc_mode() == IpcMode::UnixSock && self.on_message().is_some() {
            self.on(worker, Op::AddPipe, 0);
            if self.message_bus().is_null() {
                let size = self.max_packet_size();
                self.with_ipc(|ipc| pool_ipc_prepare(ipc, size));
                self.op(Op::PipeHandler, 0);
            } else {
                self.op(Op::PipeHandler, 1);
            }
        }
        self.op(Op::EventWait, 0)
    }

    /// `ProcessPool::wait`'s per-iteration service: signals, timers, then the
    /// workers' messages (a stopped worker is replaced and its pid detached).
    fn service(&mut self) {
        self.op(Op::SignalDispatch, 0);
        self.op(Op::TimerSelect, 0);
        if !self.read_message() {
            return;
        }
        loop {
            // The popped EventData, in native's buffer.
            let message = pointer::<u8>(self.op(Op::PopMessage, 0)).cast_const();
            if message.is_null() {
                break;
            }
            if !self.running() {
                continue;
            }
            // SAFETY: the EventData just popped: its head, then data that holds at
            // least a WorkerStopMessage's pid.
            let event =
                unsafe { std::slice::from_raw_parts(message, EVENT_DATA + size_of::<c_int>()) };
            let kind = event.get(EVENT_TYPE).copied();
            if kind != Some(WORKER_MESSAGE_STOP) && self.on_worker_message().is_some() {
                self.op(Op::OnWorkerMessage, 0);
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
            // A bailout in the new worker's callbacks leaves at once; these frames own nothing.
            if self.spawn(worker).is_err() {
                crate::pool::raise_bailout();
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
        // A bailout in the new worker's callbacks leaves at once; these frames own nothing.
        if self.spawn(worker).is_err() {
            crate::pool::raise_bailout();
        }
        true
    }

    /// `ProcessPool::wait`: reap and restart workers until the pool stops, then
    /// stop them all.
    fn wait(&mut self) -> Result<i64, Bailed> {
        reaper::run(self);

        self.set_running(false);
        self.drop_reload_task();
        if self.on_shutdown().is_some() {
            self.h.callback(Callback::Shutdown, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            let pid = self.worker_at(index).pid();
            if kill(pid, libc::SIGTERM) < 0 {
                self.op(Op::Log(Log::KillFailed), u64::from(pid.cast_unsigned()));
            }
        }
        if self.max_wait_time() != 0 {
            self.op(Op::KillTimer, 0);
        }
        // Upstream waits for the first worker only.
        if self.worker_num() > 0 {
            let pid = self.worker_at(0).pid();
            if self.op(Op::Waitpid, u64::from(pid.cast_unsigned())) < 0 {
                self.op(Op::Log(Log::WaitpidFailed), u64::from(pid.cast_unsigned()));
            }
        }
        self.set_started(false);
        Ok(SW_OK)
    }

    /// `ProcessPool::push_message(msg)`: the `EventData` with `head` and `data` into
    /// the message box, then wake the master.
    fn push_event(&mut self, head: &DataHead, data: &[u8]) -> i64 {
        if self.message_box().is_null() || data.len() > self.h.event_size() - EVENT_DATA {
            return SW_ERR;
        }
        let head = DataHead {
            len: u32::try_from(data.len()).unwrap_or(u32::MAX),
            ..*head
        };
        let message = [&head_view(&head)[..], data].concat();
        if self.h.push_box(&message) < 0 {
            return SW_ERR;
        }
        kill(self.master_pid(), libc::SIGIO).into()
    }

    /// `ProcessPool::push_message(type, data, length)`.
    fn push(&mut self, kind: u8, data: &[u8]) -> i64 {
        if self.message_box().is_null() {
            return SW_ERR;
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
    /// listener, or a worker's pipe or queue (scheduled when `*worker_id` is
    /// negative). An `event` shorter than its head's length is refused.
    fn dispatch(&mut self, event: &[u8], worker_id: &mut c_int, blocking: bool) -> i64 {
        let Some(&len) = event.get(EVENT_LEN..).and_then(<[u8]>::first_chunk) else {
            return SW_ERR;
        };
        let size = u32::from_ne_bytes(len).saturating_add(HEAD_SIZE);
        let Some(event) = event.get(..size as usize) else {
            return SW_ERR;
        };
        if self.use_socket() {
            let send = |ipc: Option<&PoolIpc>| {
                crate::pool::swr_v1_process_pool_ipc_send(ipc, event, blocking)
            };
            let sent = self
                .with_ipc(|ipc| send(Some(ipc)))
                .unwrap_or_else(|| send(None));
            return if sent { SW_OK } else { SW_ERR };
        }
        if *worker_id < 0 {
            *worker_id = if self.schedule_by_sysvmsg() {
                0
            } else {
                self.schedule().cast_signed()
            };
        }
        let Some(native) = self.h.native() else {
            return SW_ERR;
        };
        let workers = |index| {
            u32::try_from(index)
                .ok()
                .filter(|&index| index < self.worker_num())
                .map(|index| self.worker_at(index))
        };
        let sent = crate::pool::pool_dispatch(
            native,
            self.p,
            workers,
            self.start_id(),
            self.use_msgqueue(),
            worker_id,
            event,
            blocking,
        );
        if sent { SW_OK } else { SW_ERR }
    }

    /// `ProcessPool::schedule`: the next idle worker, round robin; the last one
    /// tried, with the scheduler warning set, when all are busy.
    fn schedule(&mut self) -> u32 {
        let round = &self.record().round_id;
        let (target, warning) = crate::pool::pool_schedule(self.worker_num(), round, |target| {
            crate::pool::idle(self.worker_at(target))
        });
        if warning {
            self.set_scheduler_warning(true);
        }
        target
    }

    /// `ProcessPool::destroy`.
    fn destroy(&mut self) {
        self.set_kill_timer(None);
        if !self.pipes().is_null() {
            self.op(Op::FreePipes, 0);
            self.set_pipes(ptr::null_mut());
        }
        self.set_queue(None);
        let ipc = self.ipc();
        if !ipc.is_null() {
            // SAFETY: the pool's IPC state, the box `pool_ipc_new` leaked (the field is
            // crate-private), taken back once: the field is cleared next.
            let ipc = unsafe { Box::from_raw(ipc) };
            // The native close frees its listener.
            pool_ipc_free(*ipc, |socket| self.h.close(socket));
        }
        self.set_ipc(ptr::null_mut());
        self.set_registry(None);
        self.h.free_box();
        if !self.message_bus().is_null() {
            self.op(Op::FreeMessageBus, 0);
            self.set_message_bus(ptr::null_mut());
        }
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

    fn active_reload(&mut self) -> Option<&mut ReloadTask> {
        // SAFETY: the reload `reload` started, live until `drop_reload_task`.
        unsafe { Pool::reload_task(self).as_mut() }
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
        if let Some(task) = self.active_reload() {
            for (pid, id) in workers {
                task.add(pid, id);
            }
        }
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
fn with<T>(
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

fn raise_or(result: Result<i64, Bailed>) -> i64 {
    result.unwrap_or(RAISE)
}

/// `ProcessPool::create(worker_num, key, mode)`.
pub fn swr_v1_process_pool_create(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker_num: u32,
    key: c_int,
    mode: u8,
) -> i64 {
    with(native, pool, |p| p.create(worker_num, key, mode))
}

/// `ProcessPool::push_message(type, data, length)`.
pub fn swr_v1_process_pool_push(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    kind: u8,
    data: &[u8],
) -> i64 {
    with(native, pool, |p| p.push(kind, data))
}

/// `ProcessPool::push_message(EventData *)` of the event with `head` and `data`.
pub fn swr_v1_process_pool_push_event(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    head: &DataHead,
    data: &[u8],
) -> i64 {
    with(native, pool, |p| p.push_event(head, data))
}

/// `ProcessPool::dispatch` (`blocking`: `dispatch_sync`) of the `EventData` at the
/// start of `event`.
pub fn swr_v1_process_pool_dispatch_event(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    event: &[u8],
    worker_id: &mut c_int,
    blocking: bool,
) -> i64 {
    with(native, pool, |p| p.dispatch(event, worker_id, blocking))
}

/// `swoole_get_worker_symbol`: the log's mark for a worker type (`swWorkerType`).
#[must_use]
pub extern "C" fn swr_v1_worker_symbol(kind: c_int) -> std::ffi::c_char {
    let symbol = match kind {
        1 => b'#',
        3 => b'$',
        2 => b'*',
        4 => b'^',
        5 => b'@',
        _ => b'%',
    };
    std::ffi::c_char::from_ne_bytes([symbol])
}

/// `ProcessPool::run_with_*_protocol`, or `run_async` for no `protocol`:
/// `worker`'s loop in `pool`. The loop's return value, or [`RAISE`]; -1 when `worker`
/// is none of the pool's ([`PoolRef::worker_of`]).
#[must_use]
pub fn pool_run_main(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker: Worker<'_>,
    protocol: Option<Protocol>,
) -> i64 {
    match pool.worker_of(worker.raw()) {
        Some(worker) => run_op(native, pool, Some(worker), Run::Main(protocol)),
        None => SW_ERR,
    }
}

/// One `ProcessPool` method on `pool` and, for those over one, `worker`,
/// one of the pool's ([`PoolRef::worker_of`]): the C++ method's return value, or
/// [`RAISE`].
pub(crate) fn run_op(
    native: &dyn PoolNative,
    pool: PoolRef<'_>,
    worker: Option<Worker<'_>>,
    run: Run,
) -> i64 {
    let worker = worker.map_or(ptr::null_mut(), Worker::raw);
    with(native, pool, |p| p.run(worker, run))
}

impl<H: PoolHost> Pool<'_, H> {
    fn run(&mut self, worker: *mut WorkerRecord, run: Run) -> i64 {
        match run {
            Run::CreateMessageBox(size) => self.create_message_box(size),
            Run::CreateMessageBus => self.create_message_bus(),
            Run::SetProtocol(protocol) => {
                self.set_protocol(protocol);
                SW_OK
            }
            Run::StartCheck => self.start_check(),
            Run::Start => raise_or(self.start()),
            Run::Spawn => raise_or(self.spawn(worker)),
            Run::Stop => {
                self.stop(worker);
                SW_OK
            }
            Run::Detach => self.detach().into(),
            Run::KillAll(signo) => {
                self.kill_all_workers(signo);
                SW_OK
            }
            Run::ReopenLogger => {
                self.reopen_logger();
                SW_OK
            }
            Run::Shutdown => self.shutdown().into(),
            Run::Reload => raise_or(self.reload().map(i64::from)),
            Run::DropReloadTask => {
                self.drop_reload_task();
                SW_OK
            }
            Run::Main(protocol) => self.main(worker, protocol),
            Run::Wait => raise_or(self.wait()),
            Run::Destroy => {
                self.destroy();
                SW_OK
            }
            Run::SetType(kind) => {
                self.set_kind(kind);
                for index in 0..self.worker_num() {
                    self.worker_at(index).set_kind(kind);
                }
                SW_OK
            }
            Run::SetStartId(start_id) => {
                self.set_start_id(start_id);
                for index in 0..self.worker_num() {
                    self.worker_at(index).set_id(u32::from(start_id) + index);
                }
                SW_OK
            }
            Run::Signal(signo) => raise_or(self.signal(signo, worker).map(|()| SW_OK)),
            Run::FreeMessageBox => {
                self.h.free_box();
                SW_OK
            }
            Run::SetMaxRequest { max, grace } => {
                self.set_max_request(max);
                self.set_max_request_grace(grace);
                SW_OK
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

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
        assert_eq!(size_of::<PoolRecord>(), 288);
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

    impl PoolHost for Script {
        fn op(&mut self, _: *mut c_void, op: Op, arg: u64) -> i64 {
            self.log.push((op, arg));
            if op == Op::KillTimer
                && let Some(mut record) = self.record
            {
                let timer = ArmedTimer::new(1, KILL_TIMER_ID, crate::armed_timer::tests::record);
                // SAFETY: the fixture's record, which outlives its pool.
                unsafe { record.as_mut() }.kill_timer = Some(timer);
            }
            self.answers.get(&op).copied().unwrap_or(1)
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
        fn close(&self, _: *mut c_void) {}
        fn new_box(&mut self, size: u64) -> *mut c_void {
            pointer(self.op(ptr::null_mut(), Op::NewMessageBox, size))
        }
        fn push_box(&mut self, message: &[u8]) -> i64 {
            self.op(ptr::null_mut(), Op::BoxPush, message.len() as u64)
        }
        fn free_box(&mut self) {
            if let Some(mut record) = self.record
                // SAFETY: the fixture's record, which outlives its pool.
                && !unsafe { record.as_ref() }.message_box.is_null()
            {
                self.op(ptr::null_mut(), Op::FreeMessageBox, 0);
                // SAFETY: as above.
                unsafe { record.as_mut() }.message_box = ptr::null_mut();
            }
        }
    }

    fn close(_: *mut c_void) {}

    fn warn(_: crate::reload_task::KillWarning, _: c_int, _: u32) {}

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
            self.record.workers = self.workers.as_mut_ptr();
            // `create` takes the workers `Op::AllocWorkers` answers.
            let workers = self.workers.as_mut_ptr().expose_provenance().cast_signed() as i64;
            script.answers.entry(Op::AllocWorkers).or_insert(workers);
            self.record.worker_num = u32::try_from(self.workers.len()).unwrap();
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
        let _children = crate::wait::tests::CHILDREN
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
        record(&mut pool).max_wait_time = 3;
        assert!(matches!(pool.wait(), Ok(SW_OK)));
        for child in &mut children {
            child.wait().unwrap();
        }
        assert!(ops(&pool).contains(&Op::KillTimer));
        assert!(record(&mut pool).kill_timer.is_some());
        assert_eq!(crate::armed_timer::tests::cancelled(), []);
        drop(pool);
        drop(fixture);
        assert_eq!(crate::armed_timer::tests::cancelled(), [KILL_TIMER_ID]);
    }

    #[test]
    fn destroying_the_pool_deletes_an_armed_kill_timer() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        let timer = ArmedTimer::new(1, KILL_TIMER_ID, crate::armed_timer::tests::record);
        record(&mut pool).kill_timer = Some(timer);
        pool.destroy();
        assert!(record(&mut pool).kill_timer.is_none());
        assert_eq!(crate::armed_timer::tests::cancelled(), [KILL_TIMER_ID]);
    }

    #[test]
    fn create_refuses_a_queue_without_sysvmsg() {
        let mut fixture = Fixture::new(0);
        let mut script = Script::default();
        script.answers.insert(Op::HaveMsgqueue, 0);
        let mut pool = fixture.pool(script);
        assert_eq!(pool.create(2, 0, IpcMode::MsgQueue.code()), SW_ERR);
        assert_eq!(ops(&pool), [Op::HaveMsgqueue, Op::Log(Log::NoSysvmsg)]);
    }

    #[test]
    fn create_makes_a_pipe_per_worker_and_sets_the_defaults() {
        let mut fixture = Fixture::new(3);
        let mut pool = fixture.pool(Script::default());
        assert_eq!(pool.create(3, 0, IpcMode::UnixSock.code()), SW_OK);
        assert_eq!(
            ops(&pool),
            [
                Op::AllocWorkers,
                Op::NewMessageBox,
                Op::NewPipes,
                Op::NewPipe,
                Op::NewPipe,
                Op::NewPipe,
                Op::MainLoop
            ]
        );
        assert_eq!(pool.ipc_mode(), IpcMode::UnixSock);
        assert_eq!(record(&mut pool).protocol_type, Protocol::Task.code());
        assert_eq!(pool.max_packet_size(), SW_INPUT_BUFFER_SIZE);
        assert_eq!(pool.max_wait_time(), SW_WORKER_MAX_WAIT_TIME);
        assert!(!pool.p.registry().is_null());
        pool.destroy();
        assert!(pool.p.registry().is_null() && pool.ipc().is_null());
    }

    #[test]
    fn create_stops_at_the_first_pipe_that_fails() {
        let mut fixture = Fixture::new(3);
        let mut script = Script::default();
        script.answers.insert(Op::NewPipe, 0);
        let mut pool = fixture.pool(script);
        assert_eq!(pool.create(3, 0, IpcMode::UnixSock.code()), SW_ERR);
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
        assert_eq!(pool.create(1, 0, 9), SW_OK);
        assert_eq!(pool.ipc_mode(), IpcMode::None);
        pool.destroy();
    }

    #[test]
    fn start_check_numbers_the_workers_from_start_id() {
        let mut fixture = Fixture::new(2);
        let mut pool = fixture.pool(Script::default());
        pool.set_start_id(5);
        pool.set_kind(4);
        assert_eq!(pool.start_check(), SW_OK);
        assert!(pool.running() && record(&mut pool).started);
        assert_eq!(pool.master_pid(), getpid());
        // SAFETY: the fixture's records.
        unsafe {
            assert_eq!(Worker::new(pool.worker(1)).id(), 6);
            assert_eq!(Worker::new(pool.worker(1)).kind(), 4);
        }
    }

    #[test]
    fn a_socket_pool_must_listen_first() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IpcMode::Socket);
        pool.set_ipc(pool_ipc_new(true));
        assert_eq!(pool.start_check(), SW_ERR);
        assert_eq!(ops(&pool), [Op::Log(Log::MustListen)]);
        pool.destroy();
    }

    #[test]
    fn a_bailout_in_on_start_spawns_nothing() {
        extern "C-unwind" fn on_start(_: PoolRef<'_>) {}
        let mut fixture = Fixture::new(2);
        let script = Script {
            bail: Some(Callback::Start),
            ..Script::default()
        };
        let mut pool = fixture.pool(script);
        record(&mut pool).on_start = Some(on_start);
        assert!(pool.start().is_err());
        assert!(!ops(&pool).contains(&Op::Fork));
    }

    #[test]
    fn spawn_registers_the_new_pid() {
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(Op::Fork, 4321);
        let mut pool = fixture.pool(script);
        pool.set_registry(Some(Box::default()));
        let worker = pool.worker(0);
        assert_eq!(pool.spawn(worker).ok(), Some(4321));
        assert_eq!(pool.worker_by_pid(4321), worker);
        // A respawn forgets the old pid.
        pool.h.answers.insert(Op::Fork, 4322);
        assert_eq!(pool.spawn(worker).ok(), Some(4322));
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
        assert!(pool.reload_task().is_null());
    }

    #[test]
    fn detach_needs_a_running_pool_with_a_message_box() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        assert!(!pool.detach());
        pool.set_running(true);
        pool.set_message_box(ptr::dangling_mut());
        record(&mut pool).is_async = true;
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
    fn stop_outside_an_event_loop_only_shuts_down() {
        extern "C-unwind" fn on_worker_exit(_: PoolRef<'_>, _: Worker<'_>) {}
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(Op::EventAvailable, 0);
        let mut pool = fixture.pool(script);
        record(&mut pool).on_worker_exit = Some(on_worker_exit);
        pool.stop(pool.worker(0));
        assert_eq!(ops(&pool), [Op::WorkerShutdown, Op::EventAvailable]);
    }

    #[test]
    fn the_message_protocol_needs_unix_sockets() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IpcMode::MsgQueue);
        assert_eq!(pool.main(pool.worker(0), Some(Protocol::Message)), SW_ERR);
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
        assert_eq!(pool.push(1, b"ab"), SW_ERR);
        record(&mut pool).message_box = ptr::dangling_mut();
        pool.h.answers.insert(Op::BoxPush, -1);
        assert_eq!(pool.push(1, b"ab"), SW_ERR);
        assert_eq!(ops(&pool), [Op::BoxPush]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no network sockets")]
    fn a_socket_pool_sends_a_dispatch_length_prefixed() {
        use std::io::Read;
        let path = std::env::temp_dir().join(format!("swr-pool-sync-{}.sock", std::process::id()));
        std::fs::remove_file(&path).unwrap_or_default();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let file = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        record(&mut pool).use_socket = true;
        let ipc = pool_ipc_new(true);
        // SAFETY: a live handle.
        crate::pool::swr_v1_process_pool_ipc_address(unsafe { &mut *ipc }, &file, 0);
        record(&mut pool).ipc = ipc;
        let mut event = [0u8; 64];
        event[EVENT_LEN] = 3;
        event[HEAD_SIZE as usize..][..3].copy_from_slice(b"abc");
        let mut worker_id = -1;
        // Blocking, then over a stream (upstream's async dispatch).
        for blocking in [true, false] {
            assert_eq!(pool.dispatch(&event, &mut worker_id, blocking), SW_OK);
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
        record(&mut pool).ipc = ptr::null_mut();
        // SAFETY: the handle made above; it has no listener to close.
        pool_ipc_free(*unsafe { Box::from_raw(ipc) }, close);
        // A pool whose listener is gone fails.
        let ipc = pool_ipc_new(true);
        // SAFETY: as above.
        crate::pool::swr_v1_process_pool_ipc_address(unsafe { &mut *ipc }, &file, 0);
        record(&mut pool).ipc = ipc;
        for blocking in [true, false] {
            assert_eq!(pool.dispatch(&event, &mut worker_id, blocking), SW_ERR);
        }
        record(&mut pool).ipc = ptr::null_mut();
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
        pool.set_registry(Some(Box::default()));
        assert!(!pool.reap(99, 0));
        assert_eq!(pool.h.log, [(Op::Log(Log::UnknownChild), 99)]);
        pool.destroy();
    }
}
