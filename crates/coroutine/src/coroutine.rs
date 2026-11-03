// SPDX-License-Identifier: Apache-2.0
//! The coroutine scheduler and each coroutine's state: ids, lifecycle state,
//! resume code, task, cancel function, the origin tree and timing.
//!
//! C++ stores the scheduler in its original thread-local scope and each
//! [`Coroutine`] inside its `swoole::Coroutine` facade, and reads fields inline;
//! every write goes through these functions. Registering and releasing keep the
//! live map (`live.rs`). C++ keeps the effects Rust must not run: the swap
//! callbacks (PHP may longjmp out of them), the context switch, timers and `delete`.
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

/// One coroutine. `native` is the C++ facade that embeds it.
#[repr(C)]
#[derive(Debug)]
pub struct Coroutine {
    pub native: *mut c_void,
    pub cid: c_long,
    pub state: i32,
    pub resume_code: i32,
    pub task: *mut c_void,
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
    pub activated: bool,
}

/// Swap callback slots for [`swr_v1_coroutine_set_callback`].
pub const ON_YIELD: u32 = 0;
pub const ON_RESUME: u32 = 1;
pub const ON_CLOSE: u32 = 2;

impl Scheduler {
    /// Takes the next id and seeds `co`, which `native` embeds. Returns the id.
    pub fn register(
        &mut self,
        co: &mut Coroutine,
        native: *mut c_void,
        init_msec: c_long,
        now: c_long,
    ) -> c_long {
        self.last_cid = self.last_cid.wrapping_add(1);
        *co = Coroutine {
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
        };
        self.last_cid
    }

    /// Records a new live count. Returns whether the scheduler needs activating.
    pub fn admit(&mut self, count: usize) -> bool {
        if count as u64 > self.peak_num {
            self.peak_num = count as u64;
        }
        !self.activated
    }

    pub fn set_stack_size(&mut self, size: usize) {
        let bounded = size.clamp(STACK_MIN, STACK_MAX);
        self.stack_size = (bounded + STACK_ALIGNMENT - 1) & !(STACK_ALIGNMENT - 1);
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
    // SAFETY: forwarded from the caller.
    unsafe { set_origin(co, ptr::null_mut()) };
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

macro_rules! export {
    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)? $body:block) => {
        $(#[$doc])*
        /// # Safety
        /// Pointer arguments are live, aligned and not borrowed elsewhere for the
        /// call; coroutines are linked only to live coroutines.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) $(-> $ret)? $body
    };
}

export! {
    /// Constructor: seeds `co` with the next id and the creation clocks, and
    /// makes it live.
    fn swr_v1_coroutine_register(s: *mut Scheduler, co: *mut Coroutine, native: *mut c_void, init_msec: c_long, now: c_long) -> c_long {
        // SAFETY: the caller lends the scheduler exclusively.
        let s = unsafe { &mut *s };
        // SAFETY: the caller lends `co` exclusively.
        let cid = s.register(unsafe { &mut *co }, native, init_msec, now);
        crate::live::insert(co, cid);
        cid
    }
}

export! {
    /// After registering: tracks the peak, returns whether to activate.
    fn swr_v1_coroutine_admit(s: *mut Scheduler) -> bool {
        // SAFETY: the caller lends the scheduler exclusively.
        unsafe { (*s).admit(crate::live::count()) }
    }
}

export! {
    fn swr_v1_coroutine_set_active(s: *mut Scheduler, active: bool) {
        // SAFETY: the caller lends the scheduler exclusively.
        unsafe { (*s).activated = active };
    }
}

export! {
    fn swr_v1_coroutine_set_stack_size(s: *mut Scheduler, size: usize) {
        // SAFETY: the caller lends the scheduler exclusively.
        unsafe { (*s).set_stack_size(size) };
    }
}

export! {
    fn swr_v1_coroutine_bind_socket(s: *mut Scheduler, cid: c_long) {
        // SAFETY: the caller lends the scheduler exclusively.
        unsafe { (*s).socket_bound_cid = cid };
    }
}

export! {
    /// `slot` is [`ON_YIELD`], [`ON_RESUME`] or [`ON_CLOSE`].
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

export! {
    /// Run: the current coroutine becomes `co`'s origin and `co` runs.
    fn swr_v1_coroutine_enter(s: *mut Scheduler, co: *mut Coroutine) {
        // SAFETY: the caller lends the scheduler exclusively.
        let s = unsafe { &mut *s };
        // SAFETY: the caller lends `co` and its links exclusively.
        unsafe { enter(s, co) };
    }
}

export! {
    /// Resume, before the resume callback.
    fn swr_v1_coroutine_wake(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).state = STATE_RUNNING };
    }
}

export! {
    /// Yield, before the yield callback: waiting, with a clean resume code.
    fn swr_v1_coroutine_suspend(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        let co = unsafe { &mut *co };
        co.state = STATE_WAITING;
        co.resume_code = RC_OK;
    }
}

export! {
    /// Yield, after the yield callback: control returns to `co`'s origin.
    fn swr_v1_coroutine_leave(s: *mut Scheduler, co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        let origin = unsafe { (*co).origin };
        // SAFETY: the caller lends the scheduler exclusively.
        unsafe { (*s).current = origin };
    }
}

export! {
    /// Close, before the close callback.
    fn swr_v1_coroutine_end(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).state = STATE_END };
    }
}

export! {
    /// Close, after the close callback: control returns to the origin, `co`
    /// unlinks from the origin tree, orphaning the coroutines it resumed, and is
    /// no longer live.
    fn swr_v1_coroutine_release(s: *mut Scheduler, co: *mut Coroutine) {
        // SAFETY: the caller lends the scheduler exclusively.
        let s = unsafe { &mut *s };
        // SAFETY: the caller lends `co` and its links exclusively.
        unsafe { release(s, co) };
        // SAFETY: `co` is live for this call.
        crate::live::remove(unsafe { (*co).cid });
    }
}

export! {
    fn swr_v1_coroutine_set_origin(co: *mut Coroutine, origin: *mut Coroutine) {
        // SAFETY: forwarded from the caller.
        unsafe { set_origin(co, origin) };
    }
}

export! {
    fn swr_v1_coroutine_set_task(co: *mut Coroutine, task: *mut c_void) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).task = task };
    }
}

export! {
    fn swr_v1_coroutine_set_cancel_fn(co: *mut Coroutine, cancel_fn: *mut c_void) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).cancel_fn = cancel_fn };
    }
}

export! {
    /// Cancel, after C++ copied the cancel function.
    fn swr_v1_coroutine_cancel(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        let co = unsafe { &mut *co };
        co.cancel_fn = ptr::null_mut();
        co.resume_code = RC_CANCELED;
    }
}

export! {
    /// A timed wait expired, before its resume.
    fn swr_v1_coroutine_timeout(co: *mut Coroutine) {
        // SAFETY: the caller lends `co` exclusively.
        unsafe { (*co).resume_code = RC_TIMEDOUT };
    }
}

export! {
    /// `SW_CORO_TIME`: charge `yielding` up to `now`, start `resuming`'s slice.
    /// Either may be null; they may be the same coroutine.
    fn swr_v1_coroutine_switch_time(yielding: *mut Coroutine, resuming: *mut Coroutine, now: c_long) {
        if yielding == resuming {
            // SAFETY: null or live, lent exclusively.
            let co = unsafe { yielding.as_mut() };
            if let Some(co) = co {
                switch_time(Some(co), None, now);
                co.switch_usec = now;
            }
            return;
        }
        // SAFETY: null or live and lent exclusively; distinct from `resuming`.
        let yielding = unsafe { yielding.as_mut() };
        // SAFETY: null or live and lent exclusively.
        let resuming = unsafe { resuming.as_mut() };
        switch_time(yielding, resuming, now);
    }
}

export! {
    fn swr_v1_coroutine_execute_usec(co: *const Coroutine, now: c_long) -> c_long {
        // SAFETY: the caller passes a live coroutine.
        let co = unsafe { &*co };
        now.wrapping_sub(co.switch_usec).wrapping_add(co.execute_usec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler() -> Scheduler {
        Scheduler {
            current: ptr::null_mut(),
            last_cid: 0,
            socket_bound_cid: 0,
            peak_num: 0,
            stack_size: 2 * 1024 * 1024,
            on_yield: None,
            on_resume: None,
            on_close: None,
            activated: false,
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
        assert_eq!(size_of::<Scheduler>(), 72);
        assert_eq!(core::mem::offset_of!(Scheduler, activated), 64);
    }

    #[test]
    fn register_counts_ids_and_peak() {
        let mut s = scheduler();
        let (mut a, mut b) = (blank(), blank());
        assert_eq!(s.register(&mut a, ptr::null_mut(), 5, 9), 1);
        assert!(s.admit(1));
        s.activated = true;
        assert_eq!(s.register(&mut b, ptr::null_mut(), 5, 9), 2);
        assert!(!s.admit(2));
        assert!(!s.admit(1));
        assert_eq!(s.peak_num, 2);
        assert_eq!(
            (b.cid, b.state, b.init_msec, b.switch_usec),
            (2, STATE_INIT, 5, 9)
        );
        s.last_cid = c_long::MAX;
        assert_eq!(s.register(&mut b, ptr::null_mut(), 0, 0), c_long::MIN);
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
            enter(&mut s, a);
            enter(&mut s, b);
            assert_eq!((s.current, (*b).origin, (*a).first_child), (b, a, b));
            swr_v1_coroutine_leave(&raw mut s, b);
            enter(&mut s, c);
            // a resumed b then c: c heads a's children.
            assert_eq!(
                ((*a).first_child, (*c).next_sibling, (*b).prev_sibling),
                (c, b, c)
            );
            swr_v1_coroutine_leave(&raw mut s, c);
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
        }
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
    fn cancel_and_timeout_codes() {
        let mut co = blank();
        let co = &raw mut co;
        // SAFETY: live on this frame.
        unsafe {
            swr_v1_coroutine_set_cancel_fn(co, co.cast());
            swr_v1_coroutine_cancel(co);
            assert_eq!(
                ((*co).resume_code, (*co).cancel_fn),
                (RC_CANCELED, ptr::null_mut())
            );
            swr_v1_coroutine_wake(co);
            assert_eq!(
                ((*co).state, (*co).resume_code),
                (STATE_RUNNING, RC_CANCELED)
            );
            swr_v1_coroutine_suspend(co);
            assert_eq!(((*co).state, (*co).resume_code), (STATE_WAITING, RC_OK));
            swr_v1_coroutine_timeout(co);
            assert_eq!((*co).resume_code, RC_TIMEDOUT);
        }
    }

    #[test]
    fn timing_charges_the_yielding_slice() {
        let (mut a, mut b) = (blank(), blank());
        a.switch_usec = 10;
        b.switch_usec = 20;
        // SAFETY: live on this frame.
        unsafe {
            swr_v1_coroutine_switch_time(&raw mut a, &raw mut b, 30);
            assert_eq!((a.execute_usec, b.switch_usec), (20, 30));
            swr_v1_coroutine_switch_time(&raw mut a, &raw mut a, 50);
            assert_eq!((a.execute_usec, a.switch_usec), (60, 50));
            swr_v1_coroutine_switch_time(ptr::null_mut(), ptr::null_mut(), 0);
            assert_eq!(swr_v1_coroutine_execute_usec(&raw const a, 55), 65);
        }
    }
}
