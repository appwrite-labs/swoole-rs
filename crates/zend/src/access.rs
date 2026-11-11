//! Reading zvals and filling arrays.
use std::ffi::c_char;

use crate::sys::{add_assoc_null_ex, add_assoc_stringl_ex, zval};

/// A key for Zend's `add_assoc_*_ex`, which reads its first byte even when the length is 0
/// (`ZEND_HANDLE_NUMERIC_STR`): an empty slice's dangling pointer becomes `""`.
#[must_use]
pub fn key_ptr(key: &[u8]) -> *const c_char {
    if key.is_empty() {
        c"".as_ptr()
    } else {
        key.as_ptr().cast()
    }
}

/// `Z_STR_P`'s bytes when `value` is a string.
/// # Safety
/// `value` is a live zval that outlives the slice.
#[must_use]
#[inline]
pub unsafe fn string_of<'a>(value: *const zval) -> Option<&'a [u8]> {
    // SAFETY: per the caller.
    unsafe { crate::Value::from_ptr(value) }.and_then(crate::Value::bytes)
}

/// `add_assoc_stringl_ex`, or `add_assoc_null_ex` when `value` is `None`.
/// # Safety
/// `array` is a live array zval.
pub unsafe fn set_key(array: *mut zval, key: &[u8], value: Option<&[u8]>) {
    match value {
        // SAFETY: per the caller; Zend copies key and value.
        Some(v) => unsafe {
            add_assoc_stringl_ex(array, key_ptr(key), key.len(), v.as_ptr().cast(), v.len());
        },
        // SAFETY: per the caller; Zend copies the key.
        None => unsafe { add_assoc_null_ex(array, key_ptr(key), key.len()) },
    }
}
