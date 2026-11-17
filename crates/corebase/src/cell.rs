/// Exclusive access to a cell's contents while callbacks may re-enter its owner.
pub trait CellExt<T> {
    /// Leaves the default in the cell during the call. On return, restores the
    /// edited value before dropping anything a nested call stored. On unwind,
    /// the local value drops and the cell retains its current contents.
    fn with_taken<R>(&self, f: impl FnOnce(&mut T) -> R) -> R;
}

impl<T: Default> CellExt<T> for core::cell::Cell<T> {
    fn with_taken<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.take();
        let result = f(&mut value);
        self.set(value);
        result
    }
}

/// Optional boxed state. An empty cell stays untouched, even if dropping an
/// unused callback re-enters its owner.
pub trait BoxCellExt<T: ?Sized> {
    fn with_box<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R>;
    fn restore_box(&self, value: Box<T>);
}

impl<T: ?Sized> BoxCellExt<T> for core::cell::Cell<Option<Box<T>>> {
    fn with_box<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut value = self.take()?;
        let result = f(&mut value);
        self.restore_box(value);
        Some(result)
    }

    fn restore_box(&self, value: Box<T>) {
        if let Some(nested) = self.replace(Some(value)) {
            release(nested);
        }
    }
}

// Keep the nested owner's drop glue off the normal path that replaces None.
#[cold]
#[inline(never)]
fn release<T>(value: T) {
    drop(value);
}

#[cfg(test)]
mod tests {
    use super::{BoxCellExt, CellExt};
    use std::cell::Cell;

    #[test]
    fn nested_access_sees_the_default_and_outer_changes_win_on_return() {
        let slot = Cell::new(vec![1]);
        slot.with_taken(|outer| {
            slot.with_taken(|inner| {
                assert_eq!(inner.as_slice(), []);
                inner.push(2);
            });
            outer.push(3);
        });
        assert_eq!(slot.into_inner(), [1, 3]);
    }

    #[test]
    fn dropping_an_unused_callback_can_fill_an_empty_slot() {
        struct Fill<'a>(&'a Cell<Option<Box<u8>>>);
        impl Drop for Fill<'_> {
            fn drop(&mut self) {
                self.0.set(Some(Box::new(7)));
            }
        }
        let slot = Cell::default();
        let fill = Fill(&slot);
        assert!(slot.with_box(|_| drop(fill)).is_none());
        assert_eq!(slot.into_inner().as_deref(), Some(&7));
    }
}
