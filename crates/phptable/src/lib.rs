//! Arrays, conversions and object properties as Swoole's own helpers see them
//! (`SW_HASHTABLE_FOREACH_START2`, `zend::String`, `sw_zend_read_property_ex`
//! and friends), for the coroutine HTTP and HTTP/2 clients.
//!
//! Without the `php` feature the crate is empty.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::{ffi::CStr, marker::PhantomData, ptr};
use swoole_rs_zend::{
    string::bytes,
    sys::{
        HashPosition, HashTable, IS_ARRAY, IS_NULL, IS_STRING, swr_zend_array_add_or_merge,
        swr_zend_property_array, swr_zend_property_not_null, swr_zend_property_slot,
        swr_zend_string_release, swr_zval_get_double, swr_zval_get_long, swr_zval_get_string,
        swr_zval_is_true, zend_class_entry, zend_hash_get_current_data_ex,
        zend_hash_get_current_key_ex, zend_hash_internal_pointer_reset_ex,
        zend_hash_move_forward_ex, zend_hash_str_find, zend_long, zend_object, zend_string,
        zend_update_property, zend_update_property_null, zend_update_property_stringl, zval,
    },
    type_of, value,
};

/// `HASH_KEY_IS_STRING`.
const KEY_IS_STRING: i64 = 1;

/// A string reference the holder owns (`zend::String`): released on drop.
pub struct Owned(*mut zend_string);

impl Owned {
    /// `zval_get_string(value)`: the value as a string, converting as PHP does.
    /// # Safety
    /// `value` is a live zval.
    #[must_use]
    pub unsafe fn of(value: *mut zval) -> Self {
        // SAFETY: per the caller; Zend returns a new reference.
        Self(unsafe { swr_zval_get_string(value) })
    }

    /// Take over a reference the caller owns.
    /// # Safety
    /// `string` is a live string carrying a reference the caller gives up.
    #[must_use]
    pub unsafe fn from_raw(string: *mut zend_string) -> Self {
        Self(string)
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the reference this holds keeps the string alive.
        unsafe { bytes(self.0) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the reference this holds.
        unsafe { swr_zend_string_release(self.0) };
    }
}

/// `zval_get_long`.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn long(value: *const zval) -> zend_long {
    // SAFETY: per the caller.
    if unsafe { type_of(value) } == swoole_rs_zend::sys::IS_LONG {
        // SAFETY: an int zval.
        return unsafe { value::long_of(value) };
    }
    // SAFETY: per the caller.
    unsafe { swr_zval_get_long(value) }
}

/// `zval_get_double`.
/// # Safety
/// `value` is a live zval.
#[must_use]
pub unsafe fn double(value: *const zval) -> f64 {
    // SAFETY: per the caller.
    unsafe { swr_zval_get_double(value) }
}

/// `zval_is_true`.
/// # Safety
/// `value` is a live zval.
#[must_use]
pub unsafe fn truthy(value: *const zval) -> bool {
    // SAFETY: per the caller.
    unsafe { swr_zval_is_true(value) }
}

/// `ZVAL_IS_ARRAY`.
/// # Safety
/// `value` is null or a live zval.
#[must_use]
#[inline]
pub unsafe fn is_array(value: *const zval) -> bool {
    // SAFETY: per the caller.
    !value.is_null() && unsafe { type_of(value) } == IS_ARRAY
}

/// `ZVAL_IS_NULL`.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn is_null(value: *const zval) -> bool {
    // SAFETY: per the caller.
    (unsafe { type_of(value) }) == IS_NULL
}

/// `Z_TYPE_P(value) == IS_STRING`: its bytes.
/// # Safety
/// `value` is a live zval, and the bytes are used while it holds that string.
#[must_use]
pub unsafe fn string_bytes<'a>(value: *const zval) -> Option<&'a [u8]> {
    // SAFETY: per the caller.
    if unsafe { type_of(value) } != IS_STRING {
        return None;
    }
    // SAFETY: a string zval, live per the caller.
    let string = unsafe { value::str_of(value) };
    // SAFETY: as above.
    Some(unsafe { bytes(string) })
}

/// `zend_hash_num_elements(Z_ARRVAL_P(array))` (`php_swoole_array_length`).
/// # Safety
/// `array` is a live array zval.
#[must_use]
pub unsafe fn length(array: *const zval) -> u32 {
    // SAFETY: per the caller.
    let table = unsafe { value::table(array) };
    // SAFETY: a live table.
    unsafe { (*table).nNumOfElements }
}

/// `php_swoole_array_length_safe`: 0 unless `array` is a non-null array zval.
/// # Safety
/// `array` is null or a live zval.
#[must_use]
pub unsafe fn length_safe(array: *const zval) -> u32 {
    // SAFETY: per the caller.
    if unsafe { is_array(array) } {
        // SAFETY: an array zval.
        unsafe { length(array) }
    } else {
        0
    }
}

/// `zend_hash_str_find(Z_ARRVAL_P(array), key)`, or null.
/// # Safety
/// `array` is a live array zval.
#[must_use]
pub unsafe fn find(array: *const zval, key: &[u8]) -> *mut zval {
    // SAFETY: per the caller.
    let table = unsafe { value::table(array) };
    // SAFETY: a live table; Zend reads `key` only.
    unsafe { zend_hash_str_find(table, key.as_ptr().cast(), key.len()) }
}

/// `php_swoole_array_get_value`: the value under `key` unless absent or null.
/// # Safety
/// As `find`.
#[must_use]
pub unsafe fn get(array: *const zval, key: &[u8]) -> Option<*mut zval> {
    // SAFETY: per the caller.
    let found = unsafe { find(array, key) };
    // SAFETY: a value Zend found.
    (!found.is_null() && !unsafe { is_null(found) }).then_some(found)
}

/// `SW_HASHTABLE_FOREACH_START2`: each element in order, with its string key
/// (None for an integer key).
pub struct Entries<'a> {
    table: *mut HashTable,
    position: HashPosition,
    life: PhantomData<&'a zval>,
}

/// The elements of `array` in order.
/// # Safety
/// `array` is a live array zval that no one changes while iterating.
#[must_use]
pub unsafe fn entries<'a>(array: *const zval) -> Entries<'a> {
    // SAFETY: per the caller.
    let table = unsafe { value::table(array) };
    let mut position = 0;
    // SAFETY: a live table; `position` is a writable local.
    unsafe { zend_hash_internal_pointer_reset_ex(table, &raw mut position) };
    Entries {
        table,
        position,
        life: PhantomData,
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = (Option<&'a [u8]>, *mut zval);

    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: a live table and our position in it.
        let data = unsafe { zend_hash_get_current_data_ex(self.table, &raw mut self.position) };
        if data.is_null() {
            return None;
        }
        let mut key: *mut zend_string = ptr::null_mut();
        let mut index = 0;
        // SAFETY: as above; the outputs are writable locals.
        let kind = unsafe {
            zend_hash_get_current_key_ex(
                self.table,
                &raw mut key,
                &raw mut index,
                &raw const self.position,
            )
        };
        // SAFETY: as above.
        unsafe { zend_hash_move_forward_ex(self.table, &raw mut self.position) };
        let key = if i64::from(kind) == KEY_IS_STRING && !key.is_null() {
            // SAFETY: the table keeps its key strings alive.
            Some(unsafe { bytes(key) })
        } else {
            None
        };
        Some((key, data))
    }
}

/// `sw_zend_read_property_ex`: the declared property's slot.
/// # Safety
/// `object` is live and declares `name`.
#[must_use]
pub unsafe fn property(object: *mut zend_object, name: &CStr) -> *mut zval {
    // SAFETY: per the caller.
    unsafe { swr_zend_property_slot(object, name.as_ptr(), name.count_bytes()) }
}

/// `sw_zend_read_property_not_null_ex`: the property unless it is null.
/// # Safety
/// As `property`.
#[must_use]
pub unsafe fn property_not_null(object: *mut zend_object, name: &CStr) -> Option<*mut zval> {
    // SAFETY: per the caller.
    let found = unsafe { swr_zend_property_not_null(object, name.as_ptr(), name.count_bytes()) };
    (!found.is_null()).then_some(found)
}

/// `sw_zend_read_and_convert_property_array(ce, object, name)`.
/// # Safety
/// `ce` is the object's class (or an ancestor declaring `name`) and `object` live.
#[must_use]
pub unsafe fn property_array(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
) -> *mut zval {
    // SAFETY: per the caller.
    unsafe { swr_zend_property_array(ce, object, name.as_ptr(), name.count_bytes()) }
}

/// `zend_update_property(ce, object, name, value)`: Zend copies the value.
/// # Safety
/// As `property_array`; `value` is a live zval.
pub unsafe fn update(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    value: *mut zval,
) {
    // SAFETY: per the caller.
    unsafe { zend_update_property(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

/// `zend_update_property_null(ce, object, name)`.
/// # Safety
/// As `property_array`.
pub unsafe fn update_null(ce: *mut zend_class_entry, object: *mut zend_object, name: &CStr) {
    // SAFETY: per the caller.
    unsafe { zend_update_property_null(ce, object, name.as_ptr(), name.count_bytes()) };
}

/// `zend_update_property_stringl(ce, object, name, bytes)`.
/// # Safety
/// As `property_array`.
pub unsafe fn update_bytes(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    data: &[u8],
) {
    // SAFETY: per the caller; Zend copies the bytes.
    unsafe {
        zend_update_property_stringl(
            ce,
            object,
            name.as_ptr(),
            name.count_bytes(),
            data.as_ptr().cast(),
            data.len(),
        );
    };
}

/// `zend::array_add_or_merge(array, key, element)`: the array takes over
/// `element`.
/// # Safety
/// `array` is a live, separated array zval; `element` a zval the caller gives up.
pub unsafe fn add_or_merge(array: *mut zval, key: &[u8], element: *mut zval) {
    // SAFETY: per the caller.
    unsafe { swr_zend_array_add_or_merge(array, key.as_ptr().cast(), key.len(), element) };
}

/// `add_next_index_zval(array, element)`: whether the array took `element`.
/// # Safety
/// `array` is a live, separated array zval; `element` a zval the caller gives up.
pub unsafe fn push(array: *mut zval, element: *mut zval) -> bool {
    // SAFETY: per the caller.
    let table = unsafe { value::table(array) };
    // SAFETY: per the caller.
    !unsafe { swoole_rs_zend::sys::zend_hash_next_index_insert(table, element) }.is_null()
}
