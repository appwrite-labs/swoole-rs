//! Zvals, arrays and properties for crates/phpprocess: zval construction,
//! hash iteration, typed property updates.
use crate::{
    Value,
    sys::{
        module_registry, swr_zend_hash_foreach, swr_zend_property_slot, zend_array,
        zend_class_entry, zend_declare_property_null, zend_fcall_info, zend_fcall_info_cache,
        zend_hash_str_find, zend_object, zend_update_property, zval,
    },
};
use std::ffi::{CStr, c_int, c_void};

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

/// `ZEND_HASH_FOREACH_VAL`: `each` returns false to stop early.
/// # Safety
/// `table` is a live array that `each` does not modify, and `each` does not
/// unwind.
pub unsafe fn each_value<F: FnMut(*mut zval) -> bool>(table: *const zend_array, mut each: F) {
    unsafe extern "C" fn trampoline<F: FnMut(*mut zval) -> bool>(
        value: *mut zval,
        context: *mut c_void,
    ) -> bool {
        // SAFETY: `context` is the closure `each_value` passed, live for the loop.
        let each = unsafe { &mut *context.cast::<F>() };
        each(value)
    }
    let context = (&raw mut each).cast::<c_void>();
    // SAFETY: per the caller; the closure outlives the loop.
    unsafe { swr_zend_hash_foreach(table, Some(trampoline::<F>), context) };
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

/// `zend_update_property(scope, object, name, value)`: the property takes a copy.
/// # Safety
/// As [`update_property_long`], with a live `value`.
pub unsafe fn update_property(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: &CStr,
    value: *mut zval,
) {
    // SAFETY: per the caller.
    unsafe { zend_update_property(scope, object, name.as_ptr(), name.count_bytes(), value) };
}

/// `zend_declare_property_null(class, name, access)`, at MINIT; `access` is a
/// `ZEND_ACC_*` visibility.
/// # Safety
/// `class` is a class being registered.
pub unsafe fn declare_property_null(class: *mut zend_class_entry, name: &CStr, access: u32) {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "ZEND_ACC_* visibilities are small"
    )]
    let access = access as c_int;
    // SAFETY: per the caller.
    unsafe { zend_declare_property_null(class, name.as_ptr(), name.count_bytes(), access) };
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
