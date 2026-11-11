//! The coroutine socket's connection state, which Rust owns behind an opaque
//! handle (#439), and `Socket::shutdown` over it: admission, the TLS close, the
//! `shutdown(2)` call and the half-close flags. Native code keeps `set_err` and
//! the TLS session.
use crate::vector::Barrier;
use core::ffi::c_void;
use swoole_rs_corebase::constants::EventType;

/// Whether the socket is connected and which directions are shut.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketFlags {
    pub connected: bool,
    pub read: bool,
    pub write: bool,
}

pub(crate) const READ: u32 = EventType::SW_EVENT_READ as u32;
pub(crate) const WRITE: u32 = EventType::SW_EVENT_WRITE as u32;

/// One coroutine socket's state; native code holds it only as a pointer.
pub struct SocketState {
    pub(crate) flags: SocketFlags,
    /// The native `Coroutine` waiting to read and to write, or null.
    read_co: *mut c_void,
    write_co: *mut c_void,
    /// `want_event`: the direction a TLS wait added instead of its own, or 0.
    pub(crate) want: u32,
    /// The barriers installed for the read and the write wait.
    pub(crate) barriers: [Option<Barrier>; 2],
    /// The native `TimerNode` timing the read and the write wait, or null.
    pub(crate) timers: [*mut c_void; 2],
}

impl SocketState {
    /// A new socket's state: not connected, nothing shut, no coroutine bound.
    #[must_use]
    pub fn alloc() -> *mut Self {
        Box::into_raw(Box::new(Self {
            flags: SocketFlags::default(),
            read_co: core::ptr::null_mut(),
            write_co: core::ptr::null_mut(),
            want: 0,
            barriers: [None; 2],
            timers: [core::ptr::null_mut(); 2],
        }))
    }

    /// # Safety
    /// `state` is null or came from [`SocketState::alloc`], and is not used afterwards.
    pub unsafe fn free(state: *mut Self) {
        if !state.is_null() {
            // SAFETY: as the caller promises.
            drop(unsafe { Box::from_raw(state) });
        }
    }

    /// The coroutine bound to `event` (`SW_EVENT_READ` or `SW_EVENT_WRITE`), or null.
    #[must_use]
    pub fn co(&self, event: u32) -> *mut c_void {
        if event == READ {
            self.read_co
        } else {
            self.write_co
        }
    }

    /// Binds `co` (null to unbind) for reading, writing, or both when `event` has both bits.
    pub fn bind(&mut self, event: u32, co: *mut c_void) {
        if event & READ != 0 {
            self.read_co = co;
        }
        if event & WRITE != 0 {
            self.write_co = co;
        }
    }

    /// `get_bound_co(event)`: the reader if `event` includes reading and one is bound,
    /// otherwise the writer if it includes writing, otherwise null.
    #[must_use]
    pub fn bound_co(&self, event: u32) -> *mut c_void {
        if event & READ != 0 && !self.read_co.is_null() {
            self.read_co
        } else if event & WRITE != 0 && !self.write_co.is_null() {
            self.write_co
        } else {
            core::ptr::null_mut()
        }
    }

    /// `get_event_str`'s choice: 0 "reading", 1 "writing", 2 "reading or writing". An
    /// event other than read or write names whichever coroutine is bound.
    #[must_use]
    pub fn event_name(&self, event: u32) -> u32 {
        let (read_bound, write_bound) = (!self.read_co.is_null(), !self.write_co.is_null());
        if event == READ {
            0
        } else if event == WRITE {
            1
        } else if read_bound && write_bound {
            2
        } else {
            u32::from(!read_bound)
        }
    }
}

/// `Socket::shutdown(how)` after `set_err(0)`: 0 on success, otherwise the error
/// to set. `closed` is `is_closed()`; `ssl_shutdown`, when the socket has TLS,
/// closes the session first. `syscall` is `shutdown(2)`, which sets errno.
pub(crate) fn shutdown(
    syscall: unsafe extern "C" fn(i32, i32) -> i32,
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
    // SAFETY: shutdown(2) on the socket's descriptor; failure is reported through errno.
    let error = if unsafe { syscall(fd, how) } < 0 {
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
        shutdown(libc::shutdown, fd, how, closed, flags, None)
    }

    #[test]
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
    fn tls_closes_first() {
        let fds = pair();
        let closed = core::cell::Cell::new(false);
        let mut flags = connected();
        let mark = || closed.set(true);
        let error = shutdown(
            libc::shutdown,
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
        let state = unsafe { &mut *SocketState::alloc() };
        let (mut reader, mut writer) = (1u8, 2u8);
        let (r, w) = (
            (&raw mut reader).cast::<c_void>(),
            (&raw mut writer).cast::<c_void>(),
        );
        assert!(state.bound_co(READ | WRITE).is_null());
        assert_eq!(state.event_name(READ | WRITE), 1);
        state.bind(WRITE, w);
        assert_eq!(state.bound_co(READ | WRITE), w);
        assert!(state.bound_co(READ).is_null());
        assert_eq!(state.event_name(READ | WRITE), 1);
        state.bind(READ, r);
        assert_eq!(state.bound_co(READ | WRITE), r);
        assert_eq!(state.co(WRITE), w);
        assert_eq!(state.event_name(READ | WRITE), 2);
        assert_eq!(state.event_name(READ), 0);
        state.bind(READ | WRITE, core::ptr::null_mut());
        assert!(state.co(READ).is_null() && state.co(WRITE).is_null());
        unsafe { SocketState::free(state) };
    }
}
