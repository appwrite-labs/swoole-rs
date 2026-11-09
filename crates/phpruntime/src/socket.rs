//! The coroutine socket (`SocketImpl`), which crates/hooks makes, runs and frees;
//! what the streams need of Swoole's PHP side is in the runtime's
//! [`Ops`](crate::Ops), declared in `native/hooks/swoole_rs_phpruntime.h`.
use libc::{c_char, c_int, c_void, size_t, sockaddr, socklen_t};
use std::{ffi::CStr, ptr::NonNull};
use swoole_rs_hooks::{
    co_io::swr_v1_hook_co_socket_set_err,
    co_socket::{CoSocket, swr_v1_hook_co_socket_free, swr_v1_hook_co_socket_make},
    connect::swr_v1_hook_co_socket_connect_host,
    lifecycle::{
        swr_v1_hook_co_socket_check_liveness, swr_v1_hook_co_socket_close,
        swr_v1_hook_co_socket_shutdown,
    },
    socket_members, socket_tls,
};
use swoole_rs_reactor::{SocketRecord, swr_v1_socket_set_timeout};
use swoole_rs_tls::upstream::{socket_recv_sync, socket_send_sync, socket_ssl_close};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    php_error,
    sys::zval,
};

use crate::ops;

/// `make`'s and `close`'s arguments and results, for [`trap`].
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

unsafe extern "C" {
    /// crates/phpclients' settings over a coroutine socket; `which` 2 applies
    /// the SSL ones (`php_swoole_socket_set_ssl`). Raises a bailout it caught.
    fn swr_v1_phpclients_socket_settings(
        socket: *mut c_void,
        settings: *const zval,
        which: c_int,
    ) -> bool;
}

/// [`set_ssl_body`]'s call.
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

/// `call.socket = new SocketImpl(...)` over a [`SocketCall`]: a body for [`trap`],
/// since the constructor's warnings can run a PHP error handler.
unsafe extern "C" fn make_body(call: *mut c_void) {
    // SAFETY: `trap` passes the caller's live `SocketCall`.
    let call = unsafe { &mut *call.cast::<SocketCall>() };
    call.socket =
        swr_v1_hook_co_socket_make(call.fd, call.domain, call.kind, call.protocol, call.family);
}

/// `call.result = socket->close()` over a [`SocketCall`]: a body for [`trap`],
/// since the close may resume coroutines that run PHP.
unsafe extern "C" fn close_body(call: *mut c_void) {
    // SAFETY: `trap` passes the caller's live `SocketCall`.
    let call = unsafe { &mut *call.cast::<SocketCall>() };
    // SAFETY: the call's live socket; `close` is not virtual.
    call.result = unsafe { swr_v1_hook_co_socket_close(socket_members::record(call.socket)) };
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

/// One `op` transfer of `length` bytes at `buf` on the `SocketImpl` at `socket`:
/// its Rust member, or its `network::Socket`'s for the synchronous pair.
/// # Safety
/// `socket` is live and `buf` holds `length` bytes (writable for the receiving operations).
unsafe fn io(socket: *mut c_void, op: Io, buf: *mut c_char, length: usize) -> isize {
    use socket_members as m;
    let buf = buf.cast::<c_void>();
    match op {
        Io::SendSync | Io::RecvSync => {
            // SAFETY: per the caller, its live record.
            let network = unsafe { (*m::record(socket)).socket };
            if matches!(op, Io::SendSync) {
                // SAFETY: the live network::Socket and the caller's buffer.
                unsafe { socket_send_sync(network, buf, length, 0) }
            } else {
                // SAFETY: as above.
                unsafe { socket_recv_sync(network, buf, length, 0) }
            }
        }
        // SAFETY: per the caller.
        Io::SendAll => unsafe { m::send_all(socket, buf, length) },
        // SAFETY: per the caller.
        Io::SendOnce => unsafe { m::send_once(socket, buf, length) },
        // SAFETY: per the caller.
        Io::Recv => unsafe { m::recv(socket, buf, length) },
        // SAFETY: per the caller.
        Io::RecvOnce => unsafe { m::recv_once(socket, buf, length) },
        // SAFETY: per the caller.
        Io::Peek => unsafe { m::peek(socket, buf, length) },
        // SAFETY: per the caller.
        Io::Send => unsafe { m::send(socket, buf, length) },
        // SAFETY: per the caller.
        Io::RecvAll => unsafe { m::recv_all(socket, buf, length) },
    }
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
            if unsafe { bailout::trap(swr_v1_hook_co_socket_free, self.raw()) }.is_err() {
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
            socket.enable_ssl();
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
        if unsafe { trap(make_body, &mut call) }.is_err() {
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

    /// The record, which the native `coroutine::Socket` is.
    fn co_ptr(&self) -> *mut CoSocket {
        socket_members::record(self.raw())
    }

    /// The live coroutine record, for copying a field out; callers hold it no
    /// longer than that read.
    fn co(&self) -> &CoSocket {
        // SAFETY: the live native object embeds this actual Rust record, and nothing
        // writes it during a caller's one-field read.
        unsafe { &*self.co_ptr() }
    }

    pub(crate) fn fd(&self) -> i32 {
        self.co().sock_fd
    }

    pub(crate) fn domain(&self) -> i32 {
        self.co().sock_domain
    }

    pub(crate) fn socket_type(&self) -> i32 {
        self.co().sock_type
    }

    /// The network record can change during reinitialization; read its pointer on each use.
    /// The closure performs only immediate operations and never suspends or invokes PHP.
    fn with_record<T>(&self, absent: T, f: impl FnOnce(&mut SocketRecord) -> T) -> T {
        let record = self.co().socket;
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
        self.co().err_code
    }

    /// `errMsg`, if set.
    pub(crate) fn message(&self) -> Option<&CStr> {
        let message = self.co().err_msg;
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
        unsafe { io(self.raw(), op, buf, length) }
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
        unsafe { socket_members::recvfrom(self.raw(), buf.cast(), length, addr, addrlen) }
    }

    pub(crate) fn close(&self) -> Result<bool, Bailout> {
        // SAFETY: a live socket.
        unsafe { co::close_trapped(self.raw()) }
    }

    /// `setsockopt(level, name, value)`; `logged` through the coroutine
    /// socket, which logs a failure.
    pub(crate) fn set_option(&self, level: i32, name: i32, value: i32, logged: bool) {
        if logged {
            // SAFETY: a live socket; the member is not virtual.
            let _ = unsafe { socket_members::set_int_option(self.raw(), level, name, value) };
        } else {
            self.with_record(false, |record| {
                swoole_rs_reactor::swr_v1_socket_set_int(record.fd, level, name, value)
            });
        }
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
        // SAFETY: a live socket and its NUL-terminated host.
        unsafe { socket_members::bind_host(self.raw(), host.as_ptr(), host.count_bytes(), port) }
    }

    pub(crate) fn connect(&self, host: &CStr, port: i32) -> bool {
        let host = host.to_bytes();
        // SAFETY: a live socket and `host.len()` readable bytes.
        unsafe {
            swr_v1_hook_co_socket_connect_host(
                self.co_ptr(),
                host.as_ptr().cast(),
                host.len(),
                port,
            )
        }
    }

    pub(crate) fn listen(&self, backlog: i32) -> bool {
        // SAFETY: a live socket.
        unsafe { socket_members::listen(self.raw(), backlog) }
    }

    /// `accept()`: the connection, if any.
    pub(crate) fn accept(&self) -> Option<Self> {
        // SAFETY: a live socket; the connection is new and owned.
        Self::wrap(unsafe { socket_members::accept(self.raw(), 0.0) }, true)
    }

    pub(crate) fn shutdown(&self, how: i32) -> bool {
        // SAFETY: a live socket.
        unsafe { swr_v1_hook_co_socket_shutdown(self.co_ptr(), how) }
    }

    /// `poll(SW_EVENT_WRITE or SW_EVENT_READ, timeout)`.
    pub(crate) fn poll(&self, write: bool, timeout: f64) -> bool {
        let event = if write { co::WRITE } else { co::READ };
        // SAFETY: a live socket.
        unsafe { socket_members::poll(self.raw(), event, timeout) }
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
        !unsafe { socket_tls::owner(self.co_ptr()) }.is_null()
    }

    /// `ssl_is_available()`: the handshake has completed.
    pub(crate) fn ssl_available(&self) -> bool {
        let co = self.co_ptr();
        // SAFETY: a scalar of the live coroutine socket.
        let connected = !unsafe { (*co).socket }.is_null();
        // SAFETY: as above.
        connected && unsafe { (*co).ssl_handshaked }
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    pub(crate) fn enable_ssl(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { socket_tls::enable_ssl(self.co_ptr()) }
    }

    /// The connection's `SSL *`, or null.
    pub(crate) fn ssl(&self) -> *mut openssl_sys::SSL {
        self.with_record(std::ptr::null_mut(), |record| record.ssl.cast())
    }

    pub(crate) fn ssl_handshake(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { socket_tls::ssl_handshake(self.raw()) }
    }

    /// `ssl_close()`; the socket holds an SSL.
    pub(crate) fn ssl_close(&self) {
        // SAFETY: a scalar of the live coroutine socket.
        let network = unsafe { (*self.co_ptr()).socket };
        // SAFETY: its live network::Socket, which holds an SSL.
        unsafe { socket_ssl_close(network) };
    }

    /// The peer certificate's PEM, empty without one.
    pub(crate) fn peer_cert(&self) -> Vec<u8> {
        // SAFETY: a live socket.
        unsafe { socket_members::peer_cert(self.raw()) }
    }

    /// The peer's certificate chain: `ssl_get_peer_cert_chain(INT_MAX)`, each
    /// certificate as PEM, skipping one that fails.
    pub(crate) fn peer_chain(&self) -> Vec<Vec<u8>> {
        use openssl_sys::{OPENSSL_STACK, OPENSSL_sk_num, OPENSSL_sk_value};
        use swoole_rs_tls::upstream::{
            socket_ssl_get_peer_cert_chain, swr_v1_socket_ssl_read_x509,
        };
        self.with_record(Vec::new(), |record| {
            // SAFETY: a live record.
            let chain = unsafe { socket_ssl_get_peer_cert_chain(record) }.cast::<OPENSSL_STACK>();
            if chain.is_null() {
                return Vec::new();
            }
            // SAFETY: the SSL's live chain.
            let count = unsafe { OPENSSL_sk_num(chain) };
            let mut pem = vec![0_u8; swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize];
            (0..count)
                .filter_map(|index| {
                    // SAFETY: an index within the live chain.
                    let cert = unsafe { OPENSSL_sk_value(chain, index) };
                    // SAFETY: that certificate, written into the local buffer.
                    let written = unsafe {
                        swr_v1_socket_ssl_read_x509(
                            cert.cast(),
                            pem.as_mut_ptr().cast(),
                            pem.len(),
                            false,
                        )
                    };
                    let written = usize::try_from(written).ok().filter(|&n| n > 0)?;
                    pem.get(..written).map(<[u8]>::to_vec)
                })
                .collect()
        })
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
        // SAFETY: per the caller; `set_ssl_body` takes an `SslCall`.
        unsafe { trap(set_ssl_body, &mut call) }?;
        Ok(call.result)
    }
}

/// `php_swoole_socket_set_ssl` over an [`SslCall`], a `bailout::trap` body.
/// The socket is a `SocketImpl`, whose `coroutine::Socket` base is at its address.
unsafe extern "C" fn set_ssl_body(call: *mut c_void) {
    // SAFETY: `Socket::set_ssl` passes a live `SslCall`.
    let call = unsafe { &mut *call.cast::<SslCall>() };
    // SAFETY: a live socket and settings array, per `Socket::set_ssl`.
    call.result = unsafe { swr_v1_phpclients_socket_settings(call.socket, call.settings, 2) };
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
    // SAFETY: Swoole's static message table or strerror's text.
    unsafe { CStr::from_ptr(swoole_rs_error::swoole_strerror(code)) }
}

/// `Coroutine::get_current_safe()`: outside a coroutine Swoole raises a fatal
/// error.
/// # Errors
/// The fatal error bailed out.
pub fn coroutine_check() -> Result<(), Bailout> {
    // SAFETY: called while PHP runs; the body ignores its argument.
    unsafe { bailout::trap(ops().coroutine_check, std::ptr::null_mut()) }
}

/// `php_swoole_convert_to_fd(value)`: an fd, or negative after a warning.
/// # Errors
/// The warning's error handler bailed out.
/// # Safety
/// `value` is a live zval.
pub unsafe fn convert_to_fd(value: *mut zval) -> Result<i32, Bailout> {
    let mut call = FdCall { value, fd: -1 };
    // SAFETY: per the caller; `convert_to_fd` takes an `FdCall`.
    unsafe { trap(ops().convert_to_fd, &mut call) }?;
    Ok(call.fd)
}

/// `System::socket_poll` of `(fd, events)` rows merged by fd, in the order
/// `stream_select` added them: `None` on a timeout or failure, else each polled
/// fd and its revents in the poll set's order.
pub(crate) fn poll(rows: &[(i32, i32)], timeout: f64) -> Option<Vec<(i32, i32)>> {
    swoole_rs_hooks::poll::socket_poll_rows(rows, timeout)
}

/// What the client classes (crates/phpclients and crates/phphttp2) need of a
/// `SocketImpl *` beyond the record.
pub mod co {
    pub use super::Io;
    use super::{SocketCall, close_body, make_body, trap};
    use libc::{c_char, c_int, c_void, iovec};
    use swoole_rs_corebase::CoreStringView;
    pub use swoole_rs_hooks::co_socket::CoSocket;
    use swoole_rs_hooks::{
        TimeoutScope,
        co_io::{
            swr_v1_hook_co_socket_bound_cid, swr_v1_hook_co_socket_check_bound,
            swr_v1_hook_co_socket_owned_err, swr_v1_hook_co_socket_set_err,
            swr_v1_hook_co_socket_timed_out,
        },
        co_socket::swr_v1_hook_co_socket_free,
        connect::{self, swr_v1_hook_co_socket_connect_host, swr_v1_hook_co_socket_proxy},
        lifecycle::{
            swr_v1_hook_co_socket_cancel, swr_v1_hook_co_socket_check_liveness,
            swr_v1_hook_co_socket_close, swr_v1_hook_co_socket_shutdown,
        },
        shutdown::{swr_v1_hook_socket_bound_co, swr_v1_hook_socket_flags},
        socket_members, socket_tls, swr_v1_hook_timeout_restore, swr_v1_hook_timeout_scope,
    };
    use swoole_rs_reactor::{
        swr_v1_reactor_iov_destroy, swr_v1_reactor_iov_prepare, swr_v1_reactor_iov_view,
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
        unsafe { trap(make_body, &mut call) }?;
        Ok(call.socket)
    }

    /// `delete socket`.
    /// # Errors
    /// A close hook bailed out.
    /// # Safety
    /// `socket` is live and owned; it is gone after the call.
    pub unsafe fn free(socket: Socket) -> Result<(), Bailout> {
        // SAFETY: per the caller; `free` takes the socket.
        unsafe { swoole_rs_zend::bailout::trap(swr_v1_hook_co_socket_free, socket) }
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
        unsafe { trap(close_body, &mut call) }?;
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

        /// `TimeoutController::has_timedout(kind)`: the first call starts the
        /// clock; later ones shrink the socket's timeout to what is left, or
        /// set `ETIMEDOUT` once none is.
        pub fn timed_out(&mut self, kind: c_int) -> bool {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |now| now.as_secs_f64());
            let scope = &mut self.0;
            // SAFETY: the scope's socket is live, per `set`.
            unsafe {
                swr_v1_hook_co_socket_timed_out(
                    scope.socket,
                    scope.timeout,
                    &raw mut scope.startup,
                    now,
                    kind,
                )
            }
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
        unsafe { super::io(socket, op, buf, length) }
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
    /// `count` vectors, with the vector index and byte offset where it stopped.
    /// # Safety
    /// `socket` is live and the vectors are valid for the operation.
    pub unsafe fn vector(
        socket: Socket,
        op: c_int,
        vectors: *const iovec,
        count: c_int,
    ) -> (isize, c_int, usize) {
        // SAFETY: per the caller, `count` readable vectors.
        let prepared = unsafe { swr_v1_reactor_iov_prepare(vectors, count) };
        if prepared.status != 0 {
            // `IOVector`'s constructor throws here, which ended at this Rust frame.
            std::process::abort();
        }
        // Each member reads only an `IOVector`'s first field, its owner.
        let mut owner = prepared.owner;
        let vector = (&raw mut owner).cast::<c_void>();
        let member: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize = match op {
            0 => socket_members::readv,
            1 => socket_members::readv_all,
            2 => socket_members::writev,
            _ => socket_members::writev_all,
        };
        // SAFETY: per the caller, over the live vector; no subclass overrides the members.
        let result = unsafe { member(socket, vector) };
        // SAFETY: the live owner.
        let state = unsafe { swr_v1_reactor_iov_view(owner) };
        // SAFETY: the owner `prepare` made, released once.
        unsafe { swr_v1_reactor_iov_destroy(owner) };
        (result, state.index, state.offset_bytes)
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
    /// `socket` is live, in a coroutine; `host` and `data` hold their bytes.
    pub unsafe fn sendto(
        socket: Socket,
        host: *const c_char,
        length: usize,
        port: c_int,
        data: *const c_char,
        size: usize,
    ) -> isize {
        // SAFETY: per the caller.
        let host = unsafe { terminated(host, length) };
        // SAFETY: per the caller, with the host's NUL-terminated copy.
        unsafe {
            socket_members::sendto_host(
                socket,
                host.as_ptr().cast(),
                length,
                port,
                data.cast(),
                size,
            )
        }
    }

    /// `std::string(host, length)`: the bytes, NUL-terminated.
    /// # Safety
    /// `host` holds `length` bytes.
    unsafe fn terminated(host: *const c_char, length: usize) -> Vec<u8> {
        // SAFETY: per the caller.
        let mut copy = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) }.to_vec();
        copy.push(0);
        copy
    }

    /// `sendfile(file, offset, length)`.
    /// # Safety
    /// `socket` is live and `file` NUL-terminated.
    pub unsafe fn sendfile(socket: Socket, file: *const c_char, offset: i64, length: i64) -> bool {
        // SAFETY: per the caller.
        let length = usize::try_from(length.cast_unsigned()).unwrap_or(usize::MAX);
        // SAFETY: per the caller; size_t's conversion of the length, as upstream's call.
        unsafe { socket_members::sendfile(socket, file, offset, length) }
    }

    /// `bind(std::string(host, length), port)`.
    /// # Safety
    /// `socket` is live and `host` holds `length` bytes.
    pub unsafe fn bind(socket: Socket, host: *const c_char, length: usize, port: c_int) -> bool {
        // SAFETY: per the caller.
        let host = unsafe { terminated(host, length) };
        // SAFETY: per the caller, with the host's NUL-terminated copy.
        unsafe { socket_members::bind_host(socket, host.as_ptr().cast(), length, port) }
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
        unsafe { socket_members::accept(socket, timeout) }
    }

    /// `ssl_handshake()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn ssl_handshake(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { socket_tls::ssl_handshake(socket) }
    }

    /// The `SSLContext`'s owner, or null while TLS is off.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn ssl_context(socket: Socket) -> *const c_void {
        // SAFETY: per the caller.
        unsafe { socket_tls::owner(socket_members::record(socket)) }.cast()
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
        let view = |text: &[u8]| CoreStringView {
            data: text.as_ptr().cast(),
            length: text.len(),
        };
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: each view lends its bytes for the call.
        unsafe {
            swr_v1_hook_co_socket_proxy(record, http, view(host), port, view(user), view(password));
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

    /// `SW_EVENT_READ`.
    pub const READ: u32 = 1 << 9;
    /// `SW_EVENT_WRITE`.
    pub const WRITE: u32 = 1 << 10;

    /// A `SocketImpl *`.
    pub type Socket = *mut c_void;

    /// The socket's record.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn record<'a>(socket: Socket) -> &'a mut CoSocket {
        // SAFETY: per the caller; the socket is its record.
        unsafe { &mut *socket_members::record(socket) }
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
        unsafe { super::io(socket, Io::SendAll, data.cast_mut(), length) }
    }

    /// `send(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` readable bytes.
    pub unsafe fn send(socket: Socket, data: *const c_char, length: usize) -> isize {
        // SAFETY: per the caller; a send only reads the buffer.
        unsafe { super::io(socket, Io::Send, data.cast_mut(), length) }
    }

    /// `recv(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` writable bytes.
    pub unsafe fn recv(socket: Socket, data: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { super::io(socket, Io::Recv, data, length) }
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
        let _ = flags;
        // SAFETY: per the caller.
        unsafe {
            swr_v1_hook_co_socket_connect_host(socket_members::record(socket), host, length, port)
        }
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn enable_ssl(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { socket_tls::enable_ssl(socket_members::record(socket)) }
    }

    /// `ssl_verify(allow_self_signed)`.
    /// # Safety
    /// `socket` is live and has TLS.
    pub unsafe fn ssl_verify(socket: Socket, allow_self_signed: bool) -> bool {
        // SAFETY: per the caller.
        unsafe { socket_tls::ssl_verify(socket, allow_self_signed) }
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

    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn check_bound(socket: Socket, event: u32) {
        // SAFETY: per the caller.
        let record = unsafe { record(socket) };
        // SAFETY: a live record.
        unsafe { swr_v1_hook_co_socket_check_bound(record, event) };
    }

    /// `ssl_is_enable()`.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn ssl_enabled(socket: Socket) -> bool {
        // SAFETY: per the caller.
        !unsafe { socket_tls::owner(socket_members::record(socket)) }.is_null()
    }

    /// The HTTP proxy's `ProxyState`, or null.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn http_proxy(socket: Socket) -> *mut c_void {
        // SAFETY: per the caller.
        connect::http_proxy(unsafe { record(socket) }).cast()
    }

    /// `ssl_get_peer_cert()`: empty (with the socket's error set) without one.
    /// # Safety
    /// `socket` is live and has TLS.
    #[must_use]
    pub unsafe fn peer_cert(socket: Socket) -> Vec<u8> {
        // SAFETY: per the caller.
        unsafe { socket_members::peer_cert(socket) }
    }

    /// `set_dtor`: `hook(arg, socket)` runs once as the socket is destroyed.
    /// # Safety
    /// `socket` is live and `hook` may be called with `arg` until then.
    pub unsafe fn close_hook(
        socket: Socket,
        hook: unsafe extern "C" fn(*mut c_void, *mut c_void),
        arg: *mut c_void,
    ) {
        let record = socket_members::record(socket);
        // SAFETY: per the caller.
        unsafe { swoole_rs_hooks::socket_extra::set_close_hook(record, socket, hook, arg) };
    }
}
