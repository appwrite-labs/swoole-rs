//! `coroutine::async`: hand a job to the `AsyncThreads` pool and park the current
//! coroutine until the pool's completion resumes it. [`AsyncOps`] holds the pool's
//! `async::dispatch` and the coroutine operations.
pub mod threads;

use crate::coroutine::Waker;
use crate::set_errno;
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicI32, AtomicIsize, AtomicU8, Ordering::Relaxed};

/// Where a park stores its waker before it switches out.
pub type WakerSlot = core::cell::Cell<Option<Waker>>;

/// `co->yield()` of the current coroutine (fatal outside one), its waker in `waker`.
pub fn park(waker: &WakerSlot) {
    crate::coroutine::park(crate::coroutine::current_safe().cast(), |park| {
        waker.set(Some(park));
    });
}

/// `co->yield_ex(timeout)` of the current coroutine (fatal outside one), its waker
/// in `waker`.
pub fn park_for_armed(timeout: f64, waker: &WakerSlot) -> bool {
    let arm = |park| waker.set(Some(park));
    let code =
        crate::coroutine::yield_timed_armed(crate::coroutine::current_safe().cast(), timeout, arm);
    crate::coroutine::yield_resumed(code)
}

/// `coroutine::async`'s coroutine and pool operations.
pub static COROUTINE_ASYNC: AsyncOps = AsyncOps {
    current: crate::coroutine::current_safe,
    dispatch: threads::dispatch,
    park,
    park_for: park_for_armed,
    resume: Waker::wake,
    last_error: crate::log::last_error,
};

/// `async::Handler`: runs on a pool thread over its own copy of the event, of which
/// the pool keeps `error` and `retval`.
pub type Handler = extern "C" fn(&mut AsyncEvent);

/// The completion: runs on the dispatching thread over the completed event.
pub type Callback = extern "C" fn(&AsyncEvent);

/// Upstream's `AsyncEvent`, with its `shared_ptr` request replaced by a counted
/// DNS record.
pub struct AsyncEvent {
    pub task_id: usize,
    /// The dispatching thread may set it while a pool thread reads it.
    pub canceled: AtomicU8,
    /// The result, which a pool thread stores into its heap copy while the
    /// dispatching thread may set `canceled` there: [`AsyncEvent::result`].
    error: AtomicI32,
    /// A reference to the request's resolver record, or none.
    data: Option<crate::ResolverRef>,
    retval: AtomicIsize,
    pub pipe_socket: *mut c_void,
    pub timestamp: f64,
    /// What the handler and completion read; paired with them at construction.
    object: *mut c_void,
    handler: Option<Handler>,
    callback: Option<Callback>,
}

impl AsyncEvent {
    /// `AsyncEvent event = {}`: no request, handler or completion.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new(core::ptr::null_mut(), None, None, core::ptr::null_mut())
    }

    /// `AsyncEvent event = {}` with these members set: no request. Pairing a handler
    /// with an object asks nothing of the caller: a [`Handler`] or [`Callback`] is a
    /// safe function, sound over any event it is handed, and one that reads its object
    /// as its own type stays private to the module that dispatches it with that object.
    #[must_use]
    pub const fn new(
        object: *mut c_void,
        handler: Option<Handler>,
        callback: Option<Callback>,
        pipe_socket: *mut c_void,
    ) -> Self {
        Self {
            task_id: 0,
            canceled: AtomicU8::new(0),
            error: AtomicI32::new(0),
            data: None,
            retval: AtomicIsize::new(0),
            pipe_socket,
            timestamp: 0.0,
            object,
            handler,
            callback,
        }
    }

    /// `(retval, error)`.
    #[must_use]
    pub fn result(&self) -> (isize, c_int) {
        (self.retval.load(Relaxed), self.error.load(Relaxed))
    }

    /// `retval` and `error`, set by the event's owner.
    pub fn set_result(&mut self, retval: isize, error: c_int) {
        *self.retval.get_mut() = retval;
        *self.error.get_mut() = error;
    }

    /// `retval` and `error`, stored through a shared copy: the pool's result.
    fn publish(&self, (retval, error): (isize, c_int)) {
        self.retval.store(retval, Relaxed);
        self.error.store(error, Relaxed);
    }

    /// The object the handler and completion read.
    #[must_use]
    pub const fn object(&self) -> *mut c_void {
        self.object
    }

    /// Retains the resolver request in this event and each queued copy.
    pub fn set_record(&mut self, record: crate::ResolverRef) {
        self.data = Some(record);
    }

    /// The request's resolver record, when the event has one.
    #[must_use]
    pub fn record(&self) -> Option<&crate::ResolverRequest> {
        self.data.as_deref()
    }
}

impl Clone for AsyncEvent {
    fn clone(&self) -> Self {
        Self {
            canceled: AtomicU8::new(self.canceled.load(Relaxed)),
            error: AtomicI32::new(self.error.load(Relaxed)),
            retval: AtomicIsize::new(self.retval.load(Relaxed)),
            data: self.data.clone(),
            ..*self
        }
    }
}

/// The native operations an offloaded job needs. Only this crate builds one, so the
/// offload trusts what they return.
pub struct AsyncOps {
    /// `Coroutine::get_current_safe()`: fatal outside a coroutine.
    pub(crate) current: extern "C" fn() -> *mut c_void,
    /// `async::dispatch`: a pool-owned copy of the event, live until its completion
    /// returns, or null if refused.
    pub(crate) dispatch: fn(&AsyncEvent) -> *mut AsyncEvent,
    /// `co->yield()` of the current coroutine, its waker stored first.
    pub(crate) park: fn(&WakerSlot),
    /// `co->yield_ex(timeout)` of the current coroutine, its waker stored first:
    /// false on timeout or cancellation.
    pub(crate) park_for: fn(f64, &WakerSlot) -> bool,
    /// `co->resume()` of a parked coroutine.
    pub(crate) resume: fn(Waker),
    /// `swoole_get_last_error()`.
    pub(crate) last_error: fn() -> c_int,
}

/// The waiting coroutine, on its stack; the event's `object` while the job runs.
/// The completion copies the pool's result here before it resumes the coroutine, so
/// the coroutine reads it from its own frame, never from the pool's copy.
struct Task<'a> {
    /// Where the park stores its waker; outside the task, which the pool thread
    /// borrows meanwhile.
    waker: &'a WakerSlot,
    ops: &'a AsyncOps,
    job: Option<&'a mut (dyn FnMut() + Send)>,
    /// `(retval, error)` of the completed event.
    result: core::cell::Cell<(isize, c_int)>,
}

impl<'a> Task<'a> {
    fn new(
        ops: &'a AsyncOps,
        waker: &'a WakerSlot,
        job: Option<&'a mut (dyn FnMut() + Send)>,
    ) -> Self {
        // Fatal outside a coroutine.
        (ops.current)();
        Self {
            waker,
            ops,
            job,
            result: core::cell::Cell::new((0, 0)),
        }
    }
}

/// The pool thread's handler for a job: runs it, then records its errno.
extern "C" fn run_job(event: &mut AsyncEvent) {
    // SAFETY: only `swr_v1_hook_async_job` dispatches this handler, its parked task
    // the object, which the coroutine leaves alone until the completion.
    let task = unsafe { &mut *event.object.cast::<Task<'_>>() };
    if let Some(job) = &mut task.job {
        job();
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    event.set_result(0, error);
}

/// The reactor thread's completion: resumes the parked coroutine unless it gave up.
extern "C" fn completed(event: &AsyncEvent) {
    if event.canceled.load(Relaxed) != 0 {
        return;
    }
    // SAFETY: only this module dispatches this completion, its task the object; not
    // canceled, so the task's coroutine is still parked on it.
    let task = unsafe { &*event.object.cast::<Task<'_>>() };
    task.result.set(event.result());
    if let Some(waker) = task.waker.get() {
        (task.ops.resume)(waker);
    }
}

/// `coroutine::async(handler, event, timeout)`, inside a coroutine (`ops.current` is
/// fatal outside one): runs `handler` on the pool and waits up to `timeout`. True with
/// the pool's result in `event`; false if refused, or on timeout or cancellation
/// (`event` canceled, -1, and the last error as errno). While `handler` runs, `event`'s
/// object is the waiting task, which only this module's handlers read: a handler can
/// reach it only through the raw [`AsyncEvent::object`].
pub fn swr_v1_hook_async(
    event: &mut AsyncEvent,
    handler: Handler,
    timeout: f64,
    ops: &AsyncOps,
) -> bool {
    let waker = WakerSlot::new(None);
    let mut task = Task::new(ops, &waker, None);
    event.object = (&raw mut task).cast();
    event.handler = Some(handler);
    event.callback = Some(completed);
    let dispatched = (ops.dispatch)(event);
    if dispatched.is_null() {
        return false;
    }
    // Parks this coroutine until `completed`, a timeout or a cancel.
    let parked = (ops.park_for)(timeout, &waker);
    // SAFETY: the pool's copy lives until its completion on this thread returns:
    // resumed by `completed`, which has not returned, or pending after a timeout or cancel.
    let done = unsafe { &*dispatched };
    if parked {
        *event.canceled.get_mut() = done.canceled.load(Relaxed);
        let (retval, error) = done.result();
        event.set_result(retval, error);
        set_errno(error);
        true
    } else {
        // Canceling the pending copy keeps `completed` off `task`.
        done.canceled.store(1, Relaxed);
        *event.canceled.get_mut() = 1;
        let error = (ops.last_error)();
        event.set_result(-1, error);
        set_errno(error);
        false
    }
}

/// `coroutine::async_job`: [`swr_v1_hook_async_job`] over the coroutine's own
/// operations; the hooked functions' and `System::read_file`'s offload. Outside a
/// coroutine it raises upstream's fatal error.
pub fn async_job(job: &mut (dyn FnMut() + Send)) -> bool {
    swr_v1_hook_async_job(job, &COROUTINE_ASYNC)
}

/// `coroutine::async(fn)`, inside a coroutine: runs `job` on the pool and waits for
/// it, then sets errno to the job's. False if the pool refused it.
pub fn swr_v1_hook_async_job(job: &mut (dyn FnMut() + Send), ops: &AsyncOps) -> bool {
    let waker = WakerSlot::new(None);
    let mut task = Task::new(ops, &waker, Some(job));
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
    (ops.park)(&waker);
    set_errno(task.result.get().1);
    true
}

#[must_use]
pub fn sleep_timeout(seconds: f64, minimum: f64) -> f64 {
    if seconds < minimum { minimum } else { seconds }
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
    fn dispatch(event: &AsyncEvent) -> *mut AsyncEvent {
        if REFUSE.get() {
            return core::ptr::null_mut();
        }
        let copy = Box::new(event.clone());
        PENDING.with_borrow_mut(|slot| &raw mut **slot.insert(copy))
    }
    // Runs the pool's half (handler, then completion) as the reactor would, unless timed out.
    fn park(waker: &WakerSlot) {
        waker.set(Some(Waker::NONE));
        let raw = PENDING.with_borrow_mut(|slot| &raw mut **slot.as_mut().unwrap());
        unsafe {
            ((*raw).handler.unwrap())(&mut *raw);
            if (*raw).canceled.load(Relaxed) == 0 {
                ((*raw).callback.unwrap())(&*raw);
            }
        }
    }
    fn park_for(_: f64, waker: &WakerSlot) -> bool {
        if TIME_OUT.get() {
            return false;
        }
        park(waker);
        true
    }
    fn resume(_: Waker) {
        RESUMED.set(RESUMED.get() + 1);
    }
    fn last_error() -> c_int {
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
        event.set_result(7, 3);
    }

    fn empty() -> AsyncEvent {
        let mut event = AsyncEvent::empty();
        event.set_result(1, 0);
        event
    }

    use crate::sys::errno;

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
        assert!(swr_v1_hook_async(&mut event, handler, 1.0, &OPS));
        assert_eq!(
            (
                event.result().0,
                event.result().1,
                event.canceled.load(Relaxed),
                errno(),
                RESUMED.get()
            ),
            (7, 3, 0, 3, 1)
        );

        TIME_OUT.set(true);
        let mut event = empty();
        assert!(!swr_v1_hook_async(&mut event, handler, 1.0, &OPS));
        TIME_OUT.set(false);
        assert_eq!(
            (
                event.result().0,
                event.result().1,
                event.canceled.load(Relaxed),
                errno()
            ),
            (-1, 9001, 1, 9001)
        );
        // The pool finishes later; the completion skips the gone coroutine.
        park(&WakerSlot::new(None));
        assert_eq!(RESUMED.get(), 1);
    }
}
