//! The coroutine socket's operations the methods need, over
//! `swoole_rs_phpruntime::co` and the socket's record, plus Swoole's PHP error
//! reporting; and `Swoole\Client`'s `network::Client`, whose operations
//! crates/clients runs.
use crate::settings::php_swoole_length_func;
use libc::{c_int, c_void, size_t, ssize_t};
use std::{
    ffi::{CStr, CString},
    mem::ManuallyDrop,
    ptr,
    rc::Rc,
};
use swoole_rs_clients::{
    address::{swr_v1_clients_address_port, with_host},
    client::{self, Client, Op},
};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_hooks::co_socket::Co as Handle;
use swoole_rs_phpruntime::{Report, co, report};
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_protocols_ffi::set_protocol as mqtt_set_protocol;
use swoole_rs_reactor::Socket as Net;
use swoole_rs_zend::{
    Owned, Value, ZString,
    bailout::{self, Bailout},
    host::callable::Callable,
    string::{self, ZBuf},
    sys::zval,
};

/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;

/// A `SocketImpl *`.
pub(crate) type Socket = *mut libc::c_void;

/// `SW_TIMEOUT_*`.
const TIMEOUT_CONNECT: c_int = 1 << 1;
const TIMEOUT_READ: c_int = 1 << 2;
const TIMEOUT_WRITE: c_int = 1 << 3;
pub(crate) const TIMEOUT_RDWR: c_int = TIMEOUT_READ | TIMEOUT_WRITE;
/// `SW_SOCK_UDP6`.
const SOCK_UDP6: c_int = 4;
/// `SW_ERROR_WRONG_OPERATION`.
const WRONG_OPERATION: c_int = 509;
/// `SW_IPC_BUFFER_SIZE`: `SW_IPC_MAX_SIZE - sizeof(DataHead)`.
#[expect(clippy::cast_possible_truncation, reason = "a u16 field's value")]
const IPC_BUFFER_SIZE: u16 = (swoole_rs_corebase::constants::SW_IPC_MAX_SIZE - 40) as u16;

/// `php_swoole_error(level, "%s", message)`, or with `fatal`
/// `php_swoole_fatal_error`; a bailout of the error handler is deferred. Nothing is
/// reported, and the last error stays, while one is pending.
pub(crate) fn swoole_rs_php_error(level: c_int, fatal: bool, message: &CStr) {
    let kind = if fatal { Report::Fatal } else { Report::Swoole };
    bailout::defer(bailout::outcome(()).and_then(|()| report(kind, level, message)));
}

/// `swoole_fatal_error`'s arguments for a method called before the constructor.
pub(crate) const UNCONSTRUCTED: (c_int, &CStr) = (WRONG_OPERATION, c"must call constructor first");

/// `new SocketImpl(...)`: by family (`domain`, `kind`, `protocol`) or by
/// `swSocketType` (`kind`), over `fd` unless it is negative.
/// # Errors
/// The constructor bailed out.
pub(crate) fn make(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    family: bool,
) -> Result<NewSocket, Bailout> {
    // The new socket, which nothing else holds.
    Ok(NewSocket(Co::from(co::make(
        fd, domain, kind, protocol, family,
    )?)))
}

/// A coroutine socket that nothing else holds: its holder frees it or gives it
/// to an object. Dropped, it leaks.
#[must_use]
pub struct NewSocket(Co);

impl NewSocket {
    pub(crate) fn co(&self) -> Co {
        self.0
    }

    /// Give up ownership, to the object that now holds the socket.
    pub(crate) fn into_co(self) -> Co {
        self.0
    }

    /// `delete socket`.
    /// # Errors
    /// A close hook bailed out.
    pub(crate) fn free(self) -> Result<(), Bailout> {
        // SAFETY: an owned socket, which nothing else holds; gone after the call.
        unsafe { co::free(self.0.as_ptr()) }
    }
}

/// `php_swoole_check_reactor()` inside `zend_try`.
/// # Errors
/// Starting the reactor bailed out.
pub(crate) fn check_reactor() -> Result<(), Bailout> {
    // SAFETY: starting the reactor holds nothing a bailout would skip dropping.
    unsafe { bailout::trap_with(swoole_rs_phpruntime::check_reactor) }
}

/// Whether the build has an optional SSL option: the host name (0) or grease
/// (1, `BoringSSL`'s).
pub(crate) fn ssl_supports(option: c_int) -> bool {
    match option {
        0 => built_with(Switch::TlsHostName),
        1 => built_with(Switch::BoringSsl),
        _ => false,
    }
}

/// A live coroutine socket (`SocketImpl *`) over crates/hooks' record handle.
/// Its constructor is the only unsafe point: a socket stays live while the
/// object or client that owns it is, and a method holds that owner for its
/// whole call.
#[derive(Clone, Copy)]
pub struct Co(Handle);

/// crates/hooks' handle is the same record under the same promise.
impl From<Handle> for Co {
    fn from(handle: Handle) -> Self {
        Self(handle)
    }
}

impl Co {
    /// # Safety
    /// `socket` is a live coroutine socket that outlives every use of the handle.
    pub const unsafe fn new(socket: Socket) -> Self {
        // SAFETY: per the caller; the native socket is its record.
        Self(unsafe { Handle::of(socket) })
    }

    #[must_use]
    pub fn as_ptr(self) -> Socket {
        self.0.this()
    }

    /// crates/hooks' handle to the same record.
    #[must_use]
    pub fn handle(self) -> Handle {
        self.0
    }

    /// The `network::Socket` (`get_socket()`), while the socket is open.
    pub(crate) fn net<'a>(self) -> Option<Net<'a>> {
        // SAFETY: null or an open socket's live record, read fresh on every use.
        unsafe { Net::from_ptr(self.0.socket()) }
    }

    /// The socket's buffers are zend strings from now on.
    pub fn use_zend_strings(self) {
        // SAFETY: the static zend string allocator.
        unsafe {
            self.0
                .set_buffer_allocator(swoole_rs_phpswoole::sw_zend_string_allocator());
        }
    }

    /// `socket_coro_init`'s socket half: zero copy and the zend string
    /// allocator; its fd, domain, type and protocol.
    pub(crate) fn adopt(self) -> [i64; 4] {
        self.0.set_zero_copy(true);
        self.use_zend_strings();
        let s = self.0;
        [
            s.sock_fd(),
            s.sock_domain(),
            s.sock_type(),
            s.sock_protocol(),
        ]
        .map(i64::from)
    }

    pub(crate) fn fd(self) -> c_int {
        self.0.sock_fd()
    }

    /// `move_fd()`: the socket forgets its fd, which stays open.
    pub(crate) fn move_fd(self) {
        self.0.set_sock_fd(-1);
        if let Some(net) = self.net() {
            net.edit(|record| record.fd = -1);
        }
    }

    /// `close()` inside `zend_try`.
    /// # Errors
    /// A close hook bailed out.
    pub(crate) fn close_trapped(self) -> Result<bool, Bailout> {
        co::close_trapped(self.0)
    }

    /// `close()`; the socket may be gone after it returns true.
    pub(crate) fn close(self) -> bool {
        self.0.close()
    }

    pub(crate) fn is_closed(self) -> bool {
        self.0.is_closed()
    }

    /// `f` over `errCode` and a copy of `errMsg` (empty when null), which the socket
    /// may replace or free.
    pub fn with_error<T>(self, f: impl FnOnce(c_int, &CStr) -> T) -> T {
        let code = self.0.err_code();
        self.0
            .with_err_message(|message| f(code, message.unwrap_or_default()))
    }

    /// `errCode`.
    pub(crate) fn err_code(self) -> c_int {
        self.0.err_code()
    }

    /// `set_err(code)`: the code's message.
    pub(crate) fn set_err(self, code: c_int) {
        self.0.set_err(code);
    }

    /// `set_err(code, message)`.
    pub(crate) fn set_error(self, code: c_int, message: &[u8]) {
        if self.0.set_error(code, message) != 0 {
            // std::bad_alloc, which upstream leaves uncaught.
            std::process::abort();
        }
    }

    /// `socket->errCode = code`, nothing else.
    pub(crate) fn set_err_code(self, code: c_int) {
        self.0.set_err_code(code);
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn bind(self, address: &[u8], port: i64) -> bool {
        self.0.bind(address, port as c_int)
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int backlog")]
    pub(crate) fn listen(self, backlog: i64) -> bool {
        self.0.listen(backlog as c_int)
    }

    /// `sw_callable_free(protocol.private_data_1)`, the `package_length_func` the socket
    /// owns. It leaves the slot first, so the destructor, which may reach the
    /// socket, finds none.
    /// # Errors
    /// The callable's destructor bailed out; the bailout stays pending.
    pub fn free_length_callable(self) -> Result<(), Bailout> {
        let callable = self.0.edit_protocol(|protocol| {
            std::mem::replace(&mut protocol.private_data_1, ptr::null_mut())
        });
        if callable.is_null() {
            return Ok(());
        }
        callable_free(callable)
    }

    /// `accept(timeout)`: the new connection, owned by the caller. It copies
    /// the listener's length callable, so closing it leaves the listener's.
    #[must_use]
    pub fn accept(self, timeout: f64) -> Option<NewSocket> {
        let conn = NewSocket(Self(self.0.accept(timeout)?));
        let callable = self.0.protocol().private_data_1;
        if !callable.is_null() {
            let copy = callable_dup(callable);
            conn.co()
                .0
                .edit_protocol(|protocol| protocol.private_data_1 = copy);
        }
        Some(conn)
    }

    pub(crate) fn port_required(self) -> bool {
        self.0.kind() <= SOCK_UDP6
    }

    /// `connect(host, port)` under a connect timeout of `timeout`.
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn connect(self, host: &[u8], port: i64, timeout: f64) -> bool {
        let _timeout = self.timeout(timeout, TIMEOUT_CONNECT);
        self.0.connect_host(host, port as c_int)
    }

    /// `set_timeout(timeout, SW_TIMEOUT_CONNECT)`, then `connect(host, port)` of
    /// the host up to its first NUL, as upstream reads it.
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn connect_host(self, host: &[u8], port: i64, timeout: f64) -> bool {
        self.set_timeout(timeout, TIMEOUT_CONNECT);
        let host = host.split(|byte| *byte == 0).next().unwrap_or_default();
        self.0.connect_host(host, port as c_int)
    }

    pub(crate) fn check_liveness(self) -> bool {
        self.0.check_liveness()
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an event bit set"
    )]
    pub(crate) fn bound_cid(self, event: i64) -> i64 {
        self.0.bound_cid(event as u32)
    }

    /// A timeout of `kind` for the guard's life (`TimeoutSetter`).
    pub(crate) fn timeout(self, timeout: f64, kind: c_int) -> co::Timeout {
        co::Timeout::set(self.0, timeout, kind)
    }

    /// One send [`Transfer`] of `data` under `timeout`.
    pub(crate) fn send(self, op: Transfer, data: &[u8], timeout: f64) -> ssize_t {
        let _timeout = self.timeout(timeout, TIMEOUT_WRITE);
        if matches!(op, Transfer::SendAll) {
            self.0.send_all(data)
        } else {
            self.0.send(data)
        }
    }

    /// `recv_packet(timeout)`; with `clear_empty`, an empty packet clears the
    /// read buffer.
    pub(crate) fn recv_packet(self, timeout: f64, clear_empty: bool) -> ssize_t {
        let retval = swoole_rs_hooks::packet::recv_packet(self.0, timeout);
        if retval == 0
            && clear_empty
            && let Some(buffer) = self.read_buffer()
        {
            buffer.clear();
        }
        retval
    }

    fn read_buffer<'a>(self) -> Option<&'a mut swoole_rs_buffer::string::CppString> {
        let buffer = self.0.read_buffer();
        // SAFETY: the socket's live read buffer, or null before the first read.
        unsafe {
            buffer
                .cast::<swoole_rs_buffer::string::CppString>()
                .as_mut()
        }
    }

    /// `pop_packet()`: the `length`-byte packet [`Co::recv_packet`] returned,
    /// or None after `set_err(ENOMEM)`.
    pub(crate) fn pop_packet(self, length: usize) -> Option<ZString> {
        let init_size = self.0.buffer_init_size();
        let packet = match self.read_buffer() {
            Some(buffer) if buffer.offset() != 0 => buffer.pop(init_size),
            _ => ptr::null_mut(),
        };
        if packet.is_null() {
            self.set_err(libc::ENOMEM);
            return None;
        }
        // SAFETY: `zend::assign_zend_string_by_val`: the packet is the value of
        // an unshared zend string holding its `length` bytes, now ours.
        Some(unsafe { ZBuf::from_raw(string::from_val(packet), length) }.into_zstring())
    }

    /// `readv`/`writev` over `vectors` (with `all`, `readv_all`/`writev_all`)
    /// under `timeout`; with the vector index and byte offset where it stopped.
    pub(crate) fn vector(
        self,
        vectors: co::Vectors<'_, '_>,
        all: bool,
        timeout: f64,
    ) -> (ssize_t, c_int, size_t) {
        let kind = if matches!(vectors, co::Vectors::Read(_)) {
            TIMEOUT_READ
        } else {
            TIMEOUT_WRITE
        };
        let _timeout = self.timeout(timeout, kind);
        co::vector(self.0, vectors, all)
    }

    /// `sendfile(file, offset, length)` of the file named up to the first NUL,
    /// as upstream reads it.
    pub(crate) fn sendfile(self, file: &[u8], offset: i64, length: i64) -> bool {
        let file = file.split(|byte| *byte == 0).next().unwrap_or_default();
        let file = CString::new(file).unwrap_or_default();
        // size_t's conversion of the length, as upstream's call.
        let length = usize::try_from(length.cast_unsigned()).unwrap_or(usize::MAX);
        self.0.sendfile(&file, offset, length)
    }

    /// `recvfrom` into `buffer`'s room under a read timeout of `timeout`.
    pub(crate) fn recvfrom(self, buffer: &mut ZBuf, timeout: f64) -> ssize_t {
        let _timeout = self.timeout(timeout, TIMEOUT_READ);
        let received = self.0.recvfrom_peer(room(buffer));
        filled(buffer, received)
    }

    /// One receiving [`Transfer`] into `buffer`'s room under `timeout`.
    pub(crate) fn receive(self, op: Transfer, buffer: &mut ZBuf, timeout: f64) -> ssize_t {
        let _timeout = (!matches!(op, Transfer::Peek)).then(|| self.timeout(timeout, TIMEOUT_READ));
        let out = room(buffer);
        let received = match op {
            Transfer::Peek => self.0.peek(out),
            Transfer::RecvAll => self.0.recv_all(out),
            Transfer::RecvLine => self.0.recv_line(out),
            Transfer::RecvWithBuffer => self.0.recv_with_buffer(out),
            _ => self.0.recv(out),
        };
        filled(buffer, received)
    }

    /// `get_addr()` and `get_port()`.
    pub(crate) fn address(self) -> (CString, c_int) {
        self.net()
            .map_or_else(|| (CString::default(), 0), net_address)
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn sendto(self, address: &[u8], port: i64, data: &[u8]) -> ssize_t {
        self.0.sendto(address, port as c_int, data)
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int how")]
    pub(crate) fn shutdown(self, how: i64) -> bool {
        self.0.shutdown(how as c_int)
    }

    pub(crate) fn getsockname(self) -> bool {
        self.0.getsockname()
    }

    /// `getpeername(&sa)`: the peer's address text and port.
    pub(crate) fn getpeername(self) -> Option<(CString, c_int)> {
        let mut sa = SocketAddress::default();
        if !self.0.getpeername(&mut sa) {
            return None;
        }
        Some(address(&sa))
    }

    #[expect(clippy::cast_sign_loss, reason = "an event bit set")]
    pub(crate) fn cancel(self, event: c_int) -> bool {
        self.0.cancel(event as u32)
    }

    pub(crate) fn ssl_handshake(self) -> bool {
        self.0.ssl_handshake()
    }

    /// `open_length_check` (0), `open_eof_check` (1) or `http2` (2) = `value`.
    pub(crate) fn set_flag(self, flag: c_int, value: bool) {
        match flag {
            0 => self.0.set_open_length_check(value),
            1 => self.0.set_open_eof_check(value),
            _ => self.0.set_http2(value),
        }
    }

    /// `mqtt::set_protocol` (0) or `Protocol::default_length_func` (1).
    pub(crate) fn protocol_preset(self, kind: c_int) {
        self.0.edit_protocol(|protocol| {
            if kind == 0 {
                mqtt_set_protocol(protocol);
            } else {
                protocol.get_package_length =
                    Some(swoole_rs_ports::framing_recv::default_length_func);
            }
        });
    }

    /// `set_option(level, name, value)`, warning (a log line) on failure.
    pub(crate) fn set_option(self, level: c_int, name: c_int, value: c_int) -> bool {
        self.0.set_int_option(level, name, value)
    }

    pub(crate) fn set_timeout(self, timeout: f64, kind: c_int) {
        self.0.set_timeout(timeout, kind);
    }

    pub(crate) fn get_timeout(self, kind: c_int) -> f64 {
        self.0.timeout(kind)
    }

    /// `set_http_proxy` or `set_socks5_proxy`.
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn proxy(self, http: bool, host: &[u8], port: i64, user: &[u8], password: &[u8]) {
        self.0.set_proxy(http, host, port as c_int, user, password);
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    pub(crate) fn enable_ssl(self) -> bool {
        self.0.enable_ssl()
    }

    /// The socket's `SSLContext` owner; null while TLS is off.
    pub(crate) fn ssl_context(self) -> *const c_void {
        self.0.tls_owner().cast()
    }

    /// The socket's domain and type.
    pub(crate) fn family(self) -> (c_int, c_int) {
        (self.0.sock_domain(), self.0.sock_type())
    }

    /// `swSocketType`.
    pub(crate) fn kind(self) -> c_int {
        self.0.kind()
    }

    /// `open_length_check || open_eof_check`.
    pub(crate) fn packet_mode(self) -> bool {
        self.0.open_length_check() || self.0.open_eof_check()
    }

    pub(crate) fn is_connected(self) -> bool {
        self.0.is_connected()
    }

    /// `get_ssl() != nullptr`.
    pub(crate) fn has_ssl(self) -> bool {
        self.net()
            .is_some_and(|net| net.read(|record| !record.ssl().is_null()))
    }

    /// The peer certificate; the socket has TLS.
    pub(crate) fn peer_cert(self) -> Option<ZString> {
        self.net().and_then(peer_cert)
    }

    pub(crate) fn verify_peer(self, allow_self_signed: bool) -> bool {
        self.0.ssl_verify(allow_self_signed)
    }
}

/// `package_length_func`: the callable `value` makes replaces any earlier
/// one, which is freed once the socket no longer holds it. Nothing more is set
/// once a bailout is pending.
pub(crate) fn length_callable(socket: Co, value: &Value) {
    let Ok(callable) = self::callable(value) else {
        return;
    };
    if callable.is_null() {
        return;
    }
    let old = socket.0.edit_protocol(|protocol| {
        protocol.get_package_length = Some(php_swoole_length_func);
        std::mem::replace(&mut protocol.private_data_1, callable)
    });
    // The destructor may reach the socket, so it runs between the edits.
    if !old.is_null() && callable_free(old).is_err() {
        return;
    }
    socket.0.edit_protocol(|protocol| {
        protocol.package_length_size = 0;
        protocol.package_length_type = 0;
        protocol.package_length_offset = IPC_BUFFER_SIZE;
    });
}

/// A live `network::Client` (crates/clients' state), from [`create`] until
/// [`destroy`].
#[derive(Clone, Copy)]
pub(crate) struct Cli(pub(crate) *mut Client);

impl Cli {
    pub(crate) fn get<'a>(self) -> &'a Client {
        // SAFETY: a live client, as the type promises.
        unsafe { &*self.0 }
    }

    /// `cli->socket`, while the client has one.
    pub(crate) fn net<'a>(self) -> Option<Net<'a>> {
        self.get().socket()
    }

    /// crates/clients' handle on the client.
    pub(crate) fn handle(self) -> client::Handle {
        // SAFETY: a live client, as the type promises.
        unsafe { client::Handle::new(self.0) }
    }

    /// Run `op` inside `zend_try`.
    /// # Errors
    /// PHP code it ran bailed out.
    pub(crate) fn run(self, op: Op<'_>) -> Result<i64, Bailout> {
        let (handle, mut result) = (self.handle(), 0);
        // SAFETY: crates/clients' operations own nothing a bailout would skip
        // dropping, and the closure holds only `op`'s borrows and scalars.
        unsafe { bailout::trap_with(|| result = handle.run(op)) }?;
        Ok(result)
    }

    /// Run an `op` that cannot reach PHP code (a sync client's send or plain
    /// receive), without the `zend_try` frame.
    pub(crate) fn run_plain(self, op: Op<'_>) -> i64 {
        self.handle().run(op)
    }
}

/// `bytes` as C's `%s` reads them: up to the first NUL.
pub(crate) fn c_text(bytes: &[u8]) -> CString {
    let text = bytes.split(|&b| b == 0).next().unwrap_or_default();
    CString::new(text).unwrap_or_default()
}

/// `new Client(type, async)`: the client, or the errno of its failed socket.
pub(crate) fn create(kind: c_int, is_async: bool) -> Result<Cli, c_int> {
    let cli = swoole_rs_clients::client::swr_v1_clients_new(kind, is_async);
    if cli.is_null() {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    } else {
        Ok(Cli(cli))
    }
}

/// `delete cli`, after the length callable of `package_length_func`.
/// # Errors
/// Freeing the callable or closing the client ran PHP code that bailed out.
pub(crate) fn destroy(cli: Cli) -> Result<(), Bailout> {
    let callable = cli.get().edit_record(|record| {
        std::mem::replace(&mut record.protocol.private_data_1, ptr::null_mut())
    });
    if !callable.is_null() {
        callable_free(callable)?;
    }
    let handle = cli.handle();
    let free = || {
        // SAFETY: `destroy` is the client's last operation.
        unsafe { handle.free() };
    };
    // SAFETY: as in `Cli::run`.
    unsafe { bailout::trap_with(free) }
}

/// `sw_callable_create(value)`, for a protocol's `private_data_1`: the slot's
/// reference (`Rc::into_raw`), or null after a warning.
/// # Errors
/// The warning's handler bailed out; the bailout stays pending.
pub(crate) fn callable(value: &Value) -> Result<*mut c_void, Bailout> {
    let callable = Callable::new(value)?;
    Ok(callable.map_or(ptr::null_mut(), |callable| {
        Rc::into_raw(callable).cast_mut().cast()
    }))
}

/// The slot's callable `callable` made, still the slot's: the caller's own
/// reference.
fn held(callable: *mut c_void) -> Rc<Callable> {
    // SAFETY: a slot's live reference (`callable`), borrowed: the slot keeps it.
    let slot = ManuallyDrop::new(unsafe { Rc::from_raw(callable.cast::<Callable>()) });
    Rc::clone(&slot)
}

/// `callable->dup()`: another slot's reference to `callable`.
fn callable_dup(callable: *mut c_void) -> *mut c_void {
    Rc::into_raw(held(callable)).cast_mut().cast()
}

/// `sw_callable_free(callable)`, out of its slot: it is gone either way.
/// # Errors
/// A destructor bailed out; the bailout stays pending.
pub(crate) fn callable_free(callable: *mut c_void) -> Result<(), Bailout> {
    // SAFETY: the reference `callable` gave a slot, which holds it no more.
    drop(unsafe { Rc::from_raw(callable.cast::<Callable>()) });
    bailout::outcome(())
}

/// `sw_callable_create(value)`: none after a warning. The last owner's drop
/// frees it, deferring a destructor's bailout.
/// # Errors
/// The warning's handler bailed out; the bailout stays pending.
pub(crate) fn owned_callable(value: &Value) -> Result<Option<Rc<Callable>>, Bailout> {
    Callable::new(value)
}

/// `sw_zend_call_function_ex2(nullptr, callable->ptr(), argv, retval)`: whether
/// the call succeeded.
/// # Errors
/// The callable bailed out.
pub(crate) fn invoke(
    callable: *mut c_void,
    argv: &mut [zval],
    retval: &mut zval,
) -> Result<bool, Bailout> {
    // Its own reference: the callable may close the socket, freeing the slot's.
    let callable = held(callable);
    let called = callable.call(argv, Some(Value::of(retval)), true);
    // The last reference, then: its destructor's bailout is the caller's to raise.
    drop(callable);
    called.and_then(bailout::outcome)
}

/// A `Socket` over a dup of the client's fd: `php_swoole_convert_to_socket`, which
/// makes one whatever the import's result. `None` without ext/sockets.
pub(crate) fn export_socket(cli: Cli) -> Option<Owned> {
    if !built_with(Switch::Sockets) {
        return None;
    }
    let fd = swoole_rs_corebase::sys::dup(cli.net()?.fd());
    if fd < 0 {
        return None;
    }
    Some(swoole_rs_phpswoole::socket_create(fd).0)
}

/// The peer certificate of `socket`, which has TLS:
/// `ssl_get_peer_certificate` into a buffer of `sw_tg_buffer()`'s size.
pub(crate) fn peer_cert(socket: Net<'_>) -> Option<ZString> {
    let mut buffer = vec![0_u8; swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize];
    let length = swoole_rs_tls::upstream::socket_ssl_peer_certificate_pem(socket, &mut buffer);
    let length = usize::try_from(length).ok()?;
    Some(ZString::new(buffer.get(..length).unwrap_or(&buffer)))
}

/// The unwritten room of `buffer`.
pub(crate) fn room(buffer: &mut ZBuf) -> &mut [u8] {
    let written = buffer.len();
    let room = buffer.capacity() - written;
    let at = buffer.as_mut_ptr().wrapping_add(written);
    // SAFETY: `capacity()` bytes are writable from the start, so `room` from `at`.
    unsafe { std::slice::from_raw_parts_mut(at, room) }
}

/// Count the `received` bytes a transfer wrote into `buffer`'s [`room`].
pub(crate) fn filled<T: Copy + TryInto<usize>>(buffer: &mut ZBuf, received: T) -> T {
    if let Ok(count) = received.try_into() {
        let len = buffer.capacity().min(buffer.len() + count);
        // SAFETY: the transfer wrote `count` bytes of the room it was given.
        unsafe { buffer.set_len(len) };
    }
    received
}

/// [`address`] of `socket`'s `info`, copied out of the record.
pub(crate) fn net_address(socket: Net<'_>) -> (CString, c_int) {
    socket.read(|record| address(&record.info))
}

/// The address and port of `address`: `get_addr()` and `get_port()`.
pub(crate) fn address(address: &SocketAddress) -> (CString, c_int) {
    let host = with_host(address, CStr::to_owned);
    (host, swr_v1_clients_address_port(address))
}

/// `swoole_error_log(SW_LOG_WARNING, SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, ...)`
/// from upstream's `recv`, which the log line names.
pub(crate) fn package_too_big(length: c_int) {
    swoole_rs_corebase::log::error_log(
        LOG_WARNING,
        swoole_rs_error::ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE as c_int,
        c"zim_swoole_client_recv",
        format_args!("Package is too big. package_length={length}"),
    );
}

/// Which reporter [`crate::net::report`] uses.
#[derive(Clone, Copy)]
pub(crate) enum ErrorKind {
    /// `php_swoole_error`.
    Error,
    /// `php_swoole_fatal_error`.
    Fatal,
    /// `php_swoole_core_error`.
    Core,
}

/// What `swoole_socket_coro_transfer` does.
#[derive(Clone, Copy)]
#[repr(i32)]
pub(crate) enum Transfer {
    Peek = 0,
    Recv = 1,
    RecvAll = 2,
    RecvLine = 3,
    RecvWithBuffer = 4,
    Send = 5,
    SendAll = 6,
}
