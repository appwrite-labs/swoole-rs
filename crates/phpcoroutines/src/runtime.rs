//! `PHPCoroutine`'s per-thread state: its configuration, activation (the
//! library, reactor, hooks and switch callbacks), coroutine creation, the
//! deadlock check and the preemptive scheduler's interrupt thread.
use crate::{
    context::{self, Args},
    host,
};
use std::{
    cell::Cell,
    ffi::c_long,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use swoole_rs_zend::{
    E_ERROR, E_WARNING, Fcc, Function, Php, Zvals,
    bailout::{self, Bailout},
    host::{Creator, NO_THEN, Trap, function_call},
    message,
    sys::{zend_execute_data, zval},
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
    /// The interrupt thread, set by `interrupt_start` and taken by `interrupt_stop`.
    static INTERRUPT: Cell<Option<Interrupt>> = Cell::new(None);
}

/// The frame Zend passes its interrupt function (`zend_interrupt_function`).
/// Rust never makes one: it is the handlers' parameter, which Zend's call
/// stands for, and it only passes it on.
#[repr(transparent)]
struct Interrupted(*mut zend_execute_data);

/// `zend_interrupt_function`: [`interrupt_function`] or the one Zend ran before
/// it, each taking the frame Zend passes.
type Handler = Option<extern "C" fn(Interrupted)>;

/// How many threads' runtimes are active, and the interrupt function Zend ran
/// before the first of them. `zend_interrupt_function` is process-global: the
/// first activation installs ours and the last deactivation restores Zend's,
/// so no thread saves another's hook as the original or removes it under one
/// still running.
static INTERRUPT_HOOK: Mutex<(usize, Handler)> = Mutex::new((0, None));

fn interrupt_hook() -> std::sync::MutexGuard<'static, (usize, Handler)> {
    INTERRUPT_HOOK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The interrupt function Zend ran before activation.
fn original_interrupt() -> Handler {
    interrupt_hook().1
}

/// One more (`hold`) or one fewer active runtime: the first installs
/// [`interrupt_function`], keeping the one Zend had, and the last restores it.
fn hold_interrupt(hold: bool) {
    unsafe extern "C" {
        /// `zend_interrupt_function = handler`, returning the one before: only
        /// [`interrupt_function`] or the handler Zend ran before it are passed.
        safe fn swr_zend_coroutine_set_interrupt(handler: Handler) -> Handler;
    }
    let mut hook = interrupt_hook();
    let (count, original) = &mut *hook;
    if *count <= usize::from(!hold) {
        let ours: Handler = Some(interrupt_function);
        // Zend's interrupt function becomes ours, or again the one it ran before,
        // which only this keeps.
        let previous = swr_zend_coroutine_set_interrupt(if hold { ours } else { *original });
        if hold {
            *original = previous;
        }
    }
    *count = if hold {
        *count + 1
    } else {
        count.saturating_sub(1)
    };
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
    php: Php<'_>,
    fcc: Option<Fcc<'_>>,
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
    let Some((fcc, function)) = fcc.and_then(|fcc| Some((fcc, fcc.function()?))) else {
        host::fatal_error(E_ERROR, c"invalid function call info cache");
        return Ok(ERR_INVALID);
    };
    let kind = function.kind();
    if kind != Function::USER && kind != Function::INTERNAL {
        host::fatal_error(E_ERROR, message!("invalid function type {kind}").as_cstr());
        return Ok(ERR_INVALID);
    }
    if !activated() {
        activate(php)?;
    }
    let args = Args {
        fcc: *fcc.get(),
        // A user function's first instruction, for a fiber's switch notification.
        opcodes: function.opcodes().filter(|_| kind == Function::USER),
        argv,
        argc,
        callable,
    };
    let cid = host::create(args);
    bailout::trapped(0)?;
    Ok(cid)
}

/// `PHPCoroutine::activate`.
fn activate(php: Php<'_>) -> Result<(), Bailout> {
    if !crate::leased::library_loaded(php) {
        swoole_rs_phpswoole::load_library()?;
    }
    host::check_reactor();
    hold_interrupt(true);
    if host::flag(host::PREEMPTIVE) || CONFIG.with(|c| c.preemptive.get()) {
        interrupt_start();
    }
    if host::flag(host::MAIN_THREAD) {
        let flags = CONFIG.with(|c| c.hook_flags.get());
        host::hook(flags);
    }
    host::attach(context::on_yield, context::on_resume);
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
    hold_interrupt(false);
    if CONFIG.with(|c| c.deadlock_check.get()) {
        deadlock_check()?;
    }
    host::deactivate();
    Ok(())
}

unsafe extern "C" {
    /// Upstream's deadlock report through stdio's `printf`, so stdio buffers
    /// it as it did: one formatted write of the count.
    safe fn swr_zend_coroutine_deadlock_report(count: std::ffi::c_ulong);
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
    swr_zend_coroutine_deadlock_report(count as std::ffi::c_ulong);
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
extern "C" fn interrupt_function(frame: Interrupted) {
    let task = context::current();
    let (co, enabled, last) = (task.co(), task.enable_scheduler(), task.last_msec());
    if let Some(co) = co
        && interrupt_running()
        && enabled
        && host::absolute_msec() - last > MAX_EXEC_MSEC
    {
        host::defer(InterruptResume(co));
        host::yield_(co);
    }
    if let Some(original) = original_interrupt() {
        // Zend's previous interrupt function, over the frame Zend passed.
        let body = || original(frame);
        // The closure owns nothing; a bailout in it is re-raised below.
        if swoole_rs_zend::contain_with(body).is_err() {
            raise();
        }
    }
}

/// `coro_interrupt_resume`: the loop resumes the coroutine the interrupt yielded.
struct InterruptResume(host::Co);

impl swoole_rs_reactor::event_api::Deferred for InterruptResume {
    fn run(self) {
        let co = self.0;
        if !host::is_end(co) {
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
    /// `&EG(vm_interrupt)`: this PHP thread's interrupt flag, a `zend_atomic_bool`
    /// (an atomic bool, as `AtomicBool` is) of its executor, which outlives the
    /// interrupt thread `interrupt_stop` joins; Zend and that thread only touch it
    /// atomically. It takes nothing from its caller and runs no PHP code.
    safe fn swr_zend_vm_interrupt() -> &'static AtomicBool;
}

fn interrupt_start() {
    if interrupt_running() {
        return;
    }
    let running = Arc::new(AtomicBool::new(true));
    let flag = swr_zend_vm_interrupt();
    let shared = Arc::clone(&running);
    // The thread inherits a fully blocked mask (upstream's `interrupt_thread_start`).
    let spawned = swoole_rs_corebase::with_signals_blocked(|| {
        thread::Builder::new().spawn(move || {
            swoole_rs_corebase::signal_block_all();
            let half = Duration::from_millis(MAX_EXEC_MSEC.unsigned_abs() / 2);
            while shared.load(Ordering::Acquire) {
                // `zend_atomic_bool_store(flag, 1)`.
                flag.store(true, Ordering::SeqCst);
                thread::sleep(half);
            }
        })
    });
    if let Ok(thread) = spawned {
        INTERRUPT.with(|i| i.set(Some(Interrupt { running, thread })));
        INTERRUPT_RUNNING.with(|r| r.set(true));
    }
}

/// `interrupt_thread_stop`.
fn interrupt_stop() {
    if !interrupt_running() {
        return;
    }
    INTERRUPT_RUNNING.with(|r| r.set(false));
    if let Some(interrupt) = INTERRUPT.with(Cell::take) {
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
/// `swoole_rs_zend::host::function_call` and the zend-sys shims pass (C ABI):
/// only a shim makes its [`Trap`], so only a shim calls it, with the call's
/// cache and arguments.
pub extern "C" fn create_coroutine(
    fcc: Option<Fcc<'_>>,
    argc: u32,
    argv: *mut zval,
    callable: *mut zval,
    mut trap: Trap<'_>,
) -> c_long {
    let created = create(trap.php(), fcc, argc, argv, callable);
    trap.set(created.is_err());
    created.unwrap_or(ERR_INVALID)
}

/// [`create_coroutine`], the leaf of every coroutine call.
pub const CREATOR: Creator = Creator::new(create_coroutine);

/// `zend::function::call(&fcc, argc, argv, nullptr, coroutine)` of a kept
/// callable with its bound arguments: whether it ran, warning as
/// `sw_callable_warning` when it could not.
/// # Errors
/// PHP bailed out.
pub fn call_kept(callable: &swoole_rs_zend::Callable, coroutine: bool) -> Result<bool, Bailout> {
    let (cache, args) = callable.call_parts();
    function_call(cache, args, None, coroutine, CREATOR, NO_THEN)
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
    function_call(
        handler.cache(),
        Zvals::owned(args),
        Some(retval),
        coroutine,
        CREATOR,
        NO_THEN,
    )
}

/// [`call_handler`] over the zvals of a callback's arguments, with the return
/// slot `retval` or none (a coroutine has none).
/// # Errors
/// PHP bailed out.
pub fn call_args(
    handler: &swoole_rs_zend::host::callable::Callable,
    args: &mut [zval],
    retval: Option<&swoole_rs_zend::Value>,
    coroutine: bool,
) -> Result<bool, Bailout> {
    function_call(
        handler.cache(),
        Zvals::of(args),
        retval,
        coroutine,
        CREATOR,
        NO_THEN,
    )
}
