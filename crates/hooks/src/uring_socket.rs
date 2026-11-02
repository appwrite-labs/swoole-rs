//! `UringSocket`'s operations, whole: the availability check, binding the
//! current coroutine to the direction, the `io_uring` request (or the TLS transfer
//! over it), the result check, and the loops built on them (`recv_all`,
//! `send_all`, `readv`/`writev` and their `_all` forms, the TLS vector loops),
//! plus `connect` and `poll`. Native code keeps the socket's availability, its
//! result check and `get_name`, and passes them in [`UringSocketOps`].
use crate::shutdown::{SocketState, swr_v1_hook_socket_bind, swr_v1_hook_socket_connect};
use crate::uring::{
    Host, swr_v1_uring_connect, swr_v1_uring_poll, swr_v1_uring_read, swr_v1_uring_readv,
    swr_v1_uring_recv, swr_v1_uring_recvfrom, swr_v1_uring_recvmsg, swr_v1_uring_send,
    swr_v1_uring_sendmsg, swr_v1_uring_write, swr_v1_uring_writev,
};
use crate::uring_tls::{Tls, swr_v1_uring_tls_recv, swr_v1_uring_tls_send};
use core::ffi::{c_int, c_void};

const EVENT_READ: u32 = 1 << 9;
const EVENT_WRITE: u32 = 1 << 10;
const EVENT_RDWR: u32 = EVENT_READ | EVENT_WRITE;

/// The socket's view and effects.
#[repr(C)]
pub struct UringSocketOps {
    pub socket: *mut c_void,
    pub state: *mut SocketState,
    /// The socket's TLS view; its `ssl` is null without TLS, and it carries the
    /// descriptor, the timeouts and the ring's host.
    pub tls: Tls,
    pub connect_timeout: f64,
    /// `is_available(event)`.
    pub available: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    /// `check_return_value(retval)`.
    pub check: unsafe extern "C" fn(*mut c_void, isize),
    /// `set_err(code)`.
    pub set_err: unsafe extern "C" fn(*mut c_void, c_int),
    /// `socket->get_name()`.
    pub name: unsafe extern "C" fn(*mut c_void),
}

/// A native `network::IOVector`: its current iovec and how many remain, and
/// advancing it past a result.
#[repr(C)]
pub struct IoVectorOps {
    pub vector: *mut c_void,
    pub current: unsafe extern "C" fn(*mut c_void, *mut c_int) -> *const libc::iovec,
    pub update: unsafe extern "C" fn(*mut c_void, isize),
}

impl IoVectorOps {
    fn current(&self) -> (*const libc::iovec, c_int) {
        let mut count = 0;
        // SAFETY: the caller's live vector.
        let iov = unsafe { (self.current)(self.vector, &raw mut count) };
        (iov, count)
    }

    fn update(&self, result: isize) {
        // SAFETY: the caller's live vector.
        unsafe { (self.update)(self.vector, result) };
    }
}

/// The scalar operations of [`swr_v1_hook_uring_io`].
pub const READ: c_int = 0;
pub const WRITE: c_int = 1;
pub const RECV: c_int = 2;
pub const SEND: c_int = 3;
pub const RECVMSG: c_int = 4;
pub const SENDMSG: c_int = 5;
pub const RECVFROM: c_int = 6;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl UringSocketOps {
    fn host(&self) -> *const Host {
        // SAFETY: the native static TLS host table.
        unsafe { (*self.tls.host).uring }
    }

    fn ssl(&self) -> bool {
        !self.tls.ssl.is_null()
    }

    fn available(&self, event: u32) -> bool {
        // SAFETY: the live socket's availability.
        unsafe { (self.available)(self.socket, event) }
    }

    fn check(&self, result: isize) {
        // SAFETY: the live socket's result check.
        unsafe { (self.check)(self.socket, result) };
    }

    /// `call` with the current coroutine bound to `event`.
    fn bound<T>(&self, event: u32, call: impl FnOnce() -> T) -> T {
        // SAFETY: the native static host table.
        let current = unsafe { (*self.host()).current };
        // SAFETY: the host's current coroutine.
        let co = unsafe { current() };
        // SAFETY: the live state.
        unsafe { swr_v1_hook_socket_bind(self.state, event, co) };
        let result = call();
        // SAFETY: as above.
        unsafe { swr_v1_hook_socket_bind(self.state, event, core::ptr::null_mut()) };
        result
    }

    /// One scalar operation (see [`swr_v1_hook_uring_io`]).
    ///
    /// # Safety
    /// As [`swr_v1_hook_uring_io`].
    unsafe fn io(
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
        if !self.available(event) {
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
    /// As [`swr_v1_hook_uring_io`].
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
            READ => unsafe { swr_v1_uring_read(host, fd, buf, n, rt) },
            // SAFETY: as above.
            WRITE => unsafe { swr_v1_uring_write(host, fd, buf, n, wt) },
            // SAFETY: as above.
            RECV if self.ssl() => unsafe { swr_v1_uring_tls_recv(tls, buf, n) },
            // SAFETY: as above.
            RECV => unsafe { swr_v1_uring_recv(host, fd, buf, n, 0, rt) },
            // SAFETY: as above.
            SEND if self.ssl() => unsafe { swr_v1_uring_tls_send(tls, buf, n) },
            // SAFETY: as above.
            SEND => unsafe { swr_v1_uring_send(host, fd, buf, n, 0, wt) },
            // SAFETY: as above.
            RECVMSG => unsafe { swr_v1_uring_recvmsg(host, fd, extra.cast(), flags, rt) },
            // SAFETY: as above.
            SENDMSG => unsafe { swr_v1_uring_sendmsg(host, fd, extra.cast(), flags, wt) },
            // SAFETY: as above.
            _ => unsafe { swr_v1_uring_recvfrom(host, fd, buf, n, extra.cast(), extra_length, rt) },
        }
    }

    /// `ssl_readv`/`ssl_writev`: TLS transfers over each iovec while they move data.
    fn tls_vector(&self, vector: &IoVectorOps, write: bool) -> isize {
        let mut total = 0isize;
        loop {
            let (iov, _) = vector.current();
            // SAFETY: the vector's current iovec.
            let iov = unsafe { *iov };
            let result = if write {
                // SAFETY: the iovec's buffer.
                unsafe { swr_v1_uring_tls_send(self.tls, iov.iov_base, iov.iov_len) }
            } else {
                // SAFETY: the iovec's buffer.
                unsafe { swr_v1_uring_tls_recv(self.tls, iov.iov_base, iov.iov_len) }
            };
            total += result.max(0);
            vector.update(result);
            if !(result > 0 && vector.current().1 > 0) {
                return if total > 0 { total } else { result };
            }
        }
    }

    /// `readv`/`writev`.
    fn vector(&self, vector: &IoVectorOps, write: bool) -> isize {
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
                        unsafe { swr_v1_uring_writev(host, fd, iov, count, self.tls.write_timeout) }
                    } else {
                        // SAFETY: the vector's remaining iovecs.
                        unsafe { swr_v1_uring_readv(host, fd, iov, count, self.tls.read_timeout) }
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

/// One scalar operation: `op` is 0 read, 1 write, 2 recv, 3 send (TLS when the
/// socket has it), 4 recvmsg and 5 sendmsg (`extra` the msghdr, with `flags`),
/// 6 recvfrom (`extra`/`extra_length` the address).
///
/// # Safety
/// `ops` is live; the buffers are valid as for the system call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_uring_io(
    ops: *const UringSocketOps,
    op: c_int,
    buf: *mut c_void,
    n: usize,
    extra: *mut c_void,
    extra_length: *mut libc::socklen_t,
    flags: c_int,
) -> isize {
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
    // SAFETY: the caller's buffers.
    unsafe { ops.io(op, buf, n, extra, extra_length, flags) }
}

/// `recv_all` (`write` false) or `send_all`: recv/send until `n` bytes moved, it
/// failed, or (receiving) the peer closed.
///
/// # Safety
/// `ops` is live and `buf` holds `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_uring_io_all(
    ops: *const UringSocketOps,
    write: bool,
    buf: *mut c_void,
    n: usize,
) -> isize {
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
    let mut total = 0usize;
    let result = loop {
        let rest = buf.cast::<u8>().wrapping_add(total).cast();
        // SAFETY: the unsent rest of the caller's buffer.
        let result = unsafe {
            ops.io(
                if write { SEND } else { RECV },
                rest,
                n - total,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                0,
            )
        };
        total += result.max(0).unsigned_abs();
        if !(result > 0 && total < n) {
            break result;
        }
    };
    ops.check(result);
    transfer_result(result, total)
}

/// `swr_v1_hook_transfer_result`: -1 when it failed before moving anything, else
/// the bytes moved.
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
/// `ops` and `vector` are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_uring_vector(
    ops: *const UringSocketOps,
    vector: *const IoVectorOps,
    write: bool,
    all: bool,
) -> isize {
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
    // SAFETY: the caller's live vector.
    let vector = unsafe { &*vector };
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

/// `connect(addr, len)` through the ring under the connect timeout.
///
/// # Safety
/// `ops` is live and `addr` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_uring_connect(
    ops: *const UringSocketOps,
    addr: *const libc::sockaddr,
    length: libc::socklen_t,
) -> bool {
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
    if !ops.available(EVENT_RDWR) {
        return false;
    }
    let (host, fd) = (ops.host(), ops.tls.fd);
    // SAFETY: the caller's address.
    let result = ops.bound(EVENT_RDWR, || unsafe {
        swr_v1_uring_connect(host, fd, addr, length, ops.connect_timeout)
    });
    if result < 0 {
        return false;
    }
    // SAFETY: the live state, then the live socket's name and error.
    unsafe {
        swr_v1_hook_socket_connect(ops.state);
    }
    // SAFETY: as above.
    unsafe { (ops.name)(ops.socket) };
    // SAFETY: as above.
    unsafe { (ops.set_err)(ops.socket, 0) };
    true
}

/// `poll(type, timeout)`: one descriptor for `events` (poll(2) bits), within
/// `timeout` seconds (0 checks without the ring, negative waits indefinitely).
///
/// # Safety
/// `ops` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_uring_poll(
    ops: *const UringSocketOps,
    event: u32,
    events: libc::c_short,
    timeout: f64,
) -> bool {
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
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
    // SAFETY: the host table and one local pollfd.
    let rc = unsafe { swr_v1_uring_poll(ops.host(), &raw mut fd, milliseconds) };
    if rc != 1 {
        let error = if rc == 0 { libc::ETIMEDOUT } else { errno() };
        // SAFETY: the live socket's error.
        unsafe { (ops.set_err)(ops.socket, error) };
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shutdown::{
        swr_v1_hook_socket_bound_co, swr_v1_hook_socket_state_free, swr_v1_hook_socket_state_new,
    };
    use crate::uring::testing::{HOST, with_engine};
    use crate::uring_tls::TlsHost;
    use core::ffi::{c_char, c_ulong};
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
    unsafe extern "C" fn available(_: *mut c_void, event: u32) -> bool {
        log(format!("available {event}"));
        AVAILABLE.with(|a| *a.borrow())
    }
    unsafe extern "C" fn check(_: *mut c_void, result: isize) {
        log(format!("check {result}"));
    }
    unsafe extern "C" fn set_err(_: *mut c_void, code: c_int) {
        log(format!("err {code}"));
    }
    unsafe extern "C" fn name(_: *mut c_void) {}
    unsafe extern "C" fn tls_set_err(_: *mut c_void, _: c_int) {}
    unsafe extern "C" fn check_failure(_: *mut c_void) {}
    unsafe extern "C" fn buffer(_: *mut c_void, _: bool, _: *mut usize) -> *mut c_char {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn ssl_error(_: *mut c_void, _: *const c_char, _: c_int, _: c_ulong) {}
    static TLS_HOST: TlsHost = TlsHost {
        uring: &HOST,
        set_err: tls_set_err,
        check_failure,
        buffer,
        ssl_error,
        bad_protocol: 1008,
        reset: 1009,
    };

    fn ops(fd: c_int, state: *mut SocketState) -> UringSocketOps {
        UringSocketOps {
            socket: core::ptr::null_mut(),
            state,
            tls: Tls {
                host: &raw const TLS_HOST,
                socket: core::ptr::null_mut(),
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
            name,
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
            let state = swr_v1_hook_socket_state_new();
            let (a, b) = (ops(fds[0], state), ops(fds[1], state));
            let mut message = *b"hello world";
            let mut received = [0u8; 11];
            // SAFETY: local tables and buffers.
            unsafe {
                assert_eq!(
                    swr_v1_hook_uring_io_all(&raw const a, true, message.as_mut_ptr().cast(), 11),
                    11
                );
                assert_eq!(
                    swr_v1_hook_uring_io_all(&raw const b, false, received.as_mut_ptr().cast(), 11),
                    11
                );
            }
            assert_eq!(&received, b"hello world");
            // SAFETY: the live state.
            let bound = unsafe { swr_v1_hook_socket_bound_co(state, EVENT_READ) };
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
            // SAFETY: as above.
            let result = unsafe {
                swr_v1_hook_uring_io(
                    &raw const a,
                    READ,
                    message.as_mut_ptr().cast(),
                    1,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    0,
                )
            };
            AVAILABLE.with(|v| *v.borrow_mut() = true);
            assert_eq!(result, -1);
            assert_eq!(logged(), std::slice::from_ref(&read));
            // A poll with nothing to read times out.
            // SAFETY: as above.
            let polled =
                unsafe { swr_v1_hook_uring_poll(&raw const a, EVENT_READ, libc::POLLIN, 0.01) };
            assert!(!polled);
            assert_eq!(logged(), [read, format!("err {}", libc::ETIMEDOUT)]);
            // SAFETY: the pair and state made above.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
                swr_v1_hook_socket_state_free(state);
            }
        });
    }
}
