//! Safe Zend values: `&Value` borrows a zval, `&Array` a hash table and `&ZStr`
//! a string; `ZString` owns a string reference and `Owned` a zval, each
//! released in `Drop`. Every `unsafe` they need lives here; a caller builds one
//! from a raw pointer once (`from_ptr`), or gets one from [`crate::Call`], and
//! reads it safely from then on.
use crate::{
    bailout,
    sys::{
        _zval_struct__bindgen_ty_1, _zval_struct__bindgen_ty_2, HashPosition, HashTable, IS_ARRAY,
        IS_DOUBLE, IS_FALSE, IS_INDIRECT, IS_INTERNED_STRING_EX, IS_LONG, IS_NULL, IS_OBJECT,
        IS_PTR, IS_REFERENCE, IS_STR_INTERNED, IS_STRING, IS_STRING_EX, IS_TRUE, IS_UNDEF,
        zend_class_entry, zend_long, zend_object, zend_reference, zend_string, zend_ulong,
        zend_value, zval,
    },
};
use std::{
    borrow::{Borrow, Cow},
    cell::{Cell, UnsafeCell},
    ffi::{CStr, c_void},
    io::IoSlice,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};
use swoole_rs_corebase::lent::{Count, In, lend_in};

unsafe extern "C" {
    /// `zend_array_count`: it reads the table, and may recount its elements.
    safe fn zend_array_count(table: &Array) -> u32;
    /// `zend_array_is_list`: it reads the table.
    safe fn swr_zend_array_is_list(table: &Array) -> bool;
    /// `zend_hash_index_find`: null (`None`), or the element under `index`,
    /// which lives as the table's others do, while the table is borrowed.
    safe fn zend_hash_index_find(table: &Array, index: zend_ulong) -> Option<&Value>;
    /// `zend_hash_str_find`: as [`zend_hash_index_find`], under the lent
    /// bytes (an `IoSlice` is an iovec), which it reads only.
    safe fn zend_hash_str_find<'a, 'b>(
        table: &'a Array,
        key: In<'b, u8>,
        len: Count<'b>,
    ) -> Option<&'a Value>;
    /// `Z_OBJPROP` of a live object inside `zend_try` (nonzero for a bailout:
    /// its `get_properties` handler may build the table, and a lazy object's
    /// initializer runs PHP code): `out` then holds the table, if it has one,
    /// with a reference of its own, in place of what it held, which is not
    /// released.
    safe fn swr_zend_try_object_properties(
        object: crate::ObjectRef<'_>,
        out: &Owned,
    ) -> std::ffi::c_int;
    /// `convert_to_array` of the zval in place, inside `zend_try` (nonzero for
    /// a bailout): the reference it held goes into the new array, or (an
    /// object's) is released, which can run `__destruct`.
    safe fn swr_zend_try_convert_to_array(value: &Owned) -> std::ffi::c_int;
    /// `zend_hash_next_index_insert`: the table takes `value` under its next
    /// index, or (null, at `ZEND_LONG_MAX`) leaves it to the caller. No PHP
    /// code runs.
    safe fn zend_hash_next_index_insert(table: Insertions<'_>, value: Given<'_>) -> *mut zval;
    /// `zend_hash_index_add`: the table takes `value` under `index`, or (null,
    /// the index taken) leaves it to the caller. No PHP code runs.
    safe fn zend_hash_index_add(
        table: Insertions<'_>,
        index: zend_ulong,
        value: Given<'_>,
    ) -> *mut zval;
    /// `zend_hash_add_new`: the table takes `value` under `key`, which it
    /// references itself. No PHP code runs: it releases nothing.
    safe fn zend_hash_add_new(table: Insertions<'_>, key: &ZStr, value: Given<'_>) -> *mut zval;
    /// `zend_hash_update` under `key`, or `zend_hash_index_update` under
    /// `index` for none, inside `zend_try` (nonzero for a bailout): the table
    /// takes `value` and references the key itself; the value it replaces is
    /// released, which can run `__destruct`.
    safe fn swr_zend_try_hash_update(
        table: Insertions<'_>,
        key: Option<&ZStr>,
        index: zend_ulong,
        value: Given<'_>,
    ) -> std::ffi::c_int;
    /// `zend_hash_index_del` inside `zend_try` (nonzero for a bailout): the
    /// value under `index`, if any, is released, which can run `__destruct`.
    safe fn swr_zend_try_hash_index_del(
        table: Insertions<'_>,
        index: zend_ulong,
    ) -> std::ffi::c_int;
    /// `add_next_index_stringl`: the array takes a new string of the lent bytes
    /// (an `IoSlice` is an iovec) under its next index. No PHP code runs.
    safe fn swr_lent_push_bytes(array: Separated<'_>, bytes: IoSlice<'_>);
    /// `add_assoc_zval_ex` inside `zend_try` (nonzero for a bailout): the array
    /// takes `value` under the lent key, which Zend copies; the value it
    /// replaces is released, which can run `__destruct`.
    safe fn swr_lent_symtable_update(
        array: Separated<'_>,
        key: IoSlice<'_>,
        value: Given<'_>,
    ) -> std::ffi::c_int;
    /// Swoole's `zend::array_add_or_merge`: the array takes `value` under the
    /// lent key, which Zend copies, or appends it to what is there, made a list
    /// first unless it is one. No PHP code runs: it releases nothing.
    safe fn swr_lent_array_add_or_merge(array: Separated<'_>, key: IoSlice<'_>, value: Given<'_>);
    /// `php_register_variable_ex`: the array takes `value` under the request
    /// variable `name`, by PHP's rules for its `a[b]` nesting, or releases it
    /// (an empty name, or one nested past `max_input_nesting_level`, which
    /// warns: a user error handler runs PHP code).
    safe fn php_register_variable_ex(
        name: crate::helpers::CText<'_>,
        value: Given<'_>,
        array: Separated<'_>,
    );
    /// `zend_zval_value_name`: it reads the zval, and returns a static type
    /// name or the name of the object's class, which outlives its objects.
    safe fn zend_zval_value_name(value: &Value) -> crate::helpers::CText<'_>;
    /// `zend_zval_type_name`: it reads the zval, and returns a static type
    /// name or, before PHP 8.3, the name of the object's class.
    safe fn zend_zval_type_name(value: &Value) -> crate::helpers::CText<'_>;
    /// `ZEND_HASH_FOREACH`'s step: null (`None`) past the end, or the next
    /// defined element at or after `position` (any position: the walk stops at
    /// the table's last slot), which then points past it, with its key (null,
    /// `None`, for an integer key: a string the table keeps while borrowed)
    /// and index written. Past the end it writes neither.
    safe fn swr_zend_hash_next<'a>(
        table: &'a Array,
        position: &mut HashPosition,
        key: &mut Option<&'a ZStr>,
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
    /// `zend_read_property(scope, object, name, silent, rv)` of a live object
    /// under a class or none, inside `zend_try` (nonzero for a bailout): `rv`
    /// then holds what it read (the property, with a reference of its own, or
    /// a computed value), in place of what it held, which is not released.
    /// Zend reads the lent name (an `IoSlice` is an iovec).
    safe fn swr_lent_read_property(
        scope: Option<crate::ClassEntry<'_>>,
        object: crate::ObjectRef<'_>,
        name: IoSlice<'_>,
        silent: bool,
        rv: &Owned,
    ) -> std::ffi::c_int;
    /// `zend_call_method(object, scope, NULL, name, result, arg1, arg2)`
    /// inside `zend_try` (nonzero for a bailout), with the arguments given,
    /// which Zend copies into the call; nothing is called when the object is no
    /// `scope`. `result` then holds what it returned, in place of what it held,
    /// which is not released.
    safe fn swr_lent_call_method(
        object: crate::ObjectRef<'_>,
        scope: Option<crate::ClassEntry<'_>>,
        name: IoSlice<'_>,
        result: &Owned,
        arg1: Option<&Value>,
        arg2: Option<&Value>,
    ) -> std::ffi::c_int;
    /// `php_format_date(format, timestamp, local)` of the lent format: a new
    /// string; no PHP code runs.
    safe fn php_format_date<'a>(
        format: In<'a, u8>,
        len: Count<'a>,
        timestamp: zend_long,
        local: bool,
    ) -> *mut zend_string;
    /// `php_url_encode`: it reads the lent bytes and
    /// returns a new string; no PHP code runs.
    safe fn php_url_encode<'a>(bytes: In<'a, u8>, len: Count<'a>) -> *mut zend_string;
    /// `php_url_decode` of a new string's copy of the lent bytes: them as
    /// `urldecode()` decodes them.
    safe fn swr_bytes_url_decode(bytes: IoSlice<'_>) -> *mut zend_string;
    /// `json_decode(text, true)` of the lent bytes (an `IoSlice` is an iovec)
    /// into `out`, in place of what it held, which is not released; false when
    /// they are not JSON. Objects decode as arrays, so no PHP code runs.
    safe fn swr_bytes_json_decode(text: IoSlice<'_>, out: &Owned);
    /// `zval_get_long` inside `zend_try` (nonzero for a bailout): it reads the
    /// value (an object warns, which runs the error handler) and writes `out`.
    safe fn swr_zend_try_get_long(value: &Value, out: &mut zend_long) -> std::ffi::c_int;
    /// `zval_get_double`, as [`swr_zend_try_get_long`].
    safe fn swr_zend_try_get_double(value: &Value, out: &mut f64) -> std::ffi::c_int;
    /// `zval_get_string`, as [`swr_zend_try_get_long`]: `out` takes a new
    /// reference, even after a bailout.
    safe fn swr_zend_try_get_string(value: &Value, out: &mut *mut zend_string) -> std::ffi::c_int;
    /// `zval_is_true`: it reads the value (an object's cast handler may run).
    safe fn swr_zval_is_true(value: &Value) -> bool;
    /// `http_build_query($value)`: null or a new string.
    safe fn swr_zend_http_build_query(value: &Value) -> *mut zend_string;
    /// `serialize($value)` inside `zend_try` (`bailed` set for a bailout): null
    /// or a new string.
    safe fn swr_zend_serialize(value: &Value, bailed: &mut bool) -> *mut zend_string;
    /// `json_encode($value)` inside `zend_try` (nonzero for a bailout): `out`
    /// takes null or a new string.
    safe fn swr_zend_try_json_encode(value: &Value, out: &mut *mut zend_string) -> std::ffi::c_int;
    /// `zend_trace_to_string(trace, false)`: it reads the table and returns a
    /// new string.
    safe fn zend_trace_to_string(trace: &Array, include_main: bool) -> *mut zend_string;
    /// `SEPARATE_ARRAY` of a zval holding an array (the shim leaves anything
    /// else alone): a shared or immutable array is duplicated, and the zval's
    /// reference moves to the copy. The exclusive borrow means nothing holds a
    /// borrow of the table it may give up.
    safe fn swr_zend_separate_array(array: &mut ArrayMut);
    /// `SEPARATE_ARRAY(array)`, then `zend_hash_clean` inside `zend_try`
    /// (nonzero for a bailout); nothing unless the zval holds an array. Its
    /// destructors run PHP code, which can't reach the borrowed zval.
    safe fn swr_zend_try_hash_clean(array: &mut ArrayMut) -> std::ffi::c_int;
    /// `SEPARATE_ARRAY(dest)`, then `php_array_merge(dest, source)` inside
    /// `zend_try` (nonzero for a bailout); nothing unless both hold arrays. A
    /// replaced value's `__destruct` runs PHP code.
    safe fn swr_zend_try_array_merge(dest: &mut ArrayMut, source: &Value) -> std::ffi::c_int;
    /// `zend_string_release`: the string loses the reference the `ZString`
    /// held, and is freed with its last.
    safe fn swr_zend_string_release(string: ZString);
    /// `zval_ptr_dtor` of a string [`Owned`]'s drop gives up, which runs no
    /// PHP code.
    #[link_name = "zval_ptr_dtor"]
    safe fn release_string(value: Dying<'_>);
    /// `zval_ptr_dtor` inside `zend_try` (nonzero for a bailout) of anything
    /// else [`Owned`]'s drop gives up.
    safe fn swr_zend_try_ptr_dtor(value: Dying<'_>) -> std::ffi::c_int;
}

/// The zval of an [`Owned`] being dropped, made only by its drop: Zend destroys
/// it, once, and nothing reads it again.
#[repr(transparent)]
struct Dying<'a>(&'a Value);

/// The zval an [`Owned`] gave up ([`Owned::into_zval`]) to a Zend function
/// that takes its reference over (or, where it says so, leaves it to the
/// caller, who releases it): made only from that zval, for that one call, so
/// nothing releases the reference twice.
#[repr(transparent)]
struct Given<'a>(&'a mut zval);

/// New-key writes to one separated table. It exposes no values or copies of
/// the array, so the exclusive borrow lasts across consecutive insertions.
#[repr(transparent)]
pub struct Insertions<'a>(&'a Array);

/// A call's arguments lent to C (`swr_zvals`): `argc` zvals from `argv`, which
/// the callee reads and copies, keeping no pointer to them. Made only from a
/// borrowed slice, whose length bounds `argc`. Raw bindgen bits are not values:
///
/// ```compile_fail
/// use swoole_rs_zend::{Zvals, UNDEF};
/// let _args = Zvals::of(&mut [UNDEF]);
/// ```
///
/// Temporary argument slots cannot escape the call:
///
/// ```compile_fail
/// use swoole_rs_zend::{Owned, Zvals};
/// let value = Owned::long(1);
/// let escaped = Zvals::with([&value], |args| args);
/// ```
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Zvals<'a> {
    argv: *mut zval,
    argc: u32,
    lent: std::marker::PhantomData<&'a [zval]>,
}

impl<'a> Zvals<'a> {
    /// Zvals this code holds, as [`Value::of`] borrows one.
    #[must_use]
    #[inline]
    pub(crate) fn of(args: &'a mut [zval]) -> Self {
        Self::lend(args.as_mut_ptr(), args.len())
    }

    /// Lend stack copies of validated values for one call. The originals stay
    /// borrowed until the call returns; the temporary argument slots cannot escape.
    #[inline]
    pub fn with<const N: usize, R>(args: [&Value; N], call: impl FnOnce(Zvals<'_>) -> R) -> R {
        let mut slots = args.map(Value::copy_value);
        call(Zvals::of(&mut slots))
    }

    /// Keep at most the first `count` arguments.
    #[must_use]
    #[inline]
    pub fn take(mut self, count: usize) -> Self {
        self.argc = self.argc.min(u32::try_from(count).unwrap_or(u32::MAX));
        self
    }

    /// Values this code owns (`Owned` is a transparent zval), which the callee
    /// only reads.
    #[must_use]
    #[inline]
    pub fn owned(args: &'a [Owned]) -> Self {
        Self::lend(args.as_ptr().cast_mut().cast(), args.len())
    }

    /// One value, which the callee only reads.
    #[must_use]
    #[inline]
    pub fn one(value: &'a Value) -> Self {
        Self::lend(value.as_ptr(), 1)
    }

    pub(crate) fn argv(&self) -> *mut zval {
        self.argv
    }

    pub(crate) fn argc(&self) -> u32 {
        self.argc
    }

    /// The values, borrowed for `'a`.
    pub(crate) fn values(self) -> &'a [Value] {
        if self.argc == 0 {
            return &[];
        }
        // SAFETY: each maker lends `argc` live zvals from `argv` for `'a` (a
        // borrowed slice, or what a frame or a kept callable's record holds);
        // `Value` is a transparent zval, whose cell allows the writes Zend makes.
        unsafe { std::slice::from_raw_parts(self.argv.cast::<Value>(), self.argc as usize) }
    }

    /// `argc` zvals from `argv`, which a frame or a callable's record holds for `'a`.
    pub(crate) fn counted(argv: *mut zval, argc: u32) -> Self {
        Self {
            argv,
            argc,
            lent: std::marker::PhantomData,
        }
    }

    /// `len` zvals from `argv`, or the first `u32::MAX` of more.
    fn lend(argv: *mut zval, len: usize) -> Self {
        Self {
            argv,
            argc: u32::try_from(len).unwrap_or(u32::MAX),
            lent: std::marker::PhantomData,
        }
    }
}

/// A borrowed zval. Zend may change it and what it points to (refcounts, a
/// conversion's cache) through pointers on this thread, hence the cell.
/// Arbitrary bindgen records cannot be turned into values through a safe API:
///
/// ```compile_fail
/// use swoole_rs_zend::{Value, UNDEF};
/// let mut bits = UNDEF;
/// let _value = Value::of(&mut bits);
/// ```
///
/// Contents cannot be lent out or edited through a shared Value. Mutate an
/// owned array through [`Owned::array_mut`], or a property/argument through its
/// owning handle, which protects any earlier reads.
///
/// ```compile_fail
/// use swoole_rs_zend::{Owned, ZString};
/// let mut value = Owned::from(ZString::new(b"borrowed"));
/// let text = value.str().unwrap();
/// value.lend(|held| *held = Owned::null());
/// let _ = text.bytes();
/// ```
///
/// ```compile_fail
/// use swoole_rs_zend::OwnedArray;
/// let mut value = OwnedArray::default().into_owned();
/// value.array_mut().unwrap().push_long(1);
/// let first = value.array().unwrap().index(0).unwrap();
/// value.edit_array(|array| for i in 0..64 { array.push_long(i); });
/// let _ = first.long();
/// ```
#[repr(transparent)]
pub struct Value(Cell<zval>);

/// A zval's value word, type and `u2`, without the unions bindgen gives
/// them: what [`Value::word`], [`Value::kind`] and [`Value::u2`] read.
#[repr(C)]
struct Tagged {
    word: *mut c_void,
    type_info: u32,
    u2: u32,
}

const _: () = assert!(
    std::mem::offset_of!(Tagged, word) == std::mem::offset_of!(zval, value)
        && std::mem::offset_of!(Tagged, type_info) == std::mem::offset_of!(zval, u1)
        && std::mem::offset_of!(Tagged, u2) == std::mem::offset_of!(zval, u2)
        && size_of::<Tagged>() <= size_of::<zval>()
        && align_of::<Tagged>() <= align_of::<zval>(),
    "Tagged is a zval's value and type_info"
);

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

    /// The value a caller made by value (such as [`crate::ObjectRef::copy_value`]'s),
    /// borrowed.
    #[must_use]
    #[inline]
    pub(crate) fn of(value: &mut Self) -> &Self {
        value
    }

    /// `ZVAL_OBJ`: a value naming `object` without a reference of its own, which
    /// must not outlive the object's borrow ([`crate::ObjectRef::copy_value`]).
    #[must_use]
    #[inline]
    pub(crate) fn naming(object: crate::ObjectRef<'_>) -> Self {
        Self(Cell::new(zval {
            value: zend_value {
                obj: object.as_ptr(),
            },
            u1: _zval_struct__bindgen_ty_1 {
                type_info: crate::sys::IS_OBJECT_EX,
            },
            u2: _zval_struct__bindgen_ty_2 { next: 0 },
        }))
    }

    #[must_use]
    #[inline]
    pub fn as_ptr(&self) -> *mut zval {
        self.0.as_ptr()
    }

    /// The zval's value word and type, copied out.
    #[inline]
    fn tagged(&self) -> Tagged {
        // SAFETY: a live zval, every one of which sets `u1.type_info` and
        // which its value union leads, read through `Tagged`'s plain fields at
        // their offsets; any eight bytes are a raw pointer.
        unsafe { self.as_ptr().cast::<Tagged>().read() }
    }

    /// `Z_TYPE_P`.
    #[must_use]
    #[inline]
    pub fn kind(&self) -> u32 {
        self.tagged().type_info & 0xff
    }

    /// `Z_NEXT_P`'s union (`u2`): a frame's `This` keeps its argument count
    /// there.
    #[inline]
    pub(crate) fn u2(&self) -> u32 {
        self.tagged().u2
    }

    /// The value union's first word: what `Z_STR_P`, `Z_ARRVAL_P`, `Z_OBJ_P`,
    /// `Z_REF_P` and `Z_PTR_P` read as a pointer, and `Z_LVAL_P` and
    /// `Z_DVAL_P` as bits. Only [`Value::kind`] says what it holds.
    #[inline]
    fn word(&self) -> *mut c_void {
        self.tagged().word
    }

    /// [`Value::word`] as a pointer to `T`.
    #[inline]
    fn payload<T>(&self) -> *mut T {
        self.word().cast()
    }

    /// `Z_COUNTED_P`'s header, when `Z_REFCOUNTED_P`: the zval holds a counted
    /// value (never an interned string or an immutable array), as long as it is
    /// borrowed.
    #[inline]
    fn counted(&self) -> Option<&Counted> {
        self.held()
    }

    /// [`Value::word`] as bits.
    #[inline]
    fn bits(&self) -> u64 {
        self.word().addr() as u64
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
        // Copied before any PHP code can assign the reference.
        Cow::Owned(self.referred().copy())
    }

    /// `ZVAL_DEREF`, borrowed: the slot of what a reference refers to, which
    /// the reference this zval holds keeps alive while it is borrowed; else
    /// this value. PHP code that assigns the reference rewrites the slot.
    #[must_use]
    #[inline]
    pub fn referred(&self) -> &Self {
        self.held::<Self>().unwrap_or(self)
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

    /// `Z_INDIRECT_P`: the slot an indirect zval points to. A pointer, since
    /// the table holding that slot may move it under any borrow.
    #[must_use]
    #[inline]
    pub fn indirect(&self) -> Option<*mut zval> {
        // An indirect zval holds `zv`.
        (self.kind() == IS_INDIRECT).then(|| self.payload::<zval>())
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
        self.held()
    }

    /// The counted value a zval of a type `T` accepts holds, borrowed: what
    /// `Z_COUNTED_P`, `Z_STR_P` and `Z_ARRVAL_P` read.
    #[inline]
    fn held<T: Held>(&self) -> Option<&T> {
        let Tagged {
            word, type_info, ..
        } = self.tagged();
        if !T::held_by(type_info) {
            return None;
        }
        // SAFETY: a zval of a type `T` accepts holds a pointer (never null)
        // to a live counted value that holds `T`'s fields at their offsets from
        // `T::AT` (`Held`), which it keeps while borrowed; they are `Cell`s or
        // in an `UnsafeCell`, which allow the writes Zend makes meanwhile.
        Some(unsafe { &*word.wrapping_byte_add(T::AT).cast::<T>() })
    }

    /// `Z_OBJ_P` of an object.
    #[must_use]
    #[inline]
    pub fn object(&self) -> Option<*mut zend_object> {
        // An object zval holds `obj`.
        (self.kind() == IS_OBJECT).then(|| self.payload())
    }

    /// [`Value::object`], borrowed: the zval holds the object while borrowed.
    #[must_use]
    #[inline]
    pub fn object_ref(&self) -> Option<crate::ObjectRef<'_>> {
        Some(crate::ObjectRef(self.object()?, std::marker::PhantomData))
    }

    /// `Z_OBJ_P` of an object with `class`'s handlers: one that `class`'s
    /// create handler made, or a subclass's that inherited it.
    #[must_use]
    #[inline]
    pub fn object_of(&self, class: &crate::Class) -> Option<*mut zend_object> {
        let object = self.object_ref()?;
        (object.handlers() == class.handlers()).then_some(object.as_ptr())
    }

    /// The [`Custom`](crate::Custom) data of a `T` object.
    #[must_use]
    #[inline]
    pub fn custom<T: crate::Custom>(&self) -> Option<&T> {
        self.object_ref()?.custom()
    }

    /// `Z_PTR_P` of a pointer (`IS_PTR`), as function and class tables hold.
    #[must_use]
    #[inline]
    pub fn pointer(&self) -> Option<*mut c_void> {
        // A pointer zval holds `ptr`.
        (self.kind() == IS_PTR).then(|| self.payload())
    }

    /// Hold `value` and its reference in place of what this held, which is
    /// not released: for a slot holding nothing that needs it (a return slot,
    /// a fresh object's default), else a leak.
    #[inline]
    pub(crate) fn put(&self, value: Owned) {
        self.0.set(value.into_zval());
    }

    /// `Z_TYPE_INFO_P(value) = IS_NULL`: null, its value word and `u2` left as
    /// they were and what it held not released, for a slot holding nothing
    /// that needs it. One store of the type word.
    #[inline]
    pub(crate) fn retype_null(&self) {
        let mut value = self.0.get();
        value.u1.type_info = IS_NULL;
        self.0.set(value);
    }

    /// `ZVAL_UNDEF` then `Z_FE_ITER_P(value) = -1`: a failed `FE_RESET_*`'s
    /// result, with no iterator. What the slot held is not released: for a
    /// result slot holding nothing that needs it, else a leak.
    #[inline]
    pub fn undef_iteration(&self) {
        let mut value = UNDEF;
        value.u2.fe_iter_idx = u32::MAX;
        self.0.set(value);
    }

    // Only slot owners use this: public property and return-slot reads hold
    // copies, and argument edits require an exclusive Call borrow.
    #[inline(always)]
    pub(crate) fn lend_slot<R>(&self, edit: impl FnOnce(&mut Owned) -> R) -> R {
        let mut held = Owned::from_zval(self.0.replace(Owned::null().into_zval()));
        let result = edit(&mut held);
        // Replace the slot once: any value installed by a reentrant edit is
        // released after the held value is back in its slot.
        drop(Owned::from_zval(self.0.replace(held.into_zval())));
        result
    }

    #[inline(always)]
    pub(crate) fn edit_array_slot<R>(&self, edit: impl FnOnce(&mut ArrayMut) -> R) -> Option<R> {
        self.lend_slot(|held| held.array_mut().map(|array| edit(array)))
    }

    /// Property writes on the object this holds, under `scope` (Zend's view of
    /// its properties; `None` for none), while it is borrowed.
    #[must_use]
    #[inline]
    pub fn props(&self, scope: Option<&crate::Class>) -> Option<crate::Props<'_>> {
        Some(crate::Props::of(scope, self.object_ref()?))
    }

    /// An owned snapshot of the declared instance property `name`, unaffected
    /// by later writes through this object or another alias. An uninitialised
    /// typed property reads as null; None for no object or no such property.
    #[must_use]
    #[inline]
    pub fn property(&self, name: &CStr) -> Option<Owned> {
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
        let Some(object) = self.object_ref() else {
            return Ok(None);
        };
        let rv = Owned::default();
        bailout::trapped(swr_lent_read_property(
            scope,
            object,
            IoSlice::new(name),
            silent,
            &rv,
        ))?;
        Ok(Some(rv))
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
        let Some(object) = self.object_ref() else {
            return Ok(result);
        };
        if scope.is_some_and(|scope| !self.instance_of(scope)) {
            return Ok(result);
        }
        if bailout::pending() {
            // Nothing is called, as upstream never ran past a bailout.
            return Err(bailout::Bailout);
        }
        let scope = scope.and_then(crate::Class::class_entry);
        let name = IoSlice::new(method);
        let (arg1, arg2) = (args.first().copied(), args.get(1).copied());
        let called = bailout::trapped(swr_lent_call_method(
            object, scope, name, &result, arg1, arg2,
        ));
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
        // Nothing is called while a bailout is pending: upstream never ran past it.
        bailout::outcome(())?;
        let callable = self.copy();
        let result = Owned::default();
        let called = crate::host::user_call(&callable, Zvals::owned(&[]), &result);
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
        bailout::defer(bailout::trapped(swr_zend_try_get_long(self, &mut out)));
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
        bailout::defer(bailout::trapped(swr_zend_try_get_double(self, &mut out)));
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
        swr_zval_is_true(self)
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
        bailout::defer(bailout::trapped(swr_zend_try_get_string(self, &mut out)));
        ZString(out)
    }

    /// `http_build_query($value)` (RFC 1738): None for a value that is no array or
    /// object, or one with nothing to encode.
    #[must_use]
    pub fn http_build_query(&self) -> Option<ZString> {
        let query = swr_zend_http_build_query(self);
        (!query.is_null()).then(|| ZString(query))
    }

    /// `serialize($value)`: None when it produced nothing.
    /// # Errors
    /// The bailout a `__serialize` raised, trapped.
    pub fn serialize(&self) -> Result<Option<ZString>, bailout::Bailout> {
        let mut bailed = false;
        let serialized = swr_zend_serialize(self, &mut bailed);
        bailout::trapped(bailed.into())?;
        Ok((!serialized.is_null()).then(|| ZString(serialized)))
    }

    /// `json_encode($value)`: None when the encoder failed.
    /// # Errors
    /// The bailout a `jsonSerialize` raised, trapped.
    pub fn json_encode(&self) -> Result<Option<ZString>, bailout::Bailout> {
        let mut encoded = ptr::null_mut();
        bailout::trapped(swr_zend_try_json_encode(self, &mut encoded))?;
        Ok((!encoded.is_null()).then(|| ZString(encoded)))
    }

    /// `Z_STRVAL_P`/`Z_STRLEN_P` of a string.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> Option<&[u8]> {
        self.str().map(ZStr::bytes)
    }

    /// `Z_OBJPROP_P` of an object: its property table, which Zend builds if
    /// need be, held as [`Value::hold`] holds an array, so a property write
    /// meanwhile separates the object's table instead of changing this one.
    /// None for no object or no table, or after a bailout, which is deferred.
    #[must_use]
    pub fn properties(&self) -> Option<HeldArray> {
        let object = self.object_ref()?;
        let table = Owned::default();
        bailout::trapped(swr_zend_try_object_properties(object, &table))
            .map_err(bailout::Bailout::defer)
            .ok()?;
        table.array()?;
        Some(HeldArray(table))
    }

    /// `Z_TRY_ADDREF_P`: one more reference to what a counted zval holds, which
    /// whoever the caller hands it to releases (nobody: a leak).
    #[inline]
    pub fn add_ref(&self) {
        if let Some(counted) = self.counted() {
            counted[REFCOUNT].set(counted[REFCOUNT].get() + 1);
        }
    }

    /// `zend_zval_value_name`: the type, or the object's class, a type error names.
    #[must_use]
    #[inline]
    pub fn value_name(&self) -> &CStr {
        if crate::sys::PHP_VERSION_ID < 80300 {
            return self.type_name();
        }
        if self.referred().kind() > crate::sys::IS_RESOURCE {
            return c"mixed";
        }
        zend_zval_value_name(self).get()
    }

    /// `zend_zval_type_name`: the type an error names (an object's is `object`
    /// from PHP 8.3, its class before).
    #[must_use]
    #[inline]
    pub fn type_name(&self) -> &CStr {
        if self.referred().kind() > crate::sys::IS_RESOURCE {
            return c"mixed";
        }
        zend_zval_type_name(self).get()
    }

    /// `Z_ARRVAL_P` of an array.
    #[must_use]
    #[inline]
    pub fn array(&self) -> Option<&Array> {
        self.held()
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
        self.0.get()
    }

    /// [`Value::copy`] unless the zval holds an object whose free has begun
    /// (`IS_OBJ_FREE_CALLED`): a new reference to it would dangle.
    #[inline]
    pub(crate) fn copy_unfreed(&self) -> Option<Owned> {
        let freeing = self.object().is_some()
            && self.counted().is_some_and(|counted| {
                counted[TYPE_INFO].get() & crate::sys::IS_OBJ_FREE_CALLED != 0
            });
        (!freeing).then(|| self.copy())
    }

    /// `ZVAL_COPY`: the same value with a reference of its own.
    #[must_use]
    #[inline]
    pub fn copy(&self) -> Owned {
        // The copy carries the reference it adds below.
        let owned = Owned(Self(Cell::new(self.copy_value())));
        owned.add_ref();
        owned
    }
}

/// A view of the counted value a zval points to, for [`Value::held`]: the
/// fields, at their offsets from `AT` bytes in, of whatever a zval whose
/// `type_info` passes `held_by` points to. Only the four below.
trait Held {
    /// Where the view starts in the counted value.
    const AT: usize = 0;
    fn held_by(type_info: u32) -> bool;
}

/// `Z_REFCOUNTED_P`: the type flags, the byte above the type, say counted.
impl Held for Counted {
    #[inline]
    fn held_by(type_info: u32) -> bool {
        (type_info >> 8) & crate::sys::IS_TYPE_REFCOUNTED != 0
    }
}

impl Held for ZStr {
    #[inline]
    fn held_by(type_info: u32) -> bool {
        type_info & 0xff == IS_STRING
    }
}

impl Held for Array {
    #[inline]
    fn held_by(type_info: u32) -> bool {
        type_info & 0xff == IS_ARRAY
    }
}

/// A reference's slot: what the reference refers to, which PHP code assigning
/// the reference rewrites (a `Value`, so a cell).
impl Held for Value {
    const AT: usize = std::mem::offset_of!(zend_reference, val);

    #[inline]
    fn held_by(type_info: u32) -> bool {
        type_info & 0xff == IS_REFERENCE
    }
}

/// A borrowed hash table (`zend_array`), word by word, each a `Cell`, which
/// allows the writes Zend makes through pointers on this thread. Its first
/// two words are its counted header ([`REFCOUNT`], [`TYPE_INFO`]); this crate
/// reads and writes no others but its flags and element count.
#[repr(transparent)]
pub struct Array([Cell<u32>; size_of::<HashTable>() / 4]);

const _: () = assert!(
    size_of::<HashTable>().is_multiple_of(4)
        && align_of::<HashTable>() >= align_of::<Array>()
        && std::mem::offset_of!(HashTable, gc) == 0,
    "Array is a HashTable, its counted header first"
);

/// `HT_FLAGS(ht)`'s word in an [`Array`].
const FLAGS: usize = std::mem::offset_of!(HashTable, u) / 4;
/// `zend_hash_num_elements`'s word in an [`Array`].
const ELEMENTS: usize = std::mem::offset_of!(HashTable, nNumOfElements) / 4;

const _: () = assert!(
    std::mem::offset_of!(HashTable, u).is_multiple_of(4)
        && std::mem::offset_of!(HashTable, nNumOfElements).is_multiple_of(4),
    "an Array's flags and element count are words of it"
);

/// The `zend_refcounted_h` a counted value begins with: its refcount
/// ([`REFCOUNT`]), then `type_info` ([`TYPE_INFO`]), which Zend reads and
/// writes through pointers on this thread.
type Counted = [Cell<u32>; 2];
const REFCOUNT: usize = 0;
const TYPE_INFO: usize = 1;

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
        ptr::from_ref(self).cast_mut().cast()
    }

    /// `zend_array_count`.
    #[must_use]
    #[inline]
    pub fn count(&self) -> u32 {
        zend_array_count(self)
    }

    /// `HT_FLAGS(ht)`, which Zend rewrites in place as it changes the table.
    #[must_use]
    #[inline]
    pub fn flags(&self) -> &Cell<u32> {
        &self.0[FLAGS]
    }

    /// `zend_hash_num_elements`.
    #[must_use]
    #[inline]
    pub fn elements(&self) -> u32 {
        self.0[ELEMENTS].get()
    }

    /// `zend_trace_to_string(trace, false)`: a backtrace array as an exception prints it.
    #[must_use]
    pub fn trace_to_string(&self) -> ZString {
        ZString(zend_trace_to_string(self, false))
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
        lend_in(key, |bytes, len| zend_hash_str_find(self, bytes, len))
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
            // The element, or what the reference it holds refers to, which the
            // element keeps alive while the table is borrowed.
            each(key, value.referred());
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
        let header = &array.0;
        let refcount = header[REFCOUNT].get();
        let immutable = header[TYPE_INFO].get() & crate::sys::IS_ARRAY_IMMUTABLE != 0;
        if !immutable && refcount == 1 {
            // The last reference: `Owned`'s drop destroys the table inside `zend_try`.
            return;
        }
        if !immutable {
            // Another holder keeps the table live; this gives up its reference.
            header[REFCOUNT].set(refcount - 1);
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
        let (mut key, mut index) = (None, 0);
        let value = swr_zend_hash_next(self.table, &mut self.position, &mut key, &mut index)?;
        Some((key, index, value))
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

/// A borrowed `zend_string`: the counted header, the cached hash and the
/// length, which Zend reads and writes through pointers on this thread, then
/// the bytes and the NUL after them, which run on past the struct.
#[repr(C)]
pub struct ZStr {
    counted: Counted,
    hash: Cell<zend_ulong>,
    len: Cell<usize>,
    val: UnsafeCell<[u8; size_of::<zend_string>() - std::mem::offset_of!(zend_string, val)]>,
}

const _: () = assert!(
    std::mem::offset_of!(ZStr, hash) == std::mem::offset_of!(zend_string, h)
        && std::mem::offset_of!(ZStr, len) == std::mem::offset_of!(zend_string, len)
        && std::mem::offset_of!(ZStr, val) == std::mem::offset_of!(zend_string, val)
        && size_of::<ZStr>() == size_of::<zend_string>()
        && align_of::<ZStr>() == align_of::<zend_string>(),
    "ZStr is a zend_string"
);

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
        ptr::from_ref(self).cast_mut().cast()
    }

    /// `ZSTR_VAL`/`ZSTR_LEN`: the bytes, without the NUL that follows them.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        self.val::<false>()
    }

    /// The bytes and the NUL Zend keeps after them; a NUL among the bytes stays.
    #[must_use]
    #[inline]
    pub fn bytes_with_nul(&self) -> &[u8] {
        self.val::<true>()
    }

    /// `ZSTR_VAL` for `ZSTR_LEN` bytes, and the NUL after them when `NUL`.
    #[inline]
    fn val<const NUL: bool>(&self) -> &[u8] {
        let length = self.len.get() + usize::from(NUL);
        // SAFETY: a live zend_string holds `len` bytes at `val` and a NUL after
        // them, which it keeps while borrowed.
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
            // A live, counted string; the new reference is the `ZString`'s.
            let refcount = &self.counted[REFCOUNT];
            refcount.set(refcount.get() + 1);
        }
        ZString(self.as_ptr())
    }

    /// `ZSTR_IS_INTERNED`: interned strings are never counted or freed.
    #[must_use]
    #[inline]
    pub fn interned(&self) -> bool {
        self.counted[TYPE_INFO].get() & IS_STR_INTERNED != 0
    }

    /// `zendi_smart_strcmp`: PHP's `==` order of two strings, numeric when
    /// both are numeric strings.
    #[must_use]
    #[inline]
    pub fn smart_cmp(&self, other: &Self) -> core::cmp::Ordering {
        swr_zend_smart_strcmp(self, other).cmp(&0)
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
/// Built in this crate only, from a live string's reference it gives up.
#[repr(transparent)]
pub struct ZString(pub(crate) *mut zend_string);

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
        Self(lend_in(bytes, |data, len| php_url_encode(data, len)))
    }

    /// `php_url_decode` of a copy of `bytes`: them as `urldecode()` decodes them.
    #[must_use]
    pub fn url_decode(bytes: &[u8]) -> Self {
        Self(swr_bytes_url_decode(IoSlice::new(bytes)))
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
        Self(lend_in(format.to_bytes(), |bytes, len| {
            php_format_date(bytes, len, timestamp, local)
        }))
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

/// A zval a record keeps across calls at its own address, which Rust never
/// releases: the record hands the reference [`Kept::put`] gave it to Zend
/// (`zval_ptr_dtor` through [`Kept::as_ptr`]), or another holder keeps what
/// [`Kept::keep`] copied. All zeroes (`IS_UNDEF`) is an empty one.
#[repr(transparent)]
pub struct Kept(std::mem::ManuallyDrop<Owned>);

impl Kept {
    /// The zval; None while empty.
    #[must_use]
    #[inline]
    pub fn get(&self) -> Option<&Value> {
        (self.0.kind() != IS_UNDEF).then_some(&self.0)
    }

    /// The held slot, including its undefined value while empty.
    #[must_use]
    #[inline]
    pub fn value(&self) -> &Value {
        &self.0
    }

    /// The array it holds, to add to; None for anything else.
    #[inline]
    pub fn array_mut(&mut self) -> Option<&mut ArrayMut> {
        self.0.array_mut().map(|array| &mut **array)
    }

    /// Hold `value` and its reference, in place of what it held, which is
    /// not released.
    #[inline]
    pub fn put(&mut self, value: Owned) {
        self.0 = std::mem::ManuallyDrop::new(value);
    }

    /// `ZVAL_COPY_VALUE`: hold `value`'s bits without a reference of their
    /// own, in place of what it held, which is not released.
    /// # Safety
    /// What `value` holds lives until the copy is replaced or cleared.
    #[inline]
    pub unsafe fn keep(&mut self, value: &Value) {
        self.put(Owned(Value(Cell::new(value.copy_value()))));
    }

    /// Empty it, releasing nothing.
    #[inline]
    pub fn clear(&mut self) {
        self.put(Owned::default());
    }

    /// The zval, for Zend to read or release; null while empty. Once Zend
    /// releases it, clear it before it is read again, unless another holder
    /// keeps what it holds.
    #[must_use]
    #[inline]
    pub fn as_ptr(&self) -> *mut zval {
        self.get().map_or(ptr::null_mut(), Value::as_ptr)
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
        // The reference this holds, handed over as it goes.
        swr_zend_string_release(Self(self.0));
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
        Self::empty()
    }
}

impl Owned {
    /// `ZVAL_UNDEF`, for a constant (all-zero bytes, as a zeroed record holds).
    #[must_use]
    pub const fn empty() -> Self {
        Self(Value(Cell::new(UNDEF)))
    }

    /// `php_json_decode(text)` with objects as arrays; false when `text` is
    /// not JSON. It runs no PHP code.
    #[must_use]
    pub fn json_decode(text: &[u8]) -> Self {
        let decoded = Self::default();
        swr_bytes_json_decode(IoSlice::new(text), &decoded);
        decoded
    }

    /// `ZVAL_LONG`.
    #[must_use]
    #[inline]
    pub fn long(value: zend_long) -> Self {
        Self(Value(Cell::new(crate::long_zval(value))))
    }

    /// `ZVAL_DOUBLE`.
    #[must_use]
    #[inline]
    pub fn double(value: f64) -> Self {
        let mut zval = UNDEF;
        zval.value = zend_value { dval: value };
        zval.u1.type_info = IS_DOUBLE;
        Self(Value(Cell::new(zval)))
    }

    /// `ZVAL_BOOL`.
    #[must_use]
    #[inline]
    pub fn bool(value: bool) -> Self {
        let mut zval = UNDEF;
        zval.u1.type_info = if value { IS_TRUE } else { IS_FALSE };
        Self(Value(Cell::new(zval)))
    }

    /// `ZVAL_EMPTY_ARRAY`: the immutable empty array, which is not counted.
    #[must_use]
    #[inline]
    pub fn empty_array() -> Self {
        let mut zval = UNDEF;
        zval.value = zend_value {
            arr: crate::value::empty_array(),
        };
        zval.u1.type_info = IS_ARRAY;
        Self(Value(Cell::new(zval)))
    }

    /// `ZVAL_NULL`.
    #[must_use]
    #[inline]
    pub fn null() -> Self {
        let mut value = UNDEF;
        value.u1.type_info = IS_NULL;
        Self(Value(Cell::new(value)))
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
        Self(Value(Cell::new(zval)))
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

    /// Keep this array as a read-only hold without taking another reference.
    #[must_use]
    #[inline]
    pub fn into_held_array(self) -> Option<HeldArray> {
        self.array()?;
        Some(HeldArray(self))
    }

    /// `ZVAL_OBJ(value, std)`: the result takes over the new object's reference.
    #[must_use]
    #[inline]
    pub(crate) fn object(std: crate::object::Fresh) -> Self {
        let mut zval = UNDEF;
        zval.value = zend_value {
            obj: std.into_ptr(),
        };
        zval.u1.type_info = crate::sys::IS_OBJECT_EX;
        Self(Value(Cell::new(zval)))
    }

    /// Give up the zval to whoever takes it (`RETVAL_ZVAL(value, 0, 0)`),
    /// leaving nothing for the drop.
    #[must_use]
    #[inline]
    pub fn into_zval(self) -> zval {
        *std::mem::ManuallyDrop::new(self).0.0.get_mut()
    }

    /// `zval_delref_p`: give up this reference without releasing what it holds,
    /// which at the last reference stays allocated (a leak), never destroyed.
    #[inline]
    pub fn delref(self) {
        if let Some(counted) = self.counted() {
            counted[REFCOUNT].set(counted[REFCOUNT].get() - 1);
        }
        // The reference went above: nothing is left to release.
        let _given_up = self.into_zval();
    }

    /// `convert_to_array`, as PHP's `(array)` cast: this then holds an array,
    /// unless the conversion bailed out.
    /// # Errors
    /// Releasing an object ran PHP code that bailed out.
    #[inline]
    pub fn convert_to_array(&mut self) -> Result<(), bailout::Bailout> {
        bailout::trapped(swr_zend_try_convert_to_array(self))
    }

    /// The zval a [`Given`] left to its giver, with the reference it holds.
    #[inline]
    fn from_zval(value: zval) -> Self {
        Self(Value(Cell::new(value)))
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
        if self.0.array()?.0[REFCOUNT].get() > 1 {
            swr_zend_separate_array(self);
        }
        Some(Separated(self))
    }

    /// `add_next_index_stringl`: Zend copies the bytes.
    #[inline]
    pub fn push_bytes(&mut self, data: &[u8]) {
        if let Some(array) = self.separate() {
            swr_lent_push_bytes(array, IoSlice::new(data));
        }
    }

    /// `php_register_variable_ex(name, value, array)`: the array takes `value`
    /// under the request variable `name` (`a[b][]` nests), or releases it.
    /// # Errors
    /// A warning's user error handler bailed out.
    pub fn register_variable(&mut self, name: &CStr, value: Owned) -> Result<(), bailout::Bailout> {
        let Some(array) = self.separate() else {
            return Ok(());
        };
        let mut value = value.into_zval();
        crate::contain_with(|| php_register_variable_ex(name.into(), Given(&mut value), array))
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
        if let Some(array) = self.separate() {
            array.push(value);
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
        let Some(table) = self.separate().and_then(Separated::insertions) else {
            return;
        };
        let mut value = value.into_zval();
        if zend_hash_index_add(table, index, Given(&mut value)).is_null() {
            // Left to this, which releases it.
            drop(Owned::from_zval(value));
        }
    }

    /// `zend_hash_add_new`: the array takes over `value` under `key`, which the
    /// caller knows is absent, and references the key itself.
    #[inline]
    pub fn add_new(&mut self, key: &ZStr, value: Owned) {
        if let Some(mut array) = self.separate().and_then(Separated::insertions) {
            array.add_new(key, value);
        }
    }

    /// `zend_hash_update`: the array takes over `value` under `key`, releasing
    /// what was there (as [`Owned`]'s drop does), and references the key itself.
    #[inline]
    pub fn update(&mut self, key: &ZStr, value: Owned) {
        self.update_at(Some(key), 0, value);
    }

    /// `zend_hash_index_update`, as [`ArrayMut::update`].
    #[inline]
    pub fn update_index(&mut self, index: zend_ulong, value: Owned) {
        self.update_at(None, index, value);
    }

    /// [`ArrayMut::update`] under `key`, or under `index` for none.
    #[inline]
    fn update_at(&mut self, key: Option<&ZStr>, index: zend_ulong, value: Owned) {
        let Some(table) = self.separate().and_then(Separated::insertions) else {
            return;
        };
        let mut value = value.into_zval();
        bailout::defer(bailout::trapped(swr_zend_try_hash_update(
            table,
            key,
            index,
            Given(&mut value),
        )));
    }

    /// `zend_hash_index_del`: the value under `index`, if any, is released,
    /// and a bailout its `__destruct` raises is deferred.
    #[inline]
    pub fn remove_index(&mut self, index: zend_ulong) {
        if let Some(table) = self.separate().and_then(Separated::insertions) {
            bailout::defer(bailout::trapped(swr_zend_try_hash_index_del(table, index)));
        }
    }

    /// Swoole's `zend::array_add_or_merge`: `value` under `key`, or appended to
    /// what is there, made a list first unless it is one.
    #[inline]
    pub fn merge(&mut self, key: &[u8], value: Owned) {
        let Some(array) = self.separate() else {
            return;
        };
        let mut value = value.into_zval();
        swr_lent_array_add_or_merge(array, IoSlice::new(key), Given(&mut value));
    }

    /// `php_array_merge(this, source)`, with a replaced value's `__destruct`
    /// run inside `zend_try` and a bailout deferred; nothing when `source` is
    /// no array.
    #[inline]
    pub fn merge_array(&mut self, source: &Value) {
        if source.array().is_none() {
            return;
        }
        if let Some(array) = self.separate() {
            bailout::defer(bailout::trapped(swr_zend_try_array_merge(array.0, source)));
        }
    }

    /// The array zval, separated, for a C writer; None once a bailout is
    /// pending.
    #[must_use]
    #[inline]
    pub fn as_mut_ptr(&mut self) -> Option<*mut zval> {
        self.separate().map(|array| array.0.as_ptr())
    }
}

/// An [`ArrayMut`] that [`ArrayMut::separate`] separated: its array has no
/// other holder and is not immutable while this borrows it, so only this may
/// change the table in place. One pointer wide, as C takes the `zval *`.
#[repr(transparent)]
pub struct Separated<'a>(&'a mut ArrayMut);

impl Insertions<'_> {
    /// `zend_hash_add_new` for a key the caller knows is absent. Unlike an
    /// update, this releases no value and runs no PHP code, so it keeps the
    /// exclusive borrow for another insertion without separating again.
    #[inline]
    pub fn add_new(&mut self, key: &ZStr, value: Owned) {
        if bailout::pending() {
            return;
        }
        let mut value = value.into_zval();
        zend_hash_add_new(Insertions(self.0), key, Given(&mut value));
    }
}

impl<'a> Separated<'a> {
    /// `add_next_index_zval`: the array takes `value`, which is released
    /// instead when the array has no next index (it would pass `ZEND_LONG_MAX`).
    #[inline]
    pub fn push(self, value: Owned) {
        let Some(table) = self.insertions() else {
            return;
        };
        let mut value = value.into_zval();
        if zend_hash_next_index_insert(table, Given(&mut value)).is_null() {
            // Left to this, which releases it.
            drop(Owned::from_zval(value));
        }
    }

    /// Retain this table for new-key writes, which cannot release an existing
    /// value or expose the array to another holder.
    #[inline]
    pub fn insertions(self) -> Option<Insertions<'a>> {
        let array: &'a ArrayMut = self.0;
        array.array().map(Insertions)
    }

    /// `add_assoc_zval_ex(array, key, value)`, with a released value's
    /// `__destruct` run inside `zend_try` and a bailout deferred.
    #[inline]
    pub fn update(self, key: &[u8], value: Owned) {
        let mut value = value.into_zval();
        bailout::defer(bailout::trapped(swr_lent_symtable_update(
            self,
            IoSlice::new(key),
            Given(&mut value),
        )));
    }

    /// `zend_hash_clean`; a destructor's bailout is deferred. It takes the
    /// borrow, because a destructor may run PHP code that replaces the array.
    #[inline]
    pub fn clear(self) {
        bailout::defer(bailout::trapped(swr_zend_try_hash_clean(self.0)));
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
            release_string(Dying(self));
        } else if self.kind() > IS_STRING {
            bailout::defer(bailout::trapped(swr_zend_try_ptr_dtor(Dying(self))));
        }
    }
}
