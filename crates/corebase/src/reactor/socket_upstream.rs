// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! Rust socket methods and constructors from `socket.cc`, including their last-error
//! and warning behavior.
use crate::CCast;
use crate::log;
use crate::reactor::IoError;
use crate::reactor::socket_sync::set_errno;
use crate::reactor::{SocketAddress, SocketFields, SocketRecord};
use std::ffi::{CStr, c_int};

use crate::sys::errno;

/// `void Socket::clean() const`.
pub fn socket_clean(socket: &SocketFields) {
    crate::reactor::fd_clean(socket.fd);
}

/// `int Socket::wait_event(int timeout_ms, int events) const`.
/// # Errors
/// The timeout or poll error, also recorded for the caller.
pub fn socket_wait_event(
    socket: &SocketFields,
    timeout_ms: c_int,
    events: c_int,
) -> Result<(), IoError> {
    wait_event(
        socket.fd,
        socket.dont_restart,
        timeout_ms,
        crate::reactor::Events::from_bits(events),
    )
}

/// [`socket_wait_event`] over a socket's `fd` and `dont_restart`: whether an event
/// came, else the last error and errno say why not.
pub(crate) fn wait_event(
    fd: c_int,
    dont_restart: bool,
    timeout_ms: c_int,
    events: crate::reactor::Events,
) -> Result<(), IoError> {
    crate::reactor::fd_wait(
        fd,
        crate::reactor::swr_v1_reactor_poll_events_to(events),
        timeout_ms,
        dont_restart,
    )
    .inspect_err(|error| {
        log::set_last_error(error.last_error());
        set_errno(error.errno());
    })
}

/// What `Socket::connect_async` returns: `SW_READY`, `SW_WAIT` (in progress) or
/// `SW_ERROR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connect {
    Ready,
    Wait,
    Error,
}

/// `swReturnCode Socket::connect_async(const Address &sa)`.
pub fn socket_connect_async(socket: &mut SocketFields, address: &SocketAddress) -> Connect {
    socket_set_fd_option(socket, 1, -1);
    match crate::reactor::fd_connect(socket.fd, address) {
        Ok(()) => Connect::Ready,
        Err(error) => {
            log::set_last_error(error.errno());
            set_errno(error.errno());
            if error == crate::reactor::IoError::InProgress {
                Connect::Wait
            } else {
                Connect::Error
            }
        }
    }
}

/// `int Socket::connect_sync(const Address &sa)` of `this`, a live socket on this
/// thread; it waits outside any borrow.
/// # Errors
/// The connection or readiness failure recorded in Swoole's last-error slot.
pub fn connect_sync(
    socket: crate::reactor::Socket<'_>,
    address: &SocketAddress,
) -> Result<(), IoError> {
    match socket.edit(|record| socket_connect_async(record, address)) {
        Connect::Wait => {}
        Connect::Ready => return Ok(()),
        Connect::Error => return Err(IoError::Failed(log::last_error())),
    }
    let timeout = socket.read(|record| record.timeouts.connect);
    // Upstream's `timeout > 0 ? sec2msec(timeout) : timeout` is a double.
    let timeout_ms = if timeout > 0.0 {
        (timeout * 1000.0).c_cast::<i64>().c_cast::<c_int>()
    } else {
        timeout.c_cast::<c_int>()
    };
    if !socket.wait_event(timeout_ms, crate::reactor::Events::WRITE) {
        log::set_last_error(libc::ETIMEDOUT);
        return Err(IoError::TimedOut);
    }
    if let Err(error) = crate::reactor::fd_pending_error(socket.fd()) {
        log::set_last_error(error);
        return Err(IoError::Failed(error));
    }
    socket.edit(|record| socket_set_fd_option(record, 0, -1));
    Ok(())
}

/// `bool Socket::has_timedout() const`.
#[must_use]
pub fn socket_has_timedout() -> bool {
    let error = errno();
    error == libc::EAGAIN
        || error == libc::ETIMEDOUT
        || log::last_error() == crate::reactor::socket_io::POLL_TIMEOUT
}

/// `bool Socket::set_fd_option(int nonblock, int cloexec)`, warning as
/// `_fcntl_set_option`.
pub fn socket_set_fd_option(socket: &mut SocketFields, nonblock: c_int, cloexec: c_int) -> bool {
    let result = socket.set_fd_option(nonblock, cloexec);
    let fd = socket.fd;
    for (bit, call) in [
        (1, "GETFL"),
        (2, "SETFL, opts"),
        (4, "GETFL"),
        (8, "SETFD, opts"),
    ] {
        if result.warnings & bit != 0 {
            log::sys_warning(
                c"network::_fcntl_set_option",
                format_args!("fcntl({fd}, {call}) failed"),
            );
        }
    }
    result.ok
}

/// `if (!nonblock) set_fd_option(1, -1)`: the descriptor made nonblocking unless the
/// socket knows it is, warning as [`socket_set_fd_option`] does when that fails.
#[inline]
pub fn socket_make_nonblocking(socket: &mut SocketFields) {
    if !socket.is_nonblocking() {
        socket_set_fd_option(socket, 1, -1);
    }
}

/// `_set_timeout` and the timeout it records: `SO_SNDTIMEO` (`send`) or `SO_RCVTIMEO`.
fn set_kernel_timeout(socket: &mut SocketFields, send: bool, timeout: f64) -> bool {
    if socket.set_kernel_timeout(send, timeout) {
        return true;
    }
    let which = if send { "SEND" } else { "RECV" };
    log::sys_warning(
        c"network::_set_timeout",
        format_args!("setsockopt(SO_SNDTIMEO, {which}) failed"),
    );
    false
}

/// `bool Socket::set_kernel_read_timeout(double)`: upstream sets `SO_SNDTIMEO`.
pub fn socket_set_kernel_read_timeout(socket: &mut SocketFields, timeout: f64) -> bool {
    set_kernel_timeout(socket, true, timeout)
}

/// `bool Socket::set_kernel_write_timeout(double)`: upstream sets `SO_RCVTIMEO`.
pub fn socket_set_kernel_write_timeout(socket: &mut SocketFields, timeout: f64) -> bool {
    set_kernel_timeout(socket, false, timeout)
}

/// `cork` or `uncork`: the TCP flags in and out of the record, and the warnings.
fn cork_as(socket: &mut SocketFields, cork: bool) -> bool {
    let mut state = crate::reactor::CorkState {
        tcp_nopush: socket.tcp_nopush,
        tcp_nodelay: socket.tcp_nodelay,
    };
    let result = if cork {
        crate::reactor::swr_v1_socket_cork(socket.fd, &mut state)
    } else {
        crate::reactor::swr_v1_socket_uncork(socket.fd, socket.enable_tcp_nodelay, &mut state)
    };
    socket.tcp_nopush = state.tcp_nopush;
    socket.tcp_nodelay = state.tcp_nodelay;
    let (origin, nopush, nodelay): (&CStr, _, _) = if cork {
        (c"Socket::cork", "ON", "OFF")
    } else {
        (c"Socket::uncork", "OFF", "ON")
    };
    let fd = socket.fd;
    if result.warnings & 1 != 0 {
        log::sys_warning(
            origin,
            format_args!("set_tcp_nopush(fd={fd}, {nopush}) failed"),
        );
    }
    if result.warnings & 2 != 0 {
        log::sys_warning(
            origin,
            format_args!("set_tcp_nodelay(fd={fd}, {nodelay}) failed"),
        );
    }
    result.ok
}

/// `bool Socket::cork()`.
pub fn socket_cork(socket: &mut SocketFields) -> bool {
    cork_as(socket, true)
}

/// `bool Socket::uncork()`.
pub fn socket_uncork(socket: &mut SocketFields) -> bool {
    cork_as(socket, false)
}

/// Opens a socket, recording the last error on failure and warning on unknown types.
#[must_use]
pub fn make_typed(socket_type: c_int, fd_type: c_int, flags: c_int) -> Option<Box<SocketRecord>> {
    let mut unknown = false;
    let socket = SocketRecord::opened_typed(socket_type, fd_type, flags, &mut unknown);
    if socket.is_none() {
        let error = errno();
        if unknown {
            log::warning(
                c"make_socket",
                format_args!("unknown socket type [{socket_type}]"),
            );
        }
        log::set_last_error(error);
    }
    socket
}

/// `Socket *make_socket(SocketType, FdType, int domain, int type, int protocol, int flags)`.
#[must_use]
pub fn make_socket_open(
    socket_type: c_int,
    fd_type: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    flags: c_int,
) -> *mut SocketRecord {
    let socket = SocketRecord::open(socket_type, fd_type, domain, kind, protocol, flags);
    if socket.is_null() {
        log::set_last_error(errno());
    }
    socket
}

/// `Reactor::change`'s log: `code` 0 is `swoole_sys_warning`, else
/// `swoole_error_log(SW_LOG_WARNING, code, ...)`, as from `origin`; then the backtrace.
pub fn swr_v1_reactor_log(origin: &CStr, code: c_int, message: &str) {
    if code == 0 {
        log::sys_warning(origin, format_args!("{message}"));
    } else {
        log::error_log(
            log::LogLevel::SW_LOG_WARNING,
            code,
            origin,
            format_args!("{message}"),
        );
    }
    crate::swoole_print_backtrace_on_error();
}

/// The thread's event loop while it is available (`SwooleTG.reactor`, unless null
/// or destroyed), with the prepared state it begins with.
#[must_use]
pub fn event_reactor() -> Option<(*mut std::ffi::c_void, crate::reactor::ReactorPrepared)> {
    let reactor = crate::reactor::event_api::swr_v1_event_reactor();
    (!reactor.is_null()).then(|| (reactor, crate::reactor::ReactorPrepared::of(reactor)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reactor_errors_log_at_warning_level() {
        log::capture::install();
        swr_v1_reactor_log(c"Reactor::change", 1015, "failed");
        let mut seen = log::capture::logged().into_iter();
        let level = seen
            .rfind(|entry| entry.3 == "Reactor::change")
            .map(|entry| entry.1);
        assert_eq!(level, Some(4), "upstream SW_LOG_WARNING");
    }

    #[test]
    fn wait_event_times_out_with_the_poll_error() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let socket = SocketRecord::with_fd(fds[0], 0);
        assert_eq!(socket_wait_event(&socket, 0, 0), Err(IoError::TimedOut));
        assert_eq!(log::last_error(), crate::reactor::socket_io::POLL_TIMEOUT);
        assert!(socket_has_timedout());
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn make_socket_with_an_unknown_type_sets_the_last_error() {
        assert!(make_typed(-1, 0, 0).is_none());
        assert_eq!(log::last_error(), libc::ESOCKTNOSUPPORT);
    }
}
