// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! `swoole_random_string`: alphanumerics drawn one at a time with `swoole_rand(0, 61)`.
use crate::host::swoole_rand;
use std::ffi::{c_char, c_int, c_void};
use std::slice;

/// Upstream's `characters` table.
const CHARACTERS: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/// `sizeof(characters) - 1`.
const LAST: c_int = 61;

/// `characters[draw(0, 61)]`. A draw is always in range; the remainder only keeps the
/// lookup total.
fn character(draw: &mut impl FnMut(c_int, c_int) -> c_int) -> u8 {
    let index = draw(0, LAST).rem_euclid(LAST + 1).unsigned_abs() as usize;
    CHARACTERS.get(index).copied().unwrap_or(CHARACTERS[0])
}

/// The buffer overload over a draw source: a character per byte but the last, then
/// a NUL there.
fn fill(buffer: &mut [u8], draw: &mut impl FnMut(c_int, c_int) -> c_int) {
    if let Some((nul, characters)) = buffer.split_last_mut() {
        for byte in characters {
            *byte = character(draw);
        }
        *nul = 0;
    }
}

/// `void swoole_random_string(char *buf, size_t len)`: `len` characters, then a NUL.
/// # Safety
/// `buf` is writable for `len + 1` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_random_stringPcm")
)]
pub unsafe extern "C" fn swoole_random_string(buf: *mut c_char, len: usize) {
    // SAFETY: the caller lends `len + 1` writable bytes.
    let buffer = unsafe { slice::from_raw_parts_mut(buf.cast::<u8>(), len + 1) };
    fill(buffer, &mut |low, high| swoole_rand(low, high));
}

/// One character for the `std::string` overload, which appends them in C++ (its
/// mangled name differs between libc++ and libstdc++).
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_corebase_random_char() -> c_char {
    c_char::from_ne_bytes([character(&mut |low, high| swoole_rand(low, high))])
}

/// `swoole_random_bytes`: fills `buf` from the system CSPRNG, retrying
/// interrupted calls; returns how many bytes it filled.
/// # Safety
/// `buf` points to `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_random_bytes(buf: *mut c_void, size: usize) -> usize {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut done = 0;
        while done < size {
            // SAFETY: `done < size`, so the range stays inside the caller's buffer.
            let at = unsafe { buf.byte_add(done) };
            // SAFETY: `at` has `size - done` writable bytes.
            let n = unsafe { libc::getrandom(at, size - done, 0) };
            if n == -1 {
                if matches!(
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
                    libc::EINTR | libc::EAGAIN
                ) {
                    continue;
                }
                break;
            }
            done += n.unsigned_abs();
        }
        done
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // SAFETY: `buf` has `size` writable bytes.
        unsafe { libc::arc4random_buf(buf, size) };
        size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_fill_the_buffer() {
        let mut buf = [0_u8; 64];
        assert_eq!(
            unsafe { swr_v1_os_random_bytes(buf.as_mut_ptr().cast(), 64) },
            64,
            "all requested bytes"
        );
        assert!(buf.iter().any(|&b| b != 0), "not left zeroed");
        assert_eq!(
            unsafe { swr_v1_os_random_bytes(buf.as_mut_ptr().cast(), 0) },
            0,
            "empty request"
        );
    }

    /// Upstream's `characters` table, spelled out independently of `CHARACTERS`.
    const UPSTREAM: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    /// A scripted `swoole_rand`: draw `n` of a stream returns `(seed + n) % 62` and
    /// records its bounds.
    struct Script {
        seed: usize,
        draws: Vec<(c_int, c_int)>,
    }

    impl Script {
        fn draw(&mut self) -> impl FnMut(c_int, c_int) -> c_int + '_ {
            |low, high| {
                let value = (self.seed + self.draws.len()) % 62;
                self.draws.push((low, high));
                c_int::try_from(value).unwrap()
            }
        }
    }

    /// The multipart boundary: 24 random bytes after `SwooleBoundary`, through both
    /// overloads, for 62 seeds (the old http-boundary-rng gate's cases).
    #[test]
    fn boundary_overloads_over_scripted_draws() {
        for seed in 0..62 {
            // The std::string overload appends one character per draw.
            let mut append = Script {
                seed,
                draws: Vec::new(),
            };
            let mut boundary = b"SwooleBoundary".to_vec();
            {
                let mut draw = append.draw();
                for _ in 0..24 {
                    boundary.push(character(&mut draw));
                }
            }
            assert_eq!(append.draws.len(), 24, "positive append draw count");

            let mut buffer_script = Script {
                seed,
                draws: Vec::new(),
            };
            let mut buffer = [0xa5_u8; 39];
            fill(&mut buffer[..25], &mut buffer_script.draw());
            assert!(
                buffer_script.draws.len() == 24 && buffer_script.draws == append.draws,
                "positive buffer draw sequence"
            );
            assert_eq!(buffer[24], 0, "positive buffer terminator");
            assert!(
                buffer[25..].iter().all(|&byte| byte == 0xa5),
                "untouched scratch suffix"
            );
            assert_eq!(
                boundary,
                [b"SwooleBoundary".as_slice(), &buffer[..24]].concat(),
                "append/buffer boundary byte equivalence"
            );
            let expected: Vec<u8> = (0..24).map(|i| UPSTREAM[(seed + i) % 62]).collect();
            assert_eq!(&boundary[14..], expected, "pinned/production result");
            assert_eq!(boundary.len(), 38, "normal multipart boundary size");
            assert!(
                append.draws.iter().all(|&draw| draw == (0, 61)),
                "frozen RNG bounds"
            );
        }
    }

    #[test]
    fn empty_buffer_gets_only_the_nul() {
        let mut script = Script {
            seed: 0,
            draws: Vec::new(),
        };
        let mut buffer = [0xa5_u8; 2];
        fill(&mut buffer[..1], &mut script.draw());
        assert_eq!(buffer, [0, 0xa5]);
        assert_eq!(script.draws.len(), 0);
    }

    #[test]
    fn exported_overloads_draw_from_the_table() {
        assert_eq!(CHARACTERS, UPSTREAM);
        let mut buffer = [0xa5_u8; 66];
        unsafe { swoole_random_string(buffer.as_mut_ptr().cast(), 64) };
        assert!(buffer[..64].iter().all(|byte| UPSTREAM.contains(byte)));
        assert_eq!(buffer[64..], [0, 0xa5]);
        let byte = swr_v1_corebase_random_char().to_ne_bytes()[0];
        assert!(UPSTREAM.contains(&byte));
    }
}
