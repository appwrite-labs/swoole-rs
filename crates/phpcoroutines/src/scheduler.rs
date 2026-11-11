//! `Swoole\Coroutine\Scheduler`, the coroutine options (`Co::set()`,
//! `Co::getOptions()`, `php_swoole_set_coroutine_option`) and the reactor's
//! user exit condition: what `swoole_coroutine_scheduler.cc` held.
use crate::{host, runtime};
use std::{cell::Cell, collections::VecDeque, mem, ptr};
use swoole_rs_phpstubs::coroutine_scheduler;
use swoole_rs_zend::host::callable as kept;
use swoole_rs_zend::{
    Call, Callable, Class, E_ERROR, E_WARNING, Minit, Object, OwnedArray, Parsed,
    bailout::{self, Bailout},
    kind, message,
    sys::{
        IS_ARRAY_EX, IS_FALSE, IS_NULL, ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, zend_array,
        zend_array_destroy, zend_array_dup, zend_class_entry, zend_hash_merge, zend_object,
        zend_object_handlers, zval, zval_add_ref,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

per_thread! {
    /// `PHPCoroutine::options`: what `Co::set()` was given.
    static OPTIONS: Cell<*mut zend_array> = Cell::new(ptr::null_mut());
    /// `exit_condition_fci_cache`: `Co::set()`'s `exit_condition` callable.
    static EXIT_CONDITION: Cell<*mut kept::Callable> = Cell::new(ptr::null_mut());
}

/// One `add()` or `parallel()`: its kept callable and how many coroutines run it.
struct Task {
    callable: Callable,
    count: i64,
}

/// `SchedulerObject`: the queued tasks (none until the first) and whether
/// `start()` is running. All-zero is a fresh scheduler.
struct Scheduler {
    #[expect(
        clippy::box_collection,
        reason = "Zend zeroes the object: a null Box is None, while an empty queue is not all-zero"
    )]
    list: Option<Box<VecDeque<Task>>>,
    started: bool,
}

type SchedulerObject = Object<Scheduler>;

static SCHEDULER: Class = Class::new();

method_table! {
    static METHODS = [
        c"add" => zim_swoole_coroutine_scheduler_add, &coroutine_scheduler::arginfo_class_Swoole_Coroutine_Scheduler_add, z::ZEND_ACC_PUBLIC;
        c"parallel" => zim_swoole_coroutine_scheduler_parallel, &coroutine_scheduler::arginfo_class_Swoole_Coroutine_Scheduler_parallel, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_coroutine_scheduler_set, &coroutine_scheduler::arginfo_class_Swoole_Coroutine_Scheduler_set, z::ZEND_ACC_PUBLIC;
        c"getOptions" => zim_swoole_coroutine_scheduler_getOptions, &coroutine_scheduler::arginfo_class_Swoole_Coroutine_Scheduler_getOptions, z::ZEND_ACC_PUBLIC;
        c"start" => zim_swoole_coroutine_scheduler_start, &coroutine_scheduler::arginfo_class_Swoole_Coroutine_Scheduler_start, z::ZEND_ACC_PUBLIC;
    ];
}

swoole_rs_zend::minit!(php_swoole_coroutine_scheduler_minit => register);

/// MINIT: `Swoole\Coroutine\Scheduler` (`Co\Scheduler`), final, not
/// serializable, clone and unset denied.
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register(
        &SCHEDULER,
        c"Swoole\\Coroutine\\Scheduler",
        &METHODS.0,
        configure,
    );
    if host::flag(host::SHORTNAME) {
        minit.alias(&SCHEDULER, c"Co\\Scheduler")?;
    }
    Ok(())
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    SchedulerObject::install(entry, handlers, create, free);
    entry.ce_flags |= ZEND_ACC_FINAL;
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates an object of the scheduler class.
    unsafe { SchedulerObject::create(&SCHEDULER, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees one of our objects.
    let list = mem::take(&mut unsafe { SchedulerObject::data(std) }.list);
    if let Some(tasks) = list {
        for task in *tasks {
            bailout::defer(task.callable.release());
        }
    }
    // SAFETY: as above.
    unsafe { SchedulerObject::dtor(std) };
    bailout::finish();
}

swoole_rs_zend::handlers! {
    zim_swoole_coroutine_scheduler_add => add;
    zim_swoole_coroutine_scheduler_parallel => parallel;
    zim_swoole_coroutine_scheduler_start => start;
    zim_swoole_coroutine_scheduler_set => set;
    zim_swoole_coroutine_scheduler_getOptions => get_options;
}

fn this(call: &Call) -> &'static mut Scheduler {
    // SAFETY: Scheduler's instance methods run on one of our objects.
    unsafe { SchedulerObject::data(call.this()) }
}

/// The "scheduler is running/started, unable to execute %s->%s" warning.
fn refuse(call: &Call, state: &str, method: &str) {
    let class = call.class_name().unwrap_or_default();
    let text = message!(
        "scheduler is {state}, unable to execute {}->{method}",
        class.to_string_lossy()
    );
    host::fatal_error(E_WARNING, text.as_cstr());
    call.set_bool(false);
}

/// Queue a task with its callable kept past the call.
fn queue(scheduler: &mut Scheduler, callable: Parsed, count: i64) {
    let callable = callable.keep(None);
    scheduler
        .list
        .get_or_insert_default()
        .push_back(Task { callable, count });
}

/// `add(callable $func, mixed ...$params)`.
fn add(call: &Call) {
    if this(call).started {
        refuse(call, "running", "add");
        return;
    }
    let Ok(callable) = call
        .arity(1, u32::MAX)
        .and_then(|()| Callable::parse(call, 1))
    else {
        call.set_bool(false);
        return;
    };
    queue(this(call), callable, 1);
}

/// `parallel(int $n, callable $func, mixed ...$params)`.
fn parallel(call: &Call) {
    if this(call).started {
        refuse(call, "running", "parallel");
        return;
    }
    let parsed = call
        .arity(2, u32::MAX)
        .and_then(|()| Ok((call.long(1, 0)?, Callable::parse(call, 2)?)));
    let Ok((count, callable)) = parsed else {
        call.set_bool(false);
        return;
    };
    queue(this(call), callable, count);
}

/// `start()`: create every task's coroutines in order, then run the event
/// loop until they finish.
fn start(call: &Call) {
    if this(call).started {
        refuse(call, "started", "start");
        return;
    }
    if host::reactor_init() < 0 {
        call.set_bool(false);
        return;
    }
    this(call).started = true;
    if this(call).list.is_none() {
        host::fatal_error(E_WARNING, c"no coroutine task");
        call.set_bool(false);
        return;
    }
    // Each step pops before creating, so no frame here owns a task while a
    // coroutine runs.
    while let Some(task) = this(call).list.as_mut().and_then(|list| list.pop_front()) {
        for _ in 0..task.count {
            let (fcc, argc, argv, name) = task.callable.parts();
            if runtime::create(fcc, argc, argv, name).is_err() {
                runtime::raise();
                return;
            }
        }
        bailout::defer(task.callable.release());
        if bailout::pending() {
            return;
        }
    }
    host::event_wait();
    let scheduler = this(call);
    scheduler.list = None;
    scheduler.started = false;
    call.set_bool(true);
}

/// `php_swoole_set_coroutine_option`. It stops at a deferred bailout, where
/// upstream's ended the call, so a caller checks [`bailout::pending`] after it.
pub fn set_coroutine_option(options: &swoole_rs_zend::Array) {
    if let Some(value) = options
        .get(b"max_coro_num")
        .or_else(|| options.get(b"max_coroutine"))
    {
        let Some(max) = value.applied_long() else {
            return;
        };
        let max = if max <= 0 {
            runtime::DEFAULT_MAX_NUM
        } else {
            max
        };
        runtime::set_max_num(max.cast_unsigned());
    }
    if let Some(value) = options.get(b"enable_deadlock_check") {
        runtime::set_deadlock_check(value.truthy());
    }
    if let Some(value) = options.get(b"hook_flags") {
        let Some(flags) = value.applied_long() else {
            return;
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream passes zend_long as uint32_t"
        )]
        set_hook_flags(flags as u32);
    }
    if let Some(value) = options.get(b"enable_preemptive_scheduler") {
        runtime::set_preemptive(value.truthy());
    }
    if let Some(value) = options
        .get(b"c_stack_size")
        .or_else(|| options.get(b"stack_size"))
    {
        host::set_stack_size(value);
    }
    if let Some(value) = options.get(b"name_resolver") {
        if let Some(resolvers) = value.array() {
            for (_, resolver) in resolvers.entries() {
                host::add_name_resolver(resolver);
            }
        } else {
            host::fatal_error(E_WARNING, c"name_resolver must be an array");
        }
    }
    let options = options.as_ptr();
    let current = OPTIONS.with(Cell::get);
    if current.is_null() {
        // SAFETY: a live array.
        OPTIONS.with(|o| o.set(unsafe { zend_array_dup(options) }));
    } else {
        // SAFETY: two live arrays; values are copied with a reference.
        unsafe { zend_hash_merge(current, options, Some(zval_add_ref), true) };
    }
}

/// `PHPCoroutine::set_hook_flags`: the flags, recorded in the options too.
pub fn set_hook_flags(flags: u32) {
    let mut array = OwnedArray::default();
    array.insert_long(b"hook_flags", i64::from(flags));
    let array = array.into_owned();
    let current = OPTIONS.with(Cell::get);
    if current.is_null() {
        let table = array
            .array()
            .map_or(ptr::null_mut(), swoole_rs_zend::Array::as_ptr);
        // The options now own the table.
        let _kept = array.into_zval();
        OPTIONS.with(|o| o.set(table));
    } else if let Some(table) = array.array() {
        // SAFETY: two live arrays; ours is released after the merge.
        unsafe { zend_hash_merge(current, table.as_ptr(), None, true) };
    }
    runtime::store_hook_flags(flags);
}

/// Free the options, at RSHUTDOWN.
pub(crate) fn free_options() {
    let options = OPTIONS.with(|o| o.replace(ptr::null_mut()));
    if !options.is_null() {
        // SAFETY: our array.
        unsafe { zend_array_destroy(options) };
    }
}

/// `Co::set(array $options)`/`Scheduler::set()`.
fn set(call: &Call) {
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        call.set_bool(false);
        return;
    };
    swoole_rs_phpswoole::php_swoole_set_global_option(array);
    set_coroutine_option(array);
    if bailout::pending() {
        return;
    }
    if let Some(value) = array.get(b"dns_cache_expire") {
        let Some(expire) = value.applied_long() else {
            return;
        };
        host::set_dns_cache_expire(expire);
    }
    if let Some(value) = array.get(b"dns_cache_capacity") {
        let Some(capacity) = value.applied_long() else {
            return;
        };
        host::set_dns_cache_capacity(capacity);
    }
    if let Some(condition) = array.find(b"exit_condition") {
        bailout::raise(set_exit_condition(condition.as_ptr()));
    }
}

/// `php_swoole_coroutine_set_exit_condition`: keep `condition` if it is
/// callable (warning otherwise) and install or clear the reactor's user exit
/// condition to match.
fn set_exit_condition(condition: *mut zval) -> Result<(), Bailout> {
    release_exit_condition()?;
    // SAFETY: a live zval; no argument name, so no exception class is read;
    // C++'s warning reporter.
    let handle =
        unsafe { kept::factory(condition, ptr::null(), true, ptr::null_mut(), host::warning) }?;
    EXIT_CONDITION.with(|c| c.set(handle));
    host::set_exit_condition(!handle.is_null());
    Ok(())
}

fn release_exit_condition() -> Result<(), Bailout> {
    let handle = EXIT_CONDITION.with(|c| c.replace(ptr::null_mut()));
    if handle.is_null() {
        return Ok(());
    }
    // SAFETY: the kept callable, not used again.
    unsafe { kept::release(handle) }
}

/// Install `Co::set`'s exit condition, if it gave one, on this thread's
/// reactor, as `php_swoole_reactor_init` did for a new reactor.
pub fn install_exit_condition() {
    if !EXIT_CONDITION.with(Cell::get).is_null() {
        host::set_exit_condition(true);
    }
}

/// The reactor's user exit condition: whether it may exit. A bailout is raised
/// from here, as upstream's lambda raised it.
pub(crate) extern "C-unwind" fn reactor_can_exit(_event_num: &mut usize) -> bool {
    match can_exit() {
        Ok(exit) => exit,
        Err(error) => {
            bailout::raise(Err(error));
            false
        }
    }
}

/// `php_swoole_coroutine_reactor_can_exit`: call the exit condition.
fn can_exit() -> Result<bool, Bailout> {
    let handle = EXIT_CONDITION.with(Cell::get);
    if handle.is_null() {
        return Ok(true);
    }
    let mut retval = swoole_rs_zend::UNDEF;
    retval.u1.type_info = IS_NULL;
    // SAFETY: the kept callable, no arguments and a result slot, which (as
    // upstream's) is not released.
    if !unsafe { kept::invoke(handle, 0, ptr::null_mut(), &raw mut retval) }? {
        host::try_fatal_error(E_ERROR, c"Coroutine can_exit callback handler error")?;
    }
    // SAFETY: the executor's pending exception, if any.
    let exception = unsafe { z::swr_zend_facade_exception() };
    if !exception.is_null() {
        // SAFETY: zend_exception_error(EG(exception), E_ERROR), inside a trap.
        unsafe { bailout::trap(z::swr_zend_facade_exception_error, exception.cast()) }?;
    }
    Ok(kind(&retval) != IS_FALSE)
}

/// RSHUTDOWN: drop the PHP name resolvers and the exit condition.
pub extern "C" fn php_swoole_coroutine_scheduler_rshutdown() {
    bailout::raise(rshutdown());
}

fn rshutdown() -> Result<(), Bailout> {
    let Some(families) = swoole_rs_phpswoole::families() else {
        return Ok(());
    };
    loop {
        // Every PHP resolver matches `None`; each match is unlinked and returned
        // (crates/phpclients).
        let resolver = (families.name_resolver_remove)(None);
        if resolver.is_null() {
            break;
        }
        // SAFETY: zval_dtor of the unlinked resolver, inside a trap.
        unsafe { bailout::trap(z::swr_zend_zval_release, resolver.cast()) }?;
        // SAFETY: emalloc'd by the resolver's registration; nothing refers to it.
        unsafe { z::swr_zend_efree(resolver.cast()) };
    }
    release_exit_condition()
}

/// `Co::getOptions()`/`Scheduler::getOptions()`.
fn get_options(call: &Call) {
    let options = OPTIONS.with(Cell::get);
    if options.is_null() {
        return;
    }
    // SAFETY: a live array, copied into the return slot.
    let copy = unsafe { zend_array_dup(options) };
    // SAFETY: the return slot takes the copy (RETURN_ARR).
    let slot = unsafe { &mut *call.return_value() };
    slot.value.arr = copy;
    slot.u1.type_info = IS_ARRAY_EX;
}
