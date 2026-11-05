//! Bounded, stable storage for small Copy records with opaque native cursors.
use std::alloc::{Layout, alloc, dealloc};
use std::ptr::{self, NonNull};

const SLOTS: usize = 128;

pub struct Pool<T: Copy> {
    block: *mut T,
    free: *mut T,
    started: bool,
}
impl<T: Copy> Default for Pool<T> {
    fn default() -> Self {
        assert!(size_of::<T>() >= size_of::<*mut T>());
        assert!(align_of::<T>() >= align_of::<*mut T>());
        Self {
            block: ptr::null_mut(),
            free: ptr::null_mut(),
            started: false,
        }
    }
}
impl<T: Copy> Pool<T> {
    pub fn insert(&mut self, value: T) -> Option<NonNull<T>> {
        // Avoid allocating/initializing a slab for a one-record owner.
        if !self.started {
            let slot = NonNull::new(unsafe { alloc(Layout::new::<T>()).cast::<T>() })?;
            unsafe { slot.as_ptr().write(value) };
            self.started = true;
            return Some(slot);
        }
        if self.block.is_null() {
            self.block = unsafe { alloc(Layout::array::<T>(SLOTS).ok()?).cast() };
            if self.block.is_null() {
                return None;
            }
            for i in (0..SLOTS).rev() {
                let slot = unsafe { self.block.add(i) };
                unsafe { slot.cast::<*mut T>().write(self.free) };
                self.free = slot;
            }
        }
        let slot = if self.free.is_null() {
            NonNull::new(unsafe { alloc(Layout::new::<T>()).cast::<T>() })?
        } else {
            let slot = self.free;
            self.free = unsafe { slot.cast::<*mut T>().read() };
            unsafe { NonNull::new_unchecked(slot) }
        };
        unsafe { slot.as_ptr().write(value) };
        Some(slot)
    }
    /// # Safety
    /// `slot` is a live allocation from this pool, retired exactly once. No
    /// cursor may access it afterwards. All live slots must be retired on drop.
    pub unsafe fn remove(&mut self, slot: NonNull<T>) {
        let address = slot.as_ptr().addr();
        if address >= self.block.addr() && address < self.block.addr() + size_of::<T>() * SLOTS {
            unsafe { slot.as_ptr().cast::<*mut T>().write(self.free) };
            self.free = slot.as_ptr();
        } else {
            unsafe { dealloc(slot.as_ptr().cast(), Layout::new::<T>()) };
        }
    }
}
impl<T: Copy> Drop for Pool<T> {
    fn drop(&mut self) {
        if !self.block.is_null() {
            unsafe { dealloc(self.block.cast(), Layout::array::<T>(SLOTS).unwrap()) };
        }
    }
}

#[cfg(test)]
#[path = "../tests/modules/storage/pool.rs"]
mod tests;
