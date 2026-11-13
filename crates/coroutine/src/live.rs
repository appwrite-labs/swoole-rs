// SPDX-License-Identifier: Apache-2.0
//! The live coroutines (upstream's `Coroutine::coroutines`): a scheduler's
//! coroutines by id, from creation until close, kept in the scheduler itself so
//! no lookup goes through a thread-local. Lists run newest first, the order
//! upstream's map iterates in while its buckets keep one coroutine each.
use crate::coroutine::{Co, Scheduler, State};
use core::ffi::{c_long, c_void};
use core::ptr;

/// The registry behind [`Scheduler::live`], allocated with the first coroutine:
/// entries sorted by id (ids come in rising order, so an insert appends), with a
/// removed coroutine's entry emptied until half the entries are. Lookups are
/// a binary search over one allocation; a `BTreeMap` cost a node per coroutine.
#[derive(Debug, Default)]
pub struct Live {
    entries: Vec<(c_long, Option<Co>)>,
    count: usize,
}

impl Live {
    /// The index of the first entry whose id is not below `cid`.
    fn at(&self, cid: c_long) -> usize {
        self.entries.partition_point(|&(id, _)| id < cid)
    }

    fn get(&self, cid: c_long) -> Option<Co> {
        match self.entries.get(self.at(cid)) {
            Some(&(id, co)) if id == cid => co,
            _ => None,
        }
    }
}

impl Scheduler {
    fn live(&self) -> Option<&Live> {
        self.live.as_deref()
    }

    pub(crate) fn insert(&mut self, cid: c_long, co: Co) {
        let live = self.live.get_or_insert_default();
        let at = live.at(cid);
        match live.entries.get_mut(at) {
            Some(entry) if entry.0 == cid => {
                live.count += usize::from(entry.1.is_none());
                entry.1 = Some(co);
            }
            _ => {
                live.entries.insert(at, (cid, Some(co)));
                live.count += 1;
            }
        }
    }

    pub(crate) fn remove(&mut self, cid: c_long) {
        let Some(live) = self.live.as_deref_mut() else {
            return;
        };
        let at = live.at(cid);
        let Some(entry) = live
            .entries
            .get_mut(at)
            .filter(|e| e.0 == cid && e.1.is_some())
        else {
            return;
        };
        entry.1 = None;
        live.count -= 1;
        if live.count * 2 <= live.entries.len() {
            live.entries.retain(|e| e.1.is_some());
        }
    }

    /// Frees the registry once no coroutine is live (on deactivation).
    #[cfg(any(test, feature = "upstream-symbols"))]
    pub(crate) fn release_live(&mut self) {
        if self.count() == 0 {
            self.live = None;
        }
    }

    /// How many coroutines are live.
    #[must_use]
    pub fn count(&self) -> usize {
        self.live().map_or(0, |live| live.count)
    }

    /// The newest live coroutine older than `before`, with its id.
    fn older(&self, before: c_long) -> Option<(c_long, Co)> {
        let live = self.live()?;
        live.entries
            .get(..live.at(before))?
            .iter()
            .rev()
            .find_map(|&(cid, co)| Some((cid, co?)))
    }
}

/// A registered coroutine's facade: live until its close removes it.
fn native(co: Co) -> *mut c_void {
    co.with(|co| co.native)
}

/// `print_list`'s state label.
fn state_label(state: State) -> &'static str {
    match state {
        State::Init => "[INIT]",
        State::Waiting => "[WAITING]",
        State::Running => "[RUNNING]",
        State::End => "[END]",
    }
}

impl Scheduler {
    /// `Coroutine::get_by_cid`: the facade of live coroutine `cid`, or null.
    #[must_use]
    pub fn get(&self, cid: c_long) -> *mut c_void {
        self.live()
            .and_then(|live| live.get(cid))
            .map_or(ptr::null_mut(), native)
    }

    /// The newest live coroutine older than `before` (`LONG_MAX` starts a walk):
    /// its id and its facade.
    #[must_use]
    pub fn older_native(&self, before: c_long) -> Option<(c_long, *mut c_void)> {
        self.older(before).map(|(cid, co)| (cid, native(co)))
    }

    /// `swoole_coroutine_iterator_reset`.
    pub fn iterator_reset(&mut self) {
        self.cursor = c_long::MAX;
    }

    /// `swoole_coroutine_iterator_each`: the walk's next facade, or null at its end.
    pub fn iterator_each(&mut self) -> *mut c_void {
        let Some((cid, co)) = self.older(self.cursor) else {
            self.cursor = c_long::MIN;
            return ptr::null_mut();
        };
        self.cursor = cid;
        native(co)
    }

    /// `Coroutine::print_list`'s text: one `Coroutine\t<cid>\t<state>` line per
    /// live coroutine.
    #[must_use]
    pub fn list_text(&self) -> String {
        let mut text = String::new();
        let mut before = c_long::MAX;
        while let Some((cid, co)) = self.older(before) {
            before = cid;
            let label = state_label(co.with(|co| co.state));
            text.push_str("Coroutine\t");
            text.push_str(&cid.to_string());
            text.push('\t');
            text.push_str(label);
            text.push('\n');
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coroutine::{Coroutine, tests::scheduler};

    fn co(co: &mut Coroutine) -> Co {
        // SAFETY: the cases' coroutines outlive their registration.
        unsafe { Co::of(co) }.unwrap()
    }

    fn coroutine(native: usize, state: State) -> Coroutine {
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
        let mut a = coroutine(0xa0, State::Running);
        let mut b = coroutine(0xb0, State::Waiting);
        let mut c = coroutine(0xc0, State::Init);
        // SAFETY: the scheduler and coroutines live on this frame.
        unsafe {
            assert_eq!((*sp).count(), 0);
            assert!((*sp).get(1).is_null());
            (*sp).insert(1, co(&mut a));
            (*sp).insert(2, co(&mut b));
            (*sp).insert(3, co(&mut c));
            assert_eq!((*sp).count(), 3);
            assert_eq!((*sp).get(2) as usize, 0xb0);
            assert!((*sp).get(4).is_null());

            let (cid, first) = (*sp).older_native(c_long::MAX).unwrap();
            assert_eq!((first as usize, cid), (0xc0, 3));
            let (cid, next) = (*sp).older_native(cid).unwrap();
            assert_eq!((next as usize, cid), (0xb0, 2));

            (*sp).remove(2);
            (*sp).iterator_reset();
            assert_eq!((*sp).iterator_each() as usize, 0xc0);
            assert_eq!((*sp).iterator_each() as usize, 0xa0);
            assert!((*sp).iterator_each().is_null());
            assert!((*sp).iterator_each().is_null());
            assert!((*sp).older_native(1).is_none());

            (*sp).release_live();
            assert!((*sp).live.is_some(), "kept while coroutines are live");
            (*sp).remove(1);
            (*sp).remove(3);
            assert_eq!((*sp).count(), 0);
            (*sp).release_live();
            assert!((*sp).live.is_none());
        }
    }

    #[test]
    fn print_list_writes_one_line_each_newest_first() {
        let mut s = scheduler();
        let mut a = coroutine(0, State::Waiting);
        let mut b = coroutine(0, State::Running);
        s.insert(7, co(&mut a));
        s.insert(9, co(&mut b));
        let text = s.list_text();
        s.remove(7);
        s.remove(9);
        s.release_live();
        assert_eq!(
            text.as_bytes(),
            b"Coroutine\t9\t[RUNNING]\nCoroutine\t7\t[WAITING]\n"
        );
    }

    #[test]
    fn labels_cover_the_states() {
        assert_eq!(state_label(State::Init), "[INIT]");
        assert_eq!(state_label(State::Waiting), "[WAITING]");
        assert_eq!(state_label(State::Running), "[RUNNING]");
        assert_eq!(state_label(State::End), "[END]");
    }
}
