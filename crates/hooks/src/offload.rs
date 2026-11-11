//! `coroutine::async`: hand a job to the `AsyncThreads` pool and park the current
//! coroutine until the pool's completion resumes it. [`AsyncOps`] holds the pool's
//! `async::dispatch` and the coroutine operations.
use core::ffi::{c_int, c_void};
use core::mem::{offset_of, size_of};
use core::sync::atomic::{AtomicU8, Ordering::Relaxed};
use swoole_rs_corebase::set_errno;

/// `async::Handler`: runs on a pool thread over its own copy of the event, of which
/// the pool keeps `error` and `retval`.
pub type Handler = extern "C" fn(&mut AsyncEvent);

/// The completion: runs on the dispatching thread over the completed event.
pub type Callback = extern "C" fn(&AsyncEvent);

/// Upstream's `AsyncEvent`, with its `shared_ptr` request replaced by a counted
/// DNS record; native code asserts the same layout.
#[repr(C)]
pub struct AsyncEvent {
    pub task_id: usize,
    /// The dispatching thread may set it while a pool thread reads it.
    pub canceled: AtomicU8,
    pub error: c_int,
    /// The request's resolver record: one reference per heap copy, or null.
    pub(crate) data: *mut swoole_rs_corebase::ResolverRequest,
    pub retval: isize,
    pub pipe_socket: *mut c_void,
    pub timestamp: f64,
    /// What the handler and completion read; paired with them at construction.
    pub(crate) object: *mut c_void,
    pub(crate) handler: Option<Handler>,
    pub(crate) callback: Option<Callback>,
}

impl AsyncEvent {
    /// `AsyncEvent event = {}`: no request, handler or completion.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new(core::ptr::null_mut(), None, None, core::ptr::null_mut())
    }

    /// `AsyncEvent event = {}` with these members set: no request.
    ///
    /// # Safety
    /// `handler` and `callback` are sound over `object`: whatever they read through
    /// it stays valid, as they expect it, until the completion has run.
    #[must_use]
    pub const unsafe fn request(
        object: *mut c_void,
        handler: Option<Handler>,
        callback: Option<Callback>,
        pipe_socket: *mut c_void,
    ) -> Self {
        Self::new(object, handler, callback, pipe_socket)
    }

    /// [`Self::request`] for this crate, whose callers pair each handler and
    /// completion with the object they read.
    pub(crate) const fn new(
        object: *mut c_void,
        handler: Option<Handler>,
        callback: Option<Callback>,
        pipe_socket: *mut c_void,
    ) -> Self {
        Self {
            task_id: 0,
            canceled: AtomicU8::new(0),
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

    /// The object the handler and completion read.
    #[must_use]
    pub const fn object(&self) -> *mut c_void {
        self.object
    }
}

impl Clone for AsyncEvent {
    fn clone(&self) -> Self {
        Self {
            canceled: AtomicU8::new(self.canceled.load(Relaxed)),
            ..*self
        }
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<AsyncEvent>() == 72
        && offset_of!(AsyncEvent, canceled) == 8
        && offset_of!(AsyncEvent, error) == 12
        && offset_of!(AsyncEvent, retval) == 24
        && offset_of!(AsyncEvent, object) == 48
        && offset_of!(AsyncEvent, callback) == 64,
    "swoole::AsyncEvent's layout moved"
);

/// The native operations an offloaded job needs. Only this crate builds one, so the
/// offload trusts what they return.
pub struct AsyncOps {
    /// `Coroutine::get_current_safe()`: fatal outside a coroutine.
    pub(crate) current: extern "C" fn() -> *mut c_void,
    /// `async::dispatch`: a pool-owned copy of the event, live until its completion
    /// returns, or null if refused.
    pub(crate) dispatch: extern "C" fn(&AsyncEvent) -> *mut AsyncEvent,
    /// `co->yield()` of the current coroutine.
    pub(crate) park: fn(),
    /// `co->yield_ex(timeout)` of the current coroutine: false on timeout or
    /// cancellation.
    pub(crate) park_for: fn(f64) -> bool,
    /// `co->resume()`.
    pub(crate) resume: unsafe extern "C" fn(*mut c_void),
    /// `swoole_get_last_error()`.
    pub(crate) last_error: extern "C" fn() -> c_int,
}

/// The waiting coroutine, on its stack; the event's `object` while the job runs.
struct Task<'a> {
    co: *mut c_void,
    ops: &'a AsyncOps,
    job: Option<&'a mut (dyn FnMut() + Send)>,
}

/// The pool thread's handler for a job: runs it, then records its errno.
extern "C" fn run_job(event: &mut AsyncEvent) {
    // SAFETY: only `swr_v1_hook_async_job` dispatches this handler, its parked task
    // the object, which the coroutine leaves alone until the completion.
    let task = unsafe { &mut *event.object.cast::<Task<'_>>() };
    if let Some(job) = &mut task.job {
        job();
    }
    event.error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    event.retval = 0;
}

/// The reactor thread's completion: resumes the parked coroutine unless it gave up.
extern "C" fn completed(event: &AsyncEvent) {
    if event.canceled.load(Relaxed) != 0 {
        return;
    }
    // SAFETY: only this module dispatches this completion, its task the object; not
    // canceled, so the task's coroutine is still parked on it.
    let task = unsafe { &*event.object.cast::<Task<'_>>() };
    // SAFETY: `co` is the parked coroutine.
    unsafe { (task.ops.resume)(task.co) };
}

/// `coroutine::async(handler, event, timeout)`, inside a coroutine (`ops.current` is
/// fatal outside one): runs `handler` on the pool and waits up to `timeout`. True with
/// the pool's result in `event`; false if refused, or on timeout or cancellation
/// (`event` canceled, -1, and the last error as errno).
///
/// # Safety
/// `handler` doesn't read `event`'s object, which is the waiting task while it runs.
pub unsafe fn swr_v1_hook_async(
    event: &mut AsyncEvent,
    handler: Handler,
    timeout: f64,
    ops: &AsyncOps,
) -> bool {
    hook_async(event, handler, timeout, ops)
}

/// [`swr_v1_hook_async`] for this crate, whose handlers read the event's request,
/// never its object.
pub(crate) fn hook_async(
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
    let dispatched = (ops.dispatch)(event);
    if dispatched.is_null() {
        return false;
    }
    // Parks this coroutine until `completed`, a timeout or a cancel.
    if (ops.park_for)(timeout) {
        // SAFETY: the pool's copy lives until `completed` returns, which resumed us.
        let done = unsafe { &*dispatched };
        *event.canceled.get_mut() = done.canceled.load(Relaxed);
        event.error = done.error;
        event.retval = done.retval;
        set_errno(done.error);
        true
    } else {
        // SAFETY: the pool's copy is pending, live until its completion on this
        // thread; canceling it keeps `completed` off `task`.
        unsafe { (*dispatched).canceled.store(1, Relaxed) };
        *event.canceled.get_mut() = 1;
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
    swr_v1_hook_async_job(job, &crate::resolve::COROUTINE_ASYNC)
}

/// `coroutine::async(fn)`, inside a coroutine: runs `job` on the pool and waits for
/// it, then sets errno to the job's. False if the pool refused it.
pub fn swr_v1_hook_async_job(job: &mut (dyn FnMut() + Send), ops: &AsyncOps) -> bool {
    let co = (ops.current)();
    let mut task = Task {
        co,
        ops,
        job: Some(job),
    };
    let event = AsyncEvent::new(
        (&raw mut task).cast(),
        Some(run_job),
        Some(completed),
        core::ptr::null_mut(),
    );
    let dispatched = (ops.dispatch)(&event);
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
    extern "C" fn dispatch(event: &AsyncEvent) -> *mut AsyncEvent {
        if REFUSE.get() {
            return core::ptr::null_mut();
        }
        let copy = Box::new(event.clone());
        PENDING.with_borrow_mut(|slot| &raw mut **slot.insert(copy))
    }
    // Runs the pool's half (handler, then completion) as the reactor would, unless timed out.
    fn park() {
        let raw = PENDING.with_borrow_mut(|slot| &raw mut **slot.as_mut().unwrap());
        unsafe {
            ((*raw).handler.unwrap())(&mut *raw);
            if (*raw).canceled.load(Relaxed) == 0 {
                ((*raw).callback.unwrap())(&*raw);
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

    extern "C" fn handler(event: &mut AsyncEvent) {
        event.retval = 7;
        event.error = 3;
    }

    fn empty() -> AsyncEvent {
        let mut event = AsyncEvent::empty();
        event.retval = 1;
        event
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
        assert!(swr_v1_hook_async_job(&mut job, &OPS));
        assert_eq!((RESUMED.get(), errno()), (1, libc::ENOENT));
        REFUSE.set(true);
        assert!(!swr_v1_hook_async_job(&mut job, &OPS));
        REFUSE.set(false);
        assert_eq!(runs, 1);
    }

    #[test]
    fn handler_result_or_timeout_reaches_the_event() {
        RESUMED.set(0);
        let mut event = empty();
        assert!(hook_async(&mut event, handler, 1.0, &OPS));
        assert_eq!(
            (
                event.retval,
                event.error,
                event.canceled.load(Relaxed),
                errno(),
                RESUMED.get()
            ),
            (7, 3, 0, 3, 1)
        );

        TIME_OUT.set(true);
        let mut event = empty();
        assert!(!hook_async(&mut event, handler, 1.0, &OPS));
        TIME_OUT.set(false);
        assert_eq!(
            (
                event.retval,
                event.error,
                event.canceled.load(Relaxed),
                errno()
            ),
            (-1, 9001, 1, 9001)
        );
        // The pool finishes later; the completion skips the gone coroutine.
        park();
        assert_eq!(RESUMED.get(), 1);
    }
}
