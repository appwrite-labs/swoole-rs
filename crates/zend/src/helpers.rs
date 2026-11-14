//! Zvals, arrays and properties for crates/phpprocess: zval construction,
//! typed property updates.
use crate::sys::{time_t, zend_fcall_info, zend_fcall_info_cache, zval};
use std::{
    ffi::{CStr, c_char},
    io::IoSlice,
    marker::PhantomData,
    ptr::NonNull,
};

/// A NUL-terminated C string borrowed for `'a`: a `&CStr` one pointer wide, as
/// C takes and returns one, for the `safe fn` declarations over such strings.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct CText<'a>(NonNull<c_char>, PhantomData<&'a CStr>);

impl<'a> From<&'a CStr> for CText<'a> {
    fn from(text: &'a CStr) -> Self {
        Self(NonNull::from(text).cast(), PhantomData)
    }
}

impl<'a> CText<'a> {
    /// The string.
    pub(crate) fn get(self) -> &'a CStr {
        // SAFETY: made from a `&'a CStr`, or returned by a function declared to
        // return a NUL-terminated string that lives for `'a`.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }
    }
}

unsafe extern "C" {
    /// Zend's table of loaded extensions, keyed by lowercase name: filled at
    /// startup (and by `dl()`, on the thread running PHP) and only read here.
    safe static module_registry: crate::Array;
    /// `tsrm_is_main_thread()`: TSRM reads its own per-thread flag, on any
    /// thread; `true` on a PHP without thread safety.
    safe fn swr_zend_tsrm_is_main_thread() -> bool;
    /// PHP's clock: no arguments, no PHP state.
    safe fn php_time() -> time_t;
    /// `is_numeric_string(text, len, lval, dval, false)` over a terminated copy of
    /// the lent bytes (an `IoSlice` is an iovec), which it writes the two numbers
    /// from; errors are off, so it raises nothing.
    safe fn swr_bytes_numeric(text: IoSlice<'_>, lval: &mut i64, dval: &mut f64) -> u8;
}

mod sealed {
    /// Only the records `Record` documents.
    pub trait Zero {}
}

/// Zend's C records of integers, raw pointers, nullable function pointers and
/// zvals, for which all-zero is the empty value (an undef zval, a null handler).
/// Sealed: only the types listed here.
pub trait Record: sealed::Zero {}
impl<T: sealed::Zero> Record for T {}
impl sealed::Zero for zend_fcall_info {}
impl sealed::Zero for zend_fcall_info_cache {}
impl sealed::Zero for crate::sys::zend_execute_data {}
impl sealed::Zero for crate::sys::zend_function_entry {}
impl sealed::Zero for crate::sys::zend_module_entry {}
impl sealed::Zero for crate::sys::zend_object_handlers {}
impl sealed::Zero for crate::sys::swr_zend_callable {}
impl sealed::Zero for crate::sys::swr_zend_coroutine_vm {}
impl<T: sealed::Zero, const N: usize> sealed::Zero for [T; N] {}

/// An all-zero `T`, as C's `= {}`.
#[must_use]
pub const fn zeroed<T: Record>() -> T {
    // SAFETY: zero bytes are the empty value of every `Record`.
    unsafe { std::mem::zeroed() }
}

/// An empty `zend_fcall_info` (no function, object or arguments), for a
/// parser or the caller to fill.
pub const EMPTY_FCI: zend_fcall_info = zeroed();
/// An empty `zend_fcall_info_cache`, likewise.
pub const EMPTY_FCC: zend_fcall_info_cache = zeroed();

/// Whether the extension `name` (lowercase, as `module_registry` keys it) is
/// loaded.
#[must_use]
pub fn module_loaded(name: &CStr) -> bool {
    module_registry.find(name.to_bytes()).is_some()
}

/// `tsrm_is_main_thread()`: whether this is the thread that started PHP, always
/// true on a PHP without thread safety.
#[must_use]
pub fn is_main_thread() -> bool {
    swr_zend_tsrm_is_main_thread()
}

/// `php_time`: PHP's clock, in seconds.
#[must_use]
pub fn time() -> i64 {
    php_time()
}

/// `is_numeric_string(text, len, &lval, &dval, false)`: the kind (`IS_LONG`,
/// `IS_DOUBLE`, or 0 for text that is not numeric) with the long and double
/// it filled.
#[must_use]
pub fn numeric(text: &[u8]) -> (u8, i64, f64) {
    let (mut lval, mut dval) = (0, 0.0);
    let kind = swr_bytes_numeric(IoSlice::new(text), &mut lval, &mut dval);
    (kind, lval, dval)
}

/// `ZVAL_LONG`: a zval holding `value`.
#[must_use]
pub fn long_zval(value: i64) -> zval {
    zval {
        value: crate::sys::zend_value { lval: value },
        u1: crate::sys::_zval_struct__bindgen_ty_1 {
            type_info: crate::sys::IS_LONG,
        },
        u2: crate::sys::_zval_struct__bindgen_ty_2 { next: 0 },
    }
}
