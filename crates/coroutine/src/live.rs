// SPDX-License-Identifier: Apache-2.0
//! The live coroutines (upstream's `Coroutine::coroutines`): each thread's
//! coroutines by id, from creation until close. Lists run newest first, the order
//! upstream's map iterates in while its buckets keep one coroutine each.
use crate::coroutine::{Coroutine, STATE_END, STATE_INIT, STATE_RUNNING, STATE_WAITING};
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_long, c_void};
use core::ptr;
use std::collections::BTreeMap;

thread_local! {
    static LIVE: RefCell<BTreeMap<c_long, *mut Coroutine>> = const { RefCell::new(BTreeMap::new()) };
    /// Where `swoole_coroutine_iterator_each` (gdb's walk) resumes.
    static CURSOR: Cell<c_long> = const { Cell::new(c_long::MAX) };
}

pub(crate) fn insert(co: *mut Coroutine, cid: c_long) {
    LIVE.with_borrow_mut(|live| live.insert(cid, co));
}

pub(crate) fn remove(cid: c_long) {
    LIVE.with_borrow_mut(|live| live.remove(&cid));
}

/// How many coroutines are live.
pub(crate) fn count() -> usize {
    LIVE.with_borrow(BTreeMap::len)
}

fn native(co: *mut Coroutine) -> *mut c_void {
    // SAFETY: registered coroutines are live until their close removes them.
    unsafe { (*co).native }
}

/// The newest live coroutine older than `before`, with its id.
fn older(before: c_long) -> Option<(c_long, *mut Coroutine)> {
    LIVE.with_borrow(|live| {
        live.range(..before)
            .next_back()
            .map(|(cid, co)| (*cid, *co))
    })
}

/// `print_list`'s state label.
fn state_label(state: i32) -> Option<&'static core::ffi::CStr> {
    match state {
        STATE_INIT => Some(c"[INIT]"),
        STATE_WAITING => Some(c"[WAITING]"),
        STATE_RUNNING => Some(c"[RUNNING]"),
        STATE_END => Some(c"[END]"),
        _ => None,
    }
}

/// `Coroutine::get_by_cid`: the facade of live coroutine `cid`, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_coroutine_get(cid: c_long) -> *mut c_void {
    LIVE.with_borrow(|live| live.get(&cid).copied())
        .map_or(ptr::null_mut(), native)
}

/// `Coroutine::count`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_coroutine_count() -> usize {
    count()
}

/// The newest live coroutine older than `before` (`LONG_MAX` starts a walk):
/// its id in `*cid` and its facade, or null when none is left.
///
/// # Safety
/// `cid` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_older(before: c_long, cid: *mut c_long) -> *mut c_void {
    let Some((id, co)) = older(before) else {
        return ptr::null_mut();
    };
    // SAFETY: the caller lends `cid`.
    unsafe { *cid = id };
    native(co)
}

/// `swoole_coroutine_iterator_reset`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_coroutine_iterator_reset() {
    CURSOR.set(c_long::MAX);
}

/// `swoole_coroutine_iterator_each`: the walk's next facade, or null at its end.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_coroutine_iterator_each() -> *mut c_void {
    let Some((cid, co)) = older(CURSOR.get()) else {
        CURSOR.set(c_long::MIN);
        return ptr::null_mut();
    };
    CURSOR.set(cid);
    native(co)
}

/// `Coroutine::print_list`: one `Coroutine\t<cid>\t<state>` line per live
/// coroutine on `out` (Swoole's stdout stream); aborts on a state outside the four.
///
/// # Safety
/// `out` is an open stream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_print_list(out: *mut libc::FILE) {
    let mut before = c_long::MAX;
    while let Some((cid, co)) = older(before) {
        before = cid;
        // SAFETY: registered coroutines are live.
        let state = unsafe { (*co).state };
        let Some(label) = state_label(state) else {
            std::process::abort();
        };
        let format: *const c_char = c"Coroutine\t%ld\t%s\n".as_ptr();
        // SAFETY: `out` is open; the format takes a long and a C string, which follow it.
        unsafe { libc::fprintf(out, format, cid, label.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coroutine(native: usize, state: i32) -> Coroutine {
        // SAFETY: all-zero is a valid Coroutine (null pointers, zero scalars).
        let mut co: Coroutine = unsafe { core::mem::zeroed() };
        co.native = native as *mut c_void;
        co.state = state;
        co
    }

    #[test]
    fn registry_finds_counts_and_walks_newest_first() {
        let mut a = coroutine(0xa0, STATE_RUNNING);
        let mut b = coroutine(0xb0, STATE_WAITING);
        let mut c = coroutine(0xc0, STATE_INIT);
        insert(&raw mut a, 1);
        insert(&raw mut b, 2);
        insert(&raw mut c, 3);
        assert_eq!(swr_v1_coroutine_count(), 3);
        assert_eq!(swr_v1_coroutine_get(2) as usize, 0xb0);
        assert!(swr_v1_coroutine_get(4).is_null());

        let mut cid = 0;
        // SAFETY: `cid` is a local.
        let first = unsafe { swr_v1_coroutine_older(c_long::MAX, &raw mut cid) };
        assert_eq!((first as usize, cid), (0xc0, 3));
        // SAFETY: as above.
        let next = unsafe { swr_v1_coroutine_older(cid, &raw mut cid) };
        assert_eq!((next as usize, cid), (0xb0, 2));

        remove(2);
        swr_v1_coroutine_iterator_reset();
        assert_eq!(swr_v1_coroutine_iterator_each() as usize, 0xc0);
        assert_eq!(swr_v1_coroutine_iterator_each() as usize, 0xa0);
        assert!(swr_v1_coroutine_iterator_each().is_null());
        assert!(swr_v1_coroutine_iterator_each().is_null());
        // SAFETY: as above.
        assert!(unsafe { swr_v1_coroutine_older(1, &raw mut cid) }.is_null());

        remove(1);
        remove(3);
        assert_eq!(swr_v1_coroutine_count(), 0);
    }

    #[test]
    fn print_list_writes_one_line_each_newest_first() {
        let mut a = coroutine(0, STATE_WAITING);
        let mut b = coroutine(0, STATE_RUNNING);
        insert(&raw mut a, 7);
        insert(&raw mut b, 9);
        let mut text = [0u8; 64];
        // SAFETY: a fresh temporary stream, read back and closed here.
        let n = unsafe {
            let out = libc::tmpfile();
            swr_v1_coroutine_print_list(out);
            libc::rewind(out);
            let n = libc::fread(text.as_mut_ptr().cast(), 1, text.len(), out);
            libc::fclose(out);
            n
        };
        remove(7);
        remove(9);
        assert_eq!(
            &text[..n],
            b"Coroutine\t9\t[RUNNING]\nCoroutine\t7\t[WAITING]\n"
        );
    }

    #[test]
    fn labels_cover_the_states() {
        assert_eq!(state_label(STATE_INIT), Some(c"[INIT]"));
        assert_eq!(state_label(STATE_WAITING), Some(c"[WAITING]"));
        assert_eq!(state_label(STATE_RUNNING), Some(c"[RUNNING]"));
        assert_eq!(state_label(STATE_END), Some(c"[END]"));
        assert_eq!(state_label(7), None);
    }
}
