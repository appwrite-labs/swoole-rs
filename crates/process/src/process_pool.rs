//! `ProcessPool`'s lifecycle (`src/os/process_pool.cc`): create, start, spawn,
//! stop, detach, reload, the manager's wait and shutdown, destroy and the
//! worker-side entry into the loops of [`crate::pool`]. Rust reads and writes
//! the pool's fields in place through [`PoolRecord`], `ProcessPool`'s layout
//! (native locks it with `static_assert`s), and owns its registry, IPC state and
//! reload. Native code keeps the objects (queue, pipes, channel, message bus,
//! timers, reactor, fork) and the callbacks, and does each effect when Rust
//! asks through [`PoolNative::call`].
//!
//! A callback that can reach PHP runs through [`PoolNative::callback`] inside
//! the bailout trap; when it bails out, the export returns [`RAISE`] and its
//! native caller raises the bailout again once the Rust frames are gone.

use std::ffi::{c_int, c_void};
use std::ptr;

use crate::pool::{
    Bailed, PoolCall, PoolHooks, PoolIpc, PoolLoop, Step, swr_v1_process_pool_ipc_free,
    swr_v1_process_pool_ipc_listener, swr_v1_process_pool_ipc_new, swr_v1_process_pool_ipc_prepare,
    swr_v1_process_pool_run,
};
use crate::reaper::{RELOAD_KILL_ONE, RELOAD_NONE, ReapHost};
use crate::reload_task::{ReloadTask, Warn, swr_v1_process_reload_task_free};
use crate::worker_record::{
    WorkerRecord, set_pool, swr_v1_worker_id, swr_v1_worker_pid, swr_v1_worker_set_id,
    swr_v1_worker_set_pid, swr_v1_worker_set_type,
};
use crate::worker_registry::{
    WorkerRegistry, swr_v1_process_worker_registry_detach, swr_v1_process_worker_registry_free,
    swr_v1_process_worker_registry_lookup, swr_v1_process_worker_registry_new,
    swr_v1_process_worker_registry_reap_detached, swr_v1_process_worker_registry_register,
    swr_v1_process_worker_registry_unregister,
};

/// `swoole::ProcessPool`'s fields, in order. Rust owns this layout; the patch's
/// `static_assert`s lock the C++ struct to `swr_v1_pool_record`, its C mirror.
#[repr(C)]
pub struct PoolRecord {
    pub running: bool,
    pub reload_init: bool,
    pub read_message: bool,
    pub started: bool,
    pub schedule_by_sysvmsg: bool,
    pub is_async: bool,
    pub ipc_mode: u8,
    pub protocol_type: c_int,
    pub master_pid: c_int,
    pub max_wait_time: u32,
    pub reload_count: u64,
    pub reload_last_time: i64,
    pub kind: u8,
    pub start_id: u16,
    pub use_msgqueue: u8,
    pub use_socket: u8,
    pub max_packet_size: u32,
    pub msgqueue_key: c_int,
    pub worker_num: u32,
    pub max_request: u32,
    pub max_request_grace: u32,
    pub scheduler_warning: u8,
    pub warning_time: i64,
    pub on_start: *const c_void,
    pub on_shutdown: *const c_void,
    pub on_before_reload: *const c_void,
    pub on_after_reload: *const c_void,
    pub on_task: *const c_void,
    pub on_worker_start: *const c_void,
    pub on_message: *const c_void,
    pub on_worker_exit: *const c_void,
    pub on_worker_stop: *const c_void,
    pub on_worker_error: *const c_void,
    pub on_worker_message: *const c_void,
    pub on_worker_not_found: *const c_void,
    pub main_loop: *const c_void,
    pub round_id: u32,
    pub workers: *mut WorkerRecord,
    pub pipes: *mut c_void,
    pub registry: *mut WorkerRegistry,
    pub queue: *mut c_void,
    pub ipc: *mut PoolIpc,
    pub message_box: *mut c_void,
    pub message_bus: *mut c_void,
    pub reload_task: *mut ReloadTask,
    pub ptr: *mut c_void,
}

/// What native code lends every export.
#[repr(C)]
pub struct PoolNative {
    /// One effect (`OP_*`, `LOG_*`) on `pool` and `worker` (null when none).
    pub call:
        unsafe extern "C" fn(pool: *mut c_void, worker: *mut c_void, op: u32, arg: u64) -> i64,
    /// The callback `CB_*` in the call's `flags`, run inside the trap.
    pub callback: Step,
    /// The worker loops' hooks.
    pub hooks: *const PoolHooks,
    /// Reports a reload's failed or forced kill.
    pub warn: Warn,
    /// `sizeof(EventData)`.
    pub event_size: usize,
}

/// An export's result when a callback bailed out: the caller raises it.
pub const RAISE: i64 = i64::MIN;

const SW_OK: i64 = 0;
const SW_ERR: i64 = -1;

/// `swIPCMode`.
const IPC_NONE: u8 = 0;
const IPC_UNIXSOCK: u8 = 1;
const IPC_MSGQUEUE: u8 = 2;
const IPC_SOCKET: u8 = 3;

/// `swoole::ProtocolType`; `MAIN_ASYNC` is `OP_MAIN_LOOP`'s `run_async`.
const PROTOCOL_TASK: c_int = 1;
const PROTOCOL_STREAM: c_int = 2;
const PROTOCOL_MESSAGE: c_int = 3;
const MAIN_ASYNC: u64 = 4;

/// `swWorkerType`.
const SW_MASTER: u64 = 1;

/// `swoole_config.h`; native asserts each.
pub const SW_MESSAGE_BOX_SIZE: u64 = 65536;
pub const SW_INPUT_BUFFER_SIZE: u32 = 2 * 1024 * 1024;
pub const SW_WORKER_MAX_WAIT_TIME: u32 = 3;
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

// BEGIN NUMBERED: native/process/swoole_rs_process.h's SWR_POOL_*; a test keeps them equal.
pub const OP_HAVE_MSGQUEUE: u32 = 0;
pub const OP_ALLOC_WORKERS: u32 = 1;
pub const OP_NEW_MESSAGE_BOX: u32 = 2;
pub const OP_NEW_QUEUE: u32 = 3;
pub const OP_NEW_PIPES: u32 = 4;
pub const OP_NEW_PIPE: u32 = 5;
pub const OP_MAIN_LOOP: u32 = 6;
pub const OP_NEW_MESSAGE_BUS: u32 = 7;
pub const OP_SNDBUF: u32 = 8;
pub const OP_BUS_BUFFER: u32 = 9;
pub const OP_WORKER_TYPE: u32 = 10;
pub const OP_UNLIMIT_PIPES: u32 = 11;
pub const OP_FORK: u32 = 12;
pub const OP_WORKER_SHUTDOWN: u32 = 13;
pub const OP_EVENT_AVAILABLE: u32 = 14;
pub const OP_REMOVE_PIPE: u32 = 15;
pub const OP_EXIT_CALLBACK: u32 = 16;
pub const OP_REOPEN_LOGGER: u32 = 17;
pub const OP_IS_MASTER: u32 = 18;
pub const OP_RELOAD_TIMER: u32 = 19;
pub const OP_TIMER_DEL: u32 = 20;
pub const OP_DONT_RESTART: u32 = 21;
pub const OP_TIMER_INIT: u32 = 22;
pub const OP_TIMER_FREE: u32 = 23;
pub const OP_ADD_PIPE: u32 = 24;
pub const OP_PIPE_HANDLER: u32 = 25;
pub const OP_EVENT_WAIT: u32 = 26;
pub const OP_CURRENT_WORKER: u32 = 27;
pub const OP_PUSH_STOP: u32 = 28;
pub const OP_SIGNAL_DISPATCH: u32 = 29;
pub const OP_TIMER_SELECT: u32 = 30;
pub const OP_POP_MESSAGE: u32 = 31;
pub const OP_ON_WORKER_MESSAGE: u32 = 32;
pub const OP_ON_WORKER_NOT_FOUND: u32 = 33;
pub const OP_WORKER_ERROR: u32 = 34;
pub const OP_ON_AFTER_RELOAD: u32 = 35;
pub const OP_KILL_TIMER: u32 = 36;
pub const OP_WAITPID: u32 = 37;
pub const OP_FREE_PIPES: u32 = 38;
pub const OP_FREE_QUEUE: u32 = 39;
pub const OP_FREE_MESSAGE_BOX: u32 = 40;
pub const OP_FREE_MESSAGE_BUS: u32 = 41;
pub const OP_FREE_WORKERS: u32 = 42;
pub const OP_WORKER_INIT: u32 = 43;
pub const OP_EVENT_INIT: u32 = 44;
pub const OP_BOX_PUSH: u32 = 45;
pub const OP_STREAM_SEND: u32 = 46;
pub const OP_SYNC_SEND: u32 = 47;
pub const LOG_NO_SYSVMSG: u32 = 48;
pub const LOG_ALLOC: u32 = 49;
pub const LOG_NEEDS_UNIXSOCK: u32 = 50;
pub const LOG_BUS_EXISTS: u32 = 51;
pub const LOG_MUST_LISTEN: u32 = 52;
pub const LOG_UNKNOWN_CHILD: u32 = 53;
pub const LOG_WAIT_FAILED: u32 = 54;
pub const LOG_KILL_FAILED: u32 = 55;
pub const LOG_WAITPID_FAILED: u32 = 56;
pub const CB_START: c_int = 1;
pub const CB_BEFORE_RELOAD: c_int = 2;
pub const CB_SHUTDOWN: c_int = 3;
pub const CB_WORKER_START: c_int = 4;
pub const CB_MAIN_LOOP: c_int = 5;
pub const CB_WORKER_STOP: c_int = 6;
pub const RUN_CREATE_MESSAGE_BOX: u32 = 0;
pub const RUN_CREATE_MESSAGE_BUS: u32 = 1;
pub const RUN_SET_PROTOCOL: u32 = 2;
pub const RUN_START_CHECK: u32 = 3;
pub const RUN_START: u32 = 4;
pub const RUN_SPAWN: u32 = 5;
pub const RUN_STOP: u32 = 6;
pub const RUN_DETACH: u32 = 7;
pub const RUN_KILL_ALL: u32 = 8;
pub const RUN_REOPEN_LOGGER: u32 = 9;
pub const RUN_SHUTDOWN: u32 = 10;
pub const RUN_RELOAD: u32 = 11;
pub const RUN_DROP_RELOAD_TASK: u32 = 12;
pub const RUN_MAIN: u32 = 13;
pub const RUN_WAIT: u32 = 14;
pub const RUN_DESTROY: u32 = 15;
pub const RUN_SET_TYPE: u32 = 16;
pub const RUN_SET_START_ID: u32 = 17;
pub const RUN_SET_MAX_REQUEST: u32 = 18;
pub const RUN_PUSH_MESSAGE: u32 = 19;
// END NUMBERED

/// What a pool operation asks of native code.
pub(crate) trait PoolHost {
    fn op(&mut self, worker: *mut c_void, op: u32, arg: u64) -> i64;
    /// Run `CB_*` inside the trap: its result, or [`Bailed`].
    fn callback(&mut self, cb: c_int, worker: *mut c_void) -> Result<isize, Bailed>;
    /// Run the protocol's worker loop: `DONE` or `BAILED`.
    fn run_loop(&mut self, pool_loop: &PoolLoop, protocol: c_int) -> c_int;
    fn hooks(&self) -> *const PoolHooks;
    fn warn(&self) -> Warn;
    fn event_size(&self) -> usize;
    /// The hooks' socket free.
    fn close(&self) -> unsafe extern "C" fn(*mut c_void);
}

struct Native<'a> {
    native: &'a PoolNative,
    pool: *mut c_void,
}

impl PoolHost for Native<'_> {
    fn op(&mut self, worker: *mut c_void, op: u32, arg: u64) -> i64 {
        // SAFETY: a native effect on the live pool this export runs over.
        unsafe { (self.native.call)(self.pool, worker, op, arg) }
    }

    fn callback(&mut self, cb: c_int, worker: *mut c_void) -> Result<isize, Bailed> {
        let mut call = PoolCall::new(self.pool, worker);
        call.flags = cb;
        crate::pool::run(self.native.callback, &mut call)?;
        Ok(call.result)
    }

    fn run_loop(&mut self, pool_loop: &PoolLoop, protocol: c_int) -> c_int {
        // SAFETY: the loop describes this live pool, its worker and hooks.
        unsafe { swr_v1_process_pool_run(pool_loop, protocol) }
    }

    fn hooks(&self) -> *const PoolHooks {
        self.native.hooks
    }

    fn warn(&self) -> Warn {
        self.native.warn
    }

    fn event_size(&self) -> usize {
        self.native.event_size
    }

    fn close(&self) -> unsafe extern "C" fn(*mut c_void) {
        // SAFETY: native's static hooks.
        unsafe { (*self.native.hooks).close }
    }
}

/// One operation over a pool: its record and the native side.
struct Pool<H> {
    h: H,
    p: *mut PoolRecord,
}

macro_rules! getters {
    ($($field:ident: $ty:ty),* $(,)?) => {
        impl<H> Pool<H> {
            $(
                fn $field(&self) -> $ty {
                    // SAFETY: `p` is the live pool for this operation; read in place.
                    unsafe { (*self.p).$field }
                }
            )*
        }
    };
}

macro_rules! setters {
    ($($field:ident / $set:ident: $ty:ty),* $(,)?) => {
        impl<H> Pool<H> {
            $(
                fn $set(&mut self, value: $ty) {
                    // SAFETY: `p` is the live pool for this operation; written in place.
                    unsafe { (*self.p).$field = value };
                }
            )*
        }
    };
}

getters!(
    running: bool,
    reload_init: bool,
    read_message: bool,
    schedule_by_sysvmsg: bool,
    is_async: bool,
    ipc_mode: u8,
    master_pid: c_int,
    max_wait_time: u32,
    reload_count: u64,
    kind: u8,
    start_id: u16,
    use_msgqueue: u8,
    use_socket: u8,
    max_packet_size: u32,
    worker_num: u32,
    on_start: *const c_void,
    on_shutdown: *const c_void,
    on_before_reload: *const c_void,
    on_after_reload: *const c_void,
    on_message: *const c_void,
    on_worker_start: *const c_void,
    on_worker_stop: *const c_void,
    on_worker_exit: *const c_void,
    on_worker_message: *const c_void,
    on_worker_not_found: *const c_void,
    main_loop: *const c_void,
    workers: *mut WorkerRecord,
    pipes: *mut c_void,
    registry: *mut WorkerRegistry,
    queue: *mut c_void,
    ipc: *mut PoolIpc,
    message_box: *mut c_void,
    message_bus: *mut c_void,
    reload_task: *mut ReloadTask,
);

setters!(
    running / set_running: bool,
    reload_init / set_reload_init: bool,
    read_message / set_read_message: bool,
    started / set_started: bool,
    ipc_mode / set_ipc_mode: u8,
    protocol_type / set_protocol_type: c_int,
    master_pid / set_master_pid: c_int,
    max_wait_time / set_max_wait_time: u32,
    reload_count / set_reload_count: u64,
    reload_last_time / set_reload_last_time: i64,
    kind / set_kind: u8,
    start_id / set_start_id: u16,
    use_msgqueue / set_use_msgqueue: u8,
    use_socket / set_use_socket: u8,
    max_packet_size / set_max_packet_size: u32,
    msgqueue_key / set_msgqueue_key: c_int,
    worker_num / set_worker_num: u32,
    max_request / set_max_request: u32,
    max_request_grace / set_max_request_grace: u32,
    pipes / set_pipes: *mut c_void,
    registry / set_registry: *mut WorkerRegistry,
    queue / set_queue: *mut c_void,
    ipc / set_ipc: *mut PoolIpc,
    message_box / set_message_box: *mut c_void,
    message_bus / set_message_bus: *mut c_void,
    reload_task / set_reload_task: *mut ReloadTask,
);

fn time_now() -> i64 {
    // SAFETY: time(2) with no out-pointer.
    unsafe { libc::time(ptr::null_mut()) }
}

fn getpid() -> c_int {
    // SAFETY: getpid(2) cannot fail.
    unsafe { libc::getpid() }
}

fn kill(pid: c_int, signo: c_int) -> c_int {
    // SAFETY: kill(2) takes plain integers.
    unsafe { libc::kill(pid, signo) }
}

/// A native op's pointer result.
fn pointer<T>(address: i64) -> *mut T {
    ptr::with_exposed_provenance_mut(usize::try_from(address).unwrap_or(0))
}

fn exited(pid: c_int, status: c_int) -> u64 {
    u64::from(pid.cast_unsigned()) | (u64::from(status.cast_unsigned()) << 32)
}

impl<H: PoolHost> Pool<H> {
    fn op(&mut self, op: u32, arg: u64) -> i64 {
        self.h.op(ptr::null_mut(), op, arg)
    }

    fn on(&mut self, worker: *mut WorkerRecord, op: u32, arg: u64) -> i64 {
        self.h.op(worker.cast(), op, arg)
    }

    fn worker(&self, index: u32) -> *mut WorkerRecord {
        self.workers().wrapping_add(index as usize)
    }

    /// `get_worker_by_pid`: the pool's worker at `pid`, or null.
    fn worker_by_pid(&self, pid: c_int) -> *mut WorkerRecord {
        let registry = self.registry();
        if registry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: the pool's registry.
        unsafe { swr_v1_process_worker_registry_lookup(registry, pid) }.cast()
    }

    /// `ProcessPool::create`.
    fn create(&mut self, worker_num: u32, key: c_int, mut mode: u8) -> i64 {
        if mode == IPC_MSGQUEUE && self.op(OP_HAVE_MSGQUEUE, 0) == 0 {
            self.op(LOG_NO_SYSVMSG, 0);
            return SW_ERR;
        }
        self.set_worker_num(worker_num);
        if self.op(OP_ALLOC_WORKERS, 0) == 0 {
            self.op(LOG_ALLOC, 0);
            return SW_ERR;
        }
        if self.create_message_box(SW_MESSAGE_BOX_SIZE) < 0 {
            return SW_ERR;
        }
        match mode {
            IPC_MSGQUEUE => {
                self.set_use_msgqueue(1);
                self.set_msgqueue_key(key);
                if self.op(OP_NEW_QUEUE, 0) == 0 {
                    return SW_ERR;
                }
            }
            IPC_UNIXSOCK => {
                self.op(OP_NEW_PIPES, 0);
                for index in 0..worker_num {
                    if self.on(self.worker(index), OP_NEW_PIPE, 0) == 0 {
                        return SW_ERR;
                    }
                }
            }
            IPC_SOCKET => self.set_use_socket(1),
            _ => mode = IPC_NONE,
        }
        self.set_registry(swr_v1_process_worker_registry_new());
        self.set_ipc(swr_v1_process_pool_ipc_new(mode == IPC_SOCKET));
        self.set_ipc_mode(mode);
        self.set_protocol(PROTOCOL_TASK);
        self.set_max_packet_size(SW_INPUT_BUFFER_SIZE);
        self.set_max_wait_time(SW_WORKER_MAX_WAIT_TIME);
        for index in 0..worker_num {
            // SAFETY: one of the `worker_num` records just allocated.
            unsafe { set_pool(self.worker(index), self.p.cast()) };
        }
        SW_OK
    }

    /// `ProcessPool::create_message_box`.
    fn create_message_box(&mut self, size: u64) -> i64 {
        if self.op(OP_NEW_MESSAGE_BOX, size) == 0 {
            SW_ERR
        } else {
            SW_OK
        }
    }

    /// `ProcessPool::create_message_bus`.
    fn create_message_bus(&mut self) -> i64 {
        if self.ipc_mode() != IPC_UNIXSOCK {
            self.op(LOG_NEEDS_UNIXSOCK, 0);
            return SW_ERR;
        }
        if !self.message_bus().is_null() {
            self.op(LOG_BUS_EXISTS, 0);
            return SW_ERR;
        }
        if self.op(OP_NEW_MESSAGE_BUS, 0) == 0 {
            self.op(LOG_ALLOC, 1);
            return SW_ERR;
        }
        #[cfg(target_os = "linux")]
        let size = {
            // The largest unix dgram the pipes carry.
            let sndbuf = self.on(self.worker(0), OP_SNDBUF, 0);
            let sndbuf = if sndbuf < 0 {
                SW_IPC_MAX_SIZE.cast_signed()
            } else {
                sndbuf
            };
            sndbuf.min(SW_IPC_BUFFER_MAX_SIZE).cast_unsigned()
        };
        #[cfg(not(target_os = "linux"))]
        let size = SW_IPC_MAX_SIZE;
        if self.op(OP_BUS_BUFFER, size) == 0 {
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
    fn set_protocol(&mut self, protocol: c_int) {
        if !matches!(protocol, PROTOCOL_TASK | PROTOCOL_STREAM | PROTOCOL_MESSAGE) {
            std::process::abort();
        }
        self.op(OP_MAIN_LOOP, u64::from(protocol.cast_unsigned()));
        self.set_protocol_type(protocol);
    }

    /// `ProcessPool::start_check`.
    fn start_check(&mut self) -> i64 {
        if self.ipc_mode() == IPC_SOCKET
            // SAFETY: the pool's IPC state.
            && unsafe { swr_v1_process_pool_ipc_listener(self.ipc()) }.is_null()
        {
            self.op(LOG_MUST_LISTEN, 0);
            return SW_ERR;
        }
        self.set_running(true);
        self.set_started(true);
        self.set_master_pid(getpid());
        self.op(OP_WORKER_TYPE, SW_MASTER);
        if self.is_async() {
            self.op(OP_MAIN_LOOP, MAIN_ASYNC);
        }
        for index in 0..self.worker_num() {
            let worker = self.worker(index);
            let id = u32::from(self.start_id()) + index;
            // SAFETY: one of the pool's records.
            unsafe { set_pool(worker, self.p.cast()) };
            // SAFETY: as above.
            unsafe { swr_v1_worker_set_id(worker, id) };
            // SAFETY: as above.
            unsafe { swr_v1_worker_set_type(worker, self.kind()) };
            self.on(worker, OP_UNLIMIT_PIPES, 0);
        }
        SW_OK
    }

    /// `ProcessPool::start`.
    fn start(&mut self) -> Result<i64, Bailed> {
        if self.start_check() < 0 {
            return Ok(SW_ERR);
        }
        if !self.on_start().is_null() {
            self.h.callback(CB_START, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            if self.spawn(self.worker(index))? < 0 {
                return Ok(SW_ERR);
            }
        }
        Ok(SW_OK)
    }

    /// `ProcessPool::spawn`: fork `worker`, which runs its loop and exits.
    fn spawn(&mut self, worker: *mut WorkerRecord) -> Result<i64, Bailed> {
        // SAFETY: one of the pool's records.
        let old = unsafe { swr_v1_worker_pid(worker) };
        // SAFETY: the pool's registry, which `create` made.
        unsafe { swr_v1_process_worker_registry_unregister(self.registry(), old) };
        let pid = self.on(worker, OP_FORK, 0);
        if pid == 0 {
            self.child(worker)?;
            return Ok(0);
        }
        let pid = c_int::try_from(pid).unwrap_or(-1);
        // SAFETY: as above.
        unsafe { swr_v1_worker_set_pid(worker, pid) };
        // SAFETY: as above.
        unsafe { swr_v1_process_worker_registry_register(self.registry(), pid, worker.cast(), 0) };
        Ok(pid.into())
    }

    /// The forked worker: its callbacks and loop, then `exit`.
    fn child(&mut self, worker: *mut WorkerRecord) -> Result<(), Bailed> {
        self.on(worker, OP_WORKER_INIT, 0);
        if self.is_async() && self.op(OP_EVENT_INIT, 0) < 0 {
            // SAFETY: exit(3), as upstream.
            unsafe { libc::exit(254) };
        }
        if !self.on_worker_start().is_null() {
            self.h.callback(CB_WORKER_START, worker.cast())?;
        }
        let mut code = 0;
        if !self.main_loop().is_null() {
            code = c_int::try_from(self.h.callback(CB_MAIN_LOOP, worker.cast())?).unwrap_or(-1);
        }
        if !self.on_worker_stop().is_null() {
            self.h.callback(CB_WORKER_STOP, worker.cast())?;
        }
        // SAFETY: exit(3), as upstream.
        unsafe { libc::exit(code) }
    }

    /// `ProcessPool::stop`.
    fn stop(&mut self, worker: *mut WorkerRecord) {
        self.on(worker, OP_WORKER_SHUTDOWN, 0);
        if self.op(OP_EVENT_AVAILABLE, 0) == 0 {
            return;
        }
        self.on(worker, OP_REMOVE_PIPE, 1);
        if !self.on_worker_exit().is_null() {
            self.on(worker, OP_EXIT_CALLBACK, 0);
        }
    }

    /// `ProcessPool::detach`.
    fn detach(&mut self) -> bool {
        if !self.running() || self.message_box().is_null() {
            return false;
        }
        let worker = pointer(self.op(OP_CURRENT_WORKER, 0));
        self.on(worker, OP_WORKER_SHUTDOWN, 0);
        if self.is_async() {
            self.on(worker, OP_REMOVE_PIPE, 0);
        }
        if self.on(worker, OP_PUSH_STOP, 0) < 0 {
            return false;
        }
        self.set_running(false);
        true
    }

    /// `ProcessPool::kill_all_workers`.
    fn kill_all_workers(&mut self, signo: c_int) {
        for index in 0..self.worker_num() {
            // SAFETY: one of the pool's records.
            kill(unsafe { swr_v1_worker_pid(self.worker(index)) }, signo);
        }
    }

    /// `ProcessPool::reopen_logger`.
    fn reopen_logger(&mut self) {
        self.op(OP_REOPEN_LOGGER, 0);
        if self.op(OP_IS_MASTER, 0) != 0 {
            self.kill_all_workers(libc::SIGWINCH);
        }
    }

    /// `ProcessPool::shutdown`.
    fn shutdown(&mut self) -> bool {
        if self.op(OP_IS_MASTER, 0) != 0 {
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
        if !self.on_before_reload().is_null() {
            self.h.callback(CB_BEFORE_RELOAD, ptr::null_mut())?;
        }
        self.set_reload_task(Box::into_raw(Box::<ReloadTask>::default()));
        if self.max_wait_time() != 0 {
            self.op(OP_RELOAD_TIMER, 0);
        }
        self.set_reload_count(self.reload_count() + 1);
        self.set_reload_init(true);
        self.set_reload_last_time(time_now());
        Ok(true)
    }

    /// `ProcessPool::drop_reload_task`: free the reload and delete its timer.
    fn drop_reload_task(&mut self) {
        // SAFETY: the pool's reload (or null), freed once.
        let timer = unsafe { swr_v1_process_reload_task_free(self.reload_task()) };
        if !timer.is_null() {
            self.op(OP_TIMER_DEL, timer as u64);
        }
        self.set_reload_task(ptr::null_mut());
    }

    /// `ProcessPool::at_worker_enter`.
    fn at_worker_enter(&mut self, worker: *mut WorkerRecord) {
        self.on(worker, OP_DONT_RESTART, 0);
        if self.ipc_mode() == IPC_UNIXSOCK {
            self.op(OP_TIMER_INIT, 0);
        }
    }

    /// `ProcessPool::run_with_*_protocol` and `run_async`: the worker's loop.
    fn main(&mut self, worker: *mut WorkerRecord, protocol: c_int) -> i64 {
        if u64::from(protocol.cast_unsigned()) == MAIN_ASYNC {
            return self.run_async(worker);
        }
        if protocol == PROTOCOL_MESSAGE {
            if self.ipc_mode() != IPC_UNIXSOCK {
                self.op(LOG_NEEDS_UNIXSOCK, 1);
                return SW_ERR;
            }
            if self.message_bus().is_null() {
                self.create_message_bus();
            }
        }
        let ipc_mode = if self.use_msgqueue() != 0 {
            IPC_MSGQUEUE
        } else if self.use_socket() != 0 {
            IPC_SOCKET
        } else {
            IPC_UNIXSOCK
        };
        let pool_loop = PoolLoop {
            hooks: self.h.hooks(),
            ipc: self.ipc(),
            pool: self.p.cast(),
            worker: worker.cast(),
            // SAFETY: one of the pool's records.
            worker_id: unsafe { swr_v1_worker_id(worker) },
            ipc_mode,
            schedule_by_sysvmsg: self.schedule_by_sysvmsg(),
            max_packet_size: self.max_packet_size(),
            event_size: self.h.event_size(),
        };
        self.at_worker_enter(worker);
        if self.h.run_loop(&pool_loop, protocol) == crate::pool::BAILED {
            return RAISE;
        }
        self.op(OP_TIMER_FREE, 0);
        SW_OK
    }

    /// `ProcessPool::run_async`.
    fn run_async(&mut self, worker: *mut WorkerRecord) -> i64 {
        if self.ipc_mode() == IPC_UNIXSOCK && !self.on_message().is_null() {
            self.on(worker, OP_ADD_PIPE, 0);
            if self.message_bus().is_null() {
                // SAFETY: the pool's IPC state.
                unsafe { swr_v1_process_pool_ipc_prepare(self.ipc(), self.max_packet_size()) };
                self.op(OP_PIPE_HANDLER, 0);
            } else {
                self.op(OP_PIPE_HANDLER, 1);
            }
        }
        self.op(OP_EVENT_WAIT, 0)
    }

    /// `ProcessPool::wait`'s per-iteration service: signals, timers, then the
    /// workers' messages (a stopped worker is replaced and its pid detached).
    fn service(&mut self) {
        self.op(OP_SIGNAL_DISPATCH, 0);
        self.op(OP_TIMER_SELECT, 0);
        if !self.read_message() {
            return;
        }
        loop {
            // The popped EventData, in native's buffer.
            let message = pointer::<u8>(self.op(OP_POP_MESSAGE, 0)).cast_const();
            if message.is_null() {
                break;
            }
            if !self.running() {
                continue;
            }
            // SAFETY: the type byte of the EventData just popped.
            let kind = unsafe { message.wrapping_add(EVENT_TYPE).read() };
            if kind != WORKER_MESSAGE_STOP && !self.on_worker_message().is_null() {
                self.op(OP_ON_WORKER_MESSAGE, 0);
                continue;
            }
            // SAFETY: the WorkerStopMessage's pid at the start of its data.
            let pid = unsafe {
                message
                    .wrapping_add(EVENT_DATA)
                    .cast::<c_int>()
                    .read_unaligned()
            };
            let worker = self.worker_by_pid(pid);
            if worker.is_null() {
                continue;
            }
            // A bailout in the new worker's callbacks leaves at once; these frames own nothing.
            if self.spawn(worker).is_err() {
                crate::pool::swr_v1_process_bailout();
            }
            // SAFETY: the pool's registry.
            unsafe { swr_v1_process_worker_registry_detach(self.registry(), pid) };
        }
        self.set_read_message(false);
    }

    /// `ProcessPool::wait`'s reap: restart the exited worker. False when `pid`
    /// was no worker of the pool's.
    fn reap(&mut self, pid: c_int, status: c_int) -> bool {
        let worker = self.worker_by_pid(pid);
        if worker.is_null() {
            if !self.on_worker_not_found().is_null() {
                self.op(OP_ON_WORKER_NOT_FOUND, exited(pid, status));
            // SAFETY: the pool's registry.
            } else if !unsafe { swr_v1_process_worker_registry_reap_detached(self.registry(), pid) }
            {
                self.op(LOG_UNKNOWN_CHILD, u64::from(pid.cast_unsigned()));
            }
            return false;
        }
        if !libc::WIFEXITED(status) {
            self.on(worker, OP_WORKER_ERROR, exited(pid, status));
        }
        // A bailout in the new worker's callbacks leaves at once; these frames own nothing.
        if self.spawn(worker).is_err() {
            crate::pool::swr_v1_process_bailout();
        }
        true
    }

    /// `ProcessPool::wait`: reap and restart workers until the pool stops, then
    /// stop them all.
    fn wait(&mut self) -> Result<i64, Bailed> {
        let host = ReapHost {
            ctx: ptr::from_mut(self).cast(),
            leave_when_stopped: true,
            skip_no_child: false,
            running: reap_running::<H>,
            reload_requested: reap_reload_requested::<H>,
            reload_task: reap_reload_task::<H>,
            wait: swr_v1_process_reap_wait,
            service: reap_service::<H>,
            wait_failed: reap_wait_failed::<H>,
            start_reload: reap_start_reload::<H>,
            reap: reap_exited::<H>,
            finish_reload: reap_finish_reload::<H>,
            warn: self.h.warn(),
        };
        host.run();

        self.set_running(false);
        self.drop_reload_task();
        if !self.on_shutdown().is_null() {
            self.h.callback(CB_SHUTDOWN, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            // SAFETY: one of the pool's records.
            let pid = unsafe { swr_v1_worker_pid(self.worker(index)) };
            if kill(pid, libc::SIGTERM) < 0 {
                self.op(LOG_KILL_FAILED, u64::from(pid.cast_unsigned()));
            }
        }
        if self.max_wait_time() != 0 {
            self.op(OP_KILL_TIMER, 0);
        }
        // Upstream waits for the first worker only.
        if self.worker_num() > 0 {
            // SAFETY: the pool's first record.
            let pid = unsafe { swr_v1_worker_pid(self.worker(0)) };
            if self.op(OP_WAITPID, u64::from(pid.cast_unsigned())) < 0 {
                self.op(LOG_WAITPID_FAILED, u64::from(pid.cast_unsigned()));
            }
        }
        self.set_started(false);
        Ok(SW_OK)
    }

    /// `ProcessPool::push_message(msg)`: into the message box, then wake the master.
    fn push_message(&mut self, message: *const u8) -> i64 {
        if self.op(OP_BOX_PUSH, message as u64) < 0 {
            return SW_ERR;
        }
        kill(self.master_pid(), libc::SIGIO).into()
    }

    /// `ProcessPool::push_message(type, data, length)`.
    fn push(&mut self, kind: u8, data: *const u8, length: usize) -> i64 {
        if self.message_box().is_null() {
            return SW_ERR;
        }
        let size = self.h.event_size();
        assert!(length < size - EVENT_DATA, "the message fits in EventData");
        let mut message = vec![0u64; size.div_ceil(8)];
        let bytes = message.as_mut_ptr().cast::<u8>();
        // SAFETY: the DataHead's type and length fields, inside the buffer.
        unsafe { bytes.wrapping_add(EVENT_TYPE).write(kind) };
        let len = u32::try_from(length).unwrap_or(u32::MAX);
        let len = len.to_ne_bytes();
        // SAFETY: as above.
        unsafe { ptr::copy_nonoverlapping(len.as_ptr(), bytes.wrapping_add(EVENT_LEN), len.len()) };
        // SAFETY: the caller's `length` bytes fit in the data field (asserted).
        unsafe { ptr::copy_nonoverlapping(data, bytes.wrapping_add(EVENT_DATA), length) };
        self.push_message(bytes)
    }

    /// `ProcessPool::dispatch`/`dispatch_sync(EventData *)`: to the stream
    /// listener, or a worker's pipe or queue.
    fn dispatch(&mut self, event: *mut u8, worker_id: *mut c_int, blocking: bool) -> i64 {
        if self.use_socket() != 0 {
            return self.op(
                if blocking {
                    OP_SYNC_SEND
                } else {
                    OP_STREAM_SEND
                },
                event as u64,
            );
        }
        // SAFETY: the caller's EventData.
        let size =
            unsafe { event.wrapping_add(EVENT_LEN).cast::<u32>().read_unaligned() } + HEAD_SIZE;
        let p = self.p;
        // SAFETY: the pool's shared counter, in place.
        let round_id = unsafe { &raw mut (*p).round_id };
        // SAFETY: the pool's flag, in place.
        let warning = unsafe { &raw mut (*p).scheduler_warning };
        // SAFETY: the pool's records, counter and flag; the caller's event and out-pointer.
        let sent = unsafe {
            crate::pool::swr_v1_process_pool_dispatch(
                self.h.hooks(),
                p.cast(),
                self.workers(),
                self.worker_num(),
                self.start_id(),
                round_id,
                warning,
                self.schedule_by_sysvmsg(),
                self.use_msgqueue() != 0,
                worker_id,
                event,
                size,
                blocking,
            )
        };
        if sent { SW_OK } else { SW_ERR }
    }

    /// `ProcessPool::destroy`.
    fn destroy(&mut self) {
        if !self.pipes().is_null() {
            self.op(OP_FREE_PIPES, 0);
            self.set_pipes(ptr::null_mut());
        }
        if !self.queue().is_null() {
            self.op(OP_FREE_QUEUE, 0);
            self.set_queue(ptr::null_mut());
        }
        // SAFETY: the pool's IPC state, freed once; the native close frees its listener.
        unsafe { swr_v1_process_pool_ipc_free(self.ipc(), self.h.close()) };
        self.set_ipc(ptr::null_mut());
        // SAFETY: the pool's registry (or null), freed once.
        unsafe { swr_v1_process_worker_registry_free(self.registry()) };
        self.set_registry(ptr::null_mut());
        if !self.message_box().is_null() {
            self.op(OP_FREE_MESSAGE_BOX, 0);
            self.set_message_box(ptr::null_mut());
        }
        if !self.message_bus().is_null() {
            self.op(OP_FREE_MESSAGE_BUS, 0);
            self.set_message_bus(ptr::null_mut());
        }
        self.op(OP_FREE_WORKERS, 0);
    }
}

fn pool_of<'a, H>(ctx: *mut c_void) -> &'a mut Pool<H> {
    // SAFETY: the reap host's context is the `Pool` running `wait`, on its stack.
    unsafe { &mut *ctx.cast::<Pool<H>>() }
}

unsafe extern "C" fn reap_running<H: PoolHost>(ctx: *mut c_void) -> bool {
    pool_of::<H>(ctx).running()
}

unsafe extern "C" fn reap_reload_requested<H: PoolHost>(ctx: *mut c_void) -> bool {
    pool_of::<H>(ctx).reload_init()
}

unsafe extern "C" fn reap_reload_task<H: PoolHost>(ctx: *mut c_void) -> *mut ReloadTask {
    pool_of::<H>(ctx).reload_task()
}

/// The reap loop's wait: `wait()`, or `waitpid(-1, WNOHANG)` when `nonblock`,
/// with the status and the wait's `errno` written out.
///
/// # Safety
/// `status` and `error` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reap_wait(
    _: *mut c_void,
    nonblock: bool,
    status: *mut c_int,
    error: *mut c_int,
) -> c_int {
    let mut exit_status = 0;
    let pid = if nonblock {
        // SAFETY: waitpid(2) into a local.
        unsafe { libc::waitpid(-1, &raw mut exit_status, libc::WNOHANG) }
    } else {
        // SAFETY: wait(2) into a local.
        unsafe { libc::wait(&raw mut exit_status) }
    };
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: the reap loop's out-pointers.
    unsafe { *error = errno };
    // SAFETY: as above.
    unsafe { *status = exit_status };
    pid
}

unsafe extern "C" fn reap_service<H: PoolHost>(ctx: *mut c_void) {
    pool_of::<H>(ctx).service();
}

unsafe extern "C" fn reap_wait_failed<H: PoolHost>(ctx: *mut c_void) {
    pool_of::<H>(ctx).op(LOG_WAIT_FAILED, 0);
}

unsafe extern "C" fn reap_start_reload<H: PoolHost>(ctx: *mut c_void) -> u32 {
    let pool = pool_of::<H>(ctx);
    if !pool.reload_init() {
        return RELOAD_NONE;
    }
    pool.set_reload_init(false);
    // SAFETY: the reload `reload` started, and the pool's records.
    unsafe {
        crate::reload_task::swr_v1_process_reload_task_add_workers(
            pool.reload_task(),
            pool.workers(),
            pool.worker_num() as usize,
        );
    }
    RELOAD_KILL_ONE
}

unsafe extern "C" fn reap_exited<H: PoolHost>(ctx: *mut c_void, pid: c_int, status: c_int) -> bool {
    pool_of::<H>(ctx).reap(pid, status)
}

unsafe extern "C" fn reap_finish_reload<H: PoolHost>(ctx: *mut c_void) {
    let pool = pool_of::<H>(ctx);
    pool.drop_reload_task();
    if !pool.on_after_reload().is_null() {
        pool.op(OP_ON_AFTER_RELOAD, 0);
    }
}

/// Run `f` over the pool with the native side.
///
/// # Safety
/// `native` is native's table and `pool` a live `ProcessPool`.
unsafe fn with<T>(
    native: *const PoolNative,
    pool: *mut PoolRecord,
    f: impl FnOnce(&mut Pool<Native<'_>>) -> T,
) -> T {
    // SAFETY: the caller's table, live for the call.
    let native = unsafe { &*native };
    let mut pool = Pool {
        h: Native {
            native,
            pool: pool.cast(),
        },
        p: pool,
    };
    f(&mut pool)
}

fn raise_or(result: Result<i64, Bailed>) -> i64 {
    result.unwrap_or(RAISE)
}

/// `ProcessPool::create(worker_num, key, mode)`.
///
/// # Safety
/// `native` is native's table; `pool` a zeroed or reset `ProcessPool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_pool_create(
    native: *const PoolNative,
    pool: *mut PoolRecord,
    worker_num: u32,
    key: c_int,
    mode: u8,
) -> i64 {
    // SAFETY: as the caller guarantees.
    unsafe { with(native, pool, |p| p.create(worker_num, key, mode)) }
}

/// `ProcessPool::push_message(type, data, length)`.
///
/// # Safety
/// `native` is native's table; `pool` a live `ProcessPool`; `data` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_pool_push(
    native: *const PoolNative,
    pool: *mut PoolRecord,
    kind: u8,
    data: *const u8,
    length: usize,
) -> i64 {
    // SAFETY: as the caller guarantees.
    unsafe { with(native, pool, |p| p.push(kind, data, length)) }
}

/// `ProcessPool::dispatch` (`blocking`: `dispatch_sync`) of the `EventData` at `event`.
///
/// # Safety
/// `native` is native's table; `pool` a live `ProcessPool`; `event` an `EventData`
/// and `worker_id` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_pool_dispatch_event(
    native: *const PoolNative,
    pool: *mut PoolRecord,
    event: *mut u8,
    worker_id: *mut c_int,
    blocking: bool,
) -> i64 {
    // SAFETY: as the caller guarantees.
    unsafe { with(native, pool, |p| p.dispatch(event, worker_id, blocking)) }
}

/// `swoole_get_worker_symbol`: the log's mark for a worker type (`swWorkerType`).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_worker_symbol(kind: c_int) -> std::ffi::c_char {
    let symbol = match kind {
        1 => b'#',
        3 => b'$',
        2 => b'*',
        4 => b'^',
        5 => b'@',
        _ => b'%',
    };
    symbol.cast_signed()
}

/// One `ProcessPool` operation (`RUN_*`) on `pool` (and `worker`, for those over
/// one): the C++ method's return value, or [`RAISE`].
///
/// # Safety
/// `native` is native's table; `pool` a live `ProcessPool`; `worker` null or
/// one of its workers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_pool_run_op(
    native: *const PoolNative,
    pool: *mut PoolRecord,
    worker: *mut WorkerRecord,
    operation: u32,
    arg: u64,
) -> i64 {
    // SAFETY: as the caller guarantees.
    unsafe { with(native, pool, |p| p.run(worker, operation, arg)) }
}

impl<H: PoolHost> Pool<H> {
    fn run(&mut self, worker: *mut WorkerRecord, operation: u32, arg: u64) -> i64 {
        let signed = arg.cast_signed();
        match operation {
            RUN_CREATE_MESSAGE_BOX => self.create_message_box(arg),
            RUN_CREATE_MESSAGE_BUS => self.create_message_bus(),
            RUN_SET_PROTOCOL => {
                self.set_protocol(c_int::try_from(signed).unwrap_or(0));
                SW_OK
            }
            RUN_START_CHECK => self.start_check(),
            RUN_START => raise_or(self.start()),
            RUN_SPAWN => raise_or(self.spawn(worker)),
            RUN_STOP => {
                self.stop(worker);
                SW_OK
            }
            RUN_DETACH => self.detach().into(),
            RUN_KILL_ALL => {
                self.kill_all_workers(c_int::try_from(signed).unwrap_or(0));
                SW_OK
            }
            RUN_REOPEN_LOGGER => {
                self.reopen_logger();
                SW_OK
            }
            RUN_SHUTDOWN => self.shutdown().into(),
            RUN_RELOAD => raise_or(self.reload().map(i64::from)),
            RUN_DROP_RELOAD_TASK => {
                self.drop_reload_task();
                SW_OK
            }
            RUN_MAIN => self.main(worker, c_int::try_from(signed).unwrap_or(0)),
            RUN_WAIT => raise_or(self.wait()),
            RUN_DESTROY => {
                self.destroy();
                SW_OK
            }
            RUN_SET_TYPE => {
                let kind = u8::try_from(arg).unwrap_or(0);
                self.set_kind(kind);
                for index in 0..self.worker_num() {
                    // SAFETY: one of the pool's records.
                    unsafe { swr_v1_worker_set_type(self.worker(index), kind) };
                }
                SW_OK
            }
            RUN_SET_START_ID => {
                let start_id = u16::try_from(arg).unwrap_or(0);
                self.set_start_id(start_id);
                for index in 0..self.worker_num() {
                    // SAFETY: one of the pool's records.
                    unsafe {
                        swr_v1_worker_set_id(self.worker(index), u32::from(start_id) + index);
                    };
                }
                SW_OK
            }
            RUN_PUSH_MESSAGE => self.push_message(pointer::<u8>(signed).cast_const()),
            RUN_SET_MAX_REQUEST => {
                self.set_max_request(u32::try_from(arg & 0xffff_ffff).unwrap_or(0));
                self.set_max_request_grace(u32::try_from(arg >> 32).unwrap_or(0));
                SW_OK
            }
            _ => SW_ERR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

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
        assert_eq!(size_of::<PoolRecord>(), 264);
    }

    #[test]
    fn the_header_numbers_every_op_alike() {
        let header = include_str!("../../../native/process/swoole_rs_process.h");
        let source = include_str!("process_pool.rs");
        let numbered = source
            .split("// BEGIN NUMBERED")
            .nth(1)
            .and_then(|rest| rest.split("// END NUMBERED").next())
            .unwrap();
        let mut count = 0;
        for line in numbered
            .lines()
            .filter(|line| line.starts_with("pub const "))
        {
            let rest = &line["pub const ".len()..];
            let name = rest.split(':').next().unwrap();
            let value = rest.rsplit("= ").next().unwrap().trim_end_matches(';');
            let define = format!("#define SWR_POOL_{name} {value}\n");
            assert!(header.contains(&define), "{define}");
            count += 1;
        }
        let defined = ["OP", "LOG", "CB", "RUN"]
            .iter()
            .map(|kind| header.matches(&format!("#define SWR_POOL_{kind}_")).count())
            .sum::<usize>();
        assert_eq!(defined, count);
    }

    /// A scripted native side over a pool record allocated here.
    #[derive(Default)]
    struct Script {
        answers: std::collections::HashMap<u32, i64>,
        log: Vec<(u32, u64)>,
        callbacks: Vec<c_int>,
        bail: Option<c_int>,
    }

    impl PoolHost for Script {
        fn op(&mut self, _: *mut c_void, op: u32, arg: u64) -> i64 {
            self.log.push((op, arg));
            self.answers.get(&op).copied().unwrap_or(1)
        }
        fn callback(&mut self, cb: c_int, _: *mut c_void) -> Result<isize, Bailed> {
            self.callbacks.push(cb);
            if self.bail == Some(cb) {
                Err(Bailed)
            } else {
                Ok(0)
            }
        }
        fn run_loop(&mut self, _: &PoolLoop, _: c_int) -> c_int {
            crate::pool::DONE
        }
        fn hooks(&self) -> *const PoolHooks {
            ptr::null()
        }
        fn warn(&self) -> Warn {
            warn
        }
        fn event_size(&self) -> usize {
            8192
        }
        fn close(&self) -> unsafe extern "C" fn(*mut c_void) {
            close
        }
    }

    unsafe extern "C" fn close(_: *mut c_void) {}

    extern "C" fn warn(_: c_int, _: c_int, _: u32) {}

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

        fn pool(&mut self, script: Script) -> Pool<Script> {
            self.record.workers = self.workers.as_mut_ptr();
            self.record.worker_num = u32::try_from(self.workers.len()).unwrap();
            Pool {
                h: script,
                p: &raw mut *self.record,
            }
        }
    }

    fn record(pool: &mut Pool<Script>) -> &mut PoolRecord {
        // SAFETY: the fixture's record.
        unsafe { &mut *pool.p }
    }

    fn ops(pool: &Pool<Script>) -> Vec<u32> {
        pool.h.log.iter().map(|(op, _)| *op).collect()
    }

    #[test]
    fn create_refuses_a_queue_without_sysvmsg() {
        let mut fixture = Fixture::new(0);
        let mut script = Script::default();
        script.answers.insert(OP_HAVE_MSGQUEUE, 0);
        let mut pool = fixture.pool(script);
        assert_eq!(pool.create(2, 0, IPC_MSGQUEUE), SW_ERR);
        assert_eq!(ops(&pool), [OP_HAVE_MSGQUEUE, LOG_NO_SYSVMSG]);
    }

    #[test]
    fn create_makes_a_pipe_per_worker_and_sets_the_defaults() {
        let mut fixture = Fixture::new(3);
        let mut pool = fixture.pool(Script::default());
        assert_eq!(pool.create(3, 0, IPC_UNIXSOCK), SW_OK);
        assert_eq!(
            ops(&pool),
            [
                OP_ALLOC_WORKERS,
                OP_NEW_MESSAGE_BOX,
                OP_NEW_PIPES,
                OP_NEW_PIPE,
                OP_NEW_PIPE,
                OP_NEW_PIPE,
                OP_MAIN_LOOP
            ]
        );
        assert_eq!(pool.ipc_mode(), IPC_UNIXSOCK);
        assert_eq!(record(&mut pool).protocol_type, PROTOCOL_TASK);
        assert_eq!(pool.max_packet_size(), SW_INPUT_BUFFER_SIZE);
        assert_eq!(pool.max_wait_time(), SW_WORKER_MAX_WAIT_TIME);
        assert!(!pool.registry().is_null());
        pool.destroy();
        assert!(pool.registry().is_null() && pool.ipc().is_null());
    }

    #[test]
    fn create_stops_at_the_first_pipe_that_fails() {
        let mut fixture = Fixture::new(3);
        let mut script = Script::default();
        script.answers.insert(OP_NEW_PIPE, 0);
        let mut pool = fixture.pool(script);
        assert_eq!(pool.create(3, 0, IPC_UNIXSOCK), SW_ERR);
        assert_eq!(
            pool.h
                .log
                .iter()
                .filter(|(op, _)| *op == OP_NEW_PIPE)
                .count(),
            1
        );
    }

    #[test]
    fn an_unknown_ipc_mode_is_none() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        assert_eq!(pool.create(1, 0, 9), SW_OK);
        assert_eq!(pool.ipc_mode(), IPC_NONE);
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
            assert_eq!(swr_v1_worker_id(pool.worker(1)), 6);
            assert_eq!(crate::worker_record::swr_v1_worker_type(pool.worker(1)), 4);
        }
    }

    #[test]
    fn a_socket_pool_must_listen_first() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IPC_SOCKET);
        pool.set_ipc(swr_v1_process_pool_ipc_new(true));
        assert_eq!(pool.start_check(), SW_ERR);
        assert_eq!(ops(&pool), [LOG_MUST_LISTEN]);
        pool.destroy();
    }

    #[test]
    fn a_bailout_in_on_start_spawns_nothing() {
        let mut fixture = Fixture::new(2);
        let script = Script {
            bail: Some(CB_START),
            ..Script::default()
        };
        let mut pool = fixture.pool(script);
        record(&mut pool).on_start = ptr::dangling();
        assert!(pool.start().is_err());
        assert!(!ops(&pool).contains(&OP_FORK));
    }

    #[test]
    fn spawn_registers_the_new_pid() {
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(OP_FORK, 4321);
        let mut pool = fixture.pool(script);
        pool.set_registry(swr_v1_process_worker_registry_new());
        let worker = pool.worker(0);
        assert_eq!(pool.spawn(worker).ok(), Some(4321));
        assert_eq!(pool.worker_by_pid(4321), worker);
        // A respawn forgets the old pid.
        pool.h.answers.insert(OP_FORK, 4322);
        pool.spawn(worker).ok();
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
        assert_eq!(ops(&pool), [OP_RELOAD_TIMER]);
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
                OP_CURRENT_WORKER,
                OP_WORKER_SHUTDOWN,
                OP_REMOVE_PIPE,
                OP_PUSH_STOP
            ]
        );
    }

    #[test]
    fn stop_outside_an_event_loop_only_shuts_down() {
        let mut fixture = Fixture::new(1);
        let mut script = Script::default();
        script.answers.insert(OP_EVENT_AVAILABLE, 0);
        let mut pool = fixture.pool(script);
        record(&mut pool).on_worker_exit = ptr::dangling();
        pool.stop(pool.worker(0));
        assert_eq!(ops(&pool), [OP_WORKER_SHUTDOWN, OP_EVENT_AVAILABLE]);
    }

    #[test]
    fn the_message_protocol_needs_unix_sockets() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IPC_MSGQUEUE);
        assert_eq!(pool.main(pool.worker(0), PROTOCOL_MESSAGE), SW_ERR);
        assert_eq!(pool.h.log, [(LOG_NEEDS_UNIXSOCK, 1)]);
    }

    #[test]
    fn each_worker_type_has_its_log_symbol() {
        let symbols: String = (0..=6)
            .map(|kind| char::from(swr_v1_worker_symbol(kind).cast_unsigned()))
            .collect();
        assert_eq!(symbols, "%#*$^@%");
    }

    #[test]
    fn push_message_needs_a_message_box_then_wakes_the_master() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        assert_eq!(pool.push(1, b"ab".as_ptr(), 2), SW_ERR);
        record(&mut pool).message_box = ptr::dangling_mut();
        pool.h.answers.insert(OP_BOX_PUSH, -1);
        assert_eq!(pool.push(1, b"ab".as_ptr(), 2), SW_ERR);
        assert_eq!(ops(&pool), [OP_BOX_PUSH]);
    }

    #[test]
    fn a_socket_pool_dispatches_to_its_stream() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        record(&mut pool).use_socket = 1;
        let mut event = [0u8; 64];
        let mut worker_id = -1;
        pool.dispatch(event.as_mut_ptr(), &raw mut worker_id, false);
        pool.dispatch(event.as_mut_ptr(), &raw mut worker_id, true);
        assert_eq!(ops(&pool), [OP_STREAM_SEND, OP_SYNC_SEND]);
    }

    #[test]
    fn an_unknown_child_is_reported() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        pool.set_registry(swr_v1_process_worker_registry_new());
        assert!(!pool.reap(99, 0));
        assert_eq!(pool.h.log, [(LOG_UNKNOWN_CHILD, 99)]);
        pool.destroy();
    }
}
