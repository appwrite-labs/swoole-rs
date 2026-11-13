//! Rust string slots for native configuration views. Copies own independent
//! bytes.

use std::{
    cell::UnsafeCell,
    ffi::{CStr, c_char},
    sync::Arc,
};

/// Borrowed terminated bytes; embedded NULs are included in length.
#[repr(C)]
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
    /// Sets the bytes: 0, or -2 when the allocation fails.
    pub fn set(&mut self, value: &[u8]) -> i32 {
        Bytes::copy(value).map_or(-2, |bytes| {
            *self.bytes.get_mut() = bytes;
            0
        })
    }
    /// [`Self::set`] through a shared slot, as native configuration views do: 0, or
    /// -2 when the allocation fails, which leaves the slot as it was.
    ///
    /// # Safety
    /// Nothing else reads or sets the slot meanwhile, no borrow from
    /// [`Self::bytes`], [`Self::c_str`] or [`Self::view`] outlives the call, and
    /// `value` is not the slot's own bytes.
    pub unsafe fn set_shared(&self, value: &[u8]) -> i32 {
        let Some(replacement) = Bytes::copy(value) else {
            return -2;
        };
        // SAFETY: the caller holds the slot exclusively; the copy is published only
        // after its allocation succeeded.
        unsafe { *self.bytes.get() = replacement };
        0
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
/// [`StringSlot::view`], for native code.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_string_view(slot: &StringSlot) -> CoreStringView {
    slot.view()
}
/// [`StringSlot::set_shared`], for native code. Embedded NULs remain part of the
/// value, with a separate trailing terminator.
/// # Safety
/// slot is live/exclusive, bytes readable for length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_string_set(
    slot: *const StringSlot,
    bytes: *const c_char,
    length: usize,
) -> i32 {
    let value = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller lends `length` readable bytes, copied before mutation.
        unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), length) }
    };
    let Some(replacement) = Bytes::copy(value) else {
        return -2;
    };
    // The slot is its cell alone (`repr(transparent)`).
    let current = UnsafeCell::raw_get(slot.cast::<UnsafeCell<Bytes>>());
    // SAFETY: the slot is live and externally synchronized. Publish only after the
    // allocation succeeded; `bytes` may be the slot's own, which `set_shared` would
    // hold borrowed across the write.
    unsafe { *current = replacement };
    0
}
/// Independent value-copy for native `auto snapshot = context.field`; null when
/// the bytes' allocation fails. Native code hands the box back once, to
/// `swr_v1_core_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_string_clone(source: &StringSlot) -> Option<Box<StringSlot>> {
    let mut slot = StringSlot::new();
    (slot.set(source.bytes()) == 0).then(|| Box::new(slot))
}
/// Frees the copy `swr_v1_core_string_clone` made, or nothing for null: native code
/// hands it back once, after its view ends.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_string_free(slot: Option<Box<StringSlot>>) {
    drop(slot);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_are_values() {
        unsafe {
            let slot = StringSlot::empty();
            let pointer = Arc::as_ptr(&slot);
            assert_eq!(swr_v1_core_string_set(pointer, c"initial".as_ptr(), 7), 0);
            let copied = swr_v1_core_string_clone(&slot).unwrap();
            assert_eq!(swr_v1_core_string_set(pointer, c"changed".as_ptr(), 7), 0);
            assert_eq!(
                std::ffi::CStr::from_ptr(swr_v1_core_string_view(&copied).data).to_bytes(),
                b"initial"
            );
            let embedded = b"a\0b";
            assert_eq!(
                swr_v1_core_string_set(pointer, embedded.as_ptr().cast(), embedded.len()),
                0
            );
            let view = swr_v1_core_string_view(&slot);
            assert_eq!(view.length, 3);
            assert_eq!(
                std::slice::from_raw_parts(view.data.cast::<u8>(), 4),
                b"a\0b\0"
            );
            swr_v1_core_string_free(Some(copied));
        }
    }
    #[test]
    fn inline_heap_transitions_and_overlapping_assignments_preserve_bytes() {
        unsafe {
            let slot = StringSlot::empty();
            let pointer = Arc::as_ptr(&slot);
            for length in [0, 23, 24, 4096, 3, 0] {
                let expected = vec![b'x'; length];
                assert_eq!(
                    swr_v1_core_string_set(pointer, expected.as_ptr().cast(), length),
                    0
                );
                let view = swr_v1_core_string_view(&slot);
                assert_eq!(view.length, length);
                assert_eq!(
                    std::slice::from_raw_parts(view.data.cast::<u8>(), length),
                    expected
                );
                assert_eq!(*view.data.add(length), 0);
                if length > 2 {
                    assert_eq!(
                        swr_v1_core_string_set(pointer, view.data.add(1), length - 2),
                        0
                    );
                    let view = swr_v1_core_string_view(&slot);
                    assert_eq!(view.length, length - 2);
                    assert_eq!(
                        std::slice::from_raw_parts(view.data.cast::<u8>(), view.length),
                        &expected[1..length - 1]
                    );
                    assert_eq!(*view.data.add(view.length), 0);
                }
            }
        }
    }
}
