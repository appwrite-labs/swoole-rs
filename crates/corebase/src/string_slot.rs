//! Rust string slots for native configuration views. Copies own independent
//! bytes.

use std::{
    cell::UnsafeCell,
    ffi::{CStr, c_char},
    sync::Arc,
};

/// Borrowed terminated bytes; embedded NULs are included in length.
#[derive(Clone, Copy, Debug)]
pub struct CoreStringView {
    pub data: *const c_char,
    pub length: usize,
}

enum Bytes {
    Inline { data: [u8; 24], length: usize },
    Heap(Vec<u8>),
}
impl Bytes {
    /// A terminated copy of `value`, none when the allocation fails.
    fn copy(value: &[u8]) -> Option<Self> {
        let length = value.len();
        if length < 24 {
            let mut data = [0u8; 24];
            data.get_mut(..length)?.copy_from_slice(value);
            return Some(Self::Inline { data, length });
        }
        let mut data = Vec::new();
        data.try_reserve_exact(length.checked_add(1)?).ok()?;
        data.extend_from_slice(value);
        data.push(0);
        Some(Self::Heap(data))
    }
}
#[repr(transparent)]
pub struct StringSlot {
    bytes: UnsafeCell<Bytes>,
}
// `Send` is the compiler's: the slot owns its bytes.
// SAFETY: sharing exposes no safe mutation or references; C callers synchronize
// accesses.
unsafe impl Sync for StringSlot {}
impl Default for StringSlot {
    fn default() -> Self {
        Self::new()
    }
}
impl StringSlot {
    /// An empty slot, for owners built in a `const fn`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new(Bytes::Inline {
                data: [0; 24],
                length: 0,
            }),
        }
    }
    #[must_use]
    pub fn empty() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Sets the bytes: false when the allocation fails.
    pub fn set(&mut self, value: &[u8]) -> bool {
        Bytes::copy(value).is_some_and(|bytes| {
            *self.bytes.get_mut() = bytes;
            true
        })
    }
    /// [`Self::set`] through a shared slot, as native configuration views do: false
    /// when the allocation fails, which leaves the slot as it was.
    ///
    /// # Safety
    /// Nothing else reads or sets the slot meanwhile, no borrow from
    /// [`Self::bytes`], [`Self::c_str`] or [`Self::view`] outlives the call, and
    /// `value` is not the slot's own bytes.
    pub unsafe fn set_shared(&self, value: &[u8]) -> bool {
        let Some(replacement) = Bytes::copy(value) else {
            return false;
        };
        // SAFETY: the caller holds the slot exclusively; the copy is published only
        // after its allocation succeeded.
        unsafe { *self.bytes.get() = replacement };
        true
    }
    /// The bytes, embedded NULs included; a NUL follows them.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes_with_nul()
            .split_last()
            .map_or(&[], |(_, bytes)| bytes)
    }
    /// The bytes up to the first NUL, as C reads them.
    #[must_use]
    pub fn c_str(&self) -> &CStr {
        CStr::from_bytes_until_nul(self.bytes_with_nul()).unwrap_or_default()
    }
    /// A C view of the bytes; its pointer also covers the NUL after them, which
    /// native readers stop at. Reading through it is the reader's promise that
    /// the slot stays unchanged meanwhile.
    #[must_use]
    pub fn view(&self) -> CoreStringView {
        let bytes = self.bytes_with_nul();
        CoreStringView {
            data: bytes.as_ptr().cast(),
            length: bytes.len().saturating_sub(1),
        }
    }
    /// The bytes and their terminating NUL, which C views read as a C string.
    #[must_use]
    pub fn bytes_with_nul(&self) -> &[u8] {
        // SAFETY: a write through a shared slot is `set_shared`, whose caller holds
        // the slot exclusively, so none overlaps this borrow.
        match unsafe { &*self.bytes.get() } {
            Bytes::Inline { data, length } => data.get(..=*length).unwrap_or_default(),
            Bytes::Heap(data) => data,
        }
    }
}
/// [`StringSlot::view`].
#[must_use]
pub fn swr_v1_core_string_view(slot: &StringSlot) -> CoreStringView {
    slot.view()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn views_keep_embedded_nuls_and_terminate() {
        let mut slot = StringSlot::new();
        assert!(slot.set(b"a\0b"));
        let view = swr_v1_core_string_view(&slot);
        assert_eq!(view.length, 3);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(view.data.cast::<u8>(), 4) },
            b"a\0b\0"
        );
    }
    #[test]
    fn inline_heap_transitions_preserve_bytes() {
        let mut slot = StringSlot::new();
        for length in [0, 23, 24, 4096, 3, 0] {
            let expected = vec![b'x'; length];
            assert!(slot.set(&expected));
            let view = swr_v1_core_string_view(&slot);
            assert_eq!(view.length, length);
            assert_eq!(slot.bytes_with_nul(), [&expected[..], &[0]].concat());
            if length > 2 {
                let inner = expected[1..length - 1].to_vec();
                assert!(slot.set(&inner));
                assert_eq!(swr_v1_core_string_view(&slot).length, length - 2);
                assert_eq!(slot.bytes_with_nul(), [&inner[..], &[0]].concat());
            }
        }
    }
}
