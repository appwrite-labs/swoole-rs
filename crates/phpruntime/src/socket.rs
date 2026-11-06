//! The coroutine socket (`SocketImpl`) is still C++: what the streams need of
//! it is `swr_v1_runtime_socket`, a table `src/coroutine/system.cc` defines
//! (virtual calls reach `UringSocket`); what they need of Swoole's PHP side is
//! in the runtime's [`Ops`](crate::Ops). Both are declared in
//! `native/hooks/swoole_rs_phpruntime.h`.
use libc::{c_char, c_int, c_void, size_t, sockaddr, socklen_t, ssize_t};
use std::{ffi::CStr, ptr::NonNull};
use swoole_rs_hooks::{
    co_io::swr_v1_hook_co_socket_set_err,
    co_socket::CoSocket,
    lifecycle::{swr_v1_hook_co_socket_check_liveness, swr_v1_hook_co_socket_shutdown},
};
use swoole_rs_reactor::{SocketRecord, swr_v1_socket_set_timeout};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    php_error,
    sys::zval,
};

use crate::ops;

/// `swr_v1_runtime_socket_call`: `make`'s and `close`'s arguments and results.
#[repr(C)]
struct SocketCall {
    socket: *mut c_void,
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    family: bool,
    result: bool,
}

type Body = unsafe extern "C" fn(*mut c_void);

/// `swr_v1_runtime_socket_ops`.
#[repr(C)]
struct SocketOps {
    make: Body,
    close: Body,
    free: Body,
    base: unsafe extern "C" fn(*mut c_void) -> *mut CoSocket,
    io: unsafe extern "C" fn(*mut c_void, c_int, *mut c_char, size_t) -> ssize_t,
    recvfrom: unsafe extern "C" fn(
        *mut c_void,
        *mut c_char,
        size_t,
        *mut sockaddr,
        *mut socklen_t,
    ) -> ssize_t,
    option: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, bool),
    bind: unsafe extern "C" fn(*mut c_void, *const c_char, size_t, c_int) -> bool,
    connect: unsafe extern "C" fn(*mut c_void, *const c_char, size_t, c_int, c_int) -> bool,
    listen: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    accept: unsafe extern "C" fn(*mut c_void, f64) -> *mut c_void,
    ssl_enabled: unsafe extern "C" fn(*mut c_void) -> bool,
    ssl_encrypt: unsafe extern "C" fn(*mut c_void),
    ssl_handshake: unsafe extern "C" fn(*mut c_void) -> bool,
    ssl_close: unsafe extern "C" fn(*mut c_void),
    peer_certs: unsafe extern "C" fn(
        *mut c_void,
        bool,
        unsafe extern "C" fn(*mut c_void, *const c_char, size_t),
        *mut c_void,
    ),
    poll: unsafe extern "C" fn(*mut c_void, bool, f64) -> bool,
    poll_set: unsafe extern "C" fn(
        *const c_int,
        *const c_int,
        size_t,
        f64,
        *mut c_int,
        *mut c_int,
        *mut size_t,
    ) -> bool,
    close_hook: unsafe extern "C" fn(
        *mut c_void,
        unsafe extern "C" fn(*mut c_void, *mut c_void),
        *mut c_void,
    ),
    ssl_verify: unsafe extern "C" fn(*mut c_void, bool) -> bool,
    vector: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const libc::iovec,
        c_int,
        *mut c_int,
        *mut size_t,
    ) -> ssize_t,
    sendfile: unsafe extern "C" fn(*mut c_void, *const c_char, i64, i64) -> bool,
    sendto: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        size_t,
        c_int,
        *const c_char,
        size_t,
    ) -> ssize_t,
    proxy: unsafe extern "C" fn(
        *mut c_void,
        bool,
        *const c_char,
        size_t,
        c_int,
        *const c_char,
        size_t,
        *const c_char,
        size_t,
    ),
    ssl_context: unsafe extern "C" fn(*mut c_void) -> *const c_void,
}

unsafe extern "C" {
    static swr_v1_runtime_socket: SocketOps;
}

/// The native table: static and read-only.
fn native() -> &'static SocketOps {
    // SAFETY: system.cc defines the table as a constant.
    unsafe { &swr_v1_runtime_socket }
}

/// `swr_v1_phpruntime_ssl_call`.
#[repr(C)]
struct SslCall {
    socket: *mut c_void,
    settings: *mut zval,
    result: bool,
}

/// `swr_v1_phpruntime_x509_call`.
#[repr(C)]
struct X509Call {
    pem: *const c_char,
    length: size_t,
    retval: *mut zval,
}

/// `swr_v1_phpruntime_fd_call`.
#[repr(C)]
struct FdCall {
    value: *mut zval,
    fd: c_int,
}

/// `body(call)` under `zend_try`.
/// # Safety
/// `body` is a native body that takes a `T`.
unsafe fn trap<T>(body: Body, call: &mut T) -> Result<(), Bailout> {
    // SAFETY: per the caller; `call` is live for the call.
    unsafe { bailout::trap(body, (&raw mut *call).cast()) }
}
/// `SW_SOCK_*`.
pub(crate) mod kind {
    pub(crate) const TCP: i32 = 1;
    pub(crate) const UDP: i32 = 2;
    pub(crate) const TCP6: i32 = 3;
    pub(crate) const UNIX_STREAM: i32 = 5;
    pub(crate) const UNIX_DGRAM: i32 = 6;
}

/// `SW_TIMEOUT_*`.
pub(crate) mod timeout {
    pub(crate) const CONNECT: i32 = 1 << 1;
    pub(crate) const READ: i32 = 1 << 2;
    pub(crate) const RDWR: i32 = READ | 1 << 3;
    pub(crate) const ALL: i32 = 0xf;
}

/// The table's `io` operations.
#[derive(Clone, Copy)]
pub enum Io {
    SendSync = 0,
    RecvSync = 1,
    SendAll = 2,
    SendOnce = 3,
    Recv = 4,
    RecvOnce = 5,
    Peek = 6,
    Send = 7,
    RecvAll = 8,
}

/// `SW_WAIT`.
pub(crate) const SW_WAIT: i32 = 2;

/// A `SocketImpl *`: owned (deleted on drop), or a hooked fd's, lent by the
/// socket map until the fd's hooked close.
pub(crate) struct Socket {
    socket: NonNull<c_void>,
    owned: bool,
}

impl Drop for Socket {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: the socket is owned and live; `free` is native.
            if unsafe { bailout::trap(native().free, self.raw()) }.is_err() {
                Bailout.defer();
            }
        }
    }
}

impl Socket {
    /// `new SocketImpl(kind)`, encrypting with `ssl`; `None` without an fd.
    pub(crate) fn new(kind: i32, ssl: bool) -> Option<Self> {
        let socket = Self::make(-1, 0, kind, 0)?;
        if ssl {
            socket.ssl_encrypt();
        }
        (socket.fd() >= 0).then_some(socket)
    }

    /// `new SocketImpl(fd, domain, kind, protocol)`.
    pub(crate) fn from_fd(fd: i32, domain: i32, kind: i32, protocol: i32) -> Option<Self> {
        Self::make(fd, domain, kind, protocol)
    }

    /// The native `make`: `new SocketImpl(kind)` when `fd` is negative.
    fn make(fd: i32, domain: i32, kind: i32, protocol: i32) -> Option<Self> {
        let mut call = SocketCall {
            socket: std::ptr::null_mut(),
            fd,
            domain,
            kind,
            protocol,
            family: fd >= 0,
            result: false,
        };
        // SAFETY: `make` takes a `SocketCall` and writes the uniquely owned socket.
        if unsafe { trap(native().make, &mut call) }.is_err() {
            Bailout.defer();
            return None;
        }
        Self::wrap(call.socket, true)
    }

    /// The coroutine socket of hooked `fd`, if any.
    pub(crate) fn of_fd(fd: i32) -> Option<Self> {
        Self::wrap(
            swoole_rs_hooks::socket_map::swr_v1_hook_socket_get(fd),
            false,
        )
    }

    fn wrap(socket: *mut c_void, owned: bool) -> Option<Self> {
        Some(Self {
            socket: NonNull::new(socket)?,
            owned,
        })
    }

    fn raw(&self) -> *mut c_void {
        self.socket.as_ptr()
    }

    /// A compiler-adjusted base projection; `coroutine::Socket` has virtual methods.
    fn co_ptr(&self) -> *mut CoSocket {
        // SAFETY: the native cast returns the actual embedded Rust record of this live socket.
        unsafe { (native().base)(self.raw()) }
    }

    pub(crate) fn fd(&self) -> i32 {
        // SAFETY: read one scalar from the live coroutine socket, without retaining a record borrow.
        unsafe { (*self.co_ptr()).sock_fd }
    }

    pub(crate) fn domain(&self) -> i32 {
        // SAFETY: the live native object embeds this actual Rust record.
        unsafe { (*self.co_ptr()).sock_domain }
    }

    pub(crate) fn socket_type(&self) -> i32 {
        // SAFETY: the live native object embeds this actual Rust record.
        unsafe { (*self.co_ptr()).sock_type }
    }

    /// The network record can change during reinitialization; read its pointer on each use.
    /// The closure performs only immediate operations and never suspends or invokes PHP.
    fn with_record<T>(&self, absent: T, f: impl FnOnce(&mut SocketRecord) -> T) -> T {
        // SAFETY: copy the current network pointer from this live coroutine record.
        let record = unsafe { (*self.co_ptr()).socket };
        // SAFETY: no other network record borrow exists during this immediate closure.
        match unsafe { record.as_mut() } {
            Some(record) => f(record),
            None => absent,
        }
    }

    pub(crate) fn is_inet(&self) -> bool {
        self.with_record(false, |record| record.is_inet())
    }

    /// `errCode`.
    pub(crate) fn error(&self) -> i32 {
        // SAFETY: copy the error scalar from this live record.
        unsafe { (*self.co_ptr()).err_code }
    }

    /// `errMsg`, if set.
    pub(crate) fn message(&self) -> Option<&CStr> {
        // SAFETY: copy the current borrowed native message pointer; no record borrow escapes.
        let message = unsafe { (*self.co_ptr()).err_msg };
        // SAFETY: errMsg is null or a NUL-terminated string the socket keeps.
        (!message.is_null()).then(|| unsafe { CStr::from_ptr(message) })
    }

    /// `set_err(code)`.
    pub(crate) fn set_error(&self, code: i32) {
        // SAFETY: a live socket.
        unsafe { swr_v1_hook_co_socket_set_err(self.co_ptr(), code) };
    }

    /// `errCode = code`.
    pub(crate) fn set_error_code(&self, code: i32) {
        // SAFETY: a live socket.
        unsafe { (*self.co_ptr()).err_code = code };
    }

    /// One transfer of `length` bytes at `buf`.
    /// # Safety
    /// `buf` holds `length` bytes (writable for the receiving operations).
    pub(crate) unsafe fn io(&self, op: Io, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { (native().io)(self.raw(), op as c_int, buf, length) }
    }

    /// `catch_write_error`/`catch_read_error` of `code`, which are one rule.
    pub(crate) fn catch_error(&self, code: i32) -> i32 {
        self.with_record(0, |record| record.catch_error(code))
    }

    /// `recvfrom(buf, length, addr, addrlen)`.
    /// # Safety
    /// `buf` holds `length` writable bytes; `addr` holds `*addrlen` bytes.
    pub(crate) unsafe fn recvfrom(
        &self,
        buf: *mut c_char,
        length: usize,
        addr: *mut sockaddr,
        addrlen: *mut socklen_t,
    ) -> isize {
        // SAFETY: per the caller.
        unsafe { (native().recvfrom)(self.raw(), buf, length, addr, addrlen) }
    }

    pub(crate) fn close(&self) -> Result<bool, Bailout> {
        let mut call = SocketCall {
            socket: self.raw(),
            fd: -1,
            domain: 0,
            kind: 0,
            protocol: 0,
            family: false,
            result: false,
        };
        // SAFETY: `close` takes a `SocketCall` of this live socket; PHP callbacks may bail out.
        unsafe { trap(native().close, &mut call) }?;
        Ok(call.result)
    }

    /// `setsockopt(level, name, value)`; `logged` through the coroutine
    /// socket, which logs a failure.
    pub(crate) fn set_option(&self, level: i32, name: i32, value: i32, logged: bool) {
        // SAFETY: a live socket.
        unsafe { (native().option)(self.raw(), level, name, value, logged) };
    }

    pub(crate) fn set_nodelay(&self, value: i32) {
        self.with_record(false, |record| record.set_tcp_nodelay(value));
    }

    pub(crate) fn set_zero_copy(&self) {
        // SAFETY: a live socket.
        unsafe { (*self.co_ptr()).zero_copy = true };
    }

    pub(crate) fn set_timeout(&self, seconds: f64, kind: i32) {
        self.with_record((), |record| {
            // SAFETY: this immediate closure exclusively accesses the live timeout group.
            unsafe { swr_v1_socket_set_timeout(&raw mut record.timeouts, seconds, kind) };
        });
    }

    pub(crate) fn bind(&self, host: &CStr, port: i32) -> bool {
        // SAFETY: a live socket and its host's bytes.
        unsafe { (native().bind)(self.raw(), host.as_ptr(), host.count_bytes(), port) }
    }

    pub(crate) fn connect(&self, host: &CStr, port: i32) -> bool {
        let host = host.to_bytes();
        // SAFETY: a live socket and `host.len()` readable bytes.
        unsafe { (native().connect)(self.raw(), host.as_ptr().cast(), host.len(), port, 0) }
    }

    pub(crate) fn listen(&self, backlog: i32) -> bool {
        // SAFETY: a live socket.
        unsafe { (native().listen)(self.raw(), backlog) }
    }

    /// `accept()`: the connection, if any.
    pub(crate) fn accept(&self) -> Option<Self> {
        // SAFETY: a live socket; the connection is new and owned.
        Self::wrap(unsafe { (native().accept)(self.raw(), 0.0) }, true)
    }

    pub(crate) fn shutdown(&self, how: i32) -> bool {
        // SAFETY: a live socket.
        unsafe { swr_v1_hook_co_socket_shutdown(self.co_ptr(), how) }
    }

    /// `poll(SW_EVENT_WRITE or SW_EVENT_READ, timeout)`.
    pub(crate) fn poll(&self, write: bool, timeout: f64) -> bool {
        // SAFETY: a live socket.
        unsafe { (native().poll)(self.raw(), write, timeout) }
    }

    /// `get_err()`: `swoole_strerror(errCode)`.
    pub(crate) fn strerror(&self) -> &'static CStr {
        strerror(self.error())
    }

    pub(crate) fn check_liveness(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swr_v1_hook_co_socket_check_liveness(self.co_ptr()) }
    }

    /// `ssl_is_enable()`: encryption is configured.
    pub(crate) fn ssl_enabled(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { (native().ssl_enabled)(self.raw()) }
    }

    /// `ssl_is_available()`: the handshake has completed.
    pub(crate) fn ssl_available(&self) -> bool {
        let co = self.co_ptr();
        // SAFETY: a scalar of the live coroutine socket.
        let connected = !unsafe { (*co).socket }.is_null();
        // SAFETY: as above.
        connected && unsafe { (*co).ssl_handshaked }
    }

    /// The connection's `SSL *`, or null.
    pub(crate) fn ssl(&self) -> *mut openssl_sys::SSL {
        self.with_record(std::ptr::null_mut(), |record| record.ssl.cast())
    }

    pub(crate) fn ssl_encrypt(&self) {
        // SAFETY: a live socket.
        unsafe { (native().ssl_encrypt)(self.raw()) };
    }

    pub(crate) fn ssl_handshake(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { (native().ssl_handshake)(self.raw()) }
    }

    pub(crate) fn ssl_close(&self) {
        // SAFETY: a live socket.
        unsafe { (native().ssl_close)(self.raw()) };
    }

    /// The peer certificate (one PEM, maybe empty), or its chain.
    pub(crate) fn peer_certs(&self, chain: bool) -> Vec<Vec<u8>> {
        unsafe extern "C" fn each(arg: *mut c_void, pem: *const c_char, length: size_t) {
            // SAFETY: `pem` holds `length` bytes.
            let pem = unsafe { std::slice::from_raw_parts(pem.cast::<u8>(), length) };
            // SAFETY: `arg` is the vector below.
            let certs = unsafe { &mut *arg.cast::<Vec<Vec<u8>>>() };
            certs.push(pem.to_vec());
        }
        let mut certs: Vec<Vec<u8>> = Vec::new();
        // SAFETY: a live socket; `each` gets the vector, live for the call.
        unsafe {
            (native().peer_certs)(self.raw(), chain, each, (&raw mut certs).cast());
        }
        certs
    }

    /// `php_swoole_socket_set_ssl(socket, settings)`.
    /// # Errors
    /// A warning's error handler bailed out.
    /// # Safety
    /// `settings` is a live array.
    pub(crate) unsafe fn set_ssl(&self, settings: *mut zval) -> Result<bool, Bailout> {
        let mut call = SslCall {
            socket: self.raw(),
            settings,
            result: false,
        };
        // SAFETY: per the caller; `set_ssl` takes an `SslCall`.
        unsafe { trap(ops().set_ssl, &mut call) }?;
        Ok(call.result)
    }
}

/// `openssl_x509_read(pem)` into `retval`, which the caller then owns.
/// # Errors
/// The call bailed out.
/// # Safety
/// `retval` is writable.
pub(crate) unsafe fn x509_read(pem: &[u8], retval: *mut zval) -> Result<(), Bailout> {
    let mut call = X509Call {
        pem: pem.as_ptr().cast(),
        length: pem.len(),
        retval,
    };
    // SAFETY: per the caller; `pem` holds its bytes; `x509_read` takes an `X509Call`.
    unsafe { trap(ops().x509_read, &mut call) }
}

/// How a warning is raised.
#[derive(Clone, Copy)]
pub enum Report {
    /// `php_error_docref(NULL, ..)`.
    Docref,
    /// `php_swoole_error`.
    Swoole,
    /// `php_swoole_fatal_error`.
    Fatal,
}

/// Raise `message` at `level`, trapping a bailout of the error handler.
/// # Errors
/// The error handler bailed out.
pub fn report(kind: Report, level: i32, message: &CStr) -> Result<(), Bailout> {
    let report = match kind {
        // SAFETY: called while PHP runs.
        Report::Docref => return unsafe { php_error(level, message) },
        Report::Swoole => ops().error,
        Report::Fatal => ops().fatal_error,
    };
    // SAFETY: called while PHP runs; the reporter is C++.
    unsafe { bailout::report_level(report, level, message) }
}

/// `php_swoole_check_reactor()`.
pub fn check_reactor() {
    // SAFETY: called while PHP runs.
    unsafe { (ops().check_reactor)() };
}

/// `swoole_strerror(code)`.
#[must_use]
pub fn strerror(code: i32) -> &'static CStr {
    // SAFETY: a lookup.
    let message = unsafe { (ops().strerror)(code) };
    // SAFETY: Swoole's static message table or strerror's text.
    unsafe { CStr::from_ptr(message) }
}

/// `Coroutine::get_current_safe()`: outside a coroutine Swoole raises a fatal
/// error.
/// # Errors
/// The fatal error bailed out.
pub(crate) fn coroutine_check() -> Result<(), Bailout> {
    // SAFETY: called while PHP runs; the body ignores its argument.
    unsafe { bailout::trap(ops().coroutine_check, std::ptr::null_mut()) }
}

/// `php_swoole_convert_to_fd(value)`: an fd, or negative after a warning.
/// # Errors
/// The warning's error handler bailed out.
/// # Safety
/// `value` is a live zval.
pub(crate) unsafe fn convert_to_fd(value: *mut zval) -> Result<i32, Bailout> {
    let mut call = FdCall { value, fd: -1 };
    // SAFETY: per the caller; `convert_to_fd` takes an `FdCall`.
    unsafe { trap(ops().convert_to_fd, &mut call) }?;
    Ok(call.fd)
}

/// `System::socket_poll` of `(fd, events)` rows merged by fd, in the order
/// `stream_select` added them: `None` on a timeout or failure, else each polled
/// fd and its revents in the poll set's order.
pub(crate) fn poll(rows: &[(i32, i32)], timeout: f64) -> Option<Vec<(i32, i32)>> {
    let fds: Vec<c_int> = rows.iter().map(|row| row.0).collect();
    let events: Vec<c_int> = rows.iter().map(|row| row.1).collect();
    let mut ready = vec![0; rows.len()];
    let mut revents = vec![0; rows.len()];
    let mut size = 0;
    // SAFETY: each buffer holds `rows.len()` entries, at least the poll set.
    let polled = unsafe {
        (native().poll_set)(
            fds.as_ptr(),
            events.as_ptr(),
            rows.len(),
            timeout,
            ready.as_mut_ptr(),
            revents.as_mut_ptr(),
            &raw mut size,
        )
    };
    polled.then(|| ready.into_iter().zip(revents).take(size).collect())
}

/// What the client classes (crates/phpclients and crates/phphttp2) need of a
/// `SocketImpl *` beyond the record: its virtual and C++-only members, through
/// the native table.
pub mod co {
    pub use super::Io;
    use super::{SocketCall, native, trap};
    use libc::{c_char, c_int, c_void, iovec};
    pub use swoole_rs_hooks::co_socket::CoSocket;
    use swoole_rs_hooks::{
        TimeoutScope,
        co_io::{
            swr_v1_hook_co_socket_bound_cid, swr_v1_hook_co_socket_owned_err,
            swr_v1_hook_co_socket_set_err,
        },
        lifecycle::{
            swr_v1_hook_co_socket_cancel, swr_v1_hook_co_socket_check_liveness,
            swr_v1_hook_co_socket_close, swr_v1_hook_co_socket_shutdown,
        },
        shutdown::{swr_v1_hook_socket_bound_co, swr_v1_hook_socket_flags},
        socket_members, swr_v1_hook_timeout_restore, swr_v1_hook_timeout_scope,
    };
    use swoole_rs_zend::bailout::Bailout;

    unsafe extern "C" {
        /// `Socket::recvfrom(buf, n)`: into the network socket's peer address.
        #[link_name = "_ZN6swoole9coroutine6Socket8recvfromEPvm"]
        fn native_recvfrom(socket: Socket, buf: *mut c_void, n: usize) -> isize;
    }

    /// `new SocketImpl(...)`: by `domain`, `kind` and `protocol` with `family`,
    /// else by the `swSocketType` `kind`; over `fd` unless it is negative.
    /// # Errors
    /// The constructor bailed out.
    pub fn make(
        fd: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
        family: bool,
    ) -> Result<Socket, Bailout> {
        let mut call = SocketCall {
            socket: std::ptr::null_mut(),
            fd,
            domain,
            kind,
            protocol,
            family,
            result: false,
        };
        // SAFETY: `make` takes a `SocketCall` and writes the uniquely owned socket.
        unsafe { trap(native().make, &mut call) }?;
        Ok(call.socket)
    }

    /// `delete socket`.
    /// # Errors
    /// A close hook bailed out.
    /// # Safety
    /// `socket` is live and owned; it is gone after the call.
    pub unsafe fn free(socket: Socket) -> Result<(), Bailout> {
        // SAFETY: per the caller; `free` takes the socket.
        unsafe { swoole_rs_zend::bailout::trap(native().free, socket) }
    }

    /// `close()` inside `zend_try`.
    /// # Errors
    /// A close hook bailed out.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn close_trapped(socket: Socket) -> Result<bool, Bailout> {
        let mut call = SocketCall {
            socket,
            fd: -1,
            domain: 0,
            kind: 0,
            protocol: 0,
            family: false,
            result: false,
        };
        // SAFETY: per the caller; `close` takes a `SocketCall`.
        unsafe { trap(native().close, &mut call) }?;
        Ok(call.result)
    }

    /// A timeout of `kind` set for the guard's life (`TimeoutSetter`).
    pub struct Timeout(TimeoutScope);

    impl Timeout {
        /// # Safety
        /// `socket` is live, and stays so while the guard does.
        #[must_use]
        pub unsafe fn set(socket: Socket, timeout: f64, kind: c_int) -> Self {
            // SAFETY: per the caller.
            let record = unsafe { record(socket) };
            // SAFETY: its live record.
            Self(unsafe { swr_v1_hook_timeout_scope(record, timeout, kind) })
        }
    }

    impl Drop for Timeout {
        fn drop(&mut self) {
            // SAFETY: the scope's socket is live, per `set`.
            unsafe { swr_v1_hook_timeout_restore(&raw const self.0) };
        }
    }

    /// One table transfer of `length` bytes at `buf`.
    /// # Safety
    /// `socket` is live and `buf` holds `length` bytes, writable to receive.
    pub unsafe fn io(socket: Socket, op: Io, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { (native().io)(socket, op as c_int, buf, length) }
    }

    /// `recv_line(buf, length)`.
    /// # Safety
    /// As [`io`].
    pub unsafe fn recv_line(socket: Socket, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::recv_line(socket, buf.cast(), length) }
    }

    /// `recv_with_buffer(buf, length)`.
    /// # Safety
    /// As [`io`].
    pub unsafe fn recv_with_buffer(socket: Socket, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::recv_with_buffer(socket, buf.cast(), length) }
    }

    /// `readv`, `readv_all`, `writev` or `writev_all` (`op` 0 to 3) over
    /// `count` vectors; where it stopped into `index` and `offset`.
    /// # Safety
    /// `socket` is live and the vectors are valid for the operation.
    pub unsafe fn vector(
        socket: Socket,
        op: c_int,
        vectors: *const iovec,
        count: c_int,
        index: *mut c_int,
        offset: *mut usize,
    ) -> isize {
        // SAFETY: per the caller.
        unsafe { (native().vector)(socket, op, vectors, count, index, offset) }
    }

    /// `recvfrom(buf, length)`, into the peer address.
    /// # Safety
    /// As [`io`].
    pub unsafe fn recvfrom(socket: Socket, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { native_recvfrom(socket, buf.cast(), length) }
    }

    /// `sendto(std::string(host, length), port, data, size)`.
    /// # Safety
    /// `socket` is live; `host` and `data` hold their bytes.
    pub unsafe fn sendto(
        socket: Socket,
        host: *const c_char,
        length: usize,
        port: c_int,
        data: *const c_char,
        size: usize,
    ) -> isize {
        // SAFETY: per the caller.
        unsafe { (native().sendto)(socket, host, length, port, data, size) }
    }

    /// `sendfile(file, offset, length)`.
    /// # Safety
    /// `socket` is live and `file` NUL-terminated.
    pub unsafe fn sendfile(socket: Socket, file: *const c_char, offset: i64, length: i64) -> bool {
        // SAFETY: per the caller.
        unsafe { (native().sendfile)(socket, file, offset, length) }
    }

    /// `bind(std::string(host, length), port)`.
    /// # Safety
    /// `socket` is live and `host` holds `length` bytes.
    pub unsafe fn bind(socket: Socket, host: *const c_char, length: usize, port: c_int) -> bool {
        // SAFETY: per the caller.
        unsafe { (native().bind)(socket, host, length, port) }
    }

    /// `listen(backlog)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn listen(socket: Socket, backlog: c_int) -> bool {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::listen(socket, backlog) }
    }

    /// `accept(timeout)`: the new connection, or null.
    /// # Safety
    /// `socket` is live, in a coroutine.
    pub unsafe fn accept(socket: Socket, timeout: f64) -> Socket {
        // SAFETY: per the caller.
        unsafe { (native().accept)(socket, timeout) }
    }

    /// `ssl_handshake()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn ssl_handshake(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { (native().ssl_handshake)(socket) }
    }

    /// The `SSLContext`'s owner, or null while TLS is off.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn ssl_context(socket: Socket) -> *const c_void {
        // SAFETY: per the caller.
        unsafe { (native().ssl_context)(socket) }
    }

    /// `set_http_proxy` (`http`) or `set_socks5_proxy`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn proxy(
        socket: Socket,
        http: bool,
        host: &[u8],
        port: c_int,
        user: &[u8],
        password: &[u8],
    ) {
        // SAFETY: per the caller; each slice holds its bytes.
        unsafe {
            (native().proxy)(
                socket,
                http,
                host.as_ptr().cast(),
                host.len(),
                port,
                user.as_ptr().cast(),
                user.len(),
                password.as_ptr().cast(),
                password.len(),
            );
        }
    }

    /// `set_option(level, name, value)`, which logs a failure.
    /// # Safety
    /// `socket` is live and open.
    pub unsafe fn set_option(socket: Socket, level: c_int, name: c_int, value: c_int) -> bool {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::set_int_option(socket, level, name, value) }
    }

    /// `getsockname()`, into the network socket's address.
    /// # Safety
    /// `socket` is live and open.
    pub unsafe fn getsockname(socket: Socket) -> bool {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::getsockname(socket) }
    }

    /// `getpeername(address)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn getpeername(
        socket: Socket,
        address: &mut swoole_rs_reactor::SocketAddress,
    ) -> bool {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::getpeername(socket, address) }
    }

    /// `shutdown(how)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn shutdown(socket: Socket, how: c_int) -> bool {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: its live record.
        unsafe { swr_v1_hook_co_socket_shutdown(record, how) }
    }

    /// `cancel(event)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn cancel(socket: Socket, event: u32) -> bool {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: its live record.
        unsafe { swr_v1_hook_co_socket_cancel(record, event) }
    }

    /// `check_liveness()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn check_liveness(socket: Socket) -> bool {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: its live record.
        unsafe { swr_v1_hook_co_socket_check_liveness(record) }
    }

    /// `get_bound_cid(event)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn bound_cid(socket: Socket, event: u32) -> i64 {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: its live record.
        unsafe { swr_v1_hook_co_socket_bound_cid(record, event) }
    }

    /// `set_err(code)`: the code's message.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn set_err(socket: Socket, code: c_int) {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: its live record.
        unsafe { swr_v1_hook_co_socket_set_err(record, code) };
    }

    /// `set_err(code, message)`, copying the message.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn set_error(socket: Socket, code: c_int, message: &[u8]) {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: its live record.
        let status = unsafe {
            swr_v1_hook_co_socket_owned_err(record, code, message.as_ptr().cast(), message.len())
        };
        if status != 0 {
            // std::bad_alloc, which upstream leaves uncaught.
            std::process::abort();
        }
    }

    /// `SW_EVENT_WRITE`.
    pub const WRITE: u32 = 1 << 10;

    /// A `SocketImpl *`.
    pub type Socket = *mut c_void;

    /// The socket's record.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn record<'a>(socket: Socket) -> &'a mut CoSocket {
        // SAFETY: per the caller; the native cast returns the embedded record.
        let record = unsafe { (native().base)(socket) };
        // SAFETY: as above.
        unsafe { &mut *record }
    }

    /// `close()`; the socket may be gone after it returns true.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn close(socket: Socket) -> bool {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: a live record.
        unsafe { swr_v1_hook_co_socket_close(record) }
    }

    /// `is_connected()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn is_connected(socket: Socket) -> bool {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: a live socket's state handle.
        !record.is_closed() && unsafe { swr_v1_hook_socket_flags(record.state) }.connected
    }

    /// `has_bound(event)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn has_bound(socket: Socket, event: u32) -> bool {
        // SAFETY: per the caller.
        let state = unsafe { record(socket) }.state;
        // SAFETY: a live socket's state handle.
        !unsafe { swr_v1_hook_socket_bound_co(state, event) }.is_null()
    }

    /// `recv_packet(timeout)`.
    /// # Safety
    /// `socket` is live, in a coroutine.
    pub unsafe fn recv_packet(socket: Socket, timeout: f64) -> isize {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::recv_packet(socket, timeout) }
    }

    /// `send_all(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` readable bytes.
    pub unsafe fn send_all(socket: Socket, data: *const c_char, length: usize) -> isize {
        // SAFETY: per the caller; a send only reads the buffer.
        unsafe { (native().io)(socket, Io::SendAll as c_int, data.cast_mut(), length) }
    }

    /// `recv(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` writable bytes.
    pub unsafe fn recv(socket: Socket, data: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { (native().io)(socket, Io::Recv as c_int, data, length) }
    }

    /// `connect(std::string(host, length), port, flags)`.
    /// # Safety
    /// `socket` is live and `host` holds `length` readable bytes.
    pub unsafe fn connect(
        socket: Socket,
        host: *const c_char,
        length: usize,
        port: c_int,
        flags: c_int,
    ) -> bool {
        // SAFETY: per the caller.
        unsafe { (native().connect)(socket, host, length, port, flags) }
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn enable_ssl(socket: Socket) -> bool {
        // SAFETY: per the caller.
        if unsafe { (native().ssl_enabled)(socket) } {
            return false;
        }
        // SAFETY: as above.
        unsafe { (native().ssl_encrypt)(socket) };
        true
    }

    /// `ssl_verify(allow_self_signed)`.
    /// # Safety
    /// `socket` is live and has TLS.
    pub unsafe fn ssl_verify(socket: Socket, allow_self_signed: bool) -> bool {
        // SAFETY: per the caller.
        unsafe { (native().ssl_verify)(socket, allow_self_signed) }
    }

    /// `set_timeout(timeout, kind)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn set_timeout(socket: Socket, timeout: f64, kind: c_int) {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::set_timeout(socket, timeout, kind) };
    }

    /// `get_timeout(kind)`.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn get_timeout(socket: Socket, kind: c_int) -> f64 {
        // SAFETY: per the caller; the member is not virtual.
        unsafe { socket_members::get_timeout(socket, kind) }
    }

    /// `set_dtor`: `hook(arg, socket)` runs once as the socket is destroyed.
    /// # Safety
    /// `socket` is live and `hook` may be called with `arg` until then.
    pub unsafe fn close_hook(
        socket: Socket,
        hook: unsafe extern "C" fn(*mut c_void, *mut c_void),
        arg: *mut c_void,
    ) {
        // SAFETY: per the caller.
        unsafe { (native().close_hook)(socket, hook, arg) };
    }
}
