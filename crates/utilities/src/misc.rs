// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/misc.cc (Swoole contributors).
//! misc.cc's spinlock, futex wait/wake and hashes (and crc32.cc's), defined under their upstream
//! (Itanium) symbols with `upstream-symbols`; the corebase patch deletes the C++.
use crate::{swoole_hash, swr_v1_utils_spinlock};
use std::ffi::{c_char, c_int};
use std::sync::atomic::AtomicU32;
use swoole_rs_corebase::constants::SW_SPINLOCK_LOOP_N;
use swoole_rs_corebase::swr_v1_core_cpu_num;

/// `void sw_spinlock(sw_atomic_t *lock)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z11sw_spinlockPVj")
)]
pub extern "C" fn sw_spinlock(lock: Option<&AtomicU32>) {
    swr_v1_utils_spinlock(lock, u32::from(swr_v1_core_cpu_num()), SW_SPINLOCK_LOOP_N);
}

/// `int sw_atomic_futex_wait(sw_atomic_t *atomic, double timeout)`: 0 once the word's
/// token is taken, -1 on timeout or error. Without futex, the shared-memory registry
/// `sw_atomic_wait_init` set up, recording its error as the last error.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20sw_atomic_futex_waitPVjd")
)]
pub extern "C" fn sw_atomic_futex_wait(atomic: Option<&AtomicU32>, timeout: f64) -> c_int {
    #[cfg(target_os = "linux")]
    {
        crate::swr_v1_utils_wait(atomic, timeout)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::wait_registry::atomic(atomic).map_or_else(|e| e, |word| futex_wait(word, timeout))
    }
}

/// `int sw_atomic_futex_wakeup(sw_atomic_t *atomic, int n)`: the waiters woken.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22sw_atomic_futex_wakeupPVji")
)]
pub extern "C" fn sw_atomic_futex_wakeup(atomic: Option<&AtomicU32>, n: c_int) -> c_int {
    #[cfg(target_os = "linux")]
    {
        crate::swr_v1_utils_wakeup(atomic, n)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::wait_registry::atomic(atomic).map_or_else(|e| e, |word| futex_wakeup(word, n))
    }
}

/// [`sw_atomic_futex_wait`] on a word.
pub fn futex_wait(word: &AtomicU32, timeout: f64) -> c_int {
    #[cfg(target_os = "linux")]
    {
        swoole_rs_locks::wait(word, timeout)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::wait_registry::registry_wait(word, timeout)
    }
}

/// [`sw_atomic_futex_wakeup`] on a word.
pub fn futex_wakeup(word: &AtomicU32, n: c_int) -> c_int {
    #[cfg(target_os = "linux")]
    {
        swoole_rs_locks::wakeup(word, n)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::wait_registry::registry_wakeup(word, n)
    }
}

/// `char` is signed on x86-64 and unsigned on arm64 Linux, and the hashes promote it.
const SIGNED_CHAR: bool = c_char::MIN != 0;

macro_rules! hashes {
    ($($symbol:literal => $name:ident -> $ret:ty = |$key:ident| $hash:expr;)*) => {$(
        #[doc = concat!("`", stringify!($name), "(const char *key, size_t len)`.")]
        ///
        /// # Safety
        /// `key` is readable for `len` bytes.
        #[must_use]
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C" fn $name(key: *const c_char, len: usize) -> $ret {
            let $key: &[u8] = if key.is_null() || len == 0 {
                &[]
            } else {
                // SAFETY: `key` is readable for `len` bytes, as the caller promises.
                unsafe { std::slice::from_raw_parts(key.cast(), len) }
            };
            $hash
        }
    )*};
}

hashes! {
    "_Z15swoole_hash_phpPKcm" => swoole_hash_php -> u64 = |key| swoole_hash(key, 0, SIGNED_CHAR);
    "_Z19swoole_hash_jenkinsPKcm" => swoole_hash_jenkins -> u64 =
        |key| swoole_hash(key, 1, SIGNED_CHAR);
    "_Z18swoole_hash_austinPKcm" => swoole_hash_austin -> u64 =
        |key| swoole_hash(key, 2, SIGNED_CHAR);
    "_Z12swoole_crc32PKcm" => swoole_crc32 -> u32 = |key| swoole_rs_crc32::crc32(key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hashes_are_the_utility_hashes() {
        let key = b"swoole";
        let hashes: [(_, unsafe extern "C" fn(*const c_char, usize) -> u64); 3] = [
            (0, swoole_hash_php),
            (1, swoole_hash_jenkins),
            (2, swoole_hash_austin),
        ];
        for (algorithm, hash) in hashes {
            assert_eq!(
                unsafe { hash(key.as_ptr().cast(), key.len()) },
                swoole_hash(key, algorithm, SIGNED_CHAR)
            );
        }
    }

    #[test]
    fn the_spinlock_takes_a_free_lock() {
        let lock = AtomicU32::new(0);
        sw_spinlock(Some(&lock));
        assert_eq!(lock.into_inner(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_wakeup_publishes_the_token_a_wait_takes() {
        let word = AtomicU32::new(0);
        assert_eq!(sw_atomic_futex_wait(Some(&word), 0.01), -1);
        assert_eq!(sw_atomic_futex_wakeup(Some(&word), 1), 0);
        assert_eq!(sw_atomic_futex_wait(Some(&word), -1.0), 0);
    }
}
