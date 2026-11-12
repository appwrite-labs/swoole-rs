//! `sw_zend_string_allocator()`: the `swoole::Allocator` whose buffers are the
//! `ZSTR_VAL` of `zend_string`s, so a received message becomes a PHP string
//! without a copy.
use std::ffi::c_void;
use swoole_rs_corebase::CoreAllocator;
use swoole_rs_zend::{
    string::{self, from_val, val},
    sys::swr_zend_string_release,
};

static ZEND_STRING_ALLOCATOR: CoreAllocator = CoreAllocator {
    malloc: Some(malloc),
    calloc: Some(calloc),
    realloc: Some(realloc),
    free: Some(free),
};

/// The allocator `sw_zend_string_allocator()` returns, for Rust callers.
#[must_use]
pub fn zend_string_allocator() -> &'static CoreAllocator {
    &ZEND_STRING_ALLOCATOR
}

/// `const swoole::Allocator *sw_zend_string_allocator()`.
#[unsafe(export_name = "_Z24sw_zend_string_allocatorv")]
#[must_use]
pub extern "C" fn sw_zend_string_allocator() -> *const c_void {
    (&raw const ZEND_STRING_ALLOCATOR).cast()
}

extern "C" fn malloc(size: usize) -> *mut c_void {
    val(string::alloc(size)).cast()
}

/// Upstream zeroes the first `size` bytes, not `nmemb * size`.
unsafe extern "C" fn calloc(nmemb: usize, size: usize) -> *mut c_void {
    let memory = malloc(nmemb.wrapping_mul(size));
    if !memory.is_null() {
        // SAFETY: as upstream: a buffer of `nmemb * size` bytes.
        unsafe { memory.cast::<u8>().write_bytes(0, size) };
    }
    memory
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
