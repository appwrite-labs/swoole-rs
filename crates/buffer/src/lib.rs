//! Allocation-fallible owned buffer queue. See docs/buffer-abi-v1.md.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
/// Defines each listed function under its upstream symbol with `upstream-symbols`.
/// `off_t` is `long long` on macOS and `long` on Linux, which the symbols spell.
macro_rules! upstream {
    ($($linux:literal, $macos:literal => $item:item)*) => {$(
        #[cfg_attr(all(feature = "upstream-symbols", not(target_os = "macos")), unsafe(export_name = $linux))]
        #[cfg_attr(all(feature = "upstream-symbols", target_os = "macos"), unsafe(export_name = $macos))]
        $item
    )*};
}

pub mod cpp;
mod typed;
use std::alloc::{Layout, alloc};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::{mem, slice};
pub use typed::{Detached as TypedDetached, Front as TypedFront};

const OK: i32 = 0;
const EMPTY: i32 = 1;
const INVALID: i32 = -1;
const OVERFLOW: i32 = -2;
const NOMEM: i32 = -3;

/// A data chunk's length: chunks hold at most `chunk_size: u32` bytes (`appendv`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "a data chunk holds at most chunk_size: u32 bytes"
)]
pub(crate) fn chunk_length(bytes: &[u8]) -> u32 {
    bytes.len() as u32
}

enum Entry {
    Data(Vec<u8>),
    TypedToken(typed::Token),
}

pub struct Buffer {
    entries: VecDeque<Entry>,
    recycled: Vec<Vec<u8>>,
    recycled_bytes: usize,
    chunk_size: u32,
    length: u32,
    cursor: typed::Cursor,
}

// Validate representability before any slice construction or allocation.
fn slice_fits<T>(count: usize) -> bool {
    count <= (isize::MAX as usize) / mem::size_of::<T>()
}

impl Buffer {
    /// A new queue; chunk size 0 means `INT32_MAX`.
    /// # Errors
    /// `NOMEM` when the allocation fails.
    pub fn create(chunk_size: u32) -> Result<Box<Self>, i32> {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "alloc honours Layout::new::<Buffer>()"
        )]
        // SAFETY: `Buffer` is not zero-sized; null is handled below.
        let handle = unsafe { alloc(Layout::new::<Self>()) }.cast::<Self>();
        if handle.is_null() {
            return Err(NOMEM);
        }
        // SAFETY: a fresh, non-null allocation with `Buffer`'s layout.
        unsafe {
            handle.write(Self {
                entries: VecDeque::new(),
                recycled: Vec::new(),
                recycled_bytes: 0,
                chunk_size: if chunk_size == 0 {
                    i32::MAX as u32
                } else {
                    chunk_size
                },
                length: 0,
                cursor: typed::Cursor::default(),
            });
        };
        // SAFETY: a global allocation with `Box`'s layout, initialized above.
        Ok(unsafe { Box::from_raw(handle) })
    }

    // Limit idle retention to 64 KiB and 64 descriptors per queue. Recycling is
    // best-effort and cannot make detach fail or invoke a native disposer.
    fn recycle(&mut self, mut bytes: Vec<u8>) {
        if self.recycled.len() == self.recycled.capacity()
            || bytes.capacity() > 65536 - self.recycled_bytes
            || self.recycled.len() == 64
        {
            return;
        }
        self.recycled_bytes += bytes.capacity();
        bytes.clear();
        self.recycled.push(bytes);
    }
    fn block(&mut self, size: usize) -> Result<Vec<u8>, i32> {
        if let Some(index) = self
            .recycled
            .iter()
            .rposition(|bytes| bytes.capacity() >= size)
        {
            let bytes = self.recycled.swap_remove(index);
            self.recycled_bytes -= bytes.capacity();
            return Ok(bytes);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| NOMEM)?;
        Ok(bytes)
    }

    /// Copies `data` into new entries, atomically: on failure nothing changes.
    pub fn append(&mut self, data: &[u8]) -> i32 {
        self.appendv(std::iter::once(data), 0)
    }

    /// Queued bytes (including sent ones until their entry is popped) and entries.
    #[must_use]
    pub fn stats(&self) -> (u32, usize) {
        (self.length, self.entries.len())
    }

    /// Drops the queue.
    /// # Errors
    /// The queue itself, unchanged, while it still holds a native token.
    pub fn release(self: Box<Self>) -> Result<(), Box<Self>> {
        if self
            .entries
            .iter()
            .any(|e| matches!(e, Entry::TypedToken(_)))
        {
            return Err(self);
        }
        Ok(())
    }

    // Expose the contiguous append's single part to the optimizer.
    #[inline(always)]
    fn appendv<'a>(&mut self, parts: impl Iterator<Item = &'a [u8]> + Clone, offset: u64) -> i32 {
        let mut total = 0usize;
        for part in parts.clone() {
            let Some(sum) = total.checked_add(part.len()) else {
                return OVERFLOW;
            };
            total = sum;
        }
        if offset > total as u64 {
            return INVALID;
        }
        let Ok(offset) = usize::try_from(offset) else {
            return INVALID;
        };
        let Some(remaining) = total.checked_sub(offset) else {
            return INVALID;
        };
        if remaining == 0 {
            return OK;
        }
        let Ok(copied) = u32::try_from(remaining) else {
            return OVERFLOW;
        };
        let Some(new_length) = self.length.checked_add(copied) else {
            return OVERFLOW;
        };
        let chunk_size = self.chunk_size as usize;
        let chunks = remaining / chunk_size + usize::from(remaining % chunk_size != 0);
        let Some(new_count) = self.entries.len().checked_add(chunks) else {
            return OVERFLOW;
        };
        if !slice_fits::<Entry>(new_count) || !slice_fits::<u8>(remaining.min(chunk_size)) {
            return OVERFLOW;
        }
        let repeated = self.entries.capacity() != 0;
        let old_count = self.entries.len();
        if self.entries.try_reserve(chunks).is_err() {
            return NOMEM;
        }
        let mut parts = parts;
        let mut part = parts.next();
        let mut skip = offset;
        while let Some(current) = part
            && skip >= current.len()
        {
            skip -= current.len();
            part = parts.next();
        }
        let mut left = remaining;
        while left != 0 {
            let size = left.min(chunk_size);
            let mut bytes = match self.block(size) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.entries.truncate(old_count);
                    return error;
                }
            };
            let mut filled = 0;
            while filled < size {
                let Some(current) = part else {
                    self.entries.truncate(old_count);
                    return INVALID;
                };
                let count = (current.len() - skip).min(size - filled);
                // `bytes` has room for `size`, so this never reallocates.
                bytes.extend_from_slice(current.get(skip..skip + count).unwrap_or_default());
                filled += count;
                skip += count;
                if skip == current.len() {
                    part = parts.next();
                    skip = 0;
                }
            }
            self.entries.push_back(Entry::Data(bytes));
            left -= size;
        }
        self.length = new_length;
        // Optional cache admission follows successful payload publication. It
        // cannot change append success or make detach allocate.
        if repeated && self.recycled.capacity() == 0 {
            self.recycled.try_reserve(chunks.min(64)).unwrap_or(());
        }
        OK
    }
}

/// Diagnostics for the actual external C input layout; no conversion credit.
#[cfg(unix)]
#[repr(C)]
pub struct PosixIovecLayout {
    pub size: usize,
    pub alignment: usize,
    pub base_offset: usize,
    pub length_offset: usize,
}
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_buffer_iovec_layout() -> PosixIovecLayout {
    PosixIovecLayout {
        size: mem::size_of::<libc::iovec>(),
        alignment: mem::align_of::<libc::iovec>(),
        base_offset: mem::offset_of!(libc::iovec, iov_base),
        length_offset: mem::offset_of!(libc::iovec, iov_len),
    }
}

/// Copy actual target POSIX iovecs without allocating descriptor staging storage.
/// # Safety
/// `iov` holds `count` descriptors, each over `iov_len` readable bytes (null only when
/// empty) that the queue doesn't hold. No descriptor reference survives the call.
#[cfg(unix)]
#[must_use]
pub unsafe fn swr_v1_buffer_append_iovec(
    queue: &mut Buffer,
    iov: *const libc::iovec,
    count: usize,
    offset: i64,
) -> i32 {
    if offset < 0 || (count != 0 && iov.is_null()) {
        return INVALID;
    }
    if !slice_fits::<libc::iovec>(count) {
        return OVERFLOW;
    }
    let iov = if count == 0 {
        &[]
    } else {
        // SAFETY: non-null, `count` fits isize (checked above), readable descriptors.
        unsafe { slice::from_raw_parts(iov, count) }
    };
    for part in iov {
        if !slice_fits::<u8>(part.iov_len) {
            return OVERFLOW;
        }
        if part.iov_len != 0 && part.iov_base.is_null() {
            return INVALID;
        }
    }
    let parts = iov.iter().map(|part| {
        if part.iov_len == 0 {
            return &[][..];
        }
        // SAFETY: non-null with `iov_len` readable bytes (checked above and as the caller
        // promises), which outlast the append.
        unsafe { slice::from_raw_parts(part.iov_base.cast::<u8>(), part.iov_len) }
    });
    queue.appendv(parts, offset.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, System};
    use std::cell::Cell;
    use std::ptr;

    // Failure injection affects this test thread only, including actual Vec and
    // VecDeque allocation paths. Production has no allocator wrapper or state.
    thread_local! { pub(super) static FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) }; }
    struct FailAllocator;
    fn fail_allocation() -> bool {
        FAIL_AFTER
            .try_with(|counter| match counter.get() {
                None => false,
                Some(0) => true,
                Some(n) => {
                    counter.set(Some(n - 1));
                    false
                }
            })
            .unwrap_or(false)
    }
    unsafe impl GlobalAlloc for FailAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if fail_allocation() {
                ptr::null_mut()
            } else {
                unsafe { System.alloc(layout) }
            }
        }
        unsafe fn realloc(&self, data: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if fail_allocation() {
                ptr::null_mut()
            } else {
                unsafe { System.realloc(data, layout, size) }
            }
        }
        unsafe fn dealloc(&self, data: *mut u8, layout: Layout) {
            unsafe {
                System.dealloc(data, layout);
            }
        }
    }
    #[global_allocator]
    static ALLOCATOR: FailAllocator = FailAllocator;

    pub(super) struct Owned(pub(super) *mut Buffer);
    unsafe extern "C" fn keep(_: *mut c_void) {}
    impl Owned {
        pub(super) fn new(size: u32) -> Self {
            Self(Box::into_raw(Buffer::create(size).unwrap()))
        }
        #[expect(
            clippy::mut_from_ref,
            reason = "the test's one live queue, each borrow used and dropped at once"
        )]
        fn queue(&self) -> &mut Buffer {
            unsafe { &mut *self.0 }
        }
        pub(super) fn push(&self, token: *mut c_void) -> i32 {
            unsafe { self.queue().push(1, token, Some(keep)) }
        }
        pub(super) fn appendv(&self, parts: &[&[u8]], offset: u64) -> i32 {
            self.queue().appendv(parts.iter().copied(), offset)
        }
        pub(super) fn append(&self, bytes: &[u8]) -> i32 {
            self.queue().append(bytes)
        }
        pub(super) fn stats(&self) -> (u32, usize) {
            self.queue().stats()
        }
        pub(super) fn view(&self) -> TypedFront {
            self.queue().front().unwrap()
        }
        pub(super) fn pop(&self) -> *mut c_void {
            self.queue().pop().unwrap().token()
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            assert!(unsafe { Box::from_raw(self.0) }.release().is_ok());
        }
    }

    #[cfg(unix)]
    #[test]
    fn actual_posix_descriptors_share_validation_copy_and_atomic_rollback() {
        let a: [u8; 3] = [0, 255, 7];
        let b: [u8; 4] = [9, 0, 254, 8];
        let vectors = [
            libc::iovec {
                iov_base: a.as_ptr().cast_mut().cast(),
                iov_len: a.len(),
            },
            libc::iovec {
                iov_base: ptr::null_mut(),
                iov_len: 0,
            },
            libc::iovec {
                iov_base: b.as_ptr().cast_mut().cast(),
                iov_len: b.len(),
            },
        ];
        let expected = [0, 255, 7, 9, 0, 254, 8];
        for chunk in [1, 4, 32] {
            for offset in 0..=expected.len() + 1 {
                let owned = Owned::new(chunk);
                let status = unsafe {
                    swr_v1_buffer_append_iovec(
                        owned.queue(),
                        vectors.as_ptr(),
                        vectors.len(),
                        i64::try_from(offset).unwrap(),
                    )
                };
                if offset > expected.len() {
                    assert_eq!(status, INVALID);
                    assert_eq!(owned.stats(), (0, 0));
                    continue;
                }
                assert_eq!(status, OK);
                let mut copied = Vec::new();
                while owned.stats().1 != 0 {
                    let view = owned.view();
                    copied.extend_from_slice(unsafe {
                        slice::from_raw_parts(view.data, view.length as usize)
                    });
                    owned.pop();
                }
                assert_eq!(copied, expected[offset..]);
            }
        }
        let mut succeeded = false;
        for budget in 0..8 {
            let owned = Owned::new(2);
            assert_eq!(owned.append(b"old"), OK);
            let before = owned.stats();
            let front = owned.view();
            FAIL_AFTER.with(|counter| counter.set(Some(budget)));
            let status = unsafe {
                swr_v1_buffer_append_iovec(owned.queue(), vectors.as_ptr(), vectors.len(), 0)
            };
            FAIL_AFTER.with(|counter| counter.set(None));
            if status == OK {
                succeeded = true;
                break;
            }
            assert_eq!(status, NOMEM);
            assert_eq!(owned.stats(), before);
            assert_eq!(owned.view().data, front.data);
            assert_eq!(
                unsafe { slice::from_raw_parts(front.data, front.length as usize) },
                b"ol"
            );
        }
        assert!(succeeded);
        let owned = Owned::new(4);
        assert_eq!(
            unsafe {
                swr_v1_buffer_append_iovec(owned.queue(), vectors.as_ptr(), vectors.len(), -1)
            },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.queue(), ptr::null(), 1, 0) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.queue(), ptr::null(), 0, 0) },
            OK
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.queue(), vectors.as_ptr(), usize::MAX, 0) },
            OVERFLOW
        );
        assert_eq!(owned.stats(), (0, 0));
    }

    #[test]
    fn vector_offsets_empty_descriptors_and_binary_chunks() {
        let a = [0, 1, 2, 3, 4];
        let b = [255, 0, 253, 252, 251, 250];
        let iov: [&[u8]; 5] = [&[], &a, &[], &b, &[]];
        let combined: Vec<u8> = a.into_iter().chain(b).collect();
        for offset in 0..=combined.len() {
            let buffer = Owned::new(4);
            let contiguous = Owned::new(4);
            assert_eq!(contiguous.append(&combined[offset..]), OK);
            assert_eq!(buffer.appendv(&iov, offset as u64), OK);
            assert_eq!(buffer.stats(), contiguous.stats());
            let mut actual = Vec::new();
            while buffer.stats().1 != 0 {
                let view = buffer.view();
                assert!(view.length > 0 && view.length <= 4);
                actual.extend_from_slice(unsafe {
                    slice::from_raw_parts(view.data, view.length as usize)
                });
                let direct = contiguous.view();
                assert_eq!(direct.length, view.length);
                assert_eq!(
                    unsafe { slice::from_raw_parts(direct.data, direct.length as usize) },
                    unsafe { slice::from_raw_parts(view.data, view.length as usize) }
                );
                assert!(contiguous.pop().is_null());
                assert!(buffer.pop().is_null());
            }
            assert_eq!(actual, combined[offset..]);
            assert_eq!(buffer.stats(), (0, 0));
            assert_eq!(contiguous.stats(), (0, 0));
        }
    }

    #[test]
    fn append_keeps_views_writable_and_stable_across_queue_growth() {
        let buffer = Owned::new(3);
        assert_eq!(buffer.append(b"abc"), OK);
        let front = buffer.view();
        unsafe {
            front.data.write(b'Z');
        }
        for _ in 0..100 {
            assert_eq!(buffer.append(b"defghi"), OK);
        }
        assert_eq!(buffer.view().data, front.data);
        assert_eq!(unsafe { slice::from_raw_parts(front.data, 3) }, b"Zbc");
        assert_eq!(buffer.stats(), (603, 201));
    }

    #[test]
    fn native_tokens_preserve_order_and_block_release() {
        let buffer = Owned::new(4);
        let mut value = 42u32;
        let token = ptr::from_mut(&mut value).cast();
        assert_eq!(buffer.append(b"abcdef"), OK);
        assert_eq!(buffer.push(token), OK);
        assert_eq!(buffer.append(b"g"), OK);
        let kept = Box::into_raw(unsafe { Box::from_raw(buffer.0) }.release().unwrap_err());
        assert_eq!(kept, buffer.0);
        assert_eq!(buffer.stats(), (7, 4));
        assert!(buffer.pop().is_null());
        assert!(buffer.pop().is_null());
        assert_eq!(buffer.stats(), (1, 2));
        let view = buffer.view();
        assert_eq!(view.kind, 1);
        assert_eq!(view.token, token);
        assert!(view.data.is_null());
        assert_eq!(buffer.pop(), token);
        assert_eq!(buffer.stats(), (1, 1));
    }

    #[test]
    fn bad_inputs_do_not_change_state_and_outputs_follow_contract() {
        let buffer = Owned::new(4);
        assert_eq!(buffer.append(b"abc"), OK);
        assert_eq!(buffer.appendv(&[b"x"], 2), INVALID);
        assert_eq!(buffer.appendv(&[], 1), INVALID);
        assert_eq!(buffer.appendv(&[], 0), OK);
        assert_eq!(buffer.append(&[]), OK);
        assert_eq!(buffer.push(ptr::null_mut()), INVALID);
        assert_eq!(buffer.stats(), (3, 1));
        buffer.pop();
        assert_eq!(buffer.queue().front().err(), Some(EMPTY));
        assert_eq!(buffer.queue().pop().err(), Some(EMPTY));
    }

    #[test]
    fn total_overflow_rejected_before_reading_input() {
        let buffer = Owned::new(4);
        // Inject accounting internally to exercise UINT32_MAX without 4 GiB.
        unsafe {
            (*buffer.0).length = u32::MAX;
        }
        assert_eq!(buffer.append(b"x"), OVERFLOW);
        assert_eq!(buffer.stats(), (u32::MAX, 0));
        unsafe {
            (*buffer.0).length = 0;
        }
    }

    #[test]
    fn single_chunk_failure_keeps_front_and_accounting_unchanged() {
        for populated in [false, true] {
            for fail_after in 0..=2 {
                let buffer = Owned::new(4096);
                if populated {
                    assert_eq!(buffer.append(b"old"), OK);
                    // Force queue growth as well as a data allocation.
                    while unsafe { (*buffer.0).entries.len() < (*buffer.0).entries.capacity() } {
                        assert_eq!(buffer.append(b"old"), OK);
                    }
                }
                let before = buffer.stats();
                let front = if populated { Some(buffer.view()) } else { None };
                FAIL_AFTER.set(Some(fail_after));
                let status = buffer.append(b"new");
                FAIL_AFTER.set(None);
                if fail_after < 2 {
                    assert_eq!(status, NOMEM);
                    assert_eq!(buffer.stats(), before);
                } else {
                    assert_eq!(status, OK);
                    assert_eq!(buffer.stats(), (before.0 + 3, before.1 + 1));
                }
                if let Some(front) = front {
                    assert_eq!(buffer.view().data, front.data);
                    assert_eq!(unsafe { slice::from_raw_parts(front.data, 3) }, b"old");
                }
            }
        }
        // A drained queue retains metadata capacity. Only its new bytes should
        // need allocation, so failure of any second allocation must be harmless.
        let reused = Owned::new(4096);
        assert_eq!(reused.append(b"old"), OK);
        reused.pop();
        FAIL_AFTER.set(Some(1));
        let status = reused.append(b"new");
        FAIL_AFTER.set(None);
        assert_eq!(status, OK);
        assert_eq!(reused.stats(), (3, 1));
        assert_eq!(
            unsafe { slice::from_raw_parts(reused.view().data, 3) },
            b"new"
        );
    }

    #[test]
    #[expect(
        clippy::cognitive_complexity,
        reason = "one scenario per allocation failure point"
    )]
    fn every_allocation_failure_is_atomic() {
        FAIL_AFTER.set(Some(0));
        let status = Buffer::create(4).err();
        FAIL_AFTER.set(None);
        assert_eq!(status, Some(NOMEM));
        let mut failures = 0;
        for nth in 0..32 {
            let buffer = Owned::new(4);
            assert_eq!(buffer.append(b"abc"), OK);
            let view = buffer.view();
            FAIL_AFTER.set(Some(nth));
            let status = buffer.append(b"0123456789abcdefghijklmnop");
            FAIL_AFTER.set(None);
            if status == OK {
                break;
            }
            assert_eq!(status, NOMEM);
            assert_eq!(buffer.stats(), (3, 1));
            assert_eq!(buffer.view().data, view.data);
            assert_eq!(unsafe { slice::from_raw_parts(view.data, 3) }, b"abc");
            failures += 1;
        }
        assert!(
            failures >= 8,
            "must visit metadata, every chunk, and queue-growth allocations"
        );
        // Exercise rollback after metadata growth from a wrapped queue with a
        // native token between existing data. No provisional entry may escape.
        for nth in 0..=8 {
            let buffer = Owned::new(4);
            assert_eq!(buffer.append(b"abcdefghijkl"), OK);
            while unsafe { (*buffer.0).entries.len() < (*buffer.0).entries.capacity() } {
                assert_eq!(buffer.append(b"fill"), OK);
            }
            buffer.pop();
            buffer.pop();
            let mut native = 7u32;
            let token = ptr::from_mut(&mut native).cast();
            assert_eq!(buffer.push(token), OK);
            assert_eq!(buffer.append(b"old"), OK);
            assert!(!unsafe { &*buffer.0 }.entries.as_slices().1.is_empty());
            // Keep the cold-allocation sweep visiting every payload allocation;
            // warm-cache behavior is exercised separately.
            unsafe {
                (*buffer.0).recycled.clear();
                (*buffer.0).recycled_bytes = 0;
            }
            let before = buffer.stats();
            let front = buffer.view();
            FAIL_AFTER.set(Some(nth));
            let status = buffer.append(b"0123456789abcdefghijklmnop");
            FAIL_AFTER.set(None);
            assert_eq!(status, if nth < 8 { NOMEM } else { OK });
            assert_eq!(
                buffer.stats(),
                if status == NOMEM {
                    before
                } else {
                    (before.0 + 26, before.1 + 7)
                }
            );
            assert_eq!(buffer.view().data, front.data);
            assert_eq!(unsafe { slice::from_raw_parts(front.data, 4) }, b"ijkl");
            assert!(matches!(
                unsafe { &*buffer.0 }.entries.get(before.1 - 2),
                Some(Entry::TypedToken(_))
            ));
            let mut returned = 0;
            while buffer.stats().1 != 0 {
                let popped = buffer.pop();
                if !popped.is_null() {
                    assert_eq!(popped, token);
                    returned += 1;
                }
            }
            assert_eq!(returned, 1);
        }
        let empty = Owned::new(4);
        let mut native = 7;
        FAIL_AFTER.set(Some(0));
        let status = empty.push(ptr::from_mut(&mut native).cast());
        FAIL_AFTER.set(None);
        assert_eq!(status, NOMEM);
        assert_eq!(empty.stats(), (0, 0));
    }
}

pub mod string;

#[cfg(test)]
#[path = "../../../tests/modules/buffer/recycling.rs"]
mod recycling_tests;
