//! The `swoole::Allocator`s over Zend's request allocator: `sw_php_allocator()`
//! (`emalloc`) and `sw_zend_string_allocator()`, whose buffers are the
//! `ZSTR_VAL` of `zend_string`s, so a received message becomes a PHP string
//! without a copy.
use std::ffi::c_void;
use swoole_rs_zend::{
    string::{self, from_val, val},
    sys::{
        swr_zend_ecalloc, swr_zend_efree, swr_zend_emalloc, swr_zend_erealloc,
        swr_zend_string_release,
    },
};

/// `swoole::Allocator`.
#[repr(C)]
pub(crate) struct Allocator {
    malloc: unsafe extern "C" fn(usize) -> *mut c_void,
    calloc: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    realloc: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
}

/// The allocator `sw_php_allocator()` returns.
#[unsafe(export_name = "swr_v1_phpswoole_php_allocator")]
pub(crate) static PHP_ALLOCATOR: Allocator = Allocator {
    malloc: swr_zend_emalloc,
    calloc: swr_zend_ecalloc,
    realloc: swr_zend_erealloc,
    free: swr_zend_efree,
};

/// The allocator `sw_zend_string_allocator()` returns.
#[unsafe(export_name = "swr_v1_phpswoole_zend_string_allocator")]
pub(crate) static ZEND_STRING_ALLOCATOR: Allocator = Allocator {
    malloc,
    calloc,
    realloc,
    free,
};

unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    val(string::alloc(size)).cast()
}

/// Upstream zeroes the first `size` bytes, not `nmemb * size`.
unsafe extern "C" fn calloc(nmemb: usize, size: usize) -> *mut c_void {
    // SAFETY: Zend's allocator.
    let memory = unsafe { malloc(nmemb.wrapping_mul(size)) };
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
