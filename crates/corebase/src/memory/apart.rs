//! Allocations whose fresh memory is written by another function (#2129, #5274).
//!
//! LLVM turns an allocation followed by an all-zero store into `calloc`, and glibc's
//! `calloc` skips its per-thread cache: several hundred instructions an allocation where
//! `malloc` and a memset cost about 100. The fold needs the allocation and the store in one
//! function, so here the store is in a function that is never inlined, where no compiler
//! update can see the fresh allocation it writes. `std::hint::black_box` did this before,
//! but only as a best effort.
use std::mem::MaybeUninit;

/// `Box::new(make())`, with the allocation here and `make`'s value written by [`write`].
#[must_use]
pub fn boxed<T>(make: impl FnOnce() -> T) -> Box<T> {
    write(Box::new_uninit(), make)
}

/// The box's value, made in place by `make`.
#[inline(never)]
fn write<T>(slot: Box<MaybeUninit<T>>, make: impl FnOnce() -> T) -> Box<T> {
    Box::write(slot, make())
}

/// Zeroes `slot`, a fresh allocation's, in a function the allocating one can't see into.
#[inline(never)]
pub fn zero<T>(slot: &mut MaybeUninit<T>) {
    *slot = MaybeUninit::zeroed();
}
