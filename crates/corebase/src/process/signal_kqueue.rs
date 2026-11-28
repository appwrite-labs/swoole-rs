//! `src/os/signal.cc`'s kqueue path (BSD and macOS): registering a signal as an
//! `EVFILT_SIGNAL` event on the reactor's kqueue, with the table slot as its
//! `udata` for the reactor to dispatch, dispatching one, and clearing every registered one.
use std::ffi::c_int;
use std::sync::atomic::Ordering::Relaxed;

use crate::process::signal::{Handler, SIGNALS};

/// `swoole_signal_kqueue_set`: `handler` (0 to clear) for `signo` on the kqueue
/// `kq`; returns the previous handler. `*failed` is set when adding the event
/// failed, for native to warn with `errno`. `kq` is the reactor's kqueue; any
/// other descriptor fails the change or registers the signal where nothing reads it.
pub fn signal_kqueue_set(kq: c_int, signo: c_int, handler: Handler, failed: &mut bool) -> Handler {
    let handler = handler.0;
    let Some(slot) = usize::try_from(signo)
        .ok()
        .and_then(|index| SIGNALS.get(index))
    else {
        return Handler::NONE;
    };
    let mut origin = 0;
    let mut event: libc::kevent = crate::sys::zeroed();
    event.ident = signo.cast_unsigned() as usize;
    event.filter = libc::EVFILT_SIGNAL;
    if handler == 0 {
        if slot.activated.load(Relaxed) {
            crate::process::signal::restore_origin(signo);
        } else {
            // Restores the default disposition.
            crate::process::signal::install(signo, libc::SIG_DFL, false, false);
        }
        slot.reset();
        event.flags = libc::EV_DELETE;
    } else {
        crate::process::signal::register_with_reactor();
        // kqueue delivers the signal instead.
        let previous = crate::process::signal::install(signo, libc::SIG_IGN, false, false);
        if !slot.activated.load(Relaxed) {
            crate::process::signal::save_origin(signo, previous);
        }
        origin = slot.handler.swap(handler, Relaxed);
        // A table index: below SW_SIGNO_MAX.
        slot.signo
            .store(u16::try_from(signo).unwrap_or_default(), Relaxed);
        slot.activated.store(true, Relaxed);
        event.flags = libc::EV_ADD;
        // The table slot (`swoole::Signal *`), which the reactor dispatches.
        event.udata = std::ptr::from_ref(slot).cast_mut().cast();
    }
    let changed = crate::sys::kevent_change(kq, &event);
    *failed = changed.is_err() && handler != 0;
    Handler(origin)
}

/// `swoole_signal_kqueue_clear`: removes every
/// registered signal from `kq` (as [`signal_kqueue_set`]'s); then empties the table.
pub fn signal_kqueue_clear(kq: c_int) {
    for slot in &SIGNALS {
        if slot.activated.load(Relaxed) {
            slot.activated.store(false, Relaxed);
            slot.handler.store(0, Relaxed);
            let mut failed = false;
            let signo = c_int::from(slot.signo.load(Relaxed));
            signal_kqueue_set(kq, signo, Handler::NONE, &mut failed);
        }
    }
    crate::process::signal::signal_reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// kqueue's signal entry: inactive does nothing, no handler warns, `SIG_IGN` is skipped,
    /// any other handler runs with the signal number.
    #[test]
    fn signal_entries_run_their_handler_as_upstreams_kqueue_wait() {
        static SEEN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
        extern "C" fn handler(signo: c_int) {
            SEEN.store(signo, std::sync::atomic::Ordering::SeqCst);
        }
        let handler = Handler::new(handler);
        for (activated, seen) in [
            (None, 0),
            (Some(Handler::NONE), 0),
            (Some(Handler::IGNORE), 0),
            (Some(handler), libc::SIGUSR1),
        ] {
            SEEN.store(0, std::sync::atomic::Ordering::SeqCst);
            crate::process::signal::dispatch_kqueue(libc::SIGUSR1, activated);
            assert_eq!(SEEN.load(std::sync::atomic::Ordering::SeqCst), seen);
        }
    }

    #[test]
    fn a_signal_is_registered_on_the_kqueue_and_cleared() {
        let _state = crate::process::signal_state_lock();
        // SAFETY: a new kqueue.
        let kq = unsafe { libc::kqueue() };
        assert!(kq >= 0);
        let mut failed = true;
        let origin = signal_kqueue_set(kq, libc::SIGUSR2, Handler(0x1234), &mut failed);
        assert_eq!((origin, failed), (Handler::NONE, false));
        let slot = &SIGNALS[libc::SIGUSR2 as usize];
        assert_eq!(slot.handler.load(Relaxed), 0x1234);
        assert!(slot.activated.load(Relaxed));
        signal_kqueue_clear(kq);
        assert!(!slot.activated.load(Relaxed));
        assert_eq!(slot.handler.load(Relaxed), 0);
        // SAFETY: the test's kqueue.
        unsafe { libc::close(kq) };
    }
}
