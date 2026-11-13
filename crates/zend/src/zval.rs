//! Safe Zend values: `&Value` borrows a zval, `&Array` a hash table and `&ZStr`
//! a string; `ZString` owns a string reference and `Owned` a zval, each
//! released in `Drop`. Every `unsafe` they need lives here; a caller builds one
//! from a raw pointer once (`from_ptr`), or gets one from [`crate::Call`], and
//! reads it safely from then on.
use crate::{
    bailout,
    sys::{
        _zval_struct__bindgen_ty_1, _zval_struct__bindgen_ty_2, HashPosition, HashTable, IS_ARRAY,
        IS_DOUBLE, IS_FALSE, IS_INTERNED_STRING_EX, IS_LONG, IS_NULL, IS_OBJECT, IS_PTR,
        IS_REFERENCE, IS_STR_INTERNED, IS_STRING, IS_STRING_EX, IS_TRUE, IS_UNDEF,
        add_next_index_stringl, php_format_date, swr_zend_array_add_or_merge,
        swr_zend_http_build_query, swr_zend_separate_array, swr_zend_serialize,
        swr_zend_string_release, swr_zend_try_array_merge, swr_zend_try_get_double,
        swr_zend_try_get_long, swr_zend_try_get_string, swr_zend_try_hash_clean,
        swr_zend_try_hash_update, swr_zend_try_json_encode, swr_zend_try_ptr_dtor,
        swr_zend_try_symtable_update, swr_zend_url_decode, swr_zval_is_true, zend_class_entry,
        zend_hash_add_new, zend_hash_index_add, zend_hash_next_index_insert, zend_long,
        zend_object, zend_reference, zend_string, zend_trace_to_string, zend_ulong, zend_value,
        zval, zval_ptr_dtor,
    },
};
use std::{
    borrow::{Borrow, Cow},
    cell::UnsafeCell,
    ffi::{CStr, c_char, c_void},
    io::IoSlice,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};

unsafe extern "C" {
    /// `Z_TRY_ADDREF_P`: one more reference to what a counted zval holds, and
    /// nothing for any other. It reads the zval and writes the count only, on
    /// the thread the borrow is on; a reference nothing releases is a leak.
    safe fn swr_zval_try_addref(value: &Value);
    /// `zend_array_count`: it reads the table, and may recount its elements.
    safe fn zend_array_count(table: &Array) -> u32;
    /// `zend_array_is_list`: it reads the table.
    safe fn swr_zend_array_is_list(table: &Array) -> bool;
    /// `zend_hash_index_find`: null (`None`), or the element under `index`,
    /// which lives as the table's others do, while the table is borrowed.
    safe fn zend_hash_index_find(table: &Array, index: zend_ulong) -> Option<&Value>;
    /// `zend_hash_str_find`: as [`zend_hash_index_find`], under the `length`
    /// bytes at `key`, which it reads.
    fn zend_hash_str_find(table: &Array, key: *const c_char, length: usize) -> Option<&Value>;
    /// `Z_OBJPROP_P` of an object zval: its property table, which its
    /// `get_properties` handler may build; null (`None`) for none.
    fn swr_zend_object_properties(object: &Value) -> Option<&Array>;
    /// `zend_zval_value_name`: it reads the zval, and returns a static type
    /// name or the name of the object's class, which outlives its objects.
    /// `mixed` for a slot that holds no PHP value.
    safe fn swr_zend_zval_value_name(value: &Value) -> crate::helpers::CText<'_>;
    /// `zend_zval_type_name`: it reads the zval, and returns a static type
    /// name or, before PHP 8.3, the name of the object's class. `mixed` for a
    /// slot that holds no PHP value.
    safe fn swr_zend_zval_type_name(value: &Value) -> crate::helpers::CText<'_>;
    /// `ZEND_HASH_FOREACH`'s step: null (`None`) past the end, or the next
    /// defined element at or after `position` (any position: the walk stops at
    /// the table's last slot), which then points past it, with its key (null
    /// for an integer key) and index written. Past the end `key` is whatever
    /// the last slot it skipped held, so it stays a raw pointer.
    safe fn swr_zend_hash_next<'a>(
        table: &'a Array,
        position: &mut HashPosition,
        key: &mut *mut zend_string,
        index: &mut zend_ulong,
    ) -> Option<&'a Value>;
    /// `zend_ini_parse_quantity`: it reads the string, and writes a new error
    /// string or leaves `error` alone. It raises nothing itself.
    safe fn zend_ini_parse_quantity(value: &ZStr, error: &mut *mut zend_string) -> zend_long;
    /// `zendi_smart_strcmp`: it reads the two strings' bytes and lengths, as
    /// numbers when both are numeric (parsed with errors off, so nothing is
    /// raised) and as bytes otherwise.
    safe fn swr_zend_smart_strcmp(a: &ZStr, b: &ZStr) -> std::ffi::c_int;
    /// `zend_double_to_str`: a new string; no PHP code runs.
    safe fn zend_double_to_str(value: f64) -> *mut zend_string;
    /// `number_format(number)`: a new string; no PHP code runs.
    safe fn swr_zend_number_format(number: zend_long) -> *mut zend_string;
    /// `array_init(target)`: `target` holds a new, empty array in place of
    /// what it held, which is not released. No PHP code runs.
    safe fn swr_zend_array_init(target: &Owned);
    /// `php_url_encode`: it reads the lent bytes (an `IoSlice` is an iovec) and
    /// returns a new string; no PHP code runs.
    safe fn swr_bytes_url_encode(bytes: IoSlice<'_>) -> *mut zend_string;
}

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
        // SAFETY: a live zval, borrowed only for this read.
        crate::kind(unsafe { &*self.as_ptr() })
    }

    /// The value union's first word as a pointer: what `Z_STR_P`, `Z_ARRVAL_P`,
    /// `Z_OBJ_P`, `Z_REF_P` and `Z_PTR_P` read. Only [`Value::kind`] says what
    /// it points to, if anything.
    #[inline]
    fn payload<T>(&self) -> *mut T {
        // SAFETY: a live zval, which its value union leads (`value`'s
        // assertion); any eight bytes are a raw pointer.
        unsafe { self.as_ptr().cast::<*mut T>().read() }
    }

    /// The value union's first word as bits: what `Z_LVAL_P` and `Z_DVAL_P` read.
    #[inline]
    fn bits(&self) -> u64 {
        // SAFETY: as `payload`; any eight bytes are a `u64`.
        unsafe { self.as_ptr().cast::<u64>().read() }
    }

    /// `Z_TYPE_P(value) == IS_NULL`.
    #[must_use]
    #[inline]
    pub fn is_null(&self) -> bool {
        self.kind() == IS_NULL
    }

    /// `ZVAL_DEREF`, held: what a reference refers to now, as a copy of its own
    /// (one addref) that PHP code assigning the reference later leaves alone;
    /// anything else, this value.
    #[must_use]
    #[inline]
    pub fn dereferenced(&self) -> Cow<'_, Self> {
        if self.kind() != IS_REFERENCE {
            return Cow::Borrowed(self);
        }
        // SAFETY: the reference, which this zval keeps alive, holds the value;
        // it is copied before any PHP code can assign it.
        Cow::Owned(unsafe { &*self.referent().cast::<Self>() }.copy())
    }

    /// `ZVAL_DEREF`'s slot: the zval a reference refers to, else this one. A
    /// pointer, since PHP code that assigns the reference rewrites the slot
    /// under any borrow of it.
    #[must_use]
    #[inline]
    pub fn referent(&self) -> *mut zval {
        if self.kind() != IS_REFERENCE {
            return self.as_ptr();
        }
        // A reference zval holds `ref`.
        self.payload::<zend_reference>()
            .wrapping_byte_add(std::mem::offset_of!(zend_reference, val))
            .cast()
    }

    /// `Z_LVAL_P` of an int, with no conversion.
    #[must_use]
    #[inline]
    pub fn as_long(&self) -> Option<zend_long> {
        // An int zval holds `lval`.
        (self.kind() == IS_LONG).then(|| self.bits().cast_signed())
    }

    /// `Z_DVAL_P` of a float, with no conversion.
    #[must_use]
    #[inline]
    pub fn as_double(&self) -> Option<f64> {
        // A float zval holds `dval`.
        (self.kind() == IS_DOUBLE).then(|| f64::from_bits(self.bits()))
    }

    /// `Z_STR_P` of a string.
    #[must_use]
    #[inline]
    pub fn str(&self) -> Option<&ZStr> {
        if self.kind() != IS_STRING {
            return None;
        }
        // SAFETY: a string zval holds `str`, which it keeps alive while borrowed.
        unsafe { ZStr::from_ptr(self.payload()) }
    }

    /// `Z_OBJ_P` of an object.
    #[must_use]
    #[inline]
    pub fn object(&self) -> Option<*mut zend_object> {
        // An object zval holds `obj`.
        (self.kind() == IS_OBJECT).then(|| self.payload())
    }

    /// `Z_OBJ_P` of an object with `class`'s handlers: one that `class`'s
    /// create handler made, or a subclass's that inherited it.
    #[must_use]
    #[inline]
    pub fn object_of(&self, class: &crate::Class) -> Option<*mut zend_object> {
        let object = self.object()?;
        // SAFETY: the zval holds the live object.
        (unsafe { (*object).handlers } == class.handlers()).then_some(object)
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
        // A pointer zval holds `ptr`.
        (self.kind() == IS_PTR).then(|| self.payload())
    }

    /// Lend what the zval holds to `edit`, with null in its place meanwhile,
    /// then put it back and release whatever `edit` left there. The caller
    /// holds nothing borrowed from the zval.
    pub(crate) fn lend<R>(&self, edit: impl FnOnce(&mut Owned) -> R) -> R {
        let mut held = Owned::null();
        // SAFETY: two live zvals: nothing borrows this one's content, and
        // `held` is this frame's, borrowed by nothing else at either swap.
        let swap = |held: &Owned| unsafe { self.0.get().swap(held.0.0.get()) };
        swap(&held);
        let result = edit(&mut held);
        swap(&held);
        result
    }

    /// Property writes on the object this holds, under `scope` (Zend's view of
    /// its properties; `None` for none), while it is borrowed.
    #[must_use]
    #[inline]
    pub fn props(&self, scope: Option<&crate::Class>) -> Option<crate::Props<'_>> {
        let object = self.object()?;
        let scope = scope.map_or(ptr::null_mut(), crate::Class::entry);
        // SAFETY: the zval holds the object while borrowed; a registered class or null.
        Some(unsafe { crate::Props::new(scope, object) })
    }

    /// `sw_zend_read_property_ex` of an object: the slot of the instance
    /// property its class declares as `name` (an uninitialised typed property
    /// reads as null), or None for no object or no such property.
    #[must_use]
    #[inline]
    pub fn property(&self, name: &CStr) -> Option<&Self> {
        self.props(None)?.slot(name)
    }

    /// `zend_read_property(scope, object, name, silent)` of an object, in
    /// `zend_try`: a copy of the property `scope` sees (the object's own class
    /// for None), or the value a `__get` returned. None for no object.
    /// # Errors
    /// The bailout a `__get` raised, trapped.
    pub fn read_property(
        &self,
        scope: Option<&crate::Class>,
        name: &CStr,
    ) -> Result<Option<Owned>, bailout::Bailout> {
        self.read(self.scope(scope), name.to_bytes(), true)
    }

    /// [`Value::read_property`], not silent (`zend_read_property`'s `silent`
    /// 0): an undefined property warns, as PHP code reading it does, and reads
    /// as null.
    /// # Errors
    /// The bailout a `__get` or the warning's error handler raised, trapped.
    pub fn read_property_loud(
        &self,
        scope: Option<&crate::Class>,
        name: &CStr,
    ) -> Result<Option<Owned>, bailout::Bailout> {
        self.read(self.scope(scope), name.to_bytes(), false)
    }

    /// The class a property read sees the object's properties as: `scope`,
    /// or the object's own class for None.
    fn scope(&self, scope: Option<&crate::Class>) -> Option<crate::ClassEntry<'_>> {
        match scope {
            Some(scope) => scope.class_entry(),
            None => self.class_entry(),
        }
    }

    /// `zend_read_property(scope, object, name, silent)` of an object, in
    /// `zend_try`, as [`Value::read_property`], under `scope` (None: no scope).
    pub(crate) fn read(
        &self,
        scope: Option<crate::ClassEntry<'_>>,
        name: &[u8],
        silent: bool,
    ) -> Result<Option<Owned>, bailout::Bailout> {
        let Some(object) = self.object() else {
            return Ok(None);
        };
        let scope = scope.map_or(ptr::null_mut(), crate::ClassEntry::as_ptr);
        let rv = Owned::default();
        let mut found = ptr::null_mut();
        // SAFETY: the zval holds the object while borrowed; a class or null; Zend
        // reads `name`, and writes `rv` (undefined, this frame's) or nothing.
        bailout::trapped(unsafe {
            crate::sys::swr_zend_try_facade_read(
                scope,
                object,
                name.as_ptr().cast(),
                name.len(),
                silent,
                rv.as_ptr(),
                &raw mut found,
            )
        })?;
        if found == rv.as_ptr() {
            return Ok(Some(rv));
        }
        // SAFETY: null, or a property slot of the object, which the borrow keeps
        // alive, or the executor's null, which lives as long as it does.
        Ok(unsafe { Self::from_ptr(found) }.map(Self::copy))
    }

    /// `zend_call_method(object, scope, NULL, method, &result, args)` on the
    /// object this holds, inside `zend_try` (at most two arguments, which Zend
    /// copies into the call): `scope`'s method (`None`: the object's class's).
    /// Undefined, and nothing is called, for no object or one that isn't a
    /// `scope`. A result the call left when it bailed out is leaked, as
    /// upstream leaves it.
    /// # Errors
    /// The method bailed out, or a bailout is pending.
    /// # Panics
    /// More than two arguments: `zend_call_method` takes at most two.
    #[expect(
        clippy::panic_in_result_fn,
        reason = "more than two arguments is a caller bug, not a bailout"
    )]
    pub fn call_method(
        &self,
        scope: Option<&crate::Class>,
        method: &[u8],
        args: &[&Self],
    ) -> Result<Owned, bailout::Bailout> {
        assert!(
            args.len() <= 2,
            "zend_call_method takes at most two arguments"
        );
        let result = Owned::default();
        let Some(object) = self.object() else {
            return Ok(result);
        };
        if scope.is_some_and(|scope| !self.instance_of(scope)) {
            return Ok(result);
        }
        if bailout::pending() {
            // Nothing is called, as upstream never ran past a bailout.
            return Err(bailout::Bailout);
        }
        let scope = scope.map_or(ptr::null_mut(), crate::Class::entry);
        let arg = |n: usize| args.get(n).map_or(ptr::null_mut(), |arg| arg.as_ptr());
        #[expect(clippy::cast_possible_truncation, reason = "at most two arguments")]
        let argc = args.len() as u32;
        // SAFETY: the zval holds the object while borrowed, `scope` is null or
        // a class it is an instance of, the result a slot this owns, and the
        // arguments live values; Zend runs in the C shim's frame.
        let called = bailout::trapped(unsafe {
            crate::sys::swr_zend_try_call_method(
                object,
                scope,
                method.as_ptr().cast(),
                method.len(),
                result.as_ptr(),
                argc,
                arg(0),
                arg(1),
            )
        });
        match called {
            Ok(()) => Ok(result),
            Err(bailout) => {
                #[expect(
                    clippy::mem_forget,
                    reason = "upstream never ran past a bailout to destroy it"
                )]
                std::mem::forget(result);
                Err(bailout)
            }
        }
    }

    /// `call_user_function(NULL, nullptr, self, &result, 0, nullptr)` inside
    /// `zend_try`: the callable this holds, called with no arguments, and its
    /// result. The call holds a reference of its own to the callable, so PHP
    /// code it runs can't free it.
    /// # Errors
    /// The callable bailed out.
    pub fn call(&self) -> Result<Owned, bailout::Bailout> {
        let mut callable = self.copy();
        let result = Owned::default();
        let mut fci = crate::EMPTY_FCI;
        fci.size = size_of::<crate::sys::zend_fcall_info>();
        // ZVAL_COPY_VALUE of the reference `callable` holds through the call.
        fci.function_name = *callable.0.0.get_mut();
        fci.retval = result.as_ptr();
        // SAFETY: a live callable and a return slot this owns; no Rust borrow
        // spans the call.
        let called = unsafe { bailout::call(&raw mut fci, ptr::null_mut()) };
        drop(callable);
        called?;
        Ok(result)
    }

    /// `ZSTR_VAL(Z_OBJCE_P(value)->name)` of an object.
    #[must_use]
    #[inline]
    pub fn class_name(&self) -> Option<&CStr> {
        Some(self.class_entry()?.name().c_str())
    }

    /// `Z_OBJCE_P` of an object as a handle, for its name, flags and parent.
    #[must_use]
    #[inline]
    pub fn class_entry(&self) -> Option<crate::ClassEntry<'_>> {
        Some(self.props(None)?.class_entry())
    }

    /// `Z_OBJCE_P` of an object. Zend may change the class (its flags, its
    /// statics) at any time, so it stays a pointer.
    #[must_use]
    #[inline]
    pub fn class(&self) -> Option<*mut zend_class_entry> {
        Some(self.props(None)?.class())
    }

    /// `Z_OBJCE_P(value)->ce_flags` of an object.
    #[must_use]
    #[inline]
    pub fn class_flags(&self) -> Option<u32> {
        Some(self.class_entry()?.flags())
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

    /// `http_build_query($value)` (RFC 1738): None for a value that is no array or
    /// object, or one with nothing to encode.
    #[must_use]
    pub fn http_build_query(&self) -> Option<ZString> {
        // SAFETY: a live zval; Zend returns null or a new string.
        let query = unsafe { swr_zend_http_build_query(self.as_ptr()) };
        (!query.is_null()).then(|| ZString(query))
    }

    /// `serialize($value)`: None when it produced nothing.
    /// # Errors
    /// The bailout a `__serialize` raised, trapped.
    pub fn serialize(&self) -> Result<Option<ZString>, bailout::Bailout> {
        let mut bailed = false;
        // SAFETY: a live zval; the shim traps a bailout and returns null or a new string.
        let serialized = unsafe { swr_zend_serialize(self.as_ptr(), &raw mut bailed) };
        bailout::trapped(bailed.into())?;
        Ok((!serialized.is_null()).then(|| ZString(serialized)))
    }

    /// `json_encode($value)`: None when the encoder failed.
    /// # Errors
    /// The bailout a `jsonSerialize` raised, trapped.
    pub fn json_encode(&self) -> Result<Option<ZString>, bailout::Bailout> {
        let mut encoded = ptr::null_mut();
        // SAFETY: a live zval; the shim traps a bailout and writes null or a new string.
        bailout::trapped(unsafe { swr_zend_try_json_encode(self.as_ptr(), &raw mut encoded) })?;
        Ok((!encoded.is_null()).then(|| ZString(encoded)))
    }

    /// `Z_STRVAL_P`/`Z_STRLEN_P` of a string.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> Option<&[u8]> {
        self.str().map(ZStr::bytes)
    }

    /// `Z_OBJPROP_P` of an object: its property table, which Zend builds if
    /// need be and the object keeps.
    /// # Safety
    /// No PHP code runs while the table is borrowed: a property write can
    /// resize the table or replace it.
    #[must_use]
    #[inline]
    pub unsafe fn properties(&self) -> Option<&Array> {
        if self.kind() != IS_OBJECT {
            return None;
        }
        // SAFETY: a live object zval, whose table it keeps while borrowed.
        unsafe { swr_zend_object_properties(self) }
    }

    /// `Z_TRY_ADDREF_P`: one more reference to what a counted zval holds, which
    /// whoever the caller hands it to releases (nobody: a leak).
    #[inline]
    pub fn add_ref(&self) {
        swr_zval_try_addref(self);
    }

    /// `zend_zval_value_name`: the type, or the object's class, a type error names.
    #[must_use]
    #[inline]
    pub fn value_name(&self) -> &CStr {
        swr_zend_zval_value_name(self).get()
    }

    /// `zend_zval_type_name`: the type an error names (an object's is `object`
    /// from PHP 8.3, its class before).
    #[must_use]
    #[inline]
    pub fn type_name(&self) -> &CStr {
        swr_zend_zval_type_name(self).get()
    }

    /// `Z_ARRVAL_P` of an array.
    #[must_use]
    #[inline]
    pub fn array(&self) -> Option<&Array> {
        if self.kind() != IS_ARRAY {
            return None;
        }
        // SAFETY: an array zval holds `arr`, which it keeps while borrowed;
        // `Array` is a transparent table.
        Some(unsafe { &*self.payload::<Array>() })
    }

    /// `Z_ARRVAL_P` of an array, as the pointer the zval holds: unlike a borrow
    /// of the table, it reaches what an allocation keeps behind the table.
    #[must_use]
    #[inline]
    pub fn array_ptr(&self) -> Option<*mut HashTable> {
        // An array zval holds `arr`.
        (self.kind() == IS_ARRAY).then(|| self.payload())
    }

    /// `ZVAL_COPY_VALUE`: the same bits, without a reference of their own, for
    /// a slot that must not outlive this value or release it.
    #[must_use]
    #[inline]
    pub fn copy_value(&self) -> zval {
        // SAFETY: a live zval, read while borrowed.
        unsafe { self.as_ptr().read() }
    }

    /// `ZVAL_COPY`: the same value with a reference of its own.
    #[must_use]
    #[inline]
    pub fn copy(&self) -> Owned {
        // The copy carries the reference it adds below.
        let owned = Owned(Self(UnsafeCell::new(self.copy_value())));
        swr_zval_try_addref(&owned);
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
        zend_array_count(self)
    }

    /// `zend_trace_to_string(trace, false)`: a backtrace array as an exception prints it.
    #[must_use]
    pub fn trace_to_string(&self) -> ZString {
        // SAFETY: a live table; Zend returns a new string.
        ZString(unsafe { zend_trace_to_string(self.as_ptr(), false) })
    }

    /// `zend_array_is_list`.
    #[must_use]
    #[inline]
    pub fn is_list(&self) -> bool {
        swr_zend_array_is_list(self)
    }

    /// `zend_hash_str_find`.
    #[must_use]
    #[inline]
    pub fn find(&self, key: &[u8]) -> Option<&Value> {
        // SAFETY: Zend reads `key`'s bytes only.
        unsafe { zend_hash_str_find(self, key.as_ptr().cast(), key.len()) }
    }

    /// `zend_hash_index_find`.
    #[must_use]
    #[inline]
    pub fn index(&self, index: zend_ulong) -> Option<&Value> {
        zend_hash_index_find(self, index)
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

    /// `ZEND_HASH_FOREACH_KEY_VAL` with `ZVAL_DEREF`, stopping once a
    /// whole-thread exit is pending.
    pub fn for_each<'a, F: FnMut(Key<'a>, &'a Value)>(&'a self, mut each: F) {
        let mut entries = self.entries();
        while let Some((key, value)) = entries.next_keyed() {
            // SAFETY: the element, or what the reference it holds refers to,
            // which the element keeps alive while the table is borrowed.
            each(key, unsafe { &*value.referent().cast::<Value>() });
            if bailout::thread_exit_pending() {
                break;
            }
        }
    }
}

/// An array zval with a reference of its own ([`Value::hold`]), released the
/// way `zend_array_release` does: a shared table only loses the reference,
/// without `zval_ptr_dtor`'s possible-root check or `zend_try`, and only the
/// last one is destroyed through [`Owned`]'s drop.
pub struct HeldArray(Owned);

impl Value {
    /// `ZVAL_COPY` of an array, so a callback that replaces or unsets its
    /// holder separates the table instead of freeing it; None for anything else.
    #[must_use]
    #[inline]
    pub fn hold(&self) -> Option<HeldArray> {
        self.array()?;
        Some(HeldArray(self.copy()))
    }
}

impl std::ops::Deref for HeldArray {
    type Target = Value;

    #[inline]
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl Drop for HeldArray {
    #[inline]
    fn drop(&mut self) {
        let Some(array) = self.0.array() else { return };
        let [refcount, info] = header(&array.0);
        let immutable = info & crate::sys::IS_ARRAY_IMMUTABLE != 0;
        if !immutable && refcount == 1 {
            // The last reference: `Owned`'s drop destroys the table inside `zend_try`.
            return;
        }
        if !immutable {
            // SAFETY: another holder keeps the table live; this gives up its reference.
            unsafe { (*array.as_ptr()).gc.refcount -= 1 };
        }
        // Nothing is left for `Owned`'s drop to release.
        #[expect(
            clippy::mem_forget,
            reason = "the reference was given up above; nothing is left to release"
        )]
        std::mem::forget(std::mem::take(&mut self.0));
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
        let (name, index, value) = self.next_stored()?;
        let key = name.map_or(Key::Index(index), |name| Key::Name(name.bytes()));
        Some((key, value))
    }

    /// The next element with its key as the table stores it: the string
    /// (None for an integer key), and the integer, to put an element back
    /// under (`zend::KeyValue`).
    pub fn next_stored(&mut self) -> Option<(Option<&'a ZStr>, zend_ulong, &'a Value)> {
        let (mut key, mut index) = (ptr::null_mut(), 0);
        let value = swr_zend_hash_next(self.table, &mut self.position, &mut key, &mut index)?;
        // SAFETY: the found element's key: null or a string, which the table
        // keeps alive.
        Some((unsafe { ZStr::from_ptr(key) }, index, value))
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
        // SAFETY: a live string, read while borrowed.
        let length = unsafe { (*self.as_ptr()).len };
        // SAFETY: a live string holds `len` bytes at `val`, which it keeps while borrowed.
        unsafe { std::slice::from_raw_parts(crate::string::val(self.as_ptr()), length) }
    }

    /// The bytes and the NUL Zend keeps after them; a NUL among the bytes stays.
    #[must_use]
    #[inline]
    pub fn bytes_with_nul(&self) -> &[u8] {
        let length = self.bytes().len() + 1;
        // SAFETY: a live string holds its bytes at `val` and a NUL after them.
        unsafe { std::slice::from_raw_parts(crate::string::val(self.as_ptr()), length) }
    }

    /// `ZSTR_VAL` as C's `%s` reads it: up to the first NUL.
    #[must_use]
    #[inline]
    pub fn c_str(&self) -> &CStr {
        // One scan, which ends at the NUL after the bytes when none comes before it.
        CStr::from_bytes_until_nul(self.bytes_with_nul()).unwrap_or_default()
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

    /// `zendi_smart_strcmp`: PHP's `==` order of two strings, numeric when
    /// both are numeric strings.
    #[must_use]
    #[inline]
    pub fn smart_cmp(&self, other: &Self) -> std::ffi::c_int {
        swr_zend_smart_strcmp(self, other)
    }

    /// `zend_ini_parse_quantity`: an ini quantity ("64M").
    /// # Errors
    /// What Zend parsed before the string stopped parsing, and its message.
    pub fn parse_quantity(&self) -> Result<zend_long, (zend_long, ZString)> {
        let mut error = ptr::null_mut();
        let size = zend_ini_parse_quantity(self, &mut error);
        if error.is_null() {
            Ok(size)
        } else {
            Err((size, ZString(error)))
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

    /// `ZSTR_EMPTY_ALLOC()`: the interned empty string, whose release does nothing.
    #[must_use]
    #[inline]
    pub fn empty() -> Self {
        Self(crate::value::empty_string())
    }

    #[must_use]
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        ZStr::bytes(self)
    }

    /// `php_url_encode`: `bytes` as `urlencode()` encodes them.
    #[must_use]
    pub fn url_encode(bytes: &[u8]) -> Self {
        Self(swr_bytes_url_encode(IoSlice::new(bytes)))
    }

    /// `php_url_decode` of a copy of `bytes`: them as `urldecode()` decodes them.
    #[must_use]
    pub fn url_decode(bytes: &[u8]) -> Self {
        let mut buffer = crate::string::ZBuf::with_capacity(bytes.len());
        buffer.extend_from_slice(bytes);
        // SAFETY: the buffer's `bytes.len()` written bytes, decoded in place; the
        // NUL the decoder ends them with fits the string's own.
        let length = unsafe { swr_zend_url_decode(buffer.as_mut_ptr().cast(), bytes.len()) };
        // Decoding only shortens.
        buffer.truncate(length);
        buffer.into_zstring()
    }

    /// `zend_double_to_str`: `value` as PHP prints a float.
    #[must_use]
    pub fn from_double(value: f64) -> Self {
        Self(zend_double_to_str(value))
    }

    /// `number_format(number)`: `number` with a `,` between its thousands.
    #[must_use]
    pub fn number_format(number: zend_long) -> Self {
        Self(swr_zend_number_format(number))
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

/// A string MINIT interns permanently ([`crate::Minit::intern`]), kept in a
/// static and read for the life of the process.
#[derive(Default)]
pub struct Interned(pub(crate) AtomicPtr<zend_string>);

impl Interned {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicPtr::new(ptr::null_mut()))
    }

    /// The string; None before MINIT interned it.
    #[must_use]
    #[inline]
    pub fn get(&self) -> Option<&'static ZStr> {
        // SAFETY: null, or a permanently interned string, which Zend never frees.
        unsafe { ZStr::from_ptr(self.0.load(Relaxed)) }
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
    /// `php_json_decode(text)` with objects as arrays; false when `text` is
    /// not JSON. It runs no PHP code.
    #[must_use]
    pub fn json_decode(text: &[u8]) -> Self {
        let decoded = Self::default();
        // SAFETY: the text's bytes, only read, and an undefined slot this owns.
        unsafe {
            crate::sys::swr_zend_json_decode(text.as_ptr().cast(), text.len(), decoded.as_ptr());
        };
        decoded
    }

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
        // SAFETY: `OwnedArray` is, through `ArrayMut`, a transparent `Owned`
        // holding an array.
        Some(unsafe { &mut *ptr::from_mut(self).cast::<OwnedArray>() })
    }

    /// `ZVAL_OBJ(value, std)`: the result takes over the reference `std` carries.
    /// # Safety
    /// `std` is a live object carrying a reference the caller gives up.
    #[must_use]
    #[inline]
    pub unsafe fn object(std: *mut zend_object) -> Self {
        let mut zval = UNDEF;
        zval.value = zend_value { obj: std };
        zval.u1.type_info = crate::sys::IS_OBJECT_EX;
        Self(Value(UnsafeCell::new(zval)))
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
pub struct OwnedArray(ArrayMut);

impl Default for OwnedArray {
    #[inline]
    fn default() -> Self {
        let owned = Owned::default();
        swr_zend_array_init(&owned);
        Self(ArrayMut(owned))
    }
}

/// An array zval this may add to: an [`OwnedArray`]'s, or an array property
/// ([`crate::Props::array`]). Every write separates the array first
/// (`SEPARATE_ARRAY`), so another holder (a [`Value::copy`], a property it
/// was copied to) never sees it, and an immutable array is never written.
/// Only ever lent (`&mut ArrayMut`), but for the one an [`OwnedArray`] holds
/// by value, whose drop is the zval's: nothing else makes one, so no borrowed
/// zval is released through this.
#[repr(transparent)]
pub struct ArrayMut(Owned);

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
    /// None once a bailout is pending (nothing is written then, because
    /// upstream never ran past it) or when PHP code has since put something
    /// else in the slot. Separate again after anything that can run PHP code:
    /// it may have put another array in the slot.
    #[inline]
    pub fn separate(&mut self) -> Option<Separated<'_>> {
        if bailout::pending() {
            return None;
        }
        // `GC_REFCOUNT(arr) > 1` is the only case SEPARATE_ARRAY acts on (an
        // immutable array counts 2), so an unshared one skips the call.
        if header(&self.0.array()?.0)[0] > 1 {
            // SAFETY: an array zval borrowed mutably; a shared or immutable
            // array is duplicated, and the zval's reference moves to the copy.
            unsafe { swr_zend_separate_array(self.0.as_ptr()) };
        }
        Some(Separated(self))
    }

    /// `add_next_index_stringl`: Zend copies the bytes.
    #[inline]
    pub fn push_bytes(&mut self, data: &[u8]) {
        let Some(array) = self.separate().map(|array| array.zval()) else {
            return;
        };
        // SAFETY: a live array.
        unsafe { add_next_index_stringl(array, data.as_ptr().cast(), data.len()) };
    }

    /// `add_next_index_long`.
    #[inline]
    pub fn push_long(&mut self, value: zend_long) {
        self.push(Owned::long(value));
    }

    /// `add_next_index_null`.
    #[inline]
    pub fn push_null(&mut self) {
        self.push(Owned::null());
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
        let Some(array) = self.separate().map(|array| array.table()) else {
            return;
        };
        // SAFETY: a live array; it copies the zval in when it inserts it.
        let added = unsafe { zend_hash_next_index_insert(array, value.as_ptr()) };
        if !added.is_null() {
            #[expect(clippy::mem_forget, reason = "the array owns the zval it copied in")]
            std::mem::forget(value);
        }
    }

    /// `add_assoc_zval_ex`: the array takes over `value`. A key already there
    /// releases its value (as [`Owned`]'s drop does), which can run `__destruct`.
    #[inline]
    pub fn insert(&mut self, key: &[u8], value: Owned) {
        if let Some(array) = self.separate() {
            array.update(key, value);
        }
    }

    /// `zend_hash_index_add`: the array takes over `value` under `index`, or
    /// `value` is released when the index is taken.
    #[inline]
    pub fn insert_index(&mut self, index: zend_ulong, value: Owned) {
        let Some(array) = self.separate().map(|array| array.table()) else {
            return;
        };
        // SAFETY: a live table; it copies the zval in when it adds it.
        let added = unsafe { zend_hash_index_add(array, index, value.as_ptr()) };
        if !added.is_null() {
            #[expect(clippy::mem_forget, reason = "the array owns the zval it copied in")]
            std::mem::forget(value);
        }
    }

    /// `zend_hash_add_new`: the array takes over `value` under `key`, which the
    /// caller knows is absent, and references the key itself.
    #[inline]
    pub fn add_new(&mut self, key: &ZStr, value: Owned) {
        let Some(array) = self.separate().map(|array| array.table()) else {
            return;
        };
        let mut value = value.into_zval();
        // SAFETY: a live table and key; the table takes the value.
        unsafe { zend_hash_add_new(array, key.as_ptr(), &raw mut value) };
    }

    /// `zend_hash_update`: the array takes over `value` under `key`, releasing
    /// what was there (as [`Owned`]'s drop does), and references the key itself.
    #[inline]
    pub fn update(&mut self, key: &ZStr, value: Owned) {
        self.update_at(key.as_ptr(), 0, value);
    }

    /// `zend_hash_index_update`, as [`ArrayMut::update`].
    #[inline]
    pub fn update_index(&mut self, index: zend_ulong, value: Owned) {
        self.update_at(ptr::null_mut(), index, value);
    }

    /// [`ArrayMut::update`] under `key`, or under `index` when `key` is null.
    #[inline]
    fn update_at(&mut self, key: *mut zend_string, index: zend_ulong, value: Owned) {
        let Some(array) = self.separate().map(|array| array.table()) else {
            return;
        };
        let mut value = value.into_zval();
        // SAFETY: a live table and a borrowed key or null; the table takes the
        // value and references the key itself.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_hash_update(array, key, index, &raw mut value)
        }));
    }

    /// `swr_zend_array_add_or_merge`: `value` under `key`, or appended to what
    /// is there, made a list first unless it is one.
    #[inline]
    pub fn merge(&mut self, key: &[u8], value: Owned) {
        let Some(array) = self.separate().map(|array| array.zval()) else {
            return;
        };
        let mut value = value.into_zval();
        // SAFETY: a live array; it takes the value, and Zend copies the key.
        unsafe {
            swr_zend_array_add_or_merge(array, crate::key_ptr(key), key.len(), &raw mut value);
        }
    }

    /// `php_array_merge(this, source)`, with a replaced value's `__destruct`
    /// run inside `zend_try` and a bailout deferred; nothing when `source` is
    /// no array.
    #[inline]
    pub fn merge_array(&mut self, source: &Value) {
        if source.array().is_none() {
            return;
        }
        let Some(array) = self.separate().map(|array| array.zval()) else {
            return;
        };
        // SAFETY: two live arrays.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_array_merge(array, source.as_ptr())
        }));
    }

    /// The array zval, separated, for a C writer; None once a bailout is
    /// pending.
    #[must_use]
    #[inline]
    pub fn as_mut_ptr(&mut self) -> Option<*mut zval> {
        self.separate().map(|array| array.zval())
    }
}

/// An [`ArrayMut`] that [`ArrayMut::separate`] separated: its array has no
/// other holder and is not immutable while this borrows it, so only this may
/// change the table in place.
pub struct Separated<'a>(&'a mut ArrayMut);

impl Separated<'_> {
    #[inline]
    fn zval(&self) -> *mut zval {
        self.0.0.as_ptr()
    }

    /// The table `separate` checked was there.
    #[inline]
    fn table(&self) -> *mut HashTable {
        self.0.0.array().map_or(ptr::null_mut(), Array::as_ptr)
    }

    /// `add_assoc_zval_ex(array, key, value)`, with a released value's
    /// `__destruct` run inside `zend_try` and a bailout deferred.
    #[inline]
    fn update(&self, key: &[u8], value: Owned) {
        let mut value = value.into_zval();
        // SAFETY: a live array this zval alone holds; it takes the value, and
        // Zend copies the key.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_symtable_update(
                self.zval(),
                crate::key_ptr(key),
                key.len(),
                &raw mut value,
            )
        }));
    }

    /// `zend_hash_clean`; a destructor's bailout is deferred. It takes the
    /// borrow, because a destructor may run PHP code that replaces the array.
    #[inline]
    pub fn clear(self) {
        // SAFETY: a live table this zval alone holds.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_hash_clean(self.table())
        }));
    }
}

impl OwnedArray {
    #[must_use]
    #[inline]
    pub fn into_owned(self) -> Owned {
        self.0.0
    }
}

impl std::ops::Deref for OwnedArray {
    type Target = ArrayMut;

    #[inline]
    fn deref(&self) -> &ArrayMut {
        &self.0
    }
}

impl std::ops::DerefMut for OwnedArray {
    #[inline]
    fn deref_mut(&mut self) -> &mut ArrayMut {
        &mut self.0
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

impl Borrow<Value> for Owned {
    #[inline]
    fn borrow(&self) -> &Value {
        self
    }
}

impl ToOwned for Value {
    type Owned = Owned;

    /// [`Value::copy`].
    #[inline]
    fn to_owned(&self) -> Owned {
        self.copy()
    }
}

impl From<ZString> for Owned {
    /// `ZVAL_STR`: the zval takes over the string's reference.
    #[inline]
    fn from(string: ZString) -> Self {
        Self::string(string)
    }
}

impl Drop for Owned {
    /// `zval_ptr_dtor`. An object, or an array or reference that may hold one, can
    /// run PHP code there (`__destruct` on the last reference, or the cycle
    /// collector a possible root starts), and so can a resource (a user stream's
    /// `stream_close`), so these are released inside `zend_try` and a bailout is
    /// deferred. A string runs none, and types below it are scalars, never counted.
    #[inline]
    fn drop(&mut self) {
        if self.kind() == IS_STRING {
            // SAFETY: the zval this owns, destroyed once.
            unsafe { zval_ptr_dtor(self.as_ptr()) };
        } else if self.kind() > IS_STRING {
            // SAFETY: as above.
            bailout::defer(bailout::trapped(unsafe {
                swr_zend_try_ptr_dtor(self.as_ptr())
            }));
        }
    }
}
