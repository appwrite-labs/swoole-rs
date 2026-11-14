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
use std::ffi::{CStr, c_char};
use std::marker::PhantomData;
use std::ptr::NonNull;

pub use libc::tm as Tm;

/// A C string's address, live and NUL-terminated for `'a`: what libc's calendar
/// calls take for a `const char *`.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Text<'a>(NonNull<c_char>, PhantomData<&'a CStr>);

impl<'a> From<&'a CStr> for Text<'a> {
    fn from(text: &'a CStr) -> Self {
        Self(NonNull::from(text).cast(), PhantomData)
    }
}

// libc's calendar calls that read only C strings and a `tm` they are lent, and
// write only the `tm` (and, for `strptime`, return a pointer into `date`). None
// retains an argument, and none reads `tm_zone`. `time_t` is 64 bits wherever the
// extension builds, as `i64` was already passed for it.
unsafe extern "C" {
    #[link_name = "strptime"]
    safe fn libc_strptime(date: Text<'_>, format: Text<'_>, tm: &mut Tm) -> *mut c_char;
    #[link_name = "mktime"]
    safe fn libc_mktime(tm: &mut Tm) -> i64;
    safe fn gmtime_r(time: &i64, tm: &mut Tm) -> *mut Tm;
}

/// An all-zero `tm`, as a native `tm{}`.
#[must_use]
pub fn zeroed() -> Tm {
    Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null_mut(),
    }
}
/// `strptime(date, format, tm)`: whether `date` parsed.
pub fn strptime(date: &CStr, format: &CStr, tm: &mut Tm) -> bool {
    !libc_strptime(date.into(), format.into(), tm).is_null()
}
/// `mktime(tm)`, normalizing `tm`.
pub fn mktime(tm: &mut Tm) -> i64 {
    libc_mktime(tm)
}
/// `gmtime(&time)`'s calendar, through `gmtime_r` so no other call shares it; None
/// when `time` has no calendar date.
#[must_use]
pub fn gmtime(time: i64) -> Option<Tm> {
    let mut tm = zeroed();
    (!gmtime_r(&time, &mut tm).is_null()).then_some(tm)
}
/// `time(NULL)`: now.
#[must_use]
pub fn now() -> i64 {
    std::time::UNIX_EPOCH
        .elapsed()
        .map_or(0, |since| since.as_secs().cast_signed())
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
/// `isspace(byte)` in the current locale, the byte passed as a native `char`, as the
/// pinned C passes it.
#[must_use]
pub fn is_space(byte: u8) -> bool {
    let byte = std::ffi::c_int::from(std::ffi::c_char::from_ne_bytes([byte]));
    // SAFETY: a value in -128..=255, which glibc's, musl's and Apple's tables all index.
    unsafe { libc::isspace(byte) != 0 }
}
/// True when libc's case-insensitive comparison differs at this first byte pair in
/// every locale, so the call can be skipped: `tolower` keeps ASCII in ASCII except
/// Turkish `I` (to `ı`), and only Turkish `İ` lowers to an ASCII byte (`i`).
fn differs_at_first(left: u8, right: u8) -> bool {
    left.is_ascii()
        && right.is_ascii()
        && left | 0x20 != b'i'
        && right | 0x20 != b'i'
        && !left.eq_ignore_ascii_case(&right)
}
/// `input` starts with `expected` as `strncasecmp` compares them in the current
/// locale, which the pinned header matching uses.
#[inline]
#[must_use]
pub fn case_prefix(input: &[u8], expected: &[u8]) -> bool {
    input.len() >= expected.len()
        && !matches!((input.first(), expected.first()), (Some(&left), Some(&right)) if differs_at_first(left, right))
        && strncasecmp_equal(input, expected, expected.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_first_bytes_differ_under_libc_in_each_locale() {
        let locales = [
            c"C",
            c"C.UTF-8",
            c"en_US.ISO8859-1",
            c"en_US.UTF-8",
            c"tr_TR.ISO8859-9",
            c"tr_TR.UTF-8",
        ];
        for locale in locales {
            // SAFETY: a NUL-terminated name; this test alone changes LC_CTYPE, then restores it.
            if unsafe { libc::setlocale(libc::LC_CTYPE, locale.as_ptr()) }.is_null() {
                continue;
            }
            for left in 0..=u8::MAX {
                for right in 0..=u8::MAX {
                    if differs_at_first(left, right) {
                        assert!(
                            !strncasecmp_equal(&[left], &[right], 1),
                            "{left:#x} {right:#x} in {locale:?}"
                        );
                    }
                }
            }
        }
        // SAFETY: restores the default locale.
        unsafe { libc::setlocale(libc::LC_CTYPE, c"C".as_ptr()) };
    }
}
