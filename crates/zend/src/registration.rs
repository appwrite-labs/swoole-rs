//! Actual class alias construction, lookup and ownership.
use crate::{
    bailout::Bailout,
    string,
    sys::{self as z, zend_class_entry},
};
use std::{
    ffi::{c_char, c_int},
    ptr,
};

/// Zend's `SUCCESS` `zend_result`.
pub(crate) const SUCCESS: c_int = 0;

pub(crate) fn leaf(code: c_int) -> Result<(), Bailout> {
    if code == 0 { Ok(()) } else { Err(Bailout) }
}

/// Build the lowercase persistent interned class alias and register its real CE:
/// whether Zend registered it.
/// `sdk_prefix` retains the native SDK's handling of a leading backslash,
/// including its original string length; ordinary Rust aliases pass false.
/// # Errors
/// Returns a contained PHP interning or registration bailout.
/// # Safety
/// `name` is readable for `length` bytes, and `entry` is a live persistent CE.
pub unsafe fn class_alias(
    name: *const c_char,
    length: usize,
    entry: *mut zend_class_entry,
    sdk_prefix: bool,
) -> Result<bool, Bailout> {
    // SAFETY: the source is live; PHP allocates the persistent owned name.
    let alias = unsafe { z::swr_zend_string_persistent(name, length) };
    // SAFETY: this persistent string is newly allocated and unshared.
    let bytes = unsafe { std::slice::from_raw_parts_mut(string::val(alias), length) };
    if sdk_prefix && bytes.first() == Some(&b'\\') {
        bytes.copy_within(1.., 0);
        if let Some(last) = bytes.last_mut() {
            *last = 0;
        }
    }
    bytes.make_ascii_lowercase();
    let mut interned = ptr::null_mut();
    // SAFETY: Zend consumes or interns the persistent string, as its native API
    // does; the successful result belongs to the process intern pool.
    leaf(unsafe { z::swr_zend_try_facade_intern(alias, &raw mut interned) })?;
    // Interning returns `alias` or the pool's string of the same bytes: either
    // way `length` of them.
    let mut result = -1;
    // SAFETY: the interned binary name and registered CE remain process-lived;
    // Zend copies the alias registration inside the contained leaf.
    leaf(unsafe {
        z::swr_zend_try_facade_class_alias(
            string::val(interned).cast(),
            length,
            entry,
            &raw mut result,
        )
    })?;
    Ok(result == SUCCESS)
}
