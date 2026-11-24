//! `network::Socket`'s methods over its record: names, bind and listen, the TCP
//! and descriptor option flags, kernel timeouts, and `recv`/`send`/`peek` with their byte counters and timestamps.
//! Native `Socket` calls these inline (`swoole_socket.h`); a TLS socket passes its
//! OpenSSL call, which runs only when the record holds an `SSL`.
use crate::CCast;
use crate::constants::SW_BACKLOG;
use crate::lent::Spare;
use crate::net_types::AddressStorage;
use crate::reactor::socket_rw::interrupted;
use crate::reactor::{Drain, Socket, SocketAddress, SocketFields};
use std::ffi::{CStr, c_int};

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
    crate::sys::clock_gettime(clock)
        .map_or(0, |now| now.tv_sec * 1000 + now.tv_nsec / 1_000_000)
        .c_cast::<f64>()
}

impl SocketFields {
    /// `Socket::get_name`: getsockname(2) into `info`, which takes the socket's type.
    pub fn get_name(&mut self) -> bool {
        if self.info.local_name(self.fd).is_err() {
            return false;
        }
        self.info.kind = self.socket_type;
        true
    }

    /// `Socket::get_peer_name`: getpeername(2) into `address`, typed as the socket.
    pub fn get_peer_name(&self, address: &mut SocketAddress) -> bool {
        address.kind = self.socket_type;
        address.peer_name(self.fd).is_ok()
    }

    /// `Address::get_port` of the socket's address (the peer an accept, connect or
    /// recvfrom left there, or the socket's own name after [`Self::get_name`]): 0
    /// for an address that is not IP.
    #[inline]
    #[must_use]
    pub fn port(&self) -> u16 {
        self.info.port()
    }

    /// `Address::get_addr` of the socket's address, as [`SocketAddress::with_host`].
    pub fn with_host<R>(&self, f: impl FnOnce(&CStr) -> R) -> R {
        self.info.with_host(f)
    }

    /// The address family its storage holds.
    #[inline]
    #[must_use]
    pub const fn address_family(&self) -> libc::sa_family_t {
        self.info.addr.family()
    }

    /// The address's storage and length, to copy into a native address.
    #[inline]
    #[must_use]
    pub const fn address_storage(&self) -> (AddressStorage, libc::socklen_t) {
        (self.info.addr, self.info.len)
    }

    /// The socket's address becomes the peer it connected to or accepted: `len`
    /// bytes of `addr`, of socket type `kind`.
    #[inline]
    pub const fn record_address(
        &mut self,
        addr: AddressStorage,
        len: libc::socklen_t,
        kind: c_int,
    ) {
        self.info.addr = addr;
        self.info.len = len;
        self.info.kind = kind;
    }

    /// The socket's address takes the socket type `kind` (`info.type = type`).
    #[inline]
    pub const fn type_address(&mut self, kind: c_int) {
        self.info.kind = kind;
    }

    /// One recvfrom(2) on `fd` into `out`'s room, the sender into the socket's
    /// address, as [`Spare::recvfrom_one`].
    #[inline]
    pub fn receive_from(&mut self, out: &mut Spare<'_>, fd: c_int, flags: c_int) -> Option<usize> {
        out.recvfrom_one(fd, flags, &mut self.info)
    }

    /// `info.len = sizeof(info.addr)`: the whole address is room for a recvfrom's sender.
    #[inline]
    pub const fn make_room_for_sender(&mut self) {
        self.info.len = SocketAddress::CAPACITY;
    }

    /// An unnamed Unix sender (one whose length ends before its path) leaves the
    /// address's path empty, as upstream's `recvfrom` does for a local socket.
    pub fn clear_unnamed_sender(&mut self) {
        let info = &mut self.info;
        if info.len as usize <= core::mem::offset_of!(libc::sockaddr_un, sun_path) {
            info.addr.edit_unix(|un| un.sun_path[0] = 0);
        }
    }

    /// `Client_udp_connect`'s bind: the socket's address becomes the Unix `path`
    /// (written through its NUL, cut at the path's room), and the socket binds it at
    /// `sizeof(sockaddr_un)`; its address keeps its own length and type, as upstream.
    pub fn bind_unix_path(&mut self, path: &[u8]) -> bool {
        self.info.addr.edit_unix(|address| {
            for (slot, byte) in address.sun_path.iter_mut().zip(path.iter().chain(&[0])) {
                *slot = std::ffi::c_char::from_ne_bytes([*byte]);
            }
            address.sun_family = libc::AF_UNIX.c_cast::<libc::sa_family_t>();
        });
        let mut local = self.info;
        local.len = crate::sys::socklen::<libc::sockaddr_un>();
        local.bind(self.fd)
    }

    /// unlink(2) of the Unix path the socket's address holds, as a Unix datagram
    /// client's close removes the path it bound.
    pub fn unlink_unix_path(&self) {
        let path = self.info.addr.unix().sun_path;
        // c_char is i8 or u8 by target; its one byte either way.
        let path = path.map(|byte| byte.to_ne_bytes()[0]);
        if let Ok(path) = std::ffi::CStr::from_bytes_until_nul(&path) {
            _ = crate::sys::unlink(path);
        }
    }

    /// `Socket::bind(address)`: bind(2), marking the socket bound.
    pub fn bind(&mut self, address: &SocketAddress) -> bool {
        if !address.bind(self.fd) {
            return false;
        }
        self.bound = true;
        true
    }

    /// `Socket::listen`: a backlog of 0 or less takes `SW_BACKLOG`; marks the socket
    /// listening.
    pub fn listen(&mut self, backlog: c_int) -> bool {
        let backlog = if backlog <= 0 {
            SW_BACKLOG.cast_signed()
        } else {
            backlog
        };
        if crate::sys::listen(self.fd, backlog).is_err() {
            return false;
        }
        self.listened = true;
        true
    }

    /// `Socket::set_tcp_nopush`: `TCP_CORK`, recorded in the flag; false where unsupported.
    pub fn set_tcp_nopush(&mut self, nopush: c_int) -> bool {
        if !crate::reactor::swr_v1_socket_tcp_nopush(self.fd, nopush) {
            return false;
        }
        // Upstream's 1-bit field keeps the int's low bit.
        self.tcp_nopush = nopush & 1 != 0;
        true
    }

    /// `Socket::set_fd_option`: [`crate::reactor::swr_v1_socket_fd_option`] (-1 leaves a flag
    /// alone, 0 clears it, anything else sets it), recording both arguments (-1
    /// included, as upstream assigns them) in the flags when it succeeds.
    pub fn set_fd_option(
        &mut self,
        nonblock: c_int,
        cloexec: c_int,
    ) -> crate::reactor::SocketOptionResult {
        let flag = |value: c_int| (value >= 0).then_some(value != 0);
        let result =
            crate::reactor::swr_v1_socket_fd_option(self.fd, flag(nonblock), flag(cloexec));
        if result.ok {
            // Upstream's 1-bit fields keep each int's low bit.
            self.output.nonblock = nonblock & 1 != 0;
            self.cloexec = cloexec & 1 != 0;
        }
        result
    }

    /// `Socket::set_nonblock`: `set_fd_option(1, -1)`, which, as upstream, also
    /// records close-on-exec when it succeeds.
    pub fn set_nonblock(&mut self) -> crate::reactor::SocketOptionResult {
        self.set_fd_option(1, -1)
    }

    /// `_set_timeout`: `SO_SNDTIMEO` (`send`) or `SO_RCVTIMEO`, then the record's write
    /// or read timeout, when the kernel took it.
    pub fn set_kernel_timeout(&mut self, send: bool, timeout: f64) -> bool {
        if !crate::reactor::swr_v1_socket_kernel_timeout(self.fd, send, timeout) {
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

    /// Whether a send hit `ENOBUFS` since [`Self::has_kernel_nobufs`] last cleared it,
    /// leaving it set.
    #[inline]
    #[must_use]
    pub const fn kernel_nobufs_pending(&self) -> bool {
        self.kernel_nobufs
    }

    /// A send or receive on the socket hit `ENOBUFS`, as [`Self::has_kernel_nobufs`]
    /// reports next. It writes only that flag.
    #[inline]
    pub const fn note_kernel_nobufs(&mut self) {
        self.kernel_nobufs = true;
    }

    /// `Socket::catch_write_pipe_error`, setting the socket's `kernel_nobufs` as
    /// [`crate::reactor::catch_pipe_error`] does.
    #[inline]
    pub fn catch_pipe_error(&mut self, error: c_int) -> crate::reactor::Caught {
        crate::reactor::catch_pipe_error(error, &mut self.kernel_nobufs)
    }

    /// Whether the worker was told the peer closed (`close_notified`): the
    /// connection's reads are over.
    #[inline]
    #[must_use]
    pub const fn close_reported(&self) -> bool {
        self.close_notified
    }

    /// The worker is told the peer closed, once: true the first time, false when it
    /// already was.
    #[inline]
    pub const fn claim_close_report(&mut self) -> bool {
        !std::mem::replace(&mut self.close_notified, true)
    }
}

/// A point in a socket's receive count ([`SocketFields::receive_mark`]).
#[derive(Clone, Copy)]
pub struct ReceiveMark(usize);

/// A socket's byte counts ([`SocketFields::traffic`]).
#[derive(Clone, Copy)]
pub struct Traffic {
    pub received: u64,
    pub sent: u64,
}

impl SocketFields {
    /// The record's fd, its blocking flags and whether it holds an `SSL`. Inlining lets
    /// each I/O operation load only the fields it uses under the single socket borrow.
    #[inline(always)]
    fn io_view(&self) -> (c_int, bool, bool, bool) {
        (
            self.fd,
            self.is_nonblocking(),
            self.dont_restart,
            !self.ssl().is_null(),
        )
    }

    /// Counts `bytes` received and, while an idle timer watches receives, restarts its
    /// clock at `now()`.
    #[inline(always)]
    fn count_received(&mut self, bytes: usize, now: SteadyMs) {
        self.total_recv_bytes += bytes;
        if self.recv_timer.is_some() {
            self.last_received_time = now();
        }
    }

    /// Counts `bytes` sent and, while an idle timer watches sends, restarts its clock
    /// at `now()`.
    #[inline(always)]
    fn count_sent(&mut self, bytes: usize, now: SteadyMs) {
        self.total_send_bytes += bytes;
        if self.send_timer().is_some() {
            self.last_sent_time = now();
        }
    }

    /// Where the receive count stands, for [`Self::received_since`] to measure from.
    #[inline]
    #[must_use]
    pub const fn receive_mark(&self) -> ReceiveMark {
        ReceiveMark(self.total_recv_bytes)
    }

    /// The bytes received since `mark` was taken.
    #[inline]
    #[must_use]
    pub const fn received_since(&self, mark: ReceiveMark) -> usize {
        self.total_recv_bytes.wrapping_sub(mark.0)
    }

    /// The bytes received and sent over the socket's life.
    #[inline]
    #[must_use]
    pub const fn traffic(&self) -> Traffic {
        Traffic {
            received: self.total_recv_bytes as u64,
            sent: self.total_send_bytes as u64,
        }
    }

    /// Whether the socket received and sent within the `idle` milliseconds before
    /// `now`.
    #[inline]
    #[must_use]
    pub fn active_within(&self, now: f64, idle: f64) -> bool {
        now - self.last_received_time < idle && now - self.last_sent_time < idle
    }
}

/// An idle timer taken off a socket ([`SocketFields::disarm_recv_timer`]), deleted
/// once the socket's borrow has ended ([`crate::timer::del_disarmed`]).
#[must_use]
#[derive(Clone, Copy)]
pub struct DisarmedTimer(pub(crate) Option<crate::timer_key::Held>);

/// The receive side between readable events: the reader's buffer, the receive idle
/// timer, and the framing loop's wait and skip.
impl SocketFields {
    /// The bytes kept from the last readable event, out of the socket for this read;
    /// none when it kept none.
    #[inline]
    #[must_use]
    pub fn take_recv_buffer(&mut self) -> Option<Box<crate::buffer::string::OwnedString>> {
        self.recv_buffer.take()
    }

    /// Frees the bytes kept for the next readable event: the connection closed.
    #[inline]
    pub fn free_recv_buffer(&mut self) {
        self.recv_buffer = None;
    }

    /// Keeps `recv`'s bytes for the next readable event, unless the socket kept a buffer
    /// meanwhile, which stays.
    #[inline]
    pub fn keep_recv_buffer(&mut self, recv: Box<crate::buffer::string::OwnedString>) {
        self.recv_buffer.get_or_insert(recv);
    }

    /// Watches the socket's receives for `idle` seconds of quiet: the read timeout and
    /// `timer` as the receive idle timer.
    #[inline]
    pub fn watch_recv_idle(&mut self, idle: f64, timer: Option<crate::timer_key::Held>) {
        self.set_timeout(idle, crate::reactor::TimeoutKind::READ);
        self.recv_timer = timer;
    }

    /// The receive idle timer, disarmed, to delete outside the socket's borrow.
    #[inline]
    pub fn disarm_recv_timer(&mut self) -> DisarmedTimer {
        DisarmedTimer(self.recv_timer.take())
    }

    /// Whether the socket's reader waits for more data before it can go on: the rest
    /// of a package, or a request (`recv_wait`).
    #[inline]
    pub fn await_recv(&mut self, waiting: bool) {
        self.recv_wait = waiting;
    }

    /// Whether the socket's reader waits for more data ([`Self::await_recv`]).
    #[inline]
    #[must_use]
    pub fn awaits_recv(&self) -> bool {
        self.recv_wait
    }

    /// Applies the framing loop's `FLAG_WAIT` from `flags`: whether it waits for the
    /// rest of a package.
    #[inline]
    pub fn apply_framing_wait(&mut self, flags: u32) {
        self.recv_wait = flags & swoole_rs_framing::recv::FLAG_WAIT != 0;
    }

    /// The next framing read starts over the package length it already has
    /// (`skip_recv`).
    #[inline]
    pub fn skip_next_recv(&mut self) {
        self.skip_recv = true;
    }

    /// The pending skip is spent.
    #[inline]
    pub fn clear_recv_skip(&mut self) {
        self.skip_recv = false;
    }

    /// The framing loop's flags from the socket: removed, waiting, TLS, and paused,
    /// which `paused` is asked only of a removed socket.
    #[inline(always)]
    pub fn framing_flags(&self, paused: impl FnOnce() -> bool) -> u32 {
        use swoole_rs_framing::recv::{FLAG_PAUSED, FLAG_REMOVED, FLAG_TLS, FLAG_WAIT};
        (if self.removed { FLAG_REMOVED } else { 0 })
            | (if self.recv_wait { FLAG_WAIT } else { 0 })
            | (if self.ssl().is_null() { 0 } else { FLAG_TLS })
            | (if self.removed && paused() {
                FLAG_PAUSED
            } else {
                0
            })
    }

    /// [`Self::framing_flags`] at a read's start, with `FLAG_SKIP` while a skip is
    /// pending.
    #[inline(always)]
    pub fn framing_entry_flags(&self, paused: impl FnOnce() -> bool) -> u32 {
        use swoole_rs_framing::recv::FLAG_SKIP;
        self.framing_flags(paused) | if self.skip_recv { FLAG_SKIP } else { 0 }
    }
}

/// `Socket::recv`: recv(2), or `tls` reads until `buffer` is full when the socket is
/// nonblocking or `MSG_WAITALL` is set, retried on EINTR unless `dont_restart`. Counted
/// into `total_recv_bytes` and, with a receive timer, `last_received_time`. A read that
/// would block after `POLLHUP` reads as end of stream. `tls` runs only for a record
/// that holds an `SSL`.
#[inline(always)]
pub fn record_recv(
    socket: Socket<'_>,
    buffer: &mut Spare<'_>,
    flags: c_int,
    tls: impl Fn(Socket<'_>, &mut Spare<'_>) -> Option<usize>,
    now: SteadyMs,
) -> Option<usize> {
    let (fd, nonblock, dont_restart, secure) = socket.read(SocketFields::io_view);
    let total = loop {
        let total = if secure {
            let (mut sum, mut total) = (0, Some(0));
            // Each read fills the room after the one before.
            while buffer.room() != 0 {
                let read = tls(socket, buffer);
                let Some(read @ 1..) = read else {
                    if sum == 0 {
                        total = read;
                    }
                    break;
                };
                sum += read;
                total = Some(sum);
                // A full buffer ends the loop, as does a blocking read without `MSG_WAITALL`.
                if !(nonblock || flags & libc::MSG_WAITALL != 0) || buffer.room() == 0 {
                    break;
                }
            }
            total
        } else {
            buffer.recv(fd, flags)
        };
        if let Some(received) = total {
            break received;
        }
        let error = crate::sys::errno();
        if error == libc::EINTR && !dont_restart {
            continue;
        }
        return (socket.catch_error(error) == crate::reactor::Caught::Wait
            && socket.read(|record| record.event_hup))
        .then_some(0);
    };
    if total != 0 {
        socket.edit(|record| record.count_received(total, now));
    }
    Some(total)
}

/// `Socket::send`: send(2) or one `tls` write, retried on EINTR unless `dont_restart`,
/// counted into `total_send_bytes` and, with a send timer, `last_sent_time`. `tls` runs
/// only for a record that holds an `SSL`.
pub fn record_send(
    socket: Socket<'_>,
    data: Drain<'_>,
    flags: c_int,
    tls: impl Fn(Socket<'_>, Drain<'_>) -> Option<usize>,
    now: SteadyMs,
) -> Option<usize> {
    let (fd, _, dont_restart, secure) = socket.read(SocketFields::io_view);
    let write = || {
        if secure {
            tls(socket, data)
        } else {
            data.send(fd, flags)
        }
    };
    count_write(socket, dont_restart, write, now)
}

/// [`record_send`]'s retries and counts around any `write` to the record's fd.
pub fn record_write(
    socket: Socket<'_>,
    write: impl Fn(c_int) -> Option<usize>,
    now: SteadyMs,
) -> Option<usize> {
    let (fd, _, dont_restart, _) = socket.read(SocketFields::io_view);
    count_write(socket, dont_restart, || write(fd), now)
}

/// `write`, retried on EINTR unless `dont_restart`, counted into `total_send_bytes`
/// and, with a send timer, `last_sent_time`.
#[inline(always)]
pub fn count_write(
    socket: Socket<'_>,
    dont_restart: bool,
    write: impl Fn() -> Option<usize>,
    now: SteadyMs,
) -> Option<usize> {
    let sent = loop {
        let sent = write();
        if !(sent.is_none() && interrupted() && !dont_restart) {
            break sent;
        }
    };
    if let Some(sent @ 1..) = sent {
        socket.edit(|record| record.count_sent(sent, now));
    }
    sent
}

/// `Socket::peek`: recv(2) with `MSG_PEEK`, or `tls` (`SSL_peek`), retried on EINTR.
/// `tls` runs only for a record that holds an `SSL`.
pub fn record_peek(
    socket: Socket<'_>,
    buffer: &mut Spare<'_>,
    flags: c_int,
    tls: impl Fn(Socket<'_>, &mut Spare<'_>) -> Option<usize>,
) -> Option<usize> {
    let (fd, _, _, secure) = socket.read(SocketFields::io_view);
    loop {
        let peeked = if secure {
            tls(socket, buffer)
        } else {
            buffer.recv(fd, flags | libc::MSG_PEEK)
        };
        if !(peeked.is_none() && interrupted()) {
            return peeked;
        }
    }
}

/// `Socket::what_event_want`: the event a pending TLS read or write waits for, else
/// `default`.
#[must_use]
pub fn what_event_want(
    socket: &SocketFields,
    default: crate::reactor::Events,
) -> crate::reactor::Events {
    let wants = socket.tls_wants();
    if !socket.ssl().is_null() && !wants.is_empty() {
        if wants.contains(crate::reactor::Events::WRITE) {
            crate::reactor::Events::WRITE
        } else {
            crate::reactor::Events::READ
        }
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::SocketRecord;
    use crate::reactor::lent::raw::{socket_record_peek, socket_record_recv, socket_record_send};
    use std::ffi::c_void;

    #[test]
    fn native_event_want_preserves_write_priority_and_default() {
        let mut socket = SocketRecord::new();
        for ssl in [false, true] {
            socket.set_ssl(ssl.then(crate::reactor::SocketSsl::stub));
            for read in [false, true] {
                for write in [false, true] {
                    for default in [-1, 0, 512, 1024] {
                        socket.tls.want_read = read;
                        socket.tls.want_write = write;
                        let expected = if ssl && write {
                            1024
                        } else if ssl && read {
                            512
                        } else {
                            default
                        };
                        assert_eq!(
                            what_event_want(&socket, crate::reactor::Events::from_bits(default)),
                            crate::reactor::Events::from_bits(expected)
                        );
                    }
                }
            }
        }
        // The stub goes unfreed.
        socket.set_ssl(None);
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
    #[cfg_attr(miri, ignore = "Miri's socketpair can't peek")]
    fn send_recv_and_peek_count_bytes_and_stamp_timers() {
        let [mut a, mut b] = pair();
        let timer = Some(crate::timer_key::Held::detached(1));
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
        a.output.nonblock = true;
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
        a.event_hup = true;
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
        let length = (size_of::<libc::sockaddr_in>()).c_cast::<libc::socklen_t>();
        // SAFETY: a live local record and sockaddr_in.
        unsafe {
            assert!(socket.bind(&{
                let mut local: SocketAddress = std::mem::zeroed();
                std::ptr::copy_nonoverlapping(
                    (&raw const address).cast::<u8>(),
                    (&raw mut local.addr).cast::<u8>(),
                    length as usize,
                );
                local.len = length;
                local
            }));
            assert!(socket.listen(0));
            assert!(socket.get_name());
            assert!(socket.set_tcp_nodelay(1));
            let mut peer = socket.info;
            assert!(!socket.get_peer_name(&mut peer));
            assert_eq!(peer.kind, 1);
        }
        assert_eq!(
            (socket.bound, socket.listened, socket.tcp_nodelay),
            (true, true, true)
        );
        assert_eq!(socket.info.kind, 1);
        assert_eq!(socket.info.len, length);
        socket.kernel_nobufs = true;
        assert!(socket.has_kernel_nobufs());
        assert!(!socket.has_kernel_nobufs());
        assert!(socket.set_fd_option(1, -1).ok);
        assert!(socket.set_kernel_timeout(true, 1.5));
        assert!(socket.set_kernel_timeout(false, 0.25));
        assert_eq!((socket.output.nonblock, socket.cloexec), (true, true));
        assert_eq!((socket.timeouts.write, socket.timeouts.read), (1.5, 0.25));
        close(&[socket]);
        let mut closed = SocketRecord::with_fd(-1, 0);
        assert!(!closed.listen(0));
        assert!(!closed.get_name());
        assert!(!closed.set_fd_option(1, 1).ok);
        assert!(!closed.set_kernel_timeout(true, 3.0));
        assert_eq!(
            (closed.listened, closed.output.nonblock, closed.cloexec),
            (false, false, false)
        );
        assert_eq!(closed.timeouts.write, 60.0);
    }
}
