//! `UringSocket`'s operations, whole: the availability check, binding the
//! current coroutine to the direction, the `io_uring` request (or the TLS transfer
//! over it), the result check, and the loops built on them (`recv_all`,
//! `send_all`, `readv`/`writev` and their `_all` forms, the TLS vector loops),
//! plus `accept`, `connect`, `poll`, `sendfile`, `send_once`/`recv_once` and the
//! TLS handshake's steps. A `UringSocket` is a `coroutine::Socket` its
//! constructors mark ([`crate::socket_extra::swr_v1_hook_co_socket_uring`]); the
//! socket's own members (`socket_members.rs`, `socket_tls.rs`, `lifecycle.rs`)
//! take these paths for it.
use crate::co_io::{available, check, owned_err, set_err};
use crate::co_socket::Co;
use crate::hooked::{bytes, bytes_mut};
use crate::uring::{self, Host};
use crate::uring_tls::{SSL_BAD_PROTOCOL, Tls, TlsSocket};
use core::ffi::{CStr, c_char, c_int, c_ulong, c_void};
use openssl_sys as ssl;
use swoole_rs_corebase::log;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::{
    ReactorIov, SocketRecord, swr_v1_reactor_iov_advance, swr_v1_reactor_iov_view,
};

unsafe extern "C" {
    fn ERR_error_string_n(error: c_ulong, buffer: *mut c_char, length: usize);
}

const EVENT_READ: u32 = 1 << 9;
const EVENT_WRITE: u32 = 1 << 10;
const EVENT_RDWR: u32 = EVENT_READ | EVENT_WRITE;

/// The socket's view and effects.
pub struct UringSocketOps {
    pub socket: Co,
    /// The socket's TLS view; its `ssl` is null without TLS, and it carries the
    /// descriptor, the timeouts and the ring's host.
    pub tls: Tls<Co>,
    pub connect_timeout: f64,
    /// `is_available(event)`.
    pub available: fn(Co, u32) -> bool,
    /// `check_return_value(retval)`.
    pub check: fn(Co, isize),
    /// `set_err(code)`.
    pub set_err: fn(Co, c_int),
}

/// A `network::IOVector`'s owner: its current iovec and how many remain, and
/// advancing it past a result.
pub struct IoVector(pub *mut ReactorIov);

impl IoVector {
    fn current(&self) -> (*const libc::iovec, c_int) {
        // SAFETY: the caller's live vector.
        let view = swr_v1_reactor_iov_view(unsafe { &*self.0 });
        (view.iterator, view.remain_count)
    }

    fn update(&self, result: isize) {
        // SAFETY: the caller's live vector.
        unsafe { swr_v1_reactor_iov_advance(self.0, result) };
    }
}

impl TlsSocket for Co {
    fn set_err(self, code: c_int) {
        set_err(self, code);
    }

    fn check_failure(self) {
        check(self, -1);
    }

    fn buffer(self, write: bool) -> (*mut c_char, usize) {
        self.with_buffer(write, |buffer| (buffer.as_mut_ptr().cast(), buffer.size()))
    }

    /// `ssl_bio_perform`'s failure: OpenSSL's text for `code`, logged with the
    /// peer and set as the socket's error.
    fn ssl_error(self, function: &CStr, error: c_int, code: c_ulong) {
        let mut text = [0 as c_char; 512];
        // SAFETY: at most the buffer's length, NUL-terminated.
        unsafe { ERR_error_string_n(code, text.as_mut_ptr(), text.len()) };
        // SAFETY: NUL-terminated by ERR_error_string_n.
        let text = unsafe { CStr::from_ptr(text.as_ptr()) };
        let function = function.to_string_lossy();
        let message = text.to_string_lossy();
        self.net(|network| {
            let (host, port) = swoole_rs_tls::upstream::peer(&network.info);
            log::notice(format_args!(
                "{function}(fd={}) to server[{host}:{port}] failed. Error: {message}[{error}|{}]",
                network.fd,
                swoole_rs_tls::error_reason(code)
            ));
        });
        owned_err(self, SSL_BAD_PROTOCOL, text.to_bytes());
    }
}

/// The ring's view of `socket` when it is a `UringSocket`, else none.
pub(crate) fn of(socket: Co) -> Option<UringSocketOps> {
    let (uring, [rbio, wbio]) = socket.with_extra(|extra| (extra.uring, extra.bio))?;
    let uring = uring?;
    if socket.socket().is_null() {
        return None;
    }
    socket.net(|network| UringSocketOps {
        socket,
        tls: Tls {
            uring,
            socket,
            ssl: network.ssl().cast(),
            rbio: rbio.cast(),
            wbio: wbio.cast(),
            fd: network.fd,
            read_timeout: network.timeouts.read,
            write_timeout: network.timeouts.write,
        },
        connect_timeout: network.timeouts.connect,
        available,
        check,
        set_err,
    })
}

/// The scalar operations of [`UringSocketOps::io`].
pub const READ: c_int = 0;
pub const WRITE: c_int = 1;
pub const RECV: c_int = 2;
pub const SEND: c_int = 3;
pub const RECVMSG: c_int = 4;
pub const SENDMSG: c_int = 5;
pub const RECVFROM: c_int = 6;
pub const SENDTO: c_int = 7;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl UringSocketOps {
    fn host(&self) -> &'static Host {
        self.tls.uring
    }

    fn ssl(&self) -> bool {
        !self.tls.ssl.is_null()
    }

    fn available(&self, event: u32) -> bool {
        (self.available)(self.socket, event)
    }

    fn check(&self, result: isize) {
        (self.check)(self.socket, result);
    }

    /// `call` with the current coroutine bound to `event`.
    fn bound<T>(&self, event: u32, call: impl FnOnce() -> T) -> T {
        let co = (self.host().current)();
        self.socket.with_state(|state| state.bind(event, co));
        let result = call();
        self.socket
            .with_state(|state| state.bind(event, core::ptr::null_mut()));
        result
    }

    /// One scalar operation: `op` is 0 read, 1 write, 2 recv, 3 send (TLS when the
    /// socket has it), 4 recvmsg and 5 sendmsg (`extra` the msghdr, with `flags`),
    /// 6 recvfrom (`extra`/`extra_length` the address), 7 sendto (to that address,
    /// without a second availability check).
    ///
    /// # Safety
    /// The buffers are valid as for the system call.
    pub(crate) unsafe fn io(
        &self,
        op: c_int,
        buf: *mut c_void,
        n: usize,
        extra: *mut c_void,
        extra_length: *mut libc::socklen_t,
        flags: c_int,
    ) -> isize {
        let read = matches!(op, READ | RECV | RECVMSG | RECVFROM);
        let event = if read { EVENT_READ } else { EVENT_WRITE };
        // sendto checked availability before resolving the host.
        if op != SENDTO && !self.available(event) {
            return -1;
        }
        // SAFETY: the caller's buffers, valid as for the system call.
        let result = self.bound(event, || unsafe {
            self.request(op, buf, n, extra, extra_length, flags)
        });
        self.check(result);
        result
    }

    /// The request itself.
    ///
    /// # Safety
    /// As [`UringSocketOps::io`].
    unsafe fn request(
        &self,
        op: c_int,
        buf: *mut c_void,
        n: usize,
        extra: *mut c_void,
        extra_length: *mut libc::socklen_t,
        flags: c_int,
    ) -> isize {
        let (host, tls) = (self.host(), self.tls);
        let (fd, rt, wt) = (tls.fd, tls.read_timeout, tls.write_timeout);
        match op {
            // SAFETY: the caller's buffer (each arm alike).
            READ => unsafe { uring::read(host, fd, buf, n, rt) },
            // SAFETY: as above.
            WRITE => unsafe { uring::write(host, fd, buf, n, wt) },
            // SAFETY: as above.
            RECV if self.ssl() => tls.recv(unsafe { bytes_mut(buf, n) }),
            // SAFETY: as above.
            RECV => unsafe { uring::recv(host, fd, buf, n, 0, rt) },
            // SAFETY: as above.
            SEND if self.ssl() => tls.send(unsafe { bytes(buf, n) }),
            // SAFETY: as above.
            SEND => unsafe { uring::send(host, fd, buf, n, 0, wt) },
            // SAFETY: as above.
            RECVMSG => unsafe { uring::recvmsg(host, fd, extra.cast(), flags, rt) },
            // SAFETY: as above.
            SENDMSG => unsafe { uring::sendmsg(host, fd, extra.cast(), flags, wt) },
            SENDTO => {
                // SAFETY: the caller's address length.
                let length = unsafe { *extra_length };
                // SAFETY: the caller's buffer and address.
                unsafe { uring::sendto(host, fd, buf, n, 0, extra.cast(), length, wt) }
            }
            // SAFETY: as above.
            _ => unsafe { uring::recvfrom(host, fd, buf, n, extra.cast(), extra_length, rt) },
        }
    }

    /// `ssl_readv`/`ssl_writev`: TLS transfers over each iovec while they move data.
    fn tls_vector(&self, vector: &IoVector, write: bool) -> isize {
        let mut total = 0isize;
        loop {
            let (iov, _) = vector.current();
            // SAFETY: the vector's current iovec.
            let iov = unsafe { *iov };
            let result = if write {
                // SAFETY: the iovec's buffer.
                self.tls.send(unsafe { bytes(iov.iov_base, iov.iov_len) })
            } else {
                // SAFETY: the iovec's buffer.
                self.tls
                    .recv(unsafe { bytes_mut(iov.iov_base, iov.iov_len) })
            };
            total += result.max(0);
            vector.update(result);
            if !(result > 0 && vector.current().1 > 0) {
                return if total > 0 { total } else { result };
            }
        }
    }

    /// `readv`/`writev`.
    fn vector(&self, vector: &IoVector, write: bool) -> isize {
        let event = if write { EVENT_WRITE } else { EVENT_READ };
        if !self.available(event) {
            return -1;
        }
        let (host, fd) = (self.host(), self.tls.fd);
        let result = self.bound(event, || {
            loop {
                let result = if self.ssl() {
                    self.tls_vector(vector, write)
                } else {
                    let (iov, count) = vector.current();
                    if write {
                        // SAFETY: the vector's remaining iovecs.
                        unsafe { uring::writev(host, fd, iov, count, self.tls.write_timeout) }
                    } else {
                        // SAFETY: the vector's remaining iovecs.
                        unsafe { uring::readv(host, fd, iov, count, self.tls.read_timeout) }
                    }
                };
                vector.update(result);
                if !(result < 0 && errno() == libc::EINTR) {
                    break result;
                }
            }
        });
        self.check(result);
        result
    }
}

/// `read` into `buf`, which may be uninitialized: the ring writes it.
pub(crate) fn read(ops: &UringSocketOps, buf: &mut [impl ReadSlot]) -> isize {
    into(ops, READ, buf)
}

/// `recv` into `buf`, which TLS reads as bytes.
pub(crate) fn recv(ops: &UringSocketOps, buf: &mut [u8]) -> isize {
    into(ops, RECV, buf)
}

/// `op` ([`READ`], or [`RECV`] over initialized bytes) into `buf`.
fn into(ops: &UringSocketOps, op: c_int, buf: &mut [impl ReadSlot]) -> isize {
    let (at, n) = (buf.as_mut_ptr().cast(), buf.len());
    // SAFETY: `buf` is `n` writable bytes.
    unsafe { ops.io(op, at, n, core::ptr::null_mut(), core::ptr::null_mut(), 0) }
}

/// `write` (`op` [`WRITE`]) or `send` ([`SEND`]) from `buf`.
pub(crate) fn send(ops: &UringSocketOps, op: c_int, buf: &[u8]) -> isize {
    let (at, n) = (buf.as_ptr().cast_mut().cast(), buf.len());
    // SAFETY: `buf` is `n` readable bytes, which a write only reads.
    unsafe { ops.io(op, at, n, core::ptr::null_mut(), core::ptr::null_mut(), 0) }
}

/// `recv_all` into `buf`: recv until it is full, it failed, or the peer closed.
pub(crate) fn recv_all(ops: &UringSocketOps, buf: &mut [u8]) -> isize {
    all(ops, buf.len(), |at| {
        recv(ops, buf.get_mut(at..).unwrap_or_default())
    })
}

/// `send_all` of `buf`: send until all of it went or it failed.
pub(crate) fn send_all(ops: &UringSocketOps, buf: &[u8]) -> isize {
    all(ops, buf.len(), |at| {
        send(ops, SEND, buf.get(at..).unwrap_or_default())
    })
}

/// `step(moved)` again while it moves bytes and fewer than `n` moved.
fn all(ops: &UringSocketOps, n: usize, mut step: impl FnMut(usize) -> isize) -> isize {
    let mut total = 0usize;
    let result = loop {
        let result = step(total);
        total += result.max(0).unsigned_abs();
        if !(result > 0 && total < n) {
            break result;
        }
    };
    ops.check(result);
    transfer_result(result, total)
}

/// -1 when it failed before moving anything, else the bytes moved.
fn transfer_result(result: isize, total: usize) -> isize {
    if result < 0 && total == 0 {
        -1
    } else {
        total.cast_signed()
    }
}

/// `readv`/`writev`, or with `all` their `_all` forms: again while each moves data
/// and some remains.
///
/// # Safety
/// `vector` is live.
pub(crate) unsafe fn swr_v1_hook_uring_vector(
    ops: &UringSocketOps,
    vector: &IoVector,
    write: bool,
    all: bool,
) -> isize {
    if !all {
        return ops.vector(vector, write);
    }
    let mut total = 0usize;
    let result = loop {
        let result = ops.vector(vector, write);
        if result <= 0 {
            break result;
        }
        total += result.unsigned_abs();
        if vector.current().1 <= 0 {
            break result;
        }
    };
    ops.check(result);
    transfer_result(result, total)
}

/// `accept`'s request: the ring's accept into `addr`/`length` (close-on-exec,
/// non-blocking) under the read timeout, reading bound to the current coroutine.
/// A failure sets the socket's error from errno.
///
/// # Safety
/// `addr`/`length` are valid as for accept(2).
pub(crate) unsafe fn swr_v1_hook_uring_accept(
    ops: &UringSocketOps,
    addr: *mut libc::sockaddr,
    length: *mut libc::socklen_t,
) -> c_int {
    let (host, tls) = (ops.host(), ops.tls);
    let flags = libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK;
    // SAFETY: the caller's address buffer.
    let fd = ops.bound(EVENT_READ, || unsafe {
        uring::accept(host, tls.fd, addr, length, flags, tls.read_timeout)
    });
    if fd < 0 {
        (ops.set_err)(ops.socket, errno());
    }
    fd
}

/// `connect(addr, len)` through the ring under the connect timeout.
///
/// # Safety
/// `addr` holds `length` bytes.
pub(crate) unsafe fn swr_v1_hook_uring_connect(
    ops: &UringSocketOps,
    addr: *const libc::sockaddr,
    length: libc::socklen_t,
) -> bool {
    if !ops.available(EVENT_RDWR) {
        return false;
    }
    let (host, fd) = (ops.host(), ops.tls.fd);
    // SAFETY: the caller's address.
    let result = ops.bound(EVENT_RDWR, || unsafe {
        uring::connect(host, fd, addr, length, ops.connect_timeout)
    });
    if result < 0 {
        return false;
    }
    ops.socket.with_state(|state| state.flags.connected = true);
    // A failure is ignored, as upstream does.
    ops.socket.net(swoole_rs_reactor::SocketFields::get_name);
    (ops.set_err)(ops.socket, 0);
    true
}

/// `poll(type, timeout)`: one descriptor for `events` (poll(2) bits), within
/// `timeout` seconds (0 checks without the ring, negative waits indefinitely).
pub(crate) fn swr_v1_hook_uring_poll(
    ops: &UringSocketOps,
    event: u32,
    events: libc::c_short,
    timeout: f64,
) -> bool {
    if !ops.available(event) {
        return false;
    }
    let mut fd = libc::pollfd {
        fd: ops.tls.fd,
        events,
        revents: 0,
    };
    // Upstream converts the (milliseconds) double to the int parameter.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's implicit double-to-int conversion"
    )]
    let milliseconds = if timeout > 0.0 {
        timeout * 1000.0
    } else {
        timeout
    } as c_int;
    let rc = uring::poll(ops.host(), &mut fd, milliseconds);
    if rc != 1 {
        let error = if rc == 0 { libc::ETIMEDOUT } else { errno() };
        (ops.set_err)(ops.socket, error);
        return false;
    }
    true
}

/// `UringSocket::accept`'s connection: the ring's accept (under the read timeout,
/// not the caller's, as upstream) as a new `UringSocket`, or none with the error
/// set. `socket` is available, in a coroutine, and `ops` is its view.
pub(crate) fn accept(socket: Co, ops: &UringSocketOps) -> Option<Co> {
    let mut conn = SocketRecord::new();
    // SAFETY: the new record's address.
    let fd = unsafe {
        swr_v1_hook_uring_accept(
            ops,
            (&raw mut conn.info.addr).cast(),
            &raw mut conn.info.len,
        )
    };
    if fd < 0 {
        return None;
    }
    conn.fd = fd;
    conn.removed = 1;
    conn.info.kind = socket
        .net(|listener| listener.socket_type)
        .unwrap_or_default();
    conn.output.nonblock = 1;
    conn.cloexec = 1;
    // SAFETY: the record the listening socket accepted.
    let client = unsafe { socket.adopt(conn.into_raw()) };
    crate::socket_extra::swr_v1_hook_co_socket_uring(client);
    Some(client)
}

/// `UringSocket::sendfile`'s transfer once the file is open: as upstream, `end`
/// bytes from `offset`, through the TLS session when there is one.
pub(crate) fn sendfile(ops: &UringSocketOps, file: c_int, offset: libc::off_t, end: usize) -> bool {
    let mut offset = offset;
    let sent = ops.bound(EVENT_WRITE, || {
        if !ops.ssl() {
            // MSG_SPLICE_PAGES is kernel-internal: the build's headers lack it, so
            // upstream splices through a pipe.
            return uring::sendfile(
                ops.host(),
                ops.tls.fd,
                file,
                Some(&mut offset),
                end,
                ops.tls.write_timeout,
                0,
            );
        }
        let (total, warn) = ops.tls.sendfile(file, &mut offset, end, |error| {
            ops.socket.catch_error(error)
        });
        match warn {
            1 => log::sys_warning(c"UringSocket::ssl_sendfile", format_args!("pread() failed")),
            2 => log::sys_warning(c"UringSocket::ssl_sendfile", format_args!("write() failed")),
            _ => {}
        }
        total.cast_signed()
    });
    sent == end.cast_signed()
}

/// The socket's `ssl_want_read` (bit 0) and `ssl_want_write` (bit 1).
fn want(socket: Co) -> c_int {
    let tls = socket.net(|network| network.tls);
    tls.map_or(0, |tls| {
        c_int::from(tls.want_read == 1) | (c_int::from(tls.want_write == 1) << 1)
    })
}

/// `UringSocket::send_once` or `recv_once`: `io`, the open socket's one attempt on
/// its `network::Socket`, with the TLS BIOs pumped through the ring until it moves
/// bytes.
pub(crate) fn once(ops: &UringSocketOps, io: impl FnMut() -> isize) -> isize {
    ops.tls.once(io, || want(ops.socket))
}

/// `UringSocket::poll(event, timeout)`, in a coroutine.
pub(crate) fn poll(ops: &UringSocketOps, event: u32, timeout: f64) -> bool {
    let mut events = 0;
    if event & EVENT_READ != 0 {
        events |= libc::POLLIN;
    }
    if event & EVENT_WRITE != 0 {
        events |= libc::POLLOUT;
    }
    swr_v1_hook_uring_poll(ops, event, events, timeout)
}

/// `UringSocket::ssl_handshake`'s session: `create` makes the socket's own, then
/// it runs over a pair of memory BIOs the ring pumps, on the socket's side.
///
/// # Safety
/// `socket` is open, with an `SSLContext`.
pub(crate) unsafe fn tls_session(socket: Co, create: impl FnOnce() -> bool) -> bool {
    if !create() {
        return false;
    }
    let session = socket
        .net(|network| network.ssl())
        .unwrap_or(core::ptr::null_mut())
        .cast::<ssl::SSL>();
    // SAFETY: OpenSSL's static memory BIO method.
    let method = unsafe { ssl::BIO_s_mem() };
    // SAFETY: as above (each call alike).
    let rbio = unsafe { ssl::BIO_new(method) };
    // SAFETY: as above.
    let wbio = unsafe { ssl::BIO_new(method) };
    // SAFETY: the live session takes both BIOs.
    unsafe { ssl::SSL_set_bio(session, rbio, wbio) };
    socket.with_extra(|extra| extra.bio = [rbio.cast(), wbio.cast()]);
    if socket.ssl_is_server() {
        // SAFETY: the live session.
        unsafe { ssl::SSL_set_accept_state(session) };
    } else {
        // SAFETY: the live session.
        unsafe { ssl::SSL_set_connect_state(session) };
    }
    true
}

/// One `UringSocket` handshake step through the ring: 1 done, 0 again, -1 failed.
///
/// # Safety
/// `socket` is a `UringSocket` with a session from [`tls_session`].
pub(crate) unsafe fn tls_step(socket: Co) -> c_int {
    let Some(ops) = of(socket) else {
        return -1;
    };
    // SAFETY: the live session.
    let rc = unsafe { ssl::SSL_do_handshake(ops.tls.ssl) };
    if rc == 1 {
        return 1;
    }
    let function = if socket.ssl_is_server() {
        c"ssl_accept"
    } else {
        c"ssl_connect"
    };
    if ops.tls.perform(rc, function) { 0 } else { -1 }
}

#[cfg(test)]
mod ported;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::TestSocket;
    use crate::uring::testing::{HOST, with_engine};
    use std::cell::RefCell;

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static AVAILABLE: RefCell<bool> = const { RefCell::new(true) };
    }

    fn log(entry: String) {
        LOG.with(|log| log.borrow_mut().push(entry));
    }
    fn logged() -> Vec<String> {
        LOG.with(|log| log.replace(Vec::new()))
    }
    fn available(_: Co, event: u32) -> bool {
        log(format!("available {event}"));
        AVAILABLE.with(|a| *a.borrow())
    }
    fn check(_: Co, result: isize) {
        log(format!("check {result}"));
    }
    fn set_err(_: Co, code: c_int) {
        log(format!("err {code}"));
    }
    fn ops(fd: c_int, socket: Co) -> UringSocketOps {
        UringSocketOps {
            socket,
            tls: Tls {
                uring: &HOST,
                socket,
                ssl: core::ptr::null_mut(),
                rbio: core::ptr::null_mut(),
                wbio: core::ptr::null_mut(),
                fd,
                read_timeout: 1.0,
                write_timeout: 1.0,
            },
            connect_timeout: 1.0,
            available,
            check,
            set_err,
        }
    }

    #[test]
    fn transfers_bind_and_check() {
        with_engine(|| {
            let mut fds = [0; 2];
            // SAFETY: a local pair.
            let paired =
                unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
            assert_eq!(paired, 0);
            let mut socket = TestSocket::new();
            let (a, b) = (ops(fds[0], socket.co()), ops(fds[1], socket.co()));
            let mut message = *b"hello world";
            let mut received = [0u8; 11];
            assert_eq!(send_all(&a, &message), 11);
            assert_eq!(recv_all(&b, &mut received), 11);
            assert_eq!(&received, b"hello world");
            let bound = socket.co().with_state(|state| state.bound_co(EVENT_READ));
            assert!(bound.is_null(), "unbound after the call");
            let read = format!("available {EVENT_READ}");
            let write = format!("available {EVENT_WRITE}");
            assert_eq!(
                logged(),
                [
                    &*write, "check 11", "check 11", &*read, "check 11", "check 11"
                ]
            );
            // An unavailable socket stops before the request.
            AVAILABLE.with(|v| *v.borrow_mut() = false);
            let result = super::read(&a, &mut message[..1]);
            AVAILABLE.with(|v| *v.borrow_mut() = true);
            assert_eq!(result, -1);
            assert_eq!(logged(), std::slice::from_ref(&read));
            // A poll with nothing to read times out.
            let polled = swr_v1_hook_uring_poll(&a, EVENT_READ, libc::POLLIN, 0.01);
            assert!(!polled);
            assert_eq!(logged(), [read, format!("err {}", libc::ETIMEDOUT)]);
            // SAFETY: the pair made above.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
        });
    }

    #[test]
    fn accepts_and_sends_to_an_address() {
        use std::net::{TcpListener, TcpStream, UdpSocket};
        use std::os::fd::AsRawFd;
        with_engine(|| {
            let mut socket = TestSocket::new();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let server = ops(listener.as_raw_fd(), socket.co());
            // SAFETY: zeroed storage is a valid sockaddr_storage.
            let mut addr: libc::sockaddr_storage = unsafe { core::mem::zeroed() };
            let mut length = libc::socklen_t::try_from(core::mem::size_of_val(&addr)).unwrap();
            // SAFETY: a local table and address buffer.
            let fd = unsafe {
                swr_v1_hook_uring_accept(&server, (&raw mut addr).cast(), &raw mut length)
            };
            assert!(fd >= 0);
            assert_eq!(
                addr.ss_family,
                libc::sa_family_t::try_from(libc::AF_INET).unwrap()
            );
            // SAFETY: the accepted descriptor.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert_ne!(flags & libc::O_NONBLOCK, 0, "accepted non-blocking");
            // SAFETY: as above.
            unsafe { libc::close(fd) };
            drop(client);

            let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
            let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
            let to = ops(sender.as_raw_fd(), socket.co());
            let target = libc::sockaddr_in {
                sin_family: libc::sa_family_t::try_from(libc::AF_INET).unwrap(),
                sin_port: receiver.local_addr().unwrap().port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
                },
                sin_zero: [0; 8],
            };
            let mut target_length =
                libc::socklen_t::try_from(core::mem::size_of_val(&target)).unwrap();
            let mut message = *b"ping";
            // SAFETY: a local table, buffer and address.
            let sent = unsafe {
                to.io(
                    SENDTO,
                    message.as_mut_ptr().cast(),
                    4,
                    (&raw const target).cast_mut().cast(),
                    &raw mut target_length,
                    0,
                )
            };
            assert_eq!(sent, 4);
            let mut received = [0u8; 4];
            assert_eq!(receiver.recv(&mut received).unwrap(), 4);
            assert_eq!(&received, b"ping");
            // sendto checks availability itself, before resolving the host.
            assert_eq!(logged(), ["check 4"]);
        });
    }
}
