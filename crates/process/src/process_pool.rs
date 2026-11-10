//! `ProcessPool`'s lifecycle (once `src/os/process_pool.cc`): create, start, spawn,
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

use crate::worker_record::Worker;
use std::ffi::{c_int, c_void};
use std::ptr;

use crate::os::{getpid, kill};
use crate::pool::{
    Bailed, PoolCall, PoolHooks, PoolIpc, PoolLoop, Step, pool_ipc_free, pool_ipc_new,
    pool_ipc_prepare, pool_run, swr_v1_process_pool_ipc_listener,
};
use crate::reaper::{self, RELOAD_KILL_ONE, RELOAD_NONE, Reap, Waited, wait_child};
use crate::reload_task::{ReloadTask, Warn};
use crate::worker_record::WorkerRecord;
use crate::worker_registry::WorkerRegistry;

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

/// The table every export runs over (`crates/server/src/pool_native.rs`).
#[repr(C)]
pub struct PoolNative {
    /// One effect (`OP_*`) on `pool` and `worker` (null when none).
    pub call:
        unsafe extern "C" fn(pool: *mut c_void, worker: *mut c_void, op: u32, arg: u64) -> i64,
    /// The callback `CB_*` in the call's `flags`, run inside the trap.
    pub callback: Step,
    /// The worker loops' hooks.
    pub hooks: *const PoolHooks,
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
pub const PROTOCOL_STREAM: c_int = 2;
pub const PROTOCOL_MESSAGE: c_int = 3;
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

// The effects (crates/server/src/pool_native.rs), the callbacks and the worker loop.
pub const OP_HAVE_MSGQUEUE: u32 = 0;
pub const OP_ALLOC_WORKERS: u32 = 1;
pub const OP_NEW_MESSAGE_BOX: u32 = 2;
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
pub const OP_FREE_MESSAGE_BOX: u32 = 40;
pub const OP_FREE_MESSAGE_BUS: u32 = 41;
pub const OP_FREE_WORKERS: u32 = 42;
pub const OP_WORKER_INIT: u32 = 43;
pub const OP_EVENT_INIT: u32 = 44;
pub const OP_BOX_PUSH: u32 = 45;
pub const CB_START: c_int = 1;
pub const CB_BEFORE_RELOAD: c_int = 2;
pub const CB_SHUTDOWN: c_int = 3;
pub const CB_WORKER_START: c_int = 4;
pub const CB_MAIN_LOOP: c_int = 5;
pub const CB_WORKER_STOP: c_int = 6;
pub const RUN_MAIN: u32 = 13;
// The warnings a pool operation logs, which [`Native`] emits itself.
pub const LOG_NO_SYSVMSG: u32 = 48;
pub const LOG_ALLOC: u32 = 49;
pub const LOG_NEEDS_UNIXSOCK: u32 = 50;
pub const LOG_BUS_EXISTS: u32 = 51;
pub const LOG_MUST_LISTEN: u32 = 52;
pub const LOG_UNKNOWN_CHILD: u32 = 53;
pub const LOG_WAIT_FAILED: u32 = 54;
pub const LOG_KILL_FAILED: u32 = 55;
pub const LOG_WAITPID_FAILED: u32 = 56;
// The rest of `ProcessPool`'s operations, which only crates/process/src/methods.rs runs.
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
pub const RUN_WAIT: u32 = 14;
pub const RUN_DESTROY: u32 = 15;
pub const RUN_SET_TYPE: u32 = 16;
pub const RUN_SET_START_ID: u32 = 17;
pub const RUN_SET_MAX_REQUEST: u32 = 18;
pub const RUN_PUSH_MESSAGE: u32 = 19;
pub const RUN_SIGNAL: u32 = 20;

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
        if (LOG_NO_SYSVMSG..=LOG_WAITPID_FAILED).contains(&op) {
            log(op, arg);
            return 0;
        }
        #[cfg(feature = "upstream-symbols")]
        if let Some(result) = effect(op, arg) {
            return result;
        }
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
        unsafe { pool_run(pool_loop, protocol) }
    }

    fn hooks(&self) -> *const PoolHooks {
        self.native.hooks
    }

    fn warn(&self) -> Warn {
        crate::methods::reload_warn
    }

    fn event_size(&self) -> usize {
        self.native.event_size
    }

    fn close(&self) -> unsafe extern "C" fn(*mut c_void) {
        // SAFETY: native's static hooks.
        unsafe { (*self.native.hooks).close }
    }
}

/// The `OP_*` effects that are calls into the process-wide worker, signal, timer,
/// event and wait state: `None` for the rest, which native code does.
#[cfg(feature = "upstream-symbols")]
fn effect(op: u32, arg: u64) -> Option<i64> {
    use crate::methods::{swoole_get_worker_type, swoole_set_worker_type};
    // SAFETY: crates/timer's and crates/reactor's functions, and
    // crates/reactor's swoole_waitpid, under their upstream symbols.
    unsafe extern "C" {
        #[link_name = "_Z25swoole_timer_is_availablev"]
        fn swoole_timer_is_available() -> bool;
        #[link_name = "_Z17swoole_timer_freev"]
        fn swoole_timer_free();
        #[link_name = "_Z19swoole_timer_selectv"]
        fn swoole_timer_select();
        #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
        fn swoole_timer_del(node: *mut c_void) -> bool;
        #[link_name = "_Z17swoole_event_waitv"]
        fn swoole_event_wait() -> c_int;
        #[link_name = "_Z25swoole_event_is_availablev"]
        fn swoole_event_is_available() -> bool;
        #[link_name = "_Z11swoole_forki"]
        fn swoole_fork(flags: c_int) -> c_int;
        #[link_name = "_Z14swoole_waitpidiPii"]
        fn swoole_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the int the operation widened"
    )]
    let int = arg as c_int;
    Some(match op {
        OP_WORKER_TYPE => {
            swoole_set_worker_type(int);
            0
        }
        OP_IS_MASTER => i64::from(i64::from(swoole_get_worker_type()) == SW_MASTER.cast_signed()),
        OP_CURRENT_WORKER => {
            // SAFETY: this thread's worker globals.
            let worker = unsafe { (*crate::worker_globals::thread_globals()).worker };
            worker.expose_provenance().cast_signed() as i64
        }
        OP_SIGNAL_DISPATCH => {
            crate::signal::swoole_signal_dispatch();
            0
        }
        OP_TIMER_SELECT => {
            // SAFETY: the thread's timer, when it has one.
            unsafe { swoole_timer_select() };
            0
        }
        OP_TIMER_FREE => {
            // SAFETY: as above.
            if unsafe { swoole_timer_is_available() } {
                // SAFETY: the thread's timer, freed once.
                unsafe { swoole_timer_free() };
            }
            0
        }
        OP_TIMER_DEL => {
            // SAFETY: a node of the thread's timer, deleted once.
            unsafe {
                swoole_timer_del(ptr::with_exposed_provenance_mut(
                    usize::try_from(arg).unwrap_or(0),
                ))
            };
            0
        }
        // SAFETY: the thread's event loop.
        OP_EVENT_WAIT => i64::from(unsafe { swoole_event_wait() }),
        // SAFETY: as above.
        OP_EVENT_AVAILABLE => i64::from(unsafe { swoole_event_is_available() }),
        // SAFETY: src/core/base.cc's fork, with its process bookkeeping.
        OP_FORK => i64::from(unsafe { swoole_fork(0) }),
        OP_WAITPID => {
            let mut status = 0;
            // SAFETY: `status` is writable.
            i64::from(unsafe { swoole_waitpid(int, &raw mut status, 0) })
        }
        _ => return None,
    })
}

/// `LOG_*`'s warning, under the `ProcessPool` method upstream logged it from.
fn log(op: u32, arg: u64) {
    use swoole_rs_corebase::log::{error_log, sys_warning, warning};
    /// `SW_LOG_WARNING`, `SW_ERROR_OPERATION_NOT_SUPPORT`, `SW_ERROR_WRONG_OPERATION`.
    const WARNING: c_int = 4;
    const NOT_SUPPORT: c_int = 507;
    const WRONG_OPERATION: c_int = 509;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the pid the operation widened from an int"
    )]
    let pid = arg as c_int;
    match op {
        LOG_NO_SYSVMSG => warning(
            c"ProcessPool::create",
            format_args!("current platform does not support `sysvmsg`"),
        ),
        LOG_ALLOC => sys_warning(
            if arg == 0 {
                c"ProcessPool::create"
            } else {
                c"ProcessPool::create_message_bus"
            },
            format_args!("malloc[1] failed"),
        ),
        LOG_NEEDS_UNIXSOCK => error_log(
            WARNING,
            NOT_SUPPORT,
            if arg == 0 {
                c"ProcessPool::create_message_bus"
            } else {
                c"ProcessPool::run_with_message_protocol"
            },
            format_args!("not support, ipc_mode must be SW_IPC_UNIXSOCK"),
        ),
        LOG_BUS_EXISTS => error_log(
            WARNING,
            WRONG_OPERATION,
            c"ProcessPool::create_message_bus",
            format_args!("the message bus has been created"),
        ),
        LOG_MUST_LISTEN => error_log(
            WARNING,
            WRONG_OPERATION,
            c"ProcessPool::start_check",
            format_args!("must first listen to an tcp port"),
        ),
        LOG_UNKNOWN_CHILD => warning(
            c"ProcessPool::wait_detached_worker",
            format_args!("received an exit signal from an unknown child process[pid={pid}]"),
        ),
        LOG_WAIT_FAILED => sys_warning(c"ProcessPool::wait", format_args!("wait() failed")),
        LOG_KILL_FAILED => sys_warning(
            c"ProcessPool::wait",
            format_args!("kill({pid}, SIGTERM) failed"),
        ),
        _ => sys_warning(c"ProcessPool::wait", format_args!("waitpid({pid}) failed")),
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

    /// The worker at `index`, which must be below `worker_num`.
    fn worker_at(&self, index: u32) -> Worker<'_> {
        assert!(index < self.worker_num(), "worker {index} out of range");
        // SAFETY: one of the `worker_num` records `create` allocated, live for the pool's lifetime.
        unsafe { Worker::new(self.worker(index)) }
    }

    /// The pool's `worker_num` records.
    fn worker_records(&self) -> &[WorkerRecord] {
        if self.workers().is_null() {
            return &[];
        }
        // SAFETY: `create` allocates `worker_num` records, live for the pool's lifetime.
        unsafe { std::slice::from_raw_parts(self.workers(), self.worker_num() as usize) }
    }

    /// `get_worker_by_pid`: the pool's worker at `pid`, or null.
    fn worker_by_pid(&self, pid: c_int) -> *mut WorkerRecord {
        let registry = self.registry();
        if registry.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: the pool's registry.
        unsafe { (*registry).lookup(pid) }.cast()
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
                let Some(queue) = swoole_rs_ossupport::QueueRecord::open(key) else {
                    return SW_ERR;
                };
                self.set_queue(Box::into_raw(queue).cast());
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
        self.set_registry(Box::into_raw(Box::<WorkerRegistry>::default()));
        self.set_ipc(pool_ipc_new(mode == IPC_SOCKET));
        self.set_ipc_mode(mode);
        self.set_protocol(PROTOCOL_TASK);
        self.set_max_packet_size(packet_limit(self.use_msgqueue() != 0, SW_INPUT_BUFFER_SIZE));
        self.set_max_wait_time(SW_WORKER_MAX_WAIT_TIME);
        for index in 0..worker_num {
            self.worker_at(index).set_pool(self.p.cast());
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
            let worker = unsafe { Worker::new(worker) };
            worker.set_pool(self.p.cast());
            worker.set_id(id);
            worker.set_kind(self.kind());
            self.on(worker.raw(), OP_UNLIMIT_PIPES, 0);
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
        let worker = unsafe { Worker::new(worker) };
        let old = worker.pid();
        // SAFETY: the pool's registry, which `create` made.
        unsafe { (*self.registry()).unregister(old) };
        let pid = self.on(worker.raw(), OP_FORK, 0);
        if pid == 0 {
            self.child(worker.raw())?;
            return Ok(0);
        }
        let pid = c_int::try_from(pid).unwrap_or(-1);
        worker.set_pid(pid);
        let record = worker.raw().cast();
        // SAFETY: as above.
        unsafe { (*self.registry()).register(pid, record, 0) };
        Ok(pid.into())
    }

    /// The forked worker: its callbacks and loop, then `exit`.
    fn child(&mut self, worker: *mut WorkerRecord) -> Result<(), Bailed> {
        self.on(worker, OP_WORKER_INIT, 0);
        if self.is_async() && self.op(OP_EVENT_INIT, 0) < 0 {
            // exit(3), as upstream.
            std::process::exit(254);
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
        // exit(3), as upstream.
        std::process::exit(code)
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
        self.set_reload_last_time(swoole_rs_corebase::time_now());
        Ok(true)
    }

    /// `ProcessPool::drop_reload_task`: free the reload and delete its timer.
    fn drop_reload_task(&mut self) {
        let task = self.reload_task();
        let timer = if task.is_null() {
            ptr::null_mut()
        } else {
            // SAFETY: the pool's reload, boxed by `reload` and freed once.
            unsafe { *Box::from_raw(task) }.free()
        };
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
            worker_id: unsafe { Worker::new(worker).id() },
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
                unsafe { pool_ipc_prepare(self.ipc(), self.max_packet_size()) };
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
                crate::pool::raise_bailout();
            }
            // SAFETY: the pool's registry.
            unsafe { (*self.registry()).detach(pid) };
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
            } else if !unsafe { (*self.registry()).reap_detached(pid) } {
                self.op(LOG_UNKNOWN_CHILD, u64::from(pid.cast_unsigned()));
            }
            return false;
        }
        if !libc::WIFEXITED(status) {
            self.on(worker, OP_WORKER_ERROR, exited(pid, status));
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
        if !self.on_shutdown().is_null() {
            self.h.callback(CB_SHUTDOWN, ptr::null_mut())?;
        }
        for index in 0..self.worker_num() {
            let pid = self.worker_at(index).pid();
            if kill(pid, libc::SIGTERM) < 0 {
                self.op(LOG_KILL_FAILED, u64::from(pid.cast_unsigned()));
            }
        }
        if self.max_wait_time() != 0 {
            self.op(OP_KILL_TIMER, 0);
        }
        // Upstream waits for the first worker only.
        if self.worker_num() > 0 {
            let pid = self.worker_at(0).pid();
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
        // SAFETY: the caller's EventData.
        let size =
            unsafe { event.wrapping_add(EVENT_LEN).cast::<u32>().read_unaligned() } + HEAD_SIZE;
        if self.use_socket() != 0 {
            // SAFETY: the caller's EventData holds `size` bytes.
            let data = unsafe { std::slice::from_raw_parts(event, size as usize) };
            // SAFETY: a socket pool's IPC state.
            let sent =
                unsafe { crate::pool::swr_v1_process_pool_ipc_send(self.ipc(), data, blocking) };
            return if sent { SW_OK } else { SW_ERR };
        }
        let p = self.p;
        let round_id = p
            .wrapping_byte_add(std::mem::offset_of!(PoolRecord, round_id))
            .cast::<u32>();
        let warning = p
            .wrapping_byte_add(std::mem::offset_of!(PoolRecord, scheduler_warning))
            .cast::<u8>();
        // SAFETY: the pool's records, counter and flag; the caller's event and out-pointer.
        let sent = unsafe {
            crate::pool::pool_dispatch(
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
        let queue = self.queue().cast::<swoole_rs_ossupport::QueueRecord>();
        if !queue.is_null() {
            // SAFETY: the pool's boxed queue, freed once.
            drop(unsafe { Box::from_raw(queue) });
        }
        self.set_queue(ptr::null_mut());
        // SAFETY: the pool's IPC state, freed once; the native close frees its listener.
        unsafe { pool_ipc_free(self.ipc(), self.h.close()) };
        self.set_ipc(ptr::null_mut());
        if !self.registry().is_null() {
            // SAFETY: the pool's boxed registry, freed once.
            drop(unsafe { Box::from_raw(self.registry()) });
        }
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

impl<H: PoolHost> Reap for Pool<H> {
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
        self.op(LOG_WAIT_FAILED, 0);
    }

    fn start_reload(&mut self) -> u32 {
        if !self.reload_init() {
            return RELOAD_NONE;
        }
        self.set_reload_init(false);
        // SAFETY: the reload `reload` started, live until `drop_reload_task`.
        unsafe { &mut *Pool::reload_task(self) }.add_workers(self.worker_records());
        RELOAD_KILL_ONE
    }

    fn reap(&mut self, pid: c_int, status: c_int) -> bool {
        Pool::reap(self, pid, status)
    }

    fn finish_reload(&mut self) {
        self.drop_reload_task();
        if !self.on_after_reload().is_null() {
            self.op(OP_ON_AFTER_RELOAD, 0);
        }
    }

    fn warn(&self) -> Warn {
        self.h.warn()
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

/// One `ProcessPool` operation (`RUN_*`) on `pool` (and `worker`, for those over
/// one): the C++ method's return value, or [`RAISE`].
///
/// # Safety
/// `native` is native's table; `pool` a live `ProcessPool`; `worker` null or
/// one of its workers.
pub unsafe fn pool_run_op(
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
                    self.worker_at(index).set_kind(kind);
                }
                SW_OK
            }
            RUN_SET_START_ID => {
                let start_id = u16::try_from(arg).unwrap_or(0);
                self.set_start_id(start_id);
                for index in 0..self.worker_num() {
                    self.worker_at(index).set_id(u32::from(start_id) + index);
                }
                SW_OK
            }
            RUN_SIGNAL => raise_or(
                self.signal(c_int::try_from(signed).unwrap_or(0), worker)
                    .map(|()| SW_OK),
            ),
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
        assert_eq!(size_of::<PoolRecord>(), 264);
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
            assert_eq!(Worker::new(pool.worker(1)).id(), 6);
            assert_eq!(Worker::new(pool.worker(1)).kind(), 4);
        }
    }

    #[test]
    fn a_socket_pool_must_listen_first() {
        let mut fixture = Fixture::new(1);
        let mut pool = fixture.pool(Script::default());
        pool.set_ipc_mode(IPC_SOCKET);
        pool.set_ipc(pool_ipc_new(true));
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
        pool.set_registry(Box::into_raw(Box::<WorkerRegistry>::default()));
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
            .map(|kind| char::from(u8::from_ne_bytes(swr_v1_worker_symbol(kind).to_ne_bytes())))
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
    fn a_socket_pool_sends_a_dispatch_length_prefixed() {
        use std::io::Read;
        let path = std::env::temp_dir().join(format!("swr-pool-sync-{}.sock", std::process::id()));
        std::fs::remove_file(&path).ok();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let file = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        record(&mut pool).use_socket = 1;
        let ipc = pool_ipc_new(true);
        // SAFETY: a live handle and string.
        unsafe { crate::pool::swr_v1_process_pool_ipc_address(ipc, file.as_ptr(), 0) };
        record(&mut pool).ipc = ipc;
        let mut event = [0u8; 64];
        event[EVENT_LEN] = 3;
        event[HEAD_SIZE as usize..][..3].copy_from_slice(b"abc");
        let mut worker_id = -1;
        // Blocking, then over a stream (upstream's async dispatch).
        for blocking in [true, false] {
            assert_eq!(
                pool.dispatch(event.as_mut_ptr(), &raw mut worker_id, blocking),
                SW_OK
            );
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
        assert_eq!(ops(&pool), [] as [u32; 0]);
        std::fs::remove_file(&path).unwrap();
        record(&mut pool).ipc = ptr::null_mut();
        // SAFETY: the handle made above; it has no listener to close.
        unsafe { pool_ipc_free(ipc, close) };
        // A pool whose listener is gone fails.
        let ipc = pool_ipc_new(true);
        // SAFETY: as above.
        unsafe { crate::pool::swr_v1_process_pool_ipc_address(ipc, file.as_ptr(), 0) };
        record(&mut pool).ipc = ipc;
        for blocking in [true, false] {
            assert_eq!(
                pool.dispatch(event.as_mut_ptr(), &raw mut worker_id, blocking),
                SW_ERR
            );
        }
        record(&mut pool).ipc = ptr::null_mut();
        // SAFETY: as above.
        unsafe { pool_ipc_free(ipc, close) };
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
        assert!(ops(&pool).contains(&OP_WORKER_SHUTDOWN));
    }

    #[test]
    fn an_unknown_child_is_reported() {
        let mut fixture = Fixture::new(0);
        let mut pool = fixture.pool(Script::default());
        pool.set_registry(Box::into_raw(Box::<WorkerRegistry>::default()));
        assert!(!pool.reap(99, 0));
        assert_eq!(pool.h.log, [(LOG_UNKNOWN_CHILD, 99)]);
        pool.destroy();
    }
}
