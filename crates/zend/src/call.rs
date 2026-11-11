//! The frame of an internal function call, parsed as Zend's fast ZPP macros do.
mod ext;

use crate::{
    Array, Class, Value, ZStr,
    sys::{
        _zend_expected_type_Z_EXPECTED_ARRAY, _zend_expected_type_Z_EXPECTED_BOOL,
        _zend_expected_type_Z_EXPECTED_BOOL_OR_NULL, _zend_expected_type_Z_EXPECTED_DOUBLE,
        _zend_expected_type_Z_EXPECTED_LONG, _zend_expected_type_Z_EXPECTED_RESOURCE,
        _zend_expected_type_Z_EXPECTED_STRING, _zval_struct__bindgen_ty_1, IS_ARRAY, IS_FALSE,
        IS_LONG, IS_NULL, IS_OBJ_DESTRUCTOR_CALLED, IS_RESOURCE, IS_TRUE, ZPP_ERROR_WRONG_ARG,
        swr_zend_array_init, swr_zend_try_parse_bool, swr_zend_try_parse_double,
        swr_zend_try_parse_long, swr_zend_try_parse_str, swr_zval_str, zend_argument_type_error,
        zend_argument_value_error, zend_execute_data, zend_expected_type, zend_object, zend_string,
        zend_value, zend_wrong_parameter_error, zend_wrong_parameters_count_error, zval,
    },
};
use std::{ffi::CStr, ptr};

/// Zval slots `zend_execute_data` occupies before the first argument
/// (`ZEND_CALL_FRAME_SLOT`).
const FRAME_SLOT: usize = size_of::<zend_execute_data>().div_ceil(size_of::<zval>());

/// `Z_TYPE`.
#[must_use]
#[inline]
pub fn kind(value: &zval) -> u32 {
    // SAFETY: every zval sets `u1.type_info`, and any four bytes are a `u32`.
    unsafe { value.u1.type_info & 0xff }
}

/// `Z_TYPE_P`.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn type_of(value: *const zval) -> u32 {
    // SAFETY: per the caller.
    kind(unsafe { &*value })
}

/// The arguments did not parse; Zend has raised the error (or a user error
/// handler bailed out of a coercion's deprecation, which is deferred).
#[derive(Debug)]
pub struct Invalid;

/// A ZPP slow-path coercion inside `zend_try`: whether it parsed. Coercions
/// raise deprecations, which run the user error handler.
/// `parse` is one of the shim's `swr_zend_try_parse_*`, whose output is a `T`.
/// # Errors
/// That handler bailed out; the bailout is deferred and nothing more is raised.
#[cold]
fn coerce<T>(
    parse: unsafe extern "C" fn(*mut zval, *mut T, u32, *mut bool) -> std::ffi::c_int,
    arg: &Value,
    out: &mut T,
    n: u32,
) -> Result<bool, Invalid> {
    let mut parsed = false;
    // SAFETY: a live argument, a writable `T` (per `parse`) and a writable local.
    let bailed = unsafe { parse(arg.as_ptr(), out, n, &raw mut parsed) };
    match crate::bailout::trapped(bailed) {
        Ok(()) => Ok(parsed),
        Err(bailout) => {
            bailout.defer();
            Err(Invalid)
        }
    }
}

/// One call of an internal function or method: what Zend passes a handler
/// (`INTERNAL_FUNCTION_PARAMETERS`).
pub struct Call {
    pub(crate) frame: *mut zend_execute_data,
    value: *mut zval,
}

impl Call {
    /// # Safety
    /// `frame` and `value` are the live frame and return slot Zend passed this
    /// handler, and the `Call` does not outlive the handler.
    #[must_use]
    #[inline]
    pub unsafe fn new(frame: *mut zend_execute_data, value: *mut zval) -> Self {
        Self { frame, value }
    }

    /// This thread runs PHP for as long as the call lasts.
    #[must_use]
    pub fn php(&self) -> crate::local::Php<'_> {
        crate::local::Php(std::marker::PhantomData)
    }

    /// `ZEND_NUM_ARGS()`.
    #[must_use]
    #[inline]
    pub fn num_args(&self) -> u32 {
        // SAFETY: a live frame.
        let this = unsafe { (*self.frame).This };
        // SAFETY: a call frame's `This.u2` holds the argument count.
        unsafe { this.u2.num_args }
    }

    /// `Z_OBJ_P(ZEND_THIS)`: valid in a non-static method only.
    #[must_use]
    #[inline]
    pub fn this(&self) -> *mut zend_object {
        // SAFETY: a live frame.
        let this = unsafe { (*self.frame).This };
        // SAFETY: a non-static method's `This` holds its object.
        unsafe { this.value.obj }
    }

    /// `GC_FLAGS(Z_OBJ_P(ZEND_THIS)) & IS_OBJ_DESTRUCTOR_CALLED`: Zend is running
    /// `$this`'s destructor (`SW_PREVENT_USER_DESTRUCT`), not user code. False
    /// without a `$this`.
    #[must_use]
    #[inline]
    pub fn destructor_called(&self) -> bool {
        let Some(this) = self.this_object() else {
            return false;
        };
        // SAFETY: the frame's live `$this`.
        let header = unsafe { (*this).gc.u };
        // SAFETY: a refcounted header always sets `type_info`.
        unsafe { header.type_info & IS_OBJ_DESTRUCTOR_CALLED != 0 }
    }

    /// `ZVAL_COPY(&value, ZEND_THIS)`: `$this` with a reference of its own,
    /// or None without one (a static method).
    #[must_use]
    pub fn this_owned(&self) -> Option<crate::Owned> {
        let this = self.this_object()?;
        // SAFETY: the frame's live `$this`.
        unsafe { (*this).gc.refcount += 1 };
        // SAFETY: as above, carrying the reference added.
        Some(unsafe { crate::Owned::object(this) })
    }

    /// `$this`'s [`Custom`](crate::Custom) data, or None when `$this` is no `T`
    /// object (a static method). The frame holds `$this` for the call.
    #[must_use]
    #[inline]
    pub fn custom<T: crate::Custom>(&self) -> Option<&T> {
        // SAFETY: the frame's live `$this`, held while `self` is borrowed.
        unsafe { crate::object::custom(self.this_object()?) }
    }

    /// `ZEND_THIS` holding an object, borrowed for the call, or None without
    /// one (a static method).
    #[must_use]
    #[inline]
    pub fn this_value(&self) -> Option<&Value> {
        // SAFETY: a live frame.
        let this = unsafe { &raw const (*self.frame).This };
        // SAFETY: the frame holds `This` for the call.
        unsafe { Value::from_ptr(this) }.filter(|this| this.object().is_some())
    }

    /// `Z_OBJ_P(ZEND_THIS)`, or None without an object (a static method).
    #[inline]
    pub(crate) fn this_object(&self) -> Option<*mut zend_object> {
        // SAFETY: a live frame.
        let this = unsafe { &raw const (*self.frame).This };
        // SAFETY: `This` is a zval whose type says whether it holds an object.
        unsafe { Value::from_ptr(this) }?.object()
    }

    /// `ZEND_CALL_ARG(execute_data, n)`'s address, 1-based; a slot only when
    /// [`Call::passed_arg`].
    #[inline]
    pub(crate) fn arg(&self, n: u32) -> *mut zval {
        self.frame
            .cast::<zval>()
            .wrapping_add(FRAME_SLOT + (n as usize).wrapping_sub(1))
    }

    /// Whether argument `n` (1-based) was passed: `1..=num_args()`. Argument 0
    /// would be the frame's own last bytes, and one past the count is not
    /// part of the frame.
    #[inline]
    pub(crate) fn passed_arg(&self, n: u32) -> bool {
        n.wrapping_sub(1) < self.num_args()
    }

    /// Passed argument `n`, borrowed for the call.
    /// # Panics
    /// Argument `n` was not passed (the caller's arity check allows it).
    #[inline]
    pub(crate) fn arg_value(&self, n: u32) -> &Value {
        assert!(self.passed_arg(n), "argument {n} was not passed");
        // SAFETY: a passed argument lives for the call.
        unsafe { &*self.arg(n).cast::<Value>() }
    }

    /// `ZEND_PARSE_PARAMETERS_START(min, max)`'s count check.
    /// # Errors
    /// The count is outside `min..=max`; Zend has thrown an `ArgumentCountError`.
    #[inline]
    pub fn arity(&self, min: u32, max: u32) -> Result<(), Invalid> {
        let count = self.num_args();
        if (min..=max).contains(&count) {
            return Ok(());
        }
        // SAFETY: called from a handler, as Zend's own macro is.
        unsafe { zend_wrong_parameters_count_error(min, max) };
        Err(Invalid)
    }

    /// `Z_PARAM_LONG` of argument `n` (1-based), or `default` when an optional
    /// argument was not passed.
    /// # Errors
    /// The argument is not an int under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn long(&self, n: u32, default: i64) -> Result<i64, Invalid> {
        if !self.passed_arg(n) {
            return Ok(default);
        }
        let arg = self.arg_value(n);
        if let Some(long) = arg.as_long() {
            return Ok(long);
        }
        let mut out = 0;
        if coerce(swr_zend_try_parse_long, arg, &mut out, n)? {
            return Ok(out);
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_LONG))
    }

    /// `Z_PARAM_BOOL` of argument `n` (1-based), or `default` when an optional
    /// argument was not passed.
    /// # Errors
    /// The argument is not a bool under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn bool(&self, n: u32, default: bool) -> Result<bool, Invalid> {
        if !self.passed_arg(n) {
            return Ok(default);
        }
        self.parse_bool(n, false)
            .map(|parsed| parsed.unwrap_or(false))
    }

    /// `Z_PARAM_BOOL_OR_NULL` of argument `n` (1-based): `None` when null or
    /// not passed.
    /// # Errors
    /// The argument is not a bool or null under the caller's typing mode; Zend
    /// has thrown.
    #[inline]
    pub fn bool_or_null(&self, n: u32) -> Result<Option<bool>, Invalid> {
        if !self.passed_arg(n) {
            return Ok(None);
        }
        self.parse_bool(n, true)
    }

    /// Passed argument `n` as a bool; a null is `None` when `nullable`, and
    /// is coerced (`false`, with Zend's deprecation) otherwise.
    fn parse_bool(&self, n: u32, nullable: bool) -> Result<Option<bool>, Invalid> {
        let arg = self.arg_value(n);
        match arg.kind() {
            IS_TRUE => return Ok(Some(true)),
            IS_FALSE => return Ok(Some(false)),
            IS_NULL if nullable => return Ok(None),
            _ => {}
        }
        let mut out = false;
        if coerce(swr_zend_try_parse_bool, arg, &mut out, n)? {
            return Ok(Some(out));
        }
        let expected = if nullable {
            _zend_expected_type_Z_EXPECTED_BOOL_OR_NULL
        } else {
            _zend_expected_type_Z_EXPECTED_BOOL
        };
        Err(Self::wrong(n, arg, expected))
    }

    /// `Z_PARAM_STR` of argument `n` (1-based): a string the frame keeps alive
    /// (a coerced value replaces the argument), or `None` when an optional
    /// argument was not passed.
    /// # Errors
    /// The argument is not a string under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn str(&self, n: u32) -> Result<Option<*mut zend_string>, Invalid> {
        Ok(self.zstr(n)?.map(ZStr::as_ptr))
    }

    /// [`Call::str`], borrowed for the call.
    /// # Errors
    /// As [`Call::str`].
    #[inline]
    pub fn zstr(&self, n: u32) -> Result<Option<&ZStr>, Invalid> {
        if !self.passed_arg(n) {
            return Ok(None);
        }
        self.parse_str(n, _zend_expected_type_Z_EXPECTED_STRING)
            .map(Some)
    }

    /// Passed argument `n` as a string, reporting `expected` if it is none.
    pub(crate) fn parse_str(&self, n: u32, expected: zend_expected_type) -> Result<&ZStr, Invalid> {
        let arg = self.arg_value(n);
        if let Some(string) = arg.str() {
            return Ok(string);
        }
        let mut out = ptr::null_mut();
        if coerce(swr_zend_try_parse_str, arg, &mut out, n)? {
            // SAFETY: Zend left the converted string in the argument slot,
            // which keeps it for the call.
            if let Some(string) = unsafe { ZStr::from_ptr(out) } {
                return Ok(string);
            }
        }
        Err(Self::wrong(n, arg, expected))
    }

    /// `Z_PARAM_DOUBLE` of argument `n` (1-based), or `default` when an optional
    /// argument was not passed.
    /// # Errors
    /// The argument is not a float under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn double(&self, n: u32, default: f64) -> Result<f64, Invalid> {
        if !self.passed_arg(n) {
            return Ok(default);
        }
        let arg = self.arg_value(n);
        if let Some(double) = arg.as_double() {
            return Ok(double);
        }
        let mut out = 0.0;
        if coerce(swr_zend_try_parse_double, arg, &mut out, n)? {
            return Ok(out);
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_DOUBLE))
    }

    /// `Z_PARAM_STRING` of argument `n` (1-based), or `default` when an optional
    /// argument was not passed. The bytes are NUL-terminated and live for the call.
    /// # Errors
    /// The argument is not a string under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn string<'a>(&'a self, n: u32, default: &'a [u8]) -> Result<&'a [u8], Invalid> {
        Ok(self.zstr(n)?.map_or(default, ZStr::bytes))
    }

    /// `Z_PARAM_ARRAY` of argument `n` (1-based), which must be passed.
    /// # Errors
    /// The argument is not an array; Zend has thrown.
    #[inline]
    pub fn array(&self, n: u32) -> Result<&Value, Invalid> {
        self.array_ref(n).map(|_| self.arg_value(n))
    }

    /// [`Call::array`]'s table, borrowed for the call.
    /// # Errors
    /// As [`Call::array`].
    #[inline]
    pub fn array_ref(&self, n: u32) -> Result<&Array, Invalid> {
        let arg = self.arg_value(n);
        arg.array()
            .ok_or_else(|| Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_ARRAY))
    }

    /// `Z_PARAM_RESOURCE` of argument `n` (1-based), which must be passed.
    /// # Errors
    /// The argument is not a resource; Zend has thrown.
    #[inline]
    pub fn resource(&self, n: u32) -> Result<&Value, Invalid> {
        let arg = self.arg_value(n);
        if arg.kind() == IS_RESOURCE {
            return Ok(arg);
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_RESOURCE))
    }

    /// `zend_argument_type_error(n, "%s", message)`: a `TypeError` naming argument `n`.
    #[inline]
    pub fn argument_type_error(&self, n: u32, message: &CStr) {
        // SAFETY: from a handler; "%s" takes the message.
        unsafe { zend_argument_type_error(n, c"%s".as_ptr(), message.as_ptr()) };
    }

    /// `zend_argument_value_error(n, "%s", message)`: a `ValueError` naming argument `n`.
    #[inline]
    pub fn argument_value_error(&self, n: u32, message: &CStr) {
        // SAFETY: from a handler; "%s" takes the message.
        unsafe { zend_argument_value_error(n, c"%s".as_ptr(), message.as_ptr()) };
    }

    /// `Z_PARAM_ZVAL_EX(.., 0, 1)` of argument `n` (1-based), which must be
    /// passed: a reference yields the value it refers to.
    #[must_use]
    #[inline]
    pub fn zval_deref(&self, n: u32) -> *mut zval {
        self.arg_value(n).dereferenced().as_ptr()
    }

    /// Make by-reference argument `n` (1-based), which must be passed, hold
    /// `value`: `zval_ptr_dtor` of what it referred to, then `ZVAL_COPY_VALUE`.
    /// # Safety
    /// Nothing borrowed from argument `n` ([`Call::value`], [`Call::zstr`],
    /// [`Call::array_ref`], ...) is used afterwards: what it held is released.
    #[inline]
    pub unsafe fn assign(&self, n: u32, value: crate::Owned) {
        let slot = self.arg_value(n).dereferenced();
        // SAFETY: a passed argument's live value, released here; per the
        // caller, nothing borrows it any more.
        unsafe { slot.release() };
        // SAFETY: as above; the slot takes over the value.
        unsafe { slot.as_ptr().write(value.into_zval()) };
    }

    /// `ZEND_PARSE_PARAMETERS_END`'s report of a wrong argument.
    #[cold]
    fn wrong(n: u32, arg: &Value, expected: zend_expected_type) -> Invalid {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "ZPP error codes are small; Zend's macro passes the same int"
        )]
        // SAFETY: as Zend's macro does, with the argument that failed to parse.
        unsafe {
            zend_wrong_parameter_error(
                ZPP_ERROR_WRONG_ARG as i32,
                n,
                ptr::null_mut(),
                expected,
                arg.as_ptr(),
            );
        }
        Invalid
    }

    #[inline]
    fn set(&self, type_info: u32) {
        // SAFETY: the return slot is live for the handler.
        unsafe { (*self.value).u1 = _zval_struct__bindgen_ty_1 { type_info } };
    }

    /// `RETVAL_LONG`.
    #[inline]
    pub fn set_long(&self, value: i64) {
        // SAFETY: the return slot is live for the handler.
        unsafe { (*self.value).value = zend_value { lval: value } };
        self.set(IS_LONG);
    }

    /// `Z_PARAM_ZVAL` of argument `n` (1-based), or null when an optional
    /// argument was not passed.
    #[must_use]
    #[inline]
    pub fn zval(&self, n: u32) -> *mut zval {
        if !self.passed_arg(n) {
            return ptr::null_mut();
        }
        self.arg(n)
    }

    /// [`Call::zval`], borrowed for the call.
    #[must_use]
    #[inline]
    pub fn value(&self, n: u32) -> Option<&crate::Value> {
        // SAFETY: a passed argument lives for the call, or null.
        unsafe { crate::Value::from_ptr(self.zval(n)) }
    }

    /// The return slot (`return_value`).
    #[must_use]
    #[inline]
    pub fn return_value(&self) -> *mut zval {
        self.value
    }

    /// `RETVAL_ZVAL(value, 0, 0)`: the return slot takes over `value`.
    /// # Safety
    /// `value` is a valid zval carrying the reference the slot takes, as
    /// [`crate::Owned::into_zval`] gives one up.
    #[inline]
    pub unsafe fn set_value(&self, value: zval) {
        // SAFETY: the return slot is live for the handler; `value` per the caller.
        unsafe { *self.value = value };
    }

    /// `RETVAL_ZVAL(value, 0, 0)` of a value this owns.
    #[inline]
    pub fn set_owned(&self, value: crate::Owned) {
        // SAFETY: the return slot is live for the handler, and takes the
        // value `Owned` gives up.
        unsafe { *self.value = value.into_zval() };
    }

    /// `RETVAL_STR(string)`: the return slot takes over the reference.
    /// # Safety
    /// `string` is a live string carrying a reference the caller gives up.
    pub unsafe fn set_string(&self, string: *mut zend_string) {
        // SAFETY: the return slot is live for the handler; per the caller,
        // `string` carries a reference.
        unsafe { swr_zval_str(self.value, string) };
    }

    /// `RETVAL_STR(string)`, taking over its reference.
    #[inline]
    pub fn set_zstring(&self, string: crate::ZString) {
        // SAFETY: a live string and the reference it gives up.
        unsafe { self.set_string(string.into_raw()) };
    }

    /// `RETURN_ZVAL(ZEND_THIS, 1, 0)`: return `$this` with a new reference
    /// (nothing in a static method).
    pub fn set_this(&self) {
        if let Some(this) = self.this_owned() {
            self.set_owned(this);
        }
    }

    /// `array_init(return_value)`.
    pub fn set_array(&self) {
        // SAFETY: the return slot is live for the handler.
        unsafe { swr_zend_array_init(self.value) };
    }

    /// `add_assoc_long_ex(return_value, key, value)`, after `set_array`
    /// (nothing unless the return slot holds an array).
    pub fn add_assoc_long(&self, key: &CStr, value: i64) {
        // SAFETY: the return slot is live for the handler, and nothing else
        // borrows it; `Owned` is a transparent zval, and is not dropped here.
        let slot = unsafe { &mut *self.value.cast::<crate::Owned>() };
        if let Some(array) = slot.array_mut() {
            array.insert_long(key.to_bytes(), value);
        }
    }

    /// `zend_update_property_long(scope, Z_OBJ_P(ZEND_THIS), name, value)`;
    /// `scope` is the class whose view of the property to update (NULL if none).
    /// A static method has no `$this` to update.
    #[inline]
    pub fn update_property_long(&self, scope: Option<&Class>, name: &CStr, value: i64) {
        let Some(this) = self.this_object() else {
            return;
        };
        let scope = scope.map_or(ptr::null_mut(), Class::entry);
        // SAFETY: a method's live `$this`; a registered class or null.
        unsafe { crate::Props::new(scope, this) }.long(name, value);
    }

    /// `zend_update_property(scope, Z_OBJ_P(ZEND_THIS), name, value)`: the
    /// property takes a copy. A dynamic property's deprecation runs the user
    /// error handler, so this traps its bailout. A static method has no `$this`
    /// to update.
    /// # Errors
    /// The error handler bailed out.
    pub fn update_property(
        &self,
        scope: Option<&Class>,
        name: &[u8],
        value: &Value,
    ) -> Result<(), crate::bailout::Bailout> {
        let Some(this) = self.this_object() else {
            return Ok(());
        };
        let scope = scope.map_or(ptr::null_mut(), Class::entry);
        // SAFETY: the frame's live `$this`, a registered class or null and a
        // live value; Zend copies the name and the value.
        let bailed = unsafe {
            crate::sys::swr_zend_try_facade_update(
                scope,
                this,
                name.as_ptr().cast(),
                name.len(),
                value.as_ptr(),
            )
        };
        crate::bailout::trapped(bailed)
    }

    /// `RETURN_ZVAL(zend_read_property(scope, Z_OBJ_P(ZEND_THIS), name, 1, &rv), 1, 0)`:
    /// a copy of the property, read silently. The read can run PHP code (a
    /// subclass's `__get` or property hook), so it runs inside `zend_try` and a
    /// bailout is deferred, leaving the return slot as it was. A static method
    /// has no `$this` to read.
    pub fn set_property(&self, scope: Option<&Class>, name: &[u8]) {
        let Some(this) = self.this_object() else {
            return;
        };
        let scope = scope.map_or(ptr::null_mut(), Class::entry);
        let rv = crate::Owned::default();
        let mut property = ptr::null_mut();
        // SAFETY: the frame's live `$this`, a registered class or null, a slot
        // for a computed value, which outlives the copy below, and a writable local.
        let bailed = unsafe {
            crate::sys::swr_zend_try_facade_read(
                scope,
                this,
                name.as_ptr().cast(),
                name.len(),
                true,
                rv.as_ptr(),
                &raw mut property,
            )
        };
        if let Err(bailout) = crate::bailout::trapped(bailed) {
            bailout.defer();
            return;
        }
        // SAFETY: the property's live value, copied with a reference.
        unsafe { crate::sys::swr_zval_copy(self.value, property) };
    }

    /// `RETVAL_BOOL`.
    #[inline]
    pub fn set_bool(&self, value: bool) {
        self.set(if value { IS_TRUE } else { IS_FALSE });
    }

    /// `RETVAL_NULL`.
    #[inline]
    pub fn set_null(&self) {
        self.set(IS_NULL);
    }

    /// `RETVAL_EMPTY_STRING`.
    #[inline]
    pub fn set_empty_string(&self) {
        // SAFETY: the interned empty string needs no reference.
        unsafe { self.set_string(crate::value::empty_string()) };
    }

    /// `RETVAL_EMPTY_ARRAY`.
    #[inline]
    pub fn set_empty_array(&self) {
        let empty = zend_value {
            arr: crate::value::empty_array(),
        };
        // SAFETY: the return slot is live; the empty array is immutable.
        unsafe { crate::value::set(self.value, empty, IS_ARRAY) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_follow_the_frame() {
        // The slot count Zend's macro computes: whole zvals covering the frame.
        assert_eq!(
            FRAME_SLOT * size_of::<zval>() - size_of::<zend_execute_data>(),
            (size_of::<zval>() - size_of::<zend_execute_data>() % size_of::<zval>())
                % size_of::<zval>(),
            "frame slot rounds the frame up to whole zvals"
        );
    }
}
