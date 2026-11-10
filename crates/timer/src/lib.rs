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
use std::alloc::{Layout, alloc, dealloc};
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{BuildHasherDefault, Hasher};
use std::{mem, ptr};
#[cfg(feature = "upstream-symbols")]
pub use wrapper::{php, swr_v1_timer_add_plain, swr_v1_timer_add_plain_sec};

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
const PENDING: i32 = -8;

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

/// # Safety
/// Output must be valid, aligned and disjoint as specified in timer ABI v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_create(out: *mut *mut Timer) -> i32 {
    if out.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above), valid, aligned output.
    unsafe { out.write(ptr::null_mut()) };
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<Timer>()"
    )]
    // SAFETY: `Timer` is not zero-sized; null is handled below.
    let queue = unsafe { alloc(Layout::new::<Timer>()) }.cast::<Timer>();
    if queue.is_null() {
        return NOMEM;
    }
    // SAFETY: `queue` is a fresh, non-null allocation with `Timer`'s layout.
    unsafe { queue.write(Timer::default()) };
    // SAFETY: as for the first write to `out`.
    unsafe { out.write(queue) };
    OK
}

/// # Safety
/// Handle must be live and exclusively owned; release is exactly once on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_release(queue: *mut Timer) -> i32 {
    if queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, not concurrently mutated.
    let state = unsafe { &*queue };
    if !state.heap.is_empty() || state.selecting {
        return PENDING;
    }
    // SAFETY: a live, exclusively owned handle from create, released exactly once.
    unsafe { ptr::drop_in_place(queue) };
    // SAFETY: allocated with this layout by create; its value was just dropped.
    unsafe { dealloc(queue.cast(), Layout::new::<Timer>()) };
    OK
}

/// # Safety
/// Handle/output follow ABI validity/disjointness; token is a unique borrowed identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_add(
    queue: *mut Timer,
    now: i64,
    delay: i64,
    persistent: u32,
    token: *mut c_void,
    out: *mut TimerView,
) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() {
        return INVALID;
    }
    if now < 0 || delay <= 0 || persistent > 1 || token.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if state.closing {
        return CLOSING;
    }
    if now < state.last_time {
        return INVALID;
    }
    let Some(deadline) = now.checked_add(delay) else {
        return OVERFLOW;
    };
    let Some(id) = state.last_id.checked_add(1) else {
        return OVERFLOW;
    };
    if state.heap.len() >= (isize::MAX as usize) / mem::size_of::<TimerView>() {
        return OVERFLOW;
    }
    if state.heap.try_reserve(1).is_err() || state.positions.try_reserve(1).is_err() {
        return NOMEM;
    }
    let view = TimerView {
        id,
        deadline,
        interval: if persistent == 1 { delay } else { 0 },
        birth_round: state.round,
        token,
        ..TimerView::default()
    };
    let slot = state.heap.push(view);
    state.positions.insert(id, slot);
    state.last_id = id;
    state.last_time = now;
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    OK
}

/// # Safety
/// Handle and exclusive output satisfy ABI validity/disjointness rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_get(
    queue: *const Timer,
    id: i64,
    out: *mut TimerView,
) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() || id <= 0 {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, not concurrently mutated.
    let state = unsafe { &*queue };
    let Some((_, view)) = state.find(id) else {
        return NOT_FOUND;
    };
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    OK
}

/// The token `id` was added with, or null. Native code applies every view it is
/// handed to its token, so a lookup that only needs the token copies no view.
/// # Safety
/// The handle is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_token(queue: *const Timer, id: i64) -> *mut c_void {
    // SAFETY: null or a live handle from create, not concurrently mutated.
    let Some(state) = (unsafe { queue.as_ref() }) else {
        return ptr::null_mut();
    };
    state
        .index(id)
        .and_then(|index| state.heap.get(index))
        .map_or(ptr::null_mut(), |view| view.token)
}

/// # Safety
/// Handle and output are valid, exclusive and disjoint per ABI v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_update(
    queue: *mut Timer,
    id: i64,
    deadline: i64,
    out: *mut TimerView,
) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() || id <= 0 || deadline < 0 {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if state.closing {
        return CLOSING;
    }
    let Some((index, mut view)) = state.find(id) else {
        return NOT_FOUND;
    };
    if view.removed != 0 {
        return NOT_FOUND;
    }
    view.deadline = deadline;
    state.heap.update(index, view);
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    OK
}

/// # Safety
/// Handle/output are valid; only RELEASE transfers the returned token's custody.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_remove(
    queue: *mut Timer,
    id: i64,
    out: *mut TimerView,
) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() || id <= 0 {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    let Some((index, mut view)) = state.find(id) else {
        return NOT_FOUND;
    };
    if view.removed != 0 {
        return NOT_FOUND;
    }
    let status = if state.active_id == id {
        view.removed = 1;
        state.heap.update(index, view);
        DEFERRED
    } else if let Some(entry) = state.detach(index) {
        view = entry;
        RELEASE
    } else {
        return NOT_FOUND;
    };
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    status
}

/// # Safety
/// Handle is live and exclusively owned by its native thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_begin(queue: *mut Timer, now: i64) -> i32 {
    if queue.is_null() || now < 0 {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if state.closing {
        return CLOSING;
    }
    if state.selecting {
        return BUSY;
    }
    if now < state.last_time {
        return INVALID;
    }
    if state.round == u64::MAX {
        return OVERFLOW;
    }
    state.last_time = now;
    state.captured = now;
    state.selecting = true;
    OK
}

/// # Safety
/// Handle/output satisfy ABI v1; returned metadata does not transfer ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_next(queue: *mut Timer, out: *mut TimerView) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if !state.selecting {
        return INVALID;
    }
    if state.active_id != 0 {
        return BUSY;
    }
    if state.closing {
        return CLOSING;
    }
    let Some(view) = state.heap.first().copied() else {
        return EMPTY;
    };
    if view.deadline > state.captured || view.birth_round == state.round {
        return EMPTY;
    }
    state.active_id = view.id;
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    OK
}

/// # Safety
/// Exact active ID and valid disjoint output are required by ABI v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_complete(
    queue: *mut Timer,
    id: i64,
    out: *mut TimerView,
) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() || id <= 0 {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if !state.selecting || state.active_id != id {
        return INVALID;
    }
    let Some((index, mut view)) = state.find(id) else {
        return INVALID;
    };
    let mut next = Some(view.deadline);
    if view.deadline <= state.captured && view.interval > 0 {
        next = (state.captured - view.deadline)
            .checked_div(view.interval)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(view.interval))
            .and_then(|n| view.deadline.checked_add(n));
    }
    let executions = view.exec_count.checked_add(1);
    state.active_id = 0;
    let status = if view.removed != 0 || state.closing || view.interval == 0 {
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
        state.heap.update(index, view);
    } else if let Some(entry) = state.detach(index) {
        view = entry;
    }
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    status
}

/// # Safety
/// Handle and `next_delay` output are valid and disjoint as specified by ABI v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_end(queue: *mut Timer, next_delay: *mut i64) -> i32 {
    if next_delay.is_null() {
        return INVALID;
    }
    // SAFETY: `next_delay` is non-null (checked above), valid, aligned and writable.
    unsafe { next_delay.write(-1) };
    if queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if !state.selecting {
        return INVALID;
    }
    if state.active_id != 0 {
        return BUSY;
    }
    let Some(round) = state.round.checked_add(1) else {
        return OVERFLOW;
    };
    state.selecting = false;
    state.round = round;
    if !state.closing
        && let Some(head) = state.heap.first()
    {
        // SAFETY: `next_delay` is non-null (checked above), valid, aligned and writable.
        unsafe { next_delay.write((head.deadline - state.captured).max(1)) };
    }
    OK
}

/// # Safety
/// Handle and exclusive output must follow ABI v1 validity/disjointness rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_stats_get(queue: *const Timer, out: *mut TimerStats) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, not concurrently mutated.
    let state = unsafe { &*queue };
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe {
        out.write(TimerStats {
            count: state.heap.len(),
            round: state.round,
            active_id: state.active_id,
            selecting: u32::from(state.selecting),
            closing: u32::from(state.closing),
        });
    }
    OK
}

/// # Safety
/// Handle/count/ID storage must be valid, aligned and mutually disjoint per ABI v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_ids(
    queue: *const Timer,
    ids: *mut i64,
    capacity: usize,
    count: *mut usize,
) -> i32 {
    // SAFETY: `count` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(count) } || queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, not concurrently mutated.
    let state = unsafe { &*queue };
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

/// # Safety
/// Handle is live and exclusively owned; tokens and native callbacks remain native.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_close(queue: *mut Timer) -> i32 {
    if queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    state.closing = true;
    if let Some((index, mut view)) = state.find(state.active_id) {
        view.removed = 1;
        state.heap.update(index, view);
    }
    OK
}

/// # Safety
/// Handle/output are valid; only RELEASE transfers a token for native disposal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_timer_drain(queue: *mut Timer, out: *mut TimerView) -> i32 {
    // SAFETY: `out` is null or valid, aligned output disjoint from the handle.
    if !unsafe { zero(out) } || queue.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above); a live handle from create, exclusively accessed.
    let state = unsafe { &mut *queue };
    if !state.closing {
        return INVALID;
    }
    let Some(head) = state.heap.first() else {
        return EMPTY;
    };
    let Some(view) = state.detach(usize::from(head.id == state.active_id)) else {
        return DEFERRED;
    };
    // SAFETY: `out` is non-null (checked above), valid, aligned and writable.
    unsafe { out.write(view) };
    RELEASE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, System};
    use std::cell::Cell;

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
    struct Owned(*mut Timer);
    impl Owned {
        fn new() -> Self {
            let mut out = ptr::null_mut();
            assert_eq!(unsafe { swr_v1_timer_create(&raw mut out) }, OK);
            Self(out)
        }
        fn add(&self, now: i64, delay: i64, persistent: bool, token: usize) -> TimerView {
            let mut out = TimerView::default();
            assert_eq!(
                unsafe {
                    swr_v1_timer_add(
                        self.0,
                        now,
                        delay,
                        u32::from(persistent),
                        ptr::without_provenance_mut(token),
                        &raw mut out,
                    )
                },
                OK
            );
            out
        }
        fn get(&self, id: i64) -> TimerView {
            let mut out = TimerView::default();
            assert_eq!(unsafe { swr_v1_timer_get(self.0, id, &raw mut out) }, OK);
            out
        }
        fn stats(&self) -> TimerStats {
            let mut out = TimerStats::default();
            assert_eq!(unsafe { swr_v1_timer_stats_get(self.0, &raw mut out) }, OK);
            out
        }
        fn next(&self, expected: i32) -> TimerView {
            let mut out = TimerView::default();
            assert_eq!(unsafe { swr_v1_timer_next(self.0, &raw mut out) }, expected);
            out
        }
        fn complete(&self, id: i64, expected: i32) -> TimerView {
            let mut out = TimerView::default();
            assert_eq!(
                unsafe { swr_v1_timer_complete(self.0, id, &raw mut out) },
                expected
            );
            out
        }
        fn end(&self) -> i64 {
            let mut delay = 99;
            assert_eq!(unsafe { swr_v1_timer_end(self.0, &raw mut delay) }, OK);
            delay
        }
        fn prime(&self) {
            assert_eq!(unsafe { swr_v1_timer_begin(self.0, 0) }, OK);
            self.end();
        }
        fn invariant(&self) {
            let state = unsafe { &*self.0 };
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
            unsafe {
                assert_eq!(swr_v1_timer_close(self.0), OK);
                let stats = self.stats();
                if stats.active_id != 0 {
                    self.complete(stats.active_id, RELEASE);
                }
                if stats.selecting != 0 {
                    self.end();
                }
                let mut view = TimerView::default();
                while swr_v1_timer_drain(self.0, &raw mut view) == RELEASE {}
                assert_eq!(swr_v1_timer_release(self.0), OK);
            }
        }
    }

    #[test]
    fn equal_deadlines_fire_in_upstream_heap_order_and_birth_round_blocks_head() {
        let q = Owned::new();
        let a = q.add(0, 1, false, 1);
        let b = q.add(0, 1, false, 2);
        let c = q.add(0, 1, false, 3);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 1) }, OK);
        assert_eq!(q.next(EMPTY), TimerView::default());
        assert_eq!(q.end(), 1);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 1) }, OK);
        for expected in [a, c, b] {
            assert_eq!(q.next(OK), expected);
            assert_eq!(q.complete(expected.id, RELEASE), expected);
            q.invariant();
        }
        q.next(EMPTY);
        assert_eq!(q.end(), -1);
        // A callback-born head blocks an older due entry behind it.
        let old = q.add(1, 10, false, 4);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 20) }, OK);
        let young = q.add(20, 1, false, 5);
        let mut out = TimerView::default();
        assert_eq!(
            unsafe { swr_v1_timer_update(q.0, young.id, 0, &raw mut out) },
            OK
        );
        q.next(EMPTY);
        q.end();
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 20) }, OK);
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
            assert_eq!(unsafe { swr_v1_timer_begin(q.0, 10) }, OK);
            assert_eq!(q.next(OK).id, a.id);
            assert_eq!(q.complete(b.id, INVALID), TimerView::default());
            assert_eq!(q.stats().active_id, a.id);
            let mut out = TimerView::default();
            assert_eq!(
                unsafe { swr_v1_timer_update(q.0, a.id, 100, &raw mut out) },
                OK
            );
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
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 23) }, OK);
        assert_eq!(q.next(OK).id, a.id);
        let c = q.add(100, 1, false, 3); // Accepted add time does not alter captured 23.
        assert_eq!(q.complete(a.id, OK).deadline, 25);
        q.next(EMPTY);
        assert_eq!(q.end(), 2);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 99) }, INVALID);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 100) }, OK);
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
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 10) }, OK);
        q.next(OK);
        let mut out = TimerView::default();
        assert_eq!(
            unsafe { swr_v1_timer_remove(q.0, a.id, &raw mut out) },
            DEFERRED
        );
        assert_eq!(q.get(a.id).removed, 1);
        assert_eq!(
            unsafe { swr_v1_timer_remove(q.0, a.id, &raw mut out) },
            NOT_FOUND
        );
        assert_eq!(out, TimerView::default());
        assert_eq!(unsafe { swr_v1_timer_release(q.0) }, PENDING);
        assert_eq!(unsafe { swr_v1_timer_close(q.0) }, OK);
        q.next(BUSY);
        assert_eq!(unsafe { swr_v1_timer_drain(q.0, &raw mut out) }, RELEASE);
        assert_eq!(out.id, b.id);
        assert_eq!(unsafe { swr_v1_timer_drain(q.0, &raw mut out) }, DEFERRED);
        assert_eq!(out, TimerView::default());
        assert_eq!(q.complete(a.id, RELEASE).token, a.token);
        q.next(CLOSING);
        assert_eq!(q.end(), -1);
        assert_eq!(unsafe { swr_v1_timer_drain(q.0, &raw mut out) }, EMPTY);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 10) }, CLOSING);
    }

    #[test]
    fn ids_snapshot_never_partially_writes_and_survives_removal() {
        let q = Owned::new();
        let a = q.add(0, 2, false, 1);
        let b = q.add(0, 1, false, 2);
        let mut ids = [99, 99];
        let mut count = 99;
        assert_eq!(
            unsafe { swr_v1_timer_ids(q.0, ptr::null_mut(), 0, &raw mut count) },
            CAPACITY
        );
        assert_eq!(count, 2);
        assert_eq!(
            unsafe { swr_v1_timer_ids(q.0, ids.as_mut_ptr(), 1, &raw mut count) },
            CAPACITY
        );
        assert_eq!(ids, [99, 99]);
        assert_eq!(
            unsafe { swr_v1_timer_ids(q.0, ids.as_mut_ptr(), 2, &raw mut count) },
            OK
        );
        ids.sort_unstable();
        assert_eq!(ids, [a.id, b.id]);
        let mut out = TimerView::default();
        for id in ids {
            assert_eq!(
                unsafe { swr_v1_timer_remove(q.0, id, &raw mut out) },
                RELEASE
            );
            q.invariant();
        }
        assert_eq!(
            unsafe { swr_v1_timer_ids(q.0, ptr::null_mut(), 0, &raw mut count) },
            OK
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn checked_id_round_and_repeat_limits_are_terminal_without_wrap() {
        let q = Owned::new();
        unsafe {
            (*q.0).last_id = i64::MAX - 1;
        }
        assert_eq!(q.add(0, 1, false, 1).id, i64::MAX);
        let before = q.stats();
        let mut out = TimerView::default();
        assert_eq!(
            unsafe { swr_v1_timer_add(q.0, 5, 1, 0, ptr::dangling_mut(), &raw mut out) },
            OVERFLOW
        );
        assert_eq!(out, TimerView::default());
        assert_eq!(q.stats(), before);
        assert_eq!(unsafe { (*q.0).last_time }, 0);
        let rounds = Owned::new();
        unsafe {
            (*rounds.0).round = u64::MAX - 1;
        }
        assert_eq!(unsafe { swr_v1_timer_begin(rounds.0, 0) }, OK);
        rounds.end();
        assert_eq!(rounds.stats().round, u64::MAX);
        assert_eq!(unsafe { swr_v1_timer_begin(rounds.0, 1) }, OVERFLOW);
        assert_eq!(unsafe { (*rounds.0).last_time }, 0);
        for execution_limit in [false, true] {
            let queue = Owned::new();
            let a = queue.add(0, 1, true, 1);
            queue.prime();
            if execution_limit {
                let heap = unsafe { &mut (*queue.0).heap };
                let view = TimerView {
                    exec_count: u64::MAX,
                    ..*heap.first().unwrap()
                };
                heap.update(0, view);
            }
            assert_eq!(
                unsafe { swr_v1_timer_begin(queue.0, if execution_limit { 1 } else { i64::MAX }) },
                OK
            );
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
                let ids: Vec<i64> = unsafe { &*q.0 }.positions.keys().copied().collect();
                let id = ids[usize::try_from(seed % ids.len() as u64).unwrap()];
                let mut out = TimerView::default();
                if step % 3 == 1 {
                    assert_eq!(
                        unsafe {
                            swr_v1_timer_update(
                                q.0,
                                id,
                                i64::try_from(seed % 100).unwrap(),
                                &raw mut out,
                            )
                        },
                        OK
                    );
                } else {
                    assert_eq!(
                        unsafe { swr_v1_timer_remove(q.0, id, &raw mut out) },
                        RELEASE
                    );
                }
            }
            q.invariant();
        }
    }

    #[test]
    fn allocation_failures_preserve_id_time_entries_and_existing_views() {
        let mut handle = ptr::dangling_mut();
        FAIL_AFTER.set(Some(0));
        let status = unsafe { swr_v1_timer_create(&raw mut handle) };
        FAIL_AFTER.set(None);
        assert_eq!(status, NOMEM);
        assert!(handle.is_null());
        for nth in 0..=3 {
            let q = Owned::new();
            let mut out = TimerView::default();
            FAIL_AFTER.set(Some(nth));
            let status =
                unsafe { swr_v1_timer_add(q.0, 10, 5, 0, ptr::dangling_mut(), &raw mut out) };
            FAIL_AFTER.set(None);
            if nth < 3 {
                assert_eq!(status, NOMEM);
                assert_eq!(out, TimerView::default());
                assert_eq!(q.stats().count, 0);
                assert_eq!(unsafe { (*q.0).last_id }, 0);
                assert_eq!(unsafe { (*q.0).last_time }, 0);
            } else {
                assert_eq!(status, OK);
                assert_eq!(out.id, 1);
            }
        }
        // Force hash-table growth while retaining an existing timer and a heap slot.
        let q = Owned::new();
        let first = q.add(0, 1, false, 1);
        while unsafe { (*q.0).positions.len() < (*q.0).positions.capacity() } {
            q.add(0, 2, false, unsafe {
                usize::try_from((*q.0).last_id).unwrap() + 1
            });
        }
        unsafe {
            (*q.0).heap.try_reserve(1).unwrap();
        }
        let stats = q.stats();
        let last_id = unsafe { (*q.0).last_id };
        let mut out = first;
        FAIL_AFTER.set(Some(0));
        let status = unsafe {
            swr_v1_timer_add(
                q.0,
                99,
                1,
                0,
                ptr::without_provenance_mut(999),
                &raw mut out,
            )
        };
        FAIL_AFTER.set(None);
        assert_eq!(status, NOMEM);
        assert_eq!(out, TimerView::default());
        assert_eq!(q.stats(), stats);
        assert_eq!(q.get(first.id), first);
        assert_eq!(unsafe { (*q.0).last_id }, last_id);
        assert_eq!(unsafe { (*q.0).last_time }, 0);
        q.invariant();
    }

    #[test]
    fn repairs_and_removals_do_not_allocate_at_full_index_capacity() {
        let q = Owned::new();
        q.add(0, 10, false, 1);
        while unsafe { (*q.0).positions.len() < (*q.0).positions.capacity() } {
            q.add(0, 20, false, unsafe {
                usize::try_from((*q.0).last_id).unwrap() + 1
            });
        }
        let last = unsafe { (*q.0).last_id };
        let mut out = TimerView::default();
        FAIL_AFTER.set(Some(0));
        let updated = unsafe { swr_v1_timer_update(q.0, last, 0, &raw mut out) };
        let removed = unsafe { swr_v1_timer_remove(q.0, 1, &raw mut out) };
        let allocation_not_attempted = FAIL_AFTER.replace(None);
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
            let mut out = TimerView::default();
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
                        assert_eq!(
                            unsafe { swr_v1_timer_remove(q.0, id, &raw mut out) },
                            RELEASE
                        );
                        assert_eq!(out, model.remove(&id).unwrap());
                    } else {
                        let deadline = now + i64::try_from(seed % 53).unwrap();
                        assert_eq!(
                            unsafe { swr_v1_timer_update(q.0, id, deadline, &raw mut out) },
                            OK
                        );
                        model.get_mut(&id).unwrap().deadline = deadline;
                        assert_eq!(out, model[&id]);
                    }
                }
                _ => {
                    now += i64::try_from(seed % 7).unwrap();
                    assert_eq!(unsafe { swr_v1_timer_begin(q.0, now) }, OK);
                    loop {
                        let status = unsafe { swr_v1_timer_next(q.0, &raw mut out) };
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
        assert_eq!(unsafe { swr_v1_timer_token(q.0, view.id) }, view.token);
        assert!(unsafe { swr_v1_timer_token(q.0, view.id + 1) }.is_null());
        assert!(unsafe { swr_v1_timer_token(ptr::null(), view.id) }.is_null());
        let mut out = TimerView::default();
        assert_eq!(
            unsafe { swr_v1_timer_remove(q.0, view.id, &raw mut out) },
            RELEASE
        );
        assert!(unsafe { swr_v1_timer_token(q.0, view.id) }.is_null());
    }

    #[test]
    fn invalid_inputs_zero_outputs_and_do_not_change_selection() {
        let q = Owned::new();
        let mut view = q.add(0, 1, false, 1);
        let original = view;
        assert_eq!(
            unsafe { swr_v1_timer_get(ptr::null(), 1, &raw mut view) },
            INVALID
        );
        assert_eq!(view, TimerView::default());
        assert_eq!(
            unsafe { swr_v1_timer_complete(q.0, original.id, &raw mut view) },
            INVALID
        );
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, -1) }, INVALID);
        assert_eq!(
            unsafe { swr_v1_timer_add(q.0, i64::MAX, 1, 0, ptr::dangling_mut(), &raw mut view) },
            OVERFLOW
        );
        assert_eq!(
            unsafe { swr_v1_timer_add(q.0, 0, 1, 2, ptr::dangling_mut(), &raw mut view) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_timer_update(q.0, 1, -1, &raw mut view) },
            INVALID
        );
        assert_eq!(unsafe { swr_v1_timer_drain(q.0, &raw mut view) }, INVALID);
        assert_eq!(q.get(original.id), original);
        q.prime();
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 1) }, OK);
        assert_eq!(unsafe { swr_v1_timer_begin(q.0, 100) }, BUSY);
        assert_eq!(unsafe { (*q.0).captured }, 1);
        q.next(OK);
        let mut delay = 123;
        assert_eq!(unsafe { swr_v1_timer_end(q.0, &raw mut delay) }, BUSY);
        assert_eq!(delay, -1);
        q.complete(original.id, RELEASE);
        q.end();
    }
}
