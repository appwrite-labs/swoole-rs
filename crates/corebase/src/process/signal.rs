//! `src/os/signal.cc`'s handler table and its sigaction path: registering a handler,
//! the two async-signal handlers, dispatching triggered signals from a safe context,
//! looking a handler up, clearing the table and blocking every signal.
//! The signalfd and kqueue paths (`signalfd.rs`, `signal_kqueue.rs`) share the
//! table, `SIGNALS`.
use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock, PoisonError};

/// `SW_SIGNO_MAX`.
const SIGNO_MAX: usize = crate::constants::SW_SIGNO_MAX as usize;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

/// `swoole::Signal`: the handler (a `SignalHandler`, `SIG_IGN` or `SIG_DFL`), its
/// signal number and whether it is registered.
pub struct Slot {
    pub(crate) handler: AtomicUsize,
    pub(crate) signo: AtomicU16,
    pub(crate) activated: AtomicBool,
}

impl Slot {
    const fn empty() -> Self {
        Self {
            handler: AtomicUsize::new(SIG_DFL),
            signo: AtomicU16::new(0),
            activated: AtomicBool::new(false),
        }
    }

    pub(crate) fn reset(&self) {
        self.handler.store(SIG_DFL, Relaxed);
        self.signo.store(0, Relaxed);
        self.activated.store(false, Relaxed);
    }
}

/// The handler table (upstream's `signals`).
pub static SIGNALS: [Slot; SIGNO_MAX] = [const { Slot::empty() }; SIGNO_MAX];
/// Each signal's action from before Swoole first registered it (upstream's
/// `origin_actions`; none: `SIG_DFL`), put back when its handler is removed.
static ORIGIN: Mutex<[Option<libc::sigaction>; SIGNO_MAX]> =
    Mutex::new([const { None }; SIGNO_MAX]);

fn origin(signo: c_int, f: impl FnOnce(&mut Option<libc::sigaction>)) {
    let mut origin = ORIGIN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(action) = usize::try_from(signo).ok().and_then(|i| origin.get_mut(i)) {
        f(action);
    }
}

/// Records `previous`, the action a first registration of `signo` replaced.
pub(crate) fn save_origin(signo: c_int, previous: Option<libc::sigaction>) {
    origin(signo, |action| *action = previous);
}

/// Puts back the action [`save_origin`] recorded for `signo`.
pub(crate) fn restore_origin(signo: c_int) {
    origin(signo, |action| {
        if let Some(action) = action.take() {
            // An action sigaction returned.
            sigaction(signo, Some(&action));
        } else {
            install(signo, SIG_DFL, false, false);
        }
    });
}

static TRIGGERED: [AtomicBool; SIGNO_MAX] = [const { AtomicBool::new(false) }; SIGNO_MAX];
/// `SwooleG.signal_dispatch`: some signal is waiting for `swr_v1_signal_dispatch`.
static PENDING: AtomicBool = AtomicBool::new(false);
/// `swoole_signal_callback` and `sw_reactor() != nullptr`, for the simple handler:
/// the pair the first `signal_set` gave. Reading it takes no lock, so a signal
/// frame may.
static HOOKS: OnceLock<(Dispatch, HasReactor)> = OnceLock::new();

/// A signal's handler as the table holds it: none, `SIG_IGN`, or a function to call
/// with the signal's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Handler(pub(crate) usize);

impl Handler {
    pub const NONE: Self = Self(SIG_DFL);
    pub const IGNORE: Self = Self(SIG_IGN);

    #[must_use]
    pub fn new(handler: extern "C" fn(c_int)) -> Self {
        Self(address(handler))
    }

    #[must_use]
    pub const fn raw(self) -> usize {
        self.0
    }

    /// Runs the handler with `signo`; nothing for [`Self::NONE`] or [`Self::IGNORE`].
    pub fn call(self, signo: c_int) {
        if self == Self::NONE || self == Self::IGNORE {
            return;
        }
        let pointer = std::ptr::with_exposed_provenance::<()>(self.0);
        // SAFETY: any other handler came from `new` (a safe `extern "C" fn(c_int)`)
        // or the table holding a previously registered handler.
        let handler = unsafe { std::mem::transmute::<*const (), extern "C" fn(c_int)>(pointer) };
        handler(signo);
    }
}

/// `swoole_signal_callback`: runs the registered handler, or warns.
pub type Dispatch = extern "C" fn(c_int);
/// Whether this thread has a reactor (`sw_reactor()`).
pub type HasReactor = fn() -> bool;

fn slot(signo: c_int) -> Option<&'static Slot> {
    SIGNALS.get(usize::try_from(signo).ok()?)
}

/// `SignalHandler swoole_signal_set(int signo, SignalHandler func, int restart, int mask)`.
#[must_use]
pub fn swoole_signal_set_action(signo: c_int, func: usize, restart: c_int, mask: c_int) -> usize {
    set_action(signo, func, restart != 0, mask != 0)
}

/// `swoole_signal_set(signo, func, restart, mask)`: installs `func` with sigaction
/// (null means `SIG_IGN`, -1 `SIG_DFL`, either of which unregisters `signo`) and
/// returns the previous action's handler, or null if sigaction failed.
fn set_action(signo: c_int, func: usize, restart: bool, mask: bool) -> usize {
    let func = match func {
        0 => SIG_IGN,
        usize::MAX => SIG_DFL,
        func => func,
    };
    if (func == SIG_IGN || func == SIG_DFL)
        && let Some(slot) = slot(signo)
    {
        slot.handler.store(SIG_DFL, Relaxed);
        slot.activated.store(false, Relaxed);
    }
    install(signo, func, restart, mask).map_or(0, |previous| previous.sa_sigaction)
}

/// Installs `func` (a handler, `SIG_IGN` or `SIG_DFL`) with sigaction; returns the
/// previous action, or none if sigaction failed.
pub(crate) fn install(
    signo: c_int,
    func: usize,
    restart: bool,
    mask: bool,
) -> Option<libc::sigaction> {
    let mut action: libc::sigaction = crate::sys::zeroed();
    action.sa_sigaction = func;
    if mask {
        crate::sys::sigfillset(&mut action.sa_mask);
    } else {
        crate::sys::sigemptyset(&mut action.sa_mask);
    }
    action.sa_flags = if restart { libc::SA_RESTART } else { 0 };
    // `func` is a handler or SIG_IGN/SIG_DFL.
    sigaction(signo, Some(&action))
}

/// `sigaction(2)`: the previous action, or none if it failed.
pub(super) fn sigaction(signo: c_int, action: Option<&libc::sigaction>) -> Option<libc::sigaction> {
    let mut previous: libc::sigaction = crate::sys::zeroed();
    let action = action.map_or(std::ptr::null(), std::ptr::from_ref);
    // SAFETY: the optional action is valid or null and previous receives the old action.
    let installed = unsafe { libc::sigaction(signo, action, &raw mut previous) };
    (installed == 0).then_some(previous)
}

/// `swoole_signal_set(signo, handler, safety)` without signalfd or kqueue: registers
/// `handler` and installs the safe (deferred) or simple (immediate unless a reactor
/// runs) async handler, with every signal masked while it runs. A null `handler`
/// removes a registered signal and puts back the action it replaced. `callback` and
/// `has_reactor` are the process's for good once given: later ones are ignored.
pub fn signal_set(
    signo: c_int,
    handler: Handler,
    safety: bool,
    callback: Dispatch,
    has_reactor: HasReactor,
) -> Handler {
    let handler = handler.0;
    HOOKS.get_or_init(|| (callback, has_reactor));
    register_with_reactor();
    if let Some(slot) = slot(signo) {
        if handler == 0 && slot.activated.load(Relaxed) {
            slot.reset();
            restore_origin(signo);
            return Handler::NONE;
        }
        let first = !slot.activated.load(Relaxed);
        slot.handler.store(handler, Relaxed);
        slot.activated.store(true, Relaxed);
        slot.signo.store(u16::try_from(signo).unwrap_or(0), Relaxed);
        let async_handler: extern "C" fn(c_int) = if safety { deferred } else { immediate };
        let previous = install(signo, address(async_handler), false, true);
        if first {
            save_origin(signo, previous);
        }
        return Handler(previous.map_or(0, |previous| previous.sa_sigaction));
    }
    let async_handler: extern "C" fn(c_int) = if safety { deferred } else { immediate };
    Handler(set_action(signo, address(async_handler), false, true))
}

/// Registers the loop's signal entries (`reactor::Signals`) once. Every installer that
/// can mark a signal pending ([`signal_set`]'s deferred handler) or add a kqueue signal
/// (`signal_kqueue_set`) calls it first, so the loop always reaches what they set.
pub(crate) fn register_with_reactor() {
    crate::reactor::set_signals(crate::reactor::Signals {
        dispatch: dispatch_if_pending,
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        kqueue: |signo| dispatch_kqueue(signo, signal_activated(signo)),
    });
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
/// An activated signal (`Some` handler) runs its handler unless that is `SIG_IGN`,
/// and warns when it has none.
pub(super) fn dispatch_kqueue(signo: c_int, activated: Option<Handler>) {
    let Some(handler) = activated else {
        return;
    };
    match handler {
        Handler::NONE => {
            let name = signal_name(signo);
            crate::log::error_log(
                crate::log::LogLevel::SW_LOG_WARNING,
                swoole_rs_error::ErrorCode::SW_ERROR_UNREGISTERED_SIGNAL as c_int,
                c"ReactorKqueue::wait",
                format_args!(
                    "Unable to find callback function for signal {}",
                    name.as_cstr().to_string_lossy()
                ),
            );
        }
        handler => handler.call(signo),
    }
}

/// `signal_handler_safety`: marks the signal for the next dispatch.
extern "C" fn deferred(signo: c_int) {
    if let Some(triggered) = usize::try_from(signo).ok().and_then(|i| TRIGGERED.get(i)) {
        triggered.store(true, Relaxed);
    }
    PENDING.store(true, Relaxed);
}

/// The `Dispatch` the first `signal_set` stored; none before it.
fn callback() -> Option<Dispatch> {
    HOOKS.get().map(|&(callback, _)| callback)
}

/// A handler's address, as `sigaction`'s `sa_sigaction` holds it.
fn address(handler: extern "C" fn(c_int)) -> usize {
    (handler as *const ()).expose_provenance()
}

/// `signal_handler_simple`: deferred under a reactor, otherwise the callback runs in
/// the signal frame, dropping a signal that arrives while one is running.
extern "C" fn immediate(signo: c_int) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if HOOKS.get().is_some_and(|&(_, has_reactor)| has_reactor()) {
        deferred(signo);
        return;
    }
    if RUNNING.swap(true, Relaxed) {
        return;
    }
    if let Some(callback) = callback() {
        callback(signo);
    }
    RUNNING.store(false, Relaxed);
}

/// `void swoole_signal_dispatch()`: runs `swoole_signal_callback` for each signal
/// the deferred handler marked, in signal order, clearing each mark before its
/// callback.
pub fn swoole_signal_dispatch() {
    dispatch_if_pending();
}

pub(crate) fn pending() -> bool {
    PENDING.load(Relaxed)
}

pub(crate) fn dispatch_if_pending() -> bool {
    let pending = pending();
    if pending {
        dispatch_pending(crate::process::pool::bailout_pending);
    }
    pending
}

// Keep the signal walk out of the hot reactor callback loop so its fixed slots
// can be unrolled even when the process and reactor modules share a crate.
#[inline(never)]
fn dispatch_pending(bailout_pending: impl Fn() -> bool) {
    // Only the deferred handler marks a signal, and `signal_set` stores its
    // `Dispatch` before installing that handler.
    let Some(callback) = callback() else {
        return;
    };
    PENDING.store(false, Relaxed);
    for (signo, triggered) in (0..).zip(&TRIGGERED) {
        if triggered.load(Relaxed) {
            triggered.store(false, Relaxed);
            // Upstream calls dispatch from a safe context, never a signal frame; the
            // callback may run PHP, and no value with a destructor is live across it.
            callback(signo);
            if bailout_pending() {
                // Resume the unfinished walk at the next safe dispatch.
                PENDING.store(true, Relaxed);
                return;
            }
        }
    }
    if pending() {
        // A callback raised a signal whose slot was already visited.
        _ = crate::reactor::event_api::swoole_event_defer_value(());
    }
}

/// What `swoole_signal_callback` should do for a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Call the registered handler.
    Handler(Handler),
    /// The number is out of range.
    OutOfRange,
    /// Nothing is registered (`SIG_DFL`).
    Default,
    /// The handler is `SIG_IGN`.
    Ignored,
}

impl Lookup {
    /// `swoole_signal_get_handler`: the registered handler, `SIG_DFL` or `SIG_IGN`,
    /// and 0 out of range.
    #[must_use]
    pub fn handler(self) -> Handler {
        match self {
            Self::Handler(handler) => handler,
            Self::OutOfRange | Self::Default => Handler::NONE,
            Self::Ignored => Handler::IGNORE,
        }
    }
}

/// The handler registered for `signo`.
#[must_use]
pub fn signal_lookup(signo: c_int) -> Lookup {
    let Some(slot) = slot(signo) else {
        return Lookup::OutOfRange;
    };
    match slot.handler.load(Relaxed) {
        SIG_DFL => Lookup::Default,
        SIG_IGN => Lookup::Ignored,
        handler => Lookup::Handler(Handler(handler)),
    }
}

/// The kqueue path's registration of `signo`: its handler while it is activated, else
/// none (and none out of range).
#[must_use]
pub fn signal_activated(signo: c_int) -> Option<Handler> {
    let slot = slot(signo).filter(|slot| slot.activated.load(Relaxed))?;
    Some(Handler(slot.handler.load(Relaxed)))
}

/// `bool swoole_signal_isset(int)`: a handler is registered and active.
#[must_use]
pub fn swoole_signal_isset(signo: c_int) -> bool {
    slot(signo).is_some_and(|slot| slot.handler.load(Relaxed) != 0 && slot.activated.load(Relaxed))
}

/// Empties the table (upstream's `sw_memset_zero(signals, sizeof(signals))`).
pub fn signal_reset() {
    SIGNALS.iter().for_each(Slot::reset);
}

/// `swoole_signal_clear` without signalfd or kqueue: restores `SIG_DFL` for every
/// registered signal, then empties the table.
pub fn signal_clear() {
    clear_actions(|_| true);
    signal_reset();
}

/// Whether a signal now routed to signalfd still has one of our native handlers.
#[cfg(target_os = "linux")]
pub(super) fn has_native_handler(signo: c_int) -> bool {
    sigaction(signo, None).is_some_and(|action| {
        [address(immediate), address(deferred)].contains(&action.sa_sigaction)
    })
}

/// Restore the default disposition of active signals handled through sigaction.
pub(super) fn clear_actions(native: impl Fn(c_int) -> bool) {
    for slot in &SIGNALS {
        let signo = c_int::from(slot.signo.load(Relaxed));
        if slot.activated.load(Relaxed) && native(signo) {
            set_action(signo, usize::MAX, true, false);
        }
    }
}

struct SavedSignal {
    number: c_int,
    handler: usize,
    registered: u16,
    slot: &'static Slot,
    active: bool,
    action: libc::sigaction,
    origin: Option<libc::sigaction>,
    blocked: bool,
    #[cfg(target_os = "linux")]
    descriptor: bool,
}

/// Temporarily install native pool handlers, preserving how the calling thread
/// received each signal, including those previously routed through signalfd.
#[must_use]
pub struct SignalGuard<const N: usize> {
    saved: [Option<SavedSignal>; N],
    mask: libc::sigset_t,
    pid: c_int,
}

impl<const N: usize> SignalGuard<N> {
    /// Install the handlers until the guard drops in the installing process.
    pub fn install(numbers: [c_int; N], handler: Handler) -> Self {
        use crate::sys;
        let mut mask = sys::zeroed();
        sys::sigemptyset(&mut mask);
        for signo in numbers.into_iter().filter(|&signo| signo > 0) {
            sys::sigaddset(&mut mask, signo);
        }
        let mut previous = sys::zeroed();
        sys::pthread_sigmask(libc::SIG_BLOCK, Some(&mask), Some(&mut previous));
        let mut unblock = mask;
        let saved = numbers.map(|number| {
            if number == 0 {
                return None;
            }
            let slot = slot(number)?;
            let action = sigaction(number, None)?;
            let mut saved_origin = None;
            origin(number, |old| saved_origin = *old);
            let saved = SavedSignal {
                number,
                handler: slot.handler.load(Relaxed),
                registered: slot.signo.load(Relaxed),
                slot,
                active: slot.activated.load(Relaxed),
                action,
                origin: saved_origin,
                blocked: sys::sigismember(&previous, number) == 1,
                #[cfg(target_os = "linux")]
                descriptor: super::signalfd::route(number, false),
            };
            let keep_blocked = saved.blocked;
            #[cfg(target_os = "linux")]
            let keep_blocked = keep_blocked && !saved.descriptor;
            if keep_blocked {
                sys::sigdelset(&mut unblock, number);
            }
            signal_set(
                number,
                handler,
                false,
                super::signal_upstream::swoole_signal_callback,
                super::signal_upstream::has_reactor,
            );
            Some(saved)
        });
        #[cfg(target_os = "linux")]
        super::signalfd::update_mask();
        sys::pthread_sigmask(libc::SIG_UNBLOCK, Some(&unblock), None);
        Self {
            saved,
            mask,
            pid: super::os::getpid(),
        }
    }
}

impl<const N: usize> Drop for SignalGuard<N> {
    fn drop(&mut self) {
        use crate::sys;
        if super::os::getpid() != self.pid {
            return;
        }
        sys::pthread_sigmask(libc::SIG_BLOCK, Some(&self.mask), None);
        let mut unblock = self.mask;
        for saved in self.saved.iter().flatten().rev() {
            let number = saved.number;
            saved.slot.handler.store(saved.handler, Relaxed);
            saved.slot.signo.store(saved.registered, Relaxed);
            saved.slot.activated.store(saved.active, Relaxed);
            save_origin(number, saved.origin);
            sigaction(number, Some(&saved.action));
            if saved.blocked {
                sys::sigdelset(&mut unblock, number);
            }
            #[cfg(target_os = "linux")]
            super::signalfd::route(number, saved.descriptor);
        }
        #[cfg(target_os = "linux")]
        {
            super::signalfd::update_mask();
            if self.saved.iter().flatten().any(|saved| saved.descriptor) {
                super::signal_upstream::restore_descriptor();
            }
        }
        sys::pthread_sigmask(libc::SIG_UNBLOCK, Some(&unblock), None);
    }
}

/// `void swoole_signal_unblock_all()`: [`crate::signal_unblock_all`].
pub fn swoole_signal_unblock_all() {
    crate::signal_unblock_all();
}

/// `swoole_signal_to_str`'s text, in a buffer of its own.
pub struct SignalName([u8; 64]);

impl SignalName {
    #[must_use]
    pub fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.0).unwrap_or_default()
    }
}

/// `strsignal`'s text, with `": <signo>"` appended when the text has no colon. It
/// may run in a signal frame (an unregistered signal's warning), so it does not
/// allocate.
#[must_use]
pub fn signal_name(signo: c_int) -> SignalName {
    // libc's `strsignal(3)`, which takes any number, reads no caller memory and
    // returns a C string that stays put (glibc, musl and Apple's never null).
    unsafe extern "C" {
        safe fn strsignal(signo: c_int) -> Option<crate::sys::Text<'static>>;
    }
    let text = strsignal(signo).map_or(c"", crate::sys::Text::get);
    let mut buf = [0; 64];
    signal_text(text.to_bytes(), signo, &mut buf);
    SignalName(buf)
}

/// `snprintf(buf, 64, "%s", name)`, then `": <signo>"` into what is left when
/// the name has no colon; always NUL-terminated.
fn signal_text(name: &[u8], signo: c_int, buf: &mut [u8; 64]) {
    let name = name.get(..63).unwrap_or(name);
    // A sign and up to ten digits, written from the end.
    let mut digits = [0_u8; 12];
    let mut at = digits.len();
    let mut value = signo.unsigned_abs();
    for digit in digits.iter_mut().rev() {
        *digit = b'0' + u8::try_from(value % 10).unwrap_or(0);
        at -= 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    if signo < 0 {
        at -= 1;
        if let Some(sign) = digits.get_mut(at) {
            *sign = b'-';
        }
    }
    let suffix: [&[u8]; 2] = if name.contains(&b':') {
        [b"", b""]
    } else {
        [b": ", digits.get(at..).unwrap_or_default()]
    };
    let text = name.iter().chain(suffix.into_iter().flatten()).take(63);
    let mut len = 0;
    for (slot, &byte) in buf.iter_mut().zip(text) {
        *slot = byte;
        len += 1;
    }
    if let Some(terminator) = buf.get_mut(len) {
        *terminator = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI32;

    static SEEN: AtomicI32 = AtomicI32::new(0);
    extern "C" fn record(signo: c_int) {
        SEEN.store(signo, Relaxed);
    }
    fn no_reactor() -> bool {
        false
    }
    fn with_reactor() -> bool {
        true
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn registers_dispatches_and_clears() {
        let _state = crate::process::signal_state_lock();
        let handler = record as *const () as usize;
        signal_set(libc::SIGUSR1, Handler(handler), false, record, no_reactor);
        assert!(swoole_signal_isset(libc::SIGUSR1));
        assert_eq!(
            signal_lookup(libc::SIGUSR1),
            Lookup::Handler(Handler(handler))
        );
        // SAFETY: SIGUSR1 has the immediate handler installed above.
        unsafe { libc::raise(libc::SIGUSR1) };
        assert_eq!(SEEN.swap(0, Relaxed), libc::SIGUSR1);

        signal_set(libc::SIGUSR2, Handler(handler), true, record, with_reactor);
        // SAFETY: SIGUSR2 has the deferred handler installed above.
        unsafe { libc::raise(libc::SIGUSR2) };
        assert_eq!(SEEN.load(Relaxed), 0);
        // Through the entry `signal_set` registered with the loop, as its end callback runs it.
        assert!(crate::reactor::signals::dispatch());
        assert_eq!(SEEN.swap(0, Relaxed), libc::SIGUSR2);
        assert!(!crate::reactor::signals::dispatch());

        // A bailout interrupts the walk after the first callback.
        let mut signals = [libc::SIGUSR1, libc::SIGUSR2];
        signals.sort_unstable();
        for signo in signals {
            deferred(signo);
        }
        dispatch_pending(|| true);
        assert_eq!(SEEN.swap(0, Relaxed), signals[0]);
        assert!(dispatch_if_pending());
        assert_eq!(SEEN.swap(0, Relaxed), signals[1]);
        assert!(!pending());

        // Removing a handler puts back the action from before its registration.
        set_action(libc::SIGUSR2, 0, false, false);
        signal_set(libc::SIGUSR2, Handler(handler), true, record, with_reactor);
        signal_set(libc::SIGUSR2, Handler(handler), false, record, with_reactor);
        assert_eq!(
            signal_set(libc::SIGUSR2, Handler::NONE, false, record, with_reactor),
            Handler::NONE
        );
        assert!(!swoole_signal_isset(libc::SIGUSR2));
        assert_eq!(set_action(libc::SIGUSR2, usize::MAX, false, false), SIG_IGN);

        #[cfg(target_os = "linux")]
        {
            use crate::process::signalfd::{signalfd_clear, signalfd_set};
            // SIGUSR1 already uses the immediate handler. Routing it through the
            // descriptor must retire that disposition before the mask is unblocked.
            signalfd_set(libc::SIGUSR1, Handler(handler));
            assert!(has_native_handler(libc::SIGUSR1));
            assert!(signalfd_clear());
            assert_eq!(set_action(libc::SIGUSR1, usize::MAX, false, false), SIG_DFL);
        }

        signal_clear();
        assert!(!swoole_signal_isset(libc::SIGUSR1));
        assert_eq!(signal_lookup(libc::SIGUSR1), Lookup::Default);
        assert_eq!(signal_lookup(200), Lookup::OutOfRange);
        assert_eq!(set_action(libc::SIGUSR1, 0, false, false), SIG_DFL);
        assert_eq!(signal_lookup(libc::SIGUSR1), Lookup::Default);
        set_action(libc::SIGUSR1, usize::MAX, false, false);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no signalfd")]
    fn pool_guard_restores_a_descriptor_signal() {
        use crate::process::signalfd;
        use crate::signal_mask::tests::{current, same};

        let _state = crate::process::signal_state_lock();
        let signo = libc::SIGURG;
        let original_mask = current();
        let old = sigaction(signo, None).unwrap();
        let handler = Handler::new(record);
        signal_set(signo, handler, true, record, no_reactor);
        let action = sigaction(signo, None).unwrap();
        signalfd::signalfd_set(signo, handler);
        signalfd::signalfd_create().unwrap();
        let blocked = current();

        let guard = SignalGuard::install([signo], Handler::new(deferred));
        assert_eq!(
            signal_lookup(signo),
            Lookup::Handler(Handler::new(deferred))
        );
        assert_ne!(
            sigaction(signo, None).unwrap().sa_sigaction,
            action.sa_sigaction
        );
        assert_eq!(crate::sys::sigismember(&current(), signo), 0);
        assert!(!signalfd::route(signo, false));
        drop(guard);
        assert_eq!(signal_lookup(signo), Lookup::Handler(handler));
        assert_eq!(
            sigaction(signo, None).unwrap().sa_sigaction,
            action.sa_sigaction
        );
        assert!(same(current(), blocked));
        assert!(signalfd::route(signo, true));

        assert!(signalfd::signalfd_clear());
        signalfd::signalfd_close();
        sigaction(signo, Some(&old));
        crate::sys::pthread_sigmask(libc::SIG_SETMASK, Some(&original_mask), None);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strsignal")]
    fn signal_text_appends_the_number_without_a_colon() {
        let text = |name: &[u8], signo| {
            let mut buf = [0xff; 64];
            signal_text(name, signo, &mut buf);
            let end = buf.iter().position(|&b| b == 0).unwrap();
            buf[..end].to_vec()
        };
        assert_eq!(text(b"Interrupt", 2), b"Interrupt: 2");
        assert_eq!(text(b"Unknown signal: 99", 99), b"Unknown signal: 99");
        assert_eq!(text(b"x", -12), b"x: -12");
        let long = [b'x'; 70];
        assert_eq!(text(&long, 7), &long[..63]);
        let mut near = [b'y'; 61].to_vec();
        near.extend_from_slice(b": 1");
        assert_eq!(text(&[b'y'; 61], 15), &near[..63]);
        let named = signal_name(libc::SIGTERM);
        let named = named.as_cstr();
        assert!(named.to_bytes().ends_with(b": 15"), "{named:?}");
    }
}
