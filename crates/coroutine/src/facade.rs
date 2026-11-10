// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
//! `swoole::Coroutine`, whose C++ class (`native/coroutine/swoole_coroutine.h`)
//! keeps only inline getters: a coroutine's record (its state, then its
//! context) from creation, activation, cancel, the timed yield, the
//! socket-bound error and the `swoole_coroutine_*` functions gdb and C callers
//! use, under their upstream symbols, over scheduler.rs. These need the timer
//! and the event loop, which only the extension links.
//! The build's switches are `swoole_rs_corebase::built_with`'s (`build`).
use crate::context::{Native, swr_v1_context_init};
use crate::scheduler::{
    Routine, bailed_out, context, current, resume, scheduler, with_scheduler, yield_with_cancel,
};
use crate::thread::{THREAD, swr_v1_thread_context_clean, swr_v1_thread_context_init};
use crate::{
    Canceler, Coroutine, RC_CANCELED, RC_TIMEDOUT, STACK_MARK, STACK_PROTECT, swr_v1_coroutine_get,
    swr_v1_coroutine_iterator_each, swr_v1_coroutine_iterator_reset, swr_v1_coroutine_print_list,
    swr_v1_coroutine_run, swr_v1_coroutine_timeout,
};
use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;
use std::ffi::{CStr, CString};
use std::io;
use swoole_rs_corebase::{
    Switch, built_with, fatal_error,
    log::{self, set_last_error},
    swoole_get_stdout_stream,
};

const SW_ERROR_CO_HAS_BEEN_BOUND: c_int = 10002;
const SW_ERROR_CO_CANNOT_CANCEL: c_int = 10015;
const SW_ERROR_CO_CANCELED: c_int = 10017;
const SW_ERROR_CO_TIMEDOUT: c_int = 10018;
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
/// [`build`]'s `SW_CORO_TIME` bit, beside the context's flags.
const CORO_TIME: u32 = 8;

// SAFETY: crates/timer's, crates/hooks' and crates/reactor's upstream
// functions; those marked safe take no pointers and have no preconditions.
// Declarations, since each of those crates depends on this one.
unsafe extern "C-unwind" {
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

/// The build's coroutine switches: the context's flags
/// (`SW_USE_THREAD_CONTEXT`, `SW_CONTEXT_PROTECT_STACK_PAGE`,
/// `SW_CONTEXT_DETECT_STACK_USAGE`) and [`CORO_TIME`].
fn build() -> u32 {
    [
        (Switch::ThreadContext, THREAD),
        (Switch::ContextProtectStackPage, STACK_PROTECT),
        (Switch::ContextDetectStackUsage, STACK_MARK),
        (Switch::CoroTime, CORO_TIME),
    ]
    .into_iter()
    .filter(|(switch, _)| built_with(*switch))
    .fold(0, |flags, (_, bit)| flags | bit)
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
pub unsafe extern "C-unwind" fn swr_v1_coroutine_create(
    routine: Routine,
    arg: *mut c_void,
) -> c_long {
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

/// `yield_ex`'s timer: the wait timed out.
unsafe extern "C" fn timed_out(co: *mut c_void) {
    // SAFETY: the coroutine waits in `yield_ex` until this resumes it.
    unsafe { swr_v1_coroutine_timeout(co.cast()) };
    // SAFETY: as above.
    unsafe { resume(co) };
}

/// `yield_ex`'s cancel function.
unsafe extern "C-unwind" fn resume_canceled(co: *mut c_void, _: *mut c_void) -> bool {
    // SAFETY: the canceled coroutine waits in `yield_ex`.
    unsafe { resume(co) };
    true
}

/// `bool Coroutine::yield_ex(double timeout)`: false, with the last error, when
/// it timed out or was canceled.
///
/// # Safety
/// `co` is the running coroutine.
#[unsafe(export_name = "_ZN6swoole9Coroutine8yield_exEd")]
unsafe extern "C-unwind" fn yield_ex(co: *mut Coroutine, timeout: f64) -> bool {
    // SAFETY: as the caller promises.
    resumed(unsafe { wait(co.cast(), timeout) })
}

/// `yield_ex`'s result for a [`wait`]'s code: false, with the last error,
/// when it timed out or was canceled.
#[must_use]
pub fn resumed(code: i32) -> bool {
    match code {
        RC_TIMEDOUT => set_last_error(SW_ERROR_CO_TIMEDOUT),
        RC_CANCELED => set_last_error(SW_ERROR_CO_CANCELED),
        _ => return true,
    }
    false
}

/// `yield_ex` without its last error: the resume code, `RC_TIMEDOUT` or
/// `RC_CANCELED` when it timed out or was canceled.
///
/// # Safety
/// `co` is the running coroutine; the caller's frames own nothing that needs a drop.
#[must_use]
pub unsafe fn wait(co: *mut c_void, timeout: f64) -> i32 {
    let timer = if timeout > 0.0 {
        // SAFETY: the timer fires only while `co` waits below, or is deleted first.
        unsafe { swr_v1_timer_add_plain_sec(timeout, false, timed_out, co) }
    } else {
        ptr::null_mut()
    };
    // SAFETY: as the caller promises.
    unsafe { yield_with_cancel(co, resume_canceled, ptr::null_mut()) };
    // SAFETY: `co` runs again.
    let code = unsafe { (*co.cast::<Coroutine>()).resume_code };
    if code != RC_TIMEDOUT && !timer.is_null() {
        // SAFETY: the pending timer added above.
        unsafe { swoole_timer_del(timer) };
    }
    code
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
