//! Non-persistent `zend_string`s: allocated here, handed to Zend by a return
//! value or an array, or freed.
use crate::sys::{swr_zend_string_alloc, zend_string};
use std::mem::offset_of;

/// `ZSTR_VAL`/`ZSTR_LEN` as bytes (without the terminating NUL, which follows).
/// # Safety
/// `string` is live for `'a`.
#[must_use]
#[inline]
pub unsafe fn bytes<'a>(string: *const zend_string) -> &'a [u8] {
    // SAFETY: a live string.
    let length = unsafe { (*string).len };
    // SAFETY: a live string holds `len` bytes at `val`.
    unsafe { std::slice::from_raw_parts(val(string.cast_mut()), length) }
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
#[must_use]
#[inline]
pub fn alloc(length: usize) -> *mut zend_string {
    // SAFETY: Zend's request allocator; it bails out rather than return null.
    unsafe { swr_zend_string_alloc(length) }
}

/// `zend_string_init(bytes, len, 0)`: a new string holding a copy of `bytes`.
#[must_use]
pub fn copy(bytes: &[u8]) -> *mut zend_string {
    let string = alloc(bytes.len());
    // SAFETY: a fresh allocation of `bytes.len()` bytes and a NUL.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), val(string), bytes.len()) };
    // SAFETY: as above.
    unsafe { truncate(string, bytes.len()) };
    string
}

/// `ZSTR_LEN(string) = length; ZSTR_VAL(string)[length] = '\0'`.
/// # Safety
/// `string` is live, unshared and holds at least `length` bytes.
#[inline]
pub unsafe fn truncate(string: *mut zend_string, length: usize) {
    // SAFETY: per the caller.
    unsafe { (*string).len = length };
    // SAFETY: an allocation of at least `length` bytes has room for the NUL.
    let end = unsafe { val(string).add(length) };
    // SAFETY: as above.
    unsafe { *end = 0 };
}

/// `zend_string_free` of a live, unshared, non-interned string nothing else references.
pub use crate::sys::swr_zend_string_free as free;
/// `zend_string_realloc(string, length, 0)` of a live, unshared, non-interned string.
pub use crate::sys::swr_zend_string_realloc as realloc;
