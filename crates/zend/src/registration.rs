//! Actual class alias construction, lookup and ownership.
use crate::{
    bailout::Bailout,
    host::swr_bytes_persistent,
    string,
    sys::{self as z, zend_class_entry},
};
use std::{ffi::c_int, io::IoSlice, ptr};

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
/// MINIT, on its thread, and `entry` is a registered (persistent) class.
pub unsafe fn class_alias(
    name: &[u8],
    entry: *mut zend_class_entry,
    sdk_prefix: bool,
) -> Result<bool, Bailout> {
    let mut alias = name.to_vec();
    if sdk_prefix && alias.first() == Some(&b'\\') {
        alias.copy_within(1.., 0);
        if let Some(last) = alias.last_mut() {
            *last = 0;
        }
    }
    alias.make_ascii_lowercase();
    let alias = swr_bytes_persistent(IoSlice::new(&alias));
    let mut interned = ptr::null_mut();
    // SAFETY: Zend consumes or interns the new persistent string, as its native
    // API does; the successful result belongs to the process intern pool.
    leaf(unsafe { z::swr_zend_try_facade_intern(alias, &raw mut interned) })?;
    // Interning returns `alias` or the pool's string of the same bytes: either
    // way `name.len()` of them.
    let mut result = -1;
    // SAFETY: the interned binary name and registered CE remain process-lived;
    // Zend copies the alias registration inside the contained leaf.
    leaf(unsafe {
        z::swr_zend_try_facade_class_alias(
            string::val(interned).cast(),
            name.len(),
            entry,
            &raw mut result,
        )
    })?;
    Ok(result == SUCCESS)
}
