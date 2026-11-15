// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.5 src/coroutine/uring_socket.cc (Swoole contributors).
//! `UringSocket`'s TLS transport: OpenSSL runs over a pair of memory BIOs, and
//! this pumps their bytes through the ring ([`crate::uring`]). Errors land on the
//! socket through [`TlsSocket`], in upstream's order (`set_err`, then
//! `check_return_value(-1)` where upstream calls it).

use std::ffi::{CStr, c_char, c_int, c_ulong};
use std::mem::MaybeUninit;
use swoole_rs_corebase::set_errno;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_tls::{Bio, Ssl};

use openssl_sys as ssl;

use crate::uring::{self, Host};

/// `SW_ERROR_SSL_BAD_PROTOCOL` and `SW_ERROR_SSL_RESET`.
pub const SSL_BAD_PROTOCOL: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SSL_BAD_PROTOCOL as c_int;
pub const SSL_RESET: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SSL_RESET as c_int;

/// The socket's side.
pub trait TlsSocket: Copy {
    /// `set_err(code)`.
    fn set_err(self, code: c_int);
    /// `check_return_value(-1)`.
    fn check_failure(self);
    /// The socket's read (`write` false) or write buffer, and its size.
    fn buffer(self, write: bool) -> (*mut c_char, usize);
    /// Logs and sets a TLS failure that OpenSSL's error queue explains.
    fn ssl_error(self, function: &CStr, error: c_int, code: c_ulong);
}

/// One TLS operation's view of a `UringSocket`, borrowing its session for `'a`.
#[derive(Clone, Copy)]
pub struct Tls<'a, S> {
    pub uring: &'static Host,
    pub socket: S,
    pub ssl: Option<Ssl<'a>>,
    pub rbio: Option<Bio<'a>>,
    pub wbio: Option<Bio<'a>>,
    pub fd: c_int,
    pub read_timeout: f64,
    pub write_timeout: f64,
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl<'a, S> Tls<'a, S> {
    /// The view with `ssl` as its session, over the session's own BIOs while they are
    /// memory BIOs (as [`crate::uring_socket::tls_session`] sets them), else none.
    pub(crate) fn with_session(self, ssl: Option<Ssl<'a>>) -> Self {
        let memory = |bio: Option<Bio<'a>>| bio.filter(|bio| bio.is_memory());
        Self {
            ssl,
            rbio: memory(ssl.and_then(Ssl::read_bio)),
            wbio: memory(ssl.and_then(Ssl::write_bio)),
            ..self
        }
    }
}

impl<S: TlsSocket> Tls<'_, S> {
    fn pending(&self) -> usize {
        self.wbio.map_or(0, Bio::pending)
    }

    /// Upstream's `ssl_bio_write`: send everything OpenSSL queued.
    pub(crate) fn bio_write(&self) -> bool {
        let (buf, size) = self.socket.buffer(true);
        let Some(wbio) = self.wbio else {
            return true;
        };
        while self.pending() != 0 {
            // SAFETY: `buf` holds `size` bytes, which the BIO may only write.
            let slots =
                unsafe { core::slice::from_raw_parts_mut(buf.cast::<MaybeUninit<u8>>(), size) };
            let nread = wbio.read(slots);
            if nread <= 0 {
                self.socket.set_err(SSL_BAD_PROTOCOL);
                return false;
            }
            let nread = nread.unsigned_abs() as usize;
            let mut written = 0;
            while written < nread {
                let rest = buf.wrapping_add(written);
                // SAFETY: the first `nread` bytes of `buf` are filled.
                let rest =
                    unsafe { core::slice::from_raw_parts(rest.cast::<u8>(), nread - written) };
                let n = uring::send(self.uring, self.fd, rest, 0, self.write_timeout);
                if n > 0 {
                    written += n.unsigned_abs();
                } else if n == 0 {
                    set_errno(SSL_RESET);
                    return false;
                } else if errno() != libc::EINTR {
                    self.socket.set_err(errno());
                    return false;
                }
            }
        }
        true
    }

    /// Upstream's `ssl_bio_read`: feed OpenSSL one receive.
    pub(crate) fn bio_read(&self) -> bool {
        let (buf, size) = self.socket.buffer(false);
        loop {
            // SAFETY: `buf` holds `size` writable bytes, which the receive may only write.
            let slots =
                unsafe { core::slice::from_raw_parts_mut(buf.cast::<MaybeUninit<u8>>(), size) };
            let rv = uring::recv(self.uring, self.fd, slots, 0, self.read_timeout);
            if rv > 0 {
                let length = rv.unsigned_abs();
                // SAFETY: the first `rv` bytes of `buf` were received.
                let received = unsafe { core::slice::from_raw_parts(buf.cast::<u8>(), length) };
                let rv = c_int::try_from(rv).unwrap_or(c_int::MAX);
                if self.rbio.map_or(-1, |rbio| rbio.write(received)) != rv {
                    self.socket.set_err(SSL_BAD_PROTOCOL);
                    return false;
                }
                return true;
            } else if rv == 0 {
                self.socket.set_err(SSL_RESET);
                return false;
            } else if errno() != libc::EINTR {
                self.socket.set_err(errno());
                return false;
            }
        }
    }

    /// Upstream's `ssl_bio_prepare`.
    pub(crate) fn prepare(&self) -> bool {
        if self.pending() > 0 && !self.bio_write() {
            self.socket.check_failure();
            return false;
        }
        true
    }

    /// Upstream's `ssl_bio_perform` for `rc` from the OpenSSL call named
    /// `function`: move bytes for what OpenSSL waits on.
    pub(crate) fn perform(&self, rc: c_int, function: &CStr) -> bool {
        if !self.prepare() {
            return false;
        }
        let error = self.ssl.map_or(ssl::SSL_ERROR_SSL, |ssl| ssl.error(rc));
        let moved = match error {
            ssl::SSL_ERROR_WANT_WRITE => self.bio_write(),
            ssl::SSL_ERROR_WANT_READ => self.bio_read(),
            ssl::SSL_ERROR_ZERO_RETURN => {
                self.socket.set_err(SSL_RESET);
                false
            }
            ssl::SSL_ERROR_SYSCALL => false,
            _ => {
                let code = swoole_rs_tls::pop_error();
                if code == 0 {
                    self.socket.set_err(SSL_BAD_PROTOCOL);
                } else {
                    self.socket.ssl_error(function, error, code);
                }
                return false;
            }
        };
        if !moved {
            self.socket.check_failure();
        }
        moved
    }

    /// Upstream's `ssl_recv` / `ssl_send` loop around `SSL_read` / `SSL_write`.
    fn transfer(&self, function: &CStr, mut io: impl FnMut() -> c_int) -> isize {
        loop {
            let n = io();
            if !self.prepare() {
                return -1;
            }
            if n > 0 {
                return isize::try_from(n).unwrap_or(-1);
            }
            if !self.perform(n, function) {
                return -1;
            }
        }
    }

    /// `ssl_recv`: decrypted bytes into `buf`, or -1.
    pub(crate) fn recv(&self, buf: &mut [impl ReadSlot]) -> isize {
        let Some(ssl) = self.ssl else {
            return -1;
        };
        self.transfer(c"ssl_recv", || ssl.read(buf))
    }

    /// `ssl_send`: bytes of `buf` encrypted and sent, or -1.
    pub(crate) fn send(&self, buf: &[u8]) -> isize {
        let Some(ssl) = self.ssl else {
            return -1;
        };
        self.transfer(c"ssl_send", || ssl.write(buf))
    }

    /// `send_once` / `recv_once`: `io` is the network socket's `send` or `recv`,
    /// and `want` its `ssl_want_read` (bit 0) and `ssl_want_write` (bit 1).
    /// Without a TLS session it runs once.
    pub(crate) fn once(&self, mut io: impl FnMut() -> isize, want: impl FnMut() -> c_int) -> isize {
        if self.ssl.is_none() {
            return io();
        }
        once(
            io,
            want,
            || self.prepare(),
            || self.bio_read(),
            || self.bio_write(),
        )
    }

    /// `UringSocket::ssl_sendfile`: `size` bytes of `file` from `*offset`,
    /// encrypted ([`crate::sendfile::tls_sendfile`]); the bytes sent and the
    /// warning to log (`crate::sendfile::TLS_WARN_*`, or 0). `classify` is the
    /// socket's `catch_write_error`, given errno.
    pub(crate) fn sendfile(
        &self,
        file: c_int,
        offset: &mut libc::off_t,
        size: usize,
        mut classify: impl FnMut(c_int) -> Option<swoole_rs_reactor::Caught>,
    ) -> (usize, c_int) {
        crate::sendfile::tls_sendfile(
            size,
            offset,
            |buf, at| swoole_rs_corebase::sys::pread(file, buf, at),
            |data| self.send(data),
            || classify(errno()),
        )
    }
}

/// `UringSocket::send_once` / `recv_once`'s retry: one `io` attempt, then the
/// BIO pumps the socket's want flags (`want`: bit 0 read, bit 1 write, read
/// afresh at each check as upstream does) ask for, until `io` moves bytes.
fn once(
    mut io: impl FnMut() -> isize,
    mut want: impl FnMut() -> c_int,
    mut prepare: impl FnMut() -> bool,
    mut read: impl FnMut() -> bool,
    mut write: impl FnMut() -> bool,
) -> isize {
    loop {
        let n = io();
        if n == -1 && want() == 0 {
            return -1;
        }
        if !prepare() {
            return -1;
        }
        if want() & 1 != 0 && !read() {
            return -1;
        }
        if want() & 2 != 0 && !write() {
            return -1;
        }
        if n >= 0 {
            return n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uring::testing::{HOST, with_engine};
    use std::cell::RefCell;
    use swoole_rs_reactor::{Socket, SocketRecord};

    thread_local! {
        static ERRORS: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
        static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0; 64]);
    }
    /// A socket that records its errors.
    #[derive(Clone, Copy)]
    struct Recorder;

    impl TlsSocket for Recorder {
        fn set_err(self, code: c_int) {
            ERRORS.with(|errors| errors.borrow_mut().push(code));
        }
        fn check_failure(self) {
            ERRORS.with(|errors| errors.borrow_mut().push(-1));
        }
        fn buffer(self, _write: bool) -> (*mut c_char, usize) {
            BUFFER.with(|buffer| {
                let mut buffer = buffer.borrow_mut();
                (buffer.as_mut_ptr().cast(), buffer.len())
            })
        }
        fn ssl_error(self, _function: &CStr, _error: c_int, _code: c_ulong) {}
    }

    /// A socket record holding `session`, as a socket's does.
    fn holding(session: *mut ssl::SSL) -> SocketRecord {
        let mut record = SocketRecord::with_fd(-1, 0);
        unsafe { record.set_ssl(session.cast()) };
        record
    }

    /// The view over the session `network` holds.
    fn tls(fd: c_int, network: Socket<'_>) -> Tls<'_, Recorder> {
        let tls = Tls {
            uring: &HOST,
            socket: Recorder,
            ssl: None,
            rbio: None,
            wbio: None,
            fd,
            read_timeout: 1.0,
            write_timeout: 1.0,
        };
        tls.with_session(Ssl::of(network))
    }

    #[test]
    fn once_pumps_the_wanted_bio_until_bytes_move() {
        let steps = RefCell::new(Vec::new());
        let mut attempts = [-1, -1, 5].into_iter();
        let mut wants = [1, 1, 0, 2, 0, 2, 0, 0].into_iter();
        let n = once(
            || attempts.next().unwrap(),
            || wants.next().unwrap(),
            || {
                steps.borrow_mut().push("prepare");
                true
            },
            || {
                steps.borrow_mut().push("read");
                true
            },
            || {
                steps.borrow_mut().push("write");
                true
            },
        );
        assert_eq!(n, 5);
        assert_eq!(
            steps.into_inner(),
            ["prepare", "read", "prepare", "write", "prepare"]
        );
    }

    #[test]
    fn once_fails_without_a_want_or_a_pump() {
        assert_eq!(
            once(|| -1, || 0, || panic!("prepare"), || true, || true),
            -1
        );
        assert_eq!(once(|| -1, || 1, || true, || false, || panic!("write")), -1);
        assert_eq!(once(|| 3, || 0, || false, || true, || true), -1);
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
            let ctx = ssl::SSL_CTX_new(ssl::TLS_method());
            let session = ssl::SSL_new(ctx);
            let (rbio, wbio) = (
                ssl::BIO_new(ssl::BIO_s_mem()),
                ssl::BIO_new(ssl::BIO_s_mem()),
            );
            ssl::SSL_set_bio(session, rbio, wbio);
            let mut record = holding(session);
            let network = Socket::new(&mut record);
            // Queued output, more than one buffer's worth, reaches the peer whole.
            let message = [7_u8; 100];
            assert_eq!(
                ssl::BIO_write(wbio, message.as_ptr().cast(), 100),
                100,
                "queued"
            );
            assert!(tls(fds[0], network).prepare(), "prepare sends");
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
            assert!(tls(fds[0], network).bio_read(), "read");
            let mut out = [0_u8; 8];
            assert_eq!(
                ssl::BIO_read(rbio, out.as_mut_ptr().cast(), 8),
                3,
                "BIO holds the bytes"
            );
            // The peer closing is a TLS reset.
            libc::close(fds[1]);
            assert!(!tls(fds[0], network).bio_read(), "closed");
            assert_eq!(
                ERRORS.with(|errors| errors.borrow().clone()),
                vec![SSL_RESET],
                "reset"
            );
            ssl::SSL_free(session);
            ssl::SSL_CTX_free(ctx);
            libc::close(fds[0]);
        });
    }

    /// Upstream's `uring_socket.ssl_close_notify`: a peer that ends the TLS session
    /// with `close_notify` but keeps the TCP connection open makes the next read fail
    /// with `SW_ERROR_SSL_RESET` instead of leaving errCode
    /// at 0. The sessions use an anonymous cipher in place of core-tests' certificate.
    #[test]
    fn ssl_close_notify() {
        use std::net::{TcpListener, TcpStream};
        use std::os::fd::AsRawFd;
        use std::sync::atomic::{AtomicBool, Ordering};

        unsafe fn context(method: *const ssl::SSL_METHOD) -> *mut ssl::SSL_CTX {
            unsafe {
                let ctx = ssl::SSL_CTX_new(method);
                ssl::SSL_CTX_set_security_level(ctx, 0);
                ssl::SSL_CTX_set_max_proto_version(ctx, ssl::TLS1_2_VERSION);
                assert_eq!(ssl::SSL_CTX_set_cipher_list(ctx, c"aNULL".as_ptr()), 1);
                ctx
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client_done = std::sync::Arc::new(AtomicBool::new(false));
        let server = std::thread::spawn({
            let client_done = client_done.clone();
            move || unsafe {
                let ctx = context(ssl::TLS_server_method());
                let (stream, _) = listener.accept().unwrap();
                let session = ssl::SSL_new(ctx);
                let bio = ssl::BIO_new_socket(stream.as_raw_fd(), 0);
                ssl::SSL_set_bio(session, bio, bio);
                if ssl::SSL_accept(session) == 1 {
                    let mut buf = [0_u8; 16];
                    ssl::SSL_read(session, buf.as_mut_ptr().cast(), 16);
                    ssl::SSL_write(session, b"ok".as_ptr().cast(), 2);
                    // sends close_notify, the TCP connection stays open
                    ssl::SSL_shutdown(session);
                    for _ in 0..5000 {
                        if client_done.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                ssl::SSL_free(session);
                ssl::SSL_CTX_free(ctx);
                drop(stream);
            }
        });

        with_engine(|| unsafe {
            let stream = TcpStream::connect(address).unwrap();
            let ctx = context(ssl::TLS_client_method());
            let session = ssl::SSL_new(ctx);
            let (rbio, wbio) = (
                ssl::BIO_new(ssl::BIO_s_mem()),
                ssl::BIO_new(ssl::BIO_s_mem()),
            );
            ssl::SSL_set_bio(session, rbio, wbio);
            ssl::SSL_set_connect_state(session);
            let mut record = holding(session);
            let tls = tls(stream.as_raw_fd(), Socket::new(&mut record));
            loop {
                let rc = ssl::SSL_do_handshake(session);
                if rc == 1 {
                    break;
                }
                assert!(tls.perform(rc, c"ssl_connect"), "connect");
            }
            assert_eq!(
                ERRORS.with(|errors| errors.borrow().clone()),
                [],
                "errCode 0"
            );

            assert_eq!(tls.send(b"ping"), 4);

            let mut buf = [0_u8; 16];
            assert_eq!(tls.recv(&mut buf), 2);

            // the next read sees the close_notify
            assert!(tls.recv(&mut buf) <= 0);
            assert_eq!(
                ERRORS.with(|errors| errors.borrow().clone()),
                [SSL_RESET, -1],
                "errCode is the reset, then check_return_value(-1)"
            );

            client_done.store(true, Ordering::SeqCst);
            ssl::SSL_free(session);
            ssl::SSL_CTX_free(ctx);
        });
        client_done.store(true, Ordering::SeqCst);
        server.join().unwrap();
    }
}
