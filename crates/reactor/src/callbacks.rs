//! Ordered callback metadata only. Native owns every `std::function` payload and
//! performs its copy, call and destruction after each Rust function returns.
//! Native walks each kind's entries through `views`, which every mutation here
//! keeps current, so a walk makes no call per callback (#680).
use crate::owner::{publish, release as release_metadata};
use std::ptr;

/// `swr_v1_reactor_callback_entry`.
#[repr(C)]
pub struct Entry {
    pub key: i32,
    pub token: usize,
}
/// `swr_v1_reactor_callback_view`: one kind's entries, in ascending key order.
#[repr(C)]
pub struct View {
    pub entries: *const Entry,
    pub len: usize,
}
/// Native reads `views` (`struct swr_v1_reactor_callbacks`), so they come first.
#[repr(C)]
pub struct ReactorCallbacks {
    views: [View; 2],
    entries: [Vec<Entry>; 2],
}
impl ReactorCallbacks {
    fn new() -> Self {
        let empty = || View {
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
        let entries = &self.entries[kind];
        self.views[kind] = View {
            entries: entries.as_ptr(),
            len: entries.len(),
        };
    }
    fn lookup(&self, kind: u32, key: i32) -> Option<usize> {
        let entries = self.entries.get(kind as usize)?;
        entries
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .map(|index| entries[index].token)
    }
    fn insert(&mut self, kind: u32, key: i32, token: usize) -> Result<(i32, usize), i32> {
        let entries = self.entries.get_mut(kind as usize).ok_or(-1)?;
        match entries.binary_search_by_key(&key, |entry| entry.key) {
            Ok(index) => Ok((0, entries[index].token)),
            Err(index) => {
                entries.try_reserve(1).map_err(|_| -2)?;
                entries.insert(index, Entry { key, token });
                self.refresh(kind as usize);
                Ok((1, token))
            }
        }
    }
    fn erase(&mut self, kind: usize, key: i32) -> Option<usize> {
        let entries = &mut self.entries[kind];
        let index = entries.binary_search_by_key(&key, |entry| entry.key).ok()?;
        let token = entries.remove(index).token;
        self.refresh(kind);
        Some(token)
    }
    fn drain(&mut self, kind: &mut u32, token: &mut usize) -> i32 {
        *kind = u32::from(!self.entries[1].is_empty());
        let entry = self.entries[*kind as usize].pop();
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
    unsafe { owner.as_ref() }.map_or(0, |owner| {
        owner.entries[kind as usize].partition_point(|entry| entry.key <= key)
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
                    assert!(view(owner, kind).iter().all(|&(_, value)| value != token));
                }
                released.push((action, token));
            }
            assert_eq!(released, [(2, 202), (1, 105), (1, 101)]);
            assert!(owner.is_null());
        }
    }
}
