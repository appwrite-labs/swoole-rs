// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/misc.cc (Swoole contributors).
//! misc.cc's spinlock, futex wait/wake and hashes (and crc32.cc's), defined under their upstream
//! (Itanium) symbols with `upstream-symbols`; the corebase patch deletes the C++.
use crate::constants::SW_SPINLOCK_LOOP_N;
use crate::lent::{Count, In};
use crate::swr_v1_core_cpu_num;
use crate::utilities::{swoole_hash, swr_v1_utils_spinlock};
use std::ffi::{c_char, c_int};
use std::sync::atomic::AtomicU32;

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
    word(atomic).map_or_else(|e| e, |word| if futex_wait(word, timeout) { 0 } else { -1 })
}

/// `int sw_atomic_futex_wakeup(sw_atomic_t *atomic, int n)`: the waiters woken.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22sw_atomic_futex_wakeupPVji")
)]
pub extern "C" fn sw_atomic_futex_wakeup(atomic: Option<&AtomicU32>, n: c_int) -> c_int {
    word(atomic).map_or_else(
        |e| e,
        |word| {
            futex_wakeup(word, n).map_or(-1, |woken| c_int::try_from(woken).unwrap_or(c_int::MAX))
        },
    )
}

/// The word a wait or wake takes: -1 for none, which records `EINVAL` as the last error
/// where the registry stands in for the futex.
fn word(atomic: Option<&AtomicU32>) -> Result<&AtomicU32, c_int> {
    #[cfg(target_os = "linux")]
    {
        atomic.ok_or(-1)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::utilities::wait_registry::atomic(atomic)
    }
}

/// [`sw_atomic_futex_wait`] on a word: whether it took the word's token.
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

/// [`sw_atomic_futex_wakeup`] on a word: the waiters woken, or none on failure.
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
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
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
