//! Bounded, stable storage for small Copy records with opaque native cursors.
use std::alloc::{Layout, alloc, dealloc};
use std::ptr::{self, NonNull};

const SLOTS: usize = 128;

pub(crate) struct Pool<T: Copy> {
    block: *mut T,
    free: *mut T,
    started: bool,
}
impl<T: Copy> Default for Pool<T> {
    fn default() -> Self {
        assert!(
            size_of::<T>() >= size_of::<*mut T>(),
            "free link must fit in a slot"
        );
        assert!(
            align_of::<T>() >= align_of::<*mut T>(),
            "free link must be aligned"
        );
        Self {
            block: ptr::null_mut(),
            free: ptr::null_mut(),
            started: false,
        }
    }
}
impl<T: Copy> Pool<T> {
    pub(crate) fn insert(&mut self, value: T) -> Option<NonNull<T>> {
        // Avoid allocating/initializing a slab for a one-record owner.
        if !self.started {
            // SAFETY: T is non-zero-sized; null is handled by NonNull.
            let slot = NonNull::new(unsafe { alloc(Layout::new::<T>()).cast::<T>() })?;
            // SAFETY: this allocation has T's layout and is exclusively owned.
            unsafe { slot.as_ptr().write(value) };
            self.started = true;
            return Some(slot);
        }
        if self.block.is_null() {
            // SAFETY: a checked non-zero layout; null is handled below.
            self.block = unsafe { alloc(Layout::array::<T>(SLOTS).ok()?).cast() };
            if self.block.is_null() {
                return None;
            }
            for i in (0..SLOTS).rev() {
                // SAFETY: i is in the SLOTS-element allocation.
                let slot = unsafe { self.block.add(i) };
                // SAFETY: default checked pointer size/alignment; this free slot has no live T.
                unsafe { slot.cast::<*mut T>().write(self.free) };
                self.free = slot;
            }
        }
        let slot = if self.free.is_null() {
            // SAFETY: T is non-zero-sized; null is handled by NonNull.
            NonNull::new(unsafe { alloc(Layout::new::<T>()).cast::<T>() })?
        } else {
            let slot = self.free;
            // SAFETY: free slots contain initialized, pointer-aligned free-list links.
            self.free = unsafe { slot.cast::<*mut T>().read() };
            // SAFETY: this branch was entered only with a non-null free-list head.
            unsafe { NonNull::new_unchecked(slot) }
        };
        // SAFETY: a fresh allocation or free slab slot, exclusively owned and aligned for T.
        unsafe { slot.as_ptr().write(value) };
        Some(slot)
    }
    /// # Safety
    /// `slot` is a live allocation from this pool, retired exactly once. No
    /// cursor may access it afterwards. All live slots must be retired on drop.
    pub(crate) unsafe fn remove(&mut self, slot: NonNull<T>) {
        let address = slot.as_ptr().addr();
        if !self.block.is_null()
            && address >= self.block.addr()
            && address < self.block.addr() + size_of::<T>() * SLOTS
        {
            // SAFETY: this retired slab slot has pointer size/alignment and no surviving cursor.
            unsafe { slot.as_ptr().cast::<*mut T>().write(self.free) };
            self.free = slot.as_ptr();
        } else {
            // SAFETY: a live fallback allocation issued with T's layout, retired once.
            unsafe { dealloc(slot.as_ptr().cast(), Layout::new::<T>()) };
        }
    }
}
impl<T: Copy> Drop for Pool<T> {
    fn drop(&mut self) {
        if !self.block.is_null() {
            // SAFETY: the slab was allocated with this checked layout, and is freed once.
            unsafe {
                dealloc(
                    self.block.cast(),
                    Layout::array::<T>(SLOTS)
                        .unwrap_or_else(|_| unreachable!("slab was allocated with this layout")),
                );
            };
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/pool.rs"]
mod tests;
