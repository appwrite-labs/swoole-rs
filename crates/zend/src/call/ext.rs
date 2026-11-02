//! More of the call frame: optional and nullable parameters as `Option`s, and
//! byte-slice and copied return values.
use super::{Call, Invalid, type_of};
use crate::{
    string::bytes,
    sys::{
        _zend_expected_type_Z_EXPECTED_ARRAY, _zend_expected_type_Z_EXPECTED_STRING_OR_LONG,
        _zend_expected_type_Z_EXPECTED_STRING_OR_NULL, IS_ARRAY, IS_LONG, IS_NULL, IS_STRING,
        swr_zval_copy, zend_parse_arg_str_or_long_slow, zend_parse_arg_str_slow, zend_string, zval,
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
        if unsafe { zend_parse_arg_str_slow(arg, &raw mut out, n) } {
            return Ok(Some(out));
        }
        Err(Self::wrong(
            n,
            arg,
            _zend_expected_type_Z_EXPECTED_STRING_OR_NULL,
        ))
    }

    /// The frame Zend passed (`execute_data`), to pass on to another handler.
    #[must_use]
    #[inline]
    pub fn frame(&self) -> *mut crate::sys::zend_execute_data {
        self.frame
    }

    /// `Z_PARAM_STR_OR_LONG` of argument `n`: `Ok(None)` when not passed,
    /// else the string (`Err` side of the inner result) or the int.
    /// # Errors
    /// The argument is neither under the caller's typing mode; Zend has thrown.
    #[inline]
    pub fn str_or_long(&self, n: u32) -> Result<Option<Result<i64, *mut zend_string>>, Invalid> {
        let Some(arg) = self.passed(n) else {
            return Ok(None);
        };
        // SAFETY: `arg` is a passed, live zval.
        let kind = unsafe { type_of(arg) };
        // SAFETY: as above.
        let value = unsafe { (*arg).value };
        if kind == IS_STRING {
            // SAFETY: an IS_STRING zval holds `str_`.
            return Ok(Some(Err(unsafe { value.str_ })));
        }
        if kind == IS_LONG {
            // SAFETY: an IS_LONG zval holds `lval`.
            return Ok(Some(Ok(unsafe { value.lval })));
        }
        let (mut string, mut long) = (ptr::null_mut(), 0);
        // SAFETY: `arg` is live and the outputs are writable locals.
        if unsafe { zend_parse_arg_str_or_long_slow(arg, &raw mut string, &raw mut long, n) } {
            return Ok(Some(if string.is_null() {
                Ok(long)
            } else {
                Err(string)
            }));
        }
        Err(Self::wrong(
            n,
            arg,
            _zend_expected_type_Z_EXPECTED_STRING_OR_LONG,
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
