//! `System::wait_signal(signals, timeout)`: park the current coroutine until one of
//! `signals` arrives. The listener table, the wake-up handler and [`NATIVE`]'s signal
//! registration, reactor exit condition and coroutine operations are here; tests
//! pass their own `SignalOps`.
use std::ffi::{c_int, c_long, c_void};
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

/// `SW_SIGNO_MAX`.
const SIGNO_MAX: i32 = 128;

/// A coroutine waiting for a signal (upstream's `SignalListener`), on its stack.
struct Listener {
    co: *mut c_void,
    signo: i32,
    resume: fn(*mut c_void),
}

/// Which listener each signal wakes (upstream's `listeners`).
static LISTENERS: [AtomicPtr<Listener>; SIGNO_MAX as usize] =
    [const { AtomicPtr::new(std::ptr::null_mut()) }; SIGNO_MAX as usize];

/// The signal table, reactor and coroutine operations a signal wait needs. The wait
/// passes each only its own coroutine and the records on its own stack.
struct SignalOps {
    /// `Coroutine::get_current_safe()`.
    current: extern "C" fn() -> *mut c_void,
    /// Whether a non-coroutine signal listener exists (`signal_listener_num > 0`).
    busy: fn() -> bool,
    /// On the loop's thread under thread contexts: with `true`, unless the record says
    /// so already, records `signo`'s handler in it and installs `swr_v1_hook_signal_wake`;
    /// with `false`, puts the recorded handler back.
    listen: fn(i32, *mut Previous, bool),
    /// The reactor exit condition and `signal_async_listener_num++` (true), or `--`.
    waiting: fn(bool),
    /// `co->yield_ex(timeout)`: false on timeout or cancellation.
    yield_for: fn(*mut c_void, f64) -> bool,
    /// `co->resume()`.
    resume: fn(*mut c_void),
}

/// A signal's handler from before a wait installed its own (upstream's `PreviousSignal`).
#[derive(Clone, Copy, Default)]
struct Previous {
    /// The `SignalHandler`, or 0.
    handler: usize,
    /// Whether the wait's handler is installed.
    registered: bool,
}

/// What `wait_signal` returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Signalled {
    /// The signal that arrived, or -1.
    signo: i32,
    /// For native to set as the last error, or 0.
    last_error: i32,
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
pub extern "C" fn swr_v1_hook_signal_wake(signo: i32) {
    let Some(slot) = listener(signo) else {
        return;
    };
    let waiting = slot.swap(std::ptr::null_mut(), Relaxed);
    // SAFETY: a registered listener lives on its parked coroutine's stack.
    let Some(waiting) = (unsafe { waiting.as_mut() }) else {
        return;
    };
    waiting.signo = signo;
    (waiting.resume)(waiting.co);
}

/// `signo` as upstream's `std::vector<int>` holds it.
fn narrow(signo: c_long) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream copies the longs into a vector<int>"
    )]
    let signo = signo as i32;
    signo
}

/// `int System::wait_signal(const std::vector<int> &signals, double timeout)`: the
/// signal that arrived, or -1 with the last error set. Outside a coroutine it is
/// upstream's fatal error.
#[must_use]
pub fn system_wait_signal(signals: &[c_long], timeout: f64) -> c_int {
    let signalled = wait_signal(signals, timeout, &NATIVE);
    if signalled.last_error != 0 {
        swoole_rs_corebase::log::set_last_error(signalled.last_error);
    }
    signalled.signo
}

/// Waits for any of `signals` for up to `timeout` seconds.
fn wait_signal(signals: &[c_long], timeout: f64, ops: &SignalOps) -> Signalled {
    let co = (ops.current)();
    let mut waiting = Listener {
        co,
        signo: -1,
        resume: ops.resume,
    };
    let failed = |last_error| Signalled {
        signo: -1,
        last_error,
    };
    if (ops.busy)() {
        return failed(libc::EBUSY);
    }
    let signals = signals.iter().map(|&signo| narrow(signo));
    if signals
        .clone()
        .any(|signo| signo == libc::SIGCHLD || listener(signo).is_none())
    {
        return failed(libc::EINVAL);
    }
    // Lives on the parked coroutine's stack, where deferred registrations record into it.
    let mut previous = [Previous::default(); SIGNO_MAX as usize];
    let records = previous.as_mut_ptr();
    for signo in signals.clone() {
        if let Some((index, slot)) = slot(signo) {
            slot.store(&raw mut waiting, Relaxed);
            // The record, inside the table-long `previous`, outlives the wait.
            (ops.listen)(signo, records.wrapping_add(index), true);
        }
    }
    (ops.waiting)(true);
    // The handler writes `waiting` while this frame is parked here.
    let woken = (ops.yield_for)(co, timeout);
    for signo in signals {
        let Some((index, slot)) = slot(signo) else {
            continue;
        };
        let Some(record) = previous.get_mut(index).filter(|record| record.registered) else {
            continue;
        };
        record.registered = false;
        (ops.listen)(signo, record, false);
        slot.store(std::ptr::null_mut(), Relaxed);
    }
    (ops.waiting)(false);
    Signalled {
        signo: if woken { waiting.signo } else { -1 },
        last_error: 0,
    }
}

/// System's signal operations: registration through the signal table (on the loop's
/// thread under thread contexts), the reactor's exit condition and the coroutine.
#[cfg(not(test))]
static NATIVE: SignalOps = SignalOps {
    current: crate::wait_event::native::current,
    busy: native::busy,
    listen: native::listen,
    waiting: native::waiting,
    yield_for: native::yield_for,
    resume: crate::poll::native::resume,
};

/// The unit tests link no signal table: theirs is the fake.
#[cfg(test)]
static NATIVE: SignalOps = tests::OPS;

#[cfg(not(test))]
mod native {
    use super::Previous;
    use core::ffi::{c_int, c_void};
    use swoole_rs_corebase::{
        swr_v1_core_set_signal_async_listener_num, swr_v1_core_signal_async_listener_num,
        swr_v1_core_signal_listener_num,
    };

    /// `Reactor::EXIT_CONDITION_CO_SIGNAL_LISTENER`.
    const EXIT_CONDITION_CO_SIGNAL_LISTENER: c_int = 3;

    // SAFETY: native/reactor/reactor.cc's and crates/reactor's.
    unsafe extern "C" {
        #[link_name = "_Z17swoole_signal_setiPFviEb"]
        fn swoole_signal_set(signo: c_int, handler: usize, safety: bool) -> usize;
        #[link_name = "_Z25swoole_signal_get_handleri"]
        safe fn swoole_signal_get_handler(signo: c_int) -> usize;
    }

    /// A non-coroutine signal listener exists.
    pub(super) fn busy() -> bool {
        swr_v1_core_signal_listener_num() > 0
    }

    pub(super) fn yield_for(co: *mut c_void, timeout: f64) -> bool {
        // SAFETY: the current coroutine, parking for a wait.
        unsafe { crate::wait_event::native::coroutine_yield_ex(co, timeout) }
    }

    /// A signal change: register `previous` (non-null), or put `handler` back.
    struct Change {
        signo: c_int,
        handler: usize,
        previous: *mut Previous,
    }

    impl Change {
        /// Records the handler when the registration runs, so a deferred one sees those
        /// pending.
        fn apply(&self) {
            // SAFETY: a register's record lives on the parked waiter's stack until the
            // wait unregisters it.
            let handler = match unsafe { self.previous.as_mut() } {
                Some(previous) if previous.registered => return,
                Some(previous) => {
                    previous.handler = swoole_signal_get_handler(self.signo);
                    previous.registered = true;
                    super::swr_v1_hook_signal_wake as extern "C" fn(i32) as usize
                }
                None => self.handler,
            };
            // SAFETY: the wake handler, or the one recorded from the table.
            let _previous = unsafe { swoole_signal_set(self.signo, handler, false) };
        }
    }

    unsafe extern "C-unwind" fn run(change: *mut c_void) {
        // SAFETY: the box `listen` deferred, run once.
        unsafe { Box::from_raw(change.cast::<Change>()) }.apply();
    }

    unsafe extern "C-unwind" fn dispose(change: *mut c_void) {
        // SAFETY: the box `listen` deferred, never run.
        drop(unsafe { Box::from_raw(change.cast::<Change>()) });
    }

    pub(super) fn listen(signo: c_int, previous: *mut Previous, on: bool) {
        let change = Change {
            signo,
            // SAFETY: the waiter's record for `signo`.
            handler: unsafe { (*previous).handler },
            previous: if on { previous } else { core::ptr::null_mut() },
        };
        if !swoole_rs_coroutine::thread_context_active() {
            return change.apply();
        }
        // Coroutine threads block every signal: change it on the loop's thread.
        let change = Box::into_raw(Box::new(change)).cast();
        // SAFETY: a waiter runs on the loop; `run` or `dispose` takes the box once.
        unsafe { crate::reactor_handlers::defer(run, change, Some(dispose)) };
    }

    extern "C-unwind" fn listeners_done(_: &mut usize) -> bool {
        swr_v1_core_signal_async_listener_num() == 0
    }

    pub(super) fn waiting(on: bool) {
        let count = swr_v1_core_signal_async_listener_num();
        if !on {
            swr_v1_core_set_signal_async_listener_num(count.wrapping_sub(1));
            return;
        }
        swoole_rs_reactor::swr_v1_event_set_exit_condition(
            EXIT_CONDITION_CO_SIGNAL_LISTENER,
            listeners_done,
        );
        swr_v1_core_set_signal_async_listener_num(count + 1);
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
    extern "C" fn current() -> *mut c_void {
        std::ptr::dangling_mut()
    }
    fn idle() -> bool {
        false
    }
    fn busy() -> bool {
        true
    }
    fn listen(signo: i32, previous: *mut Previous, on: bool) {
        let previous = unsafe { &mut *previous };
        if !on {
            log(format!("restore {signo} {}", previous.handler));
        } else if !previous.registered {
            previous.handler = 7;
            previous.registered = true;
            log(format!("listen {signo}"));
        }
    }
    fn waiting(on: bool) {
        log(format!("waiting {on}"));
    }
    /// Stands in for a park during which SIGUSR2 arrives.
    fn signalled(_: *mut c_void, _: f64) -> bool {
        swr_v1_hook_signal_wake(libc::SIGUSR2);
        swr_v1_hook_signal_wake(libc::SIGUSR2);
        true
    }
    fn timed_out(_: *mut c_void, _: f64) -> bool {
        false
    }
    fn resume(_: *mut c_void) {
        log("resume".into());
    }

    pub(super) const OPS: SignalOps = ops(idle, timed_out);

    const fn ops(busy_check: fn() -> bool, park: fn(*mut c_void, f64) -> bool) -> SignalOps {
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
        let signals: Vec<c_long> = signals.iter().map(|&signo| signo.into()).collect();
        let result = wait_signal(&signals, 1.0, ops);
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
