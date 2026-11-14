//! Flat containers (docs/migration.md, Data layout): a table indexed by a dense key
//! in place of a hash map, and a bitset in place of an array of bools.

/// Values keyed by a small dense integer, such as an fd: a flat array, no hashing.
/// Iteration is in key order.
#[derive(Default)]
pub struct Table<T> {
    slots: Vec<Option<T>>,
    len: usize,
}

impl<T> Table<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            len: 0,
        }
    }

    /// How many keys hold a value.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn get(&self, key: usize) -> Option<&T> {
        self.slots.get(key)?.as_ref()
    }

    pub fn get_mut(&mut self, key: usize) -> Option<&mut T> {
        self.slots.get_mut(key)?.as_mut()
    }

    /// Stores `value` at `key` and returns the value it replaced. Grows the array to
    /// `key + 1` slots; if that allocation fails, the table is unchanged and `value`
    /// comes back as the error.
    ///
    /// # Errors
    /// `value`, when the array can't grow to hold `key`.
    pub fn insert(&mut self, key: usize, value: T) -> Result<Option<T>, T> {
        if key >= self.slots.len() {
            let Some(extra) = key.checked_add(1).map(|end| end - self.slots.len()) else {
                return Err(value);
            };
            if self.slots.try_reserve(extra).is_err() {
                return Err(value);
            }
            self.slots.resize_with(key + 1, || None);
        }
        let Some(slot) = self.slots.get_mut(key) else {
            return Err(value);
        };
        let old = slot.replace(value);
        if old.is_none() {
            self.len += 1;
        }
        Ok(old)
    }

    pub fn remove(&mut self, key: usize) -> Option<T> {
        let old = self.slots.get_mut(key)?.take();
        if old.is_some() {
            self.len -= 1;
        }
        old
    }

    /// Each key and its value, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(key, slot)| slot.as_ref().map(|value| (key, value)))
    }
}

/// A growable sequence of bits, 64 to a word: an array of bools at an eighth of the
/// size, which also works as a stack.
#[derive(Default)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bit `index`, or `None` past the end.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= self.len {
            return None;
        }
        self.words
            .get(index / 64)
            .map(|word| (word >> (index % 64)) & 1 == 1)
    }

    /// Sets bit `index` if it is within the sequence; whether it was.
    pub fn set(&mut self, index: usize, bit: bool) -> bool {
        if index >= self.len {
            return false;
        }
        let Some(word) = self.words.get_mut(index / 64) else {
            return false;
        };
        let mask = 1 << (index % 64);
        if bit {
            *word |= mask;
        } else {
            *word &= !mask;
        }
        true
    }

    pub fn push(&mut self, bit: bool) {
        if self.len.is_multiple_of(64) {
            self.words.push(0);
        }
        self.len += 1;
        self.set(self.len - 1, bit);
    }

    pub fn pop(&mut self) -> Option<bool> {
        let bit = self.last()?;
        self.len -= 1;
        if self.len.is_multiple_of(64) {
            self.words.pop();
        }
        Some(bit)
    }

    #[must_use]
    pub fn last(&self) -> Option<bool> {
        self.get(self.len.checked_sub(1)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_keys_are_slots() {
        let mut table = Table::new();
        assert!(table.is_empty(), "a new table is empty");
        assert_eq!(table.insert(5, 'a'), Ok(None), "first insert");
        assert_eq!(table.insert(2, 'b'), Ok(None), "insert below the end");
        assert_eq!(
            table.insert(5, 'c'),
            Ok(Some('a')),
            "replace returns the old value"
        );
        assert_eq!(table.len(), 2, "replacing doesn't count twice");
        assert_eq!(table.get(5), Some(&'c'), "get");
        assert_eq!(table.get(3), None, "an empty slot");
        assert_eq!(table.get(99), None, "past the end");
        if let Some(value) = table.get_mut(2) {
            *value = 'd';
        }
        assert_eq!(
            table.iter().collect::<Vec<_>>(),
            [(2, &'d'), (5, &'c')],
            "key order"
        );
        assert_eq!(table.remove(2), Some('d'), "remove");
        assert_eq!(table.remove(2), None, "remove twice");
        assert_eq!(table.len(), 1, "len after remove");
        assert_eq!(
            table.insert(usize::MAX, 'e'),
            Err('e'),
            "a key that can't fit"
        );
        assert_eq!(table.len(), 1, "a failed insert changes nothing");
    }

    #[test]
    fn bitset_is_a_stack_of_bits() {
        let mut bits = BitSet::new();
        assert_eq!(bits.pop(), None, "pop on empty");
        for index in 0..130 {
            bits.push(index % 3 == 0);
        }
        assert_eq!(bits.len(), 130, "len");
        assert_eq!(bits.get(129), Some(true), "last bit, third word");
        assert_eq!(bits.get(64), Some(false), "first bit of the second word");
        assert_eq!(bits.get(130), None, "past the end");
        assert!(bits.set(64, true), "set within");
        assert!(!bits.set(130, true), "set past the end");
        assert_eq!(bits.get(64), Some(true), "set took");
        for index in (0..130).rev() {
            assert_eq!(
                bits.pop(),
                Some(index % 3 == 0 || index == 64),
                "pop {index}"
            );
        }
        assert!(bits.is_empty(), "drained");
        assert_eq!(bits.last(), None, "last on empty");
    }
}
