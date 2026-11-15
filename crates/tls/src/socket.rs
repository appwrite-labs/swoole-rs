//! `network::Socket`'s TLS operations over the [`Ssl`] handle: creating the SSL,
//! handshake steps, reads and writes, host and verify checks, PEM export and error
//! reasons, and the socket's TLS state group they set. `upstream` defines the
//! `Socket` methods over them, errno, the last error and logs included.
use crate::SslState;
use crate::handle::Ssl;
use crate::handle::{clear_errors, peek_error, pop_error, reason_string};
use openssl_sys::{
    SSL_ERROR_SSL, SSL_ERROR_SYSCALL, SSL_ERROR_WANT_READ, SSL_ERROR_WANT_WRITE,
    SSL_ERROR_ZERO_RETURN, SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER, SSL_RECEIVED_SHUTDOWN,
    SSL_SENT_SHUTDOWN, X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK,
};
use std::ffi::CStr;
use std::os::raw::{c_int, c_long, c_ulong};
use swoole_rs_corebase::constants::ReturnCode;
use swoole_rs_corebase::log;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::SocketFields;

/// `SW_SSL_SERVER` and `SW_SSL_CLIENT`.
const SERVER: c_int = 1;
const CLIENT: c_int = 2;

const BAD_CLIENT: c_int = ErrorCode::SW_ERROR_SSL_BAD_CLIENT as c_int;
const RESET: c_int = ErrorCode::SW_ERROR_SSL_RESET as c_int;
const HANDSHAKE_FAILED: c_int = ErrorCode::SW_ERROR_SSL_HANDSHAKE_FAILED as c_int;
use SslState::{SW_SSL_STATE_READY as STATE_READY, SW_SSL_STATE_WAIT_STREAM as STATE_WAIT_STREAM};

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `network::Socket`'s TLS state group (`ssl_state`, `ssl_want_read`,
/// `ssl_want_write`): the record's own field.
pub use swoole_rs_reactor::SocketTlsState as SocketTls;

fn want(tls: &mut SocketTls, read: bool, write: bool) {
    tls.want_read = read;
    tls.want_write = write;
}

/// `Socket::ssl_clear_error`: clears this thread's OpenSSL error queue and the want flags.
pub fn swr_v1_socket_ssl_clear(tls: &mut SocketTls) {
    clear_errors();
    want(tls, false, false);
}

/// A TLS read or write: what `ssl_recv`/`ssl_send` return, the errno to set when
/// nonzero, and whether to log a protocol error first.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SslIoResult {
    pub value: isize,
    pub error: c_int,
    pub protocol_error: bool,
}

fn classify(tls: &mut SocketTls, ssl: Ssl, n: c_int, write: bool) -> SslIoResult {
    want(tls, false, false);
    let mut result = SslIoResult {
        value: n as isize,
        ..SslIoResult::default()
    };
    if n >= 0 {
        return result;
    }
    match ssl.error(n) {
        SSL_ERROR_WANT_READ => {
            want(tls, true, false);
            result.error = libc::EAGAIN;
        }
        SSL_ERROR_WANT_WRITE => {
            want(tls, false, true);
            result.error = libc::EAGAIN;
        }
        SSL_ERROR_SYSCALL if write => result.error = RESET,
        SSL_ERROR_SYSCALL => {
            if errno() == 0 {
                result.value = 0;
            }
        }
        SSL_ERROR_SSL => {
            result.protocol_error = true;
            result.error = BAD_CLIENT;
        }
        _ => {}
    }
    result
}

/// `Socket::ssl_recv`: `SSL_read` into `buffer`.
pub fn swr_v1_socket_ssl_recv(
    tls: &mut SocketTls,
    ssl: Ssl,
    buffer: &mut [impl ReadSlot],
) -> SslIoResult {
    clear_errors();
    let n = ssl.read(buffer);
    classify(tls, ssl, n, false)
}

/// `Socket::ssl_send`: `SSL_write` of `data` (native code applies the DTLS chunk
/// limit first).
pub fn swr_v1_socket_ssl_send(tls: &mut SocketTls, ssl: Ssl, data: &[u8]) -> SslIoResult {
    clear_errors();
    let n = ssl.write(data);
    classify(tls, ssl, n, true)
}

/// What a failed handshake step logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeLog {
    Nothing,
    /// A bad client (accept).
    BadClient,
    /// A failed handshake, with its `SSL_get_error` code.
    Failed(c_int),
}

/// What a `Socket::ssl_accept` step returns: `swReturnCode`'s `SW_READY`, `SW_WAIT`
/// and `SW_ERROR`, at their numbers.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accept {
    Ready = ReturnCode::SW_READY as i32,
    Wait = ReturnCode::SW_WAIT as i32,
    Error = ReturnCode::SW_ERROR as i32,
}

/// A handshake step for `Socket::ssl_accept`/`ssl_connect`, which also sets the
/// socket's want flags and, once it changes, `ssl_state`. The caller sets the last
/// error when `last_error` is nonzero, then logs. `code` is what the method returns:
/// an [`Accept`], or for `ssl_connect` whether it is `SW_OK`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SslHandshakeResult<C> {
    pub code: C,
    pub last_error: c_int,
    pub log: HandshakeLog,
}

impl<C> SslHandshakeResult<C> {
    const fn returning(code: C) -> Self {
        Self {
            code,
            last_error: 0,
            log: HandshakeLog::Nothing,
        }
    }
}

/// `Socket::ssl_accept`: one `SSL_accept` step. A DTLS socket waits to read on a
/// syscall error that left errno 0.
pub fn swr_v1_socket_ssl_accept(
    tls: &mut SocketTls,
    ssl: Ssl,
    dtls: bool,
) -> SslHandshakeResult<Accept> {
    want(tls, false, false);
    clear_errors();
    let n = ssl.accept();
    if n == 1 {
        tls.state = STATE_READY;
        return SslHandshakeResult::returning(Accept::Ready);
    }
    if n == 0 {
        return SslHandshakeResult::returning(Accept::Error);
    }
    let error = ssl.error(n);
    let mut result = SslHandshakeResult::returning(Accept::Error);
    match error {
        SSL_ERROR_WANT_READ => {
            want(tls, true, false);
            result.code = Accept::Wait;
        }
        SSL_ERROR_WANT_WRITE => {
            want(tls, false, true);
            result.code = Accept::Wait;
        }
        SSL_ERROR_SSL => result.log = HandshakeLog::BadClient,
        SSL_ERROR_SYSCALL => {
            if dtls && errno() == 0 {
                want(tls, true, false);
                result.code = Accept::Wait;
            }
        }
        _ => result.log = HandshakeLog::Failed(error),
    }
    result
}

/// `Socket::ssl_connect`: one `SSL_connect` step.
pub fn swr_v1_socket_ssl_connect(tls: &mut SocketTls, ssl: Ssl) -> SslHandshakeResult<bool> {
    want(tls, false, false);
    clear_errors();
    let n = ssl.connect();
    if n == 1 {
        tls.state = STATE_READY;
        return SslHandshakeResult::returning(true);
    }
    let error = ssl.error(n);
    let mut result = SslHandshakeResult::returning(false);
    match error {
        SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => {
            want(
                tls,
                error == SSL_ERROR_WANT_READ,
                error == SSL_ERROR_WANT_WRITE,
            );
            tls.state = STATE_WAIT_STREAM;
            result.code = true;
            return result;
        }
        SSL_ERROR_ZERO_RETURN => {
            result.last_error = RESET;
            return result;
        }
        SSL_ERROR_SYSCALL if n != 0 => {
            result.last_error = errno();
            return result;
        }
        SSL_ERROR_SYSCALL => {}
        _ => result.last_error = HANDSHAKE_FAILED,
    }
    result.log = HandshakeLog::Failed(error);
    result
}

/// `Socket::ssl_verify`: the peer's verify result if it rejects the peer, else
/// `X509_V_OK`. A self-signed leaf passes when `allow_self_signed`.
#[must_use]
pub fn swr_v1_socket_ssl_verify(ssl: Ssl, allow_self_signed: bool) -> c_long {
    let result = ssl.verify_result();
    if allow_self_signed && result == c_long::from(X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT) {
        return c_long::from(X509_V_OK);
    }
    result
}

/// OpenSSL 3's `ERR_GET_REASON`: a system error's reason is its errno.
/// openssl-sys's port returns `ERR_LIB_SYS` (2) for those instead.
#[must_use]
pub fn error_reason(error: c_ulong) -> c_int {
    const SYSTEM_FLAG: c_ulong = 1 << 31;
    if error & SYSTEM_FLAG == 0 {
        openssl_sys::ERR_GET_REASON(error)
    } else {
        c_int::try_from(error & (SYSTEM_FLAG - 1)).unwrap_or(c_int::MAX)
    }
}

/// `Socket::ssl_get_error_reason`: pops the oldest queued OpenSSL error and
/// returns its reason code and string.
#[must_use]
pub fn swr_v1_socket_ssl_error_reason() -> (c_int, Option<&'static CStr>) {
    let error = pop_error();
    (error_reason(error), reason_string(error))
}

/// `Socket::ssl_catch_error`'s reason: the oldest queued error's reason code, left queued.
#[must_use]
pub fn swr_v1_socket_ssl_peek_reason() -> c_int {
    error_reason(peek_error())
}

/// `SSL_shutdown` and `SSL_get_error`, which tests replace to inject failures.
trait ShutdownCalls {
    fn shutdown(&mut self, ssl: Ssl) -> c_int;
    fn error(&mut self, ssl: Ssl, result: c_int) -> c_int;
}

struct OpenSsl;

impl ShutdownCalls for OpenSsl {
    fn shutdown(&mut self, ssl: Ssl) -> c_int {
        ssl.shutdown()
    }
    fn error(&mut self, ssl: Ssl, result: c_int) -> c_int {
        ssl.error(result)
    }
}

/// `Socket::ssl_shutdown`'s results: false, true, or false after native code warns
/// with `ssl_get_error_reason`.
const SHUTDOWN_FALSE: c_int = 0;
const SHUTDOWN_TRUE: c_int = 1;
const SHUTDOWN_WARN: c_int = 2;

fn shutdown(
    tls: &mut SocketTls,
    ssl: Ssl,
    closed: &mut bool,
    quiet: bool,
    repaired: bool,
    calls: &mut impl ShutdownCalls,
) -> c_int {
    if *closed || ssl.in_init() {
        return SHUTDOWN_FALSE;
    }
    // If the peer closed first, shut down quietly: sending data then makes the peer
    // reply with an RST segment.
    if quiet {
        ssl.set_quiet_shutdown();
    }
    let result = if repaired {
        clear_errors();
        want(tls, false, false);
        let n = calls.shutdown(ssl);
        let error = if n < 0 { calls.error(ssl, n) } else { 0 };
        want(
            tls,
            error == SSL_ERROR_WANT_READ,
            error == SSL_ERROR_WANT_WRITE,
        );
        *closed = !tls.want_read && !tls.want_write;
        (n, error)
    } else {
        // Upstream marks both directions shut first, so OpenSSL writes no alert.
        ssl.set_shutdown_mode(ssl.shutdown_mode() | SSL_RECEIVED_SHUTDOWN | SSL_SENT_SHUTDOWN);
        let n = calls.shutdown(ssl);
        *closed = true;
        let error = if n != 1 && peek_error() != 0 {
            calls.error(ssl, n)
        } else {
            0
        };
        (n, error)
    };
    match result {
        (1, _) | (_, 0 | SSL_ERROR_ZERO_RETURN) => SHUTDOWN_TRUE,
        _ => SHUTDOWN_WARN,
    }
}

/// `Socket::ssl_shutdown`: one `SSL_shutdown` once the handshake is done, quiet when
/// `quiet` is set. Returns 1 for true, 0 for false and 2 for false after native code
/// warns with `ssl_get_error_reason`.
///
/// Upstream marks the shutdown sent and received first, so no `close_notify` is
/// written, and closes the socket (`closed`, its `ssl_closed_`) whatever the result.
/// `repaired` (`--fix-halfclose`, M15) writes the alert: a pending read or write
/// leaves the socket open with its want flag set, to retry, and anything else closes it.
pub fn swr_v1_socket_ssl_shutdown(
    tls: &mut SocketTls,
    ssl: Ssl,
    closed: &mut bool,
    quiet: bool,
    repaired: bool,
) -> c_int {
    shutdown(tls, ssl, closed, quiet, repaired, &mut OpenSsl)
}

/// The `Socket::ssl_create` step that made no SSL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SslNewError {
    New,
    /// `SSL_set_fd`, with the peeked error.
    SetFd(c_ulong),
    ExData,
}

impl SslNewError {
    /// `Socket::ssl_create`'s warning.
    pub fn warn(self) {
        let origin = c"Socket::ssl_create";
        match self {
            Self::New => log::warning(origin, format_args!("SSL_new() failed")),
            Self::SetFd(error) => log::warning(
                origin,
                format_args!(
                    "SSL_set_fd() failed. Error: {}[{error}]",
                    crate::reason_text(error)
                ),
            ),
            Self::ExData => log::warning(origin, format_args!("SSL_set_ex_data() failed")),
        }
    }
}

/// `Socket::ssl_create` on `socket`'s record: a new SSL from `context` on its
/// descriptor, in connect state for `SW_SSL_CLIENT` or accept state for
/// `SW_SSL_SERVER`, carrying the record as its ex data at the connection's index
/// (which the info callback reads), after clearing the want flags. It accepts a write
/// retried from another buffer: `Socket::ssl_sendfile` reads each try into a new one,
/// where upstream's stack buffer happens to keep its address. The record owns the SSL
/// once made (null when `SSL_new` fails), and its state is cleared on success.
/// Without a context `SSL_new` fails.
///
/// # Errors
/// The step that failed; the caller warns.
pub fn socket_ssl_new(
    socket: swoole_rs_reactor::Socket<'_>,
    context: Option<&crate::SslCtx>,
    flags: c_int,
) -> Result<(), SslNewError> {
    socket.edit(|record| want(&mut record.tls, false, false));
    clear_errors();
    let Some(mut made) = crate::handle::SSL_new(context) else {
        // The record's SSL becomes the null `SSL_new` returned, as upstream's.
        socket.edit(SocketFields::take_ssl);
        return Err(SslNewError::New);
    };
    made.get().set_mode(SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER);
    let made_up = if made.set_fd(socket.fd()) {
        if flags & CLIENT != 0 {
            made.get().set_connect_state();
        } else if flags & SERVER != 0 {
            made.get().set_accept_state();
        }
        // The record that will own the SSL, and so outlives it: the one ex data the
        // info callback reads at the connection's index, which only this sets.
        let (index, this) = (crate::swr_v1_tls_connection_index(), socket.as_ptr().cast());
        if crate::handle::SSL_set_ex_data(made.get(), index, this) == 0 {
            Err(SslNewError::ExData)
        } else {
            Ok(())
        }
    } else {
        Err(SslNewError::SetFd(peek_error()))
    };
    let made = made.into_raw();
    socket.edit(|record| {
        // SAFETY: the new SSL, which nothing else holds: the record owns it from now
        // on and frees it only in `ssl_close`.
        unsafe { record.set_ssl(made.cast()) };
        if made_up.is_ok() {
            record.tls.state = SslState::SW_SSL_STATE_HANDSHAKE;
        }
    });
    made_up
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::{Cert, CertRef};
    use openssl_sys::*;
    use std::os::raw::c_int;
    use std::ptr;

    /// A test's live SSL.
    fn handle(ssl: *mut SSL) -> Ssl<'static> {
        unsafe { Ssl::new(ssl) }.unwrap()
    }

    /// A client SSL over memory BIOs, before its handshake; returns it and its read BIO.
    unsafe fn client() -> (*mut SSL, *mut BIO) {
        unsafe {
            let context = SSL_CTX_new(TLS_method());
            let ssl = SSL_new(context);
            SSL_CTX_free(context);
            let (read, write) = (BIO_new(BIO_s_mem()), BIO_new(BIO_s_mem()));
            SSL_set_bio(ssl, read, write);
            SSL_set_connect_state(ssl);
            (ssl, read)
        }
    }

    #[test]
    fn want_read_before_the_handshake() {
        unsafe {
            let (ssl, _) = client();
            let mut tls = SocketTls::default();
            let mut buffer = [0_u8; 16];
            let read = swr_v1_socket_ssl_recv(&mut tls, handle(ssl), &mut buffer);
            assert_eq!(
                read,
                SslIoResult {
                    value: -1,
                    error: libc::EAGAIN,
                    ..Default::default()
                }
            );
            assert_eq!((tls.want_read, tls.want_write), (true, false));
            tls.want_write = true;
            let sent = swr_v1_socket_ssl_send(&mut tls, handle(ssl), &buffer);
            assert_eq!(sent, read, "a write also waits for the server's hello");
            assert_eq!(
                (tls.want_read, tls.want_write),
                (true, false),
                "and replaces the flags"
            );
            swr_v1_socket_ssl_clear(&mut tls);
            assert_eq!(
                (tls.want_read, tls.want_write),
                (false, false),
                "nothing pending"
            );
            SSL_free(ssl);
        }
    }

    #[test]
    fn handshake_steps() {
        unsafe {
            let (ssl, read) = client();
            let mut tls = SocketTls::default();
            assert_eq!(
                swr_v1_socket_ssl_connect(&mut tls, handle(ssl)),
                SslHandshakeResult::returning(true),
                "the client hello is out"
            );
            assert_eq!(
                tls,
                SocketTls {
                    state: STATE_WAIT_STREAM,
                    want_read: true,
                    want_write: false
                },
                "waiting for the server"
            );
            let garbage = [
                0x16_u8, 0x03, 0x03, 0x00, 0x05, 0xff, 0xff, 0xff, 0xff, 0xff,
            ];
            BIO_write(
                read,
                garbage.as_ptr().cast(),
                c_int::try_from(garbage.len()).unwrap(),
            );
            let failed = swr_v1_socket_ssl_connect(&mut tls, handle(ssl));
            assert_eq!(
                (failed.code, failed.last_error, failed.log),
                (false, HANDSHAKE_FAILED, HandshakeLog::Failed(SSL_ERROR_SSL))
            );
            assert_eq!(
                (tls.state, tls.want_read),
                (STATE_WAIT_STREAM, false),
                "a failure keeps the state"
            );
            SSL_free(ssl);

            let context = SSL_CTX_new(TLS_method());
            let server = SSL_new(context);
            SSL_CTX_free(context);
            SSL_set_bio(server, BIO_new(BIO_s_mem()), BIO_new(BIO_s_mem()));
            let mut tls = SocketTls::default();
            assert_eq!(
                swr_v1_socket_ssl_accept(&mut tls, handle(server), false),
                SslHandshakeResult::returning(Accept::Wait),
                "no client hello yet"
            );
            assert_eq!(
                (tls.state as u32, tls.want_read, tls.want_write),
                (0, true, false)
            );
            SSL_free(server);
        }
    }

    unsafe extern "C" {
        /// Not bound by openssl-sys; sets what a handshake would have verified.
        fn SSL_set_verify_result(ssl: *mut SSL, result: c_long);
    }

    #[test]
    fn check_host_and_verify() {
        unsafe {
            let check = |cert, name: &str| {
                let cert = Cert(std::ptr::NonNull::new(cert).unwrap());
                if cert.get().matches_host(name.as_bytes()) {
                    1
                } else {
                    2
                }
            };
            // The name as a subjectAltName dNSName: OpenSSL 4 no longer falls back to
            // the subject CN when the flags are 0.
            let named = |host: &str| {
                let cert = X509_new();
                let value = std::ffi::CString::new(format!("DNS:{host}")).unwrap();
                let ext = X509V3_EXT_nconf_nid(
                    ptr::null_mut(),
                    ptr::null_mut(),
                    NID_subject_alt_name,
                    value.as_ptr(),
                );
                assert!(!ext.is_null());
                assert_eq!(X509_add_ext(cert, ext, -1), 1);
                X509_EXTENSION_free(ext);
                cert
            };
            assert_eq!(check(named("example.com"), "EXAMPLE.com"), 1);
            assert_eq!(
                check(named("*.example.com"), "a.example.com"),
                1,
                "wildcard"
            );
            assert_eq!(check(named("example.com"), "example.org"), 2);

            let (ssl, _) = client();
            assert_eq!(
                swr_v1_socket_ssl_verify(handle(ssl), false),
                c_long::from(X509_V_OK)
            );
            SSL_set_verify_result(ssl, c_long::from(X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT));
            assert_eq!(
                swr_v1_socket_ssl_verify(handle(ssl), true),
                c_long::from(X509_V_OK)
            );
            assert_eq!(
                swr_v1_socket_ssl_verify(handle(ssl), false),
                c_long::from(X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT)
            );
            SSL_set_verify_result(ssl, c_long::from(X509_V_ERR_CERT_HAS_EXPIRED));
            assert_eq!(
                swr_v1_socket_ssl_verify(handle(ssl), true),
                c_long::from(X509_V_ERR_CERT_HAS_EXPIRED)
            );
            SSL_free(ssl);
        }
    }

    #[test]
    fn pem_reason_and_new() {
        unsafe {
            let mut pem = [0_u8; 4096];
            // `_ssl_read_x509_file` before its warnings: the PEM, then the reference
            // released when `consume`.
            let pem_of = |cert: *mut X509, buffer: &mut [u8], consume: bool| {
                let written =
                    CertRef::new(cert).map_or(Err(crate::PemError::Write), |cert| cert.pem(buffer));
                drop(consume.then(|| std::ptr::NonNull::new(cert).map(Cert)));
                written
            };
            let empty = X509_new();
            assert_eq!(
                pem_of(empty, &mut pem, true),
                Err(crate::PemError::Write),
                "nothing to encode"
            );
            ERR_clear_error();
            // A self-signed Ed25519 certificate.
            let keygen = EVP_PKEY_CTX_new_id(EVP_PKEY_ED25519, std::ptr::null_mut());
            let mut key = std::ptr::null_mut();
            assert_eq!(EVP_PKEY_keygen_init(keygen), 1);
            assert_eq!(EVP_PKEY_keygen(keygen, &raw mut key), 1);
            EVP_PKEY_CTX_free(keygen);
            let cert = X509_new();
            assert_eq!(X509_set_pubkey(cert, key), 1);
            assert!(X509_sign(cert, key, std::ptr::null()) > 0);
            EVP_PKEY_free(key);
            let written = pem_of(cert, &mut pem, false);
            assert!(written.is_ok_and(|n| n > 0));
            assert!(pem.starts_with(b"-----BEGIN CERTIFICATE-----"));
            let mut short = [0_u8; 10];
            assert_eq!(
                pem_of(cert, &mut short, true),
                Ok(10),
                "bounded by the buffer"
            );

            let context = SSL_CTX_new(TLS_method());
            crate::swr_v1_tls_init().unwrap();
            let index = crate::swr_v1_tls_connection_index();
            let mut record = swoole_rs_reactor::SocketRecord::with_fd(3, 0);
            record.tls = SocketTls {
                state: STATE_READY,
                want_read: true,
                want_write: true,
            };
            let socket = swoole_rs_reactor::Socket::from_ptr(&raw mut record).unwrap();
            let made = socket_ssl_new(socket, context.cast::<crate::SslCtx>().as_ref(), CLIENT);
            assert_eq!(made, Ok(()));
            assert_eq!(
                record.tls,
                SocketTls::default(),
                "a fresh SSL clears the state group"
            );
            let made = record.take_ssl().cast::<SSL>();
            assert_eq!(SSL_is_server(made), 0, "connect state");
            assert_eq!(SSL_get_ex_data(made, index), (&raw mut record).cast());
            SSL_free(made);
            let socket = swoole_rs_reactor::Socket::from_ptr(&raw mut record).unwrap();
            assert_eq!(
                socket_ssl_new(socket, context.cast::<crate::SslCtx>().as_ref(), SERVER),
                Ok(())
            );
            let ssl = record.take_ssl();
            assert_eq!(SSL_is_server(ssl.cast()), 1, "accept state");
            SSL_free(ssl.cast());
            SSL_CTX_free(context);

            assert_eq!(swr_v1_socket_ssl_peek_reason(), 0, "empty queue");
            assert_eq!(swr_v1_socket_ssl_error_reason(), (0, None));
        }
    }

    #[test]
    fn garbage_is_a_protocol_error() {
        unsafe {
            let (ssl, read) = client();
            let mut tls = SocketTls::default();
            let mut buffer = [0_u8; 16];
            swr_v1_socket_ssl_recv(&mut tls, handle(ssl), &mut buffer);
            let garbage = [
                0x16_u8, 0x03, 0x03, 0x00, 0x05, 0xff, 0xff, 0xff, 0xff, 0xff,
            ];
            BIO_write(
                read,
                garbage.as_ptr().cast(),
                c_int::try_from(garbage.len()).unwrap(),
            );
            let result = swr_v1_socket_ssl_recv(&mut tls, handle(ssl), &mut buffer);
            assert_eq!(
                result,
                SslIoResult {
                    value: -1,
                    error: BAD_CLIENT,
                    protocol_error: true,
                }
            );
            assert_ne!(ERR_get_error(), 0, "the queue is left for ssl_catch_error");
            SSL_free(ssl);
        }
    }
}

/// `Socket::ssl_shutdown` and `ssl_close`, and a retried write, over real TLS 1.2 peers
/// (memory BIO pairs and a nonblocking socketpair), with `SSL_shutdown`/`SSL_get_error`
/// observed and, for five negative results, injected. The repaired (`--fix-halfclose`) cases were the
/// halfclose-tls-native gate's, which this replaces.
#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use openssl_sys::*;
    use std::os::raw::{c_int, c_long};
    use std::ptr;

    // libssl/libcrypto's own declarations; openssl-sys binds none of them.
    unsafe extern "C" {
        fn BIO_new_bio_pair(
            left: *mut *mut BIO,
            left_size: usize,
            right: *mut *mut BIO,
            right_size: usize,
        ) -> c_int;
        fn BIO_test_flags(bio: *const BIO, flags: c_int) -> c_int;
    }

    /// `BIO_CTRL_PENDING`, behind the `BIO_pending` macro.
    const BIO_CTRL_PENDING: c_int = 10;

    /// `BIO_FLAGS_SHOULD_RETRY`, behind the `BIO_should_retry` macro.
    const BIO_FLAGS_SHOULD_RETRY: c_int = 0x08;

    /// Counts and records the real calls, or fails them with `injected`.
    #[derive(Default)]
    struct Observed {
        injected: c_int,
        shutdown_calls: usize,
        last_shutdown_errno: c_int,
        last_error_class: c_int,
        last_error_queue: std::os::raw::c_ulong,
    }

    impl ShutdownCalls for Observed {
        fn shutdown(&mut self, ssl: Ssl) -> c_int {
            self.shutdown_calls += 1;
            let result = if self.injected == 0 {
                unsafe { SSL_shutdown(ssl.as_ptr()) }
            } else {
                -1
            };
            self.last_shutdown_errno = errno();
            result
        }
        fn error(&mut self, ssl: Ssl, result: c_int) -> c_int {
            self.last_error_class = if self.injected == 0 {
                unsafe { SSL_get_error(ssl.as_ptr(), result) }
            } else {
                self.injected
            };
            self.last_error_queue = crate::peek_error();
            self.last_error_class
        }
    }

    /// `network::Socket`'s TLS fields and the native caller: its warning formats
    /// `ssl_get_error_reason`'s string, or a fallback when OpenSSL has none.
    struct Socket {
        ssl: *mut SSL,
        tls: SocketTls,
        closed: bool,
        quiet: bool,
        repaired: bool,
        calls: Observed,
        warnings: usize,
        null_warnings: usize,
    }

    impl Socket {
        fn new(ssl: *mut SSL) -> Self {
            Self {
                ssl,
                tls: SocketTls::default(),
                closed: false,
                quiet: false,
                repaired: true,
                calls: Observed::default(),
                warnings: 0,
                null_warnings: 0,
            }
        }

        fn ssl_shutdown(&mut self) -> bool {
            let result = shutdown(
                &mut self.tls,
                unsafe { Ssl::new(self.ssl) }.unwrap(),
                &mut self.closed,
                self.quiet,
                self.repaired,
                &mut self.calls,
            );
            if result == SHUTDOWN_WARN {
                let (_, reason_string) = swr_v1_socket_ssl_error_reason();
                let message = reason_string
                    .unwrap_or(c"no OpenSSL reason available")
                    .as_ptr();
                self.warnings += 1;
                self.null_warnings += usize::from(message.is_null());
            }
            result == SHUTDOWN_TRUE
        }

        fn ssl_close(&mut self) {
            if !self.closed {
                self.ssl_shutdown();
            }
            unsafe { SSL_free(self.ssl) };
            self.ssl = ptr::null_mut();
        }
    }

    /// A server certificate: self-signed Ed25519.
    unsafe fn certificate(context: *mut SSL_CTX) {
        unsafe {
            let keygen = EVP_PKEY_CTX_new_id(EVP_PKEY_ED25519, ptr::null_mut());
            let mut key = ptr::null_mut();
            assert_eq!(EVP_PKEY_keygen_init(keygen), 1);
            assert_eq!(EVP_PKEY_keygen(keygen, &raw mut key), 1);
            EVP_PKEY_CTX_free(keygen);
            let cert = X509_new();
            assert_eq!(X509_set_version(cert, 2), 1);
            assert_eq!(ASN1_INTEGER_set(X509_get_serialNumber(cert), 1), 1);
            assert!(!X509_gmtime_adj(X509_getm_notBefore(cert), 0).is_null());
            assert!(!X509_gmtime_adj(X509_getm_notAfter(cert), 3600).is_null());
            assert_eq!(X509_set_pubkey(cert, key), 1);
            assert!(X509_sign(cert, key, ptr::null()) > 0);
            assert_eq!(SSL_CTX_use_certificate(context, cert), 1);
            assert_eq!(SSL_CTX_use_PrivateKey(context, key), 1);
            X509_free(cert);
            EVP_PKEY_free(key);
        }
    }

    /// A server and client after a completed TLS 1.2 handshake.
    struct Pair {
        server_ctx: *mut SSL_CTX,
        client_ctx: *mut SSL_CTX,
        server: *mut SSL,
        client: *mut SSL,
        fds: [c_int; 2],
    }

    impl Pair {
        fn new(native_socket: bool) -> Self {
            unsafe {
                let server_ctx = SSL_CTX_new(TLS_server_method());
                let client_ctx = SSL_CTX_new(TLS_client_method());
                for context in [server_ctx, client_ctx] {
                    assert_eq!(SSL_CTX_set_max_proto_version(context, TLS1_2_VERSION), 1);
                }
                SSL_CTX_set_verify(client_ctx, SSL_VERIFY_NONE, None);
                certificate(server_ctx);
                let client = SSL_new(client_ctx);
                let mut fds = [-1; 2];
                let server = if native_socket {
                    assert_eq!(
                        libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()),
                        0
                    );
                    for fd in fds {
                        assert_eq!(libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK), 0);
                    }
                    // `SSL_set_fd`: a socket BIO that leaves the descriptor open.
                    let bio = BIO_new_socket(fds[1], 0);
                    SSL_set_bio(client, bio, bio);
                    // The server end as `Socket::ssl_create` makes it.
                    let mut record = swoole_rs_reactor::SocketRecord::with_fd(fds[0], 0);
                    let socket = swoole_rs_reactor::Socket::from_ptr(&raw mut record).unwrap();
                    let made =
                        socket_ssl_new(socket, server_ctx.cast::<crate::SslCtx>().as_ref(), SERVER);
                    assert_eq!(made, Ok(()));
                    record.take_ssl().cast()
                } else {
                    let server = SSL_new(server_ctx);
                    let (mut left, mut right) = (ptr::null_mut(), ptr::null_mut());
                    assert_eq!(BIO_new_bio_pair(&raw mut left, 0, &raw mut right, 0), 1);
                    SSL_set_bio(server, left, left);
                    SSL_set_bio(client, right, right);
                    SSL_set_accept_state(server);
                    server
                };
                SSL_set_connect_state(client);
                for _ in 0..1000 {
                    if [server, client]
                        .iter()
                        .all(|&ssl| !crate::Ssl::new(ssl).unwrap().in_init())
                    {
                        break;
                    }
                    SSL_do_handshake(client);
                    SSL_do_handshake(server);
                }
                assert!(
                    [server, client]
                        .iter()
                        .all(|&ssl| !crate::Ssl::new(ssl).unwrap().in_init())
                );
                ERR_clear_error();
                Self {
                    server_ctx,
                    client_ctx,
                    server,
                    client,
                    fds,
                }
            }
        }

        /// Bytes waiting for the client: an alert the server sent, say.
        fn client_pending(&self) -> c_long {
            unsafe {
                BIO_ctrl(
                    SSL_get_rbio(self.client),
                    BIO_CTRL_PENDING,
                    0,
                    ptr::null_mut(),
                )
            }
        }

        /// The client's next read: 0 with `SSL_ERROR_ZERO_RETURN` after a `close_notify`.
        fn client_reads_close_notify(&self) -> bool {
            let mut byte = 0_u8;
            unsafe {
                let n = SSL_read(self.client, (&raw mut byte).cast(), 1);
                n == 0 && SSL_get_error(self.client, n) == SSL_ERROR_ZERO_RETURN
            }
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            unsafe {
                SSL_free(self.server);
                SSL_free(self.client);
                SSL_CTX_free(self.server_ctx);
                SSL_CTX_free(self.client_ctx);
                for fd in self.fds {
                    if fd >= 0 {
                        libc::close(fd);
                    }
                }
            }
        }
    }

    /// `Socket::ssl_sendfile` reads each try of a chunk into a new buffer, so a write
    /// retried after `WANT_WRITE` comes from another address with the same bytes. The
    /// socket's SSL takes it; without `SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER` OpenSSL fails
    /// it as a bad write retry and the server drops the rest of the file (#2584).
    #[test]
    fn a_write_retried_from_another_buffer_completes() {
        let pair = Pair::new(true);
        let small: c_int = 4096;
        unsafe {
            let size = libc::socklen_t::try_from(std::mem::size_of::<c_int>()).unwrap();
            let option = (&raw const small).cast();
            libc::setsockopt(pair.fds[0], libc::SOL_SOCKET, libc::SO_SNDBUF, option, size);
            libc::setsockopt(pair.fds[1], libc::SOL_SOCKET, libc::SO_RCVBUF, option, size);
        }
        let data: Vec<u8> = (0..=u8::MAX).cycle().take(65536).collect();
        let mut tls = SocketTls::default();
        // Every try's buffer stays live, so each retry is from a new address.
        let mut tries: Vec<Vec<u8>> = Vec::new();
        let mut received = Vec::new();
        let mut chunk = [0_u8; 16384];
        let mut drain = |received: &mut Vec<u8>| unsafe {
            loop {
                let n = SSL_read(pair.client, chunk.as_mut_ptr().cast(), 16384);
                if n <= 0 {
                    break;
                }
                received.extend_from_slice(&chunk[..usize::try_from(n).unwrap()]);
            }
        };
        loop {
            assert!(tries.len() < 1000, "the write never completed");
            tries.push(data.clone());
            let buffer = tries.last().unwrap();
            let sent =
                unsafe { swr_v1_socket_ssl_send(&mut tls, Ssl::new(pair.server).unwrap(), buffer) };
            if sent.value == 65536 {
                break;
            }
            assert_eq!(
                (sent.value, sent.error, tls.want_write),
                (-1, libc::EAGAIN, true),
                "try {} waits to write",
                tries.len()
            );
            drain(&mut received);
        }
        assert!(tries.len() > 1, "the first write waited");
        drain(&mut received);
        assert_eq!(received.len(), data.len());
        assert!(received == data, "the bytes arrive once, in order");
    }

    #[test]
    fn one_way_shutdown() {
        let mut pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        unsafe {
            assert_eq!(
                SSL_write(pair.server, b"bytes".as_ptr().cast(), 5),
                5,
                "payload write"
            );
            assert!(
                socket.ssl_shutdown(),
                "one-way shutdown succeeds without peer wait"
            );
            let mut received = [0_u8; 5];
            assert_eq!(
                SSL_read(pair.client, received.as_mut_ptr().cast(), 5),
                5,
                "payload precedes alert"
            );
            assert_eq!(&received, b"bytes", "payload precedes alert");
        }
        assert!(pair.client_reads_close_notify(), "strict close_notify");
        let calls = socket.calls.shutdown_calls;
        assert!(
            !socket.ssl_shutdown() && socket.calls.shutdown_calls == calls,
            "one shutdown only"
        );
        socket.ssl_close();
        pair.server = ptr::null_mut();
        assert!(
            socket.ssl.is_null() && socket.calls.shutdown_calls == calls,
            "close frees without duplicate shutdown"
        );
    }

    #[test]
    fn quiet_shutdown() {
        let pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        socket.quiet = true;
        assert!(socket.ssl_shutdown(), "quiet shutdown succeeds");
        assert_eq!(pair.client_pending(), 0, "quiet shutdown emits no alert");
    }

    #[test]
    fn peer_first() {
        let pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        unsafe {
            assert_eq!(
                SSL_shutdown(pair.client),
                0,
                "peer first sends alert without waiting"
            );
            let mut byte = 0_u8;
            let n = SSL_read(pair.server, (&raw mut byte).cast(), 1);
            assert!(
                n == 0 && SSL_get_error(pair.server, n) == SSL_ERROR_ZERO_RETURN,
                "peer alert received"
            );
            assert!(socket.ssl_shutdown(), "peer first acknowledged");
            assert_eq!(SSL_shutdown(pair.client), 1, "peer first completes");
        }
    }

    #[test]
    fn unfinished_handshake() {
        unsafe {
            let context = SSL_CTX_new(TLS_server_method());
            let mut socket = Socket::new(SSL_new(context));
            SSL_set_accept_state(socket.ssl);
            let calls = socket.calls.shutdown_calls;
            assert!(
                !socket.ssl_shutdown() && socket.calls.shutdown_calls == calls,
                "unfinished handshake not shut down"
            );
            socket.ssl_close();
            assert!(socket.ssl.is_null(), "unfinished handshake freed");
            SSL_CTX_free(context);
        }
    }

    #[test]
    fn nonblocking_want_read() {
        let pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        unsafe {
            assert_eq!(SSL_shutdown(pair.server), 0, "pending shutdown sends alert");
            ERR_clear_error();
        }
        assert!(
            !socket.ssl_shutdown(),
            "real nonblocking WANT_READ reported without waiting"
        );
        assert!(
            socket.tls.want_read && !socket.closed,
            "pending read remains retryable"
        );
    }

    #[test]
    fn pending_write() {
        let pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        let data = vec![b'x'; 16384];
        let (mut result, mut attempts) = (1, 0);
        unsafe {
            while result > 0 && attempts < 99 {
                attempts += 1;
                result = SSL_write(pair.server, data.as_ptr().cast(), 16384);
            }
            assert!(
                result < 0 && SSL_get_error(pair.server, result) == SSL_ERROR_WANT_WRITE,
                "real bounded BIO write backpressure"
            );
            ERR_clear_error();
        }
        assert!(
            !socket.ssl_shutdown(),
            "real pending write shutdown reports incomplete/error"
        );
        assert!(
            socket.tls.want_write && !socket.closed,
            "pending write remains retryable"
        );
    }

    #[test]
    fn retryable_alert_write() {
        let pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        // Fill only the transport BIO: no unfinished application SSL_write.
        let mut filler = vec![b'x'; 32768];
        let (mut bytes, mut drained) = (0, 0);
        unsafe {
            let wbio = SSL_get_wbio(pair.server);
            loop {
                let n = BIO_write(wbio, filler.as_ptr().cast(), 32768);
                if n <= 0 {
                    break;
                }
                bytes += n;
            }
            assert!(
                bytes > 0 && BIO_test_flags(wbio, BIO_FLAGS_SHOULD_RETRY) != 0,
                "alert transport full"
            );
            assert!(
                !socket.ssl_shutdown() && socket.tls.want_write && !socket.closed,
                "alert write remains retryable"
            );
            loop {
                let n = BIO_read(SSL_get_rbio(pair.client), filler.as_mut_ptr().cast(), 32768);
                if n <= 0 {
                    break;
                }
                drained += n;
            }
        }
        assert_eq!(drained, bytes, "transport capacity released");
        assert!(
            socket.ssl_shutdown(),
            "pending alert retries after peer drains"
        );
        assert!(
            pair.client_reads_close_notify(),
            "retried alert reaches peer"
        );
    }

    /// A completed handshake followed by peer transport death is distinct from alert
    /// backpressure: the real write fails with no OpenSSL reason. (The test harness
    /// ignores SIGPIPE, as Swoole does.)
    #[test]
    fn closed_peer() {
        let mut pair = Pair::new(true);
        let mut socket = Socket::new(pair.server);
        assert_eq!(
            unsafe { libc::close(pair.fds[1]) },
            0,
            "peer transport closes after handshake"
        );
        pair.fds[1] = -1;
        assert!(
            !socket.ssl_shutdown(),
            "closed peer cannot receive close_notify"
        );
        let queue = socket.calls.last_error_queue;
        assert!(
            socket.calls.last_error_class == SSL_ERROR_SYSCALL
                && socket.calls.last_shutdown_errno == libc::EPIPE
                && (queue == 0
                    || (ERR_GET_LIB(queue) == ERR_LIB_SYS && error_reason(queue) == libc::EPIPE)),
            "real closed-peer shutdown is EPIPE with only empty/system error queue"
        );
        assert!(
            socket.closed && !socket.tls.want_read && !socket.tls.want_write,
            "broken pipe is terminal rather than readiness retry"
        );
        assert_eq!(socket.warnings, 1, "fatal peer write remains reported");
        assert_eq!(
            socket.null_warnings, 0,
            "real peer failure diagnostic never formats a null string"
        );
        let calls = socket.calls.shutdown_calls;
        assert!(
            !socket.ssl_shutdown() && socket.calls.shutdown_calls == calls,
            "terminal broken pipe is not retried"
        );
    }

    #[test]
    fn injected_negative_results() {
        let mut null_warnings = 0;
        for error in [
            SSL_ERROR_WANT_READ,
            SSL_ERROR_WANT_WRITE,
            SSL_ERROR_SSL,
            SSL_ERROR_SYSCALL,
            SSL_ERROR_ZERO_RETURN,
        ] {
            let pair = Pair::new(false);
            let mut socket = Socket::new(pair.server);
            socket.calls.injected = error;
            unsafe { ERR_clear_error() };
            let ok = socket.ssl_shutdown();
            assert_eq!(
                ok,
                error == SSL_ERROR_ZERO_RETURN,
                "negative native result classified without queued error"
            );
            assert_eq!(
                socket.warnings,
                usize::from(error != SSL_ERROR_ZERO_RETURN),
                "incomplete/error shutdown reported"
            );
            assert_eq!(
                pair.client_pending(),
                0,
                "injected failure never counted as delivered alert"
            );
            null_warnings += socket.null_warnings;
        }
        assert_eq!(
            null_warnings, 0,
            "all terminal and injected diagnostics have a non-null message"
        );
    }
    /// Upstream's shutdown (no `--fix-halfclose`): both directions are marked shut
    /// first, so no alert is written, and the socket closes whatever OpenSSL returns.
    #[test]
    fn upstream_premarks_shutdown() {
        let pair = Pair::new(false);
        let mut socket = Socket::new(pair.server);
        socket.repaired = false;
        assert!(socket.ssl_shutdown(), "upstream shutdown succeeds");
        assert_eq!(pair.client_pending(), 0, "upstream writes no close_notify");
        assert!(socket.closed, "upstream closes at once");
        let calls = socket.calls.shutdown_calls;
        assert!(
            !socket.ssl_shutdown() && socket.calls.shutdown_calls == calls,
            "one shutdown only"
        );

        // A negative result counts only with a queued error, which upstream peeks
        // without clearing the queue first.
        for queued in [false, true] {
            let pair = Pair::new(false);
            let mut socket = Socket::new(pair.server);
            socket.repaired = false;
            socket.calls.injected = SSL_ERROR_WANT_WRITE;
            if queued {
                assert!(crate::OwnedBio::open(c"/nonexistent/m21").is_none());
            }
            assert_eq!(socket.ssl_shutdown(), !queued);
            assert_eq!(socket.warnings, usize::from(queued));
            assert_eq!(
                (socket.closed, socket.tls.want_read, socket.tls.want_write),
                (true, false, false),
                "closed with no retry"
            );
        }
    }
}

#[cfg(test)]
mod reason_tests {
    use super::error_reason;

    #[test]
    fn system_errors_give_their_errno() {
        // OpenSSL 3 packs a system error as ERR_SYSTEM_FLAG | errno.
        assert_eq!(error_reason(0x8000_0000 | 32), 32);
        // ERR_PACK(ERR_LIB_SSL, 0, SSL_R_SSLV3_ALERT_BAD_CERTIFICATE).
        assert_eq!(error_reason(0x0A00_0412), 1042);
        assert_eq!(error_reason(0), 0);
    }
}
