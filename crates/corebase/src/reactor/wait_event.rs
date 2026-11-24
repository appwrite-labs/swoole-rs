//! `System::wait_event(fd, events, timeout)`: wait until `fd` is readable and/or
//! writable. A zero timeout is one `poll(2)` here; otherwise the coroutine parks on
//! the reactor as an `EventWaiter`, whose reactor socket and coroutine `EventWaitOps`
//! drive (the reactor's [`Watch`] and the coroutine's under `System::wait_event`, fakes
//! in tests).
use crate::coroutine::Waker;
use crate::offload::WakerSlot;
pub use crate::reactor::Events;
use core::cell::Cell;
use core::ffi::c_void;

pub use native::Watch;

/// The reactor's and the coroutine's side of an `EventWaiter`; `S` is its reactor
/// socket, which dropping takes off the reactor and releases, not its fd.
pub struct EventWaitOps<S> {
    /// The current coroutine.
    pub(crate) current: extern "C" fn() -> *mut c_void,
    /// `make_socket(fd, SW_FD_CO_EVENT)` carrying `waiter`, added to the reactor for
    /// `events`; the last error if the reactor refuses it.
    pub(crate) add: fn(fd: i32, events: Events, waiter: Registered) -> Result<S, i32>,
    /// `co->yield_ex(timeout)`, its waker stored first: the last error if it timed
    /// out or was cancelled.
    pub(crate) park: fn(timeout: f64, waker: &WakerSlot) -> Result<(), i32>,
}

/// A parked `wait_event` (the object of its reactor socket): its coroutine's waker
/// and the events fired so far.
pub(crate) struct Waiter {
    waker: WakerSlot,
    revents: Cell<Events>,
}

/// A waiter's park, which this ends once the deferred wake runs. A wake still queued
/// when the wait ended otherwise (a timeout or a cancel in the same loop pass) names
/// a park that is over, so it does nothing.
pub(crate) struct Parked(Waker);

impl Parked {
    #[cfg(test)]
    pub(crate) const fn waker(&self) -> Waker {
        self.0
    }
}

impl crate::reactor::event_api::Deferred for Parked {
    fn run(self) {
        self.0.wake();
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
/// (deferred) on the first event. Native `EventWaiter` queues at most one deferred
/// wake before publishing the bits.
pub(crate) fn event_ready(waiter: &Waiter, event: Events) -> Option<Parked> {
    let received = waiter.revents.get();
    waiter.revents.set(received | event);
    waiter
        .waker
        .get()
        .filter(|_| received.is_empty())
        .map(Parked)
}

/// Park the current coroutine until `events` fire on `fd` or `timeout` passes: the
/// fired events, or the error.
fn park<S>(fd: i32, events: Events, timeout: f64, ops: &EventWaitOps<S>) -> Result<Events, i32> {
    // Fatal outside a coroutine.
    (ops.current)();
    let waiter = Waiter {
        waker: WakerSlot::new(None),
        revents: Cell::new(Events::NONE),
    };
    // `waiter` outlives the socket, dropped (off the reactor) below.
    let socket = (ops.add)(
        fd,
        events,
        Registered((&raw const waiter).cast_mut().cast()),
    )?;
    let parked = (ops.park)(timeout, &waiter.waker);
    drop(socket);
    parked.map(|()| waiter.revents.get())
}

/// A failed event wait, retaining where upstream publishes its error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitError {
    /// Invalid event bits or an immediate poll failure: Swoole's last error.
    Poll(i32),
    /// Reactor registration or the suspended wait failed: errno.
    Coroutine(i32),
}

/// `translate_events_to_poll`.
#[must_use]
pub fn to_poll(events: Events) -> i16 {
    let mut poll = 0;
    if events.intersects(Events::READ) {
        poll |= libc::POLLIN;
    }
    if events.intersects(Events::WRITE) {
        poll |= libc::POLLOUT;
    }
    poll
}

/// `translate_events_from_poll`: error and hang-up count only without IN/OUT.
pub(crate) fn from_poll(revents: i16) -> Events {
    let mut events = Events::NONE;
    if revents & libc::POLLIN != 0 {
        events |= Events::READ;
    }
    if revents & libc::POLLOUT != 0 {
        events |= Events::WRITE;
    }
    if revents & (libc::POLLERR | libc::POLLHUP) != 0
        && revents & (libc::POLLIN | libc::POLLOUT) == 0
    {
        events |= Events::ERROR;
    }
    events
}

/// `System::wait_event`: the fired events, publishing the error on failure.
/// Outside a coroutine a nonzero `timeout` is upstream's fatal error, which does not
/// return.
///
/// # Errors
/// Invalid events, immediate polling, reactor registration, or the suspended wait.
pub fn wait_event(fd: i32, events: Events, timeout: f64) -> Result<Events, WaitError> {
    let ops = EventWaitOps {
        current: native::current,
        add: |fd, events, waiter| Watch::add(fd, events, native::Object::Waiter(waiter)),
        park: native::park,
    };
    let waited = wait_with(fd, events, timeout, &ops);
    match waited {
        Err(WaitError::Poll(code)) if code != 0 => crate::log::set_last_error(code),
        Err(WaitError::Coroutine(code)) if code != 0 => crate::set_errno(code),
        _ => {}
    }
    waited
}

/// The event waiter's reactor socket and coroutine: upstream's `make_socket`,
/// `swoole_event_add`/`del` and `Coroutine::yield_ex`.
pub(crate) mod native {
    use crate::constants::FdType;
    use crate::log;
    use crate::reactor::{ChangeOp, event_api, reactor_upstream};
    use crate::reactor::{Events, SocketRecord};
    use crate::tls::event_socket::EventSocket;
    use core::ffi::c_int;

    /// `Coroutine::get_current_safe()`.
    pub(crate) use crate::coroutine::current_safe as current;

    /// `SW_ERROR_WRONG_OPERATION`: there is no event loop to register with.
    const WRONG_OPERATION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int;

    /// What a watch's socket carries, with the fd type whose readiness handler
    /// (`crate::reactor::readiness`) takes that object. Each is a parked wait's own
    /// registration, live until the wait drops its watches.
    #[derive(Clone, Copy)]
    pub(crate) enum Object {
        /// A hooked poll's task (`SW_FD_CO_POLL`).
        Poll(crate::reactor::socket_poll::Registered),
        /// A `wait_event` waiter (`SW_FD_CO_EVENT`).
        Waiter(super::Registered),
    }

    /// A reactor socket over an fd its owner borrows (upstream's `make_socket`), on
    /// the thread's reactor with an object the readiness handlers reach. Dropping it
    /// takes it off the reactor and frees the record, leaving the fd open.
    pub struct Watch(Option<EventSocket>);

    impl Watch {
        /// Registers `fd` for `events`, with `object` and its fd type. The readiness
        /// handlers reach `object`'s task or waiter through the record until the watch
        /// drops, which its `Registered` keeps live until then.
        ///
        /// # Errors
        /// The last error, when there is no reactor or it refuses the socket.
        pub(crate) fn add(fd: i32, events: Events, object: Object) -> Result<Self, i32> {
            let reactor = event_api::available().ok_or(WRONG_OPERATION)?;
            let (fd_type, object) = match object {
                Object::Poll(task) => (FdType::SW_FD_CO_POLL, task.as_ptr()),
                Object::Waiter(waiter) => (FdType::SW_FD_CO_EVENT, waiter.as_ptr()),
            };
            let mut record = SocketRecord::with_fd(fd, fd_type as c_int);
            record.attach_object(object);
            // The record is this watch's until it drops, off the reactor by then.
            let socket = EventSocket::adopt(Box::new(record));
            if !reactor_upstream::change(reactor, socket.heap(), ChangeOp::Add, events) {
                let error = log::last_error();
                socket.release_record(true);
                return Err(error);
            }
            Ok(Self(Some(socket)))
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            let Some(socket) = self.0.take() else {
                return;
            };
            if let Some(reactor) = event_api::available() {
                _ = reactor_upstream::reactor_del(reactor, socket.socket());
            }
            socket.release_record(true);
        }
    }

    pub(super) fn park(timeout: f64, waker: &super::WakerSlot) -> Result<(), i32> {
        if crate::offload::park_for_armed(timeout, waker) {
            Ok(())
        } else {
            Err(log::last_error())
        }
    }
}

/// Waits for `events` (only `SW_EVENT_READ`/`WRITE` count) on `fd`.
pub(crate) fn wait_with<S>(
    fd: i32,
    events: Events,
    timeout: f64,
    ops: &EventWaitOps<S>,
) -> Result<Events, WaitError> {
    let events = events & Events::RDWR;
    if events.is_empty() {
        return Err(WaitError::Poll(libc::EINVAL));
    }
    if timeout == 0.0 {
        let mut pfd = libc::pollfd {
            fd,
            events: to_poll(events),
            revents: 0,
        };
        let ready = crate::sys::poll(core::slice::from_mut(&mut pfd), 0);
        if matches!(ready, Ok(1)) {
            if pfd.revents & libc::POLLNVAL != 0 {
                return Err(WaitError::Poll(libc::EBADF));
            }
            return Ok(from_poll(pfd.revents));
        }
        let error = ready
            .err()
            .map_or(libc::ETIMEDOUT, |error| error.raw_os_error().unwrap_or(0));
        return Err(WaitError::Poll(error));
    }
    let fired = park(fd, events, timeout, ops).map_err(WaitError::Coroutine)?;
    // Error readiness becomes the requested read/write bits.
    let result = if fired.intersects(Events::ERROR) {
        fired.without(Events::ERROR) | events
    } else {
        fired
    };
    Ok(result)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    clippy::unnecessary_wraps,
    reason = "the fakes have EventWaitOps' signatures and assert what fired while parked"
)]
mod tests {
    use super::*;

    // Adapt the typed result to the former gate record, preserving every assertion.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct WaitEvent {
        result: i32,
        last_error: i32,
        error: i32,
    }

    const fn failed(last_error: i32, error: i32) -> WaitEvent {
        WaitEvent {
            result: -1,
            last_error,
            error,
        }
    }

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
    fn add(fd: i32, _: Events, waiter: Registered) -> Result<Socket, i32> {
        if fd == 9 {
            return Err(libc::EBADF);
        }
        WAITER.set(waiter.as_ptr());
        ADDED.set(ADDED.get() + 1);
        Ok(Socket)
    }
    /// The reactor fires an error then a read before the deferred resume runs.
    fn fires_error(_: f64, waker: &WakerSlot) -> Result<(), i32> {
        waker.set(Some(Waker::NONE));
        assert_eq!(
            event_ready(waiter(), Events::ERROR).map(|co| co.waker()),
            Some(Waker::NONE)
        );
        assert!(event_ready(waiter(), Events::READ).is_none());
        Ok(())
    }
    fn times_out(_: f64, _: &WakerSlot) -> Result<(), i32> {
        Err(libc::ETIMEDOUT)
    }
    fn unused(_: f64, _: &WakerSlot) -> Result<(), i32> {
        unreachable!()
    }
    thread_local! {
        static PARKS: Cell<i32> = const { Cell::new(0) };
    }
    /// A read fires, then the timer times the wait out before the deferred
    /// resume runs: that resume names the park the timeout ended.
    fn fires_then_times_out(_: f64, waker: &WakerSlot) -> Result<(), i32> {
        PARKS.set(PARKS.get() + 1);
        waker.set(Some(Waker::NONE));
        assert_eq!(
            event_ready(waiter(), Events::READ).map(|co| co.waker()),
            Some(Waker::NONE)
        );
        Err(libc::ETIMEDOUT)
    }

    #[test]
    fn a_timeout_after_an_event_parks_once() {
        assert_eq!(
            wait(7, Events::READ, 1.0, fires_then_times_out),
            failed(0, libc::ETIMEDOUT)
        );
        assert_eq!(PARKS.get(), 1);
        assert_eq!(ADDED.get(), 0);
    }

    fn wait(
        fd: i32,
        events: Events,
        timeout: f64,
        park: fn(f64, &WakerSlot) -> Result<(), i32>,
    ) -> WaitEvent {
        let ops = EventWaitOps { current, add, park };
        match wait_with(fd, events, timeout, &ops) {
            Ok(events) => WaitEvent {
                result: events.bits(),
                ..WaitEvent::default()
            },
            Err(WaitError::Poll(code)) => failed(code, 0),
            Err(WaitError::Coroutine(code)) => failed(0, code),
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri's poll reports a pipe's read end writable")]
    fn polls_now_or_parks() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let [reader, writer] = fds;
        let ok = |result: Events| WaitEvent {
            result: result.bits(),
            ..WaitEvent::default()
        };

        assert_eq!(
            wait(reader, Events::ERROR, 0.0, unused),
            failed(libc::EINVAL, 0)
        );
        assert_eq!(
            wait(reader, Events::READ, 0.0, unused),
            failed(libc::ETIMEDOUT, 0)
        );
        assert_eq!(
            wait(writer, Events::WRITE | Events::ERROR, 0.0, unused),
            ok(Events::WRITE)
        );
        assert_eq!(unsafe { libc::write(writer, b"x".as_ptr().cast(), 1) }, 1);
        assert_eq!(
            wait(reader, Events::READ | Events::WRITE, 0.0, unused),
            ok(Events::READ)
        );
        unsafe { libc::close(writer) };
        assert_eq!(
            unsafe { libc::read(reader, [0u8; 1].as_mut_ptr().cast(), 1) },
            1
        );
        let result = Events::from_bits(wait(reader, Events::READ, 0.0, unused).result);
        assert!(result.intersects(Events::READ | Events::ERROR));
        unsafe { libc::close(reader) };
        // Never open, so poll reports POLLNVAL (a just-closed fd may be reused).
        assert_eq!(
            wait(100_000, Events::READ, 0.0, unused),
            failed(libc::EBADF, 0)
        );

        assert_eq!(wait(7, Events::READ, 1.0, fires_error), ok(Events::READ));
        assert_eq!(
            wait(7, Events::READ, -1.0, times_out),
            failed(0, libc::ETIMEDOUT)
        );
        assert_eq!(wait(9, Events::READ, 1.0, unused), failed(0, libc::EBADF));
        assert_eq!(ADDED.get(), 0);
    }

    /// The former hooks-ordered gate's `EventWaiter` scenarios (1023..1059): a waiter holding
    /// `held` fires `event` then `Events::WRITE`. Upstream's callback deferred a resume only
    /// while nothing had fired yet, and published the bits after choosing.
    #[test]
    fn ported_event_waiter_matrix() {
        let masks = [
            Events::NONE,
            Events::READ,
            Events::WRITE,
            Events::ERROR,
            Events::READ | Events::WRITE,
            Events::READ | Events::ERROR,
        ];
        for held in masks {
            for event in masks {
                let waiter = Waiter {
                    waker: WakerSlot::new(Some(Waker::NONE)),
                    revents: Cell::new(held),
                };
                let mut wakes = 0;
                for fired in [event, Events::WRITE] {
                    if let Some(co) = event_ready(&waiter, fired) {
                        assert_eq!(co.waker(), Waker::NONE);
                        wakes += 1;
                    }
                }
                let expected =
                    usize::from(held.is_empty()) + usize::from((held | event).is_empty());
                assert_eq!(wakes, expected, "held {held} event {event}");
                assert_eq!(waiter.revents.get(), held | event | Events::WRITE);
            }
        }
    }
}
