// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/misc.cc (Swoole contributors).
//! Atomic wait/wakeup.
use crate::timer::Deadline;
use std::ffi::c_int;
use std::sync::atomic::AtomicU32;

/// Waits for the word's token until `deadline`: whether it took it.
#[must_use]
pub fn futex_wait(word: &AtomicU32, deadline: Deadline) -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::locks::wait(word, deadline)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::utilities::wait_registry::registry_wait(word, deadline)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!futex_wait(&word, Deadline::from_futex_timeout(0.01)));
        assert_eq!(futex_wakeup(&word, 1), Some(0));
        assert!(futex_wait(&word, Deadline::never()));
    }
}
