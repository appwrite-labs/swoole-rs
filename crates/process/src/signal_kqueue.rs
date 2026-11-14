//! `src/os/signal.cc`'s kqueue path (BSD and macOS): registering a signal as an
//! `EVFILT_SIGNAL` event on the reactor's kqueue, with the table slot as its
//! `udata` for the reactor to dispatch, and clearing every registered one.
use std::ffi::c_int;
use std::sync::atomic::Ordering::Relaxed;

use crate::signal::{Handler, SIGNALS};

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
    let mut event: libc::kevent = swoole_rs_corebase::sys::zeroed();
    event.ident = signo.cast_unsigned() as usize;
    event.filter = libc::EVFILT_SIGNAL;
    if handler == 0 {
        if slot.activated.load(Relaxed) {
            crate::signal::restore_origin(signo);
        } else {
            // Restores the default disposition.
            crate::signal::install(signo, libc::SIG_DFL, false, false);
        }
        slot.reset();
        event.flags = libc::EV_DELETE;
    } else {
        // kqueue delivers the signal instead.
        let previous = crate::signal::install(signo, libc::SIG_IGN, false, false);
        if !slot.activated.load(Relaxed) {
            crate::signal::save_origin(signo, previous);
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
    let n = swoole_rs_corebase::sys::kevent_change(kq, &event);
    *failed = n < 0 && handler != 0;
    Handler(origin)
}

/// `swoole_signal_kqueue_clear`: when the event loop is `available`, removes every
/// registered signal from `kq` (as [`signal_kqueue_set`]'s); then empties the table.
pub fn signal_kqueue_clear(kq: c_int, available: bool) {
    for slot in &SIGNALS {
        if slot.activated.load(Relaxed) && available {
            slot.activated.store(false, Relaxed);
            slot.handler.store(0, Relaxed);
            let mut failed = false;
            let signo = c_int::from(slot.signo.load(Relaxed));
            signal_kqueue_set(kq, signo, Handler::NONE, &mut failed);
        }
    }
    crate::signal::signal_reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_is_registered_on_the_kqueue_and_cleared() {
        let _state = crate::signal_state_lock();
        // SAFETY: a new kqueue.
        let kq = unsafe { libc::kqueue() };
        assert!(kq >= 0);
        let mut failed = true;
        let origin = signal_kqueue_set(kq, libc::SIGUSR2, Handler(0x1234), &mut failed);
        assert_eq!((origin, failed), (Handler::NONE, false));
        let slot = &SIGNALS[libc::SIGUSR2 as usize];
        assert_eq!(slot.handler.load(Relaxed), 0x1234);
        assert!(slot.activated.load(Relaxed));
        signal_kqueue_clear(kq, true);
        assert!(!slot.activated.load(Relaxed));
        assert_eq!(slot.handler.load(Relaxed), 0);
        // SAFETY: the test's kqueue.
        unsafe { libc::close(kq) };
    }
}
