//! `System::wait_signal(signals, timeout)`: park the current coroutine until one of
//! `signals` arrives. The listener table and the wake-up handler are here; native
//! code passes signal registration, the reactor exit condition and listener count,
//! and the coroutine operations in `SignalOps`.
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering::Relaxed};

/// `SW_SIGNO_MAX`.
const SIGNO_MAX: i32 = 128;

/// A coroutine waiting for a signal (upstream's `SignalListener`), on its stack.
struct Listener {
    co: *mut c_void,
    signo: i32,
}

/// Which listener each signal wakes (upstream's `listeners`).
static LISTENERS: [AtomicPtr<Listener>; SIGNO_MAX as usize] =
    [const { AtomicPtr::new(std::ptr::null_mut()) }; SIGNO_MAX as usize];
/// `SignalOps::resume`, for the signal handler.
static RESUME: AtomicUsize = AtomicUsize::new(0);

/// The native operations a signal wait needs.
#[repr(C)]
pub struct SignalOps {
    /// `Coroutine::get_current_safe()`.
    pub current: unsafe extern "C" fn() -> *mut c_void,
    /// Whether a non-coroutine signal listener exists (`signal_listener_num > 0`).
    pub busy: unsafe extern "C" fn() -> bool,
    /// On the loop's thread under thread contexts: with `true`, unless the record says
    /// so already, records `signo`'s handler in it and installs `swr_v1_hook_signal_wake`;
    /// with `false`, puts the recorded handler back.
    pub listen: unsafe extern "C" fn(i32, *mut Previous, bool),
    /// The reactor exit condition and `signal_async_listener_num++` (true), or `--`.
    pub waiting: unsafe extern "C" fn(bool),
    /// `co->yield_ex(timeout)`: false on timeout or cancellation.
    pub yield_for: unsafe extern "C" fn(*mut c_void, f64) -> bool,
    /// `co->resume()`.
    pub resume: unsafe extern "C" fn(*mut c_void),
}

/// A signal's handler from before a wait installed its own (upstream's `PreviousSignal`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Previous {
    /// The `SignalHandler`, or 0.
    pub handler: usize,
    /// Whether the wait's handler is installed.
    pub registered: bool,
}

/// What `wait_signal` returns.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signalled {
    /// The signal that arrived, or -1.
    pub signo: i32,
    /// For native to set as the last error, or 0.
    pub last_error: i32,
}

fn listener(signo: i32) -> Option<&'static AtomicPtr<Listener>> {
    LISTENERS.get(usize::try_from(signo).ok()?)
}

/// `signo`'s table index and listener slot.
fn slot(signo: i32) -> Option<(usize, &'static AtomicPtr<Listener>)> {
    let index = usize::try_from(signo).ok()?;
    Some((index, LISTENERS.get(index)?))
}

/// The signal handler: wakes the coroutine listening for `signo`, once.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_signal_wake(signo: i32) {
    let Some(slot) = listener(signo) else {
        return;
    };
    let waiting = slot.swap(std::ptr::null_mut(), Relaxed);
    if waiting.is_null() {
        return;
    }
    // SAFETY: a registered listener lives on its parked coroutine's stack.
    let waiting = unsafe { &mut *waiting };
    waiting.signo = signo;
    // SAFETY: `wait_signal` stored the native resume before registering anyone.
    let resume: unsafe extern "C" fn(*mut c_void) =
        unsafe { std::mem::transmute(RESUME.load(Relaxed)) };
    // SAFETY: `co` is parked in `swr_v1_hook_wait_signal`.
    unsafe { resume(waiting.co) };
}

/// Waits for any of `signals[..count]` for up to `timeout` seconds.
///
/// # Safety
/// `signals` holds `count` numbers; `ops` holds the native operations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_wait_signal(
    signals: *const i32,
    count: usize,
    timeout: f64,
    ops: &SignalOps,
) -> Signalled {
    // SAFETY: the native accessor has no preconditions.
    let co = unsafe { (ops.current)() };
    let mut waiting = Listener { co, signo: -1 };
    let failed = |last_error| Signalled {
        signo: -1,
        last_error,
    };
    // SAFETY: as above.
    if unsafe { (ops.busy)() } {
        return failed(libc::EBUSY);
    }
    let signals = if count == 0 {
        &[][..]
    } else {
        // SAFETY: the caller passes `count` numbers at `signals`.
        unsafe { std::slice::from_raw_parts(signals, count) }
    };
    if signals
        .iter()
        .any(|&signo| signo == libc::SIGCHLD || listener(signo).is_none())
    {
        return failed(libc::EINVAL);
    }
    RESUME.store(ops.resume as usize, Relaxed);
    // Lives on the parked coroutine's stack, where deferred registrations record into it.
    let mut previous = [Previous::default(); SIGNO_MAX as usize];
    let records = previous.as_mut_ptr();
    for &signo in signals {
        if let Some((index, slot)) = slot(signo) {
            slot.store(&raw mut waiting, Relaxed);
            // SAFETY: as above; the record, inside the table-long `previous`, outlives
            // the wait.
            unsafe { (ops.listen)(signo, records.wrapping_add(index), true) };
        }
    }
    // SAFETY: as above.
    unsafe { (ops.waiting)(true) };
    // SAFETY: `co` is the current coroutine; the handler writes `waiting` while
    // this frame is parked here.
    let woken = unsafe { (ops.yield_for)(co, timeout) };
    for &signo in signals {
        let Some((index, slot)) = slot(signo) else {
            continue;
        };
        let Some(record) = previous.get_mut(index).filter(|record| record.registered) else {
            continue;
        };
        record.registered = false;
        // SAFETY: as above.
        unsafe { (ops.listen)(signo, record, false) };
        slot.store(std::ptr::null_mut(), Relaxed);
    }
    // SAFETY: as above.
    unsafe { (ops.waiting)(false) };
    Signalled {
        signo: if woken { waiting.signo } else { -1 },
        last_error: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    fn log(entry: String) {
        LOG.with_borrow_mut(|log| log.push(entry));
    }
    unsafe extern "C" fn current() -> *mut c_void {
        std::ptr::dangling_mut()
    }
    unsafe extern "C" fn idle() -> bool {
        false
    }
    unsafe extern "C" fn busy() -> bool {
        true
    }
    unsafe extern "C" fn listen(signo: i32, previous: *mut Previous, on: bool) {
        let previous = unsafe { &mut *previous };
        if !on {
            log(format!("restore {signo} {}", previous.handler));
        } else if !previous.registered {
            previous.handler = 7;
            previous.registered = true;
            log(format!("listen {signo}"));
        }
    }
    unsafe extern "C" fn waiting(on: bool) {
        log(format!("waiting {on}"));
    }
    /// Stands in for a park during which SIGUSR2 arrives.
    unsafe extern "C" fn signalled(_: *mut c_void, _: f64) -> bool {
        swr_v1_hook_signal_wake(libc::SIGUSR2);
        swr_v1_hook_signal_wake(libc::SIGUSR2);
        true
    }
    unsafe extern "C" fn timed_out(_: *mut c_void, _: f64) -> bool {
        false
    }
    unsafe extern "C" fn resume(_: *mut c_void) {
        log("resume".into());
    }

    fn ops(
        busy_check: unsafe extern "C" fn() -> bool,
        park: unsafe extern "C" fn(*mut c_void, f64) -> bool,
    ) -> SignalOps {
        SignalOps {
            current,
            busy: busy_check,
            listen,
            waiting,
            yield_for: park,
            resume,
        }
    }

    fn wait(signals: &[i32], ops: &SignalOps) -> (Signalled, Vec<String>) {
        LOG.with_borrow_mut(Vec::clear);
        let result = unsafe { swr_v1_hook_wait_signal(signals.as_ptr(), signals.len(), 1.0, ops) };
        (result, LOG.with_borrow_mut(std::mem::take))
    }

    #[test]
    fn wakes_once_and_unregisters() {
        let (result, log) = wait(
            &[libc::SIGUSR1, libc::SIGUSR2, libc::SIGUSR1],
            &ops(idle, signalled),
        );
        assert_eq!(
            result,
            Signalled {
                signo: libc::SIGUSR2,
                last_error: 0
            }
        );
        let (one, two) = (libc::SIGUSR1, libc::SIGUSR2);
        let expected = [
            format!("listen {one}"),
            format!("listen {two}"),
            "waiting true".into(),
            "resume".into(),
            format!("restore {one} 7"),
            format!("restore {two} 7"),
            "waiting false".into(),
        ];
        assert_eq!(log, expected);
        assert!(LISTENERS.iter().all(|slot| slot.load(Relaxed).is_null()));

        let (result, _) = wait(&[libc::SIGUSR1], &ops(idle, timed_out));
        assert_eq!(result.signo, -1);
        assert_eq!(
            wait(&[libc::SIGUSR1], &ops(busy, timed_out)).0.last_error,
            libc::EBUSY
        );
        assert_eq!(
            wait(&[libc::SIGCHLD], &ops(idle, timed_out)).0.last_error,
            libc::EINVAL
        );
        assert_eq!(
            wait(&[SIGNO_MAX], &ops(idle, timed_out)).0.last_error,
            libc::EINVAL
        );
        // An invalid signal anywhere in the list registers none of them.
        let (result, log) = wait(&[libc::SIGUSR1, libc::SIGCHLD], &ops(idle, timed_out));
        assert_eq!(result.last_error, libc::EINVAL);
        assert_eq!(log, Vec::<String>::new());
    }
}
