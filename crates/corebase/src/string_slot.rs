//! Rust string slots for native configuration views. Copies own independent
//! bytes.

use std::{cell::UnsafeCell, ffi::c_char, ptr, sync::Arc};

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
pub struct StringSlot {
    bytes: UnsafeCell<Bytes>,
}
// SAFETY: every operation that reads or mutates a slot is unsafe and requires
// external synchronization, matching the native configuration's caller contract.
unsafe impl Send for StringSlot {}
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
}
/// # Safety
/// slot is live; its bytes cannot change while the returned view is used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_string_view(slot: *const StringSlot) -> CoreStringView {
    // SAFETY: the caller lends a live synchronized slot.
    let slot = unsafe { &*slot };
    // SAFETY: the slot's bytes are not concurrently mutated.
    let bytes = unsafe { &*slot.bytes.get() };
    match bytes {
        Bytes::Inline { data, length } => CoreStringView {
            data: data.as_ptr().cast(),
            length: *length,
        },
        Bytes::Heap(data) => CoreStringView {
            data: data.as_ptr().cast(),
            length: data.len() - 1,
        },
    }
}
impl Bytes {
    /// A copy of `bytes` with its own trailing terminator, or none on allocation refusal.
    fn copied(bytes: &[u8]) -> Option<Self> {
        let length = bytes.len();
        if length < 24 {
            let mut data = [0u8; 24];
            data.get_mut(..length)?.copy_from_slice(bytes);
            return Some(Self::Inline { data, length });
        }
        let mut data = Vec::new();
        data.try_reserve_exact(length.checked_add(1)?).ok()?;
        data.extend_from_slice(bytes);
        data.push(0);
        Some(Self::Heap(data))
    }
}
impl StringSlot {
    /// Replaces the bytes; false, leaving them, on allocation refusal.
    pub fn assign(&mut self, bytes: &[u8]) -> bool {
        Bytes::copied(bytes)
            .map(|bytes| *self.bytes.get_mut() = bytes)
            .is_some()
    }
}
/// Fallible byte assignment; 0 success, -2 allocation refusal.
/// Embedded NULs remain part of the value, with a separate trailing terminator.
/// # Safety
/// slot is live/exclusive, bytes readable for length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_string_set(
    slot: *const StringSlot,
    bytes: *const c_char,
    length: usize,
) -> i32 {
    let bytes = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller lends `length` readable bytes, copied before mutation.
        unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), length) }
    };
    let Some(replacement) = Bytes::copied(bytes) else {
        return -2;
    };
    // SAFETY: the slot is live and externally synchronized.
    let slot = unsafe { &*slot };
    // SAFETY: publish only after the allocation succeeded.
    unsafe { *slot.bytes.get() = replacement };
    0
}
/// Independent value-copy for native `auto snapshot = context.field`.
/// # Safety
/// source is live and not concurrently mutated. The returned Arc is released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_string_clone(source: *const StringSlot) -> *const StringSlot {
    let slot = StringSlot::empty();
    // SAFETY: the caller lends the source bytes.
    let view = unsafe { swr_v1_core_string_view(source) };
    // SAFETY: a new exclusive slot, and a still-live source view.
    if unsafe { swr_v1_core_string_set(Arc::as_ptr(&slot), view.data, view.length) } != 0 {
        return ptr::null();
    }
    Arc::into_raw(slot)
}
/// # Safety
/// slot is one owned Arc returned by clone, released once after its view ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_string_free(slot: *const StringSlot) {
    if !slot.is_null() {
        // SAFETY: consumes the caller's owned Arc.
        unsafe { drop(Arc::from_raw(slot)) };
    }
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
            let copied = swr_v1_core_string_clone(pointer);
            assert!(!copied.is_null());
            assert_eq!(swr_v1_core_string_set(pointer, c"changed".as_ptr(), 7), 0);
            assert_eq!(
                std::ffi::CStr::from_ptr(swr_v1_core_string_view(copied).data).to_bytes(),
                b"initial"
            );
            let embedded = b"a\0b";
            assert_eq!(
                swr_v1_core_string_set(pointer, embedded.as_ptr().cast(), embedded.len()),
                0
            );
            let view = swr_v1_core_string_view(pointer);
            assert_eq!(view.length, 3);
            assert_eq!(
                std::slice::from_raw_parts(view.data.cast::<u8>(), 4),
                b"a\0b\0"
            );
            swr_v1_core_string_free(copied);
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
                let view = swr_v1_core_string_view(pointer);
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
                    let view = swr_v1_core_string_view(pointer);
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
