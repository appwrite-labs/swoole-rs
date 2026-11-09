//! The coroutine socket's operations the methods need, over
//! `swoole_rs_phpruntime::co` and the socket's record, plus Swoole's PHP error
//! reporting; and `Swoole\Client`'s `network::Client`, whose operations
//! crates/clients runs.
use crate::settings::{default_length_func, mqtt_set_protocol, php_swoole_length_func};
use libc::{c_char, c_int, c_void, iovec, size_t, ssize_t};
use std::{ffi::CStr, ptr};
use swoole_rs_clients::{
    address::{swr_v1_clients_address_host, swr_v1_clients_address_port},
    client::{Call, Client, ClientRecord, Op, swr_v1_clients_run},
};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_phpruntime::{Report, co, report};
use swoole_rs_phpswoole::{Capability, built_with};
use swoole_rs_reactor::SocketRecord;
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{zend_string, zval},
};

/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;

/// A `SocketImpl *`.
pub(crate) type Socket = *mut libc::c_void;

/// `package_length_func`'s arguments.
#[repr(C)]
pub(crate) struct LengthCallable {
    pub(crate) socket: Socket,
    pub(crate) value: *mut zval,
}

unsafe extern "C" {
    /// `php_swoole_convert_to_socket`'s Socket over `fd`, into `out`.
    fn swr_cpp_phpthread_php_socket_create(out: *mut zval, fd: c_int) -> bool;
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
const TIMEOUT_READ: c_int = 1 << 2;
const TIMEOUT_WRITE: c_int = 1 << 3;
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

/// `swoole_strerror(code)`.
pub(crate) fn swoole_strerror(code: c_int) -> *const c_char {
    swoole_rs_error::swoole_strerror(code)
}

/// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
pub(crate) unsafe fn swoole_socket_coro_unconstructed() {
    // SAFETY: a fatal error; the caller owns nothing it skips.
    unsafe { swoole_rs_corebase::fatal_error(WRONG_OPERATION, c"must call constructor first") };
}

fn co_record<'a>(socket: Socket) -> &'a mut co::CoSocket {
    // SAFETY: every caller passes a live socket.
    unsafe { co::record(socket) }
}

/// `socket_coro_init`'s socket half: zero copy and the zend string allocator,
/// then its fd, domain, type and protocol into `fields`.
pub(crate) unsafe fn swoole_socket_coro_adopt(socket: Socket, fields: *mut i64) {
    let r = co_record(socket);
    r.zero_copy = true;
    r.buffer_allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
    // SAFETY: the caller's four fields.
    let fields = unsafe { &mut *fields.cast::<[i64; 4]>() };
    *fields = [r.sock_fd, r.sock_domain, r.sock_type, r.sock_protocol].map(i64::from);
}

/// `*result = socket`: whether making it bailed out.
fn made(socket: Result<Socket, Bailout>, result: *mut Socket) -> u8 {
    socket.map_or(1, |socket| {
        // SAFETY: the caller's writable slot.
        unsafe { *result = socket };
        0
    })
}

pub(crate) unsafe fn swoole_socket_coro_new_family(
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    result: *mut Socket,
) -> u8 {
    made(co::make(-1, domain, kind, protocol, true), result)
}
pub(crate) unsafe fn swoole_socket_coro_new_type(kind: c_int, result: *mut Socket) -> u8 {
    made(co::make(-1, 0, kind, 0, false), result)
}
pub(crate) unsafe fn swoole_socket_coro_new_fd(fd: c_int, kind: c_int, result: *mut Socket) -> u8 {
    made(co::make(fd, 0, kind, 0, false), result)
}
pub(crate) unsafe fn swoole_socket_coro_new_fd_family(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    result: *mut Socket,
) -> u8 {
    made(co::make(fd, domain, kind, protocol, true), result)
}
pub(crate) unsafe fn swoole_socket_coro_fd(socket: Socket) -> c_int {
    co_record(socket).sock_fd
}
pub(crate) unsafe fn swoole_socket_coro_destroy(socket: Socket) -> u8 {
    // SAFETY: per the caller, a live owned socket.
    u8::from(unsafe { co::free(socket) }.is_err())
}
/// `move_fd()`: the socket forgets its fd, which stays open.
pub(crate) unsafe fn swoole_socket_coro_move_fd(socket: Socket) {
    let r = co_record(socket);
    r.sock_fd = -1;
    // SAFETY: its network::Socket record.
    unsafe { r.record() }.fd = -1;
}
pub(crate) unsafe fn swoole_socket_coro_close_native(socket: Socket) -> u8 {
    // SAFETY: per the caller.
    u8::from(unsafe { co::close_trapped(socket) }.is_err())
}
unsafe extern "C" fn free_callable(callable: *mut c_void) {
    // SAFETY: a `zend_try` body over a callable `callable_create` made.
    unsafe { swoole_rs_phpruntime::callable_free(callable) };
}
pub(crate) unsafe fn swoole_socket_coro_callable_free(callable: *mut c_void) -> u8 {
    // SAFETY: per the caller.
    u8::from(unsafe { bailout::trap(free_callable, callable) }.is_err())
}
pub(crate) unsafe fn swoole_socket_coro_is_closed(socket: Socket) -> bool {
    co_record(socket).is_closed()
}
/// `errCode`, with `errMsg` in `message`.
pub(crate) unsafe fn swoole_socket_coro_error(
    socket: Socket,
    message: *mut *const c_char,
) -> c_int {
    let r = co_record(socket);
    // SAFETY: the caller's slot.
    unsafe { *message = r.err_msg };
    r.err_code
}
/// `set_err(code, message)`.
pub(crate) unsafe fn swoole_socket_coro_set_error(
    socket: Socket,
    code: c_int,
    message: *const c_char,
) {
    // SAFETY: per the caller, a NUL-terminated message.
    let message = unsafe { CStr::from_ptr(message) };
    // SAFETY: per the caller.
    unsafe { co::set_error(socket, code, message.to_bytes()) };
}
#[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
pub(crate) unsafe fn swoole_socket_coro_bind(
    socket: Socket,
    address: *const c_char,
    length: size_t,
    port: i64,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::bind(socket, address, length, port as c_int) }
}
#[expect(clippy::cast_possible_truncation, reason = "upstream's int backlog")]
pub(crate) unsafe fn swoole_socket_coro_listen(socket: Socket, backlog: i64) -> bool {
    // SAFETY: per the caller.
    unsafe { co::listen(socket, backlog as c_int) }
}
/// `accept(timeout)`; the connection copies the listener's length callable,
/// so closing it leaves the listener's.
pub(crate) unsafe fn swoole_socket_coro_accept(socket: Socket, timeout: f64) -> Socket {
    // SAFETY: per the caller.
    let conn = unsafe { co::accept(socket, timeout) };
    let callable = co_record(socket).protocol.private_data_1;
    if !conn.is_null() && !callable.is_null() {
        // SAFETY: the listener's live callable.
        co_record(conn).protocol.private_data_1 =
            unsafe { swoole_rs_phpruntime::callable_dup(callable) };
    }
    conn
}
pub(crate) unsafe fn swoole_socket_coro_port_required(socket: Socket) -> bool {
    co_record(socket).kind <= SOCK_UDP6
}
/// `connect(host, port)` under a connect timeout of `timeout`.
#[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
pub(crate) unsafe fn swoole_socket_coro_connect(
    socket: Socket,
    host: *const c_char,
    length: size_t,
    port: i64,
    timeout: f64,
) -> bool {
    // SAFETY: per the caller.
    let _timeout = unsafe { co::Timeout::set(socket, timeout, TIMEOUT_CONNECT) };
    // SAFETY: as above.
    unsafe { co::connect(socket, host, length, port as c_int, 0) }
}
pub(crate) unsafe fn swoole_socket_coro_check_liveness(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::check_liveness(socket) }
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an event bit set"
)]
pub(crate) unsafe fn swoole_socket_coro_bound_cid(socket: Socket, event: i64) -> i64 {
    // SAFETY: per the caller.
    unsafe { co::bound_cid(socket, event as u32) }
}
/// One [`Transfer`] of `length` bytes at `buffer` under `timeout`.
pub(crate) unsafe fn swoole_socket_coro_transfer(
    socket: Socket,
    op: c_int,
    buffer: *mut c_char,
    length: size_t,
    timeout: f64,
) -> ssize_t {
    use co::Io;
    if op == Transfer::Peek as c_int {
        // SAFETY: per the caller.
        return unsafe { co::io(socket, Io::Peek, buffer, length) };
    }
    let kind = if op < Transfer::Send as c_int {
        TIMEOUT_READ
    } else {
        TIMEOUT_WRITE
    };
    // SAFETY: per the caller.
    let _timeout = unsafe { co::Timeout::set(socket, timeout, kind) };
    let io = match op {
        1 => Io::Recv,
        2 => Io::RecvAll,
        // SAFETY: per the caller.
        3 => return unsafe { co::recv_line(socket, buffer, length) },
        // SAFETY: per the caller.
        4 => return unsafe { co::recv_with_buffer(socket, buffer, length) },
        5 => Io::Send,
        _ => Io::SendAll,
    };
    // SAFETY: per the caller.
    unsafe { co::io(socket, io, buffer, length) }
}
/// `recv_packet(timeout)`; with `clear_empty`, an empty packet clears the
/// read buffer.
pub(crate) unsafe fn swoole_socket_coro_recv_packet(
    socket: Socket,
    timeout: f64,
    clear_empty: bool,
) -> ssize_t {
    // SAFETY: per the caller.
    let retval = unsafe { co::recv_packet(socket, timeout) };
    let buffer = co_record(socket)
        .read_buffer
        .cast::<swoole_rs_buffer::string::CppString>();
    if retval == 0 && clear_empty && !buffer.is_null() {
        // SAFETY: the socket's live read buffer.
        let buffer = unsafe { &mut *buffer };
        buffer.length = 0;
        buffer.offset = 0;
    }
    retval
}
/// `pop_packet()`: the `ZSTR_VAL` of the packet's string, or null after
/// `set_err(ENOMEM)`.
pub(crate) unsafe fn swoole_socket_coro_pop_packet(socket: Socket) -> *mut c_char {
    let r = co_record(socket);
    // SAFETY: recv_packet made the read buffer.
    let buffer = unsafe { &mut *r.read_buffer.cast::<swoole_rs_buffer::string::CppString>() };
    let packet: *mut c_char = if buffer.offset == 0 {
        std::ptr::null_mut()
    } else {
        buffer.pop(r.buffer_init_size).cast()
    };
    if packet.is_null() {
        // SAFETY: a live socket.
        unsafe { co::set_err(socket, libc::ENOMEM) };
    }
    packet
}
/// `readv`/`readv_all`/`writev`/`writev_all` (`op` 0..=3) over `count`
/// vectors under `timeout`; `index` and `offset` report where it stopped.
pub(crate) unsafe fn swoole_socket_coro_vector(
    socket: Socket,
    op: c_int,
    vectors: *mut iovec,
    count: c_int,
    timeout: f64,
) -> (ssize_t, c_int, size_t) {
    let kind = if op < 2 { TIMEOUT_READ } else { TIMEOUT_WRITE };
    // SAFETY: per the caller.
    let _timeout = unsafe { co::Timeout::set(socket, timeout, kind) };
    // SAFETY: per the caller.
    unsafe { co::vector(socket, op, vectors, count) }
}
pub(crate) unsafe fn swoole_socket_coro_sendfile(
    socket: Socket,
    file: *const c_char,
    offset: i64,
    length: i64,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::sendfile(socket, file, offset, length) }
}
/// `recvfrom(buffer, length)` under a read timeout of `timeout`.
pub(crate) unsafe fn swoole_socket_coro_recvfrom(
    socket: Socket,
    buffer: *mut c_char,
    length: size_t,
    timeout: f64,
) -> ssize_t {
    // SAFETY: per the caller.
    let _timeout = unsafe { co::Timeout::set(socket, timeout, TIMEOUT_READ) };
    // SAFETY: as above.
    unsafe { co::recvfrom(socket, buffer, length) }
}
/// `get_addr()`, with `get_port()` in `port`.
pub(crate) unsafe fn swoole_socket_coro_address(socket: Socket, port: *mut c_int) -> *const c_char {
    // SAFETY: an open socket's network::Socket record.
    let info = &raw const unsafe { co_record(socket).record() }.info;
    // SAFETY: a live address.
    let number = unsafe { swoole_rs_clients::address::swr_v1_clients_address_port(info) };
    // SAFETY: the caller's slot.
    unsafe { *port = number };
    // SAFETY: a live address.
    unsafe { swoole_rs_clients::address::swr_v1_clients_address_host(info) }
}
#[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
pub(crate) unsafe fn swoole_socket_coro_sendto(
    socket: Socket,
    address: *const c_char,
    length: size_t,
    port: i64,
    data: *const c_char,
    data_length: size_t,
) -> ssize_t {
    // SAFETY: per the caller.
    unsafe { co::sendto(socket, address, length, port as c_int, data, data_length) }
}
#[expect(clippy::cast_possible_truncation, reason = "upstream's int how")]
pub(crate) unsafe fn swoole_socket_coro_shutdown(socket: Socket, how: i64) -> bool {
    // SAFETY: per the caller.
    unsafe { co::shutdown(socket, how as c_int) }
}
pub(crate) unsafe fn swoole_socket_coro_getsockname(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::getsockname(socket) }
}
/// `getpeername(&sa)`: the address, NUL-terminated, into `address`
/// (`swoole_strlcpy`).
pub(crate) unsafe fn swoole_socket_coro_getpeername(
    socket: Socket,
    address: *mut c_char,
    size: size_t,
    port: *mut c_int,
) -> bool {
    // SAFETY: an all-zero address is valid.
    let mut sa: SocketAddress = unsafe { std::mem::zeroed() };
    // SAFETY: per the caller.
    if !unsafe { co::getpeername(socket, &mut sa) } {
        return false;
    }
    // SAFETY: the address getpeername filled.
    let host = unsafe { swoole_rs_clients::address::swr_v1_clients_address_host(&raw const sa) };
    // SAFETY: a NUL-terminated host.
    let host = unsafe { CStr::from_ptr(host) }.to_bytes();
    // SAFETY: the caller's `size` (> 0) writable bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(address.cast::<u8>(), size) };
    let terminated = host.iter().copied().take(size - 1).chain([0]);
    for (slot, byte) in out.iter_mut().zip(terminated) {
        *slot = byte;
    }
    // SAFETY: as above.
    let number = unsafe { swoole_rs_clients::address::swr_v1_clients_address_port(&raw const sa) };
    // SAFETY: the caller's slot.
    unsafe { *port = number };
    true
}
#[expect(clippy::cast_sign_loss, reason = "an event bit set")]
pub(crate) unsafe fn swoole_socket_coro_cancel(socket: Socket, event: c_int) -> bool {
    // SAFETY: per the caller.
    unsafe { co::cancel(socket, event as u32) }
}
pub(crate) unsafe fn swoole_socket_coro_ssl_handshake(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::ssl_handshake(socket) }
}
/// `open_length_check` (0), `open_eof_check` (1) or `http2` (2) = `value`.
pub(crate) unsafe fn swoole_socket_coro_set_flag(socket: Socket, flag: c_int, value: bool) {
    let r = co_record(socket);
    *match flag {
        0 => &mut r.open_length_check,
        1 => &mut r.open_eof_check,
        _ => &mut r.http2,
    } = value;
}
/// `&protocol`.
pub(crate) unsafe fn swoole_socket_coro_protocol(socket: Socket) -> *mut c_void {
    (&raw mut co_record(socket).protocol).cast()
}
/// `mqtt::set_protocol` (0) or `Protocol::default_length_func` (1).
pub(crate) unsafe fn swoole_socket_coro_protocol_preset(socket: Socket, kind: c_int) {
    let protocol = &mut co_record(socket).protocol;
    if kind == 0 {
        // SAFETY: the socket's protocol.
        unsafe { mqtt_set_protocol(protocol) };
    } else {
        protocol.get_package_length = default_length_func as *mut c_void;
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
    let protocol = &mut co_record(args.socket).protocol;
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
/// `set_option(level, name, value)`, warning (a log line) on failure.
pub(crate) unsafe fn swoole_socket_coro_set_option(
    socket: Socket,
    level: c_int,
    name: c_int,
    value: c_int,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::set_option(socket, level, name, value) }
}
pub(crate) unsafe fn swoole_socket_coro_set_timeout(socket: Socket, timeout: f64, kind: c_int) {
    // SAFETY: per the caller.
    unsafe { co::set_timeout(socket, timeout, kind) };
}
pub(crate) unsafe fn swoole_socket_coro_get_timeout(socket: Socket, kind: c_int) -> f64 {
    // SAFETY: per the caller.
    unsafe { co::get_timeout(socket, kind) }
}
/// `set_http_proxy` or `set_socks5_proxy`.
#[expect(clippy::too_many_arguments, reason = "three strings and a port")]
#[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
pub(crate) unsafe fn swoole_socket_coro_proxy(
    socket: Socket,
    http: bool,
    host: *const c_char,
    host_length: size_t,
    port: i64,
    user: *const c_char,
    user_length: size_t,
    password: *const c_char,
    password_length: size_t,
) {
    let bytes = |text: *const c_char, length| {
        // SAFETY: the caller's `length` bytes.
        unsafe { std::slice::from_raw_parts(text.cast::<u8>(), length) }
    };
    // SAFETY: per the caller.
    unsafe {
        co::proxy(
            socket,
            http,
            bytes(host, host_length),
            port as c_int,
            bytes(user, user_length),
            bytes(password, password_length),
        );
    }
}
/// `enable_ssl_encrypt()`: false when TLS was already on.
pub(crate) unsafe fn swoole_socket_coro_enable_ssl(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::enable_ssl(socket) }
}
/// The socket's `SSLContext` owner; null while TLS is off.
pub(crate) unsafe fn swoole_socket_coro_ssl_context(socket: Socket) -> *const c_void {
    // SAFETY: per the caller.
    unsafe { co::ssl_context(socket) }
}
/// Whether the build has an optional SSL option: the host name (0) or grease
/// (1, `BoringSSL`'s).
pub(crate) unsafe fn swoole_socket_coro_ssl_supports(option: c_int) -> bool {
    match option {
        0 => built_with(Capability::TlsHostName),
        1 => built_with(Capability::BoringSsl),
        _ => false,
    }
}
/// The socket's domain, and its type into `kind`.
pub(crate) unsafe fn swoole_socket_coro_family(socket: Socket, kind: *mut c_int) -> c_int {
    let r = co_record(socket);
    // SAFETY: the caller's slot.
    unsafe { *kind = r.sock_type };
    r.sock_domain
}
/// `socket->errCode = code`, nothing else.
pub(crate) unsafe fn swoole_socket_coro_set_err_code(socket: Socket, code: c_int) {
    co_record(socket).err_code = code;
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
    // SAFETY: a callable `callable` made, freed once.
    if unsafe { swoole_socket_coro_callable_free(callable) } == 0 {
        Ok(())
    } else {
        Err(Bailout)
    }
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
pub(crate) fn export_socket(cli: Cli) -> Option<zval> {
    if !built_with(Capability::Sockets) {
        return None;
    }
    // SAFETY: dup(2) of the client's descriptor.
    let fd = unsafe { libc::dup(cli.socket_mut().fd) };
    if fd < 0 {
        return None;
    }
    let mut socket = swoole_rs_zend::long_zval(0);
    // SAFETY: a writable zval and a descriptor it takes.
    unsafe { swr_cpp_phpthread_php_socket_create(&raw mut socket, fd) };
    Some(socket)
}

/// The peer certificate of `socket`, a live record with TLS, as a new string,
/// or null: `ssl_get_peer_certificate` into a buffer of `sw_tg_buffer()`'s size.
pub(crate) unsafe fn peer_cert(socket: *mut SocketRecord) -> *mut zend_string {
    let mut buffer = vec![0_u8; swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize];
    // SAFETY: a live record with TLS and a writable buffer.
    let length = unsafe {
        swoole_rs_tls::upstream::socket_ssl_get_peer_certificate_pem(
            socket,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    match usize::try_from(length) {
        Ok(length) => swoole_rs_zend::string::copy(buffer.get(..length).unwrap_or(&buffer)),
        Err(_) => std::ptr::null_mut(),
    }
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

/// The record of `socket`, a live coroutine socket, read fresh on every use: it
/// changes when the socket is reinitialised.
///
/// # Safety
/// `socket` is live, and nothing else touches its record while the borrow lasts.
pub(crate) unsafe fn record<'a>(socket: Socket) -> Option<&'a mut swoole_rs_reactor::SocketRecord> {
    // SAFETY: as the caller promises.
    let record = unsafe { swoole_client_coro_socket_record(socket) };
    // SAFETY: as above.
    unsafe { record.as_mut() }
}

/// `php_swoole_check_reactor()`; a `swr_zend_try_hook` body.
pub(crate) unsafe extern "C" fn swoole_rs_check_reactor() {
    swoole_rs_phpruntime::check_reactor();
}

/// `SW_TIMEOUT_CONNECT` and `SW_TIMEOUT_RDWR`.
const TIMEOUT_CONNECT: c_int = 1 << 1;
const TIMEOUT_RDWR: c_int = (1 << 2) | (1 << 3);

/// `set_timeout(timeout, SW_TIMEOUT_CONNECT)`, then `connect(host, port, flags)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream's int port and flags"
)]
pub(crate) unsafe fn swoole_client_coro_connect(
    socket: Socket,
    host: *const c_char,
    port: i64,
    timeout: f64,
    flags: i64,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::set_timeout(socket, timeout, TIMEOUT_CONNECT) };
    // SAFETY: a NUL-terminated host.
    let length = unsafe { libc::strlen(host) };
    // SAFETY: per the caller.
    unsafe { co::connect(socket, host, length, port as c_int, flags as c_int) }
}

/// `set_timeout(timeout, SW_TIMEOUT_RDWR)`.
pub(crate) unsafe fn swoole_client_coro_rdwr_timeout(socket: Socket, timeout: f64) {
    // SAFETY: per the caller.
    unsafe { co::set_timeout(socket, timeout, TIMEOUT_RDWR) };
}

pub(crate) unsafe fn swoole_client_coro_close(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::close(socket) }
}

/// The coroutine socket's `network::Socket` record (`get_socket()`), or null.
pub(crate) unsafe fn swoole_client_coro_socket_record(
    socket: Socket,
) -> *mut swoole_rs_reactor::SocketRecord {
    // SAFETY: per the caller.
    unsafe { co::record(socket) }.socket
}

pub(crate) unsafe fn swoole_client_coro_type(socket: Socket) -> c_int {
    // SAFETY: per the caller.
    unsafe { co::record(socket) }.kind
}

/// `open_length_check || open_eof_check`.
pub(crate) unsafe fn swoole_client_coro_packet_mode(socket: Socket) -> bool {
    // SAFETY: per the caller.
    let record = unsafe { co::record(socket) };
    record.open_length_check || record.open_eof_check
}

pub(crate) unsafe fn swoole_client_coro_is_connected(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::is_connected(socket) }
}

/// `get_ssl() != nullptr`.
pub(crate) unsafe fn swoole_client_coro_has_ssl(socket: Socket) -> bool {
    // SAFETY: per the caller.
    let record = unsafe { swoole_client_coro_socket_record(socket) };
    // SAFETY: an open socket's record.
    !record.is_null() && !unsafe { (*record).ssl }.is_null()
}

/// The peer certificate as a new string, or null.
pub(crate) unsafe fn swoole_client_coro_peer_cert(socket: Socket) -> *mut zend_string {
    // SAFETY: per the caller.
    let record = unsafe { swoole_client_coro_socket_record(socket) };
    // SAFETY: per the caller, a socket with TLS.
    unsafe { peer_cert(record) }
}

pub(crate) unsafe fn swoole_client_coro_verify_peer(
    socket: Socket,
    allow_self_signed: bool,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::ssl_verify(socket, allow_self_signed) }
}
