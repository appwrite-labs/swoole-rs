//! Reading zvals and filling arrays.
use std::ffi::c_char;

use crate::sys::zval;

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

/// `add_assoc_stringl_ex`, or `add_assoc_null_ex` when `value` is `None`,
/// without separating the array; nothing once a bailout is pending.
/// # Safety
/// `array` is a live array zval.
pub unsafe fn set_key(array: *mut zval, key: &[u8], value: Option<&[u8]>) {
    if !crate::bailout::pending() {
        let value = value.map_or_else(crate::Owned::null, |v| crate::ZString::new(v).into());
        // SAFETY: per the caller.
        unsafe { crate::zval::symtable_update(array, key, value) };
    }
}
