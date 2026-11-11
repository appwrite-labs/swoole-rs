//! PHP `Swoole\Timer` (with the `php` feature, see docs/zend.md): class and
//! function registration, constants and every method, over crates/timer's PHP
//! timers. A timer's callback and destructor run the callable here and raise a
//! bailout once the timer's selection is abandoned.
#![cfg(feature = "php")]
#![forbid(unsafe_code)]

use std::ffi::{CStr, c_int};
use swoole_rs_corebase::{log::set_last_error, thread_reactor};
use swoole_rs_phpstubs::timer;
use swoole_rs_timer::php;
use swoole_rs_zend::{
    Call, Callable, Class, E_WARNING, Holder, Minit, OwnedArray,
    bailout::{self, Bailout},
    report,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_TIMER_MIN_MS`, `SW_TIMER_MAX_MS` (`LONG_MAX`) and their seconds.
const MIN_MS: i64 = 1;
const MAX_MS: i64 = i64::MAX;
/// `SW_ERROR_PHP_FATAL_ERROR` and `SW_ERROR_PHP_RUNTIME_NOTICE`.
const PHP_FATAL_ERROR: c_int = 503;
const PHP_RUNTIME_NOTICE: c_int = 510;

static TIMER: Class = Class::new();
static ITERATOR: Class = Class::new();

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

swoole_rs_zend::minit!(minit => register);

/// MINIT: `Swoole\Timer` (not instantiable) and the `swoole_timer_*` functions,
/// `Swoole\Timer\Iterator` (an `ArrayIterator`) and the `SWOOLE_TIMER_*` constants.
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register_static(&TIMER, c"Swoole\\Timer", &METHODS.0);
    minit.function_aliases(&TIMER, &METHODS.0, &ALIASES)?;
    let parent = minit.array_iterator();
    minit.register_child(
        &ITERATOR,
        c"Swoole\\Timer\\Iterator",
        &[],
        parent,
        |_, _| {},
    );
    minit.long_constant(c"SWOOLE_TIMER_MIN_MS", MIN_MS);
    minit.double_constant(c"SWOOLE_TIMER_MIN_SEC", 0.001);
    minit.long_constant(c"SWOOLE_TIMER_MAX_MS", MAX_MS);
    #[expect(
        clippy::cast_precision_loss,
        reason = "upstream's ((double) LONG_MAX / 1000)"
    )]
    let max_sec = MAX_MS as f64 / 1000.0;
    minit.double_constant(c"SWOOLE_TIMER_MAX_SEC", max_sec);
    Ok(())
}

/// `php_swoole_fatal_error(E_WARNING, "%s", message)`.
fn fatal_warning(message: &CStr) {
    set_last_error(PHP_FATAL_ERROR);
    bailout::defer(report(E_WARNING, message));
}

/// `php_swoole_error(E_WARNING, "%s", message)`.
fn warning(message: &CStr) -> Result<(), Bailout> {
    set_last_error(PHP_RUNTIME_NOTICE);
    if !swoole_rs_phpswoole::display_errors() {
        return Ok(());
    }
    report(E_WARNING, message)
}

/// A PHP timer's callable, which the timer holds as its data.
struct Timer;

impl Holder for Timer {
    /// The timer's callback: `zend::Function::call(nullptr,
    /// php_swoole_is_enable_coroutine())`, warning when it fails. A bailout
    /// abandons the timer's selection, keeping shutdown-visible metadata until
    /// native owner teardown, and is raised from here, which owns nothing.
    fn run(callable: &Callable) {
        let called =
            swoole_rs_phpcoroutines::call_kept(callable, swoole_rs_phpprocess::enable_coroutine())
                .and_then(|called| {
                    if called {
                        Ok(())
                    } else {
                        warning(c"Swoole\\Timer->onTimeout handler error")
                    }
                });
        if called.is_err() {
            php::abandon(None);
        }
        bailout::raise(called);
    }

    /// The timer's destructor: release and free the callable. A bailout is
    /// raised once the timer has deleted the node, or at once without a timer.
    fn free(callable: Callable) {
        if let Err(error) = callable.release() {
            error.defer();
            if php::available() {
                // Return through the timer's release so the node is deleted first.
                php::abandon(Some(finish));
            } else {
                bailout::finish();
            }
        }
    }
}

/// A destructor's deferred bailout, raised by the timer after the node is gone.
extern "C-unwind" fn finish() {
    bailout::finish();
}

/// `after()` and `tick()`: `(int $ms, callable $callback, mixed ...$params)`.
fn add(call: &Call, persistent: bool) {
    let parsed = call
        .arity(2, u32::MAX)
        .and_then(|()| call.long(1, 0))
        .and_then(|ms| Ok((ms, Callable::parse(call, 2)?)));
    let Ok((ms, callable)) = parsed else {
        call.set_bool(false);
        return;
    };
    if ms < MIN_MS {
        fatal_warning(c"Timer must be greater than or equal to 1");
        callable.discard();
        call.set_bool(false);
        return;
    }
    if thread_reactor().is_null() && swoole_rs_phpprocess::timer_uses_reactor() {
        swoole_rs_phpswoole::check_reactor();
    }
    // The timer keeps the callable as its data until `free`.
    let (run, free) = Callable::callbacks::<Timer>();
    let Some(id) = php::add_php(ms, persistent, run, free, callable.as_raw()) else {
        fatal_warning(c"add timer failed");
        callable.discard();
        call.set_bool(false);
        return;
    };
    // The timer owns it now; its destructor frees it.
    let _owned = callable.keep(persistent.then_some(id)).into_raw();
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
    if !php::available() {
        call.set_bool(false);
        return;
    }
    if let Some(id) = id(call) {
        call.set_bool(php::info(id).is_some_and(|(node, _)| !node.removed));
    }
}

#[expect(clippy::cast_possible_wrap, reason = "add_assoc_long of a uint64_t")]
fn info(call: &Call) {
    if !php::available() {
        call.set_bool(false);
        return;
    }
    let Some(id) = id(call) else { return };
    let Some((node, _)) = php::info(id) else {
        call.set_null();
        return;
    };
    call.set_array();
    call.add_assoc_long(c"exec_msec", node.exec_msec);
    call.add_assoc_long(c"exec_count", node.exec_count as i64);
    call.add_assoc_long(c"interval", node.interval);
    call.add_assoc_long(c"round", node.round as i64);
    call.add_assoc_bool(c"removed", node.removed);
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "add_assoc_long of a size_t and a uint64_t"
)]
fn stats(call: &Call) {
    call.set_array();
    let stats = php::stats();
    let (count, round) = stats.unwrap_or_default();
    call.add_assoc_bool(c"initialized", stats.is_some());
    call.add_assoc_long(c"num", count as i64);
    call.add_assoc_long(c"round", round as i64);
}

/// The PHP timers' ids, in the timer's order.
fn php_ids() -> Vec<i64> {
    let mut ids = php::ids();
    ids.retain(|&id| php::info(id).is_some_and(|(_, php)| php));
    ids
}

/// `list(): Swoole\Timer\Iterator` over the PHP timers' ids.
fn list(call: &Call) {
    let mut ids = OwnedArray::default();
    for id in php_ids() {
        ids.push_long(id);
    }
    match ITERATOR.instantiate(&[&ids]) {
        Ok(iterator) => call.set_owned(iterator),
        Err(bailed) => bailed.defer(),
    }
}

fn clear(call: &Call) {
    if !php::available() {
        call.set_bool(false);
        return;
    }
    let Some(id) = id(call) else { return };
    call.set_bool(php::clear(id));
}

fn clear_all(call: &Call) {
    call.set_bool(clear_all_timers());
}

/// `php_swoole_timer_clear_all()`: remove every PHP timer, newest first; false
/// if there is no timer. A destructor can clear other timers, so each id is
/// looked up again. The ids sit on the system heap, which a request at its
/// memory limit still has; a destructor's bailout leaks them, as upstream's vector.
#[must_use]
pub fn clear_all_timers() -> bool {
    if !php::available() {
        return false;
    }
    let ids = std::mem::ManuallyDrop::new(php::ids());
    for &id in ids.iter().rev() {
        let _cleared = php::clear(id);
    }
    drop(std::mem::ManuallyDrop::into_inner(ids));
    true
}

/// RSHUTDOWN: remove every PHP timer.
pub extern "C" fn rshutdown() {
    let _cleared = clear_all_timers();
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
