//! `network::Socket`'s descriptor options as whole operations, syscall included:
//! `O_NONBLOCK`/`FD_CLOEXEC`, `TCP_CORK`, `TCP_NODELAY`, buffer sizes, kernel
//! timeouts and `listen`. Native code keeps the `Socket` fields (bitfields, which it
//! copies in and out) and the warning text.
use std::io::Error;
use std::os::raw::c_int;
use swoole_rs_corebase::CCast;

/// `setsockopt(fd, level, name, &value, sizeof(value)) == 0`, for the plain option
/// values (`int`, `timeval`, `linger`).
fn set<T: swoole_rs_corebase::sys::Pod>(fd: c_int, level: c_int, name: c_int, value: T) -> bool {
    swoole_rs_corebase::sys::setsockopt(fd, level, name, &value) == 0
}

/// `setsockopt` of an `int` option, without logging a failure.
#[must_use]
pub fn swr_v1_socket_set_int(fd: c_int, level: c_int, name: c_int, value: c_int) -> bool {
    set(fd, level, name, value)
}

/// `SO_LINGER {1, 0}`: closing the socket now sends RST, as closing one with unread
/// bytes in the kernel does.
#[must_use]
pub fn swr_v1_socket_reset_on_close(fd: c_int) -> bool {
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    set(fd, libc::SOL_SOCKET, libc::SO_LINGER, linger)
}

/// `fcntl(fd, command[, argument])`, retried on `EINTR`.
fn fcntl(fd: c_int, command: c_int, argument: Option<c_int>) -> c_int {
    loop {
        // F_GETFL/F_GETFD ignore the argument.
        let result = swoole_rs_corebase::sys::fcntl(fd, command, argument.unwrap_or(0));
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

/// What a descriptor option call did, for the caller's logging.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketOptionResult {
    pub ok: bool,
    /// `fd_option`: 1 `F_GETFL`, 2 `F_SETFL`, 4 `F_GETFD`, 8 `F_SETFD` failed. `cork`/`uncork`:
    /// 1 setting `TCP_CORK` failed, 2 setting `TCP_NODELAY` failed.
    pub warnings: u32,
}

/// `_fcntl_set_option`: `None` leaves a flag alone, `Some` sets or clears it.
#[must_use]
pub fn swr_v1_socket_fd_option(
    fd: c_int,
    nonblock: Option<bool>,
    cloexec: Option<bool>,
) -> SocketOptionResult {
    let mut warnings = 0;
    if let Some(nonblock) = nonblock {
        let (ok, warned) = update(fd, libc::F_GETFL, libc::F_SETFL, libc::O_NONBLOCK, nonblock);
        warnings |= warned;
        if !ok {
            return SocketOptionResult { ok, warnings };
        }
    }
    if let Some(cloexec) = cloexec {
        let (ok, warned) = update(fd, libc::F_GETFD, libc::F_SETFD, libc::FD_CLOEXEC, cloexec);
        warnings |= warned << 2;
        if !ok {
            return SocketOptionResult { ok, warnings };
        }
    }
    SocketOptionResult { ok: true, warnings }
}

/// `set_tcp_nopush`: `TCP_CORK`, whether it took; false where the platform has none.
#[must_use]
pub fn swr_v1_socket_tcp_nopush(fd: c_int, nopush: c_int) -> bool {
    #[cfg(target_os = "linux")]
    {
        swr_v1_socket_set_int(fd, libc::IPPROTO_TCP, libc::TCP_CORK, nopush)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (fd, nopush);
        false
    }
}

/// `set_tcp_nodelay`'s setsockopt.
#[must_use]
pub fn swr_v1_socket_tcp_nodelay(fd: c_int, nodelay: c_int) -> bool {
    swr_v1_socket_set_int(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, nodelay)
}

/// A socket's corking fields, copied in and out of its bitfields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CorkState {
    pub tcp_nopush: bool,
    pub tcp_nodelay: bool,
}

const HAS_CORK: bool = cfg!(target_os = "linux");

/// `Socket::cork`: set `TCP_CORK` and turn `TCP_NODELAY` off while corked.
pub fn swr_v1_socket_cork(fd: c_int, state: &mut CorkState) -> SocketOptionResult {
    if state.tcp_nopush {
        return SocketOptionResult::default();
    }
    if HAS_CORK {
        if !swr_v1_socket_tcp_nopush(fd, 1) {
            return SocketOptionResult {
                ok: false,
                warnings: 1,
            };
        }
        state.tcp_nopush = true;
    }
    let mut warnings = 0;
    if state.tcp_nodelay {
        if swr_v1_socket_tcp_nodelay(fd, 0) {
            state.tcp_nodelay = false;
        } else {
            warnings = 2;
        }
    }
    SocketOptionResult { ok: true, warnings }
}

/// `Socket::uncork`: clear `TCP_CORK` and restore `TCP_NODELAY` when the socket
/// enables it.
pub fn swr_v1_socket_uncork(
    fd: c_int,
    enable_tcp_nodelay: bool,
    state: &mut CorkState,
) -> SocketOptionResult {
    if !state.tcp_nopush {
        return SocketOptionResult::default();
    }
    if HAS_CORK {
        if !swr_v1_socket_tcp_nopush(fd, 0) {
            return SocketOptionResult {
                ok: false,
                warnings: 1,
            };
        }
        state.tcp_nopush = false;
    }
    if enable_tcp_nodelay && !state.tcp_nodelay {
        if !swr_v1_socket_tcp_nodelay(fd, 1) {
            return SocketOptionResult {
                ok: false,
                warnings: 2,
            };
        }
        state.tcp_nodelay = true;
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
    let set = |name| swr_v1_socket_set_int(fd, libc::SOL_SOCKET, name, size.cast_signed());
    (!send || set(libc::SO_SNDBUF)) && (!recv || set(libc::SO_RCVBUF))
}

/// `_set_timeout`: `SO_SNDTIMEO` (`send`) or `SO_RCVTIMEO` to `timeout` seconds,
/// truncated as upstream's `(int)` casts do.
#[must_use]
pub fn swr_v1_socket_kernel_timeout(fd: c_int, send: bool, timeout: f64) -> bool {
    let seconds = timeout.c_cast::<c_int>();
    let microseconds = ((timeout - f64::from(seconds)) * 1000.0 * 1000.0).c_cast::<c_int>();
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
    set(fd, libc::SOL_SOCKET, name, value)
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
    #[cfg_attr(miri, ignore = "Miri has no fcntl for these flags")]
    fn fd_options_set_and_clear_each_flag() {
        let fd = tcp();
        assert_eq!(
            swr_v1_socket_fd_option(fd, Some(true), Some(true)),
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
        assert!(swr_v1_socket_fd_option(fd, Some(false), None).ok);
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
        let closed = swr_v1_socket_fd_option(-1, Some(true), Some(true));
        assert_eq!(
            closed,
            SocketOptionResult {
                ok: false,
                warnings: 3
            }
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no setsockopt")]
    fn cork_round_trip() {
        let fd = tcp();
        let mut state = CorkState {
            tcp_nopush: false,
            tcp_nodelay: true,
        };
        let corked = swr_v1_socket_cork(fd, &mut state);
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
                tcp_nopush: HAS_CORK,
                tcp_nodelay: false
            }
        );
        let uncorked = swr_v1_socket_uncork(fd, true, &mut state);
        assert_eq!(uncorked.ok, HAS_CORK, "uncork only undoes a real cork");
        close(fd);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no setsockopt")]
    fn buffers_and_timeouts() {
        let fd = tcp();
        assert!(swr_v1_socket_buffer_size(fd, true, true, 65536));
        assert!(swr_v1_socket_kernel_timeout(fd, true, 1.5));
        assert!(swr_v1_socket_kernel_timeout(fd, false, 0.25));
        assert!(swr_v1_socket_tcp_nodelay(fd, 1));
        assert!(swr_v1_socket_reset_on_close(fd));
        close(fd);
        assert!(!swr_v1_socket_buffer_size(-1, false, true, 1));
    }
}
