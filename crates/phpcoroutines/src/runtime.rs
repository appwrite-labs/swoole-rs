//! `PHPCoroutine`'s per-thread state: its configuration, activation (the
//! library, reactor, hooks and switch callbacks), coroutine creation, the
//! deadlock check and the preemptive scheduler's interrupt thread.
use crate::{
    context::{self, Args},
    host,
};
use std::{
    cell::{Cell, RefCell},
    ffi::{c_char, c_int, c_long, c_void},
    ptr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use swoole_rs_zend::{
    E_ERROR, E_WARNING,
    bailout::{self, Bailout},
    message,
    sys::{
        swr_zend_coroutine_set_interrupt, swr_zend_interrupt_handler, swr_zend_vm_interrupt_raise,
        swr_zval_is_true, zend_atomic_bool, zend_execute_data, zend_fcall_info_cache,
        zend_get_constant_str, zval,
    },
};

/// `SW_DEFAULT_MAX_CORO_NUM`.
pub(crate) const DEFAULT_MAX_NUM: c_long = 100_000;
/// `PHPCoroutine::MAX_EXEC_MSEC`.
const MAX_EXEC_MSEC: c_long = 10;
/// `Coroutine::ERR_LIMIT` and `ERR_INVALID`.
const ERR_LIMIT: c_long = -1;
const ERR_INVALID: c_long = -2;

/// `PHPCoroutine::Config`.
struct Config {
    max_num: Cell<u64>,
    hook_flags: Cell<u32>,
    preemptive: Cell<bool>,
    deadlock_check: Cell<bool>,
}

struct Interrupt {
    running: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

per_thread! {
    static ACTIVATED: Cell<bool> = Cell::new(false);
    static CONFIG: Config =
        Config {
            max_num: Cell::new(DEFAULT_MAX_NUM.unsigned_abs()),
            hook_flags: Cell::new(0),
            preemptive: Cell::new(false),
            deadlock_check: Cell::new(true),
        };
    static INTERRUPT_RUNNING: Cell<bool> = Cell::new(false);
    static INTERRUPT: RefCell<Option<Interrupt>> = RefCell::new(None);
}

/// How many threads' runtimes are active, and the interrupt function Zend ran
/// before the first of them. `zend_interrupt_function` is process-global: the
/// first activation installs ours and the last deactivation restores Zend's,
/// so no thread saves another's hook as the original or removes it under one
/// still running.
static INTERRUPT_HOOK: Mutex<(usize, swr_zend_interrupt_handler)> = Mutex::new((0, None));

fn interrupt_hook() -> std::sync::MutexGuard<'static, (usize, swr_zend_interrupt_handler)> {
    INTERRUPT_HOOK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The interrupt function Zend ran before activation.
fn original_interrupt() -> swr_zend_interrupt_handler {
    interrupt_hook().1
}

pub(crate) fn activated() -> bool {
    ACTIVATED.with(Cell::get)
}

#[inline]
pub(crate) fn interrupt_running() -> bool {
    INTERRUPT_RUNNING.with(Cell::get)
}

/// `PHPCoroutine::get_cid()`.
#[must_use]
pub fn get_cid() -> c_long {
    if activated() { host::current_cid() } else { -1 }
}

/// `PHPCoroutine::create`: the new coroutine's id or `ERR_*`. Zend fatal
/// errors and the coroutine's own bailouts longjmp across this frame, which
/// owns nothing to drop.
pub(crate) fn create(
    fcc: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    callable: *mut zval,
) -> Result<c_long, Bailout> {
    let count = host::count();
    if count as u64 >= CONFIG.with(|c| c.max_num.get()) {
        host::fatal_error(
            E_WARNING,
            message!("exceed max number of coroutine {count}").as_cstr(),
        );
        return Ok(ERR_LIMIT);
    }
    // SAFETY: null or a resolved callable.
    let handler = unsafe { fcc.as_ref() }.map_or(ptr::null_mut(), |fcc| fcc.function_handler);
    if handler.is_null() {
        host::fatal_error(E_ERROR, c"invalid function call info cache");
        return Ok(ERR_INVALID);
    }
    // SAFETY: a resolved callable's handler; every zend_function starts with
    // its type, a byte.
    let kind = unsafe { handler.cast::<u8>().read() };
    if kind != context::USER_FUNCTION && kind != context::INTERNAL_FUNCTION {
        host::fatal_error(E_ERROR, message!("invalid function type {kind}").as_cstr());
        return Ok(ERR_INVALID);
    }
    if !activated() {
        activate()?;
    }
    let mut args = Args {
        fcc,
        argv,
        argc,
        callable,
    };
    context::save_current();
    let cid = host::create((&raw mut args).cast());
    bailout::trapped(0)?;
    Ok(cid)
}

/// `PHPCoroutine::activate`.
fn activate() -> Result<(), Bailout> {
    // SAFETY: a NUL-free constant name.
    let library = unsafe { zend_get_constant_str(c"SWOOLE_LIBRARY".as_ptr(), 14) };
    // SAFETY: null or a live constant.
    if library.is_null() || !unsafe { swr_zval_is_true(library) } {
        swoole_rs_phpswoole::load_library()?;
    }
    host::check_reactor();
    let mut hook = interrupt_hook();
    if hook.0 == 0 {
        // SAFETY: replaces Zend's interrupt function, restored by the last `deactivate`.
        hook.1 = unsafe { swr_zend_coroutine_set_interrupt(Some(interrupt_function)) };
    }
    hook.0 += 1;
    drop(hook);
    if host::flag(host::PREEMPTIVE) || CONFIG.with(|c| c.preemptive.get()) {
        interrupt_start();
    }
    if host::flag(host::MAIN_THREAD) {
        let flags = CONFIG.with(|c| c.hook_flags.get());
        host::hook(flags);
    }
    host::attach(context::on_yield, context::on_resume, context::on_close);
    ACTIVATED.with(|a| a.set(true));
    Ok(())
}

/// `PHPCoroutine::deactivate`, also the reactor's destroy callback.
pub(crate) fn deactivate() -> Result<(), Bailout> {
    if !activated() {
        return Ok(());
    }
    ACTIVATED.with(|a| a.set(false));
    interrupt_stop();
    if host::flag(host::MAIN_THREAD) {
        host::unhook();
    }
    host::detach();
    let mut hook = interrupt_hook();
    hook.0 = hook.0.saturating_sub(1);
    if hook.0 == 0 {
        // SAFETY: restores Zend's interrupt function.
        unsafe { swr_zend_coroutine_set_interrupt(hook.1) };
    }
    drop(hook);
    if CONFIG.with(|c| c.deadlock_check.get()) {
        deadlock_check()?;
    }
    host::deactivate();
    Ok(())
}

unsafe extern "C" {
    fn printf(format: *const c_char, ...) -> c_int;
}

/// `PHPCoroutine::deadlock_check`: report coroutines still asleep. Runs no
/// asynchronous code (the library's checker only prints).
fn deadlock_check() -> Result<(), Bailout> {
    let count = host::count();
    if count == 0 || host::flag(host::FATAL) {
        return Ok(());
    }
    if host::flag(host::LIBRARY) {
        return host::deadlock_check();
    }
    // Upstream's printf, so stdio buffers it as it did.
    // SAFETY: a format taking one unsigned long.
    unsafe {
        printf(
            c"\n ===================================================================\n  [FATAL ERROR]: all coroutines (count: %lu) are asleep - deadlock! \n ===================================================================\n".as_ptr(),
            count as std::ffi::c_ulong,
        );
    }
    Ok(())
}

/// `PHPCoroutine::shutdown` (RSHUTDOWN): 1 if deactivation bailed out.
pub(crate) fn shutdown() -> bool {
    let bailed = activated() && deactivate().is_err();
    if !bailed {
        crate::scheduler::free_options();
        context::free_main();
    }
    bailed
}

/// `coro_interrupt_function`: yield a coroutine that ran past its slice,
/// then run the interrupt function Zend had.
unsafe extern "C" fn interrupt_function(execute_data: *mut zend_execute_data) {
    let task = context::current();
    let (co, enabled, last) = (task.co(), task.enable_scheduler(), task.last_msec());
    if !co.is_null()
        && interrupt_running()
        && enabled
        && host::absolute_msec() - last > MAX_EXEC_MSEC
    {
        host::defer(InterruptResume(co));
        host::yield_(co);
    }
    if let Some(original) = original_interrupt() {
        let body = || {
            // SAFETY: Zend's previous interrupt function, over the frame Zend passed.
            unsafe { original(execute_data) };
        };
        // SAFETY: the closure owns nothing; a bailout in it is re-raised below.
        if unsafe { bailout::trap_with(body) }.is_err() {
            raise();
        }
    }
}

/// `coro_interrupt_resume`: the loop resumes the coroutine the interrupt yielded.
struct InterruptResume(*mut c_void);

impl swoole_rs_reactor::event_api::Deferred for InterruptResume {
    fn run(self) {
        let co = self.0;
        if !co.is_null() && !host::is_end(co) {
            host::resume(co);
        }
    }
    fn dispose(self) {}
}

/// Re-raise a trapped bailout from a handler's outermost Rust frame, which
/// owns nothing to drop (docs/zend.md).
pub(crate) fn raise() {
    Bailout.defer();
    bailout::finish();
}

unsafe extern "C" {
    /// `&EG(vm_interrupt)`: the address of this PHP thread's interrupt flag. It
    /// takes nothing from its caller and runs no PHP code.
    safe fn swr_zend_vm_interrupt() -> *mut zend_atomic_bool;
}

/// A pointer the interrupt thread may hold: `EG(vm_interrupt)` of this
/// thread's executor, which outlives the interrupt thread.
struct Flag(*mut zend_atomic_bool);
// SAFETY: Zend's atomic flag, written with an atomic store.
unsafe impl Send for Flag {}

fn interrupt_start() {
    if interrupt_running() {
        return;
    }
    let running = Arc::new(AtomicBool::new(true));
    let flag = Flag(swr_zend_vm_interrupt());
    let shared = Arc::clone(&running);
    // The thread inherits a fully blocked mask (upstream's `interrupt_thread_start`).
    let spawned = swoole_rs_corebase::with_signals_blocked(|| {
        thread::Builder::new().spawn(move || {
            let flag = flag;
            swoole_rs_corebase::signal_block_all();
            let half = Duration::from_millis(MAX_EXEC_MSEC.unsigned_abs() / 2);
            while shared.load(Ordering::Acquire) {
                // SAFETY: the executor's flag, live until `interrupt_stop` joins.
                unsafe { swr_zend_vm_interrupt_raise(flag.0) };
                thread::sleep(half);
            }
        })
    });
    if let Ok(thread) = spawned {
        INTERRUPT.with(|i| *i.borrow_mut() = Some(Interrupt { running, thread }));
        INTERRUPT_RUNNING.with(|r| r.set(true));
    }
}

/// `interrupt_thread_stop`.
fn interrupt_stop() {
    if !interrupt_running() {
        return;
    }
    INTERRUPT_RUNNING.with(|r| r.set(false));
    if let Some(interrupt) = INTERRUPT.with(|i| i.borrow_mut().take()) {
        interrupt.running.store(false, Ordering::Release);
        drop(interrupt.thread.join());
    }
}

/// `PHPCoroutine::get_hook_flags()`.
#[must_use]
pub fn hook_flags() -> u32 {
    CONFIG.with(|c| c.hook_flags.get())
}

pub(crate) fn store_hook_flags(flags: u32) {
    CONFIG.with(|c| c.hook_flags.set(flags));
}

pub(crate) fn set_max_num(max: u64) {
    CONFIG.with(|c| c.max_num.set(max));
}

pub(crate) fn set_deadlock_check(enable: bool) {
    CONFIG.with(|c| c.deadlock_check.set(enable));
}

pub(crate) fn set_preemptive(enable: bool) {
    CONFIG.with(|c| c.preemptive.set(enable));
}

/// `PHPCoroutine::create`, the coroutine leaf the callers of
/// `swoole_rs_zend::host::function_call` and the zend-sys shims pass (C ABI).
/// # Safety
/// As `zend_call_function`'s callable; `bailout` is writable.
pub unsafe extern "C" fn create_coroutine(
    fcc: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    callable: *mut zval,
    bailout: *mut u8,
) -> c_long {
    let created = create(fcc, argc, argv, callable);
    // SAFETY: per the caller.
    unsafe { bailout.write(u8::from(created.is_err())) };
    created.unwrap_or(ERR_INVALID)
}

/// `zend::function::call(&fcc, argc, argv, nullptr, coroutine)` of a kept
/// callable with its bound arguments: whether it ran, warning as
/// `sw_callable_warning` when it could not.
/// # Errors
/// PHP bailed out.
pub fn call_kept(callable: &swoole_rs_zend::Callable, coroutine: bool) -> Result<bool, Bailout> {
    let (cache, argc, argv, _) = callable.parts();
    // SAFETY: a kept callable's cache and arguments, which its owner keeps
    // live through the call; the leaves are this crate's and C's.
    unsafe {
        swoole_rs_zend::host::function_call(
            cache,
            argc,
            argv,
            ptr::null_mut(),
            coroutine,
            create_coroutine,
            swoole_rs_zend::host::NO_THEN,
        )
    }
}

/// `zend::function::call(&cache, args, retval, coroutine)` of a
/// `sw_callable_create` handler: whether it ran, warning as
/// `sw_callable_warning` when it could not.
/// # Errors
/// PHP bailed out.
pub fn call_handler(
    handler: &swoole_rs_zend::host::callable::Callable,
    args: &[swoole_rs_zend::Owned],
    retval: &swoole_rs_zend::Owned,
    coroutine: bool,
) -> Result<bool, Bailout> {
    let argc = u32::try_from(args.len()).unwrap_or(u32::MAX);
    // SAFETY: the handler's resolved cache, which its owner keeps live through
    // the call, live arguments and a return slot; the leaves are this crate's
    // and C's.
    unsafe {
        swoole_rs_zend::host::function_call(
            handler.cache_ptr(),
            argc,
            args.as_ptr().cast_mut().cast(),
            retval.as_ptr(),
            coroutine,
            create_coroutine,
            swoole_rs_zend::host::NO_THEN,
        )
    }
}
