//! `network::Client` (upstream's `src/network/client.cc`) in Rust: its state and every
//! operation over it, from construction to close, with its callbacks (one
//! listener and its object), its reactor handlers, its connect timer, its TLS
//! context, its proxies and its DNS lookup. The public POD fields are
//! [`ClientRecord`], at the state's start, so a client is also its record. The
//! socket is the reactor's record, read and written here directly; its `object`
//! is this state.
//!
//! A user callback can destroy the client (and this state with it), so no
//! reference to the state lives across one: operations work on a raw [`Handle`]
//! and borrow the state only between effects. A PHP bailout in a callback
//! longjmps out, so nothing that needs dropping is held across one.
use core::cell::{Cell, OnceCell, RefCell};
use core::ffi::{c_char, c_int, c_void};
use core::ptr::NonNull;
use std::ffi::{CStr, CString};
use std::sync::OnceLock;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::constants::{
    EventType, FdType, ResultCode, ReturnCode, SW_BUFFER_SIZE_UDP, SW_CLIENT_BUFFER_SIZE,
    SW_SEND_BUFFER_SIZE, SW_SOCKET_RETRY_COUNT, SW_SSL_BUFFER_SIZE, SocketFlag, SocketType,
    TimeoutType,
};
use swoole_rs_corebase::net_types::{PacketLength, Protocol, SocketAddress};
use swoole_rs_corebase::sys::ReadSlot;
#[cfg(not(test))]
use swoole_rs_corebase::thread_buffer;
use swoole_rs_corebase::{ProxyState, ResolverRequest, log, swr_v1_core_proxy_free};
use swoole_rs_error::ErrorCode;
#[cfg(not(test))]
use swoole_rs_reactor::event_api::swoole_event_write;
use swoole_rs_reactor::reactor_upstream::reactor_set_handler;
use swoole_rs_reactor::{Event, Running};
use swoole_rs_reactor::{
    SOCKET_DEFAULTS, Socket, SocketRecord, fd_clean, swr_v1_socket_buffer_size,
    swr_v1_socket_domain_and_type,
};
#[cfg(not(test))]
use swoole_rs_tls::ssl_context_create;
#[cfg(not(test))]
use swoole_rs_tls::upstream::{
    self as net, socket_free, socket_sendfile_async, socket_sendfile_sync, socket_ssl_check_host,
    socket_ssl_close, socket_ssl_connect, socket_ssl_create, socket_ssl_verify,
};
use swoole_rs_tls::{
    OwnedTlsContext, OwnedTlsFields, OwnedTlsPrepared, SslCreateFlag, SslState, SslVersion,
    TlsContext, swr_v1_tls_owner_view,
};
#[cfg(test)]
use tests::{
    async_dispatch, net, ops, socket_free, socket_sendfile_async, socket_sendfile_sync,
    socket_ssl_check_host, socket_ssl_close, socket_ssl_connect, socket_ssl_create,
    socket_ssl_verify, ssl_context_create, swoole_event_write, thread_buffer, writable_callback,
};

/// The flags `swr_v1_clients_flag` reads.
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

/// The events a [`Listener`] hears: bit `1 << event` of
/// [`swr_v1_clients_listen`]'s mask is set for each it wants.
pub const ON_CONNECT: c_int = 1;
pub const ON_RECEIVE: c_int = 2;
pub const ON_CLOSE: c_int = 3;
pub const ON_ERROR: c_int = 4;
pub const ON_BUFFER_FULL: c_int = 5;
pub const ON_BUFFER_EMPTY: c_int = 6;

/// The client's callbacks: `listener(object, event, data, length)`, with the
/// received bytes for [`ON_RECEIVE`] and the last error as `length` for [`ON_ERROR`].
pub type Listener =
    unsafe extern "C" fn(object: *mut c_void, event: c_int, data: *const c_char, length: usize);

/// The socket's connect modes: `connect_async` (returns `SW_READY`, `SW_WAIT` or
/// `SW_ERROR`), `connect_sync`, and `connect(2)`.
const CONNECT_ASYNC: c_int = 0;
const CONNECT_SYNC: c_int = 1;
const CONNECT_RAW: c_int = 2;

/// The reactor's `swoole_event_*` operations on the socket.
const EVENT_ADD: c_int = 0;
const EVENT_SET: c_int = 1;
const EVENT_DEL: c_int = 2;
const EVENT_ADD_OR_UPDATE: c_int = 3;

/// Swoole's constants this file uses.
const OK: c_int = ResultCode::SW_OK as c_int;
const ERR: c_int = ResultCode::SW_ERR as c_int;
const SW_WAIT: c_int = ReturnCode::SW_WAIT as c_int;
const SW_CLOSE: c_int = ReturnCode::SW_CLOSE as c_int;
const SW_ERROR: c_int = ReturnCode::SW_ERROR as c_int;
const SW_READY: c_int = ReturnCode::SW_READY as c_int;
const SOCK_UNIX_DGRAM: c_int = SocketType::SW_SOCK_UNIX_DGRAM as c_int;
const FD_STREAM_CLIENT: c_int = FdType::SW_FD_STREAM_CLIENT as c_int;
const FD_DGRAM_CLIENT: c_int = FdType::SW_FD_DGRAM_CLIENT as c_int;
const SOCK_NONBLOCK: c_int = SocketFlag::SW_SOCK_NONBLOCK as c_int;
const SOCK_CLOEXEC: c_int = SocketFlag::SW_SOCK_CLOEXEC as c_int;
const SEND_BUFFER_SIZE: u32 = SW_SEND_BUFFER_SIZE;
const SSL_BUFFER_SIZE: u32 = SW_SSL_BUFFER_SIZE;
const BUFFER_SIZE_UDP: usize = SW_BUFFER_SIZE_UDP as usize;
const CLIENT_BUFFER_SIZE: usize = SW_CLIENT_BUFFER_SIZE as usize;
const SOCKET_RETRY_COUNT: usize = SW_SOCKET_RETRY_COUNT as usize;
const EVENT_READ: c_int = EventType::SW_EVENT_READ as c_int;
const EVENT_WRITE: c_int = EventType::SW_EVENT_WRITE as c_int;
const EVENT_ERROR: c_int = EventType::SW_EVENT_ERROR as c_int;
const TIMEOUT_ALL: c_int = TimeoutType::SW_TIMEOUT_ALL as c_int;
/// crates/tls's `SW_SSL_STATE_READY`, `SW_SSL_STATE_WAIT_STREAM`, `SW_SSL_CLIENT`
/// and `SW_SSL_DTLS`.
pub const SSL_STATE_READY: u32 = SslState::SW_SSL_STATE_READY as u32;
pub const SSL_STATE_WAIT_STREAM: u32 = SslState::SW_SSL_STATE_WAIT_STREAM as u32;
pub const SSL_CLIENT: c_int = SslCreateFlag::SW_SSL_CLIENT as c_int;
pub const SSL_DTLS: u32 = SslVersion::SW_SSL_DTLS as u32;
/// `SSLContext::TLS_HOST_NAME`'s string slot.
const TLS_HOST_NAME: usize = 8;
/// OpenSSL's `SSL_CTRL_SET_TLSEXT_HOSTNAME` and `TLSEXT_NAMETYPE_host_name`.
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const TLSEXT_NAMETYPE_HOST_NAME: libc::c_long = 0;
/// The proxy states of `native/protocols/swoole_proxy.h`, which asserts them.
const SOCKS5_WAIT: c_int = 0;
const SOCKS5_HANDSHAKE: c_int = 1;
const SOCKS5_READY: c_int = 4;
const HTTP_PROXY_WAIT: c_int = 0;
const HTTP_PROXY_READY: c_int = 2;
const ERROR_OPERATION_NOT_SUPPORT: c_int = ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT as c_int;
const ERROR_DNSLOOKUP_RESOLVE_FAILED: c_int = ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_FAILED as c_int;
const ERROR_BAD_HOST_ADDR: c_int = ErrorCode::SW_ERROR_BAD_HOST_ADDR as c_int;
const ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = ErrorCode::SW_ERROR_OUTPUT_BUFFER_OVERFLOW as c_int;
const ERROR_SSL_HANDSHAKE_FAILED: c_int = ErrorCode::SW_ERROR_SSL_HANDSHAKE_FAILED as c_int;
const ERROR_SSL_CREATE_SESSION_FAILED: c_int =
    ErrorCode::SW_ERROR_SSL_CREATE_SESSION_FAILED as c_int;
const ERROR_SOCKET_POLL_TIMEOUT: c_int = ErrorCode::SW_ERROR_SOCKET_POLL_TIMEOUT as c_int;
const ERROR_SOCKS5_HANDSHAKE_FAILED: c_int = ErrorCode::SW_ERROR_SOCKS5_HANDSHAKE_FAILED as c_int;
const ERROR_HTTP_PROXY_HANDSHAKE_ERROR: c_int =
    ErrorCode::SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR as c_int;

// crates/tls's reactor write (the unit tests use a double instead).
#[cfg(not(test))]
use swoole_rs_tls::reactor_write::reactor_writable_callback as writable_callback;

/// One of `Protocol::recv_with_length_protocol` and `recv_with_eof_protocol` over a
/// live protocol, socket and input buffer.
pub type Framed = fn(*const Protocol, *mut SocketRecord, *mut CppString) -> c_int;

/// The framed receives crates/ports defines; ports depends on this crate, so
/// `swoole_init` (crates/ffi) registers them with [`set_framing`]. Only a
/// client's `on_read` calls them, so the fields stay private.
pub struct Framing {
    length: Framed,
    eof: Framed,
}

impl Framing {
    /// # Safety
    /// `length` and `eof` must be sound for any live protocol, socket and input
    /// buffer of a client, which is all a client's `on_read` passes them.
    pub const unsafe fn new(length: Framed, eof: Framed) -> Self {
        Self { length, eof }
    }
}

static FRAMING: OnceLock<Framing> = OnceLock::new();

/// Registers crates/ports' framed receives; later registrations are ignored.
pub fn set_framing(framing: Framing) {
    FRAMING.get_or_init(|| framing);
}

// SAFETY: crates/protocols-ffi's definitions (the SOCKS5 codec) and OpenSSL's, which
// the hybrid links (the unit tests define doubles). crates/protocols-ffi sits above
// this one, so it reaches them by symbol.
unsafe extern "C" {
    #[link_name = "SSL_ctrl"]
    fn ssl_ctrl(
        ssl: *mut c_void,
        cmd: c_int,
        larg: libc::c_long,
        parg: *mut c_void,
    ) -> libc::c_long;
    /// `Socks5Proxy::admit`: 0 (logged) when a credential is too long, 2 when
    /// both are set, else 1.
    #[link_name = "_ZN6swoole11Socks5Proxy5admitEmm"]
    fn socks5_admit(user_length: usize, password_length: usize) -> c_int;
    /// `Socks5Proxy::pack_negotiate_request()` into the proxy's buffer. A
    /// `Socks5Proxy` is its owner pointer.
    #[link_name = "_ZN6swoole11Socks5Proxy22pack_negotiate_requestEv"]
    fn socks5_pack_negotiate(proxy: *const *mut c_void) -> isize;
    /// `Socks5Proxy::handshake_step(data, length)`: the size of the answer it
    /// packed, 0 when none is due, -1 on failure.
    #[link_name = "_ZN6swoole11Socks5Proxy14handshake_stepEPKcm"]
    fn socks5_handshake_step(
        proxy: *const *mut c_void,
        data: *const c_char,
        length: usize,
    ) -> isize;
}

/// A `String`'s free tail: where the next read lands and how much fits.
#[derive(Clone, Copy)]
struct Tail {
    str: *mut c_char,
    size: usize,
}

/// `async::Handler`: runs on a pool thread over its own copy of the event, of which
/// the pool keeps `error` and `retval`.
type AsyncHandler = extern "C" fn(&mut AsyncEvent);

/// The completion, on the dispatching thread.
type AsyncCallback = extern "C" fn(&AsyncEvent);

/// Upstream's `AsyncEvent` as crates/hooks' pool takes it, its request a counted
/// DNS record.
#[repr(C)]
struct AsyncEvent {
    task_id: usize,
    canceled: u8,
    error: c_int,
    data: *mut ResolverRequest,
    retval: isize,
    pipe_socket: *mut c_void,
    timestamp: f64,
    object: *mut c_void,
    handler: Option<AsyncHandler>,
    callback: Option<AsyncCallback>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<AsyncEvent>() == 72
        && core::mem::offset_of!(AsyncEvent, data) == 16
        && core::mem::offset_of!(AsyncEvent, object) == 48,
    "crates/hooks' AsyncEvent layout moved"
);

// SAFETY: crates/hooks' pool (async_threads.rs), which the hybrid links; it sits
// above this crate. Its `dispatch` is a safe fn over this layout: only this module
// builds an event, whose request is null or live.
#[cfg(not(test))]
unsafe extern "C" {
    /// `async::dispatch`: queues a copy of the event, holding its own reference
    /// to the request; null without a pool.
    #[link_name = "_ZN6swoole5async8dispatchEPKNS_10AsyncEventE"]
    #[expect(
        improper_ctypes,
        reason = "the request record is opaque behind its pointer; Rust defines both sides"
    )]
    safe fn async_dispatch(event: &AsyncEvent) -> *mut c_void;
}

/// The public, trivially copyable fields of `network::Client`. C++ keeps typed
/// aliases in its base record; protocol callbacks and addresses retain their
/// original identity and lifetime, including direct SDK field writes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClientRecord {
    pub http2: bool,
    pub open_length_check: bool,
    pub open_eof_check: bool,
    pub protocol: Protocol,
    pub server_addr: SocketAddress,
    pub remote_addr: SocketAddress,
    pub socket: *mut SocketRecord,
    pub object: *mut c_void,
    pub buffer: *mut c_void,
}

const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(
        offset_of!(ClientRecord, protocol) == 8
            && offset_of!(ClientRecord, server_addr) == 72
            && offset_of!(ClientRecord, remote_addr) == 72 + size_of::<SocketAddress>()
            && offset_of!(ClientRecord, socket) == 72 + 2 * size_of::<SocketAddress>()
            && size_of::<ClientRecord>() == 96 + 2 * size_of::<SocketAddress>(),
        "network::Client's public record layout moved"
    );
};

/// One client's state, its public record first: `network::Client`.
#[repr(C)]
pub struct Client {
    record: Cell<ClientRecord>,
    flags: Cell<u32>,
    domain: c_int,
    sock_flags: Cell<c_int>,
    server_host: RefCell<CString>,
    server_port: Cell<c_int>,
    timer: Cell<*mut c_void>,
    high_watermark: Cell<u32>,
    low_watermark: Cell<u32>,
    listener: Cell<Option<Listener>>,
    /// `1 << event` for each event the listener hears.
    events: Cell<u32>,
    /// The `SSLContext`, none while TLS is off.
    tls: OnceCell<TlsContext>,
    /// The proxies, null when unset.
    socks5: Cell<*mut ProxyState>,
    http_proxy: Cell<*mut ProxyState>,
}

/// A client's proxies.
#[derive(Clone, Copy)]
enum Proxy {
    Socks5,
    Http,
}

impl Client {
    fn proxy_slot(&self, which: Proxy) -> &Cell<*mut ProxyState> {
        match which {
            Proxy::Socks5 => &self.socks5,
            Proxy::Http => &self.http_proxy,
        }
    }

    /// A copy of the public record.
    pub fn record(&self) -> ClientRecord {
        self.record.get()
    }

    /// Edit a copy of the public record and write it back, so no reference
    /// into the record is live while `edit` runs.
    pub fn edit_record<R>(&self, edit: impl FnOnce(&mut ClientRecord) -> R) -> R {
        let mut record = self.record.get();
        let result = edit(&mut record);
        self.record.set(record);
        result
    }

    /// One of the flags above; false for any other.
    pub fn flag(&self, name: u32) -> bool {
        self.flags.get() & (1 << name) != 0
    }

    pub fn set(&self, name: u32, value: bool) {
        let bit = 1 << name;
        self.flags.set(if value {
            self.flags.get() | bit
        } else {
            self.flags.get() & !bit
        });
    }

    /// `buffer_high_watermark` (`high`) or `buffer_low_watermark`.
    pub fn set_watermark(&self, high: bool, value: u32) {
        if high {
            self.high_watermark.set(value);
        } else {
            self.low_watermark.set(value);
        }
    }
}

/// A client under an operation: a raw pointer, since a callback may free it.
#[derive(Clone, Copy)]
pub struct Handle(*mut Client);

impl Handle {
    fn get(self) -> &'static Client {
        // SAFETY: the client is live between callbacks; callers drop the reference
        // before any effect that may release it.
        unsafe { &*self.0 }
    }

    fn flag(self, name: u32) -> bool {
        self.get().flag(name)
    }

    fn set(self, name: u32, value: bool) {
        self.get().set(name, value);
    }

    /// The client's public record: live with the client, and borrowed only
    /// between effects, as the state is. Each call reborrows the whole record, so
    /// it ends the borrow an earlier call (or `socket`/`socket_ptr`) returned:
    /// call those first and keep only the last borrow.
    fn fields(self) -> &'static mut ClientRecord {
        // SAFETY: the live client's record, which SDK callers reach only between
        // its operations.
        unsafe { &mut *self.get().record.as_ptr() }
    }

    fn socket_ptr(self) -> *mut SocketRecord {
        self.fields().socket
    }

    fn socket(self) -> Socket<'static> {
        // SAFETY: a constructed client's socket record, live until the client is
        // destroyed (an async one's free is deferred past its callbacks).
        unsafe { Socket::from_ptr(self.socket_ptr()) }.unwrap_or_else(|| std::process::abort())
    }

    fn socket_type(self) -> c_int {
        self.socket().read(|socket| socket.socket_type)
    }

    fn tls_state(self) -> u32 {
        self.socket().read(|socket| socket.tls.state)
    }

    /// `socket->removed = 1`: close skips the reactor.
    fn mark_removed(self) {
        self.socket().edit(|socket| socket.removed = 1);
    }

    fn has(self, event: c_int) -> bool {
        self.get().events.get() & (1 << event) != 0
    }

    /// Runs the user callback for `event`, if the listener hears it. The client
    /// may be gone afterwards.
    fn call(self, event: c_int, data: *const c_char, length: usize) {
        let Some(listener) = self.get().listener.get().filter(|_| self.has(event)) else {
            return;
        };
        let length = if event == ON_ERROR {
            log::last_error().unsigned_abs() as usize
        } else {
            length
        };
        let object = self.fields().object;
        // SAFETY: the listener and its object; nothing of the client is borrowed
        // across the call, which may release it.
        unsafe { listener(object, event, data, length) };
    }

    fn event(self, op: c_int, events: c_int) -> c_int {
        ops::event(self, op, events)
    }

    /// `Socket::recv`: one read, through the TLS session when there is one.
    fn recv_once(self, buffer: &mut [impl ReadSlot]) -> isize {
        net::recv(self.socket(), buffer, 0)
    }

    /// `Socket::sendto(server_addr, data, length)`: one sendto(2).
    fn sendto(self, data: &[u8]) -> isize {
        // The descriptor first: `socket()` borrows the record again.
        let fd = self.socket().fd();
        let address = &self.fields().server_addr;
        address.sendto(fd, data, 0)
    }

    /// `Socket::recvfrom(data, length, flags, &remote_addr)`: recvfrom(2),
    /// retried on EINTR.
    fn recvfrom(self, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
        let fd = self.socket().fd();
        let address = &mut self.fields().remote_addr;
        let mut n = -1;
        for _ in 0..SOCKET_RETRY_COUNT {
            n = address.recvfrom(fd, &mut *buffer, flags);
            if n >= 0 || errno() != libc::EINTR {
                break;
            }
        }
        n
    }

    /// `Socket::recvfrom_sync(data, length, flags, &remote_addr)`.
    fn recvfrom_sync(self, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
        // The socket first: `socket()` borrows the record again.
        let socket = self.socket();
        net::recvfrom_sync(socket, buffer, flags, &mut self.fields().remote_addr)
    }

    /// The TLS context's view; TLS is on.
    fn tls(self) -> OwnedTlsPrepared {
        let tls = self.get().tls.get();
        swr_v1_tls_owner_view(tls.unwrap_or_else(|| std::process::abort()))
    }

    fn tls_fields(self) -> &'static mut OwnedTlsFields {
        // SAFETY: the live context's fields, borrowed only between effects.
        unsafe { &mut *self.tls().fields }
    }

    /// The context's SNI host name, NUL-terminated and empty when unset.
    fn tls_host_name(self) -> &'static CStr {
        let slot = self.tls().strings[TLS_HOST_NAME];
        // SAFETY: the live context's slot.
        let name = unsafe { &*slot }.view();
        // SAFETY: a slot's view is NUL-terminated.
        unsafe { CStr::from_ptr(name.data) }
    }

    fn proxy(self, which: Proxy) -> Option<&'static ProxyState> {
        let proxy = self.get().proxy_slot(which).get();
        // SAFETY: a proxy the client owns until it is replaced or freed.
        (!proxy.is_null()).then(|| unsafe { &*proxy })
    }

    /// The proxy's state, or -1 when there is none.
    fn proxy_state(self, which: Proxy) -> c_int {
        self.proxy(which)
            .map_or(-1, |proxy| c_int::from(proxy.fields().state))
    }

    fn set_proxy_state(self, which: Proxy, state: c_int) {
        if let Some(proxy) = self.proxy(which) {
            proxy.update(|fields| fields.state = u8::try_from(state).unwrap_or_default());
        }
    }

    /// `socket->get_out_buffer_length()`.
    fn queued(self) -> u32 {
        let buffer = self.socket().read(|socket| socket.output.out_buffer);
        if buffer.is_null() {
            return 0;
        }
        // SAFETY: the socket's live output Buffer.
        swoole_rs_buffer::cpp::swr_cpp_buffer_length(unsafe { &*buffer.cast() })
    }

    /// The input buffer (`cli->buffer`), null until an async connect makes it.
    fn input(self) -> *mut CppString {
        self.fields().buffer.cast()
    }

    /// The input buffer, which exists.
    fn input_mut(self) -> &'static mut CppString {
        // SAFETY: the Box `input_init` made, live until `input_free`, and
        // borrowed only between effects.
        unsafe { &mut *self.input() }
    }

    /// `new String(SW_CLIENT_BUFFER_SIZE)`, once.
    fn input_init(self) {
        if self.input().is_null() {
            let mut buffer = Box::new(CppString::EMPTY);
            buffer.alloc(CLIENT_BUFFER_SIZE, None);
            self.fields().buffer = Box::into_raw(buffer).cast();
        }
    }

    /// `delete buffer`.
    fn input_free(self) {
        let buffer = self.input();
        if !buffer.is_null() {
            self.fields().buffer = core::ptr::null_mut();
            // SAFETY: the Box `input_init` made, released once.
            unsafe { Box::from_raw(buffer) }.dispose();
        }
    }

    /// The input buffer's free tail; the buffer exists.
    fn input_tail(self) -> Tail {
        let buffer = self.input_mut();
        let length = buffer.len();
        Tail {
            str: buffer.as_mut_ptr().wrapping_add(length).cast(),
            size: buffer.size() - length,
        }
    }

    /// The input buffer's length grows by `n` read bytes.
    fn input_grow(self, n: usize) {
        let buffer = self.input_mut();
        // SAFETY: its one caller read `n` bytes into `input_tail`, which bounds them.
        unsafe { buffer.set_len(buffer.len() + n) };
    }

    /// `buffer->clear()`.
    fn input_clear(self) {
        self.input_mut().clear();
    }

    /// The SOCKS5 negotiation request's length in [`Self::socks5_packet`].
    fn socks5_negotiate(self) -> usize {
        let owner = self.get().socks5.as_ptr().cast();
        // SAFETY: the live SOCKS5 proxy, whose buffer the request is packed into.
        let length = unsafe { socks5_pack_negotiate(owner) };
        length.cast_unsigned()
    }

    /// Send the first `length` bytes of the SOCKS5 proxy's packet buffer, copied
    /// first: the send may raise a callback that reaches the proxy again.
    fn socks5_send(self, length: usize) -> isize {
        let packet = self
            .proxy(Proxy::Socks5)
            .and_then(ProxyState::packet)
            .map_or([0; 512], |packet| *packet);
        send(self, packet.get(..length).unwrap_or_default(), 0)
    }

    /// `Client::socks5_handshake`: applies `length` received bytes at `data` and
    /// sends the answer the proxy packed.
    fn socks5_handshake(self, data: *const c_char, length: usize) -> bool {
        let owner = self.get().socks5.as_ptr().cast();
        // SAFETY: the live SOCKS5 proxy; `length` bytes at `data`.
        let n = unsafe { socks5_handshake_step(owner, data, length) };
        n == 0 || n > 0 && self.socks5_send(n.cast_unsigned()) == n
    }

    /// The HTTP proxy's CONNECT request in the shared buffer.
    /// The host is the TLS host name when set, else the proxy's target.
    fn http_pack(self) -> &'static [u8] {
        let Some(proxy) = self.proxy(Proxy::Http) else {
            return &[];
        };
        let tls = self.get().tls.get().is_some();
        let target = proxy.string(3);
        let host = if tls && !self.tls_host_name().is_empty() {
            self.tls_host_name().to_bytes()
        } else {
            &target
        };
        let shared = shared_buffer();
        let (at, size) = (shared.as_mut_ptr(), shared.size());
        // SAFETY: the shared buffer's `size` bytes, which nothing else borrows.
        let output = unsafe { core::slice::from_raw_parts_mut(at, size) };
        let length = swoole_rs_http_ffi::proxy_connect(proxy, host, output);
        output.get(..length).unwrap_or_default()
    }

    /// `HttpProxy::handshake` of the shared buffer (`shared`, its length set to
    /// `length`) or of the input buffer (its length grown by `length`).
    fn http_handshake(self, shared: bool, length: usize) -> bool {
        let buffer = if shared {
            shared_buffer()
        } else {
            self.input_mut()
        };
        let length = if shared {
            length
        } else {
            buffer.len() + length
        };
        // SAFETY: the callers' recv just filled them: the shared buffer from its
        // start, the input buffer after its content (its free tail bounds both).
        unsafe { buffer.set_len(length) };
        swoole_rs_http_ffi::proxy_accept(buffer.view())
    }

    fn set_timeout(self, timeout: f64) {
        self.socket()
            .edit(|socket| socket.timeouts.set(timeout, TIMEOUT_ALL));
    }

    fn timer_clear(self) {
        let timer = self.get().timer.replace(core::ptr::null_mut());
        if !timer.is_null() {
            // SAFETY: a pending timer this client added.
            unsafe { swoole_timer_del(timer.cast()) };
        }
    }

    /// `execute_onConnect`.
    fn on_connect(self) {
        self.timer_clear();
        self.call(ON_CONNECT, core::ptr::null(), 0);
    }
}

// The unit tests log timers instead.
#[cfg(not(test))]
use swoole_rs_timer::{swoole_timer_del, swr_v1_timer_add_plain_sec};
#[cfg(test)]
use tests::{swoole_timer_del, swr_v1_timer_add_plain_sec};

/// `RecvData`: the package `Protocol::recv_with_*` hands `onPackage`.
#[repr(C)]
struct RecvData {
    info: swoole_rs_corebase::DataHead,
    data: *const c_char,
}

/// `Client_onPackage`: a framed package for `onReceive`; the receive loop stops
/// once the callback left the socket waiting to close.
/// # Safety
/// `conn` is a client's socket record (or null), whose free is deferred past the call.
unsafe extern "C-unwind" fn on_package(
    _: *const Protocol,
    conn: *mut c_void,
    package: *const c_void,
) -> c_int {
    // SAFETY: per the caller.
    let Some(socket) = (unsafe { Socket::from_ptr(conn.cast()) }) else {
        return ERR;
    };
    // SAFETY: per the caller, the package.
    let package = unsafe { &*package.cast::<RecvData>() };
    Handle(socket.read(|socket| socket.object).cast()).call(
        ON_RECEIVE,
        package.data,
        package.info.len as usize,
    );
    if socket.read(|socket| socket.output.close_wait) != 0 {
        ERR
    } else {
        OK
    }
}

/// The client of the reactor event's socket, None without a socket.
fn event_client(event: &Event) -> Option<Handle> {
    let socket = event.socket()?;
    Some(Handle(socket.read(|socket| socket.object).cast()))
}

extern "C" fn on_stream_read(_: Running<'_>, event: &mut Event) -> c_int {
    event_client(event).map_or(ERR, on_read)
}

extern "C" fn on_dgram_read(_: Running<'_>, event: &mut Event) -> c_int {
    event_client(event).map_or(ERR, on_datagram)
}

extern "C" fn on_writable(reactor: Running<'_>, event: &mut Event) -> c_int {
    event_client(event).map_or(ERR, |this| on_write(this, reactor, event))
}

extern "C" fn on_failure(reactor: Running<'_>, event: &mut Event) -> c_int {
    event_client(event).map_or(ERR, |this| on_error(this, reactor, event))
}

/// `Client::init_reactor`: the client sockets' handlers.
///
/// # Safety
/// `reactor` is the reactor being initialised.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_init_reactor(reactor: *mut c_void) {
    let handlers: [(c_int, c_int, swoole_rs_reactor::Handler); 4] = [
        (FD_STREAM_CLIENT, EVENT_READ, on_stream_read),
        (FD_DGRAM_CLIENT, EVENT_READ, on_dgram_read),
        (FD_STREAM_CLIENT, EVENT_WRITE, on_writable),
        (FD_STREAM_CLIENT, EVENT_ERROR, on_failure),
    ];
    for (fd_type, event, handler) in handlers {
        // SAFETY: as the caller promises.
        unsafe { reactor_set_handler(reactor, fd_type, event, Some(handler)) };
    }
}

/// `Client::onConnect` and the rest: `listener(object, event, ...)` for each
/// event in `events` (`1 << ON_*`), `object` the record's.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_listen(
    client: &Client,
    object: *mut c_void,
    events: u32,
    listener: Listener,
) {
    let this = Handle(core::ptr::from_ref(client).cast_mut());
    this.fields().object = object;
    this.get().listener.set(Some(listener));
    this.get().events.set(events);
}

use swoole_rs_corebase::sys::errno;

/// `sw_tg_buffer()`: a sync proxy handshake reads into its whole allocation.
fn shared_buffer() -> &'static mut CppString {
    // SAFETY: the thread's shared String, set up by `swoole_init` before any
    // proxied connect, and borrowed only between effects.
    unsafe { &mut *thread_buffer().cast::<CppString>() }
}

/// The shared buffer, emptied, as a sync proxy handshake's read target: the read
/// lands at its start.
fn shared_spare() -> &'static mut [core::mem::MaybeUninit<u8>] {
    let shared = shared_buffer();
    shared.clear();
    shared.spare()
}

use swoole_rs_corebase::set_errno;

fn is_stream(kind: c_int) -> bool {
    matches!(kind, 1 | 3 | 5)
}

fn is_dgram(kind: c_int) -> bool {
    matches!(kind, 2 | 4 | 6)
}

/// `new Client(type, async)`: the socket and the state, or null with errno set
/// when the socket could not be made (logged as upstream's constructor does).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_new(kind: c_int, is_async: bool) -> *mut Client {
    let fd_type = if is_stream(kind) {
        FD_STREAM_CLIENT
    } else {
        FD_DGRAM_CLIENT
    };
    let flags = if is_async { SOCK_NONBLOCK } else { 0 } | SOCK_CLOEXEC;
    let socket = ops::make_socket(kind, fd_type, flags);
    // SAFETY: null or the new socket record, which nothing else holds yet.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        let error = errno();
        log::sys_warning(c"Client", format_args!("socket() failed"));
        set_errno(error);
        return core::ptr::null_mut();
    };
    let mut client = Box::new(Client {
        // SAFETY: all-zero pointers, bools, addresses and Protocol are valid.
        record: Cell::new(unsafe { core::mem::zeroed() }),
        flags: Cell::new(0),
        domain: 0,
        sock_flags: Cell::new(0),
        server_host: RefCell::new(CString::default()),
        server_port: Cell::new(0),
        timer: Cell::new(core::ptr::null_mut()),
        high_watermark: Cell::new(0),
        low_watermark: Cell::new(0),
        listener: Cell::new(None),
        events: Cell::new(0),
        tls: OnceCell::new(),
        socks5: Cell::new(core::ptr::null_mut()),
        http_proxy: Cell::new(core::ptr::null_mut()),
    });
    client.set(ASYNC, is_async);
    let fields = client.record.get_mut();
    fields.socket = socket;
    fields.remote_addr.kind = kind;
    fields.protocol.package_length_type = b'N';
    fields.protocol.package_length_size = 4;
    fields.protocol.package_max_length = 2 * 1024 * 1024;
    fields.protocol.on_package = Some(on_package);
    let socket_type = record.edit(|record| {
        record.chunk_size = SEND_BUFFER_SIZE;
        record.socket_type
    });
    client.set(STREAM, is_stream(socket_type));
    let mut kind_out = 0;
    swr_v1_socket_domain_and_type(kind, &mut client.domain, &mut kind_out);
    let client = Box::into_raw(client);
    record.edit(|record| record.object = client.cast());
    client
}

/// One `network::Client` method for [`Handle::run`], over the caller's bytes.
#[derive(Debug)]
pub enum Op<'a> {
    Connect {
        host: &'a CStr,
        port: c_int,
        timeout: f64,
        flags: c_int,
    },
    Send {
        data: &'a [u8],
        flags: c_int,
    },
    /// One datagram to `host`, resolved unless the client is async.
    Sendto {
        host: &'a CStr,
        port: c_int,
        data: &'a [u8],
    },
    Sendfile {
        file: &'a CStr,
        offset: i64,
        length: usize,
    },
    Recv {
        buffer: &'a mut [u8],
        flags: c_int,
    },
    /// `protocol.get_package_length` of these bytes.
    PackageLength(&'a [u8]),
    Shutdown(c_int),
    Close,
    Sleep,
    Wakeup,
    SslHandshake,
    /// `ssl_verify(allow_self_signed)`.
    SslVerify(bool),
    /// `swoole_event_set(socket, SW_EVENT_WRITE)`, to finish a handshake.
    SslWait,
}

impl Handle {
    /// A handle on `client`.
    ///
    /// # Safety
    /// `client` came from [`swr_v1_clients_new`] and stays live while this handle
    /// or a copy of it is used.
    #[must_use]
    pub const unsafe fn new(client: *mut Client) -> Self {
        Self(client)
    }

    /// Run `op`, which may raise the client's callbacks (and so free it): the
    /// C++ method's result.
    #[must_use]
    pub fn run(self, op: Op<'_>) -> i64 {
        match op {
            Op::Connect {
                host,
                port,
                timeout,
                flags,
            } => connect(self, host, port, timeout, flags).into(),
            Op::Send { data, flags } => send(self, data, flags) as i64,
            Op::Sendto { host, port, data } => sendto(self, host, port, data).into(),
            Op::Sendfile {
                file,
                offset,
                length,
            } => sendfile(self, file, offset, length).into(),
            Op::Recv { buffer, flags } => recv(self, buffer, flags) as i64,
            Op::PackageLength(data) => package_length(self, data) as i64,
            Op::Shutdown(how) => shutdown(self, how).into(),
            Op::Close => close(self).into(),
            Op::Sleep => sleep(self).into(),
            Op::Wakeup => wakeup(self).into(),
            Op::SslHandshake => ssl_handshake(self).into(),
            Op::SslVerify(allow_self_signed) => ssl_verify(self, allow_self_signed).into(),
            Op::SslWait => self.event(EVENT_SET, EVENT_WRITE).into(),
        }
    }

    /// `enable_ssl_encrypt()`: the new TLS context, which the caller configures
    /// before any handshake; None when TLS was already on.
    #[must_use]
    pub fn enable_ssl(self) -> Option<NonNull<OwnedTlsContext>> {
        enable_ssl(self)
    }

    /// The client's TLS context, which `set()` configures; None while TLS is off.
    #[must_use]
    pub fn ssl_context(self) -> Option<NonNull<OwnedTlsContext>> {
        self.get()
            .tls
            .get()
            .and_then(|tls| NonNull::new(tls.as_ptr().cast_mut()))
    }

    /// `delete client`, which may raise `onClose`.
    ///
    /// # Safety
    /// Neither this handle nor a copy of it is used afterwards.
    pub unsafe fn free(self) {
        if !self.flag(CLOSED) {
            close(self);
        }
        self.input_free();
        // A pending connect timer would fire on a freed client.
        self.timer_clear();
        if self.flag(ASYNC) {
            // A cleared object tells a close in progress that the client is gone.
            self.socket()
                .edit(|socket| socket.object = core::ptr::null_mut());
            // SAFETY: the live client's socket, released once.
            unsafe { socket_free(self.socket_ptr()) };
        } else {
            // SAFETY: `delete socket`: the record make_socket allocated, released once.
            unsafe { SocketRecord::free(self.socket_ptr()) };
        }
        // SAFETY: a Box from swr_v1_clients_new, released once (per the caller).
        let client = unsafe { Box::from_raw(self.0) };
        for proxy in [&client.socks5, &client.http_proxy] {
            // SAFETY: a proxy the client owns, released once.
            unsafe { swr_v1_core_proxy_free(proxy.get()) };
        }
    }

    /// `Client::get_peer_name`: datagram peers are value snapshots of the last
    /// received address; streams ask the kernel through the socket's operation.
    pub fn peer_name(self, address: &mut SocketAddress) -> c_int {
        if is_dgram(self.socket_type()) {
            *address = self.fields().remote_addr;
            OK
        } else {
            self.socket().read(|socket| socket.get_peer_name(address))
        }
    }

    /// `Client::bind(host, port)`: `SO_REUSEADDR` (a failure is logged), then
    /// bind(2) to the host's literal address.
    #[must_use]
    pub fn bind(self, host: &CStr, port: c_int) -> c_int {
        let fd = self.socket().fd();
        let enable: c_int = 1;
        if swoole_rs_corebase::sys::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &enable)
            < 0
        {
            log::sys_warning(
                c"bind",
                format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
            );
        }
        let Some(address) = address_of(self.socket_type(), host, port, false) else {
            return ERR;
        };
        if self.socket().edit(|socket| socket.bind(&address)) < 0 {
            log::set_last_error(errno());
            return ERR;
        }
        OK
    }
}

/// The client protocol's length of the packet in `data`.
#[expect(clippy::cast_possible_truncation, reason = "`(uint32_t) length`")]
fn package_length(this: Handle, data: &[u8]) -> isize {
    let record = this.fields();
    let Some(get) = record.protocol.get_package_length else {
        return -1;
    };
    let mut packet = PacketLength {
        buf: data.as_ptr().cast(),
        buf_size: data.len() as u32,
        header_len: 0,
    };
    // SAFETY: the live client's protocol and socket, and the packet's bytes.
    unsafe {
        get(
            &raw const record.protocol,
            record.socket.cast(),
            &mut packet,
        )
    }
}

/// One of the flags above; false for any other.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_flag(client: &Client, name: u32) -> bool {
    client.flag(name)
}

/// `buffer_high_watermark` (`high`) or `buffer_low_watermark`: the address of the
/// value, which C++'s `Client` keeps as a reference for its lifetime.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_watermark(client: &Client, high: bool) -> *mut u32 {
    if high {
        client.high_watermark.as_ptr()
    } else {
        client.low_watermark.as_ptr()
    }
}

/// `Client::close()`: -1 without a socket or when already closed, 0 when `onClose`
/// released the client, else `close(2)`'s result.
fn close(this: Handle) -> c_int {
    if this.socket_ptr().is_null() || this.flag(CLOSED) {
        return ERR;
    }
    this.set(CLOSED, true);
    let saved = this.socket();
    if this.flag(OPEN_SSL)
        && this.get().tls.get().is_some()
        && !saved.read(|socket| socket.ssl().is_null())
    {
        socket_ssl_close(saved);
    }
    if this.socket_type() == SOCK_UNIX_DGRAM {
        let path = saved.read(|socket| socket.info.addr.unix().sun_path);
        let path = path.map(|byte| byte.to_ne_bytes()[0]);
        if let Ok(path) = CStr::from_bytes_until_nul(&path) {
            swoole_rs_corebase::sys::unlink(path);
        }
    }
    if this.flag(ASYNC) {
        if saved.read(|socket| socket.removed) == 0 {
            this.event(EVENT_DEL, 0);
        }
        this.timer_clear();
        if this.flag(ACTIVE) {
            this.set(ACTIVE, false);
            // onClose may destroy the client: only the saved socket is read after it.
            this.call(ON_CLOSE, core::ptr::null(), 0);
        }
    } else {
        this.set(ACTIVE, false);
    }
    // The saved record's free is deferred past this call.
    let Some(fd) = saved
        .edit(|socket| (!socket.object.is_null()).then(|| core::mem::replace(&mut socket.fd, -1)))
    else {
        return OK;
    };
    // Once: -1 marks the client's descriptor closed.
    swoole_rs_corebase::sys::close(fd)
}

/// `Client::shutdown(how)`. Upstream's `SHUT_RDWR` records only a read shutdown.
fn shutdown(this: Handle, how: c_int) -> c_int {
    if this.socket_ptr().is_null() || this.flag(CLOSED) {
        return ERR;
    }
    let fd = this.socket().fd();
    let call = |how| swoole_rs_corebase::sys::shutdown(fd, how);
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
            log::set_last_error(libc::EINVAL);
            ERR
        }
    }
}

/// `Client::sleep()`: stop reading (keeping write interest).
fn sleep(this: Handle) -> c_int {
    if this.socket().read(|socket| socket.events) & EVENT_WRITE != 0 {
        this.event(EVENT_SET, EVENT_WRITE)
    } else {
        this.event(EVENT_DEL, 0)
    }
}

/// `Client::wakeup()`: read again.
fn wakeup(this: Handle) -> c_int {
    if this.socket().read(|socket| socket.events) & EVENT_WRITE != 0 {
        this.event(EVENT_SET, EVENT_READ | EVENT_WRITE)
    } else {
        this.event(EVENT_ADD, EVENT_READ)
    }
}

/// `Client::enable_ssl_encrypt()`: a TLS context, TLS on, and DTLS for a datagram
/// client. The context, or None when TLS was already on.
fn enable_ssl(this: Handle) -> Option<NonNull<OwnedTlsContext>> {
    // A new context and the client's reference to it.
    if this.get().tls.set(TlsContext::new()).is_err() {
        return None;
    }
    this.set(OPEN_SSL, true);
    if is_dgram(this.socket_type()) {
        enable_dtls(this);
    }
    this.ssl_context()
}

/// `Client::set_http_proxy` (`http`) or `set_socks5_proxy`: a new proxy, which
/// replaces the client's. A SOCKS5 proxy whose credentials are too long (logged)
/// clears it.
///
/// # Safety
/// `client` is live with a socket; each text lends its bytes for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_proxy(
    client: *mut Client,
    http: bool,
    host: *const c_char,
    host_length: usize,
    port: i64,
    user: *const c_char,
    user_length: usize,
    password: *const c_char,
    password_length: usize,
) {
    let this = Handle(client);
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int port")]
    let port = port as c_int;
    let which = if http { Proxy::Http } else { Proxy::Socks5 };
    let [host, user, password] = [
        (host, host_length),
        (user, user_length),
        (password, password_length),
    ]
    .map(|(text, length)| {
        // SAFETY: each text lends `length` bytes for the call.
        unsafe { core::slice::from_raw_parts(text.cast::<u8>(), length) }
    });
    let proxy = if http {
        Box::into_raw(swoole_rs_http_ffi::http_proxy(host, port, user, password))
    } else {
        // SAFETY: a length check that logs.
        match unsafe { socks5_admit(user.len(), password.len()) } {
            0 => core::ptr::null_mut(),
            config => {
                let owner = ProxyState::new(true);
                let texts = [host, user, password];
                owner.configure(texts, port, config == 2, this.socket_type());
                Box::into_raw(owner)
            }
        }
    };
    let previous = this.get().proxy_slot(which).replace(proxy);
    // SAFETY: the replaced proxy, released once.
    unsafe { swr_v1_core_proxy_free(previous) };
}

/// The proxy's fields (`http` or SOCKS5), null without one.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_proxy_fields(
    client: &Client,
    http: bool,
) -> *mut swoole_rs_corebase::ProxyFields {
    let which = if http { Proxy::Http } else { Proxy::Socks5 };
    Handle(core::ptr::from_ref(client).cast_mut())
        .proxy(which)
        .map_or(core::ptr::null_mut(), ProxyState::fields_ptr)
}

/// The client's `SSLContext`, which crates/tls configures; null while TLS is off.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_ssl_context(client: &Client) -> *const OwnedTlsContext {
    client
        .tls
        .get()
        .map_or(core::ptr::null(), TlsContext::as_ptr)
}

/// `Client::enable_dtls()`: the context's protocols become DTLS.
fn enable_dtls(this: Handle) {
    this.tls_fields().protocols = SSL_DTLS;
    this.socket().edit(|socket| {
        socket.dtls = 1;
        socket.chunk_size = SSL_BUFFER_SIZE;
    });
    this.set(DTLS, true);
}

/// `Client::ssl_handshake()`.
fn ssl_handshake(this: Handle) -> c_int {
    if this.tls_state() == SSL_STATE_READY {
        return ERR;
    }
    let tls = this
        .get()
        .tls
        .get()
        .map_or(core::ptr::null(), TlsContext::as_ptr);
    if this.tls_fields().context.is_null() {
        this.tls_fields().http_v2 = u8::from(this.fields().http2);
        // SAFETY: the client's live context.
        if !unsafe { ssl_context_create(tls) } {
            return ERR;
        }
    }
    if this.socket().read(|socket| socket.ssl().is_null()) {
        this.socket().edit(|socket| socket.output.ssl_send = 1);
        // SAFETY: the live socket and context.
        if unsafe { socket_ssl_create(this.socket_ptr(), tls, SSL_CLIENT) } < 0 {
            log::set_last_error(ERROR_SSL_CREATE_SESSION_FAILED);
            return ERR;
        }
        let name = this.tls_host_name();
        if !name.is_empty() {
            // SAFETY: `SSL_set_tlsext_host_name` on the new session; OpenSSL copies the name.
            unsafe {
                ssl_ctrl(
                    this.socket().read(swoole_rs_reactor::SocketFields::ssl),
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    TLSEXT_NAMETYPE_HOST_NAME,
                    name.as_ptr().cast_mut().cast(),
                )
            };
        }
    }
    if socket_ssl_connect(this.socket()) < 0 {
        return ERR;
    }
    if this.tls_state() == SSL_STATE_READY {
        let fields = this.tls_fields();
        if fields.verify_peer != 0 && ssl_verify(this, fields.allow_self_signed != 0) < 0 {
            return ERR;
        }
    }
    OK
}

/// `Client::ssl_verify(allow_self_signed)`: the peer's certificate, and its host
/// when the context names one.
fn ssl_verify(this: Handle, allow_self_signed: bool) -> c_int {
    if !socket_ssl_verify(this.socket(), allow_self_signed) {
        return ERR;
    }
    let name = this.tls_host_name();
    if !name.is_empty() && !socket_ssl_check_host(this.socket(), name) {
        return ERR;
    }
    OK
}

/// `Client_inet_addr`: the first call routes through the proxies and records the
/// server; every call assigns `server_addr`, or waits for DNS.
fn inet_addr(this: Handle, host: &CStr, port: c_int) -> c_int {
    let (mut host, mut port) = (host, port);
    // The last proxy's host, NUL-terminated, which `host` borrows.
    let mut proxy_host;
    if !this.flag(HOST_PRESET) {
        for which in [Proxy::Socks5, Proxy::Http] {
            let Some(proxy) = this.proxy(which) else {
                continue;
            };
            proxy.set_string(3, host.to_bytes());
            proxy.update(|fields| fields.target_port = port);
            proxy_host = proxy.string(0).to_vec();
            proxy_host.push(0);
            let proxy_host = CStr::from_bytes_until_nul(&proxy_host).unwrap_or_default();
            (host, port) = (proxy_host, proxy.fields().port);
        }
        host.clone_into(&mut this.get().server_host.borrow_mut());
        this.get().server_port.set(port);
        this.set(HOST_PRESET, true);
    }
    let resolve = !this.flag(ASYNC);
    if !ops::assign(this, host, port, resolve) {
        if !this.flag(DNS_COMPLETED) && log::last_error() == ERROR_BAD_HOST_ADDR {
            this.set(WAIT_DNS, true);
        } else {
            return ERR;
        }
    }
    OK
}

/// Dispatches a `GethostbynameRequest` for the server host to the async pool,
/// completing in [`resolved`].
fn dns_lookup(this: Handle) -> c_int {
    let request = {
        let host = this.get().server_host.borrow();
        crate::resolver::Request::new(this.get().domain, 0, 0, host.to_bytes(), b"")
    };
    let event = AsyncEvent {
        task_id: 0,
        canceled: 0,
        error: 0,
        data: request.as_ptr(),
        retval: 0,
        pipe_socket: core::ptr::null_mut(),
        timestamp: 0.0,
        object: this.0.cast(),
        handler: Some(resolve),
        callback: Some(resolve_completed),
    };
    // The pool's copy holds its own reference to the request.
    if async_dispatch(&event).is_null() {
        ERR
    } else {
        OK
    }
}

/// `async::handler_gethostbyname`, on a pool thread, over a copy of the event
/// [`dns_lookup`] dispatched.
extern "C" fn resolve(event: &mut AsyncEvent) {
    // SAFETY: its request record, held by the copy.
    (event.retval, event.error) = crate::resolver::host(unsafe { &*event.data });
}

/// `Client_onResolveCompleted`, on the client's thread, over the completed copy
/// of the event [`dns_lookup`] dispatched.
extern "C" fn resolve_completed(event: &AsyncEvent) {
    // SAFETY: its request record, held by the copy.
    let address = crate::resolver::address(unsafe { &*event.data });
    resolved(
        Handle(event.object.cast()),
        event.error,
        &crate::resolver::c_name(&address),
    );
}

/// `Client::connect(host, port, timeout, flags)`: `host` is NUL-terminated.
fn connect(this: Handle, host: &CStr, port: c_int, timeout: f64, flags: c_int) -> c_int {
    if !this.flag(STREAM) {
        udp_connect(this, host, port, timeout, flags)
    } else if this.flag(ASYNC) {
        tcp_connect_async(this, host, port, timeout)
    } else {
        tcp_connect_sync(this, host, port, timeout, flags != 0)
    }
}

/// `Client_tcp_connect_sync`: connect, then the proxy handshake and TLS.
fn tcp_connect_sync(this: Handle, host: &CStr, port: c_int, timeout: f64, nonblock: bool) -> c_int {
    this.set_timeout(timeout);
    if timeout > 0.0 {
        ops::kernel_timeout(this, timeout);
    }
    if inet_addr(this, host, port) < 0 {
        return ERR;
    }
    if nonblock {
        let rc = ops::connect(this, CONNECT_ASYNC);
        if rc == SW_READY {
            return OK;
        }
        if rc == SW_WAIT {
            this.set(ASYNC_CONNECT, true);
        }
        return ERR;
    }
    let ret = ops::connect(this, CONNECT_SYNC);
    if ret < 0 {
        return ret;
    }
    this.set(ACTIVE, true);
    // Only a proxy handshake reads the thread's shared buffer, which a thread
    // that never set up SwooleTG doesn't have.
    if this.proxy_state(Proxy::Socks5) >= 0 {
        if this.socks5_send(this.socks5_negotiate()) < 0 {
            return ERR;
        }
        this.set_proxy_state(Proxy::Socks5, SOCKS5_HANDSHAKE);
        loop {
            let shared = shared_spare();
            let n = recv(this, shared, 0);
            if n > 0 && this.socks5_handshake(shared.as_ptr().cast(), n.cast_unsigned()) {
                if this.proxy_state(Proxy::Socks5) == SOCKS5_READY {
                    break;
                }
                continue;
            }
            return ERR;
        }
    } else if this.proxy_state(Proxy::Http) >= 0 {
        if send(this, this.http_pack(), 0) < 0 {
            return ERR;
        }
        // After the pack, which may have grown it.
        let n = recv(this, shared_spare(), 0);
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
fn tcp_connect_async(this: Handle, host: &CStr, port: c_int, timeout: f64) -> c_int {
    if ![ON_CONNECT, ON_ERROR, ON_CLOSE, ON_RECEIVE]
        .into_iter()
        .all(|which| this.has(which))
    {
        log::warning(
            c"Client_tcp_connect_async",
            format_args!("onConnect/onError/onReceive/onClose callback have not set"),
        );
        return ERR;
    }
    this.set_timeout(timeout);
    this.input_init();
    if this.has(ON_BUFFER_FULL) && this.get().high_watermark.get() == 0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream's buffer_size * 0.8 into a uint32_t"
        )]
        let mark = (f64::from(this.socket().read(|socket| socket.output.buffer_size)) * 0.8) as u32;
        this.get().high_watermark.set(mark);
    }
    if inet_addr(this, host, port) < 0 {
        return ERR;
    }
    if this.flag(WAIT_DNS) {
        return dns_lookup(this);
    }
    let ret = loop {
        let ret = ops::connect(this, CONNECT_RAW);
        let error = errno();
        if ret < 0 && error == libc::EINTR {
            continue;
        }
        if ret == 0 || error == libc::EINPROGRESS {
            // A finished connect needs no timeout timer; either way the write
            // handler runs the proxy and TLS handshakes.
            if ret < 0 && timeout > 0.0 {
                // SAFETY: `on_timeout` takes the live client, which deletes the
                // timer before it goes.
                let timer = unsafe {
                    swr_v1_timer_add_plain_sec(timeout, false, on_timeout, this.0.cast())
                };
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
    this.mark_removed();
    close(this);
    if this.has(ON_ERROR) {
        this.set(ONERROR_CALLED, true);
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
    ret
}

/// `Client_udp_connect`.
fn udp_connect(this: Handle, host: &CStr, port: c_int, timeout: f64, udp_connect: c_int) -> c_int {
    let is_async = this.flag(ASYNC);
    this.set_timeout(timeout);
    if !is_async && timeout > 0.0 {
        ops::kernel_timeout(this, timeout);
    }
    this.get().sock_flags.set(udp_connect);
    if inet_addr(this, host, port) < 0 {
        return ERR;
    }
    if is_async && !this.has(ON_RECEIVE) {
        log::warning(
            c"Client_udp_connect",
            format_args!("`onReceive` callback have not set"),
        );
        return ERR;
    }
    if this.flag(WAIT_DNS) && is_async {
        // Without onError a failed lookup would go unreported.
        if !this.has(ON_ERROR) {
            log::warning(
                c"Client_udp_connect",
                format_args!("`onError` callback have not set"),
            );
            return ERR;
        }
        return dns_lookup(this);
    }
    this.set(ACTIVE, true);
    let fd = this.socket().fd();
    let default_size = SOCKET_DEFAULTS.buffer_size();
    swr_v1_socket_buffer_size(fd, true, true, default_size);
    if timeout > 0.0 {
        this.set_timeout(timeout);
    }
    if this.socket_type() == SOCK_UNIX_DGRAM && bind_unix(this) < 0 {
        return ERR;
    }
    let mut udp_connect = udp_connect;
    if this.flag(OPEN_SSL) {
        udp_connect = 1;
        enable_dtls(this);
    }
    if udp_connect == 1 {
        if ops::connect(this, CONNECT_RAW) != 0 {
            this.set(ACTIVE, false);
            this.mark_removed();
            close(this);
            if is_async && this.has(ON_ERROR) {
                this.set(ONERROR_CALLED, true);
                this.call(ON_ERROR, core::ptr::null(), 0);
            }
            return ERR;
        }
        fd_clean(fd);
    }
    if is_async && this.has(ON_CONNECT) {
        if this.event(EVENT_ADD, EVENT_READ) < 0 {
            return ERR;
        }
        this.on_connect();
    }
    if this.flag(OPEN_SSL) && ssl_handshake(this) < 0 {
        return ERR;
    }
    OK
}

/// Binds a Unix datagram client to `/tmp/swoole-client.<pid>.<fd>.sock`, its
/// address kept in the socket's `info` for the unlink on close; warns on failure.
fn bind_unix(this: Handle) -> c_int {
    let socket = this.socket();
    let pid = std::process::id();
    let path = format!("/tmp/swoole-client.{pid}.{}.sock", socket.fd());
    let bound = std::ffi::CString::new(path.as_str()).unwrap_or_default();
    let ret = socket.edit(|socket| {
        let address = socket.info.addr.unix_mut();
        for (slot, byte) in address.sun_path.iter_mut().zip(path.bytes().chain([0])) {
            *slot = c_char::from_ne_bytes([byte]);
        }
        #[expect(clippy::cast_possible_truncation, reason = "AF_UNIX fits sa_family_t")]
        let family = libc::AF_UNIX as libc::sa_family_t;
        address.sun_family = family;
        swoole_rs_corebase::sys::unlink(&bound);
        #[expect(clippy::cast_possible_truncation, reason = "sizeof(sockaddr_un)")]
        let length = size_of::<libc::sockaddr_un>() as libc::socklen_t;
        // SAFETY: binds the client's descriptor to its own address storage.
        unsafe { libc::bind(socket.fd, (&raw const *address).cast(), length) }
    });
    if ret < 0 {
        log::sys_warning(c"Client_udp_connect", format_args!("bind({path}) failed"));
    }
    ret
}

/// The socket record operations: crates/reactor's, which the unit tests replace
/// with recording doubles.
#[cfg(not(test))]
mod ops {
    use super::{
        CONNECT_ASYNC, CONNECT_SYNC, EVENT_ADD, EVENT_DEL, EVENT_SET, Handle, SocketRecord,
        address_of,
    };
    use core::ffi::{CStr, c_int};
    use swoole_rs_reactor::{event_api, socket_upstream};

    /// `make_socket(type, fd_type, flags)`: the record or null.
    pub(super) fn make_socket(kind: c_int, fd_type: c_int, flags: c_int) -> *mut SocketRecord {
        socket_upstream::make_socket_typed(kind, fd_type, flags)
    }

    /// `socket->set_kernel_timeout(timeout)`, the write timeout only once the
    /// read one is set.
    pub(super) fn kernel_timeout(this: Handle, timeout: f64) {
        this.socket().edit(|socket| {
            if socket_upstream::socket_set_kernel_read_timeout(socket, timeout) {
                socket_upstream::socket_set_kernel_write_timeout(socket, timeout);
            }
        });
    }

    /// `server_addr.assign(socket_type, host, port, resolve)`.
    pub(super) fn assign(this: Handle, host: &CStr, port: c_int, resolve: bool) -> bool {
        let Some(address) = address_of(this.socket_type(), host, port, resolve) else {
            return false;
        };
        this.fields().server_addr = address;
        true
    }

    /// Connects the socket to `server_addr`, by `mode` (`CONNECT_*`); errno is the call's.
    pub(super) fn connect(this: Handle, mode: c_int) -> c_int {
        let socket = this.socket();
        let address = &this.fields().server_addr;
        match mode {
            CONNECT_ASYNC => {
                socket.edit(|socket| socket_upstream::socket_connect_async(socket, address))
            }
            CONNECT_SYNC => socket_upstream::connect_sync(socket, address),
            _ => address.connect(socket.fd()),
        }
    }

    /// The reactor's `swoole_event_*` (`EVENT_*`) on the socket.
    pub(super) fn event(this: Handle, op: c_int, events: c_int) -> c_int {
        let socket = this.socket_ptr();
        match op {
            // SAFETY: the live socket on the thread's reactor.
            EVENT_ADD => unsafe { event_api::swoole_event_add(socket, events) },
            // SAFETY: as above.
            EVENT_SET => unsafe { event_api::swoole_event_set(socket, events) },
            // SAFETY: as above.
            EVENT_DEL => unsafe { event_api::swoole_event_del(socket) },
            // SAFETY: as above.
            _ => unsafe { event_api::swoole_event_add_or_update(socket, events) },
        }
    }

    /// `getsockopt(SOL_SOCKET, SO_ERROR)` into `error`.
    pub(super) fn socket_error(this: Handle, error: &mut c_int) -> c_int {
        let fd = this.socket().fd();
        swoole_rs_corebase::sys::getsockopt_int(fd, libc::SOL_SOCKET, libc::SO_ERROR, error)
    }
}

/// An address of socket type `kind` for `host`, resolved if `resolve`;
/// `None` with the last error set when it does not assign.
fn address_of(kind: c_int, host: &CStr, port: c_int, resolve: bool) -> Option<SocketAddress> {
    let mut address = SocketAddress {
        kind,
        ..SocketAddress::default()
    };
    let resolve = resolve
        .then_some(swoole_rs_corebase::swr_v1_clients_gethostbyname as crate::address::Resolve);
    // SAFETY: the local record's union and length, and the NUL-terminated host.
    let error = unsafe {
        crate::address::swr_v1_clients_address_assign(
            (&raw mut address.addr).cast(),
            &raw mut address.len,
            address.kind,
            host.as_ptr(),
            host.to_bytes().len(),
            port,
            resolve,
        )
    };
    if error != 0 {
        log::set_last_error(error);
        return None;
    }
    Some(address)
}

/// `Client::sendto(host, port, data, length)`: one datagram to the host, resolved
/// unless the client is async.
fn sendto(this: Handle, host: &CStr, port: c_int, data: &[u8]) -> c_int {
    if !is_dgram(this.socket_type()) {
        log::set_last_error(ERROR_OPERATION_NOT_SUPPORT);
        log::warning(
            c"sendto",
            format_args!("only supports SWOOLE_SOCK_(UDP/UDP6/UNIX_DGRAM)"),
        );
        return ERR;
    }
    let Some(address) = address_of(this.socket_type(), host, port, !this.flag(ASYNC)) else {
        return ERR;
    };
    let fd = this.socket().fd();
    if address.sendto(fd, data, 0) < 0 {
        log::set_last_error(errno());
        return ERR;
    }
    OK
}

/// `Client::send(data, length, flags)`.
fn send(this: Handle, data: &[u8], flags: c_int) -> isize {
    if this.flag(DTLS) || this.flag(STREAM) && !this.flag(ASYNC) {
        net::send_sync(this.socket(), data, flags)
    } else if this.flag(STREAM) {
        send_async(this, data)
    } else {
        let n = this.sendto(data);
        if n < 0 || n.cast_unsigned() < data.len() {
            ERR as isize
        } else {
            n
        }
    }
}

/// `Client_tcp_send_async`: queue on the reactor, raising `onBufferFull` at the high watermark.
fn send_async(this: Handle, data: &[u8]) -> isize {
    let (at, length) = (data.as_ptr().cast(), data.len());
    let mut n = length.cast_signed();
    // SAFETY: the live client's socket; the data's bytes, copied by the reactor.
    if unsafe { swoole_event_write(this.socket_ptr(), at, length) } < 0 {
        if log::last_error() != ERROR_OUTPUT_BUFFER_OVERFLOW {
            return ERR as isize;
        }
        n = -1;
        this.set(HIGH_WATERMARK, true);
    }
    if this.has(ON_BUFFER_FULL)
        && !this.flag(HIGH_WATERMARK)
        && this.queued() >= this.get().high_watermark.get()
    {
        this.set(HIGH_WATERMARK, true);
        this.call(ON_BUFFER_FULL, core::ptr::null(), 0);
    }
    n
}

/// `Client::sendfile(file, offset, length)`.
fn sendfile(this: Handle, file: &CStr, offset: i64, length: usize) -> c_int {
    if !this.flag(STREAM) {
        return ERR;
    }
    let is_async = this.flag(ASYNC);
    let socket = this.socket_ptr();
    let file = file.as_ptr();
    let sent = if is_async {
        // SAFETY: the live client's socket and a NUL-terminated path.
        unsafe { socket_sendfile_async(socket, file, offset, length) }
    } else {
        // SAFETY: as above.
        unsafe { socket_sendfile_sync(socket, file, offset, length) }
    };
    if sent < 0 {
        log::set_last_error(errno());
        return ERR;
    }
    if is_async && this.event(EVENT_ADD_OR_UPDATE, EVENT_WRITE) == ERR {
        return ERR;
    }
    OK
}

/// `Client::recv(data, length, flags)`.
fn recv(this: Handle, buffer: &mut [impl ReadSlot], flags: c_int) -> isize {
    if this.flag(STREAM) || this.flag(DTLS) {
        let n = net::recv_sync(this.socket(), buffer, flags);
        // A poll timeout reads as recv(2)'s EAGAIN, as it did before poll.
        if n == -1 && log::last_error() == ERROR_SOCKET_POLL_TIMEOUT {
            set_errno(libc::EAGAIN);
        }
        n
    } else if this.flag(ASYNC) {
        this.recvfrom_sync(buffer, flags)
    } else {
        this.recvfrom(buffer, flags)
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
/// then framed or plain reads, on a live async stream client with its input buffer.
fn on_read(this: Handle) -> c_int {
    let Tail { str: buf, size } = this.input_tail();
    let do_ssl_handshake = this.flag(OPEN_SSL);
    let http = this.proxy_state(Proxy::Http);
    if http >= 0 && http != HTTP_PROXY_READY {
        let n = this.recv_once(this.input_mut().spare());
        if n <= 0 || !this.http_handshake(false, n.cast_unsigned()) {
            log::set_last_error(ERROR_HTTP_PROXY_HANDSHAKE_ERROR);
            return connect_failed(this);
        }
        this.set_proxy_state(Proxy::Http, HTTP_PROXY_READY);
        this.input_clear();
        if !do_ssl_handshake {
            this.on_connect();
            return OK;
        }
    }
    let socks5 = this.proxy_state(Proxy::Socks5);
    if socks5 >= 0 && socks5 != SOCKS5_READY {
        let n = this.recv_once(this.input_mut().spare());
        if n <= 0 {
            log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        this.input_grow(n.cast_unsigned());
        // Upstream hands the handshake the buffer's free size, not what was read.
        if !this.socks5_handshake(buf, size) {
            log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.proxy_state(Proxy::Socks5) != SOCKS5_READY {
            return OK;
        }
        this.input_clear();
        if !do_ssl_handshake {
            this.on_connect();
            return OK;
        }
    }
    if this.flag(OPEN_SSL) && this.tls_state() != SSL_STATE_READY {
        if ssl_handshake(this) < 0 {
            log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.tls_state() == SSL_STATE_READY {
            this.on_connect();
        }
        return OK;
    }
    // No reference crosses the receiving callback, which may destroy the client.
    let fields = this.fields();
    let (eof, length) = (fields.open_eof_check, fields.open_length_check);
    if eof || length {
        // `swoole_init` registered them before any client existed.
        let framing = FRAMING.get().unwrap_or_else(|| std::process::abort());
        let framed = if eof { framing.eof } else { framing.length };
        let socket = this.socket();
        let protocol = &raw const this.fields().protocol;
        // The live protocol, socket and input buffer `Framing::new` asks for. A
        // package callback may destroy the client; its socket outlives this handler.
        let n = framed(protocol, socket.as_ptr(), this.input());
        // A destroyed client cleared its object.
        let released = socket.read(|socket| socket.object.is_null());
        if n < 0 && !released && !this.flag(CLOSED) {
            close(this);
        }
        return OK;
    }
    let n = this.recv_once(this.input_mut().spare());
    if n < 0 {
        let socket = this.socket();
        return match socket.catch_error(errno()) {
            SW_ERROR => {
                let fd = socket.fd();
                log::sys_warning(
                    c"Client_onStreamRead",
                    format_args!("Read from socket[{fd}] failed"),
                );
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

/// `Client_onDgramRead`, on a live async datagram client.
fn on_datagram(this: Handle) -> c_int {
    #[expect(
        clippy::large_stack_arrays,
        reason = "upstream's stack buffer: nothing to drop if the callback longjmps"
    )]
    let mut buffer = [0_u8; BUFFER_SIZE_UDP];
    // Upstream's datagram receive, even once DTLS made `recv` a stream one.
    let n = if this.flag(ASYNC) {
        this.recvfrom_sync(&mut buffer, 0)
    } else {
        this.recvfrom(&mut buffer, 0)
    };
    if n < 0 {
        return ERR;
    }
    this.call(ON_RECEIVE, buffer.as_ptr().cast(), n.cast_unsigned());
    OK
}

/// `Client_onWrite`: drain queued output, or finish an async connect, on a live
/// async stream client; `reactor` and `event` are the handler's.
fn on_write(this: Handle, reactor: Running<'_>, event: &mut Event) -> c_int {
    if this.flag(ACTIVE) {
        if this.flag(OPEN_SSL) && this.tls_state() == SSL_STATE_WAIT_STREAM {
            if ssl_handshake(this) < 0 {
                log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
                return write_failed(this);
            }
            if this.tls_state() == SSL_STATE_READY {
                this.on_connect();
                return OK;
            }
            if this.socket().read(|socket| socket.tls.want_read) != 0 {
                this.event(EVENT_SET, EVENT_READ);
            }
            return OK;
        }
        // SAFETY: the handler's reactor and event.
        if unsafe { writable_callback(reactor.as_ptr(), event) } < 0 {
            return ERR;
        }
        if this.has(ON_BUFFER_EMPTY)
            && this.flag(HIGH_WATERMARK)
            && this.queued() <= this.get().low_watermark.get()
        {
            this.set(HIGH_WATERMARK, false);
            this.call(ON_BUFFER_EMPTY, core::ptr::null(), 0);
        }
        return OK;
    }
    let mut error = 0;
    let ret = ops::socket_error(this, &mut error);
    log::set_last_error(error);
    if ret < 0 {
        let fd = this.socket().fd();
        log::sys_warning(c"Client_onWrite", format_args!("getsockopt({fd}) failed"));
        return ERR;
    }
    if error != 0 {
        return write_failed(this);
    }
    this.event(EVENT_SET, EVENT_READ);
    this.set(ACTIVE, true);
    if this.proxy_state(Proxy::Socks5) == SOCKS5_WAIT {
        let length = this.socks5_negotiate();
        this.set_proxy_state(Proxy::Socks5, SOCKS5_HANDSHAKE);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream returns the send result as an int"
        )]
        return this.socks5_send(length) as c_int;
    }
    if this.proxy_state(Proxy::Http) == HTTP_PROXY_WAIT {
        // A copy: the async send may raise `onBufferFull`, whose callback may
        // reuse the shared buffer the request was packed into.
        let request = this.http_pack().to_vec();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream returns the send result as an int"
        )]
        return send(this, &request, 0) as c_int;
    }
    if this.flag(OPEN_SSL) {
        if ssl_handshake(this) < 0 {
            log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return write_failed(this);
        }
        this.socket()
            .edit(|socket| socket.tls.state = SSL_STATE_WAIT_STREAM);
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

/// `Client_onError`, as [`on_write`].
fn on_error(this: Handle, reactor: Running<'_>, event: &mut Event) -> c_int {
    if this.flag(ACTIVE) {
        return close(this);
    }
    on_write(this, reactor, event);
    OK
}

/// `Client_onTimeout`: the async connect of the client at `data` timed out.
///
/// # Safety
/// `data` is a live client.
unsafe extern "C-unwind" fn on_timeout(data: *mut c_void) {
    let this = Handle(data.cast());
    log::set_last_error(libc::ETIMEDOUT);
    this.get().timer.set(core::ptr::null_mut());
    if this.flag(OPEN_SSL) && this.tls_state() != SSL_STATE_READY {
        this.set(ACTIVE, false);
    }
    let socks5 = this.proxy_state(Proxy::Socks5);
    let http = this.proxy_state(Proxy::Http);
    if socks5 >= 0 && socks5 != SOCKS5_READY || http >= 0 && http != HTTP_PROXY_READY {
        this.set(ACTIVE, false);
    }
    close(this);
    if this.has(ON_ERROR) {
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
}

/// Connects to the resolved `address`, or reports the failure through `onError`.
/// `this` is a live async client waiting for DNS.
fn resolved(this: Handle, error: c_int, address: &CStr) {
    this.set(WAIT_DNS, false);
    this.set(DNS_COMPLETED, true);
    if error == 0 {
        // The application can't see connect's result here, so onError reports it.
        let timeout = this.socket().read(|socket| socket.timeouts.connect);
        let (port, flags) = (this.get().server_port.get(), this.get().sock_flags.get());
        if connect(this, address, port, timeout, flags) != ERR || this.flag(ONERROR_CALLED) {
            return;
        }
    } else {
        log::set_last_error(ERROR_DNSLOOKUP_RESOLVE_FAILED);
    }
    this.mark_removed();
    close(this);
    if this.has(ON_ERROR) {
        this.set(ONERROR_CALLED, true);
        this.call(ON_ERROR, core::ptr::null(), 0);
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
