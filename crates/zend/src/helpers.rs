//! Zvals, arrays and properties for crates/phpprocess: zval construction,
//! typed property updates.
use crate::{
    Value,
    sys::{
        module_registry, php_time, swr_zend_property_not_null, swr_zend_property_slot,
        zend_class_entry, zend_fcall_info, zend_fcall_info_cache, zend_hash_str_find, zend_object,
        zval,
    },
};
use std::ffi::CStr;

/// An empty `zend_fcall_info` (no function, object or arguments), for a
/// parser or the caller to fill.
// SAFETY: pointers, integers and an undef zval, for which zero is valid.
pub const EMPTY_FCI: zend_fcall_info = unsafe { std::mem::zeroed() };
/// An empty `zend_fcall_info_cache`, likewise.
// SAFETY: pointers, for which zero is valid.
pub const EMPTY_FCC: zend_fcall_info_cache = unsafe { std::mem::zeroed() };

/// Whether the extension `name` (lowercase, as `module_registry` keys it) is
/// loaded.
#[must_use]
pub fn module_loaded(name: &CStr) -> bool {
    let registry = &raw mut module_registry;
    // SAFETY: the registry lives for the process and is filled before any
    // extension code runs; Zend reads `name` only.
    !unsafe { zend_hash_str_find(registry, name.as_ptr(), name.count_bytes()) }.is_null()
}

/// `php_time`: PHP's clock, in seconds.
#[must_use]
pub fn time() -> i64 {
    // SAFETY: reads the clock.
    unsafe { php_time() }
}

/// `sw_zend_read_property_ex`: the declared property's slot (an uninitialised
/// typed property reads as null).
/// # Safety
/// `object` is live and declares `name`, for `'a`.
#[must_use]
pub unsafe fn property<'a>(object: *mut zend_object, name: &CStr) -> &'a Value {
    // SAFETY: per the caller.
    let slot = unsafe { swr_zend_property_slot(object, name.as_ptr(), name.count_bytes()) };
    // SAFETY: a live slot, never null.
    unsafe { &*slot.cast::<Value>() }
}

/// `sw_zend_read_property_not_null_ex`: the property unless it is null.
/// # Safety
/// As [`property`].
#[must_use]
pub unsafe fn property_not_null<'a>(object: *mut zend_object, name: &CStr) -> Option<&'a Value> {
    // SAFETY: per the caller.
    let found = unsafe { swr_zend_property_not_null(object, name.as_ptr(), name.count_bytes()) };
    // SAFETY: null or a live property.
    unsafe { Value::from_ptr(found) }
}

/// `zend_update_property(scope, object, name, value)`: the property takes a copy.
/// # Safety
/// `object` is live and `scope` null or a registered class ([`crate::Props::new`]),
/// and `value` is live.
pub unsafe fn update_property(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    value: *mut zval,
) {
    // SAFETY: per the caller.
    let props = unsafe { crate::Props::new(scope, object) };
    // SAFETY: a live value, per the caller.
    if let Some(value) = unsafe { Value::from_ptr(value) } {
        props.value(name, value);
    }
}

/// `is_numeric_string(text, len, &lval, &dval, false)`: the kind (`IS_LONG`,
/// `IS_DOUBLE`, or 0 for text that is not numeric) with the long and double
/// it filled.
#[must_use]
pub fn numeric(text: &[u8]) -> (u8, i64, f64) {
    let (mut lval, mut dval) = (0, 0.0);
    // SAFETY: a live byte range and two writable locals.
    let kind = unsafe {
        crate::sys::swr_zend_is_numeric_string(
            text.as_ptr().cast(),
            text.len(),
            &raw mut lval,
            &raw mut dval,
        )
    };
    (kind, lval, dval)
}

/// `ZVAL_LONG`: a zval holding `value`.
#[must_use]
pub fn long_zval(value: i64) -> zval {
    zval {
        value: crate::sys::zend_value { lval: value },
        u1: crate::sys::_zval_struct__bindgen_ty_1 {
            type_info: crate::sys::IS_LONG,
        },
        u2: crate::sys::_zval_struct__bindgen_ty_2 { next: 0 },
    }
}
