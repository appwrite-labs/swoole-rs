//! The task worker's facts and effects (`crates/process/src/workers/task_worker.rs`'s
//! `OP_*`) over the server, pool, worker and task records Rust lays out.
//! workers.cc's `TaskWorkerContext::call` hands every op it has no case for to
//! [`swr_v1_task_worker_call`].
use core::cell::Cell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicU32, Ordering};

use swoole_rs_corebase::{DataHead, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::methods::{
    swoole_set_worker_id, swoole_set_worker_type, worker_set_max_request,
};
use swoole_rs_process::process_pool::{PoolRecord, SW_IPC_MAX_SIZE};
use swoole_rs_process::task_file::{EventHead, task_pack};
use swoole_rs_process::worker_record::{
    WorkerRecord, pool_of, swr_v1_worker_add_request_count, swr_v1_worker_exceeded_max_requests,
    swr_v1_worker_id, swr_v1_worker_init, swr_v1_worker_shutdown,
};
use swoole_rs_process::workers::entry;
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
use swoole_rs_reactor::SocketRecord;

use crate::callbacks::{CB_PIPE_MESSAGE, CB_TASK};
use crate::event_worker_ops::{set_worker_status, swr_workers_signal_set};
use crate::view;

/// workers.cc's `WorkersNative<2>`, up to the fields only C++ reads.
#[repr(C)]
struct Context {
    serv: *mut c_void,
    pool: *mut PoolRecord,
    worker: *mut WorkerRecord,
    /// The task being run, or the one being finished.
    task: *const DataHead,
    socket: *mut SocketRecord,
    /// `Server::finish`'s result, its packed form and its file.
    data: *const u8,
    data_len: usize,
    packed: *mut DataHead,
    buf: *mut DataHead,
    file: c_int,
}

const _: () = assert!(
    core::mem::offset_of!(Context, file) == 72,
    "workers.cc TaskWorkerContext layout"
);

impl Context {
    /// The worker that sent the finishing task.
    ///
    /// # Safety
    /// The context holds a live task.
    unsafe fn source(&self) -> u16 {
        // SAFETY: per the caller.
        unsafe { (*self.task).reactor_id }.cast_unsigned()
    }

    /// The source worker's slot of `task_results`.
    ///
    /// # Safety
    /// As [`Context::source`], with a live server.
    unsafe fn result_slot(&self) -> *mut u8 {
        // SAFETY: the live server's `task_results`: an `EventData` per worker.
        let results = unsafe { view::config(self.serv) }.task_results.cast::<u8>();
        // SAFETY: per the caller.
        let source = unsafe { self.source() };
        results.wrapping_add(usize::from(source) * EVENT_DATA_SIZE)
    }

    /// `buf->size()`: the packed result's header and payload.
    ///
    /// # Safety
    /// `buf` is the live packed result.
    unsafe fn packed_size(&self) -> usize {
        // SAFETY: per the caller.
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

/// Answers task-worker op `op` with `arg` over the `WorkersNative<2>` at `context`.
///
/// # Safety
/// `context` is workers.cc's live `TaskWorkerContext`, whose objects the op reads are live.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_task_worker_call(
    context: *mut c_void,
    op: u32,
    arg: u64,
) -> u64 {
    // SAFETY: per the caller.
    let ctx = unsafe { &mut *context.cast::<Context>() };
    // SAFETY: per the caller.
    let answer = unsafe { worker(ctx, op, arg) };
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { task(ctx, op) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { finish(ctx, op, arg) });
    // SAFETY: per the caller.
    let answer = answer.or_else(|| unsafe { result(ctx, op) });
    answer.unwrap_or_else(|| std::process::abort())
}

/// The task pool and worker: setup, state and callbacks.
///
/// # Safety
/// As [`swr_v1_task_worker_call`].
unsafe fn worker(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    let (serv, pool, worker) = (ctx.serv, ctx.pool, ctx.worker);
    // SAFETY: the context's live server.
    let config = unsafe { view::config(serv) };
    match op {
        OP_TASK_IPC_MODE => return Some(u64::from(config.task_ipc_mode)),
        OP_TASK_ENABLE_COROUTINE => return Some(u64::from(config.task_enable_coroutine)),
        OP_TASK_WORKER_NUM => return Some(u64::from(config.task_worker_num)),
        OP_MODE => return Some(u64::from(config.mode_)),
        OP_ALL_CORE_WORKER_NUM => {
            return Some(u64::from(config.worker_num) + u64::from(config.task_worker_num));
        }
        OP_EXCEEDED_MAX_REQUEST => {
            // SAFETY: the context's live worker.
            let exceeded = unsafe { swr_v1_worker_exceeded_max_requests(worker) };
            return Some(u64::from(exceeded));
        }
        // SAFETY: the context's live task pool.
        OP_SET_SYSVMSG => unsafe { (*pool).schedule_by_sysvmsg = true },
        OP_CREATE_TASK_WORKER => {
            // SAFETY: the live pool's worker `arg`, one of `task_worker_num`.
            let worker = unsafe { (*pool).workers }.wrapping_add(usize::try_from(arg).ok()?);
            // SAFETY: the live server and its task worker.
            unsafe { crate::worker_methods::create_worker(serv, worker) };
        }
        // SAFETY: the context's live worker.
        OP_WORKER_STATUS => unsafe { set_worker_status(worker, arg) },
        // SAFETY: as above.
        OP_WORKER_SHUTDOWN => unsafe { swr_v1_worker_shutdown(worker) },
        // SAFETY: the context's live server and worker.
        OP_STOP_ASYNC => unsafe { entry::stop_async_worker(serv, worker.cast()) },
        // SAFETY: the context's live worker.
        OP_ADD_REQUEST_COUNT => unsafe { swr_v1_worker_add_request_count(worker) },
        OP_WORKER_ENTER => {
            // SAFETY: as above.
            swoole_set_worker_id(unsafe { swr_v1_worker_id(worker) });
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
        OP_SIGNAL_SET => unsafe { swr_workers_signal_set(arg, entry::worker_signal_handler) },
        // SAFETY: the context's live server and worker.
        OP_WORKER_START_CALLBACK => unsafe { entry::worker_start_callback(serv, worker.cast()) },
        // SAFETY: as above.
        OP_WORKER_STOP_CALLBACK => unsafe { entry::worker_stop_callback(serv, worker.cast()) },
        OP_WORKER_INIT => {
            // SAFETY: the context's live worker.
            unsafe { swr_v1_worker_init(worker) };
            // SAFETY: the context's live task pool.
            let pool = unsafe { &*pool };
            worker_set_max_request(worker, pool.max_request, pool.max_request_grace);
        }
        _ => return None,
    }
    Some(0)
}

/// The task being run: its header, its callbacks and the pipe it came over.
///
/// # Safety
/// As [`swr_v1_task_worker_call`].
unsafe fn task(ctx: &mut Context, op: u32) -> Option<u64> {
    let serv = ctx.serv;
    let task = || {
        // SAFETY: the ops that read it run with a live task.
        unsafe { &*ctx.task }
    };
    Some(match op {
        LOG_UNKNOWN_COMMAND => {
            let id = task().server_fd;
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
        OP_INFO_TYPE => u64::from(task().kind),
        OP_TASK_EXT_FLAGS => u64::from(task().ext_flags),
        OP_ON_PIPE_MESSAGE => {
            // SAFETY: the context's live server.
            let slot = unsafe { view::state_ptr(serv) };
            // SAFETY: its state, live as long as it; the slot takes the server and the task.
            if let Some(slot) = unsafe { &*slot }.callbacks.slot(CB_PIPE_MESSAGE) {
                // SAFETY: as above.
                unsafe { slot.call(serv, ctx.task.cast_mut().cast()) };
            }
            0
        }
        OP_ON_TASK => {
            // SAFETY: the context's live server.
            let state = unsafe { view::state_ptr(serv) };
            // SAFETY: its state; `onTask` takes the server and the task and returns an int.
            let slot = unsafe { &*state }.callbacks.slot(CB_TASK)?;
            // SAFETY: as above.
            signed(unsafe { slot.receive(serv, ctx.task.cast_mut().cast()) })
        }
        OP_READ_TASK => {
            // SAFETY: the live pipe socket the task arrives on.
            let fd = unsafe { (*ctx.socket).fd };
            // SAFETY: a whole `EventData` to read into.
            signed(unsafe { libc::read(fd, ctx.buf.cast(), EVENT_DATA_SIZE) } as i64)
        }
        LOG_READ_TASK => {
            // SAFETY: as above.
            let fd = unsafe { (*ctx.socket).fd };
            log::sys_warning(
                c"TaskWorker_onPipeReceive",
                format_args!("read({fd}, {EVENT_DATA_SIZE}) failed"),
            );
            0
        }
        OP_ON_TASK_POOL => {
            type OnTask = unsafe extern "C-unwind" fn(
                *mut PoolRecord,
                *mut WorkerRecord,
                *mut DataHead,
            ) -> c_int;
            // SAFETY: the context's live task pool; `onTask` is set by `OP_POOL_INIT`.
            let on_task = unsafe { (*ctx.pool).on_task };
            // SAFETY: the pool's `int (*)(ProcessPool *, Worker *, EventData *)`.
            let on_task =
                unsafe { core::mem::transmute::<*const c_void, Option<OnTask>>(on_task) }?;
            // SAFETY: the live pool and worker, and the task just read.
            signed(unsafe { on_task(ctx.pool, ctx.worker, ctx.buf) })
        }
        _ => return None,
    })
}

/// `Server::finish`: the result goes back to the worker that sent the task.
///
/// # Safety
/// As [`swr_v1_task_worker_call`].
unsafe fn finish(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let serv = ctx.serv;
    // SAFETY: per the caller.
    let source = unsafe { ctx.source() };
    Some(match op {
        OP_SOURCE_WORKER => {
            // SAFETY: the live server.
            ctx.worker = unsafe { view::worker(serv, source) };
            u64::from(!ctx.worker.is_null())
        }
        LOG_INVALID_SOURCE => {
            let id = source;
            log::warning(c"Server::finish", format_args!("invalid worker_id[{id}]"));
            0
        }
        OP_PACK_RESULT => {
            ctx.packed = ctx.buf;
            // SAFETY: the context's `EventData` and the result's bytes.
            u64::from(unsafe {
                task_pack(
                    ctx.packed.cast::<EventHead>(),
                    ctx.data.cast(),
                    ctx.data_len,
                )
            })
        }
        OP_RESULT_IN_SHM => {
            // SAFETY: per the caller.
            ctx.packed = unsafe { ctx.result_slot() }.cast();
            // SAFETY: the source worker's result slot and the result's bytes.
            u64::from(unsafe {
                task_pack(
                    ctx.packed.cast::<EventHead>(),
                    ctx.data.cast(),
                    ctx.data_len,
                )
            })
        }
        OP_ZERO_RESULT => {
            // SAFETY: the packed result's header.
            unsafe { (*ctx.packed).len = 0 };
            0
        }
        OP_FINISH_HEADER => {
            // SAFETY: the finishing task.
            let id = unsafe { (*ctx.task).fd };
            // SAFETY: the packed result's header.
            let packed = unsafe { &mut *ctx.packed };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "DataHead's uint16_t ext_flags"
            )]
            let flags = arg as u16;
            packed.ext_flags |= flags;
            packed.kind = EVENT_FINISH;
            packed.fd = id;
            0
        }
        OP_STREAM_CONNECTION => {
            // SAFETY: the source worker.
            let pool = unsafe { pool_of(ctx.worker) }.cast::<PoolRecord>();
            // SAFETY: its live pool.
            let pool = unsafe { &*pool };
            // Only a stream pool has IPC state: an event worker's pool has none.
            ctx.socket = if pool.use_socket != 0 {
                // SAFETY: the stream pool's live IPC state.
                unsafe { swoole_rs_process::pool::swr_v1_process_pool_ipc_connection(pool.ipc) }
                    .cast()
            } else {
                core::ptr::null_mut()
            };
            u64::from(!ctx.socket.is_null())
        }
        OP_SEND_STREAM => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "htonl's uint32_t, as upstream"
            )]
            let length = (ctx.data_len as u32).to_be_bytes();
            // SAFETY: the stream's live connection and the length's bytes.
            let mut retval = unsafe {
                swoole_rs_tls::upstream::socket_send_sync(
                    ctx.socket,
                    length.as_ptr().cast(),
                    length.len(),
                    0,
                )
            };
            if retval > 0 {
                // SAFETY: as above, with the result's bytes.
                retval = unsafe {
                    swoole_rs_tls::upstream::socket_send_sync(
                        ctx.socket,
                        ctx.data.cast(),
                        ctx.data_len,
                        0,
                    )
                };
            }
            signed(retval as i64)
        }
        OP_SEND_PIPE => {
            // SAFETY: the packed result.
            let size = unsafe { ctx.packed_size() };
            // SAFETY: the source worker, live, and the packed result.
            let sent = unsafe {
                entry::send_to_worker_from_worker(
                    serv,
                    ctx.worker,
                    ctx.buf.cast(),
                    size,
                    PIPE_MASTER,
                )
            };
            signed(sent as i64)
        }
        OP_LOCK_WORKER => {
            // SAFETY: the live source worker.
            let lock = unsafe { (*ctx.worker).lock };
            // SAFETY: its live mutex.
            signed(unsafe { swoole_rs_locks::swr_v1_locks_lock(lock, MUTEX, libc::LOCK_EX, -1, 0) })
        }
        OP_UNLOCK_WORKER => {
            // SAFETY: the live source worker.
            let lock = unsafe { (*ctx.worker).lock };
            // SAFETY: its live mutex, which this process holds.
            signed(unsafe { swoole_rs_locks::swr_v1_locks_unlock(lock, MUTEX) })
        }
        _ => return None,
    })
}

/// `Server::finish`'s blocking result: the waitall file, the shared slot's
/// count and the source worker's notify pipe.
///
/// # Safety
/// As [`swr_v1_task_worker_call`].
unsafe fn result(ctx: &mut Context, op: u32) -> Option<u64> {
    // SAFETY: per the caller.
    let slot = unsafe { ctx.result_slot() };
    // SAFETY: a waitall slot holds a counter, then its temporary file's path.
    let path = || unsafe {
        CStr::from_ptr(
            slot.wrapping_add(size_of::<DataHead>() + 4)
                .cast::<c_char>(),
        )
    };
    // SAFETY: per the caller.
    let source = unsafe { ctx.source() };
    let notify_pipe = || {
        // SAFETY: the context's live server.
        let state = unsafe { view::state_ptr(ctx.serv) };
        // SAFETY: its state, live as long as it.
        unsafe { &*state }.task_pipe(source.into())
    };
    Some(match op {
        OP_OPEN_RESULT_FILE => {
            // SAFETY: a NUL-terminated path.
            ctx.file = unsafe {
                swoole_rs_ossupport::swr_v1_os_file_open(
                    -1,
                    path().as_ptr(),
                    libc::O_APPEND | libc::O_WRONLY,
                    0,
                )
            };
            u64::from(ctx.file != -1)
        }
        OP_WRITE_RESULT_FILE => {
            // SAFETY: the packed result.
            let bytes = unsafe { ctx.packed_size() };
            // SAFETY: the open file and the packed result.
            let written =
                unsafe { swoole_rs_ossupport::fd_write_all(ctx.file, true, ctx.buf.cast(), bytes) };
            u64::from(written == bytes)
        }
        LOG_RESULT_WRITE => {
            // SAFETY: the packed result.
            let bytes = unsafe { ctx.packed_size() };
            let path = path().to_string_lossy();
            log::sys_warning(
                c"Server::finish",
                format_args!("write({path}, {bytes}) failed"),
            );
            0
        }
        OP_FINISH_COUNT => {
            // SAFETY: the slot's leading counter, which every waiting process shares.
            let count =
                unsafe { AtomicU32::from_ptr(slot.wrapping_add(size_of::<DataHead>()).cast()) };
            count.fetch_add(1, Ordering::SeqCst);
            swoole_rs_ossupport::swr_v1_os_file_close(ctx.file);
            ctx.file = -1;
            0
        }
        OP_NOTIFY_RESULT => {
            let flag = 1u64.to_ne_bytes();
            // SAFETY: the source worker's live notify pipe.
            let written = unsafe {
                swoole_rs_tls::socket_pair::write(notify_pipe(), flag.as_ptr().cast(), flag.len())
            };
            signed(written as i64)
        }
        OP_NOTIFY_WOULD_BLOCK => {
            let error = errno();
            // SAFETY: the source worker's live notify pipe.
            let socket = unsafe { (*notify_pipe()).master_socket };
            // SAFETY: its live master socket.
            let socket = unsafe { &mut *socket };
            u64::from(socket.catch_error(error) == SW_WAIT)
        }
        OP_WAIT_NOTIFY_WRITABLE => {
            // SAFETY: as above.
            let socket = unsafe { (*notify_pipe()).master_socket };
            // SAFETY: as above.
            signed(unsafe {
                swoole_rs_reactor::socket_upstream::socket_wait_event(socket, -1, SW_EVENT_WRITE)
            })
        }
        _ => return None,
    })
}
