// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/async_thread.cc (Swoole contributors).
//! `AsyncThreads`, a reactor thread's handle on crates/ossupport's shared pool, and
//! `async::dispatch`, under their upstream (Itanium) symbols. The pool runs an event
//! on a worker and writes its address to the completion pipe; the reactor reads it
//! back and runs its callback. Each pool copy of an event holds a reference to its
//! request record. The ossupport patch deletes `src/os/async_thread.cc`.
use crate::offload::AsyncEvent;
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::{
    log, microtime, set_thread_async_threads, swoole_thread_clean, swoole_thread_init,
    swr_v1_core_aio_core_worker_num, swr_v1_core_aio_default_socket, swr_v1_core_aio_max_idle_time,
    swr_v1_core_aio_max_wait_time, swr_v1_core_aio_worker_num, swr_v1_core_cpu_num,
    swr_v1_core_set_aio_default_socket, thread_async_threads,
};
use swoole_rs_ossupport::{Pool, PoolHooks, pool_complete};
use swoole_rs_reactor::event_api::{
    on_destroy, swoole_event_add, swoole_event_del, swr_v1_event_current,
};
use swoole_rs_reactor::{Event, Running, SocketRecord};
use swoole_rs_tls::socket_pair::{SocketPair, close, drop, unix_socket};
use swoole_rs_tls::upstream::socket_write_sync;

/// `SW_FD_AIO`, `SW_EVENT_READ` and `Reactor::EXIT_CONDITION_AIO_TASK`.
const FD_AIO: c_int = 5;
const EVENT_READ: c_int = 1 << 9;
const EXIT_CONDITION_AIO_TASK: c_int = 5;
/// `SW_ERROR_SYSTEM_CALL_FAIL`, `SW_ERROR_WRONG_OPERATION`, `SW_ERROR_AIO_BAD_REQUEST`
/// and `SW_ERROR_AIO_CANCELED`.
const SYSTEM_CALL_FAIL: c_int = 502;
const WRONG_OPERATION: c_int = 509;
const AIO_BAD_REQUEST: c_int = 4001;
const AIO_CANCELED: c_int = 4002;

/// `swoole::AsyncThreads`, opaque to native code: the events in flight, the
/// completion pipe (the master end writes, the worker end reads) and the pool.
pub struct AsyncThreads {
    task_num: usize,
    pipe: SocketPair,
    pool: Option<&'static Pool>,
}

/// `SwooleTG.async_threads`.
fn current() -> *mut AsyncThreads {
    thread_async_threads().cast()
}

/// `f` over the thread's handle, or none without one. Only this module calls it, never
/// from inside another `f`, and no `f` runs a callback that could free the handle.
fn with_current<R>(f: impl FnOnce(&mut AsyncThreads) -> R) -> Option<R> {
    // SAFETY: null or this thread's live handle, which only this thread touches and
    // no other borrow reaches while `f` runs.
    unsafe { current().as_mut() }.map(f)
}

/// A heap copy of `event` holding its own reference to the request.
fn copy(event: &AsyncEvent) -> *mut AsyncEvent {
    // SAFETY: the event's request record is null or live while the event is.
    if let Some(record) = unsafe { event.data.as_ref() } {
        swoole_rs_corebase::swr_v1_core_resolver_retain(record);
    }
    Box::into_raw(Box::new(event.clone()))
}

/// Releases the request reference of a copy from `copy`, whose box the caller drops.
fn free(event: &AsyncEvent) {
    // SAFETY: the copy's own reference.
    unsafe { swoole_rs_corebase::swr_v1_core_resolver_free(event.data) };
}

/// Hands `event` to the reactor thread that owns its pipe, `socket`, or frees it if
/// the write fails.
///
/// # Safety
/// `event` is a heap event nothing else uses, and `socket` its live pipe socket.
unsafe fn send_event(event: *mut AsyncEvent, socket: *mut c_void) {
    let token = event.expose_provenance();
    let socket = socket.cast::<SocketRecord>();
    // SAFETY: `token` is readable for its size; the socket is live.
    let sent = unsafe { socket_write_sync(socket, (&raw const token).cast(), size_of::<usize>()) };
    if sent <= 0 {
        log::sys_warning(
            c"ThreadPool::main_func",
            format_args!("sendto swoole_aio_pipe_write failed"),
        );
        // SAFETY: the event never reached the reactor.
        free(&*unsafe { Box::from_raw(event) });
    }
}

/// The pool's worker: runs the event's handler over a copy unless it has none or
/// was canceled, then sends the event back with the copy's result.
extern "C" fn execute(token: usize) {
    let event = ptr::with_exposed_provenance_mut::<AsyncEvent>(token);
    // SAFETY: the pool passes a dispatched event; until it is sent back the reactor
    // thread only sets `canceled`, atomically, and reads nothing the worker writes.
    let mut job = unsafe { &*event }.clone();
    // The event's pipe, which nothing writes once it is dispatched.
    let pipe = job.pipe_socket;
    match job.handler {
        None => (job.error, job.retval) = (AIO_BAD_REQUEST, -1),
        Some(_) if job.canceled.load(Relaxed) != 0 => (job.error, job.retval) = (AIO_CANCELED, -1),
        Some(handler) => handler(&mut job),
    }
    // SAFETY: as above.
    unsafe { (*event).error = job.error };
    // SAFETY: as above.
    unsafe { (*event).retval = job.retval };
    // SAFETY: the worker owns the event until it sends it, on its own pipe.
    unsafe { send_event(event, pipe) };
}

/// A retirement notice, back on the reactor thread: joins the idle worker.
extern "C" fn release_callback(event: &AsyncEvent) {
    // The notice's object is the worker's key.
    let key = event.object.addr();
    // The reactor thread's handle owns the pipe the notice came on.
    with_current(|threads| {
        if !threads.pool.is_some_and(|pool| pool.join(key)) {
            log::warning(
                c"ThreadPool::release_thread",
                format_args!("AIO thread#{key} is missing"),
            );
        }
        // Balances `complete`'s decrement: the notice was never dispatched.
        threads.task_num += 1;
    });
}

/// Asks the main thread to join idle worker `key`.
extern "C" fn retire(key: usize) {
    let pipe = swr_v1_core_aio_default_socket();
    let notice = AsyncEvent::new(
        ptr::without_provenance_mut(key),
        None,
        Some(release_callback),
        pipe,
    );
    let event = copy(&notice);
    // SAFETY: a fresh heap event on the default pipe.
    unsafe { send_event(event, pipe) };
}

extern "C" fn thread_init() {
    swoole_thread_init(false);
}

extern "C" fn thread_clean() {
    swoole_thread_clean(false);
}

extern "C" fn spawn_failed() {
    let error = std::io::Error::last_os_error();
    let code = error.raw_os_error().unwrap_or(0);
    let text = swoole_rs_corebase::sys::strerror(code);
    log::set_last_error(code);
    log::notice(format_args!(
        "ThreadPool::create_thread(): create aio thread failed, please check your system configuration or adjust aio_worker_num, Error: {text}[{code}]"
    ));
}

static HOOKS: PoolHooks = PoolHooks {
    thread_init,
    thread_clean,
    execute,
    retire,
    spawn_failed,
};

/// The reactor's destroy callback: unregisters the pipe and frees the handle.
extern "C-unwind" fn destroyed(_data: *mut c_void) {
    let threads = current();
    if threads.is_null() {
        return;
    }
    set_thread_async_threads(ptr::null_mut());
    // SAFETY: `create` boxed it; nothing refers to it any more.
    let mut threads = unsafe { Box::from_raw(threads) };
    // SAFETY: its read end, registered on the closing reactor.
    unsafe { swoole_event_del(threads.pipe.worker_socket) };
    // The last reference this thread holds.
    if let Some(pool) = threads.pool {
        pool.release();
    }
    close(&mut threads.pipe, 0);
    drop(&mut threads.pipe);
}

/// The reactor's exit condition: in-flight events keep it running.
extern "C-unwind" fn pending(event_num: &mut usize) -> bool {
    if with_current(|threads| threads.task_num) == Some(0) {
        *event_num -= 1;
    }
    true
}

/// `new AsyncThreads()`: the pipe on the thread's reactor and a pool reference.
/// Null, with the last error set, without a reactor or a pipe.
fn create() -> *mut AsyncThreads {
    let reactor = swr_v1_event_current();
    if reactor.is_null() {
        log::warning(
            c"AsyncThreads::AsyncThreads",
            format_args!("no event loop, cannot initialized"),
        );
        log::set_last_error(WRONG_OPERATION);
        return ptr::null_mut();
    }
    let mut threads = Box::new(AsyncThreads {
        task_num: 0,
        pipe: unix_socket(false, libc::SOCK_STREAM),
        pool: None,
    });
    let pipe = &threads.pipe;
    if pipe.master_socket.is_null() || pipe.worker_socket.is_null() {
        drop(&mut threads.pipe);
        log::set_last_error(SYSTEM_CALL_FAIL);
        return ptr::null_mut();
    }
    let (read, write) = (pipe.worker_socket, pipe.master_socket);
    for end in [read, write] {
        // SAFETY: both ends are live sockets.
        unsafe { (*end).fields.fd_type = FD_AIO };
    }
    // SAFETY: a live socket on the thread's reactor.
    unsafe { swoole_event_add(read, EVENT_READ) };
    on_destroy(destroyed);
    swoole_rs_reactor::swr_v1_event_set_exit_condition(EXIT_CONDITION_AIO_TASK, pending);
    threads.pool = Some(Pool::acquire(
        HOOKS,
        usize::from(swr_v1_core_cpu_num()),
        swr_v1_core_aio_core_worker_num() as usize,
        swr_v1_core_aio_worker_num() as usize,
        swr_v1_core_aio_max_wait_time(),
        swr_v1_core_aio_max_idle_time(),
    ));
    swr_v1_core_set_aio_default_socket(write.cast());
    let threads = Box::into_raw(threads);
    set_thread_async_threads(threads.cast());
    threads
}

/// The thread's pool, created first when it has none: null, with the last error
/// set, when that fails.
#[unsafe(no_mangle)]
#[must_use]
pub extern "C" fn swr_v1_async_threads_ensure() -> *mut AsyncThreads {
    let threads = current();
    if threads.is_null() { create() } else { threads }
}

/// `AsyncEvent *async::dispatch(const AsyncEvent *request)`: queues a copy of
/// `request` on the pool and returns it, or null without a reactor or a pipe.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole5async8dispatchEPKNS_10AsyncEventE")
)]
#[must_use]
pub extern "C" fn dispatch(request: &AsyncEvent) -> *mut AsyncEvent {
    if swr_v1_async_threads_ensure().is_null() {
        return ptr::null_mut();
    }
    let mut fields = request.clone();
    with_current(|threads| {
        fields.task_id = threads.pool.map_or(0, Pool::task_id);
        fields.timestamp = microtime();
        fields.pipe_socket = threads.pipe.master_socket.cast();
        let event = copy(&fields);
        // The event stays live until a worker sends it back.
        if let Some(pool) = threads.pool {
            pool.dispatch(event.expose_provenance());
        }
        threads.task_num += 1;
        event
    })
    .unwrap_or(ptr::null_mut())
}

/// A completed event on its reactor thread: runs its callback unless canceled.
fn complete(token: usize) {
    let event = ptr::with_exposed_provenance_mut::<AsyncEvent>(token);
    // SAFETY: the worker sent the event back and no longer touches it.
    let event_ref = unsafe { &*event };
    if event_ref.canceled.load(Relaxed) == 0
        && let Some(callback) = event_ref.callback
    {
        callback(event_ref);
    }
    // The callback may not free the handle that owns its pipe.
    with_current(|threads| threads.task_num = threads.task_num.wrapping_sub(1));
    // SAFETY: done with; nothing else holds it.
    free(&*unsafe { Box::from_raw(event) });
}

/// Upstream's `AsyncThreads::callback`, the pipe's reactor handler: completes every event
/// waiting on the pipe.
pub extern "C" fn callback(_: Running<'_>, event: &mut Event) -> c_int {
    if pool_complete(event.fd(), complete) < 0 {
        log::sys_warning(
            c"AsyncThreads::callback",
            format_args!("read() aio events failed"),
        );
        return -1;
    }
    0
}

/// `Co::stats()`'s AIO counts for this thread: events in flight, the pool's
/// workers and its queue; left as they are when the thread has no pool.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_async_threads_stats(
    task_num: &mut i64,
    worker_num: &mut i64,
    queue_size: &mut i64,
) {
    with_current(|threads| {
        *task_num = i64::try_from(threads.task_num).unwrap_or(i64::MAX);
        let (workers, queued) = threads
            .pool
            .map_or((0, 0), |pool| (pool.worker_num(), pool.queue_size()));
        *worker_num = i64::try_from(workers).unwrap_or(i64::MAX);
        *queue_size = i64::try_from(queued).unwrap_or(i64::MAX);
    });
}
