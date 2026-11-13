//! `GlobalMemory`'s pages: a bump allocator over pages from `sw_shm_malloc` (shared)
//! or `sw_malloc`, freed with the pool.
use crate::members::{arena, release};
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use swoole_rs_corebase::CCast;

/// Page invariant: every entry of `pages` is a live, 8-aligned `page_size`-byte page
/// this registry got from `arena(shared, ..)` and frees on drop, and
/// `offset <= page_size` counts the bytes already allocated in the last page.
pub(crate) struct Global {
    pages: Vec<NonNull<u8>>,
    pub(crate) page_size: u32,
    offset: usize,
    shared: bool,
}

/// `requested` rounded up to whole `system` pages, as `SW_MEM_ALIGNED_SIZE_EX`.
pub(crate) fn page_size(requested: u32, system: u32) -> Option<u32> {
    if !system.is_power_of_two() || system < 8 {
        return None;
    }
    requested.checked_add(system - 1).map(|n| n & !(system - 1))
}

/// A block's payload, 8-aligned, and with its 8-byte header.
fn block(requested: u32) -> Option<(usize, usize)> {
    let size = (requested as usize).checked_add(7)? & !7;
    Some((size, size.checked_add(8)?))
}

impl Global {
    /// A registry of `page_size`-byte pages, with no pages.
    pub(crate) fn new(page_size: u32, shared: bool) -> Self {
        Self {
            pages: Vec::new(),
            page_size,
            offset: 0,
            shared,
        }
    }

    /// `GlobalMemoryImpl::new_page()`: false if the page can't be had.
    pub(crate) fn add_page(&mut self) -> bool {
        if self.pages.try_reserve(1).is_err() {
            return false;
        }
        let Some(page) = NonNull::new(arena(self.shared, self.page_size as usize).cast()) else {
            return false;
        };
        self.pages.push(page);
        self.offset = 0;
        true
    }

    /// Whether a block of `requested` bytes fits in a page.
    pub(crate) fn fits(&self, requested: u32) -> bool {
        block(requested).is_some_and(|(_, total)| total <= self.page_size as usize)
    }

    /// A zeroed block of `requested` bytes, which `fits`, from the last page or a new
    /// one; null if a new page can't be had.
    pub(crate) fn alloc(&mut self, requested: u32) -> *mut c_void {
        let Some((size, total)) = block(requested) else {
            return ptr::null_mut();
        };
        if (self.pages.is_empty() || self.offset + total > self.page_size as usize)
            && !self.add_page()
        {
            return ptr::null_mut();
        }
        let Some(page) = self.pages.last() else {
            return ptr::null_mut();
        };
        // `offset + total <= page_size` (`fits`, and checked above), so the 8-byte header
        // and `size` payload bytes lie in the last page (see `Global`); `offset` stays
        // 8-aligned.
        let block = page.as_ptr().wrapping_add(self.offset);
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "an 8-aligned native block header storing a sub-page uint32_t size"
        )]
        // SAFETY: as above.
        unsafe {
            block.cast::<u32>().write(size.c_cast::<u32>());
        };
        // SAFETY: as above.
        unsafe { ptr::write_bytes(block.wrapping_add(8), 0, size) };
        self.offset += total;
        block.wrapping_add(8).cast()
    }

    /// `GlobalMemory::capacity()`.
    pub(crate) fn capacity(&self) -> usize {
        self.page_size as usize - self.offset
    }

    /// `GlobalMemory::get_memory_size()`.
    pub(crate) fn memory_size(&self) -> usize {
        self.pages.len().saturating_mul(self.page_size as usize)
    }
}

impl Drop for Global {
    fn drop(&mut self) {
        for page in self.pages.drain(..) {
            // SAFETY: a live page `arena(shared, ..)` made (see `Global`), freed once.
            unsafe { release(self.shared, page.as_ptr().cast()) };
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/global.rs"]
mod tests;
