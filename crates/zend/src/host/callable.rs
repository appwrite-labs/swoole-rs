//! Long-lived callbacks own their zval, callable cache and diagnostic name together.
use crate::{
    Function, Owned, Value, ZStr, ZString, bailout,
    sys::{self, zend_fcall_info_cache},
};
use std::{
    ffi::{CStr, CString, c_int, c_long},
    marker::PhantomData,
    ptr,
    rc::Rc,
};

// `sw_callable_create`'s steps over a candidate value, each inside `zend_try`
// (nonzero for a bailout): user code (`__toString`, `__invoke` lookups, an
// autoloader) may run.
unsafe extern "C" {
    /// `zend_is_true(value)` into `truth`.
    safe fn swr_zend_try_facade_truth(value: &Value, truth: &mut bool) -> c_int;
    /// `zend_is_callable_at_frame(value, ...)` into `valid`, resolving `cache`.
    safe fn swr_zend_try_facade_callable(
        value: &Value,
        cache: &mut zend_fcall_info_cache,
        valid: &mut bool,
    ) -> c_int;
    /// `zend_get_callable_name_ex(value)` into `name`, in place of what it
    /// held, which is not released.
    safe fn swr_zend_try_facade_name(value: &Value, name: &Owned) -> c_int;
    /// `zend_is_callable_at_frame(value, $this, frame, 0, cache, NULL)` into
    /// `valid` (`$this`: the frame's object, if any), resolving `cache`.
    safe fn swr_zend_try_facade_callable_frame(
        value: &Value,
        frame: crate::CallFrame<'_>,
        cache: &mut zend_fcall_info_cache,
        valid: &mut bool,
    ) -> c_int;
}

/// A callable resolved in the scope of a running call
/// (`sw_zend_is_callable_at_frame`), so it can name a private method of the
/// call's `$this`, which its cache borrows: it lives no longer than the call.
pub struct FrameCallable<'a> {
    value: Owned,
    cache: zend_fcall_info_cache,
    frame: PhantomData<crate::CallFrame<'a>>,
}

impl<'a> FrameCallable<'a> {
    /// `value` resolved at `frame`; None when it is not callable there.
    /// # Errors
    /// Resolving it bailed out (an autoloader runs PHP code).
    pub fn resolve(
        value: Owned,
        frame: crate::CallFrame<'a>,
    ) -> Result<Option<Self>, bailout::Bailout> {
        let mut cache = crate::EMPTY_FCC;
        let mut valid = false;
        bailout::trapped(swr_zend_try_facade_callable_frame(
            &value, frame, &mut cache, &mut valid,
        ))?;
        Ok(valid.then_some(Self {
            value,
            cache,
            frame: PhantomData,
        }))
    }

    /// `PHPCoroutine::create(&cache, args, &value)` through `creator`, inside
    /// `zend_try`: the new coroutine's id, or a negative error.
    /// # Errors
    /// The call bailed out, or the leaf raised.
    pub fn create_coroutine(
        &self,
        args: crate::Zvals<'_>,
        creator: super::Creator,
    ) -> Result<c_long, bailout::Bailout> {
        // Zend only reads the resolved cache.
        let cache = super::Callee(ptr::from_ref(&self.cache).cast_mut(), PhantomData);
        let mut ran = false;
        let (trapped, result) = super::private::coroutine_call(
            cache,
            args,
            Some(&self.value),
            creator,
            super::NO_THEN,
            &mut ran,
        );
        bailout::trapped(trapped).map(|()| result)
    }
}

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
        let function = self.cache.function_handler;
        // The function the cache resolved, which this callable's value keeps
        // live while it is borrowed.
        (!function.is_null()).then_some(Function(function, std::marker::PhantomData))
    }

    /// The resolved cache, for a call: Zend reads it (an unresolved call
    /// resolves into a local cache), so a shared owner may lend it.
    #[must_use]
    pub fn cache_ptr(&self) -> *mut zend_fcall_info_cache {
        ptr::from_ref(&self.cache).cast_mut()
    }

    /// The cache, lent for [`super::function_call`].
    #[must_use]
    #[inline]
    pub fn cache(&self) -> super::Callee<'_> {
        super::Callee(self.cache_ptr(), std::marker::PhantomData)
    }

    /// `callable->call(args.len(), args, retval)` inside `zend_try`, or with
    /// `from_loop` `sw_zend_call_function_ex2`, which raises an uncaught
    /// exception at once as the event loop's callbacks do: whether the call
    /// succeeded. A `&Callable` is live for the whole call, so a slot PHP code
    /// can replace during it lends an `Rc` clone, never a borrow of the slot.
    /// # Errors
    /// The callable bailed out.
    ///
    /// ```compile_fail
    /// use swoole_rs_zend::{host::callable::Callable, UNDEF};
    /// fn call_raw(callable: &Callable) {
    ///     let _ = callable.call(&mut [UNDEF], None, false);
    /// }
    /// ```
    pub fn call(
        &self,
        args: crate::Zvals<'_>,
        retval: Option<&crate::Value>,
        from_loop: bool,
    ) -> Result<bool, bailout::Bailout> {
        let mut result = false;
        let body = || {
            result = if from_loop {
                let called = super::private::direct_call(self.cache(), args, retval);
                if !bailout::pending() {
                    // A loop callback reports an uncaught exception at once.
                    super::private::fatal_exception();
                }
                bailout::finish();
                called
            } else {
                let called = self.invoke(args, retval);
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
        args: crate::Zvals<'_>,
        retval: Option<&crate::Value>,
    ) -> Result<bool, bailout::Bailout> {
        // The call's result when the caller wants none; undefined otherwise.
        let temporary = Owned::default();
        // An unresolved cache resolves the borrowed function name.
        let name = self
            .cache
            .function_handler
            .is_null()
            .then_some(&*self.value);
        // No Rust borrow spans PHP reentry.
        let result = bailout::call(self.cache(), args, name, retval.unwrap_or(&temporary));
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
    if deferred(swr_zend_try_facade_truth(value, &mut truth)) {
        drop(state);
        return Err(bailout::Bailout);
    }
    if truth {
        let mut valid = false;
        let failed = deferred(swr_zend_try_facade_callable(
            value,
            &mut state.cache,
            &mut valid,
        ));
        if !failed {
            // The diagnostic name, even for an invalid callable, as upstream gets it.
            let name = Owned::default();
            deferred(swr_zend_try_facade_name(value, &name));
            state.name = name.str().map(ZStr::to_owned);
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
