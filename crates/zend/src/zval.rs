//! Safe Zend values: `&Value` borrows a zval, `&Array` a hash table, `ZString`
//! owns a string reference and `Owned` a zval, each released in `Drop`. Every
//! `unsafe` they need lives here; a caller builds one from a raw pointer once,
//! with [`Value::from_ptr`], and reads it safely from then on.
use crate::{
    string::bytes,
    sys::{
        _zval_struct__bindgen_ty_1, _zval_struct__bindgen_ty_2, HashPosition, HashTable, IS_ARRAY,
        IS_NULL, IS_STRING, IS_UNDEF, add_next_index_long, add_next_index_null,
        add_next_index_stringl, swr_zend_array_init, swr_zend_array_is_list,
        swr_zend_hash_foreach_kv, swr_zend_hash_next, swr_zend_string_release, swr_zval_is_true,
        zend_array_count, zend_hash_str_find, zend_long, zend_string, zend_ulong, zend_value, zval,
        zval_ptr_dtor,
    },
    value,
};
use std::{cell::UnsafeCell, ffi::c_void, ptr};

/// A borrowed zval. Zend may change what it points to (refcounts, a
/// conversion's cache), hence the cell.
#[repr(transparent)]
pub struct Value(UnsafeCell<zval>);

impl Value {
    /// Borrow the zval at `value`, or None for null.
    /// # Safety
    /// `value` is null or a live zval that nothing frees or overwrites for `'a`.
    #[must_use]
    #[inline]
    pub unsafe fn from_ptr<'a>(value: *const zval) -> Option<&'a Self> {
        // SAFETY: per the caller; `Value` is a transparent zval.
        unsafe { value.cast::<Self>().as_ref() }
    }

    #[must_use]
    #[inline]
    pub fn as_ptr(&self) -> *mut zval {
        self.0.get()
    }

    /// `Z_TYPE_P`.
    #[must_use]
    #[inline]
    pub fn kind(&self) -> u32 {
        // SAFETY: a live zval.
        unsafe { crate::type_of(self.as_ptr()) }
    }

    /// `Z_TYPE_P(value) == IS_NULL`.
    #[must_use]
    #[inline]
    pub fn is_null(&self) -> bool {
        self.kind() == IS_NULL
    }

    /// `zval_get_long`: a bailout from a conversion's warning is deferred.
    #[must_use]
    #[inline]
    pub fn long(&self) -> zend_long {
        // SAFETY: a live zval.
        unsafe { value::get_long(self.as_ptr()) }
    }

    /// `zval_get_double`, as [`Value::long`].
    #[must_use]
    #[inline]
    pub fn double(&self) -> f64 {
        // SAFETY: a live zval.
        unsafe { value::get_double(self.as_ptr()) }
    }

    /// `zval_is_true`.
    #[must_use]
    #[inline]
    pub fn truthy(&self) -> bool {
        // SAFETY: a live zval.
        unsafe { swr_zval_is_true(self.as_ptr()) }
    }

    /// `zval_get_string`, as [`Value::long`].
    #[must_use]
    #[inline]
    pub fn string(&self) -> ZString {
        // SAFETY: a live zval; Zend returns a new reference, even after a bailout.
        let string = unsafe { value::get_string(self.as_ptr()) };
        ZString(string)
    }

    /// `Z_STRVAL_P`/`Z_STRLEN_P` of a string.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> Option<&[u8]> {
        if self.kind() != IS_STRING {
            return None;
        }
        // SAFETY: a live string zval.
        let string = unsafe { value::str_of(self.as_ptr()) };
        // SAFETY: the zval holds the string while borrowed.
        Some(unsafe { bytes(string) })
    }

    /// `Z_ARRVAL_P` of an array.
    #[must_use]
    #[inline]
    pub fn array(&self) -> Option<&Array> {
        if self.kind() != IS_ARRAY {
            return None;
        }
        // SAFETY: a live array zval.
        let table = unsafe { value::table(self.as_ptr()) };
        // SAFETY: the zval holds the table while borrowed; `Array` is a
        // transparent table.
        Some(unsafe { &*table.cast::<Array>() })
    }
}

/// A borrowed hash table (`zend_array`).
#[repr(transparent)]
pub struct Array(UnsafeCell<HashTable>);

/// An array key: `ZEND_HASH_FOREACH_KEY`'s string or integer.
#[derive(Clone, Copy)]
pub enum Key<'a> {
    Name(&'a [u8]),
    Index(zend_ulong),
}

impl Array {
    #[must_use]
    #[inline]
    pub fn as_ptr(&self) -> *mut HashTable {
        self.0.get()
    }

    /// `zend_array_count`.
    #[must_use]
    #[inline]
    pub fn count(&self) -> u32 {
        // SAFETY: a live table.
        unsafe { zend_array_count(self.as_ptr()) }
    }

    /// `zend_array_is_list`.
    #[must_use]
    #[inline]
    pub fn is_list(&self) -> bool {
        // SAFETY: a live table.
        unsafe { swr_zend_array_is_list(self.as_ptr()) }
    }

    /// `zend_hash_str_find`.
    #[must_use]
    #[inline]
    pub fn find(&self, key: &[u8]) -> Option<&Value> {
        // SAFETY: a live table; Zend reads `key` only.
        let found = unsafe { zend_hash_str_find(self.as_ptr(), key.as_ptr().cast(), key.len()) };
        // SAFETY: null or an element of the borrowed table.
        unsafe { Value::from_ptr(found) }
    }

    /// `php_swoole_array_get_value`: the value under `key` unless absent or null.
    #[must_use]
    #[inline]
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.find(key).filter(|found| !found.is_null())
    }

    /// `SW_HASHTABLE_FOREACH_START2`: each element in order, not dereferenced,
    /// with its string key (None for an integer key). Nothing may change the
    /// table while iterating, which the borrow ensures.
    #[must_use]
    #[inline]
    pub fn entries(&self) -> Entries<'_> {
        Entries {
            table: self,
            position: 0,
        }
    }

    /// `ZEND_HASH_FOREACH_KEY_VAL` with `ZVAL_DEREF`, stopping once a bailout
    /// is pending.
    pub fn for_each<'a, F: FnMut(Key<'a>, &'a Value)>(&'a self, mut each: F) {
        unsafe extern "C" fn trampoline<'a, F: FnMut(Key<'a>, &'a Value)>(
            key: *mut zend_string,
            index: zend_ulong,
            value: *mut zval,
            context: *mut c_void,
        ) {
            let key = if key.is_null() {
                Key::Index(index)
            } else {
                // SAFETY: the table holds its keys while borrowed.
                Key::Name(unsafe { bytes(key) })
            };
            // SAFETY: an element of the borrowed table.
            let value = unsafe { &*value.cast::<Value>() };
            // SAFETY: `context` is the closure `for_each` passed, live for the loop.
            let each = unsafe { &mut *context.cast::<F>() };
            each(key, value);
        }
        let context = (&raw mut each).cast::<c_void>();
        // SAFETY: a live table; the closure outlives the loop, and a panic
        // aborts at the `extern "C"` boundary.
        unsafe { swr_zend_hash_foreach_kv(self.as_ptr(), Some(trampoline::<F>), context) };
    }
}

/// [`Array::entries`].
pub struct Entries<'a> {
    table: &'a Array,
    position: HashPosition,
}

impl<'a> Iterator for Entries<'a> {
    type Item = (Option<&'a [u8]>, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        let mut key = ptr::null_mut();
        // SAFETY: a live table and our position in it; `key` is a writable local.
        let found = unsafe {
            swr_zend_hash_next(self.table.as_ptr(), &raw mut self.position, &raw mut key)
        };
        // SAFETY: null or an element of the borrowed table.
        let value = unsafe { Value::from_ptr(found) }?;
        // SAFETY: the table keeps its key strings alive.
        Some(((!key.is_null()).then(|| unsafe { bytes(key) }), value))
    }
}

/// A `zend_string` reference this holds (`zend::String`), released on drop.
pub struct ZString(*mut zend_string);

impl ZString {
    /// `zend_string_init(bytes, len, 0)`.
    #[must_use]
    #[inline]
    pub fn new(bytes: &[u8]) -> Self {
        Self(crate::string::copy(bytes))
    }

    #[must_use]
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the reference this holds keeps the string alive.
        unsafe { bytes(self.0) }
    }

    /// Take over a reference the caller owns.
    /// # Safety
    /// `string` is a live string carrying a reference the caller gives up.
    #[must_use]
    #[inline]
    pub unsafe fn from_raw(string: *mut zend_string) -> Self {
        Self(string)
    }

    /// Give up the reference to whoever takes the pointer.
    #[must_use]
    #[inline]
    pub fn into_raw(self) -> *mut zend_string {
        std::mem::ManuallyDrop::new(self).0
    }
}

impl Drop for ZString {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the reference this holds.
        unsafe { swr_zend_string_release(self.0) };
    }
}

/// A zval this owns, destroyed on drop (`zval_ptr_dtor`).
#[repr(transparent)]
pub struct Owned(Value);

impl Default for Owned {
    /// `ZVAL_UNDEF`.
    #[inline]
    fn default() -> Self {
        Self(Value(UnsafeCell::new(zval {
            value: zend_value { lval: 0 },
            u1: _zval_struct__bindgen_ty_1 {
                type_info: IS_UNDEF,
            },
            u2: _zval_struct__bindgen_ty_2 { next: 0 },
        })))
    }
}

impl Owned {
    /// `ZVAL_LONG`.
    #[must_use]
    #[inline]
    pub fn long(value: zend_long) -> Self {
        Self(Value(UnsafeCell::new(crate::long_zval(value))))
    }
}

/// An array this owns (`array_init`), which pushes fill.
#[repr(transparent)]
pub struct OwnedArray(Owned);

impl Default for OwnedArray {
    #[inline]
    fn default() -> Self {
        let owned = Owned::default();
        // SAFETY: an undefined slot this owns.
        unsafe { swr_zend_array_init(owned.as_ptr()) };
        Self(owned)
    }
}

impl OwnedArray {
    /// `add_next_index_stringl`: Zend copies the bytes.
    #[inline]
    pub fn push_bytes(&mut self, data: &[u8]) {
        // SAFETY: an array this owns.
        unsafe { add_next_index_stringl(self.0.as_ptr(), data.as_ptr().cast(), data.len()) };
    }

    /// `add_next_index_long`.
    #[inline]
    pub fn push_long(&mut self, value: zend_long) {
        // SAFETY: an array this owns.
        unsafe { add_next_index_long(self.0.as_ptr(), value) };
    }

    /// `add_next_index_null`.
    #[inline]
    pub fn push_null(&mut self) {
        // SAFETY: an array this owns.
        unsafe { add_next_index_null(self.0.as_ptr()) };
    }

    #[must_use]
    #[inline]
    pub fn into_owned(self) -> Owned {
        self.0
    }
}

impl std::ops::Deref for Owned {
    type Target = Value;

    #[inline]
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl Drop for Owned {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the zval this owns, destroyed once.
        unsafe { zval_ptr_dtor(self.as_ptr()) };
    }
}
