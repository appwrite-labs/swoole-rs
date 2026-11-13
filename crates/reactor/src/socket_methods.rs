//! `network::Socket`'s methods over its record: names, bind and listen, the TCP
//! and descriptor option flags, kernel timeouts, and `recv`/`send`/`peek` with their byte counters and timestamps.
//! Native `Socket` calls these inline (`swoole_socket.h`); a TLS socket passes its
//! OpenSSL call, which runs only when the record holds an `SSL`.
use crate::socket_rw::interrupted;
use crate::{Drain, Fill, Socket, SocketAddress, SocketFields, TlsIo};
use std::ffi::{c_int, c_void};
use swoole_rs_corebase::sys::ReadSlot;

/// `SW_OK`, `SW_ERR`, `SW_WAIT` and `SW_BACKLOG`.
const OK: c_int = 0;
const ERR: c_int = -1;
const WAIT: c_int = 2;
const BACKLOG: c_int = 512;

/// `time<std::chrono::milliseconds>(true)`, from native code so the clock is the
/// one its timers read.
pub type SteadyMs = extern "C" fn() -> f64;

/// `swoole::time<std::chrono::milliseconds>(true)`: the steady clock in milliseconds.
#[must_use]
pub extern "C" fn steady_ms() -> f64 {
    #[cfg(target_vendor = "apple")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_vendor = "apple"))]
    let clock = libc::CLOCK_MONOTONIC;
    #[expect(
        clippy::cast_precision_loss,
        reason = "upstream converts the millisecond count to double too"
    )]
    let ms = swoole_rs_corebase::sys::clock_gettime(clock)
        .map_or(0, |now| now.tv_sec * 1000 + now.tv_nsec / 1_000_000) as f64;
    ms
}

fn status(ok: bool) -> c_int {
    if ok { OK } else { ERR }
}

impl SocketFields {
    /// `Socket::get_name`: getsockname(2) into `info`, which takes the socket's type.
    pub fn get_name(&mut self) -> c_int {
        if self.info.local_name(self.fd) != 0 {
            return ERR;
        }
        self.info.kind = self.socket_type;
        OK
    }

    /// `Socket::get_peer_name`: getpeername(2) into `address`, typed as the socket.
    pub fn get_peer_name(&self, address: &mut SocketAddress) -> c_int {
        address.kind = self.socket_type;
        status(address.peer_name(self.fd) == 0)
    }

    /// `Socket::bind(address)`: bind(2), marking the socket bound.
    pub fn bind(&mut self, address: &SocketAddress) -> c_int {
        if address.bind(self.fd) != 0 {
            return ERR;
        }
        self.bound = 1;
        OK
    }

    /// `Socket::listen`: a backlog of 0 or less takes `SW_BACKLOG`; marks the socket
    /// listening.
    pub fn listen(&mut self, backlog: c_int) -> c_int {
        let backlog = if backlog <= 0 { BACKLOG } else { backlog };
        if swoole_rs_corebase::sys::listen(self.fd, backlog) != 0 {
            return ERR;
        }
        self.listened = 1;
        OK
    }

    /// `Socket::set_tcp_nopush`: `TCP_CORK`, recorded in the flag; -1 where unsupported.
    pub fn set_tcp_nopush(&mut self, nopush: c_int) -> c_int {
        if crate::swr_v1_socket_tcp_nopush(self.fd, nopush) < 0 {
            return ERR;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream assigns the int to its flag"
        )]
        let flag = nopush as u8;
        self.tcp_nopush = flag;
        OK
    }

    /// `Socket::set_fd_option`: [`crate::swr_v1_socket_fd_option`], recording both
    /// arguments (-1 included, as upstream assigns them) in the flags when it succeeds.
    pub fn set_fd_option(&mut self, nonblock: c_int, cloexec: c_int) -> crate::SocketOptionResult {
        let result = crate::swr_v1_socket_fd_option(self.fd, nonblock, cloexec);
        if result.ok {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "upstream assigns the ints to its flags"
            )]
            let (nonblock, cloexec) = (nonblock as u8, cloexec as u8);
            self.output.nonblock = nonblock;
            self.cloexec = cloexec;
        }
        result
    }

    /// `_set_timeout`: `SO_SNDTIMEO` (`send`) or `SO_RCVTIMEO`, then the record's write
    /// or read timeout, when the kernel took it.
    pub fn set_kernel_timeout(&mut self, send: bool, timeout: f64) -> bool {
        if !crate::swr_v1_socket_kernel_timeout(self.fd, send, timeout) {
            return false;
        }
        let slot = if send {
            &mut self.timeouts.write
        } else {
            &mut self.timeouts.read
        };
        *slot = timeout;
        true
    }

    /// `Socket::has_kernel_nobufs`: whether a send hit `ENOBUFS` since the last call.
    pub fn has_kernel_nobufs(&mut self) -> bool {
        std::mem::take(&mut self.kernel_nobufs)
    }
}

/// The record's fd, its blocking flags and whether it holds an `SSL`.
fn io_of(socket: Socket<'_>) -> (c_int, bool, bool, bool) {
    socket.read(|record| {
        (
            record.fd,
            record.output.nonblock != 0,
            record.dont_restart != 0,
            !record.ssl().is_null(),
        )
    })
}

/// `Socket::recv`: recv(2), or `tls` reads until `buffer` is full when the socket is
/// nonblocking or `MSG_WAITALL` is set, retried on EINTR unless `dont_restart`. Counted
/// into `total_recv_bytes` and, with a receive timer, `last_received_time`. A read that
/// would block after `POLLHUP` reads as end of stream. `tls` runs only for a record
/// that holds an `SSL`.
pub fn record_recv<S: ReadSlot>(
    socket: Socket<'_>,
    mut buffer: Fill<'_, S>,
    flags: c_int,
    tls: impl Fn(Socket<'_>, Fill<'_, S>) -> isize,
    now: SteadyMs,
) -> isize {
    let (fd, nonblock, dont_restart, secure) = io_of(socket);
    let total = loop {
        let total = if secure {
            let mut rest = buffer.reborrow();
            let mut total: isize = 0;
            while !rest.is_empty() {
                let read = tls(socket, rest.reborrow());
                if read <= 0 {
                    if total == 0 {
                        total = read;
                    }
                    break;
                }
                total += read;
                // A full buffer ends the loop, as does a blocking read without `MSG_WAITALL`.
                if !(nonblock || flags & libc::MSG_WAITALL != 0)
                    || !rest.advance(read.cast_unsigned())
                {
                    break;
                }
            }
            total
        } else {
            buffer.recv(fd, flags)
        };
        if !(total < 0 && interrupted() && !dont_restart) {
            break total;
        }
    };
    if total > 0
        && socket.edit(|record| {
            record.total_recv_bytes += total.cast_unsigned();
            !record.recv_timer.is_null()
        })
    {
        let time = now();
        socket.edit(|record| record.last_received_time = time);
    } else if total < 0 {
        // Nothing since the call has touched errno.
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if socket.catch_error(error) == WAIT && socket.read(|record| record.event_hup != 0) {
            return 0;
        }
    }
    total
}

/// `Socket::send`: send(2) or one `tls` write, retried on EINTR unless `dont_restart`,
/// counted into `total_send_bytes` and, with a send timer, `last_sent_time`. `tls` runs
/// only for a record that holds an `SSL`.
pub fn record_send(
    socket: Socket<'_>,
    data: Drain<'_>,
    flags: c_int,
    tls: impl Fn(Socket<'_>, Drain<'_>) -> isize,
    now: SteadyMs,
) -> isize {
    let (fd, _, dont_restart, secure) = io_of(socket);
    let sent = loop {
        let sent = if secure {
            tls(socket, data)
        } else {
            data.send(fd, flags)
        };
        if !(sent < 0 && interrupted() && !dont_restart) {
            break sent;
        }
    };
    if sent > 0
        && socket.edit(|record| {
            record.total_send_bytes += sent.cast_unsigned();
            !record.output.send_timer.is_null()
        })
    {
        let time = now();
        socket.edit(|record| record.last_sent_time = time);
    }
    sent
}

/// [`record_send`] of the `length` bytes at `buffer`, with a TLS write C defines.
///
/// # Safety
/// `buffer` denotes `length` readable bytes; `tls` is valid for the socket.
pub unsafe fn socket_record_send(
    socket: Socket<'_>,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    tls: TlsIo,
    now: SteadyMs,
) -> isize {
    // SAFETY: the caller lends `length` readable bytes.
    let data = unsafe { Drain::from_raw(buffer, length) };
    let tls = |socket: Socket<'_>, data: Drain<'_>| {
        let (record, at) = (socket.as_ptr().cast(), data.as_ptr().cast_mut());
        // SAFETY: the caller's TLS write for this socket; it only reads the bytes.
        unsafe { tls(record, at, data.len()) }
    };
    record_send(socket, data, flags, tls, now)
}

/// `Socket::peek`: recv(2) with `MSG_PEEK`, or `tls` (`SSL_peek`), retried on EINTR.
/// `tls` runs only for a record that holds an `SSL`.
pub fn record_peek<S: ReadSlot>(
    socket: Socket<'_>,
    mut buffer: Fill<'_, S>,
    flags: c_int,
    tls: impl Fn(Socket<'_>, Fill<'_, S>) -> isize,
) -> isize {
    let (fd, _, _, secure) = io_of(socket);
    loop {
        let peeked = if secure {
            tls(socket, buffer.reborrow())
        } else {
            buffer.recv(fd, flags | libc::MSG_PEEK)
        };
        if !(peeked < 0 && interrupted()) {
            return peeked;
        }
    }
}

/// `Socket::what_event_want`: the event a pending TLS read or write waits for, else
/// `default`.
#[must_use]
pub fn what_event_want(socket: &SocketFields, default: c_int) -> c_int {
    let tls = socket.tls;
    if !socket.ssl().is_null() && (tls.want_read != 0 || tls.want_write != 0) {
        use swoole_rs_corebase::constants::EventType::{SW_EVENT_READ, SW_EVENT_WRITE};
        if tls.want_write != 0 {
            SW_EVENT_WRITE as c_int
        } else {
            SW_EVENT_READ as c_int
        }
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SocketRecord;
    use crate::lent::raw::{socket_record_peek, socket_record_recv};

    #[test]
    fn native_event_want_preserves_write_priority_and_default() {
        let mut socket: SocketRecord = unsafe { std::mem::zeroed() };
        for ssl in [false, true] {
            let live = if ssl {
                std::ptr::dangling_mut()
            } else {
                std::ptr::null_mut()
            };
            unsafe { socket.set_ssl(live) };
            for read in [0, 1] {
                for write in [0, 1] {
                    for default in [-1, 0, 512, 1024] {
                        socket.tls.want_read = read;
                        socket.tls.want_write = write;
                        let expected = if ssl && write != 0 {
                            1024
                        } else if ssl && read != 0 {
                            512
                        } else {
                            default
                        };
                        assert_eq!(what_event_want(&socket, default), expected);
                    }
                }
            }
        }
    }

    unsafe extern "C" fn no_tls(_: *mut c_void, _: *mut c_void, _: usize) -> isize {
        unreachable!("plain sockets never take the TLS call")
    }

    extern "C" fn clock() -> f64 {
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
    #[cfg_attr(miri, ignore = "Miri has no datagram socketpair")]
    fn send_recv_and_peek_count_bytes_and_stamp_timers() {
        let [mut a, mut b] = pair();
        let timer = std::ptr::NonNull::<c_void>::dangling().as_ptr();
        b.output.send_timer = timer;
        a.recv_timer = timer;
        let mut buffer = [0_u8; 8];
        // SAFETY: live local records and buffers; no TLS.
        unsafe {
            let sent = socket_record_send(
                Socket::from_ptr(&raw mut b).unwrap(),
                b"hello".as_ptr().cast(),
                5,
                0,
                no_tls,
                clock,
            );
            assert_eq!(sent, 5);
            let peeked = socket_record_peek(
                Socket::from_ptr(&raw mut a).unwrap(),
                buffer.as_mut_ptr().cast(),
                8,
                0,
                no_tls,
            );
            assert_eq!(peeked, 5);
            let read = socket_record_recv(
                Socket::from_ptr(&raw mut a).unwrap(),
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
            socket_record_recv(
                Socket::from_ptr(&raw mut a).unwrap(),
                buffer.as_mut_ptr().cast(),
                4,
                0,
                no_tls,
                clock,
            )
        };
        assert_eq!(read, -1);
        a.event_hup = 1;
        // SAFETY: as above.
        let read = unsafe {
            socket_record_recv(
                Socket::from_ptr(&raw mut a).unwrap(),
                buffer.as_mut_ptr().cast(),
                4,
                0,
                no_tls,
                clock,
            )
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
                socket.bind(&{
                    let mut local: SocketAddress = std::mem::zeroed();
                    std::ptr::copy_nonoverlapping(
                        (&raw const address).cast::<u8>(),
                        (&raw mut local.addr).cast::<u8>(),
                        length as usize,
                    );
                    local.len = length;
                    local
                }),
                OK
            );
            assert_eq!(socket.listen(0), OK);
            assert_eq!(socket.get_name(), OK);
            assert!(socket.set_tcp_nodelay(1));
            let mut peer = socket.info;
            assert_eq!(socket.get_peer_name(&mut peer), ERR);
            assert_eq!(peer.kind, 1);
        }
        assert_eq!(
            (socket.bound, socket.listened, socket.tcp_nodelay),
            (1, 1, 1)
        );
        assert_eq!(socket.info.kind, 1);
        assert_eq!(socket.info.len, length);
        socket.kernel_nobufs = true;
        assert!(socket.has_kernel_nobufs());
        assert!(!socket.has_kernel_nobufs());
        assert!(socket.set_fd_option(1, -1).ok);
        assert!(socket.set_kernel_timeout(true, 1.5));
        assert!(socket.set_kernel_timeout(false, 0.25));
        assert_eq!((socket.output.nonblock, socket.cloexec), (1, 255));
        assert_eq!((socket.timeouts.write, socket.timeouts.read), (1.5, 0.25));
        close(&[socket]);
        let mut closed = SocketRecord::with_fd(-1, 0);
        assert_eq!(closed.listen(0), ERR);
        assert_eq!(closed.get_name(), ERR);
        assert!(!closed.set_fd_option(1, 1).ok);
        assert!(!closed.set_kernel_timeout(true, 3.0));
        assert_eq!(
            (closed.listened, closed.output.nonblock, closed.cloexec),
            (0, 0, 0)
        );
        assert_eq!(closed.timeouts.write, 60.0);
    }
}
