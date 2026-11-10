// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 include/swoole.h and include/swoole_util.h.
//! `swoole::get_timezone`, and the header helper `swoole_strlchr`.

use std::ffi::{c_char, c_long};

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

/// `swoole_strlchr`: the first `c` in `[p, last)`, or null.
/// # Safety
/// `p..last` is a readable range (empty when `last <= p`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_strlchr(
    p: *mut c_char,
    last: *const c_char,
    c: c_char,
) -> *mut c_char {
    let length = (last as usize).saturating_sub(p as usize);
    if length == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller lends `length` readable bytes at `p`.
    let bytes = unsafe { std::slice::from_raw_parts(p.cast_const().cast::<u8>(), length) };
    memchr::memchr(c.to_ne_bytes()[0], bytes)
        .map_or(std::ptr::null_mut(), |index| p.wrapping_add(index))
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

    const DOT: c_char = 46;

    #[test]
    fn strlchr_stops_at_last() {
        let mut text = *b"a.b.c";
        let p = text.as_mut_ptr().cast::<c_char>();
        unsafe {
            assert_eq!(swr_v1_utils_strlchr(p, p.add(5), DOT), p.add(1));
            assert_eq!(swr_v1_utils_strlchr(p.add(2), p.add(5), DOT), p.add(3));
            assert!(swr_v1_utils_strlchr(p, p.add(1), DOT).is_null());
            assert!(swr_v1_utils_strlchr(p, p, DOT).is_null());
        }
    }
}
