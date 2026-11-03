//! Ordered callback metadata only. Native owns every `std::function` payload and
//! performs its copy, call and destruction after each Rust function returns.
use crate::owner::{publish, release as release_metadata};
use std::ptr;

struct Entry {
    key: i32,
    token: usize,
}
pub struct ReactorCallbacks {
    entries: [Vec<Entry>; 2],
}
impl ReactorCallbacks {
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
                Ok((1, token))
            }
        }
    }
    fn drain(&mut self, kind: &mut u32, token: &mut usize) -> i32 {
        let entry = if let Some(entry) = self.entries[1].pop() {
            *kind = 1;
            Some(entry)
        } else {
            *kind = 0;
            self.entries[0].pop()
        };
        if let Some(entry) = entry {
            *token = entry.token;
            1
        } else {
            0
        }
    }
}
#[repr(C)]
#[derive(Default)]
pub struct ReactorCallbackCursor {
    pub key: i32,
    ///0 initial,1 previous callback returned,2 complete true,3 complete false.
    pub phase: u32,
    /// Where `key` was; reused while it still holds `key`, so the walk is O(1) per step.
    pub index: usize,
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
        publish(
            slot,
            || ReactorCallbacks {
                entries: [Vec::new(), Vec::new()],
            },
            |owner| {
                let (status, value) = owner.insert(kind, key, token)?;
                *actual = value;
                Ok((status, status == 1))
            },
        )
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
    let entries = &mut owner.entries[kind as usize];
    match entries.binary_search_by_key(&key, |entry| entry.key) {
        Ok(index) => {
            *token = entries.remove(index).token;
            1
        }
        Err(_) => 0,
    }
}

/// Fresh ascending-key traversal after each Native callback. Insertions after
/// the current key become visible; removed future keys disappear. No iterator,
/// generation snapshot or borrowed payload reference survives return.
///0 complete true/1 call returned token/2 exit rejected/-1 invalid.
/// # Safety
/// Owner is null/live, cursor/output live, aligned/exclusive/disjoint. Cursor is
/// zero initialized per traversal. Native callback result is supplied freshly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_next(
    owner: *const ReactorCallbacks,
    kind: u32,
    cursor: *mut ReactorCallbackCursor,
    previous: u32,
    token: *mut usize,
) -> i32 {
    // SAFETY: `cursor` is null or live, aligned and exclusively lent for this call.
    let cursor = unsafe { cursor.as_mut() };
    // SAFETY: `token` is null or live, aligned and exclusively lent for this call.
    let token = unsafe { token.as_mut() };
    let (Some(cursor), Some(token)) = (cursor, token) else {
        return -1;
    };
    if kind > 1 || previous > 1 || cursor.phase > 3 {
        return -1;
    }
    // SAFETY: `owner` is null or live and aligned for this call.
    next_metadata(unsafe { owner.as_ref() }, kind, cursor, previous, token)
}

#[inline]
fn next_metadata(
    owner: Option<&ReactorCallbacks>,
    kind: u32,
    cursor: &mut ReactorCallbackCursor,
    previous: u32,
    token: &mut usize,
) -> i32 {
    // The common step: an end callback returned and the walk goes on to the next entry.
    if kind == 0
        && cursor.phase == 1
        && let Some(owner) = owner
    {
        let entries = &owner.entries[0];
        if entries
            .get(cursor.index)
            .is_some_and(|entry| entry.key == cursor.key)
        {
            return if let Some(entry) = entries.get(cursor.index + 1) {
                cursor.key = entry.key;
                cursor.index += 1;
                *token = entry.token;
                1
            } else {
                cursor.phase = 2;
                *token = 0;
                0
            };
        }
    }
    *token = 0;
    if cursor.phase == 3 {
        return 2;
    }
    if cursor.phase == 2 {
        return 0;
    }
    if kind == 1 && cursor.phase == 1 && previous == 0 {
        cursor.phase = 3;
        return 2;
    }
    let Some(owner) = owner else {
        cursor.phase = 2;
        return 0;
    };
    let entries = &owner.entries[kind as usize];
    let index = if cursor.phase == 0 {
        0
    } else if entries
        .get(cursor.index)
        .is_some_and(|entry| entry.key == cursor.key)
    {
        // Keys are unique and sorted, so the next key is the next entry.
        cursor.index + 1
    } else {
        // A callback changed the map before the current key; search afresh.
        entries.partition_point(|entry| entry.key <= cursor.key)
    };
    if let Some(entry) = entries.get(index) {
        cursor.key = entry.key;
        cursor.index = index;
        cursor.phase = 1;
        *token = entry.token;
        1
    } else {
        cursor.phase = 2;
        0
    }
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

/// Copy traversal owns the end-before-exit sequence and fresh ordered cursor.
/// No token reference or iterator survives each Native function-copy effect.
#[repr(C)]
#[derive(Default)]
pub struct ReactorCallbackCopy {
    pub cursor: ReactorCallbackCursor,
    pub kind: u32,
}
/// 0 complete, 1 copy end token, 2 copy exit token, -1 invalid.
/// # Safety
/// Source/state/output follow `callbacks_next`, with zero initialized state.
/// Source storage and payloads stay live; caller applies each copy after return.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_wrap,
    reason = "native int callback and action codes"
)]
pub unsafe extern "C" fn swr_v1_reactor_callbacks_copy_next(
    source: *const ReactorCallbacks,
    state: *mut ReactorCallbackCopy,
    token: *mut usize,
) -> i32 {
    // SAFETY: `state` is null or live, aligned and exclusively lent for this call.
    let Some(state) = (unsafe { state.as_mut() }) else {
        return -1;
    };
    if state.kind > 2 || token.is_null() {
        return -1;
    }
    while state.kind < 2 {
        if state.cursor.phase > 3 {
            return -1;
        }
        match next_metadata(
            // SAFETY: `source` is null or live and aligned for this call.
            unsafe { source.as_ref() },
            state.kind,
            &mut state.cursor,
            1,
            // SAFETY: `token` is non-null (checked above), aligned and exclusive.
            unsafe { &mut *token },
        ) {
            0 => {
                state.kind += 1;
                state.cursor = ReactorCallbackCursor::default();
            }
            1 => return state.kind as i32 + 1,
            _ => return -1,
        }
    }
    // SAFETY: `token` is non-null (checked above), aligned and exclusive.
    unsafe { *token = 0 };
    0
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_callbacks_copy_layout(field: u32) -> usize {
    match field {
        0 => std::mem::size_of::<ReactorCallbackCopy>(),
        1 => std::mem::align_of::<ReactorCallbackCopy>(),
        2 => std::mem::offset_of!(ReactorCallbackCopy, cursor),
        3 => std::mem::offset_of!(ReactorCallbackCopy, kind),
        _ => usize::MAX,
    }
}

#[cfg(test)]
#[expect(
    clippy::cast_sign_loss,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;
    #[test]
    fn callbacks_positive_whole_publication_copy_and_clear_control() {
        unsafe {
            let mut source = ptr::null_mut();
            let mut token = 0;
            for (kind, key, value) in [(0, 3, 103), (0, 1, 101), (1, 2, 202)] {
                assert_eq!(
                    swr_v1_reactor_callbacks_ensure(&raw mut source, kind, key, 0, &raw mut token),
                    2
                );
                assert_eq!(
                    swr_v1_reactor_callbacks_ensure(
                        &raw mut source,
                        kind,
                        key,
                        value,
                        &raw mut token
                    ),
                    3
                );
                assert_eq!(token, value);
                assert_eq!(
                    swr_v1_reactor_callbacks_ensure(&raw mut source, kind, key, 0, &raw mut token),
                    1
                );
                assert_eq!(token, value);
            }
            let mut target = ptr::null_mut();
            let mut copy = ReactorCallbackCopy::default();
            let mut observed = Vec::new();
            loop {
                let action =
                    swr_v1_reactor_callbacks_copy_next(source, &raw mut copy, &raw mut token);
                if action == 0 {
                    break;
                }
                observed.push((action, copy.cursor.key, token));
                let kind = (action - 1) as u32;
                let value = token + 1000;
                assert_eq!(
                    swr_v1_reactor_callbacks_ensure(
                        &raw mut target,
                        kind,
                        copy.cursor.key,
                        0,
                        &raw mut token
                    ),
                    2
                );
                assert_eq!(
                    swr_v1_reactor_callbacks_ensure(
                        &raw mut target,
                        kind,
                        copy.cursor.key,
                        value,
                        &raw mut token
                    ),
                    3
                );
            }
            assert_eq!(observed, [(1, 1, 101), (1, 3, 103), (2, 2, 202)]);
            for owner in [&mut source, &mut target] {
                let mut released = Vec::new();
                loop {
                    let action = swr_v1_reactor_callbacks_clear_next(owner, &raw mut token);
                    if action == 0 {
                        break;
                    }
                    released.push((action, token % 1000));
                }
                assert_eq!(released, [(2, 202), (1, 103), (1, 101)]);
                assert!(owner.is_null());
            }
        }
    }
}
