//! More of the call frame: optional ints, callables, nullable zvals, `$this` as a
//! zval and string returns (crates/phpprocess).
use crate::{
    Call, Invalid, Value,
    sys::{zend_fcall_info, zend_fcall_info_cache, zend_long},
};
use std::{ffi::c_int, io::IoSlice};

unsafe extern "C" {
    /// `Z_PARAM_FUNC` of `arg`, numbered `number` for the error, into `fci`
    /// and `fcc`: nonzero when callable; Zend has raised the parameter error
    /// otherwise. The call info copies `arg`'s bits without a reference.
    safe fn swr_zend_parse_func(
        arg: &Value,
        number: u32,
        fci: &mut zend_fcall_info,
        fcc: &mut zend_fcall_info_cache,
    ) -> c_int;
    /// `php_json_decode_ex` of `text` but its last byte, which the decoder may
    /// read, into `out`, in place of what it held, which is not released (as
    /// [`Value::put`]), inside `zend_try` (nonzero for a bailout).
    safe fn swr_bytes_json_decode_ex(
        out: &Value,
        text: IoSlice<'_>,
        options: zend_long,
        depth: zend_long,
    ) -> c_int;
}

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
        let (mut fci, mut fcc) = (crate::EMPTY_FCI, crate::EMPTY_FCC);
        self.callable(n, &mut fci, &mut fcc)
    }

    /// `Z_PARAM_FUNC` of passed argument `n` (1-based) into `fci` and `fcc`: the
    /// callable it names, resolved. The call info borrows the argument's value (no
    /// reference of its own), as Zend's macro leaves it.
    /// # Errors
    /// It is not callable; Zend has raised its parameter error.
    /// # Panics
    /// Argument `n` was not passed (the caller's arity check allows it).
    pub fn callable(
        &self,
        n: u32,
        fci: &mut zend_fcall_info,
        fcc: &mut zend_fcall_info_cache,
    ) -> Result<(), Invalid> {
        if swr_zend_parse_func(self.arg_value(n), n, fci, fcc) != 0 {
            Ok(())
        } else {
            Err(Invalid)
        }
    }

    /// `Z_PARAM_ZVAL_EX(dest, 1, 0)`: argument `n`, `None` when null or not passed.
    #[must_use]
    #[inline]
    pub fn zval_or_null(&self, n: u32) -> Option<&crate::Value> {
        self.value(n).filter(|value| !value.is_null())
    }

    /// Swoole's `zend::json_decode` of `text[window]` into the return value,
    /// inside `zend_try`. The decoder may read the byte after the window, which
    /// a Zend string always has (its own, or its NUL). A window reaching past
    /// `text` is cut at its end.
    /// # Errors
    /// The decoder bailed out; the caller defers it.
    pub fn json_decode(
        &self,
        text: &crate::ZStr,
        window: std::ops::Range<usize>,
        options: zend_long,
        depth: zend_long,
    ) -> Result<(), crate::bailout::Bailout> {
        // The string's bytes and its NUL: the window and the byte after it.
        let bytes = text.bytes_with_nul();
        let end = window.end.min(bytes.len().saturating_sub(1));
        let start = window.start.min(end);
        // Always in bounds: `start <= end < bytes.len()`.
        let lent = IoSlice::new(bytes.get(start..=end).unwrap_or_default());
        crate::bailout::trapped(swr_bytes_json_decode_ex(
            self.ret_slot(),
            lent,
            options,
            depth,
        ))
    }

    /// `RETVAL_STRINGL`: a copy of `bytes`.
    pub fn set_bytes(&self, bytes: &[u8]) {
        self.set_zstring(crate::ZString::new(bytes));
    }
}
