// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/misc.cc (Swoole contributors).
//! Atomic wait/wakeup and the native hash entry points.
use crate::lent::{Count, In};
use crate::utilities::swoole_hash;
use std::ffi::{c_char, c_int};
use std::sync::atomic::AtomicU32;

/// Whether the wait took the word's token.
#[must_use]
pub fn futex_wait(word: &AtomicU32, timeout: f64) -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::locks::wait(word, timeout)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::utilities::wait_registry::registry_wait(word, timeout)
    }
}

/// The number of waiters woken, or none on failure.
#[must_use]
pub fn futex_wakeup(word: &AtomicU32, n: c_int) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        crate::locks::wakeup(word, n)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::utilities::wait_registry::registry_wakeup(word, n)
    }
}

/// `char` is signed on x86-64 and unsigned on arm64 Linux, and the hashes promote it.
const SIGNED_CHAR: bool = c_char::MIN != 0;

macro_rules! hashes {
    ($($symbol:literal => $name:ident -> $ret:ty = |$key:ident| $hash:expr;)*) => {$(
        #[doc = concat!("`", stringify!($name), "(const char *key, size_t len)`: the `len`")]
        /// bytes C lends at `key` (none for null).
        #[must_use]
        #[unsafe(export_name = $symbol)]
        pub extern "C" fn $name<'id>(key: In<'id, u8>, len: Count<'id>) -> $ret {
            let $key = key.drain(len).bytes();
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
        let hashes: [(_, for<'id> extern "C" fn(In<'id, u8>, Count<'id>) -> u64); 3] = [
            (0, swoole_hash_php),
            (1, swoole_hash_jenkins),
            (2, swoole_hash_austin),
        ];
        for (algorithm, hash) in hashes {
            assert_eq!(
                crate::lent::lend_in(key, |at, n: Count<'_>| hash(at, n)),
                swoole_hash(key, algorithm, SIGNED_CHAR)
            );
        }
    }

    #[test]
    fn the_spinlock_takes_a_free_lock() {
        let lock = AtomicU32::new(0);
        crate::utilities::swr_v1_utils_spinlock(
            Some(&lock),
            u32::from(crate::swr_v1_core_cpu_num()),
            crate::constants::SW_SPINLOCK_LOOP_N,
        );
        assert_eq!(lock.into_inner(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_wakeup_publishes_the_token_a_wait_takes() {
        let word = AtomicU32::new(0);
        assert!(!futex_wait(&word, 0.01));
        assert_eq!(futex_wakeup(&word, 1), Some(0));
        assert!(futex_wait(&word, -1.0));
    }
}
