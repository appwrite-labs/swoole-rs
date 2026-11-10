// SPDX-License-Identifier: Apache-2.0
//! The live coroutines (upstream's `Coroutine::coroutines`): a scheduler's
//! coroutines by id, from creation until close, kept in the scheduler itself so
//! no lookup goes through a thread-local. Lists run newest first, the order
//! upstream's map iterates in while its buckets keep one coroutine each.
use crate::coroutine::{Coroutine, STATE_END, STATE_INIT, STATE_RUNNING, STATE_WAITING, Scheduler};
use core::ffi::{c_char, c_long, c_void};
use core::ptr;

/// The registry behind [`Scheduler::live`], allocated with the first coroutine:
/// entries sorted by id (ids come in rising order, so an insert appends), with a
/// removed coroutine's entry nulled until half the entries are null. Lookups are
/// a binary search over one allocation; a `BTreeMap` cost a node per coroutine.
#[derive(Debug, Default)]
pub struct Live {
    entries: Vec<(c_long, *mut Coroutine)>,
    count: usize,
}

impl Live {
    /// The index of the first entry whose id is not below `cid`.
    fn at(&self, cid: c_long) -> usize {
        self.entries.partition_point(|&(id, _)| id < cid)
    }

    fn get(&self, cid: c_long) -> Option<*mut Coroutine> {
        match self.entries.get(self.at(cid)) {
            Some(&(id, co)) if id == cid && !co.is_null() => Some(co),
            _ => None,
        }
    }
}

impl Scheduler {
    fn live(&self) -> Option<&Live> {
        // SAFETY: `live` is null or the box `insert` leaked, owned by this scheduler.
        unsafe { self.live.as_ref() }
    }

    pub(crate) fn insert(&mut self, cid: c_long, co: *mut Coroutine) {
        if self.live.is_null() {
            self.live = Box::into_raw(Box::default());
        }
        // SAFETY: just allocated, or the box from an earlier insert.
        let live = unsafe { &mut *self.live };
        let at = live.at(cid);
        match live.entries.get_mut(at) {
            Some(entry) if entry.0 == cid => {
                live.count += usize::from(entry.1.is_null());
                entry.1 = co;
            }
            _ => {
                live.entries.insert(at, (cid, co));
                live.count += 1;
            }
        }
    }

    pub(crate) fn remove(&mut self, cid: c_long) {
        // SAFETY: as for `live`.
        let Some(live) = (unsafe { self.live.as_mut() }) else {
            return;
        };
        let at = live.at(cid);
        let Some(entry) = live
            .entries
            .get_mut(at)
            .filter(|e| e.0 == cid && !e.1.is_null())
        else {
            return;
        };
        entry.1 = ptr::null_mut();
        live.count -= 1;
        if live.count * 2 <= live.entries.len() {
            live.entries.retain(|e| !e.1.is_null());
        }
    }

    /// Frees the registry once no coroutine is live (on deactivation).
    #[cfg(any(test, feature = "upstream-symbols"))]
    pub(crate) fn release_live(&mut self) {
        if self.count() == 0 && !self.live.is_null() {
            // SAFETY: the box `insert` leaked; nothing else refers to it.
            drop(unsafe { Box::from_raw(self.live) });
            self.live = ptr::null_mut();
        }
    }

    /// How many coroutines are live.
    #[must_use]
    pub fn count(&self) -> usize {
        self.live().map_or(0, |live| live.count)
    }

    /// The newest live coroutine older than `before`, with its id.
    fn older(&self, before: c_long) -> Option<(c_long, *mut Coroutine)> {
        let live = self.live()?;
        live.entries
            .get(..live.at(before))?
            .iter()
            .rev()
            .find(|e| !e.1.is_null())
            .copied()
    }
}

fn native(co: *mut Coroutine) -> *mut c_void {
    // SAFETY: registered coroutines are live until their close removes them.
    unsafe { (*co).native }
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

macro_rules! operation {
    ($(#[$doc:meta])* fn $name:ident($s:ident: $sty:ty $(, $arg:ident: $ty:ty)*) $(-> $ret:ty)? $body:block) => {
        $(#[$doc])*
        /// # Safety
        /// The scheduler is live and not borrowed elsewhere for the call; other
        /// pointer arguments are live.
        pub unsafe extern "C" fn $name($s: $sty $(, $arg: $ty)*) $(-> $ret)? $body
    };
}

operation! {
    /// `Coroutine::get_by_cid`: the facade of live coroutine `cid`, or null.
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_get(s: *const Scheduler, cid: c_long) -> *mut c_void {
        // SAFETY: the caller lends the scheduler.
        let s = unsafe { &*s };
        s.live().and_then(|live| live.get(cid)).map_or(ptr::null_mut(), native)
    }
}

operation! {
    /// `Coroutine::count`.
    #[unsafe(no_mangle)]
    fn swr_v1_coroutine_count(s: *const Scheduler) -> usize {
        // SAFETY: the caller lends the scheduler.
        unsafe { (*s).count() }
    }
}

operation! {
    /// The newest live coroutine older than `before` (`LONG_MAX` starts a walk):
    /// its id in `*cid` and its facade, or null when none is left.
    fn swr_v1_coroutine_older(s: *const Scheduler, before: c_long, cid: *mut c_long) -> *mut c_void {
        // SAFETY: the caller lends the scheduler.
        let Some((id, co)) = (unsafe { (*s).older(before) }) else {
            return ptr::null_mut();
        };
        // SAFETY: the caller lends `cid`.
        unsafe { *cid = id };
        native(co)
    }
}

operation! {
    /// `swoole_coroutine_iterator_reset`.
    fn swr_v1_coroutine_iterator_reset(s: *mut Scheduler) {
        // SAFETY: the caller lends the scheduler.
        unsafe { (*s).cursor = c_long::MAX };
    }
}

operation! {
    /// `swoole_coroutine_iterator_each`: the walk's next facade, or null at its end.
    fn swr_v1_coroutine_iterator_each(s: *mut Scheduler) -> *mut c_void {
        // SAFETY: the caller lends the scheduler.
        let s = unsafe { &mut *s };
        let Some((cid, co)) = s.older(s.cursor) else {
            s.cursor = c_long::MIN;
            return ptr::null_mut();
        };
        s.cursor = cid;
        native(co)
    }
}

operation! {
    /// `Coroutine::print_list`: one `Coroutine\t<cid>\t<state>` line per live
    /// coroutine on `out` (Swoole's stdout stream); aborts on a state outside the
    /// four. `out` is an open stream.
    fn swr_v1_coroutine_print_list(s: *const Scheduler, out: *mut libc::FILE) {
        // SAFETY: the caller lends the scheduler.
        let s = unsafe { &*s };
        let mut before = c_long::MAX;
        while let Some((cid, co)) = s.older(before) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coroutine::tests::scheduler;

    fn coroutine(native: usize, state: i32) -> Coroutine {
        // SAFETY: all-zero is a valid Coroutine (null pointers, zero scalars).
        let mut co: Coroutine = unsafe { core::mem::zeroed() };
        co.native = native as *mut c_void;
        co.state = state;
        co
    }

    #[test]
    fn registry_finds_counts_and_walks_newest_first() {
        let mut s = scheduler();
        let sp = &raw mut s;
        let mut a = coroutine(0xa0, STATE_RUNNING);
        let mut b = coroutine(0xb0, STATE_WAITING);
        let mut c = coroutine(0xc0, STATE_INIT);
        // SAFETY: the scheduler and coroutines live on this frame.
        unsafe {
            assert_eq!(swr_v1_coroutine_count(sp), 0);
            assert!(swr_v1_coroutine_get(sp, 1).is_null());
            (*sp).insert(1, &raw mut a);
            (*sp).insert(2, &raw mut b);
            (*sp).insert(3, &raw mut c);
            assert_eq!(swr_v1_coroutine_count(sp), 3);
            assert_eq!(swr_v1_coroutine_get(sp, 2) as usize, 0xb0);
            assert!(swr_v1_coroutine_get(sp, 4).is_null());

            let mut cid = 0;
            let first = swr_v1_coroutine_older(sp, c_long::MAX, &raw mut cid);
            assert_eq!((first as usize, cid), (0xc0, 3));
            let next = swr_v1_coroutine_older(sp, cid, &raw mut cid);
            assert_eq!((next as usize, cid), (0xb0, 2));

            (*sp).remove(2);
            swr_v1_coroutine_iterator_reset(sp);
            assert_eq!(swr_v1_coroutine_iterator_each(sp) as usize, 0xc0);
            assert_eq!(swr_v1_coroutine_iterator_each(sp) as usize, 0xa0);
            assert!(swr_v1_coroutine_iterator_each(sp).is_null());
            assert!(swr_v1_coroutine_iterator_each(sp).is_null());
            assert!(swr_v1_coroutine_older(sp, 1, &raw mut cid).is_null());

            (*sp).release_live();
            assert!(!(*sp).live.is_null(), "kept while coroutines are live");
            (*sp).remove(1);
            (*sp).remove(3);
            assert_eq!(swr_v1_coroutine_count(sp), 0);
            (*sp).release_live();
            assert!((*sp).live.is_null());
        }
    }

    #[test]
    fn print_list_writes_one_line_each_newest_first() {
        let mut s = scheduler();
        let mut a = coroutine(0, STATE_WAITING);
        let mut b = coroutine(0, STATE_RUNNING);
        s.insert(7, &raw mut a);
        s.insert(9, &raw mut b);
        let mut text = [0u8; 64];
        // SAFETY: a fresh temporary stream, read back and closed here.
        let n = unsafe {
            let out = libc::tmpfile();
            swr_v1_coroutine_print_list(&raw const s, out);
            libc::rewind(out);
            let n = libc::fread(text.as_mut_ptr().cast(), 1, text.len(), out);
            libc::fclose(out);
            n
        };
        s.remove(7);
        s.remove(9);
        s.release_live();
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
