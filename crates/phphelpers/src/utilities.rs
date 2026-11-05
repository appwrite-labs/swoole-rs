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

/// Encode and duplicate the bytes in Zend request memory, returning an efree-owned buffer.
/// # Safety
/// `value` is readable for `count` bytes; `length` is writable.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_url_encode(
    value: *const c_char,
    count: usize,
    length: *mut usize,
) -> *mut c_char {
    let mut encoded = ptr::null_mut::<zend_string>();
    // SAFETY: the SDK lends the source span; PHP allocates the owned result below this trap.
    if unsafe { z::swr_zend_try_facade_url(value, count, &raw mut encoded) } != 0 {
        Bailout.defer();
        bailout::finish();
        return ptr::null_mut();
    }
    // SAFETY: the encoder returned an owned initialized string.
    let count = unsafe { (*encoded).len };
    // SAFETY: preserve the original publication before duplicate allocation.
    unsafe { length.write(count) };
    // SAFETY: duplicate the live binary encoded string into request memory.
    let result =
        unsafe { super::owned::duplicate_bytes(string::val(encoded).cast(), count, true, false) };
    // SAFETY: the temporary string owns no user-code destructor.
    unsafe { z::swr_zend_string_release(encoded) };
    bailout::finish();
    result
}

/// Append PHP's query encoding to the caller's actual `smart_str` owner.
/// # Safety
/// `value` is a live zval; `buffer` points to its caller's initialized `smart_str`;
/// `length` is writable. The caller owns the buffer and returned borrowed bytes.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_http_query(
    value: *mut zval,
    length: *mut usize,
    buffer: *mut c_void,
) -> *mut c_char {
    let buffer = buffer.cast::<z::smart_str>();
    let mut table = ptr::null_mut();
    // SAFETY: HASH_OF runs on the live value and any native handler is contained.
    if unsafe { z::swr_zend_try_facade_hash(value, &raw mut table) } != 0 {
        Bailout.defer();
        bailout::finish();
        return ptr::null_mut();
    }
    if table.is_null() {
        // SAFETY: the caller's smart buffer is initialized and remains exclusively borrowed.
        unsafe { z::swr_zend_facade_smart_free(buffer) };
        return ptr::null_mut();
    }
    // HASH_OF is evaluated twice upstream: an object's native get_properties
    // handler may observe each evaluation, so reload it before encoding.
    // SAFETY: this is the second original HASH_OF effect, independently contained.
    if unsafe { z::swr_zend_try_facade_hash(value, &raw mut table) } != 0 {
        Bailout.defer();
        bailout::finish();
        return ptr::null_mut();
    }
    // SAFETY: PHP borrows the real table and mutates the caller-owned smart buffer within its trap.
    if unsafe { z::swr_zend_try_facade_query(table, buffer) } != 0 {
        Bailout.defer();
        bailout::finish();
        return ptr::null_mut();
    }
    // SAFETY: the caller's buffer remains live through this call.
    let string = unsafe { (*buffer).s };
    if string.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: null termination may grow this buffer, and that allocation is contained.
    if unsafe { z::swr_zend_try_facade_smart_zero(buffer) } != 0 {
        Bailout.defer();
        bailout::finish();
        return ptr::null_mut();
    }
    // SAFETY: smart_str_0 may have reallocated; reload the owned string.
    let string = unsafe { (*buffer).s };
    // SAFETY: the buffer holds a live string after successful termination.
    let count = unsafe { (*string).len };
    // SAFETY: the SDK lends this writable output.
    unsafe { length.write(count) };
    string::val(string).cast()
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

/// Register a complete function alias for a native SDK consumer.
/// # Safety
/// The tables, binary names and arginfo follow Zend's persistent registration
/// contract. The assertion callback is native-only and cannot unwind or bail out.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_function_alias(
    source: *mut z::HashTable,
    origin: *const c_char,
    origin_length: usize,
    target: *mut z::HashTable,
    alias: *const c_char,
    alias_length: usize,
    info: *const z::zend_internal_arg_info,
    assert_internal: Option<unsafe extern "C" fn(u8)>,
) -> c_int {
    // SAFETY: the native SDK lends every registration input through this call.
    let result = unsafe {
        swoole_rs_zend::registration::function_alias(
            source,
            origin,
            origin_length,
            target,
            alias,
            alias_length,
            info,
            assert_internal,
        )
    };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            bailout::finish();
            -1
        }
    }
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
