//! Reading zvals and filling arrays.
use crate::sys::{add_assoc_null_ex, add_assoc_stringl_ex, zend_value, zval};

/// `Z_STR_P`, `Z_LVAL_P`, ...: the value union, read by its type's field.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn value_of(value: *const zval) -> zend_value {
    // SAFETY: per the caller.
    unsafe { (*value).value }
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
            add_assoc_stringl_ex(
                array,
                key.as_ptr().cast(),
                key.len(),
                v.as_ptr().cast(),
                v.len(),
            );
        },
        // SAFETY: per the caller; Zend copies the key.
        None => unsafe { add_assoc_null_ex(array, key.as_ptr().cast(), key.len()) },
    }
}
