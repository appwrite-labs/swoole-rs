//! What only C++ can do, defined in the patched `ext-src/swoole_socket_coro.cc`
//! (declared in `native/clients/swoole_rs_phpclients.h`): the coroutine
//! `Socket` class (`SocketImpl`) is still C++, so each operation a method needs
//! is one call over its opaque pointer, plus Swoole's PHP error reporting.
use libc::{c_char, c_int, c_void, iovec, size_t, ssize_t};
use swoole_rs_phpruntime::co;
use swoole_rs_zend::sys::{zend_object, zend_string, zval};

/// A `SocketImpl *`.
pub(crate) type Socket = *mut libc::c_void;

/// `swoole_socket_coro_callable`.
#[repr(C)]
pub(crate) struct LengthCallable {
    pub(crate) socket: Socket,
    pub(crate) value: *mut zval,
}

unsafe extern "C" {
    /// `php_swoole_error(level, "%s", message)`, or with `fatal`
    /// `php_swoole_fatal_error`.
    pub(crate) fn swoole_rs_php_error(level: c_int, fatal: bool, message: *const c_char);
    /// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
    pub(crate) fn swoole_socket_coro_unconstructed();
    /// `socket_coro_init`'s socket half: zero copy and the zend string
    /// allocator, then its fd, domain, type and protocol into `fields`.
    pub(crate) fn swoole_socket_coro_adopt(socket: Socket, fields: *mut i64);
    pub(crate) fn swoole_socket_coro_new_family(
        domain: c_int,
        kind: c_int,
        protocol: c_int,
        result: *mut Socket,
    ) -> u8;
    /// Individual native C++ constructors; each contains PHP bailouts before returning.
    pub(crate) fn swoole_socket_coro_new_type(kind: c_int, result: *mut Socket) -> u8;
    pub(crate) fn swoole_socket_coro_new_fd(fd: c_int, kind: c_int, result: *mut Socket) -> u8;
    pub(crate) fn swoole_socket_coro_new_fd_family(
        fd: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
        result: *mut Socket,
    ) -> u8;
    pub(crate) fn swoole_socket_coro_fd(socket: Socket) -> c_int;
    /// Core lifetime leaves contain any PHP callback bailout below Rust.
    pub(crate) fn swoole_socket_coro_destroy(socket: Socket) -> u8;
    pub(crate) fn swoole_socket_coro_move_fd(socket: Socket);
    pub(crate) fn swoole_socket_coro_close_native(socket: Socket) -> u8;
    pub(crate) fn swoole_socket_coro_callable_free(callable: *mut c_void) -> u8;
    pub(crate) fn swoole_socket_coro_is_closed(socket: Socket) -> bool;
    /// `errCode`, with `errMsg` in `message`.
    pub(crate) fn swoole_socket_coro_error(socket: Socket, message: *mut *const c_char) -> c_int;
    /// `set_err(code, message)`.
    pub(crate) fn swoole_socket_coro_set_error(socket: Socket, code: c_int, message: *const c_char);
    pub(crate) fn swoole_socket_coro_bind(
        socket: Socket,
        address: *const c_char,
        length: size_t,
        port: i64,
    ) -> bool;
    pub(crate) fn swoole_socket_coro_listen(socket: Socket, backlog: i64) -> bool;
    /// `accept(timeout)`; the connection copies the listener's length callable.
    pub(crate) fn swoole_socket_coro_accept(socket: Socket, timeout: f64) -> Socket;
    pub(crate) fn swoole_socket_coro_port_required(socket: Socket) -> bool;
    /// `connect(host, port)` under a connect timeout of `timeout`.
    pub(crate) fn swoole_socket_coro_connect(
        socket: Socket,
        host: *const c_char,
        length: size_t,
        port: i64,
        timeout: f64,
    ) -> bool;
    pub(crate) fn swoole_socket_coro_check_liveness(socket: Socket) -> bool;
    pub(crate) fn swoole_socket_coro_bound_cid(socket: Socket, event: i64) -> i64;
    /// One `Transfer` of `length` bytes at `buffer` under `timeout`.
    pub(crate) fn swoole_socket_coro_transfer(
        socket: Socket,
        op: c_int,
        buffer: *mut c_char,
        length: size_t,
        timeout: f64,
    ) -> ssize_t;
    /// `recv_packet(timeout)`; with `clear_empty`, an empty packet clears the
    /// read buffer.
    pub(crate) fn swoole_socket_coro_recv_packet(
        socket: Socket,
        timeout: f64,
        clear_empty: bool,
    ) -> ssize_t;
    /// `pop_packet()`: the `ZSTR_VAL` of the packet's string, or null after
    /// `set_err(ENOMEM)`.
    pub(crate) fn swoole_socket_coro_pop_packet(socket: Socket) -> *mut c_char;
    /// `readv`/`readv_all`/`writev`/`writev_all` (`op` 0..=3) over `count`
    /// vectors under `timeout`; `index` and `offset` report where it stopped.
    pub(crate) fn swoole_socket_coro_vector(
        socket: Socket,
        op: c_int,
        vectors: *mut iovec,
        count: c_int,
        timeout: f64,
        index: *mut c_int,
        offset: *mut size_t,
    ) -> ssize_t;
    pub(crate) fn swoole_socket_coro_sendfile(
        socket: Socket,
        file: *const c_char,
        offset: i64,
        length: i64,
    ) -> bool;
    /// `recvfrom(buffer, length)` under a read timeout of `timeout`.
    pub(crate) fn swoole_socket_coro_recvfrom(
        socket: Socket,
        buffer: *mut c_char,
        length: size_t,
        timeout: f64,
    ) -> ssize_t;
    /// `get_addr()`, with `get_port()` in `port`.
    pub(crate) fn swoole_socket_coro_address(socket: Socket, port: *mut c_int) -> *const c_char;
    pub(crate) fn swoole_socket_coro_sendto(
        socket: Socket,
        address: *const c_char,
        length: size_t,
        port: i64,
        data: *const c_char,
        data_length: size_t,
    ) -> ssize_t;
    pub(crate) fn swoole_socket_coro_shutdown(socket: Socket, how: i64) -> bool;
    pub(crate) fn swoole_socket_coro_getsockname(socket: Socket) -> bool;
    /// `getpeername(&sa)`: the address, NUL-terminated, into `address`.
    pub(crate) fn swoole_socket_coro_getpeername(
        socket: Socket,
        address: *mut c_char,
        size: size_t,
        port: *mut c_int,
    ) -> bool;
    pub(crate) fn swoole_socket_coro_cancel(socket: Socket, event: c_int) -> bool;
    pub(crate) fn swoole_socket_coro_ssl_handshake(socket: Socket) -> bool;
    /// `open_length_check` (0), `open_eof_check` (1) or `http2` (2) = `value`.
    pub(crate) fn swoole_socket_coro_set_flag(socket: Socket, flag: c_int, value: bool);
    /// `&protocol`, a `swoole::Protocol`.
    pub(crate) fn swoole_socket_coro_protocol(socket: Socket) -> *mut c_void;
    /// `mqtt::set_protocol` (0) or `Protocol::default_length_func` (1).
    pub(crate) fn swoole_socket_coro_protocol_preset(socket: Socket, kind: c_int);
    /// `package_length_func` from a [`LengthCallable`]; run inside `zend_try`.
    pub(crate) fn swoole_socket_coro_length_callable(call: *mut c_void);
    /// `set_option(level, name, value)`, warning (a log line) on failure.
    pub(crate) fn swoole_socket_coro_set_option(
        socket: Socket,
        level: c_int,
        name: c_int,
        value: c_int,
    ) -> bool;
    pub(crate) fn swoole_socket_coro_set_timeout(socket: Socket, timeout: f64, kind: c_int);
    pub(crate) fn swoole_socket_coro_get_timeout(socket: Socket, kind: c_int) -> f64;
    /// `set_http_proxy` or `set_socks5_proxy`.
    pub(crate) fn swoole_socket_coro_proxy(
        socket: Socket,
        http: bool,
        host: *const c_char,
        host_length: size_t,
        port: i64,
        user: *const c_char,
        user_length: size_t,
        password: *const c_char,
        password_length: size_t,
    );
    /// `enable_ssl_encrypt()`: false when TLS was already on.
    pub(crate) fn swoole_socket_coro_enable_ssl(socket: Socket) -> bool;
    /// The socket's `SSLContext` owner; null while TLS is off.
    pub(crate) fn swoole_socket_coro_ssl_context(socket: Socket) -> *const c_void;
    /// Whether the build has an optional SSL option (host name, grease).
    pub(crate) fn swoole_socket_coro_ssl_supports(option: c_int) -> bool;
    /// The socket's domain, and its type into `kind`.
    pub(crate) fn swoole_socket_coro_family(socket: Socket, kind: *mut c_int) -> c_int;
    /// `socket->errCode = code`, nothing else.
    pub(crate) fn swoole_socket_coro_set_err_code(socket: Socket, code: c_int);
    /// `TCP_INFO` into `array`: 1, 0 when `name` is not it, -1 when it failed.
    pub(crate) fn swoole_socket_coro_tcp_info(
        socket: Socket,
        name: c_int,
        array: *mut zval,
    ) -> c_int;
}

unsafe extern "C" {
    /// `php_swoole_error`, `_fatal_error`, `_sys_error` or `_core_error`
    /// (`kind`, [`ErrorKind`]) of `level` with `"%s"` and `message`.
    pub(crate) fn swoole_client_php_error(kind: c_int, level: c_int, message: *const c_char);
    /// `swoole_error_log(SW_LOG_WARNING, SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, ...)`,
    /// named as upstream's `recv`.
    pub(crate) fn swoole_client_package_too_big(length: c_int);
    /// `new Client(type, async)`; null, with errno in `error`, without a socket.
    pub(crate) fn swoole_client_create(kind: c_int, is_async: bool, error: *mut c_int) -> Cli;
    pub(crate) fn swoole_client_fd(cli: Cli) -> c_int;
    /// The client's `ClientState` (crates/clients).
    pub(crate) fn swoole_client_state(cli: Cli) -> *mut c_void;
    pub(crate) fn swoole_client_has_socket(cli: Cli) -> bool;
    pub(crate) fn swoole_client_socket_type(cli: Cli) -> c_int;
    pub(crate) fn swoole_client_has_ssl(cli: Cli) -> bool;
    pub(crate) fn swoole_client_check_liveness(cli: Cli) -> bool;
    /// `getsockopt(SO_ERROR)`: whether it succeeded.
    pub(crate) fn swoole_client_socket_error(cli: Cli, error: *mut c_int) -> bool;
    /// `socket->catch_error(error)`.
    pub(crate) fn swoole_client_catch_error(cli: Cli, error: c_int) -> c_int;
    pub(crate) fn swoole_client_framing(cli: Cli, framing: *mut Framing);
    /// getsockname: the address, the port in `port`; null on failure.
    pub(crate) fn swoole_client_sockname(cli: Cli, port: *mut c_int) -> *const c_char;
    pub(crate) fn swoole_client_peername(
        cli: Cli,
        address: *mut c_char,
        size: size_t,
        port: *mut c_int,
    ) -> bool;
    pub(crate) fn swoole_client_peer_cert(cli: Cli) -> *mut zend_string;
    /// `php_swoole_convert_to_socket(fd)` into `zsocket`.
    pub(crate) fn swoole_client_export_socket(cli: Cli, zsocket: *mut zval) -> bool;
    /// Merge `settings` into the `setting` property.
    pub(crate) fn swoole_client_merge_setting(object: *mut zend_object, settings: *const zval);
    /// Install the async callbacks in `events`, raised on `object`.
    pub(crate) fn swoole_client_async_listen(cli: Cli, object: *mut zend_object, events: u32);
    pub(crate) fn swoole_client_has_on_error(cli: Cli) -> bool;
    /// `socket->ssl_state = SW_SSL_STATE_WAIT_STREAM`.
    pub(crate) fn swoole_client_ssl_wait_stream(cli: Cli);
    /// `swoole_get_last_error()`.
    pub(crate) fn swoole_client_last_error() -> c_int;
    /// Run one [`ClientCall`]; a `bailout::trap` body.
    pub(crate) fn swoole_client_run(call: *mut c_void);
    /// `&cli->protocol`, whose head crates/framing mirrors.
    pub(crate) fn swoole_client_protocol(cli: Cli) -> *mut c_void;
    /// Write one `SWOOLE_CLIENT_OPT_*` into the client (`crate::settings`).
    pub(crate) fn swoole_client_option(
        cli: Cli,
        option: c_int,
        value: i64,
        text: *const c_char,
        length: size_t,
    ) -> i64;
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

/// A `network::Client *`.
pub(crate) type Cli = *mut c_void;

/// crates/clients' per-client state.
pub(crate) type ClientState = swoole_rs_clients::client::Client;

/// `struct swoole_client_framing`.
#[repr(C)]
#[derive(Default)]
pub(crate) struct Framing {
    pub(crate) eof_check: bool,
    pub(crate) length_check: bool,
    pub(crate) eof_length: u8,
    pub(crate) eof: [u8; 8],
    pub(crate) max_length: u32,
    pub(crate) length_offset: u16,
    pub(crate) length_size: u8,
}

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
    Destruct = 16,
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

/// The peer certificate as a new string, or null: `ssl_get_peer_certificate`
/// into a buffer of `sw_tg_buffer()`'s size.
pub(crate) unsafe fn swoole_client_coro_peer_cert(socket: Socket) -> *mut zend_string {
    let mut buffer = vec![0_u8; swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE as usize];
    // SAFETY: per the caller.
    let record = unsafe { swoole_client_coro_socket_record(socket) };
    // SAFETY: a live record with TLS and a writable buffer.
    let length = unsafe {
        swoole_rs_tls::upstream::socket_ssl_get_peer_certificate_pem(
            record,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    match usize::try_from(length) {
        Ok(length) => swoole_rs_zend::string::copy(&buffer[..length]),
        Err(_) => std::ptr::null_mut(),
    }
}

pub(crate) unsafe fn swoole_client_coro_verify_peer(
    socket: Socket,
    allow_self_signed: bool,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::ssl_verify(socket, allow_self_signed) }
}
