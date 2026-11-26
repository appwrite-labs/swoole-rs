//! The signal layer's entries into the loop: the end callback, `fd_wait`'s EINTR retry
//! and kqueue's signal events call what the process layer registers here, so the loop
//! never names it.

/// The signal layer's entries into the loop, which the process layer registers through
/// [`set_signals`] before it installs its first handler: none are needed before then.
pub struct Signals {
    /// `swoole_signal_dispatch`: runs the signals the deferred handler marked; whether
    /// there were any.
    pub dispatch: fn() -> bool,
    /// `ReactorKqueue::wait`'s entry for a signal event.
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    pub kqueue: fn(std::ffi::c_int),
}

static SIGNALS: std::sync::OnceLock<Signals> = std::sync::OnceLock::new();

/// Registers the signal layer's entries once; later ones are ignored.
pub fn set_signals(signals: Signals) {
    SIGNALS.get_or_init(|| signals);
}

/// The signal layer's entries, once registered.
pub(crate) fn get() -> Option<&'static Signals> {
    SIGNALS.get()
}

/// `swoole_signal_dispatch`, once the signal layer is registered: whether a signal was
/// pending.
pub(crate) fn dispatch() -> bool {
    get().is_some_and(|signals| (signals.dispatch)())
}
