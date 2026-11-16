// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
//! This thread's scheduler (`Coroutine::scheduler_`) over [`Native`] records, and
//! the operations Rust callers run on it by path: the current coroutine, resume,
//! the cancelable yield and bailout. They need no timer or event loop, so they
//! build without `upstream-symbols`; facade.rs builds the rest on them.
use crate::context::{self, Native};
use crate::coroutine::{self as ops, Co, Sched};
use crate::{Canceler, Coroutine, CoroutineTask, Hooks, Routine, Scheduler};
use core::cell::UnsafeCell;
use core::ffi::{c_long, c_void};
use core::ptr::{self, NonNull};
use swoole_rs_corebase::log;

const SW_DEFAULT_C_STACK_SIZE: usize = 2 * 1024 * 1024;
const SW_CORO_BAILOUT_EXIT_CODE: i32 = 1;

/// A fresh scheduler: no coroutine, upstream's default stack size, and the
/// hooks over [`Native`] records.
const fn fresh() -> Scheduler {
    Scheduler {
        current: ptr::null_mut(),
        last_cid: 0,
        socket_bound_cid: 0,
        peak_num: 0,
        stack_size: SW_DEFAULT_C_STACK_SIZE,
        on_yield: None,
        on_resume: None,
        on_close: None,
        hooks: Hooks {
            swap_in: context::swap_in,
            swap_out: context::swap_out,
            destroy,
            bailout: None,
            bailout_data: ptr::null_mut(),
            clock: None,
        },
        live: None,
        cursor: c_long::MAX,
        activated: false,
        bailout: false,
    }
}

// Upstream's SW_THREAD_LOCAL: per thread on a thread-safe PHP, one process
// value otherwise (where thread-context coroutines share it from their threads).
#[cfg(zts)]
thread_local! {
    static SCHEDULER: UnsafeCell<Scheduler> = const { UnsafeCell::new(fresh()) };
}

#[cfg(not(zts))]
struct Process(UnsafeCell<Scheduler>);

// SAFETY: a non-thread-safe PHP runs coroutines on one thread at a time (the
// thread context hands a baton between threads), as upstream's plain global.
#[cfg(not(zts))]
unsafe impl Sync for Process {}

#[cfg(not(zts))]
static SCHEDULER: Process = Process(UnsafeCell::new(fresh()));

/// This thread's scheduler (`Coroutine::scheduler_`), live for the thread's life.
#[must_use]
pub fn scheduler() -> *mut Scheduler {
    this().as_ptr()
}

/// [`scheduler`], never null.
pub(crate) fn this() -> NonNull<Scheduler> {
    #[cfg(zts)]
    return SCHEDULER.with(|s| NonNull::from(s).cast());
    #[cfg(not(zts))]
    NonNull::from(&SCHEDULER.0).cast()
}

/// [`scheduler`], for `swoole_coroutine.h`'s inline getters.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_coroutine_scheduler_get() -> *mut Scheduler {
    scheduler()
}

/// `Coroutine::count()` on this thread.
#[must_use]
pub fn coroutine_count() -> usize {
    with_scheduler(|s| s.count())
}

/// `Coroutine::get_by_cid` on this thread: the facade, or null.
#[must_use]
pub fn coroutine_get(cid: c_long) -> *mut c_void {
    with_scheduler(|s| s.get(cid))
}

/// [`Scheduler::state`] on this thread.
#[must_use]
pub fn coroutine_state(cid: c_long) -> Option<crate::State> {
    with_scheduler(|s| s.state(cid))
}

/// [`Scheduler::cid_of`] on this thread.
#[must_use]
pub fn coroutine_cid_of(co: *const c_void) -> Option<c_long> {
    with_scheduler(|s| s.cid_of(co.cast()))
}

/// [`Scheduler::older_native`] on this thread.
#[must_use]
pub fn coroutine_older(before: c_long) -> Option<(c_long, *mut c_void)> {
    with_scheduler(|s| s.older_native(before))
}

/// The coroutine the last socket-bound error named (`Coroutine::get_socket_bound_cid`).
#[must_use]
pub fn socket_bound_cid() -> c_long {
    with_scheduler(|s| s.socket_bound_cid)
}

/// Names `cid` as the coroutine a socket (or a cURL handle) is bound to, for the
/// fatal error that follows.
pub fn set_socket_bound_cid(cid: c_long) {
    with_scheduler(|s| s.socket_bound_cid = cid);
}

/// [`Scheduler::set_stack_size`] on this thread.
pub fn set_stack_size(size: usize) {
    with_scheduler(|s| s.set_stack_size(size));
}

/// [`Scheduler::set_callback`] on this thread.
pub fn set_callback(slot: u32, function: crate::SwapCallback) {
    with_scheduler(|s| s.set_callback(slot, function));
}

/// `f` over this thread's scheduler.
pub(crate) fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    Sched::this().with(f)
}

/// Frees the ended `co`'s record and its stack (upstream's `delete`).
fn destroy(co: Co) {
    let native = co.with(|co| co.native);
    // SAFETY: the scheduler passes an ended, released coroutine whose record
    // facade.rs's `swr_v1_coroutine_create` boxed; it is freed once, here.
    drop(unsafe { Box::from_raw(native.cast::<Native>()) });
}

/// The bailout callback once nothing can resume (upstream's `bailed_out`).
pub(crate) extern "C" fn bailed_out(_: *mut c_void) {
    log::error(
        c"Coroutine::bailout",
        format_args!("have been bailout, can not resume any coroutine"),
    );
}

/// The running coroutine, or null.
#[must_use]
pub fn current() -> *mut Coroutine {
    with_scheduler(|s| s.current)
}

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: core::ffi::c_int = 10001;

/// `Coroutine::get_current_safe()`: the running coroutine; outside one, Swoole's fatal
/// error, which exits the process or bails out over the callers' frames as upstream's
/// does.
#[must_use]
pub extern "C" fn current_safe() -> *mut c_void {
    let co = current().cast::<c_void>();
    if co.is_null() {
        // SAFETY: Swoole's fatal error hook; this frame owns nothing.
        unsafe {
            swoole_rs_corebase::fatal_error(
                OUT_OF_COROUTINE,
                c"API must be called in the coroutine",
            );
        }
    }
    co
}

/// `Coroutine::get_current()`: the running coroutine's record, null outside one.
#[must_use]
pub fn current_native() -> *mut c_void {
    Co::current(Sched::this()).map_or(ptr::null_mut(), |co| co.with(|co| co.native))
}

/// Sets the running coroutine's task, which the swap callbacks get; nothing
/// outside one.
///
/// # Safety
/// `task` is the kind of object this thread's swap callbacks take, and stays live
/// until the running coroutine's task is cleared ([`clear_current_task`]) or its
/// close callback has run; no other coroutine has it as its task.
pub unsafe fn set_current_task(task: NonNull<c_void>) {
    if let Some(co) = Co::current(Sched::this()) {
        co.with(|co| co.task = Some(CoroutineTask(task)));
    }
}

/// The running coroutine's swap callbacks get no task from here on; nothing
/// outside one.
pub fn clear_current_task() {
    if let Some(co) = Co::current(Sched::this()) {
        co.with(|co| co.task = None);
    }
}

/// `Coroutine::get_current_cid()`: -1 outside a coroutine.
#[must_use]
pub fn current_cid() -> c_long {
    Co::current(Sched::this()).map_or(-1, |co| co.with(|co| co.cid))
}

/// How the running coroutine's last yield ended,
/// which its last yield returned with; `None` outside a coroutine.
#[must_use]
pub fn current_resume_code() -> Option<crate::Resume> {
    Co::current(Sched::this()).map(|co| co.with(|co| co.resume_code))
}

/// This thread's scheduler's counters, for `Co::stats()`.
#[derive(Clone, Copy, Debug)]
pub struct Counters {
    pub stack_size: usize,
    pub peak_num: u64,
    pub last_cid: c_long,
    /// Whether `SW_CORO_TIME` clocks the coroutines.
    pub timed: bool,
}

/// This thread's [`Counters`].
#[must_use]
pub fn counters() -> Counters {
    with_scheduler(|s| Counters {
        stack_size: s.stack_size,
        peak_num: s.peak_num,
        last_cid: s.last_cid,
        timed: s.hooks.clock.is_some(),
    })
}

/// `co->resume()`, a reactor deferred call.
///
/// # Safety
/// `co` is a live, suspended coroutine.
/// If its bailout callback can longjmp, every main-stack frame between this
/// call and the callback's trap must own nothing that needs dropping. Abandoned
/// coroutine stacks stay allocated, but the main stack can be reused.
pub unsafe extern "C-unwind" fn resume(co: *mut c_void) {
    // SAFETY: as the caller promises.
    if let Some(co) = unsafe { Co::of(co.cast()) } {
        ops::resume(Sched::this(), co);
    }
}

/// Runs a copy of the yield's closure at `data` over the canceled `co`.
///
/// # Safety
/// `data` is the `F` of the yield `co` still waits in.
unsafe fn run_canceler<F: FnOnce(*mut c_void) -> bool + Copy>(
    co: *mut c_void,
    data: *const c_void,
) -> bool {
    // SAFETY: as the caller promises; copied, as `cancel` may end its frame.
    let cancel = unsafe { data.cast::<F>().read() };
    cancel(co)
}

/// `co->yield()` of `co`, the running coroutine (aborting otherwise), until it
/// is resumed.
///
/// A yield asks nothing of its caller. `co` is only compared with the running
/// coroutine. A coroutine nothing resumes (a bailout abandons every suspended one)
/// stays parked for good with its stack, or its thread, in place: only `destroy`
/// frees one, and only once its entry returned. So what the caller's frames own is
/// leaked, as by a thread that never wakes, and never freed under them. Freeing a
/// suspended coroutine anywhere (at shutdown, say) would make this `unsafe` again.
pub fn suspend(co: *mut c_void) {
    park(co, |_| ());
}

/// [`suspend`], handing `arm` the [`Waker`](crate::Waker) that ends this park before
/// `co` switches out.
#[inline]
pub fn park(co: *mut c_void, arm: impl FnOnce(crate::Waker)) {
    let s = Sched::this();
    ops::yield_armed(s, Co::running(s, co.cast()), arm);
}

/// `co->yield(&cancel_fn)` of `co`, the running coroutine (aborting
/// otherwise), where `cancel_fn` calls `cancel(co)` once `co` is marked canceled.
///
/// As [`suspend`], it asks nothing of its caller: `cancel` and its canceler sit on
/// this frame, which stays in place while the coroutine is parked, and the
/// coroutine's cancel function is cleared before this returns.
pub fn yield_with_cancel<F: FnOnce(*mut c_void) -> bool + Copy>(co: *mut c_void, cancel: F) {
    yield_with_cancel_armed(co, cancel, |_| ());
}

/// [`yield_with_cancel`], handing `arm` the [`Waker`](crate::Waker) that ends this
/// park before `co` switches out.
#[inline]
pub fn yield_with_cancel_armed<F: FnOnce(*mut c_void) -> bool + Copy>(
    co: *mut c_void,
    cancel: F,
    arm: impl FnOnce(crate::Waker),
) {
    let s = Sched::this();
    let co = Co::running(s, co.cast());
    let canceler = Canceler {
        call: run_canceler::<F>,
        data: (&raw const cancel).cast(),
    };
    // The canceler outlives the yield.
    ops::yield_cancelable(s, co, (&raw const canceler).cast_mut().cast(), arm);
}

/// `static void Coroutine::bailout(callback)`: inside a coroutine the
/// outermost one switches to the main context, where `callback(data)` runs (a
/// PHP bailout that never returns; otherwise the process exits); outside one,
/// later resumes are refused.
///
/// # Safety
/// `callback` takes `data`; the caller's frames own nothing with a destructor.
pub unsafe fn swr_v1_coroutine_bail(callback: Option<Routine>, data: *mut c_void) {
    let inside = !current().is_null();
    if inside && callback.is_none() {
        log::error(
            c"Coroutine::bailout",
            format_args!("bailout without callback function"),
        );
    }
    with_scheduler(|s| {
        (s.hooks.bailout, s.hooks.bailout_data) = if inside {
            (callback, data)
        } else {
            (Some::<Routine>(bailed_out), ptr::null_mut())
        };
    });
    ops::bailout(Sched::this());
    if inside {
        std::process::exit(SW_CORO_BAILOUT_EXIT_CODE);
    }
}
