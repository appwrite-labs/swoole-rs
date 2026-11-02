//! The coroutine socket (`SocketImpl`) is still C++: what the streams need of
//! it, and of Swoole's PHP error reporting, is one call each, defined in the
//! patched `ext-src/swoole_runtime.cc` (declared in
//! `native/hooks/swoole_rs_phpruntime.h`).
use libc::{c_char, c_int, c_void, size_t, sockaddr, socklen_t, ssize_t};
use std::{ffi::CStr, ptr::NonNull};
use swoole_rs_zend::{bailout::Bailout, sys::zval};

unsafe extern "C" {
    fn swoole_runtime_socket_create(kind: c_int, ssl: bool) -> *mut c_void;
    fn swoole_runtime_socket_from_fd(
        fd: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
    ) -> *mut c_void;
    fn swoole_runtime_socket_free(socket: *mut c_void);
    fn swoole_runtime_socket_fd(socket: *mut c_void) -> c_int;
    fn swoole_runtime_socket_domain(socket: *mut c_void) -> c_int;
    fn swoole_runtime_socket_type(socket: *mut c_void) -> c_int;
    fn swoole_runtime_socket_inet(socket: *mut c_void) -> bool;
    fn swoole_runtime_socket_error(socket: *mut c_void, message: *mut *const c_char) -> c_int;
    fn swoole_runtime_socket_set_error(socket: *mut c_void, code: c_int);
    fn swoole_runtime_socket_set_error_code(socket: *mut c_void, code: c_int);
    fn swoole_runtime_socket_io(
        socket: *mut c_void,
        op: c_int,
        buf: *mut c_char,
        length: size_t,
    ) -> ssize_t;
    fn swoole_runtime_socket_catch_error(socket: *mut c_void, write: bool, code: c_int) -> c_int;
    fn swoole_runtime_socket_recvfrom(
        socket: *mut c_void,
        buf: *mut c_char,
        length: size_t,
        addr: *mut sockaddr,
        addrlen: *mut socklen_t,
    ) -> ssize_t;
    fn swoole_runtime_socket_close(socket: *mut c_void) -> bool;
    fn swoole_runtime_socket_option(
        socket: *mut c_void,
        level: c_int,
        name: c_int,
        value: c_int,
        logged: bool,
    );
    fn swoole_runtime_socket_nodelay(socket: *mut c_void, value: c_int);
    fn swoole_runtime_socket_zero_copy(socket: *mut c_void);
    fn swoole_runtime_socket_timeout(socket: *mut c_void, timeout: f64, kind: c_int);
    fn swoole_runtime_socket_bind(socket: *mut c_void, host: *const c_char, port: c_int) -> bool;
    fn swoole_runtime_socket_connect(socket: *mut c_void, host: *const c_char, port: c_int)
    -> bool;
    fn swoole_runtime_socket_listen(socket: *mut c_void, backlog: c_int) -> bool;
    fn swoole_runtime_socket_accept(socket: *mut c_void) -> *mut c_void;
    fn swoole_runtime_socket_shutdown(socket: *mut c_void, how: c_int) -> bool;
    fn swoole_runtime_socket_check_liveness(socket: *mut c_void) -> bool;
    fn swoole_runtime_socket_ssl_enabled(socket: *mut c_void) -> bool;
    fn swoole_runtime_socket_ssl_available(socket: *mut c_void) -> bool;
    fn swoole_runtime_socket_ssl(socket: *mut c_void) -> *mut c_void;
    fn swoole_runtime_socket_ssl_encrypt(socket: *mut c_void);
    fn swoole_runtime_socket_ssl_handshake(socket: *mut c_void) -> bool;
    fn swoole_runtime_socket_ssl_close(socket: *mut c_void);
    fn swoole_runtime_socket_peer_certs(
        socket: *mut c_void,
        chain: bool,
        each: unsafe extern "C" fn(*mut c_void, *const c_char, size_t),
        arg: *mut c_void,
    );
    fn swoole_runtime_socket_set_ssl(socket: *mut c_void, settings: *mut zval) -> c_int;
    fn swoole_runtime_x509_read(pem: *const c_char, length: size_t, retval: *mut zval) -> u8;
    fn swoole_runtime_error(kind: c_int, level: c_int, message: *const c_char) -> u8;
    fn swoole_runtime_check_reactor();
    fn swoole_runtime_fd_socket(fd: c_int, holder: *mut *mut c_void) -> *mut c_void;
    fn swoole_runtime_fd_socket_release(holder: *mut c_void);
    fn swoole_runtime_socket_poll(socket: *mut c_void, write: bool, timeout: f64) -> bool;
    fn swoole_runtime_strerror(code: c_int) -> *const c_char;
    fn swoole_runtime_coroutine_check() -> u8;
    fn swoole_runtime_convert_to_fd(value: *mut zval, fd: *mut c_int) -> u8;
    fn swoole_runtime_poll(
        fds: *const c_int,
        events: *const c_int,
        count: size_t,
        timeout: f64,
        ready: *mut c_int,
        revents: *mut c_int,
        size: *mut size_t,
    ) -> bool;
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

/// `swoole_runtime_socket_io` operations.
#[derive(Clone, Copy)]
pub(crate) enum Io {
    SendSync = 0,
    RecvSync = 1,
    SendAll = 2,
    SendOnce = 3,
    Recv = 4,
    RecvOnce = 5,
    Peek = 6,
    Send = 7,
}

/// `SW_WAIT`.
pub(crate) const SW_WAIT: i32 = 2;

/// A `SocketImpl *`: owned (deleted on drop), or a hooked fd's, kept alive
/// by a `shared_ptr` the drop releases.
pub(crate) struct Socket {
    socket: NonNull<c_void>,
    holder: Option<NonNull<c_void>>,
}

impl Drop for Socket {
    fn drop(&mut self) {
        match self.holder {
            // SAFETY: the holder `swoole_runtime_fd_socket` made, released once.
            Some(holder) => unsafe { swoole_runtime_fd_socket_release(holder.as_ptr()) },
            // SAFETY: the socket is owned and live.
            None => unsafe { swoole_runtime_socket_free(self.raw()) },
        }
    }
}

impl Socket {
    /// `new SocketImpl(kind)`, encrypting with `ssl`; `None` without an fd.
    pub(crate) fn new(kind: i32, ssl: bool) -> Option<Self> {
        // SAFETY: plain construction.
        Self::owned(unsafe { swoole_runtime_socket_create(kind, ssl) })
    }

    /// `new SocketImpl(fd, domain, kind, protocol)`.
    pub(crate) fn from_fd(fd: i32, domain: i32, kind: i32, protocol: i32) -> Option<Self> {
        // SAFETY: plain construction; the socket owns the fd.
        let socket = unsafe { swoole_runtime_socket_from_fd(fd, domain, kind, protocol) };
        Self::wrap(socket, None)
    }

    /// The coroutine socket of hooked `fd`, if any.
    pub(crate) fn of_fd(fd: i32) -> Option<Self> {
        let mut holder = std::ptr::null_mut();
        // SAFETY: a lookup; the holder keeps the socket alive until dropped.
        let socket = unsafe { swoole_runtime_fd_socket(fd, &raw mut holder) };
        Self::wrap(socket, Some(NonNull::new(holder)?))
    }

    fn wrap(socket: *mut c_void, holder: Option<NonNull<c_void>>) -> Option<Self> {
        Some(Self {
            socket: NonNull::new(socket)?,
            holder,
        })
    }

    /// Own `socket`, a fresh `SocketImpl *` or null; `None` when it is null or
    /// has no fd.
    fn owned(socket: *mut c_void) -> Option<Self> {
        let socket = Self::wrap(socket, None)?;
        (socket.fd() >= 0).then_some(socket)
    }

    fn raw(&self) -> *mut c_void {
        self.socket.as_ptr()
    }

    pub(crate) fn fd(&self) -> i32 {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_fd(self.raw()) }
    }

    pub(crate) fn domain(&self) -> i32 {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_domain(self.raw()) }
    }

    pub(crate) fn kind(&self) -> i32 {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_type(self.raw()) }
    }

    pub(crate) fn is_inet(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_inet(self.raw()) }
    }

    /// `errCode`.
    pub(crate) fn error(&self) -> i32 {
        // SAFETY: a live socket; no message wanted.
        unsafe { swoole_runtime_socket_error(self.raw(), std::ptr::null_mut()) }
    }

    /// `errMsg`, if set.
    pub(crate) fn message(&self) -> Option<&CStr> {
        let mut message = std::ptr::null();
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_error(self.raw(), &raw mut message) };
        // SAFETY: errMsg is null or a NUL-terminated string the socket keeps.
        (!message.is_null()).then(|| unsafe { CStr::from_ptr(message) })
    }

    /// `set_err(code)`.
    pub(crate) fn set_error(&self, code: i32) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_set_error(self.raw(), code) };
    }

    /// `errCode = code`.
    pub(crate) fn set_error_code(&self, code: i32) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_set_error_code(self.raw(), code) };
    }

    /// One transfer of `length` bytes at `buf`.
    /// # Safety
    /// `buf` holds `length` bytes (writable for the receiving operations).
    pub(crate) unsafe fn io(&self, op: Io, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { swoole_runtime_socket_io(self.raw(), op as c_int, buf, length) }
    }

    /// `catch_write_error` (`write`) or `catch_read_error` of `code`.
    pub(crate) fn catch_error(&self, write: bool, code: i32) -> i32 {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_catch_error(self.raw(), write, code) }
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
        unsafe { swoole_runtime_socket_recvfrom(self.raw(), buf, length, addr, addrlen) }
    }

    pub(crate) fn close(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_close(self.raw()) }
    }

    /// `setsockopt(level, name, value)`; `logged` through the coroutine
    /// socket, which logs a failure.
    pub(crate) fn set_option(&self, level: i32, name: i32, value: i32, logged: bool) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_option(self.raw(), level, name, value, logged) };
    }

    pub(crate) fn set_nodelay(&self, value: i32) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_nodelay(self.raw(), value) };
    }

    pub(crate) fn set_zero_copy(&self) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_zero_copy(self.raw()) };
    }

    pub(crate) fn set_timeout(&self, seconds: f64, kind: i32) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_timeout(self.raw(), seconds, kind) };
    }

    pub(crate) fn bind(&self, host: &CStr, port: i32) -> bool {
        // SAFETY: a live socket and a NUL-terminated host.
        unsafe { swoole_runtime_socket_bind(self.raw(), host.as_ptr(), port) }
    }

    pub(crate) fn connect(&self, host: &CStr, port: i32) -> bool {
        // SAFETY: a live socket and a NUL-terminated host.
        unsafe { swoole_runtime_socket_connect(self.raw(), host.as_ptr(), port) }
    }

    pub(crate) fn listen(&self, backlog: i32) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_listen(self.raw(), backlog) }
    }

    /// `accept()`: the connection, if any.
    pub(crate) fn accept(&self) -> Option<Self> {
        // SAFETY: a live socket; the connection is new and owned.
        Self::wrap(unsafe { swoole_runtime_socket_accept(self.raw()) }, None)
    }

    pub(crate) fn shutdown(&self, how: i32) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_shutdown(self.raw(), how) }
    }

    /// `poll(SW_EVENT_WRITE or SW_EVENT_READ, timeout)`.
    pub(crate) fn poll(&self, write: bool, timeout: f64) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_poll(self.raw(), write, timeout) }
    }

    /// `get_err()`: `swoole_strerror(errCode)`.
    pub(crate) fn strerror(&self) -> &'static CStr {
        strerror(self.error())
    }

    pub(crate) fn check_liveness(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_check_liveness(self.raw()) }
    }

    /// `ssl_is_enable()`: encryption is configured.
    pub(crate) fn ssl_enabled(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_ssl_enabled(self.raw()) }
    }

    /// `ssl_is_available()`: the handshake has completed.
    pub(crate) fn ssl_available(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_ssl_available(self.raw()) }
    }

    /// The connection's `SSL *`, or null.
    pub(crate) fn ssl(&self) -> *mut openssl_sys::SSL {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_ssl(self.raw()) }.cast()
    }

    pub(crate) fn ssl_encrypt(&self) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_ssl_encrypt(self.raw()) };
    }

    pub(crate) fn ssl_handshake(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_ssl_handshake(self.raw()) }
    }

    pub(crate) fn ssl_close(&self) {
        // SAFETY: a live socket.
        unsafe { swoole_runtime_socket_ssl_close(self.raw()) };
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
            swoole_runtime_socket_peer_certs(self.raw(), chain, each, (&raw mut certs).cast());
        }
        certs
    }

    /// `php_swoole_socket_set_ssl(socket, settings)`.
    /// # Errors
    /// A warning's error handler bailed out.
    /// # Safety
    /// `settings` is a live array.
    pub(crate) unsafe fn set_ssl(&self, settings: *mut zval) -> Result<bool, Bailout> {
        // SAFETY: per the caller.
        match unsafe { swoole_runtime_socket_set_ssl(self.raw(), settings) } {
            -1 => Err(Bailout),
            result => Ok(result != 0),
        }
    }
}

/// `openssl_x509_read(pem)` into `retval`, which the caller then owns.
/// # Errors
/// The call bailed out.
/// # Safety
/// `retval` is writable.
pub(crate) unsafe fn x509_read(pem: &[u8], retval: *mut zval) -> Result<(), Bailout> {
    // SAFETY: per the caller; `pem` holds its bytes.
    if unsafe { swoole_runtime_x509_read(pem.as_ptr().cast(), pem.len(), retval) } != 0 {
        return Err(Bailout);
    }
    Ok(())
}

/// How a warning is raised.
#[derive(Clone, Copy)]
pub(crate) enum Report {
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
pub(crate) fn report(kind: Report, level: i32, message: &CStr) -> Result<(), Bailout> {
    // SAFETY: called while PHP runs; both strings are NUL-terminated.
    let bailed = unsafe { swoole_runtime_error(kind as c_int, level, message.as_ptr()) };
    if bailed != 0 {
        return Err(Bailout);
    }
    Ok(())
}

/// `php_swoole_check_reactor()`.
pub(crate) fn check_reactor() {
    // SAFETY: called while PHP runs.
    unsafe { swoole_runtime_check_reactor() };
}

/// `swoole_strerror(code)`.
pub(crate) fn strerror(code: i32) -> &'static CStr {
    // SAFETY: a lookup.
    let message = unsafe { swoole_runtime_strerror(code) };
    // SAFETY: Swoole's static message table or strerror's text.
    unsafe { CStr::from_ptr(message) }
}

/// `Coroutine::get_current_safe()`: outside a coroutine Swoole raises a fatal
/// error.
/// # Errors
/// The fatal error bailed out.
pub(crate) fn coroutine_check() -> Result<(), Bailout> {
    // SAFETY: called while PHP runs.
    if unsafe { swoole_runtime_coroutine_check() } != 0 {
        return Err(Bailout);
    }
    Ok(())
}

/// `php_swoole_convert_to_fd(value)`: an fd, or negative after a warning.
/// # Errors
/// The warning's error handler bailed out.
/// # Safety
/// `value` is a live zval.
pub(crate) unsafe fn convert_to_fd(value: *mut zval) -> Result<i32, Bailout> {
    let mut fd = -1;
    // SAFETY: per the caller.
    if unsafe { swoole_runtime_convert_to_fd(value, &raw mut fd) } != 0 {
        return Err(Bailout);
    }
    Ok(fd)
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
        swoole_runtime_poll(
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
