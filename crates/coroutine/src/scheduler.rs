// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
//! This thread's scheduler (`Coroutine::scheduler_`) over [`Native`] records, and
//! the operations Rust callers run on it by path: the current coroutine, resume,
//! the cancelable yield and bailout. They need no timer or event loop, so they
//! build without `upstream-symbols`; facade.rs builds the rest on them.
use crate::context::{
    Context, Native, swr_v1_context_free, swr_v1_context_swap_in, swr_v1_context_swap_out,
};
use crate::{
    Canceler, Coroutine, Hooks, Scheduler, swr_v1_coroutine_bailout, swr_v1_coroutine_cancel,
    swr_v1_coroutine_resume, swr_v1_coroutine_yield_cancelable,
};
use core::cell::UnsafeCell;
use core::ffi::{c_long, c_void};
use core::mem::MaybeUninit;
use core::ptr;
use swoole_rs_corebase::log;

const SW_DEFAULT_C_STACK_SIZE: usize = 2 * 1024 * 1024;
const SW_CORO_BAILOUT_EXIT_CODE: i32 = 1;

/// A C function over its data, `void (*)(void *)`: a coroutine's entry, a
/// timer's callback or the bailout callback.
pub type Routine = unsafe extern "C" fn(*mut c_void);

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
            swap_in,
            swap_out,
            destroy,
            bailout: None,
            bailout_data: ptr::null_mut(),
            clock: None,
        },
        live: ptr::null_mut(),
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
pub fn scheduler() -> *mut Scheduler {
    #[cfg(zts)]
    return SCHEDULER.with(UnsafeCell::get);
    #[cfg(not(zts))]
    SCHEDULER.0.get()
}

/// [`scheduler`], for `swoole_coroutine.h`'s inline getters.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_coroutine_scheduler_get() -> *mut Scheduler {
    scheduler()
}

/// `f` over this thread's scheduler.
pub(crate) fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    // SAFETY: this thread's scheduler, live for the thread; the callers' `f`s
    // only touch its fields, so no other borrow of it overlaps.
    f(unsafe { &mut *scheduler() })
}

const _: () = assert!(
    core::mem::offset_of!(Native, ctx) == size_of::<Coroutine>(),
    "a record's context follows its coroutine"
);

/// The context of the record `native`.
pub(crate) fn context(native: *mut c_void) -> *mut Context {
    native.cast::<Coroutine>().wrapping_add(1).cast()
}

/// Switches into the record `native`: whether its coroutine ended.
unsafe extern "C" fn swap_in(native: *mut c_void) -> bool {
    let ctx = context(native);
    // SAFETY: the scheduler passes a live record whose coroutine has not ended.
    unsafe { swr_v1_context_swap_in(ctx) };
    // SAFETY: as above; its context switched back out.
    unsafe { (*ctx).end }
}

/// Switches out of the running record `native`.
unsafe extern "C" fn swap_out(native: *mut c_void) {
    // SAFETY: the scheduler passes the running record.
    unsafe { swr_v1_context_swap_out(context(native)) };
}

/// Frees the ended record `native` and its stack (upstream's `delete`).
unsafe extern "C" fn destroy(native: *mut c_void) {
    // SAFETY: the scheduler passes an ended, released record.
    unsafe { swr_v1_context_free(context(native)) };
    // SAFETY: allocated by facade.rs's `swr_v1_coroutine_create`, freed once.
    drop(unsafe { Box::from_raw(native.cast::<MaybeUninit<Native>>()) });
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

/// `co->resume()`, a reactor deferred call.
///
/// # Safety
/// `co` is a live, suspended coroutine.
pub unsafe extern "C-unwind" fn resume(co: *mut c_void) {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_coroutine_resume(scheduler(), co.cast()) };
}

/// Marks `co` canceled, then calls the caller's canceler, `inner`.
unsafe extern "C-unwind" fn mark_canceled(co: *mut c_void, inner: *mut c_void) -> bool {
    // SAFETY: the yield that set the canceler is still waiting.
    unsafe { swr_v1_coroutine_cancel(co.cast()) };
    // SAFETY: as above, so its frame (and `inner`) is live.
    let Canceler { call, data } = unsafe { inner.cast::<Canceler>().read() };
    // SAFETY: the caller's cancel function for this yield.
    unsafe { call(co, data) }
}

/// `co->yield(&cancel_fn)`, where `cancel_fn` calls `call(co, data)` once `co`
/// is marked canceled.
///
/// # Safety
/// `co` is the running coroutine; the caller's frames own nothing with a destructor.
pub unsafe fn yield_with_cancel(
    co: *mut c_void,
    call: unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> bool,
    data: *mut c_void,
) {
    let mut inner = Canceler { call, data };
    let mut canceler = Canceler {
        call: mark_canceled,
        data: (&raw mut inner).cast(),
    };
    let canceler = (&raw mut canceler).cast();
    // SAFETY: as the caller promises; both cancelers outlive the yield.
    unsafe { swr_v1_coroutine_yield_cancelable(scheduler(), co.cast(), canceler) };
}

/// `static void Coroutine::bailout(callback)`: inside a coroutine the
/// outermost one switches to the main context, where `callback(data)` runs (a
/// PHP bailout that never returns; otherwise the process exits); outside one,
/// later resumes are refused.
///
/// # Safety
/// `callback` takes `data`; the caller's frames own nothing with a destructor.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_coroutine_bail(
    callback: Option<Routine>,
    data: *mut c_void,
) {
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
            (Some(bailed_out as Routine), ptr::null_mut())
        };
    });
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_bailout(scheduler()) };
    if inside {
        std::process::exit(SW_CORO_BAILOUT_EXIT_CODE);
    }
}
