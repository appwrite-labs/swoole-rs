// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/async_thread.cc (Swoole contributors).
//! `AsyncThreads`, a reactor thread's handle on crates/ossupport's shared pool, and
//! `async::dispatch`, under their upstream (Itanium) symbols. The pool runs an event
//! on a worker and writes its address to the completion pipe; the reactor reads it
//! back and runs its callback. Each pool copy of an event holds a reference to its
//! request record. The ossupport patch deletes `src/os/async_thread.cc`.
use crate::offload::AsyncEvent;
use std::cell::Cell;
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
use swoole_rs_reactor::event_api::{event_del, on_destroy, swr_v1_event_current};
use swoole_rs_reactor::{Event, Running, Socket};
use swoole_rs_tls::socket_pair::{SocketPair, close, drop, unix_socket};
use swoole_rs_tls::upstream::write_sync;

/// `SW_FD_AIO`.
const FD_AIO: c_int = 5;
/// `SW_ERROR_SYSTEM_CALL_FAIL`, `SW_ERROR_WRONG_OPERATION`, `SW_ERROR_AIO_BAD_REQUEST`
/// and `SW_ERROR_AIO_CANCELED`.
const SYSTEM_CALL_FAIL: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SYSTEM_CALL_FAIL as c_int;
const WRONG_OPERATION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int;
const AIO_BAD_REQUEST: c_int = swoole_rs_error::ErrorCode::SW_ERROR_AIO_BAD_REQUEST as c_int;
const AIO_CANCELED: c_int = swoole_rs_error::ErrorCode::SW_ERROR_AIO_CANCELED as c_int;

/// `swoole::AsyncThreads`, opaque to native code: the events in flight, the
/// completion pipe (the master end writes, the worker end reads) and the pool.
pub struct AsyncThreads {
    task_num: usize,
    pipe: SocketPair,
    pool: Option<&'static Pool>,
}

thread_local! {
    /// The thread's handle; `SwooleTG.async_threads` holds its address while it is here.
    static THREADS: Cell<Option<Box<AsyncThreads>>> = const { Cell::new(None) };
}

/// `f` over the current reactor's handle, including a thread-context coroutine
/// that adopted its pointer. Those threads run serially; no `f` switches coroutine
/// or runs a callback that can free the handle.
fn with_current<R>(f: impl FnOnce(&mut AsyncThreads) -> R) -> Option<R> {
    let current = thread_async_threads().cast::<AsyncThreads>();
    // SAFETY: the reactor owner keeps the box live, including while a serialized
    // thread-context coroutine adopts it. No other borrow reaches it during `f`.
    unsafe { current.as_mut() }.map(f)
}

/// A heap copy of an event, holding its own reference to the request, which only
/// [`HeapEvent::copy`] makes, and whose `pipe_socket` is the live write end of the
/// pipe the event goes back on: the dispatching thread's, or the default one. Whoever
/// holds this owns the copy. `dispatch` hands its address to the pool and to its
/// caller, which only sets `canceled` (atomically) until the worker sends it back, so
/// the copy is reached through its address, never a `Box`, until it is freed.
struct HeapEvent(*mut AsyncEvent);

impl HeapEvent {
    /// A copy of `event` with its own reference to the request.
    fn copy(event: &AsyncEvent) -> Self {
        if let Some(record) = event.record() {
            swoole_rs_corebase::swr_v1_core_resolver_retain(record);
        }
        Self(Box::into_raw(Box::new(event.clone())))
    }

    /// The copy whose address the pool or the pipe hands back as `token`: the worker,
    /// or the reactor thread it came back to, owns it from here.
    fn of_token(token: usize) -> Self {
        Self(ptr::with_exposed_provenance_mut(token))
    }

    /// The copy's address, for the pool or the pipe to hand to its next owner.
    fn into_raw(self) -> *mut AsyncEvent {
        core::mem::ManuallyDrop::new(self).0
    }

    /// The copy, shared. The one other party that reaches it, the dispatching
    /// coroutine, only sets `canceled` (an atomic) until its completion, and the
    /// worker writes its result only once it has finished reading.
    fn event(&self) -> &AsyncEvent {
        // SAFETY: the type's invariant: the live copy, written through no other
        // pointer but its atomic `canceled` while its owner borrows it.
        unsafe { &*self.0 }
    }

    /// Hands the copy to the reactor thread that owns its pipe, or frees it if the
    /// write fails.
    fn send(self) {
        let pipe = self.event().pipe_socket;
        // SAFETY: the type's invariant: the live write end of the event's pipe.
        let socket = unsafe { Socket::from_ptr(pipe.cast()) };
        let token = self.0.expose_provenance().to_ne_bytes();
        if socket.and_then(|socket| write_sync(socket, &token)) > Some(0) {
            // The reactor thread owns it now.
            _ = self.into_raw();
            return;
        }
        log::sys_warning(
            c"ThreadPool::main_func",
            format_args!("sendto swoole_aio_pipe_write failed"),
        );
    }
}

impl Drop for HeapEvent {
    fn drop(&mut self) {
        // SAFETY: the copy this owns, which nothing reaches any more.
        let event = unsafe { Box::from_raw(self.0) };
        // SAFETY: the copy's own reference.
        unsafe { swoole_rs_corebase::swr_v1_core_resolver_free(event.data) };
    }
}

/// The pool's worker: runs the event's handler over a copy unless it has none or
/// was canceled, then sends the event back with the copy's result.
extern "C" fn execute(token: usize) {
    let event = HeapEvent::of_token(token);
    // Until it is sent back the reactor thread only sets `canceled`, atomically, and
    // reads nothing the worker writes.
    let mut job = event.event().clone();
    match job.handler {
        None => job.set_result(-1, AIO_BAD_REQUEST),
        Some(_) if job.canceled.load(Relaxed) != 0 => job.set_result(-1, AIO_CANCELED),
        Some(handler) => handler(&mut job),
    }
    event.event().publish(job.result());
    event.send();
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
    HeapEvent::copy(&notice).send();
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
    let Some(mut threads) = THREADS.take() else {
        return;
    };
    set_thread_async_threads(ptr::null_mut());
    // Its read end, registered on the closing reactor.
    if let Some(read) = threads.pipe.worker_end() {
        _ = event_del(read);
    }
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
    let (Some(read), Some(write)) = (threads.pipe.worker_end(), threads.pipe.master_end()) else {
        drop(&mut threads.pipe);
        log::set_last_error(SYSTEM_CALL_FAIL);
        return ptr::null_mut();
    };
    for end in [read, write] {
        end.edit(|end| end.fd_type = FD_AIO);
    }
    // The pipe owns its read end, which stays registered until `destroyed` frees the
    // pipe with the reactor.
    if let Some(read) = threads.pipe.worker_event() {
        _ = read.add(swoole_rs_reactor::Events::READ);
    }
    on_destroy(destroyed);
    swoole_rs_reactor::swr_v1_event_set_exit_condition(
        swoole_rs_reactor::ExitCondition::AioTask,
        pending,
    );
    threads.pool = Some(Pool::acquire(
        HOOKS,
        usize::from(swr_v1_core_cpu_num()),
        swr_v1_core_aio_core_worker_num() as usize,
        swr_v1_core_aio_worker_num() as usize,
        swr_v1_core_aio_max_wait_time(),
        swr_v1_core_aio_max_idle_time(),
    ));
    swr_v1_core_set_aio_default_socket(write.as_ptr().cast());
    let address = ptr::from_mut(&mut *threads);
    set_thread_async_threads(address.cast());
    THREADS.set(Some(threads));
    address
}

/// The thread's pool, created first when it has none: null, with the last error
/// set, when that fails.
#[unsafe(no_mangle)]
#[must_use]
pub extern "C" fn swr_v1_async_threads_ensure() -> *mut AsyncThreads {
    let threads = thread_async_threads().cast::<AsyncThreads>();
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
        fields.pipe_socket = threads.pipe.raw_ends().0.cast();
        // The event stays live until a worker sends it back.
        let event = HeapEvent::copy(&fields).into_raw();
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
    let event = HeapEvent::of_token(token);
    // The worker sent the event back and no longer touches it.
    let event_ref = event.event();
    if event_ref.canceled.load(Relaxed) == 0
        && let Some(callback) = event_ref.callback
    {
        callback(event_ref);
    }
    // The callback may not free the handle that owns its pipe.
    with_current(|threads| threads.task_num = threads.task_num.wrapping_sub(1));
}

/// Upstream's `AsyncThreads::callback`, the pipe's reactor handler: completes every event
/// waiting on the pipe.
pub fn callback(_: Running<'_>, event: &mut Event) -> c_int {
    if pool_complete(event.fd(), complete).is_none() {
        log::sys_warning(
            c"AsyncThreads::callback",
            format_args!("read() aio events failed"),
        );
        return -1;
    }
    0
}

/// `Co::stats()`'s AIO counts for this thread: events in flight, the pool's
/// workers and its queue; none when the thread has no async threads.
#[must_use]
pub fn stats() -> Option<(usize, usize, usize)> {
    with_current(|threads| {
        let (workers, queued) = threads
            .pool
            .map_or((0, 0), |pool| (pool.worker_num(), pool.queue_size()));
        (threads.task_num, workers, queued)
    })
}
