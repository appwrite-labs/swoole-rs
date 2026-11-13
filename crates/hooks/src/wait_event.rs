//! `System::wait_event(fd, events, timeout)`: wait until `fd` is readable and/or
//! writable. A zero timeout is one `poll(2)` here; otherwise the coroutine parks on
//! the reactor as an `EventWaiter`, whose reactor socket and coroutine `EventWaitOps`
//! drive (the reactor's [`Watch`] and the coroutine's under `System::wait_event`, fakes
//! in tests).
use crate::{event_waiter, wait_revents};
use core::cell::Cell;
use core::ffi::c_void;
use swoole_rs_corebase::constants::FdType;

pub use native::Watch;

/// `SW_EVENT_READ`, `SW_EVENT_WRITE` and `SW_EVENT_ERROR`.
pub(crate) const READ: i32 = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as i32;
pub(crate) const WRITE: i32 = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as i32;
pub(crate) const ERROR: i32 = swoole_rs_corebase::constants::EventType::SW_EVENT_ERROR as i32;

/// The reactor's and the coroutine's side of an `EventWaiter`; `S` is its reactor
/// socket, which dropping takes off the reactor and releases, not its fd.
pub struct EventWaitOps<S> {
    /// The current coroutine.
    pub(crate) current: extern "C" fn() -> *mut c_void,
    /// `make_socket(fd, SW_FD_CO_EVENT)` carrying `waiter`, added to the reactor for
    /// `events`; the last error if the reactor refuses it.
    pub(crate) add: fn(fd: i32, events: i32, waiter: Registered) -> Result<S, i32>,
    /// `co->yield_ex(timeout)`: 0, or the last error if it timed out or was cancelled.
    pub(crate) park: fn(timeout: f64) -> i32,
}

/// A parked `wait_event` (the object of its reactor socket): its coroutine and the
/// events fired so far.
pub(crate) struct Waiter {
    co: *mut c_void,
    revents: Cell<u32>,
}

/// A waiter's coroutine, which stays parked until this resumes it: `park` takes
/// a resume still queued when the wait ends otherwise.
pub(crate) struct Parked(*mut c_void);

impl Parked {
    #[cfg(test)]
    pub(crate) fn co(&self) -> *mut c_void {
        self.0
    }
}

impl swoole_rs_reactor::event_api::Deferred for Parked {
    fn run(self) {
        // SAFETY: the waiter's coroutine, parked until this deferred resume runs.
        unsafe { swoole_rs_coroutine::resume(self.0) };
    }

    fn dispose(self) {}
}

/// A parked wait's waiter as its socket's object. Only `park` makes one, for its own
/// waiter, which outlives the socket: it drops the socket before it returns.
#[derive(Clone, Copy)]
pub(crate) struct Registered(*mut c_void);

impl Registered {
    pub(crate) fn as_ptr(self) -> *mut c_void {
        self.0
    }
}

/// The reactor fired `event` on a waiter's socket: the coroutine to resume
/// (deferred) on the first event.
pub(crate) fn event_ready(waiter: &Waiter, event: i32) -> Option<Parked> {
    let plan = event_waiter(waiter.revents.get(), event.cast_unsigned());
    waiter.revents.set(plan.revents);
    plan.wake.then_some(Parked(waiter.co))
}

/// Park the current coroutine until `events` fire on `fd` or `timeout` passes: the
/// fired events, or the error.
fn park<S>(fd: i32, events: i32, timeout: f64, ops: &EventWaitOps<S>) -> Result<u32, i32> {
    let co = (ops.current)();
    let waiter = Waiter {
        co,
        revents: Cell::new(0),
    };
    // `waiter` outlives the socket, dropped (off the reactor) below.
    let socket = (ops.add)(
        fd,
        events,
        Registered((&raw const waiter).cast_mut().cast()),
    )?;
    let park = ops.park;
    let error = park(timeout);
    if error != 0 && waiter.revents.get() != 0 {
        // An event fired in the loop pass that timed out or cancelled the
        // wait, and its deferred resume is still queued: take it here, or it
        // resumes this coroutine wherever it next waits, or after it ends.
        park(0.0);
    }
    drop(socket);
    if error == 0 {
        Ok(waiter.revents.get())
    } else {
        Err(error)
    }
}

/// What `wait_event` returns.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaitEvent {
    /// The fired events, or -1.
    pub result: i32,
    /// For native to set as the last error, or 0.
    pub last_error: i32,
    /// For native to set as errno, or 0.
    pub error: i32,
}

const fn failed(last_error: i32, error: i32) -> WaitEvent {
    WaitEvent {
        result: -1,
        last_error,
        error,
    }
}

/// `translate_events_to_poll`.
pub(crate) fn to_poll(events: i32) -> i16 {
    let mut poll = 0;
    if events & READ != 0 {
        poll |= libc::POLLIN;
    }
    if events & WRITE != 0 {
        poll |= libc::POLLOUT;
    }
    poll
}

/// `translate_events_from_poll`: error and hang-up count only without IN/OUT.
pub(crate) fn from_poll(revents: i16) -> i32 {
    let mut events = 0;
    if revents & libc::POLLIN != 0 {
        events |= READ;
    }
    if revents & libc::POLLOUT != 0 {
        events |= WRITE;
    }
    if revents & (libc::POLLERR | libc::POLLHUP) != 0
        && revents & (libc::POLLIN | libc::POLLOUT) == 0
    {
        events |= ERROR;
    }
    events
}

/// `int System::wait_event(int fd, int events, double timeout)`: the fired events, or
/// -1 with errno and the last error set. Outside a coroutine a nonzero `timeout` is
/// upstream's fatal error, which does not return.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System10wait_eventEiid")
)]
#[must_use]
pub extern "C" fn system_wait_event(fd: i32, events: i32, timeout: f64) -> i32 {
    let ops = EventWaitOps {
        current: native::current,
        add: |fd, events, waiter| {
            // SAFETY: `park`'s own waiter, the object `waiter_ready` takes, which
            // outlives the watch it drops.
            unsafe { Watch::add(fd, FdType::SW_FD_CO_EVENT as i32, events, waiter.as_ptr()) }
        },
        park: native::park,
    };
    let waited = swr_v1_hook_wait_event(fd, events, timeout, &ops);
    if waited.last_error != 0 {
        swoole_rs_corebase::log::set_last_error(waited.last_error);
    }
    if waited.error != 0 {
        crate::co_io::set_errno(waited.error);
    }
    waited.result
}

/// The event waiter's reactor socket and coroutine: upstream's `make_socket`,
/// `swoole_event_add`/`del` and `Coroutine::yield_ex`.
pub(crate) mod native {
    use core::ffi::{c_int, c_void};
    use swoole_rs_corebase::log;
    use swoole_rs_reactor::SocketRecord;
    use swoole_rs_reactor::event_api::{event_del, swoole_event_is_available};
    use swoole_rs_tls::event_socket::OwnedSocket;

    /// `SW_ERROR_CO_OUT_OF_COROUTINE`.
    const OUT_OF_COROUTINE: c_int = 10001;

    /// `Coroutine::get_current_safe()`.
    pub(crate) extern "C" fn current() -> *mut c_void {
        let co = swoole_rs_coroutine::current().cast::<c_void>();
        if co.is_null() {
            // SAFETY: Swoole's fatal error hook; this frame owns nothing.
            unsafe {
                swoole_rs_corebase::fatal_error(
                    OUT_OF_COROUTINE,
                    c"API must be called in the coroutine",
                );
            }
        }
        co
    }

    /// `SW_ERROR_WRONG_OPERATION`: there is no event loop to register with.
    const WRONG_OPERATION: c_int = 509;

    /// A reactor socket over an fd its owner borrows (upstream's `make_socket`), on
    /// the thread's reactor with an object the readiness handlers reach. Dropping it
    /// takes it off the reactor and frees the record, leaving the fd open.
    pub struct Watch(Option<OwnedSocket>);

    impl Watch {
        /// Registers `fd`, typed `fd_type`, for `events` with `object`.
        ///
        /// # Errors
        /// The last error, when there is no reactor or it refuses the socket.
        ///
        /// # Safety
        /// `object` is what `fd_type`'s readiness handlers take it for, and stays
        /// live until the watch is dropped.
        pub unsafe fn add(
            fd: i32,
            fd_type: c_int,
            events: i32,
            object: *mut c_void,
        ) -> Result<Self, i32> {
            if !swoole_event_is_available() {
                return Err(WRONG_OPERATION);
            }
            let mut record = SocketRecord::with_fd(fd, fd_type);
            record.object = object;
            // The record is this watch's until `release` frees it.
            let socket = OwnedSocket::new(Box::new(record));
            if socket.add(events) < 0 {
                let error = log::last_error();
                release(socket);
                return Err(error);
            }
            Ok(Self(Some(socket)))
        }
    }

    /// Frees the record, off the reactor, without closing the fd it borrows.
    fn release(socket: OwnedSocket) {
        socket.get().edit(|record| record.fd = -1);
        socket.free();
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            let Some(socket) = self.0.take() else {
                return;
            };
            if swoole_event_is_available() {
                _ = event_del(socket.get());
            }
            release(socket);
        }
    }

    pub(super) fn park(timeout: f64) -> i32 {
        if crate::resolve::park_for(timeout) {
            0
        } else {
            log::last_error()
        }
    }
}

/// Waits for `events` (only `SW_EVENT_READ`/`WRITE` count) on `fd`.
#[must_use]
pub fn swr_v1_hook_wait_event<S>(
    fd: i32,
    events: i32,
    timeout: f64,
    ops: &EventWaitOps<S>,
) -> WaitEvent {
    let events = events & (READ | WRITE);
    if events == 0 {
        return failed(libc::EINVAL, 0);
    }
    if timeout == 0.0 {
        let mut pfd = libc::pollfd {
            fd,
            events: to_poll(events),
            revents: 0,
        };
        let ready = swoole_rs_corebase::sys::poll(core::slice::from_mut(&mut pfd), 0);
        if ready == 1 {
            if pfd.revents & libc::POLLNVAL != 0 {
                return failed(libc::EBADF, 0);
            }
            return WaitEvent {
                result: from_poll(pfd.revents),
                ..WaitEvent::default()
            };
        }
        let error = if ready < 0 {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        } else {
            libc::ETIMEDOUT
        };
        return failed(error, 0);
    }
    let fired = match park(fd, events, timeout, ops) {
        Ok(fired) => fired,
        Err(error) => return failed(0, error),
    };
    let read_write = (READ | WRITE).cast_unsigned();
    let result = wait_revents(
        events.cast_unsigned(),
        fired,
        read_write,
        ERROR.cast_unsigned(),
    );
    WaitEvent {
        result: result.cast_signed(),
        ..WaitEvent::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::Cell;

    const CO: *mut c_void = 0x10 as *mut c_void;

    /// The fake reactor's socket: dropping it takes it off.
    struct Socket;

    impl Drop for Socket {
        fn drop(&mut self) {
            ADDED.set(ADDED.get() - 1);
        }
    }

    thread_local! {
        /// The waiter `add` registered, and how many sockets are on the reactor.
        static WAITER: Cell<*mut c_void> = const { Cell::new(core::ptr::null_mut()) };
        static ADDED: Cell<i32> = const { Cell::new(0) };
    }

    extern "C" fn current() -> *mut c_void {
        CO
    }
    /// The parked waiter `add` registered.
    fn waiter<'a>() -> &'a Waiter {
        unsafe { &*WAITER.get().cast::<Waiter>() }
    }
    fn add(fd: i32, _: i32, waiter: Registered) -> Result<Socket, i32> {
        if fd == 9 {
            return Err(libc::EBADF);
        }
        WAITER.set(waiter.as_ptr());
        ADDED.set(ADDED.get() + 1);
        Ok(Socket)
    }
    /// The reactor fires an error then a read before the deferred resume runs.
    fn fires_error(_: f64) -> i32 {
        assert_eq!(event_ready(waiter(), ERROR).map(|co| co.0), Some(CO));
        assert!(event_ready(waiter(), READ).is_none());
        0
    }
    fn times_out(_: f64) -> i32 {
        libc::ETIMEDOUT
    }
    fn unused(_: f64) -> i32 {
        unreachable!()
    }
    thread_local! {
        static PARKS: Cell<i32> = const { Cell::new(0) };
    }
    /// A read fires, then the timer times the wait out before the deferred
    /// resume runs; the second park takes that resume.
    fn fires_then_times_out(timeout: f64) -> i32 {
        PARKS.set(PARKS.get() + 1);
        if PARKS.get() == 2 {
            assert_eq!(timeout, 0.0);
            return 0;
        }
        assert_eq!(event_ready(waiter(), READ).map(|co| co.0), Some(CO));
        libc::ETIMEDOUT
    }

    #[test]
    fn a_timeout_after_an_event_takes_its_queued_resume() {
        assert_eq!(
            wait(7, READ, 1.0, fires_then_times_out),
            failed(0, libc::ETIMEDOUT)
        );
        assert_eq!(PARKS.get(), 2);
        assert_eq!(ADDED.get(), 0);
    }

    fn wait(fd: i32, events: i32, timeout: f64, park: fn(f64) -> i32) -> WaitEvent {
        let ops = EventWaitOps { current, add, park };
        swr_v1_hook_wait_event(fd, events, timeout, &ops)
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri's poll reports a pipe's read end writable")]
    fn polls_now_or_parks() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let [reader, writer] = fds;
        let ok = |result| WaitEvent {
            result,
            ..WaitEvent::default()
        };

        assert_eq!(wait(reader, ERROR, 0.0, unused), failed(libc::EINVAL, 0));
        assert_eq!(wait(reader, READ, 0.0, unused), failed(libc::ETIMEDOUT, 0));
        assert_eq!(wait(writer, WRITE | ERROR, 0.0, unused), ok(WRITE));
        assert_eq!(unsafe { libc::write(writer, b"x".as_ptr().cast(), 1) }, 1);
        assert_eq!(wait(reader, READ | WRITE, 0.0, unused), ok(READ));
        unsafe { libc::close(writer) };
        assert_eq!(
            unsafe { libc::read(reader, [0u8; 1].as_mut_ptr().cast(), 1) },
            1
        );
        assert!(wait(reader, READ, 0.0, unused).result & (READ | ERROR) != 0);
        unsafe { libc::close(reader) };
        // Never open, so poll reports POLLNVAL (a just-closed fd may be reused).
        assert_eq!(wait(100_000, READ, 0.0, unused), failed(libc::EBADF, 0));

        assert_eq!(wait(7, READ, 1.0, fires_error), ok(READ));
        assert_eq!(wait(7, READ, -1.0, times_out), failed(0, libc::ETIMEDOUT));
        assert_eq!(wait(9, READ, 1.0, unused), failed(0, libc::EBADF));
        assert_eq!(ADDED.get(), 0);
    }

    /// The former hooks-ordered gate's `EventWaiter` scenarios (1023..1059): a waiter holding
    /// `held` fires `event` then `WRITE`. Upstream's callback deferred a resume only
    /// while nothing had fired yet, and published the bits after choosing.
    #[test]
    fn ported_event_waiter_matrix() {
        let masks = [0, READ, WRITE, ERROR, READ | WRITE, READ | ERROR];
        for held in masks {
            for event in masks {
                let waiter = Waiter {
                    co: CO,
                    revents: Cell::new(held.cast_unsigned()),
                };
                let mut wakes = 0;
                for fired in [event, WRITE] {
                    if let Some(co) = event_ready(&waiter, fired) {
                        assert_eq!(co.0, CO);
                        wakes += 1;
                    }
                }
                let expected = usize::from(held == 0) + usize::from(held | event == 0);
                assert_eq!(wakes, expected, "held {held} event {event}");
                assert_eq!(waiter.revents.get(), (held | event | WRITE).cast_unsigned());
            }
        }
    }
}
