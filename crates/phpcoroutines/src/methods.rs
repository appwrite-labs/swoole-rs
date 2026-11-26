//! `Swoole\Coroutine`'s static methods, `go()` and `defer()`. A failed
//! coroutine defers its bailout until the handler has completed Rust cleanup.
use crate::{
    classes,
    classes::ExceptionClass,
    context::{self, Ctx, Exception},
    host::{
        self, SW_ERROR_CO_CANCELED, SW_ERROR_CO_NOT_EXISTS, SW_ERROR_INVALID_PARAMS,
        SW_ERROR_WRONG_OPERATION,
    },
    leased, runtime, tracer,
};
use std::{cell::Cell, ffi::c_long, ptr, rc::Rc};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::CellExt;
use swoole_rs_corebase::timer::Deadline;
use swoole_rs_zend::{
    Call, Callable, E_ERROR, E_WARNING, OwnedArray, Php, Value, bailout::Bailout,
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
    let (mut fci, mut fcc) = (swoole_rs_zend::EMPTY_FCI, swoole_rs_zend::EMPTY_FCC);
    let Ok(fcc) = call
        .arity(1, u32::MAX)
        .and_then(|()| call.resolve(1, &mut fci, &mut fcc))
    else {
        call.set_bool(false);
        return;
    };
    let argc = call.num_args() - 1;
    let argv = if argc > 0 {
        call.zval(2)
    } else {
        ptr::null_mut()
    };
    if host::flag(host::SHUTDOWN_FUNCTIONS) && leased::called_by_destructor(call.php()) {
        host::fatal_error(
            E_ERROR,
            c"can not use coroutine in __destruct after php_request_shutdown",
        );
        call.set_bool(false);
        return;
    }
    match runtime::create(
        call.php(),
        Some(fcc),
        argc,
        argv,
        &raw mut fci.function_name,
    ) {
        Ok(cid) if cid > 0 => call.set_long(cid),
        Ok(_) => call.set_bool(false),
        Err(Bailout) => Bailout.defer(),
    }
}

/// `defer()`/`Co::defer()`: `(callable $callback)`.
fn defer(call: &Call) {
    let Ok(callable) = call.arity(1, 1).and_then(|()| Callable::parse(call, 1)) else {
        call.set_bool(false);
        return;
    };
    host::require_coroutine();
    context::defer(callable.keep(None));
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
    call.set_bool(host::by_cid(cid).is_some());
}

per_thread! {
    /// The ids of the coroutines suspended by `Co::yield()`, sorted
    /// (`user_yield_coros`). Ids rise, so a yield usually appends. Taken out
    /// and put back around a search and an insert or remove.
    static YIELDED: Cell<Vec<c_long>> = Cell::new(Vec::new());
}

/// Adds `cid` to [`YIELDED`] or takes it out; whether it was there.
fn set_yielded(cid: c_long, yielded: bool) -> bool {
    YIELDED.with(|ids| {
        ids.with_taken(|ids| match (ids.binary_search(&cid), yielded) {
            (Ok(at), false) => {
                ids.remove(at);
                true
            }
            (Err(at), true) => {
                ids.insert(at, cid);
                false
            }
            (found, _) => found.is_ok(),
        })
    })
}

/// `Co::yield()`/`Co::suspend()`.
fn yield_(call: &Call) {
    let Some(co) = host::require_coroutine() else {
        return;
    };
    set_yielded(host::cid(co), true);
    _ = swoole_rs_corebase::coroutine::yield_with_cancel(co.raw(), Deadline::never(), move || {
        cancel_yield(co)
    });
    if host::is_canceled(co) {
        host::set_last_error(SW_ERROR_CO_CANCELED);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn cancel_yield(co: host::Co) -> bool {
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
    let Some(co) = co.filter(|_| set_yielded(cid, false)) else {
        host::fatal_error(
            E_WARNING,
            c"can not resume the coroutine which is in IO operation or non-existent",
        );
        call.set_bool(false);
        return;
    };
    host::resume(co);
    call.set_bool(true);
}

/// A `Co::join()` in progress: the contexts it still waits for, each of which
/// holds it until it closes or the join gives up.
pub(crate) struct Join {
    /// Taken out and put back around list edits, which neither yield nor run
    /// PHP code.
    pending: Cell<Vec<Ctx>>,
    co: host::Co,
    canceled: Cell<bool>,
}

impl Join {
    /// A joined coroutine closed: once none are left, resume the joiner from
    /// the event loop (unless it gave up first).
    pub(crate) fn closed(self: Rc<Self>, ctx: Ctx) {
        let waiting = self.pending.with_taken(|pending| {
            pending.retain(|&pending| pending != ctx);
            !pending.is_empty()
        });
        if waiting {
            return;
        }
        let resume = JoinResume { join: self };
        if swoole_rs_corebase::reactor::event_api::swoole_event_defer_value(resume).is_err() {
            // Upstream dereferences `sw_reactor()` here unchecked.
            swoole_rs_corebase::fatal("no reactor to defer the resume to");
        }
    }

    /// Stop waiting for the contexts registered so far, which leave the list
    /// for good: none of them holds the join after.
    fn forget(&self) {
        // A context still waited for is live.
        for ctx in self.pending.take() {
            drop(ctx.replace_join(None));
        }
    }
}

/// The joiner's resume, deferred to the loop once its last joined coroutine closed.
struct JoinResume {
    join: Rc<Join>,
}

impl swoole_rs_corebase::reactor::event_api::Deferred for JoinResume {
    fn run(self) {
        // The token goes before the resume, which may not return.
        let (co, canceled) = (self.join.co, self.join.canceled.get());
        drop(self);
        if !canceled {
            host::resume(co);
        }
    }
    fn dispose(self) {}
}

/// `Co::join(array $cid_array, float $timeout = -1)`.
fn join(call: &Call) {
    let Some(co) = host::require_coroutine() else {
        return;
    };
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.array_ref(1)?, call.double(2, -1.0)?)));
    let Ok((cids, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    if cids.count() == 0 {
        host::set_last_error(SW_ERROR_INVALID_PARAMS);
        call.set_bool(false);
        return;
    }
    let own = host::cid(co);
    let mut pending = Vec::new();
    for (_, value) in cids.entries() {
        let cid = value.long();
        if cid == own {
            host::error_ex(E_WARNING, SW_ERROR_WRONG_OPERATION, c"can not join self");
            call.set_bool(false);
            return;
        }
        let Some(ctx) = context::by_cid(cid) else {
            continue;
        };
        // Another join's, or this one's for a repeated id.
        if ctx.joined() || pending.contains(&ctx) {
            host::set_last_error(SW_ERROR_WRONG_OPERATION);
            call.set_bool(false);
            return;
        }
        pending.push(ctx);
    }
    if pending.is_empty() {
        host::set_last_error(SW_ERROR_INVALID_PARAMS);
        call.set_bool(false);
        return;
    }
    let join = Rc::new(Join {
        pending: Cell::new(pending),
        co,
        canceled: Cell::new(false),
    });
    // Each context was joined by none, so no join is replaced.
    join.pending.with_taken(|pending| {
        for &ctx in pending.iter() {
            drop(ctx.replace_join(Some(Rc::clone(&join))));
        }
    });
    // This frame lives on the running coroutine's stack, which no bailout
    // unwinds while it waits.
    if !host::yield_ex(co, Deadline::from_timeout(timeout)) {
        join.forget();
        join.canceled.set(true);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `php_swoole_coroutine_throw_exception`: cancel `cid`, throwing `ce` in it
/// when `throw`.
fn throw_exception(
    cid: c_long,
    throw: bool,
    class: Option<ExceptionClass>,
    reason: Option<&Value>,
) -> bool {
    if !throw {
        let Some(co) = host::by_cid(cid) else {
            host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
            return false;
        };
        return host::cancel(co);
    }
    let Some((task, co)) = context::by_cid(cid).and_then(|task| Some((task, task.co()?))) else {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        return false;
    };
    // Registered at MINIT, before any coroutine runs.
    let Some(class) = class else {
        return false;
    };
    let is_current = Some(co) == host::current();
    if !is_current && task.pending_class() == Some(class.as_ptr()) {
        host::set_last_error(SW_ERROR_CO_CANCELED);
        return false;
    }
    let exception = Exception::new(class, reason);
    // The new exception, located in the coroutine's frame: the running one's
    // is the live Co::cancel frame, internal, so Zend redirects the user caller
    // when it returns.
    task.in_frame(|| exception.locate());
    if is_current {
        // `zend_throw_exception_object` of a zval holding it, past its
        // Throwable check (a Swoole exception class): Zend takes the new
        // object's reference.
        exception.throw();
        return true;
    }
    task.set_exception(exception, class);
    // Even an irrevocable wait ends: the exception terminates the coroutine.
    host::cancel(co);
    true
}

/// `Co::cancel(int $cid, bool $throw_exception = false, mixed $reason = null)`.
fn cancel(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(2, false)?)));
    let Ok((cid, throw)) = parsed else {
        call.set_bool(false);
        return;
    };
    let reason = call
        .value(3)
        .map(Value::referred)
        .filter(|value| !value.is_null());
    if !throw && reason.is_some() {
        call.argument_value_error(
            3,
            c"must be null when argument #2 ($throw_exception) is false",
        );
        return;
    }
    call.set_bool(throw_exception(cid, throw, classes::canceled(), reason));
}

/// `Co::setTimeLimit(float $timeout)`.
fn set_time_limit(call: &Call) {
    let Ok(timeout) = call.arity(1, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return;
    };
    let co = host::require_coroutine();
    let Some(co) = co.filter(|_| timeout >= 0.0) else {
        call.set_bool(false);
        return;
    };
    let cid = host::cid(co);
    let Some(ctx) = context::current_task() else {
        call.set_bool(false);
        return;
    };
    let timer = if timeout == 0.0 {
        None
    } else {
        let Some(timer) =
            swoole_rs_corebase::timer::add_fn_sec(timeout, false, move || time_limit(cid))
        else {
            call.set_bool(false);
            return;
        };
        Some(timer)
    };
    if let Some(old) = context::swap_time_limit(ctx, cid, timer) {
        swoole_rs_corebase::timer::del_id(old);
    }
    call.set_bool(true);
}

fn time_limit(cid: c_long) {
    if let Some(ctx) = context::by_cid(cid) {
        // Its timer has fired.
        let _fired = context::swap_time_limit(ctx, cid, None);
        throw_exception(cid, true, classes::timeout(), None);
    }
}

/// `Co::isCanceled()`.
fn is_canceled(call: &Call) {
    let Some(co) = host::require_coroutine() else {
        return;
    };
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
    if stats.iouring {
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
        Some(context::current())
    } else {
        context::by_cid(cid)
    };
    let pcid = ctx.map_or(0, Ctx::pcid);
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
        context::task(cid)
    };
    let Some(ctx) = ctx else {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        call.set_null();
        return;
    };
    let Some(object) = ctx.context_object() else {
        // A context destroyed with its coroutine (swoole-src#2991).
        host::fatal_error(E_WARNING, c"The context of coroutine has been destroyed");
        call.set_null();
        return;
    };
    // The return value takes a new reference; the context keeps the first.
    call.set_owned(object);
}

/// `Co::getElapsed(int $cid = 0)`.
fn get_elapsed(call: &Call) {
    let Some(cid) = optional_cid(call) else {
        call.set_bool(false);
        return;
    };
    let elapsed = if runtime::activated() {
        swoole_rs_corebase::coroutine::elapsed(cid)
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
    let Some(ctx) = context::by_cid(cid) else {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        call.set_bool(false);
        return;
    };
    let usage = ctx.stack_usage();
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
    // `(int) value`, as C's conversion to the `int` Zend takes.
    let (options, limit) = (options.c_cast::<i32>(), limit.c_cast::<i32>());
    // The return slot holds nothing yet, as the frames were written straight into it.
    let trace = || call.set_owned(tracer::backtrace(0, options, limit));
    if cid == 0 || cid == runtime::get_cid() {
        return trace();
    }
    let Some(ctx) = context::by_cid(cid) else {
        host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
        call.set_bool(false);
        return;
    };
    // Another coroutine's trace is taken from its frame.
    ctx.in_frame(trace);
}

/// `sw_php_print_backtrace`.
fn print_backtrace(
    php: Php<'_>,
    cid: c_long,
    options: i64,
    limit: i64,
    call: Option<&Call>,
) -> Result<(), Bailout> {
    // `(int) value`, as C's conversion to the `int` Zend takes.
    let (options, limit) = (options.c_cast::<i32>(), limit.c_cast::<i32>());
    let trace = if cid == 0 || cid == runtime::get_cid() {
        tracer::backtrace(1, options, limit)
    } else {
        let Some(ctx) = context::by_cid(cid) else {
            host::set_last_error(SW_ERROR_CO_NOT_EXISTS);
            if let Some(call) = call {
                call.set_bool(false);
            }
            return Ok(());
        };
        // Fetched in the coroutine's frame, which runs no PHP code. The output
        // below may, so it runs once this coroutine's frame is back.
        ctx.in_frame(|| tracer::backtrace(0, options, limit))
    };
    leased::print_trace(php, &trace)
}

/// `sw_php_print_backtrace(cid)`, printed (crates/phpswoole's fatal error).
/// # Errors
/// The output bailed out.
pub fn print_coroutine_backtrace(php: Php<'_>, cid: c_long) -> Result<(), Bailout> {
    print_backtrace(php, cid, 0, 0, None)
}

/// `Co::printBackTrace(int $cid = 0, int $options = 0, int $limit = 0)`.
fn print_back_trace(call: &Call) {
    let Some((cid, options, limit)) = trace_args(call, 0) else {
        call.set_bool(false);
        return;
    };
    if print_backtrace(call.php(), cid, options, limit, Some(call)).is_err() {
        Bailout.defer();
    }
}

/// `Co::list()`/`Co::listCoroutines()`: a `Coroutine\Iterator` of the ids.
fn list(call: &Call) {
    let mut cids: Vec<c_long> = vec![0; host::count()];
    let count = host::list(&mut cids);
    let mut ids = OwnedArray::default();
    for &cid in cids.iter().take(count) {
        ids.push_long(cid);
    }
    let ids = ids.into_owned();
    drop(cids);
    match classes::iterator().instantiate(&[&ids]) {
        Ok(iterator) => call.set_owned(iterator),
        Err(bailed) => bailed.defer(),
    }
}

/// `Co::getExecuteTime()` (`SW_CORO_TIME` builds).
fn get_execute_time(call: &Call) {
    let time = if runtime::activated() {
        swoole_rs_corebase::coroutine::current_execute_time()
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
    let Some(ctx) = context::current_task() else {
        return false;
    };
    if ctx.enable_scheduler() == enable {
        return false;
    }
    ctx.set_enable_scheduler(enable);
    true
}
