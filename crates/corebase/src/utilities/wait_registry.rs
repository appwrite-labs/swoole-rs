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

use crate::timer::Deadline;
use std::ffi::c_int;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

const ENTRIES: usize = 1024;

/// One address's words: the address, its waiters, its generation and the
/// notifications not yet taken.
type Entry = [AtomicU64; 4];
const ADDRESS: usize = 0;
const WAITERS: usize = 1;
const GENERATION: usize = 2;
const NOTIFICATIONS: usize = 3;

/// The registry's words, shared with every process that waits: the lock word, then
/// the entries.
#[derive(Clone, Copy)]
pub(crate) struct Registry {
    lock: &'static AtomicU64,
    entries: &'static [Entry; ENTRIES],
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

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

fn reset(entry: &Entry, address: u64) -> &Entry {
    entry[ADDRESS].store(address, R);
    for word in &entry[WAITERS..] {
        word.store(0, R);
    }
    entry
}

impl Registry {
    /// The registry over `memory`'s leading words; none when they are too few.
    fn of(memory: &'static [AtomicU64]) -> Option<Self> {
        let (lock, rest) = memory.split_first()?;
        let (entries, _) = rest.as_chunks::<4>();
        Some(Self {
            lock,
            entries: entries.first_chunk()?,
        })
    }

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
        let address = address as u64;
        let mut reusable = None;
        for entry in self.entries.iter().cycle().skip(start).take(ENTRIES) {
            let current = entry[ADDRESS].load(R);
            if current == address {
                return Some(entry);
            }
            if current == 0 {
                return create.then(|| reset(reusable.unwrap_or(entry), address));
            }
            if create
                && reusable.is_none()
                && entry[WAITERS].load(R) == 0
                && entry[NOTIFICATIONS].load(R) == 0
            {
                reusable = Some(entry);
            }
        }
        reusable.map(|entry| reset(entry, address))
    }

    fn wait(&self, word: &AtomicU32, deadline: Deadline) -> Result<(), c_int> {
        if acquire(word) {
            return Ok(());
        }
        let address = word.as_ptr() as usize;
        let locked = self.lock();
        // Close the race between the first acquire and registering this waiter.
        if acquire(word) {
            return Ok(());
        }
        let Some(entry) = self.entry(&locked, address, true) else {
            return Err(libc::ENOSPC);
        };
        let mut generation = entry[GENERATION].load(R);
        entry[WAITERS].fetch_add(1, R);
        drop(locked);
        loop {
            let locked = self.lock();
            let current = entry[GENERATION].load(R);
            if current != generation {
                generation = current;
                if entry[NOTIFICATIONS].load(R) > 0 {
                    entry[NOTIFICATIONS].fetch_sub(1, R);
                    entry[WAITERS].fetch_sub(1, R);
                    return if acquire(word) { Ok(()) } else { Err(0) };
                }
            } else if acquire(word) {
                entry[WAITERS].fetch_sub(1, R);
                return Ok(());
            }
            if deadline.remaining().is_some_and(|left| left.is_zero()) {
                entry[WAITERS].fetch_sub(1, R);
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
        let available = entry[WAITERS]
            .load(R)
            .saturating_sub(entry[NOTIFICATIONS].load(R));
        let notified = u64::from(n).min(available);
        if notified > 0 {
            entry[NOTIFICATIONS].fetch_add(notified, R);
            entry[GENERATION].fetch_add(1, R);
        }
        Ok(c_int::try_from(notified).unwrap_or(c_int::MAX))
    }
}

/// Install one anonymous shared mapping before workers fork. Its words stay with the
/// process-wide registry across pool teardown and reinitialization.
pub fn wait_registry_init() {
    REGISTRY.get_or_init(|| {
        let words = swr_v1_utils_wait_registry_size() / size_of::<AtomicU64>();
        crate::memory::members::pool::wait_registry_words(words)
            .and_then(Registry::of)
            .unwrap_or_else(|| crate::fatal("no shared memory for the atomic wait registry"))
    });
}

/// Bytes `sw_atomic_wait_init` needs in shared memory: the lock word
/// and the entries.
#[must_use]
pub fn swr_v1_utils_wait_registry_size() -> usize {
    size_of::<AtomicU64>() + size_of::<[Entry; ENTRIES]>()
}

/// Zero and adopt `memory` as the registry: words shared with every process that
/// waits, lent for the program's life. False when they are too few; the first
/// registry adopted stays.
pub fn swr_v1_utils_wait_registry_init(memory: &'static mut [AtomicU64]) -> bool {
    for word in memory.iter_mut() {
        *word.get_mut() = 0;
    }
    let Some(registry) = Registry::of(memory) else {
        return false;
    };
    _ = REGISTRY.set(registry);
    true
}

fn with<T>(
    word: &AtomicU32,
    run: impl FnOnce(&Registry, &AtomicU32) -> Result<T, c_int>,
) -> Result<T, c_int> {
    let registry = REGISTRY.get().ok_or(libc::EINVAL)?;
    run(registry, word)
}

/// Record a nonzero registry error before returning to the caller.
#[cfg(not(target_os = "linux"))]
fn record_error(code: c_int) {
    if code != 0 {
        crate::log::set_last_error(code);
    }
}

/// `sw_atomic_futex_wait` without futex: take the word's token (1 -> 0),
/// polling every millisecond until a wakeup or `deadline` passes. Whether it took
/// the token; a failure's error is recorded as the last error.
#[cfg(not(target_os = "linux"))]
pub(crate) fn registry_wait(word: &AtomicU32, deadline: Deadline) -> bool {
    with(word, |registry, word| registry.wait(word, deadline))
        .map_err(record_error)
        .is_ok()
}

/// `sw_atomic_futex_wakeup` without futex: publish the token (0 -> 1) and
/// notify up to `n` registered waiters. Returns the number notified, or none with
/// the error recorded as the last error.
#[cfg(not(target_os = "linux"))]
pub(crate) fn registry_wakeup(word: &AtomicU32, n: c_int) -> Option<u32> {
    with(word, |registry, word| registry.wakeup(word, n))
        .map_err(record_error)
        .ok()
        .map(|notified| u32::try_from(notified).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn wait(word: &AtomicU32, timeout: f64) -> (c_int, c_int) {
        let deadline = Deadline::from_futex_timeout(timeout);
        match with(word, |registry, word| registry.wait(word, deadline)) {
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
    #[cfg(not(miri))] // Miri cannot fork.
    fn registry_is_shared_after_pool_reinitialization() {
        use crate::memory::members::pool;

        if crate::utilities::isolated_process::run_in_child(
            "utilities::wait_registry::tests::registry_is_shared_after_pool_reinitialization",
        ) {
            return;
        }
        assert!(crate::swr_v1_core_init());
        pool::init();
        wait_registry_init();
        let address = &REGISTRY.get().unwrap().entries[0][ADDRESS];
        pool::delete();
        pool::init();
        wait_registry_init();
        pool::delete();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            address.store(42, R);
            unsafe { libc::_exit(0) };
        }
        let mut status = 0;
        assert_eq!(
            crate::sys::waitpid(child, Some(&mut status), 0).unwrap(),
            Some(child)
        );
        assert_eq!(status, 0);
        assert_eq!(REGISTRY.get().unwrap().entries[0][ADDRESS].load(R), 42);
    }

    #[test]
    fn a_ready_token_is_taken_without_waiting() {
        wait_registry_init();
        let word = AtomicU32::new(1);
        assert_eq!(wait(&word, 0.01), (0, 0));
        assert_eq!(word.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_wait_without_wakeup_times_out() {
        wait_registry_init();
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
        wait_registry_init();
        let word = AtomicU32::new(0);
        assert_eq!(wakeup(&word, 0), (-1, libc::EINVAL));
        assert_eq!(wakeup(&word, 1), (0, 0), "token set, nobody registered");
        assert_eq!(word.load(Ordering::SeqCst), 1);
        assert_eq!(wakeup(&word, 1), (0, 0), "token already set");
    }

    #[test]
    fn wakeup_notifies_a_registered_waiter() {
        wait_registry_init();
        let word = Arc::new(AtomicU32::new(0));
        let waiter = {
            let word = Arc::clone(&word);
            std::thread::spawn(move || wait(&word, 5.0))
        };
        // Wake only once the waiter has registered: a token published before
        // it starts would be taken by its fast path, leaving nobody to notify.
        let registry = REGISTRY.get().unwrap();
        let registered = || {
            let locked = registry.lock();
            registry
                .entry(&locked, word.as_ptr() as usize, false)
                .is_some_and(|entry| entry[WAITERS].load(R) > 0)
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
        let words = swr_v1_utils_wait_registry_size().div_ceil(8);
        let memory: &'static [AtomicU64] =
            Vec::from_iter((0..words).map(|_| AtomicU64::new(0))).leak();
        let owned = std::ptr::from_ref(memory).cast_mut();
        let registry = &Registry::of(memory).unwrap();
        let locked = registry.lock();
        for address in 1..=ENTRIES {
            assert!(
                registry.entry(&locked, address << 3, true).is_some(),
                "slot free"
            );
        }
        let entry = registry.entry(&locked, (ENTRIES + 1) << 3, true).unwrap();
        assert_eq!(
            entry[ADDRESS].load(R),
            ((ENTRIES + 1) << 3) as u64,
            "an idle slot reused"
        );
        for e in registry.entries {
            e[WAITERS].store(1, R);
        }
        assert!(
            registry.entry(&locked, (ENTRIES + 2) << 3, true).is_none(),
            "all busy"
        );
        assert!(
            registry.entry(&locked, 7 << 3, false).is_some(),
            "lookup only"
        );
        drop(locked);
        // SAFETY: the words leaked above, which nothing borrows once the lock is
        // released and this registry view is no longer used.
        drop(unsafe { Box::from_raw(owned) });
    }
}
