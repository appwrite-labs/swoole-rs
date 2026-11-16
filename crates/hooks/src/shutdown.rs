//! The coroutine socket's connection state, which Rust owns behind an opaque
//! handle (#439), and `Socket::shutdown` over it: admission, the TLS close, the
//! `shutdown(2)` call and the half-close flags. Native code keeps `set_err` and
//! the TLS session.
use crate::vector::Barrier;
use core::ffi::{CStr, c_void};
use swoole_rs_reactor::Events;

/// Whether the socket is connected and which directions are shut.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketFlags {
    pub connected: bool,
    pub read: bool,
    pub write: bool,
}

/// One coroutine socket's state; native code holds it only as a pointer.
pub struct SocketState {
    pub(crate) flags: SocketFlags,
    /// The native `Coroutine` waiting to read and to write, or null.
    read_co: *mut c_void,
    write_co: *mut c_void,
    /// The read and the write wait's waker, which its park stores.
    wakers: [Option<swoole_rs_coroutine::Waker>; 2],
    /// `want_event`: the direction a TLS wait added instead of its own, or none.
    pub(crate) want: Events,
    /// The barriers installed for the read and the write wait.
    pub(crate) barriers: [Option<Barrier>; 2],
    /// The timers of the read and the write wait.
    pub(crate) timers: [WaitTimer; 2],
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
            flags: SocketFlags {
                connected: false,
                read: false,
                write: false,
            },
            read_co: core::ptr::null_mut(),
            write_co: core::ptr::null_mut(),
            wakers: [None; 2],
            want: Events::NONE,
            barriers: [None; 2],
            timers: [WaitTimer::None; 2],
        }
    }

    /// The coroutine bound to `event` (`SW_EVENT_READ` or `SW_EVENT_WRITE`), or null.
    #[must_use]
    pub fn co(&self, event: Events) -> *mut c_void {
        if event == Events::READ {
            self.read_co
        } else {
            self.write_co
        }
    }

    /// Binds `co` (null to unbind) for reading, writing, or both when `event` has both bits.
    /// Only this crate binds, and only the running coroutine for an operation it is about
    /// to suspend in, which unbinds it before it returns: so a bound coroutine is live
    /// and suspended, as [`Co::cancel`](crate::co_socket::Co::cancel) and the reactor
    /// callbacks that resume it, and the bound-coroutine checks that read its id, rely on.
    pub(crate) fn bind(&mut self, event: Events, co: *mut c_void) {
        if event.intersects(Events::READ) {
            self.read_co = co;
            *self.waker_slot(Events::READ) = None;
        }
        if event.intersects(Events::WRITE) {
            self.write_co = co;
            *self.waker_slot(Events::WRITE) = None;
        }
    }

    /// The waker slot of the wait for `event` (`SW_EVENT_READ`, else writing).
    fn waker_slot(&mut self, event: Events) -> &mut Option<swoole_rs_coroutine::Waker> {
        let [read, write] = &mut self.wakers;
        if event == Events::READ { read } else { write }
    }

    /// The waker of the coroutine parked for `event` (`SW_EVENT_READ`, else writing).
    #[must_use]
    pub(crate) fn waker(&mut self, event: Events) -> Option<swoole_rs_coroutine::Waker> {
        *self.waker_slot(event)
    }

    /// Stores the park's waker for `event`, as [`SocketState::waker`] reads it.
    pub(crate) fn park(&mut self, event: Events, waker: swoole_rs_coroutine::Waker) {
        *self.waker_slot(event) = Some(waker);
    }

    /// `get_bound_co(event)`: the reader if `event` includes reading and one is bound,
    /// otherwise the writer if it includes writing, otherwise null.
    #[must_use]
    pub fn bound_co(&self, event: Events) -> *mut c_void {
        if event.intersects(Events::READ) && !self.read_co.is_null() {
            self.read_co
        } else if event.intersects(Events::WRITE) && !self.write_co.is_null() {
            self.write_co
        } else {
            core::ptr::null_mut()
        }
    }

    /// `get_event_str(event)`. An event other than read or write names whichever
    /// coroutine is bound.
    #[must_use]
    pub fn event_name(&self, event: Events) -> &'static CStr {
        let (read_bound, write_bound) = (!self.read_co.is_null(), !self.write_co.is_null());
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

/// `shutdown(2)`, which reports failure through errno.
pub(crate) fn sys_shutdown(fd: i32, how: i32) -> i32 {
    swoole_rs_corebase::sys::shutdown(fd, how).map_or(-1, |()| 0)
}

/// `Socket::shutdown(how)` after `set_err(0)`: 0 on success, otherwise the error
/// to set. `closed` is `is_closed()`; `ssl_shutdown`, when the socket has TLS,
/// closes the session first. `syscall` is [`sys_shutdown`], which sets errno.
pub(crate) fn shutdown(
    syscall: fn(i32, i32) -> i32,
    fd: i32,
    how: i32,
    closed: bool,
    flags: &mut SocketFlags,
    ssl_shutdown: Option<&dyn Fn()>,
) -> i32 {
    if !flags.connected
        || closed
        || how == libc::SHUT_RD && flags.read
        || how == libc::SHUT_WR && flags.write
    {
        return libc::ENOTCONN;
    }
    if let Some(ssl_shutdown) = ssl_shutdown {
        ssl_shutdown();
    }
    let error = if syscall(fd, how) < 0 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    } else {
        0
    };
    if error != 0 && error != libc::ENOTCONN {
        return error;
    }
    // A connection reset by the peer shuts both directions.
    let how = if error == libc::ENOTCONN {
        libc::SHUT_RDWR
    } else {
        how
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
    0
}

#[cfg(test)]
mod tests {
    use super::*;

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
        );
        assert_eq!((error, closed.get(), flags.connected), (0, true, false));
        unsafe { libc::close(fds[0]) };
        unsafe { libc::close(fds[1]) };
    }

    #[test]
    fn binding_follows_get_bound_co_and_event_names() {
        let state = &mut SocketState::new();
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
