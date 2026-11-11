//! The coroutine socket (`SocketImpl`), which crates/hooks makes, runs and
//! frees, and what the streams need of Swoole's PHP side.
use libc::{c_char, c_int, c_void, sockaddr, socklen_t};
use std::{ffi::CStr, ptr::NonNull};
use swoole_rs_corebase::log::set_last_error;
use swoole_rs_hooks::co_socket::{Co, swr_v1_hook_co_socket_free};
use swoole_rs_reactor::SocketFields;
use swoole_rs_tls::upstream::{socket_recv_sync, socket_send_sync, socket_ssl_close};
use swoole_rs_zend::{
    E_ERROR, Owned, Value,
    bailout::{self, Bailout},
    sys::{swr_zval_stringl, zval, zval_ptr_dtor},
};

// crates/phpevents' functions: declared, since crates/phpevents depends on this crate.
unsafe extern "C" {
    /// crates/phpevents.
    fn php_swoole_convert_to_fd(value: *mut zval) -> c_int;
}

/// `new SocketImpl(...)` inside `zend_try`, since the constructor's warnings can
/// run a PHP error handler: the socket is in `made`, which owns it before a
/// handler can bail out. By `domain`, `kind` and `protocol` with `family`, else
/// by the `swSocketType` `kind`; over `fd` unless it is negative.
fn make_trapped(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    family: bool,
    made: &mut *mut c_void,
) -> Result<(), Bailout> {
    let make = || {
        let (socket, result) =
            swoole_rs_hooks::co_socket::make_quiet(fd, domain, kind, protocol, family);
        *made = socket.this();
        swoole_rs_hooks::co_socket::warn_created(socket, result);
    };
    // SAFETY: the body holds copies, a `Copy` handle and the caller's slot.
    unsafe { bailout::trap_with(make) }
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
        let mut made = std::ptr::null_mut();
        let trapped = make_trapped(fd, domain, kind, protocol, fd >= 0, &mut made);
        // Owns, so a bailout frees, the socket `make` wrote before it.
        let socket = Self::wrap(made, true);
        if trapped.is_err() {
            Bailout.defer();
            return None;
        }
        socket
    }

    /// The coroutine socket of hooked `fd`, if any.
    pub(crate) fn of_fd(fd: i32) -> Option<Self> {
        Self::wrap(swoole_rs_hooks::socket_map::get(fd)?.this(), false)
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
    fn with_record<T>(&self, absent: T, f: impl FnOnce(&mut SocketFields) -> T) -> T {
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
        co::close_trapped(self.co())
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
        self.with_record(std::ptr::null_mut(), |record| record.ssl().cast())
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
        let chain = self.co().with_network(socket_ssl_get_peer_cert_chain);
        let chain = chain
            .unwrap_or(std::ptr::null_mut())
            .cast::<OPENSSL_STACK>();
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
    }

    /// `php_swoole_socket_set_ssl(socket, settings)`.
    /// # Errors
    /// A warning's error handler bailed out.
    pub(crate) fn set_ssl(&self, settings: &Value) -> Result<bool, Bailout> {
        let (socket, settings) = (self.raw(), settings.as_ptr());
        let mut applied = false;
        let apply = || {
            if let Some(families) = swoole_rs_phpswoole::families() {
                // SAFETY: a live socket and settings value, which crates/phpclients
                // ignores unless it is an array; its settings apply the SSL ones (2).
                applied = unsafe { (families.socket_settings)(socket, settings, 2) };
            }
        };
        // SAFETY: the body holds copies and the result slot; the settings' warnings
        // can run a PHP error handler.
        unsafe { bailout::trap_with(apply) }?;
        Ok(applied)
    }
}

/// `openssl_x509_read(pem)`.
/// # Errors
/// The call bailed out.
pub(crate) fn x509_read(pem: &[u8]) -> Result<Owned, Bailout> {
    let certificate = Owned::default();
    let retval = certificate.as_ptr();
    let read = || {
        // SAFETY: zero is an undefined zval, which the next line fills.
        let mut string: zval = unsafe { std::mem::zeroed() };
        // SAFETY: the caller's bytes, copied into a new string.
        unsafe { swr_zval_stringl(&raw mut string, pem.as_ptr().cast(), pem.len()) };
        let name = c"openssl_x509_read";
        // SAFETY: one live argument and the caller's writable result.
        unsafe {
            swoole_rs_zend::host::named_call_into(
                name.as_ptr(),
                name.count_bytes(),
                1,
                &raw mut string,
                retval,
            );
        }
        bailout::finish();
        // SAFETY: the string this body made.
        unsafe { zval_ptr_dtor(&raw mut string) };
    };
    // SAFETY: the body holds the borrowed bytes, a plain zval and the slot of
    // `certificate`, which lives outside it.
    unsafe { bailout::trap_with(read) }?;
    Ok(certificate)
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
    // SAFETY: called while PHP runs; the body holds nothing.
    unsafe { bailout::trap_with(swoole_rs_phpswoole::coroutine_check) }
}

/// `php_swoole_convert_to_fd(value)`: an fd, or negative after a warning.
/// # Errors
/// The warning's error handler bailed out.
pub fn convert_to_fd(value: &Value) -> Result<i32, Bailout> {
    let mut fd = -1;
    // SAFETY: a live zval, borrowed for the call.
    let convert = || fd = unsafe { php_swoole_convert_to_fd(value.as_ptr()) };
    // SAFETY: the body holds a copy and the result slot.
    unsafe { bailout::trap_with(convert) }?;
    Ok(fd)
}

/// `System::socket_poll` of `(fd, events)` rows merged by fd, in the order
/// `stream_select` added them: `None` on a timeout or failure, else each polled
/// fd and its revents in the poll set's order.
pub(crate) fn poll(rows: &[(i32, i32)], timeout: f64) -> Option<Vec<(i32, i32)>> {
    swoole_rs_hooks::poll::socket_poll_rows(rows, timeout)
}

/// What the client classes (crates/phpclients, crates/phphttp and crates/phphttp2)
/// need of a `SocketImpl *` beyond [`Co`]'s own members.
pub mod co {
    pub use super::Io;
    use libc::{c_char, c_int, c_void, iovec};
    pub use swoole_rs_hooks::co_socket::{Co, CoSocket};
    pub use swoole_rs_hooks::packet::recv_packet;
    use swoole_rs_hooks::vector::IoVector;
    use swoole_rs_hooks::{TimeoutScope, co_socket::swr_v1_hook_co_socket_free};
    use swoole_rs_zend::bailout::{self, Bailout};

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
        let mut made = std::ptr::null_mut();
        if super::make_trapped(fd, domain, kind, protocol, family, &mut made).is_err() {
            // Frees the socket `make` wrote before the bailout, if any.
            drop(super::Socket::wrap(made, true));
            return Err(Bailout);
        }
        Ok(made)
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
    pub fn close_trapped(socket: Co) -> Result<bool, Bailout> {
        let mut closed = false;
        // SAFETY: the body holds a `Copy` handle and the result slot; the close may
        // resume coroutines that run PHP.
        unsafe { bailout::trap_with(|| closed = socket.close()) }?;
        Ok(closed)
    }

    /// A timeout of `kind` set for the guard's life (`TimeoutSetter`).
    pub struct Timeout(TimeoutScope);

    impl Timeout {
        #[must_use]
        pub fn set(socket: Co, timeout: f64, kind: c_int) -> Self {
            Self(TimeoutScope::new(socket, timeout, kind))
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
    /// `buf` holds `length` bytes, writable to receive.
    pub unsafe fn io(socket: Co, op: Io, buf: *mut c_char, length: usize) -> isize {
        // SAFETY: per the caller.
        unsafe { super::io(socket, op, buf, length) }
    }

    /// `readv`, `readv_all`, `writev` or `writev_all` (`op` 0 to 3) over
    /// `count` vectors, with the vector index and byte offset where it stopped.
    /// # Safety
    /// The vectors are valid for the operation.
    #[must_use]
    pub unsafe fn vector(
        socket: Co,
        op: c_int,
        vectors: *const iovec,
        count: c_int,
    ) -> (isize, c_int, usize) {
        // SAFETY: per the caller, `count` vectors valid for the operation.
        let Some(mut vector) = (unsafe { IoVector::new(vectors, count) }) else {
            // `IOVector`'s constructor throws here, which ended at this Rust frame.
            std::process::abort();
        };
        let result = socket.vector(&mut vector, op >= 2, op & 1 == 1);
        let (index, offset) = vector.stopped();
        (result, index, offset)
    }

    /// `set_err(code, message)`, copying the message.
    pub fn set_error(socket: Co, code: c_int, message: &[u8]) {
        if socket.set_error(code, message) != 0 {
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

    /// `set_dtor`: `hook(arg, socket)` runs once as the socket is destroyed.
    /// # Safety
    /// `hook` may be called with `arg` until then.
    pub unsafe fn close_hook(
        socket: Co,
        hook: unsafe extern "C" fn(*mut c_void, *mut c_void),
        arg: *mut c_void,
    ) {
        swoole_rs_hooks::socket_extra::set_close_hook(
            socket,
            // SAFETY: per the caller, `hook` takes `arg` and the socket until it goes,
            // which is when this runs.
            Box::new(move |owner| unsafe { hook(arg, owner) }),
        );
    }
}
