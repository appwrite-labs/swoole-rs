//! Safe Zend values: `&Value` borrows a zval, `&Array` a hash table and `&ZStr`
//! a string; `ZString` owns a string reference and `Owned` a zval, each
//! released in `Drop`. Every `unsafe` they need lives here; a caller builds one
//! from a raw pointer once (`from_ptr`), or gets one from [`crate::Call`], and
//! reads it safely from then on.
use crate::{
    bailout,
    string::bytes,
    sys::{
        _zval_struct__bindgen_ty_1, _zval_struct__bindgen_ty_2, HashPosition, HashTable, IS_ARRAY,
        IS_DOUBLE, IS_FALSE, IS_INTERNED_STRING_EX, IS_LONG, IS_NULL, IS_OBJECT, IS_PTR,
        IS_REFERENCE, IS_STR_INTERNED, IS_STRING, IS_STRING_EX, IS_TRUE, IS_UNDEF,
        add_next_index_long, add_next_index_null, add_next_index_stringl, php_format_date,
        php_url_encode, swr_zend_array_add_or_merge, swr_zend_array_init, swr_zend_array_is_list,
        swr_zend_hash_foreach_kv, swr_zend_hash_next, swr_zend_object_properties,
        swr_zend_separate_array, swr_zend_string_release, swr_zend_try_get_double,
        swr_zend_try_get_long, swr_zend_try_get_string, swr_zend_try_hash_update,
        swr_zend_try_ptr_dtor, swr_zend_try_symtable_update, swr_zend_zval_value_name,
        swr_zval_is_true, swr_zval_try_addref, zend_array_count, zend_class_entry,
        zend_hash_add_new, zend_hash_index_find, zend_hash_next_index_insert, zend_hash_str_find,
        zend_ini_parse_quantity, zend_long, zend_object, zend_reference, zend_string, zend_ulong,
        zend_value, zval, zval_ptr_dtor,
    },
    value,
};
use std::{
    cell::UnsafeCell,
    ffi::{CStr, c_void},
    ptr,
};

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

    /// Borrow a zval this code holds exclusively, such as a local return slot.
    #[must_use]
    #[inline]
    pub fn of(value: &mut zval) -> &Self {
        // SAFETY: `Value` is a transparent zval, and the exclusive borrow keeps
        // anything else from freeing or overwriting it while borrowed.
        unsafe { &*ptr::from_mut(value).cast::<Self>() }
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

    /// `ZVAL_DEREF`: the value a reference refers to, else this one.
    #[must_use]
    #[inline]
    pub fn dereferenced(&self) -> &Self {
        if self.kind() != IS_REFERENCE {
            return self;
        }
        // SAFETY: a reference zval holds `ref`, at offset 0.
        let reference = unsafe { self.as_ptr().cast::<*mut zend_reference>().read() };
        let value = reference.wrapping_byte_add(std::mem::offset_of!(zend_reference, val));
        // SAFETY: the reference, which this zval keeps alive, holds the value.
        unsafe { &*value.cast::<Self>() }
    }

    /// `Z_LVAL_P` of an int, with no conversion.
    #[must_use]
    #[inline]
    pub fn as_long(&self) -> Option<zend_long> {
        // SAFETY: an int zval holds `lval`, at offset 0.
        (self.kind() == IS_LONG).then(|| unsafe { self.as_ptr().cast::<zend_long>().read() })
    }

    /// `Z_DVAL_P` of a float, with no conversion.
    #[must_use]
    #[inline]
    pub fn as_double(&self) -> Option<f64> {
        // SAFETY: a float zval holds `dval`, at offset 0.
        (self.kind() == IS_DOUBLE).then(|| unsafe { self.as_ptr().cast::<f64>().read() })
    }

    /// `Z_STR_P` of a string.
    #[must_use]
    #[inline]
    pub fn str(&self) -> Option<&ZStr> {
        if self.kind() != IS_STRING {
            return None;
        }
        // SAFETY: a live string zval.
        let string = unsafe { value::str_of(self.as_ptr()) };
        // SAFETY: the zval holds the string while borrowed.
        unsafe { ZStr::from_ptr(string) }
    }

    /// `Z_OBJ_P` of an object.
    #[must_use]
    #[inline]
    pub fn object(&self) -> Option<*mut zend_object> {
        // SAFETY: a live object zval.
        (self.kind() == IS_OBJECT).then(|| unsafe { value::obj_of(self.as_ptr()) })
    }

    /// The [`Custom`](crate::Custom) data of a `T` object.
    #[must_use]
    #[inline]
    pub fn custom<T: crate::Custom>(&self) -> Option<&T> {
        // SAFETY: the zval holds the object while borrowed.
        unsafe { crate::object::custom(self.object()?) }
    }

    /// `Z_PTR_P` of a pointer (`IS_PTR`), as function and class tables hold.
    #[must_use]
    #[inline]
    pub fn pointer(&self) -> Option<*mut c_void> {
        // SAFETY: a pointer zval holds `ptr`, at offset 0.
        (self.kind() == IS_PTR).then(|| unsafe { self.as_ptr().cast::<*mut c_void>().read() })
    }

    /// `Z_OBJCE_P` of an object. Zend may change the class (its flags, its
    /// statics) at any time, so it stays a pointer.
    #[must_use]
    #[inline]
    pub fn class(&self) -> Option<*mut zend_class_entry> {
        let object = self.object()?;
        // SAFETY: the zval holds the object while borrowed.
        Some(unsafe { (*object).ce })
    }

    /// `Z_OBJCE_P(value)->ce_flags` of an object.
    #[must_use]
    #[inline]
    pub fn class_flags(&self) -> Option<u32> {
        let class = self.class()?;
        // SAFETY: a class outlives its objects; one field is read.
        Some(unsafe { (*class).ce_flags })
    }

    /// `zval_get_long`: any value converted as PHP does. An object warns, and
    /// a user error handler that bails out leaves 0 with the bailout deferred.
    #[must_use]
    #[inline]
    pub fn long(&self) -> zend_long {
        if let Some(long) = self.as_long() {
            return long;
        }
        let mut out = 0;
        // SAFETY: a live zval; Zend reads the value only.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_get_long(self.as_ptr(), &raw mut out)
        }));
        out
    }

    /// `zval_get_double`, as [`Value::long`].
    #[must_use]
    #[inline]
    pub fn double(&self) -> f64 {
        if let Some(double) = self.as_double() {
            return double;
        }
        let mut out = 0.0;
        // SAFETY: a live zval; Zend reads the value only.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_get_double(self.as_ptr(), &raw mut out)
        }));
        out
    }

    /// [`Value::long`], or `None` once a bailout is pending: upstream's
    /// `zval_get_long` bailed out there, so the caller applies nothing more.
    #[must_use]
    #[inline]
    pub fn applied_long(&self) -> Option<zend_long> {
        let long = self.long();
        (!bailout::pending()).then_some(long)
    }

    /// [`Value::double`], as [`Value::applied_long`].
    #[must_use]
    #[inline]
    pub fn applied_double(&self) -> Option<f64> {
        let double = self.double();
        (!bailout::pending()).then_some(double)
    }

    /// [`Value::string`], as [`Value::applied_long`].
    #[must_use]
    #[inline]
    pub fn applied_string(&self) -> Option<ZString> {
        let string = self.string();
        (!bailout::pending()).then_some(string)
    }

    /// `zval_is_true`.
    #[must_use]
    #[inline]
    pub fn truthy(&self) -> bool {
        // SAFETY: a live zval.
        unsafe { swr_zval_is_true(self.as_ptr()) }
    }

    /// `zval_get_string`: a new reference to the value as a string. An array
    /// warns and an object runs `__toString`; a bailout leaves the empty
    /// string with the bailout deferred.
    #[must_use]
    #[inline]
    pub fn string(&self) -> ZString {
        if let Some(string) = self.str() {
            return string.to_owned();
        }
        let mut out = ptr::null_mut();
        // SAFETY: a live zval; Zend writes a new reference, even after a bailout.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_get_string(self.as_ptr(), &raw mut out)
        }));
        ZString(out)
    }

    /// `Z_STRVAL_P`/`Z_STRLEN_P` of a string.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> Option<&[u8]> {
        self.str().map(ZStr::bytes)
    }

    /// `Z_OBJPROP_P` of an object: its property table, which Zend builds if
    /// need be and the object keeps.
    #[must_use]
    #[inline]
    pub fn properties(&self) -> Option<&Array> {
        if self.kind() != IS_OBJECT {
            return None;
        }
        // SAFETY: a live object zval.
        let table = unsafe { swr_zend_object_properties(self.as_ptr()) };
        // SAFETY: the object's table, which the zval keeps while borrowed.
        unsafe { Array::from_ptr(table) }
    }

    /// `zend_zval_value_name`: the type, or the object's class, a type error names.
    #[must_use]
    #[inline]
    pub fn value_name(&self) -> &CStr {
        // SAFETY: a live zval.
        let name = unsafe { swr_zend_zval_value_name(self.as_ptr()) };
        // SAFETY: a static type name, or the name of a class, which outlives its objects.
        unsafe { CStr::from_ptr(name) }
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

    /// `ZVAL_COPY`: the same value with a reference of its own.
    #[must_use]
    #[inline]
    pub fn copy(&self) -> Owned {
        // SAFETY: a live zval; the copy carries the reference it adds.
        let copy = unsafe { self.as_ptr().read() };
        let owned = Owned(Self(UnsafeCell::new(copy)));
        // SAFETY: as above.
        unsafe { swr_zval_try_addref(owned.as_ptr()) };
        owned
    }
}

/// A borrowed hash table (`zend_array`).
#[repr(transparent)]
pub struct Array(UnsafeCell<HashTable>);

/// The `zend_refcounted_h` a counted value begins with: its refcount, then
/// `type_info`. Only for a `zend_string` or `HashTable`, which both do.
#[inline]
fn header<T>(counted: &UnsafeCell<T>) -> [u32; 2] {
    // SAFETY: the borrow keeps the value live, and its first eight bytes are
    // the header's two `u32`s.
    unsafe { counted.get().cast::<[u32; 2]>().read() }
}

/// An array key: `ZEND_HASH_FOREACH_KEY`'s string or integer.
#[derive(Clone, Copy)]
pub enum Key<'a> {
    Name(&'a [u8]),
    Index(zend_ulong),
}

impl Array {
    /// Borrow the table at `table`, or None for null.
    /// # Safety
    /// `table` is null or a live table that nothing frees or changes for `'a`.
    #[must_use]
    #[inline]
    pub unsafe fn from_ptr<'a>(table: *const HashTable) -> Option<&'a Self> {
        // SAFETY: per the caller; `Array` is a transparent table.
        unsafe { table.cast::<Self>().as_ref() }
    }

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

    /// `zend_hash_index_find`.
    #[must_use]
    #[inline]
    pub fn index(&self, index: zend_ulong) -> Option<&Value> {
        // SAFETY: a live table.
        let found = unsafe { zend_hash_index_find(self.as_ptr(), index) };
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

impl<'a> Entries<'a> {
    /// The next element with its key, a string or an integer.
    pub fn next_keyed(&mut self) -> Option<(Key<'a>, &'a Value)> {
        let (mut key, mut index) = (ptr::null_mut(), 0);
        // SAFETY: a live table and our position in it; `key` and `index` are
        // writable locals.
        let found = unsafe {
            swr_zend_hash_next(
                self.table.as_ptr(),
                &raw mut self.position,
                &raw mut key,
                &raw mut index,
            )
        };
        // SAFETY: null or an element of the borrowed table.
        let value = unsafe { Value::from_ptr(found) }?;
        if key.is_null() {
            return Some((Key::Index(index), value));
        }
        // SAFETY: the table keeps its key strings alive.
        Some((Key::Name(unsafe { bytes(key) }), value))
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = (Option<&'a [u8]>, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        let (key, value) = self.next_keyed()?;
        let name = match key {
            Key::Name(name) => Some(name),
            Key::Index(_) => None,
        };
        Some((name, value))
    }
}

/// A borrowed `zend_string`.
#[repr(transparent)]
pub struct ZStr(UnsafeCell<zend_string>);

impl ZStr {
    /// Borrow the string at `string`, or None for null.
    /// # Safety
    /// `string` is null or a live string that nothing frees for `'a`.
    #[must_use]
    #[inline]
    pub unsafe fn from_ptr<'a>(string: *const zend_string) -> Option<&'a Self> {
        // SAFETY: per the caller; `ZStr` is a transparent string.
        unsafe { string.cast::<Self>().as_ref() }
    }

    #[must_use]
    #[inline]
    pub fn as_ptr(&self) -> *mut zend_string {
        self.0.get()
    }

    /// `ZSTR_VAL`/`ZSTR_LEN`: the bytes, without the NUL that follows them.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: a live string.
        unsafe { bytes(self.as_ptr()) }
    }

    /// `ZSTR_VAL` as C's `%s` reads it: up to the first NUL.
    #[must_use]
    #[inline]
    pub fn c_str(&self) -> &CStr {
        CStr::from_bytes_until_nul(self.bytes()).unwrap_or_else(|_| {
            // SAFETY: a live string, NUL-terminated after its bytes.
            unsafe { CStr::from_ptr(crate::string::val(self.as_ptr()).cast()) }
        })
    }

    /// `zend_string_copy`: a new reference.
    #[must_use]
    #[inline]
    pub fn to_owned(&self) -> ZString {
        if !self.interned() {
            // SAFETY: a live, counted string; the new reference is the `ZString`'s.
            unsafe { (*self.as_ptr()).gc.refcount += 1 };
        }
        ZString(self.as_ptr())
    }

    /// `ZSTR_IS_INTERNED`: interned strings are never counted or freed.
    #[must_use]
    #[inline]
    pub fn interned(&self) -> bool {
        let [_, info] = header(&self.0);
        info & IS_STR_INTERNED != 0
    }

    /// `zend_ini_parse_quantity`: an ini quantity ("64M").
    /// # Errors
    /// Zend's message when the string does not parse.
    pub fn parse_quantity(&self) -> Result<zend_long, ZString> {
        let mut error = ptr::null_mut();
        // SAFETY: a live string; Zend writes a new error string or leaves null.
        let size = unsafe { zend_ini_parse_quantity(self.as_ptr(), &raw mut error) };
        if error.is_null() {
            Ok(size)
        } else {
            Err(ZString(error))
        }
    }
}

/// A `zend_string` reference this holds (`zend::String`), released on drop.
pub struct ZString(*mut zend_string);

impl std::ops::Deref for ZString {
    type Target = ZStr;

    #[inline]
    fn deref(&self) -> &ZStr {
        // SAFETY: the reference this holds keeps the string alive.
        unsafe { &*self.0.cast::<ZStr>() }
    }
}

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
        ZStr::bytes(self)
    }

    /// `php_url_encode`: `bytes` as `urlencode()` encodes them.
    #[must_use]
    pub fn url_encode(bytes: &[u8]) -> Self {
        // SAFETY: Zend reads the bytes and returns a new string.
        Self(unsafe { php_url_encode(bytes.as_ptr().cast(), bytes.len()) })
    }

    /// `php_format_date`: `timestamp` as `date()` (`local`) or `gmdate()`
    /// formats it.
    #[must_use]
    pub fn format_date(format: &CStr, timestamp: zend_long, local: bool) -> Self {
        // SAFETY: Zend reads the format and returns a new string.
        Self(unsafe { php_format_date(format.as_ptr(), format.count_bytes(), timestamp, local) })
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

impl Clone for ZString {
    /// `zend_string_copy`.
    #[inline]
    fn clone(&self) -> Self {
        ZStr::to_owned(self)
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

/// `ZVAL_UNDEF`: an empty slot, which Zend fills or ignores.
pub const UNDEF: zval = zval {
    value: zend_value { lval: 0 },
    u1: _zval_struct__bindgen_ty_1 {
        type_info: IS_UNDEF,
    },
    u2: _zval_struct__bindgen_ty_2 { next: 0 },
};

impl Default for Owned {
    /// `ZVAL_UNDEF`.
    #[inline]
    fn default() -> Self {
        Self(Value(UnsafeCell::new(UNDEF)))
    }
}

impl Owned {
    /// `ZVAL_LONG`.
    #[must_use]
    #[inline]
    pub fn long(value: zend_long) -> Self {
        Self(Value(UnsafeCell::new(crate::long_zval(value))))
    }

    /// `ZVAL_DOUBLE`.
    #[must_use]
    #[inline]
    pub fn double(value: f64) -> Self {
        let mut zval = UNDEF;
        zval.value = zend_value { dval: value };
        zval.u1.type_info = IS_DOUBLE;
        Self(Value(UnsafeCell::new(zval)))
    }

    /// `ZVAL_BOOL`.
    #[must_use]
    #[inline]
    pub fn bool(value: bool) -> Self {
        let mut zval = UNDEF;
        zval.u1.type_info = if value { IS_TRUE } else { IS_FALSE };
        Self(Value(UnsafeCell::new(zval)))
    }

    /// `ZVAL_NULL`.
    #[must_use]
    #[inline]
    pub fn null() -> Self {
        let mut value = UNDEF;
        value.u1.type_info = IS_NULL;
        Self(Value(UnsafeCell::new(value)))
    }

    /// `ZVAL_STR(value, string)`: the result takes over the reference.
    #[must_use]
    #[inline]
    pub fn string(string: ZString) -> Self {
        let mut zval = UNDEF;
        zval.u1.type_info = if string.interned() {
            IS_INTERNED_STRING_EX
        } else {
            IS_STRING_EX
        };
        zval.value = zend_value {
            str_: string.into_raw(),
        };
        Self(Value(UnsafeCell::new(zval)))
    }

    /// The array this holds, to add to ([`ArrayMut`] separates it first), or
    /// None when it holds no array.
    #[inline]
    pub fn array_mut(&mut self) -> Option<&mut OwnedArray> {
        if self.kind() != IS_ARRAY {
            return None;
        }
        // SAFETY: `OwnedArray` is a transparent `Owned` holding an array.
        Some(unsafe { &mut *ptr::from_mut(self).cast::<OwnedArray>() })
    }

    /// `ZVAL_OBJ(value, std)`: the result takes over the reference `std` carries.
    /// # Safety
    /// `std` is a live object carrying a reference the caller gives up.
    #[must_use]
    #[inline]
    pub unsafe fn object(std: *mut zend_object) -> Self {
        let owned = Self::default();
        // SAFETY: per the caller, into a slot this owns.
        unsafe { crate::set_object(owned.as_ptr(), std) };
        owned
    }

    /// `object_init_ex(value, ce)`: a new object of class `ce`.
    /// # Safety
    /// `ce` is a registered, instantiable class.
    #[must_use]
    #[inline]
    pub unsafe fn new_object(ce: *mut zend_class_entry) -> Self {
        let owned = Self::default();
        // SAFETY: per the caller, into a slot this owns.
        unsafe { crate::init_object(owned.as_ptr(), ce) };
        owned
    }

    /// Give up the zval to whoever takes it (`RETVAL_ZVAL(value, 0, 0)`),
    /// leaving nothing for the drop.
    #[must_use]
    #[inline]
    pub fn into_zval(self) -> zval {
        *std::mem::ManuallyDrop::new(self).0.0.get_mut()
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

/// An array zval this may add to: an [`OwnedArray`]'s, or an array property
/// ([`crate::Props::array`]). Every write separates the array first
/// (`SEPARATE_ARRAY`), so another holder (a [`Value::copy`], a property it
/// was copied to) never sees it, and an immutable array is never written.
#[repr(transparent)]
pub struct ArrayMut(Value);

impl ArrayMut {
    /// # Safety
    /// `value` is a live array zval that nothing else borrows or frees for `'a`.
    #[must_use]
    #[inline]
    pub unsafe fn from_ptr<'a>(value: *mut zval) -> &'a mut Self {
        // SAFETY: per the caller; `ArrayMut` is a transparent zval.
        unsafe { &mut *value.cast::<Self>() }
    }

    /// `SEPARATE_ARRAY`: the zval, holding an array of its own, to write, or
    /// None once a bailout is pending: nothing is written then, because
    /// upstream never ran past it.
    #[inline]
    fn separate(&mut self) -> Option<*mut zval> {
        if bailout::pending() {
            return None;
        }
        let zval = self.0.as_ptr();
        // `GC_REFCOUNT(arr) > 1` is the only case SEPARATE_ARRAY acts on (an
        // immutable array counts 2), so an unshared one skips the call.
        if self.0.array().is_some_and(|array| header(&array.0)[0] <= 1) {
            return Some(zval);
        }
        // SAFETY: an array zval borrowed mutably; a shared or immutable array
        // is duplicated, and the zval's reference moves to the copy.
        unsafe { swr_zend_separate_array(zval) };
        Some(zval)
    }

    /// `add_next_index_stringl`: Zend copies the bytes.
    #[inline]
    pub fn push_bytes(&mut self, data: &[u8]) {
        let Some(array) = self.separate() else { return };
        // SAFETY: a live array.
        unsafe { add_next_index_stringl(array, data.as_ptr().cast(), data.len()) };
    }

    /// `add_next_index_long`.
    #[inline]
    pub fn push_long(&mut self, value: zend_long) {
        let Some(array) = self.separate() else { return };
        // SAFETY: a live array.
        unsafe { add_next_index_long(array, value) };
    }

    /// `add_next_index_null`.
    #[inline]
    pub fn push_null(&mut self) {
        let Some(array) = self.separate() else { return };
        // SAFETY: a live array.
        unsafe { add_next_index_null(array) };
    }

    /// `add_assoc_stringl_ex`, or `add_assoc_null_ex` for `None`.
    #[inline]
    pub fn insert_bytes(&mut self, key: &[u8], value: Option<&[u8]>) {
        self.insert(
            key,
            value.map_or_else(Owned::null, |value| ZString::new(value).into()),
        );
    }

    /// `add_assoc_long_ex`.
    #[inline]
    pub fn insert_long(&mut self, key: &[u8], value: zend_long) {
        self.insert(key, Owned::long(value));
    }

    /// `add_assoc_double_ex`.
    #[inline]
    pub fn insert_double(&mut self, key: &[u8], value: f64) {
        self.insert(key, Owned::double(value));
    }

    /// `add_assoc_bool_ex`.
    #[inline]
    pub fn insert_bool(&mut self, key: &[u8], value: bool) {
        self.insert(key, Owned::bool(value));
    }

    /// `zend::array_set`: `Z_TRY_ADDREF_P(value)`, then `add_assoc_zval_ex`, so
    /// the array holds its own reference.
    #[inline]
    pub fn insert_value(&mut self, key: &[u8], value: &Value) {
        self.insert(key, value.copy());
    }

    /// `add_next_index_zval`: the array takes `value`, which is released
    /// instead when the array has no next index (it would pass `ZEND_LONG_MAX`).
    #[inline]
    pub fn push(&mut self, value: Owned) {
        let Some(array) = self.separate().and_then(|_| self.0.array()) else {
            return;
        };
        // SAFETY: a live array; it copies the zval in when it inserts it.
        let added = unsafe { zend_hash_next_index_insert(array.as_ptr(), value.as_ptr()) };
        if !added.is_null() {
            std::mem::forget(value);
        }
    }

    /// `add_assoc_zval_ex`: the array takes over `value`. A key already there
    /// releases its value (as [`release`] does), which can run `__destruct`.
    #[inline]
    pub fn insert(&mut self, key: &[u8], value: Owned) {
        let Some(array) = self.separate() else { return };
        // SAFETY: a live array.
        unsafe { symtable_update(array, key, value) };
    }

    /// `zend_hash_add_new`: the array takes over `value` under `key`, which the
    /// caller knows is absent, and references the key itself.
    #[inline]
    pub fn add_new(&mut self, key: &ZStr, value: Owned) {
        let Some(array) = self.separate().and_then(|_| self.0.array()) else {
            return;
        };
        let mut value = value.into_zval();
        // SAFETY: a live table and key; the table takes the value.
        unsafe { zend_hash_add_new(array.as_ptr(), key.as_ptr(), &raw mut value) };
    }

    /// `zend_hash_update`: the array takes over `value` under `key`, releasing
    /// what was there (as [`release`] does), and references the key itself.
    #[inline]
    pub fn update(&mut self, key: &ZStr, value: Owned) {
        let Some(array) = self.separate().and_then(|_| self.0.array()) else {
            return;
        };
        let mut value = value.into_zval();
        // SAFETY: as `add_new`.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_hash_update(array.as_ptr(), key.as_ptr(), &raw mut value)
        }));
    }

    /// `swr_zend_array_add_or_merge`: `value` under `key`, or appended to what
    /// is there, made a list first unless it is one.
    #[inline]
    pub fn merge(&mut self, key: &[u8], value: Owned) {
        let Some(array) = self.separate() else { return };
        let mut value = value.into_zval();
        // SAFETY: a live array; it takes the value, and Zend copies the key.
        unsafe {
            swr_zend_array_add_or_merge(array, crate::key_ptr(key), key.len(), &raw mut value);
        }
    }
}

/// `add_assoc_zval_ex(array, key, value)`, with a released value's
/// `__destruct` run inside `zend_try` and a bailout deferred.
/// # Safety
/// `array` is a live array zval.
pub(crate) unsafe fn symtable_update(array: *mut zval, key: &[u8], value: Owned) {
    let mut value = value.into_zval();
    // SAFETY: per the caller; the array takes the value, and Zend copies the key.
    bailout::defer(bailout::trapped(unsafe {
        swr_zend_try_symtable_update(array, crate::key_ptr(key), key.len(), &raw mut value)
    }));
}

impl OwnedArray {
    #[must_use]
    #[inline]
    pub fn into_owned(self) -> Owned {
        self.0
    }
}

impl std::ops::Deref for OwnedArray {
    type Target = ArrayMut;

    #[inline]
    fn deref(&self) -> &ArrayMut {
        // SAFETY: `ArrayMut`, `Owned` and `Value` are one transparent zval.
        unsafe { &*ptr::from_ref::<Value>(&self.0).cast::<ArrayMut>() }
    }
}

impl std::ops::DerefMut for OwnedArray {
    #[inline]
    fn deref_mut(&mut self) -> &mut ArrayMut {
        // SAFETY: as above, borrowed mutably.
        unsafe { ArrayMut::from_ptr(self.0.as_ptr()) }
    }
}

impl std::ops::Deref for ArrayMut {
    type Target = Value;

    #[inline]
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl std::ops::Deref for Owned {
    type Target = Value;

    #[inline]
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl From<ZString> for Owned {
    /// `ZVAL_STR`: the zval takes over the string's reference.
    #[inline]
    fn from(string: ZString) -> Self {
        let owned = Self::default();
        // SAFETY: a slot this owns takes the reference `string` gives up.
        unsafe { crate::value::set_str(owned.as_ptr(), string.into_raw()) };
        owned
    }
}

impl Drop for Owned {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the zval this owns, destroyed once.
        unsafe { self.0.release() };
    }
}

impl Value {
    /// `zval_ptr_dtor`. An object, or an array or reference that may hold one, can
    /// run PHP code there (`__destruct` on the last reference, or the cycle
    /// collector a possible root starts), so it is released inside `zend_try` and
    /// a bailout is deferred; anything else runs none.
    /// # Safety
    /// The caller gives the value up.
    #[inline]
    pub(crate) unsafe fn release(&self) {
        if matches!(self.kind(), IS_OBJECT | IS_ARRAY | IS_REFERENCE) {
            // SAFETY: a live value, as the caller promises.
            bailout::defer(bailout::trapped(unsafe {
                swr_zend_try_ptr_dtor(self.as_ptr())
            }));
        } else if self.kind() >= IS_STRING {
            // Types below `IS_STRING` are scalars, never counted.
            // SAFETY: as above.
            unsafe { zval_ptr_dtor(self.as_ptr()) };
        }
    }
}
