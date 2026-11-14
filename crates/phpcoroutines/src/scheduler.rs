//! `Swoole\Coroutine\Scheduler`, the coroutine options (`Co::set()`,
//! `Co::getOptions()`, `php_swoole_set_coroutine_option`) and the reactor's
//! user exit condition: what `swoole_coroutine_scheduler.cc` held.
use crate::{host, runtime};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    mem::ManuallyDrop,
    rc::Rc,
};
use swoole_rs_corebase::CCast;
use swoole_rs_phpstubs::coroutine_scheduler;
use swoole_rs_zend::host::callable as kept;
use swoole_rs_zend::{
    Call, Callable, Custom, CustomClass, E_ERROR, E_WARNING, Minit, Owned, OwnedArray, Parsed,
    Value,
    bailout::{self, Bailout},
    kind, message,
    sys::{IS_FALSE, IS_NULL, ZEND_ACC_FINAL},
};
use swoole_rs_zend::{method_table, sys as z};

per_thread! {
    /// `PHPCoroutine::options`: what `Co::set()` was given. RSHUTDOWN frees
    /// them; a thread's locals drop after Zend's request memory is gone, so
    /// one that outlives it (a shutdown that bailed out) leaks with it.
    static OPTIONS: RefCell<Option<ManuallyDrop<OwnedArray>>> = RefCell::new(None);
    /// `exit_condition_fci_cache`: `Co::set()`'s `exit_condition` callable.
    static EXIT_CONDITION: RefCell<Option<Rc<kept::Callable>>> = RefCell::new(None);
    /// The references crates/corebase's PHP name resolvers hold (crates/phpclients),
    /// one per entry, until a remove or RSHUTDOWN unlinks it.
    static NAME_RESOLVERS: RefCell<Vec<Owned>> = RefCell::new(Vec::new());
}

/// `f` on this thread's kept PHP name resolvers. `f` runs no PHP code, which
/// could register or remove one.
pub fn name_resolvers<R>(f: impl FnOnce(&mut Vec<Owned>) -> R) -> R {
    NAME_RESOLVERS.with(|kept| f(&mut kept.borrow_mut()))
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

/// MINIT: `Swoole\Coroutine\Scheduler` (`Co\Scheduler`), final, not
/// serializable, clone and unset denied.
/// # Errors
/// A registration bailed out.
pub fn php_swoole_coroutine_scheduler_minit(minit: &Minit) -> Result<(), Bailout> {
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
    if !host::reactor_init() {
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
            if runtime::create(call.php(), fcc, argc, argv, name).is_err() {
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
        set_hook_flags(flags.c_cast::<u32>());
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
    // zend_hash_merge(current, options, zval_add_ref, overwrite)
    let mut entries = options.entries();
    while let Some((name, index, value)) = entries.next_stored() {
        with_options(|current| {
            let old = current.array().and_then(|array| match name {
                Some(name) => array.find(name.bytes()),
                None => array.index(index),
            });
            let old = old.map(Value::copy);
            match name {
                Some(name) => current.update(name, value.copy()),
                None => current.update_index(index, value.copy()),
            }
            old
        });
    }
}

/// Edit this thread's options, an empty array until the first edit. The edit
/// returns a copy of the value it overwrites, released once the options are
/// back in the cell: a destructor that runs then and calls `Co::set()` edits
/// them as they now are, as upstream's does.
fn with_options(edit: impl FnOnce(&mut OwnedArray) -> Option<Owned>) {
    let mut options = OPTIONS
        .with(RefCell::take)
        .map_or_else(OwnedArray::default, ManuallyDrop::into_inner);
    let old = edit(&mut options);
    OPTIONS.with(|o| o.replace(Some(ManuallyDrop::new(options))));
    drop(old);
}

/// `PHPCoroutine::set_hook_flags`: the flags, recorded in the options too.
pub fn set_hook_flags(flags: u32) {
    with_options(|options| {
        let old = options.array().and_then(|array| array.find(b"hook_flags"));
        let old = old.map(Value::copy);
        options.insert_long(b"hook_flags", i64::from(flags));
        old
    });
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
        bailout::raise(set_exit_condition(condition));
    }
}

/// `php_swoole_coroutine_set_exit_condition`: keep `condition` if it is
/// callable (warning otherwise) and install or clear the reactor's user exit
/// condition to match.
fn set_exit_condition(condition: &Value) -> Result<(), Bailout> {
    release_exit_condition()?;
    let handle = kept::Callable::new(condition)?;
    let installed = handle.is_some();
    drop(EXIT_CONDITION.with(|c| c.replace(handle)));
    host::set_exit_condition(installed);
    Ok(())
}

/// Drop the exit condition, deferring its destructor's bailout.
fn release_exit_condition() -> Result<(), Bailout> {
    drop(EXIT_CONDITION.with(RefCell::take));
    bailout::outcome(())
}

/// Install `Co::set`'s exit condition, if it gave one, on this thread's
/// reactor, as `php_swoole_reactor_init` did for a new reactor.
pub fn install_exit_condition() {
    if EXIT_CONDITION.with(|c| c.borrow().is_some()) {
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
    // Its own reference: the condition may replace itself (`Co::set`).
    let Some(handle) = EXIT_CONDITION.with(|c| c.borrow().clone()) else {
        return Ok(true);
    };
    let mut retval = swoole_rs_zend::UNDEF;
    retval.u1.type_info = IS_NULL;
    // The result slot is, as upstream's, not released.
    if !handle.call(&mut [], Some(Value::of(&mut retval)), false)? {
        host::try_fatal_error(E_ERROR, c"Coroutine can_exit callback handler error")?;
    }
    swoole_rs_zend::host::uncaught_exception()?;
    // The last reference once the condition replaced itself: a bailout its
    // destructor deferred is raised by the reactor's callback.
    drop(handle);
    bailout::outcome(kind(&retval) != IS_FALSE)
}

/// RSHUTDOWN: drop the PHP name resolvers and the exit condition.
pub extern "C" fn php_swoole_coroutine_scheduler_rshutdown() {
    bailout::raise(rshutdown());
}

fn rshutdown() -> Result<(), Bailout> {
    let Some(families) = swoole_rs_phpswoole::families() else {
        return Ok(());
    };
    // Every PHP resolver matches `None`; each match is unlinked and its
    // reference returned (crates/phpclients), released here inside `zend_try`.
    while let Some(resolver) = (families.name_resolver_remove)(None) {
        drop(resolver);
        bailout::outcome(())?;
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
