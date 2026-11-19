//! Pinned utility arithmetic and logging byte policy; no native callbacks.
use crate::CCast;
mod helpers;
pub use helpers::timezone;
mod logger;
mod sni;
pub use sni::sni_wildcard_matches;
pub mod misc;
mod wait_registry;
pub use logger::*;
pub use wait_registry::{
    swr_v1_utils_wait_registry_init, swr_v1_utils_wait_registry_size, wait_registry_init,
};

use std::sync::atomic::{AtomicU32, Ordering};

/// Upstream's `move_first_element`: the first argument moves to `position`.
/// With `position` or fewer arguments the first argument moves to the end, and
/// the target function reports the missing arguments itself.
pub fn move_first_argument<T>(args: &mut [T], position: usize) {
    let Some(last) = args.len().checked_sub(1) else {
        return;
    };
    if let Some(moved) = args.get_mut(..=position.min(last)) {
        moved.rotate_left(1);
    }
}

fn promoted(byte: u8, signed: bool) -> u64 {
    if signed {
        i64::from(byte.cast_signed()).cast_unsigned()
    } else {
        u64::from(byte)
    }
}
fn word<'a>(bytes: impl IntoIterator<Item = &'a u8>, signed: bool) -> u32 {
    bytes.into_iter().enumerate().fold(0u32, |sum, (i, byte)| {
        sum.wrapping_add(
            promoted(*byte, signed)
                .c_cast::<u32>()
                .wrapping_shl((8 * i).c_cast::<u32>()),
        )
    })
}
fn mix(a: &mut u32, b: &mut u32, c: &mut u64) {
    *a = a.wrapping_sub(*b).wrapping_sub((*c).c_cast::<u32>());
    *a ^= (*c >> 13).c_cast::<u32>();
    *b = b.wrapping_sub((*c).c_cast::<u32>()).wrapping_sub(*a);
    *b ^= a.wrapping_shl(8);
    *c = c.wrapping_sub(u64::from(*a)).wrapping_sub(u64::from(*b));
    *c ^= u64::from(*b >> 13);
    *a = a.wrapping_sub(*b).wrapping_sub((*c).c_cast::<u32>());
    *a ^= (*c >> 12).c_cast::<u32>();
    *b = b.wrapping_sub((*c).c_cast::<u32>()).wrapping_sub(*a);
    *b ^= a.wrapping_shl(16);
    *c = c.wrapping_sub(u64::from(*a)).wrapping_sub(u64::from(*b));
    *c ^= u64::from(*b >> 5);
    *a = a.wrapping_sub(*b).wrapping_sub((*c).c_cast::<u32>());
    *a ^= (*c >> 3).c_cast::<u32>();
    *b = b.wrapping_sub((*c).c_cast::<u32>()).wrapping_sub(*a);
    *b ^= a.wrapping_shl(10);
    *c = c.wrapping_sub(u64::from(*a)).wrapping_sub(u64::from(*b));
    *c ^= u64::from(*b >> 15);
}
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
                    promoted(*byte, signed)
                        .c_cast::<u32>()
                        .wrapping_shl((8 * (i - 7)).c_cast::<u32>()),
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
                    let shifted = promoted(*byte, signed)
                        .c_cast::<u32>()
                        .wrapping_shl((8 * i).c_cast::<u32>())
                        .c_cast::<i32>();
                    k |= i64::from(shifted).c_cast::<u64>();
                }
                k = k.wrapping_mul(0x5bd1_e995);
                k ^= k >> 24;
                k = k.wrapping_mul(0x5bd1_e995);
                h = h.wrapping_mul(0x5bd1_e995) ^ k;
            }
            for (i, byte) in tail.iter().enumerate() {
                h ^= (i64::from(
                    promoted(*byte, signed)
                        .c_cast::<u32>()
                        .wrapping_shl((8 * i).c_cast::<u32>())
                        .c_cast::<i32>(),
                ))
                .c_cast::<u64>();
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
    let prefix = bytes.len().c_cast::<u32>() as usize;
    let count = if algorithm == 1 { prefix } else { bytes.len() };
    hash(
        bytes.get(..count).unwrap_or_default(),
        bytes.len(),
        algorithm,
        signed_char,
    )
}

/// Native-compatible process-shared spinlock. Unlock remains the native release store.
pub fn swr_v1_utils_spinlock(lock: Option<&AtomicU32>, cpus: u32, backoff_limit: u32) {
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

const SW_LOG_DEBUG: i32 = crate::log::LogLevel::SW_LOG_DEBUG as i32;
const SW_LOG_TRACE: i32 = crate::log::LogLevel::SW_LOG_TRACE as i32;
const SW_LOG_INFO: i32 = crate::log::LogLevel::SW_LOG_INFO as i32;
const SW_LOG_NOTICE: i32 = crate::log::LogLevel::SW_LOG_NOTICE as i32;
const SW_LOG_WARNING: i32 = crate::log::LogLevel::SW_LOG_WARNING as i32;
const SW_LOG_ERROR: i32 = crate::log::LogLevel::SW_LOG_ERROR as i32;
const SW_LOG_NONE: i32 = crate::log::LogLevel::SW_LOG_NONE as i32;

/// Clamp the configured native logging threshold to its defined enum range.
pub(crate) fn log_level(level: i32) -> i32 {
    level.clamp(SW_LOG_DEBUG, SW_LOG_NONE)
}

fn log_label(level: i32) -> &'static std::ffi::CStr {
    match level {
        SW_LOG_DEBUG => c"DEBUG",
        SW_LOG_TRACE => c"TRACE",
        SW_LOG_NOTICE => c"NOTICE",
        SW_LOG_WARNING => c"WARNING",
        SW_LOG_ERROR => c"ERROR",
        _ => c"INFO",
    }
}

/// The rotation date format; unknown modes preserve the native daily fallback.
fn log_rotation(rotation: i32) -> &'static std::ffi::CStr {
    use crate::log::rotation;
    match rotation {
        rotation::MONTHLY => c"%Y%m",
        rotation::HOURLY => c"%Y%m%d%H",
        rotation::EVERY_MINUTE => c"%Y%m%d%H%M",
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
pub(crate) mod isolated_process {
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

#[cfg(test)]
mod argument_order {
    use super::move_first_argument;

    fn moved(count: usize, position: usize) -> Vec<usize> {
        let mut args: Vec<_> = (0..count).collect();
        move_first_argument(&mut args, position);
        args
    }

    #[test]
    fn first_argument_moves_to_its_position() {
        assert_eq!(moved(3, 1), [1, 0, 2]);
        assert_eq!(moved(4, 2), [1, 2, 0, 3]);
        assert_eq!(moved(3, 2), [1, 2, 0]);
        assert_eq!(moved(1, 1), [0], "too few arguments: to the end");
        assert_eq!(moved(2, 2), [1, 0]);
        assert_eq!(moved(0, 1), Vec::<usize>::new());
    }
}
