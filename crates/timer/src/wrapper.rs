// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/timer.cc (Swoole contributors).
//! The `swoole_timer_*` C API (upstream `src/wrapper/timer.cc`, which the timer
//! patch deletes) under its upstream symbols: this thread's `SwooleTG.timer`, its
//! lazy creation, unit conversion and warnings. `swoole::Timer` stays a C++ object
//! (its `std::function` members and callback nodes); these call its out-of-line
//! members by symbol and read the fields its inline members read, at the offsets
//! src/core/timer.cc asserts.
use crate::{TimerView, swr_v1_timer_delay_deadline, swr_v1_timer_token, swr_v1_timer_update};
use std::ffi::{CStr, c_long, c_void};
use swoole_rs_corebase::{log, swoole_print_backtrace};

/// `SW_ERR`, `SW_TIMER_MIN_SEC` and `sizeof(swoole::Timer)`.
const ERR: c_long = -1;
const MIN_SEC: f64 = 0.001;
const TIMER_SIZE: usize = 112;

/// The leading members of `swoole::Timer` the inline members read.
#[repr(C)]
struct Timer {
    _reactor: *mut c_void,
    queue: *mut c_void,
    /// `owner_pid_`, `abandoned_selection_` and `pending_bailout_`.
    _selection: [usize; 2],
    base_time: i64,
    next_msec: i64,
}

/// The leading members of `swoole::TimerNode`.
#[repr(C)]
struct TimerNode {
    id: c_long,
    _type: i32,
    exec_msec: i64,
    /// `interval`, `exec_count` and `round`.
    _counts: [u64; 3],
    removed: bool,
}

/// `const swoole::TimerCallback &`, passed through untouched.
type Callback = *const c_void;

// SAFETY: native/corebase/globals.cc's, src/core/timer.cc's and the C++ runtime's.
unsafe extern "C-unwind" {
    fn swr_v1_core_timer_slot() -> *mut *mut Timer;
    #[link_name = "_ZN6swoole5TimerC1Eb"]
    fn timer_construct(this: *mut Timer, manually_trigger: bool);
    #[link_name = "_ZN6swoole5TimerD1Ev"]
    fn timer_destruct(this: *mut Timer);
    #[cfg_attr(
        target_os = "macos",
        link_name = "_ZN6swoole5Timer3addElbPvRKNSt3__18functionIFvPS0_PNS_9TimerNodeEEEE"
    )]
    #[cfg_attr(
        not(target_os = "macos"),
        link_name = "_ZN6swoole5Timer3addElbPvRKSt8functionIFvPS0_PNS_9TimerNodeEEE"
    )]
    fn timer_add(
        this: *mut Timer,
        msec: c_long,
        persistent: bool,
        data: *mut c_void,
        callback: Callback,
    ) -> *mut TimerNode;
    #[link_name = "_ZN6swoole5Timer6removeEl"]
    fn timer_remove(this: *mut Timer, id: c_long) -> bool;
    #[link_name = "_ZN6swoole5Timer6selectEv"]
    fn timer_select(this: *mut Timer);
    #[link_name = "_ZN6swoole5Timer5applyERK17swr_v1_timer_view"]
    fn timer_apply(view: *const TimerView) -> *mut TimerNode;
    #[link_name = "_Znwm"]
    fn operator_new(size: usize) -> *mut c_void;
    #[link_name = "_ZdlPv"]
    fn operator_delete(pointer: *mut c_void);
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

/// `Timer *swoole_timer_create(bool manually_trigger)`: `new Timer`, published
/// as this thread's.
#[unsafe(export_name = "_Z19swoole_timer_createb")]
extern "C-unwind" fn swoole_timer_create(manually_trigger: bool) -> *mut c_void {
    /// Frees the allocation if the constructor throws, as a new-expression does.
    struct Allocation(*mut c_void);
    impl Drop for Allocation {
        fn drop(&mut self) {
            // SAFETY: operator new's, never constructed.
            unsafe { operator_delete(self.0) };
        }
    }
    // SAFETY: operator new throws rather than return null.
    let allocation = Allocation(unsafe { operator_new(TIMER_SIZE) });
    let timer = allocation.0.cast::<Timer>();
    // SAFETY: fresh storage of sizeof(Timer) bytes.
    unsafe { timer_construct(timer, manually_trigger) };
    std::mem::forget(allocation);
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
    unsafe { (*timer).next_msec }
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
    // SAFETY: this thread's live timer; `callback` as the caller promises.
    unsafe { timer_add(timer(), ms, persistent, data, callback) }.cast()
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
    unsafe { timer_remove(timer, node.id) }
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
    let timer = unsafe { &*timer };
    let now = crate::swr_v1_timer_relative_msec(crate::absolute_msec(), timer.base_time);
    let mut deadline = 0;
    // SAFETY: a writable deadline.
    if unsafe { swr_v1_timer_delay_deadline(now, node.exec_msec, delay_ms, &raw mut deadline) } == 0
    {
        return;
    }
    let mut view = TimerView::default();
    // SAFETY: the timer's live queue and a writable view.
    if unsafe { swr_v1_timer_update(timer.queue.cast(), node.id, deadline, &raw mut view) }
        == crate::OK
    {
        // SAFETY: the view of a live node.
        unsafe { timer_apply(&raw const view) };
    }
}

/// `Timer::get(id)`.
fn get(timer: *mut Timer, id: c_long) -> *mut TimerNode {
    // SAFETY: this thread's live timer.
    let queue = unsafe { (*timer).queue };
    // SAFETY: its live queue.
    unsafe { swr_v1_timer_token(queue.cast(), id) }.cast()
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
    unsafe { timer_remove(timer, timer_id) }
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
    // SAFETY: the timer swoole_timer_create made; destroyed, then freed, once.
    unsafe { timer_destruct(timer) };
    // SAFETY: as above.
    unsafe { operator_delete(timer.cast()) };
    // SAFETY: this thread's live slot.
    unsafe { *slot() = std::ptr::null_mut() };
}

/// `void swoole_timer_select()`: runs due timers, if there is a timer.
#[unsafe(export_name = "_Z19swoole_timer_selectv")]
extern "C-unwind" fn swoole_timer_select() {
    let timer = timer();
    if !timer.is_null() {
        // SAFETY: this thread's live timer.
        unsafe { timer_select(timer) };
    }
}
