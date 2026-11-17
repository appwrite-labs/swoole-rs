// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/timer.cc (Swoole contributors).
//! The `swoole_timer_*` API (upstream `src/wrapper/timer.cc`, which the timer
//! patch deletes): this thread's `SwooleTG.timer`, its lazy creation, unit
//! conversion and warnings, over the Rust-owned timer (`owner.rs`). Timer
//! callbacks own their captures and operations use issued timer IDs.
use crate::CCast;
use crate::timer::owner::{Owner as Timer, TimerNode};
pub use crate::timer_key::Held;
use crate::{log, swoole_print_backtrace};
use std::ffi::{CStr, c_long, c_void};

/// `SW_TIMER_MIN_SEC`.
const MIN_SEC: f64 = 0.001;

pub(crate) fn timer() -> *mut Timer {
    crate::thread_timer().cast()
}

/// Publishes `timer` (or null) as this thread's.
pub(crate) fn publish(timer: *mut Timer) {
    crate::set_thread_timer(timer.cast());
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
#[must_use]
pub fn swoole_timer_is_available() -> bool {
    !timer().is_null()
}

/// `Timer *swoole_timer_create(bool manually_trigger)`: a new timer, published
/// as this thread's.
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

/// Whether this thread has a timer that no reactor drives (`!timer->get_reactor()`).
#[must_use]
pub fn thread_timer_detached() -> bool {
    current().is_some_and(|timer| crate::timer::owner::owner_reactor(timer).is_null())
}

/// `SwooleTG.timer->reinit(manually_trigger)`, when this thread has a timer.
pub fn thread_timer_reinit(manually_trigger: bool) {
    if let Some(timer) = current() {
        crate::timer::owner::owner_reinit(timer, manually_trigger);
    }
}

/// `int64_t swoole_timer_get_next_msec()`: -1 without a timer.
pub extern "C" fn swoole_timer_get_next_msec() -> i64 {
    current().map_or(-1, Timer::next_msec)
}

/// `sec2msec(timeout)`, at least `SW_TIMER_MIN_SEC`.
fn sec2msec(timeout: f64) -> c_long {
    let timeout = if timeout < MIN_SEC { MIN_SEC } else { timeout };
    (timeout * 1000.0).c_cast::<c_long>()
}

/// This thread's timer, created on first use.
fn created<'a>() -> Option<&'a Timer> {
    // One thread-local read when the timer exists, the usual case.
    current().or_else(|| {
        swoole_timer_create(false);
        current()
    })
}

/// `swoole_timer_add(ms, persistent, ...)` of `f`, held by id, or none.
#[must_use]
pub fn add_fn(ms: c_long, persistent: bool, f: impl Fn() + Copy + 'static) -> Option<Held> {
    held(ms, persistent, TimerNode::closure(f))
}

/// [`add_fn`] of a callback that owns what it captures, a `Weak` say: it is dropped
/// once the timer fires for the last time, is deleted, or this thread's timer is freed.
#[must_use]
pub fn add_owned(ms: c_long, persistent: bool, f: impl Fn() + 'static) -> Option<Held> {
    held(ms, persistent, TimerNode::owning(std::rc::Rc::new(f)))
}

/// Adds `node` to this thread's timer, held by id, or none.
#[inline]
fn held(ms: c_long, persistent: bool, node: TimerNode) -> Option<Held> {
    let timer = created()?;
    let (_, id) = timer.add(ms, persistent, node)?;
    let held = Held::new(timer.generation(), id);
    if held.is_none() {
        // An id past what a `Held` packs: as if the add had failed.
        timer.remove(id);
    }
    held
}

/// [`add_fn`] of `timeout` seconds, at least `SW_TIMER_MIN_SEC`.
#[must_use]
pub fn add_fn_sec(timeout: f64, persistent: bool, f: impl Fn() + Copy + 'static) -> Option<Held> {
    add_fn(sec2msec(timeout), persistent, f)
}

/// [`add_owned`] of `timeout` seconds, as [`add_fn_sec`].
#[must_use]
pub fn add_owned_sec(timeout: f64, persistent: bool, f: impl Fn() + 'static) -> Option<Held> {
    add_owned(sec2msec(timeout), persistent, f)
}

/// This thread's timer and the held timer's id, while that timer is the one that
/// issued it.
fn issuer(timer: Option<&Timer>, held: Held) -> Option<(&Timer, c_long)> {
    timer
        .filter(|timer| timer.generation() == held.generation())
        .map(|timer| (timer, held.id()))
}

/// Delete a held timer. A timer never reuses an id, so one that
/// already fired is left alone, as is one whose thread timer has since been freed.
pub fn del_id(held: Held) {
    _ = delete_held(Some(held));
}

/// [`del_id`] without the warning when the thread has no timer.
pub fn cancel_held(held: Held) {
    if let Some((timer, id)) = issuer(current(), held) {
        timer.remove(id);
    }
}

/// [`cancel_held`] of a held timer kept as its generation and id (`corebase::process`'s
/// `ArmedTimer`): parts no timer issued name none, or at worst another timer of this
/// thread, as any id does.
pub extern "C-unwind" fn cancel_issued(generation: u64, id: c_long) {
    if let Some(held) = u16::try_from(generation)
        .ok()
        .and_then(|generation| Held::new(generation, id))
    {
        cancel_held(held);
    }
}

/// Move a held timer's deadline to `delay_ms` from now. None is a no-op;
/// a missing thread timer reports the original `swoole_timer_delay` warning.
pub fn delay_held(held: Option<Held>, delay_ms: c_long) {
    let timer = available(c"swoole_timer_delay");
    if let Some((timer, id)) = held.and_then(|held| issuer(timer, held)) {
        timer.delay(id, None, delay_ms);
    }
}

/// Schedule an owned `after` or `tick` callback. Nonpositive intervals warn and
/// fail before creating a thread timer, as those APIs do.
#[must_use]
pub fn schedule_owned(ms: c_long, persistent: bool, f: impl Fn() + 'static) -> Option<Held> {
    if ms <= 0 {
        let origin = if persistent {
            c"swoole_timer_tick"
        } else {
            c"swoole_timer_after"
        };
        log::warning(origin, format_args!("Timer must be greater than 0"));
        return None;
    }
    add_owned(ms, persistent, f)
}

/// Delete an issued timer. None, an expired timer, or an ID from a different
/// thread timer returns false. A missing thread timer retains the original warning.
#[must_use]
pub fn delete_held(held: Option<Held>) -> bool {
    let timer = available(c"swoole_timer_del");
    held.and_then(|held| issuer(timer, held))
        .is_some_and(|(timer, id)| timer.remove(id))
}

/// `bool swoole_timer_exists(long timer_id)`: a node that isn't removed.
#[must_use]
pub fn swoole_timer_exists(timer_id: c_long) -> bool {
    available(c"swoole_timer_exists")
        .and_then(|timer| timer.get(timer_id))
        .is_some_and(|node| !node.fields.removed)
}

/// `bool swoole_timer_clear(long timer_id)`: `Timer::remove(id)`.
#[must_use]
pub fn swoole_timer_clear(timer_id: c_long) -> bool {
    available(c"swoole_timer_clear").is_some_and(|timer| timer.remove(timer_id))
}

/// `swoole_timer_get(timer_id)` as the timer's issued id, or none. The handle
/// remains usable after the node fires; operations then leave it alone.
#[must_use]
pub fn swoole_timer_get(timer_id: c_long) -> Option<Held> {
    let timer = available(c"swoole_timer_get")?;
    timer.get(timer_id)?;
    Held::new(timer.generation(), timer_id)
}

/// `void swoole_timer_free()`: `delete SwooleTG.timer`, then clear the slot.
pub fn swoole_timer_free() {
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

/// `void swoole_timer_select()`: runs due timers, if there is a timer; also the
/// reactor's timer end callback (`EndFn`).
pub fn swoole_timer_select() {
    if let Some(timer) = current() {
        timer.select();
    }
}

/// What `Swoole\Timer` (crates/phptimer) needs: PHP timers on this thread's timer.
pub mod php {
    use super::{Timer, current};
    pub use crate::timer::owner::{Bailout, Info};

    /// `swoole_timer_is_available()`.
    #[must_use]
    pub fn available() -> bool {
        current().is_some()
    }

    /// A retained callback of at most two words, aligned no more than a word.
    /// The timer owns it inline. Release runs after detaching the node
    /// and before deleting it, so a caller can defer a bailout until deletion.
    pub trait Callback: 'static {
        fn run(&self);
        fn release(self);
    }

    /// A reserved PHP timer, confined to this thread. Dropping it cancels the
    /// empty node. No borrow of a timer spans the caller's retention operation.
    #[must_use]
    pub struct Reservation {
        generation: u64,
        id: std::num::NonZeroI64,
        armed: bool,
        thread: std::marker::PhantomData<std::rc::Rc<()>>,
    }

    impl Reservation {
        /// The issued timer id, also passed to a persistent PHP callback.
        #[must_use]
        pub fn id(&self) -> std::num::NonZeroI64 {
            self.id
        }

        fn issuer(&self) -> Option<&Timer> {
            current().filter(|timer| timer.identity() == self.generation)
        }

        /// Transfer the retained callback to the reserved node. If retention
        /// re-entered the timer and removed it, release the callback instead.
        ///
        /// Raw callback/data pairs cannot pass through this safe API:
        /// ```compile_fail
        /// use swoole_rs_corebase::timer::php::reserve;
        /// unsafe extern "C-unwind" fn callback(_: *mut std::ffi::c_void) {}
        /// reserve(1, false).unwrap().commit((callback, callback, std::ptr::null_mut::<()>()));
        /// ```
        #[must_use]
        pub fn commit<C: Callback>(mut self, callback: C) -> bool {
            self.armed = false;
            if let Some(node) = self.issuer().and_then(|timer| timer.get(self.id.get())) {
                node.install(callback);
                true
            } else {
                callback.release();
                false
            }
        }
    }

    impl Drop for Reservation {
        fn drop(&mut self) {
            if self.armed
                && let Some(timer) = self.issuer()
            {
                timer.remove(self.id.get());
            }
        }
    }

    /// Reserve a node before retaining the callback; refusal retains nothing.
    #[must_use]
    pub fn reserve(ms: i64, persistent: bool) -> Option<Reservation> {
        let timer = super::created()?;
        Some(Reservation {
            generation: timer.identity(),
            id: std::num::NonZeroI64::new(timer.reserve_php(ms, persistent)?)?,
            armed: true,
            thread: std::marker::PhantomData,
        })
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
        current().map_or_default(Timer::ids)
    }

    /// `swoole_timer_clear`'s lookup and removal of a PHP timer: whether the
    /// timer `id` was one and was removed.
    #[must_use]
    pub fn clear(id: i64) -> bool {
        current().is_some_and(|timer| timer.remove_php(id))
    }

    /// `Timer::abandon_selection(bailout)` of this thread's timer: after a
    /// caught Zend bailout whose selection will never resume.
    pub fn abandon(bailout: Option<Bailout>) {
        if let Some(timer) = current() {
            timer.abandon(bailout);
        }
    }
}
