// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.5 src/os/signal.cc (Swoole contributors).
//! `swoole_signal_block_all` and `swoole_signal_unblock_all`: a thread blocks every
//! signal and later puts back the mask it had, and a thread that starts another
//! blocks everything around the start, so the new thread never takes a signal
//! before it can block them itself.
use crate::log;
use std::cell::Cell;

thread_local! {
    /// `SwooleTG.signal_blocking_all`: this thread has blocked every signal.
    static BLOCKING_ALL: Cell<bool> = const { Cell::new(false) };
    /// `SwooleTG.signal_previous_mask`: the mask to put back.
    static PREVIOUS: Cell<Option<libc::sigset_t>> = const { Cell::new(None) };
}

/// `NSIG`.
const NSIG: libc::c_int = if cfg!(target_os = "linux") { 65 } else { 32 };

fn empty() -> libc::sigset_t {
    // SAFETY: plain data; all zeroes is the empty set.
    unsafe { std::mem::zeroed() }
}

fn full() -> libc::sigset_t {
    let mut all = empty();
    // SAFETY: `all` is a valid set.
    unsafe { libc::sigfillset(&raw mut all) };
    all
}

/// `pthread_sigmask(how, set, old)`, warning as `origin` when it fails.
fn set_mask(how: libc::c_int, set: &libc::sigset_t, old: Option<&mut libc::sigset_t>) -> bool {
    let old = old.map_or(std::ptr::null_mut(), std::ptr::from_mut);
    // SAFETY: a valid set, and a valid or null out-set.
    let error = unsafe { libc::pthread_sigmask(how, set, old) };
    if error != 0 {
        let call = if how == libc::SIG_BLOCK {
            "SIG_BLOCK"
        } else {
            "SIG_SETMASK"
        };
        log::sys_warning_errno(
            error,
            c"pthread_sigmask",
            format_args!("pthread_sigmask({call}) failed"),
        );
    }
    error == 0
}

/// `bool swoole_signal_block_all()`: true only when this call saved and changed the
/// mask; that caller restores it with [`signal_unblock_all`].
#[expect(
    clippy::must_use_candidate,
    reason = "a thread that blocks for the rest of its life has no mask to put back"
)]
pub fn signal_block_all() -> bool {
    if BLOCKING_ALL.get() {
        return false;
    }
    let mut previous = empty();
    if !set_mask(libc::SIG_BLOCK, &full(), Some(&mut previous)) {
        return false;
    }
    PREVIOUS.set(Some(previous));
    BLOCKING_ALL.set(true);
    true
}

/// `void swoole_signal_unblock_all()`: puts back the mask [`signal_block_all`] saved.
pub fn signal_unblock_all() {
    if !BLOCKING_ALL.get() {
        return;
    }
    let previous = PREVIOUS.get().unwrap_or_else(empty);
    if set_mask(libc::SIG_SETMASK, &previous, None) {
        BLOCKING_ALL.set(false);
    }
}

/// Whether this thread has blocked every signal (`SwooleTG.signal_blocking_all`).
#[must_use]
pub fn signal_blocking_all() -> bool {
    BLOCKING_ALL.get()
}

/// Blocks every signal for a fork, and returns the mask before for
/// [`restore_after_fork`].
pub(crate) fn block_for_fork() -> libc::sigset_t {
    let mut previous = empty();
    set_mask(libc::SIG_BLOCK, &full(), Some(&mut previous));
    previous
}

/// After [`block_for_fork`]: the parent puts `previous` back. The child, once it has
/// dropped the parent's handlers, keeps blocked what both `previous` and that reset
/// left blocked (it unblocks signalfd's signals), and a signal sent meanwhile
/// arrives now.
pub(crate) fn restore_after_fork(previous: &libc::sigset_t, child: bool) {
    let mut keep = *previous;
    if child {
        let mut now = empty();
        set_mask(libc::SIG_BLOCK, &empty(), Some(&mut now));
        for signo in 1..NSIG {
            // SAFETY: a valid set.
            if unsafe { libc::sigismember(&raw const now, signo) } != 1 {
                // SAFETY: a valid set.
                unsafe { libc::sigdelset(&raw mut keep, signo) };
            }
        }
    }
    set_mask(libc::SIG_SETMASK, &keep, None);
}

/// Runs `start` with every signal blocked, then puts back this thread's mask, so
/// the thread `start` creates inherits a fully blocked mask.
pub fn with_signals_blocked<T>(start: impl FnOnce() -> T) -> T {
    let restore = signal_block_all();
    let started = start();
    if restore {
        signal_unblock_all();
    }
    started
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// This thread's mask.
    pub(crate) fn current() -> libc::sigset_t {
        let mut mask = empty();
        assert_eq!(
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &raw mut mask) },
            0
        );
        mask
    }

    pub(crate) fn only(signo: libc::c_int) -> libc::sigset_t {
        let mut mask = empty();
        unsafe { libc::sigaddset(&raw mut mask, signo) };
        assert_eq!(
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &raw const mask, std::ptr::null_mut())
            },
            0
        );
        mask
    }

    pub(crate) fn same(left: libc::sigset_t, right: libc::sigset_t) -> bool {
        (1..NSIG).all(|signo| unsafe {
            libc::sigismember(&raw const left, signo) == libc::sigismember(&raw const right, signo)
        })
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn masks_all_once() {
        std::thread::spawn(|| {
            assert!(signal_block_all());
            assert!(BLOCKING_ALL.get());
            assert!(!signal_block_all());
            assert!(BLOCKING_ALL.get());
            signal_unblock_all();
            assert!(!BLOCKING_ALL.get());
        })
        .join()
        .unwrap();
    }

    /// core-tests/src/os/signal.cpp `TEST(os_signal, restore_previous_mask)`, in a new
    /// thread rather than a forked child: the mask and the flag are per thread.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigaddset")]
    fn restore_previous_mask() {
        std::thread::spawn(|| {
            for signo in [libc::SIGUSR1, libc::SIGUSR2] {
                let expected = only(signo);
                signal_block_all();
                signal_block_all(); // Must not overwrite the saved mask.
                signal_unblock_all();
                signal_unblock_all(); // Already restored.
                assert!(!BLOCKING_ALL.get());
                assert!(same(expected, current()));
            }
        })
        .join()
        .unwrap();
    }
}
