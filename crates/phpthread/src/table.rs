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
        self.slots[slot].as_ref().map(|(_, value)| value)
    }

    pub(crate) fn get_mut(&mut self, key: &Key) -> Option<&mut V> {
        let slot = self.slot(key)?;
        self.slots[slot].as_mut().map(|(_, value)| value)
    }

    pub(crate) fn contains(&self, key: &Key) -> bool {
        self.slot(key).is_some()
    }

    /// `zend_hash_update_ptr` / `zend_hash_index_update_ptr`: replace in place
    /// (the old value is dropped) or append.
    pub(crate) fn update(&mut self, key: Key, value: V) {
        match self.slot(&key) {
            Some(slot) => {
                if let Some(entry) = self.slots[slot].as_mut() {
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
        let removed = self.slots[slot].take().map(|(_, value)| value);
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
            && start < count
        {
            // Rotate the values (not the keys) of positions start.. down by one,
            // so the removed value ends up last.
            for position in start..count - 1 {
                let (current, next) = self.slots.split_at_mut(position + 1);
                if let (Some((_, a)), Some((_, b))) = (current[position].as_mut(), next[0].as_mut())
                {
                    std::mem::swap(a, b);
                }
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

    /// `zend_hash_sort(ht, compare, renumber)`: `sort` orders the entries
    /// (Zend's own `zend_sort`); `renumber` rekeys them `0..n` as a list.
    pub(crate) fn sort(&mut self, renumber: bool, sort: impl FnOnce(&mut [(Key, V)])) {
        if !(self.len() > 1 || renumber && self.len() > 0) {
            return;
        }
        let mut entries: Vec<(Key, V)> = self.slots.drain(..).flatten().collect();
        sort(&mut entries);
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
        table.sort(false, |entries| entries.sort_by_key(|entry| entry.1));
        assert_eq!(
            keys(&table),
            [
                Key::Str(b"y".as_slice().into()),
                Key::Str(b"x".as_slice().into())
            ],
            "keys move with their values"
        );
        table.sort(true, |entries| entries.sort_by_key(|entry| entry.1));
        assert_eq!(keys(&table), [Key::Int(0), Key::Int(1)], "renumbered");
        assert_eq!(table.get(&Key::Int(1)), Some(&3));
    }
}
