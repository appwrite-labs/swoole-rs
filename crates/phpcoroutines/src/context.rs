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
use swoole_rs_zend::sys::{
    IS_UNDEF, swr_zend_callable, swr_zend_callable_hold, swr_zend_coroutine_fcc_discard,
    swr_zend_coroutine_main, swr_zend_coroutine_output_reset, swr_zend_coroutine_restore,
    swr_zend_coroutine_set_frame, swr_zend_coroutine_start, swr_zend_coroutine_switch,
    swr_zend_coroutine_vm, swr_zend_coroutine_vm_destroy, swr_zend_efree, swr_zend_emalloc,
    swr_zend_object_release, swr_zval_try_addref, zend_class_entry, zend_execute_data,
    zend_fcall_info, zend_fcall_info_cache, zend_fiber_context, zend_fiber_status,
    zend_fiber_status_ZEND_FIBER_STATUS_DEAD, zend_fiber_status_ZEND_FIBER_STATUS_INIT,
    zend_fiber_status_ZEND_FIBER_STATUS_RUNNING, zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
    zend_object, zend_observer_fiber_destroy_notify, zend_observer_fiber_init_notify,
    zend_observer_fiber_switch_notify, zval, zval_ptr_dtor,
};

/// Upstream's `INVALID_PTR`: a destroyed context object, or an unusable fiber
/// field.
pub(crate) const INVALID: usize = usize::MAX;

/// `zend_function.type`: `ZEND_INTERNAL_FUNCTION` and `ZEND_USER_FUNCTION`.
pub(crate) const INTERNAL_FUNCTION: u8 = 1;
pub(crate) const USER_FUNCTION: u8 = 2;

/// `Coroutine::State`.
const STATE_INIT: c_int = 0;
const STATE_WAITING: c_int = 1;
const STATE_RUNNING: c_int = 2;
const STATE_END: c_int = 3;

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
    defer: Vec<*mut swr_zend_callable>,
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
        // SAFETY: all-zero is a valid C struct of pointers and integers.
        vm: unsafe { mem::zeroed() },
        // SAFETY: as above.
        call: unsafe { mem::zeroed() },
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
/// and the host's task pointers. Each method reads or writes through the
/// pointer for that one access and keeps no reference.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ctx(NonNull<Context>);

/// `get`/`set` for `Copy` fields: one read or write of a live context's field.
macro_rules! fields {
    ($($field:ident, $set:ident: $ty:ty;)*) => {
        impl Ctx {
            $(
                #[inline]
                pub(crate) fn $field(self) -> $ty {
                    // SAFETY: a live context (the handle's invariant); a field read.
                    unsafe { (*self.0.as_ptr()).$field }
                }

                #[inline]
                pub(crate) fn $set(self, value: $ty) {
                    // SAFETY: as above; a field write.
                    unsafe { (*self.0.as_ptr()).$field = value };
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
    unsafe fn of(ctx: *mut c_void) -> Option<Self> {
        NonNull::new(ctx.cast()).map(Self)
    }

    #[inline]
    fn ptr(self) -> *mut Context {
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

    /// The VM state, borrowed for `f`, which neither switches nor reaches the context.
    fn with_vm<R>(self, f: impl FnOnce(&mut swr_zend_coroutine_vm) -> R) -> R {
        // SAFETY: a live context's VM state; the borrow ends with `f`.
        f(unsafe { &mut *self.vm() })
    }

    /// The VM state saved by the last switch out: the frame it stopped in.
    pub(crate) fn execute_data(self) -> *mut zend_execute_data {
        self.with_vm(|vm| vm.execute_data)
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

    /// The defer tasks, borrowed for `f`, which neither switches nor reaches the context.
    fn defers<R>(self, f: impl FnOnce(&mut Vec<*mut swr_zend_callable>) -> R) -> R {
        // SAFETY: a live context's own Vec; the borrow ends with `f`.
        f(unsafe { &mut (*self.ptr()).defer })
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
    // SAFETY: the host's tasks are contexts from `open`, live until `on_close`.
    unsafe { Ctx::of(host::current_task()) }
}

/// `PHPCoroutine::get_context()`: the running coroutine's, or the main one.
#[inline]
pub(crate) fn current() -> Ctx {
    current_task().unwrap_or_else(main)
}

/// The context of coroutine `cid`, if it exists.
pub(crate) fn task(cid: c_long) -> Option<Ctx> {
    // SAFETY: as in `current_task`.
    unsafe { Ctx::of(host::task_by_cid(cid)) }
}

/// `get_context_by_cid`: -1 is the main context.
pub(crate) fn by_cid(cid: c_long) -> Option<Ctx> {
    if cid == -1 { Some(main()) } else { task(cid) }
}

/// `get_origin_context`: the context that resumed `ctx`'s coroutine.
#[inline]
fn origin(ctx: Ctx) -> Ctx {
    // SAFETY: as in `current_task`.
    unsafe { Ctx::of(host::origin_task(ctx.co())) }.unwrap_or_else(main)
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

/// `create_context`, on the new coroutine.
/// # Safety
/// `args` is the creator's live `Args`, and this runs on the new coroutine.
pub(crate) unsafe fn open(args: *mut c_void) -> *mut c_void {
    // SAFETY: per the caller.
    let args = unsafe { &*args.cast::<Args>() };
    let co = host::current();
    let pcid = host::origin_cid(co);
    // SAFETY: Zend's allocator, for one context.
    let ctx = unsafe { swr_zend_emalloc(size_of::<Context>()) }.cast::<Context>();
    // Built in place: a `Context::EMPTY` literal is staged on the stack and
    // copied over. All-zero bytes are `Context::EMPTY` except `defer`.
    // SAFETY: fresh storage for one context.
    unsafe { ctx.write_bytes(0, 1) };
    // SAFETY: a raw place in the storage above; nothing reads it.
    let defer = unsafe { &raw mut (*ctx).defer };
    // SAFETY: a write, so the zeroed `defer` is never read or dropped.
    unsafe { defer.write(Vec::new()) };
    // SAFETY: every field now holds a valid value, and the context lives until `on_close`.
    let Some(this) = (unsafe { Ctx::of(ctx.cast()) }) else {
        return ptr::null_mut();
    };
    this.set_co(co);
    this.set_pcid(pcid);
    this.set_enable_scheduler(true);
    host::set_task(co, ctx.cast());
    if host::flag(host::FIBER_MOCK) {
        fiber_init(this);
    }
    let (stack, size) = host::stack(co);
    // SAFETY: on the new coroutine, into its own context.
    unsafe { swr_zend_coroutine_start(this.vm(), stack, size) };
    record_last_msec(this);
    let call = this.call();
    // SAFETY: the context's callable, which nothing else reaches yet.
    let callable = unsafe { &mut *call };
    // SAFETY: the creator's resolved callable.
    callable.fcc = unsafe { *args.fcc };
    let fci = &mut callable.fci;
    fci.size = size_of::<zend_fcall_info>();
    fci.object = ptr::null_mut();
    fci.param_count = args.argc;
    fci.params = args.argv;
    fci.named_params = ptr::null_mut();
    fci.retval = this.return_value();
    if !args.callable.is_null() {
        // SAFETY: the creator's live callable zval.
        fci.function_name = unsafe { *args.callable };
        // SAFETY: the copy takes a reference.
        unsafe { swr_zval_try_addref(&raw mut fci.function_name) };
    }
    // SAFETY: a resolved callable (sw_zend_fci_cache_persist).
    unsafe { swr_zend_callable_hold(call) };
    ctx.cast()
}

/// The start hook and the fiber switch into the coroutine's function; its
/// callable, to call (`zend::Function *`).
/// # Safety
/// `context` came from `open` on the running coroutine.
pub(crate) unsafe fn enter(context: *mut c_void) -> *mut swr_zend_callable {
    // SAFETY: per the caller.
    let Some(ctx) = (unsafe { Ctx::of(context) }) else {
        return ptr::null_mut();
    };
    host::coro_hook(false, context);
    let fiber = ctx.fiber();
    // SAFETY: the context's resolved callable; a field read.
    let handler = unsafe { (*ctx.call()).fcc.function_handler };
    // SAFETY: a resolved callable's handler.
    let function = unsafe { &*handler };
    // SAFETY: every zend_function starts with its type.
    if !fiber.is_null() && unsafe { function.type_ } == USER_FUNCTION {
        // SAFETY: all-zero is an empty frame.
        let mut frame: zend_execute_data = unsafe { mem::zeroed() };
        // SAFETY: a user function's op array.
        frame.opline = unsafe { function.op_array.opcodes };
        frame.func = handler;
        // SAFETY: the frame outlives the notification.
        let previous = unsafe { swr_zend_coroutine_set_frame(&raw mut frame) };
        fiber_switch_try_notify(origin(ctx), ctx);
        // SAFETY: as above.
        unsafe { swr_zend_coroutine_set_frame(previous) };
    }
    ctx.call()
}

/// The next defer task (last in, first out), with the coroutine's return
/// value as its argument when it has one; null when none are left.
/// # Safety
/// `context` came from `open` on the running coroutine.
pub(crate) unsafe fn defer_next(context: *mut c_void) -> *mut swr_zend_callable {
    // SAFETY: per the caller.
    let Some(ctx) = (unsafe { Ctx::of(context) }) else {
        return ptr::null_mut();
    };
    let Some(task) = ctx.defers(Vec::pop) else {
        return ptr::null_mut();
    };
    let return_value = ctx.return_value();
    // SAFETY: the context's return value.
    let value = unsafe { &*return_value };
    // SAFETY: a zval's type tag.
    if unsafe { value.u1.type_info } != IS_UNDEF {
        // SAFETY: a kept callable.
        let task = unsafe { &mut *task };
        task.fci.param_count = 1;
        task.fci.params = return_value;
    }
    task
}

/// `destroy_context`. The coroutine's bailout address is null here, so a
/// bailout in a destructor ends the process rather than longjmp.
/// # Safety
/// `context` came from `open` on the running, finishing coroutine.
pub(crate) unsafe fn close(context: *mut c_void) {
    // SAFETY: per the caller.
    let Some(ctx) = (unsafe { Ctx::of(context) }) else {
        return;
    };
    host::coro_hook(true, context);
    // SAFETY: output state is the coroutine's own.
    unsafe { swr_zend_coroutine_output_reset() };
    // Its defer tasks were kept and are not called.
    for task in ctx.defers(mem::take) {
        // SAFETY: a kept callable.
        unsafe { swr_zend_coroutine_fcc_discard(fcc(task)) };
        // SAFETY: emalloc'd by `Callable::parse`.
        unsafe { swr_zend_efree(task.cast()) };
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
    // SAFETY: a live context; `open` added this reference.
    let function_name = unsafe { &raw mut (*call).fci.function_name };
    // SAFETY: as above.
    unsafe { zval_ptr_dtor(function_name) };
    // SAFETY: as above.
    let undef = unsafe { &mut *function_name };
    undef.u1.type_info = IS_UNDEF;
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
        unsafe { host::timer_del(timer) };
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
    // SAFETY: its VM state.
    main.set_fiber(unsafe { swr_zend_coroutine_main(main.vm()) });
}

/// `free_main_context`, at RSHUTDOWN.
pub(crate) fn free_main() {
    // SAFETY: the thread's main context, which no reference reaches meanwhile.
    unsafe { *main().ptr() = Context::EMPTY };
}

/// `PHPCoroutine::defer`: keep a task on the running context.
pub(crate) fn defer(task: *mut c_void) {
    current().defers(|defers| defers.push(task.cast()));
}

fn fiber_init(ctx: Ctx) {
    // SAFETY: Zend's allocator, for one fiber context.
    let fiber =
        unsafe { swr_zend_emalloc(size_of::<zend_fiber_context>()) }.cast::<zend_fiber_context>();
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
    // SAFETY: the context's own fiber.
    unsafe { (*fiber).status = zend_fiber_status_ZEND_FIBER_STATUS_DEAD };
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
        STATE_INIT => zend_fiber_status_ZEND_FIBER_STATUS_INIT,
        STATE_WAITING => zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
        STATE_RUNNING => zend_fiber_status_ZEND_FIBER_STATUS_RUNNING,
        STATE_END => zend_fiber_status_ZEND_FIBER_STATUS_DEAD,
        _ => {
            host::fatal_error(
                swoole_rs_zend::E_ERROR,
                c"Unexpected state when get fiber status",
            );
            zend_fiber_status_ZEND_FIBER_STATUS_DEAD
        }
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
