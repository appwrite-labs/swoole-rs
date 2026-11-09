//! `Swoole\Coroutine`'s static methods, `go()` and `defer()`. Handlers that
//! switch coroutines hold nothing that needs a drop at the switch: a bailout
//! in another coroutine may longjmp across them on the main stack.
use crate::{
    classes,
    context::{self, Context, INVALID},
    host::{
        self, SW_ERROR_CO_CANCELED, SW_ERROR_CO_NOT_EXISTS, SW_ERROR_INVALID_PARAMS,
        SW_ERROR_WRONG_OPERATION,
    },
    runtime,
};
use std::{
    cell::{Cell, RefCell},
    ffi::{c_long, c_void},
    mem, ptr,
    rc::Rc,
};
use swoole_rs_zend::{
    Call, Callable, E_ERROR, E_WARNING, add_next_index_long,
    bailout::{self, Bailout},
    each_value, init_object, set_object,
    sys::{
        object_properties_init, swr_zend_array_init, swr_zend_coroutine_called_by_destructor,
        swr_zend_coroutine_locate, swr_zend_coroutine_print_trace, swr_zend_coroutine_redirect,
        swr_zend_coroutine_set_frame, swr_zend_coroutine_stack_usage, swr_zend_object_addref,
        swr_zval_get_long, zend_call_method, zend_class_entry, zend_exception_set_previous,
        zend_fcall_info, zend_fcall_info_cache, zend_fetch_debug_backtrace, zend_object,
        zend_objects_new, zend_throw_exception_object, zval, zval_ptr_dtor,
    },
};

/// `DEBUG_BACKTRACE_PROVIDE_OBJECT`.
const PROVIDE_OBJECT: i64 = 1;

swoole_rs_zend::handlers! {
    zif_swoole_coroutine_create => create;
    zif_swoole_coroutine_defer => defer;
    zif_swoole_test_kernel_coroutine => kernel_coroutine;
    zim_swoole_coroutine_exists => exists;
    zim_swoole_coroutine_yield => yield_;
    zim_swoole_coroutine_resume => resume;
    zim_swoole_coroutine_join => join;
    zim_swoole_coroutine_cancel => cancel;
    zim_swoole_coroutine_setTimeLimit => set_time_limit;
    zim_swoole_coroutine_isCanceled => is_canceled;
    zim_swoole_coroutine_stats => stats;
    zim_swoole_coroutine_getCid => get_cid;
    zim_swoole_coroutine_getPcid => get_pcid;
    zim_swoole_coroutine_getContext => get_context;
    zim_swoole_coroutine_getBackTrace => get_back_trace;
    zim_swoole_coroutine_printBackTrace => print_back_trace;
    zim_swoole_coroutine_getElapsed => get_elapsed;
    zim_swoole_coroutine_getStackUsage => get_stack_usage;
    zim_swoole_coroutine_list => list;
    zim_swoole_coroutine_getExecuteTime => get_execute_time;
    zim_swoole_coroutine_enableScheduler => enable_scheduler;
    zim_swoole_coroutine_disableScheduler => disable_scheduler;
}

/// `go()`/`Co::create()`: `(callable $func, mixed ...$params)`.
fn create(call: &Call) {
    // SAFETY: all-zero is an empty call info, filled by the parser.
    let mut fci: zend_fcall_info = unsafe { mem::zeroed() };
    // SAFETY: as above.
    let mut fcc: zend_fcall_info_cache = unsafe { mem::zeroed() };
    let parsed = call.arity(1, u32::MAX).and_then(|()| {
        // SAFETY: argument 1 is passed; the outputs are writable locals.
        unsafe { bailout::parse_func(call.zval(1), 1, &raw mut fci, &raw mut fcc) }
    });
    if parsed.is_err() {
        call.set_bool(false);
        return;
    }
    let argc = call.num_args() - 1;
    let argv = if argc > 0 {
        call.zval(2)
    } else {
        ptr::null_mut()
    };
    // SAFETY: called from a handler.
    if host::flag(host::SHUTDOWN_FUNCTIONS) && unsafe { swr_zend_coroutine_called_by_destructor() }
    {
        host::fatal_error(
            E_ERROR,
            c"can not use coroutine in __destruct after php_request_shutdown",
        );
        call.set_bool(false);
        return;
    }
    match runtime::create(&raw mut fcc, argc, argv, &raw mut fci.function_name) {
        Ok(cid) if cid > 0 => call.set_long(cid),
        Ok(_) => call.set_bool(false),
        Err(Bailout) => runtime::raise(),
    }
}

/// `defer()`/`Co::defer()`: `(callable $callback)`.
fn defer(call: &Call) {
    let Ok(mut callable) = call.arity(1, 1).and_then(|()| Callable::parse(call, 1)) else {
        call.set_bool(false);
        return;
    };
    host::current_safe();
    callable.keep(None);
    context::defer(callable.into_raw());
}

/// `swoole_test_kernel_coroutine(int $count = 100, float $sleep = 1)`.
fn kernel_coroutine(call: &Call) {
    if !runtime::activated() {
        call.set_bool(false);
        return;
    }
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, 100)?, call.double(2, 1.0)?)));
    let Ok((count, sleep)) = parsed else {
        call.set_bool(false);
        return;
    };
    host::kernel_coroutine(count, sleep);
}

/// `Co::exists(int $cid)`.
fn exists(call: &Call) {
    let Ok(cid) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    call.set_bool(!host::by_cid(cid).is_null());
}

per_thread! {
    /// The ids of the coroutines suspended by `Co::yield()`, sorted
    /// (`user_yield_coros`). Ids rise, so a yield usually appends.
    static YIELDED: RefCell<Vec<c_long>> = RefCell::new(Vec::new());
}

/// Adds `cid` to [`YIELDED`] or takes it out; whether it was there.
fn set_yielded(cid: c_long, yielded: bool) -> bool {
    YIELDED.with(|ids| {
        let mut ids = ids.borrow_mut();
        match (ids.binary_search(&cid), yielded) {
            (Ok(at), false) => {
                ids.remove(at);
                true
            }
            (Err(at), true) => {
                ids.insert(at, cid);
                false
            }
            (found, _) => found.is_ok(),
        }
    })
}

/// `Co::yield()`/`Co::suspend()`.
fn yield_(call: &Call) {
    let co = host::current_safe();
    set_yielded(host::cid(co), true);
    // SAFETY: the running coroutine; this frame owns nothing that needs a drop.
    unsafe { host::yield_cancelable(co, cancel_yield, ptr::null_mut()) };
    if host::is_canceled(co) {
        host::set_last_error(SW_ERROR_CO_CANCELED);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

unsafe extern "C-unwind" fn cancel_yield(co: *mut c_void, _: *mut c_void) -> bool {
    set_yielded(host::cid(co), false);
    host::resume(co);
    true
}

/// `Co::resume(int $cid)`.
fn resume(call: &Call) {
    let Ok(cid) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let co = host::by_cid(cid);
    if co.is_null() || !set_yielded(cid, false) {
        host::fatal_error(
            E_WARNING,
            c"can not resume the coroutine which is in IO operation or non-existent",
        );
        call.set_bool(false);
        return;
    }
    host::resume(co);
    call.set_bool(true);
}

/// A `Co::join()` in progress: the contexts it still waits for.
pub(crate) struct Join {
    pending: Vec<*mut Context>,
    co: *mut c_void,
    canceled: Rc<Cell<bool>>,
}

impl Join {
    /// A joined coroutine closed: once none are left, resume the joiner from
    /// the event loop (unless it gave up first).
    pub(crate) fn closed(join: *mut Self, ctx: *mut Context) {
        // SAFETY: a join stays live while it is set on a context.
        let this = unsafe { &mut *join };
        this.pending.retain(|&pending| pending != ctx);
        if !this.pending.is_empty() {
            return;
        }
        let resume = Box::new((this.co, Rc::clone(&this.canceled)));
        host::defer(join_resume, Box::into_raw(resume).cast());
    }

    /// Stop waiting for the contexts registered so far.
    fn forget(&self) {
        for &ctx in &self.pending {
            // SAFETY: a context still waited for is live.
            unsafe { context::at(ctx) }.join = ptr::null_mut();
        }
    }
}

unsafe extern "C-unwind" fn join_resume(data: *mut c_void) {
    // SAFETY: the box `Join::closed` passed.
    let resume = unsafe { Box::from_raw(data.cast::<(*mut c_void, Rc<Cell<bool>>)>()) };
    let (co, canceled) = (resume.0, resume.1.get());
    drop(resume);
    if !canceled {
        host::resume(co);
    }
}

/// `Co::join(array $cid_array, float $timeout = -1)`.
fn join(call: &Call) {
    let co = host::current_safe();
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.array(1)?, call.double(2, -1.0)?)));
    let Ok((cids, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    // SAFETY: an array argument.
    let table = unsafe { swoole_rs_zend::value::table(cids) };
    // SAFETY: as above.
    if unsafe { swoole_rs_zend::value::count(cids) } == 0 {
        host::set_last_error(SW_ERROR_INVALID_PARAMS);
        call.set_bool(false);
        return;
    }
    let mut join = Join {
        pending: Vec::new(),
        co,
        canceled: Rc::new(Cell::new(false)),
    };
    let own = host::cid(co);
    let mut failed = false;
    let mut each = |value: *mut zval| {
        // SAFETY: a live element.
        let cid = unsafe { swr_zval_get_long(value) };
        if cid == own {
            host::error_ex(E_WARNING, SW_ERROR_WRONG_OPERATION, c"can not join self");
            failed = true;
            return false;
        }
        let ctx = context::by_cid(cid);
        if ctx.is_null() {
            return true;
        }
        // SAFETY: a live context.
        let this = unsafe { context::at(ctx) };
        if !this.join.is_null() {
            host::set_last_error(SW_ERROR_WRONG_OPERATION);
            failed = true;
            return false;
        }
        this.join = &raw mut join;
        join.pending.push(ctx);
        true
    };
    // SAFETY: the array is live and the closure does not modify it.
    unsafe { each_value(table, &mut each) };
    if failed || join.pending.is_empty() {
        // Unregister the contexts this call registered.
        join.forget();
        if !failed {
            host::set_last_error(SW_ERROR_INVALID_PARAMS);
        }
        call.set_bool(false);
        return;
    }
    // This frame lives on the running coroutine's stack, which no bailout
    // unwinds while it waits.
    if !host::yield_ex(co, timeout) {
        join.forget();
        join.canceled.set(true);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `php_swoole_coroutine_throw_exception`: cancel `cid`, throwing `ce` in it
/// when `throw`.
pub(crate) fn throw_exception(cid: c_long, throw: bool, ce: *mut zend_class_entry) -> bool {
    if !throw {
        let co = host::by_cid(cid);
        if co.is_null() {
            host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
            return false;
        }
        return host::cancel(co);
    }
    let task = context::by_cid(cid);
    // SAFETY: null or a live context.
    let co = unsafe { task.as_ref() }.map_or(ptr::null_mut(), |task| task.co);
    if co.is_null() {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        return false;
    }
    let is_current = co == host::current();
    let (pending, class) = {
        // SAFETY: a live context.
        let vm = &unsafe { context::at(task) }.vm;
        (vm.exception, vm.exception_class)
    };
    if !is_current && !pending.is_null() && class == ce {
        host::set_last_error(SW_ERROR_CO_CANCELED);
        return false;
    }
    // SAFETY: a registered exception class.
    let exception = unsafe { zend_objects_new(ce) };
    // SAFETY: a new object of `ce`.
    unsafe { object_properties_init(exception, ce) };
    if is_current {
        // The live Co::cancel frame is internal: Zend redirects the user
        // caller when it returns.
        // SAFETY: a new exception, in the running frame.
        unsafe { swr_zend_coroutine_locate(exception) };
        // SAFETY: all-zero is IS_UNDEF, then set to the exception.
        let mut thrown: zval = unsafe { mem::zeroed() };
        // SAFETY: the zval takes over the new object's reference.
        unsafe { set_object(&raw mut thrown, exception) };
        // SAFETY: Zend takes the exception.
        unsafe { zend_throw_exception_object(&raw mut thrown) };
        return true;
    }
    {
        // SAFETY: a suspended coroutine's context.
        let vm = &mut unsafe { context::at(task) }.vm;
        vm.exception_class = ce;
        vm.exception = exception;
    }
    if !pending.is_null() {
        // SAFETY: two live exceptions.
        unsafe { zend_exception_set_previous(exception, pending) };
    }
    // SAFETY: as above.
    let frame = unsafe { context::at(task) }.vm.execute_data;
    // SAFETY: the suspended coroutine's frame, restored below.
    let backup = unsafe { swr_zend_coroutine_set_frame(frame) };
    // SAFETY: the exception, located in that frame.
    unsafe { swr_zend_coroutine_locate(exception) };
    // SAFETY: restores the running frame.
    unsafe { swr_zend_coroutine_set_frame(backup) };
    if pending.is_null() {
        // SAFETY: the suspended coroutine's VM state.
        unsafe { swr_zend_coroutine_redirect(context::vm(task)) };
    }
    // Even an irrevocable wait ends: the exception terminates the coroutine.
    host::cancel(co);
    true
}

/// `Co::cancel(int $cid, bool $throw_exception = false)`.
fn cancel(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(2, false)?)));
    let Ok((cid, throw)) = parsed else {
        call.set_bool(false);
        return;
    };
    call.set_bool(throw_exception(cid, throw, classes::canceled()));
}

/// `Co::setTimeLimit(float $timeout)`.
fn set_time_limit(call: &Call) {
    let Ok(timeout) = call.arity(1, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return;
    };
    let co = host::current_safe();
    if timeout <= 0.0 || co.is_null() {
        call.set_bool(false);
        return;
    }
    let cid = host::cid(co);
    let ctx = context::current_task();
    // Coroutine ids are positive.
    let data = ptr::without_provenance_mut(usize::try_from(cid).unwrap_or_default());
    // SAFETY: time_limit takes the id as its data.
    let timer = unsafe { host::timer_add(timeout, data, time_limit) };
    if timer.is_null() {
        call.set_bool(false);
        return;
    }
    // SAFETY: the running coroutine's context.
    let old = mem::replace(&mut unsafe { context::at(ctx) }.timer_limit, timer);
    if !old.is_null() {
        // SAFETY: the coroutine's armed timer.
        unsafe { host::timer_del(old) };
    }
    call.set_bool(true);
}

unsafe extern "C" fn time_limit(cid: *mut c_void) {
    let cid = c_long::try_from(cid.addr()).unwrap_or_default();
    let ctx = context::by_cid(cid);
    if !ctx.is_null() {
        // SAFETY: a live context; its timer has fired.
        unsafe { context::at(ctx) }.timer_limit = ptr::null_mut();
        throw_exception(cid, true, classes::timeout());
    }
}

/// `Co::isCanceled()`.
fn is_canceled(call: &Call) {
    let co = host::current_safe();
    call.set_bool(host::is_canceled(co));
}

/// `Co::stats()`.
fn stats(call: &Call) {
    let mut stats = host::Stats::default();
    host::stats(&mut stats);
    call.set_array();
    let values: [(&std::ffi::CStr, i64); 9] = [
        (c"event_num", stats.event_num),
        (c"signal_listener_num", stats.signal_listener_num),
        (c"aio_task_num", stats.aio_task_num),
        (c"aio_worker_num", stats.aio_worker_num),
        (c"aio_queue_size", stats.aio_queue_size),
        (c"c_stack_size", stats.c_stack_size),
        (c"coroutine_num", stats.coroutine_num),
        (c"coroutine_peak_num", stats.coroutine_peak_num),
        (c"coroutine_last_cid", stats.coroutine_last_cid),
    ];
    for (key, value) in values {
        call.add_assoc_long(key, value);
    }
    if stats.iouring != 0 {
        call.add_assoc_long(c"iouring_task_num", stats.iouring_task_num);
        call.add_assoc_long(c"iouring_sq_usage_percent", stats.iouring_sq_usage_percent);
        call.add_assoc_long(c"iouring_waiting_task_num", stats.iouring_waiting_task_num);
    }
}

/// `Co::getCid()`/`Co::getuid()`.
fn get_cid(call: &Call) {
    call.set_long(runtime::get_cid());
}

/// The optional `int $cid = 0` most methods take.
fn optional_cid(call: &Call) -> Option<c_long> {
    call.arity(0, 1).and_then(|()| call.long(1, 0)).ok()
}

/// `Co::getPcid(int $cid = 0)`.
fn get_pcid(call: &Call) {
    let Some(cid) = optional_cid(call) else {
        call.set_bool(false);
        return;
    };
    let ctx = if cid == 0 {
        context::current()
    } else {
        context::by_cid(cid)
    };
    // SAFETY: null or a live context.
    let pcid = unsafe { ctx.as_ref() }.map_or(0, |ctx| ctx.pcid);
    if pcid == 0 {
        call.set_bool(false);
    } else {
        call.set_long(pcid);
    }
}

/// `Co::getContext(int $cid = 0)`.
fn get_context(call: &Call) {
    let Some(cid) = optional_cid(call) else {
        call.set_bool(false);
        return;
    };
    let ctx = if cid == 0 {
        context::current_task()
    } else {
        host::task_by_cid(cid).cast::<Context>()
    };
    if ctx.is_null() {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        call.set_null();
        return;
    }
    // SAFETY: a live context.
    let object = unsafe { context::at(ctx) }.context;
    if object.addr() == INVALID {
        // A context destroyed with its coroutine (swoole-src#2991).
        host::fatal_error(E_WARNING, c"The context of coroutine has been destroyed");
        call.set_null();
        return;
    }
    let object = if object.is_null() {
        // SAFETY: the return slot, with Swoole's registered context class.
        unsafe { init_object(call.return_value(), classes::context_class()) };
        // SAFETY: the new object.
        let object = unsafe { swoole_rs_zend::value::obj_of(call.return_value()) };
        // SAFETY: a live context takes the object's first reference.
        unsafe { context::at(ctx) }.context = object;
        object
    } else {
        object
    };
    // SAFETY: a live object; the return value takes the new reference.
    unsafe { swr_zend_object_addref(object) };
    // SAFETY: as above.
    unsafe { set_object(call.return_value(), object) };
}

/// `Co::getElapsed(int $cid = 0)`.
fn get_elapsed(call: &Call) {
    let Some(cid) = optional_cid(call) else {
        call.set_bool(false);
        return;
    };
    let elapsed = if runtime::activated() {
        host::elapsed(cid)
    } else {
        -1
    };
    call.set_long(elapsed);
}

/// `Co::getStackUsage(int $cid = 0)`.
fn get_stack_usage(call: &Call) {
    let Some(cid) = optional_cid(call) else {
        call.set_bool(false);
        return;
    };
    let current = runtime::get_cid();
    let cid = if cid == 0 { current } else { cid };
    let ctx = context::by_cid(cid);
    if ctx.is_null() {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        call.set_bool(false);
        return;
    }
    // SAFETY: a live context.
    let stack = unsafe { context::at(ctx) }.vm.vm_stack;
    // SAFETY: a VM stack chain (or the running one).
    let usage = unsafe { swr_zend_coroutine_stack_usage(stack, cid == current) };
    call.set_long(i64::try_from(usage).unwrap_or(i64::MAX));
}

/// The trace options `getBackTrace` and `printBackTrace` take.
fn trace_args(call: &Call, options: i64) -> Option<(c_long, i64, i64)> {
    call.arity(0, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, options)?, call.long(3, 0)?)))
        .ok()
}

/// `Co::getBackTrace(int $cid = 0, int $options = DEBUG_BACKTRACE_PROVIDE_OBJECT, int $limit = 0)`.
fn get_back_trace(call: &Call) {
    let Some((cid, options, limit)) = trace_args(call, PROVIDE_OBJECT) else {
        call.set_bool(false);
        return;
    };
    let (options, limit) = (narrow(options), narrow(limit));
    if cid == 0 || cid == runtime::get_cid() {
        // SAFETY: the return slot.
        unsafe { zend_fetch_debug_backtrace(call.return_value(), 0, options, limit) };
        return;
    }
    let ctx = context::by_cid(cid);
    if ctx.is_null() {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        call.set_bool(false);
        return;
    }
    // SAFETY: a live context.
    let frame = unsafe { context::at(ctx) }.vm.execute_data;
    // SAFETY: the coroutine's frame, restored below.
    let backup = unsafe { swr_zend_coroutine_set_frame(frame) };
    // SAFETY: the return slot.
    unsafe { zend_fetch_debug_backtrace(call.return_value(), 0, options, limit) };
    // SAFETY: restores the running frame.
    unsafe { swr_zend_coroutine_set_frame(backup) };
}

/// `(int) value`, as C's conversion to the `int` Zend takes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream passes zend_long to int"
)]
fn narrow(value: i64) -> i32 {
    value as i32
}

/// `sw_php_print_backtrace`.
fn print_backtrace(
    cid: c_long,
    options: i64,
    limit: i64,
    return_value: *mut zval,
) -> Result<(), Bailout> {
    let (options, limit) = (narrow(options), narrow(limit));
    if cid == 0 || cid == runtime::get_cid() {
        // SAFETY: output handlers run inside the shim's zend_try.
        return trapped(unsafe { swr_zend_coroutine_print_trace(1, options, limit) });
    }
    let ctx = context::by_cid(cid);
    if ctx.is_null() {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        if !return_value.is_null() {
            // SAFETY: the caller's return slot.
            let slot = unsafe { &mut *return_value };
            slot.u1.type_info = swoole_rs_zend::sys::IS_FALSE;
        }
        return Ok(());
    }
    // SAFETY: a live context.
    let frame = unsafe { context::at(ctx) }.vm.execute_data;
    // SAFETY: the coroutine's frame, restored below unless output bailed.
    let backup = unsafe { swr_zend_coroutine_set_frame(frame) };
    // SAFETY: as above.
    trapped(unsafe { swr_zend_coroutine_print_trace(0, options, limit) })?;
    // SAFETY: restores the running frame.
    unsafe { swr_zend_coroutine_set_frame(backup) };
    Ok(())
}

/// A shim's trapped-bailout flag.
fn trapped(bailed: std::ffi::c_int) -> Result<(), Bailout> {
    if bailed == 0 { Ok(()) } else { Err(Bailout) }
}

/// `sw_php_print_backtrace` for C++: 1 if output bailed out.
/// # Safety
/// `return_value` is null or a writable zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcoroutines_print_backtrace(
    cid: c_long,
    options: c_long,
    limit: c_long,
    return_value: *mut zval,
) -> u8 {
    u8::from(print_backtrace(cid, options, limit, return_value).is_err())
}

/// `Co::printBackTrace(int $cid = 0, int $options = 0, int $limit = 0)`.
fn print_back_trace(call: &Call) {
    let Some((cid, options, limit)) = trace_args(call, 0) else {
        call.set_bool(false);
        return;
    };
    if print_backtrace(cid, options, limit, call.return_value()).is_err() {
        runtime::raise();
    }
}

/// `Co::list()`/`Co::listCoroutines()`: a `Coroutine\Iterator` of the ids.
fn list(call: &Call) {
    let mut cids: Vec<c_long> = vec![0; host::count()];
    let count = host::list(&mut cids);
    // SAFETY: all-zero is IS_UNDEF, then an array.
    let mut ids: zval = unsafe { mem::zeroed() };
    // SAFETY: a writable zval.
    unsafe { swr_zend_array_init(&raw mut ids) };
    for &cid in cids.iter().take(count) {
        // SAFETY: an array.
        unsafe { add_next_index_long(&raw mut ids, cid) };
    }
    drop(cids);
    let iterator = classes::iterator();
    // SAFETY: the return slot, with Swoole's registered iterator class.
    unsafe { init_object(call.return_value(), iterator) };
    // SAFETY: the new object.
    let object: *mut zend_object = unsafe { swoole_rs_zend::value::obj_of(call.return_value()) };
    let constructor = iterator
        .wrapping_byte_add(mem::offset_of!(zend_class_entry, constructor))
        .cast();
    // SAFETY: ArrayIterator's internal constructor, with one array argument.
    unsafe {
        zend_call_method(
            object,
            iterator,
            constructor,
            c"__construct".as_ptr(),
            11,
            ptr::null_mut(),
            1,
            &raw mut ids,
            ptr::null_mut(),
        );
    }
    // SAFETY: the array, which the iterator copied.
    unsafe { zval_ptr_dtor(&raw mut ids) };
}

/// `Co::getExecuteTime()` (`SW_CORO_TIME` builds).
fn get_execute_time(call: &Call) {
    let time = if runtime::activated() {
        host::execute_time(0)
    } else {
        -1
    };
    call.set_long(time);
}

/// `Co::enableScheduler()`.
fn enable_scheduler(call: &Call) {
    call.set_bool(set_scheduler(true));
}

/// `Co::disableScheduler()`.
fn disable_scheduler(call: &Call) {
    call.set_bool(set_scheduler(false));
}

fn set_scheduler(enable: bool) -> bool {
    let ctx = context::current_task();
    if ctx.is_null() {
        return false;
    }
    // SAFETY: the running coroutine's context.
    let this = unsafe { context::at(ctx) };
    if this.enable_scheduler == enable {
        return false;
    }
    this.enable_scheduler = enable;
    true
}
