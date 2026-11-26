//! The `coroutine::Socket` members of `src/coroutine/socket.cc` that are whole Rust
//! operations, as methods of the socket's handle ([`Co`]): the reactor callbacks, the
//! local and peer names, the timeouts and options, the buffered and packet reads,
//! the read and write buffers, `bind` of an address, `listen` and `accept`, and the
//! transfers (`recv`, `send`, their `_all` and `_once` forms, `read`, `write`,
//! `readv`, `writev` and their `_all` forms, `recvmsg`, `sendmsg`, `recvfrom`),
//! `peek` and `poll`. Only the read and write buffers keep their upstream symbols,
//! which crates/phpclients and crates/phphttp link by name.
use crate::buffered::{recv_buffered, recv_line};
use crate::co_io::{
    Message, Timer, available, borrowed_err, check, default_err, errno, ops, owned_err, set_err,
};
use crate::co_socket::{Co, OwnedCo, peer_name};
use crate::sendfile::{SendFailure, send_range};
use crate::setup::{Setup, swr_v1_hook_accept};
use crate::socket_wait::{error_event, ready, wait_for};
use crate::transfer::{ALL, At, Buffer, CHECKED, RETRY_INTERRUPTED, timeout_kind, transfer};
#[cfg(feature = "iouring")]
use crate::uring_socket as ring;
use crate::vector::{RecvMessage, SendMessage, Vectored, vector_all};
use core::ffi::{CStr, c_int};
use swoole_rs_corebase::constants::{SW_STACK_BUFFER_SIZE, SocketType};
use swoole_rs_corebase::lent::{Spare, lend_spare};
use swoole_rs_corebase::log;
use swoole_rs_corebase::reactor::Events;
use swoole_rs_corebase::reactor::{
    Drain, Event, Running, Socket, SocketAddress, SocketFields, SocketRecord, SpareVectors,
};
use swoole_rs_corebase::sys::{self, ReadSlot, strerror};
use swoole_rs_corebase::timer::Deadline;
use swoole_rs_corebase::tls::upstream::{
    peek_spare, recv, recv_spare, send, send_drain, sendfile as socket_sendfile_fd,
    socket_ssl_peer_certificate_pem,
};

/// Which socket receive fills a lent buffer.
#[derive(Clone, Copy)]
pub enum Receive {
    Recv,
    All,
    Line,
    Buffered,
    Peek,
    Peer,
}

/// The coroutine socket a reactor event came from, and its `network::Socket`: none
/// once the socket let go of it.
fn event_socket(event: &Event) -> Option<(Co, Socket<'_>)> {
    let source = event.socket()?;
    let socket = source.read(SocketFields::owner::<Co>)?;
    Some((socket, *source))
}

/// `Socket::readable_event_callback` and `writable_event_callback`: Rust picks the
/// coroutine to wake: the direction's own, a TLS wait's other direction, or none.
fn on_ready(event: &Event, kind: Events) {
    let Some((socket, source)) = event_socket(event) else {
        return;
    };
    set_err(socket, 0);
    let hup = source.read(SocketFields::hung_up);
    if let Some(event) = ready(socket, kind, hup) {
        (ops().resume)(socket, event);
    }
}

/// `Socket::readable_event_callback(reactor, event)`, registered for
/// `SW_FD_CO_SOCKET` during reactor initialization.
pub fn readable_event_callback(_: Running<'_>, event: &mut Event) -> bool {
    on_ready(event, Events::READ);
    true
}

/// `Socket::writable_event_callback(reactor, event)`.
pub fn writable_event_callback(_: Running<'_>, event: &mut Event) -> bool {
    on_ready(event, Events::WRITE);
    true
}

/// `Socket::error_event_callback(reactor, event)`: wakes the writer, then the
/// reader unless the writer closed the socket; the `network::Socket` outlives the wake-ups.
pub fn error_event_callback(_: Running<'_>, event: &mut Event) -> bool {
    if let Some((socket, source)) = event_socket(event) {
        error_event(socket, source);
    }
    true
}

/// What `accept`'s setup steps act on: the listening socket, its wait's timeout and
/// the connection accepted.
struct Accepting {
    socket: Co,
    timeout: f64,
    conn: Option<Box<SocketRecord>>,
}

impl Setup for Accepting {
    fn fail(&mut self, error: c_int) {
        set_err(self.socket, error);
    }

    fn wait(&mut self, event: Events) -> bool {
        let mut timer = Timer::new(self.socket, Events::READ, self.timeout);
        let ready = timer.start() && wait_for(self.socket, event);
        timer.stop();
        ready
    }

    fn accept(&mut self) -> std::io::Result<()> {
        // The new heap record, which nothing else holds.
        self.conn = self.socket.net(SocketFields::accepted).flatten();
        self.conn
            .as_ref()
            .map(|_| ())
            .ok_or_else(std::io::Error::last_os_error)
    }
}

/// `SW_ERROR_BAD_HOST_ADDR` and `SW_ERROR_SSL_EMPTY_PEER_CERTIFICATE`.
const BAD_HOST_ADDR: c_int = swoole_rs_error::ErrorCode::SW_ERROR_BAD_HOST_ADDR as c_int;
const SSL_EMPTY_PEER_CERTIFICATE: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SSL_EMPTY_PEER_CERTIFICATE as c_int;

/// `Address::assign(type, std::string(host), port, false)` into `address`; false
/// with the last error set.
pub(crate) fn assign(address: &mut SocketAddress, kind: c_int, host: &[u8], port: c_int) -> bool {
    address.kind = kind;
    let terminated = [host, &[0]].concat();
    swoole_rs_clients::address::assign(
        &mut address.addr,
        &mut address.len,
        kind,
        &terminated,
        port,
        false,
    )
    .map_err(log::set_last_error)
    .is_ok()
}

use swoole_rs_corebase::sys::setsockopt_bytes as setsockopt;

/// A descriptor `Socket::sendfile` opened, closed when it goes.
struct OpenFile(c_int);

impl Drop for OpenFile {
    fn drop(&mut self) {
        _ = sys::close(self.0);
    }
}

impl Co {
    /// `Socket::getsockname()`, into the open socket's `network::Socket`.
    #[must_use]
    pub fn getsockname(self) -> bool {
        self.net(SocketFields::get_name) == Some(true)
    }

    /// `Socket::getpeername(sa)`.
    pub fn getpeername(self, address: &mut SocketAddress) -> bool {
        match peer_name(self, address) {
            Ok(()) => true,
            Err(error) => {
                set_err(self, error.raw_os_error().unwrap_or(0));
                false
            }
        }
    }

    /// `Socket::recv_with_buffer(buf, n)` into `out`'s room, which is outside the
    /// socket's read buffer: the count it wrote there, or -1.
    pub fn recv_with_buffer(self, out: &mut Spare<'_>) -> Option<usize> {
        recv_buffered(self, out)
    }

    /// `Socket::recv_line(buf, maxlen)` of at most `out.room()` bytes, one buffered
    /// byte at a time, into `out`'s room, which is outside the socket's read buffer:
    /// the count it wrote there, or -1.
    pub fn recv_line(self, out: &mut Spare<'_>) -> Option<usize> {
        recv_line(out, |byte| recv_buffered(self, byte))
    }

    /// `Socket::set_option(level, optname, optval, optlen)`: `setsockopt`, warning on
    /// failure.
    #[must_use]
    pub fn set_option(self, level: c_int, name: c_int, value: &[u8]) -> bool {
        let fd = self.sock_fd();
        if setsockopt(fd, level, name, value).is_err() {
            log::sys_warning(
                c"Socket::set_option",
                format_args!(
                    "setsockopt({}, {level}, {name}, {}) failed",
                    self.sock_fd(),
                    value.len()
                ),
            );
            return false;
        }
        true
    }

    /// `Socket::set_option(level, optname, optval)` of an `int`.
    #[must_use]
    pub fn set_int_option(self, level: c_int, name: c_int, value: c_int) -> bool {
        self.set_option(level, name, &value.to_ne_bytes())
    }

    /// `Socket::get_option(level, optname, optval)` into an `int`: `getsockopt`,
    /// warning on failure.
    pub fn get_int_option(self, level: c_int, name: c_int, value: &mut c_int) -> bool {
        let fd = self.sock_fd();
        if swoole_rs_corebase::sys::getsockopt(fd, level, name, value).is_err() {
            log::sys_warning(
                c"Socket::get_option",
                format_args!("getsockopt({}, {level}, {name}) failed", self.sock_fd()),
            );
            return false;
        }
        true
    }

    /// `Socket::accept(timeout)`: a connection as a new native socket, which only the
    /// result holds (the accepting constructor, which shares the TLS context), or
    /// `None`. A zero timeout waits for the socket's read timeout; a TLS context is
    /// made before the first accept. In a coroutine.
    #[must_use]
    pub fn accept(self, timeout: f64) -> Option<OwnedCo> {
        if !available(self, Events::READ) {
            return None;
        }
        let needs_context = self.tls_context().is_some() && self.tls_settings().is_none();
        if needs_context && !self.ssl_context_create() {
            return None;
        }
        let client = self.accepted(timeout)?;
        if client.co().sock_fd() < 0 {
            let error = errno();
            log::sys_warning(c"Socket::accept", format_args!("new Socket() failed"));
            set_err(self, error);
            // The new native socket goes as it drops.
            return None;
        }
        Some(client)
    }

    /// `accept`'s connection as a new native socket. The socket is available.
    fn accepted(self, timeout: f64) -> Option<OwnedCo> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::accept(self, ops)) {
            return n;
        }
        let timeout = if timeout == 0.0 {
            self.read_timeout()
        } else {
            timeout
        };
        let mut state = Accepting {
            socket: self,
            timeout,
            conn: None,
        };
        if !swr_v1_hook_accept(&mut state) {
            return None;
        }
        state.conn.map(|conn| self.adopt(conn))
    }

    /// `new Socket(conn, server)`: the accepting constructor, then the server's TLS
    /// context shared with the new socket.
    pub(crate) fn adopt(self, conn: Box<SocketRecord>) -> OwnedCo {
        let client = crate::co_socket::accepted(self, conn);
        client.co().share_tls(self);
        client
    }

    /// `Socket::listen(backlog)`: `SW_BACKLOG` for a non-positive backlog; a listening
    /// socket's TLS is the server side.
    #[must_use]
    pub fn listen(self, backlog: c_int) -> bool {
        use swoole_rs_corebase::constants::SW_BACKLOG;
        // `SW_EVENT_NULL` skips the bound check.
        if !available(self, Events::NONE) {
            return false;
        }
        let backlog = if backlog <= 0 {
            SW_BACKLOG.cast_signed()
        } else {
            backlog
        };
        self.set_backlog(backlog);
        if !self.net(|record| record.listen(backlog)).unwrap_or(false) {
            set_err(self, errno());
            return false;
        }
        self.set_ssl_is_server(true);
        true
    }

    /// `Socket::transfer(event, buffer, length, flags, call)` of a read into `out`'s
    /// room: Rust's retry loop over `call`, which gets the `network::Socket` and `out`,
    /// whose room past what the tries before wrote is what is left to fill.
    fn receive(
        self,
        flags: u32,
        out: &mut Spare<'_>,
        mut call: impl FnMut(Socket<'_>, &mut Spare<'_>) -> Option<usize>,
    ) -> Option<usize> {
        let length = out.room();
        let mut each = |_| self.with_network(|s| call(s, out)).flatten();
        transfer(self, flags, Buffer::read(length), &mut each)
    }

    /// [`receive`](Self::receive) of a write of `buf`, sent from the write buffer once
    /// a wait moved the bytes there.
    fn deliver(
        self,
        flags: u32,
        buf: &[u8],
        mut call: impl FnMut(Socket<'_>, Drain<'_>) -> Option<usize>,
    ) -> Option<usize> {
        let mut each = |at| match at {
            At::Caller(skip) => {
                let mut rest = Drain::new(buf);
                rest.advance(skip);
                self.with_network(|s| call(s, rest)).flatten()
            }
            At::WriteBuffer => self.with_buffer(true, |moved| {
                self.with_network(|s| call(s, Drain::new(moved.view())))
                    .flatten()
            }),
        };
        transfer(self, flags, Buffer::write(buf), &mut each)
    }

    /// [`receive`](Self::receive) in `event`'s direction of a call that carries its
    /// own buffers.
    fn carry(
        self,
        event: Events,
        mut call: impl FnMut(Socket<'_>) -> Option<usize>,
    ) -> Option<usize> {
        let mut each = |_| self.with_network(&mut call).flatten();
        transfer(self, 0, Buffer::none(event), &mut each)
    }

    /// `Socket::bind(host, port)`: `SO_REUSEADDR` (a failure is only logged), bind to
    /// the host's literal address, then record the local name; false with the error
    /// set.
    #[must_use]
    pub fn bind(self, host: &[u8], port: c_int) -> bool {
        if !available(self, Events::NONE) {
            return false;
        }
        log::set_last_error(0);
        let enable: c_int = 1;
        let Some((kind, reuse)) = self.net(|network| {
            let reuse = network.set_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, &enable);
            (network.socket_type, reuse)
        }) else {
            return false;
        };
        if reuse.is_err() {
            log::sys_warning(
                c"bind",
                format_args!("setsockopt({}, SO_REUSEADDR) failed", self.sock_fd()),
            );
        }
        let mut address = SocketAddress::default();
        let bound = assign(&mut address, kind, host, port)
            && self.net(|record| record.bind(&address) && record.get_name()) == Some(true);
        if !bound {
            // The last error carries it too, as the Socket calls that set_err(int) do, so the
            // hooked socket_create_listen() leaves it for socket_last_error().
            let error = match log::last_error() {
                0 => errno(),
                error => error,
            };
            log::set_last_error(error);
            default_err(self, error);
        }
        bound
    }

    /// `Socket::sendto(host, port, buf, n)`: an available datagram socket sends to
    /// the host's address, resolving its name when it is not an address; -1 with the
    /// error set. In a coroutine.
    #[must_use]
    pub fn sendto(self, host: &[u8], port: c_int, buf: &[u8]) -> Option<usize> {
        if !available(self, Events::WRITE) {
            return None;
        }
        let (kind, domain) = (self.kind(), self.sock_domain());
        let (socket_type, timeout) = self
            .net(|network| (network.socket_type, network.dns_timeout()))
            .unwrap_or_default();
        if !SocketType::from_raw(socket_type).is_some_and(SocketType::is_dgram) {
            set_err(self, libc::EPROTONOSUPPORT);
            return None;
        }
        let mut address = SocketAddress::default();
        let mut assigned = assign(&mut address, kind, host, port);
        if !assigned && log::last_error() == BAD_HOST_ADDR {
            #[expect(
                clippy::disallowed_methods,
                reason = "the socket's dns_timeout, set from PHP"
            )]
            let ip =
                crate::resolve::system_gethostbyname(host, domain, Deadline::from_timeout(timeout));
            if !ip.is_empty() {
                assigned = assign(&mut address, kind, &ip, port);
            }
        }
        if !assigned {
            default_err(self, log::last_error());
            return None;
        }
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::sendto(ops, buf, &address)) {
            return n;
        }
        let call = |s: Socket<'_>, data: Drain<'_>| data.sendto(s.fd(), 0, &address);
        // Availability was checked above.
        self.deliver(RETRY_INTERRUPTED | CHECKED, buf, call)
    }

    /// The peer certificate's PEM, or empty with the error set when there is none.
    #[must_use]
    pub fn peer_cert(self) -> Vec<u8> {
        let mut pem = vec![0_u8; SW_STACK_BUFFER_SIZE as usize];
        let read = |socket: Socket<'_>| socket_ssl_peer_certificate_pem(socket, &mut pem);
        if let Some(length) = self.with_network(read).flatten() {
            pem.truncate(length);
            return pem;
        }
        set_err(self, SSL_EMPTY_PEER_CERTIFICATE);
        Vec::new()
    }

    /// The selected receive into `out`'s room, which counts what it wrote.
    #[inline]
    pub fn recv_spare(self, op: Receive, out: &mut Spare<'_>) -> Option<usize> {
        match op {
            Receive::Recv => self.recv(out),
            Receive::All => self.recv_all(out),
            Receive::Line => self.recv_line(out),
            Receive::Buffered => self.recv_with_buffer(out),
            Receive::Peek => self.peek(out),
            Receive::Peer => self.recvfrom_peer(out),
        }
    }

    /// `Socket::recv(buf, n)` into `out`'s room: the count it wrote there, or -1.
    pub fn recv(self, out: &mut Spare<'_>) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::recv(ops, out)) {
            return n;
        }
        self.receive(0, out, |s, out| recv_spare(s, out, 0))
    }

    /// `Socket::send(buf, n)`.
    #[must_use]
    pub fn send(self, buf: &[u8]) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::send(ops, ring::SEND, buf)) {
            return n;
        }
        self.deliver(0, buf, |s, data| send_drain(s, data, 0))
    }

    /// `Socket::recv_all(buf, n)` into `out`'s room: the count it wrote there, or -1.
    pub fn recv_all(self, out: &mut Spare<'_>) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::recv_all(ops, out)) {
            return n;
        }
        self.receive(ALL, out, |s, out| recv_spare(s, out, 0))
    }

    /// `Socket::send_all(buf, n)`.
    #[must_use]
    pub fn send_all(self, buf: &[u8]) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::send_all(ops, buf)) {
            return n;
        }
        self.deliver(ALL, buf, |s, data| send_drain(s, data, 0))
    }

    /// `Socket::read(buf, n)`.
    pub fn read(self, buf: &mut [impl ReadSlot]) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::read(ops, buf)) {
            return n;
        }
        let read = |s: Socket<'_>, out: &mut Spare<'_>| out.read_fd(s.fd());
        lend_spare(buf, |out| self.receive(0, out, read)).0
    }

    /// `Socket::write(buf, n)`.
    #[must_use]
    pub fn write(self, buf: &[u8]) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::send(ops, ring::WRITE, buf)) {
            return n;
        }
        self.deliver(0, buf, |s, data| data.write(s.fd()))
    }

    /// A vectored receive into lent rooms, which count what it wrote, with the
    /// cursor where it stopped.
    pub fn recv_spare_vectors(
        self,
        out: &mut SpareVectors<'_>,
        all: bool,
    ) -> Option<(usize, c_int, usize)> {
        let received = self.vector(out, all)?;
        let state = out.state();
        Some((received, state.index, state.offset_bytes))
    }

    /// `Socket::readv(io_vector)` or `writev`, in the direction the vector was made
    /// for, or with `all` `readv_all` or `writev_all`.
    #[must_use]
    pub fn vector(self, vector: &mut impl Vectored, all: bool) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::swr_v1_hook_uring_vector(ops, vector, all)) {
            return n;
        }
        let event = if vector.write() {
            Events::WRITE
        } else {
            Events::READ
        };
        if all {
            let call = |vector: &mut _| {
                self.with_network(|s| Vectored::transfer(vector, s))
                    .flatten()
            };
            return vector_all(self, event, vector, call, Vectored::remaining);
        }
        self.carry(event, |s| vector.transfer(s))
    }

    /// `set_err(error, message)` with owned text. A failed copy (allocation) leaves
    /// the previous message, as the code and errno are set.
    fn set_err_text(self, error: c_int, message: &str) {
        owned_err(self, error, message.as_bytes());
    }

    /// `Socket::sendfile(filename, offset, length)`: `length` bytes from `offset`, or
    /// the rest of the file when `length` is 0.
    #[must_use]
    pub fn sendfile(self, filename: &CStr, offset: libc::off_t, length: usize) -> bool {
        /// `SW_ERROR_SYSTEM_CALL_FAIL`.
        const SYSTEM_CALL_FAIL: c_int =
            swoole_rs_error::ErrorCode::SW_ERROR_SYSTEM_CALL_FAIL as c_int;
        if !available(self, Events::WRITE) {
            return false;
        }
        let name = filename.to_string_lossy();
        let Ok(fd) = sys::open(filename, libc::O_RDONLY, 0) else {
            let error = errno();
            self.set_err_text(error, &format!("open({name}) failed, {}", strerror(error)));
            return false;
        };
        let file = OpenFile(fd);
        let end = if length == 0 {
            let Some(stat) = sys::fstat(file.0) else {
                let error = errno();
                self.set_err_text(error, &format!("fstat({name}) failed, {}", strerror(error)));
                return false;
            };
            usize::try_from(stat.st_size).unwrap_or(0)
        } else {
            length.wrapping_add(usize::try_from(offset).unwrap_or(0))
        };
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::sendfile(ops, file.0, offset, end)) {
            return n;
        }
        let mut timer = Timer::new(
            self,
            Events::WRITE,
            self.timeout(timeout_kind(Events::WRITE)),
        );
        let mut offset = offset;
        let send = |offset: &mut libc::off_t, length| {
            self.with_network(|socket| socket_sendfile_fd(socket, file.0, offset, length))
                .flatten()
        };
        let ready = || timer.start() && wait_for(self, Events::WRITE);
        let outcome = send_range(&mut offset, end, send, ready);
        timer.stop();
        match outcome {
            Ok(()) => return true,
            Err(SendFailure::Zero) => borrowed_err(
                self,
                SYSTEM_CALL_FAIL,
                Message::Static(c"sendfile return zero"),
            ),
            Err(SendFailure::Failed(error)) => {
                let fd = self.sock_fd();
                self.set_err_text(
                    error,
                    &format!("sendfile({fd}, {name}) failed, {}", strerror(error)),
                );
            }
            Err(SendFailure::Wait) => {}
        }
        false
    }

    /// `Socket::recvfrom(buf, n, addr, addrlen)` into `buf`, the sender into the caller's
    /// `peer` and its length into `peer.len` (the storage's capacity when nothing came),
    /// retrying `EINTR` at once: the bytes received.
    pub fn recvfrom_into(
        self,
        buf: &mut [impl ReadSlot],
        peer: &mut SocketAddress,
    ) -> Option<usize> {
        peer.len = SocketAddress::CAPACITY;
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::recvfrom_into(ops, buf, peer)) {
            return n;
        }
        let fd = self.sock_fd();
        let receive = |_: Socket<'_>, out: &mut Spare<'_>| out.recvfrom_one(fd, 0, peer);
        lend_spare(buf, |out| self.receive(RETRY_INTERRUPTED, out, receive)).0
    }

    /// `Socket::recvfrom(buf, n)` into `out`'s room and the `network::Socket`'s peer
    /// address, retrying `EINTR` at once: the count it wrote there, or -1.
    pub fn recvfrom_peer(self, out: &mut Spare<'_>) -> Option<usize> {
        let receive = || {
            #[cfg(feature = "iouring")]
            // The ring's path fills the same record's peer address itself.
            if let Some(n) = ring::with(self, |ops| ring::recvfrom_peer(ops, out)) {
                return n;
            }
            let fd = self.sock_fd();
            self.receive(RETRY_INTERRUPTED, out, |s, out| {
                s.edit(|network| network.receive_from(out, fd, 0))
            })
        };
        crate::transfer::recvfrom_peer(self, receive)
    }

    /// `Socket::recvmsg(msg, flags)`.
    pub fn recvmsg(self, msg: &mut RecvMessage<'_>, flags: c_int) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ops.recvmsg(msg, flags)) {
            return n;
        }
        let fd = self.sock_fd();
        self.carry(Events::READ, |_| msg.recv(fd, flags))
    }

    /// `Socket::sendmsg(msg, flags)`.
    #[must_use]
    pub fn sendmsg(self, msg: &SendMessage<'_>, flags: c_int) -> Option<usize> {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ops.sendmsg(msg, flags)) {
            return n;
        }
        let fd = self.sock_fd();
        self.carry(Events::WRITE, |_| msg.send(fd, flags))
    }

    /// `Socket::recv_once(buf, n)`: one `network::Socket` receive, no wait; the count
    /// of `buf`'s first slots it wrote, or -1. The socket is open.
    pub fn recv_once(self, buf: &mut [impl ReadSlot]) -> Option<usize> {
        let mut io = || self.with_network(|socket| recv(socket, buf, 0)).flatten();
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::once(ops, &mut io)) {
            return n;
        }
        io()
    }

    /// `Socket::send_once(buf, n)`: one `network::Socket` send, no wait. The socket
    /// is open.
    #[must_use]
    pub fn send_once(self, buf: &[u8]) -> Option<usize> {
        let io = || self.with_network(|socket| send(socket, buf, 0)).flatten();
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::once(ops, io)) {
            return n;
        }
        io()
    }

    /// `Socket::peek(buf, n)` into `out`'s room: one `network::Socket` peek, then
    /// `check_return_value`; the count it wrote there, or -1. The socket is open.
    pub fn peek(self, out: &mut Spare<'_>) -> Option<usize> {
        let result = self
            .with_network(|socket| peek_spare(socket, out, 0))
            .flatten();
        check(self, result);
        result
    }

    /// `Socket::poll(event, timeout)`.
    #[must_use]
    pub fn poll(self, event: Events, timeout: f64) -> bool {
        #[cfg(feature = "iouring")]
        if let Some(n) = ring::with(self, |ops| ring::poll(ops, event, timeout)) {
            return n;
        }
        crate::socket_wait::poll(self, event, timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::test_host::{install, take};
    use crate::co_socket::{CoSocket, init};
    use core::ffi::c_long;
    use core::mem::MaybeUninit;

    /// A native `Socket`: the record, nothing more.
    #[repr(C)]
    struct Native {
        record: CoSocket,
    }

    /// The object, held raw: the network socket points back at it and the callbacks
    /// reach it through that pointer, so a box owning it would be an aliasing owner.
    struct Object(*mut Native);

    impl Object {
        fn new() -> Self {
            let native = Box::into_raw(Box::new(MaybeUninit::<Native>::uninit())).cast::<Native>();
            unsafe { init(&raw mut (*native).record) };
            let mut object = Self(native);
            let record = object.record();
            record.put_socket(swoole_rs_corebase::reactor::OwnedSocket::new(Box::new(
                SocketRecord::with_fd(-1, 6),
            )));
            record.set_test_fd(100);
            let co = object.co();
            socket_mut(object.record()).set_owner(Some(co));
            object
        }
        fn record(&mut self) -> &mut CoSocket {
            unsafe { &mut (*self.0).record }
        }
        fn co(&mut self) -> Co {
            unsafe { Co::new(self.0.cast()) }
        }
        fn event(&mut self) -> Event {
            Event::of(-1, 0, 6, unsafe {
                Socket::from_ptr(self.record().socket_ptr())
                    .map(swoole_rs_corebase::reactor::Heap::from_static)
            })
        }
        fn bind(&mut self, event: Events, co: *mut c_long) {
            self.co().with_state(|state| state.bind(event, co.cast()));
        }
    }

    impl Drop for Object {
        fn drop(&mut self) {
            unsafe {
                self.record()
                    .release_socket(swoole_rs_corebase::reactor::OwnedSocket::free);
                (*self.0).record.fini();
                drop(Box::from_raw(self.0));
            }
        }
    }

    fn socket_mut(socket: &mut CoSocket) -> &mut SocketFields {
        unsafe { &mut *socket.socket_ptr() }
    }

    #[test]
    fn the_record_is_the_native_socket() {
        let mut object = Object::new();
        assert_eq!(object.co().raw(), unsafe { &raw mut (*object.0).record });
    }

    #[test]
    fn a_readable_event_resumes_the_reader_and_clears_the_error() {
        install();
        let mut object = Object::new();
        object.record().err_code.set(libc::EAGAIN);
        let mut co: c_long = 3;
        object.bind(Events::READ, &raw mut co);
        let mut event = object.event();
        assert!(readable_event_callback(
            unsafe { Running::new(core::ptr::null_mut()) },
            &mut event
        ));
        assert_eq!(object.record().err_code.get(), 0);
        assert_eq!(take(), ["last-error:0", "resume:3"]);
        // Nobody writes: a writable event wakes no one.
        assert!(writable_event_callback(
            unsafe { Running::new(core::ptr::null_mut()) },
            &mut event
        ));
        assert_eq!(take(), ["last-error:0"]);
    }

    #[test]
    fn an_error_event_wakes_the_writer_then_the_reader() {
        install();
        let mut object = Object::new();
        let (mut reader, mut writer): (c_long, c_long) = (1, 2);
        object.bind(Events::READ, &raw mut reader);
        object.bind(Events::WRITE, &raw mut writer);
        socket_mut(object.record()).removed = false;
        let mut event = object.event();
        error_event_callback(unsafe { Running::new(core::ptr::null_mut()) }, &mut event);
        assert_eq!(
            take(),
            ["last-error:0", "resume:2", "last-error:0", "resume:1"]
        );
    }

    #[test]
    fn sendfile_reports_a_file_it_cannot_open() {
        install();
        let mut object = Object::new();
        let path = c"/nonexistent/swoole-rs-sendfile";
        assert!(!object.co().sendfile(path, 0, 0));
        assert_eq!(object.record().err_code.get(), libc::ENOENT);
        let message = unsafe { CStr::from_ptr(object.record().err_msg.get()) };
        assert_eq!(
            message.to_str().unwrap(),
            format!(
                "open({}) failed, {}",
                path.to_str().unwrap(),
                strerror(libc::ENOENT)
            )
        );
    }

    #[test]
    fn sendfile_of_an_empty_file_sends_nothing() {
        install();
        let mut object = Object::new();
        let path = std::env::temp_dir().join(format!("swoole-rs-sendfile-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert!(object.co().sendfile(&name, 0, 0));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn accept_reports_a_failed_accept_without_a_new_socket() {
        install();
        let mut object = Object::new();
        assert!(object.co().accept(0.0).is_none());
        assert_eq!(object.record().err_code.get(), libc::EBADF);
        assert!(!take().iter().any(|entry| entry == "accepted"));
    }

    #[test]
    fn timeouts_are_the_network_sockets() {
        let mut object = Object::new();
        let read = swoole_rs_corebase::reactor::TimeoutKind::READ;
        object.co().set_timeout(2.5, read);
        assert_eq!(object.co().timeout(read), 2.5);
    }

    #[test]
    fn listen_stores_the_backlog_and_reports_a_closed_socket() {
        install();
        let mut object = Object::new();
        object.record().set_test_fd(-1);
        assert!(!object.co().listen(0));
        assert_eq!(object.record().err_code.get(), libc::EBADF);
        assert_eq!(object.record().backlog.get(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no SOCK_DGRAM sockets")]
    fn options_go_to_the_network_sockets_descriptor() {
        let mut object = Object::new();
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        socket_mut(object.record()).fd = fd;
        let socket = object.co();
        assert!(socket.set_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, 1));
        let mut value = 0;
        assert!(socket.get_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, &mut value));
        assert_ne!(value, 0);
        unsafe { libc::close(fd) };
        socket_mut(object.record()).fd = -1;
        assert!(!socket.set_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, 1));
        assert!(!socket.get_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, &mut value));
    }
}
