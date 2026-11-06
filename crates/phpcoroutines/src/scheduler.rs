//! `Swoole\Coroutine\Scheduler`, the coroutine options (`Co::set()`,
//! `Co::getOptions()`, `php_swoole_set_coroutine_option`) and the reactor's
//! user exit condition: what `swoole_coroutine_scheduler.cc` held.
use crate::{host, runtime};
use std::{
    cell::Cell,
    collections::VecDeque,
    ffi::{CStr, c_int, c_long, c_void},
    mem, ptr,
};
use swoole_rs_phphelpers::callable as kept;
use swoole_rs_phpstubs::coroutine_scheduler;
use swoole_rs_zend::{
    Call, Callable, Class, E_ERROR, E_WARNING, Object,
    bailout::{self, Bailout},
    class_name, message,
    sys::{
        IS_ARRAY, IS_ARRAY_EX, IS_FALSE, IS_NULL, ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE,
        swr_zend_array_init, swr_zend_callable, swr_zval_get_long, swr_zval_is_true, zend_array,
        zend_array_destroy, zend_array_dup, zend_class_entry, zend_hash_merge, zend_hash_str_find,
        zend_object, zend_object_handlers, zval, zval_add_ref, zval_ptr_dtor,
    },
    type_of, unset_property_deny, value,
};
use swoole_rs_zend::{method_table, sys as z};

per_thread! {
    /// `PHPCoroutine::options`: what `Co::set()` was given.
    static OPTIONS: Cell<*mut zend_array> = Cell::new(ptr::null_mut());
    /// `exit_condition_fci_cache`: `Co::set()`'s `exit_condition` callable.
    static EXIT_CONDITION: Cell<*mut kept::Callable> = Cell::new(ptr::null_mut());
}

unsafe extern "C" {
    /// Unlinks the first PHP name resolver `same` accepts; its zval, or null.
    fn swoole_name_resolver_php_remove(
        same: unsafe extern "C" fn(*mut zval, *mut zval) -> bool,
        resolver: *mut zval,
    ) -> *mut zval;
}

/// One `add()` or `parallel()`: its kept callable and how many coroutines run it.
struct Task {
    callable: *mut c_void,
    count: i64,
}

/// `SchedulerObject`: the queued tasks (null until the first) and whether
/// `start()` is running. All-zero is a fresh scheduler.
struct Scheduler {
    list: *mut VecDeque<Task>,
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

/// MINIT: `Swoole\Coroutine\Scheduler` (`Co\Scheduler`), final, not
/// serializable, clone and unset denied.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_coroutine_scheduler_minit(_module_number: c_int) {
    // SAFETY: the native MINIT entry lends these original startup arguments.
    let result = unsafe { init_phpcoroutines_scheduler_minit() };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            swoole_rs_zend::bailout::finish();
        }
    }
}

unsafe fn init_phpcoroutines_scheduler_minit() -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // SAFETY: MINIT, with a static table.
    unsafe { SCHEDULER.register(c"Swoole\\Coroutine\\Scheduler", METHODS.as_ptr(), configure) };
    if host::flag(host::SHORTNAME) {
        // SAFETY: MINIT, after registration.
        unsafe { SCHEDULER.alias(c"Co\\Scheduler") }?;
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

/// Release a queued task's callable and arguments, then free it.
fn discard(callable: *mut c_void) -> Result<(), Bailout> {
    // SAFETY: a kept callable, not used again.
    unsafe { Callable::from_raw(callable) }.release()
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees one of our objects.
    let list = mem::replace(
        &mut unsafe { SchedulerObject::data(std) }.list,
        ptr::null_mut(),
    );
    if !list.is_null() {
        // SAFETY: boxed by `queue`, owned by this object.
        let tasks = unsafe { Box::from_raw(list) };
        for task in *tasks {
            bailout::defer(discard(task.callable));
        }
    }
    // SAFETY: as above.
    unsafe { SchedulerObject::dtor(std) };
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
    // SAFETY: `$this` is live.
    let class = unsafe { class_name(call.this()) };
    let text = message!(
        "scheduler is {state}, unable to execute {}->{method}",
        class.to_string_lossy()
    );
    host::fatal_error(E_WARNING, text.as_cstr());
    call.set_bool(false);
}

/// Queue a task with its callable kept past the call.
fn queue(scheduler: &mut Scheduler, mut callable: Callable, count: i64) {
    callable.keep(None);
    if scheduler.list.is_null() {
        scheduler.list = Box::into_raw(Box::default());
    }
    // SAFETY: the scheduler's own queue.
    unsafe { &mut *scheduler.list }.push_back(Task {
        callable: callable.into_raw(),
        count,
    });
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
    // SAFETY: C++ initialises the reactor.
    if unsafe { (host::ops().reactor_init)() } < 0 {
        call.set_bool(false);
        return;
    }
    this(call).started = true;
    let list = this(call).list;
    if list.is_null() {
        host::fatal_error(E_WARNING, c"no coroutine task");
        call.set_bool(false);
        return;
    }
    // Each step pops before creating, so no frame here owns a task while a
    // coroutine runs.
    // SAFETY: the scheduler's own queue, live until the end of `start`.
    while let Some(task) = unsafe { &mut *list }.pop_front() {
        let callable = task.callable.cast::<swr_zend_callable>();
        for _ in 0..task.count {
            // SAFETY: a kept callable; no reference outlives this statement.
            let kept = unsafe { &mut *callable };
            let (argc, argv) = (kept.fci.param_count, kept.fci.params);
            let (fcc, name) = (&raw mut kept.fcc, &raw mut kept.fci.function_name);
            if runtime::create(fcc, argc, argv, name).is_err() {
                runtime::raise();
                return;
            }
        }
        bailout::defer(discard(task.callable));
        if bailout::pending() {
            return;
        }
    }
    // SAFETY: C++ runs the event loop.
    unsafe { (host::ops().event_wait)() };
    // SAFETY: the queue is empty and owned by this object.
    drop(unsafe { Box::from_raw(list) });
    let scheduler = this(call);
    scheduler.list = ptr::null_mut();
    scheduler.started = false;
    call.set_bool(true);
}

/// `php_swoole_array_get_value`: the non-null value at `key`.
fn get(options: *mut zend_array, key: &CStr) -> Option<*mut zval> {
    // SAFETY: a live array and a NUL-terminated key.
    let found = unsafe { zend_hash_str_find(options, key.as_ptr(), key.count_bytes()) };
    // SAFETY: null or a live element.
    (!found.is_null() && unsafe { swoole_rs_zend::type_of(found) } != IS_NULL).then_some(found)
}

fn long(value: *mut zval) -> c_long {
    // SAFETY: a live zval.
    unsafe { swr_zval_get_long(value) }
}

fn truthy(value: *mut zval) -> bool {
    // SAFETY: a live zval.
    unsafe { swr_zval_is_true(value) }
}

/// `php_swoole_set_coroutine_option`.
/// # Safety
/// `options` is a live array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_set_coroutine_option(options: *mut zend_array) {
    if let Some(value) = get(options, c"max_coro_num").or_else(|| get(options, c"max_coroutine")) {
        let max = long(value);
        let max = if max <= 0 {
            runtime::DEFAULT_MAX_NUM
        } else {
            max
        };
        runtime::set_max_num(max.cast_unsigned());
    }
    if let Some(value) = get(options, c"enable_deadlock_check") {
        runtime::set_deadlock_check(truthy(value));
    }
    if let Some(value) = get(options, c"hook_flags") {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream passes zend_long as uint32_t"
        )]
        set_hook_flags(long(value) as u32);
    }
    if let Some(value) = get(options, c"enable_preemptive_scheduler") {
        runtime::swr_v1_phpcoroutines_set_preemptive(u8::from(truthy(value)));
    }
    if let Some(value) = get(options, c"c_stack_size").or_else(|| get(options, c"stack_size")) {
        // SAFETY: C++ parses the size and sets the coroutine stack size.
        unsafe { (host::ops().set_stack_size)(value) };
    }
    if let Some(value) = get(options, c"name_resolver") {
        // SAFETY: a live zval.
        if unsafe { swoole_rs_zend::type_of(value) } == IS_ARRAY {
            // SAFETY: an array argument.
            let table = unsafe { value::table(value) };
            let add = |resolver: *mut zval| {
                // SAFETY: C++ adds the resolver.
                unsafe { (host::ops().add_name_resolver)(resolver) };
                true
            };
            // SAFETY: the array is live and the closure does not modify it.
            unsafe { swoole_rs_zend::each_value(table, add) };
        } else {
            host::fatal_error(E_WARNING, c"name_resolver must be an array");
        }
    }
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
pub(crate) fn set_hook_flags(flags: u32) {
    // SAFETY: all-zero is IS_UNDEF, then an array.
    let mut array: zval = unsafe { mem::zeroed() };
    // SAFETY: a writable zval.
    unsafe { swr_zend_array_init(&raw mut array) };
    // SAFETY: an array.
    unsafe { swoole_rs_zend::add_assoc_long(&raw mut array, c"hook_flags", i64::from(flags)) };
    // SAFETY: as above.
    let table = unsafe { value::table(&raw const array) };
    let current = OPTIONS.with(Cell::get);
    if current.is_null() {
        OPTIONS.with(|o| o.set(table));
    } else {
        // SAFETY: two live arrays.
        unsafe { zend_hash_merge(current, table, None, true) };
        // SAFETY: our array, merged.
        unsafe { zval_ptr_dtor(&raw mut array) };
    }
    runtime::swr_v1_phpcoroutines_set_hook_flags(flags);
}

/// `PHPCoroutine::set_hook_flags` for C++.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_set_hook_option(flags: u32) {
    set_hook_flags(flags);
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
    let Ok(options) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: an array argument.
    let options = unsafe { value::table(options) };
    // SAFETY: C++'s global options.
    unsafe { (host::ops().set_global_option)(options) };
    // SAFETY: a live array.
    unsafe { php_swoole_set_coroutine_option(options) };
    if let Some(value) = get(options, c"dns_cache_expire") {
        // SAFETY: C++'s DNS cache.
        unsafe { (host::ops().set_dns_cache_expire)(long(value)) };
    }
    if let Some(value) = get(options, c"dns_cache_capacity") {
        // SAFETY: as above.
        unsafe { (host::ops().set_dns_cache_capacity)(long(value)) };
    }
    // SAFETY: a live array and key.
    let condition = unsafe { zend_hash_str_find(options, c"exit_condition".as_ptr(), 14) };
    if !condition.is_null() {
        bailout::raise(set_exit_condition(condition));
    }
}

/// `php_swoole_coroutine_set_exit_condition`: keep `condition` if it is
/// callable (warning otherwise) and install or clear the reactor's user exit
/// condition to match.
fn set_exit_condition(condition: *mut zval) -> Result<(), Bailout> {
    release_exit_condition()?;
    // SAFETY: a live zval; no argument name, so no exception class is read;
    // C++'s warning reporter.
    let handle = unsafe {
        kept::factory(
            condition,
            ptr::null(),
            true,
            ptr::null_mut(),
            host::ops().warning,
        )
    }?;
    EXIT_CONDITION.with(|c| c.set(handle));
    // SAFETY: C++ sets or clears the reactor's user exit condition.
    unsafe { (host::ops().set_exit_condition)(u8::from(!handle.is_null())) };
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

/// `php_swoole_coroutine_reactor_can_exit`: call the exit condition; whether
/// the reactor may exit goes in `*exit`. 1 when it bailed out, which C++
/// re-raises.
/// # Safety
/// `exit` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcoroutines_can_exit(exit: *mut u8) -> u8 {
    let result = can_exit();
    // SAFETY: per the caller.
    unsafe { exit.write(u8::from(matches!(result, Ok(true)))) };
    u8::from(bailout::relay_to_c(result.map(drop)))
}

fn can_exit() -> Result<bool, Bailout> {
    let handle = EXIT_CONDITION.with(Cell::get);
    if handle.is_null() {
        return Ok(true);
    }
    // SAFETY: all-zero is a zval; it is made null.
    let mut retval: zval = unsafe { mem::zeroed() };
    retval.u1.type_info = IS_NULL;
    // SAFETY: the kept callable, no arguments and a result slot, which (as
    // upstream's) is not released.
    if !unsafe { kept::invoke(handle, 0, ptr::null_mut(), &raw mut retval) }? {
        // SAFETY: C++'s reporter, inside a trap.
        unsafe {
            bailout::report_level(
                host::ops().fatal_error,
                E_ERROR,
                c"Coroutine can_exit callback handler error",
            )
        }?;
    }
    // SAFETY: the executor's pending exception, if any.
    let exception = unsafe { z::swr_zend_facade_exception() };
    if !exception.is_null() {
        // SAFETY: zend_exception_error(EG(exception), E_ERROR), inside a trap.
        unsafe { bailout::trap(z::swr_zend_facade_exception_error, exception.cast()) }?;
    }
    // SAFETY: the callable's result.
    Ok(unsafe { type_of(&raw const retval) } != IS_FALSE)
}

unsafe extern "C" fn any(_: *mut zval, _: *mut zval) -> bool {
    true
}

/// RSHUTDOWN: drop the PHP name resolvers and the exit condition.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_coroutine_scheduler_rshutdown() {
    bailout::raise(rshutdown());
}

fn rshutdown() -> Result<(), Bailout> {
    loop {
        // SAFETY: C++'s resolver list; every PHP resolver matches.
        let resolver = unsafe { swoole_name_resolver_php_remove(any, ptr::null_mut()) };
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
