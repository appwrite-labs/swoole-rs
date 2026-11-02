//! Upstream's thread `Queue`: items behind a mutex, with a condition variable
//! for blocking pops.
use crate::item::Item;
use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

#[derive(Debug, Default)]
pub(crate) struct Queue {
    items: Mutex<VecDeque<Item>>,
    ready: Condvar,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, VecDeque<Item>> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn push(&self, item: Item) {
        self.lock().push_back(item);
    }

    /// `push_notify`.
    pub(crate) fn push_notify(&self, item: Item, all: bool) {
        self.push(item);
        if all {
            self.ready.notify_all();
        } else {
            self.ready.notify_one();
        }
    }

    pub(crate) fn pop(&self) -> Option<Item> {
        self.lock().pop_front()
    }

    /// `pop_wait`: wait up to `timeout` seconds (forever unless positive) for
    /// an item; `None` when another thread took it first or time ran out.
    pub(crate) fn pop_wait(&self, timeout: f64) -> Option<Item> {
        let guard = self.lock();
        let empty = |items: &mut VecDeque<Item>| items.is_empty();
        let mut guard = if timeout > 0.0 {
            match Duration::try_from_secs_f64(timeout) {
                Ok(limit) => {
                    self.ready
                        .wait_timeout_while(guard, limit, empty)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                Err(_) => self
                    .ready
                    .wait_while(guard, empty)
                    .unwrap_or_else(PoisonError::into_inner),
            }
        } else {
            self.ready
                .wait_while(guard, empty)
                .unwrap_or_else(PoisonError::into_inner)
        };
        guard.pop_front()
    }

    pub(crate) fn count(&self) -> usize {
        self.lock().len()
    }

    pub(crate) fn clean(&self) {
        let removed = std::mem::take(&mut *self.lock());
        drop(removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, thread};

    #[test]
    fn pop_wait_wakes_on_push_and_times_out_empty() {
        let queue = Arc::new(Queue::default());
        let producer = Arc::clone(&queue);
        let handle = thread::spawn(move || producer.push_notify(Item::long(7), false));
        let item = queue.pop_wait(5.0);
        handle.join().expect("producer");
        assert!(
            matches!(
                item,
                Some(Item {
                    value: crate::item::Value::Long(7),
                    ..
                })
            ),
            "woken with the item"
        );
        assert!(queue.pop_wait(0.01).is_none(), "times out empty");
        queue.push(Item::long(1));
        queue.push(Item::long(2));
        assert_eq!(queue.count(), 2);
        queue.clean();
        assert!(queue.pop().is_none(), "clean empties");
    }
}
