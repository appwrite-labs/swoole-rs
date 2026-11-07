// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The `network::Socket` methods and `make_socket` overloads of `socket.cc` that
//! are whole Rust operations, last error and warnings included. With
//! `upstream-symbols` (the hybrid's reactor build) they are defined under their
//! upstream (Itanium) symbols and `socket.cc` no longer defines them.
use crate::socket_sync::set_errno;
use crate::{SocketAddress, SocketRecord};
use std::ffi::{CStr, c_char, c_int};
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

/// `swoole_signal_dispatch`, which a blocking call runs when interrupted.
#[must_use]
pub fn signal_dispatch() -> Option<unsafe extern "C" fn()> {
    Some(swoole_rs_process::signal::swoole_signal_dispatch)
}

/// A static member of reference type (`static double &Socket::default_dns_timeout`),
/// which native code reads as a pointer to the referenced object.
#[repr(transparent)]
pub struct NativeRef<T>(*mut T);

// SAFETY: native code reads and assigns the referenced defaults as upstream does
// its plain statics; the pointer itself never changes.
unsafe impl<T> Sync for NativeRef<T> {}

/// `Socket::default_dns_timeout`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket19default_dns_timeoutE")
)]
pub static DEFAULT_DNS_TIMEOUT: NativeRef<f64> =
    // SAFETY: the field's address; nothing is read or written here.
    NativeRef(unsafe { &raw mut crate::swr_v1_socket_defaults.timeouts.dns });
/// `Socket::default_connect_timeout`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket23default_connect_timeoutE")
)]
pub static DEFAULT_CONNECT_TIMEOUT: NativeRef<f64> =
    // SAFETY: the field's address; nothing is read or written here.
    NativeRef(unsafe { &raw mut crate::swr_v1_socket_defaults.timeouts.connect });
/// `Socket::default_read_timeout`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket20default_read_timeoutE")
)]
pub static DEFAULT_READ_TIMEOUT: NativeRef<f64> =
    // SAFETY: the field's address; nothing is read or written here.
    NativeRef(unsafe { &raw mut crate::swr_v1_socket_defaults.timeouts.read });
/// `Socket::default_write_timeout`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket21default_write_timeoutE")
)]
pub static DEFAULT_WRITE_TIMEOUT: NativeRef<f64> =
    // SAFETY: the field's address; nothing is read or written here.
    NativeRef(unsafe { &raw mut crate::swr_v1_socket_defaults.timeouts.write });
/// `Socket::default_buffer_size`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket19default_buffer_sizeE")
)]
pub static DEFAULT_BUFFER_SIZE: NativeRef<u32> =
    // SAFETY: the field's address; nothing is read or written here.
    NativeRef(unsafe { &raw mut crate::swr_v1_socket_defaults.buffer_size });

/// `void Socket::clean() const`.
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket5cleanEv")
)]
pub unsafe extern "C" fn socket_clean(socket: *const SocketRecord) {
    // SAFETY: as the caller promises.
    crate::swr_v1_socket_clean(unsafe { (*socket).fd });
}

/// `int Socket::wait_event(int timeout_ms, int events) const`.
///
/// # Safety
/// `socket` is a live record.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket10wait_eventEii")
)]
pub unsafe extern "C" fn socket_wait_event(
    socket: *const SocketRecord,
    timeout_ms: c_int,
    events: c_int,
) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    let result = crate::swr_v1_socket_wait(
        socket.fd,
        crate::swr_v1_reactor_poll_events_to(events),
        timeout_ms,
        socket.dont_restart != 0,
        signal_dispatch(),
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
///
/// # Safety
/// `socket` is a live record and `address` a live `network::Address`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket13connect_asyncERKNS0_7AddressE")
)]
pub unsafe extern "C" fn socket_connect_async(
    socket: *mut SocketRecord,
    address: *const SocketAddress,
) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { socket_set_fd_option(socket, 1, -1) };
    // SAFETY: as the caller promises.
    let fd = unsafe { (*socket).fd };
    // SAFETY: as the caller promises.
    let address = unsafe { &*address };
    // SAFETY: the address's storage holds `len` bytes of sockaddr.
    let result =
        unsafe { crate::swr_v1_socket_connect(fd, (&raw const address.addr).cast(), address.len) };
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
///
/// # Safety
/// `socket` is a live record and `address` a live `network::Address`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket12connect_syncERKNS0_7AddressE")
)]
pub unsafe extern "C" fn socket_connect_sync(
    socket: *mut SocketRecord,
    address: *const SocketAddress,
) -> c_int {
    // SAFETY: as the caller promises.
    match unsafe { socket_connect_async(socket, address) } {
        WAIT => {}
        READY => return OK,
        _ => return ERR,
    }
    // SAFETY: as the caller promises.
    let timeout = unsafe { (*socket).timeouts.connect };
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
    // SAFETY: as the caller promises.
    if unsafe { socket_wait_event(socket, timeout_ms, EVENT_WRITE) } < 0 {
        log::set_last_error(libc::ETIMEDOUT);
        return ERR;
    }
    // SAFETY: as the caller promises.
    let pending = crate::swr_v1_socket_pending_error(unsafe { (*socket).fd });
    if pending.status != 0 {
        log::set_last_error(pending.error);
        return ERR;
    }
    // SAFETY: as the caller promises.
    unsafe { socket_set_fd_option(socket, 0, -1) };
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

/// `double Socket::get_timeout(TimeoutType type) const`: -1 for a type that is not
/// exactly one timeout, where upstream asserts.
///
/// # Safety
/// `socket` is a live record.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket11get_timeoutE13swTimeoutType")
)]
pub unsafe extern "C" fn socket_get_timeout(socket: *const SocketRecord, kind: c_int) -> f64 {
    // SAFETY: as the caller promises.
    let timeouts = unsafe { &(*socket).timeouts };
    // SAFETY: a live timeout group.
    let timeout = unsafe { crate::swr_v1_socket_timeout(timeouts, kind) };
    // SAFETY: null or a pointer into `timeouts`.
    unsafe { timeout.as_ref() }.copied().unwrap_or(-1.0)
}

/// `bool Socket::set_fd_option(int nonblock, int cloexec)`, warning as
/// `_fcntl_set_option`.
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket13set_fd_optionEii")
)]
pub unsafe extern "C" fn socket_set_fd_option(
    socket: *mut SocketRecord,
    nonblock: c_int,
    cloexec: c_int,
) -> bool {
    // SAFETY: as the caller promises.
    let result = unsafe { crate::swr_v1_socket_set_fd_option(socket, nonblock, cloexec) };
    // SAFETY: as the caller promises.
    let fd = unsafe { (*socket).fd };
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
///
/// # Safety
/// `socket` is a live record.
unsafe fn set_kernel_timeout(socket: *mut SocketRecord, send: bool, timeout: f64) -> bool {
    // SAFETY: as the caller promises.
    if unsafe { crate::swr_v1_socket_set_kernel_timeout(socket, send, timeout) } {
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
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket23set_kernel_read_timeoutEd")
)]
pub unsafe extern "C" fn socket_set_kernel_read_timeout(
    socket: *mut SocketRecord,
    timeout: f64,
) -> bool {
    // SAFETY: as the caller promises.
    unsafe { set_kernel_timeout(socket, true, timeout) }
}

/// `bool Socket::set_kernel_write_timeout(double)`: upstream sets `SO_RCVTIMEO`.
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket24set_kernel_write_timeoutEd")
)]
pub unsafe extern "C" fn socket_set_kernel_write_timeout(
    socket: *mut SocketRecord,
    timeout: f64,
) -> bool {
    // SAFETY: as the caller promises.
    unsafe { set_kernel_timeout(socket, false, timeout) }
}

/// `cork` or `uncork`: the TCP flags in and out of the record, and the warnings.
///
/// # Safety
/// `socket` is a live record.
unsafe fn cork_as(socket: *mut SocketRecord, cork: bool) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let mut state = crate::CorkState {
        tcp_nopush: socket.tcp_nopush,
        tcp_nodelay: socket.tcp_nodelay,
    };
    let result = if cork {
        // SAFETY: `state` is a local.
        unsafe { crate::swr_v1_socket_cork(socket.fd, &raw mut state) }
    } else {
        // SAFETY: `state` is a local.
        unsafe { crate::swr_v1_socket_uncork(socket.fd, socket.enable_tcp_nodelay, &raw mut state) }
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
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket4corkEv")
)]
pub unsafe extern "C" fn socket_cork(socket: *mut SocketRecord) -> bool {
    // SAFETY: as the caller promises.
    unsafe { cork_as(socket, true) }
}

/// `bool Socket::uncork()`.
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket6uncorkEv")
)]
pub unsafe extern "C" fn socket_uncork(socket: *mut SocketRecord) -> bool {
    // SAFETY: as the caller promises.
    unsafe { cork_as(socket, false) }
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
    // SAFETY: `unknown` is a local.
    let socket =
        unsafe { crate::swr_v1_socket_make_typed(socket_type, fd_type, flags, &raw mut unknown) };
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
    let socket =
        crate::swr_v1_socket_make_open(socket_type, fd_type, domain, kind, protocol, flags);
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
    crate::swr_v1_socket_make(fd, fd_type)
}

/// `int socket(int domain, int type, int protocol, int flags)`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6socketEiiii")
)]
pub extern "C" fn socket(domain: c_int, kind: c_int, protocol: c_int, flags: c_int) -> c_int {
    crate::swr_v1_socket_open(domain, kind, protocol, flags)
}

/// `Reactor::change`'s log: `code` 0 is `swoole_sys_warning`, else
/// `swoole_error_log(SW_LOG_WARNING, code, ...)`, as from `origin`; then the backtrace.
///
/// # Safety
/// Both strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_log(
    origin: *const c_char,
    code: c_int,
    message: *const c_char,
) {
    // SAFETY: as the caller promises.
    let origin = unsafe { CStr::from_ptr(origin) };
    // SAFETY: as the caller promises.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    if code == 0 {
        log::sys_warning(origin, format_args!("{message}"));
    } else {
        log::error_log(log::LOG_WARNING, code, origin, format_args!("{message}"));
    }
    swoole_rs_corebase::swoole_print_backtrace_on_error();
}

/// The thread's event loop while it is available (`SwooleTG.reactor`, unless null
/// or destroyed), with the prepared state it begins with. native/reactor/reactor.cc reads
/// the C++ `thread_local`.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub fn event_reactor() -> Option<(*mut std::ffi::c_void, crate::ReactorPrepared)> {
    // SAFETY: native/reactor/reactor.cc's.
    unsafe extern "C" {
        fn swr_v1_event_reactor() -> *mut std::ffi::c_void;
    }
    // SAFETY: no preconditions.
    let reactor = unsafe { swr_v1_event_reactor() };
    // SAFETY: null or a live `Reactor`, which begins with its prepared state.
    let prepared = unsafe { reactor.cast::<crate::ReactorPrepared>().as_ref() }.copied();
    prepared.map(|prepared| (reactor, prepared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn get_timeout_reads_one_timeout() {
        let mut socket = SocketRecord::new();
        socket.timeouts.read = 2.5;
        unsafe {
            assert_eq!(socket_get_timeout(&raw const socket, 1 << 2), 2.5);
            assert_eq!(socket_get_timeout(&raw const socket, 3), -1.0);
        }
    }

    #[test]
    fn wait_event_times_out_with_the_poll_error() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let socket = SocketRecord::with_fd(fds[0], 0);
        assert_eq!(unsafe { socket_wait_event(&raw const socket, 0, 0) }, ERR);
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
