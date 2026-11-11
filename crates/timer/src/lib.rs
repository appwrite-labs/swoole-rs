//! Thread-confined indexed timer queue; native code retains callbacks and tokens.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod heap;
mod runtime;
pub use runtime::*;
#[cfg(feature = "upstream-symbols")]
mod owner;
#[cfg(feature = "upstream-symbols")]
mod wrapper;
#[cfg(feature = "upstream-symbols")]
pub use owner::{
    Owner, TimerNode, swr_v1_timer_owner_queue, swr_v1_timer_owner_reactor,
    swr_v1_timer_owner_reinit,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{BuildHasherDefault, Hasher};
use std::{mem, ptr};
#[cfg(feature = "upstream-symbols")]
pub use wrapper::{
    php, swoole_timer_create, swoole_timer_del, swoole_timer_delay, swoole_timer_free,
    swoole_timer_get_next_msec, swoole_timer_is_available, swoole_timer_select,
    swr_v1_timer_add_plain, swr_v1_timer_add_plain_sec,
};

const OK: i32 = 0;
const EMPTY: i32 = 1;
const DEFERRED: i32 = 2;
const RELEASE: i32 = 3;
const RELEASE_OVERFLOW: i32 = 4;
const INVALID: i32 = -1;
const OVERFLOW: i32 = -2;
const NOMEM: i32 = -3;
const BUSY: i32 = -4;
const NOT_FOUND: i32 = -5;
const CLOSING: i32 = -6;
const CAPACITY: i32 = -7;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerView {
    pub id: i64,
    pub deadline: i64,
    pub interval: i64,
    pub exec_count: u64,
    pub birth_round: u64,
    pub removed: u32,
    pub reserved: u32,
    pub token: *mut c_void,
}
impl Default for TimerView {
    fn default() -> Self {
        Self {
            id: 0,
            deadline: 0,
            interval: 0,
            exec_count: 0,
            birth_round: 0,
            removed: 0,
            reserved: 0,
            token: ptr::null_mut(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TimerStats {
    pub count: usize,
    pub round: u64,
    pub active_id: i64,
    pub selecting: u32,
    pub closing: u32,
}

// Keys are internally allocated monotonic IDs, never caller-selected hash keys.
// A multiplicative integer hash distributes both bucket and control-byte bits
// without SipHash's byte-message machinery. This is not a general-purpose hasher.
#[derive(Default)]
struct TimerIdHasher(u64);
impl Hasher for TimerIdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(byte);
        }
    }
    fn write_i64(&mut self, id: i64) {
        self.0 = id.cast_unsigned();
    }
    fn finish(&self) -> u64 {
        self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

#[derive(Default)]
pub struct Timer {
    heap: heap::Heap,
    positions: HashMap<i64, usize, BuildHasherDefault<TimerIdHasher>>,
    last_id: i64,
    round: u64,
    last_time: i64,
    captured: i64,
    active_id: i64,
    selecting: bool,
    closing: bool,
}
impl Timer {
    fn index(&self, id: i64) -> Option<usize> {
        self.positions
            .get(&id)
            .and_then(|&slot| self.heap.position(slot))
    }
    fn find(&self, id: i64) -> Option<(usize, TimerView)> {
        let index = self.index(id)?;
        Some((index, *self.heap.get(index)?))
    }
    fn detach(&mut self, index: usize) -> Option<TimerView> {
        let entry = self.heap.swap_remove(index)?;
        self.positions.remove(&entry.id);
        Some(entry)
    }
}

unsafe fn zero<T: Default>(out: *mut T) -> bool {
    if out.is_null() {
        false
    } else {
        // SAFETY: the caller passes null or valid, aligned, writable output.
        unsafe { out.write(T::default()) };
        true
    }
}

/// The queue's operations, which crates/timer's owner calls directly
/// (docs/timer-abi-v1.md). Each returns a status and, where the operation has one, a
/// view (the default when it has none).
impl Timer {
    /// Adds a node due `delay` after `now`, repeating when `persistent`.
    pub fn add(
        &mut self,
        now: i64,
        delay: i64,
        persistent: bool,
        token: *mut c_void,
    ) -> (i32, TimerView) {
        let none = TimerView::default();
        if now < 0 || delay <= 0 || token.is_null() {
            return (INVALID, none);
        }
        if self.closing {
            return (CLOSING, none);
        }
        if now < self.last_time {
            return (INVALID, none);
        }
        let Some(deadline) = now.checked_add(delay) else {
            return (OVERFLOW, none);
        };
        let Some(id) = self.last_id.checked_add(1) else {
            return (OVERFLOW, none);
        };
        if self.heap.len() >= (isize::MAX as usize) / mem::size_of::<TimerView>() {
            return (OVERFLOW, none);
        }
        if self.heap.try_reserve(1).is_err() || self.positions.try_reserve(1).is_err() {
            return (NOMEM, none);
        }
        let view = TimerView {
            id,
            deadline,
            interval: if persistent { delay } else { 0 },
            birth_round: self.round,
            token,
            ..TimerView::default()
        };
        let slot = self.heap.push(view);
        self.positions.insert(id, slot);
        self.last_id = id;
        self.last_time = now;
        (OK, view)
    }

    /// The node `id`'s view.
    #[must_use]
    pub fn get(&self, id: i64) -> (i32, TimerView) {
        if id <= 0 {
            return (INVALID, TimerView::default());
        }
        self.find(id)
            .map_or((NOT_FOUND, TimerView::default()), |(_, view)| (OK, view))
    }

    /// The token `id` was added with, or null.
    #[must_use]
    pub fn token(&self, id: i64) -> *mut c_void {
        self.index(id)
            .and_then(|index| self.heap.get(index))
            .map_or(ptr::null_mut(), |view| view.token)
    }

    /// Node `id` falls due at `deadline`.
    pub fn update(&mut self, id: i64, deadline: i64) -> (i32, TimerView) {
        let none = TimerView::default();
        if id <= 0 || deadline < 0 {
            return (INVALID, none);
        }
        if self.closing {
            return (CLOSING, none);
        }
        let Some((index, mut view)) = self.find(id).filter(|(_, view)| view.removed == 0) else {
            return (NOT_FOUND, none);
        };
        view.deadline = deadline;
        self.heap.update(index, view);
        (OK, view)
    }

    /// Removes node `id`; only `RELEASE` hands the token back.
    pub fn remove(&mut self, id: i64) -> (i32, TimerView) {
        let none = TimerView::default();
        if id <= 0 {
            return (INVALID, none);
        }
        let Some((index, mut view)) = self.find(id).filter(|(_, view)| view.removed == 0) else {
            return (NOT_FOUND, none);
        };
        if self.active_id == id {
            view.removed = 1;
            self.heap.update(index, view);
            (DEFERRED, view)
        } else {
            self.detach(index)
                .map_or((NOT_FOUND, none), |entry| (RELEASE, entry))
        }
    }

    /// Begins a selection at `now`.
    pub fn begin(&mut self, now: i64) -> i32 {
        if now < 0 {
            return INVALID;
        }
        if self.closing {
            return CLOSING;
        }
        if self.selecting {
            return BUSY;
        }
        if now < self.last_time {
            return INVALID;
        }
        if self.round == u64::MAX {
            return OVERFLOW;
        }
        self.last_time = now;
        self.captured = now;
        self.selecting = true;
        OK
    }

    /// The selection's next due node, which becomes the active one.
    pub fn select_next(&mut self) -> (i32, TimerView) {
        let none = TimerView::default();
        if !self.selecting {
            return (INVALID, none);
        }
        if self.active_id != 0 {
            return (BUSY, none);
        }
        if self.closing {
            return (CLOSING, none);
        }
        let Some(view) = self.heap.first().copied() else {
            return (EMPTY, none);
        };
        if view.deadline > self.captured || view.birth_round == self.round {
            return (EMPTY, none);
        }
        self.active_id = view.id;
        (OK, view)
    }

    /// The active node `id` ran.
    pub fn complete(&mut self, id: i64) -> (i32, TimerView) {
        let none = TimerView::default();
        if id <= 0 || !self.selecting || self.active_id != id {
            return (INVALID, none);
        }
        let Some((index, mut view)) = self.find(id) else {
            return (INVALID, none);
        };
        let mut next = Some(view.deadline);
        if view.deadline <= self.captured && view.interval > 0 {
            next = (self.captured - view.deadline)
                .checked_div(view.interval)
                .and_then(|n| n.checked_add(1))
                .and_then(|n| n.checked_mul(view.interval))
                .and_then(|n| view.deadline.checked_add(n));
        }
        let executions = view.exec_count.checked_add(1);
        self.active_id = 0;
        let status = if view.removed != 0 || self.closing || view.interval == 0 {
            RELEASE
        } else if next.is_none() || executions.is_none() {
            RELEASE_OVERFLOW
        } else {
            OK
        };
        if status == OK {
            // Both values were checked above; matching avoids fallible unwraps.
            if let (Some(deadline), Some(exec_count)) = (next, executions) {
                view.deadline = deadline;
                view.exec_count = exec_count;
            }
            self.heap.update(index, view);
        } else if let Some(entry) = self.detach(index) {
            view = entry;
        }
        (status, view)
    }

    /// The selection ends; the delay to the next deadline, or -1.
    pub fn end(&mut self) -> (i32, i64) {
        if !self.selecting {
            return (INVALID, -1);
        }
        if self.active_id != 0 {
            return (BUSY, -1);
        }
        let Some(round) = self.round.checked_add(1) else {
            return (OVERFLOW, -1);
        };
        self.selecting = false;
        self.round = round;
        let delay = match self.heap.first() {
            Some(head) if !self.closing => (head.deadline - self.captured).max(1),
            _ => -1,
        };
        (OK, delay)
    }

    /// The count, round, active node and flags (`swr_v1_timer_stats_get`).
    #[must_use]
    pub fn stats(&self) -> TimerStats {
        TimerStats {
            count: self.heap.len(),
            round: self.round,
            active_id: self.active_id,
            selecting: u32::from(self.selecting),
            closing: u32::from(self.closing),
        }
    }

    /// Closes the queue: nothing is added, updated or selected after it, and the
    /// active node is marked removed.
    pub fn close(&mut self) {
        self.closing = true;
        if let Some((index, mut view)) = self.find(self.active_id) {
            view.removed = 1;
            self.heap.update(index, view);
        }
    }

    /// A closed queue's next node to dispose of (`RELEASE`), or `DEFERRED` when only
    /// the active one is left.
    pub fn drain(&mut self) -> (i32, TimerView) {
        let none = TimerView::default();
        if !self.closing {
            return (INVALID, none);
        }
        let Some(head) = self.heap.first() else {
            return (EMPTY, none);
        };
        self.detach(usize::from(head.id == self.active_id))
            .map_or((DEFERRED, none), |view| (RELEASE, view))
    }

    /// Whether the queue may be freed: no node and no selection.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.heap.is_empty() && !self.selecting
    }
}

/// Writes what `op` answers to `out` and returns its status: `INVALID`, without running
/// `op`, when `out` is null, and `INVALID` with the default when `op` has no queue.
///
/// # Safety
/// `out` is null or valid, aligned, writable output, disjoint from the queue.
unsafe fn answer<T: Default>(out: *mut T, op: impl FnOnce() -> Option<(i32, T)>) -> i32 {
    if out.is_null() {
        return INVALID;
    }
    let (status, value) = op().unwrap_or((INVALID, T::default()));
    // SAFETY: as the caller promises; non-null (checked above).
    unsafe { out.write(value) };
    status
}

/// The token `id` was added with, or null. Native code applies every view it is
/// handed to its token, so a lookup that only needs the token copies no view.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_timer_token(queue: Option<&Timer>, id: i64) -> *mut c_void {
    queue.map_or(ptr::null_mut(), |state| state.token(id))
}

/// # Safety
/// `out` is null or valid, aligned output disjoint from the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_stats_get(
    queue: Option<&Timer>,
    out: *mut TimerStats,
) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    unsafe { answer(out, || queue.map(|state| (OK, state.stats()))) }
}

/// # Safety
/// Count/ID storage must be valid, aligned and mutually disjoint per ABI v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_ids(
    queue: Option<&Timer>,
    ids: *mut i64,
    capacity: usize,
    count: *mut usize,
) -> i32 {
    // SAFETY: `count` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(count) } {
        return INVALID;
    }
    let Some(state) = queue else {
        return INVALID;
    };
    // SAFETY: `count` is non-null (checked above), valid, aligned and writable.
    unsafe { count.write(state.heap.len()) };
    if capacity > (isize::MAX as usize) / mem::size_of::<i64>() {
        return OVERFLOW;
    }
    if capacity < state.heap.len() {
        return CAPACITY;
    }
    if capacity != 0 && ids.is_null() {
        return INVALID;
    }
    for (index, entry) in state.heap.iter().enumerate() {
        // SAFETY: `index < heap.len() <= capacity`, and `ids` lends `capacity` writable,
        // aligned slots (non-null when `capacity != 0`, checked above).
        unsafe { ids.wrapping_add(index).write(entry.id) };
    }
    OK
}

#[cfg(test)]
mod kernel;

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::{Cell, RefCell, RefMut};

    thread_local! { static FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) }; }
    struct FailureAllocator;
    fn fail() -> bool {
        FAIL_AFTER
            .try_with(|counter| match counter.get() {
                None => false,
                Some(0) => {
                    counter.set(None);
                    true
                }
                Some(n) => {
                    counter.set(Some(n - 1));
                    false
                }
            })
            .unwrap_or(false)
    }
    unsafe impl GlobalAlloc for FailureAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if fail() {
                ptr::null_mut()
            } else {
                unsafe { System.alloc(layout) }
            }
        }
        unsafe fn realloc(&self, data: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if fail() {
                ptr::null_mut()
            } else {
                unsafe { System.realloc(data, layout, size) }
            }
        }
        unsafe fn dealloc(&self, data: *mut u8, layout: Layout) {
            unsafe { System.dealloc(data, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: FailureAllocator = FailureAllocator;

    /// A queue the test owns, closed, drained and checked idle when it drops.
    struct Owned(RefCell<Timer>);
    impl Owned {
        fn new() -> Self {
            Self(RefCell::new(Timer::default()))
        }
        fn q(&self) -> RefMut<'_, Timer> {
            self.0.borrow_mut()
        }
        fn add(&self, now: i64, delay: i64, persistent: bool, token: usize) -> TimerView {
            let (status, view) =
                self.q()
                    .add(now, delay, persistent, ptr::without_provenance_mut(token));
            assert_eq!(status, OK);
            view
        }
        fn get(&self, id: i64) -> TimerView {
            let (status, view) = self.q().get(id);
            assert_eq!(status, OK);
            view
        }
        fn stats(&self) -> TimerStats {
            self.q().stats()
        }
        fn begin(&self, now: i64) -> i32 {
            self.q().begin(now)
        }
        fn next(&self, expected: i32) -> TimerView {
            let (status, view) = self.q().select_next();
            assert_eq!(status, expected);
            view
        }
        fn complete(&self, id: i64, expected: i32) -> TimerView {
            let (status, view) = self.q().complete(id);
            assert_eq!(status, expected);
            view
        }
        fn end(&self) -> i64 {
            let (status, delay) = self.q().end();
            assert_eq!(status, OK);
            delay
        }
        fn prime(&self) {
            assert_eq!(self.begin(0), OK);
            self.end();
        }
        fn invariant(&self) {
            let state = self.0.borrow();
            assert_eq!(state.heap.len(), state.positions.len());
            for (i, entry) in state.heap.iter().enumerate() {
                assert_eq!(state.positions.get(&entry.id), state.heap.slot(i).as_ref());
                assert_eq!(state.index(entry.id), Some(i));
                if i != 0 {
                    assert!(state.heap.get((i - 1) / 2).unwrap().deadline <= entry.deadline);
                }
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            self.q().close();
            let stats = self.stats();
            if stats.active_id != 0 {
                self.complete(stats.active_id, RELEASE);
            }
            if stats.selecting != 0 {
                self.end();
            }
            while self.q().drain().0 == RELEASE {}
            assert!(self.q().idle());
        }
    }

    #[test]
    fn equal_deadlines_fire_in_upstream_heap_order_and_birth_round_blocks_head() {
        let q = Owned::new();
        let a = q.add(0, 1, false, 1);
        let b = q.add(0, 1, false, 2);
        let c = q.add(0, 1, false, 3);
        assert_eq!(q.begin(1), OK);
        assert_eq!(q.next(EMPTY), TimerView::default());
        assert_eq!(q.end(), 1);
        assert_eq!(q.begin(1), OK);
        for expected in [a, c, b] {
            assert_eq!(q.next(OK), expected);
            assert_eq!(q.complete(expected.id, RELEASE), expected);
            q.invariant();
        }
        q.next(EMPTY);
        assert_eq!(q.end(), -1);
        // A callback-born head blocks an older due entry behind it.
        let old = q.add(1, 10, false, 4);
        assert_eq!(q.begin(20), OK);
        let young = q.add(20, 1, false, 5);
        assert_eq!(q.q().update(young.id, 0).0, OK);
        q.next(EMPTY);
        q.end();
        assert_eq!(q.begin(20), OK);
        assert_eq!(q.next(OK).id, young.id);
        q.complete(young.id, RELEASE);
        assert_eq!(q.next(OK).id, old.id);
        q.complete(old.id, RELEASE);
        q.end();
    }

    #[test]
    fn exact_id_completion_survives_active_updates() {
        for persistent in [false, true] {
            let q = Owned::new();
            let a = q.add(0, 5, persistent, 1);
            let b = q.add(0, 6, false, 2);
            q.prime();
            assert_eq!(q.begin(10), OK);
            assert_eq!(q.next(OK).id, a.id);
            assert_eq!(q.complete(b.id, INVALID), TimerView::default());
            assert_eq!(q.stats().active_id, a.id);
            assert_eq!(q.q().update(a.id, 100).0, OK);
            let completed = q.complete(a.id, if persistent { OK } else { RELEASE });
            assert_eq!(completed.id, a.id);
            assert_eq!(completed.deadline, 100);
            if persistent {
                assert_eq!(completed.exec_count, 1);
                assert_eq!(completed.interval, 5);
            }
            assert_eq!(q.next(OK).id, b.id);
            q.complete(b.id, RELEASE);
            q.end();
            q.invariant();
        }
    }

    #[test]
    fn repeat_coalesces_captured_time_and_callback_clock_advances() {
        let q = Owned::new();
        let a = q.add(0, 5, true, 1);
        let b = q.add(0, 50, false, 2);
        q.prime();
        assert_eq!(q.begin(23), OK);
        assert_eq!(q.next(OK).id, a.id);
        let c = q.add(100, 1, false, 3); // Accepted add time does not alter captured 23.
        assert_eq!(q.complete(a.id, OK).deadline, 25);
        q.next(EMPTY);
        assert_eq!(q.end(), 2);
        assert_eq!(q.begin(99), INVALID);
        assert_eq!(q.begin(100), OK);
        assert_eq!(q.next(OK).id, a.id);
        let out = q.complete(a.id, OK);
        assert_eq!(out.deadline, 105);
        assert_eq!(out.exec_count, 2);
        assert_eq!(q.next(OK).id, b.id);
        q.complete(b.id, RELEASE);
        q.next(EMPTY);
        q.end();
        assert_eq!(q.get(c.id).deadline, 101);
        q.invariant();
    }

    #[test]
    fn close_remove_and_drain_defer_only_the_active_token() {
        let q = Owned::new();
        let a = q.add(0, 1, true, 1);
        let b = q.add(0, 2, false, 2);
        q.prime();
        assert_eq!(q.begin(10), OK);
        q.next(OK);
        assert_eq!(q.q().remove(a.id).0, DEFERRED);
        assert_eq!(q.get(a.id).removed, 1);
        assert_eq!(q.q().remove(a.id), (NOT_FOUND, TimerView::default()));
        assert!(!q.q().idle(), "release would be PENDING");
        q.q().close();
        q.next(BUSY);
        let (status, out) = q.q().drain();
        assert_eq!(status, RELEASE);
        assert_eq!(out.id, b.id);
        assert_eq!(q.q().drain(), (DEFERRED, TimerView::default()));
        assert_eq!(q.complete(a.id, RELEASE).token, a.token);
        q.next(CLOSING);
        assert_eq!(q.end(), -1);
        assert_eq!(q.q().drain().0, EMPTY);
        assert_eq!(q.begin(10), CLOSING);
    }

    #[test]
    fn ids_snapshot_never_partially_writes_and_survives_removal() {
        let q = Owned::new();
        let a = q.add(0, 2, false, 1);
        let b = q.add(0, 1, false, 2);
        let mut ids = [99, 99];
        let mut count = 99;
        assert_eq!(
            unsafe { swr_v1_timer_ids(Some(&q.0.borrow()), ptr::null_mut(), 0, &raw mut count) },
            CAPACITY
        );
        assert_eq!(count, 2);
        assert_eq!(
            unsafe { swr_v1_timer_ids(Some(&q.0.borrow()), ids.as_mut_ptr(), 1, &raw mut count) },
            CAPACITY
        );
        assert_eq!(ids, [99, 99]);
        assert_eq!(
            unsafe { swr_v1_timer_ids(Some(&q.0.borrow()), ids.as_mut_ptr(), 2, &raw mut count) },
            OK
        );
        ids.sort_unstable();
        assert_eq!(ids, [a.id, b.id]);
        for id in ids {
            assert_eq!(q.q().remove(id).0, RELEASE);
            q.invariant();
        }
        assert_eq!(
            unsafe { swr_v1_timer_ids(Some(&q.0.borrow()), ptr::null_mut(), 0, &raw mut count) },
            OK
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn checked_id_round_and_repeat_limits_are_terminal_without_wrap() {
        let q = Owned::new();
        q.q().last_id = i64::MAX - 1;
        assert_eq!(q.add(0, 1, false, 1).id, i64::MAX);
        let before = q.stats();
        assert_eq!(
            q.q().add(5, 1, false, ptr::dangling_mut()),
            (OVERFLOW, TimerView::default())
        );
        assert_eq!(q.stats(), before);
        assert_eq!(q.q().last_time, 0);
        let rounds = Owned::new();
        rounds.q().round = u64::MAX - 1;
        assert_eq!(rounds.begin(0), OK);
        rounds.end();
        assert_eq!(rounds.stats().round, u64::MAX);
        assert_eq!(rounds.begin(1), OVERFLOW);
        assert_eq!(rounds.q().last_time, 0);
        for execution_limit in [false, true] {
            let queue = Owned::new();
            let a = queue.add(0, 1, true, 1);
            queue.prime();
            if execution_limit {
                let heap = &mut queue.q().heap;
                let view = TimerView {
                    exec_count: u64::MAX,
                    ..*heap.first().unwrap()
                };
                heap.update(0, view);
            }
            assert_eq!(queue.begin(if execution_limit { 1 } else { i64::MAX }), OK);
            queue.next(OK);
            let released = queue.complete(a.id, RELEASE_OVERFLOW);
            assert_eq!(released.token, a.token);
            assert_eq!(queue.stats().count, 0);
            queue.end();
        }
    }

    #[test]
    fn indexed_heap_remains_consistent_under_seeded_updates_and_removals() {
        let q = Owned::new();
        let mut seed = 42u64;
        for step in 0..500 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            if step % 3 == 0 || q.stats().count == 0 {
                q.add(0, i64::try_from(seed % 100 + 1).unwrap(), false, step + 1);
            } else {
                let ids: Vec<i64> = q.0.borrow().positions.keys().copied().collect();
                let id = ids[usize::try_from(seed % ids.len() as u64).unwrap()];
                if step % 3 == 1 {
                    let deadline = i64::try_from(seed % 100).unwrap();
                    assert_eq!(q.q().update(id, deadline).0, OK);
                } else {
                    assert_eq!(q.q().remove(id).0, RELEASE);
                }
            }
            q.invariant();
        }
    }

    #[test]
    fn allocation_failures_preserve_id_time_entries_and_existing_views() {
        for nth in 0..=3 {
            let q = Owned::new();
            let mut queue = q.q();
            FAIL_AFTER.set(Some(nth));
            let (status, out) = queue.add(10, 5, false, ptr::dangling_mut());
            FAIL_AFTER.set(None);
            if nth < 3 {
                assert_eq!(status, NOMEM);
                assert_eq!(out, TimerView::default());
                assert_eq!(queue.stats().count, 0);
                assert_eq!(queue.last_id, 0);
                assert_eq!(queue.last_time, 0);
            } else {
                assert_eq!(status, OK);
                assert_eq!(out.id, 1);
            }
        }
        // Force hash-table growth while retaining an existing timer and a heap slot.
        let q = Owned::new();
        let first = q.add(0, 1, false, 1);
        while q.0.borrow().positions.len() < q.0.borrow().positions.capacity() {
            let token = usize::try_from(q.0.borrow().last_id).unwrap() + 1;
            q.add(0, 2, false, token);
        }
        q.q().heap.try_reserve(1).unwrap();
        let stats = q.stats();
        let last_id = q.q().last_id;
        FAIL_AFTER.set(Some(0));
        let added = q.q().add(99, 1, false, ptr::without_provenance_mut(999));
        FAIL_AFTER.set(None);
        assert_eq!(added, (NOMEM, TimerView::default()));
        assert_eq!(q.stats(), stats);
        assert_eq!(q.get(first.id), first);
        assert_eq!(q.q().last_id, last_id);
        assert_eq!(q.q().last_time, 0);
        q.invariant();
    }

    #[test]
    fn repairs_and_removals_do_not_allocate_at_full_index_capacity() {
        let q = Owned::new();
        q.add(0, 10, false, 1);
        while q.0.borrow().positions.len() < q.0.borrow().positions.capacity() {
            let token = usize::try_from(q.0.borrow().last_id).unwrap() + 1;
            q.add(0, 20, false, token);
        }
        let last = q.q().last_id;
        let mut queue = q.q();
        FAIL_AFTER.set(Some(0));
        let updated = queue.update(last, 0).0;
        let removed = queue.remove(1).0;
        let allocation_not_attempted = FAIL_AFTER.replace(None);
        drop(queue);
        assert_eq!(updated, OK);
        assert_eq!(removed, RELEASE);
        assert_eq!(allocation_not_attempted, Some(0));
        q.invariant();
    }

    #[test]
    fn seeded_lifecycle_matches_independent_ordered_map() {
        use std::collections::BTreeMap;
        let q = Owned::new();
        let mut model = BTreeMap::<i64, TimerView>::new();
        let mut seed = 0x5eed_u64;
        let (mut now, mut round) = (0, 0);
        for step in 0..8000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            match seed % 4 {
                0 => {
                    let delay = i64::try_from(seed % 47 + 1).unwrap();
                    let view = q.add(now, delay, seed & 16 != 0, step + 1);
                    assert_eq!(view.deadline, now + delay);
                    assert_eq!(view.birth_round, round);
                    model.insert(view.id, view);
                }
                1 | 2 if !model.is_empty() => {
                    let id = *model
                        .keys()
                        .nth(usize::try_from(seed % model.len() as u64).unwrap())
                        .unwrap();
                    if seed % 4 == 1 {
                        let (status, out) = q.q().remove(id);
                        assert_eq!(status, RELEASE);
                        assert_eq!(out, model.remove(&id).unwrap());
                    } else {
                        let deadline = now + i64::try_from(seed % 53).unwrap();
                        let (status, out) = q.q().update(id, deadline);
                        assert_eq!(status, OK);
                        model.get_mut(&id).unwrap().deadline = deadline;
                        assert_eq!(out, model[&id]);
                    }
                }
                _ => {
                    now += i64::try_from(seed % 7).unwrap();
                    assert_eq!(q.begin(now), OK);
                    loop {
                        let (status, out) = q.q().select_next();
                        if status == EMPTY {
                            if let Some(deadline) = model.values().map(|v| v.deadline).min() {
                                assert!(
                                    deadline > now
                                        || model
                                            .values()
                                            .any(|v| v.deadline == deadline
                                                && v.birth_round == round)
                                );
                            }
                            break;
                        }
                        assert_eq!(status, OK);
                        let old = model[&out.id];
                        assert_eq!(out, old);
                        assert_eq!(Some(old.deadline), model.values().map(|v| v.deadline).min());
                        assert!(old.deadline <= now && old.birth_round != round);
                        let completed =
                            q.complete(old.id, if old.interval == 0 { RELEASE } else { OK });
                        if old.interval == 0 {
                            assert_eq!(completed, model.remove(&old.id).unwrap());
                        } else {
                            let expected = model.get_mut(&old.id).unwrap();
                            while expected.deadline <= now {
                                expected.deadline += expected.interval;
                            }
                            expected.exec_count += 1;
                            assert_eq!(&completed, expected);
                        }
                    }
                    let delay = q.end();
                    round += 1;
                    assert_eq!(
                        delay,
                        model
                            .values()
                            .map(|v| (v.deadline - now).max(1))
                            .min()
                            .unwrap_or(-1)
                    );
                }
            }
            assert_eq!(q.stats().count, model.len());
            assert_eq!(q.stats().round, round);
            for (&id, expected) in &model {
                assert_eq!(q.get(id), *expected);
            }
            q.invariant();
        }
    }

    #[test]
    fn token_finds_live_timers_only() {
        let q = Owned::new();
        let view = q.add(0, 5, false, 7);
        assert_eq!(swr_v1_timer_token(Some(&q.0.borrow()), view.id), view.token);
        assert!(swr_v1_timer_token(Some(&q.0.borrow()), view.id + 1).is_null());
        assert!(swr_v1_timer_token(None, view.id).is_null());
        assert_eq!(q.q().remove(view.id).0, RELEASE);
        assert!(swr_v1_timer_token(Some(&q.0.borrow()), view.id).is_null());
    }

    #[test]
    fn stats_get_answers_the_queue_and_rejects_null() {
        let q = Owned::new();
        q.add(0, 1, false, 1);
        let mut stats = TimerStats {
            count: 7,
            ..TimerStats::default()
        };
        assert_eq!(
            unsafe { swr_v1_timer_stats_get(Some(&q.0.borrow()), &raw mut stats) },
            OK
        );
        assert_eq!(stats, q.stats());
        stats.count = 7;
        assert_eq!(
            unsafe { swr_v1_timer_stats_get(None, &raw mut stats) },
            INVALID
        );
        assert_eq!(stats, TimerStats::default());
        assert_eq!(
            unsafe { swr_v1_timer_stats_get(Some(&q.0.borrow()), ptr::null_mut()) },
            INVALID
        );
    }

    #[test]
    fn invalid_inputs_return_empty_views_and_do_not_change_selection() {
        let q = Owned::new();
        let original = q.add(0, 1, false, 1);
        assert_eq!(q.complete(original.id, INVALID), TimerView::default());
        assert_eq!(q.begin(-1), INVALID);
        assert_eq!(
            q.q().add(i64::MAX, 1, false, ptr::dangling_mut()),
            (OVERFLOW, TimerView::default())
        );
        assert_eq!(q.q().update(1, -1), (INVALID, TimerView::default()));
        assert_eq!(q.q().drain(), (INVALID, TimerView::default()));
        assert_eq!(q.get(original.id), original);
        q.prime();
        assert_eq!(q.begin(1), OK);
        assert_eq!(q.begin(100), BUSY);
        assert_eq!(q.q().captured, 1);
        q.next(OK);
        assert_eq!(q.q().end(), (BUSY, -1));
        q.complete(original.id, RELEASE);
        q.end();
    }
}
