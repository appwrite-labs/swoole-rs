//! `network::Socket`'s descriptor options as whole operations, syscall included:
//! `O_NONBLOCK`/`FD_CLOEXEC`, `TCP_CORK`, `TCP_NODELAY`, buffer sizes, kernel
//! timeouts and `listen`. Native code keeps the `Socket` fields (bitfields, which it
//! copies in and out) and the warning text.
use std::io::Error;
use std::os::raw::{c_int, c_void};

/// `setsockopt(fd, level, name, &value, sizeof(int)) == 0`.
fn set_int(fd: c_int, level: c_int, name: c_int, value: c_int) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sizeof(int) fits socklen_t"
    )]
    let length = size_of::<c_int>() as libc::socklen_t;
    // SAFETY: `value` is a local int and `length` is its size; fd validity is the kernel's to check.
    unsafe { libc::setsockopt(fd, level, name, (&raw const value).cast::<c_void>(), length) == 0 }
}

/// `fcntl(fd, command[, argument])`, retried on `EINTR`.
fn fcntl(fd: c_int, command: c_int, argument: Option<c_int>) -> c_int {
    loop {
        let result = match argument {
            // SAFETY: F_SETFL/F_SETFD take an int argument.
            Some(argument) => unsafe { libc::fcntl(fd, command, argument) },
            // SAFETY: F_GETFL/F_GETFD take no argument.
            None => unsafe { libc::fcntl(fd, command) },
        };
        if result >= 0 || Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return result;
        }
    }
}

/// Read one descriptor flag word, set or clear `bit`, write it back. Bit 0 of the
/// warnings is the read failing (upstream warns and continues with -1), bit 1 the write.
fn update(fd: c_int, get: c_int, set: c_int, bit: c_int, on: bool) -> (bool, u32) {
    let mut warnings = 0;
    let mut flags = fcntl(fd, get, None);
    if flags < 0 {
        warnings |= 1;
    }
    flags = if on { flags | bit } else { flags & !bit };
    if fcntl(fd, set, Some(flags)) < 0 {
        return (false, warnings | 2);
    }
    (true, warnings)
}

/// What a descriptor option call did, for native logging.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketOptionResult {
    pub ok: bool,
    /// `fd_option`: 1 `F_GETFL`, 2 `F_SETFL`, 4 `F_GETFD`, 8 `F_SETFD` failed. `cork`/`uncork`:
    /// 1 setting `TCP_CORK` failed, 2 setting `TCP_NODELAY` failed.
    pub warnings: u32,
}

/// `_fcntl_set_option`: -1 leaves a flag alone, 0 clears it, anything else sets it.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_fd_option(
    fd: c_int,
    nonblock: c_int,
    cloexec: c_int,
) -> SocketOptionResult {
    let mut warnings = 0;
    if nonblock >= 0 {
        let (ok, warned) = update(
            fd,
            libc::F_GETFL,
            libc::F_SETFL,
            libc::O_NONBLOCK,
            nonblock != 0,
        );
        warnings |= warned;
        if !ok {
            return SocketOptionResult { ok, warnings };
        }
    }
    if cloexec >= 0 {
        let (ok, warned) = update(
            fd,
            libc::F_GETFD,
            libc::F_SETFD,
            libc::FD_CLOEXEC,
            cloexec != 0,
        );
        warnings |= warned << 2;
        if !ok {
            return SocketOptionResult { ok, warnings };
        }
    }
    SocketOptionResult { ok: true, warnings }
}

/// `set_tcp_nopush`: `TCP_CORK` where the platform has it, else -1.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_tcp_nopush(fd: c_int, nopush: c_int) -> c_int {
    #[cfg(target_os = "linux")]
    if set_int(fd, libc::IPPROTO_TCP, libc::TCP_CORK, nopush) {
        return 0;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (fd, nopush);
    -1
}

/// `set_tcp_nodelay`'s setsockopt.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_tcp_nodelay(fd: c_int, nodelay: c_int) -> bool {
    set_int(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, nodelay)
}

/// A socket's corking fields, copied in and out of its bitfields.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CorkState {
    pub tcp_nopush: u8,
    pub tcp_nodelay: u8,
}

const HAS_CORK: bool = cfg!(target_os = "linux");

/// `Socket::cork`: set `TCP_CORK` and turn `TCP_NODELAY` off while corked.
///
/// # Safety
/// `state` points to an exclusive `CorkState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_cork(
    fd: c_int,
    state: *mut CorkState,
) -> SocketOptionResult {
    // SAFETY: the caller lends an exclusive CorkState.
    let state = unsafe { &mut *state };
    if state.tcp_nopush != 0 {
        return SocketOptionResult::default();
    }
    if HAS_CORK {
        if swr_v1_socket_tcp_nopush(fd, 1) < 0 {
            return SocketOptionResult {
                ok: false,
                warnings: 1,
            };
        }
        state.tcp_nopush = 1;
    }
    let mut warnings = 0;
    if state.tcp_nodelay != 0 {
        if swr_v1_socket_tcp_nodelay(fd, 0) {
            state.tcp_nodelay = 0;
        } else {
            warnings = 2;
        }
    }
    SocketOptionResult { ok: true, warnings }
}

/// `Socket::uncork`: clear `TCP_CORK` and restore `TCP_NODELAY` when the socket
/// enables it.
///
/// # Safety
/// `state` points to an exclusive `CorkState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_uncork(
    fd: c_int,
    enable_tcp_nodelay: bool,
    state: *mut CorkState,
) -> SocketOptionResult {
    // SAFETY: the caller lends an exclusive CorkState.
    let state = unsafe { &mut *state };
    if state.tcp_nopush == 0 {
        return SocketOptionResult::default();
    }
    if HAS_CORK {
        if swr_v1_socket_tcp_nopush(fd, 0) < 0 {
            return SocketOptionResult {
                ok: false,
                warnings: 1,
            };
        }
        state.tcp_nopush = 0;
    }
    if enable_tcp_nodelay && state.tcp_nodelay == 0 {
        if !swr_v1_socket_tcp_nodelay(fd, 1) {
            return SocketOptionResult {
                ok: false,
                warnings: 2,
            };
        }
        state.tcp_nodelay = 1;
    }
    SocketOptionResult {
        ok: true,
        warnings: 0,
    }
}

/// `SO_RCVBUF` (`recv`), `SO_SNDBUF` (`send`) or both, send first, as
/// `set_buffer_size` does.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_buffer_size(fd: c_int, recv: bool, send: bool, size: u32) -> bool {
    let set = |name| set_int(fd, libc::SOL_SOCKET, name, size.cast_signed());
    (!send || set(libc::SO_SNDBUF)) && (!recv || set(libc::SO_RCVBUF))
}

/// `_set_timeout`: `SO_SNDTIMEO` (`send`) or `SO_RCVTIMEO` to `timeout` seconds,
/// truncated as upstream's `(int)` casts do.
#[must_use]
pub fn swr_v1_socket_kernel_timeout(fd: c_int, send: bool, timeout: f64) -> bool {
    #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) timeout")]
    let seconds = timeout as c_int;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (int) microseconds"
    )]
    let microseconds = ((timeout - f64::from(seconds)) * 1000.0 * 1000.0) as c_int;
    let microseconds = libc::suseconds_t::from(microseconds);
    let value = libc::timeval {
        tv_sec: seconds.into(),
        tv_usec: microseconds,
    };
    let name = if send {
        libc::SO_SNDTIMEO
    } else {
        libc::SO_RCVTIMEO
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sizeof(timeval) fits socklen_t"
    )]
    let length = size_of::<libc::timeval>() as libc::socklen_t;
    // SAFETY: `value` is a local timeval and `length` its size.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            name,
            (&raw const value).cast::<c_void>(),
            length,
        ) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp() -> c_int {
        // SAFETY: socket(2) has no memory preconditions.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(fd >= 0, "a TCP socket");
        fd
    }

    fn close(fd: c_int) {
        // SAFETY: fd is a socket this test opened.
        unsafe { libc::close(fd) };
    }

    fn flags(fd: c_int, get: c_int) -> c_int {
        fcntl(fd, get, None)
    }

    #[test]
    fn fd_options_set_and_clear_each_flag() {
        let fd = tcp();
        assert_eq!(
            swr_v1_socket_fd_option(fd, 1, 1),
            SocketOptionResult {
                ok: true,
                warnings: 0
            }
        );
        assert_ne!(
            flags(fd, libc::F_GETFL) & libc::O_NONBLOCK,
            0,
            "nonblocking"
        );
        assert_ne!(
            flags(fd, libc::F_GETFD) & libc::FD_CLOEXEC,
            0,
            "close-on-exec"
        );
        assert!(swr_v1_socket_fd_option(fd, 0, -1).ok);
        assert_eq!(
            flags(fd, libc::F_GETFL) & libc::O_NONBLOCK,
            0,
            "blocking again"
        );
        assert_ne!(
            flags(fd, libc::F_GETFD) & libc::FD_CLOEXEC,
            0,
            "cloexec untouched"
        );
        close(fd);
        // -1, not the closed fd: a parallel test may reuse that number.
        let closed = swr_v1_socket_fd_option(-1, 1, 1);
        assert_eq!(
            closed,
            SocketOptionResult {
                ok: false,
                warnings: 3
            }
        );
    }

    #[test]
    fn cork_round_trip() {
        let fd = tcp();
        let mut state = CorkState {
            tcp_nopush: 0,
            tcp_nodelay: 1,
        };
        // SAFETY: `state` is a local.
        let corked = unsafe { swr_v1_socket_cork(fd, &raw mut state) };
        assert_eq!(
            corked,
            SocketOptionResult {
                ok: true,
                warnings: 0
            }
        );
        assert_eq!(
            state,
            CorkState {
                tcp_nopush: u8::from(HAS_CORK),
                tcp_nodelay: 0
            }
        );
        // SAFETY: as above.
        let uncorked = unsafe { swr_v1_socket_uncork(fd, true, &raw mut state) };
        assert_eq!(uncorked.ok, HAS_CORK, "uncork only undoes a real cork");
        close(fd);
    }

    #[test]
    fn buffers_and_timeouts() {
        let fd = tcp();
        assert!(swr_v1_socket_buffer_size(fd, true, true, 65536));
        assert!(swr_v1_socket_kernel_timeout(fd, true, 1.5));
        assert!(swr_v1_socket_kernel_timeout(fd, false, 0.25));
        assert!(swr_v1_socket_tcp_nodelay(fd, 1));
        close(fd);
        assert!(!swr_v1_socket_buffer_size(-1, false, true, 1));
    }
}
