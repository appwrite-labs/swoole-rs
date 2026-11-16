//! Non-persistent `zend_string`s: allocated here, handed to Zend by a return
//! value or an array, or freed.
use crate::sys::{zend_string, zend_ulong};
use std::{io::IoSlice, mem::offset_of};
use swoole_rs_corebase::lent::{Spare, SpareVectors, lend_spare, lend_spare_vectors};

unsafe extern "C" {
    /// `zend_hash_func`: it reads the lent bytes (an `IoSlice` is an iovec) only.
    safe fn swr_bytes_hash(key: IoSlice<'_>) -> zend_ulong;
    /// `zend_string_alloc(length, 0)`: a new string's address, from Zend's
    /// request allocator, which never returns null nor, past `memory_limit`,
    /// bails out (docs/zend.md, `memory_limit`). It takes any length and reads
    /// nothing: the shim aborts on one whose header and NUL would wrap.
    safe fn swr_zend_string_alloc(length: usize) -> *mut zend_string;
}

/// `zend_hash_func`: the hash a `zend_string` of these bytes caches
/// (`zend_string_hash_val`), whatever length of it the caller compares.
#[must_use]
pub fn hash(key: &[u8], _compared: usize) -> u64 {
    swr_bytes_hash(IoSlice::new(key))
}

/// `ZSTR_VAL`: the first byte.
#[must_use]
#[inline]
pub fn val(string: *mut zend_string) -> *mut u8 {
    string
        .wrapping_byte_add(offset_of!(zend_string, val))
        .cast()
}

/// `zend::fetch_zend_string_by_val`: the string whose `ZSTR_VAL` is `val`.
#[must_use]
#[inline]
pub fn from_val(val: *mut u8) -> *mut zend_string {
    val.wrapping_byte_sub(offset_of!(zend_string, val)).cast()
}

/// `zend_string_alloc(length, 0)`: room for `length` bytes and a NUL.
/// The shim aborts if the length plus the string header and NUL exceeds
/// `PTRDIFF_MAX`, before doing the allocation.
#[must_use]
#[inline]
pub fn alloc(length: usize) -> *mut zend_string {
    swr_zend_string_alloc(length)
}

/// `zend_string_init(bytes, len, 0)`: a new string holding a copy of `bytes`.
#[must_use]
pub fn copy(bytes: &[u8]) -> *mut zend_string {
    let mut buffer = ZBuf::with_capacity(bytes.len());
    buffer.extend_from_slice(bytes);
    buffer.into_zstring().into_raw()
}

/// `zend_string_free` of a live, unshared, non-interned string nothing else references.
pub use crate::sys::swr_zend_string_free as free;
/// `zend_string_realloc(string, length, 0)` of a live, unshared, non-interned string.
pub use crate::sys::swr_zend_string_realloc as realloc;

/// A new, unshared `zend_string` (`zend_string_alloc`) being filled, like a
/// `Vec<u8>`: something writes at `as_mut_ptr() + len()`, `set_len` counts
/// what it wrote, and `into_zstring` ends the string there. Dropped first, it
/// is freed. It is no `ZStr`: the bytes past `len` are uninitialised.
pub struct ZBuf {
    string: *mut zend_string,
    len: usize,
    capacity: usize,
}

impl ZBuf {
    /// Room for `capacity` bytes, none written.
    #[must_use]
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            string: alloc(capacity),
            len: 0,
            capacity,
        }
    }

    /// Take over `string`, holding `len` written bytes and room for no more.
    /// # Safety
    /// `string` is a live, unshared, non-interned string nothing else
    /// references, whose `ZSTR_LEN` is at least `len`, and at least `len`
    /// bytes of which are written.
    #[must_use]
    #[inline]
    pub unsafe fn from_raw(string: *mut zend_string, len: usize) -> Self {
        Self {
            string,
            len,
            capacity: len,
        }
    }

    /// The first byte; `capacity()` bytes are writable from here.
    #[must_use]
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        val(self.string)
    }

    #[must_use]
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The room past the written bytes, then the byte for the NUL: a string
    /// whose `ZSTR_LEN` is `capacity` holds a NUL after its bytes.
    #[inline]
    fn room(&mut self) -> &mut [std::mem::MaybeUninit<u8>] {
        let (len, room) = (self.len, self.capacity + 1 - self.len);
        let at = self.as_mut_ptr().wrapping_add(len).cast();
        // SAFETY: the string's own allocation holds `capacity` bytes and the
        // NUL's from its first, `len <= capacity` of them written (`set_len`'s
        // contract), so `room` bytes follow `at`; the exclusive borrow is the
        // only way to them, and unwritten bytes are valid `MaybeUninit`s.
        unsafe { std::slice::from_raw_parts_mut(at, room) }
    }

    /// The room past the written bytes, as `Vec::spare_capacity_mut`: what a
    /// reader fills before [`ZBuf::set_len`] counts it.
    #[must_use]
    #[inline]
    pub fn spare_capacity_mut(&mut self) -> &mut [std::mem::MaybeUninit<u8>] {
        self.room()
            .split_last_mut()
            .map_or(&mut [][..], |(_nul, spare)| spare)
    }

    /// Count the first `len` bytes as written.
    /// # Safety
    /// `len <= capacity()` and the first `len` bytes are written.
    #[inline]
    pub unsafe fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    /// Appends what `fill` writes into the room past the written bytes: what it
    /// returned.
    #[inline]
    pub fn fill<R>(&mut self, fill: impl for<'s> FnOnce(&mut Spare<'s>) -> R) -> R {
        self.fill_within(usize::MAX, fill)
    }

    /// [`fill`](Self::fill) of at most `limit` bytes.
    #[inline]
    pub fn fill_within<R>(
        &mut self,
        limit: usize,
        fill: impl for<'s> FnOnce(&mut Spare<'s>) -> R,
    ) -> R {
        let room = self.spare_capacity_mut();
        let limit = limit.min(room.len());
        let (result, filled) = lend_spare(room.get_mut(..limit).unwrap_or_default(), fill);
        self.len += filled;
        result
    }

    /// Lends these buffers' remaining rooms to one vectored receive, counting
    /// the initialized prefix across them in order.
    pub fn fill_vectored<R>(
        buffers: &mut [Self],
        fill: impl for<'s> FnOnce(&mut SpareVectors<'s>) -> R,
    ) -> R {
        let (result, mut filled) =
            lend_spare_vectors(buffers.iter_mut().map(Self::spare_capacity_mut), fill);
        for buffer in buffers {
            let count = filled.min(buffer.capacity - buffer.len);
            buffer.len += count;
            filled -= count;
        }
        result
    }

    /// Count only the first `len` bytes as written; nothing when fewer are.
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    /// Append as much of `bytes` as there is room for.
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        let room = self.spare_capacity_mut();
        let count = bytes.len().min(room.len());
        if let (Some(room), Some(bytes)) = (room.get_mut(..count), bytes.get(..count)) {
            room.write_copy_of_slice(bytes);
        }
        self.len += count;
    }

    /// `zend_string_realloc` down to the written bytes.
    pub fn shrink_to_fit(&mut self) {
        // SAFETY: an unshared string; Zend copies the written bytes.
        self.string = unsafe { realloc(self.string, self.len) };
        self.capacity = self.len;
    }

    /// End the string after the written bytes and hand over its reference.
    #[must_use]
    pub fn into_zstring(mut self) -> crate::ZString {
        // `ZSTR_VAL(string)[len] = '\0'`, in the room's first byte.
        if let Some(nul) = self.room().first_mut() {
            nul.write(0);
        }
        let this = std::mem::ManuallyDrop::new(self);
        // The string's only reference, which this gives up.
        let string = crate::ZString(this.string);
        // `ZSTR_LEN(string) = len`: its length is `capacity`, at least `len`.
        string.shorten(this.len);
        string
    }
}

impl Drop for ZBuf {
    /// Released as the string it is so far, through its only reference.
    #[inline]
    fn drop(&mut self) {
        drop(crate::ZString(self.string));
    }
}
