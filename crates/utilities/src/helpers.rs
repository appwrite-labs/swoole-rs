// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 include/swoole.h and include/swoole_util.h.
//! Header helpers that need no inlining: `swoole::BitMap`'s storage,
//! `swoole::get_timezone` and `swoole_strlchr`.

use std::ffi::{c_char, c_long};

/// `swoole::BitMap`: a fixed number of bits, all clear at creation.
pub struct Bitmap {
    words: Box<[u64]>,
    bits: usize,
}

impl Bitmap {
    fn locate(&self, bit: usize) -> Option<(usize, u64)> {
        (bit < self.bits).then(|| (bit / 64, 1u64 << (bit % 64)))
    }
}

/// A bitmap of `bits` clear bits; the caller frees it with
/// [`swr_v1_utils_bitmap_destroy`].
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_bitmap_create(bits: usize) -> *mut Bitmap {
    let words = vec![0; bits.div_ceil(64)].into_boxed_slice();
    Box::into_raw(Box::new(Bitmap { words, bits }))
}

/// # Safety
/// `bitmap` is null or came from [`swr_v1_utils_bitmap_create`] and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_bitmap_destroy(bitmap: *mut Bitmap) {
    if !bitmap.is_null() {
        // SAFETY: the caller gives back the box `create` leaked.
        drop(unsafe { Box::from_raw(bitmap) });
    }
}

/// # Safety
/// `bitmap` is null or live, with no other reference in use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_bitmap_clear(bitmap: *mut Bitmap) {
    // SAFETY: the caller lends null or an exclusive live bitmap.
    if let Some(bitmap) = unsafe { bitmap.as_mut() } {
        bitmap.words.fill(0);
    }
}

/// Set (`value`) or clear bit `bit`; bits past the end are ignored.
/// # Safety
/// As [`swr_v1_utils_bitmap_clear`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_bitmap_set(bitmap: *mut Bitmap, bit: usize, value: bool) {
    // SAFETY: the caller lends null or an exclusive live bitmap.
    let Some(bitmap) = (unsafe { bitmap.as_mut() }) else {
        return;
    };
    if let Some((word, mask)) = bitmap.locate(bit)
        && let Some(word) = bitmap.words.get_mut(word)
    {
        if value {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }
}

/// Whether bit `bit` is set; false past the end.
/// # Safety
/// `bitmap` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_bitmap_get(bitmap: *const Bitmap, bit: usize) -> bool {
    // SAFETY: the caller lends null or a live bitmap.
    let Some(bitmap) = (unsafe { bitmap.as_ref() }) else {
        return false;
    };
    bitmap
        .locate(bit)
        .and_then(|(word, mask)| Some(bitmap.words.get(word)? & mask != 0))
        .unwrap_or(false)
}

/// `swoole::get_timezone`: the local zone's offset from UTC now, in seconds.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_timezone() -> c_long {
    // SAFETY: time(3) accepts a null output.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
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
    fn bitmap_sets_gets_unsets_and_clears() {
        let m = swr_v1_utils_bitmap_create(4096);
        unsafe {
            for bit in [199, 1234, 3048, 4095] {
                swr_v1_utils_bitmap_set(m, bit, true);
            }
            for bit in [199, 1234, 3048, 4095] {
                assert!(swr_v1_utils_bitmap_get(m, bit), "{bit} set");
            }
            assert!(!swr_v1_utils_bitmap_get(m, 2048), "untouched");
            assert!(!swr_v1_utils_bitmap_get(m, 4096), "past the end");
            swr_v1_utils_bitmap_set(m, 4096, true);
            swr_v1_utils_bitmap_set(m, 1234, false);
            assert!(!swr_v1_utils_bitmap_get(m, 1234), "unset");
            swr_v1_utils_bitmap_clear(m);
            assert!(!swr_v1_utils_bitmap_get(m, 199), "cleared");
            swr_v1_utils_bitmap_destroy(m);
        }
    }

    #[test]
    fn timezone_matches_the_local_offset_bounds() {
        let offset = swr_v1_utils_timezone();
        assert!((-14 * 3600..=14 * 3600).contains(&offset), "{offset}");
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
