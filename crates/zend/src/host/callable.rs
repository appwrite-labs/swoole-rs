//! Long-lived callbacks own their zval, callable cache and diagnostic name together.
use crate::{
    Value, ZStr, ZString, bailout, kind,
    sys::{self, zend_fcall_info_cache, zval},
};
use std::{
    ffi::{CStr, CString, c_char},
    ptr,
    rc::Rc,
};

/// A C/C++ warning leaf, invoked below Zend's C bailout trap.
pub type Report = unsafe extern "C" fn(*const c_char);

/// A stable owner of the callback value, resolved cache and diagnostic string.
pub struct Callable {
    value: zval,
    cache: zend_fcall_info_cache,
    name: Option<ZString>,
}

fn deferred(bailed: i32) -> bool {
    if bailed != 0 {
        bailout::Bailout.defer();
        true
    } else {
        false
    }
}

impl Drop for Callable {
    fn drop(&mut self) {
        // SAFETY: this owner holds one live zval reference, initially undefined.
        bailout::defer(unsafe {
            bailout::trap(sys::swr_zend_zval_release, (&raw mut self.value).cast())
        });
        // The diagnostic string, an independent owned reference, goes next.
    }
}

impl Callable {
    /// `sw_callable_create(value)`: none, after a warning, when `value` is not
    /// callable. Dropping the last owner releases it, deferring a destructor's
    /// bailout.
    /// # Errors
    /// A native PHP leaf bailed out.
    pub fn new(value: &crate::Value) -> Result<Option<Rc<Self>>, bailout::Bailout> {
        Self::argument(Some(value), None, true, None)
    }

    /// `sw_callable_create` of the argument `name` (unnamed when `None`): none
    /// for a missing or null value when `allow_null`, else `exception` (or Zend's
    /// default) thrown;
    /// none, after a warning, when it is not callable.
    /// # Errors
    /// A native PHP leaf bailed out.
    pub fn argument(
        value: Option<&crate::Value>,
        name: Option<&CStr>,
        allow_null: bool,
        exception: Option<&crate::Class>,
    ) -> Result<Option<Rc<Self>>, bailout::Bailout> {
        let value = value.map_or(ptr::null_mut(), crate::Value::as_ptr);
        if value.is_null() && name.is_none() {
            return Ok(None);
        }
        // SAFETY: null (with a name) or a live candidate, a NUL-terminated name,
        // a registered class or null, and Swoole's C warning leaf.
        let handle = unsafe {
            factory(
                value,
                name.map_or(ptr::null(), CStr::as_ptr),
                allow_null,
                exception.map_or(ptr::null_mut(), crate::Class::entry),
                sys::swr_zend_callable_warning,
            )
        }?;
        if handle.is_null() {
            return Ok(None);
        }
        // SAFETY: `factory` gave up this Box.
        Ok(Some(Rc::from(unsafe { Box::from_raw(handle) })))
    }

    /// The resolved cache, for a call: Zend reads it (an unresolved call
    /// resolves into a local cache), so a shared owner may lend it.
    #[must_use]
    pub fn cache_ptr(&self) -> *mut zend_fcall_info_cache {
        ptr::from_ref(&self.cache).cast_mut()
    }

    /// `callable->call(args.len(), args, retval)` inside `zend_try`, or with
    /// `from_loop` `sw_zend_call_function_ex2`, which raises an uncaught
    /// exception at once as the event loop's callbacks do: whether the call
    /// succeeded. A `&Callable` is live for the whole call, so a slot PHP code
    /// can replace during it lends an `Rc` clone, never a borrow of the slot.
    /// # Errors
    /// The callable bailed out.
    pub fn call(
        &self,
        args: &mut [zval],
        retval: Option<&crate::Value>,
        from_loop: bool,
    ) -> Result<bool, bailout::Bailout> {
        let argc = u32::try_from(args.len()).unwrap_or(u32::MAX);
        let argv = args.as_mut_ptr();
        let retval = retval.map_or(ptr::null_mut(), crate::Value::as_ptr);
        let mut result = false;
        let body = || {
            result = if from_loop {
                // SAFETY: this live callable's cache, `argc` live arguments and a
                // writable return slot or null; the reporter is C.
                let called = unsafe {
                    super::swr_v1_phphelpers_call(
                        ptr::null_mut(),
                        self.cache_ptr(),
                        argc,
                        argv,
                        retval,
                        true,
                        sys::swr_zend_callable_warning,
                    )
                };
                called == 0
            } else {
                let called = self.invoke(argc, argv, retval);
                bailout::finish();
                called.unwrap_or(false)
            };
        };
        // SAFETY: the body owns nothing that needs a drop: it holds borrows,
        // copies and the result slot.
        unsafe { bailout::trap_with(body) }?;
        Ok(result)
    }

    /// `sw_zend_function_max_num_args` of the resolved function: unbounded for a
    /// variadic one, and 0 for none.
    #[must_use]
    pub fn max_num_args(&self) -> u32 {
        let function = self.cache.function_handler;
        if function.is_null() {
            return 0;
        }
        // SAFETY: the function the cache resolved, which this callable's zval keeps
        // live; every kind of `zend_function` starts with `common`.
        let common = unsafe { &*function.cast::<sys::_zend_function__bindgen_ty_1>() };
        if common.fn_flags & sys::ZEND_ACC_VARIADIC != 0 {
            u32::MAX
        } else {
            common.num_args
        }
    }

    /// `zend::Callable::call`: whether the call succeeded, a bailout left pending.
    fn invoke(
        &self,
        argc: u32,
        argv: *mut zval,
        retval: *mut zval,
    ) -> Result<bool, bailout::Bailout> {
        let mut fci = crate::EMPTY_FCI;
        let mut temporary = crate::UNDEF;
        fci.size = size_of::<sys::zend_fcall_info>();
        fci.retval = if retval.is_null() {
            &raw mut temporary
        } else {
            retval
        };
        fci.param_count = argc;
        fci.params = argv;
        if self.cache.function_handler.is_null() {
            // ZVAL_COPY_VALUE of the borrowed function name.
            fci.function_name = self.value;
        }
        // SAFETY: `call`'s live arguments and return storage, and this callable's
        // cache, which its zval keeps live; no Rust borrow spans PHP reentry.
        let result = unsafe { bailout::call(&raw mut fci, self.cache_ptr()) };
        let success = match result {
            Ok(success) => success,
            Err(error) => {
                error.defer();
                false
            }
        };
        if retval.is_null() && !bailout::pending() {
            // SAFETY: upstream destroys its temporary return value after the call.
            bailout::defer(unsafe {
                bailout::trap(sys::swr_zend_zval_release, (&raw mut temporary).cast())
            });
        }
        bailout::outcome(success)
    }
}

/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
/// # Safety
/// `report` is C/C++ and may report a PHP warning.
unsafe fn create(value: &Value, report: Report) -> Result<Box<Callable>, bailout::Bailout> {
    let mut state = Box::new(Callable {
        value: crate::UNDEF,
        cache: crate::EMPTY_FCC,
        name: None,
    });
    let mut truth = false;
    // SAFETY: a live candidate; the C shim traps any user-code bailout.
    if deferred(unsafe { sys::swr_zend_try_facade_truth(value.as_ptr(), &raw mut truth) }) {
        drop(state);
        return Err(bailout::Bailout);
    }
    if truth {
        let mut valid = false;
        // SAFETY: the cache is writable and the candidate is live.
        let failed = deferred(unsafe {
            sys::swr_zend_try_facade_callable(value.as_ptr(), &raw mut state.cache, &raw mut valid)
        });
        if !failed {
            let mut name = std::ptr::null_mut();
            // SAFETY: get the diagnostic name even for an invalid callable, as upstream does.
            deferred(unsafe { sys::swr_zend_try_facade_name(value.as_ptr(), &raw mut name) });
            // SAFETY: null (the shim bailed out), or the owned Zend string it produced.
            state.name = (!name.is_null()).then(|| unsafe { ZString::from_raw(name) });
        }
        if !bailout::pending() {
            if valid {
                // This new owner's initially undefined slot takes the successfully
                // resolved candidate, with a reference of its own.
                state.value = value.copy().into_zval();
            } else {
                let name = state.name.as_deref().map_or(&[][..], ZStr::bytes);
                let message = [b"function '".as_slice(), name, b"' is not callable"].concat();
                let text = message.split(|byte| *byte == 0).next().unwrap_or_default();
                let message = CString::new(text).unwrap_or_default();
                // SAFETY: the native warning reporter runs inside zend_try.
                bailout::defer(unsafe { bailout::report(report, &message) });
            }
        }
    } else {
        // SAFETY: the native reporter is called inside zend_try.
        bailout::defer(unsafe { bailout::report(report, c"illegal callback function") });
    }
    if bailout::pending() {
        drop(state);
        return Err(bailout::Bailout);
    }
    Ok(state)
}

/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
/// # Safety
/// Candidate, optional diagnostic argument name and registered exception class are live.
/// `report` is the native PHP warning leaf, never a Rust callback.
pub unsafe fn factory(
    value: *mut zval,
    name: *const c_char,
    allow_null: bool,
    exception: *mut sys::zend_class_entry,
    report: Report,
) -> Result<*mut Callable, bailout::Bailout> {
    // SAFETY: the caller supplies null or the NUL-terminated argument name.
    let name = (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) }.to_bytes());
    // SAFETY: null, or the caller's live candidate, borrowed for this call.
    let candidate = unsafe { Value::from_ptr(value) };
    if let Some(name) = name
        && candidate.is_none_or(Value::is_null)
    {
        if !allow_null {
            let message = [name, b" must be of type callable, null given"].concat();
            let message = CString::new(message).unwrap_or_default();
            // SAFETY: a registered exception class; Zend copies the message.
            deferred(unsafe { sys::swr_zend_try_facade_throw(exception, message.as_ptr(), 505) });
        }
        return bailout::outcome(ptr::null_mut());
    }
    // The caller's candidate; without a name it is never null.
    let Some(candidate) = candidate else {
        return bailout::outcome(ptr::null_mut());
    };
    // SAFETY: the native reporter satisfies callable_new's contract.
    let handle = unsafe { create(candidate, report) }?;
    // The owned value is initialized, even for a failed resolution.
    if kind(&handle.value) != sys::IS_UNDEF {
        return Ok(Box::into_raw(handle));
    }
    // Release the failed candidate as the original factory does.
    drop(handle);
    bailout::outcome(())?;
    if let Some(name) = name {
        // SAFETY: Zend returns a static type name for a live candidate.
        let kind = unsafe { sys::zend_zval_type_name(candidate.as_ptr()) };
        // SAFETY: Zend returned a static NUL-terminated type name.
        let kind = unsafe { CStr::from_ptr(kind) }.to_bytes();
        let message = [name, b" must be of type callable, ", kind, b" given"].concat();
        let message = CString::new(message).unwrap_or_default();
        // SAFETY: a registered exception class; Zend copies the message.
        deferred(unsafe { sys::swr_zend_try_facade_throw(exception, message.as_ptr(), 505) });
    }
    bailout::outcome(ptr::null_mut())
}
