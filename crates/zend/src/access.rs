//! Reading zvals, declaring null properties and filling arrays.
use crate::{
    call::type_of,
    string::bytes,
    sys::{
        add_assoc_null_ex, add_assoc_stringl_ex, add_next_index_str, zend_class_entry,
        zend_declare_property_null, zend_string, zend_value, zval,
    },
};
use std::ffi::CStr;

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
    if unsafe { type_of(value) } != crate::sys::IS_STRING {
        return None;
    }
    // SAFETY: per the caller.
    let value = unsafe { value_of(value) };
    // SAFETY: an IS_STRING zval holds `str_`.
    let string = unsafe { value.str_ };
    // SAFETY: the string lives as long as the zval.
    Some(unsafe { bytes(string) })
}

/// `zval_get_long`: any value converted as PHP does ([`crate::value::get_long`]).
/// # Safety
/// `value` is a live zval.
#[must_use]
pub unsafe fn to_long(value: *const zval) -> i64 {
    // SAFETY: per the caller.
    unsafe { crate::value::get_long(value) }
}

/// `zend_declare_property_null(ce, name, ZEND_ACC_PUBLIC)`.
/// # Safety
/// MINIT, with a registered class.
pub unsafe fn declare_null(ce: *mut zend_class_entry, name: &CStr) {
    #[expect(clippy::cast_possible_wrap, reason = "ZEND_ACC_PUBLIC is 1")]
    // SAFETY: per the caller; Zend copies the name.
    unsafe {
        zend_declare_property_null(
            ce,
            name.as_ptr(),
            name.count_bytes(),
            crate::sys::ZEND_ACC_PUBLIC as i32,
        );
    }
}

/// `add_next_index_str`: the array takes over `string`'s reference.
/// # Safety
/// `array` is a live array zval and `string` a live string.
pub unsafe fn push_string(array: *mut zval, string: *mut zend_string) {
    // SAFETY: per the caller.
    unsafe { add_next_index_str(array, string) };
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
