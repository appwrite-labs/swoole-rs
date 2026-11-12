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
/// libc's `tolower(left) == tolower(right)` where every locale agrees, None where the
/// locale decides: `tolower` keeps ASCII in ASCII except Turkish `I` (to `ı`), and
/// only Turkish `İ` lowers to an ASCII byte (`i`).
fn same_folded(left: u8, right: u8) -> Option<bool> {
    if left == right {
        return Some(true);
    }
    (left.is_ascii() && right.is_ascii() && left | 0x20 != b'i' && right | 0x20 != b'i')
        .then(|| left.eq_ignore_ascii_case(&right))
}
/// `input` starts with `expected` as `strncasecmp` compares them in the current
/// locale, which the pinned header matching uses. libc runs only when a pair the
/// locale decides comes before the first pair that differs in every locale.
#[inline]
#[must_use]
pub fn case_prefix(input: &[u8], expected: &[u8]) -> bool {
    if input.len() < expected.len() {
        return false;
    }
    let mut undecided = false;
    for (&left, &right) in input.iter().zip(expected) {
        match same_folded(left, right) {
            // A matching NUL ends libc's comparison.
            Some(true) if left == 0 => break,
            Some(true) => {}
            Some(false) => return false,
            None => undecided = true,
        }
    }
    !undecided || strncasecmp_equal(input, expected, expected.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn case_prefix_is_strncasecmp_in_each_locale() {
        let locales = [
            c"C",
            c"C.UTF-8",
            c"en_US.ISO8859-1",
            c"en_US.UTF-8",
            c"tr_TR.ISO8859-9",
            c"tr_TR.UTF-8",
        ];
        // Each kind of pair: equal, ASCII case, a definite difference, the locale's
        // `I`/`i`/`İ`/`ı`, other non-ASCII, and NUL, in every order.
        let bytes = [
            0, b'a', b'A', b'b', b'i', b'I', b'-', 0xc4, 0xdd, 0xfd, 0xe9, 0xc9,
        ];
        for locale in locales {
            // SAFETY: a NUL-terminated name; this test alone changes LC_CTYPE, then restores it.
            if unsafe { libc::setlocale(libc::LC_CTYPE, locale.as_ptr()) }.is_null() {
                continue;
            }
            for left in 0..=u8::MAX {
                for right in 0..=u8::MAX {
                    if let Some(same) = same_folded(left, right) {
                        assert_eq!(
                            same,
                            strncasecmp_equal(&[left], &[right], 1),
                            "{left:#x} {right:#x} in {locale:?}"
                        );
                    }
                }
            }
            for a in bytes {
                for b in bytes {
                    for c in bytes {
                        for d in bytes {
                            let (input, expected) = ([a, b, b'x'], [c, d]);
                            assert_eq!(
                                case_prefix(&input, &expected),
                                strncasecmp_equal(&input, &expected, 2),
                                "{input:x?} {expected:x?} in {locale:?}"
                            );
                        }
                    }
                }
            }
        }
        // SAFETY: restores the default locale.
        unsafe { libc::setlocale(libc::LC_CTYPE, c"C".as_ptr()) };
        assert!(case_prefix(b"Content-Length: 1", b"content-length"));
        assert!(!case_prefix(b"Content-Type: x", b"content-length"));
        assert!(!case_prefix(b"Content", b"content-length"), "shorter input");
    }
}
