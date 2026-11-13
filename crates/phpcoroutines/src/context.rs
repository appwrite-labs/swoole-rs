//! Each coroutine's PHP context (upstream's `PHPContext`): the VM state it
//! swaps out while another runs, its callable and return value, defer tasks,
//! fiber notifications and bookkeeping. The core calls `on_yield`,
//! `on_resume` and `on_close` around every switch; the C++ main function
//! (which traps bailouts) calls `open`, `enter`, `defer_next` and `close`.
//!
//! Contexts are reached through a [`Ctx`] handle, which reads and writes one
//! field at a time: another coroutine may change a suspended context
//! (`Co::cancel`, `Co::join`), so no reference to one lives across a switch.
use crate::{host, methods::Join, runtime};
use std::{
    cell::UnsafeCell,
    ffi::{c_int, c_long, c_void},
    mem,
    ptr::{self, NonNull},
};
use swoole_rs_coroutine::State;
use swoole_rs_zend::sys::{
    IS_UNDEF, swr_zend_callable, swr_zend_callable_hold, swr_zend_coroutine_fcc_discard,
    swr_zend_coroutine_main, swr_zend_coroutine_output_reset, swr_zend_coroutine_restore,
    swr_zend_coroutine_set_frame, swr_zend_coroutine_start, swr_zend_coroutine_switch,
    swr_zend_coroutine_vm, swr_zend_coroutine_vm_destroy, swr_zend_efree, swr_zend_object_release,
    zend_class_entry, zend_execute_data, zend_fcall_info, zend_fcall_info_cache,
    zend_fiber_context, zend_fiber_status, zend_fiber_status_ZEND_FIBER_STATUS_DEAD,
    zend_fiber_status_ZEND_FIBER_STATUS_INIT, zend_fiber_status_ZEND_FIBER_STATUS_RUNNING,
    zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED, zend_object, zend_observer_fiber_destroy_notify,
    zend_observer_fiber_init_notify, zend_observer_fiber_switch_notify, zval, zval_ptr_dtor,
};
use swoole_rs_zend::{Callable, Function, Value, bailout};

unsafe extern "C" {
    /// `save_context`: this PHP thread's executor, output and serializer state,
    /// moved into `vm`, with a fresh output layer started in their place. It
    /// follows no pointer `vm` holds and runs no PHP code.
    safe fn swr_zend_coroutine_save(vm: &mut swr_zend_coroutine_vm);
}

/// Upstream's `INVALID_PTR`: a destroyed context object, or an unusable fiber
/// field.
pub(crate) const INVALID: usize = usize::MAX;

/// `zend_function.type`: `ZEND_INTERNAL_FUNCTION` and `ZEND_USER_FUNCTION`.
pub(crate) const INTERNAL_FUNCTION: u8 = 1;
pub(crate) const USER_FUNCTION: u8 = 2;

/// `PHPCoroutine::Args`, built by `create` for the new coroutine's entry.
#[repr(C)]
pub(crate) struct Args {
    pub(crate) fcc: *mut zend_fcall_info_cache,
    pub(crate) argv: *mut zval,
    pub(crate) argc: u32,
    pub(crate) callable: *mut zval,
}

/// One coroutine's PHP context; the main context is a thread-local one.
pub(crate) struct Context {
    vm: swr_zend_coroutine_vm,
    /// The coroutine's callable, laid out as `zend::Function`.
    call: swr_zend_callable,
    return_value: zval,
    co: *mut c_void,
    fiber: *mut zend_fiber_context,
    fiber_init_notified: bool,
    enable_scheduler: bool,
    /// The defer tasks, each kept by `defer()` until it runs or the context closes.
    defer: Vec<Callable>,
    /// The `Co::join()` waiting for this coroutine to close.
    join: *mut Join,
    pcid: c_long,
    /// `Coroutine\Context`, null until asked for, `INVALID` once destroyed.
    context: *mut zend_object,
    timer_limit: *mut c_void,
    last_msec: c_long,
    /// The tracer's blocking detection counts this context's switches.
    watch_switches: bool,
    switch_count: usize,
}

impl Context {
    const EMPTY: Self = Self {
        vm: swoole_rs_zend::zeroed(),
        call: swoole_rs_zend::zeroed(),
        return_value: swoole_rs_zend::UNDEF,
        co: ptr::null_mut(),
        fiber: ptr::null_mut(),
        fiber_init_notified: false,
        enable_scheduler: false,
        defer: Vec::new(),
        join: ptr::null_mut(),
        pcid: 0,
        context: ptr::null_mut(),
        timer_limit: ptr::null_mut(),
        last_msec: 0,
        watch_switches: false,
        switch_count: 0,
    };
}

/// `&raw mut (*callable).fcc`.
#[inline]
fn fcc(callable: *mut swr_zend_callable) -> *mut zend_fcall_info_cache {
    callable
        .wrapping_byte_add(mem::offset_of!(swr_zend_callable, fcc))
        .cast()
}

/// A live context: the thread's main one, or a coroutine's from [`open`] until
/// [`on_close`]. Only this module makes one, from the thread-local main context
/// and the host's task pointers. Each method borrows the context for that one
/// access and keeps no reference.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ctx(NonNull<Context>);

/// `get`/`set` for `Copy` fields: one read or write of a live context's field.
macro_rules! fields {
    ($($field:ident, $set:ident: $ty:ty;)*) => {
        impl Ctx {
            $(
                #[inline]
                pub(crate) fn $field(self) -> $ty {
                    self.with(|ctx| ctx.$field)
                }

                #[inline]
                pub(crate) fn $set(self, value: $ty) {
                    self.with(|ctx| ctx.$field = value);
                }
            )*
        }
    };
}

fields! {
    co, set_co: *mut c_void;
    fiber, set_fiber: *mut zend_fiber_context;
    fiber_init_notified, set_fiber_init_notified: bool;
    enable_scheduler, set_enable_scheduler: bool;
    join, set_join: *mut Join;
    pcid, set_pcid: c_long;
    context, set_context: *mut zend_object;
    timer_limit, set_timer_limit: *mut c_void;
    last_msec, set_last_msec: c_long;
    watch_switches, set_watch_switches: bool;
    switch_count, set_switch_count: usize;
}

impl Ctx {
    /// # Safety
    /// `ctx` is null or a live context, which stays live while the handle is used.
    #[inline]
    pub(crate) unsafe fn of(ctx: *mut c_void) -> Option<Self> {
        NonNull::new(ctx.cast()).map(Self)
    }

    #[inline]
    pub(crate) fn ptr(self) -> *mut Context {
        self.0.as_ptr()
    }

    /// `&raw mut (*ctx).vm`, without dereferencing.
    #[inline]
    pub(crate) fn vm(self) -> *mut swr_zend_coroutine_vm {
        self.ptr()
            .wrapping_byte_add(mem::offset_of!(Context, vm))
            .cast()
    }

    /// `&raw mut (*ctx).call`.
    #[inline]
    fn call(self) -> *mut swr_zend_callable {
        self.ptr()
            .wrapping_byte_add(mem::offset_of!(Context, call))
            .cast()
    }

    /// `&raw mut (*ctx).return_value`.
    #[inline]
    fn return_value(self) -> *mut zval {
        self.ptr()
            .wrapping_byte_add(mem::offset_of!(Context, return_value))
            .cast()
    }

    /// The context, borrowed for `f`, which neither switches nor reaches the
    /// context through another handle or pointer.
    #[inline]
    fn with<R>(self, f: impl FnOnce(&mut Context) -> R) -> R {
        // SAFETY: a live context (the handle's invariant); the borrow ends with `f`.
        f(unsafe { &mut *self.ptr() })
    }

    /// The VM state, borrowed for `f`, as [`Ctx::with`].
    fn with_vm<R>(self, f: impl FnOnce(&mut swr_zend_coroutine_vm) -> R) -> R {
        self.with(|ctx| f(&mut ctx.vm))
    }

    /// The VM state saved by the last switch out: the frame it stopped in.
    pub(crate) fn execute_data(self) -> *mut zend_execute_data {
        self.with_vm(|vm| vm.execute_data)
    }

    /// Run `f` with that frame as Zend's current one, as its backtraces and
    /// exception locations read it, then restore the running frame; `f` just
    /// runs for the running context, whose saved frame is stale. `f` neither
    /// switches coroutines nor bails out.
    pub(crate) fn in_frame<R>(self, f: impl FnOnce() -> R) -> R {
        if self == current() {
            return f();
        }
        // SAFETY: a live context that is not running was switched out, and its
        // saved frame stays live until it resumes, which `f` does not let it.
        let running = unsafe { swr_zend_coroutine_set_frame(self.execute_data()) };
        let result = f();
        // SAFETY: the running coroutine's own frame, read above.
        unsafe { swr_zend_coroutine_set_frame(running) };
        result
    }

    /// Its VM stack chain.
    pub(crate) fn vm_stack(self) -> swoole_rs_zend::sys::zend_vm_stack {
        self.with_vm(|vm| vm.vm_stack)
    }

    /// The exception a cancellation left pending, and its class.
    pub(crate) fn exception(self) -> (*mut zend_object, *mut zend_class_entry) {
        self.with_vm(|vm| (vm.exception, vm.exception_class))
    }

    /// Leave `exception` of class `class` pending, thrown when the coroutine resumes.
    pub(crate) fn set_exception(self, exception: *mut zend_object, class: *mut zend_class_entry) {
        self.with_vm(|vm| {
            vm.exception = exception;
            vm.exception_class = class;
        });
    }

    /// `@`'s state: on with the reporting level a switch restores, or off.
    pub(crate) fn set_silence(self, reporting: Option<c_int>) {
        self.with_vm(|vm| {
            vm.in_silence = reporting.is_some();
            if let Some(reporting) = reporting {
                vm.ori_error_reporting = reporting;
            }
        });
    }

    /// The defer tasks, borrowed for `f`, as [`Ctx::with`].
    fn defers<R>(self, f: impl FnOnce(&mut Vec<Callable>) -> R) -> R {
        self.with(|ctx| f(&mut ctx.defer))
    }
}

per_thread! {
    static MAIN: UnsafeCell<Context> = UnsafeCell::new(Context::EMPTY);
}

/// The thread's main context, live as long as the thread.
#[inline]
pub(crate) fn main() -> Ctx {
    MAIN.with(|main| Ctx(NonNull::from(main).cast()))
}

/// `Coroutine::get_current_task()`: the running coroutine's context, none in
/// the main context.
#[inline]
pub(crate) fn current_task() -> Option<Ctx> {
    host::current_task()
}

/// `PHPCoroutine::get_context()`: the running coroutine's, or the main one.
#[inline]
pub(crate) fn current() -> Ctx {
    current_task().unwrap_or_else(main)
}

/// The context of coroutine `cid`, if it exists.
pub(crate) fn task(cid: c_long) -> Option<Ctx> {
    host::task_by_cid(cid)
}

/// `get_context_by_cid`: -1 is the main context.
pub(crate) fn by_cid(cid: c_long) -> Option<Ctx> {
    if cid == -1 { Some(main()) } else { task(cid) }
}

/// `get_origin_context`: the context that resumed `ctx`'s coroutine.
#[inline]
fn origin(ctx: Ctx) -> Ctx {
    host::origin_task(ctx.co()).unwrap_or_else(main)
}

/// `record_last_msec`.
#[inline]
pub(crate) fn record_last_msec(ctx: Ctx) {
    if runtime::interrupt_running() {
        ctx.set_last_msec(host::absolute_msec());
    }
}

/// `save_context(from)` then `restore_context(to)`.
#[inline]
fn switch(from: Ctx, to: Ctx) {
    // SAFETY: two live, distinct contexts.
    unsafe { swr_zend_coroutine_switch(from.vm(), to.vm()) };
}

/// `PHPCoroutine::on_yield`.
pub(crate) unsafe extern "C" fn on_yield(arg: *mut c_void) {
    // SAFETY: the core passes the yielding coroutine's task, a live context.
    let Some(ctx) = (unsafe { Ctx::of(arg) }) else {
        return;
    };
    let origin = origin(ctx);
    fiber_switch_try_notify(ctx, origin);
    switch(ctx, origin);
    count_switch(ctx);
}

/// `PHPCoroutine::on_resume`.
pub(crate) unsafe extern "C" fn on_resume(arg: *mut c_void) {
    // SAFETY: the core passes the resumed coroutine's task, a live context.
    let Some(ctx) = (unsafe { Ctx::of(arg) }) else {
        return;
    };
    let current = current();
    fiber_switch_try_notify(current, ctx);
    switch(current, ctx);
    record_last_msec(ctx);
    count_switch(ctx);
}

#[inline]
fn count_switch(ctx: Ctx) {
    if ctx.watch_switches() {
        ctx.set_switch_count(ctx.switch_count() + 1);
    }
}

/// The tracer's blocking detection (upstream's `on_yield`/`on_resume`
/// callbacks counting `switch_count`): with `watch`, count the running
/// context's switches from 0; without, stop. Returns the count.
pub(crate) fn watch_switches(watch: bool) -> usize {
    let this = current();
    this.set_watch_switches(watch);
    if watch {
        this.set_switch_count(0);
    }
    this.switch_count()
}

/// `PHPCoroutine::on_close`: run the join hook, then free the context.
pub(crate) unsafe extern "C" fn on_close(arg: *mut c_void) {
    // SAFETY: the core passes the closing coroutine's task, a live context.
    let Some(ctx) = (unsafe { Ctx::of(arg) }) else {
        return;
    };
    let join = ctx.join();
    if !join.is_null() {
        Join::closed(join, ctx);
    }
    // SAFETY: the closing context, written by `open` and not used after this.
    unsafe { ptr::drop_in_place(ctx.ptr()) };
    // SAFETY: emalloc'd by `open`.
    unsafe { swr_zend_efree(ctx.ptr().cast()) };
}

/// `create_context`, on the new coroutine: its context, live until [`on_close`].
/// # Safety
/// `args` is the creator's live `Args`, and this runs on the new coroutine.
pub(crate) unsafe fn open(args: *mut c_void) -> Option<Ctx> {
    // SAFETY: per the caller.
    let args = unsafe { &*args.cast::<Args>() };
    let co = host::current();
    let pcid = host::origin_cid(co);
    let ctx = swoole_rs_zend::value::emalloc(size_of::<Context>()).cast::<Context>();
    // Built in place: a `Context::EMPTY` literal is staged on the stack and
    // copied over. All-zero bytes are `Context::EMPTY` except `defer`.
    // SAFETY: fresh storage for one context.
    unsafe { ctx.write_bytes(0, 1) };
    // Its place in the storage above; nothing reads it.
    let defer = ctx
        .wrapping_byte_add(mem::offset_of!(Context, defer))
        .cast::<Vec<Callable>>();
    // SAFETY: a write, so the zeroed `defer` is never read or dropped.
    unsafe { defer.write(Vec::new()) };
    // SAFETY: every field now holds a valid value, and the context lives until `on_close`.
    let this = unsafe { Ctx::of(ctx.cast()) }?;
    this.set_co(co);
    this.set_pcid(pcid);
    this.set_enable_scheduler(true);
    swoole_rs_coroutine::set_current_task(ctx.cast());
    if host::flag(host::FIBER_MOCK) {
        fiber_init(this);
    }
    let (stack, size) = host::stack(co);
    // SAFETY: on the new coroutine, into its own context.
    unsafe { swr_zend_coroutine_start(this.vm(), stack, size) };
    record_last_msec(this);
    // SAFETY: the creator's resolved callable.
    let fcc = unsafe { *args.fcc };
    let retval = this.return_value();
    this.with(|context| {
        context.call.fcc = fcc;
        let fci = &mut context.call.fci;
        fci.size = size_of::<zend_fcall_info>();
        fci.object = ptr::null_mut();
        fci.param_count = args.argc;
        fci.params = args.argv;
        fci.named_params = ptr::null_mut();
        fci.retval = retval;
        // SAFETY: null, or the creator's live callable zval.
        if let Some(callable) = unsafe { Value::from_ptr(args.callable) } {
            // The copy takes a reference.
            fci.function_name = callable.copy().into_zval();
        }
    });
    // SAFETY: a resolved callable (sw_zend_fci_cache_persist).
    unsafe { swr_zend_callable_hold(this.call()) };
    Some(this)
}

/// The start hook and the fiber switch into the running coroutine's function;
/// its callable, to call (`zend::Function *`).
pub(crate) fn enter(ctx: Ctx) -> *mut swr_zend_callable {
    host::coro_hook(false, ctx.ptr().cast());
    let fiber = ctx.fiber();
    let handler = ctx.with(|context| context.call.fcc.function_handler);
    // SAFETY: a resolved callable's handler, which the context's call holds.
    let function = unsafe { Function::from_ptr(handler) };
    if !fiber.is_null()
        && let Some(function) = function.filter(|function| function.kind() == Function::USER)
        && let Some(opcodes) = function.opcodes()
    {
        let mut frame: zend_execute_data = swoole_rs_zend::zeroed();
        // A user function is its op array.
        frame.opline = opcodes;
        frame.func = handler;
        // SAFETY: the frame outlives the notification.
        let previous = unsafe { swr_zend_coroutine_set_frame(&raw mut frame) };
        fiber_switch_try_notify(origin(ctx), ctx);
        // SAFETY: as above.
        unsafe { swr_zend_coroutine_set_frame(previous) };
    }
    ctx.call()
}

/// The next defer task (last in, first out), and the coroutine's return value,
/// its one argument, when it has one; none when no task is left.
#[inline]
pub(crate) fn defer_next(ctx: Ctx) -> Option<(Callable, Option<*mut zval>)> {
    let task = ctx.defers(Vec::pop)?;
    let returned = ctx.with(|context| swoole_rs_zend::kind(&context.return_value)) != IS_UNDEF;
    Some((task, returned.then(|| ctx.return_value())))
}

/// `save_context`: the running VM's state into the running context, as a switch
/// out of it does.
pub(crate) fn save_current() {
    current().with_vm(|vm| swr_zend_coroutine_save(vm));
}

/// `destroy_context`, on the running, finishing coroutine. Its bailout
/// address is null here, so a bailout in a destructor ends the process rather
/// than longjmp.
pub(crate) fn close(ctx: Ctx) {
    host::coro_hook(true, ctx.ptr().cast());
    // SAFETY: output state is the coroutine's own.
    unsafe { swr_zend_coroutine_output_reset() };
    // Its defer tasks were kept and are not called.
    for task in ctx.defers(mem::take) {
        if let Err(bailout) = task.release() {
            bailout.defer();
            bailout::finish();
        }
    }
    // Object destruction may yield and let the origin coroutine finish: the
    // origin is looked up again before each use.
    let object = ctx.context();
    if !object.is_null() {
        ctx.set_context(ptr::without_provenance_mut(INVALID));
        // SAFETY: the context's own reference.
        unsafe { swr_zend_object_release(object) };
    }
    // Upstream only drops the references (Z_TRY_DELREF), leaking a callable or
    // return value the coroutine held last, such as `go([new Foo, 'bar'])`'s array.
    let call = ctx.call();
    let function_name = call
        .wrapping_byte_add(mem::offset_of!(swr_zend_callable, fci.function_name))
        .cast::<zval>();
    // SAFETY: a live context's callable; `open` added this reference.
    unsafe { zval_ptr_dtor(function_name) };
    ctx.with(|context| context.call.fci.function_name.u1.type_info = IS_UNDEF);
    // SAFETY: as above; `open` held its object and closure.
    unsafe { swr_zend_coroutine_fcc_discard(fcc(call)) };
    // SAFETY: a live context's return value.
    unsafe { zval_ptr_dtor(ctx.return_value()) };
    fiber_try_destroy(ctx, origin(ctx));
    // A context-owned __destruct() may have rearmed the time limit.
    let timer = ctx.timer_limit();
    ctx.set_timer_limit(ptr::null_mut());
    if !timer.is_null() {
        // SAFETY: the coroutine's armed timer.
        unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
    }
    // SAFETY: the coroutine's own VM stack.
    unsafe { swr_zend_coroutine_vm_destroy() };
    // SAFETY: the origin's live context.
    unsafe { swr_zend_coroutine_restore(origin(ctx).vm()) };
}

/// `init_main_context`, at RINIT.
pub(crate) fn init_main() {
    let main = main();
    main.set_co(ptr::null_mut());
    main.set_fiber_init_notified(true);
    // SAFETY: its VM state, at RINIT: no exception is being dispatched.
    main.set_fiber(unsafe { swr_zend_coroutine_main(main.vm()) });
}

/// `free_main_context`, at RSHUTDOWN.
pub(crate) fn free_main() {
    main().with(|context| *context = Context::EMPTY);
}

/// `PHPCoroutine::defer`: keep a task on the running context.
pub(crate) fn defer(task: Callable) {
    current().defers(|defers| defers.push(task));
}

fn fiber_init(ctx: Ctx) {
    let fiber = swoole_rs_zend::value::emalloc(size_of::<zend_fiber_context>())
        .cast::<zend_fiber_context>();
    let invalid = ptr::without_provenance_mut::<c_void>(INVALID);
    {
        // SAFETY: fresh storage; these fields are never used for a mock fiber.
        let this = unsafe { &mut *fiber };
        this.handle = invalid;
        this.kind = invalid;
        // SAFETY: upstream's INVALID_PTR in a function slot, never called.
        this.function =
            unsafe { mem::transmute::<usize, swoole_rs_zend::sys::zend_fiber_coroutine>(INVALID) };
        this.stack = invalid.cast();
    }
    ctx.set_fiber(fiber);
    // SAFETY: a new fiber context.
    unsafe { zend_observer_fiber_init_notify(fiber) };
}

fn fiber_try_destroy(ctx: Ctx, origin: Ctx) {
    let fiber = ctx.fiber();
    if fiber.is_null() {
        return;
    }
    set_status(fiber, zend_fiber_status_ZEND_FIBER_STATUS_DEAD);
    fiber_switch_try_notify(ctx, origin);
    // SAFETY: as above.
    unsafe { zend_observer_fiber_destroy_notify(fiber) };
    // SAFETY: emalloc'd by `fiber_init`.
    unsafe { swr_zend_efree(fiber.cast()) };
}

fn fiber_status(ctx: Ctx) -> zend_fiber_status {
    if ctx.fiber() == main().fiber() {
        return zend_fiber_status_ZEND_FIBER_STATUS_RUNNING;
    }
    match host::state_of(ctx.co()) {
        State::Init => zend_fiber_status_ZEND_FIBER_STATUS_INIT,
        State::Waiting => zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
        State::Running => zend_fiber_status_ZEND_FIBER_STATUS_RUNNING,
        State::End => zend_fiber_status_ZEND_FIBER_STATUS_DEAD,
    }
}

/// Set a live fiber's status.
fn set_status(fiber: *mut zend_fiber_context, status: zend_fiber_status) {
    // SAFETY: a live context's fiber; a field write.
    unsafe { (*fiber).status = status };
}

fn fiber_switch_notify(from: Ctx, to: Ctx) {
    let (from_fiber, to_fiber, notified) = (from.fiber(), to.fiber(), to.fiber_init_notified());
    set_status(from_fiber, fiber_status(from));
    set_status(to_fiber, fiber_status(to));
    if !notified {
        set_status(to_fiber, zend_fiber_status_ZEND_FIBER_STATUS_INIT);
    }
    // SAFETY: two live fibers.
    unsafe { zend_observer_fiber_switch_notify(from_fiber, to_fiber) };
    if !notified {
        set_status(to_fiber, fiber_status(to));
        to.set_fiber_init_notified(true);
    }
}

#[inline]
fn fiber_switch_try_notify(from: Ctx, to: Ctx) {
    if !from.fiber().is_null() && !to.fiber().is_null() {
        fiber_switch_notify(from, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_pointers_match_the_fields() {
        let mut context = Context::EMPTY;
        let ctx = &raw mut context;
        let handle = Ctx(NonNull::new(ctx).unwrap());
        assert_eq!(handle.vm(), unsafe { &raw mut (*ctx).vm });
        assert_eq!(handle.call(), unsafe { &raw mut (*ctx).call });
        assert_eq!(handle.return_value(), unsafe {
            &raw mut (*ctx).return_value
        });
        let callable = unsafe { &raw mut (*ctx).call };
        assert_eq!(fcc(callable), unsafe { &raw mut (*callable).fcc });
    }

    #[test]
    fn a_new_context_has_no_pending_state() {
        let context = Context::EMPTY;
        assert_eq!(context.defer.len(), 0);
        assert!(
            context.join.is_null() && context.context.is_null() && context.timer_limit.is_null()
        );
        assert_eq!(unsafe { context.return_value.u1.type_info }, IS_UNDEF);
        assert!(!context.vm.in_silence && context.vm.output_ptr.is_null());
    }
}
