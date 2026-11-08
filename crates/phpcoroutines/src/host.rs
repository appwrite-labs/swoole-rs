//! What PHP coroutines ask of the rest of Swoole: the coroutine core's
//! scheduler (crates/coroutine's state, which C++ keeps thread-local), the
//! module host, the runtime hooks and the event loop, and
//! native/phpcoroutines/coroutine.cc for the C++ objects (a Coroutine's
//! construction and context, `std::function` members, Server, Reactor).
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use swoole_rs_coroutine::{RC_CANCELED, Scheduler};
use swoole_rs_zend::{
    E_ERROR, bailout,
    sys::{swr_zend_callable, zend_array, zend_execute_data, zval},
};

/// `swr_v1_phpcoroutines_stats`.
#[repr(C)]
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) event_num: i64,
    pub(crate) signal_listener_num: i64,
    pub(crate) aio_task_num: i64,
    pub(crate) aio_worker_num: i64,
    pub(crate) aio_queue_size: i64,
    pub(crate) c_stack_size: i64,
    pub(crate) coroutine_num: i64,
    pub(crate) coroutine_peak_num: i64,
    pub(crate) coroutine_last_cid: i64,
    pub(crate) iouring: u8,
    pub(crate) iouring_task_num: i64,
    pub(crate) iouring_sq_usage_percent: i64,
    pub(crate) iouring_waiting_task_num: i64,
}

/// A `swoole::Coroutine`, which starts with its `swoole_rs_coroutine::Coroutine`.
type Co = *mut c_void;

unsafe extern "C" {
    // native/coroutine/coroutine.cc, crates/coroutine and native/phpcoroutines/coroutine.cc
    fn swr_cpp_coroutine_scheduler() -> *mut c_void;
    fn swr_cpp_coroutine_create(
        entry: unsafe extern "C" fn(*mut c_void),
        args: *mut c_void,
        func: *const c_void,
    ) -> c_long;
    fn swr_v1_coroutine_yield_with_cancel(
        co: Co,
        cancel: unsafe extern "C-unwind" fn(Co, *mut c_void) -> bool,
        data: *mut c_void,
    );
    fn swr_cpp_phpcoroutines_host_flags(mask: u32) -> u32;
    fn swr_cpp_phpcoroutines_bailout();
    fn swr_cpp_phpcoroutines_kernel_coroutine(count: c_long, sleep: f64);
    fn swr_cpp_phpcoroutines_stats(stats: *mut Stats);
    // crates/hooks/src/async_threads.rs.
    fn swr_v1_async_threads_stats(task_num: &mut i64, worker_num: &mut i64, queue_size: &mut i64);
    fn swr_cpp_phpcoroutines_stack(co: Co, size: *mut usize) -> *mut c_void;
    fn swr_cpp_phpcoroutines_is_end(co: Co) -> u8;
    fn swr_cpp_phpcoroutines_activate(deactivate: unsafe extern "C" fn(*mut c_void));
    fn swr_cpp_phpcoroutines_defer(f: unsafe extern "C" fn(*mut c_void), data: *mut c_void);
    fn swr_cpp_phpcoroutines_set_exit_condition(set: u8);
    // crates/coroutine, native/coroutine/coroutine.cc, crates/timer, crates/reactor and crates/process.
    #[link_name = "_ZN6swoole9Coroutine8yield_exEd"]
    fn coroutine_yield_ex(co: Co, timeout: f64) -> bool;
    #[link_name = "_ZN6swoole9Coroutine6cancelEv"]
    fn coroutine_cancel(co: Co) -> bool;
    #[link_name = "_ZN6swoole9Coroutine10deactivateEv"]
    fn coroutine_deactivate();
    #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
    fn swoole_timer_del(timer: *mut c_void) -> bool;
    fn swr_v1_timer_add_plain_sec(
        timeout: f64,
        persistent: bool,
        f: unsafe extern "C" fn(*mut c_void),
        data: *mut c_void,
    ) -> *mut c_void;
    #[link_name = "_Z30swoole_signal_get_listener_numv"]
    fn swoole_signal_get_listener_num() -> u32;
    // native/phphelpers/module.cc, crates/phpevents, crates/phpclients, crates/phphelpers and PHP.
    #[link_name = "_Z32php_swoole_call_original_handlerPKcmP18_zend_execute_dataP12_zval_struct"]
    fn php_swoole_call_original_handler(
        name: *const c_char,
        length: usize,
        execute_data: *mut zend_execute_data,
        return_value: *mut zval,
    ) -> bool;
    fn sw_php_exit(status: c_int);
    fn php_write(buf: *const c_void, size: usize) -> usize;
    fn php_swoole_reactor_init() -> i32;
    fn swr_v1_phpevents_wait();
    fn php_swoole_name_resolver_add(resolver: *mut zval) -> bool;
    fn swr_v1_phphelpers_call_anyway(
        fci: *mut swoole_rs_zend::sys::zend_fcall_info,
        fcc: *mut swoole_rs_zend::sys::zend_fcall_info_cache,
    ) -> c_int;
    fn swr_v1_phphelpers_cache_free(fcc: *mut swoole_rs_zend::sys::zend_fcall_info_cache);
}

/// `swr_v1_phpcoroutines_*` flag bits.
pub(crate) const CLI: u32 = 1;
pub(crate) const SHORTNAME: u32 = 2;
pub(crate) const LIBRARY: u32 = 4;
pub(crate) const FIBER_MOCK: u32 = 8;
pub(crate) const PREEMPTIVE: u32 = 16;
pub(crate) const SHUTDOWN_FUNCTIONS: u32 = 32;
pub(crate) const SERVER_STARTED: u32 = 64;
pub(crate) const MAIN_THREAD: u32 = 128;
pub(crate) const FATAL: u32 = 256;
pub(crate) const CORO_TIME: u32 = 512;
pub(crate) const THREAD: u32 = 1024;

/// Swoole error codes (`include/swoole_error.h`).
pub(crate) const SW_ERROR_INVALID_PARAMS: c_int = 505;
pub(crate) const SW_ERROR_WRONG_OPERATION: c_int = 509;
pub(crate) const SW_ERROR_CO_NOT_EXISTS: c_int = 10016;
pub(crate) const SW_ERROR_CO_CANCELED: c_int = 10017;
const SW_ERROR_PHP_FATAL_ERROR: c_int = 503;
const SW_ERROR_CO_OUT_OF_COROUTINE: c_int = 10001;
/// `SW_GLOBAL_HOOK_ON_CORO_START` (`+ 1` is `_STOP`).
const HOOK_ON_CORO_START: c_int = 3;

/// Whether `bit` of the host's flags is set.
pub(crate) fn flag(bit: u32) -> bool {
    // SAFETY: this thread's module globals, live from MINIT.
    let g = unsafe { &*swoole_rs_phpswoole::globals() };
    match bit {
        CLI => g.cli,
        SHORTNAME => g.use_shortname,
        LIBRARY => g.enable_library,
        FIBER_MOCK => g.enable_fiber_mock,
        PREEMPTIVE => g.enable_preemptive_scheduler,
        SHUTDOWN_FUNCTIONS => g.req_status == swoole_rs_phpswoole::CALL_USER_SHUTDOWNFUNC_BEGIN,
        // SAFETY: C++ reads the server, the reactor and the build's switches.
        _ => (unsafe { swr_cpp_phpcoroutines_host_flags(bit) }) != 0,
    }
}

pub(crate) use swoole_rs_corebase::log::set_last_error;

/// `php_swoole_fatal_error(level, "%s", message)`. With `E_ERROR` Zend
/// longjmps out: the caller's frames must own nothing that needs a drop.
pub(crate) fn fatal_error(level: c_int, message: &CStr) {
    bailout::raise(try_fatal_error(level, message));
}

/// [`fatal_error`], returning its bailout.
pub(crate) fn try_fatal_error(level: c_int, message: &CStr) -> Result<(), bailout::Bailout> {
    set_last_error(SW_ERROR_PHP_FATAL_ERROR);
    // SAFETY: a warning or error, printed through "%s".
    unsafe { swoole_rs_zend::php_error(level, message) }
}

/// `php_swoole_error_ex(level, code, "%s", message)`.
pub(crate) fn error_ex(level: c_int, code: c_int, message: &CStr) {
    set_last_error(code);
    if swoole_rs_phpswoole::display_errors() || level == E_ERROR {
        // SAFETY: as in `fatal_error`.
        bailout::raise(unsafe { swoole_rs_zend::php_error(level, message) });
    }
}

fn scheduler() -> *mut Scheduler {
    // SAFETY: this thread's scheduler, which lives as long as the thread.
    unsafe { swr_cpp_coroutine_scheduler() }.cast()
}

/// The Rust state `co` starts with.
fn state(co: Co) -> *mut swoole_rs_coroutine::Coroutine {
    co.cast()
}

/// The facade of `co`'s Rust state, null for none.
fn native(co: *const swoole_rs_coroutine::Coroutine) -> Co {
    // SAFETY: a live coroutine or null.
    unsafe { co.as_ref() }.map_or(std::ptr::null_mut(), |co| co.native)
}

pub(crate) fn count() -> usize {
    // SAFETY: this thread's scheduler.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_count(scheduler()) }
}

/// `Coroutine::get_current()`: null outside a coroutine.
#[must_use]
pub fn current() -> Co {
    // SAFETY: this thread's scheduler.
    native(unsafe { (*scheduler()).current })
}

/// `Coroutine::get_current_safe()`: outside a coroutine it raises a fatal
/// error and does not return, so the caller's frames must own nothing that
/// needs a drop.
#[expect(
    clippy::must_use_candidate,
    reason = "some callers want only its fatal error outside a coroutine"
)]
pub fn current_safe() -> Co {
    let co = current();
    if co.is_null() {
        // SAFETY: Swoole's fatal error hook; the frames own nothing.
        unsafe {
            swoole_rs_corebase::fatal_error(
                SW_ERROR_CO_OUT_OF_COROUTINE,
                c"API must be called in the coroutine",
            );
        }
    }
    co
}

pub(crate) fn current_task() -> *mut c_void {
    task(current())
}

pub(crate) fn current_cid() -> c_long {
    let co = current();
    if co.is_null() { -1 } else { cid(co) }
}

pub(crate) fn by_cid(cid: c_long) -> Co {
    // SAFETY: this thread's scheduler.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_get(scheduler(), cid) }
}

/// The task of `co`, null for none.
fn task(co: Co) -> *mut c_void {
    // SAFETY: a live coroutine or null.
    unsafe { state(co).as_ref() }.map_or(std::ptr::null_mut(), |co| co.task)
}

pub(crate) fn task_by_cid(cid: c_long) -> *mut c_void {
    task(by_cid(cid))
}

/// The coroutine `cid`, or the running one for 0.
fn running_or(cid: c_long) -> Co {
    if cid == 0 { current() } else { by_cid(cid) }
}

/// `Coroutine::get_elapsed`.
pub(crate) fn elapsed(cid: c_long) -> c_long {
    let co = running_or(cid);
    // SAFETY: a live coroutine or null.
    unsafe { state(co).as_ref() }.map_or(-1, |co| absolute_msec() - co.init_msec)
}

/// `Coroutine::get_execute_time`.
pub(crate) fn execute_time(cid: c_long) -> c_long {
    let co = running_or(cid);
    if co.is_null() {
        return -1;
    }
    // SAFETY: a live coroutine.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_execute_usec(state(co), now_us()) }
}

/// The live coroutines' ids, newest first, up to `cids.len()`; how many.
pub(crate) fn list(cids: &mut [c_long]) -> usize {
    let mut before = c_long::MAX;
    for (n, slot) in cids.iter_mut().enumerate() {
        // SAFETY: this thread's scheduler and a writable id.
        if unsafe {
            swoole_rs_coroutine::swr_v1_coroutine_older(scheduler(), before, &raw mut before)
        }
        .is_null()
        {
            return n;
        }
        *slot = before;
    }
    cids.len()
}

/// `Co::stats()`.
pub(crate) fn stats(stats: &mut Stats) {
    // SAFETY: C++ fills in the reactor and io_uring counts.
    unsafe { swr_cpp_phpcoroutines_stats(stats) };
    // SAFETY: plain out-parameters.
    unsafe {
        swr_v1_async_threads_stats(
            &mut stats.aio_task_num,
            &mut stats.aio_worker_num,
            &mut stats.aio_queue_size,
        );
    };
    // SAFETY: crates/reactor counts this thread's listeners.
    stats.signal_listener_num = i64::from(unsafe { swoole_signal_get_listener_num() });
    // SAFETY: this thread's scheduler.
    let s = unsafe { &*scheduler() };
    stats.c_stack_size = i64::try_from(s.stack_size).unwrap_or(i64::MAX);
    stats.coroutine_num = i64::try_from(count()).unwrap_or(i64::MAX);
    stats.coroutine_peak_num = i64::try_from(s.peak_num).unwrap_or(i64::MAX);
    stats.coroutine_last_cid = s.last_cid;
}

/// `swoole_test_kernel_coroutine`'s coroutine.
pub(crate) fn kernel_coroutine(count: c_long, sleep: f64) {
    // SAFETY: C++ creates a coroutine that sleeps.
    unsafe { swr_cpp_phpcoroutines_kernel_coroutine(count, sleep) };
}

pub(crate) fn set_task(co: Co, task: *mut c_void) {
    // SAFETY: a live coroutine.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_set_task(state(co), task) };
}

/// The task of `co`'s origin, null for the main context.
pub(crate) fn origin_task(co: Co) -> *mut c_void {
    // SAFETY: a live coroutine, whose origin is live or null.
    task(native(unsafe { (*state(co)).origin }))
}

/// `Coroutine::get_cid()` of a live coroutine.
#[must_use]
pub fn cid(co: Co) -> c_long {
    // SAFETY: a live coroutine.
    unsafe { (*state(co)).cid }
}

pub(crate) fn origin_cid(co: Co) -> c_long {
    // SAFETY: a live coroutine.
    let origin = unsafe { (*state(co)).origin };
    // SAFETY: its origin is live or null.
    unsafe { origin.as_ref() }.map_or(-1, |origin| origin.cid)
}

pub(crate) fn state_of(co: Co) -> c_int {
    // SAFETY: a live coroutine.
    unsafe { (*state(co)).state }
}

/// `co`'s C stack and its size, null under thread contexts.
pub(crate) fn stack(co: Co) -> (*mut c_void, usize) {
    let mut size = 0;
    // SAFETY: a live coroutine and a writable size.
    let stack = unsafe { swr_cpp_phpcoroutines_stack(co, &raw mut size) };
    (stack, size)
}

/// Switch out of `co`, the running coroutine. The caller's frames must own
/// nothing that needs a drop, as for [`resume`].
pub fn yield_(co: Co) {
    // SAFETY: this thread's scheduler and its running coroutine.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_yield(scheduler(), state(co)) };
}

/// [`yield_`], cancelable through `cancel(co, data)`.
/// # Safety
/// `co` is the running coroutine; the caller's frames own nothing that needs a drop.
pub unsafe fn yield_cancelable(
    co: Co,
    cancel: unsafe extern "C-unwind" fn(Co, *mut c_void) -> bool,
    data: *mut c_void,
) {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_coroutine_yield_with_cancel(co, cancel, data) };
}

/// `Coroutine::yield_ex(timeout)`: false when it timed out or was canceled.
pub(crate) fn yield_ex(co: Co, timeout: f64) -> bool {
    // SAFETY: as above.
    unsafe { coroutine_yield_ex(co, timeout) }
}

/// `Coroutine::count()` and `Coroutine::get_peak_num()`.
#[must_use]
pub fn counts() -> (usize, u64) {
    // SAFETY: this thread's scheduler.
    (count(), unsafe { (*scheduler()).peak_num })
}

/// `co->is_suspending()`.
/// # Safety
/// `co` is a live coroutine.
pub unsafe fn is_suspending(co: Co) -> bool {
    // SAFETY: per the caller.
    unsafe { (*state(co)).state == swoole_rs_coroutine::STATE_WAITING }
}

/// `co->yield()`, as [`yield_`]: the caller's frames own nothing that needs a drop.
/// # Safety
/// `co` is the running coroutine.
pub unsafe fn suspend(co: Co) {
    yield_(co);
}

/// `co->yield_ex(timeout)`, as [`yield_ex`].
/// # Safety
/// As [`suspend`].
pub unsafe fn suspend_for(co: Co, timeout: f64) -> bool {
    yield_ex(co, timeout)
}

/// Switch to `co`. The caller's frames must own nothing that needs a drop: a
/// bailout in the coroutine may longjmp across them on the main stack.
pub(crate) fn resume(co: Co) {
    // SAFETY: this thread's scheduler and a live coroutine.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_resume(scheduler(), state(co)) };
}

/// Wake a parked coroutine: `Coroutine::resume()`, or `cancel()` so its yield
/// fails. As [`resume`], the caller's frames must own nothing that needs a drop.
/// # Safety
/// `co` is a live, parked coroutine.
pub unsafe fn wake(co: Co, cancel: bool) {
    if cancel {
        let _canceled = self::cancel(co);
    } else {
        resume(co);
    }
}

/// Running in a coroutine (`swoole_coroutine_is_in`).
#[must_use]
pub fn in_coroutine() -> bool {
    !current().is_null()
}

/// Suspend the running coroutine, published in `slot` while it waits, until
/// [`wake`]. As [`yield_`], the caller's frames must own nothing that needs a drop.
/// # Safety
/// In a coroutine.
pub unsafe fn park(slot: &std::cell::Cell<Co>) {
    let co = current();
    slot.set(co);
    yield_(co);
    slot.set(std::ptr::null_mut());
}

pub(crate) fn cancel(co: Co) -> bool {
    // SAFETY: a live coroutine.
    unsafe { coroutine_cancel(co) }
}

/// `co->is_canceled()` of a live coroutine.
#[must_use]
pub fn is_canceled(co: Co) -> bool {
    // SAFETY: a live coroutine.
    unsafe { (*state(co)).resume_code == RC_CANCELED }
}

pub(crate) fn is_end(co: Co) -> bool {
    // SAFETY: a live coroutine.
    unsafe { swr_cpp_phpcoroutines_is_end(co) != 0 }
}

/// `PHPCoroutine::create`'s `Coroutine::create(main_func, args)`.
pub(crate) fn create(args: *mut c_void) -> c_long {
    // SAFETY: the coroutine's entry, with the creator's live arguments.
    unsafe { swr_cpp_coroutine_create(main, args, std::ptr::null()) }
}

/// `main_func`: the coroutine's function and defer tasks run inside
/// `zend_first_try`; a coroutine that ended in an exception or a bailout
/// bails the whole process out. Its frame owns nothing to drop.
unsafe extern "C" fn main(args: *mut c_void) {
    // SAFETY: on the new coroutine, with its creator's arguments.
    let ctx = unsafe { crate::context::open(args) };
    // SAFETY: Rust's entry and defer loop, over this coroutine's context.
    let caught =
        unsafe { swoole_rs_zend::sys::swr_zend_coroutine_run(Some(enter), Some(defers), ctx) };
    // SAFETY: the context, which nothing uses again.
    unsafe { crate::context::close(ctx) };
    if caught != 0 {
        // SAFETY: C++ bails out to the main context.
        unsafe { swr_cpp_phpcoroutines_bailout() };
    }
}

unsafe extern "C" fn enter(context: *mut c_void) -> *mut swr_zend_callable {
    // SAFETY: the running coroutine's context.
    unsafe { crate::context::enter(context) }
}

/// The defer tasks, last in first out; an exception bails out to the trap.
unsafe extern "C" fn defers(context: *mut c_void) {
    loop {
        // SAFETY: the running coroutine's context.
        let task = unsafe { crate::context::defer_next(context) };
        if task.is_null() {
            return;
        }
        // SAFETY: a kept callable.
        let fci = unsafe { &raw mut (*task).fci };
        // SAFETY: as above.
        let fcc = unsafe { &raw mut (*task).fcc };
        // SAFETY: as above.
        if unsafe { swr_v1_phphelpers_call_anyway(fci, fcc) } != 0 {
            fatal_error(swoole_rs_zend::E_WARNING, c"defer callback handler error");
        }
        // SAFETY: the executor's pending exception, if any.
        if !unsafe { swoole_rs_zend::sys::swr_zend_facade_exception() }.is_null() {
            bailout::raise(Err(bailout::Bailout));
        }
        // SAFETY: the callable's cache, then its emalloc'd holder.
        unsafe { swr_v1_phphelpers_cache_free(fcc) };
        // SAFETY: as above.
        unsafe { swoole_rs_zend::sys::swr_zend_efree(task.cast()) };
    }
}

/// `php_swoole_check_reactor()`.
pub(crate) fn check_reactor() {
    swoole_rs_phpswoole::check_reactor();
}

/// Activation: the reactor's destroy callback, `Coroutine::activate` and the
/// switch callbacks.
pub(crate) fn attach(
    on_yield: unsafe extern "C" fn(*mut c_void),
    on_resume: unsafe extern "C" fn(*mut c_void),
    on_close: unsafe extern "C" fn(*mut c_void),
) {
    // SAFETY: the reactor is running.
    unsafe { swr_cpp_phpcoroutines_activate(deactivate_on_free) };
    set_callbacks([Some(on_yield), Some(on_resume), Some(on_close)]);
}

pub(crate) fn detach() {
    set_callbacks([None; 3]);
}

fn set_callbacks(callbacks: [swoole_rs_coroutine::SwapCallback; 3]) {
    let slots = [
        swoole_rs_coroutine::ON_YIELD,
        swoole_rs_coroutine::ON_RESUME,
        swoole_rs_coroutine::ON_CLOSE,
    ];
    for (slot, callback) in slots.into_iter().zip(callbacks) {
        // SAFETY: this thread's scheduler.
        unsafe { swoole_rs_coroutine::swr_v1_coroutine_set_callback(scheduler(), slot, callback) };
    }
}

/// The reactor's destroy callback.
unsafe extern "C" fn deactivate_on_free(_: *mut c_void) {
    bailout::raise(crate::runtime::deactivate());
}

pub(crate) fn deactivate() {
    // SAFETY: the core is active.
    unsafe { coroutine_deactivate() };
}

/// The runtime hooks (when `flags`) and the unsafe functions disabled.
pub(crate) fn hook(flags: u32) {
    if flags != 0 {
        let _hooked = swoole_rs_phpruntime::php_coroutine_enable_hook(flags);
    }
    swoole_rs_phpruntime::disable_unsafe_function();
}

pub(crate) fn unhook() {
    let _unhooked = swoole_rs_phpruntime::php_coroutine_disable_hook();
    swoole_rs_phpruntime::enable_unsafe_function();
}

/// `\Swoole\Coroutine\deadlock_check()`, inside `zend_try`.
pub(crate) fn deadlock_check() -> Result<(), bailout::Bailout> {
    // SAFETY: the library's global function, with no arguments.
    unsafe { swoole_rs_phphelpers::named_call(b"\\Swoole\\Coroutine\\deadlock_check", &mut []) }
}

/// The `SW_GLOBAL_HOOK_ON_CORO_START` (`stop`: `_STOP`) hook, when set.
pub(crate) fn coro_hook(stop: bool, task: *mut c_void) {
    let kind = HOOK_ON_CORO_START + c_int::from(stop);
    if swoole_rs_corebase::swoole_isset_hook(kind) {
        // SAFETY: the hook's argument is the coroutine's context.
        unsafe { swoole_rs_corebase::swoole_call_hook(kind, task) };
    }
}

/// `swoole::time<std::chrono::microseconds>(true)`: the steady clock.
pub(crate) fn now_us() -> c_long {
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

/// `Timer::get_absolute_msec()`.
pub(crate) fn absolute_msec() -> c_long {
    now_us() / 1000
}

/// `swoole_event_defer(f, data)`.
pub(crate) fn defer(f: unsafe extern "C" fn(*mut c_void), data: *mut c_void) {
    // SAFETY: C++ queues the call on the running loop.
    unsafe { swr_cpp_phpcoroutines_defer(f, data) };
}

/// A one-shot timer calling `f(data)` after `timeout` seconds.
/// # Safety
/// `f` accepts `data` when the timer fires.
#[must_use]
pub unsafe fn timer_add(
    timeout: f64,
    data: *mut c_void,
    f: unsafe extern "C" fn(*mut c_void),
) -> *mut c_void {
    // SAFETY: crates/timer adds the timer.
    unsafe { swr_v1_timer_add_plain_sec(timeout, false, f, data) }
}

/// `swoole_timer_del`.
/// # Safety
/// `timer` is live.
pub unsafe fn timer_del(timer: *mut c_void) {
    // SAFETY: a live timer.
    unsafe { swoole_timer_del(timer) };
}

/// PHP 8.4+ `exit()`: the original handler, false if there is none.
pub(crate) fn exit_original(execute_data: *mut zend_execute_data, return_value: *mut zval) -> bool {
    // SAFETY: the handler's own frame and result.
    unsafe { php_swoole_call_original_handler(c"exit".as_ptr(), 4, execute_data, return_value) }
}

/// `exit()` without the original handler: print `message`, then exit.
pub(crate) fn exit_fallback(message: Option<&[u8]>, status: c_long) {
    if let Some(message) = message {
        // SAFETY: a live buffer.
        unsafe { php_write(message.as_ptr().cast(), message.len()) };
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes the zend_long status as int"
    )]
    // SAFETY: Swoole's exit, which does not return here.
    unsafe {
        sw_php_exit(status as c_int);
    };
}

pub(crate) fn reactor_init() -> i32 {
    // SAFETY: crates/phpevents starts the loop.
    unsafe { php_swoole_reactor_init() }
}

pub(crate) fn event_wait() {
    // SAFETY: crates/phpevents runs the loop.
    unsafe { swr_v1_phpevents_wait() };
}

pub(crate) fn set_global_option(options: *mut zend_array) {
    // SAFETY: a live array.
    unsafe { swoole_rs_phpswoole::php_swoole_set_global_option(options) };
}

/// `Coroutine::set_stack_size(php_swoole_parse_to_size(size))`.
pub(crate) fn set_stack_size(size: *mut zval) {
    // SAFETY: a live zval.
    let size = unsafe { swoole_rs_phpswoole::php_swoole_parse_to_size(size) };
    let size = usize::try_from(size.cast_unsigned()).unwrap_or(usize::MAX);
    // SAFETY: this thread's scheduler.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_set_stack_size(scheduler(), size) };
}

pub(crate) fn add_name_resolver(resolver: *mut zval) {
    // SAFETY: a live zval.
    unsafe { php_swoole_name_resolver_add(resolver) };
}

pub(crate) fn set_dns_cache_expire(expire: c_long) {
    swoole_rs_hooks::dns_cache::swr_v1_hook_dns_cache_expire(expire);
}

pub(crate) fn set_dns_cache_capacity(capacity: c_long) {
    let capacity = usize::try_from(capacity.cast_unsigned()).unwrap_or(usize::MAX);
    swoole_rs_hooks::dns_cache::swr_v1_hook_dns_cache_capacity(capacity);
}

/// Install (`set`) or clear the reactor's user exit condition, which calls
/// `swr_v1_phpcoroutines_can_exit`.
pub(crate) fn set_exit_condition(set: bool) {
    // SAFETY: C++ holds the condition as a std::function.
    unsafe { swr_cpp_phpcoroutines_set_exit_condition(u8::from(set)) };
}

/// `sw_callable_warning`.
pub(crate) use swoole_rs_zend::sys::swr_zend_callable_warning as warning;
