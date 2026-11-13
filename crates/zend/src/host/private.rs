//! Shared object/property and callback custody operations for native PHP callers.
use crate::{
    Function, Owned, bailout,
    sys::{self, zend_fcall_info, zend_fcall_info_cache, zend_object, zval},
};
use std::{
    ffi::{c_char, c_int, c_long, c_void},
    ptr,
};

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
        std::mem::forget(value);
    } else {
        drop(value);
    }
}

/// # Safety
/// A resolved cache owns the references Zend added when resolving it; each release may execute PHP.
pub unsafe fn cache_free(cache: *mut zend_fcall_info_cache) {
    // SAFETY: read the initialized cache without keeping a Rust borrow across PHP.
    let object = unsafe { (*cache).object };
    if !object.is_null() {
        // SAFETY: release the bound object's reference inside Zend's C trap.
        bailout::defer(unsafe {
            bailout::trap(sys::swr_zend_facade_object_release, object.cast())
        });
    }
    if !bailout::pending() {
        // SAFETY: reread the function after releasing the object, as the
        // original does; a resolved cache holds a live function handler.
        let handler = unsafe { (*cache).function_handler };
        // SAFETY: per the caller, a resolved cache's live function.
        let function = unsafe { Function::from_ptr(handler) };
        if function.is_some_and(|function| function.flags() & sys::ZEND_ACC_CLOSURE != 0) {
            // SAFETY: the closure flag makes Zend's version-dependent closure
            // object projection valid.
            let closure = unsafe { sys::swr_zend_facade_closure(handler) };
            // SAFETY: release the separately held closure reference inside the C trap.
            bailout::defer(unsafe {
                bailout::trap(sys::swr_zend_facade_object_release, closure.cast())
            });
        }
    }
    bailout::finish();
}

/// `EG(exception)`: the exception pending on this thread, which runs PHP;
/// null for none. A read of the executor's slot: nothing is retained.
#[must_use]
pub fn exception() -> *mut zend_object {
    // SAFETY: the executor's globals, on the thread running PHP; one pointer is read.
    unsafe { sys::swr_zend_facade_exception() }
}

/// `zend_exception_error(EG(exception), E_ERROR)` of a pending exception,
/// inside the C trap, its bailout deferred: what a callback outside the VM
/// does about an exception nothing caught. Like [`exception`] it takes
/// nothing from its caller: it reads the executor's slot itself.
fn fatal_exception() {
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

unsafe fn direct_call(
    name: *mut zval,
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    report: unsafe extern "C" fn(*const c_char),
) -> c_int {
    let mut call = crate::EMPTY_FCI;
    // The call's result when the caller wants none; undefined otherwise.
    let temporary = Owned::default();
    // SAFETY: a non-null cache is initialized by the caller.
    let uncached = cache.is_null() || unsafe { (*cache).function_handler }.is_null();
    if uncached {
        if name.is_null() {
            // SAFETY: the native reporter executes entirely within Zend's C trap.
            bailout::defer(unsafe { bailout::report(report, c"Bad function") });
            return -1;
        }
        // SAFETY: a live function-name zval, borrowed through invocation.
        call.function_name = unsafe { name.read() };
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
/// Live function-name/cache/argument zvals and optional return storage, as Zend requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphelpers_call(
    name: *mut zval,
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    fatal: bool,
    report: unsafe extern "C" fn(*const c_char),
) -> c_int {
    // SAFETY: forwarding the caller's initialized arguments and native warning reporter.
    let result = unsafe { direct_call(name, cache, argc, argv, retval, report) };
    if fatal && !bailout::pending() {
        // Preserve the immediate fatal exception check for loop callbacks.
        fatal_exception();
    }
    bailout::finish();
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
#[expect(
    clippy::too_many_arguments,
    reason = "zend::function::call's arguments, its two leaves and the caller's work after it"
)]
pub unsafe fn function_call(
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    coroutine: bool,
    create: Create,
    report: unsafe extern "C" fn(*const c_char),
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
        unsafe { direct_call(ptr::null_mut(), cache, argc, argv, retval, report) == 0 }
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

/// `zend::function::call(name, argc, argv)` into `retval`, a bailout left
/// pending: the caller finishes or relays it.
/// # Safety
/// A borrowed binary function-name span, argument slots and writable owned return value.
pub unsafe fn named_call_into(
    name: *const c_char,
    length: usize,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
) {
    if bailout::pending() {
        return;
    }
    let function = Owned::default();
    // SAFETY: Zend copies the complete binary function name into its request string.
    unsafe { sys::swr_zval_stringl(function.as_ptr(), name, length) };
    let mut result = -1;
    // SAFETY: Zend's macro invocation is one leaf effect inside the C trap.
    bailed(unsafe {
        sys::swr_zend_try_facade_user_call(function.as_ptr(), argc, argv, retval, &raw mut result)
    });
    if bailout::pending() {
        // Upstream never ran past a bailout to release the name.
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
    let mut args: Vec<crate::Owned> = args.iter().map(|arg| arg.copy()).collect();
    #[expect(clippy::cast_possible_truncation, reason = "a handful of arguments")]
    let argc = args.len() as u32;
    let retval = Owned::default();
    // SAFETY: the name's bytes, `argc` live arguments (`Owned` is a
    // transparent zval) and a writable result.
    unsafe {
        named_call_into(
            name.as_ptr().cast(),
            name.len(),
            argc,
            args.as_mut_ptr().cast::<zval>(),
            retval.as_ptr(),
        );
    };
    // The result this call owns.
    discard(retval);
    if bailout::relay_to_c(Ok(())) {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}
