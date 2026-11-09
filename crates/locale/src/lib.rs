//! libc's locale- and timezone-dependent calendar and case comparison over owned
//! values, for the HTTP static handler's dates and crates/http's header matching,
//! so that crates/http holds no `unsafe`.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::ffi::CStr;

pub use libc::tm as Tm;

/// An all-zero `tm`, as a native `tm{}`.
#[must_use]
pub fn zeroed() -> Tm {
    // SAFETY: zero is a valid value for every field of libc's tm: integers and, where
    // present, a nullable zone pointer.
    unsafe { std::mem::zeroed() }
}
/// `strptime(date, format, tm)`: whether `date` parsed.
pub fn strptime(date: &CStr, format: &CStr, tm: &mut Tm) -> bool {
    // SAFETY: two C strings and an exclusive tm, none retained.
    !unsafe { libc::strptime(date.as_ptr(), format.as_ptr(), tm) }.is_null()
}
/// `mktime(tm)`, normalizing `tm`.
pub fn mktime(tm: &mut Tm) -> i64 {
    // SAFETY: an exclusive, initialized tm.
    unsafe { libc::mktime(tm) }
}
/// `gmtime(&time)`, copied out of libc's static calendar before another call can
/// change it; None when `time` has no calendar date.
#[must_use]
pub fn gmtime(time: i64) -> Option<Tm> {
    // SAFETY: a live time_t; the result is null or libc's static tm.
    let result = unsafe { libc::gmtime(&raw const time) };
    // SAFETY: null, or the static tm, read before any other calendar call.
    unsafe { result.as_ref() }.copied()
}
/// `time(NULL)`: now.
#[must_use]
pub fn now() -> i64 {
    // SAFETY: a null out pointer asks only for the result.
    unsafe { libc::time(std::ptr::null_mut()) }
}
/// `strftime(output, format, tm)`: the bytes written, 0 when they do not fit.
pub fn strftime(output: &mut [u8], format: &CStr, tm: &Tm) -> usize {
    // SAFETY: `output.len()` writable bytes, a C string and a live tm.
    unsafe {
        libc::strftime(
            output.as_mut_ptr().cast(),
            output.len(),
            format.as_ptr(),
            tm,
        )
    }
}
/// `strncasecmp(left, right, length) == 0` in the current locale, over at most the
/// shorter slice; a matching NUL ends the comparison as it does in libc.
#[must_use]
pub fn strncasecmp_equal(left: &[u8], right: &[u8], length: usize) -> bool {
    let length = length.min(left.len()).min(right.len());
    // SAFETY: both slices hold at least `length` readable bytes.
    unsafe { libc::strncasecmp(left.as_ptr().cast(), right.as_ptr().cast(), length) == 0 }
}
