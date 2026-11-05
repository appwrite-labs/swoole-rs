//! Full timer records stay in reusable slots; sifting moves only slot ordinals.
use super::TimerView;
use std::collections::TryReserveError;
use std::ops::{Index, IndexMut};

struct Slot {
    view: TimerView,
    position: usize,
}
#[derive(Clone, Copy)]
struct Key {
    deadline: i64,
    slot: usize,
}
#[derive(Default)]
pub(super) struct Heap {
    slots: Vec<Slot>,
    order: Vec<Key>,
    count: usize,
    // One-based free-list head; zero means no reusable slots.
    free: usize,
}
impl Heap {
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn try_reserve(&mut self, count: usize) -> Result<(), TryReserveError> {
        self.order.try_reserve(count)?;
        if self.free == 0 {
            self.slots.try_reserve(count)?;
        }
        Ok(())
    }
    pub fn slot(&self, index: usize) -> usize {
        self.order[index].slot
    }
    pub fn deadline(&self, index: usize) -> i64 {
        self.order[index].deadline
    }
    pub fn refresh(&mut self, index: usize) {
        self.order[index].deadline = self[index].deadline;
    }
    pub fn position(&self, slot: usize) -> usize {
        self.slots[slot].position
    }
    pub fn place(&mut self, index: usize, slot: usize) {
        self.order[index] = Key {
            slot,
            deadline: self.slots[slot].view.deadline,
        };
        self.slots[slot].position = index;
    }
    pub fn push(&mut self, view: TimerView) {
        let slot = if self.free == 0 {
            self.slots.push(Slot { view, position: 0 });
            self.slots.len() - 1
        } else {
            let slot = self.free - 1;
            self.free = self.slots[slot].position;
            self.slots[slot].view = view;
            slot
        };
        self.order.push(Key {
            slot,
            deadline: view.deadline,
        });
        self.place(self.count, slot);
        self.count += 1;
    }
    pub fn swap_remove(&mut self, index: usize) -> TimerView {
        let slot = self.slot(index);
        let view = self.slots[slot].view;
        self.count -= 1;
        self.order.swap_remove(index);
        if index != self.count {
            let moved = self.slot(index);
            self.slots[moved].position = index;
        }
        self.slots[slot].position = self.free;
        self.free = slot + 1;
        view
    }
    pub fn first(&self) -> Option<&TimerView> {
        (!self.is_empty()).then(|| &self[0])
    }
    pub fn iter(&self) -> impl Iterator<Item = &TimerView> {
        (0..self.count).map(|index| &self[index])
    }
}
impl Index<usize> for Heap {
    type Output = TimerView;
    fn index(&self, index: usize) -> &TimerView {
        assert!(index < self.count);
        &self.slots[self.slot(index)].view
    }
}
impl IndexMut<usize> for Heap {
    fn index_mut(&mut self, index: usize) -> &mut TimerView {
        assert!(index < self.count);
        let slot = self.slot(index);
        &mut self.slots[slot].view
    }
}
