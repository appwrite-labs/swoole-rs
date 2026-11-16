//! Each coroutine's PHP context (upstream's `PHPContext`): the VM state it
//! swaps out while another runs, its callable and return value, defer tasks,
//! fiber notifications and bookkeeping. The core calls `on_yield`,
//! `on_resume` and `on_close` around every switch; the C++ main function
//! (which traps bailouts) calls `open`, `enter`, `defer_next` and `close`.
//!
//! Contexts are reached through a [`Ctx`] handle, which reads and writes one
//! field at a time: another coroutine may change a suspended context
//! (`Co::cancel`, `Co::join`), so no reference to one lives across a switch.
use crate::{host, methods::Join, put_back, runtime};
use std::{
    cell::{Cell, UnsafeCell},
    ffi::{c_int, c_long, c_void},
    mem,
    ptr::{self, NonNull},
    rc::Rc,
};
use swoole_rs_coroutine::{CoroutineTask, State};
use swoole_rs_timer::Held;
use swoole_rs_zend::sys::{
    IS_UNDEF, swr_zend_callable, swr_zend_callable_hold, swr_zend_coroutine_fcc_discard,
    swr_zend_coroutine_output_reset, swr_zend_coroutine_restore, swr_zend_coroutine_set_frame,
    swr_zend_coroutine_start, swr_zend_coroutine_switch, swr_zend_coroutine_vm,
    swr_zend_coroutine_vm_destroy, swr_zend_efree, swr_zend_object_release, zend_class_entry,
    zend_execute_data, zend_fcall_info, zend_fcall_info_cache, zend_fiber_context,
    zend_fiber_status, zend_fiber_status_ZEND_FIBER_STATUS_DEAD,
    zend_fiber_status_ZEND_FIBER_STATUS_INIT, zend_fiber_status_ZEND_FIBER_STATUS_RUNNING,
    zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED, zend_object, zval, zval_ptr_dtor,
};
use swoole_rs_zend::{Callable, Function, Value, bailout};

unsafe extern "C" {
    /// `save_context`: this PHP thread's executor, output and serializer state,
    /// moved into `vm`, with a fresh output layer started in their place. It
    /// follows no pointer `vm` holds and runs no PHP code.
    safe fn swr_zend_coroutine_save(vm: &mut swr_zend_coroutine_vm);
    /// `init_main_context`'s VM half: clears `EG(opline_before_exception)`, saves
    /// as [`swr_zend_coroutine_save`] does into `vm`, and returns
    /// `EG(main_fiber_context)`, which lives for the request. It follows no
    /// pointer `vm` holds and runs no PHP code.
    safe fn swr_zend_coroutine_main(vm: &mut swr_zend_coroutine_vm) -> Option<Fiber>;
    /// The fiber observers' notice of a new fiber.
    safe fn zend_observer_fiber_init_notify(fiber: Fiber);
    /// The fiber observers' notice of a switch from one live fiber to another.
    safe fn zend_observer_fiber_switch_notify(from: Fiber, to: Fiber);
    /// The fiber observers' notice that a live fiber goes; it stays allocated.
    safe fn zend_observer_fiber_destroy_notify(fiber: Fiber);
}

/// A live fiber context: the main context's `EG(main_fiber_context)`, or a
/// coroutine's mock one from [`fiber_init`] until [`fiber_try_destroy`] frees
/// it. Only those make one.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fiber(NonNull<zend_fiber_context>);

impl Fiber {
    /// Set its status, as Zend's fiber observers read it.
    fn set_status(self, status: zend_fiber_status) {
        // SAFETY: a live fiber (the handle's invariant); a field write.
        unsafe { (*self.0.as_ptr()).status = status };
    }
}

/// Upstream's `INVALID_PTR`: a destroyed context object, or an unusable fiber
/// field.
pub(crate) const INVALID: usize = usize::MAX;

/// `zend_function.type`: `ZEND_INTERNAL_FUNCTION` and `ZEND_USER_FUNCTION`.
pub(crate) const INTERNAL_FUNCTION: u8 = 1;
pub(crate) const USER_FUNCTION: u8 = 2;

/// `PHPCoroutine::Args`, built by `create` for the new coroutine's entry, which
/// reads them before its first switch, while `create`'s frame still lends them.
pub(crate) struct Args<'a> {
    /// The resolved callable.
    pub(crate) fcc: &'a zend_fcall_info_cache,
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
    /// The coroutine, which outlives its context; none for the main context.
    co: Option<host::Co>,
    fiber: Option<Fiber>,
    fiber_init_notified: bool,
    enable_scheduler: bool,
    /// The defer tasks, each kept by `defer()` until it runs or the context
    /// closes; none until the first.
    #[expect(
        clippy::box_collection,
        reason = "boxed, all-zero bytes are none: `open` builds the context with one zero fill"
    )]
    defer: Option<Box<Vec<Callable>>>,
    /// The `Co::join()` waiting for this coroutine to close.
    join: Option<Rc<Join>>,
    pcid: c_long,
    /// `Coroutine\Context`, null until asked for, `INVALID` once destroyed.
    context: *mut zend_object,
    /// Whether [`TIME_LIMITS`] holds a `Co::setTimeLimit` timer of this coroutine.
    time_limited: bool,
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
        co: None,
        fiber: None,
        fiber_init_notified: false,
        enable_scheduler: false,
        defer: None,
        join: None,
        pcid: 0,
        context: ptr::null_mut(),
        time_limited: false,
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

/// The frame Zend ran before a swap to another one, made current again on drop.
/// Only [`Ctx::in_frame`] and [`enter`] make one, of `swr_zend_coroutine_set_frame`'s
/// result, and they neither switch coroutines nor bail out until it drops.
struct Restore(*mut zend_execute_data);

impl Drop for Restore {
    fn drop(&mut self) {
        // SAFETY: the running coroutine's own frame, current when this was made.
        unsafe { swr_zend_coroutine_set_frame(self.0) };
    }
}

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
    co, set_co: Option<host::Co>;
    fiber, set_fiber: Option<Fiber>;
    fiber_init_notified, set_fiber_init_notified: bool;
    enable_scheduler, set_enable_scheduler: bool;
    pcid, set_pcid: c_long;
    context, set_context: *mut zend_object;
    time_limited, set_time_limited: bool;
    last_msec, set_last_msec: c_long;
    watch_switches, set_watch_switches: bool;
    switch_count, set_switch_count: usize;
}

impl Ctx {
    /// A coroutine's task, which is its live context: `open` sets every task, and
    /// `on_close` frees it as the coroutine closes.
    #[inline]
    pub(crate) fn of_task(task: CoroutineTask) -> Self {
        Self(task.as_ptr().cast())
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
        let _running = Restore(unsafe { swr_zend_coroutine_set_frame(self.execute_data()) });
        f()
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

    /// Whether a `Co::join()` waits for this context to close.
    pub(crate) fn joined(self) -> bool {
        self.with(|ctx| ctx.join.is_some())
    }

    /// The `Co::join()` waiting for this context becomes `join`; the one it
    /// replaces.
    pub(crate) fn replace_join(self, join: Option<Rc<Join>>) -> Option<Rc<Join>> {
        self.with(|ctx| mem::replace(&mut ctx.join, join))
    }

    /// The defer tasks, borrowed for `f`, as [`Ctx::with`].
    fn defers<R>(self, f: impl FnOnce(&mut Option<Box<Vec<Callable>>>) -> R) -> R {
        self.with(|ctx| f(&mut ctx.defer))
    }
}

per_thread! {
    static MAIN: UnsafeCell<Context> = UnsafeCell::new(Context::EMPTY);
    /// The armed `Co::setTimeLimit` timers, by coroutine id: few at a time, so a
    /// list. Taken out and put back around one swap, which releases nothing.
    static TIME_LIMITS: Cell<Vec<(c_long, Held)>> = Cell::new(Vec::new());
}

/// Coroutine `cid`'s time-limit timer becomes `timer` (none: it fired, or the
/// coroutine closes); the armed one it replaces, for the caller to delete.
pub(crate) fn swap_time_limit(ctx: Ctx, cid: c_long, timer: Option<Held>) -> Option<Held> {
    let limited = ctx.time_limited();
    ctx.set_time_limited(timer.is_some());
    TIME_LIMITS.with(|limits| {
        put_back(limits, |limits| {
            let old = limits
                .iter()
                .position(|&(owner, _)| limited && owner == cid)
                .map(|at| limits.swap_remove(at).1);
            limits.extend(timer.map(|timer| (cid, timer)));
            old
        })
    })
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
pub(crate) extern "C" fn on_yield(task: CoroutineTask) {
    let ctx = Ctx::of_task(task);
    let origin = origin(ctx);
    fiber_switch_try_notify(ctx, origin);
    switch(ctx, origin);
    count_switch(ctx);
}

/// `PHPCoroutine::on_resume`.
pub(crate) extern "C" fn on_resume(task: CoroutineTask) {
    let ctx = Ctx::of_task(task);
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
pub(crate) extern "C" fn on_close(task: CoroutineTask) {
    let ctx = Ctx::of_task(task);
    if let Some(join) = ctx.replace_join(None) {
        join.closed(ctx);
    }
    // Tasks a bailout left; with the join, all the context holds that needs a drop.
    drop(ctx.defers(Option::take));
    // SAFETY: emalloc'd by `open`, and not used after this.
    unsafe { swr_zend_efree(ctx.ptr().cast()) };
}

/// `create_context`, on the new coroutine: its context, live until [`on_close`].
/// # Safety
/// `args` is the creator's live `Args`, and this runs on the new coroutine.
pub(crate) unsafe fn open(args: *mut c_void) -> Option<Ctx> {
    // SAFETY: per the caller.
    let args = unsafe { &*args.cast::<Args<'_>>() };
    let co = host::current()?;
    let pcid = host::origin_cid(co);
    let ctx = swoole_rs_zend::value::emalloc(size_of::<Context>()).cast::<Context>();
    // Built in place: a `Context::EMPTY` literal is staged on the stack and
    // copied over. All-zero bytes are `Context::EMPTY`.
    // SAFETY: fresh storage for one context.
    unsafe { ctx.write_bytes(0, 1) };
    // Every field now holds a valid value, and the context lives until `on_close`.
    let this = Ctx(NonNull::new(ctx)?);
    this.set_co(Some(co));
    this.set_pcid(pcid);
    this.set_enable_scheduler(true);
    // SAFETY: a new PHP context, the kind `on_yield`, `on_resume` and `on_close`
    // take, which lives until `on_close` frees it.
    unsafe { swoole_rs_coroutine::set_current_task(this.0.cast()) };
    if host::flag(host::FIBER_MOCK) {
        fiber_init(this);
    }
    let (stack, size) = host::stack(co);
    // SAFETY: on the new coroutine, into its own context.
    unsafe { swr_zend_coroutine_start(this.vm(), stack, size) };
    record_last_msec(this);
    let retval = this.return_value();
    this.with(|context| {
        context.call.fcc = *args.fcc;
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
    if fiber.is_some()
        && let Some(function) = function.filter(|function| function.kind() == Function::USER)
        && let Some(opcodes) = function.opcodes()
    {
        let mut frame: zend_execute_data = swoole_rs_zend::zeroed();
        // A user function is its op array.
        frame.opline = opcodes;
        frame.func = handler;
        // SAFETY: the frame outlives the notification, and `_previous`, which
        // drops before it.
        let _previous = Restore(unsafe { swr_zend_coroutine_set_frame(&raw mut frame) });
        fiber_switch_try_notify(origin(ctx), ctx);
    }
    ctx.call()
}

/// The next defer task (last in, first out), and the coroutine's return value,
/// its one argument, when it has one; none when no task is left.
#[inline]
pub(crate) fn defer_next(ctx: Ctx) -> Option<(Callable, Option<*mut zval>)> {
    let task = ctx.defers(|defers| defers.as_mut()?.pop())?;
    let returned = ctx.with(|context| Value::of(&mut context.return_value).kind()) != IS_UNDEF;
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
    for task in ctx
        .defers(Option::take)
        .map_or_else(Vec::new, |tasks| *tasks)
    {
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
    if ctx.time_limited()
        && let Some(co) = ctx.co()
        && let Some(timer) = swap_time_limit(ctx, host::cid(co), None)
    {
        swoole_rs_timer::del_id(timer);
    }
    // SAFETY: the coroutine's own VM stack.
    unsafe { swr_zend_coroutine_vm_destroy() };
    // SAFETY: the origin's live context.
    unsafe { swr_zend_coroutine_restore(origin(ctx).vm()) };
}

/// `init_main_context`, at RINIT.
pub(crate) fn init_main() {
    let main = main();
    main.set_co(None);
    main.set_fiber_init_notified(true);
    // At RINIT: no exception is being dispatched.
    main.set_fiber(main.with_vm(|vm| swr_zend_coroutine_main(vm)));
}

/// `free_main_context`, at RSHUTDOWN.
pub(crate) fn free_main() {
    main().with(|context| *context = Context::EMPTY);
}

/// `PHPCoroutine::defer`: keep a task on the running context.
pub(crate) fn defer(task: Callable) {
    current().defers(|defers| defers.get_or_insert_default().push(task));
}

fn fiber_init(ctx: Ctx) {
    let invalid = ptr::without_provenance_mut::<c_void>(INVALID);
    // These fields are never used for a mock fiber.
    let mut this: zend_fiber_context = swoole_rs_zend::zeroed();
    this.handle = invalid;
    this.kind = invalid;
    this.function = Some(never_switched);
    this.stack = invalid.cast();
    let fiber = swoole_rs_zend::value::enew(this);
    // A new fiber context, which lives until `fiber_try_destroy`.
    let Some(fiber) = NonNull::new(fiber).map(Fiber) else {
        return;
    };
    ctx.set_fiber(Some(fiber));
    zend_observer_fiber_init_notify(fiber);
}

/// A mock fiber's function (upstream's `INVALID_PTR`): nothing switches to one.
extern "C" fn never_switched(_: *mut swoole_rs_zend::sys::zend_fiber_transfer) {
    swoole_rs_corebase::fatal("a coroutine's mock fiber was switched to");
}

fn fiber_try_destroy(ctx: Ctx, origin: Ctx) {
    let Some(fiber) = ctx.fiber() else {
        return;
    };
    fiber.set_status(zend_fiber_status_ZEND_FIBER_STATUS_DEAD);
    fiber_switch_try_notify(ctx, origin);
    ctx.set_fiber(None);
    zend_observer_fiber_destroy_notify(fiber);
    // SAFETY: emalloc'd by `fiber_init`, and no one uses it after this.
    unsafe { swr_zend_efree(fiber.0.as_ptr().cast()) };
}

fn fiber_status(ctx: Ctx) -> zend_fiber_status {
    if ctx.fiber() == main().fiber() {
        return zend_fiber_status_ZEND_FIBER_STATUS_RUNNING;
    }
    let Some(co) = ctx.co() else {
        return zend_fiber_status_ZEND_FIBER_STATUS_RUNNING;
    };
    match host::state_of(co) {
        State::Init => zend_fiber_status_ZEND_FIBER_STATUS_INIT,
        State::Waiting => zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
        State::Running => zend_fiber_status_ZEND_FIBER_STATUS_RUNNING,
        State::End => zend_fiber_status_ZEND_FIBER_STATUS_DEAD,
    }
}

fn fiber_switch_notify(from: (Ctx, Fiber), to: (Ctx, Fiber)) {
    let ((from, from_fiber), (to, to_fiber)) = (from, to);
    let notified = to.fiber_init_notified();
    from_fiber.set_status(fiber_status(from));
    to_fiber.set_status(fiber_status(to));
    if !notified {
        to_fiber.set_status(zend_fiber_status_ZEND_FIBER_STATUS_INIT);
    }
    zend_observer_fiber_switch_notify(from_fiber, to_fiber);
    if !notified {
        to_fiber.set_status(fiber_status(to));
        to.set_fiber_init_notified(true);
    }
}

#[inline]
fn fiber_switch_try_notify(from: Ctx, to: Ctx) {
    if let (Some(from_fiber), Some(to_fiber)) = (from.fiber(), to.fiber()) {
        fiber_switch_notify((from, from_fiber), (to, to_fiber));
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
        assert!(context.defer.is_none());
        assert!(context.join.is_none() && context.context.is_null() && !context.time_limited);
        assert_eq!(unsafe { context.return_value.u1.type_info }, IS_UNDEF);
        assert!(!context.vm.in_silence && context.vm.output_ptr.is_null());
    }
}
