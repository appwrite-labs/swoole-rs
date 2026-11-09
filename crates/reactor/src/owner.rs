//! Private fallible ownership for Rust-only metadata. Native tokens are inert.
use std::alloc::{Layout, alloc};
use std::mem::MaybeUninit;

// Allocate before initialization, retaining each caller's storage/owner order.
pub(crate) fn try_owner<T>(initialize: impl FnOnce() -> T) -> Option<Box<T>> {
    if size_of::<T>() == 0 {
        return Some(Box::new(initialize()));
    }
    // SAFETY: `T` is not zero-sized (handled above); null is handled below.
    let pointer = unsafe { alloc(Layout::new::<T>()).cast::<MaybeUninit<T>>() };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: a non-null global allocation with `T`'s layout; `MaybeUninit` needs no init.
    let mut owner = unsafe { Box::from_raw(pointer) };
    owner.write(initialize());
    // SAFETY: initialized by the `write` just above.
    Some(unsafe { owner.assume_init() })
}
