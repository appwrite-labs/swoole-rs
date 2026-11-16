//! Each coroutine's PHP context (upstream's `PHPContext`): the VM state it
//! swaps out while another runs, its callable and return value, defer tasks,
//! fiber notifications and bookkeeping. The core calls `on_yield` and
//! `on_resume` around every switch; [`run`], the coroutine's body, owns its
//! context in its frame and runs it through `open`, `enter`, `defer_next` and
//! `close`.
//!
//! Contexts are reached through a [`Ctx`] handle, which reads and writes one
//! field at a time: another coroutine may change a suspended context
//! (`Co::cancel`, `Co::join`), so no reference to one lives across a switch.
//!
//! Zend's executor state belongs to one context at a time, the running one;
//! every other keeps its own saved in its `vm`. The shims that move it take
//! that as types: [`Running`] for the context whose state is the executor's,
//! [`Suspended`] for one whose state is saved.
use crate::{classes::ExceptionClass, host, methods::Join, put_back, runtime};
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
    IS_NULL, IS_STRING, IS_UNDEF, swr_zend_callable, swr_zend_callable_hold,
    swr_zend_coroutine_fcc_discard, swr_zend_coroutine_output_reset, swr_zend_coroutine_set_frame,
    swr_zend_coroutine_vm, zend_class_entry, zend_execute_data, zend_fcall_info,
    zend_fcall_info_cache, zend_fiber_context, zend_fiber_status,
    zend_fiber_status_ZEND_FIBER_STATUS_DEAD, zend_fiber_status_ZEND_FIBER_STATUS_INIT,
    zend_fiber_status_ZEND_FIBER_STATUS_RUNNING, zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
    zend_object, zval,
};
use swoole_rs_zend::{Callable, Function, Owned, Value, bailout};

unsafe extern "C" {
    /// `init_main_context`'s VM half: clears `EG(opline_before_exception)`,
    /// moves this PHP thread's executor, output and serializer state into `vm`
    /// (`save_context`) and returns `EG(main_fiber_context)`, which lives for
    /// the request. It follows no pointer `vm` holds and runs no PHP code.
    safe fn swr_zend_coroutine_main(vm: &mut swr_zend_coroutine_vm) -> Option<Fiber>;
    /// `save_context(from)` then `restore_context(to)`: the executor's state
    /// becomes `to`'s, and `from`'s is saved for its next switch in.
    safe fn swr_zend_coroutine_switch(from: Running, to: Suspended);
    /// `save_context(from)`, then a fresh VM stack for the new coroutine,
    /// bounded by its C stack (`stack`, `size`, only compared with the stack
    /// pointer; null: none), saved into `vm` too, which it writes without
    /// reading but for `in_silence`.
    safe fn swr_zend_coroutine_start(
        from: Running,
        vm: &mut swr_zend_coroutine_vm,
        stack: *mut c_void,
        size: usize,
    );
    /// `destroy_context`'s end: frees the finishing context's VM stack, which
    /// its saved state stops naming, then restores `origin`'s.
    safe fn swr_zend_coroutine_finish(finished: Running, origin: Suspended);
    /// Makes the frame the suspended context stopped in Zend's current one, as
    /// its backtraces and exception locations read it; the running one.
    safe fn swr_zend_coroutine_show(vm: Suspended) -> Shown;
    /// Makes the running frame current again; the one shown.
    #[link_name = "swr_zend_coroutine_set_frame"]
    safe fn unshow(running: Shown) -> *mut zend_execute_data;
    /// Make the frame the suspended context stopped in (if any) throw on
    /// resume, as `zend_throw_exception_internal` does for a user frame.
    safe fn swr_zend_coroutine_redirect(vm: Suspended);
    /// The VM stack bytes in use down the suspended context's saved stack
    /// chain, or the running one's for none.
    safe fn swr_zend_coroutine_stack_usage(vm: Option<Suspended>) -> usize;
    /// A new object of `class`, its properties initialised and no constructor
    /// (nor `create_object`) run: Zend's allocator never returns null.
    safe fn swr_zend_coroutine_exception(class: ExceptionClass) -> Exception;
    /// The exception's file and line, from the frame Zend runs.
    safe fn swr_zend_coroutine_locate(exception: Lent<'_>);
    /// Hands `exception` to Zend as the running frame's pending exception.
    #[link_name = "zend_throw_exception_internal"]
    safe fn throw(exception: Exception);
    /// Makes `previous` the end of `exception`'s chain, which takes its
    /// reference: both are throwables, and `exception` is new, so no release
    /// (nor PHP code) runs.
    #[link_name = "zend_exception_set_previous"]
    safe fn chain(exception: Lent<'_>, previous: Exception);
    /// The fiber observers' notice of a new fiber.
    safe fn zend_observer_fiber_init_notify(fiber: Fiber);
    /// The fiber observers' notice of a switch from one live fiber to another.
    safe fn zend_observer_fiber_switch_notify(from: Fiber, to: Fiber);
    /// The fiber observers' notice that a live fiber goes; it stays allocated.
    safe fn zend_observer_fiber_destroy_notify(fiber: Fiber);
    /// Sets a live fiber's status, as Zend's fiber observers read it.
    #[link_name = "swr_zend_coroutine_fiber_status"]
    safe fn set_fiber_status(fiber: Fiber, status: zend_fiber_status);
    /// `efree` of a mock fiber, which the handle spends.
    #[link_name = "swr_zend_efree"]
    safe fn free_mock(fiber: Mock);
}

/// A live fiber context: the main context's `EG(main_fiber_context)`, or a
/// coroutine's mock one from [`fiber_init`] until [`fiber_try_destroy`] frees
/// it. Only those make one.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fiber(NonNull<zend_fiber_context>);

/// A coroutine's mock fiber, which [`fiber_init`] allocates and its context
/// holds until [`fiber_try_destroy`] frees it: never copied, so freed once.
#[repr(transparent)]
pub(crate) struct Mock(Fiber);

impl Fiber {
    /// Set its status, as Zend's fiber observers read it.
    fn set_status(self, status: zend_fiber_status) {
        set_fiber_status(self, status);
    }
}

/// The running context's VM state: Zend's executor state is that context's,
/// and its `vm` is stale until the switch out of it, which spends this. Made
/// only by [`Ctx::running`], at this module's switch points.
#[repr(transparent)]
struct Running(NonNull<swr_zend_coroutine_vm>);

/// A live context's VM state as its last switch out saved it, which a switch
/// in restores: any context but the running one. Made only by
/// [`Ctx::suspended`], which checks, or [`Ctx::switched_out`], at this
/// module's switch points; kept to the frame that made it, which runs only
/// while its own context does.
#[repr(transparent)]
struct Suspended(NonNull<swr_zend_coroutine_vm>);

/// A new exception object, with the one reference it was made with.
#[repr(transparent)]
pub(crate) struct Exception(NonNull<zend_object>);

/// An [`Exception`], lent to Zend for one call.
#[repr(transparent)]
struct Lent<'a>(
    NonNull<zend_object>,
    std::marker::PhantomData<&'a Exception>,
);

impl Exception {
    /// A new object of `class`, with no constructor run.
    pub(crate) fn new(class: ExceptionClass) -> Self {
        swr_zend_coroutine_exception(class)
    }

    /// The object, whose reference passes to what keeps the pointer.
    fn into_raw(self) -> *mut zend_object {
        self.0.as_ptr()
    }

    fn lend(&self) -> Lent<'_> {
        Lent(self.0, std::marker::PhantomData)
    }

    /// Its file and line, of the frame Zend runs.
    pub(crate) fn locate(&self) {
        swr_zend_coroutine_locate(self.lend());
    }

    /// Thrown in the running frame, which takes its reference.
    pub(crate) fn throw(self) {
        throw(self);
    }
}

/// Upstream's `INVALID_PTR`: a destroyed context object, or an unusable fiber
/// field.
pub(crate) const INVALID: usize = usize::MAX;

/// `zend_function.type`: `ZEND_INTERNAL_FUNCTION` and `ZEND_USER_FUNCTION`.
pub(crate) const INTERNAL_FUNCTION: u8 = 1;
pub(crate) const USER_FUNCTION: u8 = 2;

/// `PHPCoroutine::Args`, built by `create` for the new coroutine's entry, which
/// copies them in before its first switch, while `create`'s frame still lends
/// the arguments and callable they point to.
#[derive(Clone, Copy)]
pub(crate) struct Args {
    /// The resolved callable.
    pub(crate) fcc: zend_fcall_info_cache,
    pub(crate) argv: *mut zval,
    pub(crate) argc: u32,
    pub(crate) callable: *mut zval,
}

/// One coroutine's PHP context; the main context is a thread-local one.
pub(crate) struct Context {
    vm: swr_zend_coroutine_vm,
    /// The coroutine's callable, laid out as `zend::Function`; its
    /// `function_name` stays undefined, since `callable` holds that reference.
    call: swr_zend_callable,
    /// The callable zval's reference (upstream's `Z_TRY_ADDREF` of it).
    callable: Owned,
    return_value: Owned,
    /// The coroutine, which outlives its context; none for the main context.
    co: Option<host::Co>,
    fiber: Option<Fiber>,
    /// The coroutine's own fiber, which `fiber` names while it lives.
    mock: Option<Mock>,
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
    /// `Coroutine\Context`'s object: undefined until asked for, null once
    /// destroyed (upstream's `INVALID_PTR`).
    context: Owned,
    /// Whether [`TIME_LIMITS`] holds a `Co::setTimeLimit` timer of this coroutine.
    time_limited: bool,
    last_msec: c_long,
    /// The tracer's blocking detection counts this context's switches.
    watch_switches: bool,
    switch_count: usize,
}

impl Context {
    const fn empty() -> Self {
        Self {
            vm: swoole_rs_zend::zeroed(),
            call: swoole_rs_zend::zeroed(),
            callable: Owned::empty(),
            return_value: Owned::empty(),
            co: None,
            fiber: None,
            mock: None,
            fiber_init_notified: false,
            enable_scheduler: false,
            defer: None,
            join: None,
            pcid: 0,
            context: Owned::empty(),
            time_limited: false,
            last_msec: 0,
            watch_switches: false,
            switch_count: 0,
        }
    }
}

/// `&raw mut (*callable).fcc`.
#[inline]
fn fcc(callable: *mut swr_zend_callable) -> *mut zend_fcall_info_cache {
    callable
        .wrapping_byte_add(mem::offset_of!(swr_zend_callable, fcc))
        .cast()
}

/// A live context: the thread's main one, or a coroutine's from [`open`] until
/// [`run`] returns. Only this module makes one, from the thread-local main
/// context and the host's task pointers. Each method borrows the context for
/// that one access and keeps no reference.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ctx(NonNull<Context>);

/// A live context's callable, which `open` resolved and held: only [`enter`]
/// makes one, for Zend to call while the coroutine runs.
#[repr(transparent)]
pub(crate) struct Call(NonNull<swr_zend_callable>);

/// The running frame, set aside while Zend's current one is another's: only
/// `swr_zend_coroutine_show` and [`enter`] make one, and hand it to
/// [`unshow`] before anything switches coroutines.
#[repr(transparent)]
struct Shown(*mut zend_execute_data);

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
    fn vm(self) -> NonNull<swr_zend_coroutine_vm> {
        self.0
            .map_addr(|at| at.saturating_add(mem::offset_of!(Context, vm)))
            .cast()
    }

    /// Its VM state as the running context's. Only the switch points below
    /// call this, each for the context whose state Zend's executor holds there.
    #[inline]
    fn running(self) -> Running {
        Running(self.vm())
    }

    /// Its VM state as a suspended context's. Only the switch points below
    /// call this, each for a context the scheduler has switched out.
    #[inline]
    fn switched_out(self) -> Suspended {
        Suspended(self.vm())
    }

    /// Its saved VM state, unless it is the running context.
    fn suspended(self) -> Option<Suspended> {
        (self != current()).then(|| self.switched_out())
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

    /// Run `f` with that frame as Zend's current one, as its backtraces and
    /// exception locations read it, then restore the running frame; `f` just
    /// runs for the running context, whose saved frame is stale. `f` neither
    /// switches coroutines nor bails out.
    pub(crate) fn in_frame<R>(self, f: impl FnOnce() -> R) -> R {
        let Some(suspended) = self.suspended() else {
            return f();
        };
        // Its saved frame stays live until it resumes, which `f` does not let it.
        let running = swr_zend_coroutine_show(suspended);
        let result = f();
        unshow(running);
        result
    }

    /// `get_stack_usage`: the VM stack bytes its chain holds.
    pub(crate) fn stack_usage(self) -> usize {
        swr_zend_coroutine_stack_usage(self.suspended())
    }

    /// The class of the exception a cancellation left pending, if one is.
    pub(crate) fn pending_class(self) -> Option<*mut zend_class_entry> {
        self.with_vm(|vm| (!vm.exception.is_null()).then_some(vm.exception_class))
    }

    /// Leave `exception` of class `class` pending in a suspended context,
    /// thrown when it resumes: chained after the one pending before, or, with
    /// none, its frame made to throw. Nothing for the running context.
    pub(crate) fn set_exception(self, exception: Exception, class: ExceptionClass) {
        let Some(suspended) = self.suspended() else {
            return;
        };
        // The pending exception's reference passes from the saved state to the
        // chain, and the new one's to the saved state.
        let previous = self.with_vm(|vm| NonNull::new(mem::take(&mut vm.exception)));
        let redirect = previous.is_none();
        if let Some(previous) = previous {
            chain(exception.lend(), Exception(previous));
        }
        self.with_vm(|vm| {
            vm.exception = exception.into_raw();
            vm.exception_class = class.as_ptr();
        });
        if redirect {
            swr_zend_coroutine_redirect(suspended);
        }
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

    /// A new reference to its `Coroutine\Context` object, made on the first ask
    /// (null when Zend made none); none once destroyed.
    pub(crate) fn context_object(self) -> Option<Owned> {
        let kind = self.with(|ctx| ctx.context.kind());
        if kind == IS_NULL {
            return None;
        }
        if kind == IS_UNDEF {
            let created = crate::classes::CONTEXT.create_object();
            if created.object().is_none() {
                return Some(Owned::null());
            }
            // The context takes the object's first reference.
            self.with(|ctx| ctx.context = created);
        }
        Some(self.with(|ctx| ctx.context.copy()))
    }
}

per_thread! {
    static MAIN: UnsafeCell<Context> = UnsafeCell::new(Context::empty());
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

/// `PHPCoroutine::on_yield`, which the scheduler calls for the running
/// coroutine (or, bailing out, its outermost one, whose saved state the bailout
/// abandons with every suspended coroutine's) before switching to its origin.
pub(crate) extern "C" fn on_yield(task: CoroutineTask) {
    let ctx = Ctx::of_task(task);
    let origin = origin(ctx);
    fiber_switch_try_notify(ctx, origin);
    swr_zend_coroutine_switch(ctx.running(), origin.switched_out());
    count_switch(ctx);
}

/// `PHPCoroutine::on_resume`, which the scheduler calls on the running
/// context's stack before switching to `task`'s suspended coroutine.
pub(crate) extern "C" fn on_resume(task: CoroutineTask) {
    let ctx = Ctx::of_task(task);
    let current = current();
    fiber_switch_try_notify(current, ctx);
    swr_zend_coroutine_switch(current.running(), ctx.switched_out());
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

/// `main_func`, the new coroutine's body: its context lives in this frame,
/// opened, run (the function and defer tasks inside `zend_first_try`) and
/// closed. Whether an exception or a bailout ended it.
pub(crate) fn run(args: Args) -> bool {
    // `close` drops everything the context holds that needs a drop, so the frame
    // only frees its bytes, as upstream's `efree` does, with no drop pass.
    let mut context = mem::ManuallyDrop::new(Context::empty());
    let Some(ctx) = open(args, &mut context) else {
        return false;
    };
    let caught = host::run(ctx);
    close(ctx);
    // The context goes with this frame.
    swoole_rs_coroutine::clear_current_task();
    caught
}

/// `create_context`, on the new coroutine, in `context`, live until [`run`]
/// returns; the creator's executor state, still the executor's, is saved.
fn open(args: Args, context: &mut Context) -> Option<Ctx> {
    let co = host::current()?;
    let pcid = host::origin_cid(co);
    let this = Ctx(NonNull::from(context));
    this.set_co(Some(co));
    this.set_pcid(pcid);
    this.set_enable_scheduler(true);
    // SAFETY: a new PHP context, the kind `on_yield` and `on_resume` take,
    // which lives until `run` clears the task.
    unsafe { swoole_rs_coroutine::set_current_task(this.0.cast()) };
    if host::flag(host::FIBER_MOCK) {
        fiber_init(this);
    }
    let (stack, size) = host::stack(co);
    // The creator switched to this coroutine without a swap callback: its
    // state is still the executor's, saved here.
    let creator = origin(this).running();
    this.with_vm(|vm| swr_zend_coroutine_start(creator, vm, stack, size));
    record_last_msec(this);
    let retval = this.return_value();
    this.with(|context| {
        context.call.fcc = args.fcc;
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
            context.callable = callable.copy();
        }
    });
    // SAFETY: a resolved callable (sw_zend_fci_cache_persist).
    unsafe { swr_zend_callable_hold(this.call()) };
    Some(this)
}

/// The start hook and the fiber switch into the running coroutine's function;
/// its callable, to call (`zend::Function *`).
pub(crate) extern "C" fn enter(ctx: Ctx) -> Option<Call> {
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
        // SAFETY: a frame of the coroutine's function, which outlives the
        // notification, after which the running one is current again.
        let running = Shown(unsafe { swr_zend_coroutine_set_frame(&raw mut frame) });
        fiber_switch_try_notify(origin(ctx), ctx);
        unshow(running);
    }
    NonNull::new(ctx.call()).map(Call)
}

/// The next defer task (last in, first out), and a copy of the coroutine's
/// return value, its one argument, when it has one; none when no task is left.
#[inline]
pub(crate) fn defer_next(ctx: Ctx) -> Option<(Callable, Option<Owned>)> {
    let task = ctx.defers(|defers| defers.as_mut()?.pop())?;
    let returned = ctx.with(|context| {
        (context.return_value.kind() != IS_UNDEF).then(|| context.return_value.copy())
    });
    Some((task, returned))
}

/// Drops `value`, raising a destructor's bailout at once. Only an object, array,
/// reference or resource (above a string) can run one, so only those are guarded.
fn release(value: Owned) {
    if value.kind() > IS_STRING {
        bailout::finish_after(|| drop(value));
    }
}

/// `destroy_context`, on the running, finishing coroutine, then
/// `PHPCoroutine::on_close`'s join hook. Its bailout address is null here, so
/// a bailout in a destructor ends the process rather than longjmp.
fn close(ctx: Ctx) {
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
    // Each release below is the coroutine's last, and a destructor's bailout
    // raised at once, as an unguarded `zval_ptr_dtor` would.
    if let Some(object) = ctx.with(|context| {
        (context.context.kind() != IS_UNDEF)
            .then(|| mem::replace(&mut context.context, Owned::null()))
    }) {
        release(object);
    }
    // Upstream only drops the references (Z_TRY_DELREF), leaking a callable or
    // return value the coroutine held last, such as `go([new Foo, 'bar'])`'s array.
    release(ctx.with(|context| mem::take(&mut context.callable)));
    // SAFETY: a live context's callable, whose object and closure `open` held.
    unsafe { swr_zend_coroutine_fcc_discard(fcc(ctx.call())) };
    release(ctx.with(|context| mem::take(&mut context.return_value)));
    fiber_try_destroy(ctx, origin(ctx));
    // A context-owned __destruct() may have rearmed the time limit.
    if ctx.time_limited()
        && let Some(co) = ctx.co()
        && let Some(timer) = swap_time_limit(ctx, host::cid(co), None)
    {
        swoole_rs_timer::del_id(timer);
    }
    swr_zend_coroutine_finish(ctx.running(), origin(ctx).switched_out());
    if let Some(join) = ctx.replace_join(None) {
        join.closed(ctx);
    }
    // Tasks a destructor deferred; with the join, all the context holds that
    // needs a drop.
    drop(ctx.defers(Option::take));
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
    main().with(|context| *context = Context::empty());
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
    ctx.with(|context| context.mock = Some(Mock(fiber)));
    zend_observer_fiber_init_notify(fiber);
}

/// A mock fiber's function (upstream's `INVALID_PTR`): nothing switches to one.
extern "C" fn never_switched(_: *mut swoole_rs_zend::sys::zend_fiber_transfer) {
    swoole_rs_corebase::fatal("a coroutine's mock fiber was switched to");
}

fn fiber_try_destroy(ctx: Ctx, origin: Ctx) {
    let Some(mock) = ctx.with(|context| context.mock.take()) else {
        return;
    };
    let fiber = mock.0;
    fiber.set_status(zend_fiber_status_ZEND_FIBER_STATUS_DEAD);
    fiber_switch_try_notify(ctx, origin);
    ctx.set_fiber(None);
    zend_observer_fiber_destroy_notify(fiber);
    free_mock(mock);
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
        // Built at compile time and never dropped: releasing its zvals is Zend's,
        // which unit tests do not load.
        let mut context = mem::ManuallyDrop::new(const { Context::empty() });
        let ctx = &raw mut *context;
        let handle = Ctx(NonNull::new(ctx).unwrap());
        assert_eq!(handle.vm().as_ptr(), unsafe { &raw mut (*ctx).vm });
        assert_eq!(handle.call(), unsafe { &raw mut (*ctx).call });
        assert_eq!(handle.return_value(), unsafe {
            (&raw mut (*ctx).return_value).cast::<zval>()
        });
        let callable = unsafe { &raw mut (*ctx).call };
        assert_eq!(fcc(callable), unsafe { &raw mut (*callable).fcc });
    }

    #[test]
    fn a_new_context_has_no_pending_state() {
        let context = mem::ManuallyDrop::new(const { Context::empty() });
        assert!(context.defer.is_none());
        assert!(context.join.is_none() && !context.time_limited);
        assert_eq!(context.context.kind(), IS_UNDEF);
        assert_eq!(context.callable.kind(), IS_UNDEF);
        assert_eq!(context.return_value.kind(), IS_UNDEF);
        assert!(!context.vm.in_silence && context.vm.output_ptr.is_null());
    }
}
