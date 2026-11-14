//! `System::wait_signal(signals, timeout)`: park the current coroutine until one of
//! `signals` arrives. The listener table, the wake-up handler and [`NATIVE`]'s signal
//! registration, reactor exit condition and coroutine operations are here; tests
//! pass their own `SignalOps`.
use std::cell::Cell;
use std::ffi::{c_int, c_long, c_void};
use std::rc::Rc;
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};
use swoole_rs_corebase::CCast;
use swoole_rs_reactor::signal_upstream::Handler;

/// `SW_SIGNO_MAX`.
const SIGNO_MAX: i32 = 128;
/// `SW_ERROR_WRONG_OPERATION`: no event loop to register the handler on.
const WRONG_OPERATION: i32 = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as i32;

/// A coroutine waiting for a signal (upstream's `SignalListener`).
struct Listener {
    co: *mut c_void,
    signo: Cell<i32>,
    resume: unsafe extern "C-unwind" fn(*mut c_void),
}

/// Which listener each signal wakes (upstream's `listeners`). A non-null slot owns
/// one `Rc<Listener>` reference, put there by [`Published::new`].
static LISTENERS: [AtomicPtr<Listener>; SIGNO_MAX as usize] =
    [const { AtomicPtr::new(std::ptr::null_mut()) }; SIGNO_MAX as usize];

/// The signal table, reactor and coroutine operations a signal wait needs. The wait
/// passes each only its own coroutine and records.
struct SignalOps {
    /// `Coroutine::get_current_safe()`.
    current: extern "C" fn() -> *mut c_void,
    /// Whether a non-coroutine signal listener exists (`signal_listener_num > 0`).
    busy: fn() -> bool,
    /// On the loop's thread under thread contexts: with `true`, unless `signo`'s record
    /// says so already, records its handler there and installs `swr_v1_hook_signal_wake`
    /// (nothing once the wait has dropped the records); with `false`, puts the recorded
    /// handler back. False if the change can't reach the loop's thread (no reactor).
    listen: fn(i32, &Rc<Records>, bool) -> bool,
    /// The reactor exit condition and `signal_async_listener_num++` (true), or `--`.
    waiting: fn(bool),
    /// `co->yield_ex(timeout)`: false on timeout or cancellation.
    yield_for: fn(f64) -> bool,
    /// `co->resume()` of a parked coroutine.
    resume: unsafe extern "C-unwind" fn(*mut c_void),
}

/// A signal's handler from before a wait installed its own (upstream's `PreviousSignal`).
#[derive(Clone, Copy)]
struct Previous {
    /// The `SignalHandler`, or none.
    handler: Handler,
    /// Whether the wait's handler is installed.
    registered: bool,
}

/// Each signal's [`Previous`], owned by one wait.
type Records = [Cell<Previous>; SIGNO_MAX as usize];

impl Previous {
    const NONE: Self = Self {
        handler: Handler::NONE,
        registered: false,
    };
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

/// Takes the reference a slot owned.
fn take(slot: &AtomicPtr<Listener>, expected: Option<*const Listener>) -> Option<Rc<Listener>> {
    let null = std::ptr::null_mut();
    let taken = match expected {
        Some(ours) => slot
            .compare_exchange(ours.cast_mut(), null, Relaxed, Relaxed)
            .ok()?,
        None => slot.swap(null, Relaxed),
    };
    // SAFETY: a non-null slot holds a reference from `Rc::into_raw`, which it gives up here.
    (!taken.is_null()).then(|| unsafe { Rc::from_raw(taken) })
}

/// A wait's listener published in one signal's slot. Dropping it unpublishes the
/// listener unless a signal or a later wait took the slot first, so the slot never
/// outlives the wait.
struct Published {
    slot: &'static AtomicPtr<Listener>,
    listener: *const Listener,
}

impl Published {
    fn new(slot: &'static AtomicPtr<Listener>, listener: &Rc<Listener>) -> Self {
        // A later wait displaces an earlier one, as upstream's overwrite does.
        drop(take(slot, None));
        let listener = Rc::into_raw(Rc::clone(listener));
        slot.store(listener.cast_mut(), Relaxed);
        Self { slot, listener }
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        drop(take(self.slot, Some(self.listener)));
    }
}

/// `signo`'s record.
fn record(records: &Records, signo: i32) -> Option<&Cell<Previous>> {
    records.get(usize::try_from(signo).ok()?)
}

/// The signal handler: wakes the coroutine listening for `signo`, once.
pub extern "C" fn swr_v1_hook_signal_wake(signo: i32) {
    let Some(slot) = listener(signo) else {
        return;
    };
    let Some(waiting) = take(slot, None) else {
        return;
    };
    waiting.signo.set(signo);
    // SAFETY: the listener's coroutine, parked in `wait_signal`, which unpublishes
    // every slot before it returns.
    unsafe { (waiting.resume)(waiting.co) };
}

/// `signo` as upstream's `std::vector<int>` holds it.
fn narrow(signo: c_long) -> i32 {
    signo.c_cast::<i32>()
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
    let waiting = Rc::new(Listener {
        co,
        signo: Cell::new(-1),
        resume: ops.resume,
    });
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
    // Deferred registrations reach these only while the wait holds them.
    let previous: Rc<Records> = Rc::new([const { Cell::new(Previous::NONE) }; SIGNO_MAX as usize]);
    let mut published = Vec::new();
    let mut listening = true;
    for signo in signals.clone() {
        if let Some(slot) = listener(signo) {
            published.push(Published::new(slot, &waiting));
            listening = (ops.listen)(signo, &previous, true);
            if !listening {
                break;
            }
        }
    }
    if listening {
        (ops.waiting)(true);
    }
    let woken = listening && (ops.yield_for)(timeout);
    // However the wait ended, no signal wakes it any more.
    drop(published);
    for signo in signals {
        let Some(record) = record(&previous, signo).filter(|record| record.get().registered) else {
            continue;
        };
        record.set(Previous {
            registered: false,
            ..record.get()
        });
        (ops.listen)(signo, &previous, false);
    }
    if !listening {
        return failed(WRONG_OPERATION);
    }
    (ops.waiting)(false);
    Signalled {
        signo: if woken { waiting.signo.get() } else { -1 },
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
    yield_for: crate::resolve::park_for,
    resume: swoole_rs_coroutine::resume,
};

/// The unit tests link no signal table: theirs is the fake.
#[cfg(test)]
static NATIVE: SignalOps = tests::OPS;

#[cfg(not(test))]
mod native {
    use super::{Previous, Records};
    use core::ffi::c_int;
    use std::rc::{Rc, Weak};
    use swoole_rs_corebase::{
        swr_v1_core_set_signal_async_listener_num, swr_v1_core_signal_async_listener_num,
        swr_v1_core_signal_listener_num,
    };

    use swoole_rs_reactor::event_api::{Deferred, swoole_event_defer_value};
    use swoole_rs_reactor::signal_upstream::{self, Handler};

    /// A non-coroutine signal listener exists.
    pub(super) fn busy() -> bool {
        swr_v1_core_signal_listener_num() > 0
    }

    /// A signal change: register into `records`, or put `handler` back.
    struct Change {
        signo: c_int,
        handler: Handler,
        records: Option<Weak<Records>>,
    }

    impl Change {
        /// Records the handler when the registration runs, so a deferred one sees those
        /// pending. A registration that runs after its wait has ended does nothing.
        fn apply(&self) {
            let handler = match &self.records {
                None => self.handler,
                Some(records) => {
                    let Some(records) = records.upgrade() else {
                        return;
                    };
                    let Some(previous) = super::record(&records, self.signo)
                        .filter(|previous| !previous.get().registered)
                    else {
                        return;
                    };
                    previous.set(Previous {
                        handler: signal_upstream::get(self.signo),
                        registered: true,
                    });
                    Handler::new(super::swr_v1_hook_signal_wake)
                }
            };
            _ = signal_upstream::set(self.signo, handler, false);
        }
    }

    impl Deferred for Change {
        fn run(self) {
            self.apply();
        }

        fn dispose(self) {}
    }

    pub(super) fn listen(signo: c_int, records: &Rc<Records>, on: bool) -> bool {
        let change = Change {
            signo,
            handler: super::record(records, signo).map_or(Handler::NONE, |r| r.get().handler),
            records: on.then(|| Rc::downgrade(records)),
        };
        if !swoole_rs_coroutine::thread_context_active() {
            change.apply();
            return true;
        }
        // Coroutine threads block every signal: change it on the loop's thread.
        swoole_event_defer_value(change).is_ok()
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
            swoole_rs_reactor::ExitCondition::CoSignalListener,
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
        /// Registrations a deferring `listen` queued for the loop.
        static QUEUED: RefCell<Vec<(i32, std::rc::Weak<Records>)>> = const { RefCell::new(Vec::new()) };
    }
    /// The tests share the process-wide `LISTENERS`, so they run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    fn listen(signo: i32, records: &Rc<Records>, on: bool) -> bool {
        let Some(previous) = record(records, signo) else {
            return true;
        };
        if !on {
            log(format!("restore {signo} {}", previous.get().handler.raw()));
        } else if !previous.get().registered {
            previous.set(Previous {
                handler: unsafe { Handler::from_raw(7) },
                registered: true,
            });
            log(format!("listen {signo}"));
        }
        true
    }
    /// No loop to defer the registration to.
    fn unreachable_loop(signo: i32, _: &Rc<Records>, _: bool) -> bool {
        log(format!("refused {signo}"));
        false
    }
    /// Queues the registration for the loop, as under thread contexts.
    fn deferred(signo: i32, records: &Rc<Records>, on: bool) -> bool {
        if on {
            QUEUED.with_borrow_mut(|queued| queued.push((signo, Rc::downgrade(records))));
            log(format!("queued {signo}"));
            return true;
        }
        listen(signo, records, on)
    }
    fn waiting(on: bool) {
        log(format!("waiting {on}"));
    }
    /// Stands in for a park during which SIGUSR2 arrives.
    fn signalled(_: f64) -> bool {
        swr_v1_hook_signal_wake(libc::SIGUSR2);
        swr_v1_hook_signal_wake(libc::SIGUSR2);
        true
    }
    fn timed_out(_: f64) -> bool {
        false
    }
    extern "C-unwind" fn resume(_: *mut c_void) {
        log("resume".into());
    }

    pub(super) const OPS: SignalOps = ops(idle, timed_out);

    const fn ops(busy_check: fn() -> bool, park: fn(f64) -> bool) -> SignalOps {
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
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    #[test]
    fn a_registration_without_a_loop_fails_without_parking() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ops = SignalOps {
            listen: unreachable_loop,
            ..ops(idle, signalled)
        };
        let (result, log) = wait(&[libc::SIGUSR1, libc::SIGUSR2], &ops);
        assert_eq!(
            result,
            Signalled {
                signo: -1,
                last_error: WRONG_OPERATION
            }
        );
        assert_eq!(log, [format!("refused {}", libc::SIGUSR1)]);
        assert!(LISTENERS.iter().all(|slot| slot.load(Relaxed).is_null()));
    }

    #[test]
    fn a_wait_cancelled_before_its_deferred_registration_leaves_no_listener() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        QUEUED.with_borrow_mut(Vec::clear);
        let ops = SignalOps {
            listen: deferred,
            ..ops(idle, timed_out)
        };
        let (one, two) = (libc::SIGUSR1, libc::SIGUSR2);
        let (result, log) = wait(&[one, two], &ops);
        assert_eq!(result.signo, -1);
        let expected = [
            format!("queued {one}"),
            format!("queued {two}"),
            "waiting true".into(),
            "waiting false".into(),
        ];
        assert_eq!(log, expected);
        assert!(LISTENERS.iter().all(|slot| slot.load(Relaxed).is_null()));

        // The loop now runs the queued registrations, then both signals arrive.
        let queued = QUEUED.with_borrow_mut(std::mem::take);
        assert_eq!(queued.len(), 2);
        for (signo, records) in queued {
            if let Some(records) = records.upgrade() {
                listen(signo, &records, true);
            }
        }
        swr_v1_hook_signal_wake(one);
        swr_v1_hook_signal_wake(two);
        assert_eq!(LOG.with_borrow_mut(std::mem::take), Vec::<String>::new());
        assert!(LISTENERS.iter().all(|slot| slot.load(Relaxed).is_null()));
    }
}
