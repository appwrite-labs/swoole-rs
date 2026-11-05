//! `coroutine::async`: hand a job to the `AsyncThreads` pool and park the current
//! coroutine until the pool's completion resumes it. Native code passes the pool's
//! `async::dispatch` and the coroutine operations in `AsyncOps`.
use core::ffi::{c_int, c_void};
use core::mem::{offset_of, size_of};

/// `async::Handler`: runs on a pool thread.
pub type Handler = unsafe extern "C" fn(*mut AsyncEvent);

/// Upstream's `AsyncEvent`; native code asserts the same layout.
#[repr(C)]
pub struct AsyncEvent {
    pub task_id: usize,
    pub canceled: u8,
    pub error: c_int,
    /// `std::shared_ptr<AsyncRequest>`, opaque here.
    data: [usize; 2],
    pub retval: isize,
    pipe_socket: *mut c_void,
    timestamp: f64,
    pub object: *mut c_void,
    pub handler: Option<Handler>,
    pub callback: Option<Handler>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<AsyncEvent>() == 80
        && offset_of!(AsyncEvent, error) == 12
        && offset_of!(AsyncEvent, retval) == 32
        && offset_of!(AsyncEvent, object) == 56
        && offset_of!(AsyncEvent, callback) == 72,
    "swoole::AsyncEvent's layout moved"
);

/// The native operations an offloaded job needs.
#[repr(C)]
pub struct AsyncOps {
    /// `Coroutine::get_current_safe()`.
    pub current: unsafe extern "C" fn() -> *mut c_void,
    /// `async::dispatch`: a pool-owned copy of the event, or null if refused.
    pub dispatch: unsafe extern "C" fn(*const AsyncEvent) -> *mut AsyncEvent,
    /// `co->yield()`.
    pub park: unsafe extern "C" fn(*mut c_void),
    /// `co->yield_ex(timeout)`: false on timeout or cancellation.
    pub park_for: unsafe extern "C" fn(*mut c_void, f64) -> bool,
    /// `co->resume()`.
    pub resume: unsafe extern "C" fn(*mut c_void),
    /// `swoole_get_last_error()`.
    pub last_error: unsafe extern "C" fn() -> c_int,
}

/// The waiting coroutine, on its stack; the event's `object` while the job runs.
struct Task {
    co: *mut c_void,
    resume: unsafe extern "C" fn(*mut c_void),
    job: Option<(unsafe extern "C" fn(*mut c_void), *mut c_void)>,
}

fn set_errno(value: c_int) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

/// The pool thread's handler for a job: runs it, then records its errno.
unsafe extern "C" fn run_job(event: *mut AsyncEvent) {
    // SAFETY: the pool passes the dispatched copy; the reactor thread reads it only
    // after completion.
    let event = unsafe { &mut *event };
    // SAFETY: its object is the parked task.
    let task = unsafe { &*event.object.cast::<Task>() };
    if let Some((job, context)) = task.job {
        // SAFETY: the caller of `swr_v1_hook_async_job` vouched for `job(context)`.
        unsafe { job(context) };
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
    let task = unsafe { &*event.object.cast::<Task>() };
    // SAFETY: `co` is the parked coroutine.
    unsafe { (task.resume)(task.co) };
}

/// `coroutine::async(handler, event, timeout)`: runs `handler(event)` on the pool and
/// waits up to `timeout`. True with the pool's result in `event`; false if refused, or
/// on timeout or cancellation (`event` canceled, -1, and the last error as errno).
///
/// # Safety
/// `event` and `ops` are valid; `handler` is a pool handler for `event`; inside a coroutine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_async(
    event: *mut AsyncEvent,
    handler: Handler,
    timeout: f64,
    ops: *const AsyncOps,
) -> bool {
    // SAFETY: the caller passes a valid ops table.
    let ops = unsafe { &*ops };
    // SAFETY: inside a coroutine (else native fails fatally).
    let co = unsafe { (ops.current)() };
    let mut task = Task {
        co,
        resume: ops.resume,
        job: None,
    };
    // SAFETY: the caller's event, unaliased during the call.
    let event = unsafe { &mut *event };
    event.object = (&raw mut task).cast();
    event.handler = Some(handler);
    event.callback = Some(completed);
    // SAFETY: `event` is complete; the pool copies it.
    let dispatched = unsafe { (ops.dispatch)(event) };
    if dispatched.is_null() {
        return false;
    }
    // SAFETY: parks this coroutine until `completed`, a timeout or a cancel.
    if unsafe { (ops.park_for)(co, timeout) } {
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
        // SAFETY: plain native read.
        event.error = unsafe { (ops.last_error)() };
        set_errno(event.error);
        false
    }
}

/// `coroutine::async(fn)`: runs `job(context)` on the pool and waits for it, then sets
/// errno to the job's. False if the pool refused it.
///
/// # Safety
/// `ops` is valid; `job(context)` is safe to run on another thread; inside a coroutine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_async_job(
    job: unsafe extern "C" fn(*mut c_void),
    context: *mut c_void,
    ops: *const AsyncOps,
) -> bool {
    // SAFETY: the caller passes a valid ops table.
    let ops = unsafe { &*ops };
    // SAFETY: inside a coroutine (else native fails fatally).
    let co = unsafe { (ops.current)() };
    let mut task = Task {
        co,
        resume: ops.resume,
        job: Some((job, context)),
    };
    let event = AsyncEvent {
        task_id: 0,
        canceled: 0,
        error: 0,
        data: [0; 2],
        retval: 0,
        pipe_socket: core::ptr::null_mut(),
        timestamp: 0.0,
        object: (&raw mut task).cast(),
        handler: Some(run_job),
        callback: Some(completed),
    };
    // SAFETY: `event` is complete (an empty request); the pool copies it.
    let dispatched = unsafe { (ops.dispatch)(&raw const event) };
    if dispatched.is_null() {
        return false;
    }
    // SAFETY: parks this coroutine until `completed` resumes it.
    unsafe { (ops.park)(co) };
    // SAFETY: the pool's copy lives until `completed` returns, which resumed us.
    set_errno(unsafe { (*dispatched).error });
    true
}

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

    unsafe extern "C" fn current() -> *mut c_void {
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
    unsafe extern "C" fn park(_: *mut c_void) {
        let raw = PENDING.with_borrow_mut(|slot| &raw mut **slot.as_mut().unwrap());
        unsafe {
            ((*raw).handler.unwrap())(raw);
            if (*raw).canceled == 0 {
                ((*raw).callback.unwrap())(raw);
            }
        }
    }
    unsafe extern "C" fn park_for(co: *mut c_void, _: f64) -> bool {
        if TIME_OUT.get() {
            return false;
        }
        unsafe { park(co) };
        true
    }
    unsafe extern "C" fn resume(_: *mut c_void) {
        RESUMED.set(RESUMED.get() + 1);
    }
    unsafe extern "C" fn last_error() -> c_int {
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

    unsafe extern "C" fn job(context: *mut c_void) {
        unsafe { *context.cast::<u32>() += 1 };
        set_errno(libc::ENOENT);
    }
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
            data: [0; 2],
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
        RESUMED.set(0);
        assert!(unsafe { swr_v1_hook_async_job(job, (&raw mut runs).cast(), &OPS) });
        assert_eq!((runs, RESUMED.get(), errno()), (1, 1, libc::ENOENT));
        REFUSE.set(true);
        assert!(!unsafe { swr_v1_hook_async_job(job, (&raw mut runs).cast(), &OPS) });
        REFUSE.set(false);
        assert_eq!(runs, 1);
    }

    #[test]
    fn handler_result_or_timeout_reaches_the_event() {
        RESUMED.set(0);
        let mut event = empty();
        assert!(unsafe { swr_v1_hook_async(&raw mut event, handler, 1.0, &OPS) });
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
        assert!(!unsafe { swr_v1_hook_async(&raw mut event, handler, 1.0, &OPS) });
        TIME_OUT.set(false);
        assert_eq!(
            (event.retval, event.error, event.canceled, errno()),
            (-1, 9001, 1, 9001)
        );
        // The pool finishes later; the completion skips the gone coroutine.
        unsafe { park(core::ptr::null_mut()) };
        assert_eq!(RESUMED.get(), 1);
    }
}
