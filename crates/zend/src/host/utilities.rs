//! Class-alias registration for the native SDK.
use crate::{bailout, sys as z};
use std::ffi::{c_char, c_int};

/// Register the native SDK's lowercase persistent class alias.
/// # Safety
/// MINIT, on its thread; `name` is a live binary span and `entry` is a
/// registered persistent class.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_class_alias(
    name: *const c_char,
    length: usize,
    entry: *mut z::zend_class_entry,
) -> c_int {
    // SAFETY: per the caller, `length` live bytes.
    let name = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), length) };
    // SAFETY: MINIT, which the SDK's class registration runs in, and its
    // registered CE.
    let result = unsafe { crate::registration::class_alias(name, entry, true) };
    match result {
        // Zend's `SUCCESS` or `FAILURE`.
        Ok(true) => 0,
        Ok(false) => -1,
        Err(error) => {
            error.defer();
            bailout::finish();
            -1
        }
    }
}
