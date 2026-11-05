//! Callable inspection and URL/query result ownership for the native SDK.
use std::{
    ffi::{c_char, c_int, c_void},
    ptr,
};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    string,
    sys::{self as z, zend_execute_data, zend_fcall_info_cache, zend_object, zend_string, zval},
};

fn leaf(code: c_int) -> Result<(), Bailout> {
    if code == 0 { Ok(()) } else { Err(Bailout) }
}

/// Inspect the original frame and always compute the callable's display name.
/// # Safety
/// Arguments are Zend's borrowed callable/cache/output pointers; optional object,
/// frame and name/length outputs may be null exactly as the native SDK permits.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_callable_frame(
    value: *mut zval,
    object: *mut zval,
    frame: *mut zend_execute_data,
    flags: u32,
    name: *mut *mut c_char,
    length: *mut usize,
    cache: *mut zend_fcall_info_cache,
    error: *mut *mut c_char,
) -> bool {
    let mut result = false;
    // SAFETY: the SDK's optional object zval contains a live object when non-null.
    let object = if object.is_null() {
        ptr::null_mut()
    } else {
        // SAFETY: the non-null SDK object argument is an initialized object zval.
        unsafe { swoole_rs_zend::value::obj_of(object) }
    };
    // SAFETY: each borrowed argument remains live; Zend's inspection is contained below Rust.
    let inspected = leaf(unsafe {
        z::swr_zend_try_facade_callable_frame(
            value,
            object,
            frame,
            flags,
            cache,
            error,
            &raw mut result,
        )
    });
    if inspected.is_err() {
        Bailout.defer();
        bailout::finish();
        return false;
    }
    let mut display = ptr::null_mut();
    // SAFETY: the same live callable and object; the returned string is owned by this frame.
    if unsafe { z::swr_zend_try_facade_callable_name(value, object, &raw mut display) } != 0 {
        Bailout.defer();
        bailout::finish();
        return false;
    }
    // SAFETY: Zend returned an initialized owned display string.
    let count = unsafe { (*display).len };
    if !name.is_null() {
        // SAFETY: duplicate the full binary name into independently owned request memory.
        let copy = unsafe {
            super::owned::duplicate_bytes(string::val(display).cast(), count, true, false)
        };
        if !bailout::pending() {
            // SAFETY: the optional output is writable and takes the duplicate.
            unsafe { name.write(copy) };
        }
    }
    if !bailout::pending() && !length.is_null() {
        // SAFETY: the optional length output is writable.
        unsafe { length.write(count) };
    }
    // SAFETY: this frame owns the display string, which contains no PHP object.
    unsafe { z::swr_zend_string_release(display) };
    bailout::finish();
    result
}

/// Preserve the native SDK's declared-property unset guard using the shared Rust implementation.
/// # Safety
/// Zend supplies a live object, member name and optional property-cache slot.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_unset_deny(
    object: *mut zend_object,
    member: *mut zend_string,
    cache: *mut *mut c_void,
) {
    // SAFETY: this is the registered Zend handler's original argument contract.
    unsafe { swoole_rs_zend::unset_property_deny(object, member, cache) };
}

/// The native SDK retains its `SW_ASSERT` on the root class of the unset handler.
/// # Safety
/// `object` is live and all its class entries have process lifetime.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_unset_class_valid(
    object: *mut zend_object,
) -> bool {
    // SAFETY: this is a live object's class entry.
    let mut root = unsafe { (*object).ce };
    loop {
        // SAFETY: each live class entry's parent union holds a class entry or null.
        let entry = unsafe { &*root };
        // SAFETY: a live class entry stores its nullable parent in this union.
        let parent = unsafe { entry.__bindgen_anon_1.parent };
        if parent.is_null() {
            break;
        }
        root = parent;
    }
    // SAFETY: the root class is still process-lived; 1 is ZEND_INTERNAL_CLASS.
    (unsafe { (*root).type_ }) == 1
}

/// Register the native SDK's lowercase persistent class alias.
/// # Safety
/// `name` is a live binary span and `entry` is a registered persistent class.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_class_alias(
    name: *const c_char,
    length: usize,
    entry: *mut z::zend_class_entry,
) -> c_int {
    // SAFETY: the SDK supplies its original alias and registered CE.
    let result = unsafe { swoole_rs_zend::registration::class_alias(name, length, entry, true) };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            bailout::finish();
            -1
        }
    }
}
