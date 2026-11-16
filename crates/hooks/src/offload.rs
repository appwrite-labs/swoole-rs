//! `coroutine::async`: hand a job to the `AsyncThreads` pool and park the current
//! coroutine until the pool's completion resumes it. [`AsyncOps`] holds the pool's
//! `async::dispatch` and the coroutine operations.
use crate::resolve::WakerSlot;
use core::ffi::{c_int, c_void};
use core::mem::{offset_of, size_of};
use core::sync::atomic::{AtomicI32, AtomicIsize, AtomicU8, Ordering::Relaxed};
use swoole_rs_corebase::set_errno;
use swoole_rs_coroutine::Waker;

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
    /// The result, which a pool thread stores into its heap copy while the
    /// dispatching thread may set `canceled` there: [`AsyncEvent::result`].
    error: AtomicI32,
    /// A reference to the request's resolver record, or none.
    pub(crate) data: Option<swoole_rs_corebase::ResolverRef>,
    retval: AtomicIsize,
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
    pub(crate) fn publish(&self, (retval, error): (isize, c_int)) {
        self.retval.store(retval, Relaxed);
        self.error.store(error, Relaxed);
    }

    /// The object the handler and completion read.
    #[must_use]
    pub const fn object(&self) -> *mut c_void {
        self.object
    }

    /// The request's resolver record, when the event has one.
    pub(crate) fn record(&self) -> Option<&swoole_rs_corebase::ResolverRequest> {
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
    /// `co->yield()` of the current coroutine, its waker stored first.
    pub(crate) park: fn(&WakerSlot),
    /// `co->yield_ex(timeout)` of the current coroutine, its waker stored first:
    /// false on timeout or cancellation.
    pub(crate) park_for: fn(f64, &WakerSlot) -> bool,
    /// `co->resume()` of a parked coroutine.
    pub(crate) resume: fn(Waker),
    /// `swoole_get_last_error()`.
    pub(crate) last_error: extern "C" fn() -> c_int,
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
    swr_v1_hook_async_job(job, &crate::resolve::COROUTINE_ASYNC)
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
        event.set_result(7, 3);
    }

    fn empty() -> AsyncEvent {
        let mut event = AsyncEvent::empty();
        event.set_result(1, 0);
        event
    }

    use swoole_rs_corebase::sys::errno;

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
