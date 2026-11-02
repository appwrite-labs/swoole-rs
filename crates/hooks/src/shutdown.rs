//! The coroutine socket's connection state, which Rust owns behind an opaque
//! handle (#439), and `Socket::shutdown` over it: admission, the TLS close, the
//! `shutdown(2)` call and the half-close flags. Native code keeps `set_err` and
//! the TLS session.
use crate::vector::Barrier;
use core::ffi::c_void;

/// Whether the socket is connected and which directions are shut.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketFlags {
    pub connected: bool,
    pub read: bool,
    pub write: bool,
}

/// `SW_EVENT_READ` and `SW_EVENT_WRITE` (pinned by a `static_assert` in socket.cc).
pub(crate) const READ: u32 = 1 << 9;
pub(crate) const WRITE: u32 = 1 << 10;

/// One coroutine socket's state; native code holds it only as a pointer.
pub struct SocketState {
    pub(crate) flags: SocketFlags,
    /// The native `Coroutine` waiting to read and to write, or null.
    read_co: *mut c_void,
    write_co: *mut c_void,
    /// `want_event`: the direction a TLS wait added instead of its own, or 0.
    pub(crate) want: u32,
    /// The barriers installed for the read and the write wait, with their state.
    pub(crate) barriers: [Option<(Barrier, *mut c_void)>; 2],
    /// The native `TimerNode` timing the read and the write wait, or null.
    pub(crate) timers: [*mut c_void; 2],
}

/// A new socket's state: not connected, nothing shut, no coroutine bound.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_socket_state_new() -> *mut SocketState {
    Box::into_raw(Box::new(SocketState {
        flags: SocketFlags::default(),
        read_co: core::ptr::null_mut(),
        write_co: core::ptr::null_mut(),
        want: 0,
        barriers: [None; 2],
        timers: [core::ptr::null_mut(); 2],
    }))
}

/// The coroutine bound to `event` (`SW_EVENT_READ` or `SW_EVENT_WRITE`), or null.
///
/// # Safety
/// `state` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_co(
    state: *const SocketState,
    event: u32,
) -> *mut c_void {
    // SAFETY: a live handle (see above).
    let state = unsafe { &*state };
    if event == READ {
        state.read_co
    } else {
        state.write_co
    }
}

/// Binds `co` (null to unbind) for reading, writing, or both when `event` has both bits.
///
/// # Safety
/// `state` is a live handle, not borrowed elsewhere during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_bind(
    state: *mut SocketState,
    event: u32,
    co: *mut c_void,
) {
    // SAFETY: a live, unborrowed handle (see above).
    let state = unsafe { &mut *state };
    if event & READ != 0 {
        state.read_co = co;
    }
    if event & WRITE != 0 {
        state.write_co = co;
    }
}

/// `get_bound_co(event)`: the reader if `event` includes reading and one is bound,
/// otherwise the writer if it includes writing, otherwise null.
///
/// # Safety
/// `state` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_bound_co(
    state: *const SocketState,
    event: u32,
) -> *mut c_void {
    // SAFETY: a live handle (see above).
    let state = unsafe { &*state };
    if event & READ != 0 && !state.read_co.is_null() {
        state.read_co
    } else if event & WRITE != 0 && !state.write_co.is_null() {
        state.write_co
    } else {
        core::ptr::null_mut()
    }
}

/// `get_event_str`'s choice: 0 "reading", 1 "writing", 2 "reading or writing". An
/// event other than read or write names whichever coroutine is bound.
///
/// # Safety
/// `state` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_event_name(
    state: *const SocketState,
    event: u32,
) -> u32 {
    // SAFETY: a live handle (see above).
    let state = unsafe { &*state };
    let (read_bound, write_bound) = (!state.read_co.is_null(), !state.write_co.is_null());
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

/// # Safety
/// `state` is null or a handle from `swr_v1_hook_socket_state_new`, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_state_free(state: *mut SocketState) {
    if !state.is_null() {
        // SAFETY: a live handle from Box::into_raw, released once (see above).
        drop(unsafe { Box::from_raw(state) });
    }
}

/// # Safety
/// `state` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_flags(state: *const SocketState) -> SocketFlags {
    // SAFETY: a live handle (see above).
    unsafe { (*state).flags }
}

/// # Safety
/// `state` is a live handle, not borrowed elsewhere during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_set_flags(state: *mut SocketState, flags: SocketFlags) {
    // SAFETY: a live, unborrowed handle (see above).
    unsafe { (*state).flags = flags };
}

/// The socket connected (or was accepted).
///
/// # Safety
/// `state` is a live handle, not borrowed elsewhere during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_connect(state: *mut SocketState) {
    // SAFETY: a live, unborrowed handle (see above).
    unsafe { (*state).flags.connected = true };
}

/// `Socket::shutdown(how)` after `set_err(0)`: 0 on success, otherwise the error
/// to set. `closed` is `is_closed()`; `ssl_shutdown`, when the socket has TLS,
/// closes the session first. `syscall` is `shutdown(2)`, which sets errno.
///
/// # Safety
/// `state` is a live, unborrowed handle; `ssl_shutdown`, if set, acts on the live
/// socket in `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_shutdown(
    syscall: unsafe extern "C" fn(i32, i32) -> i32,
    fd: i32,
    how: i32,
    closed: bool,
    state: *mut SocketState,
    ssl_shutdown: Option<unsafe extern "C" fn(*mut c_void)>,
    context: *mut c_void,
) -> i32 {
    // SAFETY: a live, unborrowed handle (see above).
    let flags = unsafe { &mut (*state).flags };
    if !flags.connected
        || closed
        || how == libc::SHUT_RD && flags.read
        || how == libc::SHUT_WR && flags.write
    {
        return libc::ENOTCONN;
    }
    if let Some(ssl_shutdown) = ssl_shutdown {
        // SAFETY: a native effect on the live socket in `context`.
        unsafe { ssl_shutdown(context) };
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

    fn connected() -> *mut SocketState {
        let state = swr_v1_hook_socket_state_new();
        unsafe { swr_v1_hook_socket_connect(state) };
        state
    }

    fn flags(state: *mut SocketState) -> SocketFlags {
        unsafe { swr_v1_hook_socket_flags(state) }
    }

    fn shutdown(fd: i32, how: i32, closed: bool, state: *mut SocketState) -> i32 {
        unsafe {
            swr_v1_hook_shutdown(
                libc::shutdown,
                fd,
                how,
                closed,
                state,
                None,
                core::ptr::null_mut(),
            )
        }
    }

    #[test]
    fn half_closes_then_both_disconnects() {
        let fds = pair();
        let state = connected();
        assert_eq!(shutdown(fds[0], libc::SHUT_RD, false, state), 0);
        assert_eq!(
            flags(state),
            SocketFlags {
                connected: true,
                read: true,
                write: false
            }
        );
        assert_eq!(
            shutdown(fds[0], libc::SHUT_RD, false, state),
            libc::ENOTCONN
        );
        assert_eq!(shutdown(fds[0], libc::SHUT_WR, false, state), 0);
        assert_eq!(
            flags(state),
            SocketFlags {
                connected: false,
                read: true,
                write: true
            }
        );
        unsafe { swr_v1_hook_socket_state_free(state) };
        unsafe { libc::close(fds[0]) };
        unsafe { libc::close(fds[1]) };
    }

    #[test]
    fn not_connected_closed_and_bad_descriptors_fail() {
        let state = swr_v1_hook_socket_state_new();
        assert_eq!(shutdown(-1, libc::SHUT_RDWR, false, state), libc::ENOTCONN);
        unsafe { swr_v1_hook_socket_connect(state) };
        assert_eq!(shutdown(-1, libc::SHUT_RDWR, true, state), libc::ENOTCONN);
        assert_eq!(shutdown(-1, libc::SHUT_RDWR, false, state), libc::EBADF);
        assert_eq!(
            flags(state),
            SocketFlags {
                connected: true,
                read: false,
                write: false
            }
        );
        unsafe { swr_v1_hook_socket_state_free(state) };
    }

    #[test]
    fn tls_closes_first() {
        unsafe extern "C" fn mark(context: *mut c_void) {
            unsafe { *context.cast::<bool>() = true };
        }
        let fds = pair();
        let mut closed = false;
        let state = connected();
        let error = unsafe {
            swr_v1_hook_shutdown(
                libc::shutdown,
                fds[0],
                libc::SHUT_RDWR,
                false,
                state,
                Some(mark),
                (&raw mut closed).cast(),
            )
        };
        assert_eq!((error, closed, flags(state).connected), (0, true, false));
        unsafe { swr_v1_hook_socket_state_free(state) };
        unsafe { libc::close(fds[0]) };
        unsafe { libc::close(fds[1]) };
    }

    #[test]
    fn binding_follows_get_bound_co_and_event_names() {
        let state = swr_v1_hook_socket_state_new();
        let (mut reader, mut writer) = (1u8, 2u8);
        let (r, w) = (
            (&raw mut reader).cast::<c_void>(),
            (&raw mut writer).cast::<c_void>(),
        );
        unsafe {
            assert!(swr_v1_hook_socket_bound_co(state, READ | WRITE).is_null());
            assert_eq!(swr_v1_hook_socket_event_name(state, READ | WRITE), 1);
            swr_v1_hook_socket_bind(state, WRITE, w);
            assert_eq!(swr_v1_hook_socket_bound_co(state, READ | WRITE), w);
            assert!(swr_v1_hook_socket_bound_co(state, READ).is_null());
            assert_eq!(swr_v1_hook_socket_event_name(state, READ | WRITE), 1);
            swr_v1_hook_socket_bind(state, READ, r);
            assert_eq!(swr_v1_hook_socket_bound_co(state, READ | WRITE), r);
            assert_eq!(swr_v1_hook_socket_co(state, WRITE), w);
            assert_eq!(swr_v1_hook_socket_event_name(state, READ | WRITE), 2);
            assert_eq!(swr_v1_hook_socket_event_name(state, READ), 0);
            swr_v1_hook_socket_bind(state, READ | WRITE, core::ptr::null_mut());
            assert!(
                swr_v1_hook_socket_co(state, READ).is_null()
                    && swr_v1_hook_socket_co(state, WRITE).is_null()
            );
            swr_v1_hook_socket_state_free(state);
        }
    }
}
