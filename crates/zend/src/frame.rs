//! More of the call frame: optional ints, callables, nullable zvals, `$this` as a
//! zval and string returns (crates/phpprocess).
use crate::{
    Call, Invalid, bailout,
    sys::{swr_zval_stringl, zend_execute_data, zval},
};
use std::mem::MaybeUninit;

impl Call {
    /// `Z_PARAM_LONG_OR_NULL` of argument `n`: `None` when null or not passed.
    /// # Errors
    /// As [`Call::long`].
    #[inline]
    pub fn long_or_null(&self, n: u32) -> Result<Option<i64>, Invalid> {
        if self.value(n).is_none_or(crate::Value::is_null) {
            return Ok(None);
        }
        self.long(n, 0).map(Some)
    }

    /// `Z_PARAM_FUNC` of argument `n` (1-based): checks it is callable.
    /// # Errors
    /// It is not; Zend has thrown.
    #[inline]
    pub fn func(&self, n: u32) -> Result<(), Invalid> {
        let (mut fci, mut fcc) = (MaybeUninit::uninit(), MaybeUninit::uninit());
        let arg = self.arg_value(n).as_ptr();
        // SAFETY: passed argument `n`; Zend fills the scratch callable, which
        // nothing reads.
        unsafe { bailout::parse_func(arg, n, fci.as_mut_ptr(), fcc.as_mut_ptr()) }
    }

    /// `Z_PARAM_ZVAL_EX(dest, 1, 0)`: argument `n`, `None` when null or not passed.
    #[must_use]
    #[inline]
    pub fn zval_or_null(&self, n: u32) -> Option<&crate::Value> {
        self.value(n).filter(|value| !value.is_null())
    }

    /// `ZEND_THIS`: the zval holding `$this`, in a non-static method.
    #[must_use]
    #[inline]
    pub fn this_zval(&self) -> *mut zval {
        let frame: *mut zend_execute_data = self.frame;
        // SAFETY: a live frame; only the field's address is taken.
        unsafe { &raw mut (*frame).This }
    }

    /// `RETVAL_STRINGL`: a copy of `bytes`.
    pub fn set_bytes(&self, bytes: &[u8]) {
        // SAFETY: the return slot is live; Zend copies the bytes.
        unsafe { swr_zval_stringl(self.return_value(), bytes.as_ptr().cast(), bytes.len()) };
    }
}
