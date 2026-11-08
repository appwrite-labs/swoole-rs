//! PHP `Swoole\Timer` (with the `php` feature, see docs/zend.md): class and
//! function registration, constants and every method. The timer and its loop
//! stay C++ (`swoole::Timer`), which lends its operations as a table; the PHP
//! callback runs from a C++ trampoline that re-raises a trapped bailout.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::OnceLock,
};
use swoole_rs_phpstubs::timer;
use swoole_rs_zend::{
    Call, Callable, Class, register_double_constant, register_long_constant,
    sys::{
        add_next_index_long, object_init_ex, swr_zend_array_init, zend_call_method,
        zend_class_entry, zval, zval_ptr_dtor,
    },
};
use swoole_rs_zend::{method_table, sys as z};

/// `swoole::Timer`'s operations (`swr_v1_phptimer_ops`).
#[repr(C)]
pub struct Ops {
    /// `swoole_timer_is_available()`.
    available: unsafe extern "C" fn() -> u8,
    /// Start the reactor if this process uses one, then `swoole_timer_add` a
    /// PHP timer running the C++ trampoline over `callable`: its id, 0 if none.
    add: unsafe extern "C" fn(ms: i64, persistent: u8, callable: *mut c_void) -> i64,
    /// `swoole_timer_get(id)`, copied into `node`; 0 if there is none.
    get: unsafe extern "C" fn(id: i64, node: *mut Node) -> u8,
    /// The timer's `count()` and `get_round()`; the timer is available.
    stats: unsafe extern "C" fn(count: *mut usize, round: *mut u64),
    /// Up to `capacity` timer ids into `ids`: how many it copied.
    ids: unsafe extern "C" fn(ids: *mut i64, capacity: usize) -> usize,
    /// `swoole_timer_clear`'s body after its checks: `swoole_timer_del` of the
    /// timer `id` if it is a PHP timer, 0 if not or there is none.
    clear: unsafe extern "C" fn(id: i64) -> u8,
    /// `zend::Function::call(nullptr, php_swoole_is_enable_coroutine())` of an
    /// `Invocation`'s callable, a body for `swr_zend_try`.
    call: unsafe extern "C" fn(invocation: *mut c_void),
    /// `php_swoole_fatal_error(E_WARNING, "%s", message)`.
    fatal_warning: unsafe extern "C" fn(message: *const c_char),
    /// `php_swoole_error(E_WARNING, "%s", message)`.
    warning: unsafe extern "C" fn(message: *const c_char),
}

/// A `TimerNode`'s fields (`swr_v1_phptimer_node`).
#[repr(C)]
pub struct Node {
    exec_msec: i64,
    exec_count: u64,
    interval: i64,
    round: u64,
    /// `type == TimerNode::TYPE_PHP`.
    php: u8,
    removed: u8,
}

/// One call of a timer's callable (`swr_v1_phptimer_invocation`).
#[repr(C)]
pub struct Invocation {
    callable: *mut c_void,
    ok: u8,
}

/// `SW_TIMER_MIN_MS`, `SW_TIMER_MAX_MS` (`LONG_MAX`) and their seconds.
const MIN_MS: i64 = 1;
const MAX_MS: i64 = i64::MAX;

static OPS: OnceLock<&'static Ops> = OnceLock::new();
static TIMER: Class = Class::new();
static ITERATOR: Class = Class::new();

fn ops() -> &'static Ops {
    // Functions run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

/// The global functions `SW_FUNCTION_ALIAS` registers, in upstream's order.
const ALIASES: [(&CStr, &CStr); 8] = [
    (c"after", c"swoole_timer_after"),
    (c"tick", c"swoole_timer_tick"),
    (c"info", c"swoole_timer_info"),
    (c"list", c"swoole_timer_list"),
    (c"exists", c"swoole_timer_exists"),
    (c"stats", c"swoole_timer_stats"),
    (c"clear", c"swoole_timer_clear"),
    (c"clearAll", c"swoole_timer_clear_all"),
];

method_table! {
    static METHODS = [
        c"tick" => zif_swoole_timer_tick, &timer::arginfo_swoole_timer_tick, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"after" => zif_swoole_timer_after, &timer::arginfo_swoole_timer_after, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"exists" => zif_swoole_timer_exists, &timer::arginfo_swoole_timer_exists, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"info" => zif_swoole_timer_info, &timer::arginfo_swoole_timer_info, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"stats" => zif_swoole_timer_stats, &timer::arginfo_swoole_timer_stats, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"list" => zif_swoole_timer_list, &timer::arginfo_swoole_timer_list, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"clear" => zif_swoole_timer_clear, &timer::arginfo_swoole_timer_clear, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"clearAll" => zif_swoole_timer_clear_all, &timer::arginfo_swoole_timer_clear_all, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

/// MINIT: `Swoole\Timer` (not instantiable), `Swoole\Timer\Iterator` (an
/// `ArrayIterator`), the `swoole_timer_*` functions and the `SWOOLE_TIMER_*`
/// constants, as `php_swoole_timer_minit` registered them.
/// # Safety
/// Called once, at MINIT, with SPL's `ArrayIterator` and a static operations table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phptimer_minit(
    module_number: i32,
    array_iterator: *mut zend_class_entry,
    ops: *const Ops,
) {
    // SAFETY: the native MINIT entry lends these original startup arguments.
    let result = unsafe { init_phptimer_minit(module_number, array_iterator, ops) };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            swoole_rs_zend::bailout::finish();
        }
    }
}

unsafe fn init_phptimer_minit(
    module_number: i32,
    array_iterator: *mut zend_class_entry,
    ops: *const Ops,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    // SAFETY: MINIT, with a static table.
    unsafe { TIMER.register_static(c"Swoole\\Timer", METHODS.as_ptr()) };
    // SAFETY: MINIT; SPL registers ArrayIterator first.
    unsafe {
        ITERATOR.register_child(
            c"Swoole\\Timer\\Iterator",
            ptr::null(),
            array_iterator,
            |_, _| {},
        );
    }
    // SAFETY: MINIT, after registration, with the class's own table.
    unsafe { TIMER.function_aliases(METHODS.as_ptr(), &ALIASES) }?;
    // SAFETY: MINIT, with the module's number.
    unsafe { register_long_constant(c"SWOOLE_TIMER_MIN_MS", MIN_MS, module_number) };
    // SAFETY: as above.
    unsafe { register_double_constant(c"SWOOLE_TIMER_MIN_SEC", 0.001, module_number) };
    // SAFETY: as above.
    unsafe { register_long_constant(c"SWOOLE_TIMER_MAX_MS", MAX_MS, module_number) };
    #[expect(
        clippy::cast_precision_loss,
        reason = "upstream's ((double) LONG_MAX / 1000)"
    )]
    let max_sec = MAX_MS as f64 / 1000.0;
    // SAFETY: as above.
    unsafe { register_double_constant(c"SWOOLE_TIMER_MAX_SEC", max_sec, module_number) };

    Ok(())
}

/// The C++ trampoline's body: run the callable; 1 if it bailed out, which the
/// trampoline re-raises once this frame is gone.
/// # Safety
/// `callable` is a live timer's data, from `add`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phptimer_call(callable: *mut c_void) -> u8 {
    let ops = ops();
    let mut invocation = Invocation { callable, ok: 0 };
    // SAFETY: the C++ call body, with an invocation it may write.
    let called = unsafe { swoole_rs_zend::bailout::trap(ops.call, (&raw mut invocation).cast()) };
    if called.is_err() {
        return 1;
    }
    if invocation.ok == 0 {
        // SAFETY: C++'s reporter, from the trampoline's frame.
        let reported = unsafe {
            swoole_rs_zend::bailout::report(ops.warning, c"Swoole\\Timer->onTimeout handler error")
        };
        return u8::from(reported.is_err());
    }
    0
}

/// The C++ destructor trampoline's body: release and free the callable; 1 if a
/// destructor bailed out.
/// # Safety
/// `callable` is a removed timer's data, from `add`, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phptimer_free(callable: *mut c_void) -> u8 {
    // SAFETY: per the caller.
    let callable = unsafe { Callable::from_raw(callable) };
    u8::from(callable.release().is_err())
}

fn available() -> bool {
    // SAFETY: C++'s check.
    unsafe { (ops().available)() != 0 }
}

fn node(id: i64) -> Option<Node> {
    let mut node = Node {
        exec_msec: 0,
        exec_count: 0,
        interval: 0,
        round: 0,
        php: 0,
        removed: 0,
    };
    // SAFETY: a writable node.
    (unsafe { (ops().get)(id, &raw mut node) } != 0).then_some(node)
}

fn fatal_warning(message: &CStr) {
    // SAFETY: C++'s reporter, from a handler.
    swoole_rs_zend::bailout::defer(unsafe {
        swoole_rs_zend::bailout::report(ops().fatal_warning, message)
    });
}

/// `after()` and `tick()`: `(int $ms, callable $callback, mixed ...$params)`.
fn add(call: &Call, persistent: bool) {
    let parsed = call
        .arity(2, u32::MAX)
        .and_then(|()| call.long(1, 0))
        .and_then(|ms| Ok((ms, Callable::parse(call, 2)?)));
    let Ok((ms, mut callable)) = parsed else {
        call.set_bool(false);
        return;
    };
    if ms < MIN_MS {
        fatal_warning(c"Timer must be greater than or equal to 1");
        callable.discard();
        call.set_bool(false);
        return;
    }
    // SAFETY: C++ keeps the callable as the timer's data until its destructor.
    let id = unsafe { (ops().add)(ms, u8::from(persistent), callable.as_raw()) };
    if id == 0 {
        fatal_warning(c"add timer failed");
        callable.discard();
        call.set_bool(false);
        return;
    }
    callable.keep(persistent.then_some(id));
    // The timer owns it now; its destructor frees it.
    let _owned = callable.into_raw();
    call.set_long(id);
}

fn after(call: &Call) {
    add(call, false);
}

fn tick(call: &Call) {
    add(call, true);
}

/// A timer id argument (`ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)`).
fn id(call: &Call) -> Option<i64> {
    let id = call.arity(1, 1).and_then(|()| call.long(1, 0));
    if id.is_err() {
        call.set_bool(false);
    }
    id.ok()
}

fn exists(call: &Call) {
    if !available() {
        call.set_bool(false);
        return;
    }
    if let Some(id) = id(call) {
        call.set_bool(node(id).is_some_and(|node| node.removed == 0));
    }
}

#[expect(clippy::cast_possible_wrap, reason = "add_assoc_long of a uint64_t")]
fn info(call: &Call) {
    if !available() {
        call.set_bool(false);
        return;
    }
    let Some(id) = id(call) else { return };
    let Some(node) = node(id) else {
        call.set_null();
        return;
    };
    call.set_array();
    call.add_assoc_long(c"exec_msec", node.exec_msec);
    call.add_assoc_long(c"exec_count", node.exec_count as i64);
    call.add_assoc_long(c"interval", node.interval);
    call.add_assoc_long(c"round", node.round as i64);
    call.add_assoc_bool(c"removed", node.removed != 0);
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "add_assoc_long of a size_t and a uint64_t"
)]
fn stats(call: &Call) {
    call.set_array();
    let (mut count, mut round) = (0, 0);
    let initialized = available();
    if initialized {
        // SAFETY: the timer is available; writable outputs.
        unsafe { (ops().stats)(&raw mut count, &raw mut round) };
    }
    call.add_assoc_bool(c"initialized", initialized);
    call.add_assoc_long(c"num", count as i64);
    call.add_assoc_long(c"round", round as i64);
}

/// Every timer's id, oldest first; the timer is available.
fn ids() -> Vec<i64> {
    let ops = ops();
    let (mut count, mut round) = (0, 0);
    // SAFETY: the timer is available; writable outputs.
    unsafe { (ops.stats)(&raw mut count, &raw mut round) };
    let mut ids = vec![0; count];
    // SAFETY: `ids` has room for `count` ids.
    let copied = unsafe { (ops.ids)(ids.as_mut_ptr(), count) };
    ids.truncate(copied);
    ids
}

/// The PHP timers' ids, oldest first.
fn php_ids() -> Vec<i64> {
    if !available() {
        return Vec::new();
    }
    let mut ids = ids();
    ids.retain(|&id| node(id).is_some_and(|node| node.php != 0));
    ids
}

/// `list(): Swoole\Timer\Iterator` over the PHP timers' ids.
fn list(call: &Call) {
    // SAFETY: all-zero bytes are a valid (undefined) zval.
    let mut ids: zval = unsafe { std::mem::zeroed() };
    // SAFETY: a writable zval.
    unsafe { swr_zend_array_init(&raw mut ids) };
    for id in php_ids() {
        // SAFETY: an array.
        unsafe { add_next_index_long(&raw mut ids, id) };
    }
    let ce = ITERATOR.entry();
    let value = call.return_value();
    // SAFETY: the return slot and a registered class.
    unsafe { object_init_ex(value, ce) };
    // SAFETY: the return slot is live.
    let made = unsafe { *value };
    // SAFETY: `object_init_ex` made an object.
    let object = unsafe { made.value.obj };
    // SAFETY: a registered class; its constructor slot lives as long.
    let constructor = unsafe { &raw mut (*ce).constructor };
    // SAFETY: ArrayIterator's constructor, called as upstream does, with
    // the id array; it only copies the array.
    unsafe {
        zend_call_method(
            object,
            ce,
            constructor,
            c"__construct".as_ptr(),
            c"__construct".count_bytes(),
            ptr::null_mut(),
            1,
            &raw mut ids,
            ptr::null_mut(),
        );
    }
    // SAFETY: our reference to the array.
    unsafe { zval_ptr_dtor(&raw mut ids) };
}

fn clear(call: &Call) {
    if !available() {
        call.set_bool(false);
        return;
    }
    let Some(id) = id(call) else { return };
    // SAFETY: C++'s lookup and removal.
    call.set_bool(unsafe { (ops().clear)(id) } != 0);
}

fn clear_all(call: &Call) {
    call.set_bool(swr_v1_phptimer_clear_all() != 0);
}

/// `php_swoole_timer_clear_all()`: remove every PHP timer, newest first; 0 if
/// there is no timer. A destructor can clear other timers, so each id is looked
/// up again. The ids sit on the system heap, which a request at its memory
/// limit still has; a destructor's bailout leaks them, as upstream's vector.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phptimer_clear_all() -> u8 {
    if !available() {
        return 0;
    }
    let ids = std::mem::ManuallyDrop::new(ids());
    for &id in ids.iter().rev() {
        // SAFETY: C++'s lookup and removal of a PHP timer.
        unsafe { (ops().clear)(id) };
    }
    drop(std::mem::ManuallyDrop::into_inner(ids));
    1
}

/// RSHUTDOWN: remove every PHP timer.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_timer_rshutdown() {
    swr_v1_phptimer_clear_all();
}

swoole_rs_zend::handlers! {
    zif_swoole_timer_after => after;
    zif_swoole_timer_tick => tick;
    zif_swoole_timer_exists => exists;
    zif_swoole_timer_info => info;
    zif_swoole_timer_stats => stats;
    zif_swoole_timer_list => list;
    zif_swoole_timer_clear => clear;
    zif_swoole_timer_clear_all => clear_all;
}
