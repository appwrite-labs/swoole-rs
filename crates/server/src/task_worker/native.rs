//! The task pool's native side: its callbacks (`TaskWorker_*`), the coroutine loop
//! over its pipe, the commands sent to it and `Server::finish`, which sends a task's
//! result back to the worker that dispatched it.
use crate::Mode;
use core::cell::Cell;
use core::ffi::{CStr, c_char, c_int};
use core::sync::atomic::{AtomicU32, Ordering};
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::{CCast, DataHead, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_locks::SharedMutex;
use swoole_rs_process::methods::{
    send_pipe_message, set_worker_type, swoole_get_worker_id, swoole_set_worker_id,
    worker_set_max_request,
};
use swoole_rs_process::process_pool::{Hooks, PoolRef, SW_IPC_MAX_SIZE};
use swoole_rs_process::task_file::{Packed, TaskBuffer};
use swoole_rs_process::worker_record::{
    Worker, WorkerStatus, WorkerType, swr_v1_worker_init, swr_v1_worker_shutdown,
};
use swoole_rs_reactor::{Event, Running, Socket};

use crate::callbacks::TaskSlot;
use crate::event_worker::native::{
    call_worker_stop_callback, stop_async_worker, swr_workers_signal_set, worker_signal_handler,
    worker_start_callback,
};
use crate::objects::{EVENT_READ, Reactor, in_worker_bus, set_nonblock, worker_pipe};
use crate::view;

const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
/// `swoole::TaskFlag` bits.
const TASK_NONBLOCK: u16 = 1 << 2;
const TASK_WAITALL: u16 = 1 << 4;
/// `SW_PIPE_MASTER`, `SW_FD_PIPE`, `SW_WAIT` and `SW_EVENT_WRITE`.
const PIPE_MASTER: c_int = 1;
const FD_PIPE: c_int = 3;
const SW_WAIT: c_int = swoole_rs_corebase::constants::ReturnCode::SW_WAIT as c_int;
const EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;
/// `SW_LOG_WARNING`, `SW_LOG_ERROR`, `SW_ERROR_SOCKET_POLL_TIMEOUT` and
/// `SW_ERROR_SERVER_SEND_TO_WOKER_TIMEOUT`.
const SW_LOG_WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;
const SW_LOG_ERROR: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_ERROR as c_int;
const POLL_TIMEOUT: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SOCKET_POLL_TIMEOUT as c_int;
const SEND_TO_WORKER_TIMEOUT: c_int = 9014;
/// `sizeof(EventData)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "SW_IPC_MAX_SIZE is 8192 or 2048"
)]
const EVENT_DATA_SIZE: usize = SW_IPC_MAX_SIZE as usize;

/// `swoole::EventData`, aligned for its head, to read a task into.
#[repr(C, align(8))]
struct EventData([u8; EVENT_DATA_SIZE]);

thread_local! {
    /// A copy of the head of the task this task worker last ran, which
    /// `Server::finish` answers when it is given none. A copy, not a pointer: the
    /// task's buffer is gone once its callback returns.
    static LATEST_TASK: Cell<Option<DataHead>> = const { Cell::new(None) };
}

/// The server that owns task pool `pool`: its `ptr`, which [`init`] sets.
fn server_of(pool: PoolRef<'_>) -> view::Server<'_> {
    // SAFETY: a task pool's `ptr` is the live server that owns it (`init`).
    unsafe { view::Server::from_raw(pool.ptr()) }
}

/// `Server::init_task_workers()`, after the task pool is made and before it starts:
/// its callbacks, its scheduling and its workers.
pub(crate) fn init(serv: view::Server<'_>) -> bool {
    let config = serv.config();
    let (mode, coroutine) = (config.task_ipc(), config.task_enable_coroutine.get());
    if !super::coroutines_allowed(mode, coroutine) {
        return false;
    }
    let pool = serv.task_pool_ref();
    let hooks = Hooks {
        ptr: Some(serv.raw()),
        on_task: Some(on_task),
        on_worker_start: Some(on_start),
        on_worker_stop: Some(on_stop),
        // A coroutine task worker runs its tasks in an event loop.
        main_loop: coroutine.then_some(loop_async),
        ..Hooks::default()
    };
    // SAFETY: the hooks read the pool's `ptr` as the server that owns the pool
    // (`server_of`), which is what this sets it to; it outlives its pool.
    unsafe { pool.install(&hooks) };
    if super::schedule_by_sysvmsg(mode) {
        pool.set_schedule_by_sysvmsg(true);
    }
    for index in 0..config.task_worker_num.get() as usize {
        if let Some(worker) = pool.worker(index) {
            crate::worker_methods::create_worker(serv, worker);
        }
    }
    true
}

/// `TaskWorker_onTask`: the pool's task callback.
extern "C-unwind" fn on_task(pool: PoolRef<'_>, worker: Worker<'_>, task: Packed<'_>) -> c_int {
    run_task(server_of(pool), worker, task)
}

/// `TaskWorker_onTask`: one task, pipe message, shutdown or command.
fn run_task(serv: view::Server<'_>, worker: Worker<'_>, task: Packed<'_>) -> c_int {
    let head = task.head();
    LATEST_TASK.set(Some(head));
    worker.set_status(WorkerStatus::busy(true));
    let callbacks = &serv.state().callbacks;
    let ret = match head.event() {
        Some(ServerEvent::PipeMessage) => {
            callbacks.pipe_message(serv, task);
            OK
        }
        Some(ServerEvent::Shutdown) => {
            swr_v1_worker_shutdown(worker);
            if swoole_rs_reactor::event_api::swoole_event_is_available() {
                stop_async_worker(serv, worker);
            }
            return OK;
        }
        Some(ServerEvent::CommandRequest) => command(serv, worker, task),
        _ => {
            let ret = callbacks.task(TaskSlot::Task, serv, task);
            // Only server tasks count as requests, not pipe messages or commands.
            worker.add_request_count();
            ret
        }
    };
    worker.set_status(WorkerStatus::busy(false));
    ret
}

/// `TaskWorker_call_command_handler`: the command `task` names, answered over the
/// command reply socket.
fn command(serv: view::Server<'_>, worker: Worker<'_>, task: Packed<'_>) -> c_int {
    let head = task.head();
    let state = serv.state();
    let Some(handler) = crate::commands::handler(state, i32::from(head.server_fd)) else {
        log::error_log(
            SW_LOG_ERROR,
            E::SW_ERROR_SERVER_INVALID_COMMAND as c_int,
            c"TaskWorker_call_command_handler",
            format_args!("Unknown command[{}]", head.server_fd),
        );
        return OK;
    };
    // A copy: the handler may use the thread buffer the message unpacks into.
    let message = swoole_rs_buffer::string::with_thread_buffer(|buffer| {
        task.unpack(buffer).map(|(data, _)| data.to_vec())
    });
    let Some(message) = message else {
        return OK;
    };
    let Some(sock) = crate::commands::reply_socket(state, serv) else {
        return ERR;
    };
    let id = u16::try_from(worker.id()).unwrap_or(u16::MAX);
    let reply = (state.message_bus(), sock);
    if crate::commands::answer(&handler, serv, &message, reply, head.fd, id) {
        OK
    } else {
        ERR
    }
}

/// `TaskWorker_onStart`.
extern "C-unwind" fn on_start(pool: PoolRef<'_>, worker: Worker<'_>) {
    let serv = server_of(pool);
    swoole_set_worker_id(worker.id());
    set_worker_type(WorkerType::Task);
    // Make the task worker support asynchronous.
    let coroutine = serv.config().task_enable_coroutine.get();
    if coroutine && swoole_rs_reactor::swoole_event_init(0) < 0 {
        let m = format_args!("[TaskWorker] create reactor failed");
        log::error(c"TaskWorker_onStart", m);
        return;
    }
    swoole_rs_corebase::swr_v1_core_set_enable_signalfd(coroutine);
    if !coroutine {
        swoole_rs_corebase::set_thread_reactor(core::ptr::null_mut());
    }
    signal_init(serv);
    worker_start_callback(serv, worker);
    swr_v1_worker_init(worker);
    worker_set_max_request(pool.max_request(), pool.max_request_grace());
}

/// `Server::worker_signal_init`, leaving SIGPIPE to a task's own code; none in
/// thread mode, where the process's signals are the master's.
fn signal_init(serv: view::Server<'_>) {
    if serv.config().mode_ == Mode::Thread {
        return;
    }
    let none = crate::event_worker::HANDLER_NONE;
    for (signo, handler) in crate::event_worker::signals(none) {
        swr_workers_signal_set(signo, handler, worker_signal_handler);
    }
}

/// `TaskWorker_onStop`.
extern "C-unwind" fn on_stop(pool: PoolRef<'_>, worker: Worker<'_>) {
    swoole_rs_reactor::swoole_event_free();
    call_worker_stop_callback(server_of(pool), worker);
}

/// `TaskWorker_loop_async`: the coroutine task worker's event loop over its pipe.
extern "C-unwind" fn loop_async(pool: PoolRef<'_>, worker: Worker<'_>) -> c_int {
    let serv = server_of(pool);
    let (state, config) = (serv.state(), serv.config());
    worker.set_status(WorkerStatus::busy(false));
    in_worker_bus(state, config, worker_pipe(worker, false), |pipe| {
        set_nonblock(pipe);
        let reactor = Reactor::thread();
        reactor.handle().set_object(pool.raw().cast());
        reactor.add(pipe, EVENT_READ);
        reactor.set_handler(FD_PIPE, EVENT_READ, on_pipe_receive);
    });
    // A task's result may be larger than a pipe's buffer: let every worker's grow.
    let workers = config.worker_num.get() + config.task_worker_num.get();
    for id in (0..workers).filter_map(|id| u16::try_from(id).ok()) {
        for pipe in [serv.worker_pipe(id, false), serv.worker_pipe(id, true)] {
            in_worker_bus(state, config, pipe, |pipe| {
                pipe.edit(|pipe| pipe.output.buffer_size = u32::MAX);
            });
        }
    }
    swoole_rs_reactor::event_api::swoole_event_wait()
}

/// `TaskWorker_onPipeReceive`: a task from a worker, in the coroutine loop.
extern "C" fn on_pipe_receive(reactor: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: `loop_async` set this reactor's `ptr` to the live task pool.
    let pool = unsafe { PoolRef::new(reactor.object().cast()) };
    let serv = server_of(pool);
    let id = u16::try_from(swoole_get_worker_id()).unwrap_or(u16::MAX);
    let Some(worker) = serv.worker_record(id) else {
        return ERR;
    };
    let fd = event.socket().map_or(-1, Socket::fd);
    let mut buf = EventData([0; EVENT_DATA_SIZE]);
    // SAFETY: a whole `EventData` to read into.
    let read = unsafe { libc::read(fd, buf.0.as_mut_ptr().cast(), EVENT_DATA_SIZE) };
    let task = usize::try_from(read)
        .ok()
        .filter(|&read| read > 0)
        .and_then(|read| Packed::from_bytes(buf.0.get(..read)?));
    let Some(task) = task else {
        let m = format_args!("read({fd}, {EVENT_DATA_SIZE}) failed");
        log::sys_warning(c"TaskWorker_onPipeReceive", m);
        return ERR;
    };
    let ret = run_task(serv, worker, task);
    // The maximum number of requests: the worker exits.
    if worker.exceeded_max_request() {
        stop_async_worker(serv, worker);
    }
    ret
}

/// `Server::finish(data, data_len, flags, current_task)`: the task's result to the
/// worker that sent it, through its pipe (nonblocking tasks) or the shared result
/// slot and its notify pipe (blocking ones). With no `current_task`, the task this
/// worker ran last; false when there is none.
#[must_use]
pub fn finish(
    serv: view::Server<'_>,
    data: &[u8],
    flags: c_int,
    current_task: Option<&DataHead>,
) -> bool {
    let latest = LATEST_TASK.get();
    let task = current_task.or(latest.as_ref());
    let Some(task) = super::answerable(serv.config().task_worker_num.get(), task) else {
        return false;
    };
    let source = task.reactor_id.cast_unsigned();
    let Some(worker) = serv.worker_record(source) else {
        log::warning(
            c"Server::finish",
            format_args!("invalid worker_id[{source}]"),
        );
        return false;
    };
    let flags = flags.c_cast::<u16>();
    let reply = |buf: &mut TaskBuffer, flags| {
        buf.add_flags(flags);
        buf.set_kind(ServerEvent::Finish.code());
        buf.set_fd(task.fd);
    };
    let mut buf = TaskBuffer::default();
    let sent = if task.ext_flags & TASK_NONBLOCK != 0 {
        if !buf.pack(data) {
            log::warning(c"Server::finish", format_args!("large task pack failed()"));
            return false;
        }
        reply(&mut buf, super::nonblocking_flags(flags, task.ext_flags));
        match stream_connection(worker) {
            Some(socket) => send_stream(socket, data),
            None => send_pipe_message(worker, buf.bytes(), PIPE_MASTER),
        }
    } else {
        let slot = serv.config().task_results.as_ptr().cast::<u8>();
        let slot = slot.wrapping_add(usize::from(source) * EVENT_DATA_SIZE);
        let lock = worker.lock();
        _ = lock.map(SharedMutex::lock);
        let stored = if task.ext_flags & TASK_WAITALL != 0 {
            write_waitall(slot, &mut buf, data, |buf| reply(buf, flags));
            true
        } else if buf.pack(data) {
            reply(&mut buf, flags);
            let bytes = buf.bytes();
            // SAFETY: the source worker's `EventData` result slot, which its lock,
            // held, keeps this process alone writing; a packed result fills at most one.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), slot, bytes.len()) };
            true
        } else {
            false
        };
        _ = lock.map(SharedMutex::unlock);
        if !stored {
            log::warning(c"Server::finish", format_args!("large task pack failed()"));
            return false;
        }
        // SAFETY: the source worker's live notify pipe.
        let pipe = unsafe { &*serv.state().task_pipe(source.into()) };
        let flag = 1u64.to_ne_bytes();
        super::notify(
            || swoole_rs_tls::socket_pair::write_from(pipe, &flag),
            || {
                let error = errno();
                pipe.master_end()
                    .is_some_and(|socket| socket.catch_error(error) == SW_WAIT)
            },
            || {
                pipe.master_end()
                    .is_some_and(|socket| socket.wait_event(-1, EVENT_WRITE) == 0)
            },
        )
    };
    if sent < 0 {
        let error = errno();
        let timed_out =
            error == libc::EAGAIN || error == libc::ETIMEDOUT || log::last_error() == POLL_TIMEOUT;
        if timed_out {
            let m = format_args!("send result to worker timed out");
            log::error_log(SW_LOG_WARNING, SEND_TO_WORKER_TIMEOUT, c"Server::finish", m);
        } else {
            log::sys_warning(
                c"Server::finish",
                format_args!("send result to worker failed"),
            );
        }
        return false;
    }
    true
}

/// A waitall task's result, appended to the temporary file its result `slot` names,
/// and the slot's count of results raised. An empty result when it can't be packed.
fn write_waitall(
    slot: *mut u8,
    buf: &mut TaskBuffer,
    data: &[u8],
    reply: impl Fn(&mut TaskBuffer),
) {
    let path = slot
        .wrapping_add(size_of::<DataHead>() + 4)
        .cast::<c_char>();
    // SAFETY: a waitall slot holds a counter, then its temporary file's path.
    let path = unsafe { CStr::from_ptr(path) };
    let file = swoole_rs_ossupport::file_open(-1, path, libc::O_APPEND | libc::O_WRONLY, 0);
    if file == -1 {
        return;
    }
    // A failed pack leaves the result empty.
    if !buf.pack(data) {
        log::warning(c"Server::finish", format_args!("large task pack failed()"));
    }
    reply(buf);
    let bytes = buf.bytes();
    if swoole_rs_ossupport::fd_write_all(file, true, bytes) != bytes.len() {
        let (path, len) = (path.to_string_lossy(), bytes.len());
        log::sys_warning(
            c"Server::finish",
            format_args!("write({path}, {len}) failed"),
        );
    }
    let count = slot.wrapping_add(size_of::<DataHead>()).cast();
    // SAFETY: the slot's leading counter, which every waiting process shares.
    unsafe { AtomicU32::from_ptr(count) }.fetch_add(1, Ordering::SeqCst);
    swoole_rs_ossupport::file_close(file);
}

/// The stream pool's connection for the task `worker` runs: none in an event
/// worker's pool, which has no IPC state.
fn stream_connection(worker: Worker<'_>) -> Option<Socket<'_>> {
    let pool = worker.pool_ref()?;
    if !pool.use_socket() {
        return None;
    }
    // SAFETY: null, or the stream pool's live connection for this task.
    unsafe { Socket::from_ptr(pool.ipc_connection().cast()) }
}

/// The result over a stream: its length, big-endian, then its bytes.
fn send_stream(socket: Socket<'_>, data: &[u8]) -> isize {
    let send = |data: &[u8]| swoole_rs_tls::upstream::send_sync(socket, data, 0);
    let sent = send(&data.len().c_cast::<u32>().to_be_bytes());
    if sent > 0 { send(data) } else { sent }
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}
