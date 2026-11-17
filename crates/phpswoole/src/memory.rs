//! The `swoole::Allocator` whose buffers are the
//! `ZSTR_VAL` of `zend_string`s, so a received message becomes a PHP string
//! without a copy.
use std::{ffi::c_void, mem::MaybeUninit};
use swoole_rs_corebase::{CoreAllocator, TrustedAllocator};
use swoole_rs_zend::{
    string::{self, ZBuf, from_val, val},
    sys::swr_zend_string_release,
};

static ZEND_STRING_ALLOCATOR: CoreAllocator = CoreAllocator {
    malloc: Some(malloc),
    calloc: Some(calloc),
    realloc: Some(realloc),
    free: Some(free),
};

/// The allocator, for Rust callers and a socket's buffers.
#[must_use]
pub fn zend_string_allocator() -> TrustedAllocator {
    // SAFETY: each callback hands out or takes back the value of a Zend string
    // of the size asked for, or null when the allocation fails.
    unsafe { TrustedAllocator::new(&ZEND_STRING_ALLOCATOR) }
}

extern "C" fn malloc(size: usize) -> *mut c_void {
    val(string::alloc(size)).cast()
}

/// `nmemb * size` zeroed bytes, or null when that overflows.
extern "C" fn calloc(nmemb: usize, size: usize) -> *mut c_void {
    let Some(bytes) = nmemb.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    // The string's reference goes to the caller, which frees it through `free`.
    let mut buffer = std::mem::ManuallyDrop::new(ZBuf::with_capacity(bytes));
    buffer.spare_capacity_mut().fill(MaybeUninit::new(0));
    buffer.as_mut_ptr().cast()
}

/// # Safety
/// `address` came from this allocator.
unsafe extern "C" fn realloc(address: *mut c_void, size: usize) -> *mut c_void {
    // SAFETY: per the caller, the value of a live, unshared string.
    val(unsafe { string::realloc(from_val(address.cast()), size) }).cast()
}

/// # Safety
/// `address` came from this allocator.
unsafe extern "C" fn free(address: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { swr_zend_string_release(from_val(address.cast())) };
}

/// Adopt a received allocation when Zend made it, otherwise copy its initialized
/// bytes. The owner frees a copied allocation; a transferred one is Zend's again.
#[must_use]
pub fn received_string(
    mut buffer: swoole_rs_corebase::buffer::string::OwnedString,
) -> swoole_rs_zend::ZString {
    let zend = buffer
        .allocator()
        .is_some_and(|allocator| std::ptr::eq(allocator.get(), &raw const ZEND_STRING_ALLOCATOR));
    if !zend || buffer.as_ptr().is_null() {
        return swoole_rs_zend::ZString::new(buffer.view());
    }
    let length = buffer.len();
    let data = buffer.release();
    // SAFETY: this owner held the sole reference to a block from our Zend-string
    // allocator, and its length counts initialized bytes within that allocation.
    unsafe { ZBuf::from_raw(from_val(data), length) }.into_zstring()
}
