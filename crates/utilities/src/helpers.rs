// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 include/swoole.h and include/swoole_util.h.
//! `swoole::get_timezone`.

use std::ffi::c_long;

/// `swoole::get_timezone`: the local zone's offset from UTC now, in seconds.
#[must_use]
pub fn timezone() -> c_long {
    let now = swoole_rs_corebase::time_now();
    // SAFETY: all-zero is a valid `tm`.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are live locals.
    unsafe { libc::localtime_r(&raw const now, &raw mut local) };
    local.tm_gmtoff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timezone_matches_the_local_offset_bounds() {
        let offset = timezone();
        assert!((-14 * 3600..=14 * 3600).contains(&offset), "{offset}");
    }

    /// Upstream's `time.get_timezone`. Like upstream's, it holds only where the
    /// local zone is at or east of UTC, as on the CI runners.
    #[test]
    fn time_get_timezone() {
        assert!(timezone() >= 0);
    }
}
