// SPDX-License-Identifier: Apache-2.0
//! The coroutine scheduler and each coroutine's state: ids, lifecycle state,
//! resume code, task, cancel function, the origin tree, timing and the live map
//! (`live.rs`), with whole run/resume/yield/close/bailout operations.
//!
//! The extension keeps the scheduler per thread (scheduler.rs) and each
//! [`Coroutine`] at the start of its `swoole::Coroutine` record; C++ reads fields
//! inline, and every write goes through these functions. The operations call the
//! swap callbacks and the [`Hooks`]: the context switch, the release of an ended
//! coroutine and the bailout callback. No `&mut` into the scheduler or a
//! coroutine is held across one of those calls, since they change both.
use core::ffi::{c_long, c_void};
use core::ptr;

pub const STATE_INIT: i32 = 0;
pub const STATE_WAITING: i32 = 1;
pub const STATE_RUNNING: i32 = 2;
pub const STATE_END: i32 = 3;
pub const RC_OK: i32 = 0;
pub const RC_TIMEDOUT: i32 = -1;
pub const RC_CANCELED: i32 = -2;

const STACK_MIN: usize = 64 * 1024;
const STACK_MAX: usize = 16 * 1024 * 1024;
const STACK_ALIGNMENT: usize = 4 * 1024;

pub type SwapCallback = Option<unsafe extern "C" fn(*mut c_void)>;

/// One coroutine. `native` is the `swoole::Coroutine` record that starts with it.
#[repr(C)]
#[derive(Debug)]
pub struct Coroutine {
    pub native: *mut c_void,
    pub cid: c_long,
    pub state: i32,
    pub resume_code: i32,
    pub task: *mut c_void,
    /// The waiting yield's [`Canceler`], null when it can't be canceled.
    pub cancel_fn: *mut c_void,
    /// The coroutine that last ran this one; cleared when it closes first.
    pub origin: *mut Coroutine,
    /// Live coroutines whose origin is this one, linked through their siblings.
    pub first_child: *mut Coroutine,
    pub prev_sibling: *mut Coroutine,
    pub next_sibling: *mut Coroutine,
    pub init_msec: c_long,
    pub switch_usec: c_long,
    pub execute_usec: c_long,
}

/// A cancelable yield's cancel function: `call(native, data)` marks the
/// coroutine canceled ([`swr_v1_coroutine_cancel`]) once nothing can fail, then
/// wakes it and returns whether it did.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Canceler {
    pub call: unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> bool,
    pub data: *mut c_void,
}

/// The effects the scheduler runs, each given a `swoole::Coroutine` record
/// (`Coroutine::native`). Rust frames between these calls own nothing with a
/// destructor, so a coroutine stack the bailout abandons, or a longjmp out of a
/// callback, skips no cleanup.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Hooks {
    /// Switches into the coroutine; returns once it yields or ends, and whether it ended.
    pub swap_in: unsafe extern "C" fn(*mut c_void) -> bool,
    /// Switches out of the coroutine, back to whatever last switched in.
    pub swap_out: unsafe extern "C" fn(*mut c_void),
    /// Frees an ended coroutine's record (and its stack).
    pub destroy: unsafe extern "C" fn(*mut c_void),
    /// The bailout callback (upstream's `on_bailout`), which PHP's longjmps out
    /// of, and its data; set while the scheduler bails out.
    pub bailout: Option<unsafe extern "C" fn(*mut c_void)>,
    pub bailout_data: *mut c_void,
    /// `SW_CORO_TIME`'s clock in microseconds; null without it.
    pub clock: Option<unsafe extern "C" fn() -> c_long>,
}

/// Per-thread scheduler state (one per PHP thread under ZTS).
#[repr(C)]
#[derive(Debug)]
pub struct Scheduler {
    pub current: *mut Coroutine,
    pub last_cid: c_long,
    pub socket_bound_cid: c_long,
    pub peak_num: u64,
    pub stack_size: usize,
    pub on_yield: SwapCallback,
    pub on_resume: SwapCallback,
    pub on_close: SwapCallback,
    pub hooks: Hooks,
    /// The live coroutines, null until the first registers.
    pub live: *mut crate::live::Live,
    /// Where `swoole_coroutine_iterator_each` (gdb's walk) resumes.
    pub cursor: c_long,
    pub activated: bool,
    /// Mirrors `hooks.bailout` being set: resumes are refused, and a switch
    /// back that did not end the coroutine runs the callback.
    pub bailout: bool,
}

/// Swap callback slots for [`swr_v1_coroutine_set_callback`].
pub const ON_YIELD: u32 = 0;
pub const ON_RESUME: u32 = 1;
pub const ON_CLOSE: u32 = 2;

impl Scheduler {
    /// Takes the next id, seeds `co` (which `native` embeds) and makes it live.
    /// Returns whether the scheduler needs activating.
    ///
    /// # Safety
    /// `co` is writable, not borrowed, and stays put until it is released.
    pub unsafe fn register(
        &mut self,
        co: *mut Coroutine,
        native: *mut c_void,
        init_msec: c_long,
        now: c_long,
    ) -> bool {
        self.last_cid = self.last_cid.wrapping_add(1);
        // SAFETY: the caller lends `co` exclusively; it may be uninitialised.
        unsafe {
            co.write(Coroutine {
                native,
                cid: self.last_cid,
                state: STATE_INIT,
                resume_code: RC_OK,
                task: ptr::null_mut(),
                cancel_fn: ptr::null_mut(),
                origin: ptr::null_mut(),
                first_child: ptr::null_mut(),
                prev_sibling: ptr::null_mut(),
                next_sibling: ptr::null_mut(),
                init_msec,
                switch_usec: now,
                execute_usec: 0,
            });
        }
        self.insert(self.last_cid, co);
        let count = self.count() as u64;
        if count > self.peak_num {
            self.peak_num = count;
        }
        !self.activated
    }

    pub fn set_stack_size(&mut self, size: usize) {
        let bounded = size.clamp(STACK_MIN, STACK_MAX);
        self.stack_size = (bounded + STACK_ALIGNMENT - 1) & !(STACK_ALIGNMENT - 1);
    }

    /// Activation also clears the bailout mark; deactivation sets it (the
    /// caller sets the callback to match) and frees the live map if it is empty.
    #[cfg(any(test, feature = "upstream-symbols"))]
    pub(crate) fn set_active(&mut self, active: bool) {
        self.activated = active;
        self.bailout = !active;
        if !active {
            self.release_live();
        }
    }
}

/// # Safety
/// `co` and every coroutine linked from it are live and not otherwise borrowed.
unsafe fn set_origin(co: *mut Coroutine, origin: *mut Coroutine) {
    // SAFETY: the caller guarantees `co` and its links are live and exclusive.
    let this = unsafe { &mut *co };
    let parent = this.origin;
    if parent == origin {
        return;
    }
    if !parent.is_null() {
        if this.prev_sibling.is_null() {
            // SAFETY: a non-null origin is live (close clears its children's links).
            unsafe { (*parent).first_child = this.next_sibling };
        } else {
            // SAFETY: linked siblings are live.
            unsafe { (*this.prev_sibling).next_sibling = this.next_sibling };
        }
        if !this.next_sibling.is_null() {
            // SAFETY: linked siblings are live.
            unsafe { (*this.next_sibling).prev_sibling = this.prev_sibling };
        }
        this.prev_sibling = ptr::null_mut();
        this.next_sibling = ptr::null_mut();
    }
    this.origin = origin;
    if !origin.is_null() {
        // SAFETY: the new origin is live.
        let origin = unsafe { &mut *origin };
        this.next_sibling = origin.first_child;
        if !this.next_sibling.is_null() {
            // SAFETY: linked siblings are live.
            unsafe { (*this.next_sibling).prev_sibling = co };
        }
        origin.first_child = co;
    }
}

/// # Safety
/// `scheduler` and `co` (with every coroutine linked from it) are live and
/// exclusively lent for this call.
unsafe fn enter(scheduler: &mut Scheduler, co: *mut Coroutine) {
    // SAFETY: forwarded from the caller.
    unsafe { set_origin(co, scheduler.current) };
    scheduler.current = co;
    // SAFETY: `co` is live.
    unsafe { (*co).state = STATE_RUNNING };
}

/// # Safety
/// As for [`enter`]; `co` is the current coroutine.
unsafe fn release(scheduler: &mut Scheduler, co: *mut Coroutine) {
    // SAFETY: `co` is live.
    let this = unsafe { &mut *co };
    scheduler.current = this.origin;
    // The coroutines this one resumed may still be suspended: they must never
    // reach its memory again.
    while !this.first_child.is_null() {
        // SAFETY: children are live while linked.
        let child = unsafe { &mut *this.first_child };
        this.first_child = child.next_sibling;
        child.origin = ptr::null_mut();
        child.prev_sibling = ptr::null_mut();
        child.next_sibling = ptr::null_mut();
    }
    let cid = this.cid;
    // SAFETY: forwarded from the caller; `this` isn't used again, since this
    // borrows `co` anew.
    unsafe { set_origin(co, ptr::null_mut()) };
    scheduler.remove(cid);
}

fn switch_time(yielding: Option<&mut Coroutine>, resuming: Option<&mut Coroutine>, now: c_long) {
    if let Some(co) = yielding {
        co.execute_usec = co
            .execute_usec
            .wrapping_add(now.wrapping_sub(co.switch_usec));
    }
    if let Some(co) = resuming {
        co.switch_usec = now;
    }
}

/// `SW_CORO_TIME`: charges `yielding` up to now and starts `resuming`'s slice.
/// Either may be null; they are never the same coroutine.
///
/// # Safety
/// The coroutines are null or live, and neither is borrowed.
unsafe fn charge(hooks: &Hooks, yielding: *mut Coroutine, resuming: *mut Coroutine) {
    let Some(clock) = hooks.clock else {
        return;
    };
    // SAFETY: C++'s clock reads the time.
    let now = unsafe { clock() };
    // SAFETY: null or live, distinct from `resuming`, not borrowed.
    let yielding = unsafe { yielding.as_mut() };
    // SAFETY: null or live, not borrowed.
    let resuming = unsafe { resuming.as_mut() };
    switch_time(yielding, resuming, now);
}

/// Calls a swap callback with `co`'s task, if both are set.
///
/// # Safety
/// `co` is live and not borrowed; the callback may change any state.
unsafe fn notify(callback: SwapCallback, co: *const Coroutine) {
    // SAFETY: `co` is live.
    let task = unsafe { (*co).task };
    if let Some(callback) = callback
        && !task.is_null()
    {
        // SAFETY: C++ registered the callback for tasks like this one.
        unsafe { callback(task) };
    }
}

/// The scheduler's hooks and swap callbacks, copied out so no borrow of it
/// spans a call back into C++.
///
/// # Safety
/// `s` is live.
unsafe fn effects(s: *const Scheduler) -> (Hooks, [SwapCallback; 3]) {
    // SAFETY: the caller guarantees `s` is live; this reads and releases it.
    let s = unsafe { &*s };
    (s.hooks, [s.on_yield, s.on_resume, s.on_close])
}

/// Switches into `co`, which is now current, then closes it if it ended or
/// runs the bailout callback (upstream's `check_end`).
///
/// # Safety
/// `s` and `co` are live and nothing borrows them.
unsafe fn switch_in(s: *mut Scheduler, co: *mut Coroutine) {
    // SAFETY: forwarded.
    let (hooks, [_, _, on_close]) = unsafe { effects(s) };
    // SAFETY: `co` is live.
    let native = unsafe { (*co).native };
    // SAFETY: the switch runs the coroutine until it yields or ends.
    let ended = unsafe { (hooks.swap_in)(native) };
    if ended {
        // SAFETY: `co` ended and is current again: it closes.
        unsafe { (*co).state = STATE_END };
        // SAFETY: `co` is live.
        unsafe { notify(on_close, co) };
        // SAFETY: `s` is live and nothing else borrows it between effects.
        let s = unsafe { &mut *s };
        debug_assert!(s.current == co, "only the current coroutine closes");
        // SAFETY: `co` and its links are live.
        unsafe { release(s, co) };
        // SAFETY: the facade is deleted here, and `co` with it.
        unsafe { (hooks.destroy)(native) };
        return;
    }
    // The hook as it is now: the coroutine that bailed out set it while it ran.
    let bailout = {
        // SAFETY: `s` is live; the borrow ends before the callback runs.
        let s = unsafe { &*s };
        s.hooks
            .bailout
            .filter(|_| s.bailout)
            .map(|f| (f, s.hooks.bailout_data))
    };
    if let Some((bailout, data)) = bailout {
        // SAFETY: the callback longjmps over this frame, which owns nothing.
        unsafe { bailout(data) };
    }
}

/// # Safety
/// As for [`switch_in`]; `co` is current (or a bailout is under way).
unsafe fn yield_(s: *mut Scheduler, co: *mut Coroutine) {
    // SAFETY: forwarded.
    let (hooks, [on_yield, _, _]) = unsafe { effects(s) };
    {
        // SAFETY: `co` is live and exclusively ours here.
        let co = unsafe { &mut *co };
        co.state = STATE_WAITING;
        co.resume_code = RC_OK;
    }
    // SAFETY: `co` is live.
    unsafe { notify(on_yield, co) };
    let (origin, native) = {
        // SAFETY: `co` is live and exclusively ours here.
        let co = unsafe { &*co };
        (co.origin, co.native)
    };
    {
        // SAFETY: `s` is live and exclusively ours here.
        let s = unsafe { &mut *s };
        debug_assert!(
            s.current == co || s.bailout,
            "only the current coroutine yields"
        );
        s.current = origin;
    }
    // SAFETY: both live or null, and distinct.
    unsafe { charge(&hooks, co, origin) };
    // SAFETY: `co` is live; the switch returns when it is resumed.
    unsafe { (hooks.swap_out)(native) };
}

/// Run and resume: the current coroutine becomes `co`'s origin, its slice is
/// charged (and `co`'s started on resume), and `co` runs.
///
/// # Safety
/// As for [`switch_in`].
unsafe fn start(s: *mut Scheduler, co: *mut Coroutine, resuming: bool) {
    let (hooks, origin) = {
        // SAFETY: `s` is live and exclusively ours here.
        let s = unsafe { &mut *s };
        // SAFETY: `co` and its links are live.
        unsafe { enter(s, co) };
        // SAFETY: `co` is live.
        (s.hooks, unsafe { (*co).origin })
    };
    let resumed = if resuming { co } else { ptr::null_mut() };
    // SAFETY: both live or null, and distinct.
    unsafe { charge(&hooks, origin, resumed) };
    // SAFETY: forwarded.
    unsafe { switch_in(s, co) };
}

macro_rules! operation {
    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)? $body:block) => {
        $(#[$doc])*
        /// # Safety
        /// Pointer arguments are live and aligned, and not borrowed elsewhere for
        /// the call; coroutines are linked only to live coroutines.
        pub unsafe extern "C" fn $name($($arg: $ty),*) $(-> $ret)? $body
    };
}

operation! {
    fn swr_v1_coroutine_set_stack_size(s: *mut Scheduler, size: usize) {
        // SAFETY: the caller lends the scheduler exclusively.
        unsafe { (*s).set_stack_size(size) };
    }
}

operation! {
    /// `slot` is [`ON_YIELD`], [`ON_RESUME`] or [`ON_CLOSE`].
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_set_callback(s: *mut Scheduler, slot: u32, function: SwapCallback) {
        // SAFETY: the caller lends the scheduler exclusively.
        let s = unsafe { &mut *s };
        match slot {
            ON_YIELD => s.on_yield = function,
            ON_RESUME => s.on_resume = function,
            _ => s.on_close = function,
        }
    }
}

operation! {
    /// `Coroutine::run`: the current coroutine becomes `co`'s origin and `co`
    /// runs until it first yields or ends. Returns its id.
    fn swr_v1_coroutine_run(s: *mut Scheduler, co: *mut Coroutine) -> c_long {
        // SAFETY: the caller lends `co`.
        let cid = unsafe { (*co).cid };
        // SAFETY: forwarded.
        unsafe { start(s, co, false) };
        cid
    }
}

operation! {
    /// `Coroutine::resume`: refused while bailing out; otherwise the resume
    /// callback sees the old current, then `co` runs until it yields or ends.
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_resume(s: *mut Scheduler, co: *mut Coroutine) {
        {
            // SAFETY: the caller lends the scheduler.
            let s = unsafe { &*s };
            debug_assert!(s.current != co, "the current coroutine cannot resume");
            if s.bailout {
                return;
            }
        }
        // SAFETY: forwarded.
        let (_, [_, on_resume, _]) = unsafe { effects(s) };
        // SAFETY: the caller lends `co`.
        unsafe { (*co).state = STATE_RUNNING };
        // SAFETY: forwarded.
        unsafe { notify(on_resume, co) };
        // SAFETY: forwarded.
        unsafe { start(s, co, true) };
    }
}

operation! {
    /// `Coroutine::yield`: waiting, with a clean resume code; the yield callback
    /// runs, control returns to `co`'s origin, and this returns once resumed.
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_yield(s: *mut Scheduler, co: *mut Coroutine) {
        // SAFETY: forwarded.
        unsafe { yield_(s, co) };
    }
}

operation! {
    /// `Coroutine::yield(CancelFunc *)`: as [`swr_v1_coroutine_yield`] with a
    /// cancel function (a [`Canceler`]) set until it is resumed.
    fn swr_v1_coroutine_yield_cancelable(s: *mut Scheduler, co: *mut Coroutine, cancel_fn: *mut c_void) {
        // SAFETY: the caller lends `co`.
        unsafe { (*co).cancel_fn = cancel_fn };
        // SAFETY: forwarded.
        unsafe { yield_(s, co) };
        // SAFETY: `co` is live again once resumed.
        unsafe { (*co).cancel_fn = ptr::null_mut() };
    }
}

operation! {
    /// `Coroutine::bailout`, once the callback is set: marks the scheduler, and
    /// inside a coroutine yields the one closest to the main context, which
    /// switches there directly and never returns.
    fn swr_v1_coroutine_bailout(s: *mut Scheduler) {
        // SAFETY: the caller lends the scheduler.
        unsafe { (*s).bailout = true };
        // SAFETY: as above.
        let mut co = unsafe { (*s).current };
        if co.is_null() {
            return;
        }
        loop {
            // SAFETY: origins are live while linked.
            let origin = unsafe { (*co).origin };
            if origin.is_null() {
                break;
            }
            co = origin;
        }
        // SAFETY: forwarded; `co` is live.
        unsafe { yield_(s, co) };
    }
}

operation! {
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_set_task(co: *mut Coroutine, task: *mut c_void) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).task = task };
    }
}

operation! {
    /// Cancel, from the canceler once it can no longer fail.
    fn swr_v1_coroutine_cancel(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        let co = unsafe { &mut *co };
        co.cancel_fn = ptr::null_mut();
        co.resume_code = RC_CANCELED;
    }
}

operation! {
    /// A timed wait expired, before its resume.
    fn swr_v1_coroutine_timeout(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).resume_code = RC_TIMEDOUT };
    }
}

operation! {
    fn swr_v1_coroutine_execute_usec(co: *const Coroutine, now: c_long) -> c_long {
        // SAFETY: the caller passes a live coroutine.
        let co = unsafe { &*co };
        now.wrapping_sub(co.switch_usec).wrapping_add(co.execute_usec)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        /// What the test hooks saw, in order.
        static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// Coroutines whose next switch-in ends them.
        static ENDING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }

    fn event(text: String) {
        EVENTS.with_borrow_mut(|events| events.push(text));
    }

    fn take_events() -> Vec<String> {
        EVENTS.with_borrow_mut(core::mem::take)
    }

    unsafe extern "C" fn swap_in(native: *mut c_void) -> bool {
        event(format!("in {}", native as usize));
        ENDING.with_borrow_mut(|ending| {
            let at = ending.iter().position(|n| *n == native as usize);
            at.map(|at| ending.remove(at)).is_some()
        })
    }
    unsafe extern "C" fn swap_out(native: *mut c_void) {
        event(format!("out {}", native as usize));
    }
    unsafe extern "C" fn destroy(native: *mut c_void) {
        event(format!("destroy {}", native as usize));
    }
    unsafe extern "C" fn bailout(_: *mut c_void) {
        event("bailout".into());
    }
    unsafe extern "C" fn clock() -> c_long {
        100
    }
    unsafe extern "C" fn on_resume(task: *mut c_void) {
        event(format!("on_resume {}", task as usize));
    }
    unsafe extern "C" fn on_yield(task: *mut c_void) {
        event(format!("on_yield {}", task as usize));
    }
    unsafe extern "C" fn on_close(task: *mut c_void) {
        event(format!("on_close {}", task as usize));
    }

    pub(crate) fn scheduler() -> Scheduler {
        Scheduler {
            current: ptr::null_mut(),
            last_cid: 0,
            socket_bound_cid: 0,
            peak_num: 0,
            stack_size: 2 * 1024 * 1024,
            on_yield: None,
            on_resume: None,
            on_close: None,
            hooks: Hooks {
                swap_in,
                swap_out,
                destroy,
                bailout: Some(bailout),
                bailout_data: ptr::null_mut(),
                clock: None,
            },
            live: ptr::null_mut(),
            cursor: 0,
            activated: false,
            bailout: false,
        }
    }

    fn blank() -> Coroutine {
        // SAFETY: all-zero is a valid Coroutine (null pointers, zero scalars).
        unsafe { core::mem::zeroed() }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn layout_matches_the_header() {
        // swoole_coroutine.h static_asserts the same sizes on the C++ side.
        assert_eq!(size_of::<Coroutine>(), 96);
        assert_eq!(core::mem::offset_of!(Coroutine, origin), 40);
        assert_eq!(size_of::<Scheduler>(), 136);
        assert_eq!(core::mem::offset_of!(Scheduler, hooks), 64);
        assert_eq!(core::mem::offset_of!(Scheduler, activated), 128);
    }

    #[test]
    fn register_counts_ids_and_peak() {
        let mut s = scheduler();
        let (mut a, mut b, mut c) = (blank(), blank(), blank());
        // SAFETY: the coroutines live on this frame past their removal.
        unsafe {
            assert!(s.register(&raw mut a, ptr::null_mut(), 5, 9));
            assert_eq!(a.cid, 1);
            s.activated = true;
            assert!(!s.register(&raw mut b, ptr::null_mut(), 5, 9));
            assert_eq!(s.peak_num, 2);
            assert_eq!(
                (b.cid, b.state, b.init_msec, b.switch_usec),
                (2, STATE_INIT, 5, 9)
            );
            s.remove(1);
            s.remove(2);
            assert!(!s.register(&raw mut c, ptr::null_mut(), 0, 0));
            assert_eq!((c.cid, s.peak_num, s.count()), (3, 2, 1));
            s.remove(3);
            s.last_cid = c_long::MAX;
            s.register(&raw mut c, ptr::null_mut(), 0, 0);
            assert_eq!(c.cid, c_long::MIN);
        }
        s.remove(c_long::MIN);
        s.set_active(false);
        assert!(s.live.is_null() && s.bailout);
    }

    #[test]
    fn stack_size_is_bounded_and_aligned() {
        let mut s = scheduler();
        for (requested, expected) in [
            (0, STACK_MIN),
            (STACK_MIN + 1, STACK_MIN + STACK_ALIGNMENT),
            (STACK_MAX - 1, STACK_MAX),
            (usize::MAX, STACK_MAX),
        ] {
            s.set_stack_size(requested);
            assert_eq!(s.stack_size, expected);
        }
    }

    #[test]
    fn origin_tree_follows_run_yield_and_close() {
        let mut s = scheduler();
        let (mut a, mut b, mut c) = (blank(), blank(), blank());
        let (a, b, c) = (&raw mut a, &raw mut b, &raw mut c);
        // SAFETY: the three coroutines live on this frame for the whole test.
        unsafe {
            for co in [a, b, c] {
                s.register(co, ptr::null_mut(), 0, 0);
            }
            enter(&mut s, a);
            enter(&mut s, b);
            assert_eq!((s.current, (*b).origin, (*a).first_child), (b, a, b));
            s.current = (*b).origin;
            enter(&mut s, c);
            // a resumed b then c: c heads a's children.
            assert_eq!(
                ((*a).first_child, (*c).next_sibling, (*b).prev_sibling),
                (c, b, c)
            );
            s.current = (*c).origin;
            // a closes while b and c are suspended: both lose their origin.
            release(&mut s, a);
            assert!(s.current.is_null());
            assert!((*b).origin.is_null() && (*c).origin.is_null() && (*a).first_child.is_null());
            assert!((*b).prev_sibling.is_null() && (*c).next_sibling.is_null());
            // b resumes from main, then unlinks itself on close.
            enter(&mut s, b);
            assert!((*b).origin.is_null());
            enter(&mut s, c);
            release(&mut s, c);
            assert_eq!((s.current, (*b).first_child), (b, ptr::null_mut()));
            release(&mut s, b);
            assert_eq!(s.count(), 0);
        }
        s.set_active(false);
    }

    #[test]
    fn removing_a_middle_child_relinks_siblings() {
        let (mut p, mut x, mut y, mut z) = (blank(), blank(), blank(), blank());
        let (p, x, y, z) = (&raw mut p, &raw mut x, &raw mut y, &raw mut z);
        // SAFETY: all live on this frame.
        unsafe {
            for child in [x, y, z] {
                set_origin(child, p);
            }
            set_origin(y, ptr::null_mut());
            assert_eq!(
                ((*p).first_child, (*z).next_sibling, (*x).prev_sibling),
                (z, x, z)
            );
            set_origin(z, ptr::null_mut());
            assert_eq!(((*p).first_child, (*x).prev_sibling), (x, ptr::null_mut()));
        }
    }

    #[test]
    fn run_resume_yield_and_close_call_back_in_upstream_order() {
        take_events();
        let mut s = scheduler();
        s.on_yield = Some(on_yield);
        s.on_resume = Some(on_resume);
        s.on_close = Some(on_close);
        let sp = &raw mut s;
        let (mut a, mut b) = (blank(), blank());
        let (a, b) = (&raw mut a, &raw mut b);
        // SAFETY: the scheduler and coroutines live on this frame; the hooks
        // only record events.
        unsafe {
            (*sp).register(a, 10 as *mut c_void, 0, 0);
            (*sp).register(b, 20 as *mut c_void, 0, 0);
            (*a).task = 3 as *mut c_void;
            (*b).task = 2 as *mut c_void;
            // a runs and (in its body) runs b, which yields back to a.
            assert_eq!(swr_v1_coroutine_run(sp, a), 1);
            assert_eq!((*sp).current, a);
            assert_eq!(swr_v1_coroutine_run(sp, b), 2);
            swr_v1_coroutine_yield(sp, b);
            assert_eq!(((*sp).current, (*b).state), (a, STATE_WAITING));
            // a yields with a cancel function, which is cleared once resumed.
            swr_v1_coroutine_yield_cancelable(sp, a, 7 as *mut c_void);
            assert!((*sp).current.is_null() && (*a).cancel_fn.is_null());
            // main resumes b, which ends: it closes and is destroyed.
            ENDING.with_borrow_mut(|ending| ending.push(20));
            swr_v1_coroutine_resume(sp, b);
            assert!((*sp).current.is_null());
            assert_eq!((*sp).count(), 1);
        }
        assert_eq!(
            take_events(),
            [
                "in 10",
                "in 20",
                "on_yield 2",
                "out 20",
                "on_yield 3",
                "out 10",
                "on_resume 2",
                "in 20",
                "on_close 2",
                "destroy 20",
            ]
        );
        s.remove(1);
        s.set_active(false);
    }

    #[test]
    fn bailout_refuses_resumes_and_yields_the_outermost() {
        take_events();
        let mut s = scheduler();
        s.hooks.clock = Some(clock);
        let sp = &raw mut s;
        let (mut a, mut b) = (blank(), blank());
        let (a, b) = (&raw mut a, &raw mut b);
        // SAFETY: as above.
        unsafe {
            (*sp).register(a, 10 as *mut c_void, 0, 0);
            (*sp).register(b, 20 as *mut c_void, 0, 40);
            swr_v1_coroutine_run(sp, a);
            swr_v1_coroutine_run(sp, b);
            // b bails out: a, its outermost origin, yields to main.
            swr_v1_coroutine_bailout(sp);
            assert!((*sp).bailout && (*sp).current.is_null());
            // Upstream charges a when it runs b and again when it yields.
            assert_eq!(((*a).state, (*a).execute_usec), (STATE_WAITING, 200));
            swr_v1_coroutine_resume(sp, b);
            // A switch back that did not end runs the callback.
            swr_v1_coroutine_run(sp, b);
            // Outside a coroutine, bailout only marks.
            (*sp).current = ptr::null_mut();
            (*sp).bailout = false;
            swr_v1_coroutine_bailout(sp);
            assert!((*sp).bailout);
        }
        assert_eq!(
            take_events(),
            ["in 10", "in 20", "out 10", "in 20", "bailout"]
        );
        s.remove(1);
        s.remove(2);
        s.set_active(true);
        assert!(!s.bailout);
        s.set_active(false);
    }

    #[test]
    fn cancel_and_timeout_codes() {
        let mut co = blank();
        let co = &raw mut co;
        // SAFETY: live on this frame.
        unsafe {
            (*co).cancel_fn = co.cast();
            swr_v1_coroutine_cancel(co);
            assert_eq!(
                ((*co).resume_code, (*co).cancel_fn),
                (RC_CANCELED, ptr::null_mut())
            );
            swr_v1_coroutine_timeout(co);
            assert_eq!((*co).resume_code, RC_TIMEDOUT);
        }
    }

    #[test]
    fn timing_charges_the_yielding_slice() {
        let (mut a, mut b) = (blank(), blank());
        a.switch_usec = 10;
        b.switch_usec = 20;
        switch_time(Some(&mut a), Some(&mut b), 30);
        assert_eq!((a.execute_usec, b.switch_usec), (20, 30));
        switch_time(None, None, 0);
        // SAFETY: live on this frame.
        assert_eq!(
            unsafe { swr_v1_coroutine_execute_usec(&raw const a, 55) },
            65
        );
    }
}
