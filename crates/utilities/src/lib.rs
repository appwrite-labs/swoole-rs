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
pub use helpers::{swr_v1_utils_strlchr, timezone};
mod logger;
mod sni;
pub use sni::sni_wildcard_matches;
pub mod misc;
mod wait_registry;
pub use logger::*;
pub use wait_registry::{swr_v1_utils_wait_registry_init, swr_v1_utils_wait_registry_size};

use std::mem::MaybeUninit;
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

/// Pinned LP64 hash variants (0=PHP, 1=Jenkins, 2=Austin).
/// # Safety
/// Data is readable for length bytes, null only for empty input. No pointer retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_hash(
    data: *const u8,
    length: usize,
    algorithm: u32,
    signed_char: u8,
) -> u64 {
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        return 0;
    }
    // Native Jenkins consumes an unsigned-width prefix but mixes the full length.
    let count = if algorithm == 1 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "native Jenkins' unsigned-width prefix"
        )]
        let prefix = length as u32 as usize;
        prefix
    } else {
        length
    };
    let bytes = if count == 0 {
        &[]
    } else {
        // SAFETY: non-null, `count <= length <= isize::MAX` (checked above), readable bytes.
        unsafe { std::slice::from_raw_parts(data, count) }
    };
    hash(bytes, length, algorithm, signed_char != 0)
}

/// Native-compatible process-shared spinlock. Unlock remains the native release store.
/// # Safety
/// Lock is a valid aligned native u32 atomic, accessed atomically by all participants.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_spinlock(lock: *mut u32, cpus: u32, backoff_limit: u32) {
    if lock.is_null() || !(lock as usize).is_multiple_of(std::mem::align_of::<AtomicU32>()) {
        return;
    }
    // SAFETY: non-null and aligned (checked above); a live u32 accessed only atomically.
    let lock = unsafe { &*lock.cast::<AtomicU32>() };
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
/// # Safety
/// Word is aligned, live u32 atomic storage shared only through atomic accesses.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_wait(word: *mut u32, timeout: f64) -> i32 {
    // SAFETY: forwards the caller's aligned, live, atomic-only futex word.
    unsafe { swoole_rs_locks::swr_v1_locks_wait(word, timeout) }
}

/// Futex wake for the native atomic utility's waiters.
/// # Safety
/// Word has the same alignment, lifetime and atomic-access requirements as wait.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_wakeup(word: *mut u32, n: i32) -> i32 {
    // SAFETY: forwards the caller's aligned, live, atomic-only futex word.
    unsafe { swoole_rs_locks::swr_v1_locks_wakeup(word, n) }
}

/// Return the exact pinned pretty-function substring as a byte span.
/// # Safety
/// Input is borrowed/readable; both outputs are aligned, writable and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_pretty_range(
    data: *const u8,
    length: usize,
    strip: u8,
    begin: *mut usize,
    count: *mut usize,
) -> i32 {
    if begin.is_null() || count.is_null() {
        return -1;
    }
    // SAFETY: non-null (checked above), aligned, writable outputs.
    unsafe { begin.write(0) };
    // SAFETY: as above.
    unsafe { count.write(0) };
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    let bytes = if length == 0 {
        &[]
    } else {
        // SAFETY: non-null, `length <= isize::MAX` (checked above), readable bytes.
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    let Some(bracket) = bytes.iter().position(|b| *b == b'(') else {
        return 0;
    };
    let head = bytes.get(..bracket).unwrap_or_default();
    let mut start = head.iter().rposition(|b| *b == b' ').map_or(0, |p| p + 1);
    let method = head.get(start..).unwrap_or_default();
    if strip != 0
        && let Some(last) = method.windows(2).rposition(|w| w == b"::")
    {
        // Native rfind skips overlapping separators; a lone separator strips
        // itself, including the original wrapped search at positions zero/one.
        let prior = method
            .get(..last)
            .and_then(|head| head.windows(2).rposition(|w| w == b"::"));
        start += prior.unwrap_or(last) + 2;
    }
    // SAFETY: non-null (checked above), aligned, writable outputs.
    unsafe { begin.write(start) };
    // SAFETY: as above.
    unsafe { count.write(bracket - start) };
    0
}

/// Clamp the configured native logging threshold to its defined enum range.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_log_level(level: i32) -> i32 {
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

/// Static NUL-terminated log label; unknown levels preserve the native INFO fallback.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_log_label(level: i32) -> *const std::ffi::c_char {
    log_label(level).as_ptr()
}

/// Static rotation date format; unknown modes preserve the native daily fallback.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_utils_log_rotation(rotation: i32) -> *const std::ffi::c_char {
    match rotation {
        1 => c"%Y%m",
        3 => c"%Y%m%d%H",
        4 => c"%Y%m%d%H%M",
        _ => c"%Y%m%d",
    }
    .as_ptr()
}

struct LogSink<'a> {
    bytes: &'a mut [MaybeUninit<u8>],
    required: usize,
}
impl LogSink<'_> {
    fn finish(self) -> usize {
        if self.required > i32::MAX as usize {
            if let Some(first) = self.bytes.first_mut() {
                first.write(0);
            }
            return usize::MAX;
        }
        let written = self.required.min(self.bytes.len().saturating_sub(1));
        let Some(terminator) = self.bytes.get_mut(written) else {
            return self.required;
        };
        terminator.write(0);
        written
    }
    fn append(&mut self, bytes: &[u8]) {
        let start = self.required.min(self.bytes.len().saturating_sub(1));
        let count = bytes.len().min(self.bytes.len().saturating_sub(1) - start);
        for (output, byte) in self.bytes.iter_mut().skip(start).take(count).zip(bytes) {
            output.write(*byte);
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

// printf precision is a signed int: negative means read through the terminator.
unsafe fn log_bytes<'a>(data: *const u8, precision: i32) -> &'a [u8] {
    if precision == 0 {
        return &[];
    }
    if precision < 0 {
        // SAFETY: negative precision requires a NUL-terminated input.
        return unsafe { std::ffi::CStr::from_ptr(data.cast()) }.to_bytes();
    }
    let (mut length, limit) = (0, precision.unsigned_abs() as usize);
    // SAFETY: the input is readable through its first NUL or `precision` bytes.
    while length < limit && unsafe { data.wrapping_add(length).read() } != 0 {
        length += 1;
    }
    // SAFETY: the loop read exactly these `length` bytes.
    unsafe { std::slice::from_raw_parts(data, length) }
}

/// Format the pinned `Logger::put` line without allocation or native callbacks.
/// # Safety
/// Output is writable for capacity bytes and disjoint from inputs. Each input is
/// readable through its first NUL or nonnegative precision, whichever comes first;
/// negative precision requires a terminating NUL. Null is valid only at precision0.
/// Capacity must fit i32; zero capacity permits null output and queries full size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_line(
    output: *mut u8,
    capacity: usize,
    date: *const u8,
    date_precision: i32,
    symbol: u8,
    pid: i32,
    worker: i32,
    level: i32,
    content: *const u8,
    content_precision: i32,
) -> usize {
    use std::fmt::Write;
    if capacity > i32::MAX as usize || (capacity != 0 && output.is_null()) {
        return 0;
    }
    if (date.is_null() && date_precision != 0) || (content.is_null() && content_precision != 0) {
        if capacity != 0 {
            // SAFETY: non-null when `capacity != 0` (checked above), writable.
            unsafe { output.write(0) };
        }
        return 0;
    }
    let bytes = if capacity == 0 {
        &mut []
    } else {
        // SAFETY: non-null, `capacity <= i32::MAX` (checked above), writable and disjoint.
        unsafe { std::slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), capacity) }
    };
    let mut sink = LogSink { bytes, required: 0 };
    sink.append(b"[");
    // SAFETY: `date` is readable through its NUL or precision (null only at precision 0).
    sink.append(unsafe { log_bytes(date, date_precision) });
    sink.append(&[b' ', symbol]);
    #[expect(
        clippy::let_underscore_must_use,
        reason = "integer Display has no fallible callbacks and this sink never returns Err"
    )]
    let _ = write!(sink, "{pid}.{worker}]\t");
    sink.append(log_label(level).to_bytes());
    sink.append(b"\t");
    // SAFETY: as for `date`.
    sink.append(unsafe { log_bytes(content, content_precision) });
    sink.append(b"\n");
    sink.finish()
}

/// Format the logger's native `<.microseconds>` suffix without allocation.
/// # Safety
/// Output is writable for capacity bytes, null only for zero capacity. Capacity
/// must fit i32; zero queries the full length, otherwise the count is clamped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_micros(
    output: *mut u8,
    capacity: usize,
    microseconds: i64,
) -> usize {
    use std::fmt::Write;
    if capacity > i32::MAX as usize || (capacity != 0 && output.is_null()) {
        return 0;
    }
    let bytes = if capacity == 0 {
        &mut []
    } else {
        // SAFETY: non-null, `capacity <= i32::MAX` (checked above), writable and disjoint.
        unsafe { std::slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), capacity) }
    };
    let mut sink = LogSink { bytes, required: 0 };
    #[expect(
        clippy::let_underscore_must_use,
        reason = "integer Display has no fallible callbacks and this sink never returns Err"
    )]
    let _ = write!(sink, "<.{microseconds}>");
    sink.finish()
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
    #[test]
    fn spinlock_serializes_contenders() {
        let lock = AtomicU32::new(0);
        let value = AtomicU32::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..5000 {
                        unsafe {
                            swr_v1_utils_spinlock(lock.as_ptr(), 4, 1024);
                        }
                        value.store(value.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
                        lock.store(0, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(value.load(Ordering::Relaxed), 20000);
    }
}
