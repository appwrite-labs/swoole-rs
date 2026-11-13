//! `src/os/signal.cc`'s handler table and its sigaction path: registering a handler,
//! the two async-signal handlers, dispatching triggered signals from a safe context,
//! looking a handler up, clearing the table and blocking every signal.
//! The signalfd and kqueue paths (`signalfd.rs`, `signal_kqueue.rs`) share the
//! table, `SIGNALS`.
use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, PoisonError};

/// `SW_SIGNO_MAX`.
const SIGNO_MAX: usize = 128;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

/// `swoole::Signal`: the handler (a `SignalHandler`, `SIG_IGN` or `SIG_DFL`), its
/// signal number and whether it is registered.
#[repr(C)]
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
            sigaction(signo, &action);
        } else {
            install(signo, SIG_DFL, false, false);
        }
    });
}

static TRIGGERED: [AtomicBool; SIGNO_MAX] = [const { AtomicBool::new(false) }; SIGNO_MAX];
/// `SwooleG.signal_dispatch`: some signal is waiting for `swr_v1_signal_dispatch`.
static PENDING: AtomicBool = AtomicBool::new(false);
/// `swoole_signal_callback` and `sw_reactor() != nullptr`, for the simple handler.
static CALLBACK: AtomicUsize = AtomicUsize::new(0);
static HAS_REACTOR: AtomicUsize = AtomicUsize::new(0);

/// `swoole_signal_callback`: runs the registered handler, or warns.
pub type Callback = unsafe extern "C" fn(c_int);
/// Whether this thread has a reactor (`sw_reactor()`).
pub type HasReactor = fn() -> bool;

fn slot(signo: c_int) -> Option<&'static Slot> {
    SIGNALS.get(usize::try_from(signo).ok()?)
}

/// `SignalHandler swoole_signal_set(int signo, SignalHandler func, int restart, int mask)`.
#[must_use]
#[unsafe(export_name = "_Z17swoole_signal_setiPFviEii")]
pub extern "C" fn swoole_signal_set_action(
    signo: c_int,
    func: usize,
    restart: c_int,
    mask: c_int,
) -> usize {
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
    let mut action: libc::sigaction = swoole_rs_corebase::sys::zeroed();
    action.sa_sigaction = func;
    if mask {
        swoole_rs_corebase::sys::sigfillset(&mut action.sa_mask);
    } else {
        swoole_rs_corebase::sys::sigemptyset(&mut action.sa_mask);
    }
    action.sa_flags = if restart { libc::SA_RESTART } else { 0 };
    // `func` is a handler or SIG_IGN/SIG_DFL.
    sigaction(signo, &action)
}

/// `sigaction(2)`: the previous action, or none if it failed.
fn sigaction(signo: c_int, action: &libc::sigaction) -> Option<libc::sigaction> {
    let mut previous: libc::sigaction = swoole_rs_corebase::sys::zeroed();
    // SAFETY: both actions are valid for the call.
    let installed = unsafe { libc::sigaction(signo, action, &raw mut previous) };
    (installed == 0).then_some(previous)
}

/// `swoole_signal_set(signo, handler, safety)` without signalfd or kqueue: registers
/// `handler` and installs the safe (deferred) or simple (immediate unless a reactor
/// runs) async handler, with every signal masked while it runs. A null `handler`
/// removes a registered signal and puts back the action it replaced.
pub fn signal_set(
    signo: c_int,
    handler: usize,
    safety: bool,
    callback: Callback,
    has_reactor: HasReactor,
) -> usize {
    CALLBACK.store(callback as usize, Relaxed);
    HAS_REACTOR.store(has_reactor as usize, Relaxed);
    if let Some(slot) = slot(signo) {
        if handler == 0 && slot.activated.load(Relaxed) {
            slot.reset();
            restore_origin(signo);
            return 0;
        }
        let first = !slot.activated.load(Relaxed);
        slot.handler.store(handler, Relaxed);
        slot.activated.store(true, Relaxed);
        slot.signo.store(u16::try_from(signo).unwrap_or(0), Relaxed);
        let async_handler: extern "C" fn(c_int) = if safety { deferred } else { immediate };
        let previous = install(signo, async_handler as usize, false, true);
        if first {
            save_origin(signo, previous);
        }
        return previous.map_or(0, |previous| previous.sa_sigaction);
    }
    let async_handler: extern "C" fn(c_int) = if safety { deferred } else { immediate };
    set_action(signo, async_handler as usize, false, true)
}

/// `signal_handler_safety`: marks the signal for the next dispatch.
extern "C" fn deferred(signo: c_int) {
    if let Some(triggered) = usize::try_from(signo).ok().and_then(|i| TRIGGERED.get(i)) {
        triggered.store(true, Relaxed);
    }
    PENDING.store(true, Relaxed);
}

/// The `Callback` `signal_set` stored last; none before the first.
fn callback() -> Option<Callback> {
    // SAFETY: the slot holds 0 or a `Callback` `signal_set` stored, and a null
    // `Option` of a function pointer is `None`.
    unsafe { std::mem::transmute::<usize, Option<Callback>>(CALLBACK.load(Relaxed)) }
}

/// `signal_handler_simple`: deferred under a reactor, otherwise the callback runs in
/// the signal frame, dropping a signal that arrives while one is running.
extern "C" fn immediate(signo: c_int) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    // SAFETY: `signal_set` stored a `HasReactor` before installing this handler.
    let has_reactor: HasReactor = unsafe { std::mem::transmute(HAS_REACTOR.load(Relaxed)) };
    if has_reactor() {
        deferred(signo);
        return;
    }
    if RUNNING.swap(true, Relaxed) {
        return;
    }
    if let Some(callback) = callback() {
        // SAFETY: the native callback takes any signal number.
        unsafe { callback(signo) };
    }
    RUNNING.store(false, Relaxed);
}

/// `void swoole_signal_dispatch()`: runs `swoole_signal_callback` for each signal
/// the deferred handler marked.
#[unsafe(export_name = "_Z22swoole_signal_dispatchv")]
pub extern "C" fn swoole_signal_dispatch() {
    if !PENDING.load(Relaxed) {
        return;
    }
    // Only the deferred handler marks a signal, and `signal_set` stores its
    // `Callback` before installing that handler.
    let Some(callback) = callback() else {
        return;
    };
    // SAFETY: upstream calls dispatch from a safe context, never a signal frame.
    unsafe { dispatch(callback) };
}

/// Runs `callback` for each signal the deferred handler marked, in signal order,
/// clearing each mark after its callback.
///
/// # Safety
/// `callback` may run PHP; no value with a destructor is live across it.
pub unsafe fn dispatch(callback: Callback) {
    if !PENDING.load(Relaxed) {
        return;
    }
    for (signo, triggered) in (0..).zip(&TRIGGERED) {
        if triggered.load(Relaxed) {
            // SAFETY: the native callback takes any signal number.
            unsafe { callback(signo) };
            triggered.store(false, Relaxed);
        }
    }
    PENDING.store(false, Relaxed);
}

/// What `swoole_signal_callback` should do for a signal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lookup {
    /// 0 call `handler`; 1 the number is out of range; 2 nothing is registered;
    /// 3 the handler is `SIG_IGN` or `SIG_DFL`.
    pub status: u32,
    /// The registered handler (`swoole_signal_get_handler`).
    pub handler: usize,
}

/// The handler registered for `signo`.
#[must_use]
pub fn signal_lookup(signo: c_int) -> Lookup {
    let Some(slot) = slot(signo) else {
        return Lookup {
            status: 1,
            handler: 0,
        };
    };
    let handler = slot.handler.load(Relaxed);
    let status = match handler {
        SIG_DFL => 2,
        SIG_IGN => 3,
        _ => 0,
    };
    Lookup { status, handler }
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
    for slot in &SIGNALS {
        if slot.activated.load(Relaxed) {
            set_action(
                c_int::from(slot.signo.load(Relaxed)),
                usize::MAX,
                true,
                false,
            );
        }
    }
    signal_reset();
}

/// `bool swoole_signal_block_all()`: [`swoole_rs_corebase::signal_block_all`].
#[must_use]
#[unsafe(export_name = "_Z23swoole_signal_block_allv")]
pub extern "C" fn swoole_signal_block_all() -> bool {
    swoole_rs_corebase::signal_block_all()
}

/// `void swoole_signal_unblock_all()`: [`swoole_rs_corebase::signal_unblock_all`].
pub fn swoole_signal_unblock_all() {
    swoole_rs_corebase::signal_unblock_all();
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
    // SAFETY: strsignal accepts any number.
    let name = unsafe { libc::strsignal(signo) };
    // SAFETY: strsignal returns a C string.
    let text = unsafe { CStr::from_ptr(name) };
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
    unsafe extern "C" fn record(signo: c_int) {
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
        let _state = crate::signal_state_lock();
        let handler = record as *const () as usize;
        signal_set(libc::SIGUSR1, handler, false, record, no_reactor);
        assert!(swoole_signal_isset(libc::SIGUSR1));
        assert_eq!(signal_lookup(libc::SIGUSR1), Lookup { status: 0, handler });
        // SAFETY: SIGUSR1 has the immediate handler installed above.
        unsafe { libc::raise(libc::SIGUSR1) };
        assert_eq!(SEEN.swap(0, Relaxed), libc::SIGUSR1);

        signal_set(libc::SIGUSR2, handler, true, record, with_reactor);
        // SAFETY: SIGUSR2 has the deferred handler installed above.
        unsafe { libc::raise(libc::SIGUSR2) };
        assert_eq!(SEEN.load(Relaxed), 0);
        swoole_signal_dispatch();
        assert_eq!(SEEN.swap(0, Relaxed), libc::SIGUSR2);

        // Removing a handler puts back the action from before its registration.
        set_action(libc::SIGUSR2, 0, false, false);
        signal_set(libc::SIGUSR2, handler, true, record, with_reactor);
        signal_set(libc::SIGUSR2, handler, false, record, with_reactor);
        assert_eq!(signal_set(libc::SIGUSR2, 0, false, record, with_reactor), 0);
        assert!(!swoole_signal_isset(libc::SIGUSR2));
        assert_eq!(set_action(libc::SIGUSR2, usize::MAX, false, false), SIG_IGN);

        signal_clear();
        assert!(!swoole_signal_isset(libc::SIGUSR1));
        assert_eq!(signal_lookup(libc::SIGUSR1).status, 2);
        assert_eq!(signal_lookup(200).status, 1);
        assert_eq!(set_action(libc::SIGUSR1, 0, false, false), SIG_DFL);
        assert_eq!(signal_lookup(libc::SIGUSR1).status, 2);
        set_action(libc::SIGUSR1, usize::MAX, false, false);
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
