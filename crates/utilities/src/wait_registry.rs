// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/misc.cc (Swoole contributors).
//! The atomic wait/wakeup pair for builds without futex: a registry in shared
//! memory, allocated before any worker forks, keyed by the atomic's address so
//! every descendant process sees the same waiters.
#![cfg_attr(
    all(target_os = "linux", not(test)),
    expect(
        dead_code,
        reason = "Linux waits on futex; swoole_init adopts the registry there unused"
    )
)]

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const ENTRIES: usize = 1024;

#[repr(C)]
struct Entry {
    address: AtomicUsize,
    waiters: AtomicU32,
    generation: AtomicU32,
    notifications: AtomicU32,
}

#[repr(C)]
pub(crate) struct Registry {
    lock: AtomicU32,
    entries: [Entry; ENTRIES],
}

static REGISTRY: AtomicPtr<Registry> = AtomicPtr::new(std::ptr::null_mut());

// Entry fields are only touched under the registry lock, so Relaxed suffices.
const R: Ordering = Ordering::Relaxed;

fn acquire(word: &AtomicU32) -> bool {
    word.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

struct Locked<'a>(&'a Registry);

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        self.0.lock.store(0, Ordering::Release);
    }
}

impl Entry {
    fn reset(&self, address: usize) -> &Self {
        self.address.store(address, R);
        self.waiters.store(0, R);
        self.generation.store(0, R);
        self.notifications.store(0, R);
        self
    }
}

impl Registry {
    fn lock(&self) -> Locked<'_> {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            std::hint::spin_loop();
            std::thread::yield_now();
        }
        Locked(self)
    }

    /// Open addressing from the address's slot; with `create`, claim the first
    /// empty slot, or else the first idle one, as upstream did.
    fn entry(&self, _: &Locked<'_>, address: usize, create: bool) -> Option<&Entry> {
        let start = (address >> 3) % ENTRIES;
        let mut reusable = None;
        for entry in self.entries.iter().cycle().skip(start).take(ENTRIES) {
            let current = entry.address.load(R);
            if current == address {
                return Some(entry);
            }
            if current == 0 {
                return create.then(|| reusable.unwrap_or(entry).reset(address));
            }
            if create
                && reusable.is_none()
                && entry.waiters.load(R) == 0
                && entry.notifications.load(R) == 0
            {
                reusable = Some(entry);
            }
        }
        reusable.map(|entry| entry.reset(address))
    }

    fn wait(&self, word: &AtomicU32, timeout: f64) -> Result<(), c_int> {
        if acquire(word) {
            return Ok(());
        }
        let deadline = (timeout > 0.0)
            .then(|| Duration::try_from_secs_f64(timeout).ok())
            .flatten()
            .and_then(|timeout| Instant::now().checked_add(timeout));
        let address = word.as_ptr() as usize;
        let locked = self.lock();
        // Close the race between the first acquire and registering this waiter.
        if acquire(word) {
            return Ok(());
        }
        let Some(entry) = self.entry(&locked, address, true) else {
            return Err(libc::ENOSPC);
        };
        let mut generation = entry.generation.load(R);
        entry.waiters.fetch_add(1, R);
        drop(locked);
        loop {
            let locked = self.lock();
            let current = entry.generation.load(R);
            if current != generation {
                generation = current;
                if entry.notifications.load(R) > 0 {
                    entry.notifications.fetch_sub(1, R);
                    entry.waiters.fetch_sub(1, R);
                    return if acquire(word) { Ok(()) } else { Err(0) };
                }
            } else if acquire(word) {
                entry.waiters.fetch_sub(1, R);
                return Ok(());
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                entry.waiters.fetch_sub(1, R);
                return Err(libc::ETIMEDOUT);
            }
            drop(locked);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn wakeup(&self, word: &AtomicU32, n: c_int) -> Result<c_int, c_int> {
        let Ok(n) = u32::try_from(n) else {
            return Err(libc::EINVAL);
        };
        if n == 0 {
            return Err(libc::EINVAL);
        }
        let locked = self.lock();
        if word
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(0);
        }
        let Some(entry) = self.entry(&locked, word.as_ptr() as usize, false) else {
            return Ok(0);
        };
        // Notifications go only to waiters already registered, so a later
        // waiter can't consume an earlier wakeup.
        let available = entry
            .waiters
            .load(R)
            .saturating_sub(entry.notifications.load(R));
        let notified = n.min(available);
        if notified > 0 {
            entry.notifications.fetch_add(notified, R);
            entry.generation.fetch_add(1, R);
        }
        Ok(c_int::try_from(notified).unwrap_or(c_int::MAX))
    }
}

/// Bytes `sw_atomic_wait_init` allocates from the shared memory pool.
#[must_use]
pub fn swr_v1_utils_wait_registry_size() -> usize {
    size_of::<Registry>()
}

/// Zero and adopt `memory` as the registry. False for null or misaligned memory.
/// # Safety
/// `memory` is null or writable for [`swr_v1_utils_wait_registry_size`] bytes,
/// shared with every process that waits, and lives as long as they do.
pub unsafe fn swr_v1_utils_wait_registry_init(memory: *mut c_void) -> bool {
    if memory.is_null() || !memory.cast::<Registry>().is_aligned() {
        return false;
    }
    // SAFETY: the caller lends `size_of::<Registry>()` writable bytes; all-zero is a valid Registry.
    unsafe { memory.write_bytes(0, size_of::<Registry>()) };
    REGISTRY.store(memory.cast(), Ordering::SeqCst);
    true
}

fn with<T>(
    word: &AtomicU32,
    run: impl FnOnce(&Registry, &AtomicU32) -> Result<T, c_int>,
) -> Result<T, c_int> {
    let registry = REGISTRY.load(Ordering::SeqCst);
    if registry.is_null() {
        return Err(libc::EINVAL);
    }
    // SAFETY: init adopted a registry that lives as long as its users.
    run(unsafe { &*registry }, word)
}

/// `word` as an atomic, or -1 with `EINVAL` recorded for null or misaligned.
/// # Safety
/// `word` is null, misaligned, or a u32 accessed only atomically while `'a` lasts.
#[cfg(not(target_os = "linux"))]
pub(crate) unsafe fn atomic<'a>(word: *mut u32) -> Result<&'a AtomicU32, c_int> {
    if word.is_null() || !word.is_aligned() {
        return Err(failed(libc::EINVAL));
    }
    // SAFETY: per the caller, checked non-null and aligned.
    Ok(unsafe { AtomicU32::from_ptr(word) })
}

/// -1 with a nonzero error recorded as the last error.
#[cfg(not(target_os = "linux"))]
fn failed(code: c_int) -> c_int {
    if code != 0 {
        swoole_rs_corebase::log::set_last_error(code);
    }
    -1
}

/// `sw_atomic_futex_wait` without futex: take the word's token (1 -> 0),
/// polling every millisecond until a wakeup or `timeout` (seconds, <= 0 for
/// none). Returns 0, or -1 with the error recorded as the last error.
#[cfg(not(target_os = "linux"))]
pub(crate) fn registry_wait(word: &AtomicU32, timeout: f64) -> c_int {
    with(word, |registry, word| registry.wait(word, timeout)).map_or_else(failed, |()| 0)
}

/// `sw_atomic_futex_wakeup` without futex: publish the token (0 -> 1) and
/// notify up to `n` registered waiters. Returns the number notified, or -1
/// with the error recorded as the last error.
#[cfg(not(target_os = "linux"))]
pub(crate) fn registry_wakeup(word: &AtomicU32, n: c_int) -> c_int {
    with(word, |registry, word| registry.wakeup(word, n)).unwrap_or_else(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn registry() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let memory = Box::leak(Box::new(std::mem::MaybeUninit::<Registry>::uninit()));
            assert!(unsafe { swr_v1_utils_wait_registry_init(memory.as_mut_ptr().cast()) });
        });
    }

    fn wait(word: &AtomicU32, timeout: f64) -> (c_int, c_int) {
        match with(word, |registry, word| registry.wait(word, timeout)) {
            Ok(()) => (0, 0),
            Err(code) => (-1, code),
        }
    }

    fn wakeup(word: &AtomicU32, n: c_int) -> (c_int, c_int) {
        match with(word, |registry, word| registry.wakeup(word, n)) {
            Ok(notified) => (notified, 0),
            Err(code) => (-1, code),
        }
    }

    #[test]
    fn a_ready_token_is_taken_without_waiting() {
        registry();
        let word = AtomicU32::new(1);
        assert_eq!(wait(&word, 0.01), (0, 0));
        assert_eq!(word.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_wait_without_wakeup_times_out() {
        registry();
        let word = AtomicU32::new(0);
        let begun = Instant::now();
        assert_eq!(wait(&word, 0.02), (-1, libc::ETIMEDOUT));
        assert!(
            begun.elapsed() >= Duration::from_millis(20),
            "waited the timeout"
        );
    }

    #[test]
    fn wakeup_rejects_a_count_below_one_and_needs_a_free_token() {
        registry();
        let word = AtomicU32::new(0);
        assert_eq!(wakeup(&word, 0), (-1, libc::EINVAL));
        assert_eq!(wakeup(&word, 1), (0, 0), "token set, nobody registered");
        assert_eq!(word.load(Ordering::SeqCst), 1);
        assert_eq!(wakeup(&word, 1), (0, 0), "token already set");
    }

    #[test]
    fn wakeup_notifies_a_registered_waiter() {
        registry();
        let word = Arc::new(AtomicU32::new(0));
        let waiter = {
            let word = Arc::clone(&word);
            std::thread::spawn(move || wait(&word, 5.0))
        };
        // Wake only once the waiter has registered: a token published before
        // it starts would be taken by its fast path, leaving nobody to notify.
        let registry = unsafe { &*REGISTRY.load(Ordering::SeqCst) };
        let registered = || {
            let locked = registry.lock();
            registry
                .entry(&locked, word.as_ptr() as usize, false)
                .is_some_and(|entry| entry.waiters.load(R) > 0)
        };
        while !registered() {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(wakeup(&word, 3), (1, 0), "only the one registered waiter");
        assert_eq!(waiter.join().unwrap(), (0, 0));
        assert_eq!(word.load(Ordering::SeqCst), 0, "the waiter took the token");
    }

    #[test]
    fn entries_reuse_idle_slots_once_full() {
        let memory = Box::new(std::mem::MaybeUninit::<Registry>::zeroed());
        let registry = unsafe { memory.assume_init_ref() };
        let locked = registry.lock();
        for address in 1..=ENTRIES {
            assert!(
                registry.entry(&locked, address << 3, true).is_some(),
                "slot free"
            );
        }
        let entry = registry.entry(&locked, (ENTRIES + 1) << 3, true).unwrap();
        assert_eq!(
            entry.address.load(R),
            (ENTRIES + 1) << 3,
            "an idle slot reused"
        );
        for e in &registry.entries {
            e.waiters.store(1, R);
        }
        assert!(
            registry.entry(&locked, (ENTRIES + 2) << 3, true).is_none(),
            "all busy"
        );
        assert!(
            registry.entry(&locked, 7 << 3, false).is_some(),
            "lookup only"
        );
    }
}
