// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/timer.cc (Swoole contributors).
//! The `swoole_timer_*` C API (upstream `src/wrapper/timer.cc`, which the timer
//! patch deletes) under its upstream symbols: this thread's `SwooleTG.timer`, its
//! lazy creation, unit conversion and warnings, over the Rust-owned timer
//! (`owner.rs`).
use crate::owner::{Owner as Timer, TimerNode, swr_v1_timer_owner_free, uncaught};
use crate::swr_v1_timer_token;
use std::ffi::{CStr, c_long, c_void};
use swoole_rs_corebase::{log, swoole_print_backtrace};

/// `SW_ERR` and `SW_TIMER_MIN_SEC`.
const ERR: c_long = -1;
const MIN_SEC: f64 = 0.001;

/// `const swoole::TimerCallback &`, passed through untouched.
type Callback = *const c_void;

// SAFETY: native/corebase/globals.cc's.
#[expect(
    improper_ctypes,
    reason = "the slot holds an address C++ never dereferences"
)]
unsafe extern "C" {
    fn swr_v1_core_timer_slot() -> *mut *mut Timer;
}

/// `SwooleTG.timer`.
fn slot() -> *mut *mut Timer {
    // SAFETY: the address of this thread's slot.
    unsafe { swr_v1_core_timer_slot() }
}

fn timer() -> *mut Timer {
    // SAFETY: this thread's live slot.
    unsafe { *slot() }
}

/// This thread's timer, or `origin`'s "timer is not available" warning.
fn available(origin: &CStr) -> Option<*mut Timer> {
    let timer = timer();
    if timer.is_null() {
        log::warning(origin, format_args!("timer is not available"));
        return None;
    }
    Some(timer)
}

/// `bool swoole_timer_is_available()`.
#[unsafe(export_name = "_Z25swoole_timer_is_availablev")]
extern "C" fn swoole_timer_is_available() -> bool {
    !timer().is_null()
}

/// `Timer *swoole_timer_create(bool manually_trigger)`: a new timer, published
/// as this thread's.
#[unsafe(export_name = "_Z19swoole_timer_createb")]
extern "C-unwind" fn swoole_timer_create(manually_trigger: bool) -> *mut c_void {
    let timer = Timer::create(manually_trigger);
    // SAFETY: this thread's live slot.
    unsafe { *slot() = timer };
    timer.cast()
}

/// `int64_t swoole_timer_get_next_msec()`: -1 without a timer.
#[unsafe(export_name = "_Z26swoole_timer_get_next_msecv")]
extern "C" fn swoole_timer_get_next_msec() -> i64 {
    let timer = timer();
    if timer.is_null() {
        return -1;
    }
    // SAFETY: this thread's live timer.
    unsafe { &*timer }.next_msec()
}

/// `TimerNode *swoole_timer_add(double timeout, ...)`: seconds, at least
/// `SW_TIMER_MIN_SEC`.
/// # Safety
/// `callback` is a live `TimerCallback`.
#[cfg_attr(
    target_os = "macos",
    unsafe(
        export_name = "_Z16swoole_timer_adddbRKNSt3__18functionIFvPN6swoole5TimerEPNS1_9TimerNodeEEEEPv"
    )
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(
        export_name = "_Z16swoole_timer_adddbRKSt8functionIFvPN6swoole5TimerEPNS0_9TimerNodeEEEPv"
    )
)]
unsafe extern "C-unwind" fn swoole_timer_add_sec(
    timeout: f64,
    persistent: bool,
    callback: Callback,
    data: *mut c_void,
) -> *mut c_void {
    let timeout = if timeout < MIN_SEC { MIN_SEC } else { timeout };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sec2msec's static_cast<long>"
    )]
    let msec = (timeout * 1000.0) as c_long;
    // SAFETY: as the caller promises.
    unsafe { swoole_timer_add(msec, persistent, callback, data) }
}

/// `TimerNode *swoole_timer_add(long ms, ...)`: creates this thread's timer on
/// first use.
/// # Safety
/// `callback` is a live `TimerCallback`.
#[cfg_attr(
    target_os = "macos",
    unsafe(
        export_name = "_Z16swoole_timer_addlbRKNSt3__18functionIFvPN6swoole5TimerEPNS1_9TimerNodeEEEEPv"
    )
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(
        export_name = "_Z16swoole_timer_addlbRKSt8functionIFvPN6swoole5TimerEPNS0_9TimerNodeEEEPv"
    )
)]
unsafe extern "C-unwind" fn swoole_timer_add(
    ms: c_long,
    persistent: bool,
    callback: Callback,
    data: *mut c_void,
) -> *mut c_void {
    if timer().is_null() {
        swoole_timer_create(false);
    }
    // SAFETY: this thread's live timer.
    unsafe { &*timer() }
        .add(ms, persistent, data, callback)
        .cast()
}

/// `swoole_timer_after`/`swoole_timer_tick`: the new timer's id, or `SW_ERR`.
///
/// # Safety
/// `callback` is a live `TimerCallback`.
unsafe fn schedule(
    origin: &CStr,
    ms: c_long,
    persistent: bool,
    callback: Callback,
    data: *mut c_void,
) -> c_long {
    if ms <= 0 {
        log::warning(origin, format_args!("Timer must be greater than 0"));
        return ERR;
    }
    // SAFETY: as the caller promises.
    let node = unsafe { swoole_timer_add(ms, persistent, callback, data) }.cast::<TimerNode>();
    if node.is_null() {
        return ERR;
    }
    // SAFETY: a live node the timer just added.
    unsafe { (*node).id }
}

/// `long swoole_timer_after(long ms, const TimerCallback &, void *)`.
/// # Safety
/// `callback` is a live `TimerCallback`.
#[cfg_attr(
    target_os = "macos",
    unsafe(
        export_name = "_Z18swoole_timer_afterlRKNSt3__18functionIFvPN6swoole5TimerEPNS1_9TimerNodeEEEEPv"
    )
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(
        export_name = "_Z18swoole_timer_afterlRKSt8functionIFvPN6swoole5TimerEPNS0_9TimerNodeEEEPv"
    )
)]
unsafe extern "C-unwind" fn swoole_timer_after(
    ms: c_long,
    callback: Callback,
    data: *mut c_void,
) -> c_long {
    // SAFETY: as the caller promises.
    unsafe { schedule(c"swoole_timer_after", ms, false, callback, data) }
}

/// `long swoole_timer_tick(long ms, const TimerCallback &, void *)`.
/// # Safety
/// `callback` is a live `TimerCallback`.
#[cfg_attr(
    target_os = "macos",
    unsafe(
        export_name = "_Z17swoole_timer_ticklRKNSt3__18functionIFvPN6swoole5TimerEPNS1_9TimerNodeEEEEPv"
    )
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(
        export_name = "_Z17swoole_timer_ticklRKSt8functionIFvPN6swoole5TimerEPNS0_9TimerNodeEEEPv"
    )
)]
unsafe extern "C-unwind" fn swoole_timer_tick(
    ms: c_long,
    callback: Callback,
    data: *mut c_void,
) -> c_long {
    // SAFETY: as the caller promises.
    unsafe { schedule(c"swoole_timer_tick", ms, true, callback, data) }
}

/// `bool swoole_timer_del(TimerNode *)`: `Timer::remove(tnode)`.
/// # Safety
/// `node` is null or one of this thread's live nodes.
#[unsafe(export_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE")]
unsafe extern "C-unwind" fn swoole_timer_del(node: *mut c_void) -> bool {
    let Some(timer) = available(c"swoole_timer_del") else {
        return false;
    };
    let node = node.cast::<TimerNode>();
    // SAFETY: null or a live node, as the caller promises.
    let Some(node) = (unsafe { node.as_ref() }) else {
        return false;
    };
    // SAFETY: this thread's live timer.
    let (removed, failure) = unsafe { &*timer }.remove(node.id);
    uncaught(failure);
    removed
}

/// `void swoole_timer_delay(TimerNode *, long delay_ms)`: `Timer::delay`, which
/// moves the deadline to `delay_ms` from now.
/// # Safety
/// `node` is null or one of this thread's live nodes.
#[unsafe(export_name = "_Z18swoole_timer_delayPN6swoole9TimerNodeEl")]
unsafe extern "C-unwind" fn swoole_timer_delay(node: *mut c_void, delay_ms: c_long) {
    let Some(timer) = available(c"swoole_timer_delay") else {
        return;
    };
    let node = node.cast::<TimerNode>();
    // SAFETY: null or a live node, as the caller promises.
    let Some(node) = (unsafe { node.as_ref() }) else {
        return;
    };
    // SAFETY: this thread's live timer.
    unsafe { &*timer }.delay(node, delay_ms);
}

/// `Timer::get(id)`.
fn get(timer: *mut Timer, id: c_long) -> *mut TimerNode {
    // SAFETY: this thread's live timer.
    let queue = unsafe { &*timer }.queue();
    // SAFETY: its live queue.
    unsafe { swr_v1_timer_token(queue, id) }.cast()
}

/// `bool swoole_timer_exists(long timer_id)`: a node that isn't removed.
#[unsafe(export_name = "_Z19swoole_timer_existsl")]
extern "C" fn swoole_timer_exists(timer_id: c_long) -> bool {
    let Some(timer) = available(c"swoole_timer_exists") else {
        return false;
    };
    // SAFETY: null or a live node.
    unsafe { get(timer, timer_id).as_ref() }.is_some_and(|node| !node.removed)
}

/// `bool swoole_timer_clear(long timer_id)`: `Timer::remove(id)`.
#[unsafe(export_name = "_Z18swoole_timer_clearl")]
extern "C-unwind" fn swoole_timer_clear(timer_id: c_long) -> bool {
    let Some(timer) = available(c"swoole_timer_clear") else {
        return false;
    };
    // SAFETY: this thread's live timer.
    let (removed, failure) = unsafe { &*timer }.remove(timer_id);
    uncaught(failure);
    removed
}

/// `TimerNode *swoole_timer_get(long timer_id)`.
#[unsafe(export_name = "_Z16swoole_timer_getl")]
extern "C" fn swoole_timer_get(timer_id: c_long) -> *mut c_void {
    available(c"swoole_timer_get").map_or(std::ptr::null_mut(), |timer| get(timer, timer_id).cast())
}

/// `void swoole_timer_free()`: `delete SwooleTG.timer`, then clear the slot.
#[unsafe(export_name = "_Z17swoole_timer_freev")]
extern "C-unwind" fn swoole_timer_free() {
    let timer = timer();
    if timer.is_null() {
        swoole_print_backtrace();
        log::warning(c"swoole_timer_free", format_args!("timer is not available"));
        return;
    }
    // SAFETY: the timer swoole_timer_create made, freed once.
    unsafe { swr_v1_timer_owner_free(timer) };
    // SAFETY: this thread's live slot.
    unsafe { *slot() = std::ptr::null_mut() };
}

/// `void swoole_timer_select()`: runs due timers, if there is a timer.
#[unsafe(export_name = "_Z19swoole_timer_selectv")]
extern "C-unwind" fn swoole_timer_select() {
    let timer = timer();
    if !timer.is_null() {
        // SAFETY: this thread's live timer.
        uncaught(unsafe { &*timer }.select());
    }
}
