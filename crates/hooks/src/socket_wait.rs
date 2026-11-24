//! `Socket::wait_event` of the coroutine socket and its readable and writable
//! reactor callbacks, over the Rust-owned state: which event to add (a TLS session
//! may want the other direction), binding the coroutine for the wait, the retry
//! that keeps an `_all` transfer waiting while it is short, and removing the event after.
//! `poll` waits the same way under a timer. Native code keeps the yield and the
//! write buffer ([`Host`](crate::co_io::Host)).
use crate::co_io::{CLOSE_WAIT_ERROR, Timer, available, ops, set_err};
use crate::co_socket::Co;
use core::ffi::c_void;
use swoole_rs_corebase::reactor::Events;

/// The state read before a wait, after consuming the open socket's backoff flag.
struct WaitState {
    events: Events,
    nobufs: bool,
    want_read: bool,
    want_write: bool,
    removed: bool,
}

fn slot(event: Events) -> usize {
    usize::from(event == Events::WRITE)
}

/// A direction's retrying wait ([`wait_retry`]): none, suspended, or woken by the
/// ready callback, which tells it whether the peer hung up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retry {
    None,
    Waiting,
    Ready { hup: bool },
}

/// One wait on a live record. Nothing here holds a borrow of it across a native call.
struct Wait(Co);

impl Wait {
    /// `close_wait`, else `has_kernel_nobufs()` (which clears it); the TLS
    /// session's wants, `removed` and `events`.
    fn snapshot(&self) -> Option<WaitState> {
        self.0
            .net(|record| {
                if record.close_deferred() {
                    return None;
                }
                let tls = !record.ssl().is_null();
                let wants = record.tls_wants();
                Some(WaitState {
                    events: record.events & (Events::READ | Events::WRITE),
                    nobufs: record.has_kernel_nobufs(),
                    want_read: tls && wants.contains(Events::READ),
                    want_write: tls && wants.contains(Events::WRITE),
                    removed: record.has_left_reactor(),
                })
            })
            .flatten()
    }

    fn bind(&self, event: Events, co: *mut c_void) {
        self.0.with_state(|state| state.bind(event, co));
    }

    fn want(&self) -> Events {
        self.0.with_state(|state| state.want.get())
    }

    fn set_want(&self, event: Events) {
        self.0.with_state(|state| state.want.set(event));
    }

    /// Sets the direction's retry, returning the one it replaces.
    fn set_retry(&self, event: Events, retry: Retry) -> Retry {
        self.0.with_state(|state| {
            state
                .retries
                .get(slot(event))
                .map_or(Retry::None, |slot| slot.replace(retry))
        })
    }

    /// `add_event`: add `event` to the reactor unless it is there already.
    fn add(&self, state: &WaitState, event: Events) -> bool {
        let events = if state.removed {
            event
        } else {
            state.events | event
        };
        let added =
            state.events.intersects(event) || (ops().reactor)(self.0, events, state.removed);
        set_err(self.0, if added { 0 } else { crate::co_io::errno() });
        added
    }

    fn wait(
        &self,
        co: *mut c_void,
        event: Events,
        buffer: Option<WriteSlot<'_>>,
        retry: Option<&mut dyn FnMut() -> bool>,
    ) -> bool {
        let Some(state) = self.snapshot() else {
            set_err(self.0, CLOSE_WAIT_ERROR);
            return false;
        };
        let event = if event == Events::READ {
            Events::READ
        } else {
            Events::WRITE
        };
        if state.nobufs {
            set_err(self.0, 0);
            self.bind(event, co);
            (ops().sleep)(self.0, event);
            self.bind(event, core::ptr::null_mut());
            return !self.0.is_closed() && self.0.err_code() == 0;
        }
        set_err(self.0, 0);
        let mut added = event;
        if event == Events::READ && state.want_write || event == Events::WRITE && state.want_read {
            if state.want_write && self.add(&state, Events::WRITE) {
                self.set_want(Events::WRITE);
            } else if state.want_read && self.add(&state, Events::READ) {
                self.set_want(Events::READ);
            } else {
                return false;
            }
            added = self.want();
        } else if !self.add(&state, event) {
            return false;
        }
        if event == Events::WRITE
            && let Some(mut slot) = buffer
        {
            (ops().buffer)(self.0, &mut slot);
        }
        self.bind(event, co);
        if retry.is_some() {
            self.set_retry(event, Retry::Waiting);
        }
        // Suspends this coroutine until a callback, timer or cancel resumes it.
        (ops().suspend)(self.0, co, event);
        if let Some(retry) = retry {
            // Woken by the ready callback, retry the transfer, and suspend again with
            // the event and timer kept while it would block or is short, unless the
            // peer hung up. A timer, cancel or error wake leaves it `Waiting`.
            while let Retry::Ready { hup } = self.set_retry(event, Retry::Waiting) {
                if !retry() || hup {
                    break;
                }
                (ops().suspend)(self.0, co, event);
            }
            self.set_retry(event, Retry::None);
        }
        self.bind(event, core::ptr::null_mut());
        // Both coroutines may wait for the same event under TLS.
        let bound = !self
            .0
            .with_state(|state| state.bound_co(Events::READ | Events::WRITE))
            .is_null();
        if self.want().is_empty() || !bound {
            (ops().remove)(self.0, added);
        }
        self.set_want(Events::NONE);
        !self.0.is_closed() && self.0.err_code() == 0
    }
}

/// A write's buffer slot: the caller's bytes, and whether the write sends them from
/// the socket's write buffer instead, where a wait may move them.
pub(crate) struct WriteSlot<'a> {
    moved: &'a mut bool,
    written: &'a [u8],
}

impl<'a> WriteSlot<'a> {
    /// The write of `written`, sent from the write buffer once `*moved`.
    pub(crate) fn new(moved: &'a mut bool, written: &'a [u8]) -> Self {
        Self { moved, written }
    }

    /// Whether the bytes are in the write buffer already.
    pub(crate) fn moved(&self) -> bool {
        *self.moved
    }

    /// The caller's bytes, which a move copies.
    pub(crate) fn written(&self) -> &[u8] {
        self.written
    }

    pub(crate) fn length(&self) -> usize {
        self.written.len()
    }

    /// The bytes copied to the write buffer, all `length` of them.
    pub(crate) fn moved_to_write_buffer(&mut self) {
        *self.moved = true;
    }
}

/// `Socket::wait_event(event)` for the current coroutine. `buffer` is a write's
/// buffer slot, which may move to the socket's write buffer. True when woken with
/// the socket open and no error.
pub(crate) fn wait(socket: Co, event: Events, buffer: Option<WriteSlot<'_>>) -> bool {
    let co = (ops().current)();
    !co.is_null() && Wait(socket).wait(co, event, buffer, None)
}

/// [`wait_for`] for `recv_all`/`send_all` and the vector forms: each time the ready
/// callback wakes it, `retry` transfers the rest, and while it returns true (would
/// block, or still short) the coroutine suspends again, as upstream's barrier holds
/// it from the callback.
pub(crate) fn wait_retry(socket: Co, event: Events, retry: &mut dyn FnMut() -> bool) -> bool {
    let co = (ops().current)();
    !co.is_null() && Wait(socket).wait(co, event, None, Some(retry))
}

/// [`wait`] with no retry and no buffer.
pub(crate) fn wait_for(socket: Co, event: Events) -> bool {
    let co = (ops().current)();
    !co.is_null() && Wait(socket).wait(co, event, None, None)
}

/// The readable (`event` read) or writable callback: the direction whose bound
/// coroutine to resume, or none. Under a TLS wait the other direction's coroutine
/// wakes; otherwise the direction's own, and a retrying wait learns that the ready
/// callback (not a timer, cancel or error) woke it, and whether the peer hung up.
pub(crate) fn ready(socket: Co, event: Events, hup: bool) -> Option<Events> {
    let other = if event == Events::READ {
        Events::WRITE
    } else {
        Events::READ
    };
    socket.with_state(|state| {
        let want = state.want.get();
        if !want.is_empty() {
            return (want == event).then_some(other);
        }
        if let Some(retry) = state.retries.get(slot(event))
            && retry.get() == Retry::Waiting
        {
            retry.set(Retry::Ready { hup });
        }
        Some(event)
    })
}

/// `Socket::poll(event, timeout)`: wait for `event` under `timeout`, or the
/// direction's own timeout when it is zero.
pub(crate) fn poll(socket: Co, event: Events, timeout: f64) -> bool {
    if !available(socket, event) {
        return false;
    }
    let timeout = if timeout == 0.0 {
        socket.timeout(crate::transfer::timeout_kind(event))
    } else {
        timeout
    };
    let mut timer = Timer::new(socket, event, timeout);
    let woke = timer.start() && wait_for(socket, event);
    timer.stop();
    woke
}

/// The reactor's error callback: wake the writer, then the reader unless the
/// writer closed the socket (`event_socket` no longer names `owner`, the native
/// object over `socket`, or left the reactor).
///
/// A resumed coroutine may free the record but not `event_socket`.
pub(crate) fn error_event(socket: Co, event_socket: swoole_rs_corebase::reactor::Socket<'_>) {
    for event in [Events::WRITE, Events::READ] {
        let (owner, removed) =
            event_socket.read(|record| (record.owner::<Co>(), record.has_left_reactor()));
        if event == Events::READ && (owner != Some(socket) || removed) {
            return;
        }
        // The record is live: it still owns `event_socket`.
        if socket.with_state(|state| !state.co(event).is_null()) {
            set_err(socket, 0);
            crate::co_io::resume_bound(socket, event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::test_host::{ON_SUSPEND, REACTOR_OK, install, log, take};
    use crate::co_socket::TestSocket;

    /// `status` bits; the socket's `events` arrive in the `READ` and `WRITE` bits.
    const CLOSE_WAIT: u32 = 1;
    const NOBUFS: u32 = 1 << 1;
    const WANT_READ: u32 = 1 << 2;
    const WANT_WRITE: u32 = 1 << 3;
    const REMOVED: u32 = 1 << 4;

    fn socket(status: u32) -> TestSocket {
        install();
        let socket = TestSocket::new();
        let record = unsafe { socket.0.record() };
        if status & CLOSE_WAIT != 0 {
            record.defer_close();
        }
        record.kernel_nobufs = status & NOBUFS != 0;
        if status & (WANT_READ | WANT_WRITE) != 0 {
            record.set_ssl(Some(crate::co_socket::stub_ssl()));
        }
        record.tls.want_read = status & WANT_READ != 0;
        record.tls.want_write = status & WANT_WRITE != 0;
        record.removed = status & REMOVED != 0;
        record.events = Events::from_bits(status.cast_signed()) & Events::RDWR;
        socket
    }

    /// The suspended coroutine records whether the ready callback picks it.
    fn wake_on_ready() {
        ON_SUSPEND.set(Some(Box::new(|socket: Co, event| {
            let bound = socket.with_state(|state| state.co(event));
            let woken = woken(socket, event);
            log(format!("woken:{}", !woken.is_null() && woken == bound));
        })));
    }

    /// The coroutine the ready callback for `event` resumes, or null.
    fn woken(socket: Co, event: Events) -> *mut c_void {
        ready(socket, event, false).map_or(core::ptr::null_mut(), |event| {
            socket.with_state(|state| state.co(event))
        })
    }

    fn wait_on(
        socket: &mut TestSocket,
        event: Events,
        retry: Option<&mut dyn FnMut() -> bool>,
    ) -> bool {
        if let Some(retry) = retry {
            return wait_retry(socket.co(), event, retry);
        }
        let mut moved = false;
        // The test host's `buffer` only logs the length.
        wait(
            socket.co(),
            event,
            Some(WriteSlot::new(&mut moved, &[0; 4])),
        )
    }

    fn effects() -> Vec<String> {
        take()
            .into_iter()
            .filter(|e| !e.starts_with("last-error"))
            .collect()
    }

    #[test]
    fn a_plain_wait_adds_binds_yields_and_removes() {
        let mut s = socket(Events::READ.bits().cast_unsigned());
        wake_on_ready();
        assert!(wait_on(&mut s, Events::WRITE, None));
        assert_eq!(
            effects(),
            [
                "set:1536",
                "buffer:4",
                "yield:1024",
                "woken:true",
                "remove:1024"
            ]
        );
        let mut s = socket(REMOVED);
        wake_on_ready();
        assert!(wait_on(&mut s, Events::READ, None));
        assert_eq!(
            effects(),
            ["add:512", "yield:512", "woken:true", "remove:512"]
        );
    }

    #[test]
    fn close_wait_nobufs_and_reactor_failures_stop_early() {
        let mut s = socket(CLOSE_WAIT | NOBUFS);
        assert!(!wait_on(&mut s, Events::READ, None));
        assert_eq!(s.0.err_code.get(), CLOSE_WAIT_ERROR);
        assert_eq!(effects(), Vec::<String>::new());
        // close_wait leaves the no-buffers flag for the next wait.
        assert!(unsafe { s.0.record() }.kernel_nobufs);
        let mut s = socket(NOBUFS);
        assert!(wait_on(&mut s, Events::WRITE, None));
        assert_eq!(effects(), ["sleep"]);
        assert!(!unsafe { s.0.record() }.kernel_nobufs);
        let mut s = socket(0);
        REACTOR_OK.set(false);
        assert!(!wait_on(&mut s, Events::READ, None));
        assert_eq!(effects(), ["set:512"]);
    }

    #[test]
    fn cancelling_the_nobufs_backoff_stops_the_transfer() {
        let mut s = socket(NOBUFS);
        ON_SUSPEND.set(Some(Box::new(|socket, event| {
            assert!(socket.cancel(event));
        })));
        assert!(!wait_on(&mut s, Events::WRITE, None));
        assert_eq!(s.0.err_code.get(), libc::ECANCELED);
        assert!(
            s.co()
                .with_state(|state| state.bound_co(Events::RDWR))
                .is_null()
        );
    }

    #[test]
    fn tls_waits_for_the_direction_it_wants() {
        let mut s = socket(WANT_WRITE);
        wake_on_ready();
        assert!(wait_on(&mut s, Events::READ, None));
        // The writable callback wakes the reader; no one else is bound, so remove.
        assert_eq!(
            effects(),
            ["set:1024", "yield:512", "woken:false", "remove:1024"]
        );
        assert_eq!(s.with_state(|state| state.want.get()), Events::NONE);
    }

    #[test]
    fn wants_count_only_with_a_tls_session() {
        let mut s = socket(WANT_WRITE);
        unsafe { s.0.record() }.set_ssl(None);
        assert!(wait_on(&mut s, Events::READ, None));
        assert_eq!(effects(), ["set:512", "yield:512", "remove:512"]);
    }

    // Upstream's barrier holds the coroutine from the ready callback while the retry
    // keeps waiting; here the woken coroutine retries and suspends again, with the
    // event and timer kept. A hang-up wakes it after one retry, and a timer, cancel or
    // error wake runs none.
    #[test]
    fn a_retry_that_keeps_waiting_suspends_again() {
        for (wake, keeps, want, woke) in [
            (
                Some(false),
                2,
                &[
                    "yield:512",
                    "retry",
                    "yield:512",
                    "retry",
                    "yield:512",
                    "retry",
                ][..],
                true,
            ),
            (Some(true), 5, &["yield:512", "retry"][..], true),
            (None, 5, &["yield:512"][..], false),
        ] {
            let mut s = socket(0);
            ON_SUSPEND.set(Some(Box::new(move |socket: Co, event| match wake {
                Some(hup) => assert_eq!(ready(socket, event, hup), Some(event)),
                None => socket.set_err_code(libc::ETIMEDOUT),
            })));
            let mut left = keeps;
            let mut retry = || {
                log("retry");
                left -= 1;
                left >= 0
            };
            assert_eq!(wait_on(&mut s, Events::READ, Some(&mut retry)), woke);
            let mut expected = vec!["set:512"];
            expected.extend(want);
            expected.push("remove:512");
            assert_eq!(effects(), expected, "{wake:?}");
            assert_eq!(s.with_state(|state| state.retries[0].get()), Retry::None);
            s.0.err_code.set(0);
        }
    }

    #[test]
    fn a_wake_with_an_error_or_close_fails() {
        let mut s = socket(0);
        ON_SUSPEND.set(Some(Box::new(|socket: Co, _| socket.move_fd())));
        assert!(!wait_on(&mut s, Events::READ, None));
        s.0.set_test_fd(100);
    }

    // Upstream's poll: is_available, the direction's timeout when none is given,
    // then `TimerController timer(...); timer.start() && wait_event(type)`.
    #[test]
    fn poll_waits_under_the_given_or_the_directions_timeout() {
        let mut s = socket(0);
        unsafe { s.0.record() }.timeouts.read = 3.0;
        unsafe { s.0.record() }.timeouts.write = 4.0;
        for (event, timeout, expected) in [
            (Events::READ, 0.0, 3.0),
            (Events::WRITE, 0.0, 4.0),
            (Events::READ, 0.5, 0.5),
        ] {
            take();
            assert!(poll(s.co(), event, timeout));
            let add = if event == Events::READ {
                "set:512"
            } else {
                "set:1024"
            };
            let mut want = vec![format!("timer:add:{expected}"), add.into()];
            want.extend([
                format!("yield:{event}"),
                format!("remove:{event}"),
                "timer:delete".into(),
            ]);
            assert_eq!(effects(), want);
        }
        // A failed timer skips the wait; a socket held by another coroutine is unavailable.
        crate::co_io::test_host::TIMER_OK.set(false);
        assert!(!poll(s.co(), Events::READ, 1.0));
        assert_eq!(effects(), ["timer:add:1"]);
        s.0.set_test_fd(-1);
        assert!(!poll(s.co(), Events::READ, 1.0));
        assert_eq!(effects(), Vec::<String>::new());
        s.0.set_test_fd(100);
    }

    // Upstream's error_event_callback: resume the writer, then the reader while
    // the event's socket still names this one and is in the reactor.
    #[test]
    fn the_error_event_wakes_the_writer_then_the_reader_of_a_live_socket() {
        use crate::co_io::test_host::ON_RESUME;
        let (mut reader, mut writer): (core::ffi::c_long, core::ffi::c_long) = (7, 8);
        for (object_matches, removed, close_in_writer) in [
            (true, false, false),
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let mut s = socket(0);
            let owner = s.co();
            let record = unsafe { s.0.record() };
            record.set_owner(object_matches.then_some(owner));
            record.removed = removed;
            let event_socket = s.0.socket_ptr();
            s.with_state(|state| state.bind(Events::READ, (&raw mut reader).cast()));
            s.with_state(|state| state.bind(Events::WRITE, (&raw mut writer).cast()));
            ON_RESUME.set(Some(Box::new(move |cid| {
                if close_in_writer && cid == 8 {
                    unsafe { (*event_socket).fields.set_owner::<Co>(None) };
                }
            })));
            s.0.err_code.set(9);
            error_event(
                s.co(),
                unsafe { swoole_rs_corebase::reactor::Socket::from_ptr(event_socket) }.unwrap(),
            );
            let mut want = vec!["resume:8".to_string()];
            if object_matches && !removed && !close_in_writer {
                want.push("resume:7".into());
            }
            assert_eq!(effects(), want);
            assert_eq!(s.0.err_code.get(), 0);
            s.with_state(|state| state.bind(Events::READ, core::ptr::null_mut()));
            s.with_state(|state| state.bind(Events::WRITE, core::ptr::null_mut()));
        }
    }
}
