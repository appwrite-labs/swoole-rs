//! More of the call frame: optional and nullable parameters as `Option`s, and
//! byte-slice and copied return values.
use super::{Call, Invalid, coerce, type_of};
use crate::{
    string::bytes,
    sys::{
        _zend_expected_type_Z_EXPECTED_ARRAY, _zend_expected_type_Z_EXPECTED_STRING_OR_NULL,
        IS_ARRAY, IS_NULL, IS_STRING, swr_zend_try_parse_str, swr_zval_copy, zend_string, zval,
    },
};
use std::ptr;

impl Call {
    /// `Z_PARAM_ZVAL` of argument `n` (1-based); `None` when not passed.
    #[must_use]
    #[inline]
    pub fn passed(&self, n: u32) -> Option<*mut zval> {
        (n <= self.num_args()).then(|| self.arg(n))
    }

    /// `Z_PARAM_STR_OR_NULL`: as `str`, but a passed null is `None`.
    /// # Errors
    /// The argument is not a string or null under the caller's typing mode;
    /// Zend has thrown.
    #[inline]
    pub fn str_or_null(&self, n: u32) -> Result<Option<*mut zend_string>, Invalid> {
        let Some(arg) = self.passed(n) else {
            return Ok(None);
        };
        // SAFETY: `arg` is a passed, live zval.
        let kind = unsafe { type_of(arg) };
        if kind == IS_NULL {
            return Ok(None);
        }
        if kind == IS_STRING {
            // SAFETY: as above.
            let value = unsafe { (*arg).value };
            // SAFETY: an IS_STRING zval holds `str_`.
            return Ok(Some(unsafe { value.str_ }));
        }
        let mut out = ptr::null_mut();
        // SAFETY: `arg` is live and `out` is a writable local.
        if unsafe { coerce(swr_zend_try_parse_str, arg, &raw mut out, n) }? {
            return Ok(Some(out));
        }
        Err(Self::wrong(
            n,
            arg,
            _zend_expected_type_Z_EXPECTED_STRING_OR_NULL,
        ))
    }

    /// `Z_PARAM_STRING` of argument `n`: the bytes, or `None` when not passed.
    /// # Errors
    /// As `str`.
    #[inline]
    pub fn string_opt<'a>(&self, n: u32) -> Result<Option<&'a [u8]>, Invalid> {
        // SAFETY: a parsed string argument lives for the call.
        Ok(self.str(n)?.map(|s| unsafe { bytes(s) }))
    }

    /// `Z_PARAM_STRING_EX(dest, len, 1, 0)`: as `string_opt`, a passed null is `None`.
    /// # Errors
    /// As `str_or_null`.
    #[inline]
    pub fn string_or_null<'a>(&self, n: u32) -> Result<Option<&'a [u8]>, Invalid> {
        // SAFETY: a parsed string argument lives for the call.
        Ok(self.str_or_null(n)?.map(|s| unsafe { bytes(s) }))
    }

    /// `Z_PARAM_ARRAY` of argument `n`; `None` when not passed.
    /// # Errors
    /// The argument is not an array; Zend has thrown.
    #[inline]
    pub fn array_opt(&self, n: u32) -> Result<Option<*mut zval>, Invalid> {
        let Some(arg) = self.passed(n) else {
            return Ok(None);
        };
        // SAFETY: `arg` is a passed, live zval.
        if unsafe { type_of(arg) } == IS_ARRAY {
            return Ok(Some(arg));
        }
        Err(Self::wrong(n, arg, _zend_expected_type_Z_EXPECTED_ARRAY))
    }

    /// `RETVAL_ZVAL(value, 1, 0)`: a new reference to `value`.
    /// # Safety
    /// `value` is a live zval.
    #[inline]
    pub unsafe fn set_copy(&self, value: *const zval) {
        // SAFETY: the return slot is live and `value` per the caller.
        unsafe { swr_zval_copy(self.value, value) };
    }
}
