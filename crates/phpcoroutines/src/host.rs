//! What PHP coroutines ask of the rest of Swoole: the coroutine core's
//! scheduler and records (crates/coroutine), the module host, the runtime
//! hooks, the event loop and the server.
use crate::context::Ctx;
use std::ffi::{CStr, c_int, c_long, c_void};
use swoole_rs_corebase::CCast;
use swoole_rs_coroutine::RC_CANCELED;
use swoole_rs_zend::{
    E_ERROR, bailout,
    sys::{swr_zend_callable, zend_execute_data, zval},
};

/// `Co::stats()`'s native counts.
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
    pub(crate) iouring: bool,
    pub(crate) iouring_task_num: i64,
    pub(crate) iouring_sq_usage_percent: i64,
    pub(crate) iouring_waiting_task_num: i64,
}

/// A `swoole::Coroutine`, which starts with its `swoole_rs_coroutine::Coroutine`.
pub(crate) type Co = *mut c_void;

use swoole_rs_hooks::sleep::system_sleep;

// Declared, since crates/phpevents (its `php_swoole_reactor_init` and
// `swr_v1_phpevents_wait`) depends on this crate, and PHP's `php_write`.
unsafe extern "C" {
    // crates/phpevents, crates/phpclients and PHP.
    fn php_write(buf: *const c_void, size: usize) -> usize;
    safe fn php_swoole_reactor_init() -> i32;
    safe fn swr_v1_phpevents_wait();
}

/// The globals, server state and build switches [`flag`] reads.
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
    let g = swoole_rs_phpswoole::globals();
    match bit {
        CLI => g.cli.get(),
        SHORTNAME => g.use_shortname.get(),
        LIBRARY => g.enable_library.get(),
        FIBER_MOCK => g.enable_fiber_mock.get(),
        PREEMPTIVE => g.enable_preemptive_scheduler.get(),
        SHUTDOWN_FUNCTIONS => {
            g.req_status.get() == swoole_rs_phpswoole::CALL_USER_SHUTDOWNFUNC_BEGIN
        }
        // SAFETY: the process's server, read once here.
        SERVER_STARTED => unsafe { swoole_rs_server::view::Server::current() }
            .is_some_and(swoole_rs_server::view::Server::is_started),
        FATAL => {
            swoole_rs_zend::last_fatal_error()
                || swoole_rs_reactor::event_api::swoole_event_bailed_out()
        }
        // `sw_is_main_thread()`.
        MAIN_THREAD => !cfg!(zts) || swoole_rs_zend::is_main_thread(),
        // Only SW_CORO_TIME builds give the scheduler a clock.
        CORO_TIME => swoole_rs_coroutine::counters().timed,
        THREAD => swoole_rs_phpswoole::built_with(swoole_rs_phpswoole::Switch::Thread),
        _ => false,
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
    swoole_rs_zend::report(level, message)
}

/// `php_swoole_error_ex(level, code, "%s", message)`. A bailout of the
/// error's handler is deferred for the caller's handler to raise.
pub(crate) fn error_ex(level: c_int, code: c_int, message: &CStr) {
    set_last_error(code);
    if swoole_rs_phpswoole::display_errors() || level == E_ERROR {
        bailout::defer(swoole_rs_zend::report(level, message));
    }
}

/// The Rust state `co` starts with.
fn state(co: Co) -> *mut swoole_rs_coroutine::Coroutine {
    co.cast()
}

/// The Rust state of `co`, a live coroutine, borrowed for one read.
fn live(co: Co) -> &'static swoole_rs_coroutine::Coroutine {
    // SAFETY: callers pass a live coroutine, which starts with its state.
    unsafe { &*state(co) }
}

/// The Rust state `co` points at, a live coroutine's or null, borrowed for one read.
fn live_or_none(
    co: *const swoole_rs_coroutine::Coroutine,
) -> Option<&'static swoole_rs_coroutine::Coroutine> {
    // SAFETY: callers pass a live coroutine or null.
    unsafe { co.as_ref() }
}

/// The facade of `co`'s Rust state, null for none.
fn native(co: *const swoole_rs_coroutine::Coroutine) -> Co {
    live_or_none(co).map_or(std::ptr::null_mut(), |co| co.native)
}

pub(crate) fn count() -> usize {
    swoole_rs_coroutine::coroutine_count()
}

/// `Coroutine::get_current()`: null outside a coroutine.
pub(crate) fn current() -> Co {
    swoole_rs_coroutine::current_native()
}

/// `Coroutine::get_current_safe()`: outside a coroutine it raises a fatal
/// error and does not return, so the caller's frames must own nothing that
/// needs a drop.
pub(crate) fn current_safe() -> Co {
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

/// `System::sleep(seconds)` of the running coroutine: 0, or -1 once canceled,
/// and -1 outside a coroutine, where upstream's raises a fatal error.
#[must_use]
pub fn sleep(seconds: f64) -> c_int {
    if current().is_null() {
        return -1;
    }
    system_sleep(seconds)
}

pub(crate) fn current_task() -> Option<Ctx> {
    task(current())
}

pub(crate) fn current_cid() -> c_long {
    let co = current();
    if co.is_null() { -1 } else { cid(co) }
}

pub(crate) fn by_cid(cid: c_long) -> Co {
    swoole_rs_coroutine::coroutine_get(cid)
}

/// The context of `co`, none for none.
fn task(co: Co) -> Option<Ctx> {
    let task = live_or_none(state(co))?.task;
    // SAFETY: a coroutine's task is null or its context, set by `context::open`
    // and live until `context::on_close`, which runs as the coroutine ends.
    unsafe { Ctx::of(task) }
}

pub(crate) fn task_by_cid(cid: c_long) -> Option<Ctx> {
    task(by_cid(cid))
}

/// The coroutine `cid`, or the running one for 0.
fn running_or(cid: c_long) -> Co {
    if cid == 0 { current() } else { by_cid(cid) }
}

/// `Coroutine::get_elapsed`.
pub(crate) fn elapsed(cid: c_long) -> c_long {
    let co = running_or(cid);
    live_or_none(state(co)).map_or(-1, |co| absolute_msec() - co.init_msec)
}

/// `Coroutine::get_execute_time`.
pub(crate) fn execute_time(cid: c_long) -> c_long {
    let co = running_or(cid);
    live_or_none(state(co)).map_or(-1, |co| co.execute_usec(now_us()))
}

/// The live coroutines' ids, newest first, up to `cids.len()`; how many.
pub(crate) fn list(cids: &mut [c_long]) -> usize {
    let mut before = c_long::MAX;
    for (n, slot) in cids.iter_mut().enumerate() {
        let Some((cid, _)) = swoole_rs_coroutine::coroutine_older(before) else {
            return n;
        };
        before = cid;
        *slot = cid;
    }
    cids.len()
}

/// `Co::stats()`.
pub(crate) fn stats(stats: &mut Stats) {
    if let Some(count) = swoole_rs_reactor::event_api::swoole_event_num() {
        stats.event_num = i64::try_from(count).unwrap_or(i64::MAX);
    }
    #[cfg(feature = "iouring")]
    iouring_stats(stats);
    swoole_rs_hooks::async_threads::swr_v1_async_threads_stats(
        &mut stats.aio_task_num,
        &mut stats.aio_worker_num,
        &mut stats.aio_queue_size,
    );
    // crates/reactor counts this thread's listeners.
    stats.signal_listener_num =
        i64::from(swoole_rs_reactor::signal_upstream::swoole_signal_get_listener_num());
    let s = swoole_rs_coroutine::counters();
    stats.c_stack_size = i64::try_from(s.stack_size).unwrap_or(i64::MAX);
    stats.coroutine_num = i64::try_from(count()).unwrap_or(i64::MAX);
    stats.coroutine_peak_num = i64::try_from(s.peak_num).unwrap_or(i64::MAX);
    stats.coroutine_last_cid = s.last_cid;
}

/// The `io_uring` engine's counts, once the thread has one.
#[cfg(feature = "iouring")]
fn iouring_stats(stats: &mut Stats) {
    let Some((tasks, waiting, percent)) = swoole_rs_hooks::iouring::stats() else {
        return;
    };
    stats.iouring = true;
    stats.iouring_task_num = i64::from(tasks);
    let percent = percent.c_cast::<i64>();
    stats.iouring_sq_usage_percent = percent;
    stats.iouring_waiting_task_num = i64::try_from(waiting).unwrap_or(i64::MAX);
}

/// `swoole_test_kernel_coroutine`'s coroutine: sleeps `count` times.
pub(crate) fn kernel_coroutine(count: c_long, sleep: f64) {
    /// The coroutine's body, which copies its arguments before its first switch.
    /// # Safety
    /// `args` is the creator's `(count, sleep)`, live until this coroutine first yields.
    unsafe extern "C" fn sleeper(args: *mut c_void) {
        // SAFETY: per the caller.
        let (count, sleep) = unsafe { *args.cast::<(c_long, f64)>() };
        for _ in 0..count {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "upstream ignores each sleep's result"
            )]
            let _ = super::sleep(sleep);
        }
    }
    let mut args = (count, sleep);
    // SAFETY: the coroutine runs at once and reads `args` before it yields.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_create(sleeper, (&raw mut args).cast()) };
}

/// The context of `co`'s origin, none for the main context.
pub(crate) fn origin_task(co: Co) -> Option<Ctx> {
    task(native(live(co).origin))
}

/// `Coroutine::get_cid()` of a live coroutine.
pub(crate) fn cid(co: Co) -> c_long {
    live(co).cid
}

pub(crate) fn origin_cid(co: Co) -> c_long {
    live_or_none(live(co).origin).map_or(-1, |origin| origin.cid)
}

pub(crate) fn state_of(co: Co) -> swoole_rs_coroutine::State {
    live(co).state
}

/// `co`'s C stack and its size, null under thread contexts.
pub(crate) fn stack(co: Co) -> (*mut c_void, usize) {
    // SAFETY: a live coroutine's record.
    let (stack, size) = unsafe {
        (*co.cast::<swoole_rs_coroutine::context::Native>())
            .ctx
            .stack()
    };
    (stack.cast(), size)
}

/// Switch out of `co`, the running coroutine. The caller's frames must own
/// nothing that needs a drop, as for [`resume`].
pub(crate) fn yield_(co: Co) {
    // SAFETY: this thread's running coroutine; its frames own nothing that
    // needs a drop, as this function's contract says.
    unsafe { swoole_rs_coroutine::suspend(co) };
}

/// `Coroutine::yield_ex(timeout)`: false when it timed out or was canceled.
pub(crate) fn yield_ex(co: Co, timeout: f64) -> bool {
    swoole_rs_coroutine::yield_resumed(wait(co, timeout))
}

/// [`yield_ex`] without its last error: the resume code (`RC_TIMEDOUT`,
/// `RC_CANCELED` or another when resumed).
pub(crate) fn wait(co: Co, timeout: f64) -> i32 {
    // SAFETY: callers pass the running coroutine, and their frames own nothing
    // that needs a drop (as for `yield_`).
    unsafe { swoole_rs_coroutine::yield_timed(co, timeout) }
}

/// `Coroutine::count()` and `Coroutine::get_peak_num()`.
#[must_use]
pub fn counts() -> (usize, u64) {
    (count(), swoole_rs_coroutine::counters().peak_num)
}

/// Switch to `co`. The caller's frames must own nothing that needs a drop: a
/// bailout in the coroutine may longjmp across them on the main stack.
pub(crate) fn resume(co: Co) {
    // SAFETY: callers pass a live, suspended coroutine.
    unsafe { swoole_rs_coroutine::resume(co) };
}

/// A coroutine by id, the handle other crates hold. A coroutine's record is
/// freed when its function returns, which no holder controls, so each access
/// looks the id up: once the coroutine has ended, its handle reads `None` and
/// wakes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Coroutine(c_long);

impl Coroutine {
    /// The running coroutine, `None` outside one.
    #[must_use]
    pub fn current() -> Option<Self> {
        let co = current();
        (!co.is_null()).then(|| Self(cid(co)))
    }

    /// `Coroutine::get_cid()`.
    #[must_use]
    pub const fn cid(self) -> c_long {
        self.0
    }

    /// Its record, while it lives.
    fn live(self) -> Option<Co> {
        let co = by_cid(self.0);
        (!co.is_null()).then_some(co)
    }

    /// `co->is_suspending()`, `None` once it ended.
    #[must_use]
    pub fn is_suspending(self) -> Option<bool> {
        self.live()
            .map(|co| state_of(co) == swoole_rs_coroutine::State::Waiting)
    }

    /// Wake it from a wait: `Coroutine::resume()`, or `cancel()` so its yield
    /// fails; nothing once it ended. As [`resume`], the caller's frames must
    /// own nothing that needs a drop.
    /// # Safety
    /// While it lives, it waits in a suspension of the caller's, which
    /// nothing else wakes.
    pub unsafe fn wake(self, cancel: bool) {
        let Some(co) = self.live() else { return };
        if cancel {
            let _canceled = self::cancel(co);
        } else {
            resume(co);
        }
    }
}

/// `co->yield()` of the running coroutine, until it is woken; nothing outside
/// one. As [`yield_`], the caller's frames must own nothing that needs a drop.
pub fn suspend() {
    let co = current();
    if !co.is_null() {
        yield_(co);
    }
}

/// `co->yield_ex(timeout)` of the running coroutine, as [`suspend`]: false
/// when it timed out or was canceled, or outside a coroutine.
#[must_use]
pub fn suspend_for(timeout: f64) -> bool {
    let co = current();
    !co.is_null() && yield_ex(co, timeout)
}

/// One wait that someone else ends: [`Waiter::wait`] suspends the running
/// coroutine, published here while it waits, and [`Waiter::wake`] resumes it.
#[derive(Default)]
pub struct Waiter(std::cell::Cell<Option<Coroutine>>);

impl Waiter {
    /// [`suspend`] until [`Waiter::wake`], or whatever else ends the wait.
    pub fn wait(&self) {
        self.0.set(Coroutine::current());
        suspend();
        self.0.set(None);
    }

    /// Resume the coroutine in [`Waiter::wait`]; nothing when none waits. As
    /// [`resume`], the caller's frames must own nothing that needs a drop.
    pub fn wake(&self) {
        if let Some(coroutine) = self.0.get() {
            // SAFETY: published only while it is suspended in `wait`, which
            // clears it as soon as anything resumes it.
            unsafe { coroutine.wake(false) };
        }
    }
}

/// Running in a coroutine (`swoole_coroutine_is_in`).
#[must_use]
pub fn in_coroutine() -> bool {
    !current().is_null()
}

pub(crate) fn cancel(co: Co) -> bool {
    // SAFETY: a live coroutine.
    unsafe { swoole_rs_coroutine::cancel(co.cast()) }
}

/// `co->is_canceled()` of a live coroutine.
pub(crate) fn is_canceled(co: Co) -> bool {
    live(co).resume_code == RC_CANCELED
}

/// `co->is_end()`: a context that ended is closed and freed before anything
/// else runs, so a live coroutine has ended only while it closes.
pub(crate) fn is_end(co: Co) -> bool {
    state_of(co) == swoole_rs_coroutine::State::End
}

/// `PHPCoroutine::create`'s `Coroutine::create(main_func, args)`.
pub(crate) fn create(args: *mut c_void) -> c_long {
    // SAFETY: the coroutine's entry, with the creator's live arguments.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_create(main, args) }
}

/// `main_func`: the coroutine's function and defer tasks run inside
/// `zend_first_try`; a coroutine that ended in an exception or a bailout
/// bails the whole process out. Its frame owns nothing to drop.
unsafe extern "C" fn main(args: *mut c_void) {
    // SAFETY: on the new coroutine, with its creator's arguments.
    let Some(ctx) = (unsafe { crate::context::open(args) }) else {
        return;
    };
    let context = ctx.ptr().cast();
    // SAFETY: Rust's entry and defer loop, over this coroutine's context.
    let caught =
        unsafe { swoole_rs_zend::sys::swr_zend_coroutine_run(Some(enter), Some(defers), context) };
    crate::context::close(ctx);
    if caught != 0 {
        // SAFETY: the main context runs `bail_out`; this frame owns nothing.
        unsafe { swoole_rs_coroutine::swr_v1_coroutine_bail(Some(bail_out), std::ptr::null_mut()) };
    }
}

/// The bailout of a coroutine that ended in an exception or a bailout, in the
/// main context: the loop stops, then PHP bails out.
extern "C" fn bail_out(_: *mut c_void) {
    swoole_rs_reactor::event_api::swoole_event_bail_out();
    bailout::raise(Err(bailout::Bailout));
}

/// The running coroutine's entry, whose `context` `main` passes.
extern "C" fn enter(_context: *mut c_void) -> *mut swr_zend_callable {
    current_task().map_or(std::ptr::null_mut(), crate::context::enter)
}

/// The defer tasks, last in first out; an exception bails out to the trap.
extern "C" fn defers(_context: *mut c_void) {
    let Some(ctx) = current_task() else {
        return;
    };
    while let Some((task, returned)) = crate::context::defer_next(ctx) {
        let (fcc, mut argc, mut argv, _) = task.parts();
        if let Some(returned) = returned {
            (argc, argv) = (1, returned);
        }
        // The call's own frame: the task keeps its arguments, which its release drops.
        let mut fci = swoole_rs_zend::EMPTY_FCI;
        fci.size = size_of::<swoole_rs_zend::sys::zend_fcall_info>();
        fci.param_count = argc;
        fci.params = argv;
        // SAFETY: a kept callable's resolved cache, over its kept arguments or the
        // context's live return value.
        if unsafe { swoole_rs_zend::host::call_anyway(&raw mut fci, fcc) } != 0 {
            fatal_error(swoole_rs_zend::E_WARNING, c"defer callback handler error");
        }
        if !swoole_rs_zend::host::exception().is_null() {
            bailout::raise(Err(bailout::Bailout));
        }
        bailout::raise(task.release());
    }
}

/// `php_swoole_check_reactor()`.
pub(crate) fn check_reactor() {
    swoole_rs_phpswoole::check_reactor();
}

/// Activation: the reactor's destroy callback, `Coroutine::activate` and the
/// switch callbacks.
pub(crate) fn attach(
    on_yield: swoole_rs_coroutine::Routine,
    on_resume: swoole_rs_coroutine::Routine,
    on_close: swoole_rs_coroutine::Routine,
) {
    // Upstream dereferences `sw_reactor()` here unchecked.
    if swoole_rs_reactor::event_api::current_prepared().is_none() {
        swoole_rs_corebase::fatal("no reactor for the coroutine host");
    }
    swoole_rs_reactor::event_api::on_destroy(deactivate_on_free);
    // The core's activation, on a running loop.
    swoole_rs_coroutine::activate();
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
        swoole_rs_coroutine::set_callback(slot, callback);
    }
}

/// The reactor's destroy callback.
extern "C-unwind" fn deactivate_on_free(_: *mut c_void) {
    bailout::raise(crate::runtime::deactivate());
}

pub(crate) fn deactivate() {
    // The core is active.
    swoole_rs_coroutine::deactivate();
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
    swoole_rs_zend::host::named_call(b"\\Swoole\\Coroutine\\deadlock_check", &[])
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
    #[cfg(target_vendor = "apple")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_vendor = "apple"))]
    let clock = libc::CLOCK_MONOTONIC;
    swoole_rs_corebase::sys::clock_gettime(clock)
        .map_or(0, |now| now.tv_sec * 1_000_000 + now.tv_nsec / 1000)
}

/// `Timer::get_absolute_msec()`.
pub(crate) fn absolute_msec() -> c_long {
    now_us() / 1000
}

/// `swoole_event_defer` of `value`.
pub(crate) fn defer(value: impl swoole_rs_reactor::event_api::Deferred) {
    // Upstream dereferences `sw_reactor()` here unchecked.
    if swoole_rs_reactor::event_api::swoole_event_defer_value(value).is_err() {
        swoole_rs_corebase::fatal("no reactor to defer to");
    }
}

/// PHP 8.4+ `exit()`: the original handler, false if there is none.
pub(crate) fn exit_original(execute_data: *mut zend_execute_data, return_value: *mut zval) -> bool {
    let Some(handler) = swoole_rs_phpruntime::original(b"exit") else {
        return false;
    };
    // SAFETY: the handler's own frame and result.
    unsafe { handler(execute_data, return_value) };
    true
}

/// `exit()` without the original handler: print `message`, then exit.
pub(crate) fn exit_fallback(message: Option<&[u8]>, status: c_long) {
    if let Some(message) = message {
        // SAFETY: a live buffer.
        unsafe { php_write(message.as_ptr().cast(), message.len()) };
    }
    // Swoole's exit, which does not return here.
    swoole_rs_phpswoole::sw_php_exit(status.c_cast::<c_int>());
}

pub(crate) fn reactor_init() -> i32 {
    php_swoole_reactor_init()
}

pub(crate) fn event_wait() {
    swr_v1_phpevents_wait();
}

/// `Coroutine::set_stack_size(php_swoole_parse_to_size(size))`.
pub(crate) fn set_stack_size(size: &swoole_rs_zend::Value) {
    let size = swoole_rs_phpswoole::php_swoole_parse_to_size(size);
    let size = usize::try_from(size.cast_unsigned()).unwrap_or(usize::MAX);
    swoole_rs_coroutine::set_stack_size(size);
}

pub(crate) fn add_name_resolver(resolver: &swoole_rs_zend::Value) {
    if let Some(families) = swoole_rs_phpswoole::families() {
        (families.name_resolver_add)(resolver);
    }
}

pub(crate) fn set_dns_cache_expire(expire: c_long) {
    swoole_rs_hooks::dns_cache::swr_v1_hook_dns_cache_expire(expire);
}

pub(crate) fn set_dns_cache_capacity(capacity: c_long) {
    let capacity = usize::try_from(capacity.cast_unsigned()).unwrap_or(usize::MAX);
    swoole_rs_hooks::dns_cache::swr_v1_hook_dns_cache_capacity(capacity);
}

/// `Reactor::EXIT_CONDITION_USER_AFTER_DEFAULT`.
const EXIT_CONDITION_USER_AFTER_DEFAULT: c_int = 1000;

/// Install (`set`) or clear this thread's reactor's user exit condition,
/// which calls `Co::set`'s `exit_condition`; nothing without a reactor.
pub(crate) fn set_exit_condition(set: bool) {
    if swoole_rs_corebase::thread_reactor().is_null() {
        return;
    }
    if set {
        swoole_rs_reactor::swr_v1_event_set_exit_condition(
            EXIT_CONDITION_USER_AFTER_DEFAULT,
            crate::scheduler::reactor_can_exit,
        );
    } else {
        swoole_rs_reactor::swr_v1_event_remove_exit_condition(EXIT_CONDITION_USER_AFTER_DEFAULT);
    }
}
