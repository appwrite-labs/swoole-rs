//! Shared object/property and callback custody operations for native PHP callers.
use crate::{
    bailout,
    sys::{self, zend_class_entry, zend_fcall_info, zend_fcall_info_cache, zend_object, zval},
    type_of,
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

unsafe fn destroy(value: *mut zval) {
    // SAFETY: the live value's destructor executes entirely within the C trap.
    bailout::defer(unsafe { bailout::trap(sys::swr_zend_zval_release, value.cast()) });
}

/// # Safety
/// The returned request allocation is uninitialized; null carries a deferred bailout.
unsafe fn alloc_request(size: usize) -> *mut c_void {
    let mut result = ptr::null_mut();
    // SAFETY: the C leaf allocates aligned request storage and traps allocation failure.
    bailed(unsafe { sys::swr_zend_try_facade_alloc(size, &raw mut result) });
    result
}

/// # Safety
/// A live source zval. This intentionally copies only the slot, without retaining its payload.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_zval_dup(value: *const zval) -> *mut zval {
    // SAFETY: one fresh request allocation, not a reference-count operation.
    let dest = unsafe { alloc_request(size_of::<zval>()) }.cast::<zval>();
    bailout::finish();
    // SAFETY: the source is initialized and independent of the allocation.
    let value = unsafe { value.read() };
    // SAFETY: the destination has space for one initialized slot.
    unsafe { dest.write(value) };
    dest
}

unsafe fn read(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: *const c_char,
    length: usize,
    silent: bool,
    rv: *mut zval,
) -> *mut zval {
    let mut result = ptr::null_mut();
    // SAFETY: live scope/object/name and writable result/temporary slots.
    bailed(unsafe {
        sys::swr_zend_try_facade_read(scope, object, name, length, silent, rv, &raw mut result)
    });
    result
}

unsafe fn array_property(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: *const c_char,
    length: usize,
) -> *mut zval {
    let mut value = crate::UNDEF;
    // SAFETY: construct a request array in an initialized writable slot.
    if bailed(unsafe { sys::swr_zend_try_facade_array_init(&raw mut value) }) {
        return ptr::null_mut();
    }
    // SAFETY: update_property can execute PHP, which the C shim traps.
    bailed(unsafe { sys::swr_zend_try_facade_update(scope, object, name, length, &raw mut value) });
    if bailout::pending() {
        return ptr::null_mut();
    }
    // SAFETY: release the temporary after the object has taken its reference.
    unsafe { destroy(&raw mut value) };
    if bailout::pending() {
        return ptr::null_mut();
    }
    // SAFETY: the object owns this declared property; its result is not the local rv.
    unsafe { read(scope, object, name, length, true, &raw mut value) }
}

/// `sw_zend_read_and_convert_property_array(scope, value, name, length, 0)`:
/// the object's property, made an array first when it is not one.
/// # Errors
/// PHP bailed out; nothing is resumed while a Rust caller owns its state.
/// # Safety
/// A live internal scope and object with a declared property; the returned
/// storage belongs to the object.
pub unsafe fn array_property_of(
    scope: *mut zend_class_entry,
    value: *mut zval,
    name: &core::ffi::CStr,
) -> Result<*mut zval, bailout::Bailout> {
    // SAFETY: per the caller.
    let property =
        unsafe { property(scope, value, name.as_ptr(), name.count_bytes(), false, true) };
    if bailout::relay_to_c(Ok(())) {
        Err(bailout::Bailout)
    } else {
        Ok(property)
    }
}

/// The property read, a bailout left pending.
/// # Safety
/// As [`array_property_of`].
unsafe fn property(
    scope: *mut zend_class_entry,
    value: *mut zval,
    name: *const c_char,
    length: usize,
    silent: bool,
    array: bool,
) -> *mut zval {
    // SAFETY: the source holds a live object.
    let object = unsafe { crate::value::obj_of(value) };
    let mut temporary = crate::UNDEF;
    // SAFETY: caller guarantees an object-owned declared property result.
    let mut property = unsafe { read(scope, object, name, length, silent, &raw mut temporary) };
    if !bailout::pending() {
        // SAFETY: Zend's distinguished uninitialized-value address is process/thread global.
        let undefined = unsafe { sys::swr_zend_uninitialized_zval() };
        if array {
            // SAFETY: a successful property read returns a live zval.
            if unsafe { type_of(property) } != sys::IS_ARRAY {
                if property == undefined {
                    // SAFETY: construct and publish the missing array property.
                    property = unsafe { array_property(scope, object, name, length) };
                } else {
                    // SAFETY: replace the existing value in the object's property slot.
                    unsafe { destroy(property) };
                    if !bailout::pending() {
                        // SAFETY: the old payload has been released and the slot remains writable.
                        bailed(unsafe { sys::swr_zend_try_facade_array_init(property) });
                    }
                }
            }
        } else if property == undefined {
            // SAFETY: preserve the original null-materialization order before rereading.
            bailed(unsafe { sys::swr_zend_try_facade_null(scope, object, name, length) });
            if !bailout::pending() {
                // SAFETY: Zend now owns an initialized declared property slot.
                property = unsafe { read(scope, object, name, length, silent, &raw mut temporary) };
            }
        }
    }
    property
}

unsafe fn closure(cache: *const zend_fcall_info_cache) -> *mut zend_object {
    // SAFETY: a resolved cache holds a live function handler.
    let handler = unsafe { (*cache).function_handler };
    // SAFETY: every Zend function initializes its common flags.
    let function = unsafe { handler.read() };
    // SAFETY: every Zend function has an initialized common prefix.
    let common = unsafe { function.common };
    if common.fn_flags & sys::ZEND_ACC_CLOSURE == 0 {
        ptr::null_mut()
    } else {
        // SAFETY: the closure flag makes Zend's version-dependent closure object projection valid.
        unsafe { sys::swr_zend_facade_closure(handler) }
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
        // SAFETY: reread the function after releasing the object, as the original does.
        let closure = unsafe { closure(cache) };
        if !closure.is_null() {
            // SAFETY: release the separately held closure reference inside the C trap.
            bailout::defer(unsafe {
                bailout::trap(sys::swr_zend_facade_object_release, closure.cast())
            });
        }
    }
    bailout::finish();
}

unsafe fn fatal_exception() {
    // SAFETY: observe the executor exception only after the preceding call returned.
    let exception = unsafe { sys::swr_zend_facade_exception() };
    if !exception.is_null() {
        // SAFETY: Zend's fatal exception reporter runs inside the C trap.
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
    let mut temporary = crate::UNDEF;
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
        &raw mut temporary
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
    if retval.is_null() && !bailout::pending() {
        // SAFETY: discard the private return slot after invocation, as upstream does.
        unsafe { destroy(&raw mut temporary) };
    }
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
        // SAFETY: preserve the immediate fatal exception check for loop callbacks.
        unsafe { fatal_exception() };
    }
    bailout::finish();
    result
}

/// # Safety
/// Live callable frame/cache; caller accepts the frame's upstream temporary-return convention.
pub unsafe fn call_anyway(frame: *mut zend_fcall_info, cache: *mut zend_fcall_info_cache) -> c_int {
    let mut temporary = crate::UNDEF;
    // SAFETY: the frame is initialized and exclusively configured before invocation.
    if unsafe { (*frame).retval }.is_null() {
        // SAFETY: the temporary remains live through the call and destruction below.
        unsafe { (*frame).retval = &raw mut temporary };
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
    // SAFETY: reread the frame after invocation, matching the original helper.
    if !bailout::pending() && unsafe { (*frame).retval } == &raw mut temporary {
        // SAFETY: the private temporary owns any returned payload.
        unsafe { destroy(&raw mut temporary) };
    }
    bailout::finish();
    result
}

/// crates/phpcoroutines' `create_coroutine`: the new cid, with 1 in its last
/// argument when activation bailed out.
pub type Create =
    unsafe extern "C" fn(*mut zend_fcall_info_cache, u32, *mut zval, *mut zval, *mut u8) -> c_long;

/// Invoke directly or create a coroutine, preserving the immediate fatal-exception check.
/// # Errors
/// PHP bailed out; no jump is resumed while a Rust caller owns its state.
/// # Safety
/// A live resolved cache, argument span and optional return slot. `create` is the coroutine leaf.
pub unsafe fn function_call(
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    coroutine: bool,
    create: Create,
    report: unsafe extern "C" fn(*const c_char),
) -> Result<bool, bailout::Bailout> {
    // Upstream never ran past a pending bailout: no coroutine starts, as no call runs.
    bailout::outcome(())?;
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
            )
        });
        result >= 0
    } else {
        // SAFETY: the caller lends a resolved cache and live arguments.
        unsafe { direct_call(ptr::null_mut(), cache, argc, argv, retval, report) == 0 }
    };
    if !bailout::pending() {
        // SAFETY: callbacks outside the VM must report an exception immediately.
        unsafe { fatal_exception() };
    }
    if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        Ok(success)
    }
}

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
    let mut function = crate::UNDEF;
    // SAFETY: Zend copies the complete binary function name into its request string.
    bailed(unsafe { sys::swr_zend_try_facade_value_string(&raw mut function, name, length) });
    if bailout::pending() {
        return;
    }
    let mut result = -1;
    // SAFETY: Zend's macro invocation is one leaf effect inside the C trap.
    bailed(unsafe {
        sys::swr_zend_try_facade_user_call(&raw mut function, argc, argv, retval, &raw mut result)
    });
    if !bailout::pending() {
        if result != 0 {
            // SAFETY: failed named invocation publishes null before name cleanup.
            unsafe {
                (*retval).u1 = sys::_zval_struct__bindgen_ty_1 {
                    type_info: sys::IS_NULL,
                }
            };
        }
        // SAFETY: the local function-name string owns one reference.
        unsafe { destroy(&raw mut function) };
        if !bailout::pending() {
            // SAFETY: preserve the fatal exception check after function-name cleanup.
            unsafe { fatal_exception() };
        }
    }
}

/// `zend::function::call(name, args)`, its result discarded as upstream's
/// temporary `Variable` is.
/// # Errors
/// PHP bailed out; nothing is resumed while a Rust caller owns its state.
/// # Safety
/// `args` are live zvals.
pub unsafe fn named_call(name: &[u8], args: &mut [zval]) -> Result<(), bailout::Bailout> {
    #[expect(clippy::cast_possible_truncation, reason = "a handful of arguments")]
    let argc = args.len() as u32;
    let mut retval = crate::UNDEF;
    // SAFETY: the name's bytes, live arguments and a writable result.
    unsafe {
        named_call_into(
            name.as_ptr().cast(),
            name.len(),
            argc,
            args.as_mut_ptr(),
            &raw mut retval,
        );
    };
    if !bailout::pending() {
        // SAFETY: the result this call owns.
        unsafe { destroy(&raw mut retval) };
    }
    if bailout::relay_to_c(Ok(())) {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}
