//! Class-alias registration for the native SDK.
use crate::{bailout, sys as z};
use std::{ffi::c_int, io::IoSlice};

/// Register the native SDK's lowercase persistent class alias, in MINIT on its
/// thread. `name` is a `struct iovec` on the C side, the caller's bytes live for
/// the call; `entry` is a registered persistent class.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_v1_phphelpers_class_alias(
    name: IoSlice<'_>,
    entry: *mut z::zend_class_entry,
) -> c_int {
    let name: &[u8] = &name;
    // A registered class lives for the process.
    let Some(entry) = crate::ClassEntry::live(entry) else {
        return -1;
    };
    // MINIT, which the SDK's class registration runs in (the caller's contract).
    match crate::registration::class_alias(name, entry, true) {
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
