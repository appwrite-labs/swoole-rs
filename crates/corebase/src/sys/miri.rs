//! Substitutes for libc operations unsupported by Miri.

use std::ffi::CStr;
use std::sync::atomic::AtomicU64;

/// Miri has no fork or shared anonymous mmap. A private mapping exercises the
/// same memory accesses within the interpreted process.
pub(super) const MAP_SHARED: libc::c_int = libc::MAP_PRIVATE;

/// Keep the initialized mutex's default sharing attribute in this one process.
pub(crate) fn set_mutex_process_shared(_: &mut [AtomicU64]) -> Result<(), libc::c_int> {
    Ok(())
}

impl super::Interfaces {
    /// Miri has no getifaddrs. Model a host with an empty interface list.
    #[must_use]
    pub fn new() -> Option<Self> {
        Some(Self(None))
    }
}

/// Miri has no strftime. Report that no formatted date fit in the buffer.
pub fn strftime(_: &mut [u8], _: &CStr, _: &libc::tm) -> usize {
    0
}
