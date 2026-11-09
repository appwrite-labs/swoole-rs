// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
//! `swoole::Coroutine`, whose C++ class (`native/coroutine/swoole_coroutine.h`)
//! keeps only inline getters: this thread's scheduler and the hooks it runs, a
//! coroutine's record (its state, then its context) from creation to release,
//! activation, bailout, cancel, the timed and cancelable yield, the
//! socket-bound error and the `swoole_coroutine_*` functions gdb and C callers
//! use, under their upstream symbols where Rust or gdb call them by those.
//! native/coroutine/coroutine.cc keeps only the build's switches, which a Rust
//! build can't see.
use crate::context::{
    Context, Native, swr_v1_context_free, swr_v1_context_init, swr_v1_context_swap_in,
    swr_v1_context_swap_out,
};
use crate::thread::{THREAD, swr_v1_thread_context_clean, swr_v1_thread_context_init};
use crate::{
    Canceler, Coroutine, Hooks, RC_CANCELED, RC_TIMEDOUT, STACK_MARK, STACK_PROTECT, Scheduler,
    swr_v1_coroutine_bailout, swr_v1_coroutine_cancel, swr_v1_coroutine_get,
    swr_v1_coroutine_iterator_each, swr_v1_coroutine_iterator_reset, swr_v1_coroutine_print_list,
    swr_v1_coroutine_resume, swr_v1_coroutine_run, swr_v1_coroutine_timeout,
    swr_v1_coroutine_yield_cancelable,
};
use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, c_long, c_void};
use core::mem::MaybeUninit;
use core::ptr;
use std::ffi::{CStr, CString};
use std::io;
use swoole_rs_corebase::{
    fatal_error,
    log::{self, set_last_error},
    swoole_get_stdout_stream,
};

const SW_ERROR_CO_HAS_BEEN_BOUND: c_int = 10002;
const SW_ERROR_CO_CANNOT_CANCEL: c_int = 10015;
const SW_ERROR_CO_CANCELED: c_int = 10017;
const SW_ERROR_CO_TIMEDOUT: c_int = 10018;
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_DEFAULT_C_STACK_SIZE: usize = 2 * 1024 * 1024;
const SW_CORO_BAILOUT_EXIT_CODE: i32 = 1;
/// `SWR_CO_TIME` in the build's switches: `SW_CORO_TIME`.
const CORO_TIME: u32 = 8;

/// A C function over its data, `void (*)(void *)`: a coroutine's entry, a
/// timer's callback or the bailout callback.
type Routine = unsafe extern "C" fn(*mut c_void);

// SAFETY: native/coroutine/coroutine.cc's constant, and crates/timer's,
// crates/hooks' and crates/reactor's upstream functions; those marked safe take
// no pointers and have no preconditions.
unsafe extern "C-unwind" {
    /// The build's switches: `SWR_CTX_*` and `SWR_CO_TIME`.
    safe static swr_cpp_coroutine_build: u32;
    fn swr_v1_timer_add_plain_sec(
        timeout: f64,
        persistent: bool,
        f: Routine,
        data: *mut c_void,
    ) -> *mut c_void;
    fn swr_v1_timer_add_plain(
        ms: c_long,
        persistent: bool,
        f: Routine,
        data: *mut c_void,
    ) -> *mut c_void;
    #[link_name = "_Z25swoole_timer_is_availablev"]
    safe fn swoole_timer_is_available() -> bool;
    #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
    fn swoole_timer_del(node: *mut c_void) -> bool;
    safe fn swr_v1_async_threads_ensure() -> *mut c_void;
    #[link_name = "_Z17swoole_event_initi"]
    safe fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    safe fn swoole_event_wait() -> c_int;
    #[link_name = "_Z25swoole_event_is_availablev"]
    safe fn swoole_event_is_available() -> bool;
}

fn build() -> u32 {
    swr_cpp_coroutine_build
}

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
#[unsafe(no_mangle)]
extern "C" fn swr_v1_coroutine_scheduler_get() -> *mut Scheduler {
    #[cfg(zts)]
    return SCHEDULER.with(UnsafeCell::get);
    #[cfg(not(zts))]
    SCHEDULER.0.get()
}

fn scheduler() -> *mut Scheduler {
    swr_v1_coroutine_scheduler_get()
}

/// `f` over this thread's scheduler.
fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    // SAFETY: this thread's scheduler, live for the thread; the callers' `f`s
    // only touch its fields, so no other borrow of it overlaps.
    f(unsafe { &mut *scheduler() })
}

const _: () = assert!(
    core::mem::offset_of!(Native, ctx) == size_of::<Coroutine>(),
    "a record's context follows its coroutine"
);

/// The context of the record `native`.
fn context(native: *mut c_void) -> *mut Context {
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
    // SAFETY: allocated by `swr_v1_coroutine_create`, freed once.
    drop(unsafe { Box::from_raw(native.cast::<MaybeUninit<Native>>()) });
}

/// The bailout callback once nothing can resume (upstream's `bailed_out`).
extern "C" fn bailed_out(_: *mut c_void) {
    log::error(
        c"Coroutine::bailout",
        format_args!("have been bailout, can not resume any coroutine"),
    );
}

/// `std::chrono::steady_clock`, in microseconds (`time<seconds_type>(true)`).
fn steady_usec() -> c_long {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    #[cfg(target_vendor = "apple")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_vendor = "apple"))]
    let clock = libc::CLOCK_MONOTONIC;
    // SAFETY: a writable timespec.
    unsafe { libc::clock_gettime(clock, &raw mut now) };
    now.tv_sec * 1_000_000 + now.tv_nsec / 1000
}

extern "C" fn clock() -> c_long {
    steady_usec()
}

/// `Coroutine::create(routine, arg)`: a new coroutine runs `routine(arg)` until
/// it first yields or ends. Its id, or -1 when its thread context failed to
/// start (the last error is the errno).
///
/// # Safety
/// `routine` takes `arg`; the caller's frames own nothing with a destructor.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_coroutine_create(routine: Routine, arg: *mut c_void) -> c_long {
    let mut record = Box::<Native>::new_uninit();
    let flags = build() & (THREAD | STACK_PROTECT | STACK_MARK);
    let stack_size = with_scheduler(|s| s.stack_size);
    let ctx = context(record.as_mut_ptr().cast());
    // SAFETY: a writable context that stays put until `destroy` frees it.
    let error = unsafe { swr_v1_context_init(ctx, stack_size, flags, routine, arg) };
    if error != 0 {
        set_last_error(error);
        log::warning(
            c"Coroutine::create",
            format_args!(
                "failed to create coroutine, Error: {}",
                io::Error::from_raw_os_error(error)
            ),
        );
        return -1;
    }
    let native = Box::into_raw(record).cast::<c_void>();
    let now = steady_usec();
    let co = native.cast::<Coroutine>();
    // SAFETY: the record starts with its coroutine, which registers once.
    if with_scheduler(|s| unsafe { s.register(co, native, now / 1000, now) }) {
        activate();
    }
    // SAFETY: the registered coroutine, which runs until it yields or ends.
    unsafe { swr_v1_coroutine_run(scheduler(), co) }
}

extern "C" fn keep_alive(_: *mut c_void) {}

/// `static void Coroutine::activate()`.
#[unsafe(export_name = "_ZN6swoole9Coroutine8activateEv")]
extern "C-unwind" fn activate() {
    if build() & THREAD != 0 {
        // Coroutine threads share this thread's timer and async threads.
        if !swoole_timer_is_available() {
            // SAFETY: a timer whose callback does nothing.
            unsafe { swr_v1_timer_add_plain(1, false, keep_alive, ptr::null_mut()) };
        }
        swr_v1_async_threads_ensure();
        swr_v1_thread_context_init();
    }
    with_scheduler(|s| {
        s.hooks.clock = if build() & CORO_TIME == 0 {
            None
        } else {
            Some(clock)
        };
        s.hooks.bailout = None;
        s.set_active(true);
    });
}

/// `static void Coroutine::deactivate()`.
#[unsafe(export_name = "_ZN6swoole9Coroutine10deactivateEv")]
extern "C-unwind" fn deactivate() {
    if build() & THREAD != 0 {
        swr_v1_thread_context_clean();
    }
    with_scheduler(|s| {
        s.hooks.bailout = Some(bailed_out);
        s.hooks.bailout_data = ptr::null_mut();
        s.set_active(false);
    });
}

/// `static void Coroutine::bailout(callback)`: inside a coroutine the
/// outermost one switches to the main context, where `callback(data)` runs (a
/// PHP bailout that never returns; otherwise the process exits); outside one,
/// later resumes are refused.
///
/// # Safety
/// `callback` takes `data`; the caller's frames own nothing with a destructor.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_coroutine_bail(callback: Option<Routine>, data: *mut c_void) {
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

/// `bool Coroutine::cancel()`: false, with the last error, when the
/// coroutine's wait can't be canceled.
///
/// # Safety
/// `co` is a live coroutine.
#[unsafe(export_name = "_ZN6swoole9Coroutine6cancelEv")]
unsafe extern "C-unwind" fn cancel(co: *mut Coroutine) -> bool {
    // SAFETY: as the caller promises.
    let canceler = unsafe { (*co).cancel_fn }.cast::<Canceler>();
    if canceler.is_null() {
        set_last_error(SW_ERROR_CO_CANNOT_CANCEL);
        return false;
    }
    // SAFETY: the waiting yield's canceler, on its live frame.
    let Canceler { call, data } = unsafe { canceler.read() };
    // SAFETY: that yield's cancel function.
    unsafe { call(co.cast(), data) }
}

fn current() -> *mut Coroutine {
    with_scheduler(|s| s.current)
}

/// `yield_ex`'s timer: the wait timed out.
unsafe extern "C" fn timed_out(co: *mut c_void) {
    // SAFETY: the coroutine waits in `yield_ex` until this resumes it.
    unsafe { swr_v1_coroutine_timeout(co.cast()) };
    // SAFETY: as above.
    unsafe { swr_v1_coroutine_native_resume(co) };
}

/// `yield_ex`'s cancel function.
unsafe extern "C-unwind" fn resume_canceled(co: *mut c_void, _: *mut c_void) -> bool {
    // SAFETY: the canceled coroutine waits in `yield_ex`.
    unsafe { swr_v1_coroutine_native_resume(co) };
    true
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
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_coroutine_yield_with_cancel(
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

/// `bool Coroutine::yield_ex(double timeout)`: false, with the last error, when
/// it timed out or was canceled.
///
/// # Safety
/// `co` is the running coroutine.
#[unsafe(export_name = "_ZN6swoole9Coroutine8yield_exEd")]
unsafe extern "C-unwind" fn yield_ex(co: *mut Coroutine, timeout: f64) -> bool {
    let timer = if timeout > 0.0 {
        // SAFETY: the timer fires only while `co` waits below, or is deleted first.
        unsafe { swr_v1_timer_add_plain_sec(timeout, false, timed_out, co.cast()) }
    } else {
        ptr::null_mut()
    };
    // SAFETY: as the caller promises.
    unsafe { swr_v1_coroutine_yield_with_cancel(co.cast(), resume_canceled, ptr::null_mut()) };
    // SAFETY: `co` runs again.
    let code = unsafe { (*co).resume_code };
    if code == RC_TIMEDOUT {
        set_last_error(SW_ERROR_CO_TIMEDOUT);
        return false;
    }
    if !timer.is_null() {
        // SAFETY: the pending timer added above.
        unsafe { swoole_timer_del(timer) };
    }
    if code == RC_CANCELED {
        set_last_error(SW_ERROR_CO_CANCELED);
        return false;
    }
    true
}

/// `static void Coroutine::print_list()`.
#[unsafe(export_name = "_ZN6swoole9Coroutine10print_listEv")]
extern "C-unwind" fn print_list() {
    // SAFETY: this thread's scheduler and Swoole's open stdout stream.
    unsafe { swr_v1_coroutine_print_list(scheduler(), swoole_get_stdout_stream().cast()) };
}

/// `static void Coroutine::print_socket_bound_error(int, const char *, long)`:
/// records `bound_cid` and raises the fatal error.
///
/// # Safety
/// `event` is a NUL-terminated string.
#[unsafe(export_name = "_ZN6swoole9Coroutine24print_socket_bound_errorEiPKcl")]
unsafe extern "C-unwind" fn print_socket_bound_error(
    fd: c_int,
    event: *const c_char,
    bound_cid: c_long,
) {
    with_scheduler(|s| s.socket_bound_cid = bound_cid);
    // SAFETY: as the caller promises.
    let event = unsafe { CStr::from_ptr(event) }.to_string_lossy();
    let message = format!(
        "Socket#{fd} has already been bound to another coroutine#{bound_cid}, {event} of the same socket in coroutine#{} at the same time is not allowed",
        swoole_coroutine_get_id()
    );
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: Swoole's fatal error hook.
    unsafe { fatal_error(SW_ERROR_CO_HAS_BEEN_BOUND, &message) };
}

/// `long swoole_coroutine_create(void (*)(void *), void *)`: in the running
/// event loop, or (`coroutine::run`) in a fresh one that runs until it exits;
/// the coroutine's id, or -1 if the loop could not start.
///
/// # Safety
/// `routine` takes `arg`.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swoole_coroutine_create(routine: Routine, arg: *mut c_void) -> c_long {
    if swoole_event_is_available() {
        // SAFETY: as the caller promises.
        return unsafe { swr_v1_coroutine_create(routine, arg) };
    }
    if swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) < 0 {
        return -1;
    }
    activate();
    // SAFETY: as the caller promises.
    let cid = unsafe { swr_v1_coroutine_create(routine, arg) };
    swoole_event_wait();
    deactivate();
    cid
}

/// `uint8_t swoole_coroutine_is_in()`.
#[unsafe(no_mangle)]
extern "C" fn swoole_coroutine_is_in() -> u8 {
    u8::from(!current().is_null())
}

/// `long swoole_coroutine_get_id()`: -1 outside a coroutine.
#[unsafe(no_mangle)]
extern "C" fn swoole_coroutine_get_id() -> c_long {
    // SAFETY: the current coroutine is live.
    unsafe { current().as_ref() }.map_or(-1, |co| co.cid)
}

/// `swoole::Coroutine *swoole_coroutine_get(long cid)`, for gdb.
#[unsafe(export_name = "_Z20swoole_coroutine_getl")]
extern "C" fn swoole_coroutine_get(cid: c_long) -> *mut c_void {
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_get(scheduler(), cid) }
}

/// `size_t swoole_coroutine_count()`, for gdb.
#[unsafe(export_name = "_Z22swoole_coroutine_countv")]
extern "C" fn swoole_coroutine_count() -> usize {
    with_scheduler(|s| s.count())
}

/// `void swoole_coroutine_iterator_reset()`, for gdb.
#[unsafe(export_name = "_Z31swoole_coroutine_iterator_resetv")]
extern "C" fn swoole_coroutine_iterator_reset() {
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_iterator_reset(scheduler()) };
}

/// `swoole::Coroutine *swoole_coroutine_iterator_each()`, for gdb.
#[unsafe(export_name = "_Z30swoole_coroutine_iterator_eachv")]
extern "C" fn swoole_coroutine_iterator_each() -> *mut c_void {
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_iterator_each(scheduler()) }
}

/// The current coroutine's facade, or null.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_coroutine_current() -> *mut c_void {
    current().cast()
}

/// `co->resume()`.
///
/// # Safety
/// `co` is a live, suspended coroutine.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_coroutine_native_resume(co: *mut c_void) {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_coroutine_resume(scheduler(), co.cast()) };
}

/// `co->is_timedout()`, of a live coroutine.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_coroutine_timed_out(co: &Coroutine) -> bool {
    co.resume_code == RC_TIMEDOUT
}
