//! Class-alias registration for the native SDK.
use crate::{bailout, sys as z};
use std::ffi::{c_char, c_int};

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
    let result = unsafe { crate::registration::class_alias(name, length, entry, true) };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            bailout::finish();
            -1
        }
    }
}
