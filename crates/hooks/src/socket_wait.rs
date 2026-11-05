//! `Socket::wait_event` of the coroutine socket and its readable and writable
//! reactor callbacks, over the Rust-owned state: which event to add (a TLS session
//! may want the other direction), binding the coroutine for the wait, the barrier
//! that retries a transfer before the waiter wakes, and removing the event after.
//! `poll` waits the same way under a timer. Native code keeps the reactor, the
//! coroutine switch and the write buffer ([`Host`](crate::co_io::Host)).
use crate::co_io::{CLOSE_WAIT_ERROR, Timer, available, host, set_err};
use crate::co_socket::CoSocket;
use crate::shutdown::{READ, SocketState, WRITE, swr_v1_hook_socket_bind, swr_v1_hook_socket_co};
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
struct Wait(*mut CoSocket);

impl Wait {
    fn socket(&self) -> &CoSocket {
        // SAFETY: the live record the caller passes; not held across a native call.
        unsafe { &*self.0 }
    }

    fn state(&self) -> *mut SocketState {
        self.socket().state
    }

    fn closed(&self) -> bool {
        self.socket().is_closed()
    }

    /// `close_wait`, else `has_kernel_nobufs()` (which clears it); the TLS
    /// session's wants, `removed` and `events`.
    fn status(&self) -> u32 {
        // SAFETY: an open socket's record, not otherwise borrowed here.
        let record = unsafe { self.socket().record() };
        if record.output.close_wait != 0 {
            return CLOSE_WAIT;
        }
        let tls = !record.ssl.is_null();
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
    }

    fn set_err(&self, error: i32) {
        // SAFETY: the live record, not borrowed here.
        unsafe { set_err(self.0, error) };
    }

    fn bind(&self, event: u32, co: *mut c_void) {
        // SAFETY: the socket's live handle, not borrowed across this call.
        unsafe { swr_v1_hook_socket_bind(self.state(), event, co) };
    }

    fn want(&self) -> u32 {
        // SAFETY: a read of the live handle.
        unsafe { (*self.state()).want }
    }

    fn set_want(&self, event: u32) {
        // SAFETY: a write to the live handle, not borrowed across this call.
        unsafe { (*self.state()).want = event };
    }

    fn set_barrier(&self, event: u32, barrier: Option<(Barrier, *mut c_void)>) {
        // SAFETY: a write to the live handle, not borrowed across this call.
        if let Some(slot) = unsafe { (*self.state()).barriers.get_mut(slot(event)) } {
            *slot = barrier;
        }
    }

    /// `add_event`: add `event` to the reactor unless it is there already.
    fn add(&self, status: u32, event: u32) -> bool {
        let removed = status & REMOVED != 0;
        let events = if removed {
            event
        } else {
            status & (READ | WRITE) | event
        };
        // SAFETY: a native effect on the live socket's reactor registration.
        let added = status & event != 0 || unsafe { (host().reactor)(self.0, events, removed) };
        self.set_err(if added { 0 } else { crate::co_io::errno() });
        added
    }

    fn wait(&self, co: *mut c_void, event: u32, buffer: *mut *mut u8, length: usize) -> bool {
        let status = self.status();
        if status & CLOSE_WAIT != 0 {
            self.set_err(CLOSE_WAIT_ERROR);
            return false;
        }
        let event = if event == READ { READ } else { WRITE };
        if status & NOBUFS != 0 {
            self.bind(event, co);
            // SAFETY: a native coroutine sleep; this coroutine stays bound across it.
            unsafe { (host().sleep)() };
            self.bind(event, core::ptr::null_mut());
            return !self.closed();
        }
        self.set_err(0);
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
        if event == WRITE && !buffer.is_null() {
            // SAFETY: the caller's live buffer slot of `length` bytes.
            unsafe { (host().buffer)(self.0, buffer, length) };
        }
        self.bind(event, co);
        // SAFETY: suspends this coroutine until a callback, timer or cancel resumes it.
        unsafe { (host().suspend)(self.0, co, event) };
        self.bind(event, core::ptr::null_mut());
        // Both coroutines may wait for the same event under TLS.
        // SAFETY: a read of the live handle.
        let bound = unsafe {
            !crate::shutdown::swr_v1_hook_socket_bound_co(self.state(), READ | WRITE).is_null()
        };
        if self.want() == 0 || !bound {
            // SAFETY: a native effect on the live socket's reactor registration.
            unsafe { (host().remove)(self.0, added) };
        }
        self.set_want(0);
        !self.closed() && self.socket().err_code == 0
    }
}

/// `Socket::wait_event(event)` for the current coroutine, with `barrier` (and its
/// state) retrying the transfer while it is installed. `buffer`, when not null, is
/// a write's buffer slot, which may move to the socket's write buffer. True when
/// woken with the socket open and no error.
///
/// # Safety
/// `socket` is a live, open record; the barrier's state outlives the call;
/// `buffer` is null or a live slot of `length` bytes.
pub(crate) unsafe fn wait(
    socket: *mut CoSocket,
    event: u32,
    barrier: Option<(Barrier, *mut c_void)>,
    buffer: *mut *mut u8,
    length: usize,
) -> bool {
    // SAFETY: a native read (fatal outside a coroutine).
    let co = unsafe { (host().current)() };
    if co.is_null() {
        return false;
    }
    let wait = Wait(socket);
    if barrier.is_none() {
        return wait.wait(co, event, buffer, length);
    }
    wait.set_barrier(event, barrier);
    let woke = wait.wait(co, event, buffer, length);
    wait.set_barrier(event, None);
    woke
}

/// [`wait`] for native code.
///
/// # Safety
/// As [`wait`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_wait(
    socket: *mut CoSocket,
    event: u32,
    barrier: Option<Barrier>,
    barrier_state: *mut c_void,
    buffer: *mut *mut u8,
    length: usize,
) -> bool {
    // SAFETY: forwarded.
    unsafe {
        wait(
            socket,
            event,
            barrier.map(|b| (b, barrier_state)),
            buffer,
            length,
        )
    }
}

/// The readable (`event` read) or writable callback: the coroutine to resume, or
/// null. Under a TLS wait the other direction's coroutine wakes; otherwise an
/// installed barrier that keeps waiting holds the coroutine unless the peer hung up.
///
/// # Safety
/// `state` is a live handle; an installed barrier's state is live.
pub unsafe extern "C" fn swr_v1_hook_socket_ready(
    state: *mut SocketState,
    event: u32,
    hup: bool,
) -> *mut c_void {
    let other = if event == READ { WRITE } else { READ };
    // SAFETY: a read of the live handle.
    let want = unsafe { (*state).want };
    // SAFETY: a read of the live handle.
    let barrier = unsafe { (*state).barriers.get(slot(event)) }
        .copied()
        .flatten();
    if want != 0 {
        return if want == event {
            // SAFETY: a read of the live handle.
            unsafe { swr_v1_hook_socket_co(state, other) }
        } else {
            core::ptr::null_mut()
        };
    }
    if let Some((barrier, barrier_state)) = barrier {
        // SAFETY: the installed barrier with its live state; it does not touch `state`'s
        // barriers or bindings.
        if unsafe { barrier(barrier_state) } && !hup {
            return core::ptr::null_mut();
        }
    }
    // SAFETY: a read of the live handle.
    unsafe { swr_v1_hook_socket_co(state, event) }
}

/// `Socket::poll(event, timeout)`: wait for `event` under `timeout`, or the
/// direction's own timeout when it is zero.
///
/// # Safety
/// `socket` is a live record, not borrowed elsewhere for the call.
pub unsafe extern "C" fn swr_v1_hook_co_socket_poll(
    socket: *mut CoSocket,
    event: u32,
    timeout: f64,
) -> bool {
    // SAFETY: as the caller promises.
    if !unsafe { available(socket, event) } {
        return false;
    }
    let timeout = if timeout == 0.0 {
        // SAFETY: as the caller promises.
        let record = unsafe { &*socket };
        // SAFETY: an available record is open; the read ends here.
        let timeouts = unsafe { record.record() }.timeouts;
        if event == READ {
            timeouts.read
        } else {
            timeouts.write
        }
    } else {
        timeout
    };
    // SAFETY: the live record; the timer stops before this returns.
    let mut timer = unsafe { Timer::new(socket, event, timeout) };
    // SAFETY: the live, open record.
    let woke = timer.start() && unsafe { wait(socket, event, None, core::ptr::null_mut(), 0) };
    timer.stop();
    woke
}

/// The reactor's error callback: wake the writer, then the reader unless the
/// writer closed the socket (`event_socket` no longer names `owner`, the native
/// object over `socket`, or left the reactor).
///
/// # Safety
/// `socket` is a live record and `event_socket` the live `network::Socket` the
/// event came from; a resumed coroutine may free the record but not `event_socket`.
pub unsafe extern "C" fn swr_v1_hook_co_socket_error_event(
    socket: *mut CoSocket,
    owner: *const c_void,
    event_socket: *const swoole_rs_reactor::SocketRecord,
) {
    for event in [WRITE, READ] {
        // SAFETY: the live network::Socket (see above).
        let record = unsafe { &*event_socket };
        if event == READ && (record.object.cast_const() != owner || record.removed != 0) {
            return;
        }
        // SAFETY: the record is live: it still owns `event_socket`.
        let state = unsafe { (*socket).state };
        // SAFETY: the record's live state.
        let co = unsafe { swr_v1_hook_socket_co(state, event) };
        if !co.is_null() {
            // SAFETY: as above.
            unsafe { set_err(socket, 0) };
            // SAFETY: wakes the coroutine bound to `event`.
            unsafe { (host().resume)(co) };
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
            record.ssl = core::ptr::NonNull::<u8>::dangling().as_ptr().cast();
        }
        record.tls.want_read = u8::from(status & WANT_READ != 0);
        record.tls.want_write = u8::from(status & WANT_WRITE != 0);
        record.removed = u8::from(status & REMOVED != 0);
        record.events = (status & (READ | WRITE)).cast_signed();
        socket
    }

    /// The suspended coroutine records whether the ready callback picks it.
    fn wake_on_ready() {
        ON_SUSPEND.set(Some(Box::new(|socket: *mut CoSocket, event| {
            let state = unsafe { (*socket).state };
            let bound = unsafe { swr_v1_hook_socket_co(state, event) };
            let woken = unsafe { swr_v1_hook_socket_ready(state, event, false) };
            log(format!("woken:{}", !woken.is_null() && woken == bound));
        })));
    }

    fn wait_on(socket: &mut TestSocket, event: u32, barrier: Option<Barrier>) -> bool {
        let mut slot = core::ptr::null_mut();
        unsafe {
            swr_v1_hook_co_socket_wait(
                socket.ptr(),
                event,
                barrier,
                core::ptr::null_mut(),
                &raw mut slot,
                4,
            )
        }
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
        unsafe { s.0.record() }.ssl = core::ptr::null_mut();
        assert!(wait_on(&mut s, READ, None));
        assert_eq!(effects(), ["set:512", "yield:512", "remove:512"]);
    }

    #[test]
    fn a_barrier_that_keeps_waiting_holds_the_coroutine() {
        unsafe extern "C" fn again(_: *mut c_void) -> bool {
            true
        }
        let mut s = socket(0);
        ON_SUSPEND.set(Some(Box::new(|socket: *mut CoSocket, event| {
            let woken = unsafe { swr_v1_hook_socket_ready((*socket).state, event, false) };
            log(format!("woken:{}", !woken.is_null()));
        })));
        assert!(wait_on(&mut s, READ, Some(again)));
        assert_eq!(
            effects(),
            ["set:512", "yield:512", "woken:false", "remove:512"]
        );
        assert!(unsafe { (*s.state()).barriers[0] }.is_none());
    }

    #[test]
    fn a_wake_with_an_error_or_close_fails() {
        let mut s = socket(0);
        ON_SUSPEND.set(Some(Box::new(|socket: *mut CoSocket, _| unsafe {
            (*socket).sock_fd = -1;
        })));
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
            assert!(unsafe { swr_v1_hook_co_socket_poll(s.ptr(), event, timeout) });
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
        assert!(!unsafe { swr_v1_hook_co_socket_poll(s.ptr(), READ, 1.0) });
        assert_eq!(effects(), ["timer:add:1"]);
        s.0.sock_fd = -1;
        assert!(!unsafe { swr_v1_hook_co_socket_poll(s.ptr(), READ, 1.0) });
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
            unsafe { swr_v1_hook_socket_bind(s.state(), READ, (&raw mut reader).cast()) };
            unsafe { swr_v1_hook_socket_bind(s.state(), WRITE, (&raw mut writer).cast()) };
            ON_RESUME.set(Some(Box::new(move |cid| {
                if close_in_writer && cid == 8 {
                    unsafe { (*event_socket).object = core::ptr::null_mut() };
                }
            })));
            s.0.err_code = 9;
            unsafe { swr_v1_hook_co_socket_error_event(s.ptr(), owner, event_socket) };
            let mut want = vec!["resume:8".to_string()];
            if object_matches && !removed && !close_in_writer {
                want.push("resume:7".into());
            }
            assert_eq!(effects(), want);
            assert_eq!(s.0.err_code, 0);
            unsafe { swr_v1_hook_socket_bind(s.state(), READ, core::ptr::null_mut()) };
            unsafe { swr_v1_hook_socket_bind(s.state(), WRITE, core::ptr::null_mut()) };
        }
    }
}
