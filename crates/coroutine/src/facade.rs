// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
//! `swoole::Coroutine`'s out-of-line operations and the coroutine C API, under
//! their upstream symbols: the timed and cancelable yield, the socket-bound
//! error, `coroutine::run`, and the `swoole_coroutine_*` functions
//! gdb and C callers use. native/coroutine/coroutine.cc keeps the thread-local
//! scheduler, what takes or makes a `std::function`, and cancel, whose
//! `CancelFunc` copy may throw (no C++ exception may cross a Rust frame). A facade starts with its
//! [`Coroutine`], so the two pointers are the same.
use crate::{
    Canceler, Coroutine, RC_CANCELED, RC_TIMEDOUT, Scheduler, swr_v1_coroutine_cancel,
    swr_v1_coroutine_count, swr_v1_coroutine_get, swr_v1_coroutine_iterator_each,
    swr_v1_coroutine_iterator_reset, swr_v1_coroutine_print_list, swr_v1_coroutine_resume,
    swr_v1_coroutine_timeout, swr_v1_coroutine_yield_cancelable,
};
use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;
use std::ffi::{CStr, CString};
use swoole_rs_corebase::{fatal_error, log::set_last_error, swoole_get_stdout_stream};

const SW_ERROR_CO_HAS_BEEN_BOUND: c_int = 10002;
const SW_ERROR_CO_CANCELED: c_int = 10017;
const SW_ERROR_CO_TIMEDOUT: c_int = 10018;
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;

/// `const CoroutineFunc &`, passed through untouched.
type Func = *const c_void;
type Routine = Option<unsafe extern "C-unwind" fn(*mut c_void)>;

// SAFETY: native/coroutine/coroutine.cc's, and crates/timer's and
// native/reactor/reactor.cc's upstream functions.
unsafe extern "C-unwind" {
    fn swr_cpp_coroutine_scheduler() -> *mut c_void;
    fn swr_cpp_coroutine_create(routine: Routine, arg: *mut c_void, func: Func) -> c_long;
    fn swr_cpp_coroutine_timer_add(
        timeout: f64,
        f: unsafe extern "C-unwind" fn(*mut c_void),
        data: *mut c_void,
    ) -> *mut c_void;
    #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
    fn swoole_timer_del(node: *mut c_void) -> bool;
    #[link_name = "_ZN6swoole9Coroutine8activateEv"]
    fn activate();
    #[link_name = "_ZN6swoole9Coroutine10deactivateEv"]
    fn deactivate();
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    fn swoole_event_wait() -> c_int;
    #[link_name = "_Z25swoole_event_is_availablev"]
    fn swoole_event_is_available() -> bool;
}

fn scheduler() -> *mut Scheduler {
    // SAFETY: the address of this thread's scheduler.
    unsafe { swr_cpp_coroutine_scheduler() }.cast()
}

fn current() -> *mut Coroutine {
    // SAFETY: this thread's scheduler.
    unsafe { (*scheduler()).current }
}

/// Resumes the facade `co`.
///
/// # Safety
/// `co` is a live, suspended coroutine.
unsafe extern "C-unwind" fn resume(co: *mut c_void) {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_coroutine_resume(scheduler(), co.cast()) };
}

/// `yield_ex`'s timer: the wait timed out.
unsafe extern "C-unwind" fn timed_out(co: *mut c_void) {
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
        unsafe { swr_cpp_coroutine_timer_add(timeout, timed_out, co.cast()) }
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
    // SAFETY: this thread's scheduler.
    unsafe { (*scheduler()).socket_bound_cid = bound_cid };
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

/// A fresh event loop that runs the coroutine `create` makes until the loop
/// exits; the coroutine's id, or -1 if the loop could not start.
///
/// # Safety
/// As [`swr_cpp_coroutine_create`]'s arguments.
unsafe fn run_loop(routine: Routine, arg: *mut c_void, func: Func) -> c_long {
    // SAFETY: upstream's event loop API.
    if unsafe { swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) } < 0 {
        return -1;
    }
    // SAFETY: as above.
    unsafe { activate() };
    // SAFETY: as the caller promises.
    let cid = unsafe { swr_cpp_coroutine_create(routine, arg, func) };
    // SAFETY: as above.
    unsafe { swoole_event_wait() };
    // SAFETY: as above.
    unsafe { deactivate() };
    cid
}

/// `bool swoole::coroutine::run(const CoroutineFunc &fn, void *arg)`.
///
/// # Safety
/// `func` is a live `CoroutineFunc`.
#[cfg_attr(
    target_os = "macos",
    unsafe(export_name = "_ZN6swoole9coroutine3runERKNSt3__18functionIFvPvEEES3_")
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(export_name = "_ZN6swoole9coroutine3runERKSt8functionIFvPvEES2_")
)]
unsafe extern "C-unwind" fn run(func: Func, arg: *mut c_void) -> bool {
    // SAFETY: as the caller promises.
    unsafe { run_loop(None, arg, func) > 0 }
}

/// `long swoole_coroutine_create(void (*)(void *), void *)`: in the running
/// event loop, or in a fresh one it runs to the end.
///
/// # Safety
/// `routine` takes `arg`.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swoole_coroutine_create(routine: Routine, arg: *mut c_void) -> c_long {
    // SAFETY: upstream's event loop API.
    if unsafe { swoole_event_is_available() } {
        // SAFETY: as the caller promises.
        return unsafe { swr_cpp_coroutine_create(routine, arg, ptr::null()) };
    }
    // SAFETY: as the caller promises.
    unsafe { run_loop(routine, arg, ptr::null()) }
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
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_count(scheduler()) }
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
    unsafe { resume(co) };
}

/// `co->is_timedout()`.
///
/// # Safety
/// `co` is a live coroutine.
#[unsafe(no_mangle)]
unsafe extern "C" fn swr_v1_coroutine_timed_out(co: *mut Coroutine) -> bool {
    // SAFETY: as the caller promises.
    unsafe { (*co).resume_code == RC_TIMEDOUT }
}
