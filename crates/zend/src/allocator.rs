//! Rust owners on Zend's request heap. All allocations must leave Rust ownership
//! before request shutdown, which reclaims the heap itself.
use std::{
    alloc::{AllocError, Allocator, Layout},
    marker::PhantomData,
    ptr::NonNull,
};

unsafe extern "C" {
    /// Zend returns fresh, eight-byte-aligned storage, never null. Supported PHP
    /// versions defer memory-limit errors; allocation does not longjmp.
    safe fn swr_zend_emalloc(size: usize) -> NonNull<u8>;
    /// Only `deallocate` calls this, with the live allocation its caller owns.
    #[link_name = "swr_zend_efree"]
    safe fn free(allocation: NonNull<u8>);
}

/// Zend's request allocator. Rejects alignment above eight; zero-sized layouts
/// need no Zend allocation. Neither Send nor Sync: its heap belongs to PHP's
/// current request. See `docs/zend.md` for delayed memory-limit errors.
#[derive(Clone, Copy, Debug, Default)]
pub struct Emalloc(PhantomData<*const ()>);

// SAFETY: Zend supplies fresh storage aligned to eight, and `free` releases it.
// Larger alignments are rejected; zero-sized layouts use their aligned dangling
// pointer and are never freed. All instances name the same current request heap.
unsafe impl Allocator for Emalloc {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.align() > 8 {
            return Err(AllocError);
        }
        let allocation = if layout.size() == 0 {
            layout.dangling_ptr()
        } else {
            swr_zend_emalloc(layout.size())
        };
        Ok(NonNull::slice_from_raw_parts(allocation, layout.size()))
    }

    unsafe fn deallocate(&self, allocation: NonNull<u8>, layout: Layout) {
        // The Allocator contract gives this caller ownership of exactly this
        // allocation; Zend needs no size when freeing it.
        if layout.size() != 0 {
            free(allocation);
        }
    }
}
