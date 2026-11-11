//! The task worker's facts and effects (`crates/process/src/workers/task_worker.rs`'s
//! `OP_*`) over the server, pool, worker and task records Rust lays out, and the
//! pool callbacks that run its operations: what workers.cc's `TaskWorkerContext` did.
use core::cell::Cell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

use swoole_rs_corebase::{DataHead, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::methods::{
    swoole_set_worker_id, swoole_set_worker_type, worker_set_max_request,
};
use swoole_rs_process::process_pool::{PoolRecord, SW_IPC_MAX_SIZE};
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
use swoole_rs_reactor::{Socket, SocketRecord};

use crate::callbacks::{CB_PIPE_MESSAGE, CB_TASK};
use crate::commands::CommandHandler;
use crate::event_worker_ops::{
    call_worker_stop_callback, swr_workers_signal_set, worker_signal_handler, worker_start_callback,
};
use crate::view;

mod native;
pub(crate) use native::enter;

/// The server's task pool, live and owned by the server for `'a`. Native code
/// passes one as a `ProcessPool *`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct TaskPool<'a> {
    raw: *mut PoolRecord,
    live: PhantomData<&'a PoolRecord>,
}

impl<'a> TaskPool<'a> {
    /// `server`'s task pool.
    fn of(server: view::Server<'a>) -> Self {
        Self {
            raw: server.task_pool(),
            live: PhantomData,
        }
    }
    /// Runs `f` over the record, borrowed for the call alone.
    fn edit<R>(self, f: impl FnOnce(&mut PoolRecord) -> R) -> R {
        // SAFETY: a live pool, which this task worker's thread alone touches while
        // its callbacks run; the borrow ends with `f`.
        f(unsafe { &mut *self.raw })
    }
    /// The pool's server: a task pool's `ptr`.
    fn server(self) -> view::Server<'a> {
        let serv = self.edit(|pool| pool.ptr);
        // SAFETY: OP_POOL_INIT sets a task pool's `ptr` to the server that owns it.
        unsafe { view::Server::from_raw(serv) }
    }
}

/// What an operation runs over.
struct Context<'a> {
    server: view::Server<'a>,
    pool: Option<TaskPool<'a>>,
    worker: Option<Worker<'a>>,
    /// The task being run, or the one being finished: null, or live for `'a`.
    task: *const DataHead,
    socket: *mut SocketRecord,
    /// `Server::finish`'s result, its packed form and its file.
    data: *const u8,
    data_len: usize,
    packed: *mut DataHead,
    buf: *mut DataHead,
    file: c_int,
    /// The command a task runs, and its unpacked message (`PacketPtr`).
    handler: Option<CommandHandler>,
    packet: (usize, *const c_char),
}

impl<'a> Context<'a> {
    /// The pipe or stream socket the op runs over, if any.
    fn socket(&self) -> Option<Socket<'_>> {
        // SAFETY: null, or the live socket the method that built the context lent.
        unsafe { Socket::from_ptr(self.socket) }
    }
    /// The context over `pool`'s server and `worker`.
    fn of(pool: TaskPool<'a>, worker: Worker<'a>) -> Self {
        Self {
            pool: Some(pool),
            worker: Some(worker),
            ..Self::over(pool.server())
        }
    }
    /// The context over `server` alone.
    fn over(server: view::Server<'a>) -> Self {
        Self {
            server,
            pool: None,
            worker: None,
            task: core::ptr::null(),
            socket: core::ptr::null_mut(),
            data: core::ptr::null(),
            data_len: 0,
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
    fn config(&self) -> &'a mut crate::ServerConfig {
        // SAFETY: the context's live server.
        unsafe { self.server.config_mut() }
    }
    /// The task pool, which the ops reading it run with.
    fn pool(&self) -> TaskPool<'a> {
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
    /// The task this task worker runs (`TaskWorker_onTask`'s), which `Server::finish`
    /// answers when it is given none.
    static LATEST_TASK: Cell<*const DataHead> = const { Cell::new(core::ptr::null()) };
}

/// `swoole::WorkerType::SW_TASK_WORKER`, `SW_SERVER_EVENT_FINISH`, `SW_PIPE_MASTER`.
const SW_TASK_WORKER: c_int = 4;
const EVENT_FINISH: u8 = 9;
const PIPE_MASTER: c_int = 1;
/// `SW_WORKER_BUSY`, `SW_WORKER_IDLE`.
const SW_WORKER_BUSY: u8 = 1;
const SW_WORKER_IDLE: u8 = 2;
/// `SW_LOG_ERROR`, `SW_WAIT`, `Lock::MUTEX`.
const SW_LOG_ERROR: c_int = 5;
const SW_WAIT: c_int = 2;
const MUTEX: i32 = 3;
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
        if let Some(value) = finish(ctx, op, arg) {
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
    let worker = ctx.worker.map_or(core::ptr::null_mut(), Worker::raw);
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
        OP_SET_SYSVMSG => ctx.pool().edit(|pool| pool.schedule_by_sysvmsg = true),
        OP_CREATE_TASK_WORKER => {
            let id = usize::try_from(arg).ok()?;
            let worker = ctx.pool().edit(|pool| pool.workers.wrapping_add(id));
            // SAFETY: the live server and its task worker `arg`, one of `task_worker_num`.
            unsafe { crate::worker_methods::create_worker(ctx.server, worker) };
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
        // SAFETY: Rust's signal handler for the worker.
        OP_SIGNAL_SET => unsafe { swr_workers_signal_set(arg, worker_signal_handler) },
        OP_WORKER_START_CALLBACK => worker_start_callback(ctx.server, ctx.worker()),
        OP_WORKER_STOP_CALLBACK => call_worker_stop_callback(ctx.server, ctx.worker()),
        OP_WORKER_INIT => {
            swr_v1_worker_init(ctx.worker());
            let (max, grace) = ctx
                .pool()
                .edit(|pool| (pool.max_request, pool.max_request_grace));
            worker_set_max_request(worker, max, grace);
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
            LATEST_TASK.set(ctx.task);
            0
        }
        OP_SELECT_TASK => {
            if ctx.task.is_null() {
                ctx.task = LATEST_TASK.get();
            }
            0
        }
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
fn finish(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
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
            u64::from(unsafe { task_pack(packed, ctx.data.cast(), ctx.data_len) })
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
            ctx.socket = if pool.use_socket != 0 {
                // SAFETY: the stream pool's live IPC state.
                swoole_rs_process::pool::pool_ipc_connection(unsafe { &*pool.ipc }).cast()
            } else {
                core::ptr::null_mut()
            };
            u64::from(!ctx.socket.is_null())
        }
        OP_SEND_STREAM => {
            let send = |data: *const u8, length| {
                let send_sync = swoole_rs_tls::upstream::socket_send_sync;
                // SAFETY: the stream's live connection and `length` bytes at `data`.
                unsafe { send_sync(ctx.socket, data.cast(), length, 0) }
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "htonl's uint32_t, as upstream"
            )]
            let length = (ctx.data_len as u32).to_be_bytes();
            let mut retval = send(length.as_ptr(), length.len());
            if retval > 0 {
                retval = send(ctx.data, ctx.data_len);
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
        OP_LOCK_WORKER => {
            let lock = ctx.worker().lock();
            // SAFETY: the source worker's live mutex.
            signed(unsafe { swoole_rs_locks::swr_v1_locks_lock(lock, MUTEX, libc::LOCK_EX, -1, 0) })
        }
        OP_UNLOCK_WORKER => {
            let lock = ctx.worker().lock();
            // SAFETY: the source worker's live mutex, which this process holds.
            signed(unsafe { swoole_rs_locks::swr_v1_locks_unlock(lock, MUTEX) })
        }
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
    let notify_pipe = || ctx.state().task_pipe(ctx.source().into());
    let master_socket = || {
        // SAFETY: the source worker's live notify pipe.
        unsafe { (*notify_pipe()).master_socket }
    };
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
            // SAFETY: the source worker's live notify pipe.
            let pipe = unsafe { &*notify_pipe() };
            // SAFETY: a local of that size.
            let written = unsafe { write(pipe, flag.as_ptr().cast(), flag.len()) };
            signed(written as i64)
        }
        OP_NOTIFY_WOULD_BLOCK => {
            let error = errno();
            let socket = crate::objects::socket(master_socket().cast())?;
            u64::from(socket.catch_error(error) == SW_WAIT)
        }
        OP_WAIT_NOTIFY_WRITABLE => {
            let socket = crate::objects::socket(master_socket().cast())?;
            signed(socket.wait_event(-1, SW_EVENT_WRITE))
        }
        _ => return None,
    })
}
