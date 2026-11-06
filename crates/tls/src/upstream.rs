// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/socket.cc (Swoole contributors).
//! The TLS methods of `network::Socket` that are whole Rust operations, last
//! error, errno and logs included. With `upstream-symbols` (the hybrid's reactor
//! build) they are defined under their upstream (Itanium) symbols and
//! `socket.cc` no longer defines them.
use crate::{
    SocketTls, SslHandshakeResult, SslIoResult, swr_v1_socket_ssl_accept,
    swr_v1_socket_ssl_check_host, swr_v1_socket_ssl_clear, swr_v1_socket_ssl_connect,
    swr_v1_socket_ssl_error_reason, swr_v1_socket_ssl_peek_reason, swr_v1_socket_ssl_pem,
    swr_v1_socket_ssl_recv, swr_v1_socket_ssl_send, swr_v1_socket_ssl_shutdown,
    swr_v1_socket_ssl_verify,
};
use openssl_sys::{SSL, X509, X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK, stack_st_X509};
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use swoole_rs_corebase::log;
use swoole_rs_reactor::socket_upstream::{
    make_socket_typed, signal_dispatch, socket_cork, socket_set_fd_option, socket_uncork,
};
use swoole_rs_reactor::{
    CLASS_STREAM, CLASS_TCP, OutputHooks, ReactorIov, SYNC_ONCE, SYNC_RECV, SYNC_SEND,
    SYNC_SENDFILE, SocketAddress, SocketFileRequest, SocketRecord, SyncHooks,
    cpp_socket_what_event_want, steady_ms, swr_v1_reactor_poll_events_to, swr_v1_socket_file_check,
    swr_v1_socket_file_cork, swr_v1_socket_file_free, swr_v1_socket_file_length,
    swr_v1_socket_file_new, swr_v1_socket_file_sent, swr_v1_socket_handle_send_owned,
    swr_v1_socket_iov_owned, swr_v1_socket_record_peek, swr_v1_socket_record_recv,
    swr_v1_socket_record_send, swr_v1_socket_recvfrom, swr_v1_socket_sendfile_chunk,
    swr_v1_socket_ssl_sendfile, swr_v1_socket_sync, type_is,
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
/// `swReturnCode`'s `SW_CLOSE`, `SW_SOCKET_RETRY_COUNT`, `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const CLOSE: c_int = 3;
const RETRY_COUNT: u32 = 10;
const EVENT_READ: c_int = 1 << 9;
const EVENT_WRITE: c_int = 1 << 10;

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

/// `readv`/`writev` over `IOVector`, whose first member is its owner, through
/// OpenSSL when the socket holds an SSL.
///
/// # Safety
/// `socket` is a live record and `vector` a live `IOVector`.
unsafe fn vector_io(socket: *mut SocketRecord, vector: *const c_void, write: bool) -> isize {
    // SAFETY: as the caller promises.
    let owner = unsafe { *vector.cast::<*mut ReactorIov>() };
    // SAFETY: as the caller promises.
    let record = unsafe { &*socket };
    let (fd, ssl) = (record.fd, record.ssl);
    let io = (!ssl.is_null()).then_some(if write { tls_send } else { tls_recv });
    // SAFETY: the vector's owner and, with an SSL, a TLS call valid for the record.
    unsafe { swr_v1_socket_iov_owned(fd, write, owner, io, socket.cast()) }
}

/// `ssize_t Socket::readv(IOVector *)`.
///
/// # Safety
/// `socket` is a live record and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket5readvEPNS0_8IOVectorE")
)]
pub unsafe extern "C" fn socket_readv(socket: *mut SocketRecord, vector: *const c_void) -> isize {
    // SAFETY: as the caller promises.
    unsafe { vector_io(socket, vector, false) }
}

/// `ssize_t Socket::writev(IOVector *)`.
///
/// # Safety
/// `socket` is a live record and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket6writevEPNS0_8IOVectorE")
)]
pub unsafe extern "C" fn socket_writev(socket: *mut SocketRecord, vector: *const c_void) -> isize {
    // SAFETY: as the caller promises.
    unsafe { vector_io(socket, vector, true) }
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
    unsafe { vector_io(socket, vector, false) }
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
    unsafe { vector_io(socket, vector, true) }
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

unsafe extern "C" fn tls_peek(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize {
    // SAFETY: the peek lends the record holding an SSL it was given.
    let ssl = unsafe { (*socket.cast::<SocketRecord>()).ssl };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "SSL_peek takes an int length, as upstream passes it"
    )]
    let length = length as c_int;
    // SAFETY: the record's SSL and the caller's buffer.
    unsafe { openssl_sys::SSL_peek(ssl.cast(), buffer, length) as isize }
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
    // SAFETY: as the caller promises; the TLS call and clock are valid for it.
    unsafe { swr_v1_socket_record_recv(socket, buffer, length, flags, tls_recv, steady_ms) }
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
    // SAFETY: as the caller promises; the TLS call and clock are valid for it.
    unsafe { swr_v1_socket_record_send(socket, buffer, length, flags, tls_send, steady_ms) }
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
    // SAFETY: as the caller promises; the TLS call is valid for it.
    unsafe { swr_v1_socket_record_peek(socket, buffer, length, flags, tls_peek) }
}

/// `bool Socket::check_liveness()`: a nonblocking one-byte peek that neither
/// ends the stream nor fails with an error that closes it.
///
/// # Safety
/// `socket` is a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket14check_livenessEv")
)]
pub unsafe extern "C" fn socket_check_liveness(socket: *mut SocketRecord) -> bool {
    let mut byte = 0u8;
    set_errno(0);
    // SAFETY: as the caller promises, and a one-byte buffer.
    let peeked = unsafe { socket_peek(socket, (&raw mut byte).cast(), 1, libc::MSG_DONTWAIT) };
    // SAFETY: as the caller promises; the peek has returned.
    let record = unsafe { &mut *socket };
    !(peeked == 0 || (peeked < 0 && record.catch_error(errno()) == CLOSE))
}

/// One blocking call: the socket and its I/O's operands.
struct Call {
    socket: *mut SocketRecord,
    io: Io,
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

unsafe extern "C" fn sync_prepare(call: *mut c_void) {
    // SAFETY: `sync` lends its live call.
    let socket = unsafe { (*call.cast::<Call>()).socket };
    // SAFETY: the call's live record.
    let record = unsafe { &mut *socket };
    let (nonblock, ssl, tls) = (record.output.nonblock, record.ssl, tls(record));
    if nonblock == 0 {
        // SAFETY: the call's live record.
        unsafe { socket_set_fd_option(socket, 1, -1) };
    }
    if !ssl.is_null() {
        // SAFETY: the record's TLS group.
        unsafe { swr_v1_socket_ssl_clear(tls) };
    }
}

unsafe extern "C" fn sync_events(call: *mut c_void, event: c_int) -> i16 {
    // SAFETY: `sync` lends its live call.
    let socket = unsafe { (*call.cast::<Call>()).socket };
    // SAFETY: the call's live record.
    swr_v1_reactor_poll_events_to(unsafe { cpp_socket_what_event_want(socket, event) })
}

unsafe extern "C" fn sync_io(
    call: *mut c_void,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: `sync` lends its live call.
    let Call { socket, io } = unsafe { &*call.cast::<Call>() };
    // SAFETY: the call's live record.
    let fd = unsafe { (**socket).fd };
    match *io {
        // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
        Io::Send => unsafe { socket_send(*socket, buffer, length, flags) },
        // SAFETY: the loop's buffer of `length` bytes, valid for this I/O.
        Io::Recv => unsafe { socket_recv(*socket, buffer, length, flags) },
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
            unsafe { swr_v1_socket_recvfrom(fd, buffer, length, flags, address, size, RETRY_COUNT) }
        }
        // SAFETY: the call's live record and request.
        Io::Sendfile(request) => unsafe { send_chunk(*socket, request) }.0,
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
    let call = Call { socket, io };
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    let timeout = if write {
        record.timeouts.write
    } else {
        record.timeouts.read
    };
    let hooks = SyncHooks {
        socket: (&raw const call).cast_mut().cast(),
        fd: record.fd,
        dont_restart: record.dont_restart != 0,
        kernel_nobufs: &raw mut record.kernel_nobufs,
        prepare: sync_prepare,
        events: sync_events,
        io: sync_io,
        set_last_error: log::swr_v1_core_set_last_error,
        dispatch: signal_dispatch(),
    };
    let event = if write { EVENT_WRITE } else { EVENT_READ };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sec2msec's long is passed on as an int, as upstream does"
    )]
    let timeout_ms = (timeout * 1000.0) as c_long as c_int;
    let buffer = buffer.cast_mut();
    // SAFETY: the hooks are valid for this call, as is the caller's buffer.
    unsafe {
        swr_v1_socket_sync(
            &raw const hooks,
            kind,
            buffer,
            length,
            flags,
            event,
            timeout_ms,
        )
    }
}

/// `ssize_t Socket::send_sync(const void *, size_t, int)`.
///
/// # Safety
/// `socket` is a live record and `buffer` denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket9send_syncEPKvmi")
)]
pub unsafe extern "C" fn socket_send_sync(
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

/// `ssize_t Socket::recvfrom_sync(char *, size_t, int, sockaddr *, socklen_t *)`.
///
/// # Safety
/// `socket` is a live record, `buffer` denotes `length` writable bytes, and
/// `address` and `size` are as recvfrom(2) takes them.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket13recvfrom_syncEPcmiP8sockaddrPj")
)]
pub unsafe extern "C" fn socket_recvfrom_sync(
    socket: *mut SocketRecord,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    address: *mut libc::sockaddr,
    size: *mut libc::socklen_t,
) -> isize {
    let io = Io::RecvFrom(address, size);
    // SAFETY: as the caller promises.
    unsafe { sync(socket, io, SYNC_ONCE, buffer, length, flags, false) }
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
    // SAFETY: as the caller promises.
    unsafe { socket_recvfrom_sync(socket, buffer, length, flags, storage.cast(), size) }
}

/// `ssize_t Socket::writev_sync(const iovec *, size_t)`.
///
/// # Safety
/// `socket` is a live record and `vectors` denotes `count` readable `iovec`s.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket11writev_syncEPK5iovecm")
)]
pub unsafe extern "C" fn socket_writev_sync(
    socket: *mut SocketRecord,
    vectors: *const libc::iovec,
    count: usize,
) -> isize {
    let io = Io::Writev(vectors, count);
    // SAFETY: as the caller promises.
    unsafe { sync(socket, io, SYNC_ONCE, std::ptr::null(), 0, 0, true) }
}

/// `X509 *Socket::ssl_get_peer_certificate() const`: a reference the caller frees.
///
/// # Safety
/// `socket` is a live record.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket24ssl_get_peer_certificateEv")
)]
pub unsafe extern "C" fn socket_ssl_get_peer_certificate(socket: *const SocketRecord) -> *mut X509 {
    // SAFETY: as the caller promises.
    let ssl = unsafe { (*socket).ssl };
    if ssl.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the record's live SSL.
    unsafe { peer_certificate(ssl.cast()) }
}

/// `bool Socket::ssl_check_host(const char *tls_host_name) const`.
///
/// # Safety
/// `socket` is a live record and `name` NUL-terminated.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7network6Socket14ssl_check_hostEPKc")
)]
pub unsafe extern "C" fn socket_ssl_check_host(
    socket: *const SocketRecord,
    name: *const c_char,
) -> bool {
    // SAFETY: as the caller promises.
    let cert = unsafe { socket_ssl_get_peer_certificate(socket) };
    // SAFETY: the caller's NUL-terminated name.
    let length = unsafe { CStr::from_ptr(name) }.count_bytes();
    // SAFETY: the owned certificate (or null) and the name's bytes.
    match unsafe { swr_v1_socket_ssl_check_host(cert, name, length) } {
        1 => true,
        2 => {
            log::warning(
                c"Socket::ssl_check_host",
                format_args!("X509_check_host(): no match"),
            );
            false
        }
        _ => false,
    }
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
    let cert = unsafe { socket_ssl_get_peer_certificate(socket) };
    if cert.is_null() {
        return -1;
    }
    // SAFETY: the owned certificate and the caller's buffer.
    unsafe { swr_v1_socket_ssl_read_x509(cert, buffer, length, true) }
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
    let buffer = buffer.cast::<NativeString>();
    // SAFETY: the caller's live String.
    let string = unsafe { &*buffer };
    let (str, size) = (string.str, string.size);
    // SAFETY: as the caller promises; the String owns `size` bytes at `str`.
    let n = unsafe { socket_ssl_get_peer_certificate_pem(socket, str, size) };
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    // SAFETY: the caller's live String.
    unsafe { (*buffer).length = n };
    true
}

/// `ssize_t Socket::ssl_sendfile(const File &fp, off_t *offset, size_t size)`.
///
/// # Safety
/// `socket` is a live record holding an SSL, `file` a live `File` (whose first
/// member is its Rust record) and `offset` writable.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZN6swoole7network6Socket12ssl_sendfileERKNS_4FileEPxm")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole7network6Socket12ssl_sendfileERKNS_4FileEPlm")
)]
pub unsafe extern "C" fn socket_ssl_sendfile(
    socket: *mut SocketRecord,
    file: *const c_void,
    offset: *mut libc::off_t,
    size: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let fd = unsafe { file_fd(file) };
    // SAFETY: as the caller promises.
    unsafe { ssl_sendfile(socket, fd, offset, size) }
}

/// A `File`'s descriptor: its first member is its Rust record.
///
/// # Safety
/// `file` is a live `File`.
unsafe fn file_fd(file: *const c_void) -> c_int {
    // SAFETY: as the caller promises.
    let owner = unsafe { *file.cast::<*mut swoole_rs_ossupport::FileRecord>() };
    // SAFETY: the File's live record.
    unsafe { swoole_rs_ossupport::swr_v1_os_file_record_fd(owner, false) }
}

/// `Socket::ssl_sendfile` over a descriptor.
///
/// # Safety
/// `socket` is a live record holding an SSL and `offset` writable.
unsafe fn ssl_sendfile(
    socket: *mut SocketRecord,
    fd: c_int,
    offset: *mut libc::off_t,
    size: usize,
) -> isize {
    // SAFETY: the record's output flag, read while it stays live.
    let nobufs = unsafe { &raw mut (*socket).kernel_nobufs };
    // SAFETY: the caller's offset, and tls_send for this record.
    let result =
        unsafe { swr_v1_socket_ssl_sendfile(fd, offset, size, tls_send, socket.cast(), nobufs) };
    if result.warn == 1 {
        log::sys_warning(c"Socket::ssl_sendfile", format_args!("write() failed"));
    } else if result.warn == 2 {
        log::sys_warning(c"Socket::ssl_sendfile", format_args!("pread() failed"));
    }
    result.value
}

/// `Socket::sendfile` over a descriptor: through OpenSSL when the socket holds an
/// SSL, else the platform's sendfile.
///
/// # Safety
/// `socket` is a live record and `offset` writable.
unsafe fn sendfile(
    socket: *mut SocketRecord,
    fd: c_int,
    offset: *mut libc::off_t,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let record = unsafe { &*socket };
    let (out, ssl) = (record.fd, record.ssl);
    if !ssl.is_null() {
        // SAFETY: as the caller promises.
        return unsafe { ssl_sendfile(socket, fd, offset, length) };
    }
    // `swoole_sendfile` (src/os/sendfile.cc): the platform's sendfile.
    // SAFETY: the socket's and the file's descriptors, and the caller's offset.
    let sent = unsafe { swoole_rs_ossupport::swr_v1_os_sendfile(out, fd, offset, length) };
    if cfg!(not(target_os = "linux")) && sent.warn != 0 {
        log::sys_warning(c"swoole_sendfile", format_args!("sendfile failed"));
    }
    sent.result
}

/// `ssize_t Socket::sendfile(const File &fp, off_t *offset, size_t length)`.
///
/// # Safety
/// `socket` is a live record, `file` a live `File` and `offset` writable.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZN6swoole7network6Socket8sendfileERKNS_4FileEPxm")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole7network6Socket8sendfileERKNS_4FileEPlm")
)]
pub unsafe extern "C" fn socket_sendfile(
    socket: *mut SocketRecord,
    file: *const c_void,
    offset: *mut libc::off_t,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let fd = unsafe { file_fd(file) };
    // SAFETY: as the caller promises.
    unsafe { sendfile(socket, fd, offset, length) }
}

/// One chunk of `request`'s remaining range through `sendfile`, advancing its
/// offset: what `sendfile` returned and the chunk's size.
///
/// # Safety
/// `socket` is a live record and `request` a live request.
unsafe fn send_chunk(socket: *mut SocketRecord, request: *mut SocketFileRequest) -> (isize, usize) {
    // SAFETY: as the caller promises.
    let request = unsafe { &mut *request };
    let fd = request.descriptor();
    let fields = request.fields();
    let chunk = swr_v1_socket_sendfile_chunk(fields.begin, fields.end);
    // SAFETY: the live record, the request's descriptor and its offset.
    (
        unsafe { sendfile(socket, fd, &raw mut fields.begin, chunk) },
        chunk,
    )
}

/// `check_sendfile_parameters`'s origin.
const CHECK_ORIGIN: &CStr = c"network::check_sendfile_parameters";

/// A sendfile request for `filename` from `offset`, checked against `length` with
/// `check_sendfile_parameters`'s warnings; null (the request freed) when it fails.
///
/// # Safety
/// `filename` is NUL-terminated.
unsafe fn file_request(
    filename: *const c_char,
    offset: libc::off_t,
    length: usize,
) -> *mut SocketFileRequest {
    // SAFETY: as the caller promises.
    let request = unsafe { swr_v1_socket_file_new(filename, offset) }.owner;
    assert!(!request.is_null(), "out of memory for a sendfile request");
    // SAFETY: the new request.
    let status = unsafe { swr_v1_socket_file_check(request, length) };
    if status == 0 {
        return request;
    }
    // SAFETY: as the caller promises.
    let name = unsafe { CStr::from_ptr(filename) }.to_string_lossy();
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
            // SAFETY: the new request.
            let begin = unsafe { (*request).fields().begin };
            log::error_log(
                LOG_WARNING,
                INVALID_PARAMS,
                CHECK_ORIGIN,
                format_args!("length[{length}] or offset[{begin}] is invalid"),
            );
        }
    }
    // SAFETY: the request made above, which nothing else holds.
    unsafe { swr_v1_socket_file_free(request) };
    std::ptr::null_mut()
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
    let request = unsafe { file_request(filename, offset, length) };
    if request.is_null() {
        return -1;
    }
    // SAFETY: the live request.
    let total = unsafe { swr_v1_socket_file_length(request) };
    // SAFETY: as the caller promises.
    let corked = total > CORK_MIN_SIZE && unsafe { socket_cork(socket) };
    // SAFETY: the request outlives the loop, and the loop's buffer is unused.
    let rv = unsafe {
        sync(
            socket,
            Io::Sendfile(request),
            SYNC_SENDFILE,
            std::ptr::null(),
            total,
            0,
            true,
        )
    };
    if corked {
        // SAFETY: as the caller promises.
        unsafe { socket_uncork(socket) };
    }
    // SAFETY: the request made above, which the loop no longer holds.
    unsafe { swr_v1_socket_file_free(request) };
    if rv < 0 { -1 } else { 0 }
}

/// A failed Rust queue status. Native `Buffer` raises it as a C++ exception that
/// nothing catches, so the process ends either way; here it aborts.
fn buffer_check(status: i32) {
    if status != 0 {
        std::process::abort();
    }
}

/// A socket's `out_buffer` queue: `Buffer`'s first member is its Rust handle.
///
/// # Safety
/// `socket` is a live record with an output buffer.
unsafe fn out_queue(socket: *mut SocketRecord) -> *mut swoole_rs_buffer::Buffer {
    // SAFETY: as the caller promises.
    let buffer = unsafe { (*socket).output.out_buffer };
    // SAFETY: the live Buffer's first member.
    unsafe { *buffer.cast::<*mut swoole_rs_buffer::Buffer>() }
}

unsafe extern "C" fn output_send(socket: *mut c_void, data: *const c_void, length: usize) -> isize {
    // SAFETY: the output hooks lend their live record and the queue's bytes.
    unsafe { socket_send(socket.cast(), data, length, 0) }
}

unsafe extern "C" fn output_cork(socket: *mut c_void) -> bool {
    // SAFETY: the output hooks lend their live record.
    unsafe { socket_cork(socket.cast()) }
}

unsafe extern "C" fn output_uncork(socket: *mut c_void) {
    // SAFETY: the output hooks lend their live record.
    unsafe { socket_uncork(socket.cast()) };
}

unsafe extern "C" fn output_sent(socket: *mut c_void) {
    // SAFETY: the output hooks lend their live record.
    let record = unsafe { &mut *socket.cast::<SocketRecord>() };
    if !record.output.send_timer.is_null() {
        record.last_sent_time = steady_ms();
    }
}

/// The queued output's native effects (`output_hooks`).
fn output_hooks(socket: *mut SocketRecord) -> OutputHooks {
    OutputHooks {
        socket: socket.cast(),
        kernel_nobufs: socket
            .cast::<u8>()
            .wrapping_add(std::mem::offset_of!(SocketRecord, kernel_nobufs))
            .cast(),
        send: output_send,
        cork: output_cork,
        uncork: output_uncork,
        sent: output_sent,
    }
}

/// `int Socket::handle_send()`: sends the queue's front.
///
/// # Safety
/// `socket` is a live record with a nonempty output buffer.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket11handle_sendEv")
)]
pub unsafe extern "C-unwind" fn socket_handle_send(socket: *mut SocketRecord) -> c_int {
    let hooks = output_hooks(socket);
    // SAFETY: as the caller promises.
    let queue = unsafe { out_queue(socket) };
    // SAFETY: the live record, its queue and the hooks over them.
    let output = unsafe { swr_v1_socket_handle_send_owned(socket, queue, &raw const hooks) };
    buffer_check(output.status);
    if output.result.warn == 1 {
        // SAFETY: as the caller promises.
        let fd = unsafe { (*socket).fd };
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

/// `--fix-halfclose` (M15): `ssl_shutdown` writes the `close_notify` alert, and
/// `handle_sendfile` retries a sendfile interrupted before any progress instead of
/// dropping the file. Native code registers it (`SW_HALFCLOSE_TLS_SHUTDOWN`).
static HALFCLOSE_REPAIRS: AtomicBool = AtomicBool::new(false);

/// Selects M15's repairs (`SW_HALFCLOSE_TLS_SHUTDOWN`).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_set_halfclose_repairs(repaired: bool) {
    HALFCLOSE_REPAIRS.store(repaired, Ordering::Relaxed);
}

/// `int Socket::handle_sendfile()`: sends a chunk of the queue's front file.
///
/// # Safety
/// `socket` is a live record whose output buffer's front is a sendfile request.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket15handle_sendfileEv")
)]
pub unsafe extern "C-unwind" fn socket_handle_sendfile(socket: *mut SocketRecord) -> c_int {
    let hooks = output_hooks(socket);
    // SAFETY: as the caller promises.
    let queue = unsafe { out_queue(socket) };
    let mut front = swoole_rs_buffer::TypedFront::default();
    // SAFETY: the live queue and a local.
    buffer_check(unsafe { swoole_rs_buffer::swr_v1_buffer_typed_front(queue, &raw mut front) });
    let request = front.token.cast::<SocketFileRequest>();
    // SAFETY: the front's live request and the hooks over the live record.
    unsafe { swr_v1_socket_file_cork(request, &raw const hooks) };
    // SAFETY: the record and the front's live request.
    let (rv, requested) = unsafe { send_chunk(socket, request) };
    if rv < 0 && errno() == libc::EINTR && HALFCLOSE_REPAIRS.load(Ordering::Relaxed) {
        return 0;
    }
    // SAFETY: the front's live request, the record and the hooks over them.
    let result =
        unsafe { swr_v1_socket_file_sent(request, socket, &raw const hooks, rv, requested) };
    if result.warn == 2 {
        // SAFETY: the front's live request.
        let request = unsafe { &mut *request };
        let at = request.fields().begin;
        let name = String::from_utf8_lossy(request.path());
        log::sys_warning(
            c"Socket::handle_sendfile",
            format_args!("sendfile({name}, {at}, {}) failed", result.requested),
        );
    }
    if result.pop {
        let mut detached = swoole_rs_buffer::TypedDetached::default();
        // SAFETY: the live queue and a local.
        let status = unsafe { swoole_rs_buffer::swr_v1_buffer_typed_pop(queue, &raw mut detached) };
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

/// Frees a queued sendfile request when the typed queue pops or drains it.
///
/// # Safety
/// `token` is a request from `swr_v1_socket_file_checked`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_dispose(token: *mut c_void) {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_socket_file_free(token.cast()) };
}

/// `sendfile_async`'s request for `filename` from `offset`, checked against `length`
/// with `check_sendfile_parameters`'s warnings; null when it fails.
///
/// # Safety
/// `filename` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_checked(
    filename: *const c_char,
    offset: libc::off_t,
    length: usize,
) -> *mut SocketFileRequest {
    // SAFETY: as the caller promises.
    unsafe { file_request(filename, offset, length) }
}

/// `int Socket::sendfile_async(const char *filename, off_t offset, size_t length)`:
/// the checked request queued behind the output, which is created on first use.
///
/// # Safety
/// `socket` is a live record and `filename` NUL-terminated.
#[cfg(feature = "upstream-symbols")]
#[cfg_attr(
    target_os = "macos",
    unsafe(export_name = "_ZN6swoole7network6Socket14sendfile_asyncEPKcxm")
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(export_name = "_ZN6swoole7network6Socket14sendfile_asyncEPKclm")
)]
pub unsafe extern "C-unwind" fn socket_sendfile_async(
    socket: *mut SocketRecord,
    filename: *const c_char,
    offset: libc::off_t,
    length: usize,
) -> c_int {
    // SAFETY: src/core/buffer.cc's constructor, over operator new's storage for the
    // one-handle Buffer; both throw on failure.
    unsafe extern "C-unwind" {
        #[link_name = "_Znwm"]
        fn operator_new(size: usize) -> *mut c_void;
        #[link_name = "_ZN6swoole6BufferC1Ej"]
        fn buffer_new(buffer: *mut c_void, chunk_size: u32);
    }
    // SAFETY: as the caller promises.
    let request = unsafe { file_request(filename, offset, length) };
    if request.is_null() {
        return -1;
    }
    // SAFETY: as the caller promises.
    if unsafe { (*socket).output.out_buffer }.is_null() {
        // SAFETY: `new Buffer(SW_SEND_BUFFER_SIZE)`: the handle, then front()'s
        // 40-byte BufferChunk view (include/swoole_buffer.h).
        let buffer = unsafe { operator_new(size_of::<*mut c_void>() + 40) };
        // SAFETY: fresh storage for one Buffer.
        unsafe { buffer_new(buffer, 65536) };
        // SAFETY: as the caller promises.
        unsafe { (*socket).output.out_buffer = buffer };
    }
    let dispose = Some(swr_v1_socket_file_dispose as unsafe extern "C" fn(*mut c_void));
    // SAFETY: the socket's output buffer exists now.
    let queue = unsafe { out_queue(socket) };
    // SAFETY: the live output queue now owns the request.
    let status =
        unsafe { swoole_rs_buffer::swr_v1_buffer_typed_push(queue, 1, request.cast(), dispose) };
    if status != 0 {
        // SAFETY: the queue refused the request, which is still ours.
        unsafe { swr_v1_socket_file_free(request) };
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
    let Some((reactor, prepared)) = swoole_rs_reactor::socket_upstream::event_reactor() else {
        // SAFETY: as the caller promises.
        return unsafe { socket_send_sync(socket, data, length, 0) };
    };
    // SAFETY: `swoole_event_write`: the live reactor's `write`, set at construction.
    let Some(write) = (unsafe { (*prepared.fields).write }) else {
        return -1;
    };
    // SAFETY: as the caller promises, on the thread's reactor.
    unsafe { write(reactor, socket, data, length) }
}

/// `bool Socket::ssl_shutdown()`.
///
/// # Safety
/// `socket` is a live record holding an SSL.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket12ssl_shutdownEv")
)]
pub unsafe extern "C" fn socket_ssl_shutdown(socket: *mut SocketRecord) -> bool {
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    let repaired = HALFCLOSE_REPAIRS.load(Ordering::Relaxed);
    let (ssl, quiet, closed) = (
        record.ssl.cast(),
        record.ssl_quiet_shutdown != 0,
        &raw mut record.ssl_closed,
    );
    // SAFETY: the record's TLS group, SSL and closed flag.
    let result = unsafe { swr_v1_socket_ssl_shutdown(tls(record), ssl, closed, quiet, repaired) };
    if result == 2 {
        let mut reason = 0;
        // SAFETY: `reason` is a local.
        let error = unsafe { swr_v1_socket_ssl_error_reason(&raw mut reason) };
        let error = if repaired && error.is_null() {
            "no OpenSSL reason available".into()
        } else {
            // SAFETY: null or OpenSSL's static text.
            unsafe { text(error) }
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
///
/// # Safety
/// `socket` is a live record holding an SSL.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket9ssl_closeEv")
)]
pub unsafe extern "C" fn socket_ssl_close(socket: *mut SocketRecord) {
    // SAFETY: as the caller promises.
    if unsafe { (*socket).ssl_closed } == 0 {
        // SAFETY: as the caller promises.
        unsafe { socket_ssl_shutdown(socket) };
    }
    // SAFETY: as the caller promises.
    let ssl = unsafe { std::mem::replace(&mut (*socket).ssl, std::ptr::null_mut()) };
    // SAFETY: the record's SSL, now released.
    unsafe { openssl_sys::SSL_free(ssl.cast()) };
}

/// `int Socket::ssl_create(SSLContext *ssl_context, int flags)`: `SW_OK` or `SW_ERR`,
/// without setting the last error.
///
/// # Safety
/// `socket` is a live record and `context` a live `SSLContext`, whose first member
/// is its prepared Rust owner.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket10ssl_createEPNS_10SSLContextEi")
)]
pub unsafe extern "C" fn socket_ssl_create(
    socket: *mut SocketRecord,
    context: *const c_void,
    flags: c_int,
) -> c_int {
    // SAFETY: as the caller promises.
    let fields = unsafe { (*context.cast::<crate::owned::OwnedTlsPrepared>()).fields };
    // SAFETY: the context's live fields.
    let context = unsafe { (*fields).context };
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    let (fd, slot) = (record.fd, (&raw mut record.ssl).cast());
    let mut error = 0;
    // SAFETY: the record's TLS group, descriptor and SSL slot, and a live context.
    let status = unsafe {
        crate::swr_v1_socket_ssl_new(
            tls(record),
            context,
            fd,
            flags,
            crate::swr_v1_tls_connection_index(),
            socket.cast(),
            slot,
            &raw mut error,
        )
    };
    match status {
        0 => return 0,
        1 => log::warning(c"Socket::ssl_create", format_args!("SSL_new() failed")),
        2 => {
            // SAFETY: OpenSSL's static string table.
            let reason = unsafe { openssl_sys::ERR_reason_error_string(error) };
            // SAFETY: null or NUL-terminated.
            let reason = unsafe { text(reason) };
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

/// `Socket::bind(host, port)`: the address resolved without DNS, the last error set
/// when it is invalid.
///
/// # Safety
/// `socket` is a live record and `host` NUL-terminated.
unsafe fn bind_host(socket: *mut SocketRecord, host: *const c_char, port: c_int) -> c_int {
    // SAFETY: as the caller promises.
    let kind = unsafe { (*socket).socket_type };
    // SAFETY: an all-zero address is a valid, empty one.
    let mut address: SocketAddress = unsafe { std::mem::zeroed() };
    // SAFETY: as the caller promises.
    let length = unsafe { CStr::from_ptr(host) }.count_bytes();
    // SAFETY: the local address and the caller's host.
    let error = unsafe {
        swoole_rs_clients::address::swr_v1_clients_address_assign(
            (&raw mut address.addr).cast(),
            &raw mut address.len,
            kind,
            host,
            length,
            port,
            None,
        )
    };
    if error != 0 {
        log::set_last_error(error);
        return -1;
    }
    // SAFETY: the live record and the address assigned above.
    unsafe {
        swoole_rs_reactor::swr_v1_socket_bind(socket, (&raw const address.addr).cast(), address.len)
    }
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
    if socket.is_null() {
        log::sys_warning(ORIGIN, format_args!("socket() failed"));
        return socket;
    }
    // SAFETY: the new record.
    let fd = unsafe { (*socket).fd };
    // Only TCP sockets need to rebind addresses held by TIME_WAIT. Enabling reuse for
    // datagram sockets can allow duplicate binds.
    if type_is(kind, CLASS_TCP) {
        let enable: c_int = 1;
        // SAFETY: the new socket's descriptor and a local option.
        let set = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                (&raw const enable).cast(),
                libc::socklen_t::try_from(std::mem::size_of::<c_int>()).unwrap_or(4),
            )
        };
        if set < 0 {
            log::sys_warning(
                ORIGIN,
                format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
            );
        }
    }
    // SAFETY: as the caller promises.
    let host = unsafe { CStr::from_ptr(address) }.to_string_lossy();
    // SAFETY: the new record and the caller's address.
    if unsafe { bind_host(socket, address, port) } < 0 {
        log::sys_warning(
            ORIGIN,
            format_args!("bind({fd}, {host}:{port}, {backlog}) failed"),
        );
        // SAFETY: the new record, which nothing else holds.
        return unsafe { discard(socket) };
    }
    if type_is(kind, CLASS_STREAM) {
        // SAFETY: the new record.
        if unsafe { swoole_rs_reactor::swr_v1_socket_listen(socket, backlog) } < 0 {
            log::sys_warning(
                ORIGIN,
                format_args!("listen({fd}, {host}:{port}, {backlog}) failed"),
            );
            // SAFETY: the new record, which nothing else holds.
            return unsafe { discard(socket) };
        }
    }
    // SAFETY: the new record.
    if unsafe { swoole_rs_reactor::swr_v1_socket_get_name(socket) } < 0 {
        log::sys_warning(ORIGIN, format_args!("getsockname({fd}) failed"));
        // SAFETY: the new record, which nothing else holds.
        return unsafe { discard(socket) };
    }
    socket
}

/// Frees a socket `make_server_socket` gave up on.
///
/// # Safety
/// As `Socket::free`.
unsafe fn discard(socket: *mut SocketRecord) -> *mut SocketRecord {
    // SAFETY: as the caller promises.
    unsafe { swoole_rs_reactor::socket_upstream::socket_free(socket) };
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
        let socket = SocketRecord::new();
        assert!(unsafe { socket_ssl_get_peer_cert_chain(&raw const socket) }.is_null());
    }

    #[test]
    fn peer_certificate_getters_without_ssl() {
        let socket = SocketRecord::new();
        let socket = &raw const socket;
        assert!(unsafe { socket_ssl_get_peer_certificate(socket) }.is_null());
        assert!(!unsafe { socket_ssl_check_host(socket, c"localhost".as_ptr()) });
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
            let context = openssl_sys::SSL_CTX_new(openssl_sys::TLS_method());
            let mut fields = crate::OwnedTlsFields {
                http: 0,
                http_v2: 0,
                prefer_server_ciphers: 0,
                session_tickets: 0,
                stapling: 0,
                stapling_verify: 0,
                disable_tls_host_name: 0,
                grease: 0,
                verify_depth: 0,
                disable_compress: 0,
                verify_peer: 0,
                allow_self_signed: 0,
                protocols: 0,
                create_flag: 0,
                context,
            };
            let prepared = crate::OwnedTlsPrepared {
                owner: std::ptr::null(),
                fields: &raw mut fields,
                strings: [std::ptr::null(); 11],
            };
            let mut fds = [0; 2];
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()),
                0
            );
            let mut socket = SocketRecord::with_fd(fds[0], 0);
            socket.tls.state = 2;
            socket.tls.want_read = 1;
            let created = socket_ssl_create(&raw mut socket, (&raw const prepared).cast(), 2);
            assert_eq!(created, 0);
            assert!(!socket.ssl.is_null());
            assert_eq!(
                (socket.tls.state, socket.tls.want_read),
                (0, 0),
                "state and want flags cleared"
            );
            let index = crate::swr_v1_tls_connection_index();
            let data = openssl_sys::SSL_get_ex_data(socket.ssl.cast(), index);
            assert_eq!(data, (&raw mut socket).cast());
            socket_ssl_close(&raw mut socket);
            assert!(socket.ssl.is_null());
            openssl_sys::SSL_CTX_free(context);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn shutdown_repair_is_registered() {
        swr_v1_socket_set_halfclose_repairs(true);
        assert!(HALFCLOSE_REPAIRS.load(Ordering::Relaxed));
        swr_v1_socket_set_halfclose_repairs(false);
        assert!(!HALFCLOSE_REPAIRS.load(Ordering::Relaxed));
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

    fn read_all(socket: &SocketRecord, length: usize) -> Vec<u8> {
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
            unsafe { swoole_rs_buffer::swr_v1_buffer_typed_create(65536, &raw mut queue) },
            0
        );
        // `Buffer`'s only member is its Rust handle.
        let mut native = Box::new(queue);
        left.output.out_buffer = (&raw mut *native).cast();
        let (path, name) = temp_file("swoole-sendfile-async", b"abcdef");
        let socket = &raw mut left;
        let request = unsafe { swr_v1_socket_file_checked(name.as_ptr(), 1, 0) };
        assert!(!request.is_null());
        let dispose = Some(swr_v1_socket_file_dispose as unsafe extern "C" fn(*mut c_void));
        let queued = unsafe {
            swoole_rs_buffer::swr_v1_buffer_typed_push(queue, 1, request.cast(), dispose)
        };
        assert_eq!(queued, 0);
        assert!(
            unsafe { swr_v1_socket_file_checked(c"/nonexistent/x".as_ptr(), 0, 0) }.is_null(),
            "a file that won't open is refused"
        );
        assert_eq!(unsafe { socket_handle_sendfile(socket) }, 0);
        assert_eq!(read_all(&right, 5), b"bcdef");
        assert_eq!(
            unsafe { swoole_rs_buffer::swr_v1_buffer_append(queue, b"tail".as_ptr(), 4) },
            0
        );
        assert_eq!(unsafe { socket_handle_send(socket) }, 0);
        assert_eq!(read_all(&right, 4), b"tail");
        let (mut length, mut count) = (0, 0);
        unsafe { swoole_rs_buffer::swr_v1_buffer_stats(queue, &raw mut length, &raw mut count) };
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
        unsafe { swoole_rs_reactor::socket_upstream::socket_free(socket) };
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
        assert!(unsafe { socket_check_liveness(&raw mut right) });
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
        assert!(!unsafe { socket_check_liveness(&raw mut right) });
        unsafe { libc::close(fds[1]) };
    }
}
