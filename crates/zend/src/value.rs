//! Raw zval reads and stores, and the empty string and array.
use crate::sys::{HashTable, zend_empty_array, zend_string, zval};

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

/// [`emalloc`] of one `T` holding `value`, which is never dropped: the
/// storage is the caller's to `efree` (or the request's end frees it).
#[must_use]
pub fn enew<T>(value: T) -> *mut T {
    const {
        assert!(
            align_of::<T>() <= align_of::<zval>(),
            "emalloc aligns for a zval"
        );
    }
    let at = emalloc(size_of::<T>()).cast::<T>();
    // SAFETY: fresh, aligned (checked above) storage for one `T`, never null.
    unsafe { at.write(value) };
    at
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

/// `add_next_index_str(array, string)`: the array takes over the reference.
pub use crate::sys::add_next_index_str as push_str;
/// `array_init(target)`.
pub use crate::sys::swr_zend_array_init as array_init;
