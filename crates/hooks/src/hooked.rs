//! The hooked socket functions of upstream's `src/coroutine/hook.cc`
//! (`swoole_coroutine_socket`, `send`, `recv`, `connect`, `accept`, `poll`,
//! `close`, `read`, `write`, …), exported under their upstream names. Inside a
//! coroutine an fd the socket map holds goes to its native socket through
//! [`SocketOps`]; any other call goes to libc, the async pool or `io_uring`, as
//! upstream routes it.
use crate::co_io::set_errno;
use crate::co_socket::CoSocket;
use crate::fd_control::{
    swr_v1_hook_fd_poll, swr_v1_hook_fd_poll_fake, swr_v1_hook_fd_timeout, swr_v1_hook_fd_wait,
    swr_v1_hook_fd_wait_restore,
};
use crate::lifecycle::swr_v1_hook_co_socket_close;
use crate::poll::swr_v1_hook_poll;
use crate::socket_map::{
    swr_v1_hook_socket_close, swr_v1_hook_socket_exists, swr_v1_hook_socket_get,
    swr_v1_hook_socket_insert, swr_v1_hook_socket_remove,
};
use crate::socket_members::record;
use crate::stdio::{Host, coroutine, offloaded, swr_v1_hook_close_fd};
use libc::{c_int, c_void, msghdr, nfds_t, pollfd, size_t, sockaddr, socklen_t, ssize_t};
use swoole_rs_corebase::log;
use swoole_rs_reactor::{SocketRecord, swr_v1_socket_set_fd_option};

/// `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: c_int = 505;

/// The native coroutine socket, `SocketImpl` (`coroutine::Socket`, or `UringSocket`
/// with `--enable-uring-socket`): its constructors, `delete` and the calls whose
/// body depends on the class.
#[repr(C)]
pub struct SocketOps {
    /// `new SocketImpl(domain, type, protocol)`.
    pub open: unsafe extern "C" fn(c_int, c_int, c_int) -> *mut c_void,
    /// `new SocketImpl(fd, SW_SOCK_RAW)`.
    pub wrap: unsafe extern "C" fn(c_int) -> *mut c_void,
    /// `delete`.
    pub free: unsafe extern "C" fn(*mut c_void),
    pub send: unsafe extern "C" fn(*mut c_void, *const c_void, size_t) -> ssize_t,
    pub recv: unsafe extern "C" fn(*mut c_void, *mut c_void, size_t) -> ssize_t,
    pub peek: unsafe extern "C" fn(*mut c_void, *mut c_void, size_t) -> ssize_t,
    pub write: unsafe extern "C" fn(*mut c_void, *const c_void, size_t) -> ssize_t,
    pub read: unsafe extern "C" fn(*mut c_void, *mut c_void, size_t) -> ssize_t,
    pub sendmsg: unsafe extern "C" fn(*mut c_void, *const msghdr, c_int) -> ssize_t,
    pub recvmsg: unsafe extern "C" fn(*mut c_void, *mut msghdr, c_int) -> ssize_t,
    pub connect: unsafe extern "C" fn(*mut c_void, *const sockaddr, socklen_t) -> bool,
    /// `accept(0)`: the accepted socket, or null.
    pub accept: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    /// `poll(event)` with the socket's own timeout.
    pub poll: unsafe extern "C" fn(*mut c_void, u32) -> bool,
}

/// `get_socket_ex`: inside a coroutine, the host and `fd`'s socket. Otherwise
/// none, with errno `EWOULDBLOCK` outside a coroutine or `ENOTSOCK` without a socket.
fn hooked(fd: c_int) -> Option<(&'static Host, *mut c_void)> {
    let Some(host) = coroutine() else {
        set_errno(libc::EWOULDBLOCK);
        return None;
    };
    let socket = swr_v1_hook_socket_get(fd);
    (!socket.is_null()).then_some((host, socket))
}

/// The record of the native socket at `socket`, or null.
fn base(socket: Option<(&Host, *mut c_void)>) -> *mut CoSocket {
    socket.map_or(core::ptr::null_mut(), |(_, socket)| record(socket))
}

/// The `network::Socket` of the live native socket at `socket`.
///
/// # Safety
/// `socket` is a live native socket.
unsafe fn network(socket: *mut c_void) -> *mut SocketRecord {
    // SAFETY: as the caller promises.
    unsafe { (*record(socket)).socket }
}

/// The live native socket's fd.
///
/// # Safety
/// `socket` is a live native socket.
unsafe fn fd_of(socket: *mut c_void) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { (*record(socket)).sock_fd }
}

/// Map `fd` to the live `socket`, deleting the socket the map lets go.
///
/// # Safety
/// `socket` is a live native socket the map now owns.
unsafe fn keep(host: &Host, fd: c_int, socket: *mut c_void, replace: bool) {
    let released = swr_v1_hook_socket_insert(fd, socket, replace);
    if !released.is_null() {
        // SAFETY: the map no longer holds it.
        unsafe { (host.socket.free)(released) };
    }
}

/// A new native socket's fd once the map holds it, or -1 (deleting it) if it has none.
///
/// # Safety
/// `socket` is a new native socket.
unsafe fn adopt(host: &Host, socket: *mut c_void) -> c_int {
    // SAFETY: as the caller promises.
    let fd = unsafe { fd_of(socket) };
    if fd < 0 {
        // SAFETY: nothing else holds it.
        unsafe { (host.socket.free)(socket) };
        return -1;
    }
    // SAFETY: as above.
    unsafe { keep(host, fd, socket, true) };
    fd
}

/// The ring's host when `io_uring` takes the call, as upstream's `SW_USE_IOURING`.
#[cfg(feature = "iouring")]
fn ring(host: &Host) -> Option<*const crate::uring::Host> {
    (!host.uring.is_null()).then_some(host.uring.cast())
}

/// Hooked `socket(2)`.
///
/// # Safety
/// As for `socket`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_socket(
    domain: c_int,
    kind: c_int,
    protocol: c_int,
) -> c_int {
    let Some(host) = coroutine() else {
        // SAFETY: as the caller promises.
        return unsafe { libc::socket(domain, kind, protocol) };
    };
    // SAFETY: native's constructor.
    let socket = unsafe { (host.socket.open)(domain, kind, protocol) };
    // SAFETY: the new socket.
    unsafe { adopt(host, socket) }
}

/// Hooked `send(2)`.
///
/// # Safety
/// As for `send`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_send(
    fd: c_int,
    buf: *const c_void,
    len: size_t,
    flags: c_int,
) -> ssize_t {
    match hooked(fd) {
        // SAFETY: the map's live socket; the caller's buffer.
        Some((host, socket)) => unsafe { (host.socket.send)(socket, buf, len) },
        // SAFETY: as the caller promises.
        None => unsafe { libc::send(fd, buf, len, flags) },
    }
}

/// Hooked `sendmsg(2)`.
///
/// # Safety
/// As for `sendmsg`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_sendmsg(
    fd: c_int,
    msg: *const msghdr,
    flags: c_int,
) -> ssize_t {
    match hooked(fd) {
        // SAFETY: the map's live socket; the caller's message.
        Some((host, socket)) => unsafe { (host.socket.sendmsg)(socket, msg, flags) },
        // SAFETY: as the caller promises.
        None => unsafe { libc::sendmsg(fd, msg, flags) },
    }
}

/// Hooked `recvmsg(2)`.
///
/// # Safety
/// As for `recvmsg`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_recvmsg(
    fd: c_int,
    msg: *mut msghdr,
    flags: c_int,
) -> ssize_t {
    match hooked(fd) {
        // SAFETY: the map's live socket; the caller's message.
        Some((host, socket)) => unsafe { (host.socket.recvmsg)(socket, msg, flags) },
        // SAFETY: as the caller promises.
        None => unsafe { libc::recvmsg(fd, msg, flags) },
    }
}

/// Hooked `recv(2)`; `MSG_PEEK` peeks, other flags are the socket's.
///
/// # Safety
/// As for `recv`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_recv(
    fd: c_int,
    buf: *mut c_void,
    len: size_t,
    flags: c_int,
) -> ssize_t {
    let Some((host, socket)) = hooked(fd) else {
        // SAFETY: as the caller promises.
        return unsafe { libc::recv(fd, buf, len, flags) };
    };
    let call = if flags & libc::MSG_PEEK != 0 {
        host.socket.peek
    } else {
        host.socket.recv
    };
    // SAFETY: the map's live socket; the caller's buffer.
    unsafe { call(socket, buf, len) }
}

/// Hooked `connect(2)`.
///
/// # Safety
/// As for `connect`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_connect(
    fd: c_int,
    addr: *const sockaddr,
    len: socklen_t,
) -> c_int {
    match hooked(fd) {
        Some((host, socket)) => {
            // SAFETY: the map's live socket; the caller's address.
            if unsafe { (host.socket.connect)(socket, addr, len) } {
                0
            } else {
                -1
            }
        }
        // SAFETY: as the caller promises.
        None => unsafe { libc::connect(fd, addr, len) },
    }
}

/// Hooked `accept(2)`: the accepted socket joins the map unless its fd has one.
///
/// # Safety
/// As for `accept`, except that inside a coroutine `addr` and `len` are written
/// unconditionally, as upstream does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_accept(
    fd: c_int,
    addr: *mut sockaddr,
    len: *mut socklen_t,
) -> c_int {
    let Some((host, socket)) = hooked(fd) else {
        // SAFETY: as the caller promises.
        return unsafe { libc::accept(fd, addr, len) };
    };
    // SAFETY: the map's live socket.
    let conn = unsafe { (host.socket.accept)(socket) };
    if conn.is_null() {
        return -1;
    }
    // SAFETY: the new socket.
    let accepted = unsafe { network(conn) };
    // SAFETY: its network::Socket, which accept named.
    let info = unsafe { &(*accepted).info };
    // SAFETY: the caller's length slot.
    unsafe { *len = info.len };
    // SAFETY: info.len bytes of the address, into the caller's buffer, which upstream assumes holds them.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&raw const info.addr).cast::<u8>(),
            addr.cast(),
            info.len as usize,
        );
    }
    // SAFETY: the new socket.
    let conn_fd = unsafe { fd_of(conn) };
    // SAFETY: as above; the map now owns it.
    unsafe { keep(host, conn_fd, conn, false) };
    conn_fd
}

/// Hooked `poll(2)` of one fd that only reports it ready, after making `timeout`
/// its socket's timeout; libc's `poll` without a socket or a timeout.
///
/// # Safety
/// As for `poll`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_poll_fake(
    fds: *mut pollfd,
    nfds: nfds_t,
    timeout: c_int,
) -> c_int {
    if nfds != 1 {
        if let Some(host) = crate::stdio::host() {
            // SAFETY: native's last-error slot.
            unsafe { (host.set_last_error)(INVALID_PARAMS) };
        }
        log::warning(
            c"swoole_coroutine_poll_fake",
            format_args!("fake poll() implementation, only supports one fd"),
        );
        return -1;
    }
    // SAFETY: one pollfd, as the caller promises.
    let socket = base(hooked(unsafe { (*fds).fd }));
    // SAFETY: as above; the map's live socket or null.
    match unsafe { swr_v1_hook_fd_poll_fake(fds, timeout, socket) } {
        // SAFETY: as the caller promises.
        2 => unsafe { libc::poll(fds, nfds, timeout) },
        result => result,
    }
}

/// Hooked `poll(2)`: inside a coroutine with a timeout, the coroutine waits on the reactor.
///
/// # Safety
/// As for `poll`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_poll(
    fds: *mut pollfd,
    nfds: nfds_t,
    timeout: c_int,
) -> c_int {
    let host = coroutine().filter(|_| timeout != 0);
    let Some(host) = host else {
        // SAFETY: as the caller promises.
        return unsafe { libc::poll(fds, nfds, timeout) };
    };
    #[cfg(feature = "iouring")]
    if let Some(ring) = ring(host).filter(|_| nfds == 1) {
        // SAFETY: the ring's host; one pollfd.
        return unsafe { crate::uring::swr_v1_uring_poll(ring, fds, timeout) };
    }
    // SAFETY: native's static poll table.
    let ops = unsafe { &*host.poll };
    // SAFETY: as the caller promises, in a coroutine.
    unsafe { swr_v1_hook_poll(fds, nfds, timeout, ops) }
}

/// Wrap the open `fd` in a nonblocking coroutine socket: 0, or -1 outside a coroutine.
///
/// # Safety
/// `fd` is open and becomes the socket's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_socket_create(fd: c_int) -> c_int {
    let Some(host) = coroutine() else { return -1 };
    // SAFETY: native's constructor.
    let socket = unsafe { (host.socket.wrap)(fd) };
    // SAFETY: the new socket.
    if unsafe { adopt(host, socket) } < 0 {
        return -1;
    }
    // SAFETY: the map's live socket.
    let wrapped = unsafe { network(socket) };
    // SAFETY: its network::Socket.
    unsafe { swr_v1_socket_set_fd_option(wrapped, 1, -1) };
    0
}

/// Take `fd`'s socket out of the map and delete it, leaving `fd` open: 0, or -1.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_unwrap(fd: c_int) -> c_int {
    let Some(host) = coroutine() else { return -1 };
    let socket = swr_v1_hook_socket_remove(fd);
    if socket.is_null() {
        return -1;
    }
    // SAFETY: the removed live socket: `move_fd`, so its deletion leaves the fd open.
    unsafe {
        (*record(socket)).sock_fd = -1;
    }
    // SAFETY: as above.
    let unwrapped = unsafe { network(socket) };
    // SAFETY: its network::Socket.
    unsafe { (*unwrapped).fd = -1 };
    // SAFETY: the map let go of it.
    unsafe { (host.socket.free)(socket) };
    0
}

/// Whether `fd` has a coroutine socket.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_exists(fd: c_int) -> u8 {
    swr_v1_hook_socket_exists(fd).into()
}

/// `SO_RCVTIMEO` or `SO_SNDTIMEO` on `fd`'s socket: 0, or -1 with errno `EINVAL`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_set_timeout(
    fd: c_int,
    which: c_int,
    timeout: f64,
) -> c_int {
    // SAFETY: the map's live socket or null.
    unsafe { swr_v1_hook_fd_timeout(base(hooked(fd)), which, timeout, false) }
}

/// The DNS and connect timeouts of `fd`'s socket: 0, or -1 with errno `EINVAL`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_set_connect_timeout(fd: c_int, timeout: f64) -> c_int {
    // SAFETY: the map's live socket or null.
    unsafe { swr_v1_hook_fd_timeout(base(hooked(fd)), 0, timeout, true) }
}

/// Wait up to `timeout` for `event` on `fd`: `SW_OK` (0) or `SW_ERR` (-1).
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_wait_event(
    fd: c_int,
    event: c_int,
    timeout: f64,
) -> c_int {
    let Some((host, socket)) = hooked(fd) else {
        let mut poll = swr_v1_hook_fd_poll(fd, event);
        #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) cast")]
        // SAFETY: one pollfd.
        let ready = unsafe { libc::poll(&raw mut poll, 1, (timeout * 1000.0) as c_int) };
        return if ready == 1 { 0 } else { -1 };
    };
    // SAFETY: the map's live socket.
    let wait = unsafe { swr_v1_hook_fd_wait(record(socket), event.cast_unsigned(), timeout) };
    // SAFETY: as above.
    let ready = unsafe { (host.socket.poll)(socket, event.cast_unsigned()) };
    // SAFETY: the preparation's result; the socket is still live.
    unsafe { swr_v1_hook_fd_wait_restore(wait) };
    if ready { 0 } else { -1 }
}

/// `Socket::close()` of a native socket from the map.
unsafe extern "C" fn close_socket(socket: *mut c_void) -> bool {
    // SAFETY: the map's live socket.
    unsafe { swr_v1_hook_co_socket_close(record(socket)) }
}

/// Hooked `close(2)`: a coroutine socket closes and leaves the map; any other fd
/// closes on the pool or the ring.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_close(fd: c_int) -> c_int {
    let Some(host) = coroutine() else {
        // SAFETY: closing an fd touches no memory.
        return unsafe { libc::close(fd) };
    };
    // SAFETY: both act on live sockets from the map.
    let result = unsafe { swr_v1_hook_socket_close(fd, close_socket, host.socket.free) };
    if result <= 0 {
        return result;
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = ring(host) {
        // SAFETY: the ring's host.
        return unsafe { crate::uring::swr_v1_uring_close_fd(ring, fd) };
    }
    // SAFETY: native `async`, in a coroutine.
    unsafe { swr_v1_hook_close_fd(fd, Some(host.offload)) }
}

/// Hooked `read(2)`: a coroutine socket reads; any other fd reads on the ring, or
/// waits for readiness and reads on the pool.
///
/// # Safety
/// As for `read`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_read(
    fd: c_int,
    buf: *mut c_void,
    count: size_t,
) -> ssize_t {
    let Some(host) = coroutine() else {
        // SAFETY: as the caller promises.
        return unsafe { libc::read(fd, buf, count) };
    };
    let socket = swr_v1_hook_socket_get(fd);
    if !socket.is_null() {
        // SAFETY: the map's live socket; the caller's buffer.
        return unsafe { (host.socket.read)(socket, buf, count) };
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = ring(host) {
        // SAFETY: the ring's host; the caller's buffer.
        return unsafe { crate::uring::swr_v1_uring_read(ring, fd, buf, count, -1.0) };
    }
    let call = || {
        let mut socket = blocking(fd, false);
        // SAFETY: a record on this frame; the caller's buffer.
        unsafe { swoole_rs_tls::upstream::socket_read_sync(&raw mut socket, buf, count) }
    };
    // SAFETY: native `async`, in a coroutine; the job only touches its record and the buffer.
    unsafe { offloaded(Some(host.offload), -1, call) }
}

/// Hooked `write(2)`, as [`swoole_coroutine_read`].
///
/// # Safety
/// As for `write`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_write(
    fd: c_int,
    buf: *const c_void,
    count: size_t,
) -> ssize_t {
    let Some(host) = coroutine() else {
        // SAFETY: as the caller promises.
        return unsafe { libc::write(fd, buf, count) };
    };
    let socket = swr_v1_hook_socket_get(fd);
    if !socket.is_null() {
        // SAFETY: the map's live socket; the caller's buffer.
        return unsafe { (host.socket.write)(socket, buf, count) };
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = ring(host) {
        // SAFETY: the ring's host; the caller's buffer.
        return unsafe { crate::uring::swr_v1_uring_write(ring, fd, buf, count, -1.0) };
    }
    let call = || {
        let mut socket = blocking(fd, true);
        // SAFETY: a record on this frame; the caller's buffer.
        unsafe { swoole_rs_tls::upstream::socket_write_sync(&raw mut socket, buf, count) }
    };
    // SAFETY: native `async`, in a coroutine; the job only touches its record and the buffer.
    unsafe { offloaded(Some(host.offload), -1, call) }
}

/// The pool's `network::Socket` over `fd`: nonblocking, waiting without limit.
fn blocking(fd: c_int, write: bool) -> SocketRecord {
    let mut socket = SocketRecord::new();
    socket.fd = fd;
    socket.output.nonblock = 1;
    if write {
        socket.timeouts.write = -1.0;
    } else {
        socket.timeouts.read = -1.0;
    }
    socket
}

/// `SocketImpl *swoole_coroutine_get_socket_object(int)`: `fd`'s socket, or null.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z34swoole_coroutine_get_socket_objecti")
)]
#[must_use]
pub extern "C" fn swoole_coroutine_get_socket_object(fd: c_int) -> *mut c_void {
    swr_v1_hook_socket_get(fd)
}

/// `SocketImpl *swoole_coroutine_get_socket_object_ex(int)`: as
/// [`swoole_coroutine_get_socket_object`], but null outside a coroutine.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z37swoole_coroutine_get_socket_object_exi")
)]
#[must_use]
pub extern "C" fn swoole_coroutine_get_socket_object_ex(fd: c_int) -> *mut c_void {
    hooked(fd).map_or(core::ptr::null_mut(), |(_, socket)| socket)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::stdio::tests::{LAST_ERROR, Mode, with};
    use std::cell::RefCell;

    /// A native socket: the vtable pointer, then the record, as `socket_members` reads it.
    #[repr(C)]
    struct Fake {
        vtable: usize,
        record: CoSocket,
        network: SocketRecord,
    }

    thread_local! {
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn call(name: String) {
        CALLS.with_borrow_mut(|calls| calls.push(name));
    }

    fn calls() -> Vec<String> {
        CALLS.with_borrow_mut(std::mem::take)
    }

    fn fake(fd: c_int) -> *mut c_void {
        // Every field is a scalar, raw pointer or Protocol of those; zero is valid.
        let mut socket: Box<Fake> = Box::new(unsafe { core::mem::zeroed() });
        socket.network = SocketRecord::new();
        socket.network.fd = fd;
        socket.record.sock_fd = fd;
        socket.record.socket = &raw mut socket.network;
        Box::into_raw(socket).cast()
    }

    unsafe extern "C" fn open(domain: c_int, kind: c_int, _: c_int) -> *mut c_void {
        call(format!("open {domain} {kind}"));
        fake(if kind < 0 { -1 } else { 9000 + kind })
    }
    unsafe extern "C" fn wrap(fd: c_int) -> *mut c_void {
        call(format!("wrap {fd}"));
        fake(fd)
    }
    unsafe extern "C" fn free(socket: *mut c_void) {
        let socket = unsafe { Box::from_raw(socket.cast::<Fake>()) };
        call(format!(
            "free {} {}",
            socket.record.sock_fd, socket.network.fd
        ));
    }
    unsafe extern "C" fn send(_: *mut c_void, _: *const c_void, n: size_t) -> ssize_t {
        call(format!("send {n}"));
        1
    }
    unsafe extern "C" fn recv(_: *mut c_void, _: *mut c_void, n: size_t) -> ssize_t {
        call(format!("recv {n}"));
        2
    }
    unsafe extern "C" fn peek(_: *mut c_void, _: *mut c_void, n: size_t) -> ssize_t {
        call(format!("peek {n}"));
        3
    }
    unsafe extern "C" fn write(_: *mut c_void, _: *const c_void, n: size_t) -> ssize_t {
        call(format!("write {n}"));
        4
    }
    unsafe extern "C" fn read(_: *mut c_void, _: *mut c_void, n: size_t) -> ssize_t {
        call(format!("read {n}"));
        5
    }
    unsafe extern "C" fn sendmsg(_: *mut c_void, _: *const msghdr, flags: c_int) -> ssize_t {
        call(format!("sendmsg {flags}"));
        6
    }
    unsafe extern "C" fn recvmsg(_: *mut c_void, _: *mut msghdr, flags: c_int) -> ssize_t {
        call(format!("recvmsg {flags}"));
        7
    }
    unsafe extern "C" fn connect(_: *mut c_void, _: *const sockaddr, len: socklen_t) -> bool {
        call(format!("connect {len}"));
        len > 0
    }
    unsafe extern "C" fn accept(socket: *mut c_void) -> *mut c_void {
        let fd = unsafe { fd_of(socket) };
        call(format!("accept {fd}"));
        let conn = fake(fd + 1);
        let info = unsafe { &mut (*network(conn)).info };
        info.len = 4;
        info.addr.ss.sa_family = 7;
        conn
    }
    unsafe extern "C" fn poll(socket: *mut c_void, event: u32) -> bool {
        let read = unsafe { (*network(socket)).timeouts.read };
        call(format!("poll {event} {read}"));
        true
    }

    /// The test host's sockets.
    pub(crate) const OPS: SocketOps = SocketOps {
        open,
        wrap,
        free,
        send,
        recv,
        peek,
        write,
        read,
        sendmsg,
        recvmsg,
        connect,
        accept,
        poll,
    };

    #[test]
    fn a_coroutine_routes_mapped_fds_to_the_native_socket() {
        unsafe {
            with(Mode::Pool, || {
                let fd = swoole_coroutine_socket(2, 1, 0);
                assert_eq!(fd, 9001);
                assert_eq!(swoole_coroutine_socket_exists(fd), 1);
                let mut buf = [0u8; 8];
                assert_eq!(swoole_coroutine_send(fd, buf.as_ptr().cast(), 8, 0), 1);
                assert_eq!(swoole_coroutine_recv(fd, buf.as_mut_ptr().cast(), 8, 0), 2);
                assert_eq!(
                    swoole_coroutine_recv(fd, buf.as_mut_ptr().cast(), 8, libc::MSG_PEEK),
                    3
                );
                assert_eq!(swoole_coroutine_write(fd, buf.as_ptr().cast(), 8), 4);
                assert_eq!(swoole_coroutine_read(fd, buf.as_mut_ptr().cast(), 8), 5);
                assert_eq!(swoole_coroutine_sendmsg(fd, core::ptr::null(), 1), 6);
                assert_eq!(swoole_coroutine_recvmsg(fd, core::ptr::null_mut(), 2), 7);
                assert_eq!(swoole_coroutine_connect(fd, core::ptr::null(), 0), -1);
                assert_eq!(swoole_coroutine_connect(fd, core::ptr::null(), 16), 0);

                let mut addr: libc::sockaddr_storage = core::mem::zeroed();
                let mut len: socklen_t = 0;
                let conn = swoole_coroutine_accept(fd, (&raw mut addr).cast(), &raw mut len);
                assert_eq!((conn, len, addr.ss_family), (9002, 4, 7));
                assert_eq!(
                    swoole_coroutine_get_socket_object(conn),
                    swr_v1_hook_socket_get(conn)
                );

                // wait_event polls with the timeout in every direction, then restores it.
                let socket = swr_v1_hook_socket_get(fd);
                let read = (*network(socket)).timeouts.read;
                assert_eq!(swoole_coroutine_socket_wait_event(fd, 1 << 9, 0.5), 0);
                assert_eq!((*network(socket)).timeouts.read, read);
                assert_eq!(
                    swoole_coroutine_socket_set_timeout(fd, libc::SO_RCVTIMEO, 0.25),
                    0
                );
                assert_eq!((*network(socket)).timeouts.read, 0.25);
                assert_eq!(swoole_coroutine_socket_set_connect_timeout(fd, 2.0), 0);
                assert_eq!((*network(socket)).timeouts.connect, 2.0);

                // unwrap leaves the fds open: the deleted sockets no longer own them.
                assert_eq!(swoole_coroutine_socket_unwrap(conn), 0);
                assert_eq!(swoole_coroutine_socket_unwrap(fd), 0);
                assert_eq!(swoole_coroutine_socket_unwrap(fd), -1);
                assert_eq!(swoole_coroutine_socket_exists(fd), 0);
            });
        }
        assert_eq!(
            calls(),
            [
                "open 2 1",
                "send 8",
                "recv 8",
                "peek 8",
                "write 8",
                "read 8",
                "sendmsg 1",
                "recvmsg 2",
                "connect 0",
                "connect 16",
                "accept 9001",
                "poll 512 0.5",
                "free -1 -1",
                "free -1 -1",
            ]
        );
    }

    #[test]
    fn sockets_without_an_fd_are_deleted_and_wrapped_fds_are_nonblocking() {
        let mut pair = [0; 2];
        unsafe {
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()),
                0
            );
            with(Mode::Pool, || {
                assert_eq!(swoole_coroutine_socket(2, -1, 0), -1);
                assert_eq!(swoole_coroutine_socket_create(pair[0]), 0);
                assert_ne!(libc::fcntl(pair[0], libc::F_GETFL) & libc::O_NONBLOCK, 0);
                assert_eq!(swoole_coroutine_socket_unwrap(pair[0]), 0);
            });
            libc::close(pair[0]);
            libc::close(pair[1]);
        }
        let wrapped = format!("wrap {}", pair[0]);
        assert_eq!(
            calls(),
            ["open 2 -1", "free -1 -1", wrapped.as_str(), "free -1 -1"]
        );
    }

    #[test]
    fn outside_a_coroutine_calls_go_to_libc() {
        let mut pair = [0; 2];
        unsafe {
            with(Mode::Here, || {
                let fd = swoole_coroutine_socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                assert!(fd >= 0);
                assert_eq!(swoole_coroutine_close(fd), 0);
                assert_eq!(swoole_coroutine_socket_create(3), -1);
                assert_eq!(swoole_coroutine_socket_unwrap(3), -1);
                assert!(swoole_coroutine_get_socket_object_ex(3).is_null());
                assert_eq!(crate::co_io::errno(), libc::EWOULDBLOCK);

                assert_eq!(
                    libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()),
                    0
                );
                assert_eq!(
                    swoole_coroutine_send(pair[0], b"hi".as_ptr().cast(), 2, 0),
                    2
                );
                assert_eq!(swoole_coroutine_socket_wait_event(pair[1], 1 << 9, 1.0), 0);
                let mut buf = [0u8; 2];
                assert_eq!(
                    swoole_coroutine_recv(pair[1], buf.as_mut_ptr().cast(), 2, 0),
                    2
                );
                assert_eq!(
                    swoole_coroutine_socket_wait_event(pair[1], 1 << 9, 0.01),
                    -1
                );
                assert_eq!(swoole_coroutine_close(pair[0]), 0);
                assert_eq!(swoole_coroutine_close(pair[1]), 0);
            });
        }
        assert_eq!(calls(), Vec::<String>::new());
    }

    #[test]
    fn an_unmapped_fd_reads_and_writes_on_the_pool() {
        let mut pair = [0; 2];
        unsafe {
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()),
                0
            );
            with(Mode::Pool, || {
                assert_eq!(
                    swoole_coroutine_write(pair[0], b"hey".as_ptr().cast(), 3),
                    3
                );
                let mut buf = [0u8; 3];
                assert_eq!(
                    swoole_coroutine_read(pair[1], buf.as_mut_ptr().cast(), 3),
                    3
                );
                assert_eq!(&buf, b"hey");
                // Not a coroutine socket: the fd closes on the pool.
                assert_eq!(swoole_coroutine_close(pair[0]), 0);
                assert_eq!(swoole_coroutine_close(pair[1]), 0);
            });
            let mut buf = [0u8; 3];
            assert_eq!(
                with(Mode::Refuse, || swoole_coroutine_read(
                    pair[1],
                    buf.as_mut_ptr().cast(),
                    3
                )),
                -1
            );
        }
        assert_eq!(calls(), Vec::<String>::new());
    }

    #[test]
    fn fake_poll_takes_one_fd() {
        let mut pipe = [0; 2];
        unsafe {
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
            let mut fds = [pollfd {
                fd: pipe[1],
                events: libc::POLLOUT,
                revents: 0,
            }; 2];
            with(Mode::Pool, || {
                assert_eq!(swoole_coroutine_poll_fake(fds.as_mut_ptr(), 2, 10), -1);
                assert_eq!(LAST_ERROR.get(), INVALID_PARAMS);
                // No socket: libc's poll.
                assert_eq!(swoole_coroutine_poll_fake(fds.as_mut_ptr(), 1, 10), 1);
                assert_eq!(fds[0].revents, libc::POLLOUT);
                // Without a timeout, poll is libc's even in a coroutine.
                assert_eq!(swoole_coroutine_poll(fds.as_mut_ptr(), 1, 0), 1);
            });
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }
    }
}
