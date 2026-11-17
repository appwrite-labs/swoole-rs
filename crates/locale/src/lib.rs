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
use std::ffi::{CStr, c_char, c_int};
use std::marker::PhantomData;
use std::ptr::NonNull;
use swoole_rs_corebase::lent::{Count, Out, lend_out};

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
    /// Writes at most the lent count of bytes.
    #[link_name = "strftime"]
    safe fn libc_strftime(
        output: Out<'_, u8>,
        count: Count<'_>,
        format: Text<'_>,
        tm: &Tm,
    ) -> usize;
}

/// A native `char` widened to `int`, as the pinned C passes one to `<ctype.h>`: a value
/// in -128..=255, which glibc's, musl's and Apple's tables all index.
#[repr(transparent)]
struct Char(c_int);

impl From<u8> for Char {
    fn from(byte: u8) -> Self {
        Self(c_int::from(c_char::from_ne_bytes([byte])))
    }
}

// `tolower` reads only the current locale's table, and glibc and musl bound the
// argument first, so any `int` is defined. `isspace` reads the table at a [`Char`].
unsafe extern "C" {
    safe fn tolower(byte: c_int) -> c_int;
    safe fn isspace(byte: Char) -> c_int;
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
    lend_out(output, |output, count| {
        libc_strftime(output, count, format.into(), tm)
    })
}
/// `tolower(left) == tolower(right)` in the current locale. Every locale lowers
/// ASCII as `to_ascii_lowercase` does except Turkish `I` (to `ı`), so only `I` and
/// non-ASCII bytes ask libc.
fn lower_alike(left: u8, right: u8) -> bool {
    let lower = |byte: u8| {
        if byte.is_ascii() && byte != b'I' {
            c_int::from(byte.to_ascii_lowercase())
        } else {
            tolower(c_int::from(byte))
        }
    };
    lower(left) == lower(right)
}
/// glibc's portable `strncasecmp` loop, for operands [`alike`] does not settle.
#[cold]
#[inline(never)]
fn bytewise(left: &[u8], right: &[u8]) -> bool {
    for (&left, &right) in left.iter().zip(right) {
        if left != right && !lower_alike(left, right) {
            return false;
        }
        if left == 0 {
            break;
        }
    }
    true
}
const ONES: u64 = u64::from_ne_bytes([1; 8]);
const HIGHS: u64 = ONES << 7;
/// The first eight bytes, or, under eight, every byte: twice when there are four
/// or more, else beside `a`s, which compare equal on both sides.
fn word(bytes: &[u8]) -> u64 {
    if let Some(&word) = bytes.first_chunk() {
        return u64::from_ne_bytes(word);
    }
    match (bytes.first_chunk(), bytes.last_chunk()) {
        (Some(&first), Some(&last)) => {
            u64::from(u32::from_ne_bytes(first)) | (u64::from(u32::from_ne_bytes(last)) << 32)
        }
        _ => bytes.iter().fold(ONES * u64::from(b'a'), |word, &byte| {
            (word << 8) | u64::from(byte)
        }),
    }
}
/// The last eight bytes, of eight or more.
fn last_word(bytes: &[u8]) -> u64 {
    bytes
        .last_chunk()
        .map_or(0, |&word| u64::from_ne_bytes(word))
}
/// True when each byte pair of two words is equal, or is an ASCII letter other
/// than `i` in both cases, which every locale lowers alike: such words compare
/// equal in libc. The letters are read from `y`, usually a constant's word, so
/// that, inlined, they fold away.
fn alike(x: u64, y: u64) -> bool {
    let case = x ^ y;
    let lowered = y | (HIGHS >> 2);
    let ascii = lowered & !HIGHS;
    // a..=h and j..=z, each flagged in its byte's high bit.
    let letter = ((ascii + ONES * 0x1f) & !(ascii + ONES * 0x17))
        | ((ascii + ONES * 0x16) & !(ascii + ONES * 0x05));
    (case & !(HIGHS >> 2)) | ((case << 2) & !(letter & !lowered)) == 0
}
/// `strncasecmp(left, right, length) == 0` in the current locale, over at most the
/// shorter slice; a matching NUL ends the comparison as it does in libc. Compared
/// here a word at a time, the second of two overlapping the first, rather than by
/// glibc's vectorised variant, whose instruction count depends on where the
/// operands sit. When every word is [`alike`], every byte pair compares equal,
/// wherever a NUL falls; otherwise libc's loop decides.
#[inline]
#[must_use]
pub fn strncasecmp_equal(left: &[u8], right: &[u8], length: usize) -> bool {
    let length = length.min(left.len()).min(right.len());
    let (left, right) = (left.split_at(length).0, right.split_at(length).0);
    match length {
        0..=8 => alike(word(left), word(right)) || bytewise(left, right),
        9..=16 => {
            alike(word(left), word(right)) & alike(last_word(left), last_word(right))
                || bytewise(left, right)
        }
        _ => longer(left, right),
    }
}
/// [`strncasecmp_equal`] over more than sixteen bytes.
#[inline(never)]
fn longer(left: &[u8], right: &[u8]) -> bool {
    left.chunks(8)
        .zip(right.chunks(8))
        .all(|(left, right)| alike(word(left), word(right)))
        || bytewise(left, right)
}
/// `isspace(byte)` in the current locale, the byte passed as a native `char`, as the
/// pinned C passes it.
#[must_use]
pub fn is_space(byte: u8) -> bool {
    isspace(byte.into()) != 0
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
    fn is_space_matches_libc() {
        for byte in 0..=u8::MAX {
            let native = c_int::from(c_char::from_ne_bytes([byte]));
            // SAFETY: a native `char`'s value, which libc's table indexes.
            let libc = unsafe { libc::isspace(native) } != 0;
            assert_eq!(is_space(byte), libc, "{byte:#x}");
        }
    }
    #[test]
    fn case_comparison_matches_libc_in_each_locale() {
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
                    // At places in names that fold alike, compared over under four,
                    // four to eight, nine to sixteen and more bytes, followed by a
                    // NUL, an equal or a differing byte on either side, so that a
                    // matching NUL's end is checked too.
                    for at in [0, 1, 3, 5, 7, 8, 11, 15, 16, 19] {
                        for (next_left, next_right) in
                            [(0, 0), (0, b'x'), (b'x', 0), (b'a', b'A'), (b'a', b'b')]
                        {
                            let (mut input, mut expected) =
                                (*b"Accept-Content-Type!", *b"aCCEPT-cONTENT-tYPE!");
                            input[at] = left;
                            expected[at] = right;
                            if let (Some(input), Some(expected)) =
                                (input.get_mut(at + 1), expected.get_mut(at + 1))
                            {
                                (*input, *expected) = (next_left, next_right);
                            }
                            for length in [at + 1, at + 2, 20] {
                                // SAFETY: twenty readable bytes each, `length` at most 20.
                                let libc = unsafe {
                                    libc::strncasecmp(
                                        input.as_ptr().cast(),
                                        expected.as_ptr().cast(),
                                        length.min(20),
                                    )
                                } == 0;
                                assert_eq!(
                                    strncasecmp_equal(&input, &expected, length),
                                    libc,
                                    "{input:x?} {expected:x?} {length} in {locale:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
        // SAFETY: restores the default locale.
        unsafe { libc::setlocale(libc::LC_CTYPE, c"C".as_ptr()) };
    }
}
