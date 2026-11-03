// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/uring_socket.cc (Swoole contributors).
//! `UringSocket`'s TLS transport: OpenSSL runs over a pair of memory BIOs, and
//! this pumps their bytes through the ring ([`crate::uring`]). Errors land on the
//! native socket through [`TlsHost`], in upstream's order (`set_err`, then
//! `check_return_value(-1)` where upstream calls it).

use std::ffi::{c_char, c_int, c_ulong, c_void};

use openssl_sys as ssl;

use crate::uring::{Host, swr_v1_uring_recv, swr_v1_uring_send};

unsafe extern "C" {
    fn BIO_ctrl_pending(bio: *mut ssl::BIO) -> usize;
}

/// The native socket's side.
#[repr(C)]
pub struct TlsHost {
    pub uring: &'static Host,
    /// `set_err(code)`.
    pub set_err: unsafe extern "C" fn(socket: *mut c_void, code: c_int),
    /// `check_return_value(-1)`.
    pub check_failure: unsafe extern "C" fn(socket: *mut c_void),
    /// The socket's read (`write` false) or write buffer, and its size.
    pub buffer:
        unsafe extern "C" fn(socket: *mut c_void, write: bool, size: *mut usize) -> *mut c_char,
    /// Logs and sets a TLS failure that OpenSSL's error queue explains.
    pub ssl_error: unsafe extern "C" fn(
        socket: *mut c_void,
        function: *const c_char,
        error: c_int,
        code: c_ulong,
    ),
    pub bad_protocol: c_int,
    pub reset: c_int,
}

/// One TLS operation's view of a `UringSocket`.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Tls {
    pub host: *const TlsHost,
    pub socket: *mut c_void,
    pub ssl: *mut ssl::SSL,
    pub rbio: *mut ssl::BIO,
    pub wbio: *mut ssl::BIO,
    pub fd: c_int,
    pub read_timeout: f64,
    pub write_timeout: f64,
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn set_errno(value: c_int) {
    // SAFETY: this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

fn len(size: usize) -> c_int {
    c_int::try_from(size).unwrap_or(c_int::MAX)
}

impl Tls {
    fn host(&self) -> &TlsHost {
        // SAFETY: the native caller passes its static host table.
        unsafe { &*self.host }
    }

    fn set_err(&self, code: c_int) {
        // SAFETY: the socket is live for the operation.
        unsafe { (self.host().set_err)(self.socket, code) };
    }

    fn check_failure(&self) {
        // SAFETY: as above.
        unsafe { (self.host().check_failure)(self.socket) };
    }

    fn buffer(&self, write: bool) -> (*mut c_char, usize) {
        let mut size = 0;
        // SAFETY: as above.
        let data = unsafe { (self.host().buffer)(self.socket, write, &raw mut size) };
        (data, size)
    }

    fn pending(&self) -> usize {
        // SAFETY: `wbio` is the socket's live memory BIO.
        unsafe { BIO_ctrl_pending(self.wbio) }
    }

    /// Upstream's `ssl_bio_write`: send everything OpenSSL queued.
    fn bio_write(&self) -> bool {
        let (buf, size) = self.buffer(true);
        while self.pending() != 0 {
            // SAFETY: `buf` holds `size` bytes.
            let nread = unsafe { ssl::BIO_read(self.wbio, buf.cast(), len(size)) };
            if nread <= 0 {
                self.set_err(self.host().bad_protocol);
                return false;
            }
            let nread = nread.unsigned_abs() as usize;
            let mut written = 0;
            while written < nread {
                let rest = buf.wrapping_add(written);
                // SAFETY: the first `nread` bytes of `buf` are filled.
                let n = unsafe {
                    swr_v1_uring_send(
                        self.host().uring,
                        self.fd,
                        rest.cast(),
                        nread - written,
                        0,
                        self.write_timeout,
                    )
                };
                if n > 0 {
                    written += n.unsigned_abs();
                } else if n == 0 {
                    set_errno(self.host().reset);
                    return false;
                } else if errno() != libc::EINTR {
                    self.set_err(errno());
                    return false;
                }
            }
        }
        true
    }

    /// Upstream's `ssl_bio_read`: feed OpenSSL one receive.
    fn bio_read(&self) -> bool {
        let (buf, size) = self.buffer(false);
        loop {
            // SAFETY: `buf` holds `size` bytes.
            let rv = unsafe {
                swr_v1_uring_recv(
                    self.host().uring,
                    self.fd,
                    buf.cast(),
                    size,
                    0,
                    self.read_timeout,
                )
            };
            if rv > 0 {
                let rv = c_int::try_from(rv).unwrap_or(c_int::MAX);
                // SAFETY: the first `rv` bytes of `buf` were received.
                if unsafe { ssl::BIO_write(self.rbio, buf.cast(), rv) } != rv {
                    self.set_err(self.host().bad_protocol);
                    return false;
                }
                return true;
            } else if rv == 0 {
                self.set_err(self.host().reset);
                return false;
            } else if errno() != libc::EINTR {
                self.set_err(errno());
                return false;
            }
        }
    }

    /// Upstream's `ssl_bio_prepare`.
    fn prepare(&self) -> bool {
        if self.pending() > 0 && !self.bio_write() {
            self.check_failure();
            return false;
        }
        true
    }

    /// Upstream's `ssl_bio_perform`: move bytes for what OpenSSL waits on.
    fn perform(&self, rc: c_int, function: *const c_char) -> bool {
        if !self.prepare() {
            return false;
        }
        // SAFETY: `ssl` is the socket's live session.
        let error = unsafe { ssl::SSL_get_error(self.ssl, rc) };
        let moved = match error {
            ssl::SSL_ERROR_WANT_WRITE => self.bio_write(),
            ssl::SSL_ERROR_WANT_READ => self.bio_read(),
            ssl::SSL_ERROR_ZERO_RETURN | ssl::SSL_ERROR_SYSCALL => false,
            _ => {
                // SAFETY: reads this thread's OpenSSL error queue.
                let code = unsafe { ssl::ERR_get_error() };
                if code == 0 {
                    self.set_err(self.host().bad_protocol);
                } else {
                    // SAFETY: the native callback logs and sets the error.
                    unsafe { (self.host().ssl_error)(self.socket, function, error, code) };
                }
                return false;
            }
        };
        if !moved {
            self.check_failure();
        }
        moved
    }

    /// Upstream's `ssl_recv` / `ssl_send` loop around `SSL_read` / `SSL_write`.
    fn transfer(&self, function: &std::ffi::CStr, mut io: impl FnMut() -> c_int) -> isize {
        loop {
            let n = io();
            if !self.prepare() {
                return -1;
            }
            if n > 0 {
                return isize::try_from(n).unwrap_or(-1);
            }
            if !self.perform(n, function.as_ptr()) {
                return -1;
            }
        }
    }
}

/// `ssl_bio_write`.
///
/// # Safety
/// `tls` describes a live socket, its session and BIOs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_write(tls: Tls) -> bool {
    tls.bio_write()
}

/// `ssl_bio_read`.
///
/// # Safety
/// As [`swr_v1_uring_tls_write`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_read(tls: Tls) -> bool {
    tls.bio_read()
}

/// `ssl_bio_prepare`.
///
/// # Safety
/// As [`swr_v1_uring_tls_write`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_prepare(tls: Tls) -> bool {
    tls.prepare()
}

/// `ssl_bio_perform` for `rc` from the OpenSSL call named `function`.
///
/// # Safety
/// As [`swr_v1_uring_tls_write`]; `function` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_perform(
    tls: Tls,
    rc: c_int,
    function: *const c_char,
) -> bool {
    tls.perform(rc, function)
}

/// `ssl_recv`: decrypted bytes into `buf`, or -1.
///
/// # Safety
/// As [`swr_v1_uring_tls_write`]; `buf` holds `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_recv(tls: Tls, buf: *mut c_void, n: usize) -> isize {
    // SAFETY: `buf` holds `n` bytes.
    tls.transfer(c"ssl_recv", || unsafe {
        ssl::SSL_read(tls.ssl, buf, len(n))
    })
}

/// `ssl_send`: bytes of `buf` encrypted and sent, or -1.
///
/// # Safety
/// As [`swr_v1_uring_tls_write`]; `buf` holds `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_send(tls: Tls, buf: *const c_void, n: usize) -> isize {
    // SAFETY: `buf` holds `n` bytes.
    tls.transfer(c"ssl_send", || unsafe {
        ssl::SSL_write(tls.ssl, buf, len(n))
    })
}

/// `UringSocket::ssl_sendfile`'s outcome: bytes sent and the warning to log
/// (`crate::sendfile::TLS_WARN_*`, or 0).
#[repr(C)]
pub struct TlsSendfile {
    pub total: usize,
    pub warn: c_int,
}

/// `UringSocket::ssl_sendfile`: `size` bytes of `file` from `*offset`, encrypted
/// ([`crate::sendfile::tls_sendfile`]). `classify` is the socket's
/// `catch_write_error(errno)`.
///
/// # Safety
/// As [`swr_v1_uring_tls_write`]; `file` is open and `offset` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_uring_tls_sendfile(
    tls: Tls,
    file: c_int,
    offset: *mut libc::off_t,
    size: usize,
    classify: unsafe extern "C" fn(socket: *mut c_void, error: c_int) -> c_int,
) -> TlsSendfile {
    // SAFETY: the caller's offset.
    let offset = unsafe { &mut *offset };
    let (total, warn) = crate::sendfile::tls_sendfile(
        size,
        offset,
        // SAFETY: `buf` is writable for its length.
        |buf, at| unsafe { libc::pread(file, buf.as_mut_ptr().cast(), buf.len(), at) },
        // SAFETY: the operation's live TLS state, and the bytes read.
        |data| unsafe { swr_v1_uring_tls_send(tls, data.as_ptr().cast(), data.len()) },
        // SAFETY: the live socket's classifier.
        || unsafe { classify(tls.socket, errno()) },
    );
    TlsSendfile { total, warn }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uring::testing::{HOST, with_engine};
    use std::cell::RefCell;

    thread_local! {
        static ERRORS: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
        static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0; 64]);
    }
    unsafe extern "C" fn set_err(_socket: *mut c_void, code: c_int) {
        ERRORS.with(|errors| errors.borrow_mut().push(code));
    }
    unsafe extern "C" fn check_failure(_socket: *mut c_void) {
        ERRORS.with(|errors| errors.borrow_mut().push(-1));
    }
    unsafe extern "C" fn buffer(
        _socket: *mut c_void,
        _write: bool,
        size: *mut usize,
    ) -> *mut c_char {
        BUFFER.with(|buffer| {
            let mut buffer = buffer.borrow_mut();
            unsafe { *size = buffer.len() };
            buffer.as_mut_ptr().cast()
        })
    }
    unsafe extern "C" fn ssl_error(
        _socket: *mut c_void,
        _function: *const c_char,
        _error: c_int,
        _code: c_ulong,
    ) {
    }
    static TLS_HOST: TlsHost = TlsHost {
        uring: &HOST,
        set_err,
        check_failure,
        buffer,
        ssl_error,
        bad_protocol: 1008,
        reset: 1009,
    };

    fn tls(fd: c_int, rbio: *mut ssl::BIO, wbio: *mut ssl::BIO) -> Tls {
        Tls {
            host: &raw const TLS_HOST,
            socket: std::ptr::null_mut(),
            ssl: std::ptr::null_mut(),
            rbio,
            wbio,
            fd,
            read_timeout: 1.0,
            write_timeout: 1.0,
        }
    }

    #[test]
    fn pumps_bio_bytes_through_the_ring() {
        with_engine(|| unsafe {
            let mut fds = [0; 2];
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()),
                0,
                "socketpair"
            );
            let (rbio, wbio) = (
                ssl::BIO_new(ssl::BIO_s_mem()),
                ssl::BIO_new(ssl::BIO_s_mem()),
            );
            // Queued output, more than one buffer's worth, reaches the peer whole.
            let message = [7_u8; 100];
            assert_eq!(
                ssl::BIO_write(wbio, message.as_ptr().cast(), 100),
                100,
                "queued"
            );
            assert!(
                swr_v1_uring_tls_prepare(tls(fds[0], rbio, wbio)),
                "prepare sends"
            );
            let mut received = [0_u8; 128];
            assert_eq!(
                libc::read(fds[1], received.as_mut_ptr().cast(), 128),
                100,
                "peer reads all"
            );
            // Input lands in the read BIO.
            assert_eq!(
                libc::write(fds[1], b"abc".as_ptr().cast(), 3),
                3,
                "peer writes"
            );
            assert!(swr_v1_uring_tls_read(tls(fds[0], rbio, wbio)), "read");
            let mut out = [0_u8; 8];
            assert_eq!(
                ssl::BIO_read(rbio, out.as_mut_ptr().cast(), 8),
                3,
                "BIO holds the bytes"
            );
            // The peer closing is a TLS reset.
            libc::close(fds[1]);
            assert!(!swr_v1_uring_tls_read(tls(fds[0], rbio, wbio)), "closed");
            assert_eq!(
                ERRORS.with(|errors| errors.borrow().clone()),
                vec![1009],
                "reset"
            );
            ssl::BIO_free_all(rbio);
            ssl::BIO_free_all(wbio);
            libc::close(fds[0]);
        });
    }
}
