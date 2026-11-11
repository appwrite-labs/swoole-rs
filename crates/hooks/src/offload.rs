//! `coroutine::async`: hand a job to the `AsyncThreads` pool and park the current
//! coroutine until the pool's completion resumes it. Native code passes the pool's
//! `async::dispatch` and the coroutine operations in `AsyncOps`.
use core::ffi::{c_int, c_void};
use core::mem::{offset_of, size_of};
use swoole_rs_corebase::set_errno;

/// `async::Handler`: runs on a pool thread.
pub type Handler = unsafe extern "C" fn(*mut AsyncEvent);

/// Upstream's `AsyncEvent`, with its `shared_ptr` request replaced by a counted
/// DNS record; native code asserts the same layout.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsyncEvent {
    pub task_id: usize,
    pub canceled: u8,
    pub error: c_int,
    /// The request's resolver record: one reference per heap copy, or null.
    pub(crate) data: *mut swoole_rs_corebase::ResolverRequest,
    pub retval: isize,
    pub pipe_socket: *mut c_void,
    pub timestamp: f64,
    pub object: *mut c_void,
    pub handler: Option<Handler>,
    pub callback: Option<Handler>,
}

impl AsyncEvent {
    /// An event with no request.
    pub(crate) const fn request(
        object: *mut c_void,
        handler: Option<Handler>,
        callback: Option<Handler>,
        pipe_socket: *mut c_void,
    ) -> Self {
        Self {
            task_id: 0,
            canceled: 0,
            error: 0,
            data: core::ptr::null_mut(),
            retval: 0,
            pipe_socket,
            timestamp: 0.0,
            object,
            handler,
            callback,
        }
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<AsyncEvent>() == 72
        && offset_of!(AsyncEvent, error) == 12
        && offset_of!(AsyncEvent, retval) == 24
        && offset_of!(AsyncEvent, object) == 48
        && offset_of!(AsyncEvent, callback) == 64,
    "swoole::AsyncEvent's layout moved"
);

/// The native operations an offloaded job needs.
#[repr(C)]
pub struct AsyncOps {
    /// `Coroutine::get_current_safe()`.
    pub current: extern "C" fn() -> *mut c_void,
    /// `async::dispatch`: a pool-owned copy of the event, or null if refused.
    pub dispatch: unsafe extern "C" fn(*const AsyncEvent) -> *mut AsyncEvent,
    /// `co->yield()` of the current coroutine.
    pub park: fn(),
    /// `co->yield_ex(timeout)` of the current coroutine: false on timeout or
    /// cancellation.
    pub park_for: fn(f64) -> bool,
    /// `co->resume()`.
    pub resume: unsafe extern "C" fn(*mut c_void),
    /// `swoole_get_last_error()`.
    pub last_error: extern "C" fn() -> c_int,
}

/// The waiting coroutine, on its stack; the event's `object` while the job runs.
struct Task<'a> {
    co: *mut c_void,
    ops: &'a AsyncOps,
    job: Option<&'a mut (dyn FnMut() + Send)>,
}

/// The pool thread's handler for a job: runs it, then records its errno.
unsafe extern "C" fn run_job(event: *mut AsyncEvent) {
    // SAFETY: the pool passes the dispatched copy; the reactor thread reads it only
    // after completion.
    let event = unsafe { &mut *event };
    // SAFETY: its object is the parked task, untouched by its coroutine until completion.
    let task = unsafe { &mut *event.object.cast::<Task<'_>>() };
    if let Some(job) = &mut task.job {
        job();
    }
    event.error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    event.retval = 0;
}

/// The reactor thread's completion: resumes the parked coroutine unless it gave up.
unsafe extern "C" fn completed(event: *mut AsyncEvent) {
    // SAFETY: the pool passes the completed copy, still live.
    let event = unsafe { &*event };
    if event.canceled != 0 {
        return;
    }
    // SAFETY: not canceled, so the task's coroutine is still parked on it.
    let task = unsafe { &*event.object.cast::<Task<'_>>() };
    // SAFETY: `co` is the parked coroutine.
    unsafe { (task.ops.resume)(task.co) };
}

/// `coroutine::async(handler, event, timeout)`: runs `handler(event)` on the pool and
/// waits up to `timeout`. True with the pool's result in `event`; false if refused, or
/// on timeout or cancellation (`event` canceled, -1, and the last error as errno).
///
/// # Safety
/// `event` and `ops` are valid; `handler` is a pool handler for `event`; inside a coroutine.
pub unsafe fn swr_v1_hook_async(
    event: &mut AsyncEvent,
    handler: Handler,
    timeout: f64,
    ops: &AsyncOps,
) -> bool {
    let co = (ops.current)();
    let mut task = Task { co, ops, job: None };
    event.object = (&raw mut task).cast();
    event.handler = Some(handler);
    event.callback = Some(completed);
    // SAFETY: `event` is complete; the pool copies it.
    let dispatched = unsafe { (ops.dispatch)(event) };
    if dispatched.is_null() {
        return false;
    }
    // Parks this coroutine until `completed`, a timeout or a cancel.
    if (ops.park_for)(timeout) {
        // SAFETY: the pool's copy lives until `completed` returns, which resumed us.
        let done = unsafe { &*dispatched };
        event.canceled = done.canceled;
        event.error = done.error;
        event.retval = done.retval;
        set_errno(done.error);
        true
    } else {
        // SAFETY: the pool's copy is pending; canceling it keeps `completed` off `task`.
        unsafe { (*dispatched).canceled = 1 };
        event.canceled = 1;
        event.retval = -1;
        event.error = (ops.last_error)();
        set_errno(event.error);
        false
    }
}

/// `coroutine::async_job`: [`swr_v1_hook_async_job`] over the coroutine's own
/// operations; the hooked functions' and `System::read_file`'s offload. Outside a
/// coroutine it raises upstream's fatal error.
pub fn async_job(job: &mut (dyn FnMut() + Send)) -> bool {
    // SAFETY: the coroutine's operations, whose `current` is fatal outside a coroutine.
    unsafe { swr_v1_hook_async_job(job, &crate::resolve::COROUTINE_ASYNC) }
}

/// `coroutine::async(fn)`: runs `job` on the pool and waits for it, then sets errno
/// to the job's. False if the pool refused it.
///
/// # Safety
/// `ops` is valid; inside a coroutine.
pub unsafe fn swr_v1_hook_async_job(job: &mut (dyn FnMut() + Send), ops: &AsyncOps) -> bool {
    let co = (ops.current)();
    let mut task = Task {
        co,
        ops,
        job: Some(job),
    };
    let event = AsyncEvent::request(
        (&raw mut task).cast(),
        Some(run_job),
        Some(completed),
        core::ptr::null_mut(),
    );
    // SAFETY: `event` is complete (an empty request); the pool copies it.
    let dispatched = unsafe { (ops.dispatch)(&raw const event) };
    if dispatched.is_null() {
        return false;
    }
    // Parks this coroutine until `completed` resumes it.
    (ops.park)();
    // SAFETY: the pool's copy lives until `completed` returns, which resumed us.
    set_errno(unsafe { (*dispatched).error });
    true
}

#[cfg(test)]
mod ported;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    thread_local! {
        static PENDING: RefCell<Option<Box<AsyncEvent>>> = const { RefCell::new(None) };
        static REFUSE: Cell<bool> = const { Cell::new(false) };
        static TIME_OUT: Cell<bool> = const { Cell::new(false) };
        static RESUMED: Cell<u32> = const { Cell::new(0) };
    }

    extern "C" fn current() -> *mut c_void {
        core::ptr::dangling_mut()
    }
    unsafe extern "C" fn dispatch(event: *const AsyncEvent) -> *mut AsyncEvent {
        if REFUSE.get() {
            return core::ptr::null_mut();
        }
        let copy = Box::new(unsafe { core::ptr::read(event) });
        PENDING.with_borrow_mut(|slot| &raw mut **slot.insert(copy))
    }
    // Runs the pool's half (handler, then completion) as the reactor would, unless timed out.
    fn park() {
        let raw = PENDING.with_borrow_mut(|slot| &raw mut **slot.as_mut().unwrap());
        unsafe {
            ((*raw).handler.unwrap())(raw);
            if (*raw).canceled == 0 {
                ((*raw).callback.unwrap())(raw);
            }
        }
    }
    fn park_for(_: f64) -> bool {
        if TIME_OUT.get() {
            return false;
        }
        park();
        true
    }
    unsafe extern "C" fn resume(_: *mut c_void) {
        RESUMED.set(RESUMED.get() + 1);
    }
    extern "C" fn last_error() -> c_int {
        9001
    }
    const OPS: AsyncOps = AsyncOps {
        current,
        dispatch,
        park,
        park_for,
        resume,
        last_error,
    };

    unsafe extern "C" fn handler(event: *mut AsyncEvent) {
        unsafe {
            (*event).retval = 7;
            (*event).error = 3;
        }
    }

    fn empty() -> AsyncEvent {
        AsyncEvent {
            task_id: 0,
            canceled: 0,
            error: 0,
            data: core::ptr::null_mut(),
            retval: 1,
            pipe_socket: core::ptr::null_mut(),
            timestamp: 0.0,
            object: core::ptr::null_mut(),
            handler: None,
            callback: None,
        }
    }

    fn errno() -> c_int {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    #[test]
    fn job_runs_on_the_pool_and_publishes_its_errno() {
        let mut runs = 0u32;
        let mut job = || {
            runs += 1;
            set_errno(libc::ENOENT);
        };
        RESUMED.set(0);
        assert!(unsafe { swr_v1_hook_async_job(&mut job, &OPS) });
        assert_eq!((RESUMED.get(), errno()), (1, libc::ENOENT));
        REFUSE.set(true);
        assert!(!unsafe { swr_v1_hook_async_job(&mut job, &OPS) });
        REFUSE.set(false);
        assert_eq!(runs, 1);
    }

    #[test]
    fn handler_result_or_timeout_reaches_the_event() {
        RESUMED.set(0);
        let mut event = empty();
        assert!(unsafe { swr_v1_hook_async(&mut event, handler, 1.0, &OPS) });
        assert_eq!(
            (
                event.retval,
                event.error,
                event.canceled,
                errno(),
                RESUMED.get()
            ),
            (7, 3, 0, 3, 1)
        );

        TIME_OUT.set(true);
        let mut event = empty();
        assert!(!unsafe { swr_v1_hook_async(&mut event, handler, 1.0, &OPS) });
        TIME_OUT.set(false);
        assert_eq!(
            (event.retval, event.error, event.canceled, errno()),
            (-1, 9001, 1, 9001)
        );
        // The pool finishes later; the completion skips the gone coroutine.
        park();
        assert_eq!(RESUMED.get(), 1);
    }
}
