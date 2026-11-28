//! Pinned utility arithmetic and logging byte policy; no native callbacks.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod frontdoor;
pub use frontdoor::*;
mod helpers;
pub use helpers::timezone;
mod logger;
mod sni;
pub use sni::sni_wildcard_matches;
pub mod misc;
mod wait_registry;
pub use logger::*;
pub use wait_registry::{swr_v1_utils_wait_registry_init, swr_v1_utils_wait_registry_size};

use std::sync::atomic::{AtomicU32, Ordering};

fn promoted(byte: u8, signed: bool) -> u64 {
    if signed {
        i64::from(byte.cast_signed()).cast_unsigned()
    } else {
        u64::from(byte)
    }
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "the native hashes' 32-bit mixing truncates and reinterprets by design"
)]
fn word<'a>(bytes: impl IntoIterator<Item = &'a u8>, signed: bool) -> u32 {
    bytes.into_iter().enumerate().fold(0u32, |sum, (i, byte)| {
        sum.wrapping_add((promoted(*byte, signed) as u32).wrapping_shl((8 * i) as u32))
    })
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "the native hashes' 32-bit mixing truncates and reinterprets by design"
)]
fn mix(a: &mut u32, b: &mut u32, c: &mut u64) {
    *a = a.wrapping_sub(*b).wrapping_sub(*c as u32);
    *a ^= (*c >> 13) as u32;
    *b = b.wrapping_sub(*c as u32).wrapping_sub(*a);
    *b ^= a.wrapping_shl(8);
    *c = c.wrapping_sub(u64::from(*a)).wrapping_sub(u64::from(*b));
    *c ^= u64::from(*b >> 13);
    *a = a.wrapping_sub(*b).wrapping_sub(*c as u32);
    *a ^= (*c >> 12) as u32;
    *b = b.wrapping_sub(*c as u32).wrapping_sub(*a);
    *b ^= a.wrapping_shl(16);
    *c = c.wrapping_sub(u64::from(*a)).wrapping_sub(u64::from(*b));
    *c ^= u64::from(*b >> 5);
    *a = a.wrapping_sub(*b).wrapping_sub(*c as u32);
    *a ^= (*c >> 3) as u32;
    *b = b.wrapping_sub(*c as u32).wrapping_sub(*a);
    *b ^= a.wrapping_shl(10);
    *c = c.wrapping_sub(u64::from(*a)).wrapping_sub(u64::from(*b));
    *c ^= u64::from(*b >> 15);
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the native hashes' 32-bit mixing truncates and reinterprets by design"
)]
fn hash(bytes: &[u8], length: usize, algorithm: u32, signed: bool) -> u64 {
    match algorithm {
        0 => bytes.iter().fold(5381u64, |h, b| {
            h.wrapping_mul(33).wrapping_add(promoted(*b, signed))
        }),
        1 => {
            let (mut a, mut b, mut c) = (0x9e37_79b9_u32, 0x9e37_79b9_u32, 0xfeed_beef_u64);
            let (blocks, rest) = bytes.as_chunks::<12>();
            for block in blocks {
                let (words, _) = block.as_chunks::<4>();
                if let [x, y, z] = words {
                    a = a.wrapping_add(word(x, signed));
                    b = b.wrapping_add(word(y, signed));
                    c = c.wrapping_add(u64::from(word(z, signed)));
                }
                mix(&mut a, &mut b, &mut c);
            }
            c = c.wrapping_add(length as u64);
            a = a.wrapping_add(word(rest.iter().take(4), signed));
            b = b.wrapping_add(word(rest.iter().skip(4).take(4), signed));
            for (i, byte) in rest.iter().enumerate().skip(8) {
                c = c.wrapping_add(u64::from(
                    (promoted(*byte, signed) as u32).wrapping_shl((8 * (i - 7)) as u32),
                ));
            }
            mix(&mut a, &mut b, &mut c);
            c
        }
        2 => {
            let mut h = length as u64;
            let (chunks, tail) = bytes.as_chunks::<4>();
            for part in chunks {
                let mut k = promoted(part[0], signed);
                for (i, byte) in part.iter().enumerate().skip(1) {
                    let shifted =
                        (promoted(*byte, signed) as u32).wrapping_shl((8 * i) as u32) as i32;
                    k |= i64::from(shifted) as u64;
                }
                k = k.wrapping_mul(0x5bd1_e995);
                k ^= k >> 24;
                k = k.wrapping_mul(0x5bd1_e995);
                h = h.wrapping_mul(0x5bd1_e995) ^ k;
            }
            for (i, byte) in tail.iter().enumerate() {
                h ^= i64::from((promoted(*byte, signed) as u32).wrapping_shl((8 * i) as u32) as i32)
                    as u64;
            }
            if !tail.is_empty() {
                h = h.wrapping_mul(0x5bd1_e995);
            }
            h ^= h >> 13;
            h = h.wrapping_mul(0x5bd1_e995);
            h ^ (h >> 15)
        }
        _ => 0,
    }
}

/// Pinned LP64 hash variants (0=PHP, 1=Jenkins, 2=Austin) of `bytes`, promoting
/// each byte as a signed or unsigned `char`; an unknown algorithm hashes to 0.
#[must_use]
pub fn swoole_hash(bytes: &[u8], algorithm: u32, signed_char: bool) -> u64 {
    // Native Jenkins consumes an unsigned-width prefix but mixes the full length.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native Jenkins' unsigned-width prefix"
    )]
    let prefix = bytes.len() as u32 as usize;
    let count = if algorithm == 1 { prefix } else { bytes.len() };
    hash(
        bytes.get(..count).unwrap_or_default(),
        bytes.len(),
        algorithm,
        signed_char,
    )
}

/// Native-compatible process-shared spinlock. Unlock remains the native release store.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_spinlock(lock: Option<&AtomicU32>, cpus: u32, backoff_limit: u32) {
    let Some(lock) = lock else {
        return;
    };
    let acquire = || {
        lock.load(Ordering::Relaxed) == 0
            && lock
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    };
    loop {
        if acquire() {
            return;
        }
        if cpus > 1 {
            let mut n = 1u32;
            while n < backoff_limit {
                for _ in 0..n {
                    std::hint::spin_loop();
                }
                if acquire() {
                    return;
                }
                let Some(next) = n.checked_mul(2) else { break };
                n = next;
            }
        }
        std::thread::yield_now();
    }
}

/// Futex wait on the native atomic utility's word (`HAVE_FUTEX` builds only).
#[cfg(target_os = "linux")]
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_wait(word: Option<&AtomicU32>, timeout: f64) -> i32 {
    swoole_rs_locks::swr_v1_locks_wait(word, timeout)
}

/// Futex wake for the native atomic utility's waiters.
#[cfg(target_os = "linux")]
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_wakeup(word: Option<&AtomicU32>, n: i32) -> i32 {
    swoole_rs_locks::swr_v1_locks_wakeup(word, n)
}

/// Return the exact pinned pretty-function substring as a byte span.
/// # Safety
/// Input is borrowed/readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_pretty_range(
    data: *const u8,
    length: usize,
    strip: u8,
    begin: Option<&mut usize>,
    count: Option<&mut usize>,
) -> i32 {
    let (Some(begin), Some(count)) = (begin, count) else {
        return -1;
    };
    *begin = 0;
    *count = 0;
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    let bytes = if length == 0 {
        &[]
    } else {
        // SAFETY: non-null, `length <= isize::MAX` (checked above), readable bytes.
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    (*begin, *count) = pretty_range(bytes, strip != 0);
    0
}

/// `Logger::get_pretty_name`'s substring of `name` as (offset, length).
fn pretty_range(name: &[u8], strip: bool) -> (usize, usize) {
    let Some(bracket) = name.iter().position(|b| *b == b'(') else {
        return (0, 0);
    };
    let head = name.get(..bracket).unwrap_or_default();
    let mut start = head.iter().rposition(|b| *b == b' ').map_or(0, |p| p + 1);
    let method = head.get(start..).unwrap_or_default();
    if strip && let Some(last) = method.windows(2).rposition(|w| w == b"::") {
        // Native rfind skips overlapping separators; a lone separator strips
        // itself, including the original wrapped search at positions zero/one.
        let prior = method
            .get(..last)
            .and_then(|head| head.windows(2).rposition(|w| w == b"::"));
        start += prior.unwrap_or(last) + 2;
    }
    (start, bracket - start)
}

/// Clamp the configured native logging threshold to its defined enum range.
pub(crate) fn log_level(level: i32) -> i32 {
    level.clamp(0, 6)
}

fn log_label(level: i32) -> &'static std::ffi::CStr {
    match level {
        0 => c"DEBUG",
        1 => c"TRACE",
        3 => c"NOTICE",
        4 => c"WARNING",
        5 => c"ERROR",
        _ => c"INFO",
    }
}

/// The rotation date format; unknown modes preserve the native daily fallback.
fn log_rotation(rotation: i32) -> &'static std::ffi::CStr {
    match rotation {
        1 => c"%Y%m",
        3 => c"%Y%m%d%H",
        4 => c"%Y%m%d%H%M",
        _ => c"%Y%m%d",
    }
}

struct LogSink<'a> {
    bytes: &'a mut [u8],
    required: usize,
}
impl LogSink<'_> {
    fn finish(self) -> usize {
        if self.required > i32::MAX as usize {
            if let Some(first) = self.bytes.first_mut() {
                *first = 0;
            }
            return usize::MAX;
        }
        let written = self.required.min(self.bytes.len().saturating_sub(1));
        let Some(terminator) = self.bytes.get_mut(written) else {
            return self.required;
        };
        *terminator = 0;
        written
    }
    fn append(&mut self, bytes: &[u8]) {
        let start = self.required.min(self.bytes.len().saturating_sub(1));
        let count = bytes.len().min(self.bytes.len().saturating_sub(1) - start);
        for (output, byte) in self.bytes.iter_mut().skip(start).take(count).zip(bytes) {
            *output = *byte;
        }
        self.required = self.required.saturating_add(bytes.len());
    }
}
impl std::fmt::Write for LogSink<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.append(value.as_bytes());
        Ok(())
    }
}

/// `bytes` through its first NUL, as printf's `%.*s` reads it.
fn through_nul(bytes: &[u8]) -> &[u8] {
    memchr::memchr(0, bytes).map_or(bytes, |end| bytes.get(..end).unwrap_or_default())
}

/// The pinned `Logger::put` line in `output`, NUL-terminated and clamped, with
/// `date` and `content` read through their first NUL: the full length, or
/// `usize::MAX` once it no longer fits an `int`.
pub(crate) fn log_line(
    output: &mut [u8],
    date: &[u8],
    symbol: u8,
    pid: i32,
    worker: i32,
    level: i32,
    content: &[u8],
) -> usize {
    use std::fmt::Write;
    let mut sink = LogSink {
        bytes: output,
        required: 0,
    };
    sink.append(b"[");
    sink.append(through_nul(date));
    sink.append(&[b' ', symbol]);
    #[expect(
        clippy::let_underscore_must_use,
        reason = "integer Display has no fallible callbacks and this sink never returns Err"
    )]
    let _ = write!(sink, "{pid}.{worker}]\t");
    sink.append(log_label(level).to_bytes());
    sink.append(b"\t");
    sink.append(through_nul(content));
    sink.append(b"\n");
    sink.finish()
}

/// The logger's `<.microseconds>` suffix in `output`, as [`log_line`] writes a line.
pub(crate) fn log_micros(output: &mut [u8], microseconds: i64) -> usize {
    use std::fmt::Write;
    let mut sink = LogSink {
        bytes: output,
        required: 0,
    };
    #[expect(
        clippy::let_underscore_must_use,
        reason = "integer Display has no fallible callbacks and this sink never returns Err"
    )]
    let _ = write!(sink, "<.{microseconds}>");
    sink.finish()
}

// Every test here calls the C++ oracle, which Miri can't run.
#[cfg(test)]
#[cfg(not(miri))]
mod upstream;

#[cfg(test)]
#[expect(
    dead_code,
    reason = "utilities runs whole tests in a child; replacement_survives is ossupport's"
)]
mod isolated_process {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/isolated_process.rs"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hash_empty_and_binary_signedness() {
        assert_eq!(hash(&[], 0, 0, true), 5381);
        assert_eq!(hash(&[], 0, 2, true), 0);
        assert_ne!(hash(&[255], 1, 0, true), hash(&[255], 1, 0, false));
        assert_eq!(hash(b"hello", 5, 0, true), 210_714_636_441);
    }
}
