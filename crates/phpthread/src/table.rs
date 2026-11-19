//! An insertion-ordered table with int and string keys: the parts of a Zend
//! `HashTable` the thread containers observe (order, `nNextFreeElement`,
//! packed-list positions). Pure Rust, so it is tested without PHP.
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Int(i64),
    Str(Box<[u8]>),
}

/// Zend's `nNextFreeElement` before any int key: the next append uses 0.
const NO_NEXT: i64 = i64::MIN;

#[derive(Debug)]
pub(crate) struct Table<V> {
    /// Insertion order; `None` is a deleted slot (Zend's `IS_UNDEF` bucket).
    slots: Vec<Option<(Key, V)>>,
    ints: HashMap<i64, usize>,
    strs: HashMap<Box<[u8]>, usize>,
    next_free: i64,
}

impl<V> Default for Table<V> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            ints: HashMap::new(),
            strs: HashMap::new(),
            next_free: NO_NEXT,
        }
    }
}

impl<V> Table<V> {
    /// `zend_hash_num_elements`.
    pub(crate) fn len(&self) -> usize {
        self.ints.len() + self.strs.len()
    }

    fn slot(&self, key: &Key) -> Option<usize> {
        match key {
            Key::Int(index) => self.ints.get(index).copied(),
            Key::Str(name) => self.strs.get(name).copied(),
        }
    }

    pub(crate) fn get_int(&self, index: i64) -> Option<&V> {
        self.get(&Key::Int(index))
    }

    pub(crate) fn get(&self, key: &Key) -> Option<&V> {
        let slot = self.slot(key)?;
        self.slots.get(slot)?.as_ref().map(|(_, value)| value)
    }

    pub(crate) fn get_mut(&mut self, key: &Key) -> Option<&mut V> {
        let slot = self.slot(key)?;
        self.slots.get_mut(slot)?.as_mut().map(|(_, value)| value)
    }

    pub(crate) fn contains(&self, key: &Key) -> bool {
        self.slot(key).is_some()
    }

    /// `zend_hash_update_ptr` / `zend_hash_index_update_ptr`: replace in place
    /// (the old value is dropped) or append.
    pub(crate) fn update(&mut self, key: Key, value: V) {
        match self.slot(&key) {
            Some(slot) => {
                if let Some(Some(entry)) = self.slots.get_mut(slot) {
                    entry.1 = value;
                }
            }
            None => self.insert(key, value),
        }
    }

    /// `zend_hash_add_ptr`: false (and `value` dropped) if the key exists.
    pub(crate) fn add(&mut self, key: Key, value: V) -> bool {
        if self.contains(&key) {
            return false;
        }
        self.insert(key, value);
        true
    }

    /// `zend_hash_next_index_insert_ptr`.
    pub(crate) fn push(&mut self, value: V) {
        let index = if self.next_free == NO_NEXT {
            0
        } else {
            self.next_free
        };
        self.add(Key::Int(index), value);
    }

    fn insert(&mut self, key: Key, value: V) {
        let slot = self.slots.len();
        match &key {
            Key::Int(index) => {
                self.ints.insert(*index, slot);
                if *index >= self.next_free {
                    self.next_free = index.saturating_add(1);
                }
            }
            Key::Str(name) => {
                self.strs.insert(name.clone(), slot);
            }
        }
        self.slots.push(Some((key, value)));
    }

    /// `zend_hash_del` / `zend_hash_index_del`.
    pub(crate) fn remove(&mut self, key: &Key) -> Option<V> {
        let slot = match key {
            Key::Int(index) => self.ints.remove(index),
            Key::Str(name) => self.strs.remove(name),
        }?;
        let removed = self
            .slots
            .get_mut(slot)
            .and_then(Option::take)
            .map(|(_, value)| value);
        if self.slots.len() > 8 && self.len() * 2 < self.slots.len() {
            self.compact();
        }
        removed
    }

    fn compact(&mut self) {
        self.slots.retain(Option::is_some);
        self.reindex();
    }

    fn reindex(&mut self) {
        self.ints.clear();
        self.strs.clear();
        for (slot, entry) in self.slots.iter().enumerate() {
            match entry {
                Some((Key::Int(index), _)) => {
                    self.ints.insert(*index, slot);
                }
                Some((Key::Str(name), _)) => {
                    self.strs.insert(name.clone(), slot);
                }
                None => {}
            }
        }
    }

    /// `ZEND_HASH_FOREACH_KEY_PTR`.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Key, &V)> {
        self.slots
            .iter()
            .filter_map(|entry| entry.as_ref().map(|(key, value)| (key, value)))
    }

    /// `ArrayList`'s `offsetUnset`: upstream shifts the packed values down one
    /// position from `index`, leaving every key where it was, then drops the
    /// last position and decrements `nNextFreeElement`. An `index` outside the
    /// list only drops the last position. An empty table is left alone.
    pub(crate) fn unset_shift(&mut self, index: i64) {
        self.compact();
        let count = self.slots.len();
        if count == 0 {
            return;
        }
        if let Ok(start) = usize::try_from(index)
            && let Some(mut rest) = self.slots.get_mut(start..)
        {
            // Rotate the values (not the keys) of positions start.. down by one,
            // so the removed value ends up last.
            while let Some((current, tail)) = rest.split_first_mut() {
                if let (Some((_, a)), Some(Some((_, b)))) = (current.as_mut(), tail.first_mut()) {
                    std::mem::swap(a, b);
                }
                rest = tail;
            }
        }
        if let Some((key, _)) = self.slots.pop().flatten() {
            match key {
                Key::Int(index) => self.ints.remove(&index),
                Key::Str(name) => self.strs.remove(&name),
            };
        }
        self.next_free = self.next_free.wrapping_sub(1);
    }

    /// `zend_hash_sort(ht, compare, renumber)`, where `gt` is `compare(a, b) > 0`:
    /// [`zend_sort`] orders the entries by value; `renumber` rekeys them `0..n` as a list.
    pub(crate) fn sort(&mut self, renumber: bool, gt: impl Fn(&V, &V) -> bool) {
        if !(self.len() > 1 || renumber && self.len() > 0) {
            return;
        }
        let mut entries: Vec<(Key, V)> = self.slots.drain(..).flatten().collect();
        zend_sort(&mut entries, &|a: &(Key, V), b: &(Key, V)| gt(&a.1, &b.1));
        if renumber {
            for (position, entry) in entries.iter_mut().enumerate() {
                entry.0 = Key::Int(i64::try_from(position).unwrap_or(i64::MAX));
            }
            self.next_free = i64::try_from(entries.len()).unwrap_or(i64::MAX);
        }
        self.slots = entries.into_iter().map(Some).collect();
        self.reindex();
    }
}

/// Zend's `zend_sort` (libc++'s introsort without the heap fallback: insertion
/// sort up to 16 items, else a median-of-3 or of-5 quicksort step), with `gt` as
/// its `cmp(a, b) > 0`. `gt` need not be a total order, and items it doesn't
/// order land where Zend's would. `order_matches_running_zend` compares both kinds of
/// comparator with the running PHP engine.
#[expect(
    clippy::indexing_slicing,
    reason = "Zend's index steps, kept as written; its bounds checks (`i == j`, `j == 0`) hold whatever `gt` answers"
)]
pub(crate) fn zend_sort<T>(mut items: &mut [T], gt: &impl Fn(&T, &T) -> bool) {
    while items.len() > 16 {
        let n = items.len();
        let pivot = n >> 1;
        if n >> 10 == 0 {
            sort_few(items, &[0, pivot, n - 1], gt);
        } else {
            let delta = pivot >> 1;
            sort_few(items, &[0, delta, pivot, pivot + delta, n - 1], gt);
        }
        items.swap(1, pivot);
        let (mut i, mut j) = (2, n - 1);
        'done: loop {
            while gt(&items[1], &items[i]) {
                i += 1;
                if i == j {
                    break 'done;
                }
            }
            j -= 1;
            if j == i {
                break;
            }
            while gt(&items[j], &items[1]) {
                j -= 1;
                if j == i {
                    break 'done;
                }
            }
            items.swap(i, j);
            i += 1;
            if i == j {
                break;
            }
        }
        items.swap(1, i - 1);
        // Recurse into the shorter side; loop on the longer.
        let (left, right) = std::mem::take(&mut items).split_at_mut(i);
        let left = &mut left[..i - 1];
        if left.len() < right.len() {
            zend_sort(left, gt);
            items = right;
        } else {
            zend_sort(right, gt);
            items = left;
        }
    }
    insert_sort(items, gt);
}

/// `zend_insert_sort`: sorting networks up to 5 items, else insertion sort,
/// which past the sixth item probes two back at a time.
#[expect(
    clippy::indexing_slicing,
    reason = "Zend's index steps, kept as written; its bounds checks (`i == j`, `j == 0`) hold whatever `gt` answers"
)]
fn insert_sort<T>(items: &mut [T], gt: &impl Fn(&T, &T) -> bool) {
    let n = items.len();
    match n {
        0 | 1 => {}
        2 => {
            if gt(&items[0], &items[1]) {
                items.swap(0, 1);
            }
        }
        3..=5 => sort_few(items, &[0, 1, 2, 3, 4][..n], gt),
        _ => {
            for i in 1..n {
                let mut j = i - 1;
                if !gt(&items[j], &items[i]) {
                    continue;
                }
                if i < 6 {
                    while j != 0 {
                        j -= 1;
                        if !gt(&items[j], &items[i]) {
                            j += 1;
                            break;
                        }
                    }
                } else {
                    loop {
                        j -= 2;
                        if !gt(&items[j], &items[i]) {
                            j += 1;
                            if !gt(&items[j], &items[i]) {
                                j += 1;
                            }
                            break;
                        }
                        if j == 0 {
                            break;
                        }
                        if j == 1 {
                            j = usize::from(gt(&items[i], &items[0]));
                            break;
                        }
                    }
                }
                // Zend's adjacent swaps from `i` down to `j`.
                items[j..=i].rotate_right(1);
            }
        }
    }
}

/// `zend_sort_3`, then (`zend_sort_4`, `zend_sort_5`) each further item sunk
/// into place, over the items at the positions `at`.
#[expect(
    clippy::indexing_slicing,
    reason = "Zend's index steps, kept as written; its bounds checks (`i == j`, `j == 0`) hold whatever `gt` answers"
)]
fn sort_few<T>(items: &mut [T], at: &[usize], gt: &impl Fn(&T, &T) -> bool) {
    let [a, b, c] = [at[0], at[1], at[2]];
    if !gt(&items[a], &items[b]) {
        if gt(&items[b], &items[c]) {
            items.swap(b, c);
            if gt(&items[a], &items[b]) {
                items.swap(a, b);
            }
        }
    } else if !gt(&items[c], &items[b]) {
        items.swap(a, c);
    } else {
        items.swap(a, b);
        if gt(&items[b], &items[c]) {
            items.swap(b, c);
        }
    }
    for k in 3..at.len() {
        for w in (1..=k).rev() {
            if !gt(&items[at[w - 1]], &items[at[w]]) {
                break;
            }
            items.swap(at[w - 1], at[w]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys<V>(table: &Table<V>) -> Vec<Key> {
        table.iter().map(|(key, _)| key.clone()).collect()
    }

    #[test]
    fn appends_follow_next_free_element() {
        let mut table = Table::default();
        table.push('a');
        table.push('b');
        table.update(Key::Int(10), 'c');
        table.push('d');
        assert_eq!(
            table.get_int(11),
            Some(&'d'),
            "append after the largest int key"
        );
        let mut negative = Table::default();
        negative.update(Key::Int(-5), 'x');
        negative.push('y');
        assert_eq!(
            negative.get_int(-4),
            Some(&'y'),
            "PHP 8.3: append after a negative key"
        );
    }

    #[test]
    fn update_keeps_position_and_remove_keeps_order() {
        let mut table = Table::default();
        table.update(Key::Str(b"a".as_slice().into()), 1);
        table.update(Key::Int(3), 2);
        table.update(Key::Str(b"b".as_slice().into()), 3);
        table.update(Key::Str(b"a".as_slice().into()), 4);
        assert_eq!(
            table.iter().map(|(_, v)| *v).collect::<Vec<_>>(),
            [4, 2, 3],
            "update replaces in place"
        );
        assert_eq!(table.remove(&Key::Int(3)), Some(2));
        assert_eq!(
            keys(&table),
            [
                Key::Str(b"a".as_slice().into()),
                Key::Str(b"b".as_slice().into())
            ],
            "removal keeps the others' order"
        );
        assert!(
            !table.add(Key::Str(b"b".as_slice().into()), 9),
            "add refuses an existing key"
        );
        table = Table::default();
        table.push(0);
        assert_eq!(table.get_int(0), Some(&0), "clean resets the next index");
    }

    #[test]
    fn list_unset_shifts_values_and_drops_the_last_key() {
        let mut table = Table::default();
        for value in ['a', 'b', 'c', 'd'] {
            table.push(value);
        }
        table.unset_shift(1);
        assert_eq!(
            table
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect::<Vec<_>>(),
            [(Key::Int(0), 'a'), (Key::Int(1), 'c'), (Key::Int(2), 'd')],
            "values shift down, keys stay"
        );
        table.push('e');
        assert_eq!(table.get_int(3), Some(&'e'), "next index decremented");
        table.unset_shift(9);
        assert_eq!(table.len(), 3, "out of range drops the last position");
    }

    #[test]
    fn sort_renumbers_lists_and_keeps_map_keys() {
        let mut table = Table::default();
        table.update(Key::Str(b"x".as_slice().into()), 3);
        table.update(Key::Str(b"y".as_slice().into()), 1);
        table.sort(false, |a, b| a > b);
        assert_eq!(
            keys(&table),
            [
                Key::Str(b"y".as_slice().into()),
                Key::Str(b"x".as_slice().into())
            ],
            "keys move with their values"
        );
        table.sort(true, |a, b| a > b);
        assert_eq!(keys(&table), [Key::Int(0), Key::Int(1)], "renumbered");
        assert_eq!(table.get(&Key::Int(1)), Some(&3));
    }

    /// Keys with ties (an LCG modulo `n / 3 + 1`), sorted by `gt`: the original
    /// positions, in the order PHP 8.3's `zend_sort` leaves them.
    fn zend_sorted(n: u64, gt: fn(u64, u64) -> bool) -> Vec<u64> {
        let mut state = 1000 + n;
        let mut items: Vec<(u64, u64)> = (0..n)
            .map(|id| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((state >> 33) % (n / 3 + 1), id)
            })
            .collect();
        zend_sort(&mut items, &|a: &(u64, u64), b: &(u64, u64)| gt(a.0, b.0));
        items.into_iter().map(|(_, id)| id).collect()
    }

    #[test]
    fn zend_sort_places_ties_as_zend_does() {
        let gt = |a, b| a > b;
        assert_eq!(zend_sorted(12, gt), [3, 2, 5, 9, 11, 4, 6, 10, 7, 8, 0, 1]);
        assert_eq!(
            zend_sorted(40, gt),
            [
                3, 4, 26, 9, 39, 15, 38, 30, 29, 25, 12, 14, 24, 19, 18, 20, 2, 0, 1, 5, 32, 35,
                10, 33, 31, 13, 23, 27, 36, 16, 17, 7, 28, 11, 6, 34, 8, 22, 21, 37
            ]
        );
    }

    #[test]
    fn zend_sort_follows_zend_through_an_inconsistent_order() {
        // Rock-paper-scissors: no total order, so only Zend's steps decide.
        let gt = |a: u64, b: u64| (a + 3 - b % 3) % 3 == 1;
        assert_eq!(zend_sorted(12, gt), [3, 0, 1, 2, 5, 4, 6, 7, 8, 9, 11, 10]);
        assert_eq!(
            zend_sorted(40, gt),
            [
                39, 12, 21, 25, 29, 30, 17, 16, 15, 14, 11, 32, 8, 7, 5, 36, 37, 38, 22, 24, 31,
                33, 35, 20, 18, 19, 10, 6, 2, 23, 1, 26, 27, 28, 13, 9, 34, 4, 3, 0
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[cfg_attr(miri, ignore = "runs the installed PHP engine")]
    #[test]
    fn order_matches_running_zend() {
        let mut reference = std::process::Command::new("sh");
        reference.arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/modules/locks/thread/sort.sh"
        ));
        let mut expected = Vec::new();
        for n in [1, 3, 5, 12, 16, 17, 40, 1023, 1024, 1025] {
            let keys: Vec<usize> = (0..n).map(|i| (i * 7919 + 13) % (n / 3 + 1)).collect();
            reference.arg(
                keys.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
            for cyclic in [false, true] {
                let mut items: Vec<_> = keys.iter().copied().enumerate().collect();
                zend_sort(&mut items, &|a, b| {
                    if cyclic {
                        (a.1 + 3 - b.1 % 3) % 3 == 1
                    } else {
                        a.1 > b.1
                    }
                });
                expected.push(
                    items
                        .into_iter()
                        .map(|(id, _)| id.to_string())
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
        }
        let output = reference.output().expect("run PHP zend_sort");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("numeric output");
        assert_eq!(
            expected,
            stdout.lines().collect::<Vec<_>>(),
            "Zend sort permutations"
        );
    }

    #[test]
    fn zend_sort_orders_every_length() {
        for n in 0..1100_u64 {
            let mut items: Vec<u64> = (0..n).map(|i| i.wrapping_mul(7919) % (n / 2 + 1)).collect();
            zend_sort(&mut items, &|a: &u64, b: &u64| a > b);
            assert!(items.is_sorted(), "length {n}");
        }
    }
}
