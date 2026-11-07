//! Single-producer, multi-releaser ring policy over the unchanged shared arena.
use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};

/// Layout invariant (checked by `valid`): `memory` is an 8-aligned arena of `size` bytes.
/// Each record starts at an 8-aligned offset with an 8-byte header (`u16` lock, `u16`
/// count, `u32` length); the lock is the only field consumers touch, and only atomically.
/// `free_count` is a 4-aligned shared counter, also only accessed atomically.
#[repr(C)]
pub struct Ring {
    memory: *mut u8,
    size: u32,
    alloc_offset: u32,
    collect_offset: u32,
    alloc_count: u32,
    status: u8,
    free_count: *mut u32,
}
// Actual C POD shared header. Never borrow the whole object: consumers update
// free_count concurrently while only the producer accesses the cursor fields.
#[repr(C)]
pub struct Storage {
    pub(crate) memory: *mut u8,
    pub(crate) shared: u8,
    status: u8,
    pub(crate) size: u32,
    alloc_offset: u32,
    collect_offset: u32,
    alloc_count: u32,
    pub(crate) free_count: u32,
}
// The address of one `Storage` field, computed without dereferencing `storage`.
fn field<T>(storage: *mut Storage, offset: usize) -> *mut T {
    storage.wrapping_byte_add(offset).cast()
}
impl Ring {
    fn valid(&self) -> bool {
        !self.memory.is_null()
            && self.memory.addr().is_multiple_of(8)
            && self.size >= 16
            && self.alloc_offset <= self.size
            && self.collect_offset <= self.size
            && self.alloc_offset.is_multiple_of(8)
            && self.collect_offset.is_multiple_of(8)
            && !self.free_count.is_null()
            && self.free_count.addr().is_multiple_of(4)
            && self.status <= 1
    }
    unsafe fn collect(&mut self) -> bool {
        // SAFETY: `free_count` is the shared, aligned, atomic-only counter (see `Ring`).
        let count = unsafe { AtomicU32::from_ptr(self.free_count) };
        let mut index = 0;
        // The counter only bounds work; it does not publish any particular
        // record. An out-of-order consumer can supply the count while a
        // different record reaches the front. Its acquire lock load below
        // remains the publication barrier; all counter RMWs remain SeqCst.
        // Deliberately retain upstream's shrinking-bound collection loop.
        while index < count.load(Ordering::Relaxed) {
            if self.collect_offset > self.size - 8 {
                return false;
            }
            // `collect_offset <= size - 8` is 8-aligned, so a whole header fits here.
            let item = self.memory.wrapping_add(self.collect_offset as usize);
            // SAFETY: as above; the lock is aligned and only accessed atomically.
            if unsafe { AtomicU16::from_ptr(item.cast()) }.load(Ordering::Acquire) != 0 {
                break;
            }
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "records start 8-aligned (see Ring)"
            )]
            // SAFETY: as above; the acquire load published this record's header.
            let length = unsafe { item.wrapping_add(4).cast::<u32>().read() };
            let Some(next) = length
                .checked_add(8)
                .and_then(|n| self.collect_offset.checked_add(n))
                .filter(|&n| n <= self.size)
            else {
                return false;
            };
            self.collect_offset = next;
            if self.collect_offset > self.size - 8 || self.collect_offset >= self.size {
                self.collect_offset = 0;
                self.status = 0;
            }
            count.fetch_sub(1, Ordering::SeqCst);
            index += 1;
        }
        true
    }
}

/// A ring's whole arena for `requested` bytes, 8-aligned: its 32-byte header, then
/// at least 16 bytes of records.
pub(crate) fn layout(requested: u32) -> Option<u32> {
    requested
        .checked_add(7)
        .map(|n| n & !7)
        .filter(|&n| n >= 48)
}

/// Initializes a fresh ring's header in `storage`, before any consumer exists.
///
/// # Safety
/// `storage` is aligned and covers `total` exclusive bytes, a `layout` result.
pub(crate) unsafe fn construct(storage: *mut Storage, total: u32, shared: bool) {
    // SAFETY: as the caller promises; the 32-byte header precedes the payload.
    unsafe { ptr::write_bytes(storage.cast::<u8>(), 0, std::mem::size_of::<Storage>()) };
    // SAFETY: as above; the header is now initialized.
    let header = unsafe { &mut *storage };
    header.memory = storage.cast::<u8>().wrapping_add(32);
    header.size = total - 32;
    header.shared = shared.into();
}

/// A record of `requested` bytes, or null.
///
/// # Safety
/// `c`'s `free_count` is the ring's shared, aligned, atomic-only counter.
#[inline(always)]
unsafe fn allocate(c: &mut Ring, requested: u32) -> *mut c_void {
    if requested == 0 || !c.valid() {
        return ptr::null_mut();
    }
    let Some(size) = requested.checked_add(7).map(|n| n & !7) else {
        return ptr::null_mut();
    };
    let Some(total) = size.checked_add(8) else {
        return ptr::null_mut();
    };
    // SAFETY: `c` passed `valid` (above).
    if !unsafe { c.collect() } {
        return ptr::null_mut();
    }
    // SAFETY: `free_count` is the shared, aligned, atomic-only counter (see `Ring`).
    let count = unsafe { AtomicU32::from_ptr(c.free_count) };
    let capacity;
    if c.status == 0 {
        if u64::from(c.alloc_offset) + u64::from(total) >= u64::from(c.size - 8) {
            let skip = c.size - c.alloc_offset;
            if skip >= 8 {
                // `alloc_offset + 8 <= size` (`skip >= 8`), so a whole header fits here.
                let item = c.memory.wrapping_add(c.alloc_offset as usize);
                #[expect(
                    clippy::cast_ptr_alignment,
                    reason = "records start 8-aligned (see Ring)"
                )]
                // SAFETY: as above; only the producer writes this unpublished header.
                unsafe {
                    item.wrapping_add(4).cast::<u32>().write(skip - 8);
                };
                // SAFETY: as above; the aligned lock is only accessed atomically.
                unsafe { AtomicU16::from_ptr(item.cast()) }.store(0, Ordering::Release);
                count.fetch_add(1, Ordering::SeqCst);
            }
            c.alloc_offset = 0;
            c.status = 1;
            capacity = c.collect_offset;
        } else {
            capacity = c.size - c.alloc_offset;
        }
    } else {
        let Some(available) = c.collect_offset.checked_sub(c.alloc_offset) else {
            return ptr::null_mut();
        };
        capacity = available;
    }
    if capacity < total {
        return ptr::null_mut();
    }
    // `capacity >= total` bytes are free at `alloc_offset`, so the header and payload fit.
    let item = c.memory.wrapping_add(c.alloc_offset as usize);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_ptr_alignment,
        reason = "records start 8-aligned (see Ring); the native uint16_t alloc count"
    )]
    // SAFETY: as above; only the producer writes this unpublished header.
    unsafe {
        item.wrapping_add(2)
            .cast::<u16>()
            .write(c.alloc_count as u16);
    };
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "records start 8-aligned (see Ring)"
    )]
    // SAFETY: as above.
    unsafe {
        item.wrapping_add(4).cast::<u32>().write(size);
    };
    // SAFETY: as above; the aligned lock is only accessed atomically.
    unsafe { AtomicU16::from_ptr(item.cast()) }.store(1, Ordering::Release);
    c.alloc_offset += total;
    c.alloc_count = c.alloc_count.wrapping_add(1);
    item.wrapping_add(8).cast()
}
/// `RingBuffer::alloc`: a record of `requested` bytes, or null.
///
/// # Safety
/// `storage` is a constructed ring, allocated from by one producer.
pub(crate) unsafe fn storage_alloc(storage: *mut Storage, requested: u32) -> *mut c_void {
    // As the caller promises, `storage` is live and aligned. Consumers update `free_count`
    // concurrently, so only the producer-owned cursor fields are read and written, by
    // raw field pointer, and `free_count` stays a raw pointer for atomic access.
    // SAFETY: as above.
    let memory = unsafe { field::<*mut u8>(storage, offset_of!(Storage, memory)).read() };
    // SAFETY: as above.
    let size = unsafe { field::<u32>(storage, offset_of!(Storage, size)).read() };
    // SAFETY: as above.
    let alloc_offset = unsafe { field::<u32>(storage, offset_of!(Storage, alloc_offset)).read() };
    // SAFETY: as above.
    let collect_offset =
        unsafe { field::<u32>(storage, offset_of!(Storage, collect_offset)).read() };
    // SAFETY: as above.
    let alloc_count = unsafe { field::<u32>(storage, offset_of!(Storage, alloc_count)).read() };
    // SAFETY: as above.
    let status = unsafe { field::<u8>(storage, offset_of!(Storage, status)).read() };
    let free_count = field::<u32>(storage, offset_of!(Storage, free_count));
    let mut state = Ring {
        memory,
        size,
        alloc_offset,
        collect_offset,
        alloc_count,
        status,
        free_count,
    };
    // SAFETY: `state` is a snapshot of the producer cursor over the live counter.
    let result = unsafe { allocate(&mut state, requested) };
    // A failed allocation can still collect/wrap, exactly as the snapshot ABI.
    // SAFETY: as for the reads above.
    unsafe { field::<u32>(storage, offset_of!(Storage, alloc_offset)).write(state.alloc_offset) };
    // SAFETY: as above.
    unsafe {
        field::<u32>(storage, offset_of!(Storage, collect_offset)).write(state.collect_offset);
    };
    // SAFETY: as above.
    unsafe { field::<u32>(storage, offset_of!(Storage, alloc_count)).write(state.alloc_count) };
    // SAFETY: as above.
    unsafe { field::<u8>(storage, offset_of!(Storage, status)).write(state.status) };
    result
}

// All guards are scalar and can be combined before any memory access. With
// size>=8, a wrapped subtraction is larger than every permitted u32 range.
#[expect(
    clippy::needless_bitwise_bool,
    reason = "branch-free validation of scalar fields; see comment"
)]
fn release_offset(memory: usize, size: u32, count: usize, data: usize) -> Option<usize> {
    let offset = data.wrapping_sub(memory).wrapping_sub(8);
    let aligned = ((memory | data) & 7) | (count & 3) == 0;
    let valid = (memory != 0)
        & (count != 0)
        & (data >= memory)
        & (size >= 8)
        & (offset <= (size as usize).wrapping_sub(8))
        & aligned;
    valid.then_some(offset)
}

/// Releases the live record `data`; false if it is no allocated record of the ring.
///
/// # Safety
/// `memory`, `size` and `free_count` are a constructed ring's.
pub(crate) unsafe fn free(
    memory: *mut c_void,
    size: u32,
    free_count: *mut u32,
    data: *mut c_void,
) -> bool {
    let Some(offset) = release_offset(memory.addr(), size, free_count.addr(), data.addr()) else {
        return false;
    };
    // `release_offset` checked the record header lies inside the 8-aligned arena.
    let item = memory.cast::<u8>().wrapping_add(offset);
    // SAFETY: as above; the aligned lock is only accessed atomically.
    let lock = unsafe { AtomicU16::from_ptr(item.cast()) };
    // Exactly one consumer owns this live record. A second simultaneous
    // release is outside the contract, so no competing writer needs a CAS.
    // Keep the sequential duplicate-release check; the release store publishes
    // completed payload access to the producer's acquire collection load.
    if lock.load(Ordering::Relaxed) != 1 {
        return false;
    }
    lock.store(0, Ordering::Release);
    // SAFETY: `free_count` is non-null and 4-aligned (checked by `release_offset`), shared and
    // only accessed atomically.
    unsafe { AtomicU32::from_ptr(free_count) }.fetch_add(1, Ordering::SeqCst);
    true
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/ring.rs"]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
