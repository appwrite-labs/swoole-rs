// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
//! `swoole::Coroutine`, whose C++ class (`native/coroutine/swoole_coroutine.h`)
//! keeps only inline getters: a coroutine's record (its state, then its
//! context) from creation, activation, cancel, the timed yield, the
//! socket-bound error and the `swoole_coroutine_*` functions gdb and C callers
//! use, under their upstream symbols, over scheduler.rs. These need the timer
//! and the event loop, which only the extension links.
//! The build's switches are `crate::built_with`'s (`build`).
use crate::coroutine::context::{Native, Unmade};
use crate::coroutine::scheduler::{bailed_out, current, with_scheduler, yield_with_cancel_armed};
use crate::coroutine::state::{self as coroutine, Co, Sched, run};
use crate::coroutine::thread::{
    Entry, THREAD, swr_v1_thread_context_clean, swr_v1_thread_context_init,
};
use crate::coroutine::{Coroutine, Resume, Routine, STACK_MARK, STACK_PROTECT, Scheduler};
use crate::{
    Switch, built_with, fatal_error,
    log::{self, set_last_error},
    sw_print,
};
use core::ffi::{c_int, c_long, c_void};
use core::ptr;
use std::ffi::{CStr, CString};
use std::io;

const SW_ERROR_MALLOC_FAIL: c_int = 501;
const SW_ERROR_CO_HAS_BEEN_BOUND: c_int = 10002;
const SW_ERROR_CO_CANNOT_CANCEL: c_int = 10015;
const SW_ERROR_CO_CANCELED: c_int = 10017;
const SW_ERROR_CO_TIMEDOUT: c_int = 10018;
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
/// [`build`]'s `SW_CORO_TIME` bit, beside the context's flags.
const CORO_TIME: u32 = 8;

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
    #[cfg(target_vendor = "apple")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_vendor = "apple"))]
    let clock = libc::CLOCK_MONOTONIC;
    crate::sys::clock_gettime(clock).map_or(0, |now| now.tv_sec * 1_000_000 + now.tv_nsec / 1000)
}

extern "C" fn clock() -> c_long {
    steady_usec()
}

/// `Coroutine::create`: a new coroutine runs `entry` until it first yields or ends.
/// Its id, or -1 when its thread context failed to start (the last error is the
/// errno). `entry` is `Copy`, so this frame owns nothing with a destructor; the
/// coroutine's context holds a copy of it. As every creation, a fatal error or the
/// coroutine's bailout may longjmp over the caller's frames, leaking what they own.
/// Under thread contexts `entry` runs on the coroutine's thread while this one waits
/// for the baton, so the two never run at once.
#[must_use]
pub fn spawn<F: Fn() + Copy + 'static>(entry: F) -> c_long {
    create(Entry::of(entry))
}

/// [`spawn`] of a made entry: one body for every entry type.
fn create(entry: Entry) -> c_long {
    let flags = build() & (THREAD | STACK_PROTECT | STACK_MARK);
    let stack_size = with_scheduler(|s| s.stack_size);
    let native = match Native::new(stack_size, flags, entry) {
        Ok(native) => native,
        Err(unmade) => return unmade_error(&unmade),
    };
    let co = Co::leak(native);
    let now = steady_usec();
    if with_scheduler(|s| s.register(co, co.ptr().cast(), now / 1000, now)) {
        activate();
    }
    run(Sched::this(), co)
}

/// [`spawn`]'s failure: a fatal error without a stack, -1 when a thread failed.
fn unmade_error(unmade: &Unmade) -> c_long {
    match unmade {
        Unmade::NoStack => {
            // SAFETY: this frame owns nothing with a destructor; the caller's, as above.
            unsafe { fatal_error(SW_ERROR_MALLOC_FAIL, c"failed to malloc stack memory.") };
            std::process::exit(254);
        }
        &Unmade::Thread(error) => {
            set_last_error(error);
            log::warning(
                c"Coroutine::create",
                format_args!(
                    "failed to create coroutine, Error: {}",
                    io::Error::from_raw_os_error(error)
                ),
            );
            -1
        }
    }
}

/// `static void Coroutine::activate()`.
pub extern "C-unwind" fn activate() {
    if build() & THREAD != 0 {
        // Coroutine threads share this thread's timer and async threads.
        if crate::thread_timer().is_null() {
            // A 1 ms timer whose callback does nothing.
            _ = crate::timer::add_fn_sec(0.001, false, || {});
        }
        _ = crate::offload::threads::ensure();
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
pub extern "C-unwind" fn deactivate() {
    if build() & THREAD != 0 {
        swr_v1_thread_context_clean();
    }
    with_scheduler(|s| {
        s.hooks.bailout = Some(bailed_out);
        s.hooks.bailout_data = ptr::null_mut();
        s.set_active(false);
    });
}

/// `bool Coroutine::cancel()` of the live coroutine numbered `cid`: false, with the
/// last error, when its wait can't be canceled; false when none lives.
/// Its canceler may resume it: as for [`crate::coroutine::resume`], a bailout skips
/// what the caller's frames own.
#[must_use]
pub fn cancel(cid: c_long) -> bool {
    Sched::this()
        .with(|s| s.live_co(cid))
        .is_some_and(cancel_wait)
}

/// Cancels the current wait of the live coroutine numbered `cid`. A missing or
/// running coroutine cannot be canceled. The wait's own canceler does the cleanup
/// and wakes it; callers must keep the id bound only for the operation they own.
#[must_use]
pub fn cancel_cid(cid: c_long) -> bool {
    let Some(co) = Sched::this().with(|s| s.live_co(cid)) else {
        return false;
    };
    if co.with(|co| co.state != crate::coroutine::State::Waiting) {
        return false;
    }
    cancel_wait(co)
}

fn cancel_wait(co: Co) -> bool {
    let canceler = co.with(|co| {
        let canceler = co.cancel.take();
        if canceler.is_some() {
            co.mark_canceled();
        }
        canceler
    });
    let Some(canceler) = canceler else {
        set_last_error(SW_ERROR_CO_CANNOT_CANCEL);
        return false;
    };
    // Off the coroutine, which the wake may park in another cancelable yield.
    canceler.consume()
}

/// `yield_ex`'s timer: the wait of coroutine `cid`, which waits there until this
/// resumes it, timed out.
fn timed_out(cid: c_long) {
    let s = Sched::this();
    let Some(co) = s.with(|s| s.live_co(cid)) else {
        return;
    };
    co.with(Coroutine::mark_timed_out);
    coroutine::resume_parked(s, co);
}

/// `yield_ex`'s result for a [`yield_timed`]'s code: false, with the last error,
/// when it timed out or was canceled.
#[must_use]
pub fn yield_resumed(code: Resume) -> bool {
    match code {
        Resume::TimedOut => set_last_error(SW_ERROR_CO_TIMEDOUT),
        Resume::Canceled => set_last_error(SW_ERROR_CO_CANCELED),
        Resume::Ok => return true,
    }
    false
}

/// `yield_ex` without its last error: how the yield ended. `co` is the running coroutine
/// (this aborts otherwise); as [`suspend`](crate::coroutine::suspend), it asks nothing more.
#[must_use]
pub fn yield_timed(co: *mut c_void, timeout: f64) -> Resume {
    yield_timed_armed(co, timeout, |_| ())
}

/// [`yield_timed`], handing `arm` the [`Waker`](crate::coroutine::Waker) that ends this park
/// before `co` switches out.
#[must_use]
pub fn yield_timed_armed(
    co: *mut c_void,
    timeout: f64,
    arm: impl FnOnce(crate::coroutine::Waker),
) -> Resume {
    let running = Co::running(Sched::this(), co.cast());
    let timer = if timeout > 0.0 {
        // It fires only while `co` waits below, or is cleared first.
        let cid = running.with(|co| co.cid);
        crate::timer::add_fn_sec(timeout, false, move || timed_out(cid))
            .map_or(0, crate::timer::Held::id)
    } else {
        0
    };
    // The canceled coroutine is `running`, which waits below while its canceler is set.
    let resume_canceled = move || {
        coroutine::resume_parked(Sched::this(), running);
        true
    };
    yield_with_cancel_armed(co, resume_canceled, arm);
    // `co` runs again.
    let code = running.with(|co| co.resume_code);
    if code != Resume::TimedOut && timer != 0 {
        _ = crate::timer::swoole_timer_clear(timer);
    }
    code
}

/// `static void Coroutine::print_list()`.
pub fn print_list() {
    sw_print(with_scheduler(|s| s.list_text()).as_bytes());
}

/// `static void Coroutine::print_socket_bound_error(int, const char *, long)`:
/// records `bound_cid` and raises the fatal error.
///
/// # Safety
/// As [`fatal_error`]: the error may bail out of the caller's frames.
pub unsafe fn print_socket_bound_error(fd: c_int, event: &CStr, bound_cid: c_long) {
    with_scheduler(|s| s.socket_bound_cid = bound_cid);
    let event = event.to_string_lossy();
    let message = format!(
        "Socket#{fd} has already been bound to another coroutine#{bound_cid}, {event} of the same socket in coroutine#{} at the same time is not allowed",
        swoole_coroutine_get_id()
    );
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: Swoole's fatal error hook, as the caller promises.
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
    let fresh = !crate::reactor::event_api::swoole_event_is_available();
    if fresh {
        if crate::reactor::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) < 0 {
            return -1;
        }
        activate();
    }
    // SAFETY: `routine` takes `arg`, as the caller promises, wherever it runs.
    let cid = spawn(move || unsafe { routine(arg) });
    if fresh {
        _ = crate::reactor::event_api::swoole_event_wait();
        deactivate();
    }
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
    crate::coroutine::current_cid()
}

/// `void swoole_coroutine_iterator_reset()`, for gdb.
pub fn swoole_coroutine_iterator_reset() {
    with_scheduler(Scheduler::iterator_reset);
}

/// `swoole::Coroutine *swoole_coroutine_iterator_each()`, for gdb.
#[must_use]
pub fn swoole_coroutine_iterator_each() -> *mut c_void {
    with_scheduler(Scheduler::iterator_each)
}
