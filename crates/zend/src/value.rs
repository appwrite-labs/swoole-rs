//! Raw zval reads and stores, and the empty string and array.
use crate::sys::{HashTable, zend_empty_array, zend_string, zval};

unsafe extern "C" {
    /// Zend's interned empty string: set at startup, before any extension code
    /// runs, and never written again.
    safe static zend_empty_string: *mut zend_string;
}

/// Every `zend_value` field sits at the zval's first byte, so one read takes it.
const _: () = assert!(
    std::mem::offset_of!(zval, value) == 0,
    "the value union leads the zval"
);

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

/// `array_init(target)`.
pub use crate::sys::swr_zend_array_init as array_init;
