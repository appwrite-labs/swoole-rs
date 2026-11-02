//! `src/os/signal.cc`'s handler table and its sigaction path: registering a handler,
//! the two async-signal handlers, dispatching triggered signals from a safe context,
//! looking a handler up, clearing the table and blocking every signal.
//! The signalfd and kqueue paths (`signalfd.rs`, `signal_kqueue.rs`) share the
//! table, `swr_v1_signals`, laid out as `swoole::Signal`.
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering::Relaxed};

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
#[unsafe(no_mangle)]
pub static swr_v1_signals: [Slot; SIGNO_MAX] = [const { Slot::empty() }; SIGNO_MAX];
static TRIGGERED: [AtomicBool; SIGNO_MAX] = [const { AtomicBool::new(false) }; SIGNO_MAX];
/// `SwooleG.signal_dispatch`: some signal is waiting for `swr_v1_signal_dispatch`.
static PENDING: AtomicBool = AtomicBool::new(false);
/// `swoole_signal_callback` and `sw_reactor() != nullptr`, for the simple handler.
static CALLBACK: AtomicUsize = AtomicUsize::new(0);
static HAS_REACTOR: AtomicUsize = AtomicUsize::new(0);

/// `swoole_signal_callback`: runs the registered handler, or warns.
pub type Callback = unsafe extern "C" fn(c_int);
/// Whether this thread has a reactor (`sw_reactor()`).
pub type HasReactor = unsafe extern "C" fn() -> bool;

fn slot(signo: c_int) -> Option<&'static Slot> {
    swr_v1_signals.get(usize::try_from(signo).ok()?)
}

/// `swoole_signal_set(signo, func, restart, mask)`: installs `func` with sigaction
/// (null means `SIG_IGN`, -1 `SIG_DFL`, either of which unregisters `signo`) and
/// returns the previous action's handler, or null if sigaction failed.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_signal_set_action(
    signo: c_int,
    func: usize,
    restart: bool,
    mask: bool,
) -> usize {
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
    // SAFETY: sigaction is plain data; all zeroes is an empty action.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = func;
    let fill: unsafe extern "C" fn(*mut libc::sigset_t) -> c_int = if mask {
        libc::sigfillset
    } else {
        libc::sigemptyset
    };
    // SAFETY: `sa_mask` is a valid sigset_t.
    unsafe { fill(&raw mut action.sa_mask) };
    action.sa_flags = if restart { libc::SA_RESTART } else { 0 };
    // SAFETY: both actions are valid; `func` is a handler or SIG_IGN/SIG_DFL.
    if unsafe { libc::sigaction(signo, &raw const action, &raw mut previous) } < 0 {
        return 0;
    }
    previous.sa_sigaction
}

/// `swoole_signal_set(signo, handler, safety)` without signalfd or kqueue: registers
/// `handler` and installs the safe (deferred) or simple (immediate unless a reactor
/// runs) async handler, with every signal masked while it runs.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_signal_set(
    signo: c_int,
    handler: usize,
    safety: bool,
    callback: Callback,
    has_reactor: HasReactor,
) -> usize {
    CALLBACK.store(callback as usize, Relaxed);
    HAS_REACTOR.store(has_reactor as usize, Relaxed);
    if let Some(slot) = slot(signo) {
        slot.handler.store(handler, Relaxed);
        slot.activated.store(true, Relaxed);
        slot.signo.store(u16::try_from(signo).unwrap_or(0), Relaxed);
    }
    let async_handler: extern "C" fn(c_int) = if safety { deferred } else { immediate };
    swr_v1_signal_set_action(signo, async_handler as usize, false, true)
}

/// `signal_handler_safety`: marks the signal for the next dispatch.
extern "C" fn deferred(signo: c_int) {
    if let Some(triggered) = usize::try_from(signo).ok().and_then(|i| TRIGGERED.get(i)) {
        triggered.store(true, Relaxed);
    }
    PENDING.store(true, Relaxed);
}

/// `signal_handler_simple`: deferred under a reactor, otherwise the callback runs in
/// the signal frame, dropping a signal that arrives while one is running.
extern "C" fn immediate(signo: c_int) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    // SAFETY: `swr_v1_signal_set` stored a `HasReactor` before installing this handler.
    let has_reactor: HasReactor = unsafe { std::mem::transmute(HAS_REACTOR.load(Relaxed)) };
    // SAFETY: the native accessor only reads a thread-local.
    if unsafe { has_reactor() } {
        deferred(signo);
        return;
    }
    if RUNNING.swap(true, Relaxed) {
        return;
    }
    // SAFETY: as above, for the `Callback`.
    let callback: Callback = unsafe { std::mem::transmute(CALLBACK.load(Relaxed)) };
    // SAFETY: the native callback takes any signal number.
    unsafe { callback(signo) };
    RUNNING.store(false, Relaxed);
}

/// `swoole_signal_dispatch`: runs `callback` for each signal the deferred handler
/// marked, in signal order, clearing each mark after its callback.
///
/// # Safety
/// `callback` may run PHP; no value with a destructor is live across it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_signal_dispatch(callback: Callback) {
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_signal_lookup(signo: c_int) -> Lookup {
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

/// `swoole_signal_isset`: a handler is registered and active.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_signal_isset(signo: c_int) -> bool {
    slot(signo).is_some_and(|slot| slot.handler.load(Relaxed) != 0 && slot.activated.load(Relaxed))
}

/// Empties the table (upstream's `sw_memset_zero(signals, sizeof(signals))`).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_signal_reset() {
    swr_v1_signals.iter().for_each(Slot::reset);
}

/// `swoole_signal_clear` without signalfd or kqueue: restores `SIG_DFL` for every
/// registered signal, then empties the table.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_signal_clear() {
    for slot in &swr_v1_signals {
        if slot.activated.load(Relaxed) {
            swr_v1_signal_set_action(
                c_int::from(slot.signo.load(Relaxed)),
                usize::MAX,
                true,
                false,
            );
        }
    }
    swr_v1_signal_reset();
}

/// `swoole_signal_block_all` (`block`) and `swoole_signal_unblock_all`: changes this
/// thread's mask for every signal unless `*blocking_all` already says it is done.
/// Upstream's failure check (`pthread_sigmask(...) < 0`) never holds, so the flag
/// always follows.
///
/// # Safety
/// `blocking_all` points to `SwooleTG.signal_blocking_all`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_signal_mask_all(block: bool, blocking_all: *mut bool) {
    // SAFETY: the caller passes this thread's flag.
    let blocking_all = unsafe { &mut *blocking_all };
    if *blocking_all == block {
        return;
    }
    // SAFETY: sigset_t is plain data.
    let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `mask` is a valid sigset_t.
    unsafe { libc::sigfillset(&raw mut mask) };
    let how = if block {
        libc::SIG_BLOCK
    } else {
        libc::SIG_UNBLOCK
    };
    // SAFETY: `mask` is initialized; the old mask is not wanted.
    unsafe { libc::pthread_sigmask(how, &raw const mask, std::ptr::null_mut()) };
    *blocking_all = block;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI32;

    static SEEN: AtomicI32 = AtomicI32::new(0);
    unsafe extern "C" fn record(signo: c_int) {
        SEEN.store(signo, Relaxed);
    }
    unsafe extern "C" fn no_reactor() -> bool {
        false
    }
    unsafe extern "C" fn with_reactor() -> bool {
        true
    }

    #[test]
    fn registers_dispatches_and_clears() {
        let _state = crate::signal_state_lock();
        let handler = record as *const () as usize;
        swr_v1_signal_set(libc::SIGUSR1, handler, false, record, no_reactor);
        assert!(swr_v1_signal_isset(libc::SIGUSR1));
        assert_eq!(
            swr_v1_signal_lookup(libc::SIGUSR1),
            Lookup { status: 0, handler }
        );
        // SAFETY: SIGUSR1 has the immediate handler installed above.
        unsafe { libc::raise(libc::SIGUSR1) };
        assert_eq!(SEEN.swap(0, Relaxed), libc::SIGUSR1);

        swr_v1_signal_set(libc::SIGUSR2, handler, true, record, with_reactor);
        // SAFETY: SIGUSR2 has the deferred handler installed above.
        unsafe { libc::raise(libc::SIGUSR2) };
        assert_eq!(SEEN.load(Relaxed), 0);
        unsafe { swr_v1_signal_dispatch(record) };
        assert_eq!(SEEN.swap(0, Relaxed), libc::SIGUSR2);

        swr_v1_signal_clear();
        assert!(!swr_v1_signal_isset(libc::SIGUSR1));
        assert_eq!(swr_v1_signal_lookup(libc::SIGUSR1).status, 2);
        assert_eq!(swr_v1_signal_lookup(200).status, 1);
        assert_eq!(
            swr_v1_signal_set_action(libc::SIGUSR1, 0, false, false),
            SIG_DFL
        );
        assert_eq!(swr_v1_signal_lookup(libc::SIGUSR1).status, 2);
        swr_v1_signal_set_action(libc::SIGUSR1, usize::MAX, false, false);
    }

    #[test]
    fn masks_all_once() {
        let mut blocking = false;
        unsafe { swr_v1_signal_mask_all(true, &raw mut blocking) };
        assert!(blocking);
        unsafe { swr_v1_signal_mask_all(false, &raw mut blocking) };
        assert!(!blocking);
    }
}
