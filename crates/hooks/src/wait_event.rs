//! `System::wait_event(fd, events, timeout)`: wait until `fd` is readable and/or
//! writable. A zero timeout is one `poll(2)` here; otherwise the coroutine parks on
//! the reactor as an `EventWaiter`, whose reactor socket and coroutine `EventWaitOps`
//! drive (the reactor's and the coroutine's under `System::wait_event`, fakes in tests).
use crate::{event_waiter, wait_revents};
use core::ffi::c_void;
use swoole_rs_corebase::constants::FdType;

/// `SW_EVENT_READ`, `SW_EVENT_WRITE` and `SW_EVENT_ERROR`.
pub(crate) const READ: i32 = 1 << 9;
pub(crate) const WRITE: i32 = 1 << 10;
pub(crate) const ERROR: i32 = 1 << 11;

/// The reactor's and the coroutine's side of an `EventWaiter`.
#[repr(C)]
pub struct EventWaitOps {
    /// The current coroutine.
    pub current: extern "C" fn() -> *mut c_void,
    /// `make_socket(fd, SW_FD_CO_EVENT)` carrying `waiter`, added to the reactor for
    /// `events`; null with the last error in `*error` if the reactor refuses it (the
    /// socket is then released, not its fd).
    pub add: unsafe extern "C" fn(
        fd: i32,
        events: i32,
        waiter: *mut c_void,
        error: *mut i32,
    ) -> *mut c_void,
    /// `co->yield_ex(timeout)`: 0, or the last error if it timed out or was cancelled.
    pub park: fn(timeout: f64) -> i32,
    /// Take the socket off the reactor and release it, not its fd.
    pub remove: unsafe extern "C" fn(socket: *mut c_void),
}

/// A parked `wait_event`: its coroutine and the events fired so far.
struct Waiter {
    co: *mut c_void,
    revents: u32,
}

/// The reactor fired `event` on a waiter's socket: the coroutine to resume
/// (deferred) on the first event, else null.
///
/// # Safety
/// `waiter` is the socket's object, a parked `swr_v1_hook_wait_event`'s.
pub(crate) unsafe fn event_ready(waiter: *mut c_void, event: i32) -> *mut c_void {
    // SAFETY: the waiter lives in its parked frame, and only this thread touches it.
    let waiter = unsafe { &mut *waiter.cast::<Waiter>() };
    let plan = event_waiter(waiter.revents, event.cast_unsigned());
    waiter.revents = plan.revents;
    if plan.wake {
        waiter.co
    } else {
        core::ptr::null_mut()
    }
}

/// Park the current coroutine until `events` fire on `fd` or `timeout` passes: the
/// fired events, or the error.
///
/// # Safety
/// `ops` is native code's, callable in the current coroutine.
unsafe fn park(fd: i32, events: i32, timeout: f64, ops: &EventWaitOps) -> Result<u32, i32> {
    let co = (ops.current)();
    let mut waiter = Waiter { co, revents: 0 };
    let mut error = 0;
    // SAFETY: `waiter` outlives the socket, which `remove` takes off the reactor.
    let socket = unsafe { (ops.add)(fd, events, (&raw mut waiter).cast(), &raw mut error) };
    if socket.is_null() {
        return Err(error);
    }
    let park = ops.park;
    let error = park(timeout);
    if error != 0 && waiter.revents != 0 {
        // An event fired in the loop pass that timed out or cancelled the
        // wait, and its deferred resume is still queued: take it here, or it
        // resumes this coroutine wherever it next waits, or after it ends.
        park(0.0);
    }
    // SAFETY: the socket `add` made.
    unsafe { (ops.remove)(socket) };
    if error == 0 {
        Ok(waiter.revents)
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
/// -1 with errno and the last error set.
///
/// # Safety
/// In a coroutine when `timeout` is not zero (outside one it raises upstream's fatal
/// error, which does not return), with the thread's reactor running.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System10wait_eventEiid")
)]
#[must_use]
pub unsafe extern "C" fn system_wait_event(fd: i32, events: i32, timeout: f64) -> i32 {
    let ops = EventWaitOps {
        current: native::current,
        add: native::add::<{ FdType::SW_FD_CO_EVENT as i32 }>,
        park: native::park,
        remove: native::remove,
    };
    // SAFETY: the reactor's ops, in the current coroutine as the caller promises.
    let waited = unsafe { swr_v1_hook_wait_event(fd, events, timeout, &ops) };
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
    use swoole_rs_reactor::event_api::{swoole_event_add, swoole_event_del};

    pub(crate) use swoole_rs_coroutine::yield_ex as coroutine_yield_ex;

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

    /// Releases the record, not its fd: the waiter doesn't own it.
    pub(crate) unsafe fn release(socket: *mut SocketRecord) {
        // SAFETY: a live record of this module's.
        unsafe { (*socket).fields.fd = -1 };
        // SAFETY: as above; nothing uses it afterwards.
        unsafe { swoole_rs_tls::upstream::socket_free(socket) };
    }

    /// A `FD_TYPE` socket for `fd` with `object` as its object, or null (released, its
    /// fd left open: the caller doesn't own it) with the error in `error` if the
    /// reactor refused it.
    pub(crate) unsafe extern "C" fn add<const FD_TYPE: c_int>(
        fd: i32,
        events: i32,
        object: *mut c_void,
        error: *mut i32,
    ) -> *mut c_void {
        let socket = {
            let mut socket = SocketRecord::with_fd(fd, FD_TYPE);
            socket.object = object;
            socket
        }
        .into_raw();
        // SAFETY: the new record, on the running reactor.
        if unsafe { swoole_event_add(socket, events) } < 0 {
            // SAFETY: the caller's slot.
            unsafe { *error = log::last_error() };
            // SAFETY: the record, which the reactor refused.
            unsafe { release(socket) };
            return core::ptr::null_mut();
        }
        socket.cast()
    }

    pub(super) fn park(timeout: f64) -> i32 {
        if crate::resolve::park_for(timeout) {
            0
        } else {
            log::last_error()
        }
    }

    pub(crate) unsafe extern "C" fn remove(socket: *mut c_void) {
        let socket = socket.cast::<SocketRecord>();
        // SAFETY: the record `add` registered.
        unsafe { swoole_event_del(socket) };
        // SAFETY: as above, now off the reactor.
        unsafe { release(socket) };
    }
}

/// Waits for `events` (only `SW_EVENT_READ`/`WRITE` count) on `fd`.
///
/// # Safety
/// `ops` is callable in the current coroutine.
#[must_use]
pub unsafe fn swr_v1_hook_wait_event(
    fd: i32,
    events: i32,
    timeout: f64,
    ops: &EventWaitOps,
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
        // SAFETY: `pfd` is one valid pollfd.
        let ready = unsafe { libc::poll(&raw mut pfd, 1, 0) };
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
    // SAFETY: native code's ops, callable in the current coroutine.
    let fired = match unsafe { park(fd, events, timeout, ops) } {
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
    const SOCKET: *mut c_void = 0x20 as *mut c_void;

    thread_local! {
        /// The waiter `add` registered, and how many sockets are on the reactor.
        static WAITER: Cell<*mut c_void> = const { Cell::new(core::ptr::null_mut()) };
        static ADDED: Cell<i32> = const { Cell::new(0) };
    }

    extern "C" fn current() -> *mut c_void {
        CO
    }
    unsafe extern "C" fn add(fd: i32, _: i32, waiter: *mut c_void, error: *mut i32) -> *mut c_void {
        if fd == 9 {
            unsafe { *error = libc::EBADF };
            return core::ptr::null_mut();
        }
        WAITER.set(waiter);
        ADDED.set(ADDED.get() + 1);
        SOCKET
    }
    unsafe extern "C" fn remove(socket: *mut c_void) {
        assert_eq!(socket, SOCKET);
        ADDED.set(ADDED.get() - 1);
    }
    /// The reactor fires an error then a read before the deferred resume runs.
    fn fires_error(_: f64) -> i32 {
        assert_eq!(unsafe { event_ready(WAITER.get(), ERROR) }, CO);
        assert!(unsafe { event_ready(WAITER.get(), READ) }.is_null());
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
        assert_eq!(unsafe { event_ready(WAITER.get(), READ) }, CO);
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
        let ops = EventWaitOps {
            current,
            add,
            park,
            remove,
        };
        unsafe { swr_v1_hook_wait_event(fd, events, timeout, &ops) }
    }

    #[test]
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
                let mut waiter = Waiter {
                    co: CO,
                    revents: held.cast_unsigned(),
                };
                let raw = (&raw mut waiter).cast();
                let mut wakes = 0;
                for fired in [event, WRITE] {
                    let co = unsafe { event_ready(raw, fired) };
                    if !co.is_null() {
                        assert_eq!(co, CO);
                        wakes += 1;
                    }
                }
                let expected = usize::from(held == 0) + usize::from(held | event == 0);
                assert_eq!(wakes, expected, "held {held} event {event}");
                assert_eq!(waiter.revents, (held | event | WRITE).cast_unsigned());
            }
        }
    }
}
