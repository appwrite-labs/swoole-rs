//! Each coroutine's PHP context (upstream's `PHPContext`): the VM state it
//! swaps out while another runs, its callable and return value, defer tasks,
//! fiber notifications and bookkeeping. The core calls `on_yield`,
//! `on_resume` and `on_close` around every switch; the C++ main function
//! (which traps bailouts) calls `open`, `enter`, `defer_next` and `close`.
//!
//! Contexts are reached through raw pointers: another coroutine may change a
//! suspended one (`Co::cancel`, `Co::join`), so references to one never live
//! across a switch.
use crate::{host, runtime};
use std::{
    cell::UnsafeCell,
    ffi::{c_int, c_long, c_void},
    mem, ptr,
};
use swoole_rs_zend::sys::{
    IS_UNDEF, swr_zend_callable, swr_zend_callable_hold, swr_zend_coroutine_fcc_discard,
    swr_zend_coroutine_main, swr_zend_coroutine_output_reset, swr_zend_coroutine_restore,
    swr_zend_coroutine_set_frame, swr_zend_coroutine_start, swr_zend_coroutine_switch,
    swr_zend_coroutine_vm, swr_zend_coroutine_vm_destroy, swr_zend_efree, swr_zend_emalloc,
    swr_zend_object_release, swr_zval_try_addref, swr_zval_try_delref, zend_execute_data,
    zend_fcall_info, zend_fcall_info_cache, zend_fiber_context, zend_fiber_status,
    zend_fiber_status_ZEND_FIBER_STATUS_DEAD, zend_fiber_status_ZEND_FIBER_STATUS_INIT,
    zend_fiber_status_ZEND_FIBER_STATUS_RUNNING, zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
    zend_object, zend_observer_fiber_destroy_notify, zend_observer_fiber_init_notify,
    zend_observer_fiber_switch_notify, zval,
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
    pub(crate) vm: swr_zend_coroutine_vm,
    /// The coroutine's callable, laid out as `zend::Function`.
    call: swr_zend_callable,
    return_value: zval,
    pub(crate) co: *mut c_void,
    fiber: *mut zend_fiber_context,
    fiber_init_notified: bool,
    pub(crate) enable_scheduler: bool,
    defer: Vec<*mut swr_zend_callable>,
    /// The `Co::join()` waiting for this coroutine to close.
    pub(crate) join: *mut crate::methods::Join,
    pub(crate) pcid: c_long,
    /// `Coroutine\Context`, null until asked for, `INVALID` once destroyed.
    pub(crate) context: *mut zend_object,
    pub(crate) timer_limit: *mut c_void,
    pub(crate) last_msec: c_long,
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
        // SAFETY: all-zero bytes are a valid zval (IS_UNDEF).
        return_value: unsafe { mem::zeroed() },
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

/// `&raw mut (*ctx).vm`, without dereferencing.
#[inline]
pub(crate) fn vm(ctx: *mut Context) -> *mut swr_zend_coroutine_vm {
    ctx.wrapping_byte_add(mem::offset_of!(Context, vm)).cast()
}

/// `&raw mut (*ctx).call`.
#[inline]
fn call(ctx: *mut Context) -> *mut swr_zend_callable {
    ctx.wrapping_byte_add(mem::offset_of!(Context, call)).cast()
}

/// `&raw mut (*ctx).return_value`.
#[inline]
fn return_value(ctx: *mut Context) -> *mut zval {
    ctx.wrapping_byte_add(mem::offset_of!(Context, return_value))
        .cast()
}

/// `&raw mut (*callable).fcc`.
#[inline]
fn fcc(callable: *mut swr_zend_callable) -> *mut zend_fcall_info_cache {
    callable
        .wrapping_byte_add(mem::offset_of!(swr_zend_callable, fcc))
        .cast()
}

/// A short-lived reference to a live context.
/// # Safety
/// `ctx` is live, and the reference is dropped before any switch.
#[inline]
pub(crate) unsafe fn at<'a>(ctx: *mut Context) -> &'a mut Context {
    // SAFETY: per the caller.
    unsafe { &mut *ctx }
}

per_thread! {
    static MAIN: UnsafeCell<Context> = UnsafeCell::new(Context::EMPTY);
}

/// The thread's main context, live as long as the thread.
#[inline]
pub(crate) fn main() -> *mut Context {
    MAIN.with(UnsafeCell::get)
}

/// `Coroutine::get_current_task()`: the running coroutine's context, null in
/// the main context.
#[inline]
pub(crate) fn current_task() -> *mut Context {
    host::current_task().cast()
}

/// `PHPCoroutine::get_context()`: the running coroutine's, or the main one.
#[inline]
pub(crate) fn current() -> *mut Context {
    let task = current_task();
    if task.is_null() { main() } else { task }
}

/// `get_context_by_cid`: -1 is the main context; null when there is none.
pub(crate) fn by_cid(cid: c_long) -> *mut Context {
    if cid == -1 {
        return main();
    }
    host::task_by_cid(cid).cast()
}

/// `get_origin_context`: the context that resumed `ctx`'s coroutine.
#[inline]
fn origin(ctx: *mut Context) -> *mut Context {
    // SAFETY: a live coroutine context.
    let co = unsafe { at(ctx) }.co;
    let task = host::origin_task(co).cast::<Context>();
    if task.is_null() { main() } else { task }
}

/// `record_last_msec`.
#[inline]
pub(crate) fn record_last_msec(ctx: *mut Context) {
    if runtime::interrupt_running() {
        let now = host::absolute_msec();
        // SAFETY: a live context.
        unsafe { at(ctx) }.last_msec = now;
    }
}

/// `save_context(from)` then `restore_context(to)`.
#[inline]
fn switch(from: *mut Context, to: *mut Context) {
    // SAFETY: two live, distinct contexts.
    unsafe { swr_zend_coroutine_switch(vm(from), vm(to)) };
}

/// `PHPCoroutine::on_yield`.
pub(crate) unsafe extern "C" fn on_yield(arg: *mut c_void) {
    let ctx = arg.cast::<Context>();
    let origin = origin(ctx);
    fiber_switch_try_notify(ctx, origin);
    switch(ctx, origin);
    count_switch(ctx);
}

/// `PHPCoroutine::on_resume`.
pub(crate) unsafe extern "C" fn on_resume(arg: *mut c_void) {
    let ctx = arg.cast::<Context>();
    let current = current();
    fiber_switch_try_notify(current, ctx);
    switch(current, ctx);
    record_last_msec(ctx);
    count_switch(ctx);
}

#[inline]
fn count_switch(ctx: *mut Context) {
    // SAFETY: a live context.
    let this = unsafe { at(ctx) };
    if this.watch_switches {
        this.switch_count += 1;
    }
}

/// The tracer's blocking detection (upstream's `on_yield`/`on_resume`
/// callbacks counting `switch_count`).
/// # Safety
/// `task` is null or a live coroutine context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcoroutines_watch_switches(
    task: *mut c_void,
    watch: u8,
) -> usize {
    let ctx = if task.is_null() {
        current()
    } else {
        task.cast()
    };
    // SAFETY: per the caller, or the running context.
    let this = unsafe { at(ctx) };
    this.watch_switches = watch != 0;
    if this.watch_switches {
        this.switch_count = 0;
    }
    this.switch_count
}

/// `PHPCoroutine::on_close`: run the join hook, then free the context.
pub(crate) unsafe extern "C" fn on_close(arg: *mut c_void) {
    let ctx = arg.cast::<Context>();
    // SAFETY: the closing coroutine's context, not used after this.
    let join = unsafe { at(ctx) }.join;
    if !join.is_null() {
        crate::methods::Join::closed(join, ctx);
    }
    // SAFETY: as above; written by `open`.
    unsafe { ptr::drop_in_place(ctx) };
    // SAFETY: emalloc'd by `open`.
    unsafe { swr_zend_efree(ctx.cast()) };
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
    let fresh = Context {
        co,
        pcid,
        enable_scheduler: true,
        ..Context::EMPTY
    };
    // SAFETY: fresh storage for one context.
    unsafe { ctx.write(fresh) };
    host::set_task(co, ctx.cast());
    if host::flag(host::FIBER_MOCK) {
        fiber_init(ctx);
    }
    let (stack, size) = host::stack(co);
    // SAFETY: on the new coroutine, into its own context.
    unsafe { swr_zend_coroutine_start(vm(ctx), stack, size) };
    record_last_msec(ctx);
    // SAFETY: the context is live; nothing switches below.
    let this = unsafe { at(ctx) };
    // SAFETY: the creator's resolved callable.
    this.call.fcc = unsafe { *args.fcc };
    let fci = &mut this.call.fci;
    fci.size = size_of::<zend_fcall_info>();
    fci.object = ptr::null_mut();
    fci.param_count = args.argc;
    fci.params = args.argv;
    fci.named_params = ptr::null_mut();
    fci.retval = &raw mut this.return_value;
    if !args.callable.is_null() {
        // SAFETY: the creator's live callable zval.
        fci.function_name = unsafe { *args.callable };
        // SAFETY: the copy takes a reference.
        unsafe { swr_zval_try_addref(&raw mut fci.function_name) };
    }
    // SAFETY: a resolved callable (sw_zend_fci_cache_persist).
    unsafe { swr_zend_callable_hold(&raw mut this.call) };
    ctx.cast()
}

/// The start hook and the fiber switch into the coroutine's function; its
/// callable, to call (`zend::Function *`).
/// # Safety
/// `context` came from `open` on the running coroutine.
pub(crate) unsafe fn enter(context: *mut c_void) -> *mut swr_zend_callable {
    let ctx = context.cast::<Context>();
    host::coro_hook(false, context);
    let (fiber, handler) = {
        // SAFETY: a live context.
        let this = unsafe { at(ctx) };
        (this.fiber, this.call.fcc.function_handler)
    };
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
    call(ctx)
}

/// The next defer task (last in, first out), with the coroutine's return
/// value as its argument when it has one; null when none are left.
/// # Safety
/// `context` came from `open` on the running coroutine.
pub(crate) unsafe fn defer_next(context: *mut c_void) -> *mut swr_zend_callable {
    // SAFETY: a live context; nothing switches below.
    let this = unsafe { at(context.cast()) };
    let Some(task) = this.defer.pop() else {
        return ptr::null_mut();
    };
    // SAFETY: a zval's type tag.
    if unsafe { this.return_value.u1.type_info } != IS_UNDEF {
        // SAFETY: a kept callable.
        let task = unsafe { &mut *task };
        task.fci.param_count = 1;
        task.fci.params = &raw mut this.return_value;
    }
    task
}

/// `destroy_context`. The coroutine's bailout address is null here, so a
/// bailout in a destructor ends the process rather than longjmp.
/// # Safety
/// `context` came from `open` on the running, finishing coroutine.
pub(crate) unsafe fn close(context: *mut c_void) {
    let ctx = context.cast::<Context>();
    host::coro_hook(true, context);
    // SAFETY: output state is the coroutine's own.
    unsafe { swr_zend_coroutine_output_reset() };
    // SAFETY: a live context; its defer tasks were kept and are not called.
    let tasks = mem::take(&mut unsafe { at(ctx) }.defer);
    for task in tasks {
        // SAFETY: a kept callable.
        unsafe { swr_zend_coroutine_fcc_discard(fcc(task)) };
        // SAFETY: emalloc'd by `Callable::parse`.
        unsafe { swr_zend_efree(task.cast()) };
    }
    // Object destruction may yield and let the origin coroutine finish: the
    // origin is looked up again before each use.
    // SAFETY: a live context.
    let object = unsafe { at(ctx) }.context;
    if !object.is_null() {
        // SAFETY: as above.
        unsafe { at(ctx) }.context = ptr::without_provenance_mut(INVALID);
        // SAFETY: the context's own reference.
        unsafe { swr_zend_object_release(object) };
    }
    let call = call(ctx);
    // SAFETY: a live context; `open` added this reference.
    let function_name = unsafe { &raw mut (*call).fci.function_name };
    // SAFETY: as above.
    unsafe { swr_zval_try_delref(function_name) };
    // SAFETY: as above.
    let undef = unsafe { &mut *function_name };
    undef.u1.type_info = IS_UNDEF;
    // SAFETY: as above; `open` held its object and closure.
    unsafe { swr_zend_coroutine_fcc_discard(fcc(call)) };
    // SAFETY: a live context's return value.
    unsafe { swr_zval_try_delref(return_value(ctx)) };
    fiber_try_destroy(ctx, origin(ctx));
    // A context-owned __destruct() may have rearmed the time limit.
    // SAFETY: a live context.
    let timer = mem::replace(&mut unsafe { at(ctx) }.timer_limit, ptr::null_mut());
    if !timer.is_null() {
        host::timer_del(timer);
    }
    // SAFETY: the coroutine's own VM stack.
    unsafe { swr_zend_coroutine_vm_destroy() };
    let origin = origin(ctx);
    // SAFETY: the origin's live context.
    unsafe { swr_zend_coroutine_restore(vm(origin)) };
}

/// `init_main_context`, at RINIT.
pub(crate) fn init_main() {
    let main = main();
    // SAFETY: the thread's main context.
    let this = unsafe { at(main) };
    this.co = ptr::null_mut();
    this.fiber_init_notified = true;
    // SAFETY: its VM state.
    this.fiber = unsafe { swr_zend_coroutine_main(&raw mut this.vm) };
}

/// `free_main_context`, at RSHUTDOWN.
pub(crate) fn free_main() {
    // SAFETY: the thread's main context.
    *unsafe { at(main()) } = Context::EMPTY;
}

/// `PHPCoroutine::defer`: keep a task on the running context.
pub(crate) fn defer(task: *mut c_void) {
    // SAFETY: the running context.
    unsafe { at(current()) }.defer.push(task.cast());
}

fn fiber_init(ctx: *mut Context) {
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
    // SAFETY: a live context.
    unsafe { at(ctx) }.fiber = fiber;
    // SAFETY: a new fiber context.
    unsafe { zend_observer_fiber_init_notify(fiber) };
}

fn fiber_try_destroy(ctx: *mut Context, origin: *mut Context) {
    // SAFETY: a live context.
    let fiber = unsafe { at(ctx) }.fiber;
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

fn fiber_status(ctx: *mut Context) -> zend_fiber_status {
    let (fiber, co) = {
        // SAFETY: a live context.
        let this = unsafe { at(ctx) };
        (this.fiber, this.co)
    };
    // SAFETY: as above.
    if fiber == unsafe { at(main()) }.fiber {
        return zend_fiber_status_ZEND_FIBER_STATUS_RUNNING;
    }
    match host::state_of(co) {
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

fn fiber_switch_notify(from: *mut Context, to: *mut Context) {
    // SAFETY: a live context.
    let from_fiber = unsafe { at(from) }.fiber;
    // SAFETY: as above.
    let (to_fiber, notified) = (
        unsafe { at(to) }.fiber,
        unsafe { at(to) }.fiber_init_notified,
    );
    let from_status = fiber_status(from);
    // SAFETY: a live fiber.
    unsafe { (*from_fiber).status = from_status };
    let to_status = fiber_status(to);
    // SAFETY: a live fiber.
    unsafe { (*to_fiber).status = to_status };
    if notified {
        // SAFETY: two live fibers.
        unsafe { zend_observer_fiber_switch_notify(from_fiber, to_fiber) };
        return;
    }
    // SAFETY: a live fiber.
    unsafe { (*to_fiber).status = zend_fiber_status_ZEND_FIBER_STATUS_INIT };
    // SAFETY: two live fibers.
    unsafe { zend_observer_fiber_switch_notify(from_fiber, to_fiber) };
    let to_status = fiber_status(to);
    // SAFETY: a live fiber.
    unsafe { (*to_fiber).status = to_status };
    // SAFETY: a live context.
    unsafe { at(to) }.fiber_init_notified = true;
}

#[inline]
fn fiber_switch_try_notify(from: *mut Context, to: *mut Context) {
    // SAFETY: two live contexts.
    let from_fiber = unsafe { at(from) }.fiber;
    // SAFETY: as above.
    if !from_fiber.is_null() && !unsafe { at(to) }.fiber.is_null() {
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
        assert_eq!(vm(ctx), unsafe { &raw mut (*ctx).vm });
        assert_eq!(call(ctx), unsafe { &raw mut (*ctx).call });
        assert_eq!(return_value(ctx), unsafe { &raw mut (*ctx).return_value });
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
