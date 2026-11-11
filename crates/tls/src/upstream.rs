// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The TLS methods of `network::Socket` that are whole Rust operations, last
//! error, errno and logs included. With `upstream-symbols` (the hybrid's reactor
//! build) they are defined under their upstream (Itanium) symbols and
//! `socket.cc` no longer defines them.
use crate::{
    Cert, SocketTls, Ssl, SslHandshakeResult, SslIoResult, swr_v1_socket_ssl_accept,
    swr_v1_socket_ssl_clear, swr_v1_socket_ssl_connect, swr_v1_socket_ssl_error_reason,
    swr_v1_socket_ssl_peek_reason, swr_v1_socket_ssl_pem, swr_v1_socket_ssl_recv,
    swr_v1_socket_ssl_send, swr_v1_socket_ssl_shutdown, swr_v1_socket_ssl_verify,
};
use openssl_sys::{SSL, X509, X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK, stack_st_X509};
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use std::ptr::NonNull;
use swoole_rs_corebase::net_address::{BAD_HOST_ADDR, assign, platform_parses};
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_corebase::{log, set_errno};
use swoole_rs_reactor::socket_upstream::{
    make_socket_typed, socket_cork, socket_set_fd_option, socket_uncork,
};
use swoole_rs_reactor::{
    CLASS_STREAM, CLASS_TCP, OutputEffects, ReactorIov, SYNC_ONCE, SYNC_RECV, SYNC_SEND,
    SYNC_SENDFILE, Socket, SocketAddress, SocketFields, SocketFileRequest, SocketRecord,
    SyncEffects, fd_recvfrom, handle_send_owned, sendfile_chunk, socket_record_peek,
    socket_record_recv, socket_record_send, ssl_sendfile as ssl_sendfile_fd, steady_ms,
    swr_v1_reactor_poll_events_to, swr_v1_socket_iov_owned, swr_v1_socket_sync, type_is,
    what_event_want,
};

// SAFETY: libc's, libssl's and libcrypto's own declarations.
unsafe extern "C" {
    fn inet_ntop(
        family: c_int,
        address: *const c_void,
        buffer: *mut c_char,
        size: libc::socklen_t,
    ) -> *const c_char;
    fn ERR_error_string_n(error: c_ulong, buffer: *mut c_char, length: usize);
    fn X509_verify_cert_error_string(error: c_long) -> *const c_char;
    fn SSL_get_peer_cert_chain(ssl: *const SSL) -> *mut stack_st_X509;
    /// `SSL_get_peer_certificate`, a macro over `SSL_get1_peer_certificate` since
    /// OpenSSL 3 (build.rs).
    #[cfg_attr(ossl300, link_name = "SSL_get1_peer_certificate")]
    #[cfg_attr(not(ossl300), link_name = "SSL_get_peer_certificate")]
    fn peer_certificate(ssl: *const SSL) -> *mut X509;
}

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING`, `SW_ERROR_FILE_EMPTY`, `SW_ERROR_INVALID_PARAMS`,
/// `SW_ERROR_SSL_VERIFY_FAILED` and `SW_ERROR_SSL_BAD_PROTOCOL`.
const LOG_NOTICE: c_int = 3;
const LOG_WARNING: c_int = 4;
const FILE_EMPTY: c_int = 702;
const INVALID_PARAMS: c_int = 505;
/// `SW_SOCKET_CORK_MIN_SIZE` and the typed queue's `SWR_BUFFER_EMPTY`.
const CORK_MIN_SIZE: usize = 65536;
/// `SW_FD_STREAM_SERVER` and `SW_SOCK_CLOEXEC`.
const FD_STREAM_SERVER: c_int = 1;
const SOCK_CLOEXEC: c_int = 1 << 3;
const BUFFER_EMPTY: i32 = 1;
const VERIFY_FAILED: c_int = 1014;
const BAD_PROTOCOL: c_int = 1016;
/// `swReturnCode`'s `SW_CLOSE` and `SW_ERROR`, `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const CLOSE: c_int = 3;
const ERROR: c_int = 4;
const EVENT_READ: c_int = 1 << 9;
const EVENT_WRITE: c_int = 1 << 10;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// A C string for `%s`, which glibc prints as `(null)` when null.
///
/// # Safety
/// `text` is null or NUL-terminated.
pub(crate) unsafe fn text(text: *const c_char) -> String {
    if text.is_null() {
        return "(null)".into();
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// `Address::get_addr()` and `get_port()` of the socket's own address, as its logs print them.
#[must_use]
pub fn peer(info: &SocketAddress) -> (String, c_int) {
    let mut buffer = [0_u8; 46];
    let (family, address, port): (c_int, *const c_void, u16) = match info.kind {
        // TCP, UDP and raw sockets hold the IPv4 member.
        1 | 2 | 7 => {
            let v4 = info.addr.v4();
            (libc::AF_INET, (&raw const v4.sin_addr).cast(), v4.sin_port)
        }
        // TCP6, UDP6 and raw6 sockets hold the IPv6 member.
        3 | 4 | 8 => {
            let v6 = info.addr.v6();
            (
                libc::AF_INET6,
                (&raw const v6.sin6_addr).cast(),
                v6.sin6_port,
            )
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
    // SAFETY: the address is the selected family's, and the buffer holds the longest IPv6 text.
    let host = unsafe { inet_ntop(family, address, buffer.as_mut_ptr().cast(), 46) };
    // inet_ntop returned null or the buffer, NUL-terminated.
    let host = if host.is_null() {
        "(null)".into()
    } else {
        CStr::from_bytes_until_nul(&buffer).map_or_else(
            |_| String::new(),
            |host| host.to_string_lossy().into_owned(),
        )
    };
    (host, c_int::from(u16::from_be(port)))
}

/// `step` over a copy of the record's TLS group and its SSL (None without one), then
/// the group stored back: OpenSSL's info callback reads the record meanwhile.
fn with_tls<R>(socket: Socket<'_>, step: impl FnOnce(&mut SocketTls, Ssl) -> R) -> Option<R> {
    let (before, ssl) = socket.read(|record| (record.tls, Ssl::of_fields(record)));
    let mut tls = before;
    let result = step(&mut tls, ssl?);
    // Most transfers leave the group as it was, and then the record needs no edit.
    if tls != before {
        socket.edit(|record| record.tls = tls);
    }
    Some(result)
}

/// `static const char *Socket::ssl_get_error_reason(int *reason)`.
///
/// # Safety
/// `reason` is null or writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket20ssl_get_error_reasonEPi")
)]
pub unsafe extern "C" fn socket_ssl_get_error_reason(reason: *mut c_int) -> *const c_char {
    let (code, text) = swr_v1_socket_ssl_error_reason();
    // SAFETY: as the caller promises.
    if let Some(reason) = unsafe { reason.as_mut() } {
        *reason = code;
    }
    text.map_or(std::ptr::null(), CStr::as_ptr)
}

/// `STACK_OF(X509) *Socket::ssl_get_peer_cert_chain() const`.
#[must_use]
pub fn socket_ssl_get_peer_cert_chain(socket: Socket<'_>) -> *mut stack_st_X509 {
    let Some(ssl) = Ssl::of(socket) else {
        return std::ptr::null_mut();
    };
    // SAFETY: the record's live SSL.
    unsafe { SSL_get_peer_cert_chain(ssl.as_ptr()) }
}

/// [`socket_ssl_get_peer_cert_chain`] for upstream's core tests.
///
/// # Safety
/// `socket` is a live record.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZNK6swoole7network6Socket23ssl_get_peer_cert_chainEv")]
unsafe extern "C" fn native_ssl_get_peer_cert_chain(
    socket: *mut SocketRecord,
) -> *mut stack_st_X509 {
    // SAFETY: as the caller promises.
    unsafe { Socket::from_ptr(socket) }.map_or(std::ptr::null_mut(), socket_ssl_get_peer_cert_chain)
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
        // SAFETY: OpenSSL's static string table.
        let reason = unsafe { X509_verify_cert_error_string(error) };
        // SAFETY: null or NUL-terminated.
        let reason = unsafe { text(reason) };
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
///
/// # Safety
/// `socket` is a live record.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZNK6swoole7network6Socket10ssl_verifyEb")]
unsafe extern "C" fn native_ssl_verify(socket: *mut SocketRecord, allow_self_signed: bool) -> bool {
    // SAFETY: as the caller promises.
    unsafe { Socket::from_ptr(socket) }
        .is_some_and(|socket| socket_ssl_verify(socket, allow_self_signed))
}

/// `void Socket::ssl_catch_error() const`.
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

/// `ssize_t Socket::ssl_recv(void *buf, size_t n)`.
///
/// # Safety
/// `buffer` denotes `length` writable bytes.
unsafe fn socket_ssl_recv(socket: Socket<'_>, buffer: *mut c_void, length: usize) -> isize {
    let result = with_tls(socket, |tls, ssl| {
        // SAFETY: the caller's buffer.
        unsafe { swr_v1_socket_ssl_recv(tls, ssl, buffer, length) }
    });
    ssl_io(socket, result)
}

/// `ssize_t Socket::ssl_send(const void *buf, size_t n)`: a DTLS socket sends at
/// most `chunk_size` bytes.
///
/// # Safety
/// `buffer` denotes `length` readable bytes.
unsafe fn socket_ssl_send(socket: Socket<'_>, buffer: *const c_void, length: usize) -> isize {
    let length = socket.read(|record| {
        let chunk = record.chunk_size as usize;
        if record.dtls != 0 && chunk != 0 && length > chunk {
            chunk
        } else {
            length
        }
    });
    let result = with_tls(socket, |tls, ssl| {
        // SAFETY: the caller's buffer.
        unsafe { swr_v1_socket_ssl_send(tls, ssl, buffer, length) }
    });
    ssl_io(socket, result)
}

/// The reactor's TLS read over the record it was given, and
/// `ssize_t Socket::ssl_recv(void *buf, size_t n)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket8ssl_recvEPvm")
)]
unsafe extern "C" fn tls_recv(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket.cast()) }) else {
        return -1;
    };
    // SAFETY: as the caller promises.
    unsafe { socket_ssl_recv(socket, buffer, length) }
}

/// The reactor's TLS write over the record it was given, and
/// `ssize_t Socket::ssl_send(const void *buf, size_t n)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket8ssl_sendEPKvm")
)]
unsafe extern "C" fn tls_send(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket.cast()) }) else {
        return -1;
    };
    // SAFETY: as the caller promises.
    unsafe { socket_ssl_send(socket, buffer, length) }
}

/// `ssize_t Socket::readv(IOVector *)` and `writev`, which upstream's `ssl_readv` and
/// `ssl_writev` are too: through OpenSSL when the socket holds an SSL. `IOVector`'s
/// first member is its owner.
///
/// # Safety
/// `socket` is a live record and `vector` a live `IOVector`.
unsafe fn vector_io(socket: *mut SocketRecord, vector: *const c_void, write: bool) -> isize {
    // SAFETY: as the caller promises.
    let owner = unsafe { *vector.cast::<*mut ReactorIov>() };
    // SAFETY: as the caller promises.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let (fd, ssl) = record.read(|record| (record.fd, record.ssl()));
    let io = (!ssl.is_null()).then_some(if write { tls_send } else { tls_recv });
    // SAFETY: the vector's owner and, with an SSL, a TLS call valid for the record.
    unsafe { swr_v1_socket_iov_owned(fd, write, owner, io, socket.cast()) }
}

/// `ssize_t Socket::readv(IOVector *)`, and `ssl_readv`.
///
/// # Safety
/// `socket` is a live record and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket9ssl_readvEPNS0_8IOVectorE")
)]
pub unsafe extern "C" fn socket_readv(socket: *mut SocketRecord, vector: *const c_void) -> isize {
    // SAFETY: as the caller promises.
    unsafe { vector_io(socket, vector, false) }
}

/// `ssize_t Socket::writev(IOVector *)`, and `ssl_writev`.
///
/// # Safety
/// `socket` is a live record and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket10ssl_writevEPNS0_8IOVectorE")
)]
pub unsafe extern "C" fn socket_writev(socket: *mut SocketRecord, vector: *const c_void) -> isize {
    // SAFETY: as the caller promises.
    unsafe { vector_io(socket, vector, true) }
}

fn handshake(result: SslHandshakeResult) {
    if result.last_error != 0 {
        log::set_last_error(result.last_error);
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
pub fn socket_ssl_accept(socket: Socket<'_>) -> c_int {
    let dtls = socket.read(|record| record.dtls != 0);
    let Some(result) = with_tls(socket, |tls, ssl| swr_v1_socket_ssl_accept(tls, ssl, dtls)) else {
        return ERROR;
    };
    handshake(result);
    if result.log == 1 {
        let (reason, error) = error_reason_text();
        let (host, port) = socket.read(|record| peer(&record.info));
        log::warning(
            c"Socket::ssl_accept",
            format_args!("bad SSL client[{host}:{port}], reason={reason}, error_string={error}"),
        );
    } else if result.log == 2 {
        let code = errno();
        let message = swoole_rs_corebase::sys::strerror(code);
        log::warning(
            c"Socket::ssl_accept",
            format_args!(
                "SSL_do_handshake() failed. Error: {message}[{}|{code}]",
                result.ssl_error
            ),
        );
    }
    result.code
}

/// `int Socket::ssl_connect()`: `SW_ERR` without an SSL.
#[must_use]
pub fn socket_ssl_connect(socket: Socket<'_>) -> c_int {
    let Some(result) = with_tls(socket, swr_v1_socket_ssl_connect) else {
        return -1;
    };
    handshake(result);
    if result.log == 2 {
        let error = crate::socket::pop_error();
        let mut buffer = [0_u8; 512];
        // SAFETY: at most the buffer's length, NUL-terminated.
        unsafe { ERR_error_string_n(error, buffer.as_mut_ptr().cast(), buffer.len()) };
        let message = CStr::from_bytes_until_nul(&buffer)
            .map_or_else(|_| String::new(), |text| text.to_string_lossy().into());
        let ((host, port), fd) = socket.read(|record| (peer(&record.info), record.fd));
        log::notice(format_args!(
            "ssl_connect(fd={fd}) to server[{host}:{port}] failed. Error: {message}[{}|{}]",
            result.ssl_error,
            crate::socket::error_reason(error)
        ));
    }
    result.code
}

/// The reactor's TLS peek over the record it was given.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
unsafe extern "C" fn tls_peek(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket.cast()) }) else {
        return -1;
    };
    let Some(ssl) = Ssl::of(socket) else {
        return -1;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "SSL_peek takes an int length, as upstream passes it"
    )]
    let length = length as c_int;
    // SAFETY: the record's SSL and the caller's buffer.
    unsafe { openssl_sys::SSL_peek(ssl.as_ptr(), buffer, length) as isize }
}

/// `ssize_t Socket::recv(void *, size_t, int)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket4recvEPvmi")
)]
pub unsafe extern "C" fn socket_recv(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    // SAFETY: as the caller promises; the TLS call is valid for the socket.
    unsafe { socket_record_recv(socket, buffer, length, flags, tls_recv, steady_ms) }
}

/// `ssize_t Socket::send(const void *, size_t, int)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket4sendEPKvmi")
)]
pub unsafe extern "C" fn socket_send(
    socket: *mut SocketRecord,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    // SAFETY: as the caller promises; the TLS call is valid for the socket.
    unsafe { socket_record_send(socket, buffer, length, flags, tls_send, steady_ms) }
}

/// [`socket_recv`] into `buffer`.
pub fn recv(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    let (at, length) = (buffer.as_mut_ptr().cast(), buffer.len());
    // SAFETY: the buffer's writable bytes; the TLS call is valid for the socket.
    unsafe { socket_record_recv(socket, at, length, flags, tls_recv, steady_ms) }
}

/// [`socket_send`] of `data`.
#[must_use]
pub fn send(socket: Socket<'_>, data: &[u8], flags: c_int) -> isize {
    let (at, length) = (data.as_ptr().cast(), data.len());
    // SAFETY: the data's bytes; the TLS call is valid for the socket.
    unsafe { socket_record_send(socket, at, length, flags, tls_send, steady_ms) }
}

/// [`socket_recv_sync`] into `buffer`.
pub fn recv_sync(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    let (at, length) = (buffer.as_mut_ptr().cast(), buffer.len());
    // SAFETY: a live record and the buffer's writable bytes.
    unsafe {
        sync(
            socket.as_ptr(),
            Io::Recv,
            SYNC_RECV,
            at,
            length,
            flags,
            false,
        )
    }
}

/// [`socket_send_sync`] of `data`.
#[must_use]
pub fn send_sync(socket: Socket<'_>, data: &[u8], flags: c_int) -> isize {
    let (at, length) = (data.as_ptr().cast(), data.len());
    // SAFETY: a live record and the data's bytes.
    unsafe {
        sync(
            socket.as_ptr(),
            Io::Send,
            SYNC_SEND,
            at,
            length,
            flags,
            true,
        )
    }
}

/// [`socket_recvfrom_sync_address`] into `buffer` and `address`.
pub fn recvfrom_sync(
    socket: Socket<'_>,
    buffer: &mut [impl ReadSlot],
    flags: c_int,
    address: &mut SocketAddress,
) -> isize {
    let io = Io::RecvFrom((&raw mut address.addr).cast(), &raw mut address.len);
    let (at, length) = (buffer.as_mut_ptr().cast(), buffer.len());
    // SAFETY: a live record, the buffer's writable bytes and the address's storage.
    unsafe { sync(socket.as_ptr(), io, SYNC_ONCE, at, length, flags, false) }
}

/// `ssize_t Socket::peek(void *, size_t, int) const`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket4peekEPvmi")
)]
pub unsafe extern "C" fn socket_peek(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    // SAFETY: as the caller promises; the TLS call is valid for the socket.
    unsafe { socket_record_peek(socket, buffer, length, flags, tls_peek) }
}

/// [`socket_peek`] into `buffer`.
pub fn peek(socket: Socket<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    let (at, length) = (buffer.as_mut_ptr().cast(), buffer.len());
    // SAFETY: `buffer`'s bytes; the TLS call is valid for the socket.
    unsafe { socket_record_peek(socket, at, length, flags, tls_peek) }
}

/// `bool Socket::check_liveness()`: a nonblocking one-byte peek that neither
/// ends the stream nor fails with an error that closes it.
///
/// # Safety
/// `socket` is null or a live socket on this thread.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket14check_livenessEv")
)]
pub unsafe extern "C" fn socket_check_liveness(socket: *mut SocketRecord) -> bool {
    // SAFETY: as the caller promises.
    unsafe { Socket::from_ptr(socket) }.is_some_and(check_liveness)
}

/// [`socket_check_liveness`] over a socket's handle.
#[must_use]
pub fn check_liveness(socket: Socket<'_>) -> bool {
    let mut byte = [0u8];
    set_errno(0);
    let peeked = peek(socket, &mut byte, libc::MSG_DONTWAIT);
    !(peeked == 0 || (peeked < 0 && socket.catch_error(errno()) == CLOSE))
}

/// One blocking call: the socket and its I/O's operands. Only [`sync`], whose caller
/// promises a live record and valid operands, builds one.
struct Call<'a> {
    socket: Socket<'a>,
    io: Io,
    dont_restart: bool,
}

/// The I/O a blocking call repeats, with the operands beyond the loop's buffer.
#[derive(Clone, Copy)]
enum Io {
    Send,
    Recv,
    Read,
    Write,
    SendTo(*const SocketAddress),
    RecvFrom(*mut libc::sockaddr, *mut libc::socklen_t),
    Writev(*const libc::iovec, usize),
    Sendfile(*mut SocketFileRequest),
}

impl SyncEffects for Call<'_> {
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
            if record.output.nonblock == 0 {
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

    unsafe fn io(&self, buffer: *mut c_void, length: usize, flags: c_int) -> isize {
        let socket = self.socket;
        // Read on each call: a signal handler the wait dispatches may have changed it.
        let fd = socket.fd();
        match self.io {
            // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
            Io::Send => unsafe { socket_send(socket.as_ptr(), buffer, length, flags) },
            // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
            Io::Recv => unsafe { socket_recv(socket.as_ptr(), buffer, length, flags) },
            // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
            Io::Read => unsafe { libc::read(fd, buffer, length) },
            // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
            Io::Write => unsafe { libc::write(fd, buffer, length) },
            Io::SendTo(address) => {
                // SAFETY: the caller's live `network::Address`.
                let size = unsafe { (*address).len };
                // SAFETY: the loop's buffer, and the address's storage is its first member.
                unsafe { libc::sendto(fd, buffer, length, flags, address.cast(), size) }
            }
            Io::RecvFrom(address, size) => {
                // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
                unsafe { fd_recvfrom(fd, buffer, length, flags, address, size) }
            }
            Io::Sendfile(request) => {
                // SAFETY: the call's live request.
                let request = unsafe { &mut *request };
                send_chunk(socket, request).0
            }
            Io::Writev(vectors, count) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_possible_wrap,
                    reason = "writev takes an int count, as upstream passes it"
                )]
                let count = count as c_int;
                // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
                unsafe { libc::writev(fd, vectors, count) }
            }
        }
    }

    fn catch_error(&self, error: c_int) -> c_int {
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
///
/// # Safety
/// `socket` is a live record, `buffer` denotes `length` bytes valid for `io`, and
/// `io`'s operands are valid for it.
unsafe fn sync(
    socket: *mut SocketRecord,
    io: Io,
    kind: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    write: bool,
) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let (timeout, dont_restart) = socket.read(|record| {
        let timeouts = record.timeouts;
        let timeout = if write { timeouts.write } else { timeouts.read };
        (timeout, record.dont_restart != 0)
    });
    let call = Call {
        socket,
        io,
        dont_restart,
    };
    let event = if write { EVENT_WRITE } else { EVENT_READ };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sec2msec's long is passed on as an int, as upstream does"
    )]
    let timeout_ms = (timeout * 1000.0) as c_long as c_int;
    let buffer = buffer.cast_mut();
    // SAFETY: the caller's buffer, valid for the call's I/O.
    unsafe { swr_v1_socket_sync(&call, kind, buffer, length, flags, event, timeout_ms) }
}

/// `ssize_t Socket::send_sync(const void *, size_t, int)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` readable bytes.
pub unsafe fn socket_send_sync(
    socket: *mut SocketRecord,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: as the caller promises.
    unsafe { sync(socket, Io::Send, SYNC_SEND, buffer, length, flags, true) }
}

/// `ssize_t Socket::recv_sync(void *, size_t, int)`: `MSG_WAITALL` is cleared for
/// each call, since under nonblocking I/O it would spin on EAGAIN.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket9recv_syncEPvmi")
)]
pub unsafe extern "C" fn socket_recv_sync(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: as the caller promises.
    unsafe { sync(socket, Io::Recv, SYNC_RECV, buffer, length, flags, false) }
}

/// `ssize_t Socket::read_sync(void *, size_t)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket9read_syncEPvm")
)]
pub unsafe extern "C" fn socket_read_sync(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    unsafe { sync(socket, Io::Read, SYNC_ONCE, buffer, length, 0, false) }
}

/// `ssize_t Socket::write_sync(const void *, size_t)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket10write_syncEPKvm")
)]
pub unsafe extern "C" fn socket_write_sync(
    socket: *mut SocketRecord,
    buffer: *const c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    unsafe { sync(socket, Io::Write, SYNC_ONCE, buffer, length, 0, true) }
}

/// `ssize_t Socket::sendto_sync(const Address &, const void *, size_t, int)`.
///
/// # Safety
/// `socket` is a live record, `address` a live `network::Address` and `buffer`
/// denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket11sendto_syncERKNS0_7AddressEPKvmi")
)]
pub unsafe extern "C" fn socket_sendto_sync(
    socket: *mut SocketRecord,
    address: *const SocketAddress,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
) -> isize {
    let io = Io::SendTo(address);
    // SAFETY: as the caller promises.
    unsafe { sync(socket, io, SYNC_ONCE, buffer, length, flags, true) }
}

/// `ssize_t Socket::recvfrom_sync(char *, size_t, int, Address *)`, into the
/// address's storage and length.
///
/// # Safety
/// `socket` is a live record, `buffer` denotes `length` writable bytes, and
/// `address` is a live `network::Address`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket13recvfrom_syncEPcmiPNS0_7AddressE")
)]
pub unsafe extern "C" fn socket_recvfrom_sync_address(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    address: *mut SocketAddress,
) -> isize {
    // SAFETY: as the caller promises.
    let address = unsafe { &mut *address };
    let (storage, size) = (&raw mut address.addr, &raw mut address.len);
    let io = Io::RecvFrom(storage.cast(), size);
    // SAFETY: as the caller promises.
    unsafe { sync(socket, io, SYNC_ONCE, buffer, length, flags, false) }
}

/// `ssize_t Socket::writev_sync(const iovec *, size_t)`.
///
/// # Safety
/// `socket` is a live record and `vectors` denotes `count` readable `iovec`s.
pub unsafe fn socket_writev_sync(
    socket: *mut SocketRecord,
    vectors: *const libc::iovec,
    count: usize,
) -> isize {
    let io = Io::Writev(vectors, count);
    // SAFETY: as the caller promises.
    unsafe { sync(socket, io, SYNC_ONCE, std::ptr::null(), 0, 0, true) }
}

/// `X509 *Socket::ssl_get_peer_certificate() const`.
#[must_use]
pub fn ssl_get_peer_certificate(socket: Socket<'_>) -> Option<Cert> {
    let ssl = Ssl::of(socket)?;
    // SAFETY: the record's live SSL; the reference it returns is ours.
    NonNull::new(unsafe { peer_certificate(ssl.as_ptr()) }).map(Cert)
}

/// `bool Socket::ssl_check_host(const char *tls_host_name) const`.
#[must_use]
pub fn socket_ssl_check_host(socket: Socket<'_>, name: &CStr) -> bool {
    let Some(cert) = ssl_get_peer_certificate(socket) else {
        return false;
    };
    if cert.matches_host(name.to_bytes()) {
        return true;
    }
    log::warning(
        c"Socket::ssl_check_host",
        format_args!("X509_check_host(): no match"),
    );
    false
}

/// `int Socket::ssl_get_peer_certificate(char *buf, size_t n) const`: the PEM's
/// length, or `SW_ERR`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket24ssl_get_peer_certificateEPcm")
)]
pub unsafe extern "C" fn socket_ssl_get_peer_certificate_pem(
    socket: *const SocketRecord,
    buffer: *mut c_char,
    length: usize,
) -> c_int {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket.cast_mut()) }) else {
        return -1;
    };
    let Some(cert) = ssl_get_peer_certificate(socket) else {
        return -1;
    };
    // SAFETY: the certificate and the caller's buffer.
    unsafe { swr_v1_socket_ssl_read_x509(cert.as_ptr(), buffer, length, false) }
}

/// `_ssl_read_x509_file`: `swr_v1_socket_ssl_pem` with its warnings, -1 on failure.
///
/// # Safety
/// As `swr_v1_socket_ssl_pem`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_ssl_read_x509(
    cert: *mut X509,
    buffer: *mut c_char,
    length: usize,
    consume: bool,
) -> c_int {
    // SAFETY: as the caller promises.
    let written = unsafe { swr_v1_socket_ssl_pem(cert, buffer, length, consume) };
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

/// The leading fields of `swoole::String`.
#[repr(C)]
struct NativeString {
    length: usize,
    size: usize,
    offset: libc::off_t,
    str: *mut c_char,
}

/// `bool Socket::ssl_get_peer_certificate(String *buf) const`.
///
/// # Safety
/// `socket` is a live record and `buffer` a live `String`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket24ssl_get_peer_certificateEPNS_6StringE")
)]
pub unsafe extern "C" fn socket_ssl_get_peer_certificate_string(
    socket: *const SocketRecord,
    buffer: *mut c_void,
) -> bool {
    // SAFETY: the caller's live String, lent for the call; the PEM goes to the bytes
    // it owns at `str`, outside these fields.
    let string = unsafe { &mut *buffer.cast::<NativeString>() };
    let (str, size) = (string.str, string.size);
    // SAFETY: as the caller promises; the String owns `size` bytes at `str`.
    let n = unsafe { socket_ssl_get_peer_certificate_pem(socket, str, size) };
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    string.length = n;
    true
}

/// `Socket::ssl_sendfile` over a descriptor.
fn ssl_sendfile(socket: Socket<'_>, fd: c_int, offset: &mut libc::off_t, size: usize) -> isize {
    let mut nobufs = false;
    // SAFETY: the caller's offset, and tls_send for this record, which checks its SSL.
    let result = unsafe {
        ssl_sendfile_fd(
            fd,
            offset,
            size,
            tls_send,
            socket.as_ptr().cast(),
            &mut nobufs,
        )
    };
    if nobufs {
        socket.edit(|record| record.kernel_nobufs = true);
    }
    if result.warn == 1 {
        log::sys_warning(c"Socket::ssl_sendfile", format_args!("write() failed"));
    } else if result.warn == 2 {
        log::sys_warning(c"Socket::ssl_sendfile", format_args!("pread() failed"));
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
    let status = request.check(length);
    if status == 0 {
        return Some(request);
    }
    let name = filename.to_string_lossy();
    match status {
        1 => log::sys_warning(CHECK_ORIGIN, format_args!("open('{name}') failed")),
        2 => {
            log::sys_warning(c"File::stat", format_args!("fstat() failed"));
            log::sys_warning(CHECK_ORIGIN, format_args!("fstat('{name}') failed"));
        }
        3 => log::error_log(
            LOG_WARNING,
            FILE_EMPTY,
            CHECK_ORIGIN,
            format_args!("cannot send empty file '{name}'"),
        ),
        _ => {
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
/// corked for a large file, through `wait_for`'s loop.
///
/// # Safety
/// `socket` is a live record and `filename` NUL-terminated.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZN6swoole7network6Socket13sendfile_syncEPKcxm")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole7network6Socket13sendfile_syncEPKclm")
)]
pub unsafe extern "C" fn socket_sendfile_sync(
    socket: *mut SocketRecord,
    filename: *const c_char,
    offset: libc::off_t,
    length: usize,
) -> c_int {
    // SAFETY: as the caller promises.
    let Some(mut request) = file_request(unsafe { CStr::from_ptr(filename) }, offset, length)
    else {
        return -1;
    };
    let total = request.length();
    // SAFETY: as the caller promises.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let corked = total > CORK_MIN_SIZE && record.edit(|record| socket_cork(record));
    // SAFETY: the request outlives the loop, and the loop's buffer is unused.
    let rv = unsafe {
        sync(
            socket,
            Io::Sendfile(&raw mut *request),
            SYNC_SENDFILE,
            std::ptr::null(),
            total,
            0,
            true,
        )
    };
    if corked {
        record.edit(|record| socket_uncork(record));
    }
    if rv < 0 { -1 } else { 0 }
}

/// A failed Rust queue status. Native `Buffer` raises it as a C++ exception that
/// nothing catches, so the process ends either way; here it aborts.
pub(crate) fn buffer_check(status: i32) {
    if status != 0 {
        std::process::abort();
    }
}

/// A socket's `out_buffer` queue, null without one: `Buffer`'s first member is its
/// Rust handle.
pub(crate) fn out_queue(socket: Socket<'_>) -> *mut swoole_rs_buffer::Buffer {
    let buffer = socket.read(|record| record.output.out_buffer);
    if buffer.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: a socket's output buffer is a live `Buffer`, which only the socket frees.
    unsafe { *buffer.cast::<*mut swoole_rs_buffer::Buffer>() }
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

    fn catch_error(&self, error: c_int) -> c_int {
        self.0.catch_error(error)
    }
}

/// `int Socket::handle_send()`: sends the queue's front, aborting without one.
#[must_use]
pub fn socket_handle_send(record: Socket<'_>) -> c_int {
    let hooks = Output(record);
    let queue = out_queue(record);
    // SAFETY: the live record's queue and the hooks over it.
    let output = unsafe { handle_send_owned(record, queue, &hooks) };
    buffer_check(output.status);
    if output.result.warn == 1 {
        let fd = record.fd();
        log::sys_warning(
            c"Socket::handle_send",
            format_args!("send to fd[{fd}] failed"),
        );
    }
    if let Some(dispose) = output.detached.dispose {
        // SAFETY: the popped chunk's token, now ours to dispose.
        unsafe { dispose(output.detached.token) };
    }
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
    let mut front = swoole_rs_buffer::TypedFront::default();
    buffer_check(with_queue(socket, |queue| {
        swoole_rs_buffer::swr_v1_buffer_typed_front(queue.as_deref(), Some(&mut front))
    }));
    if front.kind != SENDFILE_TOKEN {
        std::process::abort();
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
    if result.warn == 2 {
        let at = request.fields().begin;
        let name = String::from_utf8_lossy(request.path());
        log::sys_warning(
            c"Socket::handle_sendfile",
            format_args!("sendfile({name}, {at}, {}) failed", result.requested),
        );
    }
    if result.pop {
        let mut detached = swoole_rs_buffer::TypedDetached::default();
        let status = with_queue(socket, |queue| {
            swoole_rs_buffer::swr_v1_buffer_typed_pop(queue, Some(&mut detached))
        });
        if status != BUFFER_EMPTY {
            buffer_check(status);
            if let Some(dispose) = detached.dispose {
                // SAFETY: the popped request, now ours to dispose.
                unsafe { dispose(detached.token) };
            }
        }
    }
    result.code
}

/// The queue's token kind for a [`SocketFileRequest`].
const SENDFILE_TOKEN: u32 = 1;

/// `int Socket::sendfile_async(const char *filename, off_t offset, size_t length)`:
/// the checked request queued behind the output, which is created on first use.
///
/// # Safety
/// `socket` is a live record and `filename` NUL-terminated.
pub unsafe fn socket_sendfile_async(
    socket: *mut SocketRecord,
    filename: *const c_char,
    offset: libc::off_t,
    length: usize,
) -> c_int {
    // SAFETY: as the caller promises.
    let Some(request) = file_request(unsafe { CStr::from_ptr(filename) }, offset, length) else {
        return -1;
    };
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let request = Box::into_raw(request);
    if socket.read(|record| record.output.out_buffer.is_null()) {
        let buffer = swoole_rs_buffer::cpp::new_buffer(65536);
        socket.edit(|record| record.output.out_buffer = buffer.cast());
    }
    let dispose = Some(SocketFileRequest::dispose as unsafe extern "C" fn(*mut c_void));
    let status = with_queue(socket, |queue| {
        // SAFETY: the queue now owns the request.
        unsafe {
            swoole_rs_buffer::swr_v1_buffer_typed_push(
                queue,
                SENDFILE_TOKEN,
                request.cast(),
                dispose,
            )
        }
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
/// `socket` is a live record and `data` denotes `length` readable bytes.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZN6swoole7network6Socket10send_asyncEPKvm")]
pub unsafe extern "C-unwind" fn socket_send_async(
    socket: *mut SocketRecord,
    data: *const c_void,
    length: usize,
) -> isize {
    if swoole_rs_reactor::socket_upstream::event_reactor().is_none() {
        // SAFETY: as the caller promises.
        return unsafe { socket_send_sync(socket, data, length, 0) };
    }
    // SAFETY: as the caller promises, on the thread's available reactor.
    unsafe { swoole_rs_reactor::event_api::swoole_event_write(socket, data, length) }
}

/// `bool Socket::ssl_shutdown()`: false without an SSL.
#[expect(
    clippy::must_use_candidate,
    reason = "ssl_close ignores the result, as upstream does"
)]
pub fn socket_ssl_shutdown(socket: Socket<'_>) -> bool {
    let repaired = HALFCLOSE_REPAIRS;
    let (quiet, mut closed) =
        socket.read(|record| (record.ssl_quiet_shutdown != 0, record.ssl_closed));
    let Some(result) = with_tls(socket, |tls, ssl| {
        swr_v1_socket_ssl_shutdown(tls, ssl, &mut closed, quiet, repaired)
    }) else {
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

/// `void Socket::ssl_close()`: shuts down unless already closed (OpenSSL 1.0.2f
/// complains about `SSL_shutdown` during a handshake), then frees the SSL.
pub fn socket_ssl_close(socket: Socket<'_>) {
    if socket.read(|record| record.ssl_closed) == 0 {
        socket_ssl_shutdown(socket);
    }
    let ssl = socket.edit(swoole_rs_reactor::SocketFields::take_ssl);
    // SAFETY: the record's SSL (or null), now released.
    unsafe { openssl_sys::SSL_free(ssl.cast()) };
}

/// `int Socket::ssl_create(SSLContext *ssl_context, int flags)`: `SW_OK` or `SW_ERR`,
/// without setting the last error.
///
/// # Safety
/// `socket` is a live record and `context` a live `SSLContext` (its Rust owner).
pub unsafe fn socket_ssl_create(
    socket: *mut SocketRecord,
    context: *const crate::OwnedTlsContext,
    flags: c_int,
) -> c_int {
    // SAFETY: as the caller promises.
    let context = unsafe { crate::owner_fields(context) }.context;
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let (this, index) = (socket.as_ptr().cast(), crate::swr_v1_tls_connection_index());
    let (status, error) = socket.edit(|record| {
        let mut error = 0;
        // SAFETY: a live context; OpenSSL calls nothing back while making the SSL.
        let status = unsafe {
            crate::swr_v1_socket_ssl_new(record, context, flags, index, this, &mut error)
        };
        (status, error)
    });
    match status {
        0 => return 0,
        1 => log::warning(c"Socket::ssl_create", format_args!("SSL_new() failed")),
        2 => {
            let reason =
                crate::reason_string(error).map_or_else(|| "(null)".into(), CStr::to_string_lossy);
            log::warning(
                c"Socket::ssl_create",
                format_args!("SSL_set_fd() failed. Error: {reason}[{error}]"),
            );
        }
        _ => log::warning(
            c"Socket::ssl_create",
            format_args!("SSL_set_ex_data() failed"),
        ),
    }
    -1
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
        let (timers, buffers) = record.read(|record| {
            (
                [record.recv_timer, record.output.send_timer],
                [record.in_buffer, record.output.out_buffer],
            )
        });
        for timer in timers {
            if !timer.is_null() {
                // SAFETY: the socket's live timer.
                unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
            }
        }
        for buffer in buffers {
            if !buffer.is_null() {
                // SAFETY: the socket owns each `new Buffer`.
                unsafe { swoole_rs_buffer::cpp::delete_buffer(buffer.cast()) };
            }
        }
        if let Some((_, reactor)) = swoole_rs_reactor::socket_upstream::event_reactor() {
            if record.read(|record| record.fd != -1 && record.removed == 0) {
                // SAFETY: `swoole_event_del(this)` on the thread's live reactor; del
                // never allocates, and a failure is logged.
                unsafe {
                    swoole_rs_reactor::swr_v1_reactor_change(
                        reactor.owner,
                        socket,
                        2,
                        0,
                        swoole_rs_reactor::socket_upstream::swr_v1_reactor_log,
                    )
                };
            }
            // SAFETY: `swoole_event_defer(socket_free_defer, this)`; a reactor freed
            // before it runs closes the socket all the same.
            unsafe {
                swoole_rs_reactor::defer(
                    reactor.fields,
                    socket_free_defer,
                    socket.cast(),
                    Some(socket_free_defer),
                );
            }
            return;
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
        // SAFETY: the platform's.
        unsafe extern "C" {
            fn inet_pton(family: c_int, source: *const c_char, destination: *mut c_void) -> c_int;
        }
        let mut bytes = [0; 16];
        // SAFETY: the NUL-terminated host and room for either family's address.
        let parsed = platform_parses(family, text)
            && unsafe { inet_pton(family, host.as_ptr(), bytes.as_mut_ptr().cast()) } == 1;
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
/// a bound (and, for a stream, listening) socket, or null after a warning.
///
/// # Safety
/// `address` is NUL-terminated.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole18make_server_socketE12swSocketTypePKcii")
)]
pub unsafe extern "C-unwind" fn make_server_socket(
    kind: c_int,
    address: *const c_char,
    port: c_int,
    backlog: c_int,
) -> *mut SocketRecord {
    const ORIGIN: &CStr = c"make_server_socket";
    let socket = make_socket_typed(kind, FD_STREAM_SERVER, SOCK_CLOEXEC);
    // SAFETY: null or the new record, which only this function holds.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        log::sys_warning(ORIGIN, format_args!("socket() failed"));
        return socket;
    };
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
    // SAFETY: as the caller promises.
    let address = unsafe { CStr::from_ptr(address) };
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
        let mut pem = [0 as c_char; 64];
        let n = unsafe { socket_ssl_get_peer_certificate_pem(socket, pem.as_mut_ptr(), 64) };
        assert_eq!(n, -1);
        let mut string = NativeString {
            length: 7,
            size: pem.len(),
            offset: 0,
            str: pem.as_mut_ptr(),
        };
        let filled =
            unsafe { socket_ssl_get_peer_certificate_string(socket, (&raw mut string).cast()) };
        assert!(!filled);
        assert_eq!(string.length, 7, "untouched on failure");
    }

    #[test]
    fn ssl_create_then_close_frees_the_ssl() {
        unsafe {
            let owner = crate::swr_v1_tls_owner_new();
            (*owner.fields).context = openssl_sys::SSL_CTX_new(openssl_sys::TLS_method());
            let mut fds = [0; 2];
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()),
                0
            );
            let mut socket = SocketRecord::with_fd(fds[0], 0);
            socket.tls.state = 2;
            socket.tls.want_read = 1;
            let created = socket_ssl_create(&raw mut socket, owner.owner, 2);
            assert_eq!(created, 0);
            assert!(!socket.ssl().is_null());
            assert_eq!(
                (socket.tls.state, socket.tls.want_read),
                (0, 0),
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
        let sent = unsafe { socket_sendfile_sync(&raw mut left, name.as_ptr(), 2, 5) };
        assert_eq!(sent, 0);
        assert_eq!(read_all(&right, 5), b"23456");
        let rejected = unsafe { socket_sendfile_sync(&raw mut left, name.as_ptr(), 20, 0) };
        assert_eq!(rejected, -1, "offset past the end");
        let missing =
            unsafe { socket_sendfile_sync(&raw mut left, c"/nonexistent/x".as_ptr(), 0, 0) };
        assert_eq!(missing, -1);
        std::fs::remove_file(path).unwrap();
        unsafe { libc::close(left.fd) };
        unsafe { libc::close(right.fd) };
    }

    #[test]
    fn queued_file_and_data_drain_through_the_handlers() {
        let (mut left, right) = pair();
        let mut queue: *mut swoole_rs_buffer::Buffer = std::ptr::null_mut();
        assert_eq!(
            swoole_rs_buffer::swr_v1_buffer_typed_create(65536, Some(&mut queue)),
            0
        );
        // `Buffer`'s only member is its Rust handle.
        let mut native = Box::new(queue);
        left.output.out_buffer = (&raw mut *native).cast();
        let (path, name) = temp_file("swoole-sendfile-async", b"abcdef");
        let socket = &raw mut left;
        let request = Box::into_raw(file_request(&name, 1, 0).unwrap());
        let dispose = Some(SocketFileRequest::dispose as unsafe extern "C" fn(*mut c_void));
        let queued = unsafe {
            swoole_rs_buffer::swr_v1_buffer_typed_push(queue.as_mut(), 1, request.cast(), dispose)
        };
        assert_eq!(queued, 0);
        assert!(
            file_request(c"/nonexistent/x", 0, 0).is_none(),
            "a file that won't open is refused"
        );
        let handle = unsafe { Socket::from_ptr(socket) }.unwrap();
        assert_eq!(socket_handle_sendfile(handle), 0);
        assert_eq!(read_all(&right, 5), b"bcdef");
        assert_eq!(
            unsafe { swoole_rs_buffer::swr_v1_buffer_append(queue.as_mut(), b"tail".as_ptr(), 4) },
            0
        );
        assert_eq!(socket_handle_send(handle), 0);
        assert_eq!(read_all(&right, 4), b"tail");
        let (mut length, mut count) = (0, 0);
        swoole_rs_buffer::swr_v1_buffer_stats(
            unsafe { queue.as_ref() },
            Some(&mut length),
            Some(&mut count),
        );
        assert_eq!((length, count), (0, 0), "both chunks popped");
        assert_eq!(unsafe { swoole_rs_buffer::swr_v1_buffer_release(queue) }, 0);
        std::fs::remove_file(path).unwrap();
        unsafe { libc::close(left.fd) };
        unsafe { libc::close(right.fd) };
    }

    #[test]
    fn make_server_socket_binds_and_listens() {
        let socket = unsafe { make_server_socket(1, c"127.0.0.1".as_ptr(), 0, 8) };
        assert!(!socket.is_null());
        let record = unsafe { &*socket };
        assert_eq!((record.bound, record.listened), (1, 1));
        assert_ne!(
            u16::from_be(unsafe { record.info.addr.inet_v4.sin_port }),
            0,
            "getsockname filled the port"
        );
        unsafe { socket_free(socket) };
        let invalid = unsafe { make_server_socket(1, c"not an address".as_ptr(), 0, 8) };
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
        let sent = unsafe { socket_send_sync(&raw mut left, b"ping".as_ptr().cast(), 4, 0) };
        assert_eq!(sent, 4);
        assert!(check_liveness(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap()
        ));
        let mut buffer = [0u8; 4];
        let received = unsafe {
            socket_recv_sync(
                &raw mut right,
                buffer.as_mut_ptr().cast(),
                4,
                libc::MSG_WAITALL,
            )
        };
        assert_eq!((received, &buffer), (4, b"ping"));
        assert_eq!(right.total_recv_bytes, 4);
        assert_ne!(right.output.nonblock, 0);
        unsafe { libc::close(fds[0]) };
        assert!(!check_liveness(
            unsafe { Socket::from_ptr(&raw mut right) }.unwrap()
        ));
        unsafe { libc::close(fds[1]) };
    }
}
