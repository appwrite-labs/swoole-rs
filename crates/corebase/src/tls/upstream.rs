// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The TLS methods of `network::Socket` that are whole Rust operations, last
//! error, errno and logs included.
use crate::CCast;
use crate::buffer::string::StringBuffer;
use crate::constants::SocketFlag;
use crate::lent::{Spare, lend_spare};
use crate::net_address::{BAD_HOST_ADDR, assign, platform_parses};
use crate::reactor::socket_upstream::{
    make_typed, socket_cork, socket_set_fd_option, socket_uncork,
};
use crate::reactor::{
    Drain, Events, Fill, OutputEffects, RangeError, Socket, SocketAddress, SocketFields,
    SocketFileRequest, SslSendfileWarning, SyncEffects, SyncMode, handle_send_owned, record_peek,
    record_recv, record_send, sendfile_chunk, ssl_sendfile as ssl_sendfile_fd, steady_ms,
    swr_v1_reactor_poll_events_to, swr_v1_socket_sync, what_event_want,
};
use crate::sys::ReadSlot;
use crate::sys::Text;
use crate::tls::{
    Accept, Cert, CertRef, HandshakeLog, SocketTls, Ssl, SslIoResult, ssl_recv_into, ssl_shutdown,
    swr_v1_socket_ssl_accept, swr_v1_socket_ssl_clear, swr_v1_socket_ssl_connect,
    swr_v1_socket_ssl_error_reason, swr_v1_socket_ssl_peek_reason, swr_v1_socket_ssl_send,
    swr_v1_socket_ssl_verify,
};
use crate::{log, set_errno};
use openssl_sys::{X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK};
use std::ffi::{CStr, c_char, c_int, c_long};
use std::io::IoSlice;
use std::mem::MaybeUninit;
use swoole_rs_error::ErrorCode;

// libcrypto's own declaration: it reads only the code, and answers one of its
// static strings.
unsafe extern "C" {
    safe fn X509_verify_cert_error_string(error: c_long) -> Option<Text<'static>>;
}

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING`, `SW_ERROR_FILE_EMPTY` and `SW_ERROR_INVALID_PARAMS`.
const FILE_EMPTY: c_int = swoole_rs_error::ErrorCode::SW_ERROR_FILE_EMPTY as c_int;
const INVALID_PARAMS: c_int = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as c_int;
/// `SW_SOCKET_CORK_MIN_SIZE`.
const CORK_MIN_SIZE: usize = 65536;
/// `SW_FD_STREAM_SERVER`.
const FD_STREAM_SERVER: c_int = 1;
const VERIFY_FAILED: c_int = ErrorCode::SW_ERROR_SSL_VERIFY_FAILED as c_int;
const BAD_PROTOCOL: c_int = ErrorCode::SW_ERROR_SSL_BAD_PROTOCOL as c_int;

use crate::sys::errno;

/// `Address::get_addr()` and `get_port()` of the socket's own address, as its logs print them.
#[must_use]
pub fn peer(info: &SocketAddress) -> (String, c_int) {
    // The address in network order, as `inet_ntop` reads it.
    let (family, address, port): (c_int, &[u8], u16) = match info.kind {
        // TCP, UDP and raw sockets hold the IPv4 member.
        1 | 2 | 7 => {
            let v4 = info.addr.v4();
            (
                libc::AF_INET,
                &v4.sin_addr.s_addr.to_ne_bytes(),
                v4.sin_port,
            )
        }
        // TCP6, UDP6 and raw6 sockets hold the IPv6 member.
        3 | 4 | 8 => {
            let v6 = info.addr.v6();
            (libc::AF_INET6, &v6.sin6_addr.s6_addr, v6.sin6_port)
        }
        5 | 6 => {
            // Unix sockets hold the Unix member, whose path `%s` prints up to its NUL.
            let path = info.addr.unix().sun_path;
            let path: Vec<u8> = path
                .iter()
                .flat_map(|unit| unit.to_ne_bytes())
                .take_while(|&byte| byte != 0)
                .collect();
            return (String::from_utf8_lossy(&path).into_owned(), 0);
        }
        _ => return ("unknown".into(), 0),
    };
    let mut binary = [0; 16];
    for (to, from) in binary.iter_mut().zip(address) {
        *to = c_char::from_ne_bytes([*from]);
    }
    // inet_ntop's NUL-terminated text, or `(null)` as `%s` prints its null.
    let host = crate::ntop(family, &binary).map_or_else(
        || "(null)".into(),
        |text| {
            let text: Vec<u8> = text
                .iter()
                .flat_map(|unit| unit.to_ne_bytes())
                .take_while(|&byte| byte != 0)
                .collect();
            String::from_utf8_lossy(&text).into_owned()
        },
    );
    (host, c_int::from(u16::from_be(port)))
}

/// `step` over a copy of the record's TLS group, its SSL (None without one) and what
/// `pick` reads in the same borrow, then the group stored back: OpenSSL's info
/// callback reads the record meanwhile.
fn with_tls<P, R>(
    socket: Socket<'_>,
    pick: impl FnOnce(&SocketFields) -> P,
    step: impl FnOnce(&mut SocketTls, Ssl, P) -> R,
) -> Option<R> {
    let (ssl, (before, picked)) = Ssl::of_reading(socket, |record| (record.tls, pick(record)));
    let held = ssl?;
    let ssl = held.get();
    let mut tls = before;
    let result = step(&mut tls, ssl, picked);
    // Most transfers leave the group as it was, and then the record needs no edit.
    if tls != before {
        socket.edit(|record| record.tls = tls);
    }
    Some(result)
}

/// The peer's certificate chain, each certificate as PEM (at most
/// `SW_STACK_BUFFER_SIZE` bytes), skipping one that fails.
#[must_use]
pub fn socket_ssl_peer_chain_pem(socket: Socket<'_>) -> Vec<Vec<u8>> {
    let mut chain = Vec::new();
    let Some(held) = Ssl::of(socket) else {
        return chain;
    };
    let mut pem = vec![0_u8; crate::constants::SW_STACK_BUFFER_SIZE as usize];
    let ssl = held.get();
    ssl.each_peer_cert(|cert| {
        let read = lend_spare(pem.as_mut_slice(), |spare| read_x509(cert, spare)).0;
        if let Some(bytes) = read.filter(|&n| n > 0) {
            chain.extend(pem.get(..bytes).map(<[u8]>::to_vec));
        }
    });
    chain
}

/// A connection's negotiated protocol and cipher. Before the handshake picks a
/// cipher, OpenSSL names it `(NONE)` with 0 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    /// `SSL_version`: `TLS1_3_VERSION` and the like.
    pub version: c_int,
    pub cipher_name: &'static CStr,
    pub cipher_bits: c_int,
    pub cipher_version: &'static CStr,
}

/// The session of `socket`'s SSL, or None without one.
#[must_use]
pub fn socket_ssl_session(socket: Socket<'_>) -> Option<Session> {
    let held = Ssl::of(socket)?;
    let ssl = held.get();
    let cipher = ssl.cipher();
    Some(Session {
        version: ssl.version(),
        cipher_name: cipher.name,
        cipher_bits: cipher.bits,
        cipher_version: cipher.version,
    })
}

/// `bool Socket::ssl_verify(bool allow_self_signed) const`: false without an SSL.
#[must_use]
pub fn socket_ssl_verify(socket: Socket<'_>, allow_self_signed: bool) -> bool {
    let Some(held) = Ssl::of(socket) else {
        return false;
    };
    let error = swr_v1_socket_ssl_verify(held.get(), allow_self_signed);
    if error == c_long::from(X509_V_OK) {
        return true;
    }
    let fd = socket.fd();
    if error == c_long::from(X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT) {
        log::error_log(
            LOG_NOTICE,
            VERIFY_FAILED,
            c"Socket::ssl_verify",
            format_args!("self signed certificate from fd#{fd} is not allowed"),
        );
    } else {
        // `%s`, which glibc prints as `(null)` when null.
        let reason = X509_verify_cert_error_string(error)
            .map_or("(null)".into(), |reason| reason.get().to_string_lossy());
        log::error_log(
            LOG_NOTICE,
            VERIFY_FAILED,
            c"Socket::ssl_verify",
            format_args!("can not verify peer from fd#{fd} with error#{error}: {reason}"),
        );
    }
    false
}

/// `void Socket::ssl_catch_error() const`, out of line so that `ssl_io`, on every
/// TLS read and write, stays small enough to inline.
#[cold]
#[inline(never)]
fn ssl_catch_error(socket: Socket<'_>) {
    let ((host, port), fd) = socket.read(|record| (peer(&record.info), record.fd));
    let reason = swr_v1_socket_ssl_peek_reason();
    log::error_log(
        LOG_NOTICE,
        BAD_PROTOCOL,
        c"Socket::ssl_catch_error",
        format_args!("SSL connection#{fd}[{host}:{port}] protocol error[{reason}]"),
    );
}

/// A transfer's byte count, or `None` for its -1.
fn count(n: isize) -> Option<usize> {
    usize::try_from(n).ok()
}

/// `ssl_io`: the protocol error's log, then errno; None on error or without an SSL.
#[inline(always)]
fn ssl_io(socket: Socket<'_>, result: Option<SslIoResult>) -> Option<usize> {
    let result = result?;
    if result.protocol_error {
        ssl_catch_error(socket);
    }
    if result.error != 0 {
        set_errno(result.error);
    }
    count(result.value)
}

/// `Socket::ssl_recv`: the record's TLS read into the bytes `buffer` lends; None on
/// error or without an SSL.
#[must_use]
pub fn ssl_recv_fill<S: ReadSlot>(socket: Socket<'_>, mut buffer: Fill<'_, S>) -> Option<usize> {
    buffer.spare(|spare| ssl_recv_spare(socket, spare))
}

/// [`ssl_recv_fill`] into `spare`'s room.
#[inline]
pub fn ssl_recv_spare(socket: Socket<'_>, spare: &mut Spare<'_>) -> Option<usize> {
    let result = with_tls(
        socket,
        |_| (),
        |tls, ssl, ()| ssl_recv_into(tls, ssl, spare),
    );
    ssl_io(socket, result)
}

/// `Socket::ssl_send`: the record's TLS write of the bytes `data` lends, of which a
/// DTLS socket sends at most `chunk_size`; None on error or without an SSL.
#[must_use]
pub fn ssl_send_drain(socket: Socket<'_>, data: Drain<'_>) -> Option<usize> {
    let result = with_tls(
        socket,
        |record| {
            let chunk = record.chunk_size as usize;
            if record.dtls && chunk != 0 {
                chunk
            } else {
                usize::MAX
            }
        },
        |tls, ssl, limit| swr_v1_socket_ssl_send(tls, ssl, data.first(limit).bytes()),
    );
    ssl_io(socket, result)
}

/// `ssize_t Socket::ssl_recv(void *buf, size_t n)` into `buffer`.
pub fn ssl_recv(socket: Socket<'_>, buffer: &mut [impl ReadSlot]) -> Option<usize> {
    ssl_recv_fill(socket, Fill::new(buffer))
}

/// `ssize_t Socket::ssl_send(const void *buf, size_t n)` of `data`.
#[must_use]
pub fn ssl_send(socket: Socket<'_>, data: &[u8]) -> Option<usize> {
    ssl_send_drain(socket, Drain::new(data))
}

fn handshake(last_error: c_int) {
    if last_error != 0 {
        log::set_last_error(last_error);
    }
}

/// `ssl_get_error_reason`'s reason code and text, as warnings print them.
fn error_reason_text() -> (c_int, String) {
    let (reason, text) = swr_v1_socket_ssl_error_reason();
    let text = text.map_or_else(|| "(null)".into(), |text| text.to_string_lossy().into());
    (reason, text)
}

/// `ReturnCode Socket::ssl_accept()`: `SW_ERROR` without an SSL.
#[must_use]
pub fn socket_ssl_accept(socket: Socket<'_>) -> Accept {
    let Some(result) = with_tls(socket, |record| record.dtls, swr_v1_socket_ssl_accept) else {
        return Accept::Error;
    };
    handshake(result.last_error);
    if result.log == HandshakeLog::BadClient {
        let (reason, error) = error_reason_text();
        let (host, port) = socket.read(|record| peer(&record.info));
        log::warning(
            c"Socket::ssl_accept",
            format_args!("bad SSL client[{host}:{port}], reason={reason}, error_string={error}"),
        );
    } else if let HandshakeLog::Failed(ssl_error) = result.log {
        let code = errno();
        let message = crate::sys::strerror(code);
        log::warning(
            c"Socket::ssl_accept",
            format_args!("SSL_do_handshake() failed. Error: {message}[{ssl_error}|{code}]"),
        );
    }
    result.code
}

/// `int Socket::ssl_connect()` is `SW_OK`: false without an SSL.
#[must_use]
pub fn socket_ssl_connect(socket: Socket<'_>) -> bool {
    let Some(result) = with_tls(
        socket,
        |_| (),
        |tls, ssl, ()| swr_v1_socket_ssl_connect(tls, ssl),
    ) else {
        return false;
    };
    handshake(result.last_error);
    if let HandshakeLog::Failed(ssl_error) = result.log {
        let error = crate::tls::pop_error();
        let message = crate::tls::error_string(error);
        let ((host, port), fd) = socket.read(|record| (peer(&record.info), record.fd));
        log::notice(format_args!(
            "ssl_connect(fd={fd}) to server[{host}:{port}] failed. Error: {message}[{ssl_error}|{}]",
            crate::tls::socket::error_reason(error)
        ));
    }
    result.code
}

/// `SSL_peek` on the record into `buffer`; None on error or without an SSL.
fn tls_peek(socket: Socket<'_>, buffer: &mut Spare<'_>) -> Option<usize> {
    match Ssl::of(socket)?.get().peek(buffer) {
        Ok(count) => Some(count),
        Err(failure) => usize::try_from(failure.returned).ok(),
    }
}

/// `ssize_t Socket::recv(void *, size_t, int)` into the bytes `buffer` lends: a slice's
/// ([`recv`]), or the address and length C handed its holder.
#[must_use]
pub fn recv_fill<S: ReadSlot>(
    socket: Socket<'_>,
    mut buffer: Fill<'_, S>,
    flags: c_int,
) -> Option<usize> {
    buffer.spare(|spare| recv_spare(socket, spare, flags))
}

/// [`recv_fill`] into `spare`'s room.
#[inline]
pub fn recv_spare(socket: Socket<'_>, spare: &mut Spare<'_>, flags: c_int) -> Option<usize> {
    record_recv(socket, spare, flags, ssl_recv_spare, steady_ms)
}

/// `ssize_t Socket::send(const void *, size_t, int)` of the bytes `data` lends, as
/// [`recv_fill`] receives.
#[must_use]
pub fn send_drain(socket: Socket<'_>, data: Drain<'_>, flags: c_int) -> Option<usize> {
    record_send(socket, data, flags, ssl_send_drain, steady_ms)
}

/// [`recv_fill`] into `buffer`.
pub fn recv(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
    lend_spare(buffer, |spare| recv_spare(socket, spare, flags)).0
}

/// One [`recv`] into `buffer`'s room past its content, which then counts what it
/// wrote: what `recv` returned.
#[inline]
pub fn recv_append(socket: Socket<'_>, buffer: &mut StringBuffer) -> Option<usize> {
    buffer.fill(|spare| recv_spare(socket, spare, 0))
}

/// [`send_drain`] of `data`.
#[must_use]
pub fn send(socket: Socket<'_>, data: &[u8], flags: c_int) -> Option<usize> {
    record_send(socket, Drain::new(data), flags, ssl_send_drain, steady_ms)
}

/// [`recv_sync_fill`] into `buffer`.
pub fn recv_sync(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
    lend_spare(buffer, |spare| recv_sync_spare(socket, spare, flags)).0
}

/// [`recv_sync_fill`] into `spare`'s room, which counts what each receive wrote.
pub fn recv_sync_spare(socket: Socket<'_>, spare: &mut Spare<'_>, flags: c_int) -> Option<usize> {
    sync(socket, Io::<MaybeUninit<u8>>::Recv(spare), flags)
}

/// [`send_sync_drain`] of `data`.
#[must_use]
pub fn send_sync(socket: Socket<'_>, data: &[u8], flags: c_int) -> Option<usize> {
    sync_out(socket, Io::Send(Drain::new(data)), flags)
}

/// [`read_sync_fill`] into `buffer`.
pub fn read_sync(socket: Socket<'_>, buffer: &mut [impl ReadSlot]) -> Option<usize> {
    sync(socket, Io::Read(Fill::new(buffer)), 0)
}

/// `ssize_t Socket::write_sync(const void *, size_t)` of `data`.
#[must_use]
pub fn write_sync(socket: Socket<'_>, data: &[u8]) -> Option<usize> {
    sync_out(socket, Io::Write(Drain::new(data)), 0)
}

/// `ssize_t Socket::sendto_sync(const Address &, const void *, size_t, int)` of `data`.
#[must_use]
pub fn sendto_sync(
    socket: Socket<'_>,
    address: &SocketAddress,
    data: &[u8],
    flags: c_int,
) -> Option<usize> {
    let io = Io::SendTo(address, Drain::new(data));
    sync_out(socket, io, flags)
}

/// `ssize_t Socket::recvfrom_sync(char *, size_t, int, Address *)` into `buffer`, and
/// the peer into `address`'s storage and length.
pub fn recvfrom_sync(
    socket: Socket<'_>,
    buffer: &mut [impl ReadSlot],
    flags: c_int,
    address: &mut SocketAddress,
) -> Option<usize> {
    lend_spare(buffer, |spare| {
        recvfrom_sync_spare(socket, spare, flags, address)
    })
    .0
}

/// [`recvfrom_sync`] into `spare`'s room, which counts what the receive wrote.
pub fn recvfrom_sync_spare(
    socket: Socket<'_>,
    spare: &mut Spare<'_>,
    flags: c_int,
    address: &mut SocketAddress,
) -> Option<usize> {
    sync(
        socket,
        Io::<MaybeUninit<u8>>::RecvFrom(address, spare),
        flags,
    )
}

/// `ssize_t Socket::peek(void *, size_t, int) const` into `buffer`.
pub fn peek(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
    lend_spare(buffer, |spare| peek_spare(socket, spare, flags)).0
}

/// [`peek`] into `spare`'s room, which counts what it wrote.
pub fn peek_spare(socket: Socket<'_>, spare: &mut Spare<'_>, flags: c_int) -> Option<usize> {
    record_peek(socket, spare, flags, tls_peek)
}

/// `bool Socket::check_liveness()`: a nonblocking one-byte peek that neither
/// ends the stream nor fails with an error that closes it.
#[must_use]
pub fn check_liveness(socket: Socket<'_>) -> bool {
    let mut byte = [0u8];
    set_errno(0);
    let peeked = peek(socket, &mut byte, libc::MSG_DONTWAIT);
    !(peeked == Some(0)
        || (peeked.is_none() && socket.catch_error(errno()) == crate::reactor::Caught::Close))
}

/// One blocking call: the socket, and its I/O with the bytes it has left.
struct Call<'a, 's, S: ReadSlot> {
    socket: Socket<'a>,
    io: Io<'a, 's, S>,
    dont_restart: bool,
}

/// The I/O a blocking call repeats, over the bytes and operands it was lent. A
/// receive fills a [`Spare`], which counts what each call wrote.
enum Io<'a, 's, S: ReadSlot = MaybeUninit<u8>> {
    Send(Drain<'a>),
    Recv(&'a mut Spare<'s>),
    Read(Fill<'a, S>),
    Write(Drain<'a>),
    SendTo(&'a SocketAddress, Drain<'a>),
    RecvFrom(&'a mut SocketAddress, &'a mut Spare<'s>),
    Writev(&'a [IoSlice<'a>]),
    /// The request, and how many of its bytes are still to send.
    Sendfile(&'a mut SocketFileRequest, usize),
}

use crate::log::LogLevel::{SW_LOG_NOTICE as LOG_NOTICE, SW_LOG_WARNING as LOG_WARNING};
pub use crate::sys::writev;

impl<S: ReadSlot> SyncEffects for Call<'_, '_, S> {
    /// Read on each wait, as upstream's `wait_event` reads `this->fd`: a signal
    /// handler an earlier wait dispatched may have changed it.
    fn fd(&self) -> c_int {
        self.socket.fd()
    }

    fn dont_restart(&self) -> bool {
        self.dont_restart
    }

    /// Edits only what it changes: threads share some records (a pool's pipe socket),
    /// and a write to a nonblocking plain socket only reads it.
    fn prepare(&self) {
        self.socket.edit(|record| {
            if !record.output.nonblock {
                socket_set_fd_option(record, 1, -1);
            }
            if !record.ssl().is_null() {
                swr_v1_socket_ssl_clear(&mut record.tls);
            }
        });
    }

    fn events(&self, event: crate::reactor::Events) -> i16 {
        swr_v1_reactor_poll_events_to(self.socket.read(|record| what_event_want(record, event)))
    }

    fn io(&mut self, flags: c_int) -> Option<usize> {
        let socket = self.socket;
        // Read on each call: a signal handler the wait dispatches may have changed it.
        let fd = socket.fd();
        match &mut self.io {
            Io::Send(data) => record_send(socket, *data, flags, ssl_send_drain, steady_ms),
            Io::Recv(spare) => recv_spare(socket, spare, flags),
            Io::Read(buffer) => buffer.read(fd),
            Io::Write(data) => data.write(fd),
            Io::SendTo(address, data) => data.sendto(fd, flags, address),
            Io::RecvFrom(address, spare) => spare.recvfrom(fd, flags, address),
            Io::Sendfile(request, _) => send_chunk(socket, request).0,
            Io::Writev(vectors) => writev(fd, vectors),
        }
    }

    fn advance(&mut self, moved: usize) -> bool {
        match &mut self.io {
            Io::Send(data) | Io::Write(data) | Io::SendTo(_, data) => data.advance(moved),
            Io::Read(buffer) => buffer.advance(moved),
            Io::Recv(spare) | Io::RecvFrom(_, spare) => spare.room() > 0,
            Io::Sendfile(_, left) => {
                *left = left.saturating_sub(moved);
                *left > 0
            }
            Io::Writev(_) => false,
        }
    }

    fn catch_error(&self, error: c_int) -> crate::reactor::Caught {
        self.socket.catch_error(error)
    }

    fn take_nobufs(&self) -> bool {
        self.socket.take_kernel_nobufs()
    }

    fn set_last_error(&self, error: c_int) {
        log::swr_v1_core_set_last_error(error);
    }
}

/// `Socket::wait_for`'s loop (`crates/corebase/src/reactor/socket_sync.rs`) around `io`,
/// waiting for writability (else readability) up to the socket's timeout.
fn sync<S: ReadSlot>(socket: Socket<'_>, io: Io<'_, '_, S>, flags: c_int) -> Option<usize> {
    let (mode, event) = match &io {
        Io::Send(_) => (SyncMode::Send, Events::WRITE),
        Io::Recv(_) => (SyncMode::Recv, Events::READ),
        Io::Sendfile(..) => (SyncMode::Sendfile, Events::WRITE),
        Io::Read(_) | Io::RecvFrom(..) => (SyncMode::Once, Events::READ),
        Io::Write(_) | Io::Writev(_) | Io::SendTo(..) => (SyncMode::Once, Events::WRITE),
    };
    let (timeout, dont_restart) = socket.read(|record| {
        let timeouts = record.timeouts;
        let timeout = if event == Events::WRITE {
            timeouts.write
        } else {
            timeouts.read
        };
        (timeout, record.dont_restart)
    });
    let mut call = Call {
        socket,
        io,
        dont_restart,
    };
    let timeout_ms = (timeout * 1000.0).c_cast::<c_long>().c_cast::<c_int>();
    swr_v1_socket_sync(&mut call, mode, flags, event, timeout_ms)
}

/// [`sync`] of an I/O that fills nothing, waiting for writability.
fn sync_out(socket: Socket<'_>, io: Io<'_, '_>, flags: c_int) -> Option<usize> {
    sync(socket, io, flags)
}

/// `ssize_t Socket::send_sync(const void *, size_t, int)` of the bytes `data` lends.
#[must_use]
pub fn send_sync_drain(socket: Socket<'_>, data: Drain<'_>, flags: c_int) -> Option<usize> {
    sync_out(socket, Io::Send(data), flags)
}

/// `ssize_t Socket::recv_sync(void *, size_t, int)` into the bytes `buffer` lends:
/// `MSG_WAITALL` is cleared for each call, since under nonblocking I/O it would spin on
/// EAGAIN.
#[must_use]
pub fn recv_sync_fill(socket: Socket<'_>, mut buffer: Fill<'_>, flags: c_int) -> Option<usize> {
    buffer.spare(|spare| recv_sync_spare(socket, spare, flags))
}

/// `ssize_t Socket::read_sync(void *, size_t)` into the bytes `buffer` lends.
#[must_use]
pub fn read_sync_fill(socket: Socket<'_>, buffer: Fill<'_>) -> Option<usize> {
    sync(socket, Io::Read(buffer), 0)
}

/// `ssize_t Socket::writev_sync(const iovec *, size_t)` of `vectors`.
#[must_use]
pub fn writev_sync(socket: Socket<'_>, vectors: &[IoSlice<'_>]) -> Option<usize> {
    sync_out(socket, Io::Writev(vectors), 0)
}

/// `X509 *Socket::ssl_get_peer_certificate() const`.
#[must_use]
pub fn ssl_get_peer_certificate(socket: Socket<'_>) -> Option<Cert> {
    Ssl::of(socket)?.get().peer_certificate()
}

/// `bool Socket::ssl_check_host(const char *tls_host_name) const`.
#[must_use]
pub fn socket_ssl_check_host(socket: Socket<'_>, name: &CStr) -> bool {
    let Some(cert) = ssl_get_peer_certificate(socket) else {
        return false;
    };
    if cert.get().matches_host(name) {
        return true;
    }
    log::warning(
        c"Socket::ssl_check_host",
        format_args!("X509_check_host(): no match"),
    );
    false
}

/// `int Socket::ssl_get_peer_certificate(char *buf, size_t n) const`: the peer
/// certificate's PEM into `buffer`: its length, or `None` (upstream's `SW_ERR`).
pub fn socket_ssl_peer_certificate_pem(
    socket: Socket<'_>,
    buffer: &mut [impl ReadSlot],
) -> Option<usize> {
    lend_spare(buffer, |spare| peer_certificate_pem(socket, spare)).0
}

/// [`socket_ssl_peer_certificate_pem`] into `spare`'s room.
fn peer_certificate_pem(socket: Socket<'_>, spare: &mut Spare<'_>) -> Option<usize> {
    ssl_get_peer_certificate(socket).and_then(|cert| read_x509(cert.get(), spare))
}

/// `_ssl_read_x509_file` without consuming: the certificate as PEM into `spare`'s
/// room, warning as upstream does when a step before the read fails.
fn read_x509(cert: CertRef<'_>, spare: &mut Spare<'_>) -> Option<usize> {
    let step = match cert.pem_into(spare) {
        Ok(written) => return Some(written),
        Err(crate::tls::PemError::Read) => return None,
        Err(crate::tls::PemError::Bio) => "BIO_new",
        Err(crate::tls::PemError::Write) => "PEM_write_bio_X509",
    };
    log::warning(
        c"network::_ssl_read_x509_file",
        format_args!("{step}() failed"),
    );
    None
}

/// `bool Socket::ssl_get_peer_certificate(String *buf) const`: the peer certificate's
/// PEM as `buffer`'s content, written from the start of its allocation; false, with
/// the content's length as it was, without one.
pub fn socket_ssl_peer_certificate_string(socket: Socket<'_>, buffer: &mut StringBuffer) -> bool {
    buffer
        .refill(|spare| peer_certificate_pem(socket, spare))
        .is_some()
}

/// `Socket::ssl_sendfile` over a descriptor.
fn ssl_sendfile(
    socket: Socket<'_>,
    fd: c_int,
    offset: &mut libc::off_t,
    size: usize,
) -> Option<usize> {
    let mut nobufs = false;
    let send = |data: &[u8]| ssl_send_drain(socket, Drain::new(data));
    let result = ssl_sendfile_fd(fd, offset, size, send, &mut nobufs);
    if nobufs {
        socket.edit(|record| record.kernel_nobufs = true);
    }
    if let Some(warning) = result.warn {
        let call = match warning {
            SslSendfileWarning::Write => "write",
            SslSendfileWarning::Pread => "pread",
        };
        log::sys_warning(c"Socket::ssl_sendfile", format_args!("{call}() failed"));
    }
    result.value
}

/// `Socket::sendfile` over a descriptor: through OpenSSL when the socket holds an
/// SSL, else the platform's sendfile: how many bytes it sent, or None with errno set.
pub fn sendfile(
    socket: Socket<'_>,
    fd: c_int,
    offset: &mut libc::off_t,
    length: usize,
) -> Option<usize> {
    let (out, ssl) = socket.read(|record| (record.fd, record.ssl()));
    if !ssl.is_null() {
        return ssl_sendfile(socket, fd, offset, length);
    }
    // `swoole_sendfile` (src/os/sendfile.cc): the platform's sendfile.
    let sent = crate::ossupport::swr_v1_os_sendfile(out, fd, Some(offset), length);
    if cfg!(not(target_os = "linux")) && sent.warn {
        log::sys_warning(c"swoole_sendfile", format_args!("sendfile failed"));
    }
    count(sent.result)
}

/// One chunk of `request`'s remaining range through `sendfile`, advancing its
/// offset: what `sendfile` returned and the chunk's size.
fn send_chunk(socket: Socket<'_>, request: &mut SocketFileRequest) -> (Option<usize>, usize) {
    let fd = request.descriptor();
    let fields = request.fields();
    let chunk = sendfile_chunk(fields.begin, fields.end);
    (sendfile(socket, fd, &mut fields.begin, chunk), chunk)
}

/// `check_sendfile_parameters`'s origin.
const CHECK_ORIGIN: &CStr = c"network::check_sendfile_parameters";

/// A sendfile request for `filename` from `offset`, checked against `length` with
/// `check_sendfile_parameters`'s warnings; `None` when it fails.
fn file_request(
    filename: &CStr,
    offset: libc::off_t,
    length: usize,
) -> Option<Box<SocketFileRequest>> {
    let request = SocketFileRequest::open(filename, offset);
    assert!(request.is_some(), "out of memory for a sendfile request");
    let mut request = request?;
    let Err(error) = request.check(length) else {
        return Some(request);
    };
    let name = filename.to_string_lossy();
    match error {
        RangeError::Open => log::sys_warning(CHECK_ORIGIN, format_args!("open('{name}') failed")),
        RangeError::Stat => {
            log::sys_warning(c"File::stat", format_args!("fstat() failed"));
            log::sys_warning(CHECK_ORIGIN, format_args!("fstat('{name}') failed"));
        }
        RangeError::Empty => log::error_log(
            LOG_WARNING,
            FILE_EMPTY,
            CHECK_ORIGIN,
            format_args!("cannot send empty file '{name}'"),
        ),
        RangeError::Outside => {
            let begin = request.fields().begin;
            log::error_log(
                LOG_WARNING,
                INVALID_PARAMS,
                CHECK_ORIGIN,
                format_args!("length[{length}] or offset[{begin}] is invalid"),
            );
        }
    }
    None
}

/// Sends the requested file range synchronously, corking the socket for large transfers.
#[must_use]
pub fn sendfile_sync(
    socket: Socket<'_>,
    filename: &CStr,
    offset: libc::off_t,
    length: usize,
) -> bool {
    let Some(mut request) = file_request(filename, offset, length) else {
        return false;
    };
    let total = request.length();
    let corked = total > CORK_MIN_SIZE && socket.edit(socket_cork);
    let io = Io::Sendfile(&mut request, total);
    let rv = sync_out(socket, io, 0);
    if corked {
        socket.edit(socket_uncork);
    }
    rv.is_some()
}

/// A failed Rust queue status. Native `Buffer` raises it as a C++ exception that
/// nothing catches, so the process ends either way; here it aborts.
pub(crate) fn buffer_check(status: Result<(), crate::buffer::Error>) {
    if status.is_err() {
        crate::fatal("a send queue operation failed");
    }
}

/// `f` over the socket's `out_buffer` queue (None without one, or while it is lent),
/// taken out of its buffer meanwhile and put back after, or freed if the socket lost
/// its buffer meanwhile. `f` may use the socket, but not reach its queue through it.
pub fn with_queue<R>(
    socket: Socket<'_>,
    f: impl FnOnce(Option<&mut crate::buffer::Buffer>) -> R,
) -> R {
    use crate::buffer::cpp::{OutputQueue, dispose_queue};
    let queue = socket.read(|record| record.output.buffer().and_then(OutputQueue::take_queue));
    let Some(mut queue) = queue else {
        return f(None);
    };
    let result = f(Some(&mut queue));
    if let Some(queue) = socket.read(|record| match record.output.buffer() {
        Some(buffer) => {
            buffer.put_queue(queue);
            None
        }
        None => Some(queue),
    }) {
        dispose_queue(queue);
    }
    result
}

/// The queued output's effects on a socket.
struct Output<'a>(Socket<'a>);

impl OutputEffects for Output<'_> {
    fn send(&self, data: &[u8]) -> Option<usize> {
        send(self.0, data, 0)
    }

    fn cork(&self) -> bool {
        self.0.edit(socket_cork)
    }

    fn uncork(&self) {
        self.0.edit(socket_uncork);
    }

    fn sent(&self) {
        self.0.edit(|record| {
            if record.output.send_timer.is_some() {
                record.last_sent_time = steady_ms();
            }
        });
    }

    fn catch_error(&self, error: c_int) -> crate::reactor::Caught {
        self.0.catch_error(error)
    }
}

/// `int Socket::handle_send()`: sends the queue's front, aborting without one;
/// false when the send failed.
#[must_use]
pub fn socket_handle_send(record: Socket<'_>) -> bool {
    let hooks = Output(record);
    let output = with_queue(record, |queue| {
        handle_send_owned(
            record,
            queue.unwrap_or_else(|| crate::fatal("handle_send on a socket without a send queue")),
            &hooks,
        )
    });
    buffer_check(output.status);
    if output.result.warn {
        let fd = record.fd();
        log::sys_warning(
            c"Socket::handle_send",
            format_args!("send to fd[{fd}] failed"),
        );
    }
    output.detached.dispose();
    !output.result.failed
}

/// `--fix-halfclose` (M15, the `halfclose` feature): `ssl_shutdown` writes the
/// `close_notify` alert, and `handle_sendfile` retries a sendfile interrupted before any
/// progress instead of dropping the file.
const HALFCLOSE_REPAIRS: bool = cfg!(feature = "halfclose");

/// `int Socket::handle_sendfile()`: sends a chunk of the queue's front file, aborting
/// when the front is not one; false when the send failed.
#[must_use]
pub fn socket_handle_sendfile(socket: Socket<'_>) -> bool {
    let hooks = Output(socket);
    // The effects send the request's file and never reach the queue itself.
    let (failed, detached) = with_queue(socket, |queue| {
        let Some(queue) = queue else {
            crate::fatal("sendfile on a socket with nothing queued");
        };
        let Some(request) = queue.front_file::<SocketFileRequest>() else {
            if queue.front().is_err() {
                crate::fatal("sendfile on a socket with nothing queued");
            }
            crate::fatal("sendfile's queued entry is not a file");
        };
        request.cork(&hooks);
        let (rv, requested) = send_chunk(socket, request);
        if rv.is_none() && errno() == libc::EINTR && HALFCLOSE_REPAIRS {
            return (false, None);
        }
        let result = request.sent(&hooks, rv, requested);
        socket.edit(|record| result.publish(&mut record.output));
        if result.warn {
            let at = request.fields().begin;
            let name = String::from_utf8_lossy(request.path());
            log::sys_warning(
                c"Socket::handle_sendfile",
                format_args!("sendfile({name}, {at}, {}) failed", result.requested),
            );
        }
        let detached = if result.pop {
            match queue.pop() {
                Ok(detached) => Some(detached),
                Err(crate::buffer::Error::Empty) => None,
                Err(_) => crate::fatal("popping the send queue failed"),
            }
        } else {
            None
        };
        (result.failed, detached)
    });
    if let Some(detached) = detached {
        detached.dispose();
    }
    !failed
}

/// `int Socket::sendfile_async(const char *filename, off_t offset, size_t length)`:
/// the checked request queued behind the output, which is created on first use.
#[must_use]
pub fn socket_sendfile_async(
    socket: Socket<'_>,
    filename: &CStr,
    offset: libc::off_t,
    length: usize,
) -> bool {
    let Some(request) = file_request(filename, offset, length) else {
        return false;
    };
    if socket.read(|record| record.output.out_buffer.is_none()) {
        let buffer = crate::buffer::cpp::new_buffer(65536);
        socket.edit(|record| record.output.out_buffer = Some(buffer));
    }
    // A refused request is dropped with its file.
    let status = with_queue(socket, |queue| {
        queue.map_or(Err(crate::buffer::Error::Invalid), |queue| {
            queue.push_file(request)
        })
    });
    buffer_check(status);
    true
}

/// `ssize_t Socket::send_async(const void *data, size_t length)`: through the event
/// loop when there is one, else `send_sync`: the bytes taken, `None` when it failed.
/// The loop may register the socket to send what it queues, so it is a heap socket.
#[must_use]
pub fn send_async(socket: crate::reactor::Heap<'_>, data: &[u8]) -> Option<usize> {
    if crate::reactor::socket_upstream::event_reactor().is_none() {
        return send_sync(*socket, data, 0);
    }
    crate::reactor::event_api::event_write(socket, data)
}

/// `bool Socket::ssl_shutdown()`: false without an SSL.
#[expect(
    clippy::must_use_candidate,
    reason = "ssl_close ignores the result, as upstream does"
)]
pub fn socket_ssl_shutdown(socket: Socket<'_>) -> bool {
    let repaired = HALFCLOSE_REPAIRS;
    let Some((result, closed)) = with_tls(
        socket,
        |record| (record.ssl_quiet_shutdown, record.ssl_closed),
        |tls, ssl, (quiet, mut closed)| {
            let result = ssl_shutdown(tls, ssl, &mut closed, quiet, repaired);
            (result, closed)
        },
    ) else {
        return false;
    };
    socket.edit(|record| record.ssl_closed = closed);
    if let Err(error) = result {
        let (reason, error) = error.reason();
        let error = match error {
            Some(error) => error.to_string_lossy().into_owned(),
            None if repaired => "no OpenSSL reason available".into(),
            None => "(null)".into(),
        };
        log::warning(
            c"Socket::ssl_shutdown",
            format_args!("SSL_shutdown() failed, reason={reason}, error_string={error}"),
        );
    }
    matches!(result, Ok(true))
}

/// `SSL_set_tlsext_host_name` on the record's SSL: false without one, or when OpenSSL
/// refuses the name.
#[expect(
    clippy::must_use_candidate,
    reason = "a client's handshake ignores the result, as upstream does"
)]
pub fn socket_ssl_set_host_name(socket: Socket<'_>, name: &CStr) -> bool {
    Ssl::of(socket).is_some_and(|held| held.get().set_host_name(name))
}

/// `void Socket::ssl_close()`: shuts down unless already closed (OpenSSL 1.0.2f
/// complains about `SSL_shutdown` during a handshake), then frees the SSL.
/// Pending operations retain their own SSL references until their handles expire.
pub fn socket_ssl_close(socket: Socket<'_>) {
    if !socket.read(|record| record.ssl_closed) {
        socket_ssl_shutdown(socket);
    }
    socket.edit(SocketFields::close_ssl);
}

/// `int Socket::ssl_create(SSLContext *ssl_context, int flags)`: whether it made the
/// session, without setting the last error.
#[must_use]
pub fn socket_ssl_create(
    socket: Socket<'_>,
    context: Option<&crate::tls::SslCtx>,
    flags: c_int,
) -> bool {
    crate::tls::socket_ssl_new(socket, context, flags)
        .inspect_err(|error| error.warn())
        .is_ok()
}

pub use crate::tls::event_socket::socket_free;

/// `Socket::bind(host, port)`: the address resolved without DNS, the last error set
/// when it is invalid.
fn bind_host(socket: Socket<'_>, host: &CStr, port: c_int) -> bool {
    let kind = socket.read(|record| record.socket_type);
    let mut address = SocketAddress::default();
    let text = host.to_bytes();
    let (storage, length) = (&mut address.addr, &mut address.len);
    let assigned = assign(storage, length, kind, text, port, |family| {
        let mut bytes = [0; 16];
        let parsed = platform_parses(family, text) && crate::pton(family, host, &mut bytes);
        if parsed {
            Ok(bytes)
        } else {
            Err(BAD_HOST_ADDR)
        }
    });
    if let Err(error) = assigned {
        log::set_last_error(error);
        return false;
    }
    socket.edit(|record| record.bind(&address))
}

/// Creates a bound socket that listens for stream connections. Logs a warning on failure.
#[must_use]
pub fn server_socket(
    kind: c_int,
    address: &CStr,
    port: c_int,
    backlog: c_int,
) -> Option<crate::reactor::OwnedSocket> {
    const ORIGIN: &CStr = c"make_server_socket";
    // The new record, which only this function holds.
    let Some(record) = make_typed(kind, FD_STREAM_SERVER, SocketFlag::SW_SOCK_CLOEXEC.bits())
    else {
        log::sys_warning(ORIGIN, format_args!("socket() failed"));
        return None;
    };
    let owned = crate::reactor::OwnedSocket::new(record);
    let record = owned.socket();
    let fd = record.fd();
    // Only TCP sockets need to rebind addresses held by TIME_WAIT. Enabling reuse for
    // datagram sockets can allow duplicate binds.
    let kind = crate::constants::SocketType::from_raw(kind);
    if kind.is_some_and(crate::constants::SocketType::is_tcp) {
        let enable: c_int = 1;
        let set = crate::sys::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &enable);
        if set.is_err() {
            log::sys_warning(
                ORIGIN,
                format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
            );
        }
    }
    let host = address.to_string_lossy();
    // Bound, listening when a stream, and named; else the warning for what failed.
    let served = if !bind_host(record, address, port) {
        log::sys_warning(
            ORIGIN,
            format_args!("bind({fd}, {host}:{port}, {backlog}) failed"),
        );
        false
    } else if kind.is_some_and(crate::constants::SocketType::is_stream)
        && !record.edit(|record| record.listen(backlog))
    {
        log::sys_warning(
            ORIGIN,
            format_args!("listen({fd}, {host}:{port}, {backlog}) failed"),
        );
        false
    } else if !record.edit(SocketFields::get_name) {
        log::sys_warning(ORIGIN, format_args!("getsockname({fd}) failed"));
        false
    } else {
        true
    };
    if served {
        return Some(owned);
    }
    socket_free(Some(owned));
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::SocketRecord;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_ntop")]
    fn peer_prints_the_address_as_upstream() {
        let mut socket = SocketRecord::new();
        socket.info.kind = 1;
        socket.info.addr.inet_v4.sin_addr.s_addr = u32::from_be_bytes([127, 0, 0, 1]).to_be();
        socket.info.addr.inet_v4.sin_port = 9501u16.to_be();
        assert_eq!(peer(&socket.info), ("127.0.0.1".into(), 9501));
        socket.info.kind = 3;
        socket.info.addr.inet_v6.sin6_addr.s6_addr =
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        assert_eq!(peer(&socket.info).0, "::1");
        socket.info.kind = 0;
        assert_eq!(peer(&socket.info), ("unknown".into(), 0));
    }

    #[test]
    fn cert_chain_without_ssl_is_null() {
        let mut socket = SocketRecord::new();
        let handle = unsafe { Socket::from_ptr(&raw mut socket) }.unwrap();
        assert!(
            Ssl::of(handle)
                .map_or(std::ptr::null_mut(), |held| held.get().peer_cert_chain())
                .is_null()
        );
    }

    #[test]
    fn peer_certificate_getters_without_ssl() {
        let mut socket = SocketRecord::new();
        let handle = unsafe { Socket::from_ptr(&raw mut socket) }.unwrap();
        assert!(ssl_get_peer_certificate(handle).is_none());
        assert!(!socket_ssl_check_host(handle, c"localhost"));
        let socket = &raw const socket;
        let n = socket_ssl_peer_certificate_pem(
            unsafe { Socket::from_ptr(socket.cast_mut()) }.unwrap(),
            &mut [0_u8; 64],
        );
        assert_eq!(n, None);
        let mut string = StringBuffer::EMPTY;
        string.alloc(64, None);
        string.append(b"content");
        let filled = socket_ssl_peer_certificate_string(
            unsafe { Socket::from_ptr(socket.cast_mut()) }.unwrap(),
            &mut string,
        );
        assert!(!filled);
        assert_eq!(string.len(), 7, "untouched on failure");
        string.dispose();
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls OpenSSL, which Miri cannot execute")]
    fn ssl_create_then_close_frees_the_ssl() {
        unsafe {
            let context = crate::tls::SslContext::tls();
            let mut fds = [0; 2];
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()),
                0
            );
            let mut socket = SocketRecord::with_fd(fds[0], 0);
            socket.tls.state = crate::tls::SslState::SW_SSL_STATE_WAIT_STREAM;
            socket.tls.want_read = true;
            let created = socket_ssl_create(
                Socket::from_ptr(&raw mut socket).unwrap(),
                Some(&context),
                2,
            );
            assert!(created);
            assert!(!socket.ssl().is_null());
            assert_eq!(
                (socket.tls.state as u32, socket.tls.want_read),
                (0, false),
                "state and want flags cleared"
            );
            let index = crate::tls::swr_v1_tls_connection_index();
            let data = openssl_sys::SSL_get_ex_data(socket.ssl().cast(), index);
            assert_eq!(data, (&raw mut socket).cast());
            socket_ssl_close(Socket::from_ptr(&raw mut socket).unwrap());
            assert!(socket.ssl().is_null());
            drop(context);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls OpenSSL, which Miri cannot execute")]
    fn session_of_an_ssl_before_its_handshake() {
        let (mut socket, peer) = pair();
        let handle = unsafe { Socket::from_ptr(&raw mut socket) }.unwrap();
        assert_eq!(socket_ssl_session(handle), None);
        unsafe {
            let context = crate::tls::SslContext::tls();
            assert!(socket_ssl_create(
                Socket::from_ptr(&raw mut socket).unwrap(),
                Some(&context),
                2
            ));
            let session = socket_ssl_session(handle).unwrap();
            assert_eq!(
                session.version,
                openssl_sys::SSL_version(socket.ssl().cast())
            );
            assert_eq!(
                (
                    session.cipher_name,
                    session.cipher_bits,
                    session.cipher_version
                ),
                (c"(NONE)", 0, c"(NONE)")
            );
            socket_ssl_close(handle);
            assert_eq!(socket_ssl_session(handle), None);
            drop(context);
            libc::close(socket.fd);
            libc::close(peer.fd);
        }
    }

    /// A connected pair of Unix stream records.
    fn pair() -> (SocketRecord, SocketRecord) {
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        (
            SocketRecord::with_fd(fds[0], 0),
            SocketRecord::with_fd(fds[1], 0),
        )
    }

    fn temp_file(name: &str, contents: &[u8]) -> (std::path::PathBuf, std::ffi::CString) {
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        (path, name)
    }

    fn read_all(socket: &SocketFields, length: usize) -> Vec<u8> {
        let mut data = vec![0u8; length];
        let mut done = 0;
        while done < length {
            let n =
                unsafe { libc::read(socket.fd, data.as_mut_ptr().add(done).cast(), length - done) };
            assert!(n > 0);
            done += n.cast_unsigned();
        }
        data
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sendfile")]
    fn sendfile_sync_sends_the_range_and_rejects_a_bad_one() {
        let (mut left, right) = pair();
        let (path, name) = temp_file("swoole-sendfile-sync", b"0123456789");
        let socket = Socket::new(&mut left);
        assert!(sendfile_sync(socket, &name, 2, 5));
        assert_eq!(read_all(&right, 5), b"23456");
        assert!(!sendfile_sync(socket, &name, 20, 0), "offset past the end");
        assert!(!sendfile_sync(socket, c"/nonexistent/x", 0, 0));
        std::fs::remove_file(path).unwrap();
        unsafe { libc::close(left.fd) };
        unsafe { libc::close(right.fd) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sendfile")]
    fn queued_file_and_data_drain_through_the_handlers() {
        let (mut left, right) = pair();
        left.output.out_buffer = Some(crate::buffer::cpp::new_buffer(65536));
        let (path, name) = temp_file("swoole-sendfile-async", b"abcdef");
        let handle = unsafe { Socket::from_ptr(&raw mut left) }.unwrap();
        let request = file_request(&name, 1, 0).unwrap();
        let queued = with_queue(handle, |queue| queue.unwrap().push_file(request));
        assert_eq!(queued, Ok(()));
        assert!(
            file_request(c"/nonexistent/x", 0, 0).is_none(),
            "a file that won't open is refused"
        );
        assert!(socket_handle_sendfile(handle));
        assert_eq!(read_all(&right, 5), b"bcdef");
        assert_eq!(
            with_queue(handle, |queue| queue.unwrap().append(b"tail")),
            Ok(())
        );
        assert!(socket_handle_send(handle));
        assert_eq!(read_all(&right, 4), b"tail");
        assert_eq!(
            with_queue(handle, |queue| queue.unwrap().stats()),
            (0, 0),
            "both chunks popped"
        );
        // The queue is released with its `Buffer`, which ends the test if it can't be.
        drop(left.output.out_buffer.take());
        std::fs::remove_file(path).unwrap();
        unsafe { libc::close(left.fd) };
        unsafe { libc::close(right.fd) };
    }

    #[test]
    fn make_server_socket_binds_and_listens() {
        let socket = server_socket(1, c"127.0.0.1", 0, 8).expect("listening socket");
        socket.socket().read(|record| {
            assert!(record.bound && record.listened);
            assert_ne!(
                u16::from_be(unsafe { record.info.addr.inet_v4.sin_port }),
                0,
                "getsockname filled the port"
            );
        });
        socket_free(Some(socket));
        // The system-warning path reports errno once the shared logger is initialized.
        crate::set_errno(libc::EINVAL);
        let invalid = server_socket(1, c"not an address", 0, 8);
        assert!(invalid.is_none());
        assert_ne!(log::last_error(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no MSG_PEEK")]
    fn blocking_calls_and_liveness_over_a_socket_pair() {
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        let mut left = SocketRecord::with_fd(fds[0], 0);
        let mut right = SocketRecord::with_fd(fds[1], 0);
        let sent = send_sync(
            unsafe { Socket::from_ptr(&raw mut left) }.unwrap(),
            b"ping",
            0,
        );
        assert_eq!(sent, Some(4));
        assert!(check_liveness(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap()
        ));
        let mut buffer = [0u8; 4];
        let received = recv_sync(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap(),
            &mut buffer,
            libc::MSG_WAITALL,
        );
        assert_eq!((received, &buffer), (Some(4), b"ping"));
        assert_eq!(right.total_recv_bytes, 4);
        assert!(right.output.nonblock);
        unsafe { libc::close(fds[0]) };
        assert!(!check_liveness(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap()
        ));
        unsafe { libc::close(fds[1]) };
    }
}
