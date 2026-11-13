// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/timer.cc (Swoole contributors).
//! The `swoole_timer_*` C API (upstream `src/wrapper/timer.cc`, which the timer
//! patch deletes) under its upstream symbols: this thread's `SwooleTG.timer`, its
//! lazy creation, unit conversion and warnings, over the Rust-owned timer
//! (`owner.rs`).
use crate::owner::{Callback, Closure, Owner as Timer, Plain, TimerNode};
use std::cell::Cell;
use std::ffi::{CStr, c_long, c_void};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::{log, swoole_print_backtrace};

/// `SW_ERR` and `SW_TIMER_MIN_SEC`.
const ERR: c_long = -1;
const MIN_SEC: f64 = 0.001;

pub(crate) fn timer() -> *mut Timer {
    swoole_rs_corebase::thread_timer().cast()
}

/// Publishes `timer` (or null) as this thread's.
pub(crate) fn publish(timer: *mut Timer) {
    if !timer.is_null() {
        GENERATION.set(GENERATION.get() + 1);
    }
    swoole_rs_corebase::set_thread_timer(timer.cast());
}

thread_local! {
    /// How many timers this thread has published. Ids restart with each, so a
    /// [`Held`] id is good only for the timer that issued it.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// A timer's id, as a holder keeps it: with the thread timer that issued it, since
/// a thread whose timer was freed and made again numbers its timers from the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    generation: u64,
    id: c_long,
}

impl Held {
    fn issued(id: c_long) -> Self {
        Self {
            generation: GENERATION.get(),
            id,
        }
    }

    /// An id no timer issued, which names none: for a holder's tests.
    #[must_use]
    pub const fn detached(id: c_long) -> Self {
        Self { generation: 0, id }
    }

    /// The generation of the thread timer that issued the id, then the id: what a
    /// holder that cannot name this type keeps (crates/process's `ArmedTimer`), for
    /// [`cancel_issued`].
    #[must_use]
    pub const fn parts(self) -> (u64, c_long) {
        (self.generation, self.id)
    }

    /// The id, while this thread's timer is still the one that issued it.
    fn live(self) -> Option<c_long> {
        (GENERATION.get() == self.generation).then_some(self.id)
    }
}

/// This thread's timer, if it has one.
pub(crate) fn current<'a>() -> Option<&'a Timer> {
    // SAFETY: null or this thread's live timer.
    unsafe { timer().as_ref() }
}

/// This thread's timer, or `origin`'s "timer is not available" warning.
fn available<'a>(origin: &CStr) -> Option<&'a Timer> {
    let timer = current();
    if timer.is_none() {
        log::warning(origin, format_args!("timer is not available"));
    }
    timer
}

/// `bool swoole_timer_is_available()`.
#[unsafe(export_name = "_Z25swoole_timer_is_availablev")]
#[must_use]
pub extern "C" fn swoole_timer_is_available() -> bool {
    !timer().is_null()
}

/// `Timer *swoole_timer_create(bool manually_trigger)`: a new timer, published
/// as this thread's.
#[unsafe(export_name = "_Z19swoole_timer_createb")]
#[expect(
    clippy::must_use_candidate,
    reason = "a caller that only wants the thread's timer created ignores it"
)]
pub extern "C-unwind" fn swoole_timer_create(manually_trigger: bool) -> *mut c_void {
    let timer = Timer::create(manually_trigger);
    publish(timer);
    timer.cast()
}

/// `SwooleTG.timer->count()`, or 0 without a timer.
#[must_use]
pub fn thread_timer_count() -> usize {
    current().map_or(0, Timer::count)
}

/// `int64_t swoole_timer_get_next_msec()`: -1 without a timer.
#[unsafe(export_name = "_Z26swoole_timer_get_next_msecv")]
pub extern "C" fn swoole_timer_get_next_msec() -> i64 {
    current().map_or(-1, Timer::next_msec)
}

/// `sec2msec(timeout)`, at least `SW_TIMER_MIN_SEC`.
fn sec2msec(timeout: f64) -> c_long {
    let timeout = if timeout < MIN_SEC { MIN_SEC } else { timeout };
    (timeout * 1000.0).c_cast::<c_long>()
}

/// Adds `node` to this thread's timer, creating it on first use: the node and its id.
fn add(ms: c_long, persistent: bool, node: TimerNode) -> Option<(*mut TimerNode, c_long)> {
    if timer().is_null() {
        swoole_timer_create(false);
    }
    current()?.add(ms, persistent, node)
}

/// The node [`add`] added, or null.
fn node(added: Option<(*mut TimerNode, c_long)>) -> *mut TimerNode {
    added.map_or(std::ptr::null_mut(), |(node, _)| node)
}

/// `TimerNode *swoole_timer_add(double timeout, ...)`: seconds, at least
/// `SW_TIMER_MIN_SEC`.
#[unsafe(export_name = "_Z16swoole_timer_adddbPFvPN6swoole5TimerEPNS_9TimerNodeEEPv")]
extern "C-unwind" fn swoole_timer_add_sec(
    timeout: f64,
    persistent: bool,
    callback: Option<Callback>,
    data: *mut c_void,
) -> *mut TimerNode {
    node(add(
        sec2msec(timeout),
        persistent,
        TimerNode::new(callback, data),
    ))
}

/// `TimerNode *swoole_timer_add(long ms, ...)`: creates this thread's timer on
/// first use.
#[unsafe(export_name = "_Z16swoole_timer_addlbPFvPN6swoole5TimerEPNS_9TimerNodeEEPv")]
extern "C-unwind" fn swoole_timer_add(
    ms: c_long,
    persistent: bool,
    callback: Option<Callback>,
    data: *mut c_void,
) -> *mut TimerNode {
    node(add(ms, persistent, TimerNode::new(callback, data)))
}

/// `swoole_timer_add(ms, persistent, ...)` of a callback of `data` alone: the
/// node, or null.
///
/// # Safety
/// Each time the timer fires, `callback(data)` meets `callback`'s contract:
/// `data` stays valid for it until the timer has fired its last time or been
/// deleted.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_timer_add_plain(
    ms: c_long,
    persistent: bool,
    callback: Plain,
    data: *mut c_void,
) -> *mut c_void {
    node(add(ms, persistent, TimerNode::plain(callback, data))).cast()
}

/// `swoole_timer_add(timeout, persistent, ...)`, in seconds, of a callback of
/// `data` alone: the node, or null.
///
/// # Safety
/// As `swr_v1_timer_add_plain`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_timer_add_plain_sec(
    timeout: f64,
    persistent: bool,
    callback: Plain,
    data: *mut c_void,
) -> *mut c_void {
    node(add(
        sec2msec(timeout),
        persistent,
        TimerNode::plain(callback, data),
    ))
    .cast()
}

/// `swoole_timer_add(ms, persistent, ...)` of `f`: the node, or null.
#[must_use]
pub fn add_fn(ms: c_long, persistent: bool, f: impl Fn() + Copy + 'static) -> *mut c_void {
    node(add(ms, persistent, TimerNode::closure(Closure::new(f)))).cast()
}

/// [`add_fn`] of `timeout` seconds, at least `SW_TIMER_MIN_SEC`.
#[must_use]
pub fn add_fn_sec(timeout: f64, persistent: bool, f: impl Fn() + Copy + 'static) -> *mut c_void {
    add_fn(sec2msec(timeout), persistent, f)
}

/// [`add_fn`]'s timer id, or none.
#[must_use]
pub fn add_fn_id(ms: c_long, persistent: bool, f: impl Fn() + Copy + 'static) -> Option<c_long> {
    add(ms, persistent, TimerNode::closure(Closure::new(f))).map(|(_, id)| id)
}

/// [`add_fn`]'s timer, held by id, or none.
#[must_use]
pub fn add_fn_held(ms: c_long, persistent: bool, f: impl Fn() + Copy + 'static) -> Option<Held> {
    add_fn_id(ms, persistent, f).map(Held::issued)
}

/// [`add_fn_sec`]'s timer, held by id, or none.
#[must_use]
pub fn add_fn_sec_id(
    timeout: f64,
    persistent: bool,
    f: impl Fn() + Copy + 'static,
) -> Option<Held> {
    add_fn_held(sec2msec(timeout), persistent, f)
}

/// [`swoole_timer_del`] of a held timer. A timer never reuses an id, so one that
/// already fired is left alone, as is one whose thread timer has since been freed.
pub fn del_id(held: Held) {
    if let Some(id) = held.live()
        && let Some(timer) = available(c"swoole_timer_del")
    {
        timer.remove(id);
    }
}

/// [`del_id`] without the warning when the thread has no timer.
pub fn cancel_held(held: Held) {
    if let Some(id) = held.live() {
        timer_cancel(id);
    }
}

/// [`cancel_held`] of a held timer kept as its [`Held::parts`]: parts no timer
/// issued name none, or at worst another timer of this thread, as any id does.
pub extern "C-unwind" fn cancel_issued(generation: u64, id: c_long) {
    cancel_held(Held { generation, id });
}

/// Deletes timer `id` if this thread's timer still holds it, quietly otherwise:
/// a timer never reuses an id, so one that already fired is left alone.
fn timer_cancel(id: c_long) {
    if let Some(timer) = current() {
        timer.remove(id);
    }
}

/// `swoole_timer_after`/`swoole_timer_tick`: the new timer's id, or `SW_ERR`.
fn schedule(
    origin: &CStr,
    ms: c_long,
    persistent: bool,
    callback: Option<Callback>,
    data: *mut c_void,
) -> c_long {
    if ms <= 0 {
        log::warning(origin, format_args!("Timer must be greater than 0"));
        return ERR;
    }
    add(ms, persistent, TimerNode::new(callback, data)).map_or(ERR, |(_, id)| id)
}

/// `long swoole_timer_after(long ms, TimerCallback, void *)`.
#[unsafe(export_name = "_Z18swoole_timer_afterlPFvPN6swoole5TimerEPNS_9TimerNodeEEPv")]
extern "C-unwind" fn swoole_timer_after(
    ms: c_long,
    callback: Option<Callback>,
    data: *mut c_void,
) -> c_long {
    schedule(c"swoole_timer_after", ms, false, callback, data)
}

/// `long swoole_timer_tick(long ms, TimerCallback, void *)`.
#[unsafe(export_name = "_Z17swoole_timer_ticklPFvPN6swoole5TimerEPNS_9TimerNodeEEPv")]
extern "C-unwind" fn swoole_timer_tick(
    ms: c_long,
    callback: Option<Callback>,
    data: *mut c_void,
) -> c_long {
    schedule(c"swoole_timer_tick", ms, true, callback, data)
}

/// `bool swoole_timer_del(TimerNode *)`: `Timer::remove(tnode)`.
/// # Safety
/// `node` is null or one of this thread's live nodes.
#[unsafe(export_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE")]
pub unsafe extern "C-unwind" fn swoole_timer_del(node: *mut TimerNode) -> bool {
    let Some(timer) = available(c"swoole_timer_del") else {
        return false;
    };
    // SAFETY: null or a live node, as the caller promises.
    unsafe { node.as_ref() }.is_some_and(|node| timer.remove(node.id))
}

/// `void swoole_timer_delay(TimerNode *, long delay_ms)`: `Timer::delay`, which
/// moves the deadline to `delay_ms` from now.
/// # Safety
/// `node` is null or one of this thread's live nodes.
#[unsafe(export_name = "_Z18swoole_timer_delayPN6swoole9TimerNodeEl")]
pub unsafe extern "C-unwind" fn swoole_timer_delay(node: *mut TimerNode, delay_ms: c_long) {
    let Some(timer) = available(c"swoole_timer_delay") else {
        return;
    };
    // SAFETY: null or a live node, as the caller promises.
    let fields = unsafe { node.as_ref() }.map(|node| (node.id, node.exec_msec));
    if let Some((id, exec_msec)) = fields {
        timer.delay(id, exec_msec, delay_ms);
    }
}

/// `bool swoole_timer_exists(long timer_id)`: a node that isn't removed.
#[unsafe(export_name = "_Z19swoole_timer_existsl")]
extern "C" fn swoole_timer_exists(timer_id: c_long) -> bool {
    available(c"swoole_timer_exists")
        .and_then(|timer| timer.get(timer_id))
        .is_some_and(|node| !node.removed)
}

/// `bool swoole_timer_clear(long timer_id)`: `Timer::remove(id)`.
#[unsafe(export_name = "_Z18swoole_timer_clearl")]
pub(crate) extern "C-unwind" fn swoole_timer_clear(timer_id: c_long) -> bool {
    available(c"swoole_timer_clear").is_some_and(|timer| timer.remove(timer_id))
}

/// `TimerNode *swoole_timer_get(long timer_id)`.
#[unsafe(export_name = "_Z16swoole_timer_getl")]
pub(crate) extern "C" fn swoole_timer_get(timer_id: c_long) -> *mut TimerNode {
    available(c"swoole_timer_get").map_or(std::ptr::null_mut(), |timer| timer.token(timer_id))
}

/// `void swoole_timer_free()`: `delete SwooleTG.timer`, then clear the slot.
#[unsafe(export_name = "_Z17swoole_timer_freev")]
pub extern "C-unwind" fn swoole_timer_free() {
    let timer = timer();
    let Some(live) = current() else {
        swoole_print_backtrace();
        log::warning(c"swoole_timer_free", format_args!("timer is not available"));
        return;
    };
    live.destroy();
    // SAFETY: this thread's timer, `Owner::create`'s allocation, which
    // swoole_timer_create published; destroyed above, borrowed no more and freed once.
    drop(unsafe { Box::from_raw(timer) });
    publish(std::ptr::null_mut());
}

/// `void swoole_timer_select()`: runs due timers, if there is a timer.
#[unsafe(export_name = "_Z19swoole_timer_selectv")]
pub extern "C-unwind" fn swoole_timer_select() {
    if let Some(timer) = current() {
        timer.select();
    }
}

/// What `Swoole\Timer` (crates/phptimer) needs: PHP (`TYPE_PHP`) timers on
/// this thread's timer.
pub mod php {
    use super::{Timer, TimerNode, add, current};
    pub use crate::owner::{Bailout, Info, Plain};
    use std::ffi::c_void;

    /// `swoole_timer_is_available()`.
    #[must_use]
    pub fn available() -> bool {
        current().is_some()
    }

    /// `swoole_timer_add(ms, persistent, ...)` of a PHP timer that runs
    /// `callback(data)` and, once removed, `destructor(data)`: its id, or none.
    pub fn add_php(
        ms: i64,
        persistent: bool,
        callback: Plain,
        destructor: Plain,
        data: *mut c_void,
    ) -> Option<i64> {
        add(ms, persistent, TimerNode::php(callback, destructor, data)).map(|(_, id)| id)
    }

    /// `swoole_timer_get(id)`'s fields, and whether it is a PHP timer.
    #[must_use]
    pub fn info(id: i64) -> Option<(Info, bool)> {
        current()?.get(id).map(|node| (node.info(), node.is_php()))
    }

    /// The timer's `count()` and `get_round()`, if there is a timer.
    #[must_use]
    pub fn stats() -> Option<(usize, u64)> {
        current().map(|timer| (timer.count(), timer.round()))
    }

    /// `Timer::get_ids()`: every timer's id, empty without a timer.
    #[must_use]
    pub fn ids() -> Vec<i64> {
        current().map(Timer::ids).unwrap_or_default()
    }

    /// `swoole_timer_clear`'s lookup and removal of a PHP timer: whether the
    /// timer `id` was one and was removed.
    #[must_use]
    pub fn clear(id: i64) -> bool {
        current()
            .is_some_and(|timer| timer.get(id).is_some_and(TimerNode::is_php) && timer.remove(id))
    }

    /// `Timer::abandon_selection(bailout)` of this thread's timer: after a
    /// caught Zend bailout whose selection will never resume.
    pub fn abandon(bailout: Option<Bailout>) {
        if let Some(timer) = current() {
            timer.abandon(bailout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GENERATION, Held};

    #[test]
    fn a_held_id_names_only_the_timer_that_issued_it() {
        // As publishing a thread's first timer does.
        GENERATION.set(GENERATION.get() + 1);
        let held = Held::issued(3);
        assert_eq!(held.live(), Some(3));
        assert_eq!(Held::detached(3).live(), None);
        // The timer is freed and the next one published, numbering from the start.
        GENERATION.set(GENERATION.get() + 1);
        assert_eq!(held.live(), None);
        assert_eq!(Held::issued(3).live(), Some(3));
    }
}
