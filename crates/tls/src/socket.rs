//! `network::Socket`'s TLS operations, OpenSSL calls included: creating the SSL,
//! handshake steps, reads and writes, host and verify checks, PEM export and error
//! reasons, and the socket's TLS state group they set. `upstream` defines the
//! `Socket` methods over them, errno, the last error and logs included.
use openssl_sys::{
    BIO_ctrl, BIO_free_all, BIO_new, BIO_read, BIO_s_mem, ERR_clear_error, ERR_get_error,
    ERR_reason_error_string, PEM_write_bio_X509, SSL, SSL_CTRL_MODE, SSL_CTX, SSL_ERROR_SSL,
    SSL_ERROR_SYSCALL, SSL_ERROR_WANT_READ, SSL_ERROR_WANT_WRITE, SSL_ERROR_ZERO_RETURN,
    SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER, SSL_RECEIVED_SHUTDOWN, SSL_SENT_SHUTDOWN, SSL_accept,
    SSL_connect, SSL_ctrl, SSL_get_error, SSL_get_shutdown, SSL_get_verify_result, SSL_new,
    SSL_read, SSL_set_accept_state, SSL_set_connect_state, SSL_set_ex_data, SSL_set_shutdown,
    SSL_shutdown, SSL_write, X509, X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT, X509_V_OK,
    X509_check_host, X509_free,
};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_long, c_ulong, c_void};
use std::ptr::NonNull;
use swoole_rs_reactor::SocketFields;

// SAFETY: libssl/libcrypto's own declarations; openssl-sys binds neither.
unsafe extern "C" {
    fn SSL_set_fd(ssl: *mut SSL, fd: c_int) -> c_int;
    fn ERR_peek_error() -> c_ulong;
    fn SSL_in_init(ssl: *const SSL) -> c_int;
    fn SSL_set_quiet_shutdown(ssl: *mut SSL, mode: c_int);
}

/// `BIO_CTRL_PENDING`, behind the `BIO_pending` macro.
const BIO_CTRL_PENDING: c_int = 10;
/// `SW_SSL_SERVER` and `SW_SSL_CLIENT`.
const SERVER: c_int = 1;
const CLIENT: c_int = 2;

/// `SW_ERROR_SSL_BAD_CLIENT` and `SW_ERROR_SSL_RESET`.
const BAD_CLIENT: c_int = 1015;
const RESET: c_int = 1017;
/// `SW_ERROR_SSL_HANDSHAKE_FAILED`.
const HANDSHAKE_FAILED: c_int = 1018;
/// `swReturnCode`'s `SW_WAIT`, `SW_ERROR` and `SW_READY`; `SW_OK`/`SW_ERR`.
const WAIT: c_int = 2;
const ERROR: c_int = 4;
const READY: c_int = 5;
const OK: c_int = 0;
const ERR: c_int = -1;
/// `SW_SSL_STATE_READY` and `SW_SSL_STATE_WAIT_STREAM`.
const STATE_READY: u32 = 1;
const STATE_WAIT_STREAM: u32 = 2;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `network::Socket`'s TLS state group (`ssl_state`, `ssl_want_read`,
/// `ssl_want_write`): the record's own field.
pub use swoole_rs_reactor::SocketTlsState as SocketTls;

fn want(tls: &mut SocketTls, read: bool, write: bool) {
    tls.want_read = u8::from(read);
    tls.want_write = u8::from(write);
}

/// A live SSL on this thread. A socket record's is live while it is set:
/// `Socket::ssl_create` sets it, and `ssl_close` frees it and clears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ssl(NonNull<SSL>);

impl Ssl {
    /// # Safety
    /// `ssl` is null or a live SSL, not freed while the handle is used.
    #[must_use]
    pub unsafe fn new(ssl: *mut SSL) -> Option<Self> {
        NonNull::new(ssl).map(Self)
    }

    /// The record's SSL, or None without one.
    #[must_use]
    pub fn of(record: &SocketFields) -> Option<Self> {
        // SAFETY: a record's SSL is live while it is set (above).
        unsafe { Self::new(record.ssl.cast()) }
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut SSL {
        self.0.as_ptr()
    }

    fn error(self, result: c_int) -> c_int {
        // SAFETY: a live SSL, and its last I/O result.
        unsafe { SSL_get_error(self.as_ptr(), result) }
    }

    /// `SSL_read` into `length` bytes at `buffer`.
    ///
    /// # Safety
    /// `buffer` denotes `length` writable bytes.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "upstream passes its size_t to SSL_read's int the same way"
    )]
    unsafe fn read(self, buffer: *mut c_void, length: usize) -> c_int {
        // SAFETY: the caller lends `length` writable bytes to a live SSL.
        unsafe { SSL_read(self.as_ptr(), buffer, length as c_int) }
    }

    /// `SSL_write` of `length` bytes at `buffer`.
    ///
    /// # Safety
    /// `buffer` denotes `length` readable bytes.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "upstream passes its size_t to SSL_write's int the same way"
    )]
    unsafe fn write(self, buffer: *const c_void, length: usize) -> c_int {
        // SAFETY: the caller lends `length` readable bytes to a live SSL.
        unsafe { SSL_write(self.as_ptr(), buffer, length as c_int) }
    }
}

/// Clears this thread's OpenSSL error queue.
pub(crate) fn clear_errors() {
    // SAFETY: this thread's queue; no preconditions.
    unsafe { ERR_clear_error() };
}

/// This thread's oldest queued OpenSSL error, left queued.
pub(crate) fn peek_error() -> c_ulong {
    // SAFETY: this thread's queue; no preconditions.
    unsafe { ERR_peek_error() }
}

/// This thread's oldest queued OpenSSL error, popped.
pub(crate) fn pop_error() -> c_ulong {
    // SAFETY: this thread's queue; no preconditions.
    unsafe { ERR_get_error() }
}

/// OpenSSL's reason string for `error`, None for an unknown code.
#[must_use]
pub fn reason_string(error: c_ulong) -> Option<&'static CStr> {
    // SAFETY: OpenSSL's static string table, null for an unknown code.
    let reason = unsafe { ERR_reason_error_string(error) };
    // SAFETY: a static NUL-terminated string when not null.
    (!reason.is_null()).then(|| unsafe { CStr::from_ptr(reason) })
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

/// `Socket::ssl_recv`: `SSL_read` up to `length` bytes.
///
/// # Safety
/// `buffer` denotes `length` writable bytes.
pub unsafe fn swr_v1_socket_ssl_recv(
    tls: &mut SocketTls,
    ssl: Ssl,
    buffer: *mut c_void,
    length: usize,
) -> SslIoResult {
    clear_errors();
    // SAFETY: as the caller promises.
    let n = unsafe { ssl.read(buffer, length) };
    classify(tls, ssl, n, false)
}

/// `Socket::ssl_send`: `SSL_write` of `length` bytes (native code applies the DTLS
/// chunk limit first).
///
/// # Safety
/// `buffer` denotes `length` readable bytes.
pub unsafe fn swr_v1_socket_ssl_send(
    tls: &mut SocketTls,
    ssl: Ssl,
    buffer: *const c_void,
    length: usize,
) -> SslIoResult {
    clear_errors();
    // SAFETY: as the caller promises.
    let n = unsafe { ssl.write(buffer, length) };
    classify(tls, ssl, n, true)
}

/// A handshake step for `Socket::ssl_accept`/`ssl_connect`, which also sets the
/// socket's want flags and, once it changes, `ssl_state`. Native code sets the last
/// error when `last_error` is nonzero, then logs: 1 a bad client (accept), 2 a failed
/// handshake with `ssl_error`, 3 a peer that closed (connect, debug). `code` is what
/// the method returns.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SslHandshakeResult {
    pub code: c_int,
    pub last_error: c_int,
    pub log: c_int,
    pub ssl_error: c_int,
}

impl SslHandshakeResult {
    const fn returning(code: c_int) -> Self {
        Self {
            code,
            last_error: 0,
            log: 0,
            ssl_error: 0,
        }
    }
}

/// `Socket::ssl_accept`: one `SSL_accept` step. A DTLS socket waits to read on a
/// syscall error that left errno 0.
pub fn swr_v1_socket_ssl_accept(tls: &mut SocketTls, ssl: Ssl, dtls: bool) -> SslHandshakeResult {
    want(tls, false, false);
    clear_errors();
    // SAFETY: a live SSL.
    let n = unsafe { SSL_accept(ssl.as_ptr()) };
    if n == 1 {
        tls.state = STATE_READY;
        return SslHandshakeResult::returning(READY);
    }
    if n == 0 {
        return SslHandshakeResult::returning(ERROR);
    }
    let error = ssl.error(n);
    let mut result = SslHandshakeResult::returning(ERROR);
    match error {
        SSL_ERROR_WANT_READ => {
            want(tls, true, false);
            result.code = WAIT;
        }
        SSL_ERROR_WANT_WRITE => {
            want(tls, false, true);
            result.code = WAIT;
        }
        SSL_ERROR_SSL => result.log = 1,
        SSL_ERROR_SYSCALL => {
            if dtls && errno() == 0 {
                want(tls, true, false);
                result.code = WAIT;
            }
        }
        _ => {
            result.log = 2;
            result.ssl_error = error;
        }
    }
    result
}

/// `Socket::ssl_connect`: one `SSL_connect` step.
pub fn swr_v1_socket_ssl_connect(tls: &mut SocketTls, ssl: Ssl) -> SslHandshakeResult {
    want(tls, false, false);
    clear_errors();
    // SAFETY: a live SSL.
    let n = unsafe { SSL_connect(ssl.as_ptr()) };
    if n == 1 {
        tls.state = STATE_READY;
        return SslHandshakeResult::returning(OK);
    }
    let error = ssl.error(n);
    let mut result = SslHandshakeResult::returning(ERR);
    match error {
        SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => {
            want(
                tls,
                error == SSL_ERROR_WANT_READ,
                error == SSL_ERROR_WANT_WRITE,
            );
            tls.state = STATE_WAIT_STREAM;
            result.code = OK;
            return result;
        }
        SSL_ERROR_ZERO_RETURN => {
            result.last_error = RESET;
            result.log = 3;
            return result;
        }
        SSL_ERROR_SYSCALL if n != 0 => {
            result.last_error = errno();
            return result;
        }
        SSL_ERROR_SYSCALL => {}
        _ => result.last_error = HANDSHAKE_FAILED,
    }
    result.log = 2;
    result.ssl_error = error;
    result
}

/// A live certificate reference this side owns, released when dropped.
pub struct Cert(pub(crate) NonNull<X509>);

impl Cert {
    #[must_use]
    pub fn as_ptr(&self) -> *mut X509 {
        self.0.as_ptr()
    }

    /// `Socket::ssl_check_host`'s test: whether the certificate matches the host
    /// `name` under `X509_check_host`.
    #[must_use]
    pub fn matches_host(&self, name: &[u8]) -> bool {
        // SAFETY: the live certificate and the name's bytes.
        let matched = unsafe {
            X509_check_host(
                self.as_ptr(),
                name.as_ptr().cast(),
                name.len(),
                0,
                std::ptr::null_mut(),
            )
        };
        matched == 1
    }
}

impl Drop for Cert {
    fn drop(&mut self) {
        // SAFETY: drops the reference this side owns.
        unsafe { X509_free(self.as_ptr()) };
    }
}

/// `Socket::ssl_verify`: the peer's verify result if it rejects the peer, else
/// `X509_V_OK`. A self-signed leaf passes when `allow_self_signed`.
#[must_use]
pub fn swr_v1_socket_ssl_verify(ssl: Ssl, allow_self_signed: bool) -> c_long {
    // SAFETY: a live SSL.
    let result = unsafe { SSL_get_verify_result(ssl.as_ptr()) };
    if allow_self_signed && result == c_long::from(X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT) {
        return c_long::from(X509_V_OK);
    }
    result
}

/// `_ssl_read_x509_file`: `cert` as PEM into at most `length` bytes at `buffer`.
/// Returns the bytes written, or -1 when `BIO_new` failed and -2 when
/// `PEM_write_bio_X509` failed (native code warns). When `consume`, frees `cert`.
///
/// # Safety
/// `cert` is a live X509 (owned by the caller when `consume`), and `buffer` denotes
/// `length` writable bytes.
pub unsafe fn swr_v1_socket_ssl_pem(
    cert: *mut X509,
    buffer: *mut c_char,
    length: usize,
    consume: bool,
) -> c_int {
    // SAFETY: OpenSSL's static memory BIO method.
    let method = unsafe { BIO_s_mem() };
    // SAFETY: a fresh BIO of that method.
    let bio = unsafe { BIO_new(method) };
    let written = if bio.is_null() {
        -1
    // SAFETY: the caller's certificate into the fresh BIO.
    } else if unsafe { PEM_write_bio_X509(bio, cert) } == 0 {
        -2
    } else {
        // SAFETY: the live BIO; BIO_pending.
        let pending = unsafe { BIO_ctrl(bio, BIO_CTRL_PENDING, 0, std::ptr::null_mut()) };
        let room = c_long::try_from(length).unwrap_or(c_long::MAX);
        let count = c_int::try_from(pending.min(room)).unwrap_or(c_int::MAX);
        // SAFETY: at most `length` bytes into the caller's buffer.
        unsafe { BIO_read(bio, buffer.cast(), count) }
    };
    if !bio.is_null() {
        // SAFETY: frees the BIO made above.
        unsafe { BIO_free_all(bio) };
    }
    if consume {
        // SAFETY: drops the reference the caller handed over.
        unsafe { X509_free(cert) };
    }
    written
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
        // SAFETY: a live SSL.
        unsafe { SSL_shutdown(ssl.as_ptr()) }
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
    closed: &mut u8,
    quiet: bool,
    repaired: bool,
    calls: &mut impl ShutdownCalls,
) -> c_int {
    // SAFETY: a live SSL.
    if *closed != 0 || unsafe { SSL_in_init(ssl.as_ptr()) } != 0 {
        return SHUTDOWN_FALSE;
    }
    // If the peer closed first, shut down quietly: sending data then makes the peer
    // reply with an RST segment.
    if quiet {
        // SAFETY: a live SSL.
        unsafe { SSL_set_quiet_shutdown(ssl.as_ptr(), 1) };
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
        *closed = u8::from(tls.want_read == 0 && tls.want_write == 0);
        (n, error)
    } else {
        // Upstream marks both directions shut first, so OpenSSL writes no alert.
        // SAFETY: a live SSL.
        let mode = unsafe { SSL_get_shutdown(ssl.as_ptr()) };
        let mode = mode | SSL_RECEIVED_SHUTDOWN | SSL_SENT_SHUTDOWN;
        // SAFETY: a live SSL.
        unsafe { SSL_set_shutdown(ssl.as_ptr(), mode) };
        let n = calls.shutdown(ssl);
        *closed = 1;
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
    closed: &mut u8,
    quiet: bool,
    repaired: bool,
) -> c_int {
    shutdown(tls, ssl, closed, quiet, repaired, &mut OpenSsl)
}

/// `Socket::ssl_create`: a new SSL from `context` on `fd`, in connect state for
/// `SW_SSL_CLIENT` or accept state for `SW_SSL_SERVER`, carrying `socket` as its
/// ex data at `index`, after clearing the want flags. It accepts a write retried from
/// another buffer: `Socket::ssl_sendfile` reads each try into a new one, where
/// upstream's stack buffer happens to keep its address. `ssl` is set once made and
/// the state cleared on success. Returns 0, or 1 when `SSL_new`
/// failed, 2 when `SSL_set_fd` failed (`error` holds the peeked error) and 3 when
/// `SSL_set_ex_data` failed; native code warns.
///
/// # Safety
/// `context` is a live `SSL_CTX`.
#[expect(
    clippy::too_many_arguments,
    reason = "Socket::ssl_create's inputs plus its two out-parameters"
)]
pub unsafe fn swr_v1_socket_ssl_new(
    tls: &mut SocketTls,
    context: *mut SSL_CTX,
    fd: c_int,
    flags: c_int,
    index: c_int,
    socket: *mut c_void,
    ssl: &mut *mut c_void,
    error: &mut c_ulong,
) -> c_int {
    want(tls, false, false);
    clear_errors();
    // SAFETY: the caller's live context.
    let made = unsafe { SSL_new(context) };
    *ssl = made.cast();
    if made.is_null() {
        return 1;
    }
    // SAFETY: the new SSL; `SSL_set_mode` is this `SSL_ctrl`.
    unsafe {
        SSL_ctrl(
            made,
            SSL_CTRL_MODE,
            SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER,
            std::ptr::null_mut(),
        )
    };
    // SAFETY: the new SSL and the caller's descriptor.
    if unsafe { SSL_set_fd(made, fd) } == 0 {
        *error = peek_error();
        return 2;
    }
    if flags & CLIENT != 0 {
        // SAFETY: the new SSL.
        unsafe { SSL_set_connect_state(made) };
    } else if flags & SERVER != 0 {
        // SAFETY: the new SSL.
        unsafe { SSL_set_accept_state(made) };
    }
    // SAFETY: the new SSL keeps the caller's socket pointer as opaque ex data.
    if unsafe { SSL_set_ex_data(made, index, socket) } == 0 {
        return 3;
    }
    tls.state = 0;
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl_sys::*;
    use std::os::raw::c_int;

    /// A test's live SSL.
    fn handle(ssl: *mut SSL) -> Ssl {
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
            let read = swr_v1_socket_ssl_recv(
                &mut tls,
                handle(ssl),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            );
            assert_eq!(
                read,
                SslIoResult {
                    value: -1,
                    error: libc::EAGAIN,
                    ..Default::default()
                }
            );
            assert_eq!((tls.want_read, tls.want_write), (1, 0));
            tls.want_write = 1;
            let sent =
                swr_v1_socket_ssl_send(&mut tls, handle(ssl), buffer.as_ptr().cast(), buffer.len());
            assert_eq!(sent, read, "a write also waits for the server's hello");
            assert_eq!(
                (tls.want_read, tls.want_write),
                (1, 0),
                "and replaces the flags"
            );
            swr_v1_socket_ssl_clear(&mut tls);
            assert_eq!((tls.want_read, tls.want_write), (0, 0), "nothing pending");
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
                SslHandshakeResult::returning(OK),
                "the client hello is out"
            );
            assert_eq!(
                tls,
                SocketTls {
                    state: STATE_WAIT_STREAM,
                    want_read: 1,
                    want_write: 0
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
                (ERR, HANDSHAKE_FAILED, 2)
            );
            assert_eq!(failed.ssl_error, SSL_ERROR_SSL);
            assert_eq!(
                (tls.state, tls.want_read),
                (STATE_WAIT_STREAM, 0),
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
                SslHandshakeResult::returning(WAIT),
                "no client hello yet"
            );
            assert_eq!((tls.state, tls.want_read, tls.want_write), (0, 1, 0));
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
                if cert.matches_host(name.as_bytes()) {
                    1
                } else {
                    2
                }
            };
            let named = |common: &str| {
                let cert = X509_new();
                let name = X509_get_subject_name(cert);
                let common = std::ffi::CString::new(common).unwrap();
                X509_NAME_add_entry_by_txt(
                    name,
                    c"CN".as_ptr(),
                    MBSTRING_ASC,
                    common.as_ptr().cast(),
                    -1,
                    -1,
                    0,
                );
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
            let empty = X509_new();
            assert_eq!(
                swr_v1_socket_ssl_pem(empty, pem.as_mut_ptr().cast(), pem.len(), true),
                -2,
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
            let written = swr_v1_socket_ssl_pem(cert, pem.as_mut_ptr().cast(), pem.len(), false);
            assert!(written > 0);
            assert!(pem.starts_with(b"-----BEGIN CERTIFICATE-----"));
            let mut short = [0_u8; 10];
            assert_eq!(
                swr_v1_socket_ssl_pem(cert, short.as_mut_ptr().cast(), short.len(), true),
                10,
                "bounded by the buffer"
            );

            let context = SSL_CTX_new(TLS_method());
            let mut ssl = std::ptr::null_mut();
            let mut error = 0;
            let mut socket = 7_u8;
            let index = SSL_get_ex_new_index(0, std::ptr::null_mut(), None, None, None);
            let mut tls = SocketTls {
                state: STATE_READY,
                want_read: 1,
                want_write: 1,
            };
            let made = swr_v1_socket_ssl_new(
                &mut tls,
                context,
                3,
                CLIENT,
                index,
                (&raw mut socket).cast(),
                &mut ssl,
                &mut error,
            );
            assert_eq!(made, 0);
            assert_eq!(
                tls,
                SocketTls::default(),
                "a fresh SSL clears the state group"
            );
            let made = ssl.cast::<SSL>();
            assert_eq!(SSL_is_server(made), 0, "connect state");
            assert_eq!(SSL_get_ex_data(made, index), (&raw mut socket).cast());
            SSL_free(made);
            assert_eq!(
                swr_v1_socket_ssl_new(
                    &mut tls,
                    context,
                    3,
                    SERVER,
                    index,
                    std::ptr::null_mut(),
                    &mut ssl,
                    &mut error
                ),
                0
            );
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
            swr_v1_socket_ssl_recv(
                &mut tls,
                handle(ssl),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            );
            let garbage = [
                0x16_u8, 0x03, 0x03, 0x00, 0x05, 0xff, 0xff, 0xff, 0xff, 0xff,
            ];
            BIO_write(
                read,
                garbage.as_ptr().cast(),
                c_int::try_from(garbage.len()).unwrap(),
            );
            let result = swr_v1_socket_ssl_recv(
                &mut tls,
                handle(ssl),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            );
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
    use std::os::raw::{c_char, c_int, c_long};
    use std::ptr;

    // SAFETY: libssl/libcrypto's own declarations; openssl-sys binds none of them.
    unsafe extern "C" {
        fn BIO_new_bio_pair(
            left: *mut *mut BIO,
            left_size: usize,
            right: *mut *mut BIO,
            right_size: usize,
        ) -> c_int;
        fn BIO_test_flags(bio: *const BIO, flags: c_int) -> c_int;
        fn BIO_new_file(filename: *const c_char, mode: *const c_char) -> *mut BIO;
    }

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
            self.last_error_queue = unsafe { ERR_peek_error() };
            self.last_error_class
        }
    }

    /// `network::Socket`'s TLS fields and the native caller: its warning formats
    /// `ssl_get_error_reason`'s string, or a fallback when OpenSSL has none.
    struct Socket {
        ssl: *mut SSL,
        tls: SocketTls,
        closed: u8,
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
                closed: 0,
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
            if self.closed == 0 {
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
                    SSL_set_fd(client, fds[1]);
                    // The server end as `Socket::ssl_create` makes it.
                    let (mut tls, mut server, mut error) =
                        (SocketTls::default(), ptr::null_mut(), 0);
                    let made = swr_v1_socket_ssl_new(
                        &mut tls,
                        server_ctx,
                        fds[0],
                        SERVER,
                        0,
                        ptr::null_mut(),
                        &mut server,
                        &mut error,
                    );
                    assert_eq!(made, 0);
                    server.cast()
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
                    if SSL_in_init(server) == 0 && SSL_in_init(client) == 0 {
                        break;
                    }
                    SSL_do_handshake(client);
                    SSL_do_handshake(server);
                }
                assert!(SSL_in_init(server) == 0 && SSL_in_init(client) == 0);
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
            let sent = unsafe {
                swr_v1_socket_ssl_send(
                    &mut tls,
                    Ssl::new(pair.server).unwrap(),
                    buffer.as_ptr().cast(),
                    buffer.len(),
                )
            };
            if sent.value == 65536 {
                break;
            }
            assert_eq!(
                (sent.value, sent.error, tls.want_write),
                (-1, libc::EAGAIN, 1),
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
            socket.tls.want_read != 0 && socket.closed == 0,
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
            socket.tls.want_write != 0 && socket.closed == 0,
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
                !socket.ssl_shutdown() && socket.tls.want_write != 0 && socket.closed == 0,
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
            socket.closed != 0 && socket.tls.want_read == 0 && socket.tls.want_write == 0,
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
        assert_ne!(socket.closed, 0, "upstream closes at once");
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
                assert!(
                    unsafe { BIO_new_file(c"/nonexistent/m21".as_ptr(), c"r".as_ptr()) }.is_null()
                );
            }
            assert_eq!(socket.ssl_shutdown(), !queued);
            assert_eq!(socket.warnings, usize::from(queued));
            assert_eq!(
                (socket.closed, socket.tls.want_read, socket.tls.want_write),
                (1, 0, 0),
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
