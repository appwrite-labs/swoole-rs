//! `Socket::wait_event` of the coroutine socket and its readable and writable
//! reactor callbacks, over the Rust-owned state: which event to add (a TLS session
//! may want the other direction), binding the coroutine for the wait, the barrier
//! that retries a transfer before the waiter wakes, and removing the event after.
//! `poll` waits the same way under a timer. Native code keeps the yield and the
//! write buffer ([`Host`](crate::co_io::Host)).
use crate::co_io::{CLOSE_WAIT_ERROR, NATIVE, Timer, available, set_err};
use crate::co_socket::Co;
use crate::shutdown::{READ, WRITE};
use crate::vector::Barrier;
use core::ffi::c_void;

/// `status` bits; the socket's `events` arrive in the `READ` and `WRITE` bits.
const CLOSE_WAIT: u32 = 1;
const NOBUFS: u32 = 1 << 1;
const WANT_READ: u32 = 1 << 2;
const WANT_WRITE: u32 = 1 << 3;
const REMOVED: u32 = 1 << 4;

fn slot(event: u32) -> usize {
    usize::from(event == WRITE)
}

/// One wait on a live record. Nothing here holds a borrow of it across a native call.
struct Wait(Co);

impl Wait {
    /// `close_wait`, else `has_kernel_nobufs()` (which clears it); the TLS
    /// session's wants, `removed` and `events`.
    fn status(&self) -> u32 {
        self.0
            .net(|record| {
                if record.output.close_wait != 0 {
                    return CLOSE_WAIT;
                }
                let tls = !record.ssl().is_null();
                let mut status = record.events.cast_unsigned() & (READ | WRITE);
                if core::mem::take(&mut record.kernel_nobufs) {
                    status |= NOBUFS;
                }
                if tls && record.tls.want_read != 0 {
                    status |= WANT_READ;
                }
                if tls && record.tls.want_write != 0 {
                    status |= WANT_WRITE;
                }
                if record.removed != 0 {
                    status |= REMOVED;
                }
                status
            })
            .unwrap_or(CLOSE_WAIT)
    }

    fn bind(&self, event: u32, co: *mut c_void) {
        self.0.with_state(|state| state.bind(event, co));
    }

    fn want(&self) -> u32 {
        self.0.with_state(|state| state.want)
    }

    fn set_want(&self, event: u32) {
        self.0.with_state(|state| state.want = event);
    }

    fn set_barrier(&self, event: u32, barrier: Option<Barrier>) {
        self.0.with_state(|state| {
            if let Some(slot) = state.barriers.get_mut(slot(event)) {
                *slot = barrier;
            }
        });
    }

    /// `add_event`: add `event` to the reactor unless it is there already.
    fn add(&self, status: u32, event: u32) -> bool {
        let removed = status & REMOVED != 0;
        let events = if removed {
            event
        } else {
            status & (READ | WRITE) | event
        };
        let added = status & event != 0 || (NATIVE.reactor)(self.0, events, removed);
        set_err(self.0, if added { 0 } else { crate::co_io::errno() });
        added
    }

    fn wait(&self, co: *mut c_void, event: u32, buffer: Option<WriteSlot<'_>>) -> bool {
        let status = self.status();
        if status & CLOSE_WAIT != 0 {
            set_err(self.0, CLOSE_WAIT_ERROR);
            return false;
        }
        let event = if event == READ { READ } else { WRITE };
        if status & NOBUFS != 0 {
            self.bind(event, co);
            (NATIVE.sleep)();
            self.bind(event, core::ptr::null_mut());
            return !self.0.is_closed();
        }
        set_err(self.0, 0);
        let mut added = event;
        if event == READ && status & WANT_WRITE != 0 || event == WRITE && status & WANT_READ != 0 {
            if status & WANT_WRITE != 0 && self.add(status, WRITE) {
                self.set_want(WRITE);
            } else if status & WANT_READ != 0 && self.add(status, READ) {
                self.set_want(READ);
            } else {
                return false;
            }
            added = self.want();
        } else if !self.add(status, event) {
            return false;
        }
        if event == WRITE
            && let Some(mut slot) = buffer
        {
            (NATIVE.buffer)(self.0, &mut slot);
        }
        self.bind(event, co);
        // Suspends this coroutine until a callback, timer or cancel resumes it.
        (NATIVE.suspend)(self.0, co, event);
        self.bind(event, core::ptr::null_mut());
        // Both coroutines may wait for the same event under TLS.
        let bound = !self
            .0
            .with_state(|state| state.bound_co(READ | WRITE))
            .is_null();
        if self.want() == 0 || !bound {
            (NATIVE.remove)(self.0, added);
        }
        self.set_want(0);
        !self.0.is_closed() && self.0.err_code() == 0
    }
}

/// A write's buffer slot: where its bytes are. A wait may move them to the socket's
/// write buffer and point the slot there.
pub(crate) struct WriteSlot<'a> {
    #[cfg_attr(test, expect(dead_code, reason = "the test host only logs the length"))]
    at: &'a mut *mut u8,
    length: usize,
}

impl<'a> WriteSlot<'a> {
    /// # Safety
    /// `*at` points to `length` readable bytes, which stay so while the slot lives.
    pub(crate) unsafe fn new(at: &'a mut *mut u8, length: usize) -> Self {
        Self { at, length }
    }

    /// Where the bytes are now.
    #[cfg(not(test))]
    pub(crate) fn bytes(&self) -> *mut u8 {
        *self.at
    }

    pub(crate) fn length(&self) -> usize {
        self.length
    }

    /// The bytes moved to `bytes`, still `length` of them and live as long.
    #[cfg(not(test))]
    pub(crate) fn moved_to(&mut self, bytes: *mut u8) {
        *self.at = bytes;
    }
}

/// `Socket::wait_event(event)` for the current coroutine. `buffer` is a write's
/// buffer slot, which may move to the socket's write buffer. True when woken with
/// the socket open and no error.
pub(crate) fn wait(socket: Co, event: u32, buffer: Option<WriteSlot<'_>>) -> bool {
    let co = (NATIVE.current)();
    !co.is_null() && Wait(socket).wait(co, event, buffer)
}

/// Clears the barrier [`wait_barrier`] installed, also when the wait unwinds.
struct Installed(Wait, u32);

impl Drop for Installed {
    fn drop(&mut self) {
        self.0.set_barrier(self.1, None);
    }
}

/// [`wait_for`] with `barrier` retrying the transfer while it is installed: the
/// ready callback calls it, and holds the coroutine while it returns true.
pub(crate) fn wait_barrier(socket: Co, event: u32, barrier: &mut dyn FnMut() -> bool) -> bool {
    let co = (NATIVE.current)();
    if co.is_null() {
        return false;
    }
    let barrier: *mut (dyn FnMut() -> bool + '_) = barrier;
    #[expect(
        clippy::transmute_ptr_to_ptr,
        reason = "a raw pointer cast cannot extend a trait object's lifetime"
    )]
    // SAFETY: only the lifetime changes. `Installed` uninstalls the barrier before
    // this returns, while the borrow is still live, and only `ready` calls it meanwhile.
    let barrier =
        unsafe { core::mem::transmute::<*mut (dyn FnMut() -> bool + '_), Barrier>(barrier) };
    let installed = Installed(Wait(socket), event);
    installed.0.set_barrier(event, Some(barrier));
    installed.0.wait(co, event, None)
}

/// [`wait`] with no barrier and no buffer.
pub(crate) fn wait_for(socket: Co, event: u32) -> bool {
    let co = (NATIVE.current)();
    !co.is_null() && Wait(socket).wait(co, event, None)
}

/// The readable (`event` read) or writable callback: the coroutine to resume, or
/// null. Under a TLS wait the other direction's coroutine wakes; otherwise an
/// installed barrier that keeps waiting holds the coroutine unless the peer hung up.
pub(crate) fn ready(socket: Co, event: u32, hup: bool) -> *mut c_void {
    let other = if event == READ { WRITE } else { READ };
    let (want, woken, barrier) = socket.with_state(|state| {
        let woken = if state.want == event {
            state.co(other)
        } else {
            core::ptr::null_mut()
        };
        (
            state.want,
            woken,
            state.barriers.get(slot(event)).copied().flatten(),
        )
    });
    if want != 0 {
        return woken;
    }
    if let Some(barrier) = barrier {
        // SAFETY: an installed barrier is the live, exclusive borrow its suspended
        // `wait_barrier` lent; it does not touch the state's barriers or bindings.
        if unsafe { (*barrier)() } && !hup {
            return core::ptr::null_mut();
        }
    }
    socket.with_state(|state| state.co(event))
}

/// `Socket::poll(event, timeout)`: wait for `event` under `timeout`, or the
/// direction's own timeout when it is zero.
pub(crate) fn poll(socket: Co, event: u32, timeout: f64) -> bool {
    if !available(socket, event) {
        return false;
    }
    let timeout = if timeout == 0.0 {
        socket.timeout(if event == READ { 4 } else { 8 })
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
pub(crate) fn error_event(socket: Co, event_socket: swoole_rs_reactor::Socket<'_>) {
    for event in [WRITE, READ] {
        let (object, removed) = event_socket.read(|record| (record.object, record.removed));
        if event == READ && (object != socket.this() || removed != 0) {
            return;
        }
        // The record is live: it still owns `event_socket`.
        let co = socket.with_state(|state| state.co(event));
        if !co.is_null() {
            set_err(socket, 0);
            // SAFETY: the coroutine bound for `event`, parked in its wait on `socket`.
            unsafe { (NATIVE.resume)(co) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::test_host::{ON_SUSPEND, REACTOR_OK, install, log, take};
    use crate::co_socket::TestSocket;

    fn socket(status: u32) -> TestSocket {
        install();
        let socket = TestSocket::new();
        let record = unsafe { socket.0.record() };
        record.output.close_wait = u8::from(status & CLOSE_WAIT != 0);
        record.kernel_nobufs = status & NOBUFS != 0;
        if status & (WANT_READ | WANT_WRITE) != 0 {
            unsafe { record.set_ssl(core::ptr::NonNull::<u8>::dangling().as_ptr().cast()) };
        }
        record.tls.want_read = u8::from(status & WANT_READ != 0);
        record.tls.want_write = u8::from(status & WANT_WRITE != 0);
        record.removed = u8::from(status & REMOVED != 0);
        record.events = (status & (READ | WRITE)).cast_signed();
        socket
    }

    /// The suspended coroutine records whether the ready callback picks it.
    fn wake_on_ready() {
        ON_SUSPEND.set(Some(Box::new(|socket: Co, event| {
            let bound = socket.with_state(|state| state.co(event));
            let woken = ready(socket, event, false);
            log(format!("woken:{}", !woken.is_null() && woken == bound));
        })));
    }

    fn wait_on(
        socket: &mut TestSocket,
        event: u32,
        barrier: Option<&mut dyn FnMut() -> bool>,
    ) -> bool {
        if let Some(barrier) = barrier {
            return wait_barrier(socket.co(), event, barrier);
        }
        let mut slot = core::ptr::null_mut();
        // The test host's `buffer` only logs the length.
        wait(
            socket.co(),
            event,
            Some(unsafe { WriteSlot::new(&mut slot, 4) }),
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
        let mut s = socket(READ);
        wake_on_ready();
        assert!(wait_on(&mut s, WRITE, None));
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
        assert!(wait_on(&mut s, READ, None));
        assert_eq!(
            effects(),
            ["add:512", "yield:512", "woken:true", "remove:512"]
        );
    }

    #[test]
    fn close_wait_nobufs_and_reactor_failures_stop_early() {
        let mut s = socket(CLOSE_WAIT | NOBUFS);
        assert!(!wait_on(&mut s, READ, None));
        assert_eq!(s.0.err_code, CLOSE_WAIT_ERROR);
        assert_eq!(effects(), Vec::<String>::new());
        // close_wait leaves the no-buffers flag for the next wait.
        assert!(unsafe { s.0.record() }.kernel_nobufs);
        let mut s = socket(NOBUFS);
        assert!(wait_on(&mut s, WRITE, None));
        assert_eq!(effects(), ["sleep"]);
        assert!(!unsafe { s.0.record() }.kernel_nobufs);
        let mut s = socket(0);
        REACTOR_OK.set(false);
        assert!(!wait_on(&mut s, READ, None));
        assert_eq!(effects(), ["set:512"]);
    }

    #[test]
    fn tls_waits_for_the_direction_it_wants() {
        let mut s = socket(WANT_WRITE);
        wake_on_ready();
        assert!(wait_on(&mut s, READ, None));
        // The writable callback wakes the reader; no one else is bound, so remove.
        assert_eq!(
            effects(),
            ["set:1024", "yield:512", "woken:false", "remove:1024"]
        );
        assert_eq!(unsafe { (*s.state()).want }, 0);
    }

    #[test]
    fn wants_count_only_with_a_tls_session() {
        let mut s = socket(WANT_WRITE);
        unsafe { s.0.record() }.take_ssl();
        assert!(wait_on(&mut s, READ, None));
        assert_eq!(effects(), ["set:512", "yield:512", "remove:512"]);
    }

    #[test]
    fn a_barrier_that_keeps_waiting_holds_the_coroutine() {
        let mut s = socket(0);
        ON_SUSPEND.set(Some(Box::new(|socket: Co, event| {
            let woken = ready(socket, event, false);
            log(format!("woken:{}", !woken.is_null()));
        })));
        assert!(wait_on(&mut s, READ, Some(&mut || true)));
        assert_eq!(
            effects(),
            ["set:512", "yield:512", "woken:false", "remove:512"]
        );
        assert!(unsafe { (*s.state()).barriers[0] }.is_none());
    }

    #[test]
    fn a_wake_with_an_error_or_close_fails() {
        let mut s = socket(0);
        ON_SUSPEND.set(Some(Box::new(|socket: Co, _| socket.set_sock_fd(-1))));
        assert!(!wait_on(&mut s, READ, None));
        s.0.sock_fd = 100;
    }

    // Upstream's poll: is_available, the direction's timeout when none is given,
    // then `TimerController timer(...); timer.start() && wait_event(type)`.
    #[test]
    fn poll_waits_under_the_given_or_the_directions_timeout() {
        let mut s = socket(0);
        unsafe { s.0.record() }.timeouts.read = 3.0;
        unsafe { s.0.record() }.timeouts.write = 4.0;
        for (event, timeout, expected) in [(READ, 0.0, 3.0), (WRITE, 0.0, 4.0), (READ, 0.5, 0.5)] {
            take();
            assert!(poll(s.co(), event, timeout));
            let add = if event == READ { "set:512" } else { "set:1024" };
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
        assert!(!poll(s.co(), READ, 1.0));
        assert_eq!(effects(), ["timer:add:1"]);
        s.0.sock_fd = -1;
        assert!(!poll(s.co(), READ, 1.0));
        assert_eq!(effects(), Vec::<String>::new());
        s.0.sock_fd = 100;
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
            let owner = s.ptr().cast::<c_void>();
            let record = unsafe { s.0.record() };
            record.object = if object_matches {
                owner
            } else {
                core::ptr::null_mut()
            };
            record.removed = u8::from(removed);
            let event_socket = s.0.socket;
            unsafe { &mut *s.state() }.bind(READ, (&raw mut reader).cast());
            unsafe { &mut *s.state() }.bind(WRITE, (&raw mut writer).cast());
            ON_RESUME.set(Some(Box::new(move |cid| {
                if close_in_writer && cid == 8 {
                    unsafe { (*event_socket).fields.object = core::ptr::null_mut() };
                }
            })));
            s.0.err_code = 9;
            error_event(
                s.co(),
                unsafe { swoole_rs_reactor::Socket::from_ptr(event_socket) }.unwrap(),
            );
            let mut want = vec!["resume:8".to_string()];
            if object_matches && !removed && !close_in_writer {
                want.push("resume:7".into());
            }
            assert_eq!(effects(), want);
            assert_eq!(s.0.err_code, 0);
            unsafe { &mut *s.state() }.bind(READ, core::ptr::null_mut());
            unsafe { &mut *s.state() }.bind(WRITE, core::ptr::null_mut());
        }
    }
}
