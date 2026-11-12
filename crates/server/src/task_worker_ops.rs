//! The task worker's facts and effects (`crates/process/src/workers/task_worker.rs`'s
//! `OP_*`) over the server, pool, worker and task records Rust lays out, and the
//! pool callbacks that run its operations: what workers.cc's `TaskWorkerContext` did.
use core::cell::Cell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicU32, Ordering};

use swoole_rs_corebase::{DataHead, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_locks::SharedMutex;
use swoole_rs_process::methods::{
    swoole_set_worker_id, swoole_set_worker_type, worker_set_max_request,
};
use swoole_rs_process::process_pool::{PoolRecord, PoolRef, SW_IPC_MAX_SIZE};
use swoole_rs_process::task_file::{EventHead, task_pack};
use swoole_rs_process::worker_record::{Worker, swr_v1_worker_init, swr_v1_worker_shutdown};
use swoole_rs_process::workers::ops::{
    LOG_INVALID_SOURCE, LOG_READ_TASK, LOG_RESULT_WRITE, LOG_UNKNOWN_COMMAND, OP_ADD_REQUEST_COUNT,
    OP_ALL_CORE_WORKER_NUM, OP_CREATE_TASK_WORKER, OP_ENABLE_SIGNALFD, OP_EXCEEDED_MAX_REQUEST,
    OP_FAIL_REACTOR, OP_FINISH_COUNT, OP_FINISH_HEADER, OP_INFO_TYPE, OP_LOCK_WORKER, OP_MODE,
    OP_NOTIFY_RESULT, OP_NOTIFY_WOULD_BLOCK, OP_ON_PIPE_MESSAGE, OP_ON_TASK, OP_ON_TASK_POOL,
    OP_OPEN_RESULT_FILE, OP_PACK_RESULT, OP_READ_TASK, OP_RESULT_IN_SHM, OP_SELECT_TASK,
    OP_SEND_PIPE, OP_SEND_STREAM, OP_SET_LATEST_TASK, OP_SET_SYSVMSG, OP_SIGNAL_SET,
    OP_SOURCE_WORKER, OP_STOP_ASYNC, OP_STREAM_CONNECTION, OP_TASK_ENABLE_COROUTINE,
    OP_TASK_EXT_FLAGS, OP_TASK_IPC_MODE, OP_TASK_WORKER_NUM, OP_UNLOCK_WORKER,
    OP_WAIT_NOTIFY_WRITABLE, OP_WORKER_ENTER, OP_WORKER_INIT, OP_WORKER_SHUTDOWN,
    OP_WORKER_START_CALLBACK, OP_WORKER_STATUS, OP_WORKER_STOP_CALLBACK, OP_WRITE_RESULT_FILE,
    OP_ZERO_RESULT,
};
use swoole_rs_reactor::Socket;

use crate::callbacks::{CB_PIPE_MESSAGE, CB_TASK};
use crate::commands::CommandHandler;
use crate::event_worker_ops::{
    call_worker_stop_callback, swr_workers_signal_set, worker_signal_handler, worker_start_callback,
};
use crate::view;

mod native;
pub use native::finish;
pub(crate) use native::init_task_workers;

/// The server that owns task pool `pool`: its `ptr`, which `OP_POOL_INIT` sets.
fn server_of(pool: PoolRef<'_>) -> view::Server<'_> {
    // SAFETY: a task pool's `ptr` is the live server that owns it (OP_POOL_INIT).
    unsafe { view::Server::from_raw(pool.ptr()) }
}

/// What an operation runs over.
struct Context<'a> {
    server: view::Server<'a>,
    pool: Option<PoolRef<'a>>,
    worker: Option<Worker<'a>>,
    /// The task being run, or the one being finished: null, or live for `'a`.
    task: *const DataHead,
    socket: Option<Socket<'a>>,
    /// `Server::finish`'s result, its packed form and its file.
    data: &'a [u8],
    packed: *mut DataHead,
    buf: *mut DataHead,
    file: c_int,
    /// The command a task runs, and its unpacked message (`PacketPtr`).
    handler: Option<CommandHandler>,
    packet: (usize, *const c_char),
}

impl<'a> Context<'a> {
    /// The pipe or stream socket the op runs over, if any.
    fn socket(&self) -> Option<Socket<'a>> {
        self.socket
    }
    /// The context over `pool`'s server and `worker`.
    fn of(pool: PoolRef<'a>, worker: Worker<'a>) -> Self {
        Self {
            pool: Some(pool),
            worker: Some(worker),
            ..Self::over(server_of(pool))
        }
    }
    /// The context over `server` alone.
    fn over(server: view::Server<'a>) -> Self {
        Self {
            server,
            pool: None,
            worker: None,
            task: core::ptr::null(),
            socket: None,
            data: &[],
            packed: core::ptr::null_mut(),
            buf: core::ptr::null_mut(),
            file: -1,
            handler: None,
            packet: (0, core::ptr::null()),
        }
    }
    fn serv(&self) -> *mut c_void {
        self.server.raw()
    }
    fn state(&self) -> &'a crate::ServerState {
        self.server.state()
    }
    fn config(&self) -> &'a crate::ServerConfig {
        self.server.config()
    }
    /// The task pool, which the ops reading it run with.
    fn pool(&self) -> PoolRef<'a> {
        self.pool.unwrap_or_else(|| std::process::abort())
    }
    /// The worker, which the ops reading it run with.
    fn worker(&self) -> Worker<'a> {
        self.worker.unwrap_or_else(|| std::process::abort())
    }
    fn task(&self) -> &DataHead {
        // SAFETY: the ops reading it run with a live task.
        unsafe { &*self.task }
    }
    /// Runs `f` over the packed result, which the ops editing it run after packing.
    fn edit_packed<R>(&self, f: impl FnOnce(&mut DataHead) -> R) -> R {
        // SAFETY: the packed result, live and borrowed for `f` alone.
        f(unsafe { &mut *self.packed })
    }
    /// The worker that sent the finishing task.
    fn source(&self) -> u16 {
        self.task().reactor_id.cast_unsigned()
    }
    /// The source worker's slot of `task_results`: an `EventData` per worker.
    fn result_slot(&self) -> *mut u8 {
        let results = self.config().task_results.as_ptr().cast::<u8>();
        results.wrapping_add(usize::from(self.source()) * EVENT_DATA_SIZE)
    }
    /// `buf->size()`: the packed result's header and payload.
    fn packed_size(&self) -> usize {
        // SAFETY: the ops reading it run with the live packed result.
        size_of::<DataHead>() + unsafe { (*self.buf).len } as usize
    }
}

thread_local! {
    /// A copy of the head of the task this task worker last ran (`TaskWorker_onTask`'s),
    /// which `Server::finish` answers when it is given none. A copy, not a pointer:
    /// the task's buffer is gone once its callback returns.
    static LATEST_TASK: Cell<Option<DataHead>> = const { Cell::new(None) };
}

/// `swoole::WorkerType::SW_TASK_WORKER`, `SW_SERVER_EVENT_FINISH`, `SW_PIPE_MASTER`.
const SW_TASK_WORKER: c_int = 4;
const EVENT_FINISH: u8 = 9;
const PIPE_MASTER: c_int = 1;
/// `SW_WORKER_BUSY`, `SW_WORKER_IDLE`.
const SW_WORKER_BUSY: u8 = 1;
const SW_WORKER_IDLE: u8 = 2;
/// `SW_LOG_ERROR`, `SW_WAIT`.
const SW_LOG_ERROR: c_int = 5;
const SW_WAIT: c_int = 2;
/// `SW_EVENT_WRITE`.
const SW_EVENT_WRITE: c_int = 1 << 10;
/// `sizeof(EventData)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "SW_IPC_MAX_SIZE is 8192 or 2048"
)]
const EVENT_DATA_SIZE: usize = SW_IPC_MAX_SIZE as usize;

fn signed(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Answers each task-worker op over the context. The groups each task asks of
/// (`task`, `finish`, then `worker`) come first; setup and commands come last.
impl swoole_rs_process::workers::Answer for Context<'_> {
    #[inline(always)]
    fn answer(&mut self, op: u32, arg: u64) -> u64 {
        // Plain branches, not `or_else` closures, so each group inlines and an op
        // known at the call folds to its arm.
        let ctx = self;
        if let Some(value) = task(ctx, op) {
            return value;
        }
        if let Some(value) = finishing(ctx, op, arg) {
            return value;
        }
        if let Some(value) = worker(ctx, op, arg) {
            return value;
        }
        if let Some(value) = native::native(ctx, op, arg) {
            return value;
        }
        result(ctx, op).unwrap_or_else(|| std::process::abort())
    }
}

/// The task pool and worker: setup, state and callbacks.
#[inline(always)]
fn worker(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let config = ctx.config();
    match op {
        OP_TASK_IPC_MODE => return Some(u64::from(config.task_ipc_mode)),
        OP_TASK_ENABLE_COROUTINE => return Some(u64::from(config.task_enable_coroutine)),
        OP_TASK_WORKER_NUM => return Some(u64::from(config.task_worker_num)),
        OP_MODE => return Some(u64::from(config.mode_)),
        OP_ALL_CORE_WORKER_NUM => {
            return Some(u64::from(config.worker_num) + u64::from(config.task_worker_num));
        }
        OP_EXCEEDED_MAX_REQUEST => return Some(u64::from(ctx.worker().exceeded_max_request())),
        OP_SET_SYSVMSG => ctx.pool().set_schedule_by_sysvmsg(true),
        OP_CREATE_TASK_WORKER => {
            let worker = ctx.pool().worker(usize::try_from(arg).ok()?)?;
            crate::worker_methods::create_worker(ctx.server, worker);
        }
        OP_WORKER_STATUS => {
            let status = if arg != 0 {
                SW_WORKER_BUSY
            } else {
                SW_WORKER_IDLE
            };
            ctx.worker().set_status(status);
        }
        OP_WORKER_SHUTDOWN => swr_v1_worker_shutdown(ctx.worker()),
        OP_STOP_ASYNC => crate::event_worker_ops::stop_async_worker(ctx.server, ctx.worker()),
        OP_ADD_REQUEST_COUNT => ctx.worker().add_request_count(),
        OP_WORKER_ENTER => {
            swoole_set_worker_id(ctx.worker().id());
            swoole_set_worker_type(SW_TASK_WORKER);
        }
        OP_FAIL_REACTOR => log::error(
            c"TaskWorker_onStart",
            format_args!("[TaskWorker] create reactor failed"),
        ),
        OP_ENABLE_SIGNALFD => {
            swoole_rs_corebase::swr_v1_core_set_enable_signalfd(arg != 0);
            if arg == 0 {
                swoole_rs_corebase::set_thread_reactor(core::ptr::null_mut());
            }
        }
        OP_SIGNAL_SET => swr_workers_signal_set(arg, worker_signal_handler),
        OP_WORKER_START_CALLBACK => worker_start_callback(ctx.server, ctx.worker()),
        OP_WORKER_STOP_CALLBACK => call_worker_stop_callback(ctx.server, ctx.worker()),
        OP_WORKER_INIT => {
            swr_v1_worker_init(ctx.worker());
            let pool = ctx.pool();
            worker_set_max_request(pool.max_request(), pool.max_request_grace());
        }
        _ => return None,
    }
    Some(0)
}

/// The task being run: its header, its callbacks and the pipe it came over.
#[inline(always)]
fn task(ctx: &mut Context, op: u32) -> Option<u64> {
    let serv = ctx.serv();
    Some(match op {
        LOG_UNKNOWN_COMMAND => {
            let id = ctx.task().server_fd;
            log::error_log(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_INVALID_COMMAND as c_int,
                c"TaskWorker_call_command_handler",
                format_args!("Unknown command[{id}]"),
            );
            0
        }
        OP_SET_LATEST_TASK => {
            LATEST_TASK.set(Some(*ctx.task()));
            0
        }
        OP_SELECT_TASK => u64::from(!ctx.task.is_null()),
        OP_INFO_TYPE => u64::from(ctx.task().kind),
        OP_TASK_EXT_FLAGS => u64::from(ctx.task().ext_flags),
        OP_ON_PIPE_MESSAGE => {
            if let Some(slot) = ctx.state().callbacks.slot(CB_PIPE_MESSAGE) {
                // SAFETY: the slot takes the server and the task.
                unsafe { slot.call(serv, ctx.task.cast_mut().cast()) };
            }
            0
        }
        OP_ON_TASK => {
            let slot = ctx.state().callbacks.slot(CB_TASK)?;
            // SAFETY: `onTask` takes the server and the task and returns an int.
            signed(unsafe { slot.receive(serv, ctx.task.cast_mut().cast()) })
        }
        OP_READ_TASK => {
            let fd = ctx.socket()?.fd();
            // SAFETY: a whole `EventData` to read into.
            signed(unsafe { libc::read(fd, ctx.buf.cast(), EVENT_DATA_SIZE) } as i64)
        }
        LOG_READ_TASK => {
            let fd = ctx.socket().map_or(-1, Socket::fd);
            log::sys_warning(
                c"TaskWorker_onPipeReceive",
                format_args!("read({fd}, {EVENT_DATA_SIZE}) failed"),
            );
            0
        }
        OP_ON_TASK_POOL => signed(native::run_task(ctx.pool(), ctx.worker(), ctx.buf)),
        _ => return None,
    })
}

/// `Server::finish`: the result goes back to the worker that sent the task.
#[inline(always)]
fn finishing(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_SOURCE_WORKER => {
            ctx.worker = ctx.server.worker_record(ctx.source());
            u64::from(ctx.worker.is_some())
        }
        LOG_INVALID_SOURCE => {
            let id = ctx.source();
            log::warning(c"Server::finish", format_args!("invalid worker_id[{id}]"));
            0
        }
        OP_PACK_RESULT | OP_RESULT_IN_SHM => {
            ctx.packed = if op == OP_PACK_RESULT {
                ctx.buf
            } else {
                ctx.result_slot().cast()
            };
            let packed = ctx.packed.cast::<EventHead>();
            // SAFETY: an `EventData` (the context's, or the source's result slot) and
            // the result's bytes.
            u64::from(unsafe { task_pack(packed, ctx.data.as_ptr().cast(), ctx.data.len()) })
        }
        OP_ZERO_RESULT => {
            ctx.edit_packed(|packed| packed.len = 0);
            0
        }
        OP_FINISH_HEADER => {
            let id = ctx.task().fd;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "DataHead's uint16_t ext_flags"
            )]
            let flags = arg as u16;
            ctx.edit_packed(|packed| {
                packed.ext_flags |= flags;
                packed.kind = EVENT_FINISH;
                packed.fd = id;
            });
            0
        }
        OP_STREAM_CONNECTION => {
            let pool = ctx.worker().pool().cast::<PoolRecord>();
            // SAFETY: its live pool.
            let pool = unsafe { &*pool };
            // Only a stream pool has IPC state: an event worker's pool has none.
            let connection = if pool.use_socket != 0 {
                pool.ipc_connection()
            } else {
                core::ptr::null_mut()
            };
            // SAFETY: null, or the stream pool's live connection for this task.
            ctx.socket = unsafe { Socket::from_ptr(connection.cast()) };
            u64::from(ctx.socket.is_some())
        }
        OP_SEND_STREAM => {
            let send = |data: &[u8]| {
                ctx.socket().map_or(-1, |socket| {
                    swoole_rs_tls::upstream::send_sync(socket, data, 0)
                })
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "htonl's uint32_t, as upstream"
            )]
            let length = (ctx.data.len() as u32).to_be_bytes();
            let mut retval = send(&length);
            if retval > 0 {
                retval = send(ctx.data);
            }
            signed(retval as i64)
        }
        OP_SEND_PIPE => {
            let (buf, size) = (ctx.buf.cast(), ctx.packed_size());
            // SAFETY: the source worker, live, and the packed result.
            let sent = unsafe {
                swoole_rs_process::methods::send_pipe_message(
                    ctx.worker().raw(),
                    buf,
                    size,
                    PIPE_MASTER,
                )
            };
            signed(sent as i64)
        }
        OP_LOCK_WORKER => signed(ctx.worker().lock().map_or(libc::EINVAL, SharedMutex::lock)),
        OP_UNLOCK_WORKER => signed(
            ctx.worker()
                .lock()
                .map_or(libc::EINVAL, SharedMutex::unlock),
        ),
        _ => return None,
    })
}

/// `Server::finish`'s blocking result: the waitall file, the shared slot's
/// count and the source worker's notify pipe.
#[inline(always)]
fn result(ctx: &mut Context, op: u32) -> Option<u64> {
    let slot = ctx.result_slot();
    let path = || {
        let path = slot
            .wrapping_add(size_of::<DataHead>() + 4)
            .cast::<c_char>();
        // SAFETY: a waitall slot holds a counter, then its temporary file's path.
        unsafe { CStr::from_ptr(path) }
    };
    // SAFETY: the source worker's live notify pipe.
    let notify_pipe = || unsafe { &*ctx.state().task_pipe(ctx.source().into()) };
    Some(match op {
        OP_OPEN_RESULT_FILE => {
            let flags = libc::O_APPEND | libc::O_WRONLY;
            ctx.file = swoole_rs_ossupport::file_open(-1, path(), flags, 0);
            u64::from(ctx.file != -1)
        }
        OP_WRITE_RESULT_FILE => {
            let bytes = ctx.packed_size();
            // SAFETY: the packed result's bytes.
            let packed = unsafe { std::slice::from_raw_parts(ctx.buf.cast::<u8>(), bytes) };
            let written = swoole_rs_ossupport::fd_write_all(ctx.file, true, packed);
            u64::from(written == bytes)
        }
        LOG_RESULT_WRITE => {
            let bytes = ctx.packed_size();
            let path = path().to_string_lossy();
            log::sys_warning(
                c"Server::finish",
                format_args!("write({path}, {bytes}) failed"),
            );
            0
        }
        OP_FINISH_COUNT => {
            let count = slot.wrapping_add(size_of::<DataHead>()).cast();
            // SAFETY: the slot's leading counter, which every waiting process shares.
            let count = unsafe { AtomicU32::from_ptr(count) };
            count.fetch_add(1, Ordering::SeqCst);
            swoole_rs_ossupport::file_close(ctx.file);
            ctx.file = -1;
            0
        }
        OP_NOTIFY_RESULT => {
            let flag = 1u64.to_ne_bytes();
            let write = swoole_rs_tls::socket_pair::write;
            let pipe = notify_pipe();
            // SAFETY: a local of that size.
            let written = unsafe { write(pipe, flag.as_ptr().cast(), flag.len()) };
            signed(written as i64)
        }
        OP_NOTIFY_WOULD_BLOCK => {
            let error = errno();
            let socket = notify_pipe().master_end()?;
            u64::from(socket.catch_error(error) == SW_WAIT)
        }
        OP_WAIT_NOTIFY_WRITABLE => {
            let socket = notify_pipe().master_end()?;
            signed(socket.wait_event(-1, SW_EVENT_WRITE))
        }
        _ => return None,
    })
}
