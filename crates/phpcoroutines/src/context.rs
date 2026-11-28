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
use crate::{classes::ExceptionClass, host, methods::Join, runtime};
use std::{
    cell::{Cell, UnsafeCell},
    ffi::{c_int, c_long, c_void},
    mem,
    ptr::{self, NonNull},
    rc::Rc,
};
use swoole_rs_corebase::CellExt;
use swoole_rs_corebase::coroutine::{CoroutineTask, State};
use swoole_rs_corebase::timer::Held;
use swoole_rs_zend::sys::{
    IS_NULL, IS_STRING, IS_UNDEF, swr_zend_callable, swr_zend_coroutine_set_frame,
    swr_zend_coroutine_vm, zend_class_entry, zend_execute_data, zend_fcall_info,
    zend_fcall_info_cache, zend_fiber_context, zend_fiber_status,
    zend_fiber_status_ZEND_FIBER_STATUS_DEAD, zend_fiber_status_ZEND_FIBER_STATUS_INIT,
    zend_fiber_status_ZEND_FIBER_STATUS_RUNNING, zend_fiber_status_ZEND_FIBER_STATUS_SUSPENDED,
    zend_object, zend_op, zval,
};
use swoole_rs_zend::{Callable, Emalloc, Owned, Value, bailout};

unsafe extern "C" {
    /// Persist the resolved callable's object and closure using `GC_ADDREF` only.
    /// Private to open: its freshly initialized call record holds the resolved
    /// cache lent by Args; no PHP code runs while that record is borrowed.
    safe fn swr_zend_callable_hold(callable: &mut swr_zend_callable);
    /// Drop only shared, non-root object/closure references, without invoking
    /// PHP. Zero means nothing was released and the slow path must trap it.
    safe fn swr_zend_callable_release_quiet(callable: &mut swr_zend_callable) -> c_int;
    /// Release the closing coroutine's held object and closure, with no owned
    /// arguments, inside the existing callable destructor trap. No Rust borrow
    /// of the context spans its PHP callbacks.
    #[link_name = "swr_zend_try_callable_release"]
    safe fn release_cache(callable: ReleasingCall) -> c_int;
    /// Reset the running coroutine's output handlers inside the native trap;
    /// no trap is needed when the executor has no output handlers.
    safe fn swr_zend_coroutine_output_reset() -> c_int;
    /// `init_main_context`'s VM half: clears `EG(opline_before_exception)`,
    /// moves this PHP thread's executor, output and serializer state into `vm`
    /// (`save_context`) and returns `EG(main_fiber_context)`, which lives for
    /// the request. It follows no pointer `vm` holds and runs no PHP code.
    safe fn swr_zend_coroutine_main(vm: &mut swr_zend_coroutine_vm) -> Option<Fiber>;
    /// Save the current executor, output and serializer globals into `from`.
    safe fn swr_zend_coroutine_save(from: Running);
    /// Restore the globals of the suspended coroutine.
    safe fn swr_zend_coroutine_restore(to: Suspended);
    /// `save_context(from)`, then a fresh VM stack for the new coroutine,
    /// bounded by its C stack (`stack`, `size`, only compared with the stack
    /// pointer; null: none), saved into `vm` too, which it initializes.
    safe fn swr_zend_coroutine_start(
        from: Running,
        vm: &mut swr_zend_coroutine_vm,
        stack: *mut c_void,
        size: usize,
        fiber: Option<Fiber>,
    );
    /// Install the running coroutine's fiber state after its entry notification.
    safe fn swr_zend_coroutine_restore_fiber(vm: Running);
    /// `destroy_context`'s end: frees the finishing context's VM stack, which
    /// its saved state stops naming, then restores `origin`'s.
    safe fn swr_zend_coroutine_finish(finished: Running, origin: Suspended) -> c_int;
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
    /// A new object of `class`, with no constructor
    /// (nor `create_object`) run: Zend's allocator never returns null.
    safe fn zend_objects_new(class: ExceptionClass) -> Exception;
    /// Populate a fresh object of this class with its declared property defaults.
    safe fn object_properties_init(object: Lent<'_>, class: ExceptionClass);
    /// Set the fresh internal exception's declared, untyped property before exposure to PHP.
    safe fn zend_update_property(
        scope: ExceptionClass,
        object: Lent<'_>,
        name: &[u8; 6],
        len: usize,
        value: &Value,
    );
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
}

/// A live fiber context: the main context's `EG(main_fiber_context)`, or a
/// coroutine's mock one from [`fiber_init`] until [`fiber_try_destroy`] frees
/// it. Only those make one.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fiber(NonNull<zend_fiber_context>);

/// A coroutine's mock fiber, which [`fiber_init`] allocates and its context
/// holds until [`fiber_try_destroy`] frees it: never copied, so freed once.
pub(crate) struct Mock(Box<UnsafeCell<zend_fiber_context>, Emalloc>);

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
    pub(crate) fn new(class: ExceptionClass, reason: Option<&Value>) -> Self {
        let exception = zend_objects_new(class);
        object_properties_init(exception.lend(), class);
        if let Some(reason) = reason {
            let name = b"reason";
            zend_update_property(class, exception.lend(), name, name.len(), reason);
        }
        exception
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

/// `PHPCoroutine::Args`, built by `create` for the new coroutine's entry, which
/// copies them in before its first switch, while `create`'s frame still lends
/// the arguments and callable they point to.
#[derive(Clone, Copy)]
pub(crate) struct Args {
    /// The resolved callable.
    pub(crate) fcc: zend_fcall_info_cache,
    /// `op_array.opcodes` of a user function; none for an internal one.
    pub(crate) opcodes: Option<*mut zend_op>,
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
    /// The first instruction of `call`'s function, a user function's.
    opcodes: Option<*mut zend_op>,
    /// The callable zval's reference (upstream's `Z_TRY_ADDREF` of it).
    callable: Owned,
    return_value: Owned,
    /// The coroutine, which outlives its context; none for the main context.
    co: Option<host::Co>,
    fiber: Option<Fiber>,
    /// The coroutine's own fiber, which `fiber` names while it lives.
    mock: Option<Mock>,
    fiber_init_notified: bool,
    callback_running: bool,
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
            opcodes: None,
            callable: Owned::empty(),
            return_value: Owned::empty(),
            co: None,
            fiber: None,
            mock: None,
            fiber_init_notified: false,
            callback_running: false,
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
#[derive(Clone, Copy)]
pub(crate) struct Call(NonNull<swr_zend_callable>);

/// The closing coroutine's one held cache, with `param_count` cleared because
/// its arguments were borrowed. Only close makes this token, after the quiet
/// release declined; the native trap consumes it without freeing its record.
#[repr(transparent)]
struct ReleasingCall(NonNull<swr_zend_callable>);

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
    callback_running, set_callback_running: bool;
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
        // SAFETY: the handle keeps track of a live PHP Context containing the saved
        // Zend VM state. Only this call accesses it until `f` returns.
        f(unsafe { &mut *self.ptr() })
    }

    /// The VM state, borrowed for `f`, as [`Ctx::with`].
    fn with_vm<R>(self, f: impl FnOnce(&mut swr_zend_coroutine_vm) -> R) -> R {
        self.with(|ctx| f(&mut ctx.vm))
    }

    /// Run `f` with that frame as Zend's current one, as its backtraces and
    /// exception locations read it, then restore the running frame; `f` just
    /// runs for the running context, whose saved frame is stale. `f` runs no PHP
    /// code: it neither switches coroutines nor bails out. Output, whose
    /// handlers may do both, belongs after it.
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

/// Replace or clear the time limit for coroutine `cid`, returning its previous timer.
pub(crate) fn swap_time_limit(ctx: Ctx, cid: c_long, timer: Option<Held>) -> Option<Held> {
    let limited = ctx.time_limited();
    ctx.set_time_limited(timer.is_some());
    TIME_LIMITS.with(|limits| {
        limits.with_taken(|limits| {
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
    swr_zend_coroutine_save(ctx.running());
    swr_zend_coroutine_restore(origin.switched_out());
    count_switch(ctx);
}

/// `PHPCoroutine::on_resume`, which the scheduler calls on the running
/// context's stack before switching to `task`'s suspended coroutine.
pub(crate) extern "C" fn on_resume(task: CoroutineTask) {
    let ctx = Ctx::of_task(task);
    let current = current();
    fiber_switch_try_notify(current, ctx);
    swr_zend_coroutine_save(current.running());
    swr_zend_coroutine_restore(ctx.switched_out());
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
pub(crate) fn run(args: &Args) -> bool {
    // `close` drops everything the context holds that needs a drop, so the frame
    // only frees its bytes, as upstream's `efree` does, with no drop pass.
    let mut context = mem::ManuallyDrop::new(Context::empty());
    let Some(ctx) = open(args, &mut context) else {
        return false;
    };
    let caught = host::run(ctx);
    let closed = close(ctx);
    if closed.is_err() {
        // A destructor failed before normal close restored the origin. Leave
        // no executor pointer into this stack context when its body returns;
        // main relays the escape from the coroutine's native bailout hook.
        finish(ctx);
    }
    // The context goes with this frame.
    swoole_rs_corebase::coroutine::clear_current_task();
    caught || closed.is_err()
}

/// `create_context`, on the new coroutine, in `context`, live until [`run`]
/// returns; the creator's executor state, still the executor's, is saved.
fn open(args: &Args, context: &mut Context) -> Option<Ctx> {
    let co = host::current()?;
    let pcid = host::origin_cid(co);
    let this = Ctx(NonNull::from(context));
    this.set_co(Some(co));
    this.set_pcid(pcid);
    this.set_enable_scheduler(true);
    // SAFETY: on_yield and on_resume expect this PHP Context with its Zend VM
    // state. It lives until run clears the task.
    unsafe { swoole_rs_corebase::coroutine::set_current_task(this.0.cast()) };
    if host::flag(host::FIBER_MOCK) {
        fiber_init(this);
    }
    let (stack, size) = host::stack(co);
    // The creator switched to this coroutine without a swap callback: its
    // state is still the executor's, saved here.
    let creator = origin(this).running();
    let fiber = this.fiber();
    this.with_vm(|vm| swr_zend_coroutine_start(creator, vm, stack, size, fiber));
    record_last_msec(this);
    let retval = this.return_value();
    this.with(|context| {
        context.call.fcc = args.fcc;
        context.opcodes = args.opcodes;
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
    this.with(|context| swr_zend_callable_hold(&mut context.call));
    Some(this)
}

swoole_rs_zend::exports! {
/// The start hook and the fiber switch into the running coroutine's function;
/// its callable, to call (`zend::Function *`).
pub(crate) extern "C" fn enter(ctx: Ctx) -> Option<Call> {
    if let Err(error) = host::coro_hook(false, ctx) {
        error.defer();
        return None;
    }
    if ctx.fiber().is_some() {
        notify_entry(ctx);
    }
    ctx.set_callback_running(true);
    NonNull::new(ctx.call()).map(Call)
}

/// Only enter the defer loop when this coroutine registered a task.
pub(crate) extern "C" fn run_defers(ctx: Ctx) {
    ctx.set_callback_running(false);
    if ctx.with(|context| context.defer.is_some()) && !swoole_rs_zend::host::unwind_exit_pending() {
        host::run_defers(ctx);
    }
}
}

/// Show the entry function to fiber observers, or the caller's frame for an internal entry.
#[inline(never)]
fn notify_entry(ctx: Ctx) {
    let (handler, opcodes) =
        ctx.with(|context| (context.call.fcc.function_handler, context.opcodes));
    let mut frame: zend_execute_data = swoole_rs_zend::zeroed();
    let caller = origin(ctx);
    let running = if let Some(opcodes) = opcodes {
        frame.opline = opcodes;
        frame.func = handler;
        // SAFETY: the zend_execute_data and its coroutine function stay live
        // through the notification. unshow restores the previous frame.
        Shown(unsafe { swr_zend_coroutine_set_frame(&raw mut frame) })
    } else {
        swr_zend_coroutine_show(caller.switched_out())
    };
    fiber_switch_try_notify(caller, ctx);
    unshow(running);
    swr_zend_coroutine_restore_fiber(ctx.running());
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

/// Drop `value` and report a newly deferred destructor bailout to the owner.
fn release(value: Owned) -> Result<(), bailout::Bailout> {
    // Scalars and strings cannot run PHP during release.
    if value.kind() <= IS_STRING {
        return Ok(());
    }
    let pending = bailout::pending();
    drop(value);
    if pending {
        Ok(())
    } else {
        bailout::outcome(())
    }
}

fn stop_on_exit() {
    if swoole_rs_zend::host::unwind_exit_pending() {
        swoole_rs_corebase::coroutine::stop_resumes();
        swoole_rs_corebase::reactor::event_api::swoole_event_bail_out();
    }
}

fn finish(ctx: Ctx) {
    // Cleanup itself can raise an exit before the origin is restored.
    stop_on_exit();
    bailout::defer(bailout::trapped(swr_zend_coroutine_finish(
        ctx.running(),
        origin(ctx).switched_out(),
    )));
}

/// `destroy_context`, on the running, finishing coroutine, then
/// `PHPCoroutine::on_close`'s join hook. Native destructors contain their jumps;
/// a failed close returns to the owner, which restores the origin's executor.
fn close(ctx: Ctx) -> Result<(), bailout::Bailout> {
    // Hooks and destructors can yield to an already-ready sibling.
    stop_on_exit();
    host::coro_hook(true, ctx)?;
    // Output handlers run PHP. Trap their escape before returning through the
    // context owner, which must restore the origin before leaving this stack.
    bailout::trapped(swr_zend_coroutine_output_reset())?;
    // Its defer tasks were kept and are not called.
    if let Some(tasks) = ctx.defers(Option::take) {
        for task in *tasks {
            task.release()?;
        }
    }
    // Object destruction may yield and let the origin coroutine finish: the
    // origin is looked up again before each use.
    // Stop after the first new destructor bailout so later context cleanup
    // cannot run PHP before the owner restores the origin.
    if let Some(object) = ctx.with(|context| {
        (context.context.kind() != IS_UNDEF)
            .then(|| mem::replace(&mut context.context, Owned::null()))
    }) {
        release(object)?;
    }
    // Upstream only drops the references (Z_TRY_DELREF), leaking a callable or
    // return value the coroutine held last, such as `go([new Foo, 'bar'])`'s array.
    release(ctx.with(|context| mem::take(&mut context.callable)))?;
    let remaining = ctx.with(|context| {
        // These arguments were lent to the coroutine, not persisted by it.
        // Only the cache's object and closure references belong to close.
        context.call.fci.param_count = 0;
        (swr_zend_callable_release_quiet(&mut context.call) == 0)
            .then(|| ReleasingCall(NonNull::from(&mut context.call)))
    });
    if let Some(cache) = remaining {
        bailout::trapped(release_cache(cache))?;
    }
    release(ctx.with(|context| mem::take(&mut context.return_value)))?;
    fiber_try_destroy(ctx);
    // A context-owned __destruct() may have rearmed the time limit.
    if ctx.time_limited()
        && let Some(co) = ctx.co()
        && let Some(timer) = swap_time_limit(ctx, host::cid(co), None)
    {
        swoole_rs_corebase::timer::del_id(timer);
    }
    finish(ctx);
    if let Some(join) = ctx.replace_join(None) {
        join.closed(ctx);
    }
    // Tasks a destructor deferred; with the join, all the context holds that
    // needs a drop.
    drop(ctx.defers(Option::take));
    Ok(())
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
    let mock = Mock(Box::new_in(UnsafeCell::new(this), Emalloc::default()));
    // UnsafeCell permits observer writes through the Fiber while the box owns it.
    let fiber = Fiber(NonNull::from(mock.0.as_ref()).cast());
    ctx.set_fiber(Some(fiber));
    ctx.with(|context| context.mock = Some(mock));
    zend_observer_fiber_init_notify(fiber);
}

/// A mock fiber's function (upstream's `INVALID_PTR`): nothing switches to one.
extern "C" fn never_switched(_: *mut swoole_rs_zend::sys::zend_fiber_transfer) {
    swoole_rs_corebase::fatal("a coroutine's mock fiber was switched to");
}

fn fiber_try_destroy(ctx: Ctx) {
    let Some(mock) = ctx.with(|context| context.mock.take()) else {
        return;
    };
    let fiber = Fiber(NonNull::from(mock.0.as_ref()).cast());
    fiber.set_status(zend_fiber_status_ZEND_FIBER_STATUS_DEAD);
    fiber_switch_try_notify(ctx, origin(ctx));
    ctx.set_fiber(None);
    zend_observer_fiber_destroy_notify(fiber);
    drop(mock);
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
        let ctx = &mut *context;
        let handle = Ctx(NonNull::from_mut(ctx));
        assert_eq!(handle.vm().as_ptr(), &raw mut ctx.vm);
        assert_eq!(handle.call(), &raw mut ctx.call);
        assert_eq!(
            handle.return_value(),
            (&raw mut ctx.return_value).cast::<zval>()
        );
    }

    #[test]
    fn a_new_context_has_no_pending_state() {
        let context = mem::ManuallyDrop::new(const { Context::empty() });
        assert!(context.defer.is_none());
        assert!(context.join.is_none() && !context.time_limited);
        assert_eq!(context.context.kind(), IS_UNDEF);
        assert_eq!(context.callable.kind(), IS_UNDEF);
        assert_eq!(context.return_value.kind(), IS_UNDEF);
        assert!(context.vm.output_ptr.is_null());
    }
}
