//! Rust-owned `ListenPort` settings and native socket/statistic loans.
use crate::config::{Published, Setting, Shared};
use core::ffi::{CStr, c_int, c_void};
use std::sync::{OnceLock, RwLock};

use swoole_rs_clients::protocol::Protocol;
use swoole_rs_corebase::constants::{SW_DATA_EOF, SW_INPUT_BUFFER_SIZE};
use swoole_rs_corebase::reactor::{Heap, KeptSockets, Socket};
use swoole_rs_corebase::tls::{SslVersion, TlsContext};
use swoole_rs_corebase::{CellExt, StringSlot};

use crate::view::Port;

/// A readable event on a live connection of `server`, which the server's connection
/// read lends a port's [`Reader`] on the connection's reactor thread, which alone
/// touches the connection meanwhile. Only that read makes one.
pub struct ReadEvent<'a> {
    server: crate::view::Server<'a>,
    event: &'a mut swoole_rs_corebase::reactor::Event,
    connection: crate::Connection<'a>,
}

impl<'a> ReadEvent<'a> {
    /// The read's server, its event and the connection of the event's socket (the
    /// slot its `object` holds).
    pub(crate) fn new(
        server: crate::view::Server<'a>,
        event: &'a mut swoole_rs_corebase::reactor::Event,
        connection: crate::Connection<'a>,
    ) -> Self {
        Self {
            server,
            event,
            connection,
        }
    }

    /// The server, the event and the connection.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        crate::view::Server<'a>,
        &'a mut swoole_rs_corebase::reactor::Event,
        crate::Connection<'a>,
    ) {
        (self.server, self.event, self.connection)
    }
}

/// `int (*onRead)(Reactor *, ListenPort *, Event *)`: a port's readable callback, which
/// the server's connection read calls under the running reactor, with the
/// connection's port; false where upstream's returns `SW_ERR`.
pub type Reader = fn(swoole_rs_corebase::reactor::Running<'_>, Port<'_>, ReadEvent<'_>) -> bool;

/// `ListenPort::onRead`'s slot: null, or a [`Reader`].
#[repr(transparent)]
pub struct ReaderSlot(Shared<Option<Reader>>);

impl ReaderSlot {
    /// The slot holds `reader`.
    pub fn set(&self, reader: Option<Reader>) {
        self.0.set(reader);
    }

    /// The reader the slot holds.
    #[must_use]
    pub fn get(&self) -> Option<Reader> {
        self.0.get()
    }

    /// The reader's address, null for none.
    #[must_use]
    pub fn address(&self) -> *mut c_void {
        self.get()
            .map_or(core::ptr::null_mut(), |reader| reader as *mut c_void)
    }
}

/// The `ListenPort` methods crates/ports defines and the server calls. Ports
/// depends on this crate, so `swoole_init` (crates/ffi) registers them with
/// [`set_methods`]. A port is one of the server's live `ListenPort`s
/// (`server_object::port_new`): its settings, or their box while the server still
/// makes it. The methods that take `&` write only the settings a `&` can
/// ([`Setting`], [`Shared`], [`Published`], the socket's atomic), so they need no
/// exclusive borrow of a port other threads read.
pub struct PortMethods {
    /// `import(fd)`, of an inherited descriptor, on a port being made.
    pub import: fn(&mut PortConfig, fd: c_int) -> bool,
    pub close: fn(&PortConfig),
    /// `ListenPort::listen()`: false when it failed.
    pub listen: fn(&PortConfig) -> bool,
    /// `ListenPort::create_socket()`: false when it failed.
    pub create_socket: fn(&PortConfig, reuse_port: bool) -> bool,
    pub close_socket: fn(&PortConfig),
    pub get_protocols: fn(&PortConfig) -> &'static CStr,
    pub init_protocol: fn(&PortConfig),
    /// `destroy_http_request(conn)`, of a live connection of an HTTP port: frees its
    /// request, if any.
    pub destroy_http_request: fn(conn: crate::Connection<'_>),
}

static METHODS: OnceLock<PortMethods> = OnceLock::new();

/// Registers crates/ports' methods; later registrations are ignored. Each is a safe
/// fn, sound for whatever its types admit, so registering one promises nothing; the
/// server calls `close` and `close_socket` as `ListenPort`'s own did, with no handle
/// to the port's socket and no borrow of what they replace held across the call.
pub fn set_methods(methods: PortMethods) {
    METHODS.get_or_init(|| methods);
}

/// The registered methods. A port exists only after `swoole_init`, which
/// registers them, so their absence aborts.
pub(crate) fn methods() -> &'static PortMethods {
    METHODS
        .get()
        .unwrap_or_else(|| swoole_rs_corebase::fatal("port methods are not registered"))
}

/// A `ListenPort`'s settings: a `ListenPort *` is the box of them.
pub struct PortConfig {
    pub object_id: u16,
    pub backlog: Setting<u16>,
    /// Set by `listen()` and `import()`, through `&`.
    pub listening: Setting<bool>,
    pub tcp_defer_accept: Setting<c_int>,
    pub tcp_fastopen: Setting<c_int>,
    pub tcp_keepidle: Setting<c_int>,
    pub tcp_keepinterval: Setting<c_int>,
    pub tcp_keepcount: Setting<c_int>,
    pub tcp_user_timeout: Setting<c_int>,
    pub max_idle_time: Setting<f64>,
    pub socket_buffer_size: Setting<c_int>,
    pub buffer_high_watermark: Setting<u32>,
    pub buffer_low_watermark: Setting<u32>,
    pub r#type: c_int,
    pub ssl: bool,
    pub port: Setting<c_int>,
    /// The listening socket's record (`network::Socket *`), or none: stored by
    /// `create_socket()` and `import()`, retired by `close_socket()` and `close()`. The
    /// port keeps every record it owned until [`Self::delete`], so a socket it lent
    /// stays live as long as the port.
    sockets: KeptSockets,
    pub thread_id: Shared<libc::pthread_t>,
    pub heartbeat_idle_time: Setting<u16>,
    pub open_eof_check: Setting<bool>,
    pub open_http_protocol: Setting<bool>,
    pub open_http2_protocol: Setting<bool>,
    pub open_websocket_protocol: Setting<bool>,
    pub open_length_check: Setting<bool>,
    pub open_mqtt_protocol: Setting<bool>,
    pub open_redis_protocol: Setting<bool>,
    pub open_tcp_nodelay: Setting<bool>,
    pub open_tcp_nopush: Setting<bool>,
    pub open_tcp_keepalive: Setting<bool>,
    pub kernel_socket_recv_buffer_size: Setting<c_int>,
    pub kernel_socket_send_buffer_size: Setting<c_int>,
    /// `gs`: the port's counters, shared with every process the server forks, once
    /// `Server::create` maps them; [`Self::delete`] unmaps them.
    pub(crate) gs: crate::table::Table<crate::shared::PortCounters>,
    /// Base mode's per-worker connection counts (`gs->connection_nums`): an
    /// `AtomicU32` per worker, which [`Self::delete`] unmaps.
    pub(crate) connection_nums: crate::table::Table<core::sync::atomic::AtomicU32>,
    /// The non-threaded PHP Port object's property, installed once at bind/listen.
    /// Shared callback storage only: it must not own the server. The PHP object
    /// clears its callbacks on release, including when the object is freed first.
    pub property: std::cell::OnceCell<std::rc::Rc<dyn core::any::Any>>,
    /// `open_websocket_{close,ping,pong}_frame` as bits 0, 1 and 2.
    pub websocket_frames: Setting<u8>,
    /// `websocket_subprotocol`, the handshake's `Sec-WebSocket-Protocol`.
    pub websocket_protocol: Published<Vec<u8>>,
    /// `ListenPort::host`.
    pub host: Box<StringSlot>,
    /// `ListenPort::protocol`, which native framing reads and writes by reference.
    pub protocol: Shared<Protocol>,
    /// `ListenPort::onRead`, the readable callback `init_protocol` picks.
    pub on_read: ReaderSlot,
    /// The port's TLS context, published so a reader keeps it alive across close.
    pub ssl_context: Published<TlsContext>,
    /// The SNI contexts crates/ports keeps; `ListenPort`'s
    /// destructor releases them with the context.
    pub tls: Box<TlsState>,
    /// The key of this port's `package_length_func` owners, one per thread that
    /// `set()` it ([`Self::length_owner`]).
    length_key: u64,
    /// Whether any thread holds an owner under [`Self::length_key`], so a receive asks
    /// its thread's registry only then.
    has_length_owner: core::sync::atomic::AtomicBool,
}

/// What `ListenPort` kept in C++ containers, each entry with a reference to its
/// context. Handshakes on any reactor thread read the SNI contexts while `set()`
/// adds to them, so each is behind a lock.
#[derive(Default)]
pub struct TlsState {
    /// Fully configured settings for reactor threads; replacing them never edits
    /// the configuration an existing reader holds.
    pub settings: Published<swoole_rs_corebase::tls::TlsSettings>,
    /// `sni_contexts`, in insertion order: the name and its context.
    pub sni: RwLock<
        Vec<(
            Vec<u8>,
            std::sync::Arc<swoole_rs_corebase::tls::TlsSettings>,
        )>,
    >,
}

/// A `package_length_func`'s owner, whose callable a measure hands the port's length
/// callback.
pub type LengthOwner = std::rc::Rc<dyn core::any::Any>;

/// The next port's [`PortConfig::length_key`]: never reused, so an owner a freed port
/// left in another thread's registry is never another port's.
static NEXT_LENGTH_KEY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

thread_local! {
    /// This thread's `package_length_func` owners, by port: a thread-mode worker
    /// measures with the callable its own `set()` registered (#4535). They are never
    /// dropped at thread exit, when PHP may be gone: a port's release drops its own.
    static LENGTH_OWNERS: core::cell::Cell<Vec<(u64, core::mem::ManuallyDrop<LengthOwner>)>> =
        const { core::cell::Cell::new(Vec::new()) };
}

/// `f` over this thread's owners, taken out and put back: `f` runs no PHP code.
fn with_length_owners<R>(
    f: impl FnOnce(&mut Vec<(u64, core::mem::ManuallyDrop<LengthOwner>)>) -> R,
) -> R {
    LENGTH_OWNERS.with(|cell| cell.with_taken(f))
}

impl PortConfig {
    /// This thread's `package_length_func` owner for the port, replaced by `owner`; the
    /// caller drops the previous one, whose destructor may reach the port.
    #[must_use]
    pub fn replace_length_owner(&self, owner: Option<LengthOwner>) -> Option<LengthOwner> {
        let key = self.length_key;
        if owner.is_some() {
            self.has_length_owner
                .store(true, core::sync::atomic::Ordering::Release);
        }
        with_length_owners(|owners| {
            let at = owners.iter().position(|(k, _)| *k == key);
            let previous =
                at.map(|at| core::mem::ManuallyDrop::into_inner(owners.swap_remove(at).1));
            if let Some(owner) = owner {
                owners.push((key, core::mem::ManuallyDrop::new(owner)));
            }
            previous
        })
    }

    /// Whether any thread gave the port a `package_length_func` owner, which a receive
    /// asks before it looks for this thread's.
    #[inline]
    #[must_use]
    pub fn has_length_owner(&self) -> bool {
        self.has_length_owner
            .load(core::sync::atomic::Ordering::Acquire)
    }

    /// A reference to this thread's `package_length_func` owner for the port, for a
    /// measure to hand its length callback, which drops it; `None` without one.
    #[inline]
    #[must_use]
    pub fn length_owner(&self) -> Option<LengthOwner> {
        if !self.has_length_owner() {
            return None;
        }
        let key = self.length_key;
        with_length_owners(|owners| {
            owners
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, owner)| LengthOwner::clone(owner))
        })
    }

    /// The host's bytes, embedded NULs included; the slot keeps a NUL after them.
    #[must_use]
    pub fn host_bytes(&self) -> &[u8] {
        self.host.bytes()
    }

    /// The host up to its first NUL, as `host.c_str()` reads it.
    #[must_use]
    pub fn host_c_str(&self) -> &CStr {
        self.host.c_str()
    }

    /// `ListenPort::is_dtls()`: the port's context speaks DTLS.
    #[must_use]
    pub fn is_dtls(&self) -> bool {
        if let Some(settings) = self.tls.settings.get() {
            return SslVersion::from_bits(settings.protocols()).contains(SslVersion::SW_SSL_DTLS);
        }
        self.with_ssl_fields(|_, fields| {
            SslVersion::from_bits(fields.protocols).contains(SslVersion::SW_SSL_DTLS)
        })
        .unwrap_or(false)
    }

    /// `verify_peer` with no client certificate file, cafile or capath.
    #[must_use]
    pub fn ssl_verify_without_ca(&self) -> bool {
        if let Some(settings) = self.tls.settings.get() {
            return settings.verify_peer() && settings.names_no_ca();
        }
        self.with_ssl_fields(|context, fields| fields.verify_peer && context.names_no_ca())
            .unwrap_or(false)
    }

    /// Reads the context's fields while holding a reference of this call's own.
    pub(crate) fn with_ssl_fields<R>(
        &self,
        read: impl FnOnce(&TlsContext, &swoole_rs_corebase::tls::OwnedTlsFields) -> R,
    ) -> Option<R> {
        let context = self.ssl_context.get()?;
        Some(read(&context, &context.fields()))
    }
}

/// The protocol a new port starts with: network-u32 length framing and the
/// default EOF; `object::port_new` then stores the server.
fn default_protocol() -> Protocol {
    let mut package_eof = [0; 8];
    for (slot, byte) in package_eof.iter_mut().zip(SW_DATA_EOF.as_bytes()) {
        *slot = *byte;
    }
    Protocol {
        split_by_eof: false,
        package_eof,
        package_eof_len: u8::try_from(SW_DATA_EOF.len()).unwrap_or(u8::MAX),
        package_length_type: b'N',
        package_length_size: 4,
        package_length_offset: 0,
        package_body_offset: 4,
        package_max_length: SW_INPUT_BUFFER_SIZE,
        ..Protocol::EMPTY
    }
}

/// Allocates settings before the native port acquires any resources.
#[must_use]
pub fn swr_v1_port_config_new(buffer_size: c_int) -> *mut PortConfig {
    Box::into_raw(PortConfig::boxed(buffer_size))
}

impl PortConfig {
    /// The port's counters, once the server has made them.
    #[inline]
    pub(crate) fn counters(&self) -> Option<&crate::shared::PortCounters> {
        self.gs.records().first()
    }

    /// The per-worker connection counts; none when the server keeps one total.
    pub(crate) fn cells(&self) -> &[core::sync::atomic::AtomicU32] {
        self.connection_nums.records()
    }

    /// The listening socket's record as native code holds it, or null.
    #[must_use]
    pub fn socket_ptr(&self) -> *mut c_void {
        self.sockets
            .get()
            .map_or(core::ptr::null_mut(), |socket| socket.as_ptr().cast())
    }

    /// Make `socket` the port's listening socket, which the port owns until
    /// [`Self::delete`].
    pub fn set_listening_socket(&self, socket: swoole_rs_corebase::reactor::OwnedSocket) {
        self.sockets.put(socket);
    }

    /// The listening socket, which stops being one: off its reactor, for the caller to
    /// close its descriptor. The port keeps the record, so a handle it lent stays live.
    #[must_use]
    pub fn retire_socket(&self) -> Option<Socket<'_>> {
        self.sockets.retire()
    }

    /// The port's listening socket, or `None` before it has one or after it closes
    /// (`OP_PORT_CLOSE_SOCKET`).
    #[must_use]
    pub fn listening_socket(&self) -> Option<Heap<'_>> {
        self.sockets.get()
    }
    /// A new port's settings.
    #[must_use]
    pub fn boxed(buffer_size: c_int) -> Box<Self> {
        Box::new(Self {
            object_id: 0,
            backlog: Setting::new(512),
            listening: Setting::new(false),
            tcp_defer_accept: Setting::new(0),
            tcp_fastopen: Setting::new(0),
            tcp_keepidle: Setting::new(3600),
            tcp_keepinterval: Setting::new(60),
            tcp_keepcount: Setting::new(5),
            tcp_user_timeout: Setting::new(0),
            max_idle_time: Setting::new(0.0),
            socket_buffer_size: Setting::new(buffer_size),
            buffer_high_watermark: Setting::new(0),
            buffer_low_watermark: Setting::new(0),
            r#type: 1,
            ssl: false,
            port: Setting::new(0),
            sockets: KeptSockets::default(),
            // Zero is the original native empty sentinel.
            thread_id: Shared::new(swoole_rs_corebase::sys::zeroed()),
            heartbeat_idle_time: Setting::new(0),
            open_eof_check: Setting::new(false),
            open_http_protocol: Setting::new(false),
            open_http2_protocol: Setting::new(false),
            open_websocket_protocol: Setting::new(false),
            open_length_check: Setting::new(false),
            open_mqtt_protocol: Setting::new(false),
            open_redis_protocol: Setting::new(false),
            open_tcp_nodelay: Setting::new(false),
            open_tcp_nopush: Setting::new(true),
            open_tcp_keepalive: Setting::new(false),
            kernel_socket_recv_buffer_size: Setting::new(0),
            kernel_socket_send_buffer_size: Setting::new(0),
            gs: crate::table::Table::empty(),
            connection_nums: crate::table::Table::empty(),
            property: std::cell::OnceCell::new(),
            websocket_frames: Setting::new(0),
            websocket_protocol: Published::default(),
            host: Box::default(),
            protocol: Shared::new(default_protocol()),
            on_read: ReaderSlot(Shared::new(None)),
            ssl_context: Published::default(),
            tls: Box::default(),
            length_key: NEXT_LENGTH_KEY.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
            has_length_owner: core::sync::atomic::AtomicBool::new(false),
        })
    }

    /// `new ListenPort(serv)`: a port of `serv` with the default settings, whose box
    /// becomes the `ListenPort *`.
    pub(crate) fn new_port(serv: *mut c_void) -> Box<Self> {
        let buffer_size = swoole_rs_corebase::reactor::SOCKET_DEFAULTS.buffer_size();
        let mut config = Self::boxed(c_int::try_from(buffer_size).unwrap_or(c_int::MAX));
        config.protocol.get_mut().private_data_2 = serv;
        config
    }

    /// `delete port` of a port still in its box: its TLS state, its settings, its
    /// connection counts, and the record of a socket `destroy` never closed.
    pub(crate) fn delete(mut self: Box<Self>) {
        self.connection_nums.free();
        self.gs.free();
        crate::port_tls::tls_free(&mut self);
        // Upstream's `~ListenPort` leaves the length callable to the port's deref
        // (`php_swoole_server_port_deref`): one a port kept past it stays to the exit.
        #[expect(
            clippy::mem_forget,
            reason = "a port kept past this owns the callable to the exit"
        )]
        std::mem::forget(self.replace_length_owner(None));
        for socket in self.sockets.take_all() {
            swoole_rs_corebase::tls::upstream::socket_free(Some(socket));
        }
    }
}
