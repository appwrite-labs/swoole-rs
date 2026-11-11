//! `Swoole\Coroutine\Scheduler`, the coroutine options (`Co::set()`,
//! `Co::getOptions()`, `php_swoole_set_coroutine_option`) and the reactor's
//! user exit condition: what `swoole_coroutine_scheduler.cc` held.
use crate::{host, runtime};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    mem::ManuallyDrop,
    ptr,
};
use swoole_rs_phpstubs::coroutine_scheduler;
use swoole_rs_zend::host::callable as kept;
use swoole_rs_zend::{
    Call, Callable, Custom, CustomClass, E_ERROR, E_WARNING, Minit, OwnedArray, Parsed,
    bailout::{self, Bailout},
    kind, message,
    sys::{IS_FALSE, IS_NULL, ZEND_ACC_FINAL, zval},
};
use swoole_rs_zend::{method_table, sys as z};

per_thread! {
    /// `PHPCoroutine::options`: what `Co::set()` was given. RSHUTDOWN frees
    /// them; a thread's locals drop after Zend's request memory is gone, so
    /// one that outlives it (a shutdown that bailed out) leaks with it.
    static OPTIONS: RefCell<Option<ManuallyDrop<OwnedArray>>> = RefCell::new(None);
    /// `exit_condition_fci_cache`: `Co::set()`'s `exit_condition` callable.
    static EXIT_CONDITION: Cell<*mut kept::Callable> = Cell::new(ptr::null_mut());
}

/// One `add()` or `parallel()`: its kept callable and how many coroutines run it.
struct Task {
    callable: Callable,
    count: i64,
}

/// `SchedulerObject`: the queued tasks and whether `start()` is running.
/// Methods, and the coroutines `start()` runs, share it.
#[derive(Default)]
struct Scheduler {
    list: RefCell<VecDeque<Task>>,
    started: Cell<bool>,
}

impl Custom for Scheduler {
    fn class() -> &'static CustomClass<Self> {
        &SCHEDULER
    }

    fn new() -> Self {
        Self::default()
    }
}

/// Tasks `start()` never ran release their callables with the object.
impl Drop for Scheduler {
    fn drop(&mut self) {
        for task in self.list.get_mut().drain(..) {
            bailout::defer(task.callable.release());
        }
    }
}

static SCHEDULER: CustomClass<Scheduler> = CustomClass::new();

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
    minit.register_object::<Scheduler>(c"Swoole\\Coroutine\\Scheduler", &METHODS.0, ZEND_ACC_FINAL);
    if host::flag(host::SHORTNAME) {
        minit.alias(&SCHEDULER, c"Co\\Scheduler")?;
    }
    Ok(())
}

swoole_rs_zend::handlers! {
    zim_swoole_coroutine_scheduler_add => add;
    zim_swoole_coroutine_scheduler_parallel => parallel;
    zim_swoole_coroutine_scheduler_start => start;
    zim_swoole_coroutine_scheduler_set => set;
    zim_swoole_coroutine_scheduler_getOptions => get_options;
}

/// The scheduler of `$this`, unless it is already `state` (`running` or
/// `started`): then `method` warns and returns false.
fn idle<'a>(call: &'a Call, state: &str, method: &str) -> Option<&'a Scheduler> {
    let scheduler = call.custom::<Scheduler>()?;
    if scheduler.started.get() {
        refuse(call, state, method);
        return None;
    }
    Some(scheduler)
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
fn queue(scheduler: &Scheduler, callable: Parsed, count: i64) {
    let callable = callable.keep(None);
    scheduler
        .list
        .borrow_mut()
        .push_back(Task { callable, count });
}

/// `add(callable $func, mixed ...$params)`.
fn add(call: &Call) {
    let Some(scheduler) = idle(call, "running", "add") else {
        return;
    };
    let Ok(callable) = call
        .arity(1, u32::MAX)
        .and_then(|()| Callable::parse(call, 1))
    else {
        call.set_bool(false);
        return;
    };
    queue(scheduler, callable, 1);
}

/// `parallel(int $n, callable $func, mixed ...$params)`.
fn parallel(call: &Call) {
    let Some(scheduler) = idle(call, "running", "parallel") else {
        return;
    };
    let parsed = call
        .arity(2, u32::MAX)
        .and_then(|()| Ok((call.long(1, 0)?, Callable::parse(call, 2)?)));
    let Ok((count, callable)) = parsed else {
        call.set_bool(false);
        return;
    };
    queue(scheduler, callable, count);
}

/// `start()`: create every task's coroutines in order, then run the event
/// loop until they finish.
fn start(call: &Call) {
    let Some(scheduler) = idle(call, "started", "start") else {
        return;
    };
    if host::reactor_init() < 0 {
        call.set_bool(false);
        return;
    }
    scheduler.started.set(true);
    if scheduler.list.borrow().is_empty() {
        host::fatal_error(E_WARNING, c"no coroutine task");
        call.set_bool(false);
        return;
    }
    // Each step pops before creating, so no frame here owns a task while a
    // coroutine runs.
    let next = || scheduler.list.borrow_mut().pop_front();
    while let Some(task) = next() {
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
    scheduler.started.set(false);
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
    with_options(|current| {
        // zend_hash_merge(current, options, zval_add_ref, overwrite)
        let mut entries = options.entries();
        while let Some((name, index, value)) = entries.next_stored() {
            match name {
                Some(name) => current.update(name, value.copy()),
                None => current.update_index(index, value.copy()),
            }
        }
    });
}

/// Edit this thread's options, an empty array until the first edit. They are
/// out of the cell meanwhile: a destructor an overwrite runs may reach them.
fn with_options(edit: impl FnOnce(&mut OwnedArray)) {
    let mut options = OPTIONS
        .with(RefCell::take)
        .map_or_else(OwnedArray::default, ManuallyDrop::into_inner);
    edit(&mut options);
    OPTIONS.with(|o| o.replace(Some(ManuallyDrop::new(options))));
}

/// `PHPCoroutine::set_hook_flags`: the flags, recorded in the options too.
pub fn set_hook_flags(flags: u32) {
    with_options(|options| options.insert_long(b"hook_flags", i64::from(flags)));
    runtime::store_hook_flags(flags);
}

/// Free the options, at RSHUTDOWN.
pub(crate) fn free_options() {
    drop(OPTIONS.with(RefCell::take).map(ManuallyDrop::into_inner));
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
    // RETURN_ARR(zend_array_dup(options)): a shared copy, which the next
    // edit separates from.
    if let Some(copy) = OPTIONS.with(|o| o.borrow().as_ref().map(|options| options.copy())) {
        call.set_owned(copy);
    }
}
