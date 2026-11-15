//! Actual class alias construction, lookup and ownership.
use crate::{ClassEntry, bailout::Bailout};
use std::{ffi::c_int, io::IoSlice};

unsafe extern "C" {
    /// The lent bytes (an `IoSlice` is an iovec) interned as a new persistent
    /// string, then `zend_register_class_alias_ex` of it for the live class,
    /// inside `zend_try` (nonzero for a bailout); `registered` says whether
    /// Zend registered it. Private: only [`class_alias`] calls it, at MINIT,
    /// where interning is permanent.
    safe fn swr_lent_class_alias(
        alias: IoSlice<'_>,
        class: ClassEntry<'static>,
        registered: &mut bool,
    ) -> c_int;
}

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
/// MINIT, on its thread.
pub unsafe fn class_alias(
    name: &[u8],
    entry: ClassEntry<'static>,
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
    let mut registered = false;
    leaf(swr_lent_class_alias(
        IoSlice::new(&alias),
        entry,
        &mut registered,
    ))?;
    Ok(registered)
}
