//! Owning Zend values used by the source-compatible C++ wrappers.
use std::{
    ffi::{c_char, c_void},
    ptr,
};
use swoole_rs_zend::{
    bailout, string,
    sys::{self, zend_string, zval},
};

/// One owned string reference. The C++ facade only holds this opaque handle.
pub(crate) struct StringValue(*mut zend_string);

impl Drop for StringValue {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle owns this reference, including interned strings.
            unsafe { sys::swr_zend_string_release(self.0) };
        }
    }
}

/// # Safety
/// `text` points to `length` readable bytes, including embedded NULs.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_new(
    text: *const c_char,
    length: usize,
) -> *mut StringValue {
    // SAFETY: the caller supplies the readable span; Zend allocates its own copy.
    let mut value = ptr::null_mut();
    // SAFETY: the C leaf contains allocation failure before returning to Rust.
    if unsafe { sys::swr_zend_try_facade_string(text, length, &raw mut value) } != 0 {
        bailout::Bailout.defer();
    }
    bailout::finish();
    Box::into_raw(Box::new(StringValue(value)))
}

/// # Safety
/// `value` is a live zval. Conversion contains user-code bailouts in the Zend shim.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_value(
    value: *mut zval,
) -> *mut StringValue {
    // SAFETY: per the caller; get_string returns a newly owned reference.
    let value = unsafe { swoole_rs_zend::value::get_string(value) };
    bailout::finish();
    Box::into_raw(Box::new(StringValue(value)))
}

/// # Safety
/// `value` is null or a live string. With `copy=false`, its reference is transferred.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_take(
    value: *mut zend_string,
    copy: bool,
) -> *mut StringValue {
    let value = if copy && !value.is_null() {
        // SAFETY: a live string; this handle takes the new reference.
        unsafe { sys::swr_zend_string_copy(value) }
    } else {
        value
    };
    Box::into_raw(Box::new(StringValue(value)))
}

/// # Safety
/// `handle` is a live handle or null (the default and moved-from states).
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_get(
    handle: *const StringValue,
) -> *mut zend_string {
    // SAFETY: per the caller; a null handle owns no string.
    unsafe { handle.as_ref() }.map_or(ptr::null_mut(), |value| value.0)
}

/// # Safety
/// `handle` is a uniquely owned handle or null. It is consumed exactly once.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_free(handle: *mut StringValue) {
    if !handle.is_null() {
        // SAFETY: the caller transfers this Box back to its allocator.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// # Safety
/// `handle` owns a live string, and the caller permits mutation as upstream does.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_trim(handle: *const StringValue) {
    // SAFETY: a live handle.
    let value = unsafe { swr_v1_phphelpers_string_get(handle) };
    // SAFETY: a live string; the borrow ends before mutation below.
    let bytes = unsafe { string::bytes(value) };
    let length = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        .map_or(0, |last| last + 1);
    // SAFETY: upstream rtrim zeroes each removed byte, including the terminator.
    let end = string::val(value).wrapping_add(length);
    // SAFETY: the remaining span lies within the string allocation.
    unsafe { ptr::write_bytes(end, 0, bytes.len() - length + 1) };
    // SAFETY: the string allocation is unchanged; publish its shortened length.
    unsafe { (*value).len = length };
}

/// # Safety
/// A readable span. The result uses Zend request memory when `request`, libc otherwise.
pub(crate) unsafe fn duplicate_bytes(
    text: *const c_char,
    length: usize,
    request: bool,
    lower: bool,
) -> *mut c_char {
    let target = if request {
        // SAFETY: a request allocation of the span and trailing NUL.
        unsafe { alloc_request(length + 1) }
    } else {
        // SAFETY: libc allocation with no borrowed memory.
        unsafe { libc::malloc(length + 1) }
    }
    .cast::<c_char>();
    if target.is_null() {
        return target;
    }
    // SAFETY: independent readable source and fresh writable allocation.
    unsafe { ptr::copy_nonoverlapping(text, target, length) };
    if lower {
        // SAFETY: exclusively owned initialized bytes, independent of the source.
        unsafe { std::slice::from_raw_parts_mut(target.cast::<u8>(), length) }
            .make_ascii_lowercase();
    }
    // SAFETY: the allocation includes the NUL byte.
    unsafe { target.wrapping_add(length).write(0) };
    target
}

/// Allocate the owning zval. C++'s public `value` aliases this stable Rust storage.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_v1_phphelpers_variable_new() -> *mut zval {
    // SAFETY: allocate one aligned slot, trapping request-memory failure in C.
    let value = unsafe { alloc_request(size_of::<zval>()) }.cast::<zval>();
    bailout::finish();
    // SAFETY: all-zero bytes initialize Zend's undefined zval, including its spare fields.
    unsafe { value.write_bytes(0, 1) };
    value
}

/// # Safety
/// Live, independent source/destination zvals. Match upstream assign's retain semantics.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_variable_assign(
    dest: *mut zval,
    source: *const zval,
) {
    // SAFETY: both slots are live; `read` copies the plain Zend value.
    let value = unsafe { source.read() };
    // SAFETY: the destination is exclusively writable.
    unsafe { dest.write(value) };
    // SAFETY: retain the reference now owned by this destination.
    unsafe { sys::swr_zval_try_addref(dest) };
}

/// # Safety
/// `value` is a uniquely owned zval allocated by `variable_new`; may run PHP destructors.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_variable_free(value: *mut zval) {
    // SAFETY: release the owned payload in the C trap before freeing its stable slot.
    bailout::defer(unsafe { bailout::trap(sys::swr_zend_zval_release, value.cast()) });
    // SAFETY: variable_new allocated this stable slot from the request allocator.
    unsafe { sys::swr_zend_efree(value.cast()) };
    bailout::finish();
}

/// # Safety
/// `value` is a live, exclusively writable zval. Reset transfers away its reference.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_variable_reset(value: *mut zval) {
    // SAFETY: ZVAL_UNDEF changes the type only, preserving the other fields.
    let value = unsafe { &mut *value };
    value.u1 = sys::_zval_struct__bindgen_ty_1 {
        type_info: sys::IS_UNDEF,
    };
}

/// # Safety
/// `handle` owns a live string; an empty string produces null as in upstream.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_dup(
    handle: *const StringValue,
    request: bool,
) -> *mut c_char {
    // SAFETY: a live handle containing a string.
    let value = unsafe { swr_v1_phphelpers_string_get(handle) };
    // SAFETY: per the caller.
    let bytes = unsafe { string::bytes(value) };
    if bytes.is_empty() {
        return ptr::null_mut();
    }
    // SAFETY: the source remains live and copying does not change it.
    let result = unsafe {
        duplicate_bytes(
            bytes.as_ptr().cast(),
            duplicate_length(bytes, request),
            request,
            false,
        )
    };
    bailout::finish();
    result
}

/// # Safety
/// `value` is a writable owned zval; `text` is readable for `length` bytes.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_variable_string(
    value: *mut zval,
    text: *const c_char,
    length: usize,
) {
    // SAFETY: a readable span; Zend takes a fresh string reference.
    if unsafe { sys::swr_zend_try_facade_value_string(value, text, length) } != 0 {
        bailout::Bailout.defer();
    }
    bailout::finish();
}

/// # Safety
/// Live, independent owned zvals. Transfer the reference and undefine the source.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_variable_move(
    dest: *mut zval,
    source: *mut zval,
) {
    // SAFETY: source is a readable live zval.
    let value = unsafe { source.read() };
    // SAFETY: destination is exclusively writable.
    unsafe { dest.write(value) };
    // SAFETY: the reference now belongs to the destination.
    unsafe { swr_v1_phphelpers_variable_reset(source) };
}

/// # Safety
/// A live zval; the caller accounts for the added or removed reference.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_variable_ref(value: *mut zval, add: bool) {
    if add {
        // SAFETY: the caller takes another reference to the live zval.
        unsafe { sys::swr_zval_try_addref(value) };
    } else {
        // SAFETY: the caller transfers away a reference, without invoking a destructor.
        unsafe { sys::swr_zval_try_delref(value) };
    }
}

fn duplicate_length(bytes: &[u8], request: bool) -> usize {
    if request {
        bytes.len()
    } else {
        bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len())
    }
}

/// # Safety
/// The returned request allocation is uninitialized; null carries a deferred bailout.
pub(crate) unsafe fn alloc_request(size: usize) -> *mut c_void {
    let mut result = ptr::null_mut();
    // SAFETY: the C leaf allocates aligned request storage and traps allocation failure.
    if unsafe { sys::swr_zend_try_facade_alloc(size, &raw mut result) } != 0 {
        bailout::Bailout.defer();
    }
    result
}
