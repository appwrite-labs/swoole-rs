//! Raw zval reads and stores, and the empty string and array.
use crate::sys::{
    _zval_struct__bindgen_ty_1, HashTable, zend_empty_array, zend_object, zend_string, zend_value,
    zval,
};

unsafe extern "C" {
    /// Zend's interned empty string: set at startup, before any extension code
    /// runs, and never written again.
    safe static zend_empty_string: *mut zend_string;
    /// `emalloc(size)`: it takes a size and returns storage; Zend's allocator
    /// never returns null and doesn't bail out (docs/zend.md, `memory_limit`).
    safe fn swr_zend_emalloc(size: usize) -> *mut std::ffi::c_void;
}

/// `emalloc(size)`: `size` uninitialised bytes of the request's heap, aligned
/// for a zval and never null, until `efree` or the request's end frees them.
#[must_use]
#[inline]
pub fn emalloc(size: usize) -> *mut std::ffi::c_void {
    swr_zend_emalloc(size)
}

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

/// `Z_OBJ_P`.
/// # Safety
/// `object` is a live object zval.
#[must_use]
#[inline]
pub unsafe fn obj_of(object: *const zval) -> *mut zend_object {
    // SAFETY: per the caller, an object zval holds `obj`, at offset 0.
    unsafe { object.cast::<*mut zend_object>().read() }
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

/// `ZSTR_EMPTY_ALLOC()`: the interned empty string, which needs no reference
/// (a zval holds it as `IS_INTERNED_STRING_EX`).
#[must_use]
#[inline]
pub fn empty_string() -> *mut zend_string {
    zend_empty_string
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
