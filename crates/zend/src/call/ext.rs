//! More of the call frame: optional and nullable parameters as `Option`s, and
//! byte-slice and copied return values.
use super::{Call, Expected, Invalid, swr_zend_try_parse_str_or_long};
use crate::{
    Function, ZStr,
    bailout::Bailout,
    sys::{zend_string, zval},
    zval::Zvals,
};
use std::ptr;

unsafe extern "C" {
    /// `zend_call_known_function(function, NULL, NULL, retval, args, NULL)`
    /// inside `zend_try` (nonzero for a bailout): a live function, called with
    /// the lent arguments, which Zend copies, writing its result to `retval`
    /// in place of what it held, which is not released.
    safe fn swr_zend_try_call_known(
        function: Function<'_>,
        retval: &crate::Value,
        args: Zvals<'_>,
    ) -> std::ffi::c_int;
    /// Forward the live frame and its return slot inside Zend's native trap.
    safe fn swr_zend_runtime_call_handler(
        handler: crate::sys::zif_handler,
        frame: crate::CallFrame<'_>,
        value: &crate::Value,
    ) -> std::ffi::c_int;

}

impl Call<'_> {
    /// `zend_call_known_function(function, NULL, NULL, return_value, argc,
    /// argv, NULL)` under `zend_try`, into this call's return slot.
    /// # Errors
    /// The function bailed out.
    pub fn call_known(&self, function: Function<'_>, argv: Zvals<'_>) -> Result<(), Bailout> {
        if swr_zend_try_call_known(function, self.ret_slot(), argv) != 0 {
            Err(Bailout)
        } else {
            Ok(())
        }
    }

    /// `handler(INTERNAL_FUNCTION_PARAM_PASSTHRU)` under `zend_try`: an internal
    /// function's handler runs over this call's frame and return slot, as
    /// upstream hands a call on to the handler it replaced.
    /// # Errors
    /// The handler bailed out.
    pub fn forward(&self, handler: crate::sys::zif_handler) -> Result<(), Bailout> {
        let bailed = swr_zend_runtime_call_handler(handler, self.call_frame(), self.ret_slot());
        if bailed != 0 { Err(Bailout) } else { Ok(()) }
    }

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
        self.parse_str(n, Expected::StringOrNull).map(Some)
    }

    /// `Z_PARAM_PATH` of argument `n`: as [`Call::string`], refusing NUL bytes.
    /// # Errors
    /// As `string`, or the path holds a NUL byte; Zend has thrown.
    #[inline]
    pub fn path<'a>(&'a self, n: u32, default: &'a [u8]) -> Result<&'a [u8], Invalid> {
        let path = self.string(n, default)?;
        if path.contains(&0) {
            return Err(Self::wrong(n, self.arg_value(n), Expected::Path));
        }
        Ok(path)
    }

    /// The frame Zend passed (`execute_data`), to pass on to another handler.
    #[must_use]
    #[inline]
    pub fn frame(&self) -> *mut crate::sys::zend_execute_data {
        self.frame
    }

    /// The frame's `run_time_cache`, which the frame's pusher may lend the
    /// function it calls, left null in its place: a pointer, not followed.
    #[must_use]
    pub fn take_run_time_cache(&self) -> *mut std::ffi::c_void {
        // The handler's live frame (`Call::new`).
        self.executing().take_run_time_cache()
    }

    /// `Z_PARAM_STR_OR_LONG` of argument `n`: `Ok(None)` when not passed,
    /// else the string (`Err` side of the inner result) or the int.
    /// # Errors
    /// The argument is neither under the caller's typing mode; Zend has
    /// thrown, or a `__toString` bailed out (deferred).
    #[inline]
    pub fn str_or_long(&self, n: u32) -> Result<Option<Result<i64, *mut zend_string>>, Invalid> {
        Ok(self
            .zstr_or_long(n)?
            .map(|parsed| parsed.map_err(ZStr::as_ptr)))
    }

    /// [`Call::str_or_long`], its string borrowed for the call: the frame
    /// keeps it alive (a coerced value replaces the argument).
    /// # Errors
    /// As [`Call::str_or_long`].
    #[inline]
    pub fn zstr_or_long(&self, n: u32) -> Result<Option<Result<i64, &ZStr>>, Invalid> {
        if !self.passed_arg(n) {
            return Ok(None);
        }
        let arg = self.arg_value(n);
        if let Some(string) = arg.str() {
            return Ok(Some(Err(string)));
        }
        if let Some(long) = arg.as_long() {
            return Ok(Some(Ok(long)));
        }
        let (mut string, mut long, mut parsed) = (ptr::null_mut(), 0, false);
        let bailed = swr_zend_try_parse_str_or_long(arg, &mut string, &mut long, n, &mut parsed);
        if let Err(bailout) = crate::bailout::trapped(bailed) {
            bailout.defer();
            return Err(Invalid);
        }
        if parsed {
            if string.is_null() {
                return Ok(Some(Ok(long)));
            }
            // Zend left the string it converted the argument to (`string`) in
            // the argument's slot, which keeps it for the call.
            if let Some(string) = arg.str() {
                return Ok(Some(Err(string)));
            }
        }
        Err(Self::wrong(n, arg, Expected::StringOrLong))
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

    /// `Z_PARAM_ARRAY_EX2(dest, 1, 1, 0)` of passed argument `n`: the array a
    /// by-reference argument refers to, with a reference of its own, or None
    /// for null.
    /// # Errors
    /// The argument is neither an array nor null; Zend has thrown.
    #[inline]
    pub fn array_or_null(&self, n: u32) -> Result<Option<crate::Owned>, Invalid> {
        let arg = self.arg_value(n).dereferenced();
        if arg.is_null() {
            return Ok(None);
        }
        if arg.array().is_some() {
            return Ok(Some(arg.into_owned()));
        }
        Err(Self::wrong(n, &arg, Expected::ArrayOrNull))
    }

    /// `Z_PARAM_OBJECT` of passed argument `n`.
    /// # Errors
    /// The argument is not an object; Zend has thrown.
    #[inline]
    pub fn object(&self, n: u32) -> Result<*mut zval, Invalid> {
        let arg = self.arg_value(n);
        if arg.dereferenced().object().is_some() {
            return Ok(arg.referent());
        }
        Err(Self::wrong(n, &arg.dereferenced(), Expected::Object))
    }
}
