//! The coroutine socket's operations the methods need, over
//! `swoole_rs_phpruntime::co` and the socket's record, plus Swoole's PHP error
//! reporting; and what only C++ can do for `Swoole\Client` (defined in
//! `native/clients/client.cc`, declared in
//! `native/clients/swoole_rs_phpclients.h`).
use crate::settings::{default_length_func, mqtt_set_protocol, php_swoole_length_func};
use libc::{c_char, c_int, c_void, iovec, size_t, ssize_t};
use std::ffi::CStr;
use swoole_rs_clients::{
    address::{swr_v1_clients_address_host, swr_v1_clients_address_port},
    client::ClientRecord,
};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_phpruntime::{Report, co, report};
use swoole_rs_phpswoole::{Capability, built_with};
use swoole_rs_reactor::SocketRecord;
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{zend_object, zend_string, zval},
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
#[unsafe(no_mangle)]
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
    extern "C" fn system(code: c_int) -> *const c_char {
        // SAFETY: libc's message table.
        unsafe { libc::strerror(code) }
    }
    // SAFETY: a pure lookup.
    unsafe { swoole_rs_error::swr_v1_error_strerror(code, Some(system)) }
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
unsafe extern "C" fn callable_free(callable: *mut c_void) {
    // SAFETY: a `zend_try` body over a callable `callable_create` made.
    unsafe { swoole_rs_phpruntime::callable_free(callable) };
}
pub(crate) unsafe fn swoole_socket_coro_callable_free(callable: *mut c_void) -> u8 {
    // SAFETY: per the caller.
    u8::from(unsafe { bailout::trap(callable_free, callable) }.is_err())
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
    index: *mut c_int,
    offset: *mut size_t,
) -> ssize_t {
    let kind = if op < 2 { TIMEOUT_READ } else { TIMEOUT_WRITE };
    // SAFETY: per the caller.
    let _timeout = unsafe { co::Timeout::set(socket, timeout, kind) };
    // SAFETY: per the caller.
    unsafe { co::vector(socket, op, vectors, count, index, offset) }
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
    let n = host.len().min(size - 1);
    out[..n].copy_from_slice(&host[..n]);
    out[n] = 0;
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

unsafe extern "C" {
    /// `php_swoole_error`, `_fatal_error`, `_sys_error` or `_core_error`
    /// (`kind`, [`ErrorKind`]) of `level` with `"%s"` and `message`.
    pub(crate) fn swoole_client_php_error(kind: c_int, level: c_int, message: *const c_char);
    /// `new Client(type, async)`; null, with errno in `error`, without a socket.
    pub(crate) fn swoole_client_create(kind: c_int, is_async: bool, error: *mut c_int) -> Cli;
    /// The client's `ClientState` (crates/clients).
    pub(crate) fn swoole_client_state(cli: Cli) -> *mut c_void;
    /// `php_swoole_convert_to_socket(fd)` into `zsocket`.
    pub(crate) fn swoole_client_export_socket(cli: Cli, zsocket: *mut zval) -> bool;
    /// Install the async callbacks in `events`, raised on `object`.
    pub(crate) fn swoole_client_async_listen(cli: Cli, object: *mut zend_object, events: u32);
    pub(crate) fn swoole_client_has_on_error(cli: Cli) -> bool;
    /// Run one [`ClientCall`]; a `bailout::trap` body.
    pub(crate) fn swoole_client_run(call: *mut c_void);
    /// The client's `SSLContext` owner; null while TLS is off.
    pub(crate) fn swoole_client_ssl_context(cli: Cli) -> *const c_void;
    /// `set_http_proxy` (`http`) or `set_socks5_proxy`.
    pub(crate) fn swoole_client_proxy(
        cli: Cli,
        http: bool,
        host: *const c_char,
        host_length: size_t,
        port: i64,
        user: *const c_char,
        user_length: size_t,
        password: *const c_char,
        password_length: size_t,
    );
}

/// The client's public record: `network::Client`'s only base, at its start.
/// # Safety
/// `cli` is a live `network::Client *`, and nothing else touches its record
/// while the borrow lasts.
pub(crate) unsafe fn client<'a>(cli: Cli) -> &'a mut ClientRecord {
    // SAFETY: per the caller.
    unsafe { &mut *cli.cast::<ClientRecord>() }
}

/// The client's socket record (`cli->socket`), or null.
/// # Safety
/// `cli` is a live client.
pub(crate) unsafe fn client_socket(cli: Cli) -> *mut SocketRecord {
    // SAFETY: per the caller.
    unsafe { client(cli) }.socket
}

/// The client's socket record.
/// # Safety
/// `cli` is a live client with a socket, which nothing else touches while the
/// borrow lasts.
pub(crate) unsafe fn client_socket_mut<'a>(cli: Cli) -> &'a mut SocketRecord {
    // SAFETY: per the caller.
    let socket = unsafe { client_socket(cli) };
    // SAFETY: as above.
    unsafe { &mut *socket }
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
        Ok(length) => swoole_rs_zend::string::copy(&buffer[..length]),
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

/// A `network::Client *`.
pub(crate) type Cli = *mut c_void;

/// crates/clients' per-client state.
pub(crate) type ClientState = swoole_rs_clients::client::Client;

/// `SWOOLE_CLIENT_ERROR*`: which reporter `swoole_client_php_error` uses.
#[derive(Clone, Copy)]
#[repr(i32)]
pub(crate) enum ErrorKind {
    Error = 0,
    Fatal = 1,
    Sys = 2,
    Core = 3,
}

/// `struct swoole_client_call`: one operation for `swoole_client_run`.
#[repr(C)]
pub(crate) struct ClientCall {
    pub(crate) cli: *mut c_void,
    pub(crate) op: Op,
    pub(crate) data: *const c_char,
    pub(crate) length: size_t,
    pub(crate) buffer: *mut c_char,
    pub(crate) a: i64,
    pub(crate) b: i64,
    pub(crate) timeout: f64,
    pub(crate) settings: *const zval,
    pub(crate) result: i64,
}

/// `SWOOLE_CLIENT_CONNECT` ...: what a [`ClientCall`] does.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum Op {
    Connect = 0,
    Send = 1,
    Sendto = 2,
    Sendfile = 3,
    Recv = 4,
    PackageLength = 5,
    Shutdown = 6,
    Close = 7,
    Sleep = 8,
    Wakeup = 9,
    EnableSsl = 11,
    SslHandshake = 12,
    SslVerify = 13,
    SslWait = 14,
    Destroy = 15,
    Callable = 17,
    CallableFree = 18,
    Invoke = 19,
    ConvertFd = 20,
}

impl ClientCall {
    pub(crate) fn new(cli: *mut c_void, op: Op) -> Self {
        Self {
            cli,
            op,
            data: std::ptr::null(),
            length: 0,
            buffer: std::ptr::null_mut(),
            a: 0,
            b: 0,
            timeout: 0.0,
            settings: std::ptr::null(),
            result: 0,
        }
    }

    /// Run the call inside `zend_try`.
    /// # Errors
    /// PHP code it ran bailed out.
    pub(crate) fn run(&mut self) -> Result<i64, swoole_rs_zend::bailout::Bailout> {
        // SAFETY: a C++ body that reads and writes only this call.
        unsafe {
            swoole_rs_zend::bailout::trap(swoole_client_run, std::ptr::from_mut(self).cast())
        }?;
        Ok(self.result)
    }

    /// Run a call that cannot reach PHP code (a sync client's send or plain
    /// receive), without the `zend_try` frame.
    pub(crate) fn run_plain(&mut self) -> i64 {
        // SAFETY: as in `run`; the caller rules out callbacks and errors.
        unsafe { swoole_client_run(std::ptr::from_mut(self).cast()) };
        self.result
    }
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

/// `swoole_set_last_error(code)`.
pub(crate) unsafe fn swoole_rs_set_last_error(code: c_int) {
    swoole_rs_corebase::log::set_last_error(code);
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
