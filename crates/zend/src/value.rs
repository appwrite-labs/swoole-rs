//! Object properties and arrays, through Zend's own update and add functions.
use crate::sys::{
    _zval_struct__bindgen_ty_1, HashTable, IS_LONG, add_assoc_long_ex, add_assoc_string_ex,
    zend_class_entry, zend_long, zend_object, zend_string, zend_update_property_bool,
    zend_update_property_long, zend_update_property_string, zend_value, zval,
};
use crate::{
    bailout,
    call::type_of,
    sys::{
        IS_DOUBLE, IS_STRING, swr_zend_string_copy, swr_zend_try_get_double, swr_zend_try_get_long,
        swr_zend_try_get_string,
    },
};
use std::ffi::CStr;

/// Every `zend_value` field sits at the zval's first byte, so one read takes it.
const _: () = assert!(
    std::mem::offset_of!(zval, value) == 0,
    "the value union leads the zval"
);

/// `Z_ARRVAL_P`.
/// # Safety
/// `array` is a live array zval.
#[must_use]
#[inline]
pub unsafe fn table(array: *const zval) -> *mut HashTable {
    // SAFETY: per the caller, an array zval holds `arr`, at offset 0.
    unsafe { array.cast::<*mut HashTable>().read() }
}

/// `Z_STR_P`.
/// # Safety
/// `string` is a live string zval.
#[must_use]
#[inline]
pub unsafe fn str_of(string: *const zval) -> *mut zend_string {
    // SAFETY: per the caller, a string zval holds `str`, at offset 0.
    unsafe { string.cast::<*mut zend_string>().read() }
}

/// `zval_get_long`: any value converted as PHP does. An object warns, and a
/// user error handler that bails out leaves 0 with the bailout deferred.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn get_long(value: *const zval) -> zend_long {
    // SAFETY: per the caller.
    if unsafe { type_of(value) } == IS_LONG {
        // SAFETY: as above.
        return unsafe { long_of(value) };
    }
    let mut out = 0;
    // SAFETY: per the caller; Zend reads the value only.
    bailout::defer(bailout::trapped(unsafe {
        swr_zend_try_get_long(value.cast_mut(), &raw mut out)
    }));
    out
}

/// `zval_get_double`, as [`get_long`].
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn get_double(value: *const zval) -> f64 {
    // SAFETY: per the caller.
    if unsafe { type_of(value) } == IS_DOUBLE {
        // SAFETY: a float zval holds `dval`, at offset 0.
        return unsafe { value.cast::<f64>().read() };
    }
    let mut out = 0.0;
    // SAFETY: per the caller; Zend reads the value only.
    bailout::defer(bailout::trapped(unsafe {
        swr_zend_try_get_double(value.cast_mut(), &raw mut out)
    }));
    out
}

/// `zval_get_string`: a new reference to the value as a string. An array
/// warns and an object runs `__toString`; a bailout leaves the empty string
/// with the bailout deferred.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn get_string(value: *const zval) -> *mut zend_string {
    // SAFETY: per the caller.
    if unsafe { type_of(value) } == IS_STRING {
        // SAFETY: per the caller, a string zval.
        let string = unsafe { str_of(value) };
        // SAFETY: a live string; the caller owns the new reference.
        return unsafe { swr_zend_string_copy(string) };
    }
    let mut out = std::ptr::null_mut();
    // SAFETY: per the caller.
    bailout::defer(bailout::trapped(unsafe {
        swr_zend_try_get_string(value.cast_mut(), &raw mut out)
    }));
    out
}

/// `Z_OBJ_P`.
/// # Safety
/// `object` is a live object zval.
#[must_use]
#[inline]
pub unsafe fn obj_of(object: *const zval) -> *mut zend_object {
    // SAFETY: per the caller, an object zval holds `obj`, at offset 0.
    unsafe { object.cast::<*mut zend_object>().read() }
}

/// `Z_LVAL_P`.
/// # Safety
/// `long` is a live int zval.
#[must_use]
#[inline]
pub unsafe fn long_of(long: *const zval) -> zend_long {
    // SAFETY: per the caller, an int zval holds `lval`, at offset 0.
    unsafe { long.cast::<zend_long>().read() }
}

/// `zend_array_count(Z_ARRVAL_P(array))`.
/// # Safety
/// `array` is a live array zval.
#[must_use]
#[inline]
pub unsafe fn count(array: *const zval) -> u32 {
    // SAFETY: per the caller.
    unsafe { crate::Value::from_ptr(array) }
        .and_then(crate::Value::array)
        .map_or(0, crate::Array::count)
}

/// `zend_update_property_long(ce, object, name, value)`.
/// # Safety
/// `ce` is a registered class and `object` a live instance of it.
#[inline]
pub unsafe fn update_long(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    value: zend_long,
) {
    // SAFETY: per the caller; Zend copies the name.
    unsafe { zend_update_property_long(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

/// `zend_update_property_bool(ce, object, name, value)`.
/// # Safety
/// As `update_long`.
#[inline]
pub unsafe fn update_bool(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    value: bool,
) {
    // SAFETY: per the caller; Zend copies the name.
    unsafe {
        zend_update_property_bool(
            ce,
            object,
            name.as_ptr(),
            name.count_bytes(),
            zend_long::from(value),
        );
    }
}

/// `zend_update_property_string(ce, object, name, value)`.
/// # Safety
/// As `update_long`.
#[inline]
pub unsafe fn update_string(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    value: &CStr,
) {
    // SAFETY: per the caller; Zend copies both strings.
    unsafe {
        zend_update_property_string(
            ce,
            object,
            name.as_ptr(),
            name.count_bytes(),
            value.as_ptr(),
        );
    }
}

/// `ZVAL_LONG(target, value)`.
/// # Safety
/// `target` is writable and holds nothing that needs releasing.
#[inline]
pub unsafe fn set_long(target: *mut zval, value: zend_long) {
    // SAFETY: per the caller.
    unsafe { (*target).value = zend_value { lval: value } };
    // SAFETY: as above.
    unsafe { (*target).u1 = _zval_struct__bindgen_ty_1 { type_info: IS_LONG } };
}

/// `add_assoc_long(array, key, value)`.
/// # Safety
/// `array` is a live array zval.
#[inline]
pub unsafe fn add_long(array: *mut zval, key: &CStr, value: zend_long) {
    // SAFETY: per the caller.
    unsafe { add_assoc_long_ex(array, key.as_ptr(), key.count_bytes(), value) };
}

/// `add_assoc_string(array, key, value)`: Zend copies the value.
/// # Safety
/// `array` is a live array zval.
#[inline]
pub unsafe fn add_string(array: *mut zval, key: &CStr, value: &CStr) {
    // SAFETY: per the caller.
    unsafe { add_assoc_string_ex(array, key.as_ptr(), key.count_bytes(), value.as_ptr()) };
}

/// `add_next_index_str(array, string)`: the array takes over the reference.
pub use crate::sys::add_next_index_str as push_str;
/// `array_init(target)`.
pub use crate::sys::swr_zend_array_init as array_init;
/// `ZVAL_STR(target, string)`: `target` takes over the string's reference.
pub use crate::sys::swr_zval_str as set_str;
