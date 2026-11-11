//! The coroutine socket (`SocketImpl`), which crates/hooks makes, runs and
//! frees, and what the streams need of Swoole's PHP side.
use libc::{c_char, c_int, c_void, size_t, sockaddr, socklen_t};
use std::{ffi::CStr, ptr::NonNull};
use swoole_rs_corebase::log::set_last_error;
use swoole_rs_hooks::co_socket::{Co, swr_v1_hook_co_socket_free};
use swoole_rs_reactor::SocketRecord;
use swoole_rs_tls::upstream::{socket_recv_sync, socket_send_sync, socket_ssl_close};
use swoole_rs_zend::{
    E_ERROR,
    bailout::{self, Bailout},
    sys::{swr_zval_stringl, zval, zval_ptr_dtor},
};

// crates/phpevents' functions: declared, since crates/phpevents depends on this crate.
unsafe extern "C" {
    /// crates/phpevents.
    fn php_swoole_convert_to_fd(value: *mut zval) -> c_int;
}

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

/// [`set_ssl_body`]'s call.
struct SslCall {
    socket: *mut c_void,
    settings: *mut zval,
    result: bool,
}

/// [`x509_read`]'s arguments.
struct X509Call {
    pem: *const c_char,
    length: size_t,
    retval: *mut zval,
}

/// [`convert_to_fd`]'s argument and result.
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
    let (made, result) = swoole_rs_hooks::co_socket::make_quiet(
        call.fd,
        call.domain,
        call.kind,
        call.protocol,
        call.family,
    );
    // The caller owns the socket before a warning's handler can bail out.
    call.socket = made.this();
    swoole_rs_hooks::co_socket::warn_created(made, result);
}

/// `call.result = socket->close()` over a [`SocketCall`]: a body for [`trap`],
/// since the close may resume coroutines that run PHP.
unsafe extern "C" fn close_body(call: *mut c_void) {
    // SAFETY: `trap` passes the caller's live `SocketCall`.
    let call = unsafe { &mut *call.cast::<SocketCall>() };
    // SAFETY: the call's live socket.
    call.result = unsafe { Co::of(call.socket) }.close();
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

/// The caller's `length` bytes at `buf`.
/// # Safety
/// `buf` holds `length` readable bytes, or `length` is 0.
unsafe fn bytes<'a>(buf: *const c_char, length: usize) -> &'a [u8] {
    if length == 0 {
        return &[];
    }
    // SAFETY: per the caller.
    unsafe { std::slice::from_raw_parts(buf.cast(), length) }
}

/// The caller's `length` writable bytes at `buf`.
/// # Safety
/// `buf` holds `length` writable bytes, or `length` is 0.
unsafe fn bytes_mut<'a>(buf: *mut c_char, length: usize) -> &'a mut [u8] {
    if length == 0 {
        return &mut [];
    }
    // SAFETY: per the caller.
    unsafe { std::slice::from_raw_parts_mut(buf.cast(), length) }
}

/// One `op` transfer of `length` bytes at `buf` on `socket`: its Rust member, or
/// its `network::Socket`'s for the synchronous pair.
/// # Safety
/// `buf` holds `length` bytes (writable for the receiving operations).
unsafe fn io(socket: Co, op: Io, buf: *mut c_char, length: usize) -> isize {
    let network = socket.socket();
    // SAFETY: per the caller.
    let data = || unsafe { bytes(buf, length) };
    // SAFETY: per the caller.
    let out = || unsafe { bytes_mut(buf, length) };
    match op {
        // SAFETY: the live network::Socket and the caller's buffer.
        Io::SendSync => unsafe { socket_send_sync(network, buf.cast(), length, 0) },
        // SAFETY: as above.
        Io::RecvSync => unsafe { socket_recv_sync(network, buf.cast(), length, 0) },
        Io::SendAll => socket.send_all(data()),
        Io::SendOnce => socket.send_once(data()),
        Io::Recv => socket.recv(out()),
        Io::RecvOnce => socket.recv_once(out()),
        Io::Peek => socket.peek(out()),
        Io::Send => socket.send(data()),
        Io::RecvAll => socket.recv_all(out()),
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
        let trapped = unsafe { trap(make_body, &mut call) };
        // Owns, so a bailout frees, the socket `make` wrote before it.
        let socket = Self::wrap(call.socket, true);
        if trapped.is_err() {
            Bailout.defer();
            return None;
        }
        socket
    }

    /// The coroutine socket of hooked `fd`, if any.
    pub(crate) fn of_fd(fd: i32) -> Option<Self> {
        Self::wrap(swoole_rs_hooks::socket_map::get(fd), false)
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

    /// The socket's handle.
    fn co(&self) -> Co {
        // SAFETY: the socket is live while this owns or borrows it.
        unsafe { Co::of(self.raw()) }
    }

    pub(crate) fn fd(&self) -> i32 {
        self.co().sock_fd()
    }

    pub(crate) fn domain(&self) -> i32 {
        self.co().sock_domain()
    }

    pub(crate) fn socket_type(&self) -> i32 {
        self.co().sock_type()
    }

    /// The network record can change during reinitialization; read its pointer on each use.
    /// The closure performs only immediate operations and never suspends or invokes PHP.
    fn with_record<T>(&self, absent: T, f: impl FnOnce(&mut SocketRecord) -> T) -> T {
        self.co().net(f).unwrap_or(absent)
    }

    pub(crate) fn is_inet(&self) -> bool {
        self.with_record(false, |record| record.is_inet())
    }

    /// `errCode`.
    pub(crate) fn error(&self) -> i32 {
        self.co().err_code()
    }

    /// `errMsg`, if set.
    pub(crate) fn message(&self) -> Option<&CStr> {
        let message = self.co().err_msg();
        // SAFETY: errMsg is null or a NUL-terminated string the socket keeps.
        (!message.is_null()).then(|| unsafe { CStr::from_ptr(message) })
    }

    /// `set_err(code)`.
    pub(crate) fn set_error(&self, code: i32) {
        self.co().set_err(code);
    }

    /// `errCode = code`.
    pub(crate) fn set_error_code(&self, code: i32) {
        self.co().set_err_code(code);
    }

    /// One transfer of `length` bytes at `buf`.
    /// # Safety
    /// `buf` holds `length` bytes (writable for the receiving operations).
    pub(crate) unsafe fn io(&self, op: Io, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { io(self.co(), op, buf, length) }
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
        unsafe { self.co().recvfrom(buf.cast(), length, addr, addrlen) }
    }

    pub(crate) fn close(&self) -> Result<bool, Bailout> {
        // SAFETY: a live socket.
        unsafe { co::close_trapped(self.raw()) }
    }

    /// `setsockopt(level, name, value)`; `logged` through the coroutine
    /// socket, which logs a failure.
    pub(crate) fn set_option(&self, level: i32, name: i32, value: i32, logged: bool) {
        if logged {
            // A failure is logged, which is all upstream does with it.
            let _logged = self.co().set_int_option(level, name, value);
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
        self.co().set_zero_copy(true);
    }

    pub(crate) fn set_timeout(&self, seconds: f64, kind: i32) {
        self.co().set_timeout(seconds, kind);
    }

    pub(crate) fn bind(&self, host: &CStr, port: i32) -> bool {
        self.co().bind(host.to_bytes(), port)
    }

    pub(crate) fn connect(&self, host: &CStr, port: i32) -> bool {
        self.co().connect_host(host.to_bytes(), port)
    }

    pub(crate) fn listen(&self, backlog: i32) -> bool {
        self.co().listen(backlog)
    }

    /// `accept()`: the connection, if any.
    pub(crate) fn accept(&self) -> Option<Self> {
        Self::wrap(self.co().accept(0.0)?.this(), true)
    }

    pub(crate) fn shutdown(&self, how: i32) -> bool {
        self.co().shutdown(how)
    }

    /// `poll(SW_EVENT_WRITE or SW_EVENT_READ, timeout)`.
    pub(crate) fn poll(&self, write: bool, timeout: f64) -> bool {
        let event = if write { co::WRITE } else { co::READ };
        self.co().poll(event, timeout)
    }

    /// `get_err()`: `swoole_strerror(errCode)`.
    pub(crate) fn strerror(&self) -> &'static CStr {
        strerror(self.error())
    }

    pub(crate) fn check_liveness(&self) -> bool {
        self.co().check_liveness()
    }

    /// `ssl_is_enable()`: encryption is configured.
    pub(crate) fn ssl_enabled(&self) -> bool {
        !self.co().tls_owner().is_null()
    }

    /// `ssl_is_available()`: the handshake has completed.
    pub(crate) fn ssl_available(&self) -> bool {
        let co = self.co();
        !co.socket().is_null() && co.ssl_handshaked()
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    pub(crate) fn enable_ssl(&self) -> bool {
        self.co().enable_ssl()
    }

    /// The connection's `SSL *`, or null.
    pub(crate) fn ssl(&self) -> *mut openssl_sys::SSL {
        self.with_record(std::ptr::null_mut(), |record| record.ssl.cast())
    }

    pub(crate) fn ssl_handshake(&self) -> bool {
        self.co().ssl_handshake()
    }

    /// `ssl_close()`; the socket holds an SSL.
    pub(crate) fn ssl_close(&self) {
        // SAFETY: the live network::Socket.
        if let Some(socket) = unsafe { swoole_rs_reactor::Socket::from_ptr(self.co().socket()) } {
            socket_ssl_close(socket);
        }
    }

    /// The peer certificate's PEM, empty without one.
    pub(crate) fn peer_cert(&self) -> Vec<u8> {
        self.co().peer_cert()
    }
    /// The peer's certificate chain: `ssl_get_peer_cert_chain(INT_MAX)`, each
    /// certificate as PEM, skipping one that fails.
    pub(crate) fn peer_chain(&self) -> Vec<Vec<u8>> {
        use openssl_sys::{OPENSSL_STACK, OPENSSL_sk_num, OPENSSL_sk_value};
        use swoole_rs_tls::upstream::{
            socket_ssl_get_peer_cert_chain, swr_v1_socket_ssl_read_x509,
        };
        self.with_record(Vec::new(), |record| {
            let chain = socket_ssl_get_peer_cert_chain(record).cast::<OPENSSL_STACK>();
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
    let Some(families) = swoole_rs_phpswoole::families() else {
        return;
    };
    // SAFETY: a live socket and settings array, per `Socket::set_ssl`; crates/phpclients'
    // settings apply the SSL ones (2).
    call.result = unsafe { (families.socket_settings)(call.socket, call.settings, 2) };
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
    // SAFETY: per the caller; `pem` holds its bytes; the body takes an `X509Call`.
    unsafe { trap(x509_read_body, &mut call) }
}

/// `openssl_x509_read(pem)` over an [`X509Call`], a `bailout::trap` body.
unsafe extern "C" fn x509_read_body(call: *mut c_void) {
    // SAFETY: `x509_read` passes a live `X509Call`.
    let call = unsafe { &mut *call.cast::<X509Call>() };
    // SAFETY: zero is an undefined zval, which the next line fills.
    let mut pem: zval = unsafe { std::mem::zeroed() };
    // SAFETY: the caller's bytes, copied into a new string.
    unsafe { swr_zval_stringl(&raw mut pem, call.pem, call.length) };
    let name = c"openssl_x509_read";
    // SAFETY: one live argument and the caller's writable result.
    unsafe {
        swoole_rs_zend::host::named_call_into(
            name.as_ptr(),
            name.count_bytes(),
            1,
            &raw mut pem,
            call.retval,
        );
    }
    swoole_rs_zend::bailout::finish();
    // SAFETY: the string this body made.
    unsafe { zval_ptr_dtor(&raw mut pem) };
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
    /// `SW_ERROR_PHP_FATAL_ERROR` and `SW_ERROR_PHP_RUNTIME_NOTICE`.
    const FATAL: i32 = 503;
    const NOTICE: i32 = 510;
    match kind {
        Report::Docref => {}
        Report::Fatal => set_last_error(FATAL),
        Report::Swoole => {
            set_last_error(NOTICE);
            if !swoole_rs_phpswoole::display_errors() && level != E_ERROR {
                return Ok(());
            }
        }
    }
    swoole_rs_zend::report(level, message)
}

pub use swoole_rs_phpswoole::check_reactor;

/// `swoole_strerror(code)`.
#[must_use]
pub fn strerror(code: i32) -> &'static CStr {
    // SAFETY: Swoole's static message table or strerror's text.
    unsafe { CStr::from_ptr(swoole_rs_error_ffi::swoole_strerror(code)) }
}

/// `Coroutine::get_current_safe()`: outside a coroutine Swoole raises a fatal
/// error.
/// # Errors
/// The fatal error bailed out.
pub fn coroutine_check() -> Result<(), Bailout> {
    // SAFETY: called while PHP runs; the body ignores its argument.
    unsafe { bailout::trap(coroutine_check_body, std::ptr::null_mut()) }
}

/// [`coroutine_check`]'s `bailout::trap` body.
extern "C" fn coroutine_check_body(_: *mut c_void) {
    swoole_rs_phpswoole::coroutine_check();
}

/// `php_swoole_convert_to_fd(value)`: an fd, or negative after a warning.
/// # Errors
/// The warning's error handler bailed out.
/// # Safety
/// `value` is a live zval.
pub unsafe fn convert_to_fd(value: *mut zval) -> Result<i32, Bailout> {
    let mut call = FdCall { value, fd: -1 };
    // SAFETY: per the caller; the body takes an `FdCall`.
    unsafe { trap(convert_to_fd_body, &mut call) }?;
    Ok(call.fd)
}

/// `php_swoole_convert_to_fd` over an [`FdCall`], a `bailout::trap` body.
unsafe extern "C" fn convert_to_fd_body(call: *mut c_void) {
    // SAFETY: `convert_to_fd` passes a live `FdCall`.
    let call = unsafe { &mut *call.cast::<FdCall>() };
    // SAFETY: the caller's live zval.
    call.fd = unsafe { php_swoole_convert_to_fd(call.value) };
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
    pub use swoole_rs_hooks::co_socket::CoSocket;
    use swoole_rs_hooks::{
        TimeoutScope,
        co_socket::{Co, swr_v1_hook_co_socket_free},
    };
    use swoole_rs_reactor::{
        swr_v1_reactor_iov_destroy, swr_v1_reactor_iov_prepare, swr_v1_reactor_iov_view,
    };
    use swoole_rs_zend::bailout::Bailout;

    /// The live socket's handle.
    /// # Safety
    /// `socket` is live.
    unsafe fn handle(socket: Socket) -> Co {
        // SAFETY: per the caller.
        unsafe { Co::of(socket) }
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
        if unsafe { trap(make_body, &mut call) }.is_err() {
            // Frees the socket `make` wrote before the bailout, if any.
            drop(super::Socket::wrap(call.socket, true));
            return Err(Bailout);
        }
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
            Self(TimeoutScope::new(unsafe { handle(socket) }, timeout, kind))
        }

        /// `TimeoutController::has_timedout(kind)`: the first call starts the
        /// clock; later ones shrink the socket's timeout to what is left, or
        /// set `ETIMEDOUT` once none is.
        pub fn timed_out(&mut self, kind: c_int) -> bool {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |now| now.as_secs_f64());
            self.0.timed_out(kind, now)
        }
    }

    /// One table transfer of `length` bytes at `buf`.
    /// # Safety
    /// `socket` is live and `buf` holds `length` bytes, writable to receive.
    pub unsafe fn io(socket: Socket, op: Io, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        let socket = unsafe { handle(socket) };
        // SAFETY: per the caller.
        unsafe { super::io(socket, op, buf, length) }
    }

    /// `recv_line(buf, length)`.
    /// # Safety
    /// As [`io`].
    pub unsafe fn recv_line(socket: Socket, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        let socket = unsafe { handle(socket) };
        // SAFETY: per the caller.
        unsafe { socket.recv_line(buf.cast(), length) }
    }

    /// `recv_with_buffer(buf, length)`.
    /// # Safety
    /// As [`io`].
    pub unsafe fn recv_with_buffer(socket: Socket, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        let socket = unsafe { handle(socket) };
        // SAFETY: per the caller.
        unsafe { socket.recv_with_buffer(buf.cast(), length) }
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
        let member = match op {
            0 => Co::readv,
            1 => Co::readv_all,
            2 => Co::writev,
            _ => Co::writev_all,
        };
        // SAFETY: per the caller.
        let socket = unsafe { handle(socket) };
        // SAFETY: per the caller, over the live vector.
        let result = unsafe { member(socket, vector) };
        // SAFETY: the live owner.
        let state = swr_v1_reactor_iov_view(unsafe { &*owner });
        // SAFETY: the owner `prepare` made, released once.
        unsafe { swr_v1_reactor_iov_destroy(owner) };
        (result, state.index, state.offset_bytes)
    }

    /// `recvfrom(buf, length)`, into the peer address.
    /// # Safety
    /// As [`io`].
    pub unsafe fn recvfrom(socket: Socket, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        let socket = unsafe { handle(socket) };
        // SAFETY: per the caller.
        socket.recvfrom_peer(unsafe { super::bytes_mut(buf, length) })
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
        let host = unsafe { super::bytes(host, length) };
        // SAFETY: per the caller.
        let data = unsafe { super::bytes(data, size) };
        // SAFETY: per the caller.
        unsafe { handle(socket) }.sendto(host, port, data)
    }

    /// `sendfile(file, offset, length)`.
    /// # Safety
    /// `socket` is live and `file` NUL-terminated.
    pub unsafe fn sendfile(socket: Socket, file: *const c_char, offset: i64, length: i64) -> bool {
        // size_t's conversion of the length, as upstream's call.
        let length = usize::try_from(length.cast_unsigned()).unwrap_or(usize::MAX);
        // SAFETY: per the caller.
        let file = unsafe { std::ffi::CStr::from_ptr(file) };
        // SAFETY: per the caller.
        unsafe { handle(socket) }.sendfile(file, offset, length)
    }

    /// `bind(std::string(host, length), port)`.
    /// # Safety
    /// `socket` is live and `host` holds `length` bytes.
    pub unsafe fn bind(socket: Socket, host: *const c_char, length: usize, port: c_int) -> bool {
        // SAFETY: per the caller.
        let host = unsafe { super::bytes(host, length) };
        // SAFETY: per the caller.
        unsafe { handle(socket) }.bind(host, port)
    }

    /// `listen(backlog)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn listen(socket: Socket, backlog: c_int) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.listen(backlog)
    }

    /// `accept(timeout)`: the new connection, or null.
    /// # Safety
    /// `socket` is live, in a coroutine.
    pub unsafe fn accept(socket: Socket, timeout: f64) -> Socket {
        // SAFETY: per the caller.
        unsafe { handle(socket) }
            .accept(timeout)
            .map_or(std::ptr::null_mut(), Co::this)
    }

    /// `ssl_handshake()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn ssl_handshake(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.ssl_handshake()
    }

    /// The `SSLContext`'s owner, or null while TLS is off.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn ssl_context(socket: Socket) -> *const c_void {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.tls_owner().cast()
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
        // SAFETY: per the caller.
        unsafe { handle(socket) }.set_proxy(http, host, port, user, password);
    }

    /// `set_option(level, name, value)`, which logs a failure.
    /// # Safety
    /// `socket` is live and open.
    pub unsafe fn set_option(socket: Socket, level: c_int, name: c_int, value: c_int) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.set_int_option(level, name, value)
    }

    /// `getsockname()`, into the network socket's address.
    /// # Safety
    /// `socket` is live and open.
    pub unsafe fn getsockname(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.getsockname()
    }

    /// `getpeername(address)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn getpeername(
        socket: Socket,
        address: &mut swoole_rs_reactor::SocketAddress,
    ) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.getpeername(address)
    }

    /// `shutdown(how)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn shutdown(socket: Socket, how: c_int) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.shutdown(how)
    }

    /// `cancel(event)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn cancel(socket: Socket, event: u32) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.cancel(event)
    }

    /// `check_liveness()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn check_liveness(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.check_liveness()
    }

    /// `get_bound_cid(event)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn bound_cid(socket: Socket, event: u32) -> i64 {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.bound_cid(event)
    }

    /// `set_err(code)`: the code's message.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn set_err(socket: Socket, code: c_int) {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.set_err(code);
    }

    /// `set_err(code, message)`, copying the message.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn set_error(socket: Socket, code: c_int, message: &[u8]) {
        // SAFETY: per the caller.
        if unsafe { handle(socket) }.set_error(code, message) != 0 {
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
        unsafe { &mut *socket.cast::<CoSocket>() }
    }

    /// `close()`; the socket may be gone after it returns true.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn close(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.close()
    }

    /// `is_connected()`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn is_connected(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.is_connected()
    }

    /// `has_bound(event)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn has_bound(socket: Socket, event: u32) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.has_bound(event)
    }

    /// `recv_packet(timeout)`.
    /// # Safety
    /// `socket` is live, in a coroutine.
    pub unsafe fn recv_packet(socket: Socket, timeout: f64) -> isize {
        // SAFETY: per the caller.
        swoole_rs_hooks::packet::recv_packet(unsafe { handle(socket) }, timeout)
    }

    /// `send_all(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` readable bytes.
    pub unsafe fn send_all(socket: Socket, data: *const c_char, length: usize) -> isize {
        // SAFETY: per the caller; a send only reads the buffer.
        unsafe { io(socket, Io::SendAll, data.cast_mut(), length) }
    }

    /// `send(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` readable bytes.
    pub unsafe fn send(socket: Socket, data: *const c_char, length: usize) -> isize {
        // SAFETY: per the caller; a send only reads the buffer.
        unsafe { io(socket, Io::Send, data.cast_mut(), length) }
    }

    /// `recv(data, length)`.
    /// # Safety
    /// `socket` is live and `data` holds `length` writable bytes.
    pub unsafe fn recv(socket: Socket, data: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { io(socket, Io::Recv, data, length) }
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
        // SAFETY: per the caller; copied before any effect.
        let host = unsafe { super::bytes(host, length) }.to_vec();
        // SAFETY: per the caller.
        unsafe { handle(socket) }.connect_host(&host, port)
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn enable_ssl(socket: Socket) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.enable_ssl()
    }

    /// `ssl_verify(allow_self_signed)`.
    /// # Safety
    /// `socket` is live and has TLS.
    pub unsafe fn ssl_verify(socket: Socket, allow_self_signed: bool) -> bool {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.ssl_verify(allow_self_signed)
    }

    /// `set_timeout(timeout, kind)`.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn set_timeout(socket: Socket, timeout: f64, kind: c_int) {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.set_timeout(timeout, kind);
    }

    /// `get_timeout(kind)`.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn get_timeout(socket: Socket, kind: c_int) -> f64 {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.timeout(kind)
    }

    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    /// # Safety
    /// `socket` is live.
    pub unsafe fn check_bound(socket: Socket, event: u32) {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.check_bound(event);
    }

    /// `ssl_is_enable()`.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn ssl_enabled(socket: Socket) -> bool {
        // SAFETY: per the caller.
        !unsafe { handle(socket) }.tls_owner().is_null()
    }

    /// The HTTP proxy's `ProxyState`, or null.
    /// # Safety
    /// `socket` is live.
    #[must_use]
    pub unsafe fn http_proxy(socket: Socket) -> *mut c_void {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.http_proxy().cast()
    }

    /// `ssl_get_peer_cert()`: empty (with the socket's error set) without one.
    /// # Safety
    /// `socket` is live and has TLS.
    #[must_use]
    pub unsafe fn peer_cert(socket: Socket) -> Vec<u8> {
        // SAFETY: per the caller.
        unsafe { handle(socket) }.peer_cert()
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
        let socket = unsafe { handle(socket) };
        // SAFETY: per the caller.
        unsafe { swoole_rs_hooks::socket_extra::set_close_hook(socket, hook, arg) };
    }
}
