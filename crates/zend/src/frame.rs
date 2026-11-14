//! More of the call frame: optional ints, callables, nullable zvals, `$this` as a
//! zval and string returns (crates/phpprocess).
use crate::{
    Call, Invalid,
    sys::{
        swr_zend_parse_func, swr_zend_try_json_decode_ex, swr_zval_stringl, zend_execute_data,
        zend_fcall_info, zend_fcall_info_cache, zval,
    },
};

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
        self.callable(n).map(drop)
    }

    /// `Z_PARAM_FUNC` of passed argument `n` (1-based): the callable it names,
    /// resolved. The call info borrows the argument's value (no reference of
    /// its own), as Zend's macro leaves it.
    /// # Errors
    /// It is not callable; Zend has raised its parameter error.
    /// # Panics
    /// Argument `n` was not passed (the caller's arity check allows it).
    pub fn callable(&self, n: u32) -> Result<(zend_fcall_info, zend_fcall_info_cache), Invalid> {
        let (mut fci, mut fcc) = (crate::EMPTY_FCI, crate::EMPTY_FCC);
        let arg = self.arg_value(n).as_ptr();
        // SAFETY: passed argument `n`, live for the call, and two writable locals.
        if unsafe { swr_zend_parse_func(arg, n, &raw mut fci, &raw mut fcc) } != 0 {
            Ok((fci, fcc))
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

    /// `ZEND_THIS`: the zval holding `$this`, in a non-static method.
    #[must_use]
    #[inline]
    pub fn this_zval(&self) -> *mut zval {
        let frame: *mut zend_execute_data = self.frame;
        // SAFETY: a live frame; only the field's address is taken.
        unsafe { &raw mut (*frame).This }
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
        options: crate::sys::zend_long,
        depth: crate::sys::zend_long,
    ) -> Result<(), crate::bailout::Bailout> {
        let bytes = text.bytes();
        let end = window.end.min(bytes.len());
        let start = window.start.min(end);
        // SAFETY: the window's bytes, and the one after them, live in `text`
        // for the call; the return slot is live and Zend traps any bailout.
        let bailed = unsafe {
            swr_zend_try_json_decode_ex(
                self.return_value(),
                bytes.as_ptr().wrapping_add(start).cast(),
                end - start,
                options,
                depth,
            )
        };
        crate::bailout::trapped(bailed)
    }

    /// `RETVAL_STRINGL`: a copy of `bytes`.
    pub fn set_bytes(&self, bytes: &[u8]) {
        // SAFETY: the return slot is live; Zend copies the bytes.
        unsafe { swr_zval_stringl(self.return_value(), bytes.as_ptr().cast(), bytes.len()) };
    }
}
