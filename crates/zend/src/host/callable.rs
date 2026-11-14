//! Long-lived callbacks own their zval, callable cache and diagnostic name together.
use crate::{
    Function, Owned, Value, ZStr, ZString, bailout,
    sys::{self, zend_fcall_info_cache, zval},
};
use std::{
    ffi::{CStr, CString},
    ptr,
    rc::Rc,
};

/// A stable owner of the callback value, resolved cache and diagnostic string.
/// Dropping it releases the value first (a destructor's bailout is deferred),
/// then the diagnostic string, an independent owned reference.
pub struct Callable {
    /// Undefined until a candidate resolves.
    value: Owned,
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
        if value.is_none() && name.is_none() {
            return Ok(None);
        }
        let handle = factory(value, name.map(CStr::to_bytes), allow_null, exception)?;
        Ok(handle.map(Rc::from))
    }

    /// The function the cache resolved, which this callable's value keeps live;
    /// None for none.
    fn function(&self) -> Option<Function<'_>> {
        // SAFETY: null, or the function the cache resolved, which this
        // callable's value keeps live while it is borrowed.
        unsafe { Function::from_ptr(self.cache.function_handler) }
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
                // writable return slot or null.
                let called =
                    unsafe { super::private::direct_call(self.cache_ptr(), argc, argv, retval) };
                if !bailout::pending() {
                    // A loop callback reports an uncaught exception at once.
                    super::private::fatal_exception();
                }
                bailout::finish();
                called == 0
            } else {
                let called = self.invoke(argc, argv, retval);
                bailout::finish();
                called.unwrap_or(false)
            };
        };
        // The body holds borrows, copies and the result slot.
        crate::contain_with(body)?;
        Ok(result)
    }

    /// `sw_zend_function_max_num_args` of the resolved function: unbounded for a
    /// variadic one, and 0 for none.
    #[must_use]
    pub fn max_num_args(&self) -> u32 {
        let Some(function) = self.function() else {
            return 0;
        };
        if function.flags() & sys::ZEND_ACC_VARIADIC != 0 {
            u32::MAX
        } else {
            function.num_args()
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
        // The call's result when the caller wants none; undefined otherwise.
        let temporary = Owned::default();
        fci.size = size_of::<sys::zend_fcall_info>();
        fci.retval = if retval.is_null() {
            temporary.as_ptr()
        } else {
            retval
        };
        fci.param_count = argc;
        fci.params = argv;
        if self.cache.function_handler.is_null() {
            // ZVAL_COPY_VALUE of the borrowed function name.
            fci.function_name = self.value.copy_value();
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
        if bailout::pending() {
            // Upstream never ran past a bailout to destroy its temporary.
            #[expect(
                clippy::mem_forget,
                reason = "upstream never ran past a bailout to destroy it"
            )]
            std::mem::forget(temporary);
        } else {
            // Upstream destroys its temporary return value after the call.
            drop(temporary);
        }
        bailout::outcome(success)
    }
}

/// `sw_callable_create`'s owner of `value`, whose own value stays undefined
/// when `value` is not callable (after a warning).
/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
fn create(value: &Value) -> Result<Box<Callable>, bailout::Bailout> {
    let warn = super::private::callable_warning;
    let mut state = Box::new(Callable {
        value: Owned::default(),
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
                state.value = value.copy();
            } else {
                let name = state.name.as_deref().map_or(&[][..], ZStr::bytes);
                let message = [b"function '".as_slice(), name, b"' is not callable"].concat();
                let text = message.split(|byte| *byte == 0).next().unwrap_or_default();
                warn(&CString::new(text).unwrap_or_default());
            }
        }
    } else {
        warn(c"illegal callback function");
    }
    if bailout::pending() {
        drop(state);
        return Err(bailout::Bailout);
    }
    Ok(state)
}

/// `sw_callable_create` of `candidate`, the argument `name` (unnamed when
/// `None`): see [`Callable::argument`].
/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
fn factory(
    candidate: Option<&Value>,
    name: Option<&[u8]>,
    allow_null: bool,
    exception: Option<&crate::Class>,
) -> Result<Option<Box<Callable>>, bailout::Bailout> {
    let throw = |message: Vec<u8>| {
        let message = CString::new(message).unwrap_or_default();
        bailout::defer(crate::error::throw_trapped(exception, &message, 505));
    };
    if let Some(name) = name
        && candidate.is_none_or(Value::is_null)
    {
        if !allow_null {
            throw([name, b" must be of type callable, null given"].concat());
        }
        return bailout::outcome(None);
    }
    // The caller's candidate; without a name it is never null.
    let Some(candidate) = candidate else {
        return bailout::outcome(None);
    };
    let handle = create(candidate)?;
    // The owned value is initialized, even for a failed resolution.
    if handle.value.kind() != sys::IS_UNDEF {
        return Ok(Some(handle));
    }
    // Release the failed candidate as the original factory does.
    drop(handle);
    bailout::outcome(())?;
    if let Some(name) = name {
        let kind = candidate.type_name().to_bytes();
        throw([name, b" must be of type callable, ", kind, b" given"].concat());
    }
    bailout::outcome(None)
}
