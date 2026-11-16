//! The hooked socket functions of upstream's `src/coroutine/hook.cc`
//! (`swoole_coroutine_socket`, `poll`, `close`, `read`, `write`, …),
//! exported under their upstream names for the C that calls them, and those only
//! Rust calls ([`send`], [`recv`], [`connect`],
//! [`accept`], [`poll_fake`], [`sendmsg`], [`recvmsg`]). Inside a
//! coroutine an fd the socket map holds goes to its native socket through
//! its Rust members; any other call goes to libc, the async pool or `io_uring`, as
//! upstream routes it.
use crate::co_io::set_errno;
use crate::co_socket::{Co, OwnedCo};
use crate::fd_control::{fd_poll_fake, fd_timeout, fd_wait, swr_v1_hook_fd_poll};
use crate::socket_map;
use crate::stdio::{Host, coroutine, on_pool, swr_v1_hook_close_fd};
pub use crate::vector::{RecvMessage, SendMessage};
use core::mem::MaybeUninit;
/// `struct pollfd`, as [`poll_fake`] takes it.
pub use libc::pollfd;
use libc::{c_int, c_void, nfds_t, size_t, ssize_t};
use std::io;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::{log, sys};
use swoole_rs_reactor::{Socket, SocketRecord};

/// `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: c_int = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as c_int;

/// How the hooked fds' sockets (`SocketImpl`: `coroutine::Socket`, or `UringSocket`
/// with `--enable-uring-socket`) are made: Rust's own (`co_socket`), or the unit
/// tests' recording ones. Each is the result's, deleted as it drops. Their members
/// are Rust (`socket_members`, `lifecycle`) and pick the ring themselves.
pub(crate) struct SocketOps {
    /// `new SocketImpl(domain, type, protocol)`.
    pub open: fn(c_int, c_int, c_int) -> OwnedCo,
    /// `new SocketImpl(fd, SW_SOCK_RAW)`.
    pub wrap: fn(c_int) -> OwnedCo,
}

/// `SW_SOCK_RAW`.
const SOCK_RAW: c_int = 7;

/// The sockets the hooks make.
const SOCKETS: SocketOps = SocketOps {
    open: |domain, kind, protocol| crate::co_socket::owned(-1, domain, kind, protocol, true),
    wrap: |fd| crate::co_socket::owned(fd, 0, SOCK_RAW, 0, false),
};

/// [`SOCKETS`], or the unit tests' recording fakes.
pub(crate) fn sockets() -> &'static SocketOps {
    #[cfg(test)]
    if let Some(ops) = tests::SOCKETS {
        return ops;
    }
    &SOCKETS
}

/// `get_socket_ex`: inside a coroutine, the host and `fd`'s socket. Otherwise
/// none, with errno `EWOULDBLOCK` outside a coroutine or `ENOTSOCK` without a socket.
fn hooked(fd: c_int) -> Option<(&'static Host, Co)> {
    let Some(host) = coroutine() else {
        set_errno(libc::EWOULDBLOCK);
        return None;
    };
    socket_map::get(fd).map(|socket| (host, socket))
}

/// `new SocketImpl(domain, type, protocol)`, which only the result holds.
pub(crate) fn open(domain: c_int, kind: c_int, protocol: c_int) -> OwnedCo {
    (sockets().open)(domain, kind, protocol)
}

/// Map `fd` to `socket`, deleting the socket the map lets go.
fn keep(fd: c_int, socket: OwnedCo, replace: bool) {
    drop(socket_map::insert(fd, socket, replace));
}

/// A new native socket's fd once the map holds it, or none (deleting it) if it has none.
fn adopt(socket: OwnedCo) -> Option<c_int> {
    let fd = socket.co().sock_fd();
    if fd < 0 {
        return None;
    }
    keep(fd, socket, true);
    Some(fd)
}

/// The ring's host when `io_uring` takes the call, as upstream's `SW_USE_IOURING`.
#[cfg(feature = "iouring")]
fn ring(host: &Host) -> Option<&'static crate::uring::Host> {
    host.uring
}

/// Hooked `socket(2)`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int {
    if coroutine().is_none() {
        return sys::socket(domain, kind, protocol).unwrap_or(-1);
    }
    adopt((sockets().open)(domain, kind, protocol)).unwrap_or(-1)
}

/// Hooked `send(2)` of `buf`.
///
/// # Errors
/// Returns the socket or syscall error, also left in errno.
pub fn send(fd: c_int, buf: &[u8], flags: c_int) -> io::Result<usize> {
    match hooked(fd) {
        Some((_, socket)) => socket.send(buf).ok_or_else(io::Error::last_os_error),
        None => sys::send(fd, buf, flags).ok_or_else(io::Error::last_os_error),
    }
}

/// Hooked `sendmsg(2)` of `msg`.
///
/// # Errors
/// Returns the socket or syscall error, also left in errno.
pub fn sendmsg(fd: c_int, msg: &SendMessage<'_>, flags: c_int) -> io::Result<usize> {
    match hooked(fd) {
        Some((_, socket)) => socket.sendmsg(msg, flags),
        None => msg.send(fd, flags),
    }
    .ok_or_else(io::Error::last_os_error)
}

/// Hooked `recvmsg(2)` into `msg`.
///
/// # Errors
/// Returns the socket or syscall error, also left in errno.
pub fn recvmsg(fd: c_int, msg: &mut RecvMessage<'_>, flags: c_int) -> io::Result<usize> {
    match hooked(fd) {
        Some((_, socket)) => socket.recvmsg(msg, flags),
        None => msg.recv(fd, flags),
    }
    .ok_or_else(io::Error::last_os_error)
}

/// Hooked `recv(2)` into `buf`; `MSG_PEEK` peeks, other flags are the socket's.
///
/// # Errors
/// Returns the socket or syscall error, also left in errno.
pub fn recv(fd: c_int, buf: &mut [u8], flags: c_int) -> io::Result<usize> {
    let Some((_, socket)) = hooked(fd) else {
        return sys::recv(fd, buf, flags).ok_or_else(io::Error::last_os_error);
    };
    if flags & libc::MSG_PEEK != 0 {
        socket.peek(buf)
    } else {
        socket.recv(buf)
    }
    .ok_or_else(io::Error::last_os_error)
}

/// Hooked `connect(2)` of `fd` to `address`.
///
/// # Errors
/// Returns the socket or syscall error, also left in errno.
pub fn connect(fd: c_int, address: &SocketAddress) -> io::Result<()> {
    let connected = match hooked(fd) {
        Some((_, socket)) => socket.connect_addr(address),
        None => address.connect(fd),
    };
    connected.then_some(()).ok_or_else(io::Error::last_os_error)
}

/// Hooked `accept(2)`, the peer's address into `peer`: the accepted socket joins the
/// map unless its fd has one.
///
/// # Errors
/// Returns the socket or syscall error, also left in errno.
pub fn accept(fd: c_int, peer: &mut SocketAddress) -> io::Result<c_int> {
    let Some((_, socket)) = hooked(fd) else {
        return peer
            .accept(fd)
            .map(std::os::fd::IntoRawFd::into_raw_fd)
            .ok_or_else(io::Error::last_os_error);
    };
    let conn = socket
        .accept_owned(0.0)
        .ok_or_else(io::Error::last_os_error)?;
    conn.co().net(|accepted| {
        peer.addr = accepted.info.addr;
        peer.len = accepted.info.len;
    });
    let conn_fd = conn.co().sock_fd();
    keep(conn_fd, conn, false);
    Ok(conn_fd)
}

/// Why the single-descriptor poll could not report readiness.
#[derive(Debug)]
pub enum FakePollError {
    /// The call did not contain exactly one descriptor.
    InvalidDescriptorCount,
    /// The fallback poll syscall failed.
    Io(io::Error),
}

/// Hooked `poll(2)` of one fd that only reports it ready, after making `timeout`
/// its socket's timeout; libc's `poll` without a socket or a timeout.
///
/// # Errors
/// Invalid descriptor counts set Swoole's last error; syscall errors leave errno.
pub fn poll_fake(fds: &mut [pollfd], timeout: c_int) -> Result<c_int, FakePollError> {
    let [fd] = fds else {
        (crate::stdio::runtime().set_last_error)(INVALID_PARAMS);
        log::warning(
            c"swoole_coroutine_poll_fake",
            format_args!("fake poll() implementation, only supports one fd"),
        );
        return Err(FakePollError::InvalidDescriptorCount);
    };
    let socket = hooked(fd.fd).map(|(_, socket)| socket);
    if fd_poll_fake(fd, timeout, socket) {
        Ok(1)
    } else {
        sys::poll(core::slice::from_mut(fd), timeout).map_err(FakePollError::Io)
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
    let fds: &mut [pollfd] = if nfds == 0 {
        &mut []
    } else if fds.is_null() {
        set_errno(libc::EFAULT);
        return -1;
    } else {
        // SAFETY: `nfds` pollfds, as the caller promises.
        unsafe { core::slice::from_raw_parts_mut(fds, usize::try_from(nfds).unwrap_or(0)) }
    };
    if timeout == 0 || coroutine().is_none() {
        return sys::poll(fds, timeout).unwrap_or(-1);
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = crate::stdio::host().and_then(ring)
        && let [fd] = fds
    {
        return crate::uring::poll(ring, fd, timeout).unwrap_or(-1);
    }
    crate::poll::poll(fds, timeout, &crate::poll::NATIVE).map_or(-1, CCast::c_cast::<c_int>)
}

/// Wrap the open `fd` in a nonblocking coroutine socket, which takes it: 0, or -1
/// outside a coroutine.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_create(fd: c_int) -> c_int {
    if coroutine().is_none() {
        return -1;
    }
    let owned = (sockets().wrap)(fd);
    // Live once mapped: the map holds it.
    let socket = owned.co();
    if adopt(owned).is_none() {
        return -1;
    }
    socket.net(|wrapped| wrapped.set_fd_option(1, -1));
    0
}

/// Take `fd`'s socket out of the map and delete it, leaving `fd` open: 0, or -1.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_unwrap(fd: c_int) -> c_int {
    if coroutine().is_none() {
        return -1;
    }
    let Some(unwrapped) = socket_map::remove(fd) else {
        return -1;
    };
    // `move_fd`, so its deletion leaves the fd open.
    unwrapped.co().set_sock_fd(-1);
    unwrapped.co().net(|record| record.fd = -1);
    drop(unwrapped);
    0
}

/// Whether `fd` has a coroutine socket.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_exists(fd: c_int) -> u8 {
    socket_map::exists(fd).into()
}

/// `SO_RCVTIMEO` or `SO_SNDTIMEO` on `fd`'s socket: 0, or -1 with errno `EINVAL`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_set_timeout(
    fd: c_int,
    which: c_int,
    timeout: f64,
) -> c_int {
    status(fd_timeout(
        hooked(fd).map(|(_, socket)| socket),
        which,
        timeout,
        false,
    ))
}

/// The DNS and connect timeouts of `fd`'s socket: 0, or -1 with errno `EINVAL`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_set_connect_timeout(fd: c_int, timeout: f64) -> c_int {
    status(fd_timeout(
        hooked(fd).map(|(_, socket)| socket),
        0,
        timeout,
        true,
    ))
}

/// `SW_OK` (0) or `SW_ERR` (-1).
fn status(done: bool) -> c_int {
    if done { 0 } else { -1 }
}

/// Wait up to `timeout` for `event` on `fd`: `SW_OK` (0) or `SW_ERR` (-1).
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_socket_wait_event(
    fd: c_int,
    event: c_int,
    timeout: f64,
) -> c_int {
    let Some((_, socket)) = hooked(fd) else {
        let mut poll = swr_v1_hook_fd_poll(fd, event);
        let ready = sys::poll(
            core::slice::from_mut(&mut poll),
            (timeout * 1000.0).c_cast::<c_int>(),
        );
        return if matches!(ready, Ok(1)) { 0 } else { -1 };
    };
    let event = swoole_rs_reactor::Events::from_bits(event);
    let saved = fd_wait(socket, event, timeout);
    let ready = socket.poll(event, 0.0);
    // The socket is still live: the map holds it.
    socket.set_timeout(saved, swoole_rs_reactor::TimeoutKind::ALL);
    if ready { 0 } else { -1 }
}

/// Hooked `close(2)`: a coroutine socket closes and leaves the map; any other fd
/// closes on the pool or the ring.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_close(fd: c_int) -> c_int {
    if coroutine().is_none() {
        return sys::close(fd).map_or(-1, |()| 0);
    }
    if let Some(closed) = socket_map::close(fd, Co::close) {
        return if closed { 0 } else { -1 };
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = crate::stdio::host().and_then(ring) {
        return crate::uring::close_fd(ring, fd).map_or(-1, |()| 0);
    }
    swr_v1_hook_close_fd(fd, Some(crate::stdio::runtime().offload))
}

/// Hooked `read(2)`, as [`read`].
///
/// # Safety
/// As for `read`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_read(
    fd: c_int,
    buf: *mut c_void,
    count: size_t,
) -> ssize_t {
    let buf: &mut [MaybeUninit<u8>] = if count == 0 {
        &mut []
    } else {
        // SAFETY: the caller's `count` writable bytes, which a read only writes, so
        // they may be uninitialized.
        unsafe { core::slice::from_raw_parts_mut(buf.cast(), count) }
    };
    read(fd, buf)
}

/// The hooked `read(2)` into `buf`: a coroutine socket reads; any other fd reads on
/// the ring, or waits for readiness and reads on the pool.
pub fn read(fd: c_int, buf: &mut [MaybeUninit<u8>]) -> ssize_t {
    if coroutine().is_none() {
        return sys::read(fd, buf).map_or(-1, usize::cast_signed);
    }
    if let Some(socket) = socket_map::get(fd) {
        return socket.read(buf).map_or(-1, usize::cast_signed);
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = crate::stdio::host().and_then(ring) {
        return crate::uring::read(ring, fd, buf, -1.0).map_or(-1, usize::cast_signed);
    }
    on_pool(Some(crate::stdio::runtime().offload), -1, || {
        let mut socket = blocking(fd, false);
        swoole_rs_tls::upstream::read_sync(Socket::new(&mut socket), buf)
            .map_or(-1, usize::cast_signed)
    })
}

/// Hooked `write(2)`, as [`write`].
///
/// # Safety
/// As for `write`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_write(
    fd: c_int,
    buf: *const c_void,
    count: size_t,
) -> ssize_t {
    let buf: &[u8] = if count == 0 {
        &[]
    } else {
        // SAFETY: the caller's `count` readable bytes.
        unsafe { core::slice::from_raw_parts(buf.cast(), count) }
    };
    write(fd, buf)
}

/// The hooked `write(2)` of `buf`, as [`read`].
#[must_use]
pub fn write(fd: c_int, buf: &[u8]) -> ssize_t {
    if coroutine().is_none() {
        return sys::write(fd, buf).map_or(-1, usize::cast_signed);
    }
    if let Some(socket) = socket_map::get(fd) {
        return socket.write(buf).map_or(-1, usize::cast_signed);
    }
    #[cfg(feature = "iouring")]
    if let Some(ring) = crate::stdio::host().and_then(ring) {
        return crate::uring::write(ring, fd, buf, -1.0).map_or(-1, usize::cast_signed);
    }
    on_pool(Some(crate::stdio::runtime().offload), -1, || {
        let mut socket = blocking(fd, true);
        swoole_rs_tls::upstream::write_sync(Socket::new(&mut socket), buf)
            .map_or(-1, usize::cast_signed)
    })
}

/// The pool's `network::Socket` over `fd`: nonblocking, waiting without limit.
fn blocking(fd: c_int, write: bool) -> SocketRecord {
    let mut socket = SocketRecord::new();
    socket.fd = fd;
    socket.output.nonblock = true;
    if write {
        socket.timeouts.write = -1.0;
    } else {
        socket.timeouts.read = -1.0;
    }
    socket
}

/// Upstream's `swoole_coroutine_get_socket_object`: `fd`'s socket, or null.
#[must_use]
pub fn swoole_coroutine_get_socket_object(fd: c_int) -> *mut c_void {
    socket_map::get(fd).map_or(core::ptr::null_mut(), Co::this)
}

/// Upstream's `swoole_coroutine_get_socket_object_ex`: as
/// [`swoole_coroutine_get_socket_object`], but null outside a coroutine.
#[must_use]
pub fn swoole_coroutine_get_socket_object_ex(fd: c_int) -> *mut c_void {
    hooked(fd).map_or(core::ptr::null_mut(), |(_, socket)| socket.this())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::stdio::tests::{LAST_ERROR, Mode, with};
    use std::cell::RefCell;

    thread_local! {
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn call(name: String) {
        CALLS.with_borrow_mut(|calls| calls.push(name));
    }

    fn calls() -> Vec<String> {
        CALLS.with_borrow_mut(std::mem::take)
    }

    /// A socket over `fd` that logs `free <sock_fd> <network fd>` as it is deleted.
    fn fake(fd: c_int) -> OwnedCo {
        let socket = crate::co_socket::over(fd);
        crate::socket_extra::set_close_hook(
            socket.co(),
            Box::new(|this| {
                let socket = unsafe { Co::new(this.cast()) };
                let net = socket.net(|record| record.fd).unwrap();
                call(format!("free {} {net}", socket.sock_fd()));
            }),
        );
        socket
    }

    fn open(domain: c_int, kind: c_int, _: c_int) -> OwnedCo {
        call(format!("open {domain} {kind}"));
        fake(if kind < 0 { -1 } else { 9000 + kind })
    }
    fn wrap(fd: c_int) -> OwnedCo {
        call(format!("wrap {fd}"));
        fake(fd)
    }

    /// The test host's sockets, in place of the native ones.
    pub(crate) const SOCKETS: Option<&SocketOps> = Some(&SocketOps { open, wrap });

    fn err_code(fd: c_int) -> c_int {
        socket_map::get(fd).unwrap().err_code()
    }

    /// Run `body` on a coroutine socket wrapping one end of a socketpair and the
    /// other end, then unwrap it: the deleted socket no longer owns the fd.
    fn wrapped(body: impl FnOnce(c_int, c_int)) {
        crate::co_io::test_host::install();
        let mut pair = [0; 2];
        unsafe {
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()),
                0
            );
            let (fd, peer) = (pair[0], pair[1]);
            with(Mode::Pool, || {
                assert_eq!(swoole_coroutine_socket_create(fd), 0);
                assert_eq!(swoole_coroutine_socket_exists(fd), 1);
                body(fd, peer);
                assert_eq!(swoole_coroutine_socket_unwrap(fd), 0);
                assert_eq!(swoole_coroutine_socket_unwrap(fd), -1);
                assert_eq!(swoole_coroutine_socket_exists(fd), 0);
            });
            assert_eq!(libc::close(fd), 0);
            libc::close(peer);
        }
        let wrap = format!("wrap {}", pair[0]);
        assert_eq!(calls(), [wrap.as_str(), "free -1 -1"]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sendmsg")]
    fn a_coroutine_transfers_on_mapped_fds_through_the_rust_members() {
        wrapped(|fd, peer| unsafe {
            let mut buf = [0u8; 16];
            assert_eq!(send(fd, b"send", 0).map_or(-1, usize::cast_signed), 4);
            assert_eq!(swoole_coroutine_write(fd, b"write".as_ptr().cast(), 5), 5);
            let iov = [std::io::IoSlice::new(b"msg")];
            assert_eq!(
                sendmsg(fd, &SendMessage::new(&iov), 0).map_or(-1, usize::cast_signed),
                3
            );
            assert_eq!(libc::read(peer, buf.as_mut_ptr().cast(), 16), 12);
            assert_eq!(&buf[..12], b"sendwritemsg");

            assert_eq!(libc::write(peer, b"abcdefgh".as_ptr().cast(), 8), 8);
            let mut got = [0u8; 2];
            assert_eq!(
                recv(fd, &mut got, libc::MSG_PEEK).map_or(-1, usize::cast_signed),
                2
            );
            assert_eq!(&got, b"ab");
            assert_eq!(recv(fd, &mut got, 0).map_or(-1, usize::cast_signed), 2);
            assert_eq!(&got, b"ab");
            assert_eq!(swoole_coroutine_read(fd, got.as_mut_ptr().cast(), 2), 2);
            assert_eq!(&got, b"cd");
            let mut into = [std::io::IoSliceMut::new(&mut buf[..4])];
            assert_eq!(
                recvmsg(fd, &mut RecvMessage::new(&mut into), 0).map_or(-1, usize::cast_signed),
                4
            );
            assert_eq!(&buf[..4], b"efgh");
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no accept on a socketpair")]
    fn a_coroutine_connects_accepts_and_waits_on_mapped_fds_through_the_rust_members() {
        use crate::co_io::test_host::{TIMER_OK, take};
        wrapped(|fd, _| unsafe {
            // The members record a failure on the socket.
            let mut addr = SocketAddress::default();
            addr.addr.unix_mut().sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).unwrap();
            addr.len = libc::socklen_t::try_from(size_of::<libc::sa_family_t>()).unwrap();
            assert_eq!(connect(fd, &addr).map_or(-1, |()| 0), -1);
            assert_ne!(err_code(fd), 0);
            assert_eq!(accept(fd, &mut addr).unwrap_or(-1), -1);
            assert_eq!(err_code(fd), libc::EINVAL);

            // wait_event polls with the timeout in every direction, then restores it.
            let socket = socket_map::get(fd).unwrap().socket();
            let read = (*socket).fields.timeouts.read;
            TIMER_OK.set(false);
            take();
            assert_eq!(swoole_coroutine_socket_wait_event(fd, 1 << 9, 0.5), -1);
            assert!(take().contains(&"timer:add:0.5".to_owned()));
            TIMER_OK.set(true);
            assert_eq!((*socket).fields.timeouts.read, read);
            assert_eq!(
                swoole_coroutine_socket_set_timeout(fd, libc::SO_RCVTIMEO, 0.25),
                0
            );
            assert_eq!((*socket).fields.timeouts.read, 0.25);
            assert_eq!(swoole_coroutine_socket_set_connect_timeout(fd, 2.0), 0);
            assert_eq!((*socket).fields.timeouts.connect, 2.0);
        });
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
    #[cfg_attr(miri, ignore = "Miri has no AF_UNIX sockets")]
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
                assert_eq!(send(pair[0], b"hi", 0).map_or(-1, usize::cast_signed), 2);
                assert_eq!(swoole_coroutine_socket_wait_event(pair[1], 1 << 9, 1.0), 0);
                let mut buf = [0u8; 2];
                assert_eq!(recv(pair[1], &mut buf, 0).map_or(-1, usize::cast_signed), 2);
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
            });
            // While the fd is still open: a closed fd's number can be reused and
            // mapped by a parallel test.
            let mut buf = [0u8; 3];
            assert_eq!(
                with(Mode::Refuse, || swoole_coroutine_read(
                    pair[1],
                    buf.as_mut_ptr().cast(),
                    3
                )),
                -1
            );
            with(Mode::Pool, || {
                // Not a coroutine socket: the fd closes on the pool.
                assert_eq!(swoole_coroutine_close(pair[0]), 0);
                assert_eq!(swoole_coroutine_close(pair[1]), 0);
            });
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
                let invalid = poll_fake(&mut fds, 10);
                assert!(matches!(
                    &invalid,
                    Err(FakePollError::InvalidDescriptorCount)
                ));
                assert_eq!(invalid.unwrap_or(-1), -1);
                assert_eq!(LAST_ERROR.get(), INVALID_PARAMS);
                // No socket: libc's poll.
                assert_eq!(poll_fake(&mut fds[..1], 10).unwrap_or(-1), 1);
                assert_eq!(fds[0].revents, libc::POLLOUT);
                // Without a timeout, poll is libc's even in a coroutine.
                assert_eq!(swoole_coroutine_poll(fds.as_mut_ptr(), 1, 0), 1);
            });
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }
    }

    #[test]
    fn polls_of_null_fds_fail_with_efault() {
        with(Mode::Pool, || unsafe {
            let result = swoole_coroutine_poll(core::ptr::null_mut(), 1, 10);
            assert_eq!((result, sys::errno()), (-1, libc::EFAULT));
            // No fds to read: libc's poll of none.
            assert_eq!(swoole_coroutine_poll(core::ptr::null_mut(), 0, 0), 0);
        });
    }
}
