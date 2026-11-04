//! `network::Socket`'s methods over its record: names, bind and listen, the TCP
//! and descriptor option flags, kernel timeouts, and `recv`/`send`/`peek` with their byte counters and timestamps.
//! Native `Socket` calls these inline (`swoole_socket.h`); a TLS socket passes its
//! OpenSSL call, which runs only when the record holds an `SSL`.
use crate::{SocketAddress, SocketRecord, TlsIo};
use std::ffi::{c_int, c_void};
use std::mem::size_of;

/// `SW_OK`, `SW_ERR`, `SW_WAIT` and `SW_BACKLOG`.
const OK: c_int = 0;
const ERR: c_int = -1;
const WAIT: c_int = 2;
const BACKLOG: c_int = 512;

/// `time<std::chrono::milliseconds>(true)`, from native code so the clock is the
/// one its timers read.
pub type SteadyMs = unsafe extern "C" fn() -> f64;

fn status(ok: bool) -> c_int {
    if ok { OK } else { ERR }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the address storage's size fits socklen_t"
)]
const ADDRESS_CAPACITY: libc::socklen_t = size_of::<crate::AddressStorage>() as libc::socklen_t;

/// `Socket::get_name`: getsockname(2) into `info`, which takes the socket's type.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_get_name(socket: *mut SocketRecord) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    socket.info.len = ADDRESS_CAPACITY;
    let address = (&raw mut socket.info.addr).cast::<libc::sockaddr>();
    // SAFETY: the record's own address storage and its length.
    if unsafe { libc::getsockname(socket.fd, address, &raw mut socket.info.len) } != 0 {
        return ERR;
    }
    socket.info.kind = socket.socket_type;
    OK
}

/// `Socket::get_peer_name`: getpeername(2) into `address`, typed as the socket.
///
/// # Safety
/// `socket` is a live record and `address` a writable `network::Address`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_get_peer_name(
    socket: *const SocketRecord,
    address: *mut SocketAddress,
) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: as the caller promises.
    let address = unsafe { &mut *address };
    address.len = ADDRESS_CAPACITY;
    address.kind = socket.socket_type;
    let storage = (&raw mut address.addr).cast::<libc::sockaddr>();
    // SAFETY: the caller's address storage and its length.
    status(unsafe { libc::getpeername(socket.fd, storage, &raw mut address.len) } == 0)
}

/// `Socket::bind(sockaddr, len)`: bind(2), marking the socket bound.
///
/// # Safety
/// `socket` is a live record and `address` denotes `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_bind(
    socket: *mut SocketRecord,
    address: *const libc::sockaddr,
    length: libc::socklen_t,
) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    // SAFETY: the caller lends `length` bytes of sockaddr for this call.
    if unsafe { libc::bind(socket.fd, address, length) } != 0 {
        return ERR;
    }
    socket.bound = 1;
    OK
}

/// `Socket::listen`: a backlog of 0 or less takes `SW_BACKLOG`; marks the socket
/// listening.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_listen(socket: *mut SocketRecord, backlog: c_int) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let backlog = if backlog <= 0 { BACKLOG } else { backlog };
    // SAFETY: listen(2) has no memory preconditions.
    if unsafe { libc::listen(socket.fd, backlog) } != 0 {
        return ERR;
    }
    socket.listened = 1;
    OK
}

/// `Socket::set_tcp_nopush`: `TCP_CORK`, recorded in the flag; -1 where unsupported.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_set_tcp_nopush(
    socket: *mut SocketRecord,
    nopush: c_int,
) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    if crate::swr_v1_socket_tcp_nopush(socket.fd, nopush) < 0 {
        return ERR;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream assigns the int to its flag"
    )]
    let flag = nopush as u8;
    socket.tcp_nopush = flag;
    OK
}

/// `Socket::set_tcp_nodelay`.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_set_tcp_nodelay(
    socket: *mut SocketRecord,
    nodelay: c_int,
) -> bool {
    // SAFETY: as the caller promises.
    unsafe { &mut *socket }.set_tcp_nodelay(nodelay)
}

/// `Socket::set_fd_option`: [`crate::swr_v1_socket_fd_option`], recording both
/// arguments (-1 included, as upstream assigns them) in the flags when it succeeds.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_set_fd_option(
    socket: *mut SocketRecord,
    nonblock: c_int,
    cloexec: c_int,
) -> crate::SocketOptionResult {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let result = crate::swr_v1_socket_fd_option(socket.fd, nonblock, cloexec);
    if result.ok {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream assigns the ints to its flags"
        )]
        let (nonblock, cloexec) = (nonblock as u8, cloexec as u8);
        socket.output.nonblock = nonblock;
        socket.cloexec = cloexec;
    }
    result
}

/// `_set_timeout`: `SO_SNDTIMEO` (`send`) or `SO_RCVTIMEO`, then the record's write
/// or read timeout, when the kernel took it.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_set_kernel_timeout(
    socket: *mut SocketRecord,
    send: bool,
    timeout: f64,
) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    if !crate::swr_v1_socket_kernel_timeout(socket.fd, send, timeout) {
        return false;
    }
    let slot = if send {
        &mut socket.timeouts.write
    } else {
        &mut socket.timeouts.read
    };
    *slot = timeout;
    true
}

/// `Socket::has_kernel_nobufs`: whether a send hit `ENOBUFS` since the last call.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_has_kernel_nobufs(socket: *mut SocketRecord) -> bool {
    // SAFETY: as the caller promises.
    std::mem::take(unsafe { &mut (*socket).kernel_nobufs })
}

/// The record's fd, blocking flags and TLS call (when it holds an `SSL`).
///
/// # Safety
/// `socket` is a live record.
unsafe fn io_of(
    socket: *mut SocketRecord,
    tls: TlsIo,
) -> (c_int, bool, bool, Option<(TlsIo, *mut c_void)>) {
    // SAFETY: as the caller promises.
    let record = unsafe { &*socket };
    let tls = (!record.ssl.is_null()).then_some((tls, socket.cast()));
    (
        record.fd,
        record.output.nonblock != 0,
        record.dont_restart != 0,
        tls,
    )
}

/// `Socket::recv`: [`crate::socket_recv`] counted into `total_recv_bytes` and, with a
/// receive timer, `last_received_time`. A read that would block after `POLLHUP`
/// reads as end of stream.
///
/// # Safety
/// `socket` is a live record; `buffer` denotes `length` writable bytes; `tls` and
/// `now` are valid for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_record_recv(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    tls: TlsIo,
    now: SteadyMs,
) -> isize {
    // SAFETY: as the caller promises.
    let (fd, nonblock, dont_restart, tls) = unsafe { io_of(socket, tls) };
    // SAFETY: the caller's buffer; the TLS call gets the socket it is valid for.
    let total =
        unsafe { crate::socket_recv(fd, buffer, length, flags, nonblock, dont_restart, tls) };
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: as the caller promises; the TLS call has returned.
    let record = unsafe { &mut *socket };
    if total > 0 {
        record.total_recv_bytes += total.cast_unsigned();
        if !record.recv_timer.is_null() {
            // SAFETY: the caller's clock.
            record.last_received_time = unsafe { now() };
        }
    }
    if total < 0 && record.catch_error(error) == WAIT && record.event_hup != 0 {
        return 0;
    }
    total
}

/// `Socket::send`: [`crate::socket_send`] counted into `total_send_bytes` and, with
/// a send timer, `last_sent_time`.
///
/// # Safety
/// `socket` is a live record; `buffer` denotes `length` readable bytes; `tls` and
/// `now` are valid for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_record_send(
    socket: *mut SocketRecord,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    tls: TlsIo,
    now: SteadyMs,
) -> isize {
    // SAFETY: as the caller promises.
    let (fd, _, dont_restart, tls) = unsafe { io_of(socket, tls) };
    // SAFETY: the caller's buffer; the TLS call gets the socket it is valid for.
    let sent = unsafe { crate::socket_send(fd, buffer, length, flags, dont_restart, tls) };
    if sent > 0 {
        // SAFETY: as the caller promises; the TLS call has returned.
        let record = unsafe { &mut *socket };
        record.total_send_bytes += sent.cast_unsigned();
        if !record.output.send_timer.is_null() {
            // SAFETY: the caller's clock.
            record.last_sent_time = unsafe { now() };
        }
    }
    sent
}

/// `Socket::peek`: [`crate::socket_peek`] on the record.
///
/// # Safety
/// `socket` is a live record; `buffer` denotes `length` writable bytes; `tls` is
/// valid for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_record_peek(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    tls: TlsIo,
) -> isize {
    // SAFETY: as the caller promises.
    let (fd, _, _, tls) = unsafe { io_of(socket, tls) };
    // SAFETY: the caller's buffer; the TLS call gets the socket it is valid for.
    unsafe { crate::socket_peek(fd, buffer, length, flags, tls) }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn no_tls(_: *mut c_void, _: *mut c_void, _: usize) -> isize {
        unreachable!("plain sockets never take the TLS call")
    }

    unsafe extern "C" fn clock() -> f64 {
        42.0
    }

    fn pair() -> [SocketRecord; 2] {
        let mut fds = [0; 2];
        // SAFETY: `fds` holds the two descriptors socketpair writes.
        let made =
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
        assert_eq!(made, 0);
        fds.map(|fd| SocketRecord::with_fd(fd, 0))
    }

    fn close(records: &[SocketRecord]) {
        for record in records {
            // SAFETY: the test's own descriptors.
            unsafe { libc::close(record.fd) };
        }
    }

    #[test]
    fn send_recv_and_peek_count_bytes_and_stamp_timers() {
        let [mut a, mut b] = pair();
        let timer = std::ptr::NonNull::<c_void>::dangling().as_ptr();
        b.output.send_timer = timer;
        a.recv_timer = timer;
        let mut buffer = [0_u8; 8];
        // SAFETY: live local records and buffers; no TLS.
        unsafe {
            let sent = swr_v1_socket_record_send(
                &raw mut b,
                b"hello".as_ptr().cast(),
                5,
                0,
                no_tls,
                clock,
            );
            assert_eq!(sent, 5);
            let peeked =
                swr_v1_socket_record_peek(&raw mut a, buffer.as_mut_ptr().cast(), 8, 0, no_tls);
            assert_eq!(peeked, 5);
            let read = swr_v1_socket_record_recv(
                &raw mut a,
                buffer.as_mut_ptr().cast(),
                8,
                0,
                no_tls,
                clock,
            );
            assert_eq!(read, 5);
        }
        assert_eq!(&buffer[..5], b"hello");
        assert_eq!((a.total_recv_bytes, b.total_send_bytes), (5, 5));
        assert_eq!((a.last_received_time, b.last_sent_time), (42.0, 42.0));
        close(&[a, b]);
    }

    #[test]
    fn a_blocked_read_after_hangup_is_end_of_stream() {
        let [mut a, b] = pair();
        a.output.nonblock = 1;
        let mut buffer = [0_u8; 4];
        // SAFETY: a nonblocking fd; the read would block.
        let set = unsafe { libc::fcntl(a.fd, libc::F_SETFL, libc::O_NONBLOCK) };
        assert_eq!(set, 0);
        // SAFETY: live local record and buffer; no TLS.
        let read = unsafe {
            swr_v1_socket_record_recv(&raw mut a, buffer.as_mut_ptr().cast(), 4, 0, no_tls, clock)
        };
        assert_eq!(read, -1);
        a.event_hup = 1;
        // SAFETY: as above.
        let read = unsafe {
            swr_v1_socket_record_recv(&raw mut a, buffer.as_mut_ptr().cast(), 4, 0, no_tls, clock)
        };
        assert_eq!(read, 0);
        assert_eq!(a.total_recv_bytes, 0);
        close(&[a, b]);
    }

    #[test]
    fn names_bind_listen_and_flags() {
        // SAFETY: socket(2) has no memory preconditions.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let mut socket = SocketRecord::with_fd(fd, 0);
        socket.socket_type = 1;
        // SAFETY: a zeroed sockaddr_in is a valid value.
        let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        address.sin_family = libc::AF_INET.try_into().unwrap_or_default();
        address.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "sizeof(sockaddr_in) fits socklen_t"
        )]
        let length = size_of::<libc::sockaddr_in>() as libc::socklen_t;
        // SAFETY: a live local record and sockaddr_in.
        unsafe {
            assert_eq!(
                swr_v1_socket_bind(&raw mut socket, (&raw const address).cast(), length),
                OK
            );
            assert_eq!(swr_v1_socket_listen(&raw mut socket, 0), OK);
            assert_eq!(swr_v1_socket_get_name(&raw mut socket), OK);
            assert!(swr_v1_socket_set_tcp_nodelay(&raw mut socket, 1));
            let mut peer = socket.info;
            assert_eq!(
                swr_v1_socket_get_peer_name(&raw const socket, &raw mut peer),
                ERR
            );
            assert_eq!(peer.kind, 1);
        }
        assert_eq!(
            (socket.bound, socket.listened, socket.tcp_nodelay),
            (1, 1, 1)
        );
        assert_eq!(socket.info.kind, 1);
        assert_eq!(socket.info.len, length);
        socket.kernel_nobufs = true;
        // SAFETY: a live local record.
        unsafe {
            assert!(swr_v1_socket_has_kernel_nobufs(&raw mut socket));
            assert!(!swr_v1_socket_has_kernel_nobufs(&raw mut socket));
            assert!(swr_v1_socket_set_fd_option(&raw mut socket, 1, -1).ok);
            assert!(swr_v1_socket_set_kernel_timeout(&raw mut socket, true, 1.5));
            assert!(swr_v1_socket_set_kernel_timeout(
                &raw mut socket,
                false,
                0.25
            ));
        }
        assert_eq!((socket.output.nonblock, socket.cloexec), (1, 255));
        assert_eq!((socket.timeouts.write, socket.timeouts.read), (1.5, 0.25));
        close(&[socket]);
        let mut closed = SocketRecord::with_fd(-1, 0);
        // SAFETY: a live local record with a bad fd.
        unsafe {
            assert_eq!(swr_v1_socket_listen(&raw mut closed, 0), ERR);
            assert_eq!(swr_v1_socket_get_name(&raw mut closed), ERR);
            assert!(!swr_v1_socket_set_fd_option(&raw mut closed, 1, 1).ok);
            assert!(!swr_v1_socket_set_kernel_timeout(
                &raw mut closed,
                true,
                3.0
            ));
        }
        assert_eq!(
            (closed.listened, closed.output.nonblock, closed.cloexec),
            (0, 0, 0)
        );
        assert_eq!(closed.timeouts.write, 60.0);
    }
}
