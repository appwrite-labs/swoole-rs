//! The frame of an internal function call, parsed as Zend's fast ZPP macros do.
mod ext;

use crate::{
    Class,
    string::bytes,
    sys::{
        _zend_expected_type_Z_EXPECTED_ARRAY, _zend_expected_type_Z_EXPECTED_BOOL,
        _zend_expected_type_Z_EXPECTED_DOUBLE, _zend_expected_type_Z_EXPECTED_LONG,
        _zend_expected_type_Z_EXPECTED_STRING, _zval_struct__bindgen_ty_1, IS_ARRAY, IS_DOUBLE,
        IS_FALSE, IS_LONG, IS_NULL, IS_OBJECT_EX, IS_REFERENCE, IS_STRING, IS_TRUE,
        ZPP_ERROR_WRONG_ARG, add_assoc_long_ex, swr_zend_array_init, swr_zend_try_parse_bool,
        swr_zend_try_parse_double, swr_zend_try_parse_long, swr_zend_try_parse_str,
        swr_zval_empty_array, swr_zval_empty_string, swr_zval_str, zend_execute_data,
        zend_expected_type, zend_object, zend_string, zend_update_property_long, zend_value,
        zend_wrong_parameter_error, zend_wrong_parameters_count_error, zval,
    },
};
use std::{ffi::CStr, ptr};

/// Zval slots `zend_execute_data` occupies before the first argument
/// (`ZEND_CALL_FRAME_SLOT`).
const FRAME_SLOT: usize = size_of::<zend_execute_data>().div_ceil(size_of::<zval>());

/// `Z_TYPE_P`.
/// # Safety
/// `value` is a live zval.
#[must_use]
#[inline]
pub unsafe fn type_of(value: *const zval) -> u32 {
    // SAFETY: per the caller.
    let zv = unsafe { *value };
    // SAFETY: every zval sets `u1.type_info`.
    unsafe { zv.u1.type_info & 0xff }
}

/// The arguments did not parse; Zend has raised the error (or a user error
/// handler bailed out of a coercion's deprecation, which is deferred).
#[derive(Debug)]
pub struct Invalid;

/// A ZPP slow-path coercion inside `zend_try`: whether it parsed. Coercions
/// raise deprecations, which run the user error handler.
/// # Errors
/// That handler bailed out; the bailout is deferred and nothing more is raised.
/// # Safety
/// `arg` is a live argument and `out` a writable slot of `parse`'s type.
#[cold]
unsafe fn coerce<T>(
    parse: unsafe extern "C" fn(*mut zval, *mut T, u32, *mut bool) -> std::ffi::c_int,
    arg: *mut zval,
    out: *mut T,
    n: u32,
) -> Result<bool, Invalid> {
    let mut parsed = false;
    // SAFETY: per the caller; `parsed` is a writable local.
    let bailed = unsafe { parse(arg, out, n, &raw mut parsed) };
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

    /// `ZEND_CALL_ARG(execute_data, n)`, 1-based; `n` is at most `num_args()`.
    #[inline]
    pub(crate) fn arg(&self, n: u32) -> *mut zval {
        self.frame
            .cast::<zval>()
            .wrapping_add(FRAME_SLOT + n as usize - 1)
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
        if n > self.num_args() {
            return Ok(default);
        }
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        let zv = unsafe { *arg };
        // SAFETY: every zval sets `u1.type_info`.
        if unsafe { zv.u1.type_info } & 0xff == IS_LONG {
            // SAFETY: an IS_LONG zval holds `lval`.
            return Ok(unsafe { zv.value.lval });
        }
        let mut out = 0;
        // SAFETY: `arg` is live and `out` is a writable local.
        if unsafe { coerce(swr_zend_try_parse_long, arg, &raw mut out, n) }? {
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
        if n > self.num_args() {
            return Ok(default);
        }
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        let zv = unsafe { *arg };
        // SAFETY: every zval sets `u1.type_info`.
        let kind = unsafe { zv.u1.type_info } & 0xff;
        if kind == IS_TRUE || kind == IS_FALSE {
            return Ok(kind == IS_TRUE);
        }
        let mut out = false;
        // SAFETY: `arg` is live and `out` is a writable local.
        if unsafe { coerce(swr_zend_try_parse_bool, arg, &raw mut out, n) }? {
            return Ok(out);
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_BOOL))
    }

    /// `Z_PARAM_STR` of argument `n` (1-based): a string the frame keeps alive
    /// (a coerced value replaces the argument), or `None` when an optional
    /// argument was not passed.
    /// # Errors
    /// The argument is not a string under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn str(&self, n: u32) -> Result<Option<*mut zend_string>, Invalid> {
        if n > self.num_args() {
            return Ok(None);
        }
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        let zv = unsafe { *arg };
        // SAFETY: every zval sets `u1.type_info`.
        if unsafe { zv.u1.type_info } & 0xff == IS_STRING {
            // SAFETY: an IS_STRING zval holds `str`.
            return Ok(Some(unsafe { zv.value.str_ }));
        }
        let mut out = ptr::null_mut();
        // SAFETY: `arg` is live and `out` is a writable local.
        if unsafe { coerce(swr_zend_try_parse_str, arg, &raw mut out, n) }? {
            return Ok(Some(out));
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_STRING))
    }

    /// `Z_PARAM_DOUBLE` of argument `n` (1-based), or `default` when an optional
    /// argument was not passed.
    /// # Errors
    /// The argument is not a float under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn double(&self, n: u32, default: f64) -> Result<f64, Invalid> {
        if n > self.num_args() {
            return Ok(default);
        }
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        let zv = unsafe { *arg };
        // SAFETY: every zval sets `u1.type_info`.
        if unsafe { zv.u1.type_info } & 0xff == IS_DOUBLE {
            // SAFETY: an IS_DOUBLE zval holds `dval`.
            return Ok(unsafe { zv.value.dval });
        }
        let mut out = 0.0;
        // SAFETY: `arg` is live and `out` is a writable local.
        if unsafe { coerce(swr_zend_try_parse_double, arg, &raw mut out, n) }? {
            return Ok(out);
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_DOUBLE))
    }

    /// `Z_PARAM_STRING` of argument `n` (1-based), or `default` when an optional
    /// argument was not passed. The bytes are NUL-terminated and live for the call.
    /// # Errors
    /// The argument is not a string under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn string<'a>(&self, n: u32, default: &'a [u8]) -> Result<&'a [u8], Invalid> {
        if n > self.num_args() {
            return Ok(default);
        }
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        let zv = unsafe { *arg };
        // SAFETY: every zval sets `u1.type_info`.
        if unsafe { zv.u1.type_info } & 0xff == IS_STRING {
            // SAFETY: an IS_STRING zval holds `str`.
            let string = unsafe { zv.value.str_ };
            // SAFETY: a live string the frame keeps.
            return Ok(unsafe { bytes(string) });
        }
        let mut out: *mut zend_string = ptr::null_mut();
        // SAFETY: `arg` is live and `out` is a writable local; Zend leaves the
        // converted string in the argument slot, so it lives for the call.
        if unsafe { coerce(swr_zend_try_parse_str, arg, &raw mut out, n) }? {
            // SAFETY: as above.
            return Ok(unsafe { bytes(out) });
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_STRING))
    }

    /// `Z_PARAM_ARRAY` of argument `n` (1-based), which must be passed.
    /// # Errors
    /// The argument is not an array; Zend has thrown.
    #[inline]
    pub fn array(&self, n: u32) -> Result<*mut zval, Invalid> {
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        if unsafe { type_of(arg) } == IS_ARRAY {
            return Ok(arg);
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_ARRAY))
    }

    /// `Z_PARAM_ZVAL_EX(.., 0, 1)` of argument `n` (1-based), which must be
    /// passed: a reference yields the value it refers to.
    #[must_use]
    #[inline]
    pub fn zval_deref(&self, n: u32) -> *mut zval {
        let arg = self.arg(n);
        // SAFETY: `n` is a passed argument, so `arg` is its live zval.
        if unsafe { type_of(arg) } == IS_REFERENCE {
            // SAFETY: `arg` is live.
            let value = unsafe { (*arg).value };
            // SAFETY: an IS_REFERENCE zval holds `ref`.
            let reference = unsafe { value.ref_ };
            // SAFETY: a live reference.
            return unsafe { &raw mut (*reference).val };
        }
        arg
    }

    /// `ZEND_PARSE_PARAMETERS_END`'s report of a wrong argument.
    #[cold]
    fn wrong(n: u32, arg: *mut zval, expected: zend_expected_type) -> Invalid {
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
                arg,
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
        if n > self.num_args() {
            return ptr::null_mut();
        }
        self.arg(n)
    }

    /// The return slot (`return_value`).
    #[must_use]
    #[inline]
    pub fn return_value(&self) -> *mut zval {
        self.value
    }

    /// `RETVAL_ZVAL(value, 0, 0)`: the return slot takes over `value`.
    #[inline]
    pub fn set_value(&self, value: zval) {
        // SAFETY: the return slot is live for the handler.
        unsafe { *self.value = value };
    }

    /// `RETVAL_STR(string)`: the return slot takes over the reference.
    /// # Safety
    /// `string` is a live string carrying a reference the caller gives up.
    pub unsafe fn set_string(&self, string: *mut zend_string) {
        // SAFETY: the return slot is live for the handler; per the caller,
        // `string` carries a reference.
        unsafe { swr_zval_str(self.value, string) };
    }

    /// `RETURN_ZVAL(ZEND_THIS, 1, 0)`: return `$this` with a new reference.
    pub fn set_this(&self) {
        let this = self.this();
        // SAFETY: a method's live `$this`.
        unsafe { (*this).gc.refcount += 1 };
        // SAFETY: the return slot is live for the handler.
        unsafe { (*self.value).value = zend_value { obj: this } };
        self.set(IS_OBJECT_EX);
    }

    /// `array_init(return_value)`.
    pub fn set_array(&self) {
        // SAFETY: the return slot is live for the handler.
        unsafe { swr_zend_array_init(self.value) };
    }

    /// `add_assoc_long_ex(return_value, key, value)`, after `set_array`.
    pub fn add_assoc_long(&self, key: &CStr, value: i64) {
        // SAFETY: the return slot holds an array; Zend copies the key.
        unsafe { add_assoc_long_ex(self.value, key.as_ptr(), key.count_bytes(), value) };
    }

    /// `zend_update_property_long(scope, Z_OBJ_P(ZEND_THIS), name, value)`;
    /// `scope` is the class whose view of the property to update (NULL if none).
    pub fn update_property_long(&self, scope: Option<&Class>, name: &CStr, value: i64) {
        let scope = scope.map_or(ptr::null_mut(), Class::entry);
        // SAFETY: a method's live `$this`; a registered class or null; Zend
        // copies the name.
        unsafe {
            zend_update_property_long(scope, self.this(), name.as_ptr(), name.count_bytes(), value);
        }
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
        // SAFETY: the return slot is live.
        unsafe { swr_zval_empty_string(self.value) };
    }

    /// `RETVAL_EMPTY_ARRAY`.
    #[inline]
    pub fn set_empty_array(&self) {
        // SAFETY: the return slot is live.
        unsafe { swr_zval_empty_array(self.value) };
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
