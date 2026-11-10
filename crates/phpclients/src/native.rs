//! The coroutine socket's operations the methods need, over
//! `swoole_rs_phpruntime::co` and the socket's record, plus Swoole's PHP error
//! reporting; and `Swoole\Client`'s `network::Client`, whose operations
//! crates/clients runs.
use crate::settings::{default_length_func, mqtt_set_protocol, php_swoole_length_func};
use libc::{c_char, c_int, c_void, iovec, size_t, ssize_t};
use std::{
    ffi::{CStr, CString},
    ptr,
};
use swoole_rs_clients::{
    address::{swr_v1_clients_address_host, swr_v1_clients_address_port},
    client::{Call, Client, ClientRecord, Op, swr_v1_clients_run},
};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_phpruntime::{Report, co, report};
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_reactor::SocketRecord;
use swoole_rs_zend::{
    Owned, ZString,
    bailout::{self, Bailout},
    string::{self, ZBuf},
    sys::zval,
};

/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;

/// A `SocketImpl *`.
pub(crate) type Socket = *mut libc::c_void;

/// `package_length_func`'s arguments.
#[repr(C)]
pub(crate) struct LengthCallable {
    pub(crate) socket: Co,
    pub(crate) value: *mut zval,
}

unsafe extern "C" {
    /// crates/server's `tcp_info` fields, in upstream's order.
    #[cfg(target_os = "linux")]
    fn swr_v1_server_tcp_info(info: *const c_void, out: *mut Fact, cap: usize) -> usize;
}

/// `swr_v1_server_fact`.
#[cfg(target_os = "linux")]
#[repr(C)]
struct Fact {
    name: *const c_char,
    value: u64,
}

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
/// `php_swoole_fatal_error`; a bailout of the error handler goes on.
/// # Safety
/// PHP is running and `message` is NUL-terminated; the caller owns nothing a
/// bailout would skip.
pub(crate) unsafe extern "C" fn swoole_rs_php_error(
    level: c_int,
    fatal: bool,
    message: *const c_char,
) {
    let kind = if fatal { Report::Fatal } else { Report::Swoole };
    // SAFETY: per the caller.
    bailout::raise(report(kind, level, unsafe { CStr::from_ptr(message) }));
}

/// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
/// # Safety
/// It may longjmp: the frames it skips own nothing.
pub(crate) unsafe fn unconstructed() {
    // SAFETY: per the caller.
    unsafe { swoole_rs_corebase::fatal_error(WRONG_OPERATION, c"must call constructor first") };
}

/// `new SocketImpl(...)`: by family (`domain`, `kind`, `protocol`) or by
/// `swSocketType` (`kind`), over `fd` unless it is negative. The new socket is
/// the caller's to own.
/// # Errors
/// The constructor bailed out.
pub(crate) fn make(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    family: bool,
) -> Result<Socket, Bailout> {
    co::make(fd, domain, kind, protocol, family)
}

/// `delete socket`.
/// # Errors
/// A close hook bailed out.
/// # Safety
/// `socket` is live and owned; it is gone after the call.
pub(crate) unsafe fn free_socket(socket: Socket) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    unsafe { co::free(socket) }
}

unsafe extern "C" fn free_callable(callable: *mut c_void) {
    // SAFETY: a `zend_try` body over a callable `callable_create` made.
    unsafe { swoole_rs_phpruntime::callable_free(callable) };
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

/// A live coroutine socket (`SocketImpl *`). Its constructor is the only
/// unsafe point: a socket stays live while the object or client that owns it
/// is, and a method holds that owner for its whole call.
#[derive(Clone, Copy)]
pub(crate) struct Co(Socket);

impl Co {
    /// # Safety
    /// `socket` is a live coroutine socket that outlives every use of the handle.
    pub(crate) const unsafe fn new(socket: Socket) -> Self {
        Self(socket)
    }

    pub(crate) fn as_ptr(self) -> Socket {
        self.0
    }

    /// The socket's record, read fresh on every use: it changes when the socket
    /// is reinitialised.
    pub(crate) fn record<'a>(self) -> &'a mut co::CoSocket {
        // SAFETY: a live socket, as the handle promises; callers drop the
        // borrow before any effect that may touch the record.
        unsafe { co::record(self.0) }
    }

    /// The `network::Socket` record (`get_socket()`), or null.
    pub(crate) fn socket_record(self) -> *mut SocketRecord {
        self.record().socket
    }

    /// The `network::Socket` record, while the socket is open.
    pub(crate) fn net<'a>(self) -> Option<&'a mut SocketRecord> {
        // SAFETY: an open socket's live record; nothing else holds it during the borrow.
        unsafe { self.socket_record().as_mut() }
    }

    /// `socket_coro_init`'s socket half: zero copy and the zend string
    /// allocator; its fd, domain, type and protocol.
    pub(crate) fn adopt(self) -> [i64; 4] {
        let r = self.record();
        r.zero_copy = true;
        r.buffer_allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
        [r.sock_fd, r.sock_domain, r.sock_type, r.sock_protocol].map(i64::from)
    }

    pub(crate) fn fd(self) -> c_int {
        self.record().sock_fd
    }

    /// `move_fd()`: the socket forgets its fd, which stays open.
    pub(crate) fn move_fd(self) {
        let r = self.record();
        r.sock_fd = -1;
        // SAFETY: its network::Socket record.
        unsafe { r.record() }.fd = -1;
    }

    /// `close()` inside `zend_try`.
    /// # Errors
    /// A close hook bailed out.
    pub(crate) fn close_trapped(self) -> Result<bool, Bailout> {
        // SAFETY: a live socket.
        unsafe { co::close_trapped(self.0) }
    }

    /// `close()`; the socket may be gone after it returns true.
    pub(crate) fn close(self) -> bool {
        // SAFETY: a live socket.
        unsafe { co::close(self.0) }
    }

    pub(crate) fn is_closed(self) -> bool {
        self.record().is_closed()
    }

    /// `errCode` and `errMsg`.
    pub(crate) fn error<'a>(self) -> (c_int, &'a CStr) {
        let r = self.record();
        let message = if r.err_msg.is_null() {
            c""
        } else {
            // SAFETY: errMsg is a NUL-terminated string the socket keeps.
            unsafe { CStr::from_ptr(r.err_msg) }
        };
        (r.err_code, message)
    }

    /// `set_err(code)`: the code's message.
    pub(crate) fn set_err(self, code: c_int) {
        // SAFETY: a live socket.
        unsafe { co::set_err(self.0, code) };
    }

    /// `set_err(code, message)`.
    pub(crate) fn set_error(self, code: c_int, message: &[u8]) {
        // SAFETY: a live socket.
        unsafe { co::set_error(self.0, code, message) };
    }

    /// `socket->errCode = code`, nothing else.
    pub(crate) fn set_err_code(self, code: c_int) {
        self.record().err_code = code;
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn bind(self, address: &[u8], port: i64) -> bool {
        // SAFETY: a live socket and the address's bytes.
        unsafe {
            co::bind(
                self.0,
                address.as_ptr().cast(),
                address.len(),
                port as c_int,
            )
        }
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int backlog")]
    pub(crate) fn listen(self, backlog: i64) -> bool {
        // SAFETY: a live socket.
        unsafe { co::listen(self.0, backlog as c_int) }
    }

    /// `accept(timeout)`: the new connection, owned by the caller, or null. It
    /// copies the listener's length callable, so closing it leaves the listener's.
    pub(crate) fn accept(self, timeout: f64) -> Socket {
        // SAFETY: a live socket.
        let conn = unsafe { co::accept(self.0, timeout) };
        let callable = self.record().protocol.private_data_1;
        if !conn.is_null() && !callable.is_null() {
            // SAFETY: the new connection, live and not yet shared.
            let conn = unsafe { Self::new(conn) };
            // SAFETY: the listener's live callable.
            conn.record().protocol.private_data_1 =
                unsafe { swoole_rs_phpruntime::callable_dup(callable) };
        }
        conn
    }

    pub(crate) fn port_required(self) -> bool {
        self.record().kind <= SOCK_UDP6
    }

    /// `connect(host, port)` under a connect timeout of `timeout`.
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn connect(self, host: &[u8], port: i64, timeout: f64) -> bool {
        let _timeout = self.timeout(timeout, TIMEOUT_CONNECT);
        // SAFETY: a live socket and the host's bytes.
        unsafe { co::connect(self.0, host.as_ptr().cast(), host.len(), port as c_int, 0) }
    }

    /// `set_timeout(timeout, SW_TIMEOUT_CONNECT)`, then `connect(host, port,
    /// flags)` of the host up to its first NUL, as upstream reads it.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's int port and flags"
    )]
    pub(crate) fn connect_host(self, host: &[u8], port: i64, timeout: f64, flags: i64) -> bool {
        self.set_timeout(timeout, TIMEOUT_CONNECT);
        let host = host.split(|byte| *byte == 0).next().unwrap_or_default();
        // SAFETY: a live socket and the host's bytes.
        unsafe {
            co::connect(
                self.0,
                host.as_ptr().cast(),
                host.len(),
                port as c_int,
                flags as c_int,
            )
        }
    }

    pub(crate) fn check_liveness(self) -> bool {
        // SAFETY: a live socket.
        unsafe { co::check_liveness(self.0) }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an event bit set"
    )]
    pub(crate) fn bound_cid(self, event: i64) -> i64 {
        // SAFETY: a live socket.
        unsafe { co::bound_cid(self.0, event as u32) }
    }

    /// A timeout of `kind` for the guard's life (`TimeoutSetter`).
    pub(crate) fn timeout(self, timeout: f64, kind: c_int) -> co::Timeout {
        // SAFETY: a live socket, which outlives the guard in every caller.
        unsafe { co::Timeout::set(self.0, timeout, kind) }
    }

    /// One [`Transfer`] of `length` bytes at `buffer` under `timeout`.
    /// # Safety
    /// `buffer` holds `length` bytes, writable unless the transfer sends.
    pub(crate) unsafe fn transfer(
        self,
        op: Transfer,
        buffer: *mut c_char,
        length: size_t,
        timeout: f64,
    ) -> ssize_t {
        use co::Io;
        let io = match op {
            Transfer::Peek => Io::Peek,
            Transfer::Recv => Io::Recv,
            Transfer::RecvAll => Io::RecvAll,
            Transfer::Send => Io::Send,
            Transfer::SendAll => Io::SendAll,
            Transfer::RecvLine | Transfer::RecvWithBuffer => Io::Recv,
        };
        if matches!(op, Transfer::Peek) {
            // SAFETY: per the caller.
            return unsafe { co::io(self.0, io, buffer, length) };
        }
        let kind = if matches!(op, Transfer::Send | Transfer::SendAll) {
            TIMEOUT_WRITE
        } else {
            TIMEOUT_READ
        };
        let _timeout = self.timeout(timeout, kind);
        match op {
            // SAFETY: per the caller.
            Transfer::RecvLine => unsafe { co::recv_line(self.0, buffer, length) },
            // SAFETY: per the caller.
            Transfer::RecvWithBuffer => unsafe { co::recv_with_buffer(self.0, buffer, length) },
            // SAFETY: per the caller.
            _ => unsafe { co::io(self.0, io, buffer, length) },
        }
    }

    /// One send [`Transfer`] of `data` under `timeout`.
    pub(crate) fn send(self, op: Transfer, data: &[u8], timeout: f64) -> ssize_t {
        // SAFETY: a send only reads the bytes.
        unsafe { self.transfer(op, data.as_ptr().cast_mut().cast(), data.len(), timeout) }
    }

    /// `recv_packet(timeout)`; with `clear_empty`, an empty packet clears the
    /// read buffer.
    pub(crate) fn recv_packet(self, timeout: f64, clear_empty: bool) -> ssize_t {
        // SAFETY: a live socket.
        let retval = unsafe { co::recv_packet(self.0, timeout) };
        if retval == 0
            && clear_empty
            && let Some(buffer) = self.read_buffer()
        {
            buffer.length = 0;
            buffer.offset = 0;
        }
        retval
    }

    fn read_buffer<'a>(self) -> Option<&'a mut swoole_rs_buffer::string::CppString> {
        let buffer = self.record().read_buffer;
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
        let init_size = self.record().buffer_init_size;
        let packet = match self.read_buffer() {
            Some(buffer) if buffer.offset != 0 => buffer.pop(init_size),
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

    /// `readv`/`readv_all`/`writev`/`writev_all` (`op` 0..=3) over `vectors`
    /// under `timeout`; with the vector index and byte offset where it stopped.
    /// # Safety
    /// Each vector holds its bytes, writable for a read.
    pub(crate) unsafe fn vector(
        self,
        op: c_int,
        vectors: &[iovec],
        timeout: f64,
    ) -> (ssize_t, c_int, size_t) {
        let kind = if op < 2 { TIMEOUT_READ } else { TIMEOUT_WRITE };
        let _timeout = self.timeout(timeout, kind);
        let count = c_int::try_from(vectors.len()).unwrap_or(c_int::MAX);
        // SAFETY: per the caller.
        unsafe { co::vector(self.0, op, vectors.as_ptr(), count) }
    }

    /// `sendfile(file, offset, length)`.
    /// # Safety
    /// `file` is NUL-terminated.
    pub(crate) unsafe fn sendfile(self, file: *const c_char, offset: i64, length: i64) -> bool {
        // SAFETY: per the caller.
        unsafe { co::sendfile(self.0, file, offset, length) }
    }

    /// `recvfrom` into `buffer`'s room under a read timeout of `timeout`.
    pub(crate) fn recvfrom(self, buffer: &mut ZBuf, timeout: f64) -> ssize_t {
        let _timeout = self.timeout(timeout, TIMEOUT_READ);
        let (at, room) = room(buffer);
        // SAFETY: `room` writable bytes at `at`.
        filled(buffer, unsafe { co::recvfrom(self.0, at, room) })
    }

    /// One receiving [`Transfer`] into `buffer`'s room under `timeout`.
    pub(crate) fn receive(self, op: Transfer, buffer: &mut ZBuf, timeout: f64) -> ssize_t {
        let (at, room) = room(buffer);
        // SAFETY: `room` writable bytes at `at`.
        filled(buffer, unsafe { self.transfer(op, at, room, timeout) })
    }

    /// `get_addr()` and `get_port()`; the text lives until this thread's next
    /// presentation.
    pub(crate) fn address<'a>(self) -> (&'a CStr, c_int) {
        // SAFETY: an open socket's network::Socket record.
        address(&unsafe { self.record().record() }.info)
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn sendto(self, address: &[u8], port: i64, data: &[u8]) -> ssize_t {
        // SAFETY: a live socket and the slices' bytes.
        unsafe {
            co::sendto(
                self.0,
                address.as_ptr().cast(),
                address.len(),
                port as c_int,
                data.as_ptr().cast(),
                data.len(),
            )
        }
    }

    #[expect(clippy::cast_possible_truncation, reason = "upstream's int how")]
    pub(crate) fn shutdown(self, how: i64) -> bool {
        // SAFETY: a live socket.
        unsafe { co::shutdown(self.0, how as c_int) }
    }

    pub(crate) fn getsockname(self) -> bool {
        // SAFETY: a live socket.
        unsafe { co::getsockname(self.0) }
    }

    /// `getpeername(&sa)`: the peer's address text and port.
    pub(crate) fn getpeername(self) -> Option<(CString, c_int)> {
        // SAFETY: an all-zero address is valid.
        let mut sa: SocketAddress = unsafe { std::mem::zeroed() };
        // SAFETY: a live socket.
        if !unsafe { co::getpeername(self.0, &mut sa) } {
            return None;
        }
        let (host, port) = address(&sa);
        Some((host.to_owned(), port))
    }

    #[expect(clippy::cast_sign_loss, reason = "an event bit set")]
    pub(crate) fn cancel(self, event: c_int) -> bool {
        // SAFETY: a live socket.
        unsafe { co::cancel(self.0, event as u32) }
    }

    pub(crate) fn ssl_handshake(self) -> bool {
        // SAFETY: a live socket.
        unsafe { co::ssl_handshake(self.0) }
    }

    /// `open_length_check` (0), `open_eof_check` (1) or `http2` (2) = `value`.
    pub(crate) fn set_flag(self, flag: c_int, value: bool) {
        let r = self.record();
        *match flag {
            0 => &mut r.open_length_check,
            1 => &mut r.open_eof_check,
            _ => &mut r.http2,
        } = value;
    }

    /// `mqtt::set_protocol` (0) or `Protocol::default_length_func` (1).
    pub(crate) fn protocol_preset(self, kind: c_int) {
        let protocol = &mut self.record().protocol;
        if kind == 0 {
            // SAFETY: the socket's protocol.
            unsafe { mqtt_set_protocol(protocol) };
        } else {
            protocol.get_package_length = default_length_func as *mut c_void;
        }
    }

    /// `set_option(level, name, value)`, warning (a log line) on failure.
    pub(crate) fn set_option(self, level: c_int, name: c_int, value: c_int) -> bool {
        // SAFETY: a live socket.
        unsafe { co::set_option(self.0, level, name, value) }
    }

    pub(crate) fn set_timeout(self, timeout: f64, kind: c_int) {
        // SAFETY: a live socket.
        unsafe { co::set_timeout(self.0, timeout, kind) };
    }

    pub(crate) fn get_timeout(self, kind: c_int) -> f64 {
        // SAFETY: a live socket.
        unsafe { co::get_timeout(self.0, kind) }
    }

    /// `set_http_proxy` or `set_socks5_proxy`.
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    pub(crate) fn proxy(self, http: bool, host: &[u8], port: i64, user: &[u8], password: &[u8]) {
        // SAFETY: a live socket.
        unsafe { co::proxy(self.0, http, host, port as c_int, user, password) };
    }

    /// `enable_ssl_encrypt()`: false when TLS was already on.
    pub(crate) fn enable_ssl(self) -> bool {
        // SAFETY: a live socket.
        unsafe { co::enable_ssl(self.0) }
    }

    /// The socket's `SSLContext` owner; null while TLS is off.
    pub(crate) fn ssl_context(self) -> *const c_void {
        // SAFETY: a live socket.
        unsafe { co::ssl_context(self.0) }
    }

    /// The socket's domain and type.
    pub(crate) fn family(self) -> (c_int, c_int) {
        let r = self.record();
        (r.sock_domain, r.sock_type)
    }

    /// `swSocketType`.
    pub(crate) fn kind(self) -> c_int {
        self.record().kind
    }

    /// `open_length_check || open_eof_check`.
    pub(crate) fn packet_mode(self) -> bool {
        let record = self.record();
        record.open_length_check || record.open_eof_check
    }

    pub(crate) fn is_connected(self) -> bool {
        // SAFETY: a live socket.
        unsafe { co::is_connected(self.0) }
    }

    /// `get_ssl() != nullptr`.
    pub(crate) fn has_ssl(self) -> bool {
        self.net().is_some_and(|record| !record.ssl.is_null())
    }

    /// The peer certificate; the socket has TLS.
    pub(crate) fn peer_cert(self) -> Option<ZString> {
        // SAFETY: a live record with TLS, as the callers check.
        unsafe { peer_cert(self.socket_record()) }
    }

    pub(crate) fn verify_peer(self, allow_self_signed: bool) -> bool {
        // SAFETY: a live socket with TLS, as the callers check.
        unsafe { co::ssl_verify(self.0, allow_self_signed) }
    }

    /// `set_dtor`: `hook(arg, socket)` runs once as the socket is destroyed.
    /// # Safety
    /// `hook` may be called with `arg` until then.
    pub(crate) unsafe fn close_hook(
        self,
        hook: unsafe extern "C" fn(*mut c_void, *mut c_void),
        arg: *mut c_void,
    ) {
        // SAFETY: a live socket; per the caller.
        unsafe { co::close_hook(self.0, hook, arg) };
    }
}

/// `package_length_func` from a [`LengthCallable`]: a callable replaces any
/// earlier one. A `zend_try` body.
pub(crate) unsafe extern "C" fn swoole_socket_coro_length_callable(call: *mut c_void) {
    // SAFETY: the caller's arguments.
    let args = unsafe { &*call.cast::<LengthCallable>() };
    // SAFETY: inside the caller's `zend_try`, owning nothing.
    let callable = unsafe { swoole_rs_phpruntime::callable_create(args.value) };
    if callable.is_null() {
        return;
    }
    let protocol = &mut args.socket.record().protocol;
    protocol.get_package_length = php_swoole_length_func as *mut c_void;
    if !protocol.private_data_1.is_null() {
        // SAFETY: as above; the earlier callable.
        unsafe { swoole_rs_phpruntime::callable_free(protocol.private_data_1) };
    }
    protocol.private_data_1 = callable;
    protocol.package_length_size = 0;
    protocol.package_length_type = 0;
    protocol.package_length_offset = IPC_BUFFER_SIZE;
}

/// The fields of `info`, in upstream's order (crates/server's list).
#[cfg(target_os = "linux")]
pub(crate) fn tcp_fields(info: &libc::tcp_info) -> impl Iterator<Item = (&'static CStr, u64)> {
    let mut facts = [const {
        Fact {
            name: std::ptr::null(),
            value: 0,
        }
    }; 64];
    // SAFETY: a `tcp_info` and room for 64 facts.
    let n = unsafe {
        swr_v1_server_tcp_info(
            std::ptr::from_ref(info).cast(),
            facts.as_mut_ptr(),
            facts.len(),
        )
    };
    facts.into_iter().take(n).map(|fact| {
        // SAFETY: a static field name.
        (unsafe { CStr::from_ptr(fact.name) }, fact.value)
    })
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

    /// The client's public record, at the start of its state; borrowed only
    /// between its operations.
    pub(crate) fn record<'a>(self) -> &'a mut ClientRecord {
        // SAFETY: as in `get`; nothing else touches the record while the borrow lasts.
        unsafe { &mut *self.0.cast::<ClientRecord>() }
    }

    /// `cli->socket`, or null.
    pub(crate) fn socket(self) -> *mut SocketRecord {
        self.record().socket
    }

    /// The socket record of a client that has one.
    pub(crate) fn socket_mut<'a>(self) -> &'a mut SocketRecord {
        // SAFETY: a constructed client's socket, live while the client is.
        unsafe { &mut *self.socket() }
    }

    /// A [`Call`] of `op` on this client.
    pub(crate) fn call(self, op: Op) -> Call {
        Call::new(self.0, op)
    }
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
    let callable = std::mem::replace(&mut cli.record().protocol.private_data_1, ptr::null_mut());
    if !callable.is_null() {
        callable_free(callable)?;
    }
    cli.call(Op::Free).run().map(drop)
}

/// Running a [`Call`] from a PHP method.
pub(crate) trait Run {
    /// Run the call inside `zend_try`.
    /// # Errors
    /// PHP code it ran bailed out.
    fn run(&mut self) -> Result<i64, Bailout>;

    /// Run a call that cannot reach PHP code (a sync client's send or plain
    /// receive), without the `zend_try` frame.
    fn run_plain(&mut self) -> i64;
}

impl Run for Call {
    fn run(&mut self) -> Result<i64, Bailout> {
        // SAFETY: a call on a live client, holding what its operation reads and
        // writes; the body owns nothing.
        unsafe { bailout::trap(swr_v1_clients_run, ptr::from_mut(self).cast()) }?;
        Ok(self.result)
    }

    fn run_plain(&mut self) -> i64 {
        // SAFETY: as in `run`; the caller rules out callbacks and errors.
        unsafe { swr_v1_clients_run(ptr::from_mut(self).cast()) };
        self.result
    }
}

/// `sw_callable_create(*slot)` into the slot: a `zend_try` body.
unsafe extern "C" fn callable_create(slot: *mut c_void) {
    // SAFETY: the caller's slot, holding a live zval.
    let slot = unsafe { &mut *slot.cast::<*mut c_void>() };
    // SAFETY: inside the caller's `zend_try`, owning nothing.
    *slot = unsafe { swoole_rs_phpruntime::callable_create(slot.cast()) };
}

/// `sw_callable_create(value)`: the callable, or null after a warning.
/// # Errors
/// The warning's handler bailed out.
pub(crate) fn callable(value: *mut zval) -> Result<*mut c_void, Bailout> {
    let mut slot = value.cast::<c_void>();
    // SAFETY: a body over the slot, which holds a live zval.
    unsafe { bailout::trap(callable_create, (&raw mut slot).cast()) }?;
    Ok(slot)
}

/// `sw_callable_free(callable)`.
/// # Errors
/// A destructor bailed out.
pub(crate) fn callable_free(callable: *mut c_void) -> Result<(), Bailout> {
    // SAFETY: a `zend_try` body over a callable `callable` made, freed once.
    unsafe { bailout::trap(free_callable, callable) }
}

/// `sw_zend_call_function_ex2(nullptr, callable->ptr(), argv, retval)`: whether
/// the call succeeded.
/// # Errors
/// The callable bailed out.
pub(crate) fn invoke(
    callable: *mut c_void,
    argv: &mut [zval],
    retval: *mut zval,
) -> Result<bool, Bailout> {
    // SAFETY: a live callable and arguments; `retval` is null or writable.
    unsafe { swoole_rs_phpruntime::callable_call(callable, argv, retval, true) }
}

/// A `Socket` over a dup of the client's fd: `php_swoole_convert_to_socket`, which
/// makes one whatever the import's result. `None` without ext/sockets.
pub(crate) fn export_socket(cli: Cli) -> Option<Owned> {
    if !built_with(Switch::Sockets) {
        return None;
    }
    // SAFETY: dup(2) of the client's descriptor.
    let fd = unsafe { libc::dup(cli.socket_mut().fd) };
    if fd < 0 {
        return None;
    }
    let socket = Owned::default();
    // SAFETY: an undefined slot this owns, and a descriptor it takes.
    unsafe { swoole_rs_phpswoole::socket_create(socket.as_ptr(), fd) };
    Some(socket)
}

/// The peer certificate of `socket`, a live record with TLS:
/// `ssl_get_peer_certificate` into a buffer of `sw_tg_buffer()`'s size.
pub(crate) unsafe fn peer_cert(socket: *mut SocketRecord) -> Option<ZString> {
    let mut buffer = vec![0_u8; swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize];
    // SAFETY: a live record with TLS and a writable buffer.
    let length = unsafe {
        swoole_rs_tls::upstream::socket_ssl_get_peer_certificate_pem(
            socket,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    let length = usize::try_from(length).ok()?;
    Some(ZString::new(buffer.get(..length).unwrap_or(&buffer)))
}

/// The unwritten room of `buffer`.
pub(crate) fn room(buffer: &mut ZBuf) -> (*mut c_char, size_t) {
    let written = buffer.len();
    let at = buffer.as_mut_ptr().wrapping_add(written);
    (at.cast(), buffer.capacity() - written)
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

/// The address and port of `address`: `get_addr()` and `get_port()`.
pub(crate) fn address(address: &SocketAddress) -> (&CStr, c_int) {
    // SAFETY: a live address; the text lives until this thread's next
    // presentation, after the caller copies it.
    let host = unsafe { swr_v1_clients_address_host(address) };
    // SAFETY: as above, NUL-terminated.
    let host = unsafe { CStr::from_ptr(host) };
    // SAFETY: as above.
    (host, unsafe { swr_v1_clients_address_port(address) })
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

/// `php_swoole_check_reactor()`; a `swr_zend_try_hook` body.
pub(crate) unsafe extern "C" fn swoole_rs_check_reactor() {
    swoole_rs_phpruntime::check_reactor();
}
