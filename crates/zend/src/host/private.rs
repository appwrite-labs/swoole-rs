//! Shared object/property and callback custody operations for native PHP callers.
use crate::{
    Owned, bailout,
    sys::{self, zend_fcall_info_cache, zend_object, zval},
};
use std::{
    ffi::{CStr, c_int, c_long, c_void},
    ptr,
};

unsafe extern "C" {
    /// `call_user_function(EG(function_table), NULL, name, retval, args)`
    /// inside `zend_try` (nonzero for a bailout): the lent arguments, which
    /// Zend copies, and `result` written; `retval` holds the call's result in
    /// place of what it held, which is not released.
    safe fn swr_zend_try_facade_user_call(
        name: &crate::Value,
        args: crate::zval::Zvals<'_>,
        retval: &Owned,
        result: &mut c_int,
    ) -> c_int;
    /// `EG(exception)`: one pointer of the executor's is read; null for none.
    safe fn swr_zend_facade_exception() -> *mut zend_object;
    /// Inspect the pending exception. Leave `UnwindExit` in the executor and
    /// report other exceptions inside `zend_try`.
    safe fn swr_zend_try_uncaught_exception(unwind_exit: &mut bool) -> c_int;
    /// `sw_callable_warning(message)` inside `zend_try` (nonzero for a
    /// bailout): it records the error and raises the warning, whose user
    /// handler runs PHP code.
    safe fn swr_zend_try_callable_warning(message: crate::helpers::CText<'_>) -> c_int;
    /// `*result = create(cache, argc, argv, callable, &raised)` inside
    /// `zend_try`, then `then(arg)` unless an exception is pending (`*ran`
    /// set): nonzero for a bailout. `raised` is the shim's own local.
    fn swr_zend_try_facade_coroutine(
        create: Option<Create>,
        cache: *mut zend_fcall_info_cache,
        argc: u32,
        argv: *mut zval,
        callable: *mut zval,
        result: *mut c_long,
        then: sys::swr_zend_then,
        arg: *mut c_void,
        ran: *mut bool,
    ) -> c_int;
}

fn bailed(code: c_int) -> bool {
    if code != 0 {
        bailout::Bailout.defer();
        true
    } else {
        false
    }
}

/// Destroy `value`, a call's private result or name, unless a bailout is
/// pending: upstream never ran past one to destroy it, so it is leaked. Its
/// destructor runs inside the C trap ([`Owned`]'s drop).
fn discard(value: Owned) {
    if bailout::pending() {
        #[expect(
            clippy::mem_forget,
            reason = "upstream never ran past a bailout to destroy it"
        )]
        std::mem::forget(value);
    } else {
        drop(value);
    }
}

/// `EG(exception)`: the exception pending on this thread, which runs PHP;
/// null for none. A read of the executor's slot: nothing is retained.
#[must_use]
pub fn exception() -> *mut zend_object {
    swr_zend_facade_exception()
}

/// `EG(exception)` with a reference of its own: the exception pending on this
/// thread, or None for none.
#[must_use]
pub fn exception_owned() -> Option<Owned> {
    let object = exception();
    if object.is_null() {
        return None;
    }
    // The executor's pending exception, which it holds while this copies it:
    // nothing runs in between.
    let mut value = crate::ObjectRef(object, std::marker::PhantomData).copy_value();
    // `ZVAL_COPY` takes the reference.
    Some(crate::Value::of(&mut value).copy())
}

/// Finish an uncaught callback exception. `UnwindExit` stops the reactor and
/// stays pending for the VM. Other exceptions go through Zend’s fatal report.
/// # Errors
/// The report bailed out, as an `E_ERROR` does.
pub fn uncaught_exception() -> Result<(), bailout::Bailout> {
    let mut unwind_exit = false;
    bailout::trapped(swr_zend_try_uncaught_exception(&mut unwind_exit))?;
    if unwind_exit {
        // Leave the exit object with Zend and stop dispatching callbacks while
        // the native loop releases its resources and returns to the VM.
        swoole_rs_corebase::reactor::event_api::swoole_event_bail_out();
    }
    Ok(())
}

/// [`uncaught_exception`], its bailout deferred.
pub(super) fn fatal_exception() {
    bailout::defer(uncaught_exception());
}

/// `sw_callable_warning(message)` inside `zend_try`, the bailout of a user
/// error handler deferred. Nothing is reported while a bailout is pending.
pub(super) fn callable_warning(message: &CStr) {
    let reported = if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        bailout::trapped(swr_zend_try_callable_warning(message.into()))
    };
    bailout::defer(reported);
}

/// `zend::function::call(cache, argc, argv, retval)` into `retval` (a
/// private slot for `None`): whether the call ran; false when it failed or
/// bailed out (the bailout deferred). An unresolved cache is reported as a bad
/// function.
pub(super) fn direct_call(
    callee: Callee<'_>,
    args: crate::zval::Zvals<'_>,
    retval: Option<&crate::Value>,
) -> bool {
    // The call's result when the caller wants none; undefined otherwise.
    let temporary = Owned::default();
    if !callee.resolved {
        callable_warning(c"Bad function");
        return false;
    }
    // No Rust borrow spans the call.
    let result = bailout::call(callee, args, None, retval.unwrap_or(&temporary));
    let result = match result {
        Ok(ran) => ran,
        Err(error) => {
            error.defer();
            false
        }
    };
    // Discard the private return slot after invocation, as upstream does.
    discard(temporary);
    result
}

/// crates/phpcoroutines' `create_coroutine`: the new cid, with its [`Trap`] set
/// when activation bailed out. The shim passes the call's live cache (resolved
/// or not), which Zend leaves alone while it runs, its arguments and its
/// callable zval (null from [`function_call`]).
pub type Create =
    extern "C" fn(Option<crate::Fcc<'_>>, u32, *mut zval, *mut zval, Trap<'_>) -> c_long;

/// The coroutine call's own bailout flag, which the zend-sys shim lends its
/// [`Create`] leaf inside the call's `zend_try`, on the thread running PHP.
/// Rust never makes one: it is the leaf's parameter, which the shim's call
/// stands for, so only the shim calls a leaf.
#[repr(transparent)]
pub struct Trap<'a>(&'a mut u8);

impl<'a> Trap<'a> {
    /// Record whether the leaf's own work bailed out.
    pub fn set(&mut self, bailed: bool) {
        *self.0 = u8::from(bailed);
    }

    /// The lease on running PHP the shim's call holds for the leaf.
    #[must_use]
    pub fn php(&self) -> crate::Php<'a> {
        crate::Php(std::marker::PhantomData)
    }
}

/// The coroutine leaf of [`function_call`].
#[derive(Clone, Copy)]
pub struct Creator(Create);

impl Creator {
    #[must_use]
    pub const fn new(create: Create) -> Self {
        Self(create)
    }
}

/// The caller's work after a coroutine call, `body(arg)` (`None`: none), done
/// inside the call's own `zend_try`, which saves it a setjmp (#3260). It runs
/// where the caller's code after [`function_call`] would: once the coroutine
/// runs or waits and no exception is pending, so the call's exception check
/// stays before it. A direct call, or one that did not run, leaves it undone,
/// so `body` records in `arg` that it began. Only [`Then::new`] makes one,
/// besides [`NO_THEN`].
#[derive(Clone, Copy)]
pub struct Then(sys::swr_zend_then, *mut c_void);

impl Then {
    /// # Safety
    /// `body(arg)` (none: nothing) may run, inside a `zend_try`, during any [`function_call`]
    /// given this: `arg` stays what `body` takes until each returns. As
    /// [`crate::contain_with`]'s body, `body`'s frame owns nothing that needs a drop.
    #[must_use]
    pub const unsafe fn new(body: sys::swr_zend_then, arg: *mut c_void) -> Self {
        Self(body, arg)
    }
}

/// The resolved (or empty) callable cache of a callable that lives for `'a`,
/// lent for a call: made only from a callable that owns it.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct CalleePtr<'a>(
    pub(crate) *mut zend_fcall_info_cache,
    pub(crate) std::marker::PhantomData<&'a zend_fcall_info_cache>,
);

/// A [`CalleePtr`] lent for a call, with whether its owner saw it name a function
/// when it lent it. Zend handles a cache without one itself (it resolves the
/// call's name, or fails), so the flag only picks upstream's warning.
#[derive(Clone, Copy)]
pub struct Callee<'a> {
    pub(crate) cache: CalleePtr<'a>,
    pub(crate) resolved: bool,
}

/// Invoke directly or create a coroutine, preserving the immediate fatal-exception check;
/// in a coroutine, then `then`. The call writes its result into `retval`, in place of
/// what it held (a coroutine call writes null), which is not released.
/// # Errors
/// PHP bailed out, in the call or in `then`; no jump is resumed while a Rust caller owns its state.
pub fn function_call(
    cache: Callee<'_>,
    args: crate::zval::Zvals<'_>,
    retval: Option<&crate::Value>,
    coroutine: bool,
    creator: Creator,
    then: Then,
) -> Result<bool, bailout::Bailout> {
    // Upstream never ran past a pending bailout: no coroutine starts, as no call runs.
    bailout::outcome(())?;
    let mut ran = false;
    let success = if coroutine {
        if let Some(retval) = retval {
            // The caller's return slot, whose type alone this writes:
            // coroutines publish null before creation and have no return value.
            retval.retype_null();
        }
        let (trapped, result) = coroutine_call(cache, args, None, creator, then, &mut ran);
        bailed(trapped);
        result >= 0
    } else {
        direct_call(cache, args, retval)
    };
    // `then` ran with no exception pending, as this check would have found it.
    if !ran && !bailout::pending() {
        // Callbacks outside the VM must report an exception immediately.
        fatal_exception();
    }
    if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        Ok(success)
    }
}

/// `create(cache, args, callable)`, then `then` once the coroutine runs or
/// waits with no exception pending (`ran` set), inside one `zend_try`: the
/// trap's flag (nonzero for a bailout, or a leaf that raised) and the leaf's
/// result, the new coroutine's id or a negative error.
#[inline]
pub(crate) fn coroutine_call(
    cache: Callee<'_>,
    args: crate::zval::Zvals<'_>,
    callable: Option<&crate::Value>,
    Creator(create): Creator,
    Then(then, arg): Then,
    ran: &mut bool,
) -> (c_int, c_long) {
    let mut result: c_long = -1;
    // SAFETY: the leaf (`Creator`) and `then` (`Then`) are as their makers
    // vouched, with a callable's cache, lent arguments and the callable's
    // live zval; they run inside the C trap, and no Rust borrow spans a switch.
    let trapped = unsafe {
        swr_zend_try_facade_coroutine(
            Some(create),
            cache.cache.0,
            args.argc(),
            args.argv(),
            callable.map_or(ptr::null_mut(), crate::Value::as_ptr),
            &raw mut result,
            then,
            arg,
            ran,
        )
    };
    (trapped, result)
}

/// No [`Then`].
pub const NO_THEN: Then = Then(None, ptr::null_mut());

/// `call_user_function(EG(function_table), NULL, name, retval, args)` inside
/// `zend_try`: `zend_call_function`'s result, 0 when the call ran. `retval`
/// holds what it returned, in place of what it held, which is not released.
/// # Errors
/// The call bailed out.
pub(crate) fn user_call(
    name: &crate::Value,
    args: crate::zval::Zvals<'_>,
    retval: &Owned,
) -> Result<c_int, bailout::Bailout> {
    let mut result = -1;
    bailout::trapped(swr_zend_try_facade_user_call(
        name,
        args,
        retval,
        &mut result,
    ))?;
    Ok(result)
}

/// `zend::function::call(name, args)` into `retval`, an undefined slot of the
/// caller's, a bailout left pending: the caller finishes or relays it. The
/// arguments are lent as they are; Zend copies them into the call.
pub fn named_call_into(name: &[u8], args: &[Owned], retval: &Owned) {
    if bailout::pending() {
        return;
    }
    // ZVAL_STRINGL: the complete binary function name, in a request string.
    let function = Owned::string(crate::ZString::new(name));
    let result =
        user_call(&function, crate::zval::Zvals::owned(args), retval).unwrap_or_else(|bailout| {
            bailout.defer();
            -1
        });
    if bailout::pending() {
        // Upstream never ran past a bailout to release the name.
        #[expect(
            clippy::mem_forget,
            reason = "upstream never ran past a bailout to release it"
        )]
        std::mem::forget(function);
    } else {
        if result != 0 {
            // A failed named invocation publishes null before name cleanup, in
            // the slot the failed call left undefined.
            retval.lend_slot(|value| *value = Owned::null());
        }
        // The local function-name string owns one reference.
        drop(function);
        if !bailout::pending() {
            // Preserve the fatal exception check after function-name cleanup.
            fatal_exception();
        }
    }
}

/// `zend::function::call(name, args)`, its result discarded as upstream's
/// temporary `Variable` is. Each argument goes with a reference of its own
/// ([`crate::Value::copy`]), so PHP code the call runs can't free one under it.
/// # Errors
/// PHP bailed out; nothing is resumed while a Rust caller owns its state.
pub fn named_call(name: &[u8], args: &[&crate::Value]) -> Result<(), bailout::Bailout> {
    let args: Vec<crate::Owned> = args.iter().map(|arg| arg.copy()).collect();
    let retval = Owned::default();
    named_call_into(name, &args, &retval);
    // The result this call owns.
    discard(retval);
    if bailout::relay_to_c(Ok(())) {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}
