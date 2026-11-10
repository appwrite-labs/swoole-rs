//! Full timer records stay in reusable slots; sifting moves only slot ordinals.
use super::TimerView;
use std::collections::TryReserveError;

struct Slot {
    view: TimerView,
    position: usize,
}
/// `rank` is the deadline alone, and every sift stops on a tie, as upstream's
/// `swoole::Heap` does, so equal deadlines fire in upstream's order: A, C, B for
/// three timers added in that order.
#[derive(Clone, Copy)]
struct Key {
    rank: i64,
    slot: usize,
}
#[derive(Default)]
pub(super) struct Heap {
    slots: Vec<Slot>,
    order: Vec<Key>,
    // One-based free-list head; zero means no reusable slots.
    free: usize,
}
impl Heap {
    pub(super) fn len(&self) -> usize {
        self.order.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    pub(super) fn try_reserve(&mut self, count: usize) -> Result<(), TryReserveError> {
        self.order.try_reserve(count)?;
        if self.free == 0 {
            self.slots.try_reserve(count)?;
        }
        Ok(())
    }
    pub(super) fn slot(&self, index: usize) -> Option<usize> {
        self.order.get(index).map(|key| key.slot)
    }
    pub(super) fn position(&self, slot: usize) -> Option<usize> {
        self.slots.get(slot).map(|entry| entry.position)
    }
    pub(super) fn get(&self, index: usize) -> Option<&TimerView> {
        self.slots.get(self.slot(index)?).map(|entry| &entry.view)
    }
    pub(super) fn first(&self) -> Option<&TimerView> {
        self.get(0)
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = &TimerView> {
        (0..self.len()).filter_map(|index| self.get(index))
    }
    /// Stores the slot's new view and restores heap order around it.
    pub(super) fn update(&mut self, index: usize, view: TimerView) {
        if let Some(slot) = self.slot(index)
            && let Some(entry) = self.slots.get_mut(slot)
        {
            entry.view = view;
        }
        self.repair(index);
    }
    /// Returns the new entry's slot.
    pub(super) fn push(&mut self, view: TimerView) -> usize {
        let mut slot = self.free.wrapping_sub(1);
        if let Some(entry) = self.slots.get_mut(slot) {
            self.free = entry.position;
            entry.view = view;
        } else {
            slot = self.slots.len();
            self.slots.push(Slot { view, position: 0 });
        }
        let index = self.order.len();
        self.order.push(Key {
            slot,
            rank: view.deadline,
        });
        self.sift_up(index);
        slot
    }
    pub(super) fn swap_remove(&mut self, index: usize) -> Option<TimerView> {
        // Upstream's Heap::remove: the last entry takes the hole and sifts from there.
        let removed = (index < self.order.len()).then(|| self.order.swap_remove(index))?;
        self.repair(index);
        let entry = self.slots.get_mut(removed.slot)?;
        entry.position = self.free;
        self.free = removed.slot + 1;
        Some(entry.view)
    }
    fn repair(&mut self, index: usize) {
        let index = self.sift_up(index);
        self.sift_down(index);
    }
    // The ordering key at `index`, with the rank read from its slot's view.
    fn key(&self, index: usize) -> Option<Key> {
        let slot = self.slot(index)?;
        let view = self.slots.get(slot)?.view;
        Some(Key {
            rank: view.deadline,
            slot,
        })
    }
    // Stores `key` at `index`; its rank is its slot's current one.
    fn put(&mut self, index: usize, key: Key) {
        if let Some(entry) = self.order.get_mut(index)
            && let Some(slot) = self.slots.get_mut(key.slot)
        {
            *entry = key;
            slot.position = index;
        }
    }
    // The lower-ranked child of `index` and its key, if it has one.
    fn smaller_child(&self, index: usize) -> Option<(usize, Key)> {
        let left = index.checked_mul(2)?.checked_add(1)?;
        let below_left = *self.order.get(left)?;
        Some(match self.order.get(left + 1) {
            Some(&right) if right.rank < below_left.rank => (left + 1, right),
            _ => (left, below_left),
        })
    }
    fn sift_up(&mut self, mut index: usize) -> usize {
        let Some(key) = self.key(index) else {
            return index;
        };
        while let Some(parent) = index.checked_sub(1).map(|i| i / 2)
            && let Some(&above) = self.order.get(parent)
            && key.rank < above.rank
        {
            self.put(index, above);
            index = parent;
        }
        self.put(index, key);
        index
    }
    fn sift_down(&mut self, mut index: usize) {
        let Some(key) = self.key(index) else {
            return;
        };
        while let Some((child, below)) = self.smaller_child(index)
            && below.rank < key.rank
        {
            self.put(index, below);
            index = child;
        }
        self.put(index, key);
    }
}
