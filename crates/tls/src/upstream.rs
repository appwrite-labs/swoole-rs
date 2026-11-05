// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The TLS methods of `network::Socket` that are whole Rust operations, last
//! error, errno and logs included. With `upstream-symbols` (the hybrid's reactor
//! build) they are defined under their upstream (Itanium) symbols and
//! `socket.cc` no longer defines them.
use crate::{
    SocketTls, SslHandshakeResult, SslIoResult, swr_v1_socket_ssl_accept,
    swr_v1_socket_ssl_connect, swr_v1_socket_ssl_error_reason, swr_v1_socket_ssl_peek_reason,
    swr_v1_socket_ssl_recv, swr_v1_socket_ssl_send, swr_v1_socket_ssl_verify,
};
use openssl_sys::{SSL, X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK, stack_st_X509};
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use swoole_rs_corebase::log;
use swoole_rs_reactor::{ReactorIov, SocketAddress, SocketRecord, swr_v1_socket_iov_owned};

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
}

/// `SW_LOG_NOTICE`, `SW_ERROR_SSL_VERIFY_FAILED` and `SW_ERROR_SSL_BAD_PROTOCOL`.
const LOG_NOTICE: c_int = 3;
const VERIFY_FAILED: c_int = 1014;
const BAD_PROTOCOL: c_int = 1016;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__error() };
    // SAFETY: the slot is this thread's live errno.
    unsafe { *slot = value };
}

/// A C string for `%s`, which glibc prints as `(null)` when null.
///
/// # Safety
/// `text` is null or NUL-terminated.
unsafe fn text(text: *const c_char) -> String {
    if text.is_null() {
        return "(null)".into();
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// `Address::get_addr()` and `get_port()` of the socket's own address, as its logs print them.
fn peer(info: &SocketAddress) -> (String, c_int) {
    let mut buffer = [0 as c_char; 46];
    let (family, address, port): (c_int, *const c_void, u16) = match info.kind {
        // SAFETY: TCP, UDP and raw sockets hold the IPv4 union member.
        1 | 2 | 7 => unsafe {
            (
                libc::AF_INET,
                (&raw const info.addr.inet_v4.sin_addr).cast(),
                info.addr.inet_v4.sin_port,
            )
        },
        // SAFETY: TCP6, UDP6 and raw6 sockets hold the IPv6 union member.
        3 | 4 | 8 => unsafe {
            (
                libc::AF_INET6,
                (&raw const info.addr.inet_v6.sin6_addr).cast(),
                info.addr.inet_v6.sin6_port,
            )
        },
        5 | 6 => {
            // SAFETY: Unix sockets hold the Unix union member's NUL-terminated path.
            let path = unsafe { text((&raw const info.addr.un.sun_path).cast()) };
            return (path, 0);
        }
        _ => return ("unknown".into(), 0),
    };
    // SAFETY: the address is the selected family's, and the buffer holds the longest IPv6 text.
    let host = unsafe { inet_ntop(family, address, buffer.as_mut_ptr(), 46) };
    // SAFETY: inet_ntop returned null or the NUL-terminated buffer.
    (unsafe { text(host) }, c_int::from(u16::from_be(port)))
}

/// The record's TLS state group, which `SocketTls` mirrors.
fn tls(socket: &mut SocketRecord) -> *mut SocketTls {
    (&raw mut socket.tls).cast()
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
    // SAFETY: as the caller promises.
    unsafe { swr_v1_socket_ssl_error_reason(reason) }
}

/// `STACK_OF(X509) *Socket::ssl_get_peer_cert_chain() const`.
///
/// # Safety
/// `socket` is a live record.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket23ssl_get_peer_cert_chainEv")
)]
pub unsafe extern "C" fn socket_ssl_get_peer_cert_chain(
    socket: *const SocketRecord,
) -> *mut stack_st_X509 {
    // SAFETY: as the caller promises.
    let ssl = unsafe { (*socket).ssl };
    if ssl.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the record's live SSL.
    unsafe { SSL_get_peer_cert_chain(ssl.cast()) }
}

/// `bool Socket::ssl_verify(bool allow_self_signed) const`.
///
/// # Safety
/// `socket` is a live record holding an SSL.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket10ssl_verifyEb")
)]
pub unsafe extern "C" fn socket_ssl_verify(
    socket: *const SocketRecord,
    allow_self_signed: bool,
) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: the record's live SSL.
    let error = unsafe { swr_v1_socket_ssl_verify(socket.ssl.cast(), allow_self_signed) };
    if error == c_long::from(X509_V_OK) {
        return true;
    }
    let fd = socket.fd;
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

/// `void Socket::ssl_catch_error() const`.
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket15ssl_catch_errorEv")
)]
pub unsafe extern "C" fn socket_ssl_catch_error(socket: *const SocketRecord) {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    let (host, port) = peer(&socket.info);
    let reason = swr_v1_socket_ssl_peek_reason();
    log::error_log(
        LOG_NOTICE,
        BAD_PROTOCOL,
        c"Socket::ssl_catch_error",
        format_args!(
            "SSL connection#{}[{host}:{port}] protocol error[{reason}]",
            socket.fd
        ),
    );
}

/// `ssl_io`: the protocol error's log, then errno.
///
/// # Safety
/// `socket` is a live record.
unsafe fn ssl_io(socket: *const SocketRecord, result: SslIoResult) -> isize {
    if result.protocol_error {
        // SAFETY: as the caller promises.
        unsafe { socket_ssl_catch_error(socket) };
    }
    if result.error != 0 {
        set_errno(result.error);
    }
    result.value
}

/// `ssize_t Socket::ssl_recv(void *buf, size_t n)`.
///
/// # Safety
/// `socket` is a live record holding an SSL, and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket8ssl_recvEPvm")
)]
pub unsafe extern "C" fn socket_ssl_recv(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    let (tls, ssl) = (tls(record), record.ssl.cast());
    // SAFETY: the record's TLS group and SSL, and the caller's buffer.
    let result = unsafe { swr_v1_socket_ssl_recv(tls, ssl, buffer, length) };
    // SAFETY: as the caller promises.
    unsafe { ssl_io(socket, result) }
}

/// `ssize_t Socket::ssl_send(const void *buf, size_t n)`: a DTLS socket sends at
/// most `chunk_size` bytes.
///
/// # Safety
/// `socket` is a live record holding an SSL, and `buffer` denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket8ssl_sendEPKvm")
)]
pub unsafe extern "C" fn socket_ssl_send(
    socket: *mut SocketRecord,
    buffer: *const c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    let chunk = record.chunk_size as usize;
    let length = if record.dtls != 0 && chunk != 0 && length > chunk {
        chunk
    } else {
        length
    };
    let (tls, ssl) = (tls(record), record.ssl.cast());
    // SAFETY: the record's TLS group and SSL, and the caller's buffer.
    let result = unsafe { swr_v1_socket_ssl_send(tls, ssl, buffer, length) };
    // SAFETY: as the caller promises.
    unsafe { ssl_io(socket, result) }
}

unsafe extern "C" fn tls_recv(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize {
    // SAFETY: the vector loop lends the record it was given and its buffer.
    unsafe { socket_ssl_recv(socket.cast(), buffer, length) }
}

unsafe extern "C" fn tls_send(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize {
    // SAFETY: the vector loop lends the record it was given and its buffer.
    unsafe { socket_ssl_send(socket.cast(), buffer, length) }
}

/// `ssl_readv`/`ssl_writev` over `IOVector`, whose first member is its owner.
///
/// # Safety
/// `socket` is a live record holding an SSL and `vector` a live `IOVector`.
unsafe fn ssl_vector(socket: *mut SocketRecord, vector: *const c_void, write: bool) -> isize {
    // SAFETY: as the caller promises.
    let owner = unsafe { *vector.cast::<*mut ReactorIov>() };
    // SAFETY: as the caller promises.
    let fd = unsafe { (*socket).fd };
    let io = if write { tls_send } else { tls_recv };
    // SAFETY: the vector's owner and a TLS call valid for the record.
    unsafe { swr_v1_socket_iov_owned(fd, write, owner, Some(io), socket.cast()) }
}

/// `ssize_t Socket::ssl_readv(IOVector *)`.
///
/// # Safety
/// `socket` is a live record holding an SSL and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket9ssl_readvEPNS0_8IOVectorE")
)]
pub unsafe extern "C" fn socket_ssl_readv(
    socket: *mut SocketRecord,
    vector: *const c_void,
) -> isize {
    // SAFETY: as the caller promises.
    unsafe { ssl_vector(socket, vector, false) }
}

/// `ssize_t Socket::ssl_writev(IOVector *)`.
///
/// # Safety
/// `socket` is a live record holding an SSL and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket10ssl_writevEPNS0_8IOVectorE")
)]
pub unsafe extern "C" fn socket_ssl_writev(
    socket: *mut SocketRecord,
    vector: *const c_void,
) -> isize {
    // SAFETY: as the caller promises.
    unsafe { ssl_vector(socket, vector, true) }
}

fn handshake(result: SslHandshakeResult) {
    if result.last_error != 0 {
        log::set_last_error(result.last_error);
    }
}

/// `ReturnCode Socket::ssl_accept()`.
///
/// # Safety
/// `socket` is a live record holding an SSL.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket10ssl_acceptEv")
)]
pub unsafe extern "C" fn socket_ssl_accept(socket: *mut SocketRecord) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let (tls, ssl) = (tls(socket), socket.ssl.cast());
    // SAFETY: the record's TLS group and SSL.
    let result = unsafe { swr_v1_socket_ssl_accept(tls, ssl, socket.dtls != 0) };
    handshake(result);
    if result.log == 1 {
        let mut reason = 0;
        // SAFETY: `reason` is a local.
        let error = unsafe { swr_v1_socket_ssl_error_reason(&raw mut reason) };
        // SAFETY: null or OpenSSL's static text.
        let error = unsafe { text(error) };
        let (host, port) = peer(&socket.info);
        log::warning(
            c"Socket::ssl_accept",
            format_args!("bad SSL client[{host}:{port}], reason={reason}, error_string={error}"),
        );
    } else if result.log == 2 {
        let code = errno();
        // SAFETY: strerror has no memory preconditions.
        let message = unsafe { libc::strerror(code) };
        // SAFETY: strerror's NUL-terminated text.
        let message = unsafe { text(message) };
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

/// `int Socket::ssl_connect()`.
///
/// # Safety
/// `socket` is a live record holding an SSL.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket11ssl_connectEv")
)]
pub unsafe extern "C" fn socket_ssl_connect(socket: *mut SocketRecord) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let (tls, ssl) = (tls(socket), socket.ssl.cast());
    // SAFETY: the record's TLS group and SSL.
    let result = unsafe { swr_v1_socket_ssl_connect(tls, ssl) };
    handshake(result);
    if result.log == 2 {
        // SAFETY: this thread's OpenSSL error queue.
        let error = unsafe { openssl_sys::ERR_get_error() };
        let mut buffer = [0 as c_char; 512];
        // SAFETY: at most the buffer's length, NUL-terminated.
        unsafe { ERR_error_string_n(error, buffer.as_mut_ptr(), buffer.len()) };
        // SAFETY: NUL-terminated by ERR_error_string_n.
        let message = unsafe { text(buffer.as_ptr()) };
        let (host, port) = peer(&socket.info);
        log::notice(format_args!(
            "ssl_connect(fd={}) to server[{host}:{port}] failed. Error: {message}[{}|{}]",
            socket.fd,
            result.ssl_error,
            crate::socket::error_reason(error)
        ));
    }
    result.code
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
        let socket = SocketRecord::new();
        assert!(unsafe { socket_ssl_get_peer_cert_chain(&raw const socket) }.is_null());
    }
}
