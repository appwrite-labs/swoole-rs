//! Ordered callback metadata only. Native owns every `std::function` payload and
//! performs its copy, call and destruction after each Rust function returns.
//! Native walks each kind's entries through `views`, which every mutation here
//! keeps current, so a walk makes no call per callback (#680).
use crate::owner::{publish, release as release_metadata};
use std::ptr;

/// `swr_v1_reactor_callback_entry`.
#[repr(C)]
pub struct CallbackEntry {
    pub key: i32,
    pub token: usize,
}
/// `swr_v1_reactor_callback_view`: one kind's entries, in ascending key order.
#[repr(C)]
pub struct CallbackView {
    pub entries: *const CallbackEntry,
    pub len: usize,
}
/// Native reads `views` (`struct swr_v1_reactor_callbacks`), so they come first.
#[repr(C)]
pub struct ReactorCallbacks {
    views: [CallbackView; 2],
    entries: [Vec<CallbackEntry>; 2],
}
impl ReactorCallbacks {
    fn new() -> Self {
        let empty = || CallbackView {
            entries: ptr::null(),
            len: 0,
        };
        Self {
            views: [empty(), empty()],
            entries: [Vec::new(), Vec::new()],
        }
    }
    /// Publish `kind`'s entries after a change to them.
    fn refresh(&mut self, kind: usize) {
        if let (Some(view), Some(entries)) = (self.views.get_mut(kind), self.entries.get(kind)) {
            *view = CallbackView {
                entries: entries.as_ptr(),
                len: entries.len(),
            };
        }
    }
    fn lookup(&self, kind: u32, key: i32) -> Option<usize> {
        let entries = self.entries.get(kind as usize)?;
        entries
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .and_then(|index| entries.get(index))
            .map(|entry| entry.token)
    }
    fn insert(&mut self, kind: u32, key: i32, token: usize) -> Result<(i32, usize), i32> {
        let entries = self.entries.get_mut(kind as usize).ok_or(-1)?;
        match entries.binary_search_by_key(&key, |entry| entry.key) {
            Ok(index) => entries.get(index).map(|entry| (0, entry.token)).ok_or(-1),
            Err(index) => {
                entries.try_reserve(1).map_err(|_| -2)?;
                entries.insert(index, CallbackEntry { key, token });
                self.refresh(kind as usize);
                Ok((1, token))
            }
        }
    }
    fn erase(&mut self, kind: usize, key: i32) -> Option<usize> {
        let entries = self.entries.get_mut(kind)?;
        let index = entries.binary_search_by_key(&key, |entry| entry.key).ok()?;
        let token = entries.remove(index).token;
        self.refresh(kind);
        Some(token)
    }
    fn drain(&mut self, kind: &mut u32, token: &mut usize) -> i32 {
        *kind = u32::from(!self.entries[1].is_empty());
        let entry = self.entries.get_mut(*kind as usize).and_then(Vec::pop);
        self.refresh(*kind as usize);
        if let Some(entry) = entry {
            *token = entry.token;
            1
        } else {
            0
        }
    }
}

///1 found/0 absent/-1 invalid. A null owner is a lazy empty store.
/// # Safety
/// Owner is null or live, and output is aligned/exclusive/disjoint. No token is
/// dereferenced; no reference escapes this call or spans a Native effect.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_lookup(
    owner: *const ReactorCallbacks,
    kind: u32,
    key: i32,
    token: *mut usize,
) -> i32 {
    // SAFETY: `token` is null or live, aligned and exclusively lent for this call.
    let Some(token) = (unsafe { token.as_mut() }) else {
        return -1;
    };
    if kind > 1 {
        return -1;
    }
    *token = 0;
    // SAFETY: `owner` is null or live and aligned for this call.
    let Some(owner) = (unsafe { owner.as_ref() }) else {
        return 0;
    };
    if let Some(found) = owner.lookup(kind, key) {
        *token = found;
        1
    } else {
        0
    }
}

// Only the opaque slot's live-pointer admission remains unsafe. All actual
// ordered-container operations compose safe methods, never exported C calls.
unsafe fn insert_metadata(
    slot: &mut *mut ReactorCallbacks,
    kind: u32,
    key: i32,
    token: usize,
    actual: &mut usize,
) -> i32 {
    // SAFETY: the caller passes its live, exclusive slot, holding null or this owner type.
    unsafe {
        publish(slot, ReactorCallbacks::new, |owner| {
            let (status, value) = owner.insert(kind, key, token)?;
            *actual = value;
            Ok((status, status == 1))
        })
    }
    .unwrap_or_else(|status| status)
}

/// Detach metadata first; Native destroys the returned payload after return.
/// # Safety
/// Same live/exclusive owner/output contract as insertion. Native token ownership
/// transfers only on result1. Rust never drops or calls the token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_erase(
    owner: *mut ReactorCallbacks,
    kind: u32,
    key: i32,
    token: *mut usize,
) -> i32 {
    // SAFETY: `token` is null or live, aligned and exclusively lent for this call.
    let Some(token) = (unsafe { token.as_mut() }) else {
        return -1;
    };
    if kind > 1 {
        return -1;
    }
    *token = 0;
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return 0;
    };
    owner.erase(kind as usize, key).map_or(0, |found| {
        *token = found;
        1
    })
}

/// Where a walk goes on after a callback changed `kind`'s entries: the index of
/// the first entry after `key`, so entries added after it run and erased ones
/// don't. `usize::MAX` for an invalid kind.
/// # Safety
/// Owner is null or live and aligned for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_after(
    owner: *const ReactorCallbacks,
    kind: u32,
    key: i32,
) -> usize {
    if kind > 1 {
        return usize::MAX;
    }
    // SAFETY: `owner` is null or live and aligned for this call.
    unsafe { owner.as_ref() }
        .and_then(|owner| owner.entries.get(kind as usize))
        .map_or(0, |entries| {
            entries.partition_point(|entry| entry.key <= key)
        })
}

/// Complete empty-payload publication/retry policy. Candidate allocation and
/// destruction/copy remain Native effects between calls. 1 existing token,
/// 2 allocate candidate, 3 committed candidate, 0 discard candidate then retry.
/// -2 capacity refusal, -1 invalid protocol. A zero candidate requests lookup.
/// # Safety
/// The slot/output follow `callbacks_insert`; a nonzero candidate is a fresh live
/// Native payload token exclusively retained until action3 transfers ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_ensure(
    slot: *mut *mut ReactorCallbacks,
    kind: u32,
    key: i32,
    candidate: usize,
    token: *mut usize,
) -> i32 {
    // SAFETY: `slot` is null or live, aligned and exclusively lent for this call.
    let slot = unsafe { slot.as_mut() };
    // SAFETY: `token` is null or live, aligned and exclusively lent for this call.
    let token = unsafe { token.as_mut() };
    let (Some(slot), Some(token)) = (slot, token) else {
        return -1;
    };
    if kind > 1 {
        return -1;
    }
    *token = 0;
    if candidate == 0 {
        // SAFETY: the slot holds null or this module's live owner, exclusively accessed.
        match unsafe { (*slot).as_ref() }.and_then(|owner| owner.lookup(kind, key)) {
            Some(found) => {
                *token = found;
                1
            }
            None => 2,
        }
    } else {
        // SAFETY: `slot` is the caller's live, exclusive opaque owner slot.
        match unsafe { insert_metadata(slot, kind, key, candidate, token) } {
            1 => 3,
            other => other,
        }
    }
}

/// Detach one payload for Native typed deletion, or release the empty metadata
/// and publish null. 1 delete end, 2 delete exit, 0 complete, -1 invalid.
/// # Safety
/// Slot/output are live/aligned/exclusive/disjoint; each returned Native token
/// is deleted after return before another call. No custom destruction/reentry
/// contract is introduced and no Rust reference survives that Native effect.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_wrap,
    reason = "native int callback and action codes"
)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_clear_next(
    slot: *mut *mut ReactorCallbacks,
    token: *mut usize,
) -> i32 {
    // SAFETY: `slot` is null or live, aligned and exclusively lent for this call.
    let Some(slot) = (unsafe { slot.as_mut() }) else {
        return -1;
    };
    // SAFETY: `token` is null or live, aligned and exclusively lent for this call.
    let Some(token) = (unsafe { token.as_mut() }) else {
        return -1;
    };
    *token = 0;
    let mut kind = 0;
    // SAFETY: the slot holds null or this module's live owner, exclusively accessed.
    let action = unsafe { (*slot).as_mut() }.map_or(0, |owner| owner.drain(&mut kind, token));
    match action {
        1 => kind as i32 + 1,
        0 => {
            // SAFETY: the drained, live owner is released once; the slot is cleared next.
            unsafe { release_metadata(*slot) };
            *slot = ptr::null_mut();
            0
        }
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    unsafe fn view(owner: *const ReactorCallbacks, kind: usize) -> Vec<(i32, usize)> {
        let view = unsafe { &(*owner).views[kind] };
        (0..view.len)
            .map(|index| {
                let entry = unsafe { &*view.entries.add(index) };
                (entry.key, entry.token)
            })
            .collect()
    }

    unsafe fn insert(owner: &mut *mut ReactorCallbacks, kind: u32, key: i32, value: usize) {
        let mut token = 0;
        unsafe {
            assert_eq!(
                swr_v1_reactor_callbacks_ensure(owner, kind, key, 0, &raw mut token),
                2
            );
            assert_eq!(
                swr_v1_reactor_callbacks_ensure(owner, kind, key, value, &raw mut token),
                3
            );
            assert_eq!(token, value);
            assert_eq!(
                swr_v1_reactor_callbacks_ensure(owner, kind, key, 0, &raw mut token),
                1
            );
            assert_eq!(token, value);
        }
    }

    #[test]
    fn views_follow_insertion_erasure_and_clear() {
        assert_eq!(std::mem::offset_of!(ReactorCallbacks, views), 0);
        unsafe {
            let mut owner = ptr::null_mut();
            assert_eq!(swr_v1_reactor_callbacks_after(owner, 0, 1), 0);
            for (kind, key, value) in [(0, 3, 103), (0, 1, 101), (1, 2, 202), (0, 5, 105)] {
                insert(&mut owner, kind, key, value);
            }
            assert_eq!(view(owner, 0), [(1, 101), (3, 103), (5, 105)]);
            assert_eq!(view(owner, 1), [(2, 202)]);
            let mut token = 0;
            assert_eq!(
                swr_v1_reactor_callbacks_erase(owner, 0, 3, &raw mut token),
                1
            );
            assert_eq!(token, 103);
            assert_eq!(
                swr_v1_reactor_callbacks_erase(owner, 0, 3, &raw mut token),
                0
            );
            assert_eq!(view(owner, 0), [(1, 101), (5, 105)]);
            // A walk that was at key 3 goes on at key 5, and at key 0 at key 1.
            assert_eq!(swr_v1_reactor_callbacks_after(owner, 0, 3), 1);
            assert_eq!(swr_v1_reactor_callbacks_after(owner, 0, 0), 0);
            assert_eq!(swr_v1_reactor_callbacks_after(owner, 0, 5), 2);
            assert_eq!(swr_v1_reactor_callbacks_after(owner, 2, 5), usize::MAX);
            let mut released = Vec::new();
            loop {
                let action = swr_v1_reactor_callbacks_clear_next(&raw mut owner, &raw mut token);
                if action == 0 {
                    break;
                }
                if !owner.is_null() {
                    let kind = usize::from(action == 2);
                    assert!(
                        view(owner, kind as usize)
                            .iter()
                            .all(|&(_, value)| value != token)
                    );
                }
                released.push((action, token));
            }
            assert_eq!(released, [(2, 202), (1, 105), (1, 101)]);
            assert!(owner.is_null());
        }
    }

    type End = Rc<dyn Fn(*mut Reactor)>;
    type Exit = Rc<dyn Fn(*mut Reactor, &mut usize) -> bool>;

    thread_local! {
        static TRACE: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    }
    fn push(value: i32) {
        TRACE.with_borrow_mut(|trace| trace.push(value));
    }
    fn take() -> Vec<i32> {
        TRACE.with_borrow_mut(std::mem::take)
    }

    /// event.cc's `ReactorCallbacks` glue over these exports, on the former callbacks
    /// gate's Reactor facade (`get_event_num()` is 5). Payloads are boxed
    /// `Option<End>`/`Option<Exit>`, the empty `std::function` a new entry starts as.
    struct Reactor {
        owner: *mut ReactorCallbacks,
        timed_out: bool,
    }
    impl Reactor {
        fn new() -> Self {
            Self {
                owner: ptr::null_mut(),
                timed_out: false,
            }
        }
        fn ensure<F>(&mut self, kind: u32, key: i32) -> *mut Option<F> {
            let mut candidate: Option<Box<Option<F>>> = None;
            loop {
                let offered = candidate
                    .as_deref_mut()
                    .map_or(0, |c| ptr::from_mut(c) as usize);
                let mut token = 0;
                match unsafe {
                    swr_v1_reactor_callbacks_ensure(
                        &raw mut self.owner,
                        kind,
                        key,
                        offered,
                        &raw mut token,
                    )
                } {
                    0 => candidate = None,
                    1 => return token as *mut Option<F>,
                    2 => candidate = Some(Box::new(None)),
                    3 => return Box::into_raw(candidate.take().unwrap()),
                    other => panic!("ensure {other}"),
                }
            }
        }
        fn set_end_callback(&mut self, key: i32, f: impl Fn(*mut Reactor) + 'static) {
            unsafe { *self.ensure::<End>(0, key) = Some(Rc::new(f)) };
        }
        fn set_exit_condition(
            &mut self,
            key: i32,
            f: impl Fn(*mut Reactor, &mut usize) -> bool + 'static,
        ) {
            unsafe { *self.ensure::<Exit>(1, key) = Some(Rc::new(f)) };
        }
        fn erase(&mut self, kind: u32, key: i32) -> usize {
            let mut token = 0;
            let result =
                unsafe { swr_v1_reactor_callbacks_erase(self.owner, kind, key, &raw mut token) };
            if result == 1 {
                if kind == 0 {
                    drop(unsafe { Box::from_raw(token as *mut Option<End>) });
                } else {
                    drop(unsafe { Box::from_raw(token as *mut Option<Exit>) });
                }
            }
            usize::try_from(result).unwrap()
        }
        fn erase_end_callback(&mut self, key: i32) {
            self.erase(0, key);
        }
        fn remove_exit_condition(&mut self, key: i32) -> usize {
            self.erase(1, key)
        }
        fn isset_exit_condition(&self, key: i32) -> bool {
            let mut token = 0;
            unsafe { swr_v1_reactor_callbacks_lookup(self.owner, 1, key, &raw mut token) == 1 }
        }
        fn execute_end_callbacks(r: *mut Self, timed_out: bool) {
            unsafe { (*r).timed_out = timed_out };
            walk(r, 0, |_, token| {
                let f = unsafe { (*(token as *const Option<End>)).clone().unwrap() };
                f(r);
                true
            });
        }
        fn if_exit(r: *mut Self) -> bool {
            let mut event_num = 5;
            walk(r, 1, |_, token| {
                let f = unsafe { (*(token as *const Option<Exit>)).clone().unwrap() };
                f(r, &mut event_num)
            })
        }
        fn copy(source: *mut Self) -> Self {
            let mut copied = Self::new();
            walk(source, 0, |key, token| {
                let f = unsafe { (*(token as *const Option<End>)).clone() };
                unsafe { *copied.ensure::<End>(0, key) = f };
                true
            });
            walk(source, 1, |key, token| {
                let f = unsafe { (*(token as *const Option<Exit>)).clone() };
                unsafe { *copied.ensure::<Exit>(1, key) = f };
                true
            });
            copied
        }
        /// `operator=`: copy, then swap owners; self-assignment does nothing.
        fn assign(r: *mut Self, source: *mut Self) {
            if r != source {
                let mut copied = Self::copy(source);
                unsafe { std::mem::swap(&mut (*r).owner, &mut copied.owner) };
            }
        }
    }
    impl Drop for Reactor {
        fn drop(&mut self) {
            loop {
                let mut token = 0;
                match unsafe {
                    swr_v1_reactor_callbacks_clear_next(&raw mut self.owner, &raw mut token)
                } {
                    0 => return,
                    1 => drop(unsafe { Box::from_raw(token as *mut Option<End>) }),
                    2 => drop(unsafe { Box::from_raw(token as *mut Option<Exit>) }),
                    other => panic!("clear {other}"),
                }
            }
        }
    }
    /// event.cc's `swr_reactor_callbacks_walk`: entries in key order, re-read after
    /// each call, stopping at the first false.
    fn walk(r: *mut Reactor, kind: u32, mut call: impl FnMut(i32, usize) -> bool) -> bool {
        let mut i = 0;
        loop {
            let owner = unsafe { (*r).owner };
            if owner.is_null() {
                return true;
            }
            let Some(&(key, token)) = unsafe { view(owner, kind as usize) }.get(i) else {
                return true;
            };
            if !call(key, token) {
                return false;
            }
            let owner = unsafe { (*r).owner };
            let entries = unsafe { view(owner, kind as usize) };
            i = if entries.get(i).is_some_and(|entry| entry.0 == key) {
                i + 1
            } else {
                unsafe { swr_v1_reactor_callbacks_after(owner, kind, key) }
            };
        }
    }

    /// The former callbacks gate: an empty store, end callbacks in key order with
    /// replacement and erasure.
    #[test]
    fn former_callbacks_gate_end_order() {
        let mut empty = Reactor::new();
        let e: *mut Reactor = &raw mut empty;
        assert!(Reactor::if_exit(e));
        Reactor::execute_end_callbacks(e, false);
        assert!(!empty.timed_out);
        assert!(!empty.isset_exit_condition(1));
        assert_eq!(empty.remove_exit_condition(1), 0);
        empty.erase_end_callback(1);

        let mut reactor = Reactor::new();
        let r: *mut Reactor = &raw mut reactor;
        reactor.set_end_callback(5, |p| {
            assert!(unsafe { (*p).timed_out });
            push(5);
        });
        reactor.set_end_callback(1, |_| push(1));
        reactor.set_end_callback(3, |_| push(3));
        Reactor::execute_end_callbacks(r, true);
        assert_eq!(take(), [1, 3, 5]);
        reactor.set_end_callback(3, |_| push(99));
        Reactor::execute_end_callbacks(r, true);
        assert_eq!(take(), [1, 99, 5]);
        reactor.erase_end_callback(3);
        reactor.erase_end_callback(7);
        Reactor::execute_end_callbacks(r, true);
        assert_eq!(take(), [1, 5]);
    }

    /// The former callbacks gate: an end callback that adds after and before itself
    /// and erases a later one.
    #[test]
    fn former_callbacks_gate_end_mutation_during_walk() {
        let mut reactor = Reactor::new();
        let r: *mut Reactor = &raw mut reactor;
        reactor.set_end_callback(1, |p| {
            push(1);
            let p = unsafe { &mut *p };
            p.set_end_callback(2, |_| push(2));
            p.set_end_callback(0, |_| push(0));
            p.erase_end_callback(3);
        });
        reactor.set_end_callback(3, |_| push(3));
        reactor.set_end_callback(5, |_| push(5));
        Reactor::execute_end_callbacks(r, false);
        assert_eq!(take(), [1, 2, 5]);
        assert!(!reactor.timed_out);
    }

    /// The former callbacks gate: exit conditions share the event count, stop at the
    /// first false, and see additions and removals made during the walk.
    #[test]
    fn former_callbacks_gate_exit_conditions() {
        let mut reactor = Reactor::new();
        let r: *mut Reactor = &raw mut reactor;
        reactor.set_exit_condition(3, |_, n| {
            push(3);
            assert_eq!(*n, 12);
            true
        });
        reactor.set_exit_condition(1, |_, n| {
            push(1);
            assert_eq!(*n, 5);
            *n += 7;
            true
        });
        reactor.set_exit_condition(2, |_, n| {
            push(2);
            assert_eq!(*n, 12);
            false
        });
        assert!(reactor.isset_exit_condition(2));
        assert!(!Reactor::if_exit(r));
        assert_eq!(take(), [1, 2]);
        assert_eq!(reactor.remove_exit_condition(2), 1);
        assert_eq!(reactor.remove_exit_condition(2), 0);
        assert!(!reactor.isset_exit_condition(2));
        assert!(Reactor::if_exit(r));
        assert_eq!(take(), [1, 3]);

        let mut mutating = Reactor::new();
        let m: *mut Reactor = &raw mut mutating;
        mutating.set_exit_condition(1, |p, n| {
            push(1);
            let p = unsafe { &mut *p };
            p.set_exit_condition(2, |_, count| {
                push(2);
                assert_eq!(*count, 6);
                true
            });
            p.remove_exit_condition(3);
            *n += 1;
            true
        });
        mutating.set_exit_condition(3, |_, _| {
            push(3);
            false
        });
        assert!(Reactor::if_exit(m));
        assert_eq!(take(), [1, 2]);
    }

    /// The former callbacks gate: a copied reactor's callbacks are its own, and
    /// assignment (including to itself) copies them back.
    #[test]
    fn former_callbacks_gate_copy_and_assign() {
        let mut original = Reactor::new();
        let o: *mut Reactor = &raw mut original;
        original.set_end_callback(1, |_| push(1));
        original.set_exit_condition(1, |_, n| *n == 5);
        let mut copied = Reactor::copy(o);
        let c: *mut Reactor = &raw mut copied;
        assert_eq!(copied.remove_exit_condition(1), 1);
        assert!(original.isset_exit_condition(1));
        assert!(!copied.isset_exit_condition(1));
        copied.set_end_callback(1, |_| push(2));
        Reactor::execute_end_callbacks(o, false);
        Reactor::execute_end_callbacks(c, false);
        assert_eq!(take(), [1, 2]);
        Reactor::assign(c, o);
        assert!(copied.isset_exit_condition(1));
        assert!(Reactor::if_exit(c));
        Reactor::execute_end_callbacks(c, false);
        assert_eq!(take(), [1]);
        Reactor::assign(c, c);
        Reactor::execute_end_callbacks(c, false);
        assert_eq!(take(), [1]);
    }
}
