//! More of the call frame: optional and nullable parameters as `Option`s, and
//! byte-slice and copied return values.
use super::{Call, Invalid};
use crate::{
    ZStr,
    sys::{
        _zend_expected_type_Z_EXPECTED_PATH, _zend_expected_type_Z_EXPECTED_STRING_OR_LONG,
        _zend_expected_type_Z_EXPECTED_STRING_OR_NULL, swr_zend_try_parse_str_or_long,
        swr_zval_copy, zend_string, zval,
    },
};
use std::ptr;

impl Call {
    /// `Z_PARAM_STR_OR_NULL`: as `str`, but a passed null is `None`.
    /// # Errors
    /// The argument is not a string or null under the caller's typing mode;
    /// Zend has thrown.
    #[inline]
    pub fn str_or_null(&self, n: u32) -> Result<Option<*mut zend_string>, Invalid> {
        Ok(self.zstr_or_null(n)?.map(ZStr::as_ptr))
    }

    /// [`Call::str_or_null`], borrowed for the call.
    /// # Errors
    /// As [`Call::str_or_null`].
    #[inline]
    pub fn zstr_or_null(&self, n: u32) -> Result<Option<&ZStr>, Invalid> {
        if self.value(n).is_none_or(crate::Value::is_null) {
            return Ok(None);
        }
        self.parse_str(n, _zend_expected_type_Z_EXPECTED_STRING_OR_NULL)
            .map(Some)
    }

    /// `Z_PARAM_PATH` of argument `n`: as [`Call::string`], refusing NUL bytes.
    /// # Errors
    /// As `string`, or the path holds a NUL byte; Zend has thrown.
    #[inline]
    pub fn path<'a>(&'a self, n: u32, default: &'a [u8]) -> Result<&'a [u8], Invalid> {
        let path = self.string(n, default)?;
        if path.contains(&0) {
            return Err(Self::wrong(
                n,
                self.arg_value(n),
                _zend_expected_type_Z_EXPECTED_PATH,
            ));
        }
        Ok(path)
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
    /// The argument is neither under the caller's typing mode; Zend has
    /// thrown, or a `__toString` bailed out (deferred).
    #[inline]
    pub fn str_or_long(&self, n: u32) -> Result<Option<Result<i64, *mut zend_string>>, Invalid> {
        if !self.passed_arg(n) {
            return Ok(None);
        }
        let arg = self.arg_value(n);
        if let Some(string) = arg.str() {
            return Ok(Some(Err(string.as_ptr())));
        }
        if let Some(long) = arg.as_long() {
            return Ok(Some(Ok(long)));
        }
        let (mut string, mut long, mut parsed) = (ptr::null_mut(), 0, false);
        // SAFETY: `arg` is live and the outputs are writable locals.
        let bailed = unsafe {
            swr_zend_try_parse_str_or_long(
                arg.as_ptr(),
                &raw mut string,
                &raw mut long,
                n,
                &raw mut parsed,
            )
        };
        if let Err(bailout) = crate::bailout::trapped(bailed) {
            bailout.defer();
            return Err(Invalid);
        }
        if parsed {
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
    pub fn string_opt(&self, n: u32) -> Result<Option<&[u8]>, Invalid> {
        Ok(self.zstr(n)?.map(ZStr::bytes))
    }

    /// `Z_PARAM_STRING_EX(dest, len, 1, 0)`: as `string_opt`, a passed null is `None`.
    /// # Errors
    /// As `str_or_null`.
    #[inline]
    pub fn string_or_null(&self, n: u32) -> Result<Option<&[u8]>, Invalid> {
        Ok(self.zstr_or_null(n)?.map(ZStr::bytes))
    }

    /// `Z_PARAM_ARRAY` of argument `n`; `None` when not passed.
    /// # Errors
    /// The argument is not an array; Zend has thrown.
    #[inline]
    pub fn array_opt(&self, n: u32) -> Result<Option<&crate::Value>, Invalid> {
        if !self.passed_arg(n) {
            return Ok(None);
        }
        self.array(n).map(Some)
    }

    /// `Z_PARAM_OBJECT` of passed argument `n`.
    /// # Errors
    /// The argument is not an object; Zend has thrown.
    #[inline]
    pub fn object(&self, n: u32) -> Result<*mut zval, Invalid> {
        let arg = self.arg_value(n).dereferenced();
        if arg.object().is_some() {
            return Ok(arg.as_ptr());
        }
        Err(Self::wrong(
            n,
            arg,
            crate::sys::_zend_expected_type_Z_EXPECTED_OBJECT,
        ))
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
