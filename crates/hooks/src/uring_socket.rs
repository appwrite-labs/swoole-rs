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
use crate::uring::{self, Host};
use crate::uring_tls::{SSL_BAD_PROTOCOL, Tls, TlsSocket};
use crate::vector::{IoVector, Lent, RecvMessage, SendMessage};
use core::ffi::{CStr, c_char, c_int, c_ulong};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::{SocketFields, SocketRecord};
use swoole_rs_tls::{OwnedBio, Ssl};

const EVENT_READ: u32 = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as u32;
const EVENT_WRITE: u32 = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as u32;
const EVENT_RDWR: u32 = EVENT_READ | EVENT_WRITE;

/// The socket's view and effects.
pub struct UringSocketOps<'a> {
    pub socket: Co,
    /// The socket's TLS view; its `ssl` is null without TLS, and it carries the
    /// descriptor, the timeouts and the ring's host.
    pub tls: Tls<'a, Co>,
    pub connect_timeout: f64,
    /// `is_available(event)`.
    pub available: fn(Co, u32) -> bool,
    /// `check_return_value(retval)`.
    pub check: fn(Co, isize),
    /// `set_err(code)`.
    pub set_err: fn(Co, c_int),
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
        let message = swoole_rs_tls::error_string(code);
        let function = function.to_string_lossy();
        self.net(|network| {
            let (host, port) = swoole_rs_tls::upstream::peer(&network.info);
            log::notice(format_args!(
                "{function}(fd={}) to server[{host}:{port}] failed. Error: {message}[{error}|{}]",
                network.fd,
                swoole_rs_tls::error_reason(code)
            ));
        });
        owned_err(self, SSL_BAD_PROTOCOL, message.as_bytes());
    }
}

/// `f` over the ring's view of `socket` when it is a `UringSocket`, else none. The
/// view lives for the call: it borrows the record's TLS session, which the unsafe
/// `socket_ssl_close` frees only between the socket's operations.
pub(crate) fn with<T>(socket: Co, f: impl FnOnce(&UringSocketOps<'_>) -> T) -> Option<T> {
    let uring = socket.with_extra(|extra| extra.uring)??;
    socket.with_network(|network| {
        let read = |record: &SocketFields| (record.fd, record.timeouts);
        let (ssl, (fd, timeouts)) = Ssl::of_reading(network, read);
        let tls = Tls {
            uring,
            socket,
            ssl: None,
            rbio: None,
            wbio: None,
            fd,
            read_timeout: timeouts.read,
            write_timeout: timeouts.write,
        };
        f(&UringSocketOps {
            socket,
            tls: tls.with_session(ssl),
            connect_timeout: timeouts.connect,
            available,
            check,
            set_err,
        })
    })
}

/// The scalar operations: [`read`], [`recv`], [`send`] and those of
/// [`UringSocketOps::io`].
pub const READ: c_int = 0;
pub const WRITE: c_int = 1;
pub const RECV: c_int = 2;
pub const SEND: c_int = 3;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl UringSocketOps<'_> {
    fn host(&self) -> &'static Host {
        self.tls.uring
    }

    fn ssl(&self) -> bool {
        self.tls.ssl.is_some()
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

    /// `request` for `event`: the availability check (unless `checked` before),
    /// the current coroutine bound to `event`, then the result check.
    fn transfer(&self, event: u32, checked: bool, request: impl FnOnce() -> isize) -> isize {
        if !checked && !self.available(event) {
            return -1;
        }
        let result = self.bound(event, request);
        self.check(result);
        result
    }

    /// `recvmsg` into `message` with `flags`.
    pub(crate) fn recvmsg(&self, message: &mut RecvMessage<'_>, flags: c_int) -> isize {
        let (host, tls) = (self.host(), self.tls);
        self.transfer(EVENT_READ, false, || {
            uring::recvmsg(host, tls.fd, message, flags, tls.read_timeout)
        })
    }

    /// `sendmsg` of `message` with `flags`.
    pub(crate) fn sendmsg(&self, message: &SendMessage<'_>, flags: c_int) -> isize {
        let (host, tls) = (self.host(), self.tls);
        self.transfer(EVENT_WRITE, false, || {
            uring::sendmsg(host, tls.fd, message, flags, tls.write_timeout)
        })
    }

    /// `readv`/`writev`. The transfer borrows the vector, so it advances here, once
    /// per transfer, after that borrow ends.
    fn vector(&self, vector: &mut IoVector<'_>) -> isize {
        let write = vector.write();
        let event = if write { EVENT_WRITE } else { EVENT_READ };
        let (host, tls) = (self.host(), self.tls);
        let timeout = if write {
            tls.write_timeout
        } else {
            tls.read_timeout
        };
        self.transfer(event, false, || {
            loop {
                let result = if self.ssl() {
                    vector.transfer_each(|buffer| match buffer {
                        Lent::Write(bytes) => tls.send(bytes),
                        Lent::Read(slots) => tls.recv(slots),
                    })
                } else {
                    uring::vector(host, tls.fd, vector, timeout)
                };
                vector.update(result);
                if !(result < 0 && errno() == libc::EINTR) {
                    break result;
                }
            }
        })
    }
}

/// `read` into `buf`, which may be uninitialized: the ring writes it.
pub(crate) fn read(ops: &UringSocketOps<'_>, buf: &mut [impl ReadSlot]) -> isize {
    let (host, tls) = (ops.host(), ops.tls);
    ops.transfer(EVENT_READ, false, || {
        uring::read(host, tls.fd, buf, tls.read_timeout)
    })
}

/// `recv` into `buf`, which may be uninitialized: the ring or `SSL_read` writes it.
pub(crate) fn recv(ops: &UringSocketOps<'_>, buf: &mut [impl ReadSlot]) -> isize {
    let (host, tls) = (ops.host(), ops.tls);
    ops.transfer(EVENT_READ, false, || {
        if ops.ssl() {
            return tls.recv(buf);
        }
        uring::recv(host, tls.fd, buf, 0, tls.read_timeout)
    })
}

/// `recvfrom` into `buf` and the caller's `peer`: a peek through the ring, then the
/// datagram itself without blocking. The result and the length of the peer's address.
pub(crate) fn recvfrom_into(
    ops: &UringSocketOps<'_>,
    buf: &mut [impl ReadSlot],
    peer: &mut libc::sockaddr_storage,
) -> (isize, libc::socklen_t) {
    let (host, tls) = (ops.host(), ops.tls);
    let mut size = swoole_rs_corebase::sys::socklen::<libc::sockaddr_storage>();
    let received = ops.transfer(EVENT_READ, false, || {
        let peeked = uring::recv(host, tls.fd, buf, libc::MSG_PEEK, tls.read_timeout);
        if peeked <= 0 {
            return peeked;
        }
        let (at, n) = (buf.as_mut_ptr().cast(), buf.len());
        let (addr, flags) = (core::ptr::from_mut(peer).cast(), libc::MSG_DONTWAIT);
        // SAFETY: `buf`'s writable bytes, and the borrowed storage with its own size.
        unsafe { libc::recvfrom(tls.fd, at, n, flags, addr, &raw mut size) }
    });
    (received, size)
}

/// `recvfrom` into `buf`, the sender into the socket's own peer address: a peek
/// through the ring, then the datagram itself without blocking.
pub(crate) fn recvfrom_peer(ops: &UringSocketOps<'_>, buf: &mut [impl ReadSlot]) -> isize {
    let (host, tls) = (ops.host(), ops.tls);
    ops.transfer(EVENT_READ, false, || {
        let peeked = uring::recv(host, tls.fd, buf, libc::MSG_PEEK, tls.read_timeout);
        if peeked <= 0 {
            return peeked;
        }
        let receive =
            |record: &mut SocketFields| record.info.recvfrom(tls.fd, buf, libc::MSG_DONTWAIT);
        ops.socket.net(receive).unwrap_or(-1)
    })
}

/// `sendto` of `buf` to `address`. The caller checked availability, before resolving
/// the host.
pub(crate) fn sendto(ops: &UringSocketOps<'_>, buf: &[u8], address: &SocketAddress) -> isize {
    let (host, tls) = (ops.host(), ops.tls);
    ops.transfer(EVENT_WRITE, true, || {
        uring::sendto(host, tls.fd, buf, address, tls.write_timeout)
    })
}

/// `write` (`op` [`WRITE`]) or `send` ([`SEND`], through TLS when the socket has
/// it) from `buf`.
pub(crate) fn send(ops: &UringSocketOps<'_>, op: c_int, buf: &[u8]) -> isize {
    let (host, tls) = (ops.host(), ops.tls);
    let wt = tls.write_timeout;
    ops.transfer(EVENT_WRITE, false, || match op {
        WRITE => uring::write(host, tls.fd, buf, wt),
        _ if ops.ssl() => tls.send(buf),
        _ => uring::send(host, tls.fd, buf, 0, wt),
    })
}

/// `recv_all` into `buf`: recv until it is full, it failed, or the peer closed.
pub(crate) fn recv_all(ops: &UringSocketOps<'_>, buf: &mut [impl ReadSlot]) -> isize {
    all(ops, buf.len(), |at| {
        recv(ops, buf.get_mut(at..).unwrap_or_default())
    })
}

/// `send_all` of `buf`: send until all of it went or it failed.
pub(crate) fn send_all(ops: &UringSocketOps<'_>, buf: &[u8]) -> isize {
    all(ops, buf.len(), |at| {
        send(ops, SEND, buf.get(at..).unwrap_or_default())
    })
}

/// `step(moved)` again while it moves bytes and fewer than `n` moved.
fn all(ops: &UringSocketOps<'_>, n: usize, mut step: impl FnMut(usize) -> isize) -> isize {
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
pub(crate) fn swr_v1_hook_uring_vector(
    ops: &UringSocketOps<'_>,
    vector: &mut IoVector<'_>,
    all: bool,
) -> isize {
    if !all {
        return ops.vector(vector);
    }
    let mut total = 0usize;
    let result = loop {
        let result = ops.vector(vector);
        if result <= 0 {
            break result;
        }
        total += result.unsigned_abs();
        if vector.iovecs().is_empty() {
            break result;
        }
    };
    ops.check(result);
    transfer_result(result, total)
}

/// `accept`'s request: the ring's accept into `address` (close-on-exec,
/// non-blocking) under the read timeout, reading bound to the current coroutine.
/// A failure sets the socket's error from errno.
pub(crate) fn swr_v1_hook_uring_accept(
    ops: &UringSocketOps<'_>,
    address: &mut SocketAddress,
) -> c_int {
    let (host, tls) = (ops.host(), ops.tls);
    let flags = libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK;
    let fd = ops.bound(EVENT_READ, || {
        uring::accept(host, tls.fd, address, flags, tls.read_timeout)
    });
    if fd < 0 {
        (ops.set_err)(ops.socket, errno());
    }
    fd
}

/// `connect(addr, len)` through the ring under the connect timeout.
pub(crate) fn swr_v1_hook_uring_connect(ops: &UringSocketOps<'_>, peer: &SocketAddress) -> bool {
    if !ops.available(EVENT_RDWR) {
        return false;
    }
    let (host, fd) = (ops.host(), ops.tls.fd);
    let result = ops.bound(EVENT_RDWR, || {
        uring::connect(host, fd, peer, ops.connect_timeout)
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
    ops: &UringSocketOps<'_>,
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
    let milliseconds = (if timeout > 0.0 {
        timeout * 1000.0
    } else {
        timeout
    })
    .c_cast::<c_int>();
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
pub(crate) fn accept(socket: Co, ops: &UringSocketOps<'_>) -> Option<Co> {
    let mut conn = SocketRecord::new();
    let fd = swr_v1_hook_uring_accept(ops, &mut conn.info);
    if fd < 0 {
        return None;
    }
    conn.fd = fd;
    conn.removed = true;
    conn.info.kind = socket
        .net(|listener| listener.socket_type)
        .unwrap_or_default();
    conn.output.nonblock = true;
    conn.cloexec = true;
    let client = socket.adopt(Box::new(conn));
    crate::socket_extra::swr_v1_hook_co_socket_uring(client);
    Some(client)
}

/// `UringSocket::sendfile`'s transfer once the file is open: as upstream, `end`
/// bytes from `offset`, through the TLS session when there is one.
pub(crate) fn sendfile(
    ops: &UringSocketOps<'_>,
    file: c_int,
    offset: libc::off_t,
    end: usize,
) -> bool {
    let mut offset = offset;
    let sent = ops.bound(EVENT_WRITE, || {
        if !ops.ssl() {
            return uring::sendfile(
                ops.host(),
                ops.tls.fd,
                file,
                Some(&mut offset),
                end,
                ops.tls.write_timeout,
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
        c_int::from(tls.want_read) | (c_int::from(tls.want_write) << 1)
    })
}

/// `UringSocket::send_once` or `recv_once`: `io`, the open socket's one attempt on
/// its `network::Socket`, with the TLS BIOs pumped through the ring until it moves
/// bytes.
pub(crate) fn once(ops: &UringSocketOps<'_>, io: impl FnMut() -> isize) -> isize {
    ops.tls.once(io, || want(ops.socket))
}

/// `UringSocket::poll(event, timeout)`, in a coroutine.
pub(crate) fn poll(ops: &UringSocketOps<'_>, event: u32, timeout: f64) -> bool {
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
/// it runs over a pair of memory BIOs the ring pumps, on the socket's side. False
/// when `create` fails or leaves no session.
pub(crate) fn tls_session(socket: Co, create: impl FnOnce() -> bool) -> bool {
    if !create() {
        return false;
    }
    let server = socket.ssl_is_server();
    let made = socket.with_network(|network| {
        let session = Ssl::of(network)?;
        let (Some(rbio), Some(wbio)) = (OwnedBio::memory(), OwnedBio::memory()) else {
            return None;
        };
        // SAFETY: `create` just made the session, whose socket BIO no handle holds.
        unsafe { session.set_bio(rbio, Some(wbio)) };
        if server {
            session.set_accept_state();
        } else {
            session.set_connect_state();
        }
        Some(())
    });
    made.flatten().is_some()
}

/// One `UringSocket` handshake step through the ring, failed without a session
/// from [`tls_session`].
pub(crate) fn tls_step(socket: Co) -> crate::connect::Step {
    with(socket, |ops| handshake_step(socket, ops)).unwrap_or(crate::connect::Step::Failed)
}

fn handshake_step(socket: Co, ops: &UringSocketOps<'_>) -> crate::connect::Step {
    use crate::connect::Step;
    let Some(session) = ops.tls.ssl else {
        return Step::Failed;
    };
    let rc = session.do_handshake();
    if rc == 1 {
        return Step::Ready;
    }
    let function = if socket.ssl_is_server() {
        c"ssl_accept"
    } else {
        c"ssl_connect"
    };
    if ops.tls.perform(rc, function) {
        Step::Again
    } else {
        Step::Failed
    }
}

#[cfg(test)]
mod ported;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::TestSocket;
    use crate::uring::testing::{HOST, with_engine};
    use crate::vector::Vectors;
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
    fn ops(fd: c_int, socket: Co) -> UringSocketOps<'static> {
        UringSocketOps {
            socket,
            tls: Tls {
                uring: &HOST,
                socket,
                ssl: None,
                rbio: None,
                wbio: None,
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

    /// A short TLS transfer over two iovecs moves the vector by the bytes moved,
    /// once: the next transfer resumes inside the second iovec.
    #[test]
    fn tls_vector_moves_the_vector_once() {
        let mut buffer = [0u8; 8];
        let base = buffer.as_mut_ptr();
        let mut source = [
            libc::iovec {
                iov_base: base.cast(),
                iov_len: 4,
            },
            libc::iovec {
                iov_base: base.wrapping_add(4).cast(),
                iov_len: 4,
            },
        ];
        // SAFETY: two iovecs over `buffer`, which outlives the vector.
        let lent = unsafe { core::slice::from_raw_parts_mut(source.as_mut_ptr().cast(), 2) };
        let mut vector = IoVector::new(Vectors::Read(lent)).unwrap();
        let mut calls = Vec::new();
        let mut transfer = |vector: &mut IoVector, results: &[isize]| {
            let mut results = results.iter().copied();
            let result = vector.transfer_each(|lent| {
                let Lent::Read(slots) = lent else {
                    panic!("a vector to read into");
                };
                calls.push((slots.as_ptr().addr() - base.addr(), slots.len()));
                results.next().unwrap()
            });
            vector.update(result);
            (result, core::mem::take(&mut calls))
        };
        // 3 then 1 fill the first iovec, 2 lands in the second, then it would block.
        let (moved, made) = transfer(&mut vector, &[3, 1, 2, -1]);
        assert_eq!((moved, made), (6, vec![(0, 4), (3, 1), (4, 4), (6, 2)]));
        assert_eq!(vector.stopped(), (1, 2));
        let (moved, made) = transfer(&mut vector, &[2]);
        assert_eq!((moved, made), (2, vec![(6, 2)]));
        assert!(vector.iovecs().is_empty());
        // Nothing moved: the failure itself.
        drop(vector);
        // SAFETY: as above.
        let lent = unsafe { core::slice::from_raw_parts_mut(source.as_mut_ptr().cast(), 2) };
        let mut fresh = IoVector::new(Vectors::Read(lent)).unwrap();
        assert_eq!(fresh.transfer_each(|_| -1), -1);
    }

    /// `with` lends a `UringSocket`'s view whose BIOs are its live session's memory
    /// BIOs: none before the session, nor while its BIO is the socket's own; the
    /// fresh pair once they are replaced.
    #[test]
    fn with_lends_the_live_sessions_memory_bios() {
        use openssl_sys as ssl;
        use std::ptr::null_mut as null;
        let mut socket = TestSocket::new();
        let co = socket.co();
        assert!(with(co, |_| ()).is_none(), "not a UringSocket");
        co.with_extra(|extra| extra.uring = Some(&HOST));
        let bios = || {
            let view = with(co, |ops| {
                let bio = |bio: Option<swoole_rs_tls::Bio<'_>>| {
                    bio.map_or(null(), swoole_rs_tls::Bio::as_ptr)
                };
                let ssl = ops.tls.ssl.map_or(null(), Ssl::as_ptr);
                (ssl, bio(ops.tls.rbio), bio(ops.tls.wbio))
            });
            view.unwrap()
        };
        assert_eq!(bios(), (null(), null(), null()), "no session");
        unsafe {
            let context = ssl::SSL_CTX_new(ssl::TLS_method());
            let made = ssl::SSL_new(context);
            let own = ssl::BIO_new_socket(0, 0);
            ssl::SSL_set_bio(made, own, own);
            co.net(|record| record.set_ssl(made.cast()));
            assert_eq!(bios(), (made, null(), null()), "the socket's own BIO");
            let (read, write) = (OwnedBio::memory().unwrap(), OwnedBio::memory());
            let pair = with(co, |ops| {
                let (read, write) = ops.tls.ssl.unwrap().set_bio(read, write);
                (read.as_ptr(), write.as_ptr())
            });
            let (read, write) = pair.unwrap();
            assert_eq!(bios(), (made, read, write), "the memory pair");
            co.net(swoole_rs_reactor::SocketFields::take_ssl);
            assert_eq!(bios(), (null(), null(), null()), "closed");
            ssl::SSL_free(made);
            ssl::SSL_CTX_free(context);
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
            let mut address = SocketAddress {
                len: libc::socklen_t::try_from(size_of::<libc::sockaddr_storage>()).unwrap(),
                ..SocketAddress::default()
            };
            let fd = swr_v1_hook_uring_accept(&server, &mut address);
            assert!(fd >= 0);
            assert_eq!(
                unsafe { address.addr.ss.sa_family },
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
            let mut to_address = SocketAddress {
                len: libc::socklen_t::try_from(core::mem::size_of_val(&target)).unwrap(),
                ..SocketAddress::default()
            };
            to_address.addr.inet_v4 = target;
            let sent = sendto(&to, b"ping", &to_address);
            assert_eq!(sent, 4);
            let mut received = [0u8; 4];
            assert_eq!(receiver.recv(&mut received).unwrap(), 4);
            assert_eq!(&received, b"ping");
            // sendto checks availability itself, before resolving the host.
            assert_eq!(logged(), ["check 4"]);
        });
    }
}
