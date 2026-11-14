// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The TLS methods of `network::Socket` that are whole Rust operations, last
//! error, errno and logs included. With `upstream-symbols` (the hybrid's reactor
//! build) they are defined under their upstream (Itanium) symbols and
//! `socket.cc` no longer defines them.
use crate::handle::CText;
use crate::{
    Accept, Cert, CertRef, HandshakeLog, SocketTls, Ssl, SslIoResult, swr_v1_socket_ssl_accept,
    swr_v1_socket_ssl_clear, swr_v1_socket_ssl_connect, swr_v1_socket_ssl_error_reason,
    swr_v1_socket_ssl_peek_reason, swr_v1_socket_ssl_recv, swr_v1_socket_ssl_send,
    swr_v1_socket_ssl_shutdown, swr_v1_socket_ssl_verify,
};
use openssl_sys::{X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK};
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::io::IoSlice;
use std::mem::MaybeUninit;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::constants::{EventType, SocketFlag};
use swoole_rs_corebase::net_address::{BAD_HOST_ADDR, assign, platform_parses};
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_corebase::{log, set_errno};
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::socket_upstream::{
    make_typed, socket_cork, socket_set_fd_option, socket_uncork,
};
use swoole_rs_reactor::{
    CLASS_STREAM, CLASS_TCP, Drain, Fill, OutputEffects, RangeError, SYNC_ONCE, SYNC_RECV,
    SYNC_SEND, SYNC_SENDFILE, Socket, SocketAddress, SocketFields, SocketFileRequest, SocketRecord,
    SslSendfileWarning, SyncEffects, handle_send_owned, record_peek, record_recv, record_send,
    sendfile_chunk, ssl_sendfile as ssl_sendfile_fd, steady_ms, swr_v1_reactor_poll_events_to,
    swr_v1_socket_sync, type_is, what_event_want,
};

// libcrypto's own declaration: it reads only the code, and answers one of its
// static strings.
unsafe extern "C" {
    safe fn X509_verify_cert_error_string(error: c_long) -> Option<CText<'static>>;
}

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING`, `SW_ERROR_FILE_EMPTY` and `SW_ERROR_INVALID_PARAMS`.
const LOG_NOTICE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_NOTICE as c_int;
const LOG_WARNING: c_int = 4;
const FILE_EMPTY: c_int = 702;
const INVALID_PARAMS: c_int = 505;
/// `SW_SOCKET_CORK_MIN_SIZE` and the typed queue's `SWR_BUFFER_EMPTY`.
const CORK_MIN_SIZE: usize = 65536;
/// `SW_FD_STREAM_SERVER`.
const FD_STREAM_SERVER: c_int = 1;
const SOCK_CLOEXEC: c_int = SocketFlag::SW_SOCK_CLOEXEC as c_int;
const BUFFER_EMPTY: i32 = 1;
const VERIFY_FAILED: c_int = ErrorCode::SW_ERROR_SSL_VERIFY_FAILED as c_int;
const BAD_PROTOCOL: c_int = ErrorCode::SW_ERROR_SSL_BAD_PROTOCOL as c_int;
const EVENT_READ: c_int = EventType::SW_EVENT_READ as c_int;
const EVENT_WRITE: c_int = EventType::SW_EVENT_WRITE as c_int;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

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
    let host = swoole_rs_corebase::ntop(family, &binary).map_or_else(
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
    let (before, picked) = socket.read(|record| (record.tls, pick(record)));
    let ssl = Ssl::of(socket)?;
    let mut tls = before;
    let result = step(&mut tls, ssl, picked);
    // Most transfers leave the group as it was, and then the record needs no edit.
    if tls != before {
        socket.edit(|record| record.tls = tls);
    }
    Some(result)
}

/// `static const char *Socket::ssl_get_error_reason(int *reason)`: `reason` is null
/// or writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket20ssl_get_error_reasonEPi")
)]
pub extern "C" fn socket_ssl_get_error_reason(reason: Option<&mut c_int>) -> *const c_char {
    let (code, text) = swr_v1_socket_ssl_error_reason();
    if let Some(reason) = reason {
        *reason = code;
    }
    text.map_or(std::ptr::null(), CStr::as_ptr)
}

/// `STACK_OF(X509) *Socket::ssl_get_peer_cert_chain() const`, for upstream's core tests.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket23ssl_get_peer_cert_chainEv")
)]
pub extern "C" fn socket_ssl_get_peer_cert_chain(
    socket: Socket<'_>,
) -> *mut openssl_sys::stack_st_X509 {
    Ssl::of(socket).map_or(std::ptr::null_mut(), Ssl::peer_cert_chain)
}

/// The peer's certificate chain, each certificate as PEM (at most
/// `SW_STACK_BUFFER_SIZE` bytes), skipping one that fails.
#[must_use]
pub fn socket_ssl_peer_chain_pem(socket: Socket<'_>) -> Vec<Vec<u8>> {
    let mut chain = Vec::new();
    let Some(ssl) = Ssl::of(socket) else {
        return chain;
    };
    let mut pem = vec![0_u8; swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize];
    ssl.each_peer_cert(|cert| {
        let written = read_x509(cert, &mut pem);
        if let Some(bytes) = usize::try_from(written).ok().filter(|&n| n > 0) {
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
    let ssl = Ssl::of(socket)?;
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
    let Some(ssl) = Ssl::of(socket) else {
        return false;
    };
    let error = swr_v1_socket_ssl_verify(ssl, allow_self_signed);
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

/// [`socket_ssl_verify`] for upstream's core tests.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZNK6swoole7network6Socket10ssl_verifyEb")]
extern "C" fn native_ssl_verify(socket: Socket<'_>, allow_self_signed: bool) -> bool {
    socket_ssl_verify(socket, allow_self_signed)
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

/// `ssl_io`: the protocol error's log, then errno; -1 without an SSL.
#[inline(always)]
fn ssl_io(socket: Socket<'_>, result: Option<SslIoResult>) -> isize {
    let Some(result) = result else {
        return -1;
    };
    if result.protocol_error {
        ssl_catch_error(socket);
    }
    if result.error != 0 {
        set_errno(result.error);
    }
    result.value
}

/// `Socket::ssl_recv`: the record's TLS read into the bytes `buffer` lends; -1 without
/// an SSL.
#[must_use]
pub fn ssl_recv_fill<S: ReadSlot>(socket: Socket<'_>, mut buffer: Fill<'_, S>) -> isize {
    let buffer = buffer.slots();
    let result = with_tls(
        socket,
        |_| (),
        |tls, ssl, ()| swr_v1_socket_ssl_recv(tls, ssl, buffer),
    );
    ssl_io(socket, result)
}

/// `Socket::ssl_send`: the record's TLS write of the bytes `data` lends, of which a
/// DTLS socket sends at most `chunk_size`; -1 without an SSL.
#[must_use]
pub fn ssl_send_drain(socket: Socket<'_>, data: Drain<'_>) -> isize {
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
pub fn ssl_recv(socket: Socket<'_>, buffer: &mut [impl ReadSlot]) -> isize {
    ssl_recv_fill(socket, Fill::new(buffer))
}

/// `ssize_t Socket::ssl_send(const void *buf, size_t n)` of `data`.
#[must_use]
pub fn ssl_send(socket: Socket<'_>, data: &[u8]) -> isize {
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
        let message = swoole_rs_corebase::sys::strerror(code);
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
        let error = crate::pop_error();
        let message = crate::error_string(error);
        let ((host, port), fd) = socket.read(|record| (peer(&record.info), record.fd));
        log::notice(format_args!(
            "ssl_connect(fd={fd}) to server[{host}:{port}] failed. Error: {message}[{ssl_error}|{}]",
            crate::socket::error_reason(error)
        ));
    }
    result.code
}

/// `SSL_peek` on the record into `buffer`; -1 without an SSL.
fn tls_peek<S: ReadSlot>(socket: Socket<'_>, mut buffer: Fill<'_, S>) -> isize {
    let Some(ssl) = Ssl::of(socket) else {
        return -1;
    };
    ssl.peek(buffer.slots()) as isize
}

/// `ssize_t Socket::recv(void *, size_t, int)` into the bytes `buffer` lends: a slice's
/// ([`recv`]), or the address and length C handed its holder.
pub fn recv_fill(socket: Socket<'_>, buffer: Fill<'_>, flags: c_int) -> isize {
    record_recv(socket, buffer, flags, ssl_recv_fill, steady_ms)
}

/// `ssize_t Socket::send(const void *, size_t, int)` of the bytes `data` lends, as
/// [`recv_fill`] receives.
#[must_use]
pub fn send_drain(socket: Socket<'_>, data: Drain<'_>, flags: c_int) -> isize {
    record_send(socket, data, flags, ssl_send_drain, steady_ms)
}

/// [`recv_fill`] into `buffer`.
pub fn recv(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    record_recv(socket, Fill::new(buffer), flags, ssl_recv_fill, steady_ms)
}

/// [`send_drain`] of `data`.
#[must_use]
pub fn send(socket: Socket<'_>, data: &[u8], flags: c_int) -> isize {
    record_send(socket, Drain::new(data), flags, ssl_send_drain, steady_ms)
}

/// [`recv_sync_fill`] into `buffer`.
pub fn recv_sync(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    sync(socket, Io::Recv(Fill::new(buffer)), SYNC_RECV, flags, false)
}

/// [`send_sync_drain`] of `data`.
#[must_use]
pub fn send_sync(socket: Socket<'_>, data: &[u8], flags: c_int) -> isize {
    sync_out(socket, Io::Send(Drain::new(data)), SYNC_SEND, flags)
}

/// [`read_sync_fill`] into `buffer`.
pub fn read_sync(socket: Socket<'_>, buffer: &mut [impl ReadSlot]) -> isize {
    sync(socket, Io::Read(Fill::new(buffer)), SYNC_ONCE, 0, false)
}

/// `ssize_t Socket::write_sync(const void *, size_t)` of `data`.
#[must_use]
pub fn write_sync(socket: Socket<'_>, data: &[u8]) -> isize {
    sync_out(socket, Io::Write(Drain::new(data)), SYNC_ONCE, 0)
}

/// `ssize_t Socket::sendto_sync(const Address &, const void *, size_t, int)` of `data`.
#[must_use]
pub fn sendto_sync(
    socket: Socket<'_>,
    address: &SocketAddress,
    data: &[u8],
    flags: c_int,
) -> isize {
    let io = Io::SendTo(address, Drain::new(data));
    sync_out(socket, io, SYNC_ONCE, flags)
}

/// `ssize_t Socket::recvfrom_sync(char *, size_t, int, Address *)` into `buffer`, and
/// the peer into `address`'s storage and length.
pub fn recvfrom_sync(
    socket: Socket<'_>,
    buffer: &mut [impl ReadSlot],
    flags: c_int,
    address: &mut SocketAddress,
) -> isize {
    let io = Io::RecvFrom(address, Fill::new(buffer));
    sync(socket, io, SYNC_ONCE, flags, false)
}

/// `ssize_t Socket::peek(void *, size_t, int) const` into `buffer`.
pub fn peek(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    record_peek(socket, Fill::new(buffer), flags, tls_peek)
}

/// `bool Socket::check_liveness()`.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZN6swoole7network6Socket14check_livenessEv")]
extern "C" fn socket_check_liveness(socket: Socket<'_>) -> bool {
    check_liveness(socket)
}

/// `bool Socket::check_liveness()`: a nonblocking one-byte peek that neither
/// ends the stream nor fails with an error that closes it.
#[must_use]
pub fn check_liveness(socket: Socket<'_>) -> bool {
    let mut byte = [0u8];
    set_errno(0);
    let peeked = peek(socket, &mut byte, libc::MSG_DONTWAIT);
    !(peeked == 0
        || (peeked < 0 && socket.catch_error(errno()) == swoole_rs_reactor::Caught::Close))
}

/// One blocking call: the socket, and its I/O with the bytes it has left.
struct Call<'a, S: ReadSlot> {
    socket: Socket<'a>,
    io: Io<'a, S>,
    dont_restart: bool,
}

/// The I/O a blocking call repeats, over the bytes and operands it was lent.
enum Io<'a, S: ReadSlot = MaybeUninit<u8>> {
    Send(Drain<'a>),
    Recv(Fill<'a, S>),
    Read(Fill<'a, S>),
    Write(Drain<'a>),
    SendTo(&'a SocketAddress, Drain<'a>),
    RecvFrom(&'a mut SocketAddress, Fill<'a, S>),
    Writev(&'a [IoSlice<'a>]),
    /// The request, and how many of its bytes are still to send.
    Sendfile(&'a mut SocketFileRequest, usize),
}

/// writev(2) of `vectors`.
#[must_use]
pub fn writev(fd: c_int, vectors: &[IoSlice<'_>]) -> isize {
    let count = vectors.len().c_cast::<c_int>();
    // SAFETY: an `IoSlice` is ABI-compatible with an iovec, of readable bytes.
    unsafe { libc::writev(fd, vectors.as_ptr().cast(), count) }
}

impl<S: ReadSlot> SyncEffects for Call<'_, S> {
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

    fn events(&self, event: c_int) -> i16 {
        swr_v1_reactor_poll_events_to(self.socket.read(|record| what_event_want(record, event)))
    }

    fn io(&mut self, flags: c_int) -> isize {
        let socket = self.socket;
        // Read on each call: a signal handler the wait dispatches may have changed it.
        let fd = socket.fd();
        match &mut self.io {
            Io::Send(data) => record_send(socket, *data, flags, ssl_send_drain, steady_ms),
            Io::Recv(buffer) => {
                record_recv(socket, buffer.reborrow(), flags, ssl_recv_fill, steady_ms)
            }
            Io::Read(buffer) => buffer.read(fd),
            Io::Write(data) => data.write(fd),
            Io::SendTo(address, data) => data.sendto(fd, flags, address),
            Io::RecvFrom(address, buffer) => buffer.recvfrom(fd, flags, address),
            Io::Sendfile(request, _) => send_chunk(socket, request).0,
            Io::Writev(vectors) => writev(fd, vectors),
        }
    }

    fn advance(&mut self, moved: usize) -> bool {
        match &mut self.io {
            Io::Send(data) | Io::Write(data) | Io::SendTo(_, data) => data.advance(moved),
            Io::Recv(buffer) | Io::Read(buffer) | Io::RecvFrom(_, buffer) => buffer.advance(moved),
            Io::Sendfile(_, left) => {
                *left = left.saturating_sub(moved);
                *left > 0
            }
            Io::Writev(_) => false,
        }
    }

    fn catch_error(&self, error: c_int) -> swoole_rs_reactor::Caught {
        self.socket.catch_error(error)
    }

    fn take_nobufs(&self) -> bool {
        self.socket.take_kernel_nobufs()
    }

    fn set_last_error(&self, error: c_int) {
        log::swr_v1_core_set_last_error(error);
    }
}

/// `Socket::wait_for`'s loop (`crates/reactor/src/socket_sync.rs`) around `io`,
/// waiting for writability (else readability) up to the socket's timeout.
fn sync<S: ReadSlot>(
    socket: Socket<'_>,
    io: Io<'_, S>,
    kind: c_int,
    flags: c_int,
    write: bool,
) -> isize {
    let (timeout, dont_restart) = socket.read(|record| {
        let timeouts = record.timeouts;
        let timeout = if write { timeouts.write } else { timeouts.read };
        (timeout, record.dont_restart)
    });
    let mut call = Call {
        socket,
        io,
        dont_restart,
    };
    let event = if write { EVENT_WRITE } else { EVENT_READ };
    let timeout_ms = (timeout * 1000.0).c_cast::<c_long>().c_cast::<c_int>();
    swr_v1_socket_sync(&mut call, kind, flags, event, timeout_ms)
}

/// [`sync`] of an I/O that fills nothing, waiting for writability.
fn sync_out(socket: Socket<'_>, io: Io<'_>, kind: c_int, flags: c_int) -> isize {
    sync(socket, io, kind, flags, true)
}

/// `ssize_t Socket::send_sync(const void *, size_t, int)` of the bytes `data` lends.
#[must_use]
pub fn send_sync_drain(socket: Socket<'_>, data: Drain<'_>, flags: c_int) -> isize {
    sync_out(socket, Io::Send(data), SYNC_SEND, flags)
}

/// `ssize_t Socket::recv_sync(void *, size_t, int)` into the bytes `buffer` lends:
/// `MSG_WAITALL` is cleared for each call, since under nonblocking I/O it would spin on
/// EAGAIN.
#[must_use]
pub fn recv_sync_fill(socket: Socket<'_>, buffer: Fill<'_>, flags: c_int) -> isize {
    sync(socket, Io::Recv(buffer), SYNC_RECV, flags, false)
}

/// `ssize_t Socket::read_sync(void *, size_t)` into the bytes `buffer` lends.
#[must_use]
pub fn read_sync_fill(socket: Socket<'_>, buffer: Fill<'_>) -> isize {
    sync(socket, Io::Read(buffer), SYNC_ONCE, 0, false)
}

/// `ssize_t Socket::writev_sync(const iovec *, size_t)` of `vectors`.
#[must_use]
pub fn writev_sync(socket: Socket<'_>, vectors: &[IoSlice<'_>]) -> isize {
    sync_out(socket, Io::Writev(vectors), SYNC_ONCE, 0)
}

/// `X509 *Socket::ssl_get_peer_certificate() const`.
#[must_use]
pub fn ssl_get_peer_certificate(socket: Socket<'_>) -> Option<Cert> {
    Ssl::of(socket)?.peer_certificate()
}

/// `bool Socket::ssl_check_host(const char *tls_host_name) const`.
#[must_use]
pub fn socket_ssl_check_host(socket: Socket<'_>, name: &CStr) -> bool {
    let Some(cert) = ssl_get_peer_certificate(socket) else {
        return false;
    };
    if cert.get().matches_host(name.to_bytes()) {
        return true;
    }
    log::warning(
        c"Socket::ssl_check_host",
        format_args!("X509_check_host(): no match"),
    );
    false
}

/// `int Socket::ssl_get_peer_certificate(char *buf, size_t n) const`: the peer
/// certificate's PEM into `buffer`: its length, or `SW_ERR`.
pub fn socket_ssl_peer_certificate_pem(socket: Socket<'_>, buffer: &mut [impl ReadSlot]) -> c_int {
    ssl_get_peer_certificate(socket).map_or(-1, |cert| read_x509(cert.get(), buffer))
}

/// `_ssl_read_x509_file` without consuming: the certificate as PEM into `buffer`,
/// warning and returning -1 on failure.
fn read_x509(cert: CertRef<'_>, buffer: &mut [impl ReadSlot]) -> c_int {
    warn_x509(cert.pem(buffer))
}

/// `_ssl_read_x509_file`'s warnings for [`CertRef::pem`]'s result.
fn warn_x509(written: c_int) -> c_int {
    match written {
        -1 => log::warning(
            c"network::_ssl_read_x509_file",
            format_args!("BIO_new() failed"),
        ),
        -2 => log::warning(
            c"network::_ssl_read_x509_file",
            format_args!("PEM_write_bio_X509() failed"),
        ),
        _ => return written,
    }
    -1
}

/// `bool Socket::ssl_get_peer_certificate(String *buf) const`: the peer certificate's
/// PEM as `buffer`'s content, written from the start of its allocation; false, with
/// the content's length as it was, without one.
pub fn socket_ssl_peer_certificate_string(socket: Socket<'_>, buffer: &mut CppString) -> bool {
    let (at, size) = (buffer.as_mut_ptr(), buffer.size());
    // SAFETY: the String's allocation of `size` bytes, borrowed with it.
    let mut bytes = unsafe { Fill::from_raw(at.cast(), size) };
    let n = socket_ssl_peer_certificate_pem(socket, bytes.slots());
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    // SAFETY: the PEM is the allocation's first `n` bytes, written just above.
    unsafe { buffer.set_len(n) };
    true
}

/// `Socket::ssl_sendfile` over a descriptor.
fn ssl_sendfile(socket: Socket<'_>, fd: c_int, offset: &mut libc::off_t, size: usize) -> isize {
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
/// SSL, else the platform's sendfile.
pub fn sendfile(socket: Socket<'_>, fd: c_int, offset: &mut libc::off_t, length: usize) -> isize {
    let (out, ssl) = socket.read(|record| (record.fd, record.ssl()));
    if !ssl.is_null() {
        return ssl_sendfile(socket, fd, offset, length);
    }
    // `swoole_sendfile` (src/os/sendfile.cc): the platform's sendfile.
    let sent = swoole_rs_ossupport::swr_v1_os_sendfile(out, fd, Some(offset), length);
    if cfg!(not(target_os = "linux")) && sent.warn != 0 {
        log::sys_warning(c"swoole_sendfile", format_args!("sendfile failed"));
    }
    sent.result
}

/// One chunk of `request`'s remaining range through `sendfile`, advancing its
/// offset: what `sendfile` returned and the chunk's size.
fn send_chunk(socket: Socket<'_>, request: &mut SocketFileRequest) -> (isize, usize) {
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

/// `int Socket::sendfile_sync(const char *filename, off_t offset, size_t length)`:
/// corked for a large file, through `wait_for`'s loop. `filename` is the caller's
/// NUL-terminated string, lent for the call.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZN6swoole7network6Socket13sendfile_syncEPKcxm")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole7network6Socket13sendfile_syncEPKclm")
)]
#[must_use]
pub extern "C" fn socket_sendfile_sync(
    socket: Socket<'_>,
    filename: CText<'_>,
    offset: libc::off_t,
    length: usize,
) -> c_int {
    sendfile_sync(socket, filename.get(), offset, length)
}

/// [`socket_sendfile_sync`] of the file `filename` names.
#[must_use]
pub fn sendfile_sync(
    socket: Socket<'_>,
    filename: &CStr,
    offset: libc::off_t,
    length: usize,
) -> c_int {
    let Some(mut request) = file_request(filename, offset, length) else {
        return -1;
    };
    let total = request.length();
    let corked = total > CORK_MIN_SIZE && socket.edit(|record| socket_cork(record));
    let io = Io::Sendfile(&mut request, total);
    let rv = sync_out(socket, io, SYNC_SENDFILE, 0);
    if corked {
        socket.edit(|record| socket_uncork(record));
    }
    if rv < 0 { -1 } else { 0 }
}

/// A failed Rust queue status. Native `Buffer` raises it as a C++ exception that
/// nothing catches, so the process ends either way; here it aborts.
pub(crate) fn buffer_check(status: i32) {
    if status != 0 {
        swoole_rs_corebase::fatal("a send queue operation failed");
    }
}

/// A socket's `out_buffer` queue, null without one.
pub(crate) fn out_queue(socket: Socket<'_>) -> *mut swoole_rs_buffer::Buffer {
    socket.read(|record| {
        let buffer = record.output.buffer();
        buffer.map_or(
            std::ptr::null_mut(),
            swoole_rs_buffer::cpp::CppBuffer::queue,
        )
    })
}

/// `f` over the socket's queue (None without one).
pub(crate) fn with_queue<R>(
    socket: Socket<'_>,
    f: impl FnOnce(Option<&mut swoole_rs_buffer::Buffer>) -> R,
) -> R {
    // SAFETY: null or the socket's live queue, borrowed for `f` alone.
    f(unsafe { out_queue(socket).as_mut() })
}

/// The queued output's effects on a socket.
struct Output<'a>(Socket<'a>);

impl OutputEffects for Output<'_> {
    fn send(&self, data: &[u8]) -> isize {
        send(self.0, data, 0)
    }

    fn cork(&self) -> bool {
        self.0.edit(|record| socket_cork(record))
    }

    fn uncork(&self) {
        self.0.edit(|record| socket_uncork(record));
    }

    fn sent(&self) {
        self.0.edit(|record| {
            if !record.output.send_timer.is_null() {
                record.last_sent_time = steady_ms();
            }
        });
    }

    fn catch_error(&self, error: c_int) -> swoole_rs_reactor::Caught {
        self.0.catch_error(error)
    }
}

/// `int Socket::handle_send()`: sends the queue's front, aborting without one.
#[must_use]
pub fn socket_handle_send(record: Socket<'_>) -> c_int {
    let hooks = Output(record);
    let output = with_queue(record, |queue| {
        handle_send_owned(
            record,
            queue.unwrap_or_else(|| {
                swoole_rs_corebase::fatal("handle_send on a socket without a send queue")
            }),
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
    output.result.code
}

/// `--fix-halfclose` (M15, the `halfclose` feature): `ssl_shutdown` writes the
/// `close_notify` alert, and `handle_sendfile` retries a sendfile interrupted before any
/// progress instead of dropping the file.
const HALFCLOSE_REPAIRS: bool = cfg!(feature = "halfclose");

/// `int Socket::handle_sendfile()`: sends a chunk of the queue's front file, aborting
/// when the front is not one.
#[must_use]
pub fn socket_handle_sendfile(socket: Socket<'_>) -> c_int {
    let hooks = Output(socket);
    let front = with_queue(socket, |queue| queue.map(|queue| queue.front()));
    let Some(Ok(front)) = front else {
        swoole_rs_corebase::fatal("sendfile on a socket with nothing queued");
    };
    if front.kind != swoole_rs_buffer::EntryKind::Sendfile {
        swoole_rs_corebase::fatal("sendfile's queued entry is not a file");
    }
    // SAFETY: a sendfile token: only `socket_sendfile_async` queues one, as a request
    // the queue keeps until the pop below.
    let request = unsafe { &mut *front.token.cast::<SocketFileRequest>() };
    request.cork(&hooks);
    let (rv, requested) = send_chunk(socket, request);
    if rv < 0 && errno() == libc::EINTR && HALFCLOSE_REPAIRS {
        return 0;
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
    if result.pop {
        match with_queue(socket, |queue| queue.map(swoole_rs_buffer::Buffer::pop)) {
            Some(Ok(detached)) => detached.dispose(),
            Some(Err(BUFFER_EMPTY)) => {}
            _ => swoole_rs_corebase::fatal("popping the send queue failed"),
        }
    }
    result.code
}

/// `int Socket::sendfile_async(const char *filename, off_t offset, size_t length)`:
/// the checked request queued behind the output, which is created on first use.
#[must_use]
pub fn socket_sendfile_async(
    socket: Socket<'_>,
    filename: &CStr,
    offset: libc::off_t,
    length: usize,
) -> c_int {
    let Some(request) = file_request(filename, offset, length) else {
        return -1;
    };
    let request = Box::into_raw(request);
    if socket.read(|record| record.output.out_buffer.is_none()) {
        let buffer = swoole_rs_buffer::cpp::new_buffer(65536);
        socket.edit(|record| record.output.out_buffer = Some(buffer));
    }
    let dispose: Option<unsafe extern "C" fn(*mut c_void)> = Some(SocketFileRequest::dispose);
    let status = with_queue(socket, |queue| {
        // SAFETY: the queue now owns the request, which only `dispose` frees.
        queue.map_or(-1, |queue| unsafe {
            queue.push(
                swoole_rs_buffer::EntryKind::Sendfile,
                request.cast(),
                dispose,
            )
        })
    });
    if status != 0 {
        // SAFETY: the queue refused the request, which is still ours.
        unsafe { SocketFileRequest::dispose(request.cast()) };
        buffer_check(status);
    }
    0
}

/// `ssize_t Socket::send_async(const void *data, size_t length)`: through the event
/// loop when there is one, else `send_sync`.
///
/// # Safety
/// `data` denotes `length` readable bytes.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZN6swoole7network6Socket10send_asyncEPKvm")]
#[must_use]
pub unsafe extern "C-unwind" fn socket_send_async(
    socket: Socket<'_>,
    data: *const c_void,
    length: usize,
) -> isize {
    if swoole_rs_reactor::socket_upstream::event_reactor().is_none() {
        // SAFETY: as the caller promises.
        let data = unsafe { Drain::from_raw(data, length) };
        return send_sync_drain(socket, data, 0);
    }
    // SAFETY: as the caller promises, on the thread's available reactor.
    unsafe { swoole_rs_reactor::event_api::swoole_event_write(socket.as_ptr(), data, length) }
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
            let result = swr_v1_socket_ssl_shutdown(tls, ssl, &mut closed, quiet, repaired);
            (result, closed)
        },
    ) else {
        return false;
    };
    socket.edit(|record| record.ssl_closed = closed);
    if result == 2 {
        let (reason, error) = swr_v1_socket_ssl_error_reason();
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
    result == 1
}

/// `SSL_set_tlsext_host_name` on the record's SSL: false without one, or when OpenSSL
/// refuses the name.
#[expect(
    clippy::must_use_candidate,
    reason = "a client's handshake ignores the result, as upstream does"
)]
pub fn socket_ssl_set_host_name(socket: Socket<'_>, name: &CStr) -> bool {
    Ssl::of(socket).is_some_and(|ssl| ssl.set_host_name(name))
}

/// `void Socket::ssl_close()`: shuts down unless already closed (OpenSSL 1.0.2f
/// complains about `SSL_shutdown` during a handshake), then frees the SSL.
///
/// # Safety
/// No [`Ssl`] or [`crate::Bio`] handle to the record's SSL or its BIOs is used
/// afterwards: they are freed.
pub unsafe fn socket_ssl_close(socket: Socket<'_>) {
    if !socket.read(|record| record.ssl_closed) {
        socket_ssl_shutdown(socket);
    }
    let ssl = socket.edit(swoole_rs_reactor::SocketFields::take_ssl);
    // SAFETY: the record's SSL (or null), now released; the caller promises no handle
    // to it or its BIOs is used afterwards.
    unsafe { openssl_sys::SSL_free(ssl.cast()) };
}

/// `int Socket::ssl_create(SSLContext *ssl_context, int flags)`: `SW_OK` or `SW_ERR`,
/// without setting the last error.
#[must_use]
pub fn socket_ssl_create(
    socket: Socket<'_>,
    context: Option<&crate::SslCtx>,
    flags: c_int,
) -> c_int {
    match crate::socket_ssl_new(socket, context, flags) {
        Ok(()) => 0,
        Err(error) => {
            error.warn();
            -1
        }
    }
}

/// base.cc's `socket_free_defer`: closes the descriptor and frees the record.
///
/// # Safety
/// `socket` is a live record nothing uses afterwards.
unsafe extern "C-unwind" fn socket_free_defer(socket: *mut c_void) {
    let socket = socket.cast::<SocketRecord>();
    // SAFETY: as the caller promises.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        return;
    };
    if !record.edit(SocketFields::release) {
        let fd = record.fd();
        log::sys_warning(c"socket_free_defer", format_args!("close({fd}) failed"));
    }
    // SAFETY: as above.
    unsafe { SocketRecord::free(socket) };
}

/// `void Socket::free()`: deletes the timers and buffers, then, while the thread's
/// event loop is available, removes the socket from it and defers the close to it;
/// otherwise closes at once. Outside the hybrid (and in this crate's tests, which
/// link no C++) there are no timers, buffers or loop.
///
/// # Safety
/// `socket` is a live record nothing uses afterwards.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket4freeEv")
)]
pub unsafe extern "C-unwind" fn socket_free(socket: *mut SocketRecord) {
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    {
        // SAFETY: as the caller promises.
        let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
            return;
        };
        // The buffers leave the record here and go below, after the timers and outside
        // the record's borrow: an entry's disposer runs as its buffer goes.
        let (timers, buffers) = record.edit(|record| {
            (
                [record.recv_timer, record.output.send_timer],
                [record.in_buffer.take(), record.output.out_buffer.take()],
            )
        });
        for timer in timers {
            if !timer.is_null() {
                // SAFETY: the socket's live timer.
                unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
            }
        }
        // `delete in_buffer; delete out_buffer`.
        drop(buffers);
        if swoole_rs_reactor::socket_upstream::event_reactor().is_some() {
            if record.read(|record| record.fd != -1 && !record.removed) {
                // `swoole_event_del(this)` on the thread's reactor, which logs a failure.
                _ = swoole_rs_reactor::event_api::event_del(record);
            }
            // SAFETY: `swoole_event_defer(socket_free_defer, this)`; a reactor freed
            // before it runs closes the socket all the same.
            let deferred = unsafe {
                swoole_rs_reactor::defer(socket_free_defer, socket.cast(), Some(socket_free_defer))
            };
            if deferred {
                return;
            }
        }
    }
    // SAFETY: as the caller promises.
    unsafe { socket_free_defer(socket.cast()) };
}

/// `Socket::bind(host, port)`: the address resolved without DNS, the last error set
/// when it is invalid.
fn bind_host(socket: Socket<'_>, host: &CStr, port: c_int) -> c_int {
    let kind = socket.read(|record| record.socket_type);
    let mut address = SocketAddress::default();
    let text = host.to_bytes();
    let (storage, length) = (&mut address.addr, &mut address.len);
    let error = assign(storage, length, kind, text, port, |family| {
        // the platform's, which reads the NUL-terminated source and writes at
        // most an IPv6 address's 16 bytes (nothing for a family it doesn't know).
        unsafe extern "C" {
            safe fn inet_pton(
                family: c_int,
                source: CText<'_>,
                destination: &mut [u8; 16],
            ) -> c_int;
        }
        let mut bytes = [0; 16];
        let parsed =
            platform_parses(family, text) && inet_pton(family, host.into(), &mut bytes) == 1;
        if parsed {
            Ok(bytes)
        } else {
            Err(BAD_HOST_ADDR)
        }
    });
    if error != 0 {
        log::set_last_error(error);
        return -1;
    }
    socket.edit(|record| record.bind(&address))
}

/// `Socket *make_server_socket(SocketType, const char *address, int port, int backlog)`:
/// a bound (and, for a stream, listening) socket, or null after a warning. `address`
/// is the caller's NUL-terminated string, lent for the call.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole18make_server_socketE12swSocketTypePKcii")
)]
pub extern "C-unwind" fn make_server_socket(
    kind: c_int,
    address: CText<'_>,
    port: c_int,
    backlog: c_int,
) -> *mut SocketRecord {
    const ORIGIN: &CStr = c"make_server_socket";
    // The new record, which only this function holds.
    let Some(record) = make_typed(kind, FD_STREAM_SERVER, SOCK_CLOEXEC) else {
        log::sys_warning(ORIGIN, format_args!("socket() failed"));
        return std::ptr::null_mut();
    };
    let record = record.into_socket();
    let socket = record.as_ptr();
    let fd = record.fd();
    // Only TCP sockets need to rebind addresses held by TIME_WAIT. Enabling reuse for
    // datagram sockets can allow duplicate binds.
    if type_is(kind, CLASS_TCP) {
        let enable: c_int = 1;
        let set =
            swoole_rs_corebase::sys::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &enable);
        if set < 0 {
            log::sys_warning(
                ORIGIN,
                format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
            );
        }
    }
    let address = address.get();
    let host = address.to_string_lossy();
    // Bound, listening when a stream, and named; else the warning for what failed.
    let served = if bind_host(record, address, port) < 0 {
        log::sys_warning(
            ORIGIN,
            format_args!("bind({fd}, {host}:{port}, {backlog}) failed"),
        );
        false
    } else if type_is(kind, CLASS_STREAM) && record.edit(|record| record.listen(backlog)) < 0 {
        log::sys_warning(
            ORIGIN,
            format_args!("listen({fd}, {host}:{port}, {backlog}) failed"),
        );
        false
    } else if record.edit(SocketFields::get_name) < 0 {
        log::sys_warning(ORIGIN, format_args!("getsockname({fd}) failed"));
        false
    } else {
        true
    };
    if served {
        return socket;
    }
    // SAFETY: the new record, which nothing else holds; `Socket::free`.
    unsafe { socket_free(socket) };
    std::ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
        assert!(socket_ssl_get_peer_cert_chain(handle).is_null());
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
        assert_eq!(n, -1);
        let mut string = CppString::EMPTY;
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
    fn ssl_create_then_close_frees_the_ssl() {
        unsafe {
            let owner = crate::swr_v1_tls_owner_new();
            (*owner.fields).context = Some(crate::SslContext::tls());
            let mut fds = [0; 2];
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()),
                0
            );
            let mut socket = SocketRecord::with_fd(fds[0], 0);
            socket.tls.state = 2;
            socket.tls.want_read = true;
            let created = socket_ssl_create(
                Socket::from_ptr(&raw mut socket).unwrap(),
                (*owner.fields).context.as_deref(),
                2,
            );
            assert_eq!(created, 0);
            assert!(!socket.ssl().is_null());
            assert_eq!(
                (socket.tls.state, socket.tls.want_read),
                (0, false),
                "state and want flags cleared"
            );
            let index = crate::swr_v1_tls_connection_index();
            let data = openssl_sys::SSL_get_ex_data(socket.ssl().cast(), index);
            assert_eq!(data, (&raw mut socket).cast());
            socket_ssl_close(Socket::from_ptr(&raw mut socket).unwrap());
            assert!(socket.ssl().is_null());
            crate::swr_v1_tls_owner_free(owner.owner);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn session_of_an_ssl_before_its_handshake() {
        let (mut socket, peer) = pair();
        let handle = unsafe { Socket::from_ptr(&raw mut socket) }.unwrap();
        assert_eq!(socket_ssl_session(handle), None);
        unsafe {
            let owner = crate::swr_v1_tls_owner_new();
            (*owner.fields).context = Some(crate::SslContext::tls());
            assert_eq!(
                socket_ssl_create(
                    Socket::from_ptr(&raw mut socket).unwrap(),
                    (*owner.fields).context.as_deref(),
                    2
                ),
                0
            );
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
            crate::swr_v1_tls_owner_free(owner.owner);
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
    fn sendfile_sync_sends_the_range_and_rejects_a_bad_one() {
        let (mut left, right) = pair();
        let (path, name) = temp_file("swoole-sendfile-sync", b"0123456789");
        let sent = unsafe {
            socket_sendfile_sync(
                Socket::from_ptr(&raw mut left).unwrap(),
                name.as_c_str().into(),
                2,
                5,
            )
        };
        assert_eq!(sent, 0);
        assert_eq!(read_all(&right, 5), b"23456");
        let rejected = unsafe {
            socket_sendfile_sync(
                Socket::from_ptr(&raw mut left).unwrap(),
                name.as_c_str().into(),
                20,
                0,
            )
        };
        assert_eq!(rejected, -1, "offset past the end");
        let missing = unsafe {
            socket_sendfile_sync(
                Socket::from_ptr(&raw mut left).unwrap(),
                c"/nonexistent/x".into(),
                0,
                0,
            )
        };
        assert_eq!(missing, -1);
        std::fs::remove_file(path).unwrap();
        unsafe { libc::close(left.fd) };
        unsafe { libc::close(right.fd) };
    }

    #[test]
    fn queued_file_and_data_drain_through_the_handlers() {
        let (mut left, right) = pair();
        let native = swoole_rs_buffer::cpp::new_buffer(65536);
        let queue = native.queue();
        left.output.out_buffer = Some(native);
        let (path, name) = temp_file("swoole-sendfile-async", b"abcdef");
        let socket = &raw mut left;
        let request = Box::into_raw(file_request(&name, 1, 0).unwrap());
        let dispose: Option<unsafe extern "C" fn(*mut c_void)> = Some(SocketFileRequest::dispose);
        let queued = unsafe {
            (*queue).push(
                swoole_rs_buffer::EntryKind::Sendfile,
                request.cast(),
                dispose,
            )
        };
        assert_eq!(queued, 0);
        assert!(
            file_request(c"/nonexistent/x", 0, 0).is_none(),
            "a file that won't open is refused"
        );
        let handle = unsafe { Socket::from_ptr(socket) }.unwrap();
        assert_eq!(socket_handle_sendfile(handle), 0);
        assert_eq!(read_all(&right, 5), b"bcdef");
        assert_eq!(unsafe { (*queue).append(b"tail") }, 0);
        assert_eq!(socket_handle_send(handle), 0);
        assert_eq!(read_all(&right, 4), b"tail");
        assert_eq!(unsafe { (*queue).stats() }, (0, 0), "both chunks popped");
        // The queue is released with its `Buffer`, which ends the test if it can't be.
        drop(left.output.out_buffer.take());
        std::fs::remove_file(path).unwrap();
        unsafe { libc::close(left.fd) };
        unsafe { libc::close(right.fd) };
    }

    #[test]
    fn make_server_socket_binds_and_listens() {
        let socket = make_server_socket(1, c"127.0.0.1".into(), 0, 8);
        assert!(!socket.is_null());
        let record = unsafe { &*socket };
        assert!(record.bound && record.listened);
        assert_ne!(
            u16::from_be(unsafe { record.info.addr.inet_v4.sin_port }),
            0,
            "getsockname filled the port"
        );
        unsafe { socket_free(socket) };
        let invalid = make_server_socket(1, c"not an address".into(), 0, 8);
        assert!(invalid.is_null());
        assert_ne!(log::last_error(), 0);
    }

    #[test]
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
        assert_eq!(sent, 4);
        assert!(check_liveness(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap()
        ));
        let mut buffer = [0u8; 4];
        let received = recv_sync(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap(),
            &mut buffer,
            libc::MSG_WAITALL,
        );
        assert_eq!((received, &buffer), (4, b"ping"));
        assert_eq!(right.total_recv_bytes, 4);
        assert!(right.output.nonblock);
        unsafe { libc::close(fds[0]) };
        assert!(!check_liveness(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap()
        ));
        unsafe { libc::close(fds[1]) };
    }
}
