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
use crate::framing_recv::Source;
use crate::owned_proxy::SocksConfig;
use crate::protocol::{Package, Protocol};
use core::cell::{Cell, OnceCell};
use core::ffi::{c_int, c_void};
use core::marker::PhantomData;
use core::ptr::NonNull;
use std::ffi::{CStr, CString};
use std::rc::Rc;
use std::sync::Arc;
use swoole_rs_corebase::buffer::string::{OwnedString, StringBuffer};
use swoole_rs_corebase::constants::{
    FdType, SW_BUFFER_SIZE_UDP, SW_CLIENT_BUFFER_SIZE, SocketFlag, SocketType,
};
use swoole_rs_corebase::lent::{Spare, lend_spare};
use swoole_rs_corebase::net_types::{PacketLength, SocketAddress};
use swoole_rs_corebase::offload::AsyncEvent;
use swoole_rs_corebase::reactor::reactor_upstream::set_handlers;
use swoole_rs_corebase::reactor::socket_upstream::Connect;
use swoole_rs_corebase::reactor::{
    Caught, OwnedSocket, SOCKET_DEFAULTS, Socket, fd_clean, set_socket_buffer_size,
    socket_domain_and_type,
};
use swoole_rs_corebase::reactor::{Event, Events, Running, SocketFields};
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_corebase::tls::{
    OwnedTlsContext, OwnedTlsFields, SslCreateFlag, SslState, SslVersion, TlsContext, TlsSettings,
};
use swoole_rs_corebase::{BoxCellExt, CCast, CellExt, TrustedAllocator};
use swoole_rs_corebase::{ProxyState, ResolverRequest, log};
use swoole_rs_error::ErrorCode;

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

/// A notification owns its payload so its handler can close the client.
pub enum Notification {
    Connect,
    Receive(OwnedString),
    Close,
    Error(u32),
    BufferFull,
    BufferEmpty,
}

impl Notification {
    #[must_use]
    pub const fn kind(&self) -> ClientEvent {
        match self {
            Self::Connect => ClientEvent::Connect,
            Self::Receive(_) => ClientEvent::Receive,
            Self::Close => ClientEvent::Close,
            Self::Error(_) => ClientEvent::Error,
            Self::BufferFull => ClientEvent::BufferFull,
            Self::BufferEmpty => ClientEvent::BufferEmpty,
        }
    }
}

/// The receiver chooses the allocator for payloads it takes ownership of.
#[derive(Clone)]
pub struct Listener {
    allocator: TrustedAllocator,
    notify: Rc<dyn Fn(Notification)>,
}

impl Listener {
    pub fn new(allocator: TrustedAllocator, notify: impl Fn(Notification) + 'static) -> Self {
        Self {
            allocator,
            notify: Rc::new(notify),
        }
    }

    fn payload(&self, bytes: &[u8]) -> OwnedString {
        OwnedString::copy_in(bytes, self.allocator)
    }
}

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
const SOCK_UNIX_DGRAM: c_int = SocketType::SW_SOCK_UNIX_DGRAM as c_int;
const FD_STREAM_CLIENT: c_int = FdType::SW_FD_STREAM_CLIENT as c_int;
const FD_DGRAM_CLIENT: c_int = FdType::SW_FD_DGRAM_CLIENT as c_int;
const BUFFER_SIZE_UDP: usize = SW_BUFFER_SIZE_UDP as usize;
const CLIENT_BUFFER_SIZE: usize = SW_CLIENT_BUFFER_SIZE as usize;
/// `corebase::tls`'s `SW_SSL_STATE_READY`, `SW_SSL_STATE_WAIT_STREAM`, `SW_SSL_CLIENT`
/// and `SW_SSL_DTLS`.
pub use SslState::{
    SW_SSL_STATE_READY as SSL_STATE_READY, SW_SSL_STATE_WAIT_STREAM as SSL_STATE_WAIT_STREAM,
};
pub const SSL_CLIENT: c_int = SslCreateFlag::SW_SSL_CLIENT as c_int;
pub const SSL_DTLS: u32 = SslVersion::SW_SSL_DTLS.bits();
/// The proxy states of upstream's `include/swoole_proxy.h`.
pub use swoole_rs_protocols::{SOCKS5_HANDSHAKE, SOCKS5_READY, SOCKS5_WAIT};
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

/// The public, trivially copyable fields of `network::Client`.
#[derive(Clone, Copy)]
pub struct ClientRecord {
    pub http2: bool,
    pub open_length_check: bool,
    pub open_eof_check: bool,
    pub protocol: Protocol,
    pub server_addr: SocketAddress,
    pub remote_addr: SocketAddress,
    pub object: *mut c_void,
    pub buffer: *mut c_void,
}

/// One client's state: `network::Client`.
pub struct Client {
    framing: Rc<ClientFraming>,
    /// `socket`: the record the constructor made, freed with the client (an async
    /// one's free is deferred past its callbacks).
    socket: OwnedSocket,
    socket_ops: &'static SocketOps,
    flags: Cell<u32>,
    domain: c_int,
    sock_flags: Cell<c_int>,
    /// Set once, by the first `inet_addr`; read by taking it out and putting it back.
    server_host: Cell<Option<CString>>,
    server_port: Cell<c_int>,
    /// The id of the pending connect timer.
    timer: Cell<Option<swoole_rs_corebase::timer::Held>>,
    high_watermark: Cell<u32>,
    low_watermark: Cell<u32>,
    listener: Cell<Option<Listener>>,
    /// [`ClientEvent::bit`] for each event the listener hears.
    events: Cell<u32>,
    /// The `SSLContext`, none while TLS is off.
    tls: OnceCell<TlsContext>,
    /// The immutable configuration kept from the first handshake through close.
    tls_settings: OnceCell<Arc<TlsSettings>>,
    /// The proxy states owned by the client.
    socks5: Cell<Option<Box<ProxyState>>>,
    http_proxy: Cell<Option<Box<ProxyState>>>,
}

/// The input and protocol settings a framed receive keeps through callbacks.
pub(crate) struct ClientFraming {
    record: Cell<ClientRecord>,
    input: Cell<StringBuffer>,
    length_owner: Cell<Option<Rc<dyn std::any::Any>>>,
}

impl Drop for ClientFraming {
    fn drop(&mut self) {
        self.input.get_mut().dispose();
    }
}

impl Source for &ClientFraming {
    fn protocol(self) -> Protocol {
        self.record.get().protocol
    }

    /// A reference to the owner of the client's `package_length_func`, for a measure
    /// to hand its length callback, which drops it: the callable may `set()` another
    /// or close the client, and the old one's destructor must run before the
    /// callback finishes.
    fn length_owner(self) -> Option<Rc<dyn std::any::Any>> {
        self.length_owner.with_taken(|owner| owner.clone())
    }

    /// The record's protocol, in place: its callbacks take it by address.
    fn address(self) -> *const Protocol {
        let record = self.record.as_ptr();
        record
            .wrapping_byte_add(core::mem::offset_of!(ClientRecord, protocol))
            .cast()
    }
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
        self.framing.length_owner.replace(owner)
    }

    /// A copy of the public record.
    pub fn record(&self) -> ClientRecord {
        self.framing.record.get()
    }

    /// `cli->socket`: a constructed client's socket record, live until the client is
    /// destroyed (an async one's free is deferred past its callbacks).
    pub fn socket(&self) -> Option<Socket<'_>> {
        Some(self.socket.socket())
    }

    /// The client's TLS context, which `set()` configures; none while TLS is off.
    #[must_use]
    pub fn tls_context(&self) -> Option<&TlsContext> {
        self.tls.get()
    }

    /// Edit a copy of the public record and write it back, so no reference
    /// into the record is live while `edit` runs.
    pub fn edit_record<R>(&self, edit: impl FnOnce(&mut ClientRecord) -> R) -> R {
        let mut record = self.framing.record.get();
        let result = edit(&mut record);
        self.framing.record.set(record);
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
pub struct Handle<'a>(*mut Client, PhantomData<&'a Client>);

impl Handle<'_> {
    /// A handle on `client`, which came from [`create`].
    const fn raw(client: *mut Client) -> Self {
        Self(client, PhantomData)
    }

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

    fn socket(self) -> Socket<'static> {
        self.get().socket.socket()
    }

    /// The client's socket as its owner lends it to the reactor.
    fn heap(self) -> swoole_rs_corebase::reactor::Heap<'static> {
        self.get().socket.heap()
    }

    fn socket_type(self) -> c_int {
        self.socket().read(|socket| socket.socket_type)
    }

    fn tls_ready(self) -> bool {
        self.socket().read(SocketFields::tls_ready)
    }

    fn tls_awaits_stream(self) -> bool {
        self.socket().read(SocketFields::tls_awaits_stream)
    }

    /// `socket->removed = 1`: close skips the reactor.
    fn mark_removed(self) {
        self.socket().edit(SocketFields::note_left_reactor);
    }

    fn has(self, event: ClientEvent) -> bool {
        self.get().events.get() & event.bit() != 0
    }

    fn listener(self, kind: ClientEvent) -> Option<Listener> {
        self.has(kind)
            .then(|| self.get().listener.with_taken(|held| held.clone()))
            .flatten()
    }

    fn call(self, event: Notification) {
        if let Some(listener) = self.listener(event.kind()) {
            (listener.notify)(event);
        }
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
    fn recvfrom(self, spare: &mut Spare<'_>, flags: c_int) -> Option<usize> {
        let fd = self.socket().fd();
        self.get()
            .edit_record(|record| spare.recvfrom(fd, flags, &mut record.remote_addr))
    }

    /// `Socket::recvfrom_sync(data, length, flags, &remote_addr)`.
    fn recvfrom_sync(self, spare: &mut Spare<'_>, flags: c_int) -> Option<usize> {
        let socket = self.socket();
        let receive = self.get().socket_ops.recvfrom_sync;
        self.get()
            .edit_record(|record| receive(socket, spare, flags, &mut record.remote_addr))
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
        swoole_rs_corebase::tls::owner_host_c_str(
            tls.unwrap_or_else(|| swoole_rs_corebase::fatal("a TLS client has no TLS context")),
        )
    }

    /// `f` over the proxy, taken out of its slot for the call and put back after it
    /// (it stays where it is on the heap); `None` without one. `f` runs no effect that
    /// reaches the client's proxies.
    fn with_proxy<R>(self, which: Proxy, f: impl FnOnce(&ProxyState) -> R) -> Option<R> {
        self.get().proxy_slot(which).with_box(|proxy| f(proxy))
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
        self.socket().read(SocketFields::queued_bytes)
    }

    /// The input buffer (`cli->buffer`), null until an async connect makes it.
    fn input(self) -> *mut StringBuffer {
        self.record().buffer.cast()
    }

    /// `edit` over the input buffer, which exists, the socket and its operations.
    /// Each edit finishes before a callback can reenter the client.
    fn edit_input<R>(
        self,
        edit: impl FnOnce(&mut StringBuffer, Socket<'static>, &SocketOps) -> R,
    ) -> R {
        let socket = self.socket();
        let ops = self.get().socket_ops;
        if self.input().is_null() {
            swoole_rs_corebase::fatal("client input buffer is not available");
        }
        let mut buffer = self.get().framing.input.replace(StringBuffer::EMPTY);
        let result = edit(&mut buffer, socket, ops);
        self.get().framing.input.set(buffer);
        result
    }

    /// `new String(SW_CLIENT_BUFFER_SIZE)`, once.
    fn input_init(self) {
        if self.input().is_null() {
            let mut buffer = StringBuffer::EMPTY;
            buffer.alloc(CLIENT_BUFFER_SIZE, None);
            self.get().framing.input.set(buffer);
            let input = self.get().framing.input.as_ptr().cast();
            self.get().edit_record(|record| record.buffer = input);
        }
    }

    /// `delete buffer`.
    fn input_free(self) {
        if !self.input().is_null() {
            self.get()
                .edit_record(|record| record.buffer = core::ptr::null_mut());
            self.get()
                .framing
                .input
                .replace(StringBuffer::EMPTY)
                .dispose();
        }
    }

    /// `buffer->clear()`.
    fn input_clear(self) {
        self.edit_input(|buffer, _, _| buffer.clear());
    }

    /// Packs the SOCKS5 negotiation request and returns its length, or failure.
    fn socks5_negotiate(self) -> Option<usize> {
        // The SOCKS5 proxy, whose buffer the request is packed into.
        self.with_proxy(Proxy::Socks5, |proxy| {
            crate::owned_proxy::pack_negotiate_request(&crate::owned_proxy::Socks5Proxy::new(proxy))
        })
        .flatten()
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

    /// `Client::socks5_handshake`: applies the received `reply` and sends the answer
    /// the proxy packed.
    fn socks5_handshake(self, reply: &[u8]) -> bool {
        let n = self
            .with_proxy(Proxy::Socks5, |proxy| {
                crate::owned_proxy::socks5_step(&crate::owned_proxy::Socks5Proxy::new(proxy), reply)
            })
            .flatten();
        n.is_some_and(|n| n == 0 || self.socks5_send(n) == Some(n))
    }

    /// A result from the scratch buffer, without holding a client borrow across
    /// the loan or letting the buffer escape it.
    fn with_buffer<R>(self, mut f: impl FnMut(&mut StringBuffer) -> R) -> R {
        let mut result = None;
        (self.get().socket_ops.with_buffer)(&mut |buffer| {
            result = Some(f(buffer));
        });
        result
            .unwrap_or_else(|| swoole_rs_corebase::fatal("the scratch-buffer callback did not run"))
    }

    /// The HTTP proxy's CONNECT request, packed into `shared`.
    /// The host is the TLS host name when set, else the proxy's target.
    fn http_pack(self, shared: &mut StringBuffer) -> &[u8] {
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
                swoole_rs_http::proxy_connect(proxy, host, output)
            });
        });
        if packed.is_some() { shared.view() } else { &[] }
    }

    fn set_timeout(self, timeout: f64) {
        self.socket().edit(|socket| {
            socket.set_timeout(timeout, swoole_rs_corebase::reactor::TimeoutKind::ALL);
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
        self.call(Notification::Connect);
    }
}

/// `Client_onPackage`: a framed package for `onReceive`; the receive loop stops
/// once the callback left the socket waiting to close. Only a client's protocol
/// holds it, and a client's framed receive calls it with that client's socket, whose
/// free is deferred past the call: as for the reactor handlers below, the socket's
/// kind is what makes its object a client.
fn on_package(_: *const Protocol, socket: Socket<'_>, mut package: Package<'_>) -> bool {
    let client = Handle::raw(socket.read(SocketFields::attached_object));
    if let Some(listener) = client.listener(ClientEvent::Receive) {
        let payload = listener.payload(package.bytes());
        (listener.notify)(Notification::Receive(payload));
    }
    !socket.read(SocketFields::close_deferred)
}

/// The client of the reactor event's socket, None without a socket.
fn event_client(event: &Event) -> Option<Handle<'static>> {
    let socket = event.socket()?;
    Some(Handle::raw(socket.read(SocketFields::attached_object)))
}

fn on_stream_read(_: Running<'_>, event: &mut Event) -> bool {
    event_client(event).is_some_and(on_read)
}

fn on_dgram_read(_: Running<'_>, event: &mut Event) -> bool {
    event_client(event).is_some_and(on_datagram)
}

fn on_writable(reactor: Running<'_>, event: &mut Event) -> bool {
    event_client(event).is_some_and(|this| on_write(this, reactor, event))
}

fn on_failure(reactor: Running<'_>, event: &mut Event) -> bool {
    event_client(event).is_some_and(|this| on_error(this, reactor, event))
}

/// `Client::init_reactor`: the client sockets' handlers.
pub fn init_reactor(reactor: Running<'_>) {
    use swoole_rs_corebase::reactor::ErrorHandler;
    set_handlers(
        reactor,
        FD_STREAM_CLIENT,
        Some(on_stream_read),
        Some(on_writable),
        ErrorHandler::Callback(on_failure),
    );
    set_handlers(
        reactor,
        FD_DGRAM_CLIENT,
        Some(on_dgram_read),
        None,
        ErrorHandler::ConnectionDefault,
    );
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

/// `new Client(type, async)`: the new client's owner; None with errno set when the
/// socket could not be made (logged as upstream's constructor does).
#[must_use]
pub fn create(kind: c_int, is_async: bool) -> Option<Owner> {
    create_with(kind, is_async, &SocketOps::NATIVE)
}

fn create_with(kind: c_int, is_async: bool, socket_ops: &'static SocketOps) -> Option<Owner> {
    let fd_type = if is_stream(kind) {
        FD_STREAM_CLIENT
    } else {
        FD_DGRAM_CLIENT
    };
    let flags = if is_async {
        SocketFlag::SW_SOCK_NONBLOCK
    } else {
        SocketFlag::NONE
    } | SocketFlag::SW_SOCK_CLOEXEC;
    // The new socket record, which nothing else holds yet.
    let Some(socket) = (socket_ops.make_socket)(kind, fd_type, flags.bits()) else {
        let error = errno();
        log::sys_warning(c"Client", format_args!("socket() failed"));
        set_errno(error);
        return None;
    };
    let socket_type = socket.socket().edit(|record| {
        record.send_in_chunks();
        record.socket_type
    });
    let mut client = Client {
        socket,
        socket_ops,
        framing: Rc::new(ClientFraming {
            input: Cell::new(StringBuffer::EMPTY),
            length_owner: Cell::new(None),
            record: Cell::new(ClientRecord {
                http2: false,
                open_length_check: false,
                open_eof_check: false,
                protocol: Protocol {
                    package_length_type: b'N',
                    package_length_size: 4,
                    package_max_length: 2 * 1024 * 1024,
                    on_package: Some(on_package),
                    ..Protocol::EMPTY
                },
                server_addr: swoole_rs_corebase::sys::zeroed(),
                remote_addr: SocketAddress {
                    kind,
                    ..swoole_rs_corebase::sys::zeroed()
                },
                object: core::ptr::null_mut(),
                buffer: core::ptr::null_mut(),
            }),
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
    };
    client.set(Flag::Async, is_async);
    client.set(Flag::Stream, is_stream(socket_type));
    if let Some((domain, _)) = socket_domain_and_type(kind) {
        client.domain = domain;
    }
    let own = Rc::new(client);
    let client = Rc::as_ptr(&own).cast_mut();
    Handle::raw(client)
        .socket()
        .edit(|record| record.attach_object(client));
    Some(Owner {
        client,
        own: Cell::new(Some(own)),
    })
}

/// The client [`create`] made, which it lends [`Handle`]s on. Dropping it is
/// `delete client`, which may raise `onClose`, unless [`Owner::keep`] kept the
/// client.
pub struct Owner {
    /// The client `own` holds, which handles reach it by: a callback may free it.
    client: *mut Client,
    /// The only strong reference to the client; none once [`Owner::keep`] leaked it.
    own: Cell<Option<Rc<Client>>>,
}

impl Owner {
    /// A handle on the client, which can't outlive this owner.
    #[must_use]
    pub const fn handle(&self) -> Handle<'_> {
        Handle::raw(self.client)
    }

    /// Never free the client: dropping this owner leaves it, as upstream leaves a
    /// client nothing deletes.
    pub fn keep(&self) {
        let _ = self.own.take().map(Rc::into_raw);
    }
}

impl Drop for Owner {
    /// `delete client`, which may raise `onClose`. Only [`create`] makes an owner,
    /// and only this frees its client.
    fn drop(&mut self) {
        let Some(own) = self.own.take() else {
            return;
        };
        let client = self.client;
        let this = Handle::raw(client);
        if !this.flag(Flag::Closed) {
            close(this);
        }
        // A framed receive keeps the allocation alive until its callback returns.
        this.input_free();
        this.get().edit_record(|r| r.protocol = Protocol::EMPTY);
        // A pending connect timer would fire on a freed client.
        this.timer_clear();
        if this.flag(Flag::WaitDns) {
            // A lookup still in the pool finds no client when it completes.
            with_awaiting(|awaiting| awaiting.retain(|&(_, waiting)| waiting.0 != client));
        }
        let is_async = this.flag(Flag::Async);
        if is_async {
            // A cleared object tells a close in progress that the client is gone.
            this.socket().edit(SocketFields::detach_object);
        }
        drop(this.get().framing.length_owner.take());
        // Only the owner holds the client.
        let Some(Client {
            socket,
            socket_ops,
            socks5,
            http_proxy,
            ..
        }) = Rc::into_inner(own)
        else {
            return;
        };
        if is_async {
            (socket_ops.free_socket)(Some(socket));
        } else {
            // `delete socket`.
            socket.free();
        }
        // The proxies first, then the rest of the state.
        drop(socks5);
        drop(http_proxy);
    }
}

impl<'a> Handle<'a> {
    /// Lend the async input allocation to one framing loop. A nested notification
    /// leaves the pending bytes to the loop that already owns them.
    fn with_framed(
        self,
        receive: impl FnOnce(&ClientFraming, Socket<'_>, &mut StringBuffer) -> bool,
    ) -> bool {
        let framing = self.get().framing.clone();
        let mut buffer = framing.input.replace(StringBuffer::EMPTY);
        if buffer.size() == 0 {
            return !self.input().is_null();
        }
        let result = receive(&framing, self.socket(), &mut buffer);
        framing.input.set(buffer);
        result
    }

    /// The client, live while its owner is.
    #[must_use]
    pub fn client(self) -> &'a Client {
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
pub fn package_length(this: Handle<'_>, data: &[u8]) -> Option<usize> {
    let record = this.record();
    let mut packet = PacketLength::new(data);
    record.protocol.get_package_length?(
        &record.protocol,
        None,
        this.get().framing.length_owner(),
        &mut packet,
    )
}

/// `Client::close()`: false when already closed or when `close(2)` fails; true when
/// it closed or `onClose` released the client.
pub fn close(this: Handle<'_>) -> bool {
    if this.flag(Flag::Closed) {
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
        saved.read(SocketFields::unlink_unix_path);
    }
    if this.flag(Flag::Async) {
        if !saved.read(SocketFields::has_left_reactor) {
            _ = this.event(EventOp::Del, Events::NONE);
        }
        this.timer_clear();
        if this.flag(Flag::Active) {
            this.set(Flag::Active, false);
            // onClose may destroy the client: only the saved socket is read after it.
            this.call(Notification::Close);
        }
    } else {
        this.set(Flag::Active, false);
    }
    // The saved record's free is deferred past this call.
    saved
        .edit(|socket| socket.has_object().then(|| socket.close_descriptor()))
        .unwrap_or(true)
}

/// `Client::shutdown(how)`. Upstream's `SHUT_RDWR` records only a read shutdown.
#[must_use]
pub fn shutdown(this: Handle<'_>, how: c_int) -> bool {
    if this.flag(Flag::Closed) {
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
pub fn sleep(this: Handle<'_>) -> bool {
    if this
        .socket()
        .read(|socket| socket.registered_for(Events::WRITE))
    {
        this.event(EventOp::Set, Events::WRITE)
    } else {
        this.event(EventOp::Del, Events::NONE)
    }
}

/// `Client::wakeup()`: read again. Whether it worked.
#[must_use]
pub fn wakeup(this: Handle<'_>) -> bool {
    if this
        .socket()
        .read(|socket| socket.registered_for(Events::WRITE))
    {
        this.event(EventOp::Set, Events::READ | Events::WRITE)
    } else {
        this.event(EventOp::Add, Events::READ)
    }
}

/// `swoole_event_set(socket, SW_EVENT_WRITE)`, to finish a handshake: whether it
/// worked.
#[must_use]
pub fn ssl_wait(this: Handle<'_>) -> bool {
    this.event(EventOp::Set, Events::WRITE)
}

/// `Client::enable_ssl_encrypt()`: a TLS context, TLS on, and DTLS for a datagram
/// client. The context, or None when TLS was already on.
fn enable_ssl(this: Handle<'_>) -> Option<NonNull<OwnedTlsContext>> {
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
    let this = Handle::raw(core::ptr::from_ref(client).cast_mut());
    let port = port.c_cast::<c_int>();
    let which = if http { Proxy::Http } else { Proxy::Socks5 };
    let proxy = if http {
        Some(swoole_rs_http::http_proxy(host, port, user, password))
    } else {
        match crate::owned_proxy::admit(user.len(), password.len()) {
            SocksConfig::Refused => None,
            config => {
                let owner = ProxyState::new(true);
                let texts = [host, user, password];
                // An allocation failure leaves the strings it didn't reach empty.
                let credentials = config == SocksConfig::Credentials;
                _ = owner.configure(texts, port, credentials, this.socket_type());
                Some(owner)
            }
        }
    };
    // The replaced proxy is released here.
    drop(client.proxy_slot(which).replace(proxy));
}

/// `Client::enable_dtls()`: the context's protocols become DTLS.
fn enable_dtls(this: Handle<'_>) {
    this.edit_tls(|fields, _| fields.protocols = SSL_DTLS);
    this.socket().edit(|socket| {
        socket.become_dtls();
        socket.send_in_dtls_records();
    });
    this.set(Flag::Dtls, true);
}

fn tls_freeze(owner: &TlsContext) -> Option<TlsSettings> {
    owner.borrow_configuration().freeze(None)
}

/// `Client::ssl_handshake()`.
#[must_use]
pub fn ssl_handshake(this: Handle<'_>) -> bool {
    if this.tls_ready() {
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
        this.socket().edit(SocketFields::send_through_tls);
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
    if this.tls_ready() && settings.verify_peer() && !ssl_verify(this, settings.allow_self_signed())
    {
        return false;
    }
    true
}

/// `Client::ssl_verify(allow_self_signed)`: the peer's certificate, and its host
/// when the context names one.
#[must_use]
pub fn ssl_verify(this: Handle<'_>, allow_self_signed: bool) -> bool {
    if !(this.get().socket_ops.tls_verify)(this.socket(), allow_self_signed) {
        return false;
    }
    let name = this.tls_host_name();
    name.is_empty() || (this.get().socket_ops.tls_check_host)(this.socket(), &name)
}

/// `Client_inet_addr`: the first call routes through the proxies and records the
/// server; every call assigns `server_addr`, or waits for DNS. False when the
/// address does not assign and no DNS wait covers it.
fn inet_addr(this: Handle<'_>, host: &CStr, port: c_int) -> bool {
    let (mut host, mut port) = (host, port);
    // The last proxy's host, NUL-terminated, which `host` borrows.
    let mut proxy_host;
    if !this.flag(Flag::HostPreset) {
        for which in [Proxy::Socks5, Proxy::Http] {
            let Some(next) = this.with_proxy(which, |proxy| {
                proxy.set_string(3, host.to_bytes());
                proxy.update(|fields| fields.target_port = port);
                (proxy.string_with_nul(0), proxy.fields().port)
            }) else {
                continue;
            };
            (proxy_host, port) = next;
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
fn dns_lookup(this: Handle<'_>) -> bool {
    // The request copies the host's bytes, so nothing points into it once it is back.
    let request = this.get().server_host.with_taken(|host| {
        let name = host.as_deref().map_or(&b""[..], CStr::to_bytes);
        crate::resolver::Request::new(this.get().domain, 0, 0, name, b"")
    });
    let id = LAST_LOOKUP.get() + 1;
    LAST_LOOKUP.set(id);
    // The entry outlives this call; `free` drops it with the client.
    #[cfg_attr(
        protocol_buffer_lint,
        expect(
            clippy::disallowed_methods,
            reason = "one registry entry per pending local DNS lookup, removed on completion or client teardown"
        )
    )]
    with_awaiting(|awaiting| awaiting.push((id, Handle::raw(this.0))));
    let lookup = Lookup(id);
    // The pool's copy holds its own reference to the request.
    let dispatched = (this.get().socket_ops.dispatch_dns)(lookup, &request);
    if !dispatched {
        _ = awaiting(lookup);
    }
    dispatched
}

/// A client's DNS lookup in the async pool: what its event carries back to
/// [`resolve_completed`] on the client's thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Lookup(usize);

fn dispatch_dns(lookup: Lookup, request: &crate::resolver::Request) -> bool {
    let mut event = AsyncEvent::new(
        std::ptr::without_provenance_mut(lookup.0),
        Some(crate::resolver::handler_gethostbyname),
        Some(client_completed),
        std::ptr::null_mut(),
    );
    event.set_record(request.reference());
    !swoole_rs_corebase::offload::threads::dispatch(&event).is_null()
}

extern "C" fn client_completed(event: &AsyncEvent) {
    if let Some(request) = event.record() {
        resolve_completed(Lookup(event.object().addr()), request, event.result().1);
    }
}

thread_local! {
    /// The last lookup's id on this thread.
    static LAST_LOOKUP: Cell<usize> = const { Cell::new(0) };
    /// The clients awaiting a lookup on this thread, each with its lookup's id;
    /// freeing a client drops its entry.
    static AWAITING: Cell<Vec<(usize, Handle<'static>)>> = const { Cell::new(Vec::new()) };
}

/// `f` over the clients awaiting a lookup, taken out and put back.
fn with_awaiting<R>(f: impl FnOnce(&mut Vec<(usize, Handle<'static>)>) -> R) -> R {
    AWAITING.with(|slot| slot.with_taken(f))
}

/// The client awaiting `lookup`, now no longer: none once it was freed.
fn awaiting(lookup: Lookup) -> Option<Handle<'static>> {
    with_awaiting(|awaiting| {
        let at = awaiting.iter().position(|&(id, _)| id == lookup.0)?;
        Some(awaiting.swap_remove(at).1)
    })
}

/// Completes a client's DNS lookup on its reactor thread, while the pool still
/// owns the request. A client freed meanwhile is not reached.
fn resolve_completed(lookup: Lookup, request: &ResolverRequest, error: c_int) {
    let Some(this) = awaiting(lookup) else {
        return;
    };
    let address = crate::resolver::address(request);
    resolved(this, error, &crate::resolver::c_name(&address));
}

/// `Client::connect(host, port, timeout, flags)`: `host` is NUL-terminated.
/// Whether it connected, or, async, began to.
#[must_use]
pub fn connect(this: Handle<'_>, host: &CStr, port: c_int, timeout: f64, flags: c_int) -> bool {
    if !this.flag(Flag::Stream) {
        udp_connect(this, host, port, timeout, flags)
    } else if this.flag(Flag::Async) {
        tcp_connect_async(this, host, port, timeout)
    } else {
        tcp_connect_sync(this, host, port, timeout, flags != 0)
    }
}

/// Receive one complete SOCKS5 reply per handshake step, leaving tunnel data queued.
fn socks5_receive_sync(this: Handle<'_>, timeout: f64) -> bool {
    let started = std::time::Instant::now();
    // The longest reply is a CONNECT with a 255-byte domain and a port.
    let mut reply = [0; 262];
    let mut used = 0;
    while let Some(state) = this.proxy_state(Proxy::Socks5) {
        if state == SOCKS5_READY {
            return true;
        }
        let Some((prefix, spare)) = reply.split_at_mut_checked(used) else {
            return false;
        };
        let Some(end) = swoole_rs_protocols::socks_reply_length(state, prefix) else {
            log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
            return false;
        };
        if end == used {
            if !this.socks5_handshake(prefix) {
                return false;
            }
            used = 0;
            continue;
        }
        if timeout > 0.0 {
            let remaining = timeout - started.elapsed().as_secs_f64();
            if remaining <= 0.0 {
                log::set_last_error(ERROR_SOCKET_POLL_TIMEOUT);
                return false;
            }
            this.socket().edit(|socket| {
                socket.set_timeout(
                    remaining.max(0.001),
                    swoole_rs_corebase::reactor::TimeoutKind::READ,
                );
            });
        }
        let Some(room) = spare.get_mut(..end.saturating_sub(used)) else {
            return false;
        };
        match recv(this, room, 0) {
            Some(0) => {
                log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
                return false;
            }
            Some(n) => used += n,
            None => {
                if timeout > 0.0 && log::last_error() == libc::ETIMEDOUT {
                    log::set_last_error(ERROR_SOCKET_POLL_TIMEOUT);
                }
                return false;
            }
        }
    }
    false
}

/// `Client_tcp_connect_sync`: connect, then the proxy handshake and TLS.
fn tcp_connect_sync(
    this: Handle<'_>,
    host: &CStr,
    port: c_int,
    timeout: f64,
    nonblock: bool,
) -> bool {
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
        if this
            .socks5_negotiate()
            .and_then(|n| this.socks5_send(n))
            .is_none()
        {
            return false;
        }
        this.set_proxy_state(Proxy::Socks5, SOCKS5_HANDSHAKE);
        let timeout = this.socket().read(|socket| socket.io_timeout(false));
        let ready = socks5_receive_sync(this, timeout);
        this.socket().edit(|socket| {
            socket.set_timeout(timeout, swoole_rs_corebase::reactor::TimeoutKind::READ);
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
            n > 0 && swoole_rs_http::proxy_accept(shared.view())
        })
    {
        return false;
    }
    !this.flag(Flag::OpenSsl) || ssl_handshake(this)
}

/// `Client_tcp_connect_async`: a nonblocking connect finished by the write handler.
fn tcp_connect_async(this: Handle<'_>, host: &CStr, port: c_int, timeout: f64) -> bool {
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
        let mark =
            (f64::from(this.socket().read(SocketFields::output_limit)) * 0.8).c_cast::<u32>();
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
        this.call(Notification::Error(log::last_error().unsigned_abs()));
    }
    false
}

/// `Client_udp_connect`.
fn udp_connect(
    this: Handle<'_>,
    host: &CStr,
    port: c_int,
    timeout: f64,
    udp_connect: c_int,
) -> bool {
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
                this.call(Notification::Error(log::last_error().unsigned_abs()));
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
fn bind_unix(this: Handle<'_>) -> bool {
    let socket = this.socket();
    let pid = std::process::id();
    let path = format!("/tmp/swoole-client.{pid}.{}.sock", socket.fd());
    let bound = std::ffi::CString::new(path.as_str()).unwrap_or_default();
    _ = swoole_rs_corebase::sys::unlink(&bound);
    let bound_ok = socket.edit(|socket| socket.bind_unix_path(path.as_bytes()));
    if !bound_ok {
        log::sys_warning(c"Client_udp_connect", format_args!("bind({path}) failed"));
    }
    bound_ok
}

type BufferLoan = fn(&mut dyn FnMut(&mut StringBuffer));

/// Host effects selected at construction; callbacks copy the function pointer
/// before an effect can release the client.
struct SocketOps {
    dispatch_dns: fn(Lookup, &crate::resolver::Request) -> bool,
    /// Lends the scratch buffer to the callback exactly once, synchronously.
    with_buffer: BufferLoan,
    recv: fn(Socket<'_>, &mut Spare<'_>, c_int) -> Option<usize>,
    recv_sync: fn(Socket<'_>, &mut Spare<'_>, c_int) -> Option<usize>,
    recvfrom_sync: fn(Socket<'_>, &mut Spare<'_>, c_int, &mut SocketAddress) -> Option<usize>,
    send_sync: fn(Socket<'_>, &[u8], c_int) -> Option<usize>,
    sendfile_sync: fn(Socket<'_>, &CStr, i64, usize) -> bool,
    sendfile_async: fn(Socket<'_>, &CStr, i64, usize) -> bool,
    free_socket: fn(Option<OwnedSocket>),
    close_tls: fn(Socket<'_>),
    write: fn(swoole_rs_corebase::reactor::Heap<'_>, &[u8]) -> Option<usize>,
    writable: swoole_rs_corebase::reactor::Handler,
    make_socket: fn(c_int, c_int, c_int) -> Option<OwnedSocket>,
    kernel_timeout: fn(Handle<'_>, f64),
    assign: fn(Handle<'_>, &CStr, c_int, bool) -> bool,
    connect_async: fn(Handle<'_>) -> Connect,
    connect: fn(Handle<'_>, ConnectMode) -> bool,
    event: fn(Handle<'_>, EventOp, Events) -> bool,
    socket_error: fn(Handle<'_>, &mut c_int) -> bool,
    timer_add: fn(f64, Handle<'_>) -> Option<swoole_rs_corebase::timer::Held>,
    timer_delete: fn(swoole_rs_corebase::timer::Held),
    tls_freeze: fn(&TlsContext) -> Option<TlsSettings>,
    tls_session: fn(&TlsSettings, Socket<'_>) -> bool,
    tls_connect: fn(Socket<'_>) -> bool,
    tls_set_host_name: fn(Socket<'_>, &CStr) -> bool,
    tls_verify: fn(Socket<'_>, bool) -> bool,
    tls_check_host: fn(Socket<'_>, &CStr) -> bool,
}

impl SocketOps {
    const NATIVE: Self = Self {
        dispatch_dns,
        with_buffer: |f| swoole_rs_corebase::buffer::string::with_thread_buffer(f),
        recv: swoole_rs_corebase::tls::upstream::recv_spare,
        recv_sync: swoole_rs_corebase::tls::upstream::recv_sync_spare,
        recvfrom_sync: swoole_rs_corebase::tls::upstream::recvfrom_sync_spare,
        send_sync: swoole_rs_corebase::tls::upstream::send_sync,
        sendfile_sync: swoole_rs_corebase::tls::upstream::sendfile_sync,
        sendfile_async: |sock, file, offset, length| {
            swoole_rs_corebase::tls::upstream::socket_sendfile_async(
                sock, file, offset, length, false,
            )
        },
        free_socket: swoole_rs_corebase::tls::upstream::socket_free,
        close_tls: swoole_rs_corebase::tls::upstream::socket_ssl_close,
        write: swoole_rs_corebase::reactor::event_api::event_write,
        writable: swoole_rs_corebase::tls::reactor_write::reactor_writable_callback,
        make_socket: ops::make_socket,
        kernel_timeout: ops::kernel_timeout,
        assign: ops::assign,
        connect_async: ops::connect_async,
        connect: ops::connect,
        event: ops::event,
        socket_error: ops::socket_error,
        timer_add: |timeout, this| {
            // `free` deletes the timer with the client.
            let this = Handle::raw(this.0);
            swoole_rs_corebase::timer::add_fn_sec(timeout, false, move || on_timeout(this))
        },
        timer_delete: swoole_rs_corebase::timer::del_id,
        tls_freeze,
        tls_session: TlsSettings::create_client_session,
        tls_connect: swoole_rs_corebase::tls::upstream::socket_ssl_connect,
        tls_set_host_name: swoole_rs_corebase::tls::upstream::socket_ssl_set_host_name,
        tls_verify: swoole_rs_corebase::tls::upstream::socket_ssl_verify,
        tls_check_host: swoole_rs_corebase::tls::upstream::socket_ssl_check_host,
    };
}

/// The reactor and socket operations used by production clients.
mod ops {
    use super::{Connect, ConnectMode, EventOp, Handle, address_of};
    use core::ffi::{CStr, c_int};
    use swoole_rs_corebase::reactor::{event_api, socket_upstream};

    /// `make_socket(type, fd_type, flags)`: the new record, or none.
    pub(super) fn make_socket(
        kind: c_int,
        fd_type: c_int,
        flags: c_int,
    ) -> Option<swoole_rs_corebase::reactor::OwnedSocket> {
        socket_upstream::make_typed(kind, fd_type, flags)
            .map(swoole_rs_corebase::reactor::OwnedSocket::new)
    }

    /// `socket->set_kernel_timeout(timeout)`, the write timeout only once the
    /// read one is set.
    pub(super) fn kernel_timeout(this: Handle<'_>, timeout: f64) {
        this.socket().edit(|socket| {
            if socket_upstream::socket_set_kernel_read_timeout(socket, timeout) {
                socket_upstream::socket_set_kernel_write_timeout(socket, timeout);
            }
        });
    }

    /// `server_addr.assign(socket_type, host, port, resolve)`.
    pub(super) fn assign(this: Handle<'_>, host: &CStr, port: c_int, resolve: bool) -> bool {
        let Some(address) = address_of(this.socket_type(), host, port, resolve) else {
            return false;
        };
        this.get()
            .edit_record(|record| record.server_addr = address);
        true
    }

    /// `connect_async` of the socket to `server_addr`; errno is the call's.
    pub(super) fn connect_async(this: Handle<'_>) -> Connect {
        let address = &this.record().server_addr;
        this.socket().edit(|socket| socket.connect_async(address))
    }

    /// Connects the socket to `server_addr`, by `mode`: whether it connected, else
    /// errno is the call's.
    pub(super) fn connect(this: Handle<'_>, mode: ConnectMode) -> bool {
        let socket = this.socket();
        let address = &this.record().server_addr;
        match mode {
            ConnectMode::Sync => socket_upstream::connect_sync(socket, address).is_ok(),
            ConnectMode::Raw => address.connect(socket.fd()),
        }
    }

    /// The reactor's `swoole_event_*` on the socket: whether it worked, false without a
    /// reactor. The client's socket stays live while it is registered: `close` deletes
    /// it from the reactor before the record is freed.
    pub(super) fn event(
        this: Handle<'_>,
        op: EventOp,
        events: swoole_rs_corebase::reactor::Events,
    ) -> bool {
        let socket = this.heap();
        match op {
            EventOp::Add => event_api::event_add(socket, events),
            EventOp::Set => event_api::event_set(socket, events),
            EventOp::Del => event_api::event_del(*socket),
            EventOp::AddOrUpdate => event_api::add_or_update(socket, events),
        }
    }

    /// `getsockopt(SOL_SOCKET, SO_ERROR)` into `error`: whether it succeeded.
    pub(super) fn socket_error(this: Handle<'_>, error: &mut c_int) -> bool {
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
pub fn sendto(this: Handle<'_>, host: &CStr, port: c_int, data: &[u8]) -> bool {
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
pub fn send(this: Handle<'_>, data: &[u8], flags: c_int) -> Option<usize> {
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
fn send_async(this: Handle<'_>, data: &[u8]) -> Option<usize> {
    let mut n = Some(data.len());
    // The live client's socket, deleted from the reactor before its record is freed.
    if (this.get().socket_ops.write)(this.heap(), data).is_none() {
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
        this.call(Notification::BufferFull);
    }
    n
}

/// `Client::sendfile(file, offset, length)`.
#[must_use]
pub fn sendfile(this: Handle<'_>, file: &CStr, offset: i64, length: usize) -> bool {
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
    !is_async || this.event(EventOp::AddOrUpdate, Events::WRITE)
}

/// `Client::recv(data, length, flags)`: the bytes read, or none (`SW_ERR`).
#[must_use]
pub fn recv(this: Handle<'_>, buffer: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
    lend_spare(buffer, |spare| recv_spare(this, spare, flags)).0
}

/// [`recv`] into lent room, which counts what the receive wrote.
#[inline]
pub fn recv_spare(this: Handle<'_>, spare: &mut Spare<'_>, flags: c_int) -> Option<usize> {
    if this.flag(Flag::Stream) || this.flag(Flag::Dtls) {
        let n = (this.get().socket_ops.recv_sync)(this.socket(), spare, flags);
        // A poll timeout reads as recv(2)'s EAGAIN, as it did before poll.
        if n.is_none() && log::last_error() == ERROR_SOCKET_POLL_TIMEOUT {
            set_errno(libc::EAGAIN);
        }
        n
    } else if this.flag(Flag::Async) {
        this.recvfrom_sync(spare, flags)
    } else {
        this.recvfrom(spare, flags)
    }
}

/// A handshake in the read or write handler failed: close and report.
fn connect_failed(this: Handle<'_>) -> bool {
    this.set(Flag::Active, false);
    close(this);
    if this.has(ClientEvent::Error) {
        this.call(Notification::Error(log::last_error().unsigned_abs()));
    }
    true
}

/// `Client_onStreamRead`: the proxy and TLS handshakes that finish a connect,
/// then framed or plain reads, on a live async stream client with its input buffer.
fn on_read(this: Handle<'_>) -> bool {
    let do_ssl_handshake = this.flag(Flag::OpenSsl);
    let http = this.proxy_state(Proxy::Http);
    if http.is_some_and(|state| state != HTTP_PROXY_READY) {
        let accepted = this.edit_input(|input, socket, ops| {
            input.fill(|room| (ops.recv)(socket, room, 0)) > Some(0)
                && swoole_rs_http::proxy_accept(input.view())
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
    if let Some(state) = this
        .proxy_state(Proxy::Socks5)
        .filter(|&s| s != SOCKS5_READY)
    {
        let reply = this.edit_input(|input, socket, ops| {
            let end = swoole_rs_protocols::socks_reply_length(state, input.view())?;
            let room = end.checked_sub(input.len())?;
            input
                .fill_within(room, |spare| (ops.recv)(socket, spare, 0))
                .filter(|&n| n > 0)?;
            let end = swoole_rs_protocols::socks_reply_length(state, input.view())?;
            // Release the buffer before sending a request or calling onConnect.
            Some((input.len() == end).then(|| {
                let frame = input.view().to_vec();
                input.clear();
                frame
            }))
        });
        match reply {
            Some(None) => return true,
            Some(Some(frame)) if this.socks5_handshake(&frame) => {}
            _ => {
                log::set_last_error(ERROR_SOCKS5_HANDSHAKE_FAILED);
                return connect_failed(this);
            }
        }
        if this.proxy_state(Proxy::Socks5) != Some(SOCKS5_READY) {
            return true;
        }
        if !do_ssl_handshake {
            this.on_connect();
            return true;
        }
    }
    if this.flag(Flag::OpenSsl) && !this.tls_ready() {
        if !ssl_handshake(this) {
            log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
            return connect_failed(this);
        }
        if this.tls_ready() {
            this.on_connect();
        }
        return true;
    }
    // No reference crosses the receiving callback, which may destroy the client.
    let record = this.record();
    let (eof, length) = (record.open_eof_check, record.open_length_check);
    if eof || length {
        let socket = this.socket();
        // A package callback may destroy the client; its socket outlives this handler.
        let received = this.with_framed(|client, socket, buffer| {
            crate::framing_recv::client_receive(client, socket, buffer, eof)
        });
        // A destroyed client cleared its object.
        let released = !socket.read(SocketFields::has_object);
        if !received && !released && !this.flag(Flag::Closed) {
            close(this);
        }
        return true;
    }
    let listener = this.listener(ClientEvent::Receive);
    let read = this.edit_input(|input, socket, ops| {
        let start = input.len();
        let n = input.fill(|room| (ops.recv)(socket, room, 0))?;
        let payload = listener
            .as_ref()
            .filter(|_| n != 0)
            .map(|listener| listener.payload(input.view().get(start..).unwrap_or_default()));
        input.truncate(start);
        Some((n, payload))
    });
    let Some((n, payload)) = read else {
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
    if let (Some(listener), Some(payload)) = (listener, payload) {
        (listener.notify)(Notification::Receive(payload));
    }
    true
}

/// `Client_onDgramRead`, on a live async datagram client.
fn on_datagram(this: Handle<'_>) -> bool {
    #[expect(
        clippy::large_stack_arrays,
        reason = "upstream's stack buffer: nothing to drop if the callback longjmps"
    )]
    let mut buffer = [0_u8; BUFFER_SIZE_UDP];
    // Upstream's datagram receive, even once DTLS made `recv` a stream one.
    let (n, filled) = lend_spare(&mut buffer, |spare| {
        if this.flag(Flag::Async) {
            this.recvfrom_sync(spare, 0)
        } else {
            this.recvfrom(spare, 0)
        }
    });
    if n.is_none() {
        return false;
    }
    if let Some(listener) = this.listener(ClientEvent::Receive) {
        let payload = listener.payload(buffer.get(..filled).unwrap_or_default());
        (listener.notify)(Notification::Receive(payload));
    }
    true
}

/// `Client_onWrite`: drain queued output, or finish an async connect, on a live
/// async stream client; `reactor` and `event` are the handler's.
fn on_write(this: Handle<'_>, reactor: Running<'_>, event: &mut Event) -> bool {
    if this.flag(Flag::Active) {
        if this.flag(Flag::OpenSsl) && this.tls_awaits_stream() {
            if !ssl_handshake(this) {
                log::set_last_error(ERROR_SSL_HANDSHAKE_FAILED);
                return write_failed(this);
            }
            if this.tls_ready() {
                this.on_connect();
                return true;
            }
            if this
                .socket()
                .read(SocketFields::tls_wants)
                .contains(Events::READ)
            {
                this.event(EventOp::Set, Events::READ);
            }
            return true;
        }
        if !(this.get().socket_ops.writable)(reactor, event) {
            return false;
        }
        if this.has(ClientEvent::BufferEmpty)
            && this.flag(Flag::HighWatermark)
            && this.queued() <= this.get().low_watermark.get()
        {
            this.set(Flag::HighWatermark, false);
            this.call(Notification::BufferEmpty);
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
        let Some(length) = this.socks5_negotiate() else {
            return false;
        };
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
        this.socket().edit(SocketFields::await_tls_stream);
        return true;
    }
    this.on_connect();
    true
}

/// The write handler's failure: unlike the read handler's, `onError` is required.
fn write_failed(this: Handle<'_>) -> bool {
    this.set(Flag::Active, false);
    close(this);
    this.call(Notification::Error(log::last_error().unsigned_abs()));
    true
}

/// `Client_onError`, as [`on_write`].
fn on_error(this: Handle<'_>, reactor: Running<'_>, event: &mut Event) -> bool {
    if this.flag(Flag::Active) {
        return close(this);
    }
    on_write(this, reactor, event);
    true
}

/// `Client_onTimeout`: the async connect of the client timed out.
fn on_timeout(this: Handle<'_>) {
    log::set_last_error(libc::ETIMEDOUT);
    this.get().timer.set(None);
    if this.flag(Flag::OpenSsl) && !this.tls_ready() {
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
        this.call(Notification::Error(log::last_error().unsigned_abs()));
    }
}

/// Connects to the resolved `address`, or reports the failure through `onError`.
/// `this` is a live async client waiting for DNS.
fn resolved(this: Handle<'_>, error: c_int, address: &CStr) {
    this.set(Flag::WaitDns, false);
    this.set(Flag::DnsCompleted, true);
    if error == 0 {
        // The application can't see connect's result here, so onError reports it.
        let timeout = this.socket().read(SocketFields::connect_timeout);
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
        this.call(Notification::Error(log::last_error().unsigned_abs()));
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/modules/clients/socket_ops.rs"]
mod socket_ops_tests;
