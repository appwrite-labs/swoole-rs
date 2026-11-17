//! The coroutine socket's connection state, which Rust owns behind an opaque
//! handle (#439), and `Socket::shutdown` over it: admission, the TLS close, the
//! `shutdown(2)` call and the half-close flags. Native code keeps `set_err` and
//! the TLS session.
use crate::socket_wait::Retry;
use core::cell::Cell;
use core::ffi::{CStr, c_void};
use std::io;
use swoole_rs_corebase::reactor::Events;

/// Whether the socket is connected and which directions are shut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketFlags {
    pub connected: bool,
    pub read: bool,
    pub write: bool,
}

/// One coroutine socket's state, a cell per field: every field is a copy, so a
/// shared borrow reads and writes it. Native code holds it only as a pointer.
pub struct SocketState {
    pub(crate) flags: Cell<SocketFlags>,
    /// The native `Coroutine` waiting to read and to write, or null.
    read_co: Cell<*mut c_void>,
    write_co: Cell<*mut c_void>,
    /// The socket wait's park. Other operations use their own canceler instead.
    wakers: [Cell<Option<swoole_rs_corebase::coroutine::Waker>>; 2],
    /// `want_event`: the direction a TLS wait added instead of its own, or none.
    pub(crate) want: Cell<Events>,
    /// The read and the write wait's retry state.
    pub(crate) retries: [Cell<Retry>; 2],
    /// The timers of the read and the write wait.
    pub(crate) timers: [Cell<WaitTimer>; 2],
}

/// A direction's wait timer (`TimerController`'s slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitTimer {
    None,
    /// A negative timeout's wait, which runs without a timer.
    Unbounded,
    Armed(swoole_rs_corebase::timer_key::Held),
}

impl SocketState {
    /// A new socket's state: not connected, nothing shut, no coroutine bound.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            flags: Cell::new(SocketFlags {
                connected: false,
                read: false,
                write: false,
            }),
            read_co: Cell::new(core::ptr::null_mut()),
            write_co: Cell::new(core::ptr::null_mut()),
            wakers: [const { Cell::new(None) }; 2],
            want: Cell::new(Events::NONE),
            retries: [const { Cell::new(Retry::None) }; 2],
            timers: [const { Cell::new(WaitTimer::None) }; 2],
        }
    }

    /// `f` over a copy of the flags, which then replaces them.
    pub(crate) fn update_flags<T>(&self, f: impl FnOnce(&mut SocketFlags) -> T) -> T {
        let mut flags = self.flags.get();
        let result = f(&mut flags);
        self.flags.set(flags);
        result
    }

    /// The coroutine bound to `event` (`SW_EVENT_READ` or `SW_EVENT_WRITE`), or null.
    #[must_use]
    pub fn co(&self, event: Events) -> *mut c_void {
        if event == Events::READ {
            self.read_co.get()
        } else {
            self.write_co.get()
        }
    }

    /// Binds `co` (null to unbind) for reading, writing, or both when `event` has both bits.
    /// Only this crate binds, and only the running coroutine for an operation it is about
    /// to suspend in, which unbinds it before it returns: so a bound coroutine is live
    /// and suspended, as [`Co::cancel`](crate::co_socket::Co::cancel) and the reactor
    /// callbacks that resume it, and the bound-coroutine checks that read its id, rely on.
    pub(crate) fn bind(&self, event: Events, co: *mut c_void) {
        let [read_waker, write_waker] = &self.wakers;
        if event.intersects(Events::READ) {
            self.read_co.set(co);
            read_waker.set(None);
        }
        if event.intersects(Events::WRITE) {
            self.write_co.set(co);
            write_waker.set(None);
        }
    }

    /// Arms the bound socket wait before it switches out.
    pub(crate) fn arm(&self, event: Events, waker: swoole_rs_corebase::coroutine::Waker) {
        if let Some(slot) = self.wakers.get(usize::from(event == Events::WRITE)) {
            slot.set(Some(waker));
        }
    }

    pub(crate) fn waker(&self, event: Events) -> Option<swoole_rs_corebase::coroutine::Waker> {
        self.wakers
            .get(usize::from(event == Events::WRITE))
            .and_then(Cell::get)
    }

    /// `get_bound_co(event)`: the reader if `event` includes reading and one is bound,
    /// otherwise the writer if it includes writing, otherwise null.
    #[must_use]
    pub fn bound_co(&self, event: Events) -> *mut c_void {
        let (read_co, write_co) = (self.read_co.get(), self.write_co.get());
        if event.intersects(Events::READ) && !read_co.is_null() {
            read_co
        } else if event.intersects(Events::WRITE) && !write_co.is_null() {
            write_co
        } else {
            core::ptr::null_mut()
        }
    }

    /// `get_event_str(event)`. An event other than read or write names whichever
    /// coroutine is bound.
    #[must_use]
    pub fn event_name(&self, event: Events) -> &'static CStr {
        let (read_bound, write_bound) = (
            !self.read_co.get().is_null(),
            !self.write_co.get().is_null(),
        );
        if event == Events::READ {
            c"reading"
        } else if event == Events::WRITE {
            c"writing"
        } else if read_bound && write_bound {
            c"reading or writing"
        } else if read_bound {
            c"reading"
        } else {
            c"writing"
        }
    }
}

/// `Socket::shutdown(how)` after `set_err(0)`. `closed` is `is_closed()`;
/// `ssl_shutdown`, when the socket has TLS, closes the session before the syscall.
///
/// # Errors
/// A disconnected or already shut direction, or the syscall's captured OS error.
pub(crate) fn shutdown(
    syscall: fn(i32, i32) -> io::Result<()>,
    fd: i32,
    how: i32,
    closed: bool,
    flags: &mut SocketFlags,
    ssl_shutdown: Option<&dyn Fn()>,
) -> io::Result<()> {
    if !flags.connected
        || closed
        || how == libc::SHUT_RD && flags.read
        || how == libc::SHUT_WR && flags.write
    {
        return Err(io::Error::from_raw_os_error(libc::ENOTCONN));
    }
    if let Some(ssl_shutdown) = ssl_shutdown {
        ssl_shutdown();
    }
    let how = match syscall(fd, how) {
        Ok(()) => how,
        // A connection reset by the peer shuts both directions.
        Err(error) if error.raw_os_error() == Some(libc::ENOTCONN) => libc::SHUT_RDWR,
        // Preserve the callback's legacy failure-without-errno behavior.
        Err(error) if error.raw_os_error() == Some(0) => how,
        Err(error) => return Err(error),
    };
    match how {
        libc::SHUT_RD => flags.read = true,
        libc::SHUT_WR => flags.write = true,
        _ => {
            flags.read = true;
            flags.write = true;
        }
    }
    if flags.read && flags.write {
        flags.connected = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_corebase::sys::shutdown as sys_shutdown;

    fn pair() -> [i32; 2] {
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        fds
    }

    fn connected() -> SocketFlags {
        SocketFlags {
            connected: true,
            ..SocketFlags::default()
        }
    }

    fn down(fd: i32, how: i32, closed: bool, flags: &mut SocketFlags) -> i32 {
        shutdown(sys_shutdown, fd, how, closed, flags, None)
            .map_or_else(|error| error.raw_os_error().unwrap_or(0), |()| 0)
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shutdown on a socketpair")]
    fn half_closes_then_both_disconnects() {
        let fds = pair();
        let mut flags = connected();
        assert_eq!(down(fds[0], libc::SHUT_RD, false, &mut flags), 0);
        assert_eq!(
            flags,
            SocketFlags {
                connected: true,
                read: true,
                write: false
            }
        );
        assert_eq!(
            down(fds[0], libc::SHUT_RD, false, &mut flags),
            libc::ENOTCONN
        );
        assert_eq!(down(fds[0], libc::SHUT_WR, false, &mut flags), 0);
        assert_eq!(
            flags,
            SocketFlags {
                connected: false,
                read: true,
                write: true
            }
        );
        unsafe { libc::close(fds[0]) };
        unsafe { libc::close(fds[1]) };
    }

    #[test]
    fn not_connected_closed_and_bad_descriptors_fail() {
        let mut flags = SocketFlags::default();
        assert_eq!(down(-1, libc::SHUT_RDWR, false, &mut flags), libc::ENOTCONN);
        flags.connected = true;
        assert_eq!(down(-1, libc::SHUT_RDWR, true, &mut flags), libc::ENOTCONN);
        assert_eq!(down(-1, libc::SHUT_RDWR, false, &mut flags), libc::EBADF);
        assert_eq!(
            flags,
            SocketFlags {
                connected: true,
                read: false,
                write: false
            }
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shutdown on a socketpair")]
    fn tls_closes_first() {
        let fds = pair();
        let closed = core::cell::Cell::new(false);
        let mut flags = connected();
        let mark = || closed.set(true);
        let error = shutdown(
            sys_shutdown,
            fds[0],
            libc::SHUT_RDWR,
            false,
            &mut flags,
            Some(&mark),
        )
        .map_or_else(|error| error.raw_os_error().unwrap_or(0), |()| 0);
        assert_eq!((error, closed.get(), flags.connected), (0, true, false));
        unsafe { libc::close(fds[0]) };
        unsafe { libc::close(fds[1]) };
    }

    #[test]
    fn binding_follows_get_bound_co_and_event_names() {
        let state = &SocketState::new();
        let (mut reader, mut writer) = (1u8, 2u8);
        let (r, w) = (
            (&raw mut reader).cast::<c_void>(),
            (&raw mut writer).cast::<c_void>(),
        );
        assert!(state.bound_co(Events::READ | Events::WRITE).is_null());
        assert_eq!(state.event_name(Events::READ | Events::WRITE), c"writing");
        state.bind(Events::WRITE, w);
        assert_eq!(state.bound_co(Events::READ | Events::WRITE), w);
        assert!(state.bound_co(Events::READ).is_null());
        assert_eq!(state.event_name(Events::READ | Events::WRITE), c"writing");
        state.bind(Events::READ, r);
        assert_eq!(state.bound_co(Events::READ | Events::WRITE), r);
        assert_eq!(state.co(Events::WRITE), w);
        assert_eq!(
            state.event_name(Events::READ | Events::WRITE),
            c"reading or writing"
        );
        assert_eq!(state.event_name(Events::READ), c"reading");
        state.bind(Events::READ | Events::WRITE, core::ptr::null_mut());
        assert!(state.co(Events::READ).is_null() && state.co(Events::WRITE).is_null());
    }
}
