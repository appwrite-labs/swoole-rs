//! Rust-owned `ListenPort` settings and native socket/statistic loans.
use crate::config::{Published, Setting, Shared};
use core::ffi::{CStr, c_int, c_void};
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::Ordering::Relaxed;
use std::sync::{Mutex, OnceLock, RwLock};

use swoole_rs_corebase::StringSlot;
use swoole_rs_corebase::constants::{SW_DATA_EOF, SW_INPUT_BUFFER_SIZE};
use swoole_rs_corebase::net_types::Protocol;
use swoole_rs_reactor::Socket;
use swoole_rs_tls::{DtlsSession, SslVersion, TlsContext, swr_v1_tls_owner_view};

use crate::view::Port;

/// A readable event on a live connection of `server`, which the server's connection
/// read lends a port's [`Reader`] on the connection's reactor thread, which alone
/// touches the connection meanwhile. Only that read makes one.
pub struct ReadEvent<'a> {
    server: crate::view::Server<'a>,
    event: &'a mut swoole_rs_reactor::Event,
}

impl<'a> ReadEvent<'a> {
    /// The read's server and its event, which is the connection's as lent: the reader
    /// reads the connection off it before anything changes it.
    pub(crate) fn new(
        server: crate::view::Server<'a>,
        event: &'a mut swoole_rs_reactor::Event,
    ) -> Self {
        Self { server, event }
    }

    /// The server and the event.
    #[must_use]
    pub fn into_parts(self) -> (crate::view::Server<'a>, &'a mut swoole_rs_reactor::Event) {
        (self.server, self.event)
    }
}

/// `int (*onRead)(Reactor *, ListenPort *, Event *)`: a port's readable callback, which
/// the server's connection read calls under the running reactor, with the
/// connection's port.
pub type Reader = fn(swoole_rs_reactor::Running<'_>, Port<'_>, ReadEvent<'_>) -> c_int;

/// `ListenPort::onRead`'s slot: null, or a [`Reader`].
#[repr(transparent)]
pub struct ReaderSlot(AtomicPtr<c_void>);

impl ReaderSlot {
    /// The slot holds `reader`.
    pub fn set(&self, reader: Option<Reader>) {
        let raw = reader.map_or(core::ptr::null_mut(), |reader| reader as *mut c_void);
        self.0.store(raw, Relaxed);
    }

    /// The reader the slot holds.
    #[must_use]
    pub fn get(&self) -> Option<Reader> {
        let raw = self.0.load(Relaxed);
        // SAFETY: the slot is private and `set` stores only null or a `Reader`, a fn
        // pointer, which `Option` lays out with null as `None`.
        unsafe { core::mem::transmute::<*mut c_void, Option<Reader>>(raw) }
    }

    /// The reader's address, null for none.
    #[must_use]
    pub fn address(&self) -> *mut c_void {
        self.0.load(Relaxed)
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
    /// On a port being made, before the server keeps it.
    pub ssl_context_init: fn(&mut PortConfig) -> bool,
    /// `ssl_create(sock)`, of a socket the port accepted.
    pub ssl_create: fn(&PortConfig, sock: Option<Socket<'_>>) -> bool,
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
    /// `destroy_http_request(conn)`, of a live connection.
    pub destroy_http_request: fn(conn: crate::Connection<'_>),
    /// Releases the port's context and its TLS state, as `ListenPort`'s destructor.
    pub tls_free: fn(&mut PortConfig),
    /// `create_dtls_session(sock)` on a DTLS port; null if it fails to initialize.
    pub dtls_create: fn(Port<'_>, Socket<'_>) -> *mut DtlsSession,
    /// The DTLS session on an fd; null if none.
    pub dtls_session: fn(&PortConfig, c_int) -> *mut DtlsSession,
    /// Frees the DTLS session on an fd, if any.
    pub dtls_free: fn(&PortConfig, c_int),
}

static METHODS: OnceLock<PortMethods> = OnceLock::new();

/// Registers crates/ports' methods; later registrations are ignored. Only the
/// server calls them, so no safe caller elsewhere can pass them a bad pointer.
///
/// # Safety
/// Each method is sound for any live port, socket or connection of the server,
/// which is all the server passes them, and `close` and `close_socket` when the
/// server calls them as `ListenPort`'s own did: with no handle to the port's socket
/// and no borrow of what they replace held across the call.
pub unsafe fn set_methods(methods: PortMethods) {
    METHODS.get_or_init(|| methods);
}

/// The registered methods. A port exists only after `swoole_init`, which
/// registers them, so their absence aborts.
pub(crate) fn methods() -> &'static PortMethods {
    METHODS
        .get()
        .unwrap_or_else(|| swoole_rs_corebase::fatal("port methods are not registered"))
}
#[repr(C)]
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
    /// The listening socket's record (`network::Socket *`), or null: stored by
    /// `create_socket()` and `import()`, nulled by `close_socket()` and `close()`.
    /// Private: only [`Self::set_listening_socket`] stores a record here.
    pub(crate) socket: AtomicPtr<c_void>,
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
    pub gs: Shared<*mut c_void>,
    /// The Port object's property, which phpserver links and reads through `&`.
    pub ptr: AtomicPtr<c_void>,
    /// `open_websocket_{close,ping,pong}_frame` as bits 0, 1 and 2.
    pub websocket_frames: Setting<u8>,
    /// `websocket_subprotocol`, the handshake's `Sec-WebSocket-Protocol`.
    pub websocket_protocol: Published<Vec<u8>>,
    /// `ListenPort::host`, which native code reads and writes through a `RustStringView`.
    pub host: Box<StringSlot>,
    /// `ListenPort::protocol`, which native code reads and writes by reference.
    pub protocol: Shared<Protocol>,
    /// `ListenPort::onRead`, the readable callback `init_protocol` picks.
    pub on_read: ReaderSlot,
    /// The port's TLS context, published so a reader keeps it alive across close.
    /// A thin box, opaque to the native layout mirror.
    pub ssl_context: Published<TlsContext>,
    /// The SNI contexts and DTLS sessions crates/ports keeps; `ListenPort`'s
    /// destructor releases them with the context. A thin box, as native code's
    /// mirror holds a `void *` here.
    pub tls: Box<TlsState>,
    /// The owner of the callable `protocol.private_data_1` names, when the port
    /// has a `package_length_func`. A thin box: native code's mirror holds a
    /// `void *` here.
    pub length_owner: Shared<Option<LengthOwner>>,
}

/// What `ListenPort` kept in C++ containers, each entry with a reference to its
/// context. Handshakes on any reactor thread read the SNI contexts while `set()`
/// adds to them, so each is behind a lock.
#[derive(Default)]
pub struct TlsState {
    /// `sni_contexts`, in insertion order: the name and its context.
    pub sni: RwLock<Vec<(Vec<u8>, TlsContext)>>,
    /// `dtls_sessions`.
    pub dtls: Mutex<DtlsSessions>,
}

/// A port's DTLS sessions, one per peer, indexed by its fd: the session and the
/// port's context. A flat table, as the connection table is, so `on_read`'s lookup
/// per datagram stays constant-time.
pub type DtlsSessions = Vec<Option<(*mut DtlsSession, TlsContext)>>;

/// A `package_length_func`'s owner, whose callable the port's protocol names.
pub type LengthOwner = Box<Box<dyn core::any::Any>>;

impl PortConfig {
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
        self.with_ssl_fields(|_, fields| fields.protocols & SslVersion::SW_SSL_DTLS as u32 != 0)
            .unwrap_or(false)
    }

    /// `verify_peer` with no client certificate file, cafile or capath.
    #[must_use]
    pub fn ssl_verify_without_ca(&self) -> bool {
        self.with_ssl_fields(|context, fields| fields.verify_peer && context.names_no_ca())
            .unwrap_or(false)
    }

    /// Reads the context's fields while holding a reference of this call's own.
    pub(crate) fn with_ssl_fields<R>(
        &self,
        read: impl FnOnce(&TlsContext, &swoole_rs_tls::OwnedTlsFields) -> R,
    ) -> Option<R> {
        let context = self.ssl_context.get()?;
        let fields = swr_v1_tls_owner_view(&context).fields;
        // SAFETY: this call's reference keeps the owner and its fields alive.
        unsafe { fields.as_ref() }.map(|fields| read(&context, fields))
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
        #[expect(clippy::cast_possible_truncation, reason = "a four-byte literal")]
        package_eof_len: SW_DATA_EOF.len() as u8,
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
    /// The port's slot in `port_gs_list`, once the server has made it.
    pub(crate) fn counters(&self) -> Option<&crate::shared::PortCounters> {
        // SAFETY: null, or the port's slot in `port_gs_list`, mapped for the server's life.
        unsafe { self.gs.get().cast::<crate::shared::PortCounters>().as_ref() }
    }
    /// The listening socket's record as native code holds it, or null.
    #[must_use]
    pub fn socket_ptr(&self) -> *mut c_void {
        self.socket.load(Relaxed)
    }

    /// Make `socket` the port's listening socket, which the port frees as it closes.
    pub fn set_listening_socket(&self, socket: Socket<'static>) {
        self.socket.store(socket.as_ptr().cast(), Relaxed);
    }

    /// The listening socket's record, or null, taken from the port so nothing finds it
    /// once the caller frees it.
    #[must_use]
    pub fn take_socket_ptr(&self) -> *mut swoole_rs_reactor::SocketRecord {
        self.socket.swap(core::ptr::null_mut(), Relaxed).cast()
    }

    /// The port's listening socket, or `None` before it has one or after it closes
    /// (`OP_PORT_CLOSE_SOCKET`): a record live until that unsafe free, whose caller
    /// answers for the handles left.
    #[must_use]
    pub fn listening_socket(&self) -> Option<Socket<'static>> {
        // SAFETY: null, or the live listening socket the port owns: closing it frees it
        // and nulls the field in one step.
        unsafe { Socket::from_ptr(self.socket_ptr().cast()) }
    }
    /// A new port's settings.
    pub(crate) fn boxed(buffer_size: c_int) -> Box<Self> {
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
            socket: AtomicPtr::new(core::ptr::null_mut()),
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
            gs: Shared::new(core::ptr::null_mut()),
            ptr: AtomicPtr::new(core::ptr::null_mut()),
            websocket_frames: Setting::new(0),
            websocket_protocol: Published::default(),
            host: Box::default(),
            protocol: Shared::new(default_protocol()),
            on_read: ReaderSlot(AtomicPtr::new(core::ptr::null_mut())),
            ssl_context: Published::default(),
            tls: Box::default(),
            length_owner: Shared::new(None),
        })
    }

    /// `new ListenPort(serv)`: a port of `serv` with the default settings, whose box
    /// becomes the `ListenPort *`.
    pub(crate) fn new_port(serv: *mut c_void) -> Box<Self> {
        let buffer_size = swoole_rs_reactor::SOCKET_DEFAULTS.buffer_size();
        let mut config = Self::boxed(c_int::try_from(buffer_size).unwrap_or(c_int::MAX));
        config.protocol.get_mut().private_data_2 = serv;
        config
    }

    /// `delete port` of a port still in its box: its TLS state, its settings, and
    /// the record of a socket `destroy` never closed.
    pub(crate) fn delete(mut self: Box<Self>) {
        (methods().tls_free)(&mut self);
        // Upstream's `~ListenPort` leaves the length callable to the port's deref
        // (`php_swoole_server_port_deref`): one a port kept past it stays to the exit.
        #[expect(
            clippy::mem_forget,
            reason = "a port kept past this owns the callable to the exit"
        )]
        std::mem::forget(self.length_owner.replace(None));
        // SAFETY: the record of a port `destroy` never closed (a server never started),
        // which nothing else holds. Its descriptor stays open to the exit, as upstream's.
        unsafe { swoole_rs_reactor::SocketRecord::free(self.take_socket_ptr()) };
    }
}
