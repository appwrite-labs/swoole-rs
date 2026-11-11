//! Raw zval reads and stores, and the empty string and array.
use crate::Value;
use crate::sys::{
    _zval_struct__bindgen_ty_1, HashTable, IS_LONG, zend_empty_array, zend_empty_string, zend_long,
    zend_object, zend_string, zend_value, zval,
};

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

/// [`Value::long`] of a raw zval.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn get_long(value: *const zval) -> zend_long {
    // SAFETY: per the caller.
    unsafe { &*value.cast::<Value>() }.long()
}

/// [`Value::string`] of a raw zval: a new reference the caller owns.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn get_string(value: *const zval) -> *mut zend_string {
    // SAFETY: per the caller.
    unsafe { &*value.cast::<Value>() }.string().into_raw()
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

/// The two stores of Zend's `ZVAL_*` setters: the value and its type info,
/// leaving `u2` (a bucket's chain, a frame's argument count) untouched.
/// # Safety
/// `target` is writable and holds nothing that needs releasing; `value` is
/// what `type_info` says it is, carrying any reference that type counts.
#[inline]
pub unsafe fn set(target: *mut zval, value: zend_value, type_info: u32) {
    // SAFETY: per the caller.
    unsafe { (*target).value = value };
    // SAFETY: as above.
    unsafe { (*target).u1 = _zval_struct__bindgen_ty_1 { type_info } };
}

/// `ZVAL_LONG(target, value)`.
/// # Safety
/// `target` is writable and holds nothing that needs releasing.
#[inline]
pub unsafe fn set_long(target: *mut zval, value: zend_long) {
    // SAFETY: per the caller.
    unsafe { set(target, zend_value { lval: value }, IS_LONG) };
}

/// `ZSTR_EMPTY_ALLOC()`: the interned empty string, which needs no reference
/// (a zval holds it as `IS_INTERNED_STRING_EX`).
#[must_use]
#[inline]
pub fn empty_string() -> *mut zend_string {
    // SAFETY: Zend sets it at startup, before any extension code runs, and
    // never changes it.
    unsafe { zend_empty_string }
}

/// `(zend_array *) &zend_empty_array`, the immutable empty array
/// `ZVAL_EMPTY_ARRAY` stores (as `IS_ARRAY`, uncounted).
#[must_use]
#[inline]
pub fn empty_array() -> *mut HashTable {
    (&raw const zend_empty_array).cast_mut()
}

/// `add_next_index_str(array, string)`: the array takes over the reference.
pub use crate::sys::add_next_index_str as push_str;
/// `array_init(target)`.
pub use crate::sys::swr_zend_array_init as array_init;
/// `ZVAL_STR(target, string)`: `target` takes over the string's reference.
pub use crate::sys::swr_zval_str as set_str;
