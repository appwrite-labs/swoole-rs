//! `network::Client` (src/network/client.cc) in Rust: its state and every
//! operation over it, from construction to close. The C++ `Client` is a facade
//! that holds this behind `state_` and the members of C++ types (the callbacks,
//! the proxies, the TLS context, the protocol, the input buffer and the
//! addresses); Rust reaches those through [`Native`], one static table the C++
//! side defines once. The socket is the reactor's record, read and written here
//! directly.
//!
//! A user callback can destroy the client (and this state with it), so no
//! reference to the state lives across one: operations work on a raw [`Handle`]
//! and borrow the state only between effects. A callback that throws a C++
//! exception returns false; the operation stops there and C++ rethrows.
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int, c_void};
use std::ffi::{CStr, CString};
use swoole_rs_reactor::{
    SocketRecord, swr_v1_socket_buffer_size, swr_v1_socket_catch_error, swr_v1_socket_clean,
    swr_v1_socket_defaults, swr_v1_socket_domain_and_type, swr_v1_socket_set_timeout,
};

/// The flags `swr_v1_clients_flag` reads (`SWR_CLIENTS_*`).
pub const ACTIVE: u32 = 0;
pub const CLOSED: u32 = 1;
pub const ASYNC: u32 = 2;
pub const SHUTDOWN_READ: u32 = 3;
pub const SHUTDOWN_WRITE: u32 = 4;
pub const ASYNC_CONNECT: u32 = 5;
pub const WAIT_DNS: u32 = 6;
pub const ONERROR_CALLED: u32 = 7;
pub const OPEN_SSL: u32 = 8;
const DNS_COMPLETED: u32 = 9;
const HOST_PRESET: u32 = 10;
const HIGH_WATERMARK: u32 = 11;
/// `enable_dtls()` ran: send and receive as a stream client does.
const DTLS: u32 = 12;
const STREAM: u32 = 13;

/// The callbacks, for [`Native::has`] and [`Native::call`] (`SWR_CLIENTS_ON_*`).
pub const ON_CONNECT: c_int = 0;
pub const ON_ERROR: c_int = 1;
pub const ON_RECEIVE: c_int = 2;
pub const ON_CLOSE: c_int = 3;
pub const ON_BUFFER_FULL: c_int = 4;
pub const ON_BUFFER_EMPTY: c_int = 5;

/// [`Native::warn`]'s warnings (`SWR_CLIENTS_WARN_*`).
pub const WARN_CALLBACKS: c_int = 1;
pub const WARN_ON_RECEIVE: c_int = 2;
pub const WARN_ON_ERROR: c_int = 3;
pub const WARN_BIND: c_int = 5;
pub const WARN_READ: c_int = 6;
pub const WARN_GETSOCKOPT: c_int = 7;

/// [`Native::connect`]'s modes (`SWR_CLIENTS_CONNECT_*`): `connect_async` (returns
/// `SW_READY`, `SW_WAIT` or `SW_ERROR`), `connect_sync`, and `connect(2)`.
pub const CONNECT_ASYNC: c_int = 0;
pub const CONNECT_SYNC: c_int = 1;
pub const CONNECT_RAW: c_int = 2;

/// [`Native::io`]'s operations (`SWR_CLIENTS_IO_*`).
pub const IO_RECV: c_int = 0;
pub const IO_RECV_SYNC: c_int = 1;
pub const IO_SEND_SYNC: c_int = 2;
pub const IO_SENDTO: c_int = 3;
pub const IO_RECVFROM: c_int = 4;
pub const IO_RECVFROM_SYNC: c_int = 5;

/// [`Native::event`]'s operations (`SWR_CLIENTS_EVENT_*`).
pub const EVENT_ADD: c_int = 0;
pub const EVENT_SET: c_int = 1;
pub const EVENT_DEL: c_int = 2;
pub const EVENT_ADD_OR_UPDATE: c_int = 3;

/// [`Native::ssl`]'s operations (`SWR_CLIENTS_SSL_*`).
pub const SSL_HAS_CONTEXT: c_int = 1;
/// Creates the context unless it is ready (with `http2`): 0 or -1.
pub const SSL_PREPARE: c_int = 2;
/// `ssl_create` and the SNI host name: 0 or -1.
pub const SSL_SESSION: c_int = 3;
pub const SSL_CONNECT: c_int = 4;
/// `verify_peer` (1) and `allow_self_signed` (2).
pub const SSL_VERIFY_PEER: c_int = 5;
/// `ssl_verify(arg)` and the host check: 0 or -1.
pub const SSL_VERIFY: c_int = 6;
pub const SSL_CLOSE: c_int = 7;
/// The context's protocols become DTLS: 0, or -1 (warned) without DTLS support.
pub const SSL_DTLS: c_int = 8;

/// The proxies, for [`Native::proxy_state`] and the rest.
pub const PROXY_SOCKS5: c_int = 0;
pub const PROXY_HTTP: c_int = 1;

/// [`Native::buffer`]'s operations on the input buffer (`SWR_CLIENTS_BUFFER_*`).
pub const BUFFER_INIT: c_int = 0;
pub const BUFFER_VIEW: c_int = 1;
pub const BUFFER_GROW: c_int = 2;
pub const BUFFER_CLEAR: c_int = 3;
pub const BUFFER_FREE: c_int = 4;
/// The thread's shared buffer (`sw_tg_buffer()`), for `BUFFER_VIEW`.
pub const BUFFER_SHARED: c_int = 5;

/// Swoole's constants this file uses; client.cc asserts each.
const OK: c_int = 0;
const ERR: c_int = -1;
const SW_WAIT: c_int = 2;
const SW_CLOSE: c_int = 3;
const SW_ERROR: c_int = 4;
const SW_READY: c_int = 5;
const SOCK_UNIX_DGRAM: c_int = 6;
const FD_STREAM_CLIENT: c_int = 17;
const FD_DGRAM_CLIENT: c_int = 18;
const SOCK_NONBLOCK: c_int = 1 << 2;
const SOCK_CLOEXEC: c_int = 1 << 3;
const SEND_BUFFER_SIZE: u32 = 65536;
const SSL_BUFFER_SIZE: u32 = 16384;
const BUFFER_SIZE_UDP: usize = 65536;
const EVENT_READ: c_int = 1 << 9;
const EVENT_WRITE: c_int = 1 << 10;
const TIMEOUT_ALL: c_int = 15;
const SSL_STATE_READY: u32 = 1;
const SSL_STATE_WAIT_STREAM: u32 = 2;
const SOCKS5_WAIT: c_int = 0;
const SOCKS5_HANDSHAKE: c_int = 1;
const SOCKS5_READY: c_int = 4;
const HTTP_PROXY_WAIT: c_int = 0;
const HTTP_PROXY_READY: c_int = 2;
const ERROR_DNSLOOKUP_RESOLVE_FAILED: c_int = 711;
const ERROR_BAD_HOST_ADDR: c_int = 722;
const ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const ERROR_SSL_HANDSHAKE_FAILED: c_int = 1018;
const ERROR_SSL_CREATE_SESSION_FAILED: c_int = 1020;
const ERROR_SOCKET_POLL_TIMEOUT: c_int = 6002;
const ERROR_SOCKS5_HANDSHAKE_FAILED: c_int = 7005;
const ERROR_HTTP_PROXY_HANDSHAKE_ERROR: c_int = 7101;

/// A view of a `String`: its bytes, length and size.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferView {
    pub str: *mut c_char,
    pub length: usize,
    pub size: usize,
}

/// What the C++ `Client` does for Rust (`swr_v1_clients_native`). `cli` is the
/// C++ `Client *`.
#[repr(C)]
pub struct Native {
    /// `make_socket(type, fd_type, flags)` into `cli->socket`: the record or null.
    pub make_socket: unsafe extern "C" fn(
        cli: *mut c_void,
        kind: c_int,
        fd_type: c_int,
        flags: c_int,
    ) -> *mut SocketRecord,
    /// Frees `cli->socket`: `socket->free()` when `deferred`, else `delete socket`.
    pub free_socket: unsafe extern "C" fn(cli: *mut c_void, deferred: bool),
    /// Whether callback `which` is set.
    pub has: unsafe extern "C" fn(cli: *mut c_void, which: c_int) -> bool,
    /// Runs callback `which` (`data` for `ON_RECEIVE`); false when it threw. The
    /// client may be gone afterwards.
    pub call: unsafe extern "C" fn(
        cli: *mut c_void,
        which: c_int,
        data: *const c_char,
        length: usize,
    ) -> bool,
    pub last_error: unsafe extern "C" fn() -> c_int,
    pub set_last_error: unsafe extern "C" fn(code: c_int),
    /// Logs warning `which`; the system ones report the current `errno`.
    pub warn: unsafe extern "C" fn(cli: *mut c_void, which: c_int),
    /// `socket->set_kernel_timeout(timeout)`.
    pub kernel_timeout: unsafe extern "C" fn(cli: *mut c_void, timeout: f64),
    /// `server_addr.assign(socket_type, host, port, resolve)`.
    pub assign: unsafe extern "C" fn(
        cli: *mut c_void,
        host: *const c_char,
        port: c_int,
        resolve: bool,
    ) -> bool,
    /// Connects the socket to `server_addr` (`CONNECT_*`); errno is the call's.
    pub connect: unsafe extern "C" fn(cli: *mut c_void, mode: c_int) -> c_int,
    /// The socket's IO (`IO_*`); `IO_SENDTO` sends to `server_addr` and the
    /// `IO_RECVFROM*` read into `remote_addr`.
    pub io: unsafe extern "C" fn(
        cli: *mut c_void,
        op: c_int,
        data: *mut c_char,
        length: usize,
        flags: c_int,
    ) -> isize,
    /// `sendfile_async` (async) or `sendfile_sync`: 0, or -1 with the last error set to errno.
    pub sendfile: unsafe extern "C" fn(
        cli: *mut c_void,
        file: *const c_char,
        offset: i64,
        length: usize,
        is_async: bool,
    ) -> c_int,
    /// The reactor's `swoole_event_*` (`EVENT_*`) on the socket.
    pub event: unsafe extern "C" fn(cli: *mut c_void, op: c_int, events: c_int) -> c_int,
    /// `swoole_event_write`.
    pub event_write:
        unsafe extern "C" fn(cli: *mut c_void, data: *const c_char, length: usize) -> c_int,
    pub out_buffer_length: unsafe extern "C" fn(cli: *mut c_void) -> u32,
    /// `Reactor::_writable_callback(reactor, event)`.
    pub writable: unsafe extern "C" fn(reactor: *mut c_void, event: *mut c_void) -> c_int,
    /// `get_option(SOL_SOCKET, SO_ERROR, error)`.
    pub socket_error: unsafe extern "C" fn(cli: *mut c_void, error: *mut c_int) -> c_int,
    /// `swoole_timer_add(timeout, false, Client_onTimeout, cli)`.
    pub timer_add: unsafe extern "C" fn(cli: *mut c_void, timeout: f64) -> *mut c_void,
    pub timer_del: unsafe extern "C" fn(timer: *mut c_void),
    /// Dispatches a `GethostbynameRequest` for `host` in `domain` to the async
    /// pool, completing in `swr_v1_clients_on_resolved`.
    pub dns_lookup:
        unsafe extern "C" fn(cli: *mut c_void, host: *const c_char, domain: c_int) -> c_int,
    /// The TLS context and session (`SSL_*`).
    pub ssl: unsafe extern "C" fn(cli: *mut c_void, op: c_int, arg: c_int) -> c_int,
    /// The proxy's state, or -1 when there is none.
    pub proxy_state: unsafe extern "C" fn(cli: *mut c_void, proxy: c_int) -> c_int,
    pub set_proxy_state: unsafe extern "C" fn(cli: *mut c_void, proxy: c_int, state: c_int),
    /// When the proxy is set: makes `host` and `port` its target and returns its
    /// own host (owned by the proxy) and port.
    pub proxy_route: unsafe extern "C" fn(
        cli: *mut c_void,
        proxy: c_int,
        host: *const c_char,
        port: c_int,
        out_port: *mut c_int,
    ) -> *const c_char,
    /// `pack_negotiate_request()`, its bytes in `*data`.
    pub socks5_negotiate: unsafe extern "C" fn(cli: *mut c_void, data: *mut *const c_char) -> isize,
    /// `socks5_proxy->handshake`, sending through `Client::send`.
    pub socks5_handshake:
        unsafe extern "C" fn(cli: *mut c_void, data: *const c_char, length: usize) -> bool,
    /// `http_proxy->pack` into the shared buffer, its bytes in `*data`.
    pub http_pack: unsafe extern "C" fn(cli: *mut c_void, data: *mut *const c_char) -> usize,
    /// `HttpProxy::handshake` of the shared buffer (`shared`, its length set to
    /// `length`) or of the input buffer (its length grown by `length`).
    pub http_handshake: unsafe extern "C" fn(cli: *mut c_void, shared: bool, length: usize) -> bool,
    /// The input buffer (`BUFFER_*`).
    pub buffer:
        unsafe extern "C" fn(cli: *mut c_void, op: c_int, length: usize, view: *mut BufferView),
    /// With `open_eof_check` or `open_length_check`, receives through the protocol
    /// into `*received` (0 when a callback released the client) and returns true.
    pub recv_framed: unsafe extern "C" fn(cli: *mut c_void, received: *mut isize) -> bool,
}

/// One client's state.
pub struct Client {
    cli: *mut c_void,
    native: &'static Native,
    socket: *mut SocketRecord,
    flags: Cell<u32>,
    domain: c_int,
    sock_flags: Cell<c_int>,
    server_host: RefCell<CString>,
    server_port: Cell<c_int>,
    timer: Cell<*mut c_void>,
    high_watermark: Cell<u32>,
    low_watermark: Cell<u32>,
}

impl Client {
    fn flag(&self, name: u32) -> bool {
        self.flags.get() & (1 << name) != 0
    }

    fn set(&self, name: u32, value: bool) {
        let bit = 1 << name;
        self.flags.set(if value {
            self.flags.get() | bit
        } else {
            self.flags.get() & !bit
        });
    }
}

/// A client under an operation: a raw pointer, since a callback may free it.
#[derive(Clone, Copy)]
struct Handle(*mut Client);

impl Handle {
    fn get(self) -> &'static Client {
        // SAFETY: the client is live between callbacks; callers drop the reference
        // before any effect that may release it.
        unsafe { &*self.0 }
    }

    fn cli(self) -> *mut c_void {
        self.get().cli
    }

    fn native(self) -> &'static Native {
        self.get().native
    }

    fn flag(self, name: u32) -> bool {
        self.get().flag(name)
    }

    fn set(self, name: u32, value: bool) {
        self.get().set(name, value);
    }

    fn socket(self) -> &'static mut SocketRecord {
        // SAFETY: a constructed client's socket record, live until the client is
        // destroyed (an async one's free is deferred past its callbacks).
        unsafe { &mut *self.get().socket }
    }

    fn has(self, which: c_int) -> bool {
        // SAFETY: a native read of the live client.
        unsafe { (self.native().has)(self.cli(), which) }
    }

    /// Runs a user callback: false when it threw. The client may be gone after.
    fn call(self, which: c_int, data: *const c_char, length: usize) -> bool {
        let (native, cli) = (self.native(), self.cli());
        // SAFETY: the live client; nothing of it is borrowed across the call.
        unsafe { (native.call)(cli, which, data, length) }
    }

    fn last_error(self) -> c_int {
        // SAFETY: reads the thread's last error.
        unsafe { (self.native().last_error)() }
    }

    fn set_last_error(self, code: c_int) {
        // SAFETY: sets the thread's last error.
        unsafe { (self.native().set_last_error)(code) };
    }

    fn warn(self, which: c_int) {
        // SAFETY: a log line about the live client.
        unsafe { (self.native().warn)(self.cli(), which) };
    }

    fn event(self, op: c_int, events: c_int) -> c_int {
        // SAFETY: the reactor and the live client's socket.
        unsafe { (self.native().event)(self.cli(), op, events) }
    }

    fn io(self, op: c_int, data: *mut c_char, length: usize, flags: c_int) -> isize {
        // SAFETY: `length` bytes at `data` are the caller's buffer.
        unsafe { (self.native().io)(self.cli(), op, data, length, flags) }
    }

    fn ssl(self, op: c_int, arg: c_int) -> c_int {
        // SAFETY: the live client's TLS context and socket.
        unsafe { (self.native().ssl)(self.cli(), op, arg) }
    }

    fn proxy_state(self, proxy: c_int) -> c_int {
        // SAFETY: a read of the live client's proxy.
        unsafe { (self.native().proxy_state)(self.cli(), proxy) }
    }

    fn set_proxy_state(self, proxy: c_int, state: c_int) {
        // SAFETY: the live client's proxy.
        unsafe { (self.native().set_proxy_state)(self.cli(), proxy, state) };
    }

    fn buffer(self, op: c_int, length: usize) -> BufferView {
        let mut view = BufferView {
            str: core::ptr::null_mut(),
            length: 0,
            size: 0,
        };
        // SAFETY: the live client's buffer; `view` is writable.
        unsafe { (self.native().buffer)(self.cli(), op, length, &raw mut view) };
        view
    }

    /// The SOCKS5 negotiation request: its bytes and length.
    fn socks5_negotiate(self) -> (*const c_char, usize) {
        let mut data = core::ptr::null();
        // SAFETY: the live client's proxy; `data` is writable.
        let length = unsafe { (self.native().socks5_negotiate)(self.cli(), &raw mut data) };
        (data, length.cast_unsigned())
    }

    /// The SOCKS5 handshake over `length` received bytes at `data`.
    fn socks5_handshake(self, data: *const c_char, length: usize) -> bool {
        // SAFETY: `length` bytes at `data`, in a buffer of the live client's.
        unsafe { (self.native().socks5_handshake)(self.cli(), data, length) }
    }

    /// The HTTP proxy's CONNECT request: its bytes and length.
    fn http_pack(self) -> (*const c_char, usize) {
        let mut data = core::ptr::null();
        // SAFETY: the live client's proxy; `data` is writable.
        let length = unsafe { (self.native().http_pack)(self.cli(), &raw mut data) };
        (data, length)
    }

    /// The HTTP proxy's handshake over `length` received bytes.
    fn http_handshake(self, shared: bool, length: usize) -> bool {
        // SAFETY: the live client's proxy and buffers.
        unsafe { (self.native().http_handshake)(self.cli(), shared, length) }
    }

    fn set_timeout(self, timeout: f64) {
        // SAFETY: the record's timeouts, live and unborrowed elsewhere.
        unsafe { swr_v1_socket_set_timeout(&raw mut self.socket().timeouts, timeout, TIMEOUT_ALL) };
    }

    fn timer_clear(self) {
        let timer = self.get().timer.replace(core::ptr::null_mut());
        if !timer.is_null() {
            // SAFETY: a pending timer this client added.
            unsafe { (self.native().timer_del)(timer) };
        }
    }

    /// `execute_onConnect`.
    fn on_connect(self) -> bool {
        self.timer_clear();
        self.call(ON_CONNECT, core::ptr::null(), 0)
    }
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

fn is_stream(kind: c_int) -> bool {
    matches!(kind, 1 | 3 | 5)
}

fn is_dgram(kind: c_int) -> bool {
    matches!(kind, 2 | 4 | 6)
}

/// `Client::Client(type, async)`: the socket and the state. Null only when the
/// allocation failed; a client whose socket failed has a null `socket`.
///
/// # Safety
/// `cli` is the C++ client being constructed and `native` the static table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_new(
    cli: *mut c_void,
    native: &'static Native,
    kind: c_int,
    is_async: bool,
) -> *mut Client {
    let stream = is_stream(kind);
    let fd_type = if stream {
        FD_STREAM_CLIENT
    } else {
        FD_DGRAM_CLIENT
    };
    let flags = if is_async { SOCK_NONBLOCK } else { 0 } | SOCK_CLOEXEC;
    // SAFETY: the C++ client stores and returns its new socket.
    let socket = unsafe { (native.make_socket)(cli, kind, fd_type, flags) };
    // C++ warns of a failed socket() with its errno.
    let error = errno();
    let client = Box::new(Client {
        cli,
        native,
        socket,
        flags: Cell::new(0),
        domain: 0,
        sock_flags: Cell::new(0),
        server_host: RefCell::new(CString::default()),
        server_port: Cell::new(0),
        timer: Cell::new(core::ptr::null_mut()),
        high_watermark: Cell::new(0),
        low_watermark: Cell::new(0),
    });
    client.set(ASYNC, is_async);
    if socket.is_null() {
        set_errno(error);
        return Box::into_raw(client);
    }
    let mut client = client;
    // SAFETY: the new socket record, which nothing else holds yet.
    let record = unsafe { &mut *socket };
    record.object = cli;
    record.chunk_size = SEND_BUFFER_SIZE;
    client.set(STREAM, is_stream(record.socket_type));
    let mut kind_out = 0;
    // SAFETY: two writable ints.
    unsafe { swr_v1_socket_domain_and_type(kind, &raw mut client.domain, &raw mut kind_out) };
    Box::into_raw(client)
}

/// `Client::~Client()`, then the state's release.
///
/// # Safety
/// `client` is a live state from `swr_v1_clients_new`, not used afterwards; the
/// C++ client is live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_free(client: *mut Client) {
    let this = Handle(client);
    if !this.get().socket.is_null() {
        if !this.flag(CLOSED) {
            close(this);
        }
        this.buffer(BUFFER_FREE, 0);
        // A pending connect timer would fire on a freed client.
        this.timer_clear();
        let deferred = this.flag(ASYNC);
        if deferred {
            // A cleared object tells a close in progress that the client is gone.
            this.socket().object = core::ptr::null_mut();
        }
        // SAFETY: the live client's socket, released once.
        unsafe { (this.native().free_socket)(this.cli(), deferred) };
    }
    // SAFETY: a Box from swr_v1_clients_new, released once (see above).
    drop(unsafe { Box::from_raw(client) });
}

/// One of the flags above; false for any other.
///
/// # Safety
/// `client` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_flag(client: *const Client, name: u32) -> bool {
    Handle(client.cast_mut()).flag(name)
}

/// # Safety
/// `client` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_set_flag(client: *mut Client, name: u32, value: bool) {
    Handle(client).set(name, value);
}

/// `buffer_high_watermark` (`high`) or `buffer_low_watermark` = `value`.
///
/// # Safety
/// `client` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_set_watermark(client: *mut Client, high: bool, value: u32) {
    let client = Handle(client).get();
    if high {
        &client.high_watermark
    } else {
        &client.low_watermark
    }
    .set(value);
}

/// `Client::close()`: -1 without a socket or when already closed, 0 when `onClose`
/// released the client, else `close(2)`'s result.
fn close(this: Handle) -> c_int {
    if this.get().socket.is_null() || this.flag(CLOSED) {
        return ERR;
    }
    this.set(CLOSED, true);
    let saved: *mut SocketRecord = this.socket();
    if this.flag(OPEN_SSL) && this.ssl(SSL_HAS_CONTEXT, 0) != 0 && !this.socket().ssl.is_null() {
        this.ssl(SSL_CLOSE, 0);
    }
    if this.socket().socket_type == SOCK_UNIX_DGRAM {
        // SAFETY: a Unix datagram socket's address is the union's Unix member.
        let path = unsafe { this.socket().info.addr.un.sun_path.as_ptr() };
        // SAFETY: the NUL-terminated path it bound.
        unsafe { libc::unlink(path) };
    }
    if this.flag(ASYNC) {
        if this.socket().removed == 0 {
            this.event(EVENT_DEL, 0);
        }
        this.timer_clear();
        if this.flag(ACTIVE) {
            this.set(ACTIVE, false);
            // onClose may destroy the client: only the saved socket is read after it.
            if !this.call(ON_CLOSE, core::ptr::null(), 0) {
                return ERR;
            }
        }
    } else {
        this.set(ACTIVE, false);
    }
    // SAFETY: the saved record, whose free is deferred past this call.
    let saved = unsafe { &mut *saved };
    if saved.object.is_null() {
        return OK;
    }
    let fd = core::mem::replace(&mut saved.fd, -1);
    // SAFETY: closes the client's descriptor once; -1 marks it closed.
    unsafe { libc::close(fd) }
}

/// # Safety
/// `client` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_close(client: *mut Client) -> c_int {
    close(Handle(client))
}

/// `Client::shutdown(how)`. Upstream's `SHUT_RDWR` records only a read shutdown.
///
/// # Safety
/// `client` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_shutdown(client: *mut Client, how: c_int) -> c_int {
    let this = Handle(client);
    if this.get().socket.is_null() || this.flag(CLOSED) {
        return ERR;
    }
    let fd = this.socket().fd;
    // SAFETY: shutdown(2) on the client's descriptor.
    let call = |how| unsafe { libc::shutdown(fd, how) };
    match how {
        libc::SHUT_RD if this.flag(SHUTDOWN_READ) || call(how) != 0 => ERR,
        libc::SHUT_WR if this.flag(SHUTDOWN_WRITE) || call(how) < 0 => ERR,
        libc::SHUT_RDWR if call(how) < 0 => ERR,
        libc::SHUT_RD | libc::SHUT_RDWR => {
            this.set(SHUTDOWN_READ, true);
            OK
        }
        libc::SHUT_WR => {
            this.set(SHUTDOWN_WRITE, true);
            OK
        }
        _ => {
            this.set_last_error(libc::EINVAL);
            ERR
        }
    }
}

/// `Client::sleep()`: stop reading (keeping write interest).
///
/// # Safety
/// `client` is live with a socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_sleep(client: *mut Client) -> c_int {
    let this = Handle(client);
    if this.socket().events & EVENT_WRITE != 0 {
        this.event(EVENT_SET, EVENT_WRITE)
    } else {
        this.event(EVENT_DEL, 0)
    }
}

/// `Client::wakeup()`: read again.
///
/// # Safety
/// `client` is live with a socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_wakeup(client: *mut Client) -> c_int {
    let this = Handle(client);
    if this.socket().events & EVENT_WRITE != 0 {
        this.event(EVENT_SET, EVENT_READ | EVENT_WRITE)
    } else {
        this.event(EVENT_ADD, EVENT_READ)
    }
}

/// `Client::enable_ssl_encrypt()` once C++ made the TLS context: TLS on, and DTLS
/// for a datagram client.
///
/// # Safety
/// `client` is live with a socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_enable_ssl(client: *mut Client) -> c_int {
    let this = Handle(client);
    this.set(OPEN_SSL, true);
    if is_dgram(this.socket().socket_type) && !enable_dtls(this) {
        return ERR;
    }
    OK
}

/// `Client::enable_dtls()`; false (warned) without DTLS support.
fn enable_dtls(this: Handle) -> bool {
    if this.ssl(SSL_DTLS, 0) != OK {
        return false;
    }
    let socket = this.socket();
    socket.dtls = 1;
    socket.chunk_size = SSL_BUFFER_SIZE;
    this.set(DTLS, true);
    true
}

/// `Client::ssl_handshake()`.
fn ssl_handshake(this: Handle) -> c_int {
    if this.socket().tls.state == SSL_STATE_READY || this.ssl(SSL_PREPARE, 0) < 0 {
        return ERR;
    }
    if this.socket().ssl.is_null() {
        this.socket().output.ssl_send = 1;
        if this.ssl(SSL_SESSION, 0) < 0 {
            this.set_last_error(ERROR_SSL_CREATE_SESSION_FAILED);
            return ERR;
        }
    }
    if this.ssl(SSL_CONNECT, 0) < 0 {
        return ERR;
    }
    if this.socket().tls.state == SSL_STATE_READY {
        let peer = this.ssl(SSL_VERIFY_PEER, 0);
        if peer & 1 != 0 && this.ssl(SSL_VERIFY, (peer >> 1) & 1) < 0 {
            return ERR;
        }
    }
    OK
}

/// # Safety
/// `client` is live with a socket and a TLS context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_ssl_handshake(client: *mut Client) -> c_int {
    ssl_handshake(Handle(client))
}

/// `Client_inet_addr`: the first call routes through the proxies and records the
/// server; every call assigns `server_addr`, or waits for DNS.
fn inet_addr(this: Handle, host: *const c_char, port: c_int) -> c_int {
    let (mut host, mut port) = (host, port);
    if !this.flag(HOST_PRESET) {
        for proxy in [PROXY_SOCKS5, PROXY_HTTP] {
            let mut proxy_port = 0;
            // SAFETY: `host` is NUL-terminated; the returned host lives in the proxy.
            let proxy_host = unsafe {
                (this.native().proxy_route)(this.cli(), proxy, host, port, &raw mut proxy_port)
            };
            if !proxy_host.is_null() {
                (host, port) = (proxy_host, proxy_port);
            }
        }
        // SAFETY: a NUL-terminated host (the caller's or a proxy's).
        let server = unsafe { CStr::from_ptr(host) };
        server.clone_into(&mut this.get().server_host.borrow_mut());
        this.get().server_port.set(port);
        this.set(HOST_PRESET, true);
    }
    let resolve = !this.flag(ASYNC);
    // SAFETY: a NUL-terminated host.
    if !unsafe { (this.native().assign)(this.cli(), host, port, resolve) } {
        if !this.flag(DNS_COMPLETED) && this.last_error() == ERROR_BAD_HOST_ADDR {
            this.set(WAIT_DNS, true);
        } else {
            return ERR;
        }
    }
    OK
}

fn dns_lookup(this: Handle) -> c_int {
    let host = this.get().server_host.borrow().clone();
    // SAFETY: a NUL-terminated host, copied by the request.
    unsafe { (this.native().dns_lookup)(this.cli(), host.as_ptr(), this.get().domain) }
}

/// `Client::connect(host, port, timeout, flags)`.
///
/// # Safety
/// `client` is live with a socket; `host` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_connect(
    client: *mut Client,
    host: *const c_char,
    port: c_int,
    timeout: f64,
    flags: c_int,
) -> c_int {
    connect(Handle(client), host, port, timeout, flags)
}

fn connect(this: Handle, host: *const c_char, port: c_int, timeout: f64, flags: c_int) -> c_int {
    if !this.flag(STREAM) {
        udp_connect(this, host, port, timeout, flags)
    } else if this.flag(ASYNC) {
        tcp_connect_async(this, host, port, timeout)
    } else {
        tcp_connect_sync(this, host, port, timeout, flags != 0)
    }
}

/// `Client_tcp_connect_sync`: connect, then the proxy handshake and TLS.
fn tcp_connect_sync(
    this: Handle,
    host: *const c_char,
    port: c_int,
    timeout: f64,
    nonblock: bool,
) -> c_int {
    this.set_timeout(timeout);
    if timeout > 0.0 {
        // SAFETY: the live client's socket.
        unsafe { (this.native().kernel_timeout)(this.cli(), timeout) };
    }
    if inet_addr(this, host, port) < 0 {
        return ERR;
    }
    if nonblock {
        // SAFETY: the live client's socket and address.
        let rc = unsafe { (this.native().connect)(this.cli(), CONNECT_ASYNC) };
        if rc == SW_READY {
            return OK;
        }
        if rc == SW_WAIT {
            this.set(ASYNC_CONNECT, true);
        }
        return ERR;
    }
    // SAFETY: as above.
    let ret = unsafe { (this.native().connect)(this.cli(), CONNECT_SYNC) };
    if ret < 0 {
        return ret;
    }
    this.set(ACTIVE, true);
    // Only a proxy handshake reads the thread's shared buffer, which a thread
    // that never set up SwooleTG doesn't have.
    if this.proxy_state(PROXY_SOCKS5) >= 0 {
        let shared = this.buffer(BUFFER_SHARED, 0);
        let (data, length) = this.socks5_negotiate();
        if send(this, data, length, 0) < 0 {
            return ERR;
        }
        this.set_proxy_state(PROXY_SOCKS5, SOCKS5_HANDSHAKE);
        loop {
            let n = recv(this, shared.str, shared.size, 0);
            if n > 0 && this.socks5_handshake(shared.str, n.cast_unsigned()) {
                if this.proxy_state(PROXY_SOCKS5) == SOCKS5_READY {
                    break;
                }
                continue;
            }
            return ERR;
        }
    } else if this.proxy_state(PROXY_HTTP) >= 0 {
        let (data, length) = this.http_pack();
        if send(this, data, length, 0) < 0 {
            return ERR;
        }
        // After the pack, which may have grown it.
        let shared = this.buffer(BUFFER_SHARED, 0);
        let n = recv(this, shared.str, shared.size, 0);
        if n <= 0 || !this.http_handshake(true, n.cast_unsigned()) {
            return ERR;
        }
    }
    if this.flag(OPEN_SSL) && ssl_handshake(this) < 0 {
        return ERR;
    }
    ret
}

/// `Client_tcp_connect_async`: a nonblocking connect finished by the write handler.
fn tcp_connect_async(this: Handle, host: *const c_char, port: c_int, timeout: f64) -> c_int {
    if ![ON_CONNECT, ON_ERROR, ON_CLOSE, ON_RECEIVE]
        .into_iter()
        .all(|which| this.has(which))
    {
        this.warn(WARN_CALLBACKS);
        return ERR;
    }
    this.set_timeout(timeout);
    this.buffer(BUFFER_INIT, 0);
    if this.has(ON_BUFFER_FULL) && this.get().high_watermark.get() == 0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream's buffer_size * 0.8 into a uint32_t"
        )]
        let mark = (f64::from(this.socket().output.buffer_size) * 0.8) as u32;
        this.get().high_watermark.set(mark);
    }
    if inet_addr(this, host, port) < 0 {
        return ERR;
    }
    if this.flag(WAIT_DNS) {
        return dns_lookup(this);
    }
    let ret = loop {
        // SAFETY: the live client's socket and address.
        let ret = unsafe { (this.native().connect)(this.cli(), CONNECT_RAW) };
        let error = errno();
        if ret < 0 && error == libc::EINTR {
            continue;
        }
        if ret == 0 || error == libc::EINPROGRESS {
            // A finished connect needs no timeout timer; either way the write
            // handler runs the proxy and TLS handshakes.
            if ret < 0 && timeout > 0.0 {
                // SAFETY: a timer over the live client.
                let timer = unsafe { (this.native().timer_add)(this.cli(), timeout) };
                this.get().timer.set(timer);
                if timer.is_null() {
                    return ERR;
                }
            }
            return this.event(EVENT_ADD, EVENT_WRITE);
        }
        break ret;
    };
    this.set(ACTIVE, false);
    this.socket().removed = 1;
    close(this);
    if this.has(ON_ERROR) {
        this.set(ONERROR_CALLED, true);
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
    ret
}

/// `Client_udp_connect`.
fn udp_connect(
    this: Handle,
    host: *const c_char,
    port: c_int,
    timeout: f64,
    udp_connect: c_int,
) -> c_int {
    let is_async = this.flag(ASYNC);
    this.set_timeout(timeout);
    if !is_async && timeout > 0.0 {
        // SAFETY: the live client's socket.
        unsafe { (this.native().kernel_timeout)(this.cli(), timeout) };
    }
    this.get().sock_flags.set(udp_connect);
    if inet_addr(this, host, port) < 0 {
        return ERR;
    }
    if is_async && !this.has(ON_RECEIVE) {
        this.warn(WARN_ON_RECEIVE);
        return ERR;
    }
    if this.flag(WAIT_DNS) && is_async {
        // Without onError a failed lookup would go unreported.
        if !this.has(ON_ERROR) {
            this.warn(WARN_ON_ERROR);
            return ERR;
        }
        return dns_lookup(this);
    }
    this.set(ACTIVE, true);
    let fd = this.socket().fd;
    // SAFETY: a plain read of the defaults, which only the owning thread assigns.
    let default_size = unsafe { swr_v1_socket_defaults.buffer_size };
    swr_v1_socket_buffer_size(fd, true, true, default_size);
    if timeout > 0.0 {
        this.set_timeout(timeout);
    }
    if this.socket().socket_type == SOCK_UNIX_DGRAM && bind_unix(this) < 0 {
        this.warn(WARN_BIND);
        return ERR;
    }
    let mut udp_connect = udp_connect;
    if this.flag(OPEN_SSL) {
        udp_connect = 1;
        if !enable_dtls(this) {
            return ERR;
        }
    }
    if udp_connect == 1 {
        // SAFETY: the live client's socket and address.
        if unsafe { (this.native().connect)(this.cli(), CONNECT_RAW) } != 0 {
            this.set(ACTIVE, false);
            this.socket().removed = 1;
            close(this);
            if is_async && this.has(ON_ERROR) {
                this.set(ONERROR_CALLED, true);
                this.call(ON_ERROR, core::ptr::null(), 0);
            }
            return ERR;
        }
        swr_v1_socket_clean(fd);
    }
    if is_async && this.has(ON_CONNECT) {
        if this.event(EVENT_ADD, EVENT_READ) < 0 {
            return ERR;
        }
        if !this.on_connect() {
            return ERR;
        }
    }
    if this.flag(OPEN_SSL) && ssl_handshake(this) < 0 {
        return ERR;
    }
    OK
}

/// Binds a Unix datagram client to `/tmp/swoole-client.<pid>.<fd>.sock`, its
/// address kept in the socket's `info` for the unlink on close.
fn bind_unix(this: Handle) -> c_int {
    let socket = this.socket();
    // SAFETY: getpid(2) has no preconditions.
    let pid = unsafe { libc::getpid() };
    let path = format!("/tmp/swoole-client.{pid}.{}.sock", socket.fd);
    // SAFETY: the address union's Unix member, which this socket's family uses.
    let address = unsafe { &mut socket.info.addr.un };
    for (slot, byte) in address.sun_path.iter_mut().zip(path.bytes().chain([0])) {
        *slot = c_char::from_ne_bytes([byte]);
    }
    #[expect(clippy::cast_possible_truncation, reason = "AF_UNIX fits sa_family_t")]
    let family = libc::AF_UNIX as libc::sa_family_t;
    address.sun_family = family;
    // SAFETY: the NUL-terminated path just written.
    unsafe { libc::unlink(address.sun_path.as_ptr()) };
    #[expect(clippy::cast_possible_truncation, reason = "sizeof(sockaddr_un)")]
    let length = size_of::<libc::sockaddr_un>() as libc::socklen_t;
    // SAFETY: binds the client's descriptor to its own address storage.
    unsafe { libc::bind(socket.fd, (&raw const *address).cast(), length) }
}

/// `Client::send(data, length, flags)`.
///
/// # Safety
/// `client` is live with a socket; `length` bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_send(
    client: *mut Client,
    data: *const c_char,
    length: usize,
    flags: c_int,
) -> isize {
    send(Handle(client), data, length, flags)
}

fn send(this: Handle, data: *const c_char, length: usize, flags: c_int) -> isize {
    if this.flag(DTLS) || this.flag(STREAM) && !this.flag(ASYNC) {
        this.io(IO_SEND_SYNC, data.cast_mut(), length, flags)
    } else if this.flag(STREAM) {
        send_async(this, data, length)
    } else {
        let n = this.io(IO_SENDTO, data.cast_mut(), length, 0);
        if n < 0 || n.cast_unsigned() < length {
            ERR as isize
        } else {
            n
        }
    }
}

/// `Client_tcp_send_async`: queue on the reactor, raising `onBufferFull` at the high watermark.
fn send_async(this: Handle, data: *const c_char, length: usize) -> isize {
    let mut n = length.cast_signed();
    // SAFETY: `length` bytes at `data`, copied by the reactor.
    if unsafe { (this.native().event_write)(this.cli(), data, length) } < 0 {
        if this.last_error() != ERROR_OUTPUT_BUFFER_OVERFLOW {
            return ERR as isize;
        }
        n = -1;
        this.set(HIGH_WATERMARK, true);
    }
    if this.has(ON_BUFFER_FULL) && !this.flag(HIGH_WATERMARK) {
        // SAFETY: the live client's socket.
        let queued = unsafe { (this.native().out_buffer_length)(this.cli()) };
        if queued >= this.get().high_watermark.get() {
            this.set(HIGH_WATERMARK, true);
            this.call(ON_BUFFER_FULL, core::ptr::null(), 0);
        }
    }
    n
}

/// `Client::sendfile(file, offset, length)`.
///
/// # Safety
/// `client` is live with a socket; `file` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_sendfile(
    client: *mut Client,
    file: *const c_char,
    offset: i64,
    length: usize,
) -> c_int {
    let this = Handle(client);
    if !this.flag(STREAM) {
        return ERR;
    }
    let is_async = this.flag(ASYNC);
    // SAFETY: as the caller promises.
    if unsafe { (this.native().sendfile)(this.cli(), file, offset, length, is_async) } < 0 {
        return ERR;
    }
    if is_async && this.event(EVENT_ADD_OR_UPDATE, EVENT_WRITE) == ERR {
        return ERR;
    }
    OK
}

/// `Client::recv(data, length, flags)`.
///
/// # Safety
/// `client` is live with a socket; `length` writable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_recv(
    client: *mut Client,
    data: *mut c_char,
    length: usize,
    flags: c_int,
) -> isize {
    recv(Handle(client), data, length, flags)
}

fn recv(this: Handle, data: *mut c_char, length: usize, flags: c_int) -> isize {
    if this.flag(STREAM) || this.flag(DTLS) {
        let n = this.io(IO_RECV_SYNC, data, length, flags);
        // A poll timeout reads as recv(2)'s EAGAIN, as it did before poll.
        if n == -1 && this.last_error() == ERROR_SOCKET_POLL_TIMEOUT {
            set_errno(libc::EAGAIN);
        }
        n
    } else if this.flag(ASYNC) {
        this.io(IO_RECVFROM_SYNC, data, length, flags)
    } else {
        this.io(IO_RECVFROM, data, length, flags)
    }
}

/// A handshake in the read or write handler failed: close and report.
fn connect_failed(this: Handle) -> c_int {
    this.set(ACTIVE, false);
    close(this);
    if this.has(ON_ERROR) {
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
    OK
}

/// `Client_onStreamRead`: the proxy and TLS handshakes that finish a connect,
/// then framed or plain reads.
///
/// # Safety
/// `client` is a live async stream client with its input buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_on_read(client: *mut Client) -> c_int {
    let this = Handle(client);
    let view = this.buffer(BUFFER_VIEW, 0);
    // SAFETY: the unused tail of the input buffer.
    let buf = unsafe { view.str.add(view.length) };
    let size = view.size - view.length;
    let do_ssl_handshake = this.flag(OPEN_SSL);
    let http = this.proxy_state(PROXY_HTTP);
    if http >= 0 && http != HTTP_PROXY_READY {
        let n = this.io(IO_RECV, buf, size, 0);
        if n <= 0 || !this.http_handshake(false, n.cast_unsigned()) {
            this.set_last_error(ERROR_HTTP_PROXY_HANDSHAKE_ERROR);
            return connect_failed(this);
        }
        this.set_proxy_state(PROXY_HTTP, HTTP_PROXY_READY);
        this.buffer(BUFFER_CLEAR, 0);
        if !do_ssl_handshake {
            this.on_connect();
            return OK;
        }
    }
    let socks5 = this.proxy_state(PROXY_SOCKS5);
    if socks5 >= 0 && socks5 != SOCKS5_READY {
        let n = this.io(IO_RECV, buf, size, 0);
        if n <= 0 {
            this.set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        this.buffer(BUFFER_GROW, n.cast_unsigned());
        // Upstream hands the handshake the buffer's free size, not what was read.
        if !this.socks5_handshake(buf, size) {
            this.set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.proxy_state(PROXY_SOCKS5) != SOCKS5_READY {
            return OK;
        }
        this.buffer(BUFFER_CLEAR, 0);
        if !do_ssl_handshake {
            this.on_connect();
            return OK;
        }
    }
    if this.flag(OPEN_SSL) && this.socket().tls.state != SSL_STATE_READY {
        if ssl_handshake(this) < 0 {
            this.set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.socket().tls.state == SSL_STATE_READY {
            this.on_connect();
        }
        return OK;
    }
    let mut n = 0;
    // SAFETY: the live client's protocol and buffer.
    if unsafe { (this.native().recv_framed)(this.cli(), &raw mut n) } {
        if n < 0 && !this.flag(CLOSED) {
            close(this);
        }
        return OK;
    }
    let n = this.io(IO_RECV, buf, size, 0);
    if n < 0 {
        let socket = this.socket();
        // SAFETY: the socket's own flag, set by the classification.
        return match unsafe { swr_v1_socket_catch_error(errno(), &raw mut socket.kernel_nobufs) } {
            SW_ERROR => {
                this.warn(WARN_READ);
                OK
            }
            SW_CLOSE => close(this),
            _ => OK,
        };
    }
    if n == 0 {
        return close(this);
    }
    this.call(ON_RECEIVE, buf, n.cast_unsigned());
    OK
}

/// `Client_onDgramRead`.
///
/// # Safety
/// `client` is a live async datagram client.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_on_dgram_read(client: *mut Client) -> c_int {
    let this = Handle(client);
    #[expect(
        clippy::large_stack_arrays,
        reason = "upstream's stack buffer: nothing to drop if the callback longjmps"
    )]
    let mut buffer = [0_u8; BUFFER_SIZE_UDP];
    // Upstream's datagram receive, even once DTLS made `recv` a stream one.
    let op = if this.flag(ASYNC) {
        IO_RECVFROM_SYNC
    } else {
        IO_RECVFROM
    };
    let n = this.io(op, buffer.as_mut_ptr().cast(), buffer.len(), 0);
    if n < 0 {
        return ERR;
    }
    this.call(ON_RECEIVE, buffer.as_ptr().cast(), n.cast_unsigned());
    OK
}

/// `Client_onWrite`: drain queued output, or finish an async connect.
///
/// # Safety
/// `client` is a live async stream client; `reactor` and `event` are the handler's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_on_write(
    client: *mut Client,
    reactor: *mut c_void,
    event: *mut c_void,
) -> c_int {
    let this = Handle(client);
    if this.flag(ACTIVE) {
        if this.flag(OPEN_SSL) && this.socket().tls.state == SSL_STATE_WAIT_STREAM {
            if ssl_handshake(this) < 0 {
                this.set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
                return write_failed(this);
            }
            if this.socket().tls.state == SSL_STATE_READY {
                this.on_connect();
                return OK;
            }
            if this.socket().tls.want_read != 0 {
                this.event(EVENT_SET, EVENT_READ);
            }
            return OK;
        }
        // SAFETY: the handler's reactor and event.
        if unsafe { (this.native().writable)(reactor, event) } < 0 {
            return ERR;
        }
        if this.has(ON_BUFFER_EMPTY) && this.flag(HIGH_WATERMARK) {
            // SAFETY: the live client's socket.
            let queued = unsafe { (this.native().out_buffer_length)(this.cli()) };
            if queued <= this.get().low_watermark.get() {
                this.set(HIGH_WATERMARK, false);
                this.call(ON_BUFFER_EMPTY, core::ptr::null(), 0);
            }
        }
        return OK;
    }
    let mut error = 0;
    // SAFETY: the live client's socket; `error` is writable.
    let ret = unsafe { (this.native().socket_error)(this.cli(), &raw mut error) };
    this.set_last_error(error);
    if ret < 0 {
        this.warn(WARN_GETSOCKOPT);
        return ERR;
    }
    if error != 0 {
        return write_failed(this);
    }
    this.event(EVENT_SET, EVENT_READ);
    this.set(ACTIVE, true);
    if this.proxy_state(PROXY_SOCKS5) == SOCKS5_WAIT {
        let (data, length) = this.socks5_negotiate();
        this.set_proxy_state(PROXY_SOCKS5, SOCKS5_HANDSHAKE);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream returns the send result as an int"
        )]
        return send(this, data, length, 0) as c_int;
    }
    if this.proxy_state(PROXY_HTTP) == HTTP_PROXY_WAIT {
        let (data, length) = this.http_pack();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream returns the send result as an int"
        )]
        return send(this, data, length, 0) as c_int;
    }
    if this.flag(OPEN_SSL) {
        if ssl_handshake(this) < 0 {
            this.set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return write_failed(this);
        }
        this.socket().tls.state = SSL_STATE_WAIT_STREAM;
        return OK;
    }
    this.on_connect();
    OK
}

/// The write handler's failure: unlike the read handler's, `onError` is required.
fn write_failed(this: Handle) -> c_int {
    this.set(ACTIVE, false);
    close(this);
    this.call(ON_ERROR, core::ptr::null(), 0);
    OK
}

/// `Client_onError`.
///
/// # Safety
/// As `swr_v1_clients_on_write`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_on_error(
    client: *mut Client,
    reactor: *mut c_void,
    event: *mut c_void,
) -> c_int {
    let this = Handle(client);
    if this.flag(ACTIVE) {
        return close(this);
    }
    // SAFETY: as the caller promises.
    unsafe { swr_v1_clients_on_write(client, reactor, event) };
    OK
}

/// `Client_onTimeout`: the async connect timed out.
///
/// # Safety
/// `client` is a live async client whose connect timer fired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_on_timeout(client: *mut Client) {
    let this = Handle(client);
    this.set_last_error(libc::ETIMEDOUT);
    this.get().timer.set(core::ptr::null_mut());
    if this.flag(OPEN_SSL) && this.socket().tls.state != SSL_STATE_READY {
        this.set(ACTIVE, false);
    }
    let socks5 = this.proxy_state(PROXY_SOCKS5);
    let http = this.proxy_state(PROXY_HTTP);
    if socks5 >= 0 && socks5 != SOCKS5_READY || http >= 0 && http != HTTP_PROXY_READY {
        this.set(ACTIVE, false);
    }
    close(this);
    if this.has(ON_ERROR) {
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
}

/// `Client_onResolveCompleted`: connect to the resolved `address`, or report
/// the failure through `onError`.
///
/// # Safety
/// `client` is a live async client waiting for DNS; `address` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_on_resolved(
    client: *mut Client,
    error: c_int,
    address: *const c_char,
) {
    let this = Handle(client);
    this.set(WAIT_DNS, false);
    this.set(DNS_COMPLETED, true);
    if error == 0 {
        // The application can't see connect's result here, so onError reports it.
        let timeout = this.socket().timeouts.connect;
        let (port, flags) = (this.get().server_port.get(), this.get().sock_flags.get());
        if connect(this, address, port, timeout, flags) != ERR || this.flag(ONERROR_CALLED) {
            return;
        }
    } else {
        this.set_last_error(ERROR_DNSLOOKUP_RESOLVE_FAILED);
    }
    this.socket().removed = 1;
    close(this);
    if this.has(ON_ERROR) {
        this.set(ONERROR_CALLED, true);
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
