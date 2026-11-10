//! Long-lived callbacks own their zval, callable cache and diagnostic name together.
use std::{
    ffi::{CStr, CString, c_char},
    ptr,
    rc::Rc,
};
use swoole_rs_zend::{
    bailout, string,
    sys::{self, zend_fcall_info_cache, zend_string, zval},
    type_of,
};

/// A C/C++ warning leaf, invoked below Zend's C bailout trap.
pub type Report = unsafe extern "C" fn(*const c_char);

/// A stable owner of the callback value, resolved cache and diagnostic string.
pub struct Callable {
    value: zval,
    cache: zend_fcall_info_cache,
    name: *mut zend_string,
}

fn outcome<T>(value: T) -> Result<T, bailout::Bailout> {
    if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        Ok(value)
    }
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
        if !self.name.is_null() {
            // SAFETY: the diagnostic string is an independent owned reference.
            unsafe { sys::swr_zend_string_release(self.name) };
        }
    }
}

impl Callable {
    /// `sw_callable_create(value)`: none, after a warning, when `value` is not
    /// callable. Dropping the last owner releases it, deferring a destructor's
    /// bailout.
    /// # Errors
    /// A native PHP leaf bailed out.
    pub fn new(value: &swoole_rs_zend::Value) -> Result<Option<Rc<Self>>, bailout::Bailout> {
        // SAFETY: a live candidate, and Swoole's C warning leaf.
        let handle = unsafe {
            factory(
                value.as_ptr(),
                ptr::null(),
                true,
                ptr::null_mut(),
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
}

/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
/// # Safety
/// `value` is a live callable candidate. `report` is C/C++ and may report a PHP warning.
pub unsafe fn create(value: *mut zval, report: Report) -> Result<*mut Callable, bailout::Bailout> {
    let mut state = Box::new(Callable {
        value: swoole_rs_zend::UNDEF,
        cache: swoole_rs_zend::EMPTY_FCC,
        name: ptr::null_mut(),
    });
    let mut truth = false;
    // SAFETY: a live candidate; the C shim traps any user-code bailout.
    if deferred(unsafe { sys::swr_zend_try_facade_truth(value, &raw mut truth) }) {
        drop(state);
        return Err(bailout::Bailout);
    }
    if truth {
        let mut valid = false;
        // SAFETY: the cache is writable and the candidate is live.
        let failed = deferred(unsafe {
            sys::swr_zend_try_facade_callable(value, &raw mut state.cache, &raw mut valid)
        });
        if !failed {
            // SAFETY: get the diagnostic name even for an invalid callable, as upstream does.
            deferred(unsafe { sys::swr_zend_try_facade_name(value, &raw mut state.name) });
        }
        if !bailout::pending() {
            if valid {
                // SAFETY: this new owner's initially undefined slot takes a shallow value copy.
                state.value = unsafe { value.read() };
                // SAFETY: retain the successfully resolved candidate for this independent owner.
                unsafe { sys::swr_zval_try_addref(&raw mut state.value) };
            } else {
                // SAFETY: callable_name is an owned Zend string produced by the shim.
                let name = unsafe { string::bytes(state.name) };
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
    Ok(Box::into_raw(state))
}

/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
/// # Safety
/// A live handle, consumed exactly once. Cleanup may run PHP destructors.
pub unsafe fn release(handle: *mut Callable) -> Result<(), bailout::Bailout> {
    // SAFETY: ownership returns to Rust, including its zval and diagnostic string.
    drop(unsafe { Box::from_raw(handle) });
    outcome(())
}

/// # Safety
/// A live initialized handle. The caller borrows its cache until free.
pub unsafe fn cache(handle: *mut Callable) -> *mut zend_fcall_info_cache {
    // SAFETY: a live stable Box; no Rust reference escapes this operation.
    unsafe { &raw mut (*handle).cache }
}

/// # Safety
/// A live initialized handle.
#[must_use]
pub unsafe fn ready(handle: *const Callable) -> bool {
    if handle.is_null() {
        return false;
    }
    // SAFETY: the owned value is initialized, even for a failed resolution.
    let value = unsafe { &raw const (*handle).value };
    // SAFETY: the live handle contains an initialized zval.
    unsafe { type_of(value) != sys::IS_UNDEF }
}

/// # Safety
/// A live initialized handle; cloning retains the original zval as upstream does.
#[must_use]
pub unsafe fn copy(handle: *const Callable) -> *mut Callable {
    // SAFETY: the handle remains borrowed throughout the copy.
    let original = unsafe { &*handle };
    let mut copy = Box::new(Callable {
        value: original.value,
        cache: original.cache,
        name: original.name,
    });
    // SAFETY: this new owner holds another zval reference.
    unsafe { sys::swr_zval_try_addref(&raw mut copy.value) };
    if !copy.name.is_null() {
        // SAFETY: retain the immutable diagnostic name rather than duplicate its bytes.
        copy.name = unsafe { sys::swr_zend_string_copy(copy.name) };
    }
    Box::into_raw(copy)
}

/// # Errors
/// A native PHP leaf bailed out. No jump is resumed here; propagate the result
/// and finish only from the outermost C-facing handler or export.
/// # Safety
/// A ready handle; argc initialized arguments and optional writable return slot.
pub unsafe fn invoke(
    handle: *mut Callable,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
) -> Result<bool, bailout::Bailout> {
    // SAFETY: the owned callback remains live; retain raw pointers across PHP reentry.
    let cache = unsafe { &raw mut (*handle).cache };
    let mut fci = swoole_rs_zend::EMPTY_FCI;
    let mut temporary = swoole_rs_zend::UNDEF;
    fci.size = size_of::<sys::zend_fcall_info>();
    fci.retval = if retval.is_null() {
        &raw mut temporary
    } else {
        retval
    };
    fci.param_count = argc;
    fci.params = argv;
    // SAFETY: the cache is initialized and held live by the owned callback zval.
    if unsafe { (*cache).function_handler }.is_null() {
        // SAFETY: copy the borrowed function-name zval, matching ZVAL_COPY_VALUE.
        fci.function_name = unsafe { (*handle).value };
    }
    // SAFETY: live cache, arguments and return storage; no Rust borrow spans PHP reentry.
    let result = unsafe { bailout::call(&raw mut fci, cache) };
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
    outcome(success)
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
    if !name.is_null() {
        // SAFETY: a non-null candidate is a live initialized zval.
        if value.is_null() || unsafe { type_of(value) } == sys::IS_NULL {
            if !allow_null {
                // SAFETY: the caller supplies the NUL-terminated argument name.
                let name = unsafe { CStr::from_ptr(name) }.to_bytes();
                let message = [name, b" must be of type callable, null given"].concat();
                let message = CString::new(message).unwrap_or_default();
                // SAFETY: a registered exception class; Zend copies the message.
                deferred(unsafe {
                    sys::swr_zend_try_facade_throw(exception, message.as_ptr(), 505)
                });
            }
            return outcome(ptr::null_mut());
        }
    }
    // SAFETY: the candidate and native reporter satisfy callable_new's contract.
    let handle = unsafe { create(value, report) }?;
    // SAFETY: callable_new returned a live initialized handle.
    if unsafe { ready(handle) } {
        return Ok(handle);
    }
    // SAFETY: release the failed candidate as the original factory does.
    unsafe { release(handle) }?;
    if !name.is_null() {
        // SAFETY: the caller supplies a NUL-terminated argument name.
        let name = unsafe { CStr::from_ptr(name) }.to_bytes();
        // SAFETY: Zend returns a static type name for a live candidate.
        let kind = unsafe { sys::zend_zval_type_name(value) };
        // SAFETY: Zend returned a static NUL-terminated type name.
        let kind = unsafe { CStr::from_ptr(kind) }.to_bytes();
        let message = [name, b" must be of type callable, ", kind, b" given"].concat();
        let message = CString::new(message).unwrap_or_default();
        // SAFETY: a registered exception class; Zend copies the message.
        deferred(unsafe { sys::swr_zend_try_facade_throw(exception, message.as_ptr(), 505) });
    }
    outcome(ptr::null_mut())
}
