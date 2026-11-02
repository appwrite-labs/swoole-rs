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
        Arc,
        atomic::{AtomicBool, AtomicPtr, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use swoole_rs_zend::{
    E_ERROR, E_WARNING,
    bailout::{self, Bailout},
    message,
    sys::{
        swr_zend_coroutine_set_interrupt, swr_zend_vm_interrupt, swr_zend_vm_interrupt_raise,
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

/// The interrupt function Zend ran before activation (process-global, as
/// `zend_interrupt_function` is).
static ORIGINAL_INTERRUPT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

pub(crate) fn activated() -> bool {
    ACTIVATED.with(Cell::get)
}

#[inline]
pub(crate) fn interrupt_running() -> bool {
    INTERRUPT_RUNNING.with(Cell::get)
}

/// `PHPCoroutine::get_cid()`.
pub(crate) fn cid() -> c_long {
    if activated() {
        // SAFETY: C++ reads the running coroutine.
        unsafe { (host::ops().current_cid)() }
    } else {
        -1
    }
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
    // SAFETY: a resolved callable's handler.
    let function = unsafe { &*handler };
    // SAFETY: every zend_function starts with its type.
    let kind = unsafe { function.type_ };
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
    let ctx = context::current();
    // SAFETY: the running context.
    unsafe { swoole_rs_zend::sys::swr_zend_coroutine_save(context::vm(ctx)) };
    // SAFETY: the new coroutine reads `args` before this frame resumes.
    Ok(unsafe { (host::ops().create)((&raw mut args).cast()) })
}

/// `PHPCoroutine::activate`.
fn activate() -> Result<(), Bailout> {
    // SAFETY: a NUL-free constant name.
    let library = unsafe { zend_get_constant_str(c"SWOOLE_LIBRARY".as_ptr(), 14) };
    // SAFETY: null or a live constant.
    if library.is_null() || !unsafe { swr_zval_is_true(library) } {
        // SAFETY: C++'s loader, a body that runs PHP.
        unsafe { bailout::trap(host::ops().load_library, ptr::null_mut()) }?;
    }
    // SAFETY: C++ starts the reactor.
    unsafe { (host::ops().check_reactor)() };
    // SAFETY: replaces Zend's interrupt function, restored by `deactivate`.
    let original = unsafe { swr_zend_coroutine_set_interrupt(Some(interrupt_function)) };
    ORIGINAL_INTERRUPT.store(
        original.map_or(ptr::null_mut(), |f| f as *mut c_void),
        Ordering::Relaxed,
    );
    if host::flag(host::PREEMPTIVE) || CONFIG.with(|c| c.preemptive.get()) {
        interrupt_start();
    }
    if host::flag(host::MAIN_THREAD) {
        let flags = CONFIG.with(|c| c.hook_flags.get());
        // SAFETY: C++ enables the hooks and disables unsafe functions.
        unsafe { (host::ops().hook)(flags) };
    }
    // SAFETY: C++ registers the reactor callback and the switch callbacks.
    unsafe {
        (host::ops().attach)(context::on_yield, context::on_resume, context::on_close);
    }
    ACTIVATED.with(|a| a.set(true));
    Ok(())
}

/// `PHPCoroutine::deactivate`, the reactor's destroy callback: 1 if the
/// deadlock check bailed out, which the C++ caller re-raises.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_deactivate() -> u8 {
    u8::from(deactivate().is_err())
}

fn deactivate() -> Result<(), Bailout> {
    if !activated() {
        return Ok(());
    }
    ACTIVATED.with(|a| a.set(false));
    interrupt_stop();
    if host::flag(host::MAIN_THREAD) {
        // SAFETY: C++ disables the hooks and restores unsafe functions.
        unsafe { (host::ops().unhook)() };
    }
    // SAFETY: C++ clears the switch callbacks.
    unsafe { (host::ops().detach)() };
    let original = ORIGINAL_INTERRUPT.load(Ordering::Relaxed);
    let original = (!original.is_null()).then(|| {
        // SAFETY: stored from a `swr_zend_interrupt_handler` by `activate`.
        unsafe {
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut zend_execute_data)>(
                original,
            )
        }
    });
    // SAFETY: restores Zend's interrupt function.
    unsafe { swr_zend_coroutine_set_interrupt(original) };
    if CONFIG.with(|c| c.deadlock_check.get()) {
        deadlock_check()?;
    }
    // SAFETY: C++ deactivates the core.
    unsafe { (host::ops().deactivate)() };
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
        // SAFETY: C++'s call of the library's checker, a body that runs PHP.
        return unsafe { bailout::trap(host::ops().deadlock_check, ptr::null_mut()) };
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
    let (co, enabled, last) = {
        // SAFETY: a live context.
        let this = unsafe { context::at(task) };
        (this.co, this.enable_scheduler, this.last_msec)
    };
    if !co.is_null()
        && interrupt_running()
        && enabled
        && host::absolute_msec() - last > MAX_EXEC_MSEC
    {
        // SAFETY: C++ defers the resume to the event loop.
        unsafe { (host::ops().defer)(interrupt_resume, co) };
        // SAFETY: the running coroutine; no frame here owns anything.
        unsafe { (host::ops().yield_)(co) };
    }
    let original = ORIGINAL_INTERRUPT.load(Ordering::Relaxed);
    if !original.is_null() {
        // SAFETY: Zend's previous interrupt function, which takes one pointer.
        let body = unsafe {
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(original)
        };
        // SAFETY: a C function; a bailout in it is re-raised below.
        if unsafe { bailout::trap(body, execute_data.cast()) }.is_err() {
            raise();
        }
    }
}

/// `coro_interrupt_resume`.
unsafe extern "C" fn interrupt_resume(co: *mut c_void) {
    // SAFETY: a coroutine that yielded to the scheduler.
    if !co.is_null() && unsafe { (host::ops().is_end)(co) } == 0 {
        host::resume(co);
    }
}

/// Re-raise a trapped bailout from a handler's outermost Rust frame, which
/// owns nothing to drop (docs/zend.md).
pub(crate) fn raise() {
    // SAFETY: Zend's own `zend_bailout()`.
    unsafe { swoole_rs_zend::sys::_zend_bailout(c"swoole_coroutine".as_ptr().cast_mut(), 0) };
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
    // SAFETY: this thread's executor globals.
    let flag = Flag(unsafe { swr_zend_vm_interrupt() });
    let shared = Arc::clone(&running);
    let spawned = thread::Builder::new().spawn(move || {
        let flag = flag;
        // SAFETY: C++ blocks every signal on this thread.
        unsafe { (host::ops().signal_block_all)() };
        let half = Duration::from_millis(MAX_EXEC_MSEC.unsigned_abs() / 2);
        while shared.load(Ordering::Acquire) {
            // SAFETY: the executor's flag, live until `interrupt_stop` joins.
            unsafe { swr_zend_vm_interrupt_raise(flag.0) };
            thread::sleep(half);
        }
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

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_cid() -> c_long {
    cid()
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_activated() -> u8 {
    u8::from(activated())
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_hook_flags() -> u32 {
    CONFIG.with(|c| c.hook_flags.get())
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_set_hook_flags(flags: u32) {
    CONFIG.with(|c| c.hook_flags.set(flags));
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_set_max_num(max: u64) {
    CONFIG.with(|c| c.max_num.set(max));
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_set_deadlock_check(enable: u8) {
    CONFIG.with(|c| c.deadlock_check.set(enable != 0));
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_set_preemptive(enable: u8) {
    CONFIG.with(|c| c.preemptive.set(enable != 0));
}

/// `PHPCoroutine::create` for C++ callers.
/// # Safety
/// As `zend_call_function`'s callable; `bailout` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcoroutines_create(
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
