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
use core::ptr::{self, NonNull};

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

impl Coroutine {
    /// Its execution time in microseconds up to `now`.
    #[must_use]
    pub fn execute_usec(&self, now: c_long) -> c_long {
        now.wrapping_sub(self.switch_usec)
            .wrapping_add(self.execute_usec)
    }

    /// Canceled, from the canceler once it can no longer fail.
    pub fn mark_canceled(&mut self) {
        self.cancel_fn = ptr::null_mut();
        self.resume_code = RC_CANCELED;
    }

    /// A timed wait expired, before its resume.
    pub fn mark_timed_out(&mut self) {
        self.resume_code = RC_TIMEDOUT;
    }
}

/// A cancelable yield's cancel function: `call(native, data)` marks the
/// coroutine canceled ([`Coroutine::mark_canceled`]) once nothing can fail, then
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

/// Swap callback slots for [`Scheduler::set_callback`].
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

    /// `slot` is [`ON_YIELD`], [`ON_RESUME`] or [`ON_CLOSE`].
    pub fn set_callback(&mut self, slot: u32, function: SwapCallback) {
        match slot {
            ON_YIELD => self.on_yield = function,
            ON_RESUME => self.on_resume = function,
            _ => self.on_close = function,
        }
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

/// A live coroutine: registered (or being registered) and not yet destroyed.
/// Coroutines linked from a live one (its origin, children and siblings) are
/// live too, since a close unlinks them first. Each access borrows it for one
/// closure, so no reference to one spans a hook, which may change it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Co(NonNull<Coroutine>);

impl Co {
    /// # Safety
    /// `co` is null or a live coroutine, which stays live while the handle is used.
    unsafe fn of(co: *mut Coroutine) -> Option<Self> {
        NonNull::new(co).map(Self)
    }

    /// A link field's coroutine, live by the handle's invariant.
    fn link(co: *mut Coroutine) -> Option<Self> {
        NonNull::new(co).map(Self)
    }

    fn ptr(self) -> *mut Coroutine {
        self.0.as_ptr()
    }

    /// The coroutine, borrowed for `f`, which neither switches nor reaches
    /// this coroutine through another handle.
    fn with<R>(self, f: impl FnOnce(&mut Coroutine) -> R) -> R {
        // SAFETY: a live coroutine; the borrow ends with `f`.
        f(unsafe { &mut *self.ptr() })
    }

    fn origin(self) -> Option<Self> {
        Self::link(self.with(|co| co.origin))
    }
}

fn raw(co: Option<Co>) -> *mut Coroutine {
    co.map_or(ptr::null_mut(), Co::ptr)
}

/// A live scheduler, borrowed one closure at a time like [`Co`].
#[derive(Clone, Copy)]
struct Sched(NonNull<Scheduler>);

impl Sched {
    /// # Safety
    /// `s` is null or a live scheduler, which stays live while the handle is used.
    unsafe fn of(s: *mut Scheduler) -> Option<Self> {
        NonNull::new(s).map(Self)
    }

    /// The scheduler, borrowed for `f`, which runs no hook or callback.
    fn with<R>(self, f: impl FnOnce(&mut Scheduler) -> R) -> R {
        // SAFETY: a live scheduler; the borrow ends with `f`.
        f(unsafe { &mut *self.0.as_ptr() })
    }
}

/// Moves `co` from its origin's children to `origin`'s.
fn link(co: Co, origin: Option<Co>) {
    let (parent, prev, next) = co.with(|this| {
        (
            Co::link(this.origin),
            Co::link(this.prev_sibling),
            Co::link(this.next_sibling),
        )
    });
    if parent == origin {
        return;
    }
    if let Some(parent) = parent {
        match prev {
            None => parent.with(|parent| parent.first_child = raw(next)),
            Some(prev) => prev.with(|prev| prev.next_sibling = raw(next)),
        }
        if let Some(next) = next {
            next.with(|next| next.prev_sibling = raw(prev));
        }
    }
    let first = origin.and_then(|origin| Co::link(origin.with(|origin| origin.first_child)));
    co.with(|this| {
        this.origin = raw(origin);
        this.prev_sibling = ptr::null_mut();
        this.next_sibling = raw(first);
    });
    if let Some(first) = first {
        first.with(|first| first.prev_sibling = co.ptr());
    }
    if let Some(origin) = origin {
        origin.with(|origin| origin.first_child = co.ptr());
    }
}

/// `co` becomes current, run from the coroutine that was.
fn enter(s: Sched, co: Co) {
    link(co, Co::link(s.with(|s| s.current)));
    s.with(|s| s.current = co.ptr());
    co.with(|co| co.state = STATE_RUNNING);
}

/// `co`, the current coroutine, closes: its origin is current again.
fn release(s: Sched, co: Co) {
    let origin = co.with(|co| co.origin);
    s.with(|s| s.current = origin);
    // The coroutines this one resumed may still be suspended: they must never
    // reach its memory again.
    while let Some(child) = Co::link(co.with(|co| co.first_child)) {
        let next = child.with(|child| {
            child.origin = ptr::null_mut();
            child.prev_sibling = ptr::null_mut();
            core::mem::replace(&mut child.next_sibling, ptr::null_mut())
        });
        co.with(|co| co.first_child = next);
    }
    link(co, None);
    let cid = co.with(|co| co.cid);
    s.with(|s| s.remove(cid));
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
/// They are never the same coroutine.
fn charge(hooks: &Hooks, yielding: Option<Co>, resuming: Option<Co>) {
    let Some(clock) = hooks.clock else {
        return;
    };
    // SAFETY: C++'s clock reads the time.
    let now = unsafe { clock() };
    if let Some(yielding) = yielding {
        yielding.with(|co| switch_time(Some(co), None, now));
    }
    if let Some(resuming) = resuming {
        resuming.with(|co| switch_time(None, Some(co), now));
    }
}

/// Calls a swap callback with `co`'s task, if both are set; it may change any state.
fn notify(callback: SwapCallback, co: Co) {
    let task = co.with(|co| co.task);
    if let Some(callback) = callback
        && !task.is_null()
    {
        // SAFETY: C++ registered the callback for tasks like this one.
        unsafe { callback(task) };
    }
}

/// The scheduler's hooks and swap callbacks, copied out so no borrow of it
/// spans a call back into C++.
fn effects(s: Sched) -> (Hooks, [SwapCallback; 3]) {
    s.with(|s| (s.hooks, [s.on_yield, s.on_resume, s.on_close]))
}

/// Switches into `co`, which is now current, then closes it if it ended or
/// runs the bailout callback (upstream's `check_end`).
fn switch_in(s: Sched, co: Co) {
    let (hooks, [_, _, on_close]) = effects(s);
    let native = co.with(|co| co.native);
    // SAFETY: a live record; the switch runs the coroutine until it yields or ends.
    let ended = unsafe { (hooks.swap_in)(native) };
    if ended {
        // `co` ended and is current again: it closes.
        co.with(|co| co.state = STATE_END);
        notify(on_close, co);
        debug_assert!(
            s.with(|s| s.current) == co.ptr(),
            "only the current coroutine closes"
        );
        release(s, co);
        // SAFETY: the record ended and is released; the facade is deleted
        // here, and `co` with it.
        unsafe { (hooks.destroy)(native) };
        return;
    }
    // The hook as it is now: the coroutine that bailed out set it while it ran.
    let bailout = s.with(|s| {
        s.hooks
            .bailout
            .filter(|_| s.bailout)
            .map(|f| (f, s.hooks.bailout_data))
    });
    if let Some((bailout, data)) = bailout {
        // SAFETY: the callback longjmps over this frame, which owns nothing.
        unsafe { bailout(data) };
    }
}

/// `co` (current, or the outermost while bailing out) yields to its origin.
fn yield_(s: Sched, co: Co) {
    let (hooks, [on_yield, _, _]) = effects(s);
    co.with(|co| {
        co.state = STATE_WAITING;
        co.resume_code = RC_OK;
    });
    notify(on_yield, co);
    let (origin, native) = co.with(|this| (Co::link(this.origin), this.native));
    s.with(|s| {
        debug_assert!(
            s.current == co.ptr() || s.bailout,
            "only the current coroutine yields"
        );
        s.current = raw(origin);
    });
    charge(&hooks, Some(co), origin);
    // SAFETY: the running record; the switch returns when it is resumed.
    unsafe { (hooks.swap_out)(native) };
}

/// Run and resume: the current coroutine becomes `co`'s origin, its slice is
/// charged (and `co`'s started on resume), and `co` runs.
fn start(s: Sched, co: Co, resuming: bool) {
    enter(s, co);
    let hooks = s.with(|s| s.hooks);
    charge(&hooks, co.origin(), resuming.then_some(co));
    switch_in(s, co);
}

/// The operations' handles, none for a null pointer.
///
/// # Safety
/// As for the operations.
unsafe fn handles(s: *mut Scheduler, co: *mut Coroutine) -> Option<(Sched, Co)> {
    // SAFETY: forwarded.
    let s = unsafe { Sched::of(s) }?;
    // SAFETY: forwarded.
    Some((s, unsafe { Co::of(co) }?))
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
    /// `Coroutine::run`: the current coroutine becomes `co`'s origin and `co`
    /// runs until it first yields or ends. Returns its id.
    fn swr_v1_coroutine_run(s: *mut Scheduler, co: *mut Coroutine) -> c_long {
        // SAFETY: forwarded.
        let Some((s, co)) = (unsafe { handles(s, co) }) else {
            return -1;
        };
        let cid = co.with(|co| co.cid);
        start(s, co, false);
        cid
    }
}

operation! {
    /// `Coroutine::resume`: refused while bailing out; otherwise the resume
    /// callback sees the old current, then `co` runs until it yields or ends.
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_resume(s: *mut Scheduler, co: *mut Coroutine) {
        // SAFETY: forwarded.
        let Some((s, co)) = (unsafe { handles(s, co) }) else {
            return;
        };
        let on_resume = s.with(|s| {
            debug_assert!(s.current != co.ptr(), "the current coroutine cannot resume");
            (!s.bailout).then_some(s.on_resume)
        });
        let Some(on_resume) = on_resume else {
            return;
        };
        co.with(|co| co.state = STATE_RUNNING);
        notify(on_resume, co);
        start(s, co, true);
    }
}

operation! {
    /// `Coroutine::yield`: waiting, with a clean resume code; the yield callback
    /// runs, control returns to `co`'s origin, and this returns once resumed.
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_yield(s: *mut Scheduler, co: *mut Coroutine) {
        // SAFETY: forwarded.
        if let Some((s, co)) = unsafe { handles(s, co) } {
            yield_(s, co);
        }
    }
}

operation! {
    /// `Coroutine::yield(CancelFunc *)`: as [`swr_v1_coroutine_yield`] with a
    /// cancel function (a [`Canceler`]) set until it is resumed.
    fn swr_v1_coroutine_yield_cancelable(s: *mut Scheduler, co: *mut Coroutine, cancel_fn: *mut c_void) {
        // SAFETY: forwarded.
        let Some((s, co)) = (unsafe { handles(s, co) }) else {
            return;
        };
        co.with(|co| co.cancel_fn = cancel_fn);
        yield_(s, co);
        // Live again once resumed.
        co.with(|co| co.cancel_fn = ptr::null_mut());
    }
}

operation! {
    /// `Coroutine::bailout`, once the callback is set: marks the scheduler, and
    /// inside a coroutine yields the one closest to the main context, which
    /// switches there directly and never returns.
    fn swr_v1_coroutine_bailout(s: *mut Scheduler) {
        // SAFETY: forwarded.
        let Some(s) = (unsafe { Sched::of(s) }) else {
            return;
        };
        let current = s.with(|s| {
            s.bailout = true;
            s.current
        });
        let Some(mut co) = Co::link(current) else {
            return;
        };
        while let Some(origin) = co.origin() {
            co = origin;
        }
        yield_(s, co);
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
        let sp = unsafe { Sched::of(&raw mut s) }.unwrap();
        let co = |co| unsafe { Co::of(co) }.unwrap();
        // SAFETY: the three coroutines live on this frame for the whole test.
        unsafe {
            for co in [a, b, c] {
                sp.with(|s| s.register(co, ptr::null_mut(), 0, 0));
            }
            enter(sp, co(a));
            enter(sp, co(b));
            assert_eq!(
                (sp.with(|s| s.current), (*b).origin, (*a).first_child),
                (b, a, b)
            );
            sp.with(|s| s.current = (*b).origin);
            enter(sp, co(c));
            // a resumed b then c: c heads a's children.
            assert_eq!(
                ((*a).first_child, (*c).next_sibling, (*b).prev_sibling),
                (c, b, c)
            );
            sp.with(|s| s.current = (*c).origin);
            // a closes while b and c are suspended: both lose their origin.
            release(sp, co(a));
            assert!(sp.with(|s| s.current).is_null());
            assert!((*b).origin.is_null() && (*c).origin.is_null() && (*a).first_child.is_null());
            assert!((*b).prev_sibling.is_null() && (*c).next_sibling.is_null());
            // b resumes from main, then unlinks itself on close.
            enter(sp, co(b));
            assert!((*b).origin.is_null());
            enter(sp, co(c));
            release(sp, co(c));
            assert_eq!(
                (sp.with(|s| s.current), (*b).first_child),
                (b, ptr::null_mut())
            );
            release(sp, co(b));
            assert_eq!(s.count(), 0);
        }
        s.set_active(false);
    }

    #[test]
    fn removing_a_middle_child_relinks_siblings() {
        let (mut p, mut x, mut y, mut z) = (blank(), blank(), blank(), blank());
        let (p, x, y, z) = (&raw mut p, &raw mut x, &raw mut y, &raw mut z);
        let co = |co| unsafe { Co::of(co) }.unwrap();
        // SAFETY: all live on this frame.
        unsafe {
            for child in [x, y, z] {
                link(co(child), Some(co(p)));
            }
            link(co(y), None);
            assert_eq!(
                ((*p).first_child, (*z).next_sibling, (*x).prev_sibling),
                (z, x, z)
            );
            link(co(z), None);
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
            (*co).mark_canceled();
            assert_eq!(
                ((*co).resume_code, (*co).cancel_fn),
                (RC_CANCELED, ptr::null_mut())
            );
            (*co).mark_timed_out();
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
        assert_eq!(a.execute_usec(55), 65);
    }
}
