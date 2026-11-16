//! `network::Client` (upstream's `src/network/client.cc`) in Rust: its state and every
//! operation over it, from construction to close, with its callbacks (one
//! listener closure), its reactor handlers, its connect timer, its TLS
//! context, its proxies and its DNS lookup. The public POD fields are
//! [`ClientRecord`], at the state's start, so a client is also its record. The
//! socket is the reactor's record, read and written here directly; its `object`
//! is this state.
//!
//! A user callback can destroy the client (and this state with it), so no
//! reference to the state lives across one: operations work on a raw [`Handle`]
//! and borrow the state only between effects. A PHP bailout in a callback
//! longjmps out, so nothing that needs dropping is held across one.
use crate::protocol::Protocol;
use core::cell::{Cell, OnceCell};
use core::ffi::{c_char, c_int, c_void};
use core::ptr::NonNull;
use std::ffi::{CStr, CString};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::constants::{
    FdType, ResultCode, SW_BUFFER_SIZE_UDP, SW_CLIENT_BUFFER_SIZE, SW_SEND_BUFFER_SIZE,
    SW_SOCKET_RETRY_COUNT, SW_SSL_BUFFER_SIZE, SocketFlag, SocketType,
};
use swoole_rs_corebase::net_types::{PacketLength, RecvData, SocketAddress};
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_corebase::{ProxyState, ResolverRequest, log};
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::reactor_upstream::set_handler;
use swoole_rs_reactor::socket_upstream::Connect;
use swoole_rs_reactor::{
    Caught, SOCKET_DEFAULTS, Socket, SocketRecord, fd_clean, set_socket_buffer_size,
    socket_domain_and_type,
};
use swoole_rs_reactor::{Event, Events, Running};
#[cfg(not(test))]
use swoole_rs_tls::upstream as net;
use swoole_rs_tls::{
    OwnedTlsContext, OwnedTlsFields, SslCreateFlag, SslState, SslVersion, TlsContext, TlsSettings,
};
#[cfg(test)]
use tests::net;

/// The flags [`Client::flag`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Flag {
    Active,
    Closed,
    Async,
    ShutdownRead,
    ShutdownWrite,
    AsyncConnect,
    WaitDns,
    OnerrorCalled,
    OpenSsl,
    DnsCompleted,
    HostPreset,
    HighWatermark,
    /// `enable_dtls()` ran: send and receive as a stream client does.
    Dtls,
    Stream,
}

impl Flag {
    const fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// The events a [`Listener`] hears; [`listen`]'s mask holds [`ClientEvent::bit`] of
/// each it wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ClientEvent {
    Connect = 1,
    Receive,
    Close,
    Error,
    BufferFull,
    BufferEmpty,
}

impl ClientEvent {
    /// The event's bit in [`listen`]'s mask.
    #[must_use]
    pub const fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// The client's callbacks: `listener(event, data, length)`, with `data`
/// holding the `length` received bytes for [`ClientEvent::Receive`] (lent for the
/// call: the listener may free the client, and with it the buffer), and the last
/// error as `length` for [`ClientEvent::Error`].
pub type Listener = Rc<dyn Fn(ClientEvent, *const c_char, usize)>;

/// The socket's blocking connect modes: `connect_sync` and `connect(2)`.
#[derive(Clone, Copy, Debug)]
enum ConnectMode {
    Sync,
    Raw,
}

/// The reactor's `swoole_event_*` operations on the socket.
#[derive(Clone, Copy, Debug)]
enum EventOp {
    Add,
    Set,
    Del,
    AddOrUpdate,
}

/// Swoole's constants this file uses.
const OK: c_int = ResultCode::SW_OK as c_int;
const ERR: c_int = ResultCode::SW_ERR as c_int;
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
/// crates/tls's `SW_SSL_STATE_READY`, `SW_SSL_STATE_WAIT_STREAM`, `SW_SSL_CLIENT`
/// and `SW_SSL_DTLS`.
pub use SslState::{
    SW_SSL_STATE_READY as SSL_STATE_READY, SW_SSL_STATE_WAIT_STREAM as SSL_STATE_WAIT_STREAM,
};
pub const SSL_CLIENT: c_int = SslCreateFlag::SW_SSL_CLIENT as c_int;
pub const SSL_DTLS: u32 = SslVersion::SW_SSL_DTLS as u32;
/// The proxy states of upstream's `include/swoole_proxy.h`.
pub const SOCKS5_WAIT: u8 = 0;
pub const SOCKS5_HANDSHAKE: u8 = 1;
pub const SOCKS5_READY: u8 = 4;
pub const HTTP_PROXY_WAIT: u8 = 0;
pub const HTTP_PROXY_READY: u8 = 2;
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

/// One of `Protocol::recv_with_length_protocol` and `recv_with_eof_protocol` over a
/// live protocol, socket and input buffer.
pub type Framed = fn(*const Protocol, Socket<'_>, *mut CppString) -> c_int;

/// The framed receives crates/ports defines; ports depends on this crate, so
/// `swoole_init` (crates/ffi) registers them with [`set_host`]. Only a
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

struct Host {
    framing: Framing,
    dispatch_dns: fn(&AsyncEvent<'_>) -> bool,
}

static HOST: OnceLock<Host> = OnceLock::new();

/// Registers framed receives and the DNS pool dispatch; later registrations are
/// ignored. A successful dispatch holds the request until its completion returns.
pub fn set_host(framing: Framing, dispatch_dns: fn(&AsyncEvent<'_>) -> bool) {
    HOST.get_or_init(|| Host {
        framing,
        dispatch_dns,
    });
}

fn native_host() -> &'static Host {
    HOST.get()
        .unwrap_or_else(|| swoole_rs_corebase::fatal("client host is not registered"))
}

// crates/protocols-ffi's definitions (the SOCKS5 codec), which the hybrid links (the
// unit tests define doubles). crates/protocols-ffi sits above this one, so it reaches
// them by symbol.
unsafe extern "C" {
    /// `Socks5Proxy::admit`: 0 (logged) when a credential is too long, 2 when
    /// both are set, else 1. A length check over two scalars, which
    /// crates/protocols-ffi defines as a safe fn.
    #[link_name = "_ZN6swoole11Socks5Proxy5admitEmm"]
    safe fn socks5_admit(user_length: usize, password_length: usize) -> c_int;
    /// `Socks5Proxy::pack_negotiate_request()` into the proxy's buffer. A
    /// `Socks5Proxy` is its owner's reference, and crates/protocols-ffi defines this
    /// as a safe fn over one.
    #[link_name = "_ZN6swoole11Socks5Proxy22pack_negotiate_requestEv"]
    #[expect(
        improper_ctypes,
        reason = "the proxy state is opaque behind its reference; Rust defines both sides"
    )]
    safe fn socks5_pack_negotiate(proxy: &&ProxyState) -> isize;
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
type AsyncHandler = extern "C" fn(&mut AsyncEvent<'_>);

/// The completion, on the dispatching thread.
type AsyncCallback = extern "C" fn(&AsyncEvent<'_>);

/// Upstream's `AsyncEvent` as crates/hooks' pool takes it, its request a counted
/// DNS record: the event [`dns_lookup`] builds borrows its request, and each copy the
/// pool makes holds a reference of its own, so the record outlives every borrow of
/// an event.
#[repr(C)]
pub struct AsyncEvent<'a> {
    task_id: usize,
    canceled: u8,
    error: c_int,
    data: Option<&'a ResolverRequest>,
    retval: isize,
    pipe_socket: *mut c_void,
    timestamp: f64,
    object: *mut c_void,
    handler: Option<AsyncHandler>,
    callback: Option<AsyncCallback>,
}

impl AsyncEvent<'_> {
    /// The event's request; none on an event built without one.
    fn request(&self) -> Option<&ResolverRequest> {
        self.data
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<AsyncEvent>() == 72
        && core::mem::offset_of!(AsyncEvent, data) == 16
        && core::mem::offset_of!(AsyncEvent, object) == 48,
    "crates/hooks' AsyncEvent layout moved"
);

/// The public, trivially copyable fields of `network::Client`.
#[derive(Clone, Copy)]
pub struct ClientRecord {
    pub http2: bool,
    pub open_length_check: bool,
    pub open_eof_check: bool,
    pub protocol: Protocol,
    pub server_addr: SocketAddress,
    pub remote_addr: SocketAddress,
    /// `socket`: the record the constructor made, live until the client is destroyed
    /// (an async one's free is deferred past its callbacks). Only the constructor sets it.
    pub(crate) socket: Option<Socket<'static>>,
    pub object: *mut c_void,
    pub buffer: *mut c_void,
}

/// One client's state: `network::Client`.
pub struct Client {
    record: Cell<ClientRecord>,
    socket_ops: &'static SocketOps,
    flags: Cell<u32>,
    domain: c_int,
    sock_flags: Cell<c_int>,
    /// Set once, by the first `inet_addr`; read by taking it out and putting it back.
    server_host: Cell<Option<CString>>,
    server_port: Cell<c_int>,
    /// The id of the pending connect timer.
    timer: Cell<Option<swoole_rs_timer::Held>>,
    high_watermark: Cell<u32>,
    low_watermark: Cell<u32>,
    listener: Cell<Option<Listener>>,
    /// [`ClientEvent::bit`] for each event the listener hears.
    events: Cell<u32>,
    /// The `SSLContext`, none while TLS is off.
    tls: OnceCell<TlsContext>,
    /// The immutable configuration kept from the first handshake through close.
    tls_settings: OnceCell<Arc<TlsSettings>>,
    /// The proxies, which the client owns: each a nullable pointer, as C++'s
    /// `Socks5Proxy` reads the slot.
    socks5: Cell<Option<Box<ProxyState>>>,
    http_proxy: Cell<Option<Box<ProxyState>>>,
    /// The owner of the callable `protocol.private_data_1` names, when the
    /// client has a `package_length_func`.
    length_owner: Cell<Option<Rc<dyn std::any::Any>>>,
}

/// A client's proxies.
#[derive(Clone, Copy)]
enum Proxy {
    Socks5,
    Http,
}

impl Client {
    fn proxy_slot(&self, which: Proxy) -> &Cell<Option<Box<ProxyState>>> {
        match which {
            Proxy::Socks5 => &self.socks5,
            Proxy::Http => &self.http_proxy,
        }
    }

    /// The owner of the client's `package_length_func`, which `owner` replaces.
    /// The caller drops the previous one: its destructor may reach the client.
    #[must_use]
    pub fn replace_length_owner(
        &self,
        owner: Option<Rc<dyn std::any::Any>>,
    ) -> Option<Rc<dyn std::any::Any>> {
        self.length_owner.replace(owner)
    }

    /// A copy of the public record.
    pub fn record(&self) -> ClientRecord {
        self.record.get()
    }

    /// `cli->socket`, while the client has one: a constructed client's socket
    /// record, live until the client is destroyed (an async one's free is
    /// deferred past its callbacks).
    pub fn socket(&self) -> Option<Socket<'_>> {
        self.record().socket
    }

    /// The client's TLS context, which `set()` configures; none while TLS is off.
    #[must_use]
    pub fn tls_context(&self) -> Option<&TlsContext> {
        self.tls.get()
    }

    /// Edit a copy of the public record and write it back, so no reference
    /// into the record is live while `edit` runs.
    pub fn edit_record<R>(&self, edit: impl FnOnce(&mut ClientRecord) -> R) -> R {
        let mut record = self.record.get();
        let result = edit(&mut record);
        self.record.set(record);
        result
    }

    #[must_use]
    pub fn flag(&self, name: Flag) -> bool {
        self.flags.get() & name.bit() != 0
    }

    pub fn set(&self, name: Flag, value: bool) {
        let bit = name.bit();
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

    fn flag(self, name: Flag) -> bool {
        self.get().flag(name)
    }

    fn set(self, name: Flag, value: bool) {
        self.get().set(name, value);
    }

    /// A copy of the client's public record.
    fn record(self) -> ClientRecord {
        self.get().record()
    }

    /// The record's protocol, in place: its callbacks take it by address.
    fn protocol(self) -> *const Protocol {
        let record = self.get().record.as_ptr();
        record
            .wrapping_byte_add(core::mem::offset_of!(ClientRecord, protocol))
            .cast()
    }

    fn socket_ptr(self) -> *mut SocketRecord {
        self.record()
            .socket
            .map_or(core::ptr::null_mut(), Socket::as_ptr)
    }

    fn socket(self) -> Socket<'static> {
        self.get()
            .socket()
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a connected client has no socket"))
    }

    fn socket_type(self) -> c_int {
        self.socket().read(|socket| socket.socket_type)
    }

    fn tls_state(self) -> SslState {
        self.socket().read(|socket| socket.tls.state)
    }

    /// `socket->removed = 1`: close skips the reactor.
    fn mark_removed(self) {
        self.socket().edit(|socket| socket.removed = true);
    }

    fn has(self, event: ClientEvent) -> bool {
        self.get().events.get() & event.bit() != 0
    }

    /// Runs the user callback for `event`, if the listener hears it. The client
    /// may be gone afterwards: the call holds its own reference to the listener.
    fn call(self, event: ClientEvent, data: *const c_char, length: usize) {
        let held = self.get().listener.take();
        self.get().listener.set(held.clone());
        let Some(listener) = held.filter(|_| self.has(event)) else {
            return;
        };
        let length = if event == ClientEvent::Error {
            log::last_error().unsigned_abs() as usize
        } else {
            length
        };
        // Nothing of the client is borrowed across the call, which may release it.
        listener(event, data, length);
    }

    fn event(self, op: EventOp, events: Events) -> bool {
        (self.get().socket_ops.event)(self, op, events)
    }

    /// `Socket::sendto(server_addr, data, length)`: one sendto(2).
    fn sendto(self, data: &[u8]) -> Option<usize> {
        let fd = self.socket().fd();
        self.record().server_addr.sendto(fd, data, 0)
    }

    /// `Socket::recvfrom(data, length, flags, &remote_addr)`: recvfrom(2),
    /// retried on EINTR.
    fn recvfrom(self, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
        let fd = self.socket().fd();
        self.get().edit_record(|record| {
            for _ in 0..SOCKET_RETRY_COUNT {
                let n = record.remote_addr.recvfrom(fd, &mut *buffer, flags);
                if n.is_some() || errno() != libc::EINTR {
                    return n;
                }
            }
            None
        })
    }

    /// `Socket::recvfrom_sync(data, length, flags, &remote_addr)`.
    fn recvfrom_sync(self, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
        let socket = self.socket();
        self.get().edit_record(|record| {
            net::recvfrom_sync(socket, buffer, flags, &mut record.remote_addr)
        })
    }

    /// `edit` over the context's fields and a copy of the record; TLS is on.
    /// `edit` is a plain fn, which cannot reach the client to borrow the fields again.
    fn edit_tls<R>(self, edit: fn(&mut OwnedTlsFields, ClientRecord) -> R) -> R {
        let record = self.record();
        self.get()
            .tls
            .get()
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a TLS client has no TLS context"))
            .borrow_configuration()
            .edit(|fields| edit(fields, record))
    }

    /// The context's SNI host name, NUL-terminated and empty when unset.
    fn tls_host_name(self) -> CString {
        if let Some(settings) = self.get().tls_settings.get() {
            return settings.host_name().to_owned();
        }
        let tls = self.get().tls.get();
        swoole_rs_tls::owner_host_c_str(
            tls.unwrap_or_else(|| swoole_rs_corebase::fatal("a TLS client has no TLS context")),
        )
    }

    /// `f` over the proxy, taken out of its slot for the call and put back after it
    /// (it stays where it is on the heap); `None` without one. `f` runs no effect that
    /// reaches the client's proxies.
    fn with_proxy<R>(self, which: Proxy, f: impl FnOnce(&ProxyState) -> R) -> Option<R> {
        let slot = self.get().proxy_slot(which);
        let proxy = slot.take()?;
        let result = f(&proxy);
        slot.set(Some(proxy));
        Some(result)
    }

    /// The proxy's state; `None` when there is no proxy.
    fn proxy_state(self, which: Proxy) -> Option<u8> {
        self.with_proxy(which, |proxy| proxy.fields().state)
    }

    fn set_proxy_state(self, which: Proxy, state: u8) {
        self.with_proxy(which, |proxy| proxy.update(|fields| fields.state = state));
    }

    /// `socket->get_out_buffer_length()`.
    fn queued(self) -> u32 {
        self.socket().read(|socket| {
            let buffer = socket.output.buffer();
            buffer.map_or(0, |buffer| {
                swoole_rs_buffer::cpp::swr_cpp_buffer_length(buffer)
            })
        })
    }

    /// The input buffer (`cli->buffer`), null until an async connect makes it.
    fn input(self) -> *mut CppString {
        self.record().buffer.cast()
    }

    /// `edit` over the input buffer, which exists, and the client's socket.
    /// `edit` is a plain fn, which cannot reach the client to borrow the buffer again.
    fn edit_input<R>(self, edit: fn(&mut CppString, Socket<'static>) -> R) -> R {
        let socket = self.socket();
        // SAFETY: the Box `input_init` made, live until `input_free`, which only
        // `edit` reaches meanwhile.
        edit(unsafe { &mut *self.input() }, socket)
    }

    /// `new String(SW_CLIENT_BUFFER_SIZE)`, once.
    fn input_init(self) {
        if self.input().is_null() {
            let mut buffer = Box::new(CppString::EMPTY);
            buffer.alloc(CLIENT_BUFFER_SIZE, None);
            let buffer = Box::into_raw(buffer).cast();
            self.get().edit_record(|record| record.buffer = buffer);
        }
    }

    /// `delete buffer`.
    fn input_free(self) {
        let buffer = self.input();
        if !buffer.is_null() {
            self.get()
                .edit_record(|record| record.buffer = core::ptr::null_mut());
            // SAFETY: the Box `input_init` made, released once.
            unsafe { Box::from_raw(buffer) }.dispose();
        }
    }

    /// The input buffer's free tail; the buffer exists.
    fn input_tail(self) -> Tail {
        self.edit_input(|buffer, _| {
            let length = buffer.len();
            Tail {
                str: buffer.as_mut_ptr().wrapping_add(length).cast(),
                size: buffer.size() - length,
            }
        })
    }

    /// One read into the input buffer's free tail; the buffer exists.
    fn input_recv(self) -> Option<usize> {
        self.edit_input(|buffer, socket| net::recv(socket, buffer.spare(), 0))
    }

    /// `buffer->clear()`.
    fn input_clear(self) {
        self.edit_input(|buffer, _| buffer.clear());
    }

    /// The SOCKS5 negotiation request's length in [`Self::socks5_packet`].
    fn socks5_negotiate(self) -> usize {
        // The SOCKS5 proxy, whose buffer the request is packed into.
        self.with_proxy(Proxy::Socks5, |proxy| {
            socks5_pack_negotiate(&proxy).cast_unsigned()
        })
        .unwrap_or(0)
    }

    /// Send the first `length` bytes of the SOCKS5 proxy's packet buffer, copied
    /// first: the send may raise a callback that reaches the proxy again.
    fn socks5_send(self, length: usize) -> Option<usize> {
        let packet = self
            .with_proxy(Proxy::Socks5, |proxy| proxy.with_packet(|packet| *packet))
            .flatten()
            .unwrap_or([0; 512]);
        send(self, packet.get(..length).unwrap_or_default(), 0)
    }

    /// `Client::socks5_handshake`: applies `length` received bytes at `data` and
    /// sends the answer the proxy packed.
    fn socks5_handshake(self, data: *const c_char, length: usize) -> bool {
        let owner = self.get().socks5.as_ptr().cast();
        // SAFETY: the live SOCKS5 proxy; `length` bytes at `data`.
        let n = unsafe { socks5_handshake_step(owner, data, length) };
        n == 0 || n > 0 && self.socks5_send(n.cast_unsigned()) == Some(n.cast_unsigned())
    }

    /// A result from the scratch buffer, without holding a client borrow across
    /// the loan or letting the buffer escape it.
    fn with_buffer<R>(self, mut f: impl FnMut(&mut CppString) -> R) -> R {
        let mut result = None;
        (self.get().socket_ops.with_buffer)(&mut |buffer| {
            result = Some(f(buffer));
        });
        result
            .unwrap_or_else(|| swoole_rs_corebase::fatal("the scratch-buffer callback did not run"))
    }

    /// The HTTP proxy's CONNECT request, packed into `shared`.
    /// The host is the TLS host name when set, else the proxy's target.
    fn http_pack(self, shared: &mut CppString) -> &[u8] {
        let tls = self.get().tls.get().is_some();
        let name = tls.then(|| self.tls_host_name());
        let packed = self.with_proxy(Proxy::Http, |proxy| {
            let target = proxy.string(3);
            let host = if let Some(name) = name.as_ref().filter(|name| !name.is_empty()) {
                name.to_bytes()
            } else {
                &target
            };
            // The whole allocation, zeroed so every byte lent is initialized.
            shared.read_zeroed(shared.size(), |output| {
                swoole_rs_http_ffi::proxy_connect(proxy, host, output)
            });
        });
        if packed.is_some() { shared.view() } else { &[] }
    }

    fn set_timeout(self, timeout: f64) {
        self.socket().edit(|socket| {
            socket
                .timeouts
                .set(timeout, swoole_rs_reactor::TimeoutKind::ALL);
        });
    }

    fn timer_clear(self) {
        if let Some(timer) = self.get().timer.take() {
            (self.get().socket_ops.timer_delete)(timer);
        }
    }

    /// `execute_onConnect`.
    fn on_connect(self) {
        self.timer_clear();
        self.call(ClientEvent::Connect, core::ptr::null(), 0);
    }
}

/// `Client_onPackage`: a framed package for `onReceive`; the receive loop stops
/// once the callback left the socket waiting to close. Only a client's protocol
/// holds it, and [`OnPackage`](crate::protocol::OnPackage)'s contract has a caller pass that protocol's socket,
/// a client's, whose free is deferred past the call: as for the reactor handlers
/// below, the socket's kind is what makes its object a client.
extern "C-unwind" fn on_package(
    _: *const Protocol,
    socket: Socket<'_>,
    package: &RecvData,
) -> c_int {
    Handle(socket.read(|socket| socket.object).cast()).call(
        ClientEvent::Receive,
        package.data.cast(),
        package.info.len as usize,
    );
    if socket.read(|socket| socket.output.close_wait) {
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

/// A handler's success as the reactor's `SW_OK`/`SW_ERR`.
const fn status(ok: bool) -> c_int {
    if ok { OK } else { ERR }
}

fn on_stream_read(_: Running<'_>, event: &mut Event) -> c_int {
    status(event_client(event).is_some_and(on_read))
}

fn on_dgram_read(_: Running<'_>, event: &mut Event) -> c_int {
    status(event_client(event).is_some_and(on_datagram))
}

fn on_writable(reactor: Running<'_>, event: &mut Event) -> c_int {
    status(event_client(event).is_some_and(|this| on_write(this, reactor, event)))
}

fn on_failure(reactor: Running<'_>, event: &mut Event) -> c_int {
    status(event_client(event).is_some_and(|this| on_error(this, reactor, event)))
}

/// `Client::init_reactor`: the client sockets' handlers.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_init_reactor(reactor: Running<'_>) {
    let handlers: [(c_int, Events, swoole_rs_reactor::Handler); 4] = [
        (FD_STREAM_CLIENT, Events::READ, on_stream_read),
        (FD_DGRAM_CLIENT, Events::READ, on_dgram_read),
        (FD_STREAM_CLIENT, Events::WRITE, on_writable),
        (FD_STREAM_CLIENT, Events::ERROR, on_failure),
    ];
    for (fd_type, event, handler) in handlers {
        set_handler(reactor, fd_type, event, Some(handler));
    }
}

/// `Client::onConnect` and the rest: `listener(event, ...)` for each event in
/// `events` ([`ClientEvent::bit`]s).
pub fn listen(client: &Client, events: u32, listener: Listener) {
    drop(client.listener.replace(Some(listener)));
    client.events.set(events);
}

use swoole_rs_corebase::sys::errno;

use swoole_rs_corebase::set_errno;

fn is_stream(kind: c_int) -> bool {
    SocketType::from_raw(kind).is_some_and(SocketType::is_stream)
}

fn is_dgram(kind: c_int) -> bool {
    SocketType::from_raw(kind).is_some_and(SocketType::is_dgram)
}

/// `new Client(type, async)`: a handle on the new client, live until [`Handle::free`];
/// None with errno set when the socket could not be made (logged as upstream's
/// constructor does).
#[must_use]
pub fn create(kind: c_int, is_async: bool) -> Option<Handle> {
    create_with(kind, is_async, &SocketOps::NATIVE)
}

fn create_with(kind: c_int, is_async: bool, socket_ops: &'static SocketOps) -> Option<Handle> {
    let fd_type = if is_stream(kind) {
        FD_STREAM_CLIENT
    } else {
        FD_DGRAM_CLIENT
    };
    let flags = if is_async { SOCK_NONBLOCK } else { 0 } | SOCK_CLOEXEC;
    // The new socket record, which nothing else holds yet.
    let Some(record) = (socket_ops.make_socket)(kind, fd_type, flags) else {
        let error = errno();
        log::sys_warning(c"Client", format_args!("socket() failed"));
        set_errno(error);
        return None;
    };
    let mut client = Box::new(Client {
        socket_ops,
        record: Cell::new(ClientRecord {
            http2: false,
            open_length_check: false,
            open_eof_check: false,
            protocol: Protocol::EMPTY,
            server_addr: swoole_rs_corebase::sys::zeroed(),
            remote_addr: swoole_rs_corebase::sys::zeroed(),
            socket: None,
            object: core::ptr::null_mut(),
            buffer: core::ptr::null_mut(),
        }),
        flags: Cell::new(0),
        domain: 0,
        sock_flags: Cell::new(0),
        server_host: Cell::new(None),
        server_port: Cell::new(0),
        timer: Cell::new(None),
        high_watermark: Cell::new(0),
        low_watermark: Cell::new(0),
        listener: Cell::new(None),
        events: Cell::new(0),
        tls: OnceCell::new(),
        tls_settings: OnceCell::new(),
        socks5: Cell::new(None),
        http_proxy: Cell::new(None),
        length_owner: Cell::new(None),
    });
    client.set(Flag::Async, is_async);
    let fields = client.record.get_mut();
    fields.socket = Some(record);
    fields.remote_addr.kind = kind;
    fields.protocol.package_length_type = b'N';
    fields.protocol.package_length_size = 4;
    fields.protocol.package_max_length = 2 * 1024 * 1024;
    fields.protocol.on_package = Some(on_package);
    let socket_type = record.edit(|record| {
        record.chunk_size = SEND_BUFFER_SIZE;
        record.socket_type
    });
    client.set(Flag::Stream, is_stream(socket_type));
    if let Some((domain, _)) = socket_domain_and_type(kind) {
        client.domain = domain;
    }
    let client = Box::into_raw(client);
    record.edit(|record| record.object = client.cast());
    Some(Handle(client))
}

impl Handle {
    /// A handle on `client`.
    ///
    /// # Safety
    /// `client` came from [`create`] ([`Handle::as_ptr`]) and stays live while this
    /// handle or a copy of it is used.
    #[must_use]
    pub const unsafe fn new(client: *mut Client) -> Self {
        Self(client)
    }

    /// The client's pointer, which [`Handle::new`] takes back.
    #[must_use]
    pub const fn as_ptr(self) -> *mut Client {
        self.0
    }

    /// The client, borrowed while this handle is: live per [`Handle::new`].
    #[must_use]
    pub fn client(&self) -> &Client {
        self.get()
    }

    /// `enable_ssl_encrypt()`: the new TLS context, which the caller configures
    /// before any handshake; None when TLS was already on.
    #[must_use]
    pub fn enable_ssl(self) -> Option<NonNull<OwnedTlsContext>> {
        enable_ssl(self)
    }

    /// The proxy's fields (`http` or SOCKS5), null without one.
    #[must_use]
    pub fn proxy_fields(self, http: bool) -> *mut swoole_rs_corebase::ProxyFields {
        let which = if http { Proxy::Http } else { Proxy::Socks5 };
        self.with_proxy(which, ProxyState::fields_ptr)
            .unwrap_or(core::ptr::null_mut())
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
        if !self.flag(Flag::Closed) {
            close(self);
        }
        self.input_free();
        // A pending connect timer would fire on a freed client.
        self.timer_clear();
        if self.flag(Flag::Async) {
            // A cleared object tells a close in progress that the client is gone.
            self.socket()
                .edit(|socket| socket.object = core::ptr::null_mut());
            (self.get().socket_ops.free_socket)(self);
        } else {
            // SAFETY: `delete socket`: the record make_socket allocated, released once.
            unsafe { SocketRecord::free(self.record().socket) };
        }
        // SAFETY: a Box from `create`, released once (per the caller).
        let client = unsafe { Box::from_raw(self.0) };
        // The proxies first, then the rest of the state.
        for proxy in [&client.socks5, &client.http_proxy] {
            drop(proxy.take());
        }
    }

    /// `Client::get_peer_name`: datagram peers are value snapshots of the last
    /// received address; streams ask the kernel through the socket's operation.
    pub fn peer_name(self, address: &mut SocketAddress) -> bool {
        if is_dgram(self.socket_type()) {
            *address = self.record().remote_addr;
            true
        } else {
            self.socket().read(|socket| socket.get_peer_name(address))
        }
    }

    /// `Client::bind(host, port)`: `SO_REUSEADDR` (a failure is logged), then
    /// bind(2) to the host's literal address.
    #[must_use]
    pub fn bind(self, host: &CStr, port: c_int) -> bool {
        let fd = self.socket().fd();
        let enable: c_int = 1;
        if swoole_rs_corebase::sys::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &enable)
            .is_err()
        {
            log::sys_warning(
                c"bind",
                format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
            );
        }
        let Some(address) = address_of(self.socket_type(), host, port, false) else {
            return false;
        };
        if !self.socket().edit(|socket| socket.bind(&address)) {
            log::set_last_error(errno());
            return false;
        }
        true
    }
}

/// The client protocol's length of the packet in `data`.
#[must_use]
pub fn package_length(this: Handle, data: &[u8]) -> isize {
    let record = this.record();
    let Some(get) = record.protocol.get_package_length else {
        return -1;
    };
    let mut packet = PacketLength::new(data);
    // SAFETY: the live client's protocol and socket, and the packet's bytes.
    unsafe { get(this.protocol(), this.socket_ptr().cast(), &mut packet) }
}

/// `Client::close()`: false without a socket, when already closed or when `close(2)`
/// fails; true when it closed or `onClose` released the client.
#[expect(
    clippy::must_use_candidate,
    reason = "most callers close for the effect"
)]
pub fn close(this: Handle) -> bool {
    if this.socket_ptr().is_null() || this.flag(Flag::Closed) {
        return false;
    }
    this.set(Flag::Closed, true);
    let saved = this.socket();
    if this.flag(Flag::OpenSsl)
        && this.get().tls.get().is_some()
        && !saved.read(|socket| socket.ssl().is_null())
    {
        (this.get().socket_ops.close_tls)(saved);
    }
    if this.socket_type() == SOCK_UNIX_DGRAM {
        let path = saved.read(|socket| socket.info.addr.unix().sun_path);
        let path = path.map(|byte| byte.to_ne_bytes()[0]);
        if let Ok(path) = CStr::from_bytes_until_nul(&path) {
            _ = swoole_rs_corebase::sys::unlink(path);
        }
    }
    if this.flag(Flag::Async) {
        if !saved.read(|socket| socket.removed) {
            _ = this.event(EventOp::Del, Events::NONE);
        }
        this.timer_clear();
        if this.flag(Flag::Active) {
            this.set(Flag::Active, false);
            // onClose may destroy the client: only the saved socket is read after it.
            this.call(ClientEvent::Close, core::ptr::null(), 0);
        }
    } else {
        this.set(Flag::Active, false);
    }
    // The saved record's free is deferred past this call.
    let Some(fd) = saved
        .edit(|socket| (!socket.object.is_null()).then(|| core::mem::replace(&mut socket.fd, -1)))
    else {
        return true;
    };
    // Once: -1 marks the client's descriptor closed.
    swoole_rs_corebase::sys::close(fd).is_ok()
}

/// `Client::shutdown(how)`. Upstream's `SHUT_RDWR` records only a read shutdown.
#[must_use]
pub fn shutdown(this: Handle, how: c_int) -> bool {
    if this.socket_ptr().is_null() || this.flag(Flag::Closed) {
        return false;
    }
    let fd = this.socket().fd();
    let call = |how| swoole_rs_corebase::sys::shutdown(fd, how);
    match how {
        libc::SHUT_RD if this.flag(Flag::ShutdownRead) || call(how).is_err() => false,
        libc::SHUT_WR if this.flag(Flag::ShutdownWrite) || call(how).is_err() => false,
        libc::SHUT_RDWR if call(how).is_err() => false,
        libc::SHUT_RD | libc::SHUT_RDWR => {
            this.set(Flag::ShutdownRead, true);
            true
        }
        libc::SHUT_WR => {
            this.set(Flag::ShutdownWrite, true);
            true
        }
        _ => {
            log::set_last_error(libc::EINVAL);
            false
        }
    }
}

/// `Client::sleep()`: stop reading (keeping write interest). Whether it worked.
#[must_use]
pub fn sleep(this: Handle) -> bool {
    if this
        .socket()
        .read(|socket| socket.events)
        .intersects(Events::WRITE)
    {
        this.event(EventOp::Set, Events::WRITE)
    } else {
        this.event(EventOp::Del, Events::NONE)
    }
}

/// `Client::wakeup()`: read again. Whether it worked.
#[must_use]
pub fn wakeup(this: Handle) -> bool {
    if this
        .socket()
        .read(|socket| socket.events)
        .intersects(Events::WRITE)
    {
        this.event(EventOp::Set, Events::READ | Events::WRITE)
    } else {
        this.event(EventOp::Add, Events::READ)
    }
}

/// `swoole_event_set(socket, SW_EVENT_WRITE)`, to finish a handshake: whether it
/// worked.
#[must_use]
pub fn ssl_wait(this: Handle) -> bool {
    this.event(EventOp::Set, Events::WRITE)
}

/// `Client::enable_ssl_encrypt()`: a TLS context, TLS on, and DTLS for a datagram
/// client. The context, or None when TLS was already on.
fn enable_ssl(this: Handle) -> Option<NonNull<OwnedTlsContext>> {
    // A new context and the client's reference to it.
    if this.get().tls.set(TlsContext::new()).is_err() {
        return None;
    }
    this.set(Flag::OpenSsl, true);
    if is_dgram(this.socket_type()) {
        enable_dtls(this);
    }
    this.ssl_context()
}

/// `Client::set_http_proxy` (`http`) or `set_socks5_proxy`: a new proxy, which
/// replaces the client's. A SOCKS5 proxy whose credentials are too long (logged)
/// clears it. The client aborts without a socket.
pub fn set_proxy(
    client: &Client,
    http: bool,
    host: &[u8],
    port: i64,
    user: &[u8],
    password: &[u8],
) {
    // Only read through, as `Handle`'s methods do.
    let this = Handle(core::ptr::from_ref(client).cast_mut());
    let port = port.c_cast::<c_int>();
    let which = if http { Proxy::Http } else { Proxy::Socks5 };
    let proxy = if http {
        Some(swoole_rs_http_ffi::http_proxy(host, port, user, password))
    } else {
        match socks5_admit(user.len(), password.len()) {
            0 => None,
            config => {
                let owner = ProxyState::new(true);
                let texts = [host, user, password];
                // An allocation failure leaves the strings it didn't reach empty.
                _ = owner.configure(texts, port, config == 2, this.socket_type());
                Some(owner)
            }
        }
    };
    // The replaced proxy is released here.
    drop(client.proxy_slot(which).replace(proxy));
}

/// `Client::enable_dtls()`: the context's protocols become DTLS.
fn enable_dtls(this: Handle) {
    this.edit_tls(|fields, _| fields.protocols = SSL_DTLS);
    this.socket().edit(|socket| {
        socket.dtls = true;
        socket.chunk_size = SSL_BUFFER_SIZE;
    });
    this.set(Flag::Dtls, true);
}

fn tls_freeze(owner: &TlsContext) -> Option<TlsSettings> {
    owner.borrow_configuration().freeze(None)
}

/// `Client::ssl_handshake()`.
#[must_use]
pub fn ssl_handshake(this: Handle) -> bool {
    if this.tls_state() == SSL_STATE_READY {
        return false;
    }
    let settings =
        if let Some(settings) = this.get().tls_settings.get() {
            Arc::clone(settings)
        } else {
            let tls =
                this.get().tls.get().cloned().unwrap_or_else(|| {
                    swoole_rs_corebase::fatal("a TLS client has no TLS context")
                });
            this.edit_tls(|fields, record| fields.http_v2 = record.http2);
            let Some(settings) = (this.get().socket_ops.tls_freeze)(&tls) else {
                return false;
            };
            let settings = Arc::new(settings);
            _ = this.get().tls_settings.set(Arc::clone(&settings));
            settings
        };
    if this.socket().read(|socket| socket.ssl().is_null()) {
        this.socket().edit(|socket| socket.output.ssl_send = true);
        if !(this.get().socket_ops.tls_session)(&settings, this.socket()) {
            log::set_last_error(ERROR_SSL_CREATE_SESSION_FAILED);
            return false;
        }
        let name = this.tls_host_name();
        if !name.is_empty() {
            // On the new session; OpenSSL copies the name.
            (this.get().socket_ops.tls_set_host_name)(this.socket(), &name);
        }
    }
    if !(this.get().socket_ops.tls_connect)(this.socket()) {
        return false;
    }
    if this.tls_state() == SSL_STATE_READY
        && settings.verify_peer()
        && !ssl_verify(this, settings.allow_self_signed())
    {
        return false;
    }
    true
}

/// `Client::ssl_verify(allow_self_signed)`: the peer's certificate, and its host
/// when the context names one.
#[must_use]
pub fn ssl_verify(this: Handle, allow_self_signed: bool) -> bool {
    if !(this.get().socket_ops.tls_verify)(this.socket(), allow_self_signed) {
        return false;
    }
    let name = this.tls_host_name();
    if !name.is_empty() && !(this.get().socket_ops.tls_check_host)(this.socket(), &name) {
        return false;
    }
    true
}

/// `Client_inet_addr`: the first call routes through the proxies and records the
/// server; every call assigns `server_addr`, or waits for DNS. False when the
/// address does not assign and no DNS wait covers it.
fn inet_addr(this: Handle, host: &CStr, port: c_int) -> bool {
    let (mut host, mut port) = (host, port);
    // The last proxy's host, NUL-terminated, which `host` borrows.
    let mut proxy_host;
    if !this.flag(Flag::HostPreset) {
        for which in [Proxy::Socks5, Proxy::Http] {
            let Some(next) = this.with_proxy(which, |proxy| {
                proxy.set_string(3, host.to_bytes());
                proxy.update(|fields| fields.target_port = port);
                (proxy.string(0), proxy.fields().port)
            }) else {
                continue;
            };
            (proxy_host, port) = next;
            proxy_host.push(0);
            host = CStr::from_bytes_until_nul(&proxy_host).unwrap_or_default();
        }
        this.get().server_host.set(Some(host.to_owned()));
        this.get().server_port.set(port);
        this.set(Flag::HostPreset, true);
    }
    let resolve = !this.flag(Flag::Async);
    if (this.get().socket_ops.assign)(this, host, port, resolve) {
        return true;
    }
    let wait = !this.flag(Flag::DnsCompleted) && log::last_error() == ERROR_BAD_HOST_ADDR;
    if wait {
        this.set(Flag::WaitDns, true);
    }
    wait
}

/// Dispatches a `GethostbynameRequest` for the server host to the async pool,
/// completing in [`resolved`].
/// Whether the pool took the lookup.
fn dns_lookup(this: Handle) -> bool {
    // The request copies the host's bytes, so nothing points into it once it is back.
    let host = this.get().server_host.take();
    let name = host.as_deref().map_or(&b""[..], CStr::to_bytes);
    let request = crate::resolver::Request::new(this.get().domain, 0, 0, name, b"");
    this.get().server_host.set(host);
    let event = AsyncEvent {
        task_id: 0,
        canceled: 0,
        error: 0,
        data: Some(request.get()),
        retval: 0,
        pipe_socket: core::ptr::null_mut(),
        timestamp: 0.0,
        object: this.0.cast(),
        handler: Some(resolve),
        callback: Some(resolve_completed),
    };
    // The pool's copy holds its own reference to the request.
    (native_host().dispatch_dns)(&event)
}

/// `async::handler_gethostbyname`, on a pool thread, over a copy of the event
/// [`dns_lookup`] dispatched.
extern "C" fn resolve(event: &mut AsyncEvent<'_>) {
    // Its request record, held by the copy.
    if let Some(request) = event.request() {
        (event.retval, event.error) = crate::resolver::host(request);
    }
}

/// `Client_onResolveCompleted`, on the client's thread, over the completed copy
/// of the event [`dns_lookup`] dispatched.
extern "C" fn resolve_completed(event: &AsyncEvent<'_>) {
    // Its request record, held by the copy.
    let Some(request) = event.request() else {
        return;
    };
    let address = crate::resolver::address(request);
    resolved(
        Handle(event.object.cast()),
        event.error,
        &crate::resolver::c_name(&address),
    );
}

/// `Client::connect(host, port, timeout, flags)`: `host` is NUL-terminated.
/// Whether it connected, or, async, began to.
#[must_use]
pub fn connect(this: Handle, host: &CStr, port: c_int, timeout: f64, flags: c_int) -> bool {
    if !this.flag(Flag::Stream) {
        udp_connect(this, host, port, timeout, flags)
    } else if this.flag(Flag::Async) {
        tcp_connect_async(this, host, port, timeout)
    } else {
        tcp_connect_sync(this, host, port, timeout, flags != 0)
    }
}

/// `Client_tcp_connect_sync`: connect, then the proxy handshake and TLS.
fn tcp_connect_sync(this: Handle, host: &CStr, port: c_int, timeout: f64, nonblock: bool) -> bool {
    this.set_timeout(timeout);
    if timeout > 0.0 {
        (this.get().socket_ops.kernel_timeout)(this, timeout);
    }
    if !inet_addr(this, host, port) {
        return false;
    }
    if nonblock {
        return match (this.get().socket_ops.connect_async)(this) {
            Connect::Ready => true,
            Connect::Wait => {
                this.set(Flag::AsyncConnect, true);
                false
            }
            Connect::Error => false,
        };
    }
    if !(this.get().socket_ops.connect)(this, ConnectMode::Sync) {
        return false;
    }
    this.set(Flag::Active, true);
    // Only a proxy handshake reads the thread's shared buffer, which a thread
    // that never set up SwooleTG doesn't have.
    if this.proxy_state(Proxy::Socks5).is_some() {
        if this.socks5_send(this.socks5_negotiate()).is_none() {
            return false;
        }
        this.set_proxy_state(Proxy::Socks5, SOCKS5_HANDSHAKE);
        // Each answer lands at the start of the emptied buffer.
        let ready = this.with_buffer(|shared| {
            loop {
                shared.clear();
                let spare = shared.spare();
                let n = recv(this, spare, 0).unwrap_or(0);
                if n == 0 || !this.socks5_handshake(spare.as_ptr().cast(), n) {
                    return false;
                }
                if this.proxy_state(Proxy::Socks5) == Some(SOCKS5_READY) {
                    return true;
                }
            }
        });
        if !ready {
            return false;
        }
    } else if this.proxy_state(Proxy::Http).is_some()
        && !this.with_buffer(|shared| {
            if send(this, this.http_pack(shared), 0).is_none() {
                return false;
            }
            // `HttpProxy::handshake` of one recv into the cleared buffer.
            shared.clear();
            let mut n = 0;
            shared.read_zeroed(shared.size(), |bytes| {
                n = recv(this, bytes, 0).unwrap_or(0);
                n
            });
            n > 0 && swoole_rs_http_ffi::proxy_accept(shared.view())
        })
    {
        return false;
    }
    !this.flag(Flag::OpenSsl) || ssl_handshake(this)
}

/// `Client_tcp_connect_async`: a nonblocking connect finished by the write handler.
fn tcp_connect_async(this: Handle, host: &CStr, port: c_int, timeout: f64) -> bool {
    if ![
        ClientEvent::Connect,
        ClientEvent::Error,
        ClientEvent::Close,
        ClientEvent::Receive,
    ]
    .into_iter()
    .all(|which| this.has(which))
    {
        log::warning(
            c"Client_tcp_connect_async",
            format_args!("onConnect/onError/onReceive/onClose callback have not set"),
        );
        return false;
    }
    this.set_timeout(timeout);
    this.input_init();
    if this.has(ClientEvent::BufferFull) && this.get().high_watermark.get() == 0 {
        let mark = (f64::from(this.socket().read(|socket| socket.output.buffer_size)) * 0.8)
            .c_cast::<u32>();
        this.get().high_watermark.set(mark);
    }
    if !inet_addr(this, host, port) {
        return false;
    }
    if this.flag(Flag::WaitDns) {
        return dns_lookup(this);
    }
    loop {
        let connected = (this.get().socket_ops.connect)(this, ConnectMode::Raw);
        let error = errno();
        if !connected && error == libc::EINTR {
            continue;
        }
        if connected || error == libc::EINPROGRESS {
            // A finished connect needs no timeout timer; either way the write
            // handler runs the proxy and TLS handshakes.
            if !connected && timeout > 0.0 {
                // The client deletes the timer before it goes.
                let timer = (this.get().socket_ops.timer_add)(timeout, this);
                this.get().timer.set(timer);
                if timer.is_none() {
                    return false;
                }
            }
            return this.event(EventOp::Add, Events::WRITE);
        }
        break;
    }
    this.set(Flag::Active, false);
    this.mark_removed();
    close(this);
    if this.has(ClientEvent::Error) {
        this.set(Flag::OnerrorCalled, true);
        this.call(ClientEvent::Error, core::ptr::null(), 0);
    }
    false
}

/// `Client_udp_connect`.
fn udp_connect(this: Handle, host: &CStr, port: c_int, timeout: f64, udp_connect: c_int) -> bool {
    let is_async = this.flag(Flag::Async);
    this.set_timeout(timeout);
    if !is_async && timeout > 0.0 {
        (this.get().socket_ops.kernel_timeout)(this, timeout);
    }
    this.get().sock_flags.set(udp_connect);
    if !inet_addr(this, host, port) {
        return false;
    }
    if is_async && !this.has(ClientEvent::Receive) {
        log::warning(
            c"Client_udp_connect",
            format_args!("`onReceive` callback have not set"),
        );
        return false;
    }
    if this.flag(Flag::WaitDns) && is_async {
        // Without onError a failed lookup would go unreported.
        if !this.has(ClientEvent::Error) {
            log::warning(
                c"Client_udp_connect",
                format_args!("`onError` callback have not set"),
            );
            return false;
        }
        return dns_lookup(this);
    }
    this.set(Flag::Active, true);
    let fd = this.socket().fd();
    let default_size = SOCKET_DEFAULTS.buffer_size();
    _ = set_socket_buffer_size(fd, true, true, default_size);
    if timeout > 0.0 {
        this.set_timeout(timeout);
    }
    if this.socket_type() == SOCK_UNIX_DGRAM && !bind_unix(this) {
        return false;
    }
    let mut udp_connect = udp_connect;
    if this.flag(Flag::OpenSsl) {
        udp_connect = 1;
        enable_dtls(this);
    }
    if udp_connect == 1 {
        if !(this.get().socket_ops.connect)(this, ConnectMode::Raw) {
            this.set(Flag::Active, false);
            this.mark_removed();
            close(this);
            if is_async && this.has(ClientEvent::Error) {
                this.set(Flag::OnerrorCalled, true);
                this.call(ClientEvent::Error, core::ptr::null(), 0);
            }
            return false;
        }
        fd_clean(fd);
    }
    if is_async && this.has(ClientEvent::Connect) {
        if !this.event(EventOp::Add, Events::READ) {
            return false;
        }
        this.on_connect();
    }
    !this.flag(Flag::OpenSsl) || ssl_handshake(this)
}

/// Binds a Unix datagram client to `/tmp/swoole-client.<pid>.<fd>.sock`, its
/// address kept in the socket's `info` for the unlink on close; false, with a
/// warning, when the bind fails.
fn bind_unix(this: Handle) -> bool {
    let socket = this.socket();
    let pid = std::process::id();
    let path = format!("/tmp/swoole-client.{pid}.{}.sock", socket.fd());
    let bound = std::ffi::CString::new(path.as_str()).unwrap_or_default();
    let bound_ok = socket.edit(|socket| {
        let address = socket.info.addr.unix_mut();
        for (slot, byte) in address.sun_path.iter_mut().zip(path.bytes().chain([0])) {
            *slot = c_char::from_ne_bytes([byte]);
        }
        let family = libc::AF_UNIX.c_cast::<libc::sa_family_t>();
        address.sun_family = family;
        _ = swoole_rs_corebase::sys::unlink(&bound);
        // The address just written, bound at `sizeof(sockaddr_un)`; the record keeps
        // its own length.
        let mut local = socket.info;
        local.len = swoole_rs_corebase::sys::socklen::<libc::sockaddr_un>();
        local.bind(socket.fd)
    });
    if !bound_ok {
        log::sys_warning(c"Client_udp_connect", format_args!("bind({path}) failed"));
    }
    bound_ok
}

type BufferLoan = fn(&mut dyn FnMut(&mut CppString));

/// Host effects selected at construction; callbacks copy the function pointer
/// before an effect can release the client.
struct SocketOps {
    /// Lends the scratch buffer to the callback exactly once, synchronously.
    with_buffer: BufferLoan,
    send_sync: fn(Socket<'_>, &[u8], c_int) -> Option<usize>,
    sendfile_sync: fn(Socket<'_>, &CStr, i64, usize) -> bool,
    sendfile_async: fn(Socket<'_>, &CStr, i64, usize) -> bool,
    free_socket: fn(Handle),
    close_tls: fn(Socket<'_>),
    write: fn(Socket<'static>, &[u8]) -> Option<usize>,
    writable: extern "C-unwind" fn(Running<'_>, &mut Event) -> c_int,
    make_socket: fn(c_int, c_int, c_int) -> Option<Socket<'static>>,
    kernel_timeout: fn(Handle, f64),
    assign: fn(Handle, &CStr, c_int, bool) -> bool,
    connect_async: fn(Handle) -> Connect,
    connect: fn(Handle, ConnectMode) -> bool,
    event: fn(Handle, EventOp, Events) -> bool,
    socket_error: fn(Handle, &mut c_int) -> bool,
    timer_add: fn(f64, Handle) -> Option<swoole_rs_timer::Held>,
    timer_delete: fn(swoole_rs_timer::Held),
    tls_freeze: fn(&TlsContext) -> Option<TlsSettings>,
    tls_session: fn(&TlsSettings, Socket<'_>) -> bool,
    tls_connect: fn(Socket<'_>) -> bool,
    tls_set_host_name: fn(Socket<'_>, &CStr) -> bool,
    tls_verify: fn(Socket<'_>, bool) -> bool,
    tls_check_host: fn(Socket<'_>, &CStr) -> bool,
}

impl SocketOps {
    const NATIVE: Self = Self {
        with_buffer: |f| swoole_rs_buffer::string::with_thread_buffer(f),
        send_sync: swoole_rs_tls::upstream::send_sync,
        sendfile_sync: swoole_rs_tls::upstream::sendfile_sync,
        sendfile_async: swoole_rs_tls::upstream::socket_sendfile_async,
        free_socket: |this| {
            // SAFETY: only Handle::free calls this, once, for its still-owned socket.
            unsafe { swoole_rs_tls::upstream::socket_free(this.record().socket) };
        },
        close_tls: |socket| {
            // SAFETY: only close calls this; session handles live only for their
            // operation, and none is running on the closing client.
            unsafe { swoole_rs_tls::upstream::socket_ssl_close(socket) };
        },
        write: swoole_rs_reactor::event_api::event_write,
        writable: swoole_rs_tls::reactor_write::reactor_writable_callback,
        make_socket: ops::make_socket,
        kernel_timeout: ops::kernel_timeout,
        assign: ops::assign,
        connect_async: ops::connect_async,
        connect: ops::connect,
        event: ops::event,
        socket_error: ops::socket_error,
        timer_add: |timeout, this| {
            swoole_rs_timer::add_fn_sec(timeout, false, move || on_timeout(this))
        },
        timer_delete: swoole_rs_timer::del_id,
        tls_freeze,
        tls_session: TlsSettings::create_client_session,
        tls_connect: swoole_rs_tls::upstream::socket_ssl_connect,
        tls_set_host_name: swoole_rs_tls::upstream::socket_ssl_set_host_name,
        tls_verify: swoole_rs_tls::upstream::socket_ssl_verify,
        tls_check_host: swoole_rs_tls::upstream::socket_ssl_check_host,
    };
}

/// The reactor and socket operations used by production clients.
mod ops {
    use super::{Connect, ConnectMode, EventOp, Handle, SocketRecord, address_of};
    use core::ffi::{CStr, c_int};
    use swoole_rs_reactor::{event_api, socket_upstream};

    /// `make_socket(type, fd_type, flags)`: the new record's socket, or none.
    pub(super) fn make_socket(
        kind: c_int,
        fd_type: c_int,
        flags: c_int,
    ) -> Option<swoole_rs_reactor::Socket<'static>> {
        socket_upstream::make_typed(kind, fd_type, flags).map(SocketRecord::into_socket)
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
        this.get()
            .edit_record(|record| record.server_addr = address);
        true
    }

    /// `connect_async` of the socket to `server_addr`; errno is the call's.
    pub(super) fn connect_async(this: Handle) -> Connect {
        let address = &this.record().server_addr;
        this.socket()
            .edit(|socket| socket_upstream::socket_connect_async(socket, address))
    }

    /// Connects the socket to `server_addr`, by `mode`: whether it connected, else
    /// errno is the call's.
    pub(super) fn connect(this: Handle, mode: ConnectMode) -> bool {
        let socket = this.socket();
        let address = &this.record().server_addr;
        match mode {
            ConnectMode::Sync => socket_upstream::connect_sync(socket, address) >= 0,
            ConnectMode::Raw => address.connect(socket.fd()),
        }
    }

    /// The reactor's `swoole_event_*` on the socket: whether it worked, false without a
    /// socket or a reactor. The client's socket stays live while it is registered:
    /// `close` deletes it from the reactor before the record is freed.
    pub(super) fn event(this: Handle, op: EventOp, events: swoole_rs_reactor::Events) -> bool {
        let Some(socket) = this.get().socket() else {
            return false;
        };
        match op {
            EventOp::Add => event_api::event_add(socket, events),
            EventOp::Set => event_api::event_set(socket, events),
            EventOp::Del => event_api::event_del(socket),
            EventOp::AddOrUpdate => event_api::add_or_update(socket, events),
        }
    }

    /// `getsockopt(SOL_SOCKET, SO_ERROR)` into `error`: whether it succeeded.
    pub(super) fn socket_error(this: Handle, error: &mut c_int) -> bool {
        let fd = this.socket().fd();
        swoole_rs_corebase::sys::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, error).is_ok()
    }
}

/// An address of socket type `kind` for `host`, resolved if `resolve`;
/// `None` with the last error set when it does not assign.
fn address_of(kind: c_int, host: &CStr, port: c_int, resolve: bool) -> Option<SocketAddress> {
    let mut address = SocketAddress {
        kind,
        ..SocketAddress::default()
    };
    let assigned = crate::address::assign(
        &mut address.addr,
        &mut address.len,
        kind,
        host.to_bytes_with_nul(),
        port,
        resolve,
    );
    assigned.map_err(log::set_last_error).ok()?;
    Some(address)
}

/// `Client::sendto(host, port, data, length)`: one datagram to the host, resolved
/// unless the client is async.
#[must_use]
pub fn sendto(this: Handle, host: &CStr, port: c_int, data: &[u8]) -> bool {
    if !is_dgram(this.socket_type()) {
        log::set_last_error(ERROR_OPERATION_NOT_SUPPORT);
        log::warning(
            c"sendto",
            format_args!("only supports SWOOLE_SOCK_(UDP/UDP6/UNIX_DGRAM)"),
        );
        return false;
    }
    let Some(address) = address_of(this.socket_type(), host, port, !this.flag(Flag::Async)) else {
        return false;
    };
    let fd = this.socket().fd();
    if address.sendto(fd, data, 0).is_none() {
        log::set_last_error(errno());
        return false;
    }
    true
}

/// `Client::send(data, length, flags)`: the bytes sent, or none (`SW_ERR`).
#[must_use]
pub fn send(this: Handle, data: &[u8], flags: c_int) -> Option<usize> {
    if this.flag(Flag::Dtls) || this.flag(Flag::Stream) && !this.flag(Flag::Async) {
        (this.get().socket_ops.send_sync)(this.socket(), data, flags)
    } else if this.flag(Flag::Stream) {
        send_async(this, data)
    } else {
        // A short datagram is a failure.
        this.sendto(data).filter(|&n| n >= data.len())
    }
}

/// `Client_tcp_send_async`: queue on the reactor, raising `onBufferFull` at the high watermark.
fn send_async(this: Handle, data: &[u8]) -> Option<usize> {
    let mut n = Some(data.len());
    // The live client's socket, deleted from the reactor before its record is freed.
    if (this.get().socket_ops.write)(this.socket(), data).is_none() {
        if log::last_error() != ERROR_OUTPUT_BUFFER_OVERFLOW {
            return None;
        }
        n = None;
        this.set(Flag::HighWatermark, true);
    }
    if this.has(ClientEvent::BufferFull)
        && !this.flag(Flag::HighWatermark)
        && this.queued() >= this.get().high_watermark.get()
    {
        this.set(Flag::HighWatermark, true);
        this.call(ClientEvent::BufferFull, core::ptr::null(), 0);
    }
    n
}

/// `Client::sendfile(file, offset, length)`.
#[must_use]
pub fn sendfile(this: Handle, file: &CStr, offset: i64, length: usize) -> bool {
    if !this.flag(Flag::Stream) {
        return false;
    }
    let is_async = this.flag(Flag::Async);
    let socket = this.socket();
    let sent = if is_async {
        (this.get().socket_ops.sendfile_async)(socket, file, offset, length)
    } else {
        (this.get().socket_ops.sendfile_sync)(socket, file, offset, length)
    };
    if !sent {
        log::set_last_error(errno());
        return false;
    }
    if is_async && !this.event(EventOp::AddOrUpdate, Events::WRITE) {
        return false;
    }
    true
}

/// `Client::recv(data, length, flags)`: the bytes read, or none (`SW_ERR`).
#[must_use]
pub fn recv(this: Handle, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
    if this.flag(Flag::Stream) || this.flag(Flag::Dtls) {
        let n = net::recv_sync(this.socket(), buffer, flags);
        // A poll timeout reads as recv(2)'s EAGAIN, as it did before poll.
        if n.is_none() && log::last_error() == ERROR_SOCKET_POLL_TIMEOUT {
            set_errno(libc::EAGAIN);
        }
        n
    } else if this.flag(Flag::Async) {
        this.recvfrom_sync(buffer, flags)
    } else {
        this.recvfrom(buffer, flags)
    }
}

/// A handshake in the read or write handler failed: close and report.
fn connect_failed(this: Handle) -> bool {
    this.set(Flag::Active, false);
    close(this);
    if this.has(ClientEvent::Error) {
        this.call(ClientEvent::Error, core::ptr::null(), 0);
    }
    true
}

/// `Client_onStreamRead`: the proxy and TLS handshakes that finish a connect,
/// then framed or plain reads, on a live async stream client with its input buffer.
fn on_read(this: Handle) -> bool {
    let Tail { str: buf, size } = this.input_tail();
    let do_ssl_handshake = this.flag(Flag::OpenSsl);
    let http = this.proxy_state(Proxy::Http);
    if http.is_some_and(|state| state != HTTP_PROXY_READY) {
        let accepted = this.edit_input(|input, socket| {
            net::recv_append(socket, input) > Some(0)
                && swoole_rs_http_ffi::proxy_accept(input.view())
        });
        if !accepted {
            log::set_last_error(ERROR_HTTP_PROXY_HANDSHAKE_ERROR);
            return connect_failed(this);
        }
        this.set_proxy_state(Proxy::Http, HTTP_PROXY_READY);
        this.input_clear();
        if !do_ssl_handshake {
            this.on_connect();
            return true;
        }
    }
    let socks5 = this.proxy_state(Proxy::Socks5);
    if socks5.is_some_and(|state| state != SOCKS5_READY) {
        let n = this.edit_input(|input, socket| net::recv_append(socket, input));
        if n <= Some(0) {
            log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        // Upstream hands the handshake the buffer's free size, not what was read.
        if !this.socks5_handshake(buf, size) {
            log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.proxy_state(Proxy::Socks5) != Some(SOCKS5_READY) {
            return true;
        }
        this.input_clear();
        if !do_ssl_handshake {
            this.on_connect();
            return true;
        }
    }
    if this.flag(Flag::OpenSsl) && this.tls_state() != SSL_STATE_READY {
        if !ssl_handshake(this) {
            log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.tls_state() == SSL_STATE_READY {
            this.on_connect();
        }
        return true;
    }
    // No reference crosses the receiving callback, which may destroy the client.
    let record = this.record();
    let (eof, length) = (record.open_eof_check, record.open_length_check);
    if eof || length {
        // `swoole_init` registered them before any client existed.
        let framing = &native_host().framing;
        let framed = if eof { framing.eof } else { framing.length };
        let socket = this.socket();
        let protocol = this.protocol();
        // The live protocol, socket and input buffer `Framing::new` asks for. A
        // package callback may destroy the client; its socket outlives this handler.
        let n = framed(protocol, socket, this.input());
        // A destroyed client cleared its object.
        let released = socket.read(|socket| socket.object.is_null());
        if n < 0 && !released && !this.flag(Flag::Closed) {
            close(this);
        }
        return true;
    }
    let Some(n) = this.input_recv() else {
        let socket = this.socket();
        return match socket.catch_error(errno()) {
            Caught::Error => {
                let fd = socket.fd();
                log::sys_warning(
                    c"Client_onStreamRead",
                    format_args!("Read from socket[{fd}] failed"),
                );
                true
            }
            Caught::Close => close(this),
            Caught::Wait | Caught::Invalid | Caught::ReduceSize => true,
        };
    };
    if n == 0 {
        return close(this);
    }
    this.call(ClientEvent::Receive, buf, n);
    true
}

/// `Client_onDgramRead`, on a live async datagram client.
fn on_datagram(this: Handle) -> bool {
    #[expect(
        clippy::large_stack_arrays,
        reason = "upstream's stack buffer: nothing to drop if the callback longjmps"
    )]
    let mut buffer = [0_u8; BUFFER_SIZE_UDP];
    // Upstream's datagram receive, even once DTLS made `recv` a stream one.
    let n = if this.flag(Flag::Async) {
        this.recvfrom_sync(&mut buffer, 0)
    } else {
        this.recvfrom(&mut buffer, 0)
    };
    let Some(n) = n else {
        return false;
    };
    this.call(ClientEvent::Receive, buffer.as_ptr().cast(), n);
    true
}

/// `Client_onWrite`: drain queued output, or finish an async connect, on a live
/// async stream client; `reactor` and `event` are the handler's.
fn on_write(this: Handle, reactor: Running<'_>, event: &mut Event) -> bool {
    if this.flag(Flag::Active) {
        if this.flag(Flag::OpenSsl) && this.tls_state() == SSL_STATE_WAIT_STREAM {
            if !ssl_handshake(this) {
                log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
                return write_failed(this);
            }
            if this.tls_state() == SSL_STATE_READY {
                this.on_connect();
                return true;
            }
            if this.socket().read(|socket| socket.tls.want_read) {
                this.event(EventOp::Set, Events::READ);
            }
            return true;
        }
        if (this.get().socket_ops.writable)(reactor, event) < 0 {
            return false;
        }
        if this.has(ClientEvent::BufferEmpty)
            && this.flag(Flag::HighWatermark)
            && this.queued() <= this.get().low_watermark.get()
        {
            this.set(Flag::HighWatermark, false);
            this.call(ClientEvent::BufferEmpty, core::ptr::null(), 0);
        }
        return true;
    }
    let mut error = 0;
    let read = (this.get().socket_ops.socket_error)(this, &mut error);
    log::set_last_error(error);
    if !read {
        let fd = this.socket().fd();
        log::sys_warning(c"Client_onWrite", format_args!("getsockopt({fd}) failed"));
        return false;
    }
    if error != 0 {
        return write_failed(this);
    }
    this.event(EventOp::Set, Events::READ);
    this.set(Flag::Active, true);
    if this.proxy_state(Proxy::Socks5) == Some(SOCKS5_WAIT) {
        let length = this.socks5_negotiate();
        this.set_proxy_state(Proxy::Socks5, SOCKS5_HANDSHAKE);
        return this.socks5_send(length).is_some();
    }
    if this.proxy_state(Proxy::Http) == Some(HTTP_PROXY_WAIT) {
        // A copy: the async send may raise `onBufferFull`, whose callback may
        // reuse the shared buffer the request was packed into.
        let request = this.with_buffer(|shared| this.http_pack(shared).to_vec());
        return send(this, &request, 0).is_some();
    }
    if this.flag(Flag::OpenSsl) {
        if !ssl_handshake(this) {
            log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return write_failed(this);
        }
        this.socket()
            .edit(|socket| socket.tls.state = SSL_STATE_WAIT_STREAM);
        return true;
    }
    this.on_connect();
    true
}

/// The write handler's failure: unlike the read handler's, `onError` is required.
fn write_failed(this: Handle) -> bool {
    this.set(Flag::Active, false);
    close(this);
    this.call(ClientEvent::Error, core::ptr::null(), 0);
    true
}

/// `Client_onError`, as [`on_write`].
fn on_error(this: Handle, reactor: Running<'_>, event: &mut Event) -> bool {
    if this.flag(Flag::Active) {
        return close(this);
    }
    on_write(this, reactor, event);
    true
}

/// `Client_onTimeout`: the async connect of the client timed out.
fn on_timeout(this: Handle) {
    log::set_last_error(libc::ETIMEDOUT);
    this.get().timer.set(None);
    if this.flag(Flag::OpenSsl) && this.tls_state() != SSL_STATE_READY {
        this.set(Flag::Active, false);
    }
    let socks5 = this.proxy_state(Proxy::Socks5);
    let http = this.proxy_state(Proxy::Http);
    if socks5.is_some_and(|state| state != SOCKS5_READY)
        || http.is_some_and(|state| state != HTTP_PROXY_READY)
    {
        this.set(Flag::Active, false);
    }
    close(this);
    if this.has(ClientEvent::Error) {
        this.call(ClientEvent::Error, core::ptr::null(), 0);
    }
}

/// Connects to the resolved `address`, or reports the failure through `onError`.
/// `this` is a live async client waiting for DNS.
fn resolved(this: Handle, error: c_int, address: &CStr) {
    this.set(Flag::WaitDns, false);
    this.set(Flag::DnsCompleted, true);
    if error == 0 {
        // The application can't see connect's result here, so onError reports it.
        let timeout = this.socket().read(|socket| socket.timeouts.connect);
        let (port, flags) = (this.get().server_port.get(), this.get().sock_flags.get());
        if connect(this, address, port, timeout, flags) || this.flag(Flag::OnerrorCalled) {
            return;
        }
    } else {
        log::set_last_error(ERROR_DNSLOOKUP_RESOLVE_FAILED);
    }
    this.mark_removed();
    close(this);
    if this.has(ClientEvent::Error) {
        this.set(Flag::OnerrorCalled, true);
        this.call(ClientEvent::Error, core::ptr::null(), 0);
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/modules/clients/socket_ops.rs"]
mod socket_ops_tests;
