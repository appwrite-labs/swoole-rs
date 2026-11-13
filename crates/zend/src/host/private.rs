//! Shared object/property and callback custody operations for native PHP callers.
use crate::{
    Owned, bailout,
    sys::{self, zend_fcall_info, zend_fcall_info_cache, zend_object, zval},
};
use std::{
    ffi::{CStr, c_int, c_long, c_void},
    ptr,
};

unsafe extern "C" {
    /// `EG(exception)`: one pointer of the executor's is read; null for none.
    safe fn swr_zend_facade_exception() -> *mut zend_object;
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

/// `zend_exception_error(EG(exception), E_ERROR)` of a pending exception,
/// inside the C trap, its bailout deferred: what a callback outside the VM
/// does about an exception nothing caught. Like [`exception`] it takes
/// nothing from its caller: it reads the executor's slot itself.
pub(super) fn fatal_exception() {
    // Observed only after the preceding call returned.
    let exception = exception();
    if !exception.is_null() {
        // SAFETY: the exception the executor holds, live until Zend clears
        // it; the reporter is C and runs inside the C trap, so a bailout
        // crosses no Rust frame.
        bailout::defer(unsafe {
            bailout::trap(sys::swr_zend_facade_exception_error, exception.cast())
        });
    }
}

/// `sw_callable_warning(message)` inside `zend_try`, the bailout of a user
/// error handler deferred. Nothing is reported while a bailout is pending.
pub(super) fn callable_warning(message: &CStr) {
    let reported = if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        // SAFETY: Swoole's C warning leaf, which records the error and raises
        // the warning with the NUL-terminated message; the shim's setjmp sits
        // between this frame and it.
        bailout::trapped(unsafe {
            sys::swr_zend_try_report(Some(sys::swr_zend_callable_warning), message.as_ptr())
        })
    };
    bailout::defer(reported);
}

/// `zend::function::call(cache, argc, argv, retval)`: 0 when the call ran, -1
/// when it failed or bailed out (the bailout deferred). An unresolved cache is
/// reported as a bad function.
/// # Safety
/// A null or initialised cache, `argc` live arguments, and a writable return
/// slot or null.
pub(super) unsafe fn direct_call(
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
) -> c_int {
    let mut call = crate::EMPTY_FCI;
    // The call's result when the caller wants none; undefined otherwise.
    let temporary = Owned::default();
    // SAFETY: a non-null cache is initialized by the caller.
    if cache.is_null() || unsafe { (*cache).function_handler }.is_null() {
        callable_warning(c"Bad function");
        return -1;
    }
    call.size = size_of::<zend_fcall_info>();
    call.retval = if retval.is_null() {
        temporary.as_ptr()
    } else {
        retval
    };
    call.param_count = argc;
    call.params = argv;
    // SAFETY: initialized frame/cache/arguments; no Rust borrow spans invocation.
    let result = unsafe { bailout::call(&raw mut call, cache) };
    let result = match result {
        Ok(true) => 0,
        Ok(false) => -1,
        Err(error) => {
            error.defer();
            -1
        }
    };
    // Discard the private return slot after invocation, as upstream does.
    discard(temporary);
    result
}

/// # Safety
/// Live callable frame/cache; caller accepts the frame's upstream temporary-return convention.
pub unsafe fn call_anyway(frame: *mut zend_fcall_info, cache: *mut zend_fcall_info_cache) -> c_int {
    let temporary = Owned::default();
    // SAFETY: the frame is initialized and exclusively configured before invocation.
    if unsafe { (*frame).retval }.is_null() {
        // SAFETY: the temporary remains live through the call and destruction below.
        unsafe { (*frame).retval = temporary.as_ptr() };
    }
    // SAFETY: Zend executes the configured frame entirely inside the C trap.
    let result = unsafe { bailout::call(frame, cache) };
    let result = match result {
        Ok(true) => 0,
        Ok(false) => -1,
        Err(error) => {
            error.defer();
            -1
        }
    };
    // The private temporary owns any returned payload; it is still undefined
    // when the frame came with a return slot of its own.
    discard(temporary);
    bailout::finish();
    result
}

/// crates/phpcoroutines' `create_coroutine`: the new cid, with 1 in its last
/// argument when activation bailed out.
pub type Create =
    unsafe extern "C" fn(*mut zend_fcall_info_cache, u32, *mut zval, *mut zval, *mut u8) -> c_long;

/// The caller's work after a coroutine call, `body(arg)` (`None`: none), done
/// inside the call's own `zend_try`, which saves it a setjmp (#3260). It runs
/// where the caller's code after [`function_call`] would: once the coroutine
/// runs or waits and no exception is pending, so the call's exception check
/// stays before it. A direct call, or one that did not run, leaves it undone,
/// so `body` records in `arg` that it began. As [`bailout::trap`]'s body,
/// `body`'s frame owns nothing that needs a drop.
pub type Then = (sys::swr_zend_then, *mut c_void);

/// Invoke directly or create a coroutine, preserving the immediate fatal-exception check;
/// in a coroutine, then `then`.
/// # Errors
/// PHP bailed out, in the call or in `then`; no jump is resumed while a Rust caller owns its state.
/// # Safety
/// A live resolved cache, argument span and optional return slot. `create` is the coroutine leaf,
/// and `then` may be called.
pub unsafe fn function_call(
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    coroutine: bool,
    create: Create,
    (then, arg): Then,
) -> Result<bool, bailout::Bailout> {
    // Upstream never ran past a pending bailout: no coroutine starts, as no call runs.
    bailout::outcome(())?;
    let mut ran = false;
    let success = if coroutine {
        if !retval.is_null() {
            // SAFETY: coroutines publish null before creation and have no return value.
            unsafe {
                (*retval).u1 = sys::_zval_struct__bindgen_ty_1 {
                    type_info: sys::IS_NULL,
                }
            };
        }
        let mut result: c_long = -1;
        // SAFETY: the native coroutine leaf runs inside the C trap; no Rust borrow spans a switch.
        bailed(unsafe {
            sys::swr_zend_try_facade_coroutine(
                Some(create),
                cache,
                argc,
                argv,
                ptr::null_mut(),
                &raw mut result,
                then,
                arg,
                &raw mut ran,
            )
        });
        result >= 0
    } else {
        // SAFETY: the caller lends a resolved cache and live arguments.
        unsafe { direct_call(cache, argc, argv, retval) == 0 }
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

/// No [`Then`].
pub const NO_THEN: Then = (None, ptr::null_mut());

/// `zend::function::call(name, args)` into `retval`, an undefined slot of the
/// caller's, a bailout left pending: the caller finishes or relays it. The
/// arguments are lent as they are; Zend copies them into the call.
pub fn named_call_into(name: &[u8], args: &[Owned], retval: &Owned) {
    if bailout::pending() {
        return;
    }
    // ZVAL_STRINGL: the complete binary function name, in a request string.
    let function = Owned::string(crate::ZString::new(name));
    // Never more than the slice holds.
    let argc = u32::try_from(args.len()).unwrap_or(0);
    let argv = args.as_ptr().cast_mut().cast::<zval>();
    let retval = retval.as_ptr();
    let mut result = -1;
    // SAFETY: a live name, `argc` live arguments (`Owned` is a transparent
    // zval) and a writable result; Zend's macro invocation is one leaf effect
    // inside the C trap.
    bailed(unsafe {
        sys::swr_zend_try_facade_user_call(function.as_ptr(), argc, argv, retval, &raw mut result)
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
            // SAFETY: failed named invocation publishes null before name cleanup.
            unsafe {
                (*retval).u1 = sys::_zval_struct__bindgen_ty_1 {
                    type_info: sys::IS_NULL,
                }
            };
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
