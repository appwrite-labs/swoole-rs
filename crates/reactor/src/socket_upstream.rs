// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The `network::Socket` methods and `make_socket` overloads of `socket.cc` that
//! are whole Rust operations, last error and warnings included. With
//! `upstream-symbols` (the hybrid's reactor build) they are defined under their
//! upstream (Itanium) symbols and `socket.cc` no longer defines them.
use crate::socket_sync::set_errno;
use crate::{SocketAddress, SocketFields, SocketRecord};
use std::ffi::{CStr, c_int};
use swoole_rs_corebase::log;

/// `SW_OK`, `SW_ERR`; `swReturnCode`'s `SW_WAIT`, `SW_ERROR` and `SW_READY`.
const OK: c_int = 0;
const ERR: c_int = -1;
const WAIT: c_int = 2;
const ERROR: c_int = 4;
const READY: c_int = 5;
/// `SW_ERROR_SOCKET_POLL_TIMEOUT`.
const POLL_TIMEOUT: c_int = 6002;
/// `SW_EVENT_WRITE`.
const EVENT_WRITE: c_int = 1 << 10;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `void Socket::clean() const`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket5cleanEv")
)]
pub extern "C" fn socket_clean(socket: &SocketFields) {
    crate::fd_clean(socket.fd);
}

/// `int Socket::wait_event(int timeout_ms, int events) const`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket10wait_eventEii")
)]
pub extern "C" fn socket_wait_event(
    socket: &SocketFields,
    timeout_ms: c_int,
    events: c_int,
) -> c_int {
    wait_event(socket.fd, socket.dont_restart != 0, timeout_ms, events)
}

/// [`socket_wait_event`] over a socket's `fd` and `dont_restart`.
pub(crate) fn wait_event(fd: c_int, dont_restart: bool, timeout_ms: c_int, events: c_int) -> c_int {
    let result = crate::fd_wait(
        fd,
        crate::swr_v1_reactor_poll_events_to(events),
        timeout_ms,
        dont_restart,
    );
    match result.status {
        0 => OK,
        1 => {
            log::set_last_error(POLL_TIMEOUT);
            set_errno(libc::EAGAIN);
            ERR
        }
        _ => {
            log::set_last_error(result.error);
            set_errno(result.error);
            ERR
        }
    }
}

/// `swReturnCode Socket::connect_async(const Address &sa)`.
pub fn socket_connect_async(socket: &mut SocketFields, address: &SocketAddress) -> c_int {
    socket_set_fd_option(socket, 1, -1);
    let result = crate::fd_connect(socket.fd, address);
    match result.status {
        0 => READY,
        status => {
            log::set_last_error(result.error);
            set_errno(result.error);
            if status == 1 { WAIT } else { ERROR }
        }
    }
}

/// `int Socket::connect_sync(const Address &sa)`.
/// # Safety
/// `socket` is null or a live socket on this thread.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket12connect_syncERKNS0_7AddressE")
)]
pub unsafe extern "C" fn socket_connect_sync(
    socket: *mut SocketRecord,
    address: &SocketAddress,
) -> c_int {
    // SAFETY: as the caller promises: `this`, a live socket.
    unsafe { crate::Socket::from_ptr(socket) }.map_or(ERR, |socket| connect_sync(socket, address))
}

/// [`socket_connect_sync`] over a socket's handle; it waits outside any borrow.
#[must_use]
pub fn connect_sync(socket: crate::Socket<'_>, address: &SocketAddress) -> c_int {
    match socket.edit(|record| socket_connect_async(record, address)) {
        WAIT => {}
        READY => return OK,
        _ => return ERR,
    }
    let timeout = socket.read(|record| record.timeouts.connect);
    // Upstream's `timeout > 0 ? sec2msec(timeout) : timeout` is a double.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sec2msec's (long) and the int parameter's conversion"
    )]
    let timeout_ms = if timeout > 0.0 {
        (timeout * 1000.0) as i64 as c_int
    } else {
        timeout as c_int
    };
    if socket.wait_event(timeout_ms, EVENT_WRITE) < 0 {
        log::set_last_error(libc::ETIMEDOUT);
        return ERR;
    }
    let pending = crate::fd_pending_error(socket.fd());
    if pending.status != 0 {
        log::set_last_error(pending.error);
        return ERR;
    }
    socket.edit(|record| socket_set_fd_option(record, 0, -1));
    OK
}

/// `bool Socket::has_timedout() const`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket12has_timedoutEv")
)]
pub extern "C" fn socket_has_timedout(_socket: *const SocketRecord) -> bool {
    let error = errno();
    error == libc::EAGAIN || error == libc::ETIMEDOUT || log::last_error() == POLL_TIMEOUT
}

/// `bool Socket::set_fd_option(int nonblock, int cloexec)`, warning as
/// `_fcntl_set_option`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket13set_fd_optionEii")
)]
pub extern "C" fn socket_set_fd_option(
    socket: &mut SocketFields,
    nonblock: c_int,
    cloexec: c_int,
) -> bool {
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket23set_kernel_read_timeoutEd")
)]
pub extern "C" fn socket_set_kernel_read_timeout(socket: &mut SocketFields, timeout: f64) -> bool {
    set_kernel_timeout(socket, true, timeout)
}

/// `bool Socket::set_kernel_write_timeout(double)`: upstream sets `SO_RCVTIMEO`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket24set_kernel_write_timeoutEd")
)]
pub extern "C" fn socket_set_kernel_write_timeout(socket: &mut SocketFields, timeout: f64) -> bool {
    set_kernel_timeout(socket, false, timeout)
}

/// `cork` or `uncork`: the TCP flags in and out of the record, and the warnings.
fn cork_as(socket: &mut SocketFields, cork: bool) -> bool {
    let mut state = crate::CorkState {
        tcp_nopush: socket.tcp_nopush,
        tcp_nodelay: socket.tcp_nodelay,
    };
    let result = if cork {
        crate::swr_v1_socket_cork(socket.fd, &mut state)
    } else {
        crate::swr_v1_socket_uncork(socket.fd, socket.enable_tcp_nodelay, &mut state)
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket4corkEv")
)]
pub extern "C" fn socket_cork(socket: &mut SocketFields) -> bool {
    cork_as(socket, true)
}

/// `bool Socket::uncork()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket6uncorkEv")
)]
pub extern "C" fn socket_uncork(socket: &mut SocketFields) -> bool {
    cork_as(socket, false)
}

/// `Socket *make_socket(SocketType, FdType, int flags)`: null with the last error
/// set when socket(2) failed, warning first for a type it has no arguments for.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11make_socketE12swSocketType8swFdTypei")
)]
pub extern "C" fn make_socket_typed(
    socket_type: c_int,
    fd_type: c_int,
    flags: c_int,
) -> *mut SocketRecord {
    let mut unknown = false;
    let socket = SocketRecord::open_typed(socket_type, fd_type, flags, &mut unknown);
    if socket.is_null() {
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11make_socketE12swSocketType8swFdTypeiiii")
)]
pub extern "C" fn make_socket_open(
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

/// `Socket *make_socket(int fd, FdType)`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11make_socketEi8swFdType")
)]
pub extern "C" fn make_socket_fd(fd: c_int, fd_type: c_int) -> *mut SocketRecord {
    SocketRecord::with_fd(fd, fd_type).into_raw()
}

/// `Reactor::change`'s log: `code` 0 is `swoole_sys_warning`, else
/// `swoole_error_log(SW_LOG_WARNING, code, ...)`, as from `origin`; then the backtrace.
pub fn swr_v1_reactor_log(origin: &CStr, code: c_int, message: &str) {
    if code == 0 {
        log::sys_warning(origin, format_args!("{message}"));
    } else {
        log::error_log(log::LOG_WARNING, code, origin, format_args!("{message}"));
    }
    swoole_rs_corebase::swoole_print_backtrace_on_error();
}

/// The thread's event loop while it is available (`SwooleTG.reactor`, unless null
/// or destroyed), with the prepared state it begins with.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn event_reactor() -> Option<(*mut std::ffi::c_void, crate::ReactorPrepared)> {
    let reactor = crate::event_api::swr_v1_event_reactor();
    // SAFETY: null or a live `Reactor`, which begins with its prepared state.
    let prepared = unsafe { reactor.cast::<crate::ReactorPrepared>().as_ref() }.copied();
    prepared.map(|prepared| (reactor, prepared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn wait_event_times_out_with_the_poll_error() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let socket = SocketRecord::with_fd(fds[0], 0);
        assert_eq!(socket_wait_event(&socket, 0, 0), ERR);
        assert_eq!(log::last_error(), POLL_TIMEOUT);
        assert!(socket_has_timedout(ptr::null()));
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn make_socket_with_an_unknown_type_sets_the_last_error() {
        assert!(make_socket_typed(-1, 0, 0).is_null());
        assert_eq!(log::last_error(), libc::ESOCKTNOSUPPORT);
    }
}

/// Upstream's `network::Socket` members over the record, under their own symbols for
/// the core-tests port (tests/modules/hooks/system), which links them.
#[cfg(feature = "upstream-symbols")]
mod members {
    use crate::{SocketAddress, SocketFields, SocketRecord};
    use std::ffi::c_int;

    /// Each member as `extern "C" fn(this, args...)`: C++ passes `this` and reference
    /// arguments as non-null pointers to live objects, which Rust takes as references.
    macro_rules! members {
        ($($symbol:literal fn $name:ident($this:ident $(, $arg:ident: $ty:ty)*) -> $ret:ty $body:block)*) => {$(
            #[unsafe(export_name = $symbol)]
            extern "C" fn $name($this: &mut SocketFields $(, $arg: $ty)*) -> $ret $body
        )*};
    }

    members! {
        "_ZN6swoole7network6Socket8get_nameEv" fn get_name(this) -> c_int { this.get_name() }
        "_ZN6swoole7network6Socket13get_peer_nameEPNS0_7AddressE"
        fn get_peer_name(this, address: &mut SocketAddress) -> c_int { this.get_peer_name(address) }
        "_ZN6swoole7network6Socket4bindERKNS0_7AddressE"
        fn bind(this, address: &SocketAddress) -> c_int { this.bind(address) }
        "_ZN6swoole7network6Socket6listenEi" fn listen(this, backlog: c_int) -> c_int { this.listen(backlog) }
        "_ZN6swoole7network6Socket6acceptEv" fn accept(this) -> *mut SocketRecord { this.accept() }
        "_ZNK6swoole7network6Socket3dupEv" fn dup(this) -> *mut SocketRecord { this.dup() }
        "_ZN6swoole7network6Socket14set_tcp_nopushEi"
        fn set_tcp_nopush(this, nopush: c_int) -> c_int { this.set_tcp_nopush(nopush) }
        "_ZN6swoole7network6Socket15set_tcp_nodelayEi"
        fn set_tcp_nodelay(this, nodelay: c_int) -> bool { this.set_tcp_nodelay(nodelay) }
        "_ZN6swoole7network6Socket11set_timeoutEdi"
        fn set_timeout(this, timeout: f64, kind: c_int) -> () { this.timeouts.set(timeout, kind) }
        "_ZN6swoole7network6Socket17catch_write_errorEi"
        fn catch_write_error(this, error: c_int) -> c_int { this.catch_error(error) }
        "_ZN6swoole7network6Socket22catch_write_pipe_errorEi"
        fn catch_write_pipe_error(this, error: c_int) -> c_int {
            crate::catch_pipe_error(error, &mut this.kernel_nobufs)
        }
    }
}
