//! Rust-owned `ListenPort` settings and native socket/statistic loans.
use crate::config::{Published, Setting, Shared};
use core::ffi::{CStr, c_int, c_void};
use core::sync::atomic::AtomicPtr;
use std::sync::{Mutex, OnceLock, RwLock};

use swoole_rs_corebase::StringSlot;
use swoole_rs_corebase::constants::{SW_DATA_EOF, SW_INPUT_BUFFER_SIZE};
use swoole_rs_corebase::net_types::Protocol;
use swoole_rs_reactor::SocketRecord;
use swoole_rs_tls::{DtlsSession, SslVersion, TlsContext, swr_v1_tls_owner_view};

use crate::records::ConnectionRecord;

/// The `ListenPort` methods crates/ports defines and the server calls. Ports
/// depends on this crate, so `swoole_init` (crates/ffi) registers them with
/// [`set_methods`]. A `port` is one of the server's live `ListenPort`s
/// (`server_object::port_new`), passed as a raw pointer.
pub struct PortMethods {
    pub ssl_context_init: fn(port: *mut c_void) -> bool,
    /// `ssl_create(sock)`, of a socket the port accepted.
    pub ssl_create: fn(port: *mut c_void, sock: *mut c_void) -> bool,
    /// `import(fd)`, of an inherited descriptor.
    pub import: fn(port: *mut c_void, fd: c_int) -> bool,
    pub close: fn(port: *mut c_void),
    pub listen: fn(port: *mut c_void) -> c_int,
    pub create_socket: fn(port: *mut c_void) -> c_int,
    pub close_socket: fn(port: *mut c_void),
    pub get_protocols: fn(&PortConfig) -> &'static CStr,
    pub init_protocol: fn(port: *mut c_void),
    /// `destroy_http_request(conn)`, of a live connection.
    pub destroy_http_request: fn(conn: *mut ConnectionRecord),
    /// Releases the port's context and its TLS state, as `ListenPort`'s destructor.
    pub tls_free: fn(&mut PortConfig),
    /// `create_dtls_session(sock)` on a DTLS port; null if it fails to initialize.
    pub dtls_create: fn(port: *mut c_void, sock: *mut SocketRecord) -> *mut DtlsSession,
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
/// which is all the server passes them.
pub unsafe fn set_methods(methods: PortMethods) {
    METHODS.get_or_init(|| methods);
}

/// The registered methods. A port exists only after `swoole_init`, which
/// registers them, so their absence aborts.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn methods() -> &'static PortMethods {
    METHODS
        .get()
        .unwrap_or_else(|| swoole_rs_corebase::fatal("port methods are not registered"))
}
#[repr(C)]
pub struct PortConfig {
    pub object_id: u16,
    pub backlog: Setting<u16>,
    pub listening: bool,
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
    pub ssl: u8,
    pub port: c_int,
    pub socket: *mut c_void,
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
    pub on_read: *mut c_void,
    /// `ListenPort::ssl_context`: the port's `SSLContext` (one reference to its
    /// Rust owner), or none: a nullable `SSLContext *` to native code.
    pub ssl_context: Option<TlsContext>,
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
        self.ssl_fields()
            .is_some_and(|fields| fields.protocols & SslVersion::SW_SSL_DTLS as u32 != 0)
    }

    /// `verify_peer` with no client certificate file, cafile or capath.
    #[must_use]
    pub fn ssl_verify_without_ca(&self) -> bool {
        self.ssl_fields()
            .is_some_and(|fields| fields.verify_peer != 0)
            && self
                .ssl_context
                .as_deref()
                .is_some_and(swoole_rs_tls::OwnedTlsContext::names_no_ca)
    }

    /// The fields of the port's `SSLContext`, if it has one.
    pub(crate) fn ssl_fields(&self) -> Option<&swoole_rs_tls::OwnedTlsFields> {
        let fields = swr_v1_tls_owner_view(self.ssl_context.as_ref()?).fields;
        // SAFETY: the owner's fields live as long as the port's reference.
        unsafe { fields.as_ref() }
    }
}

/// The settings of a `ListenPort`, which is their box (`server_object::port_new`).
///
/// # Safety
/// `port` is a live `ListenPort`, and nothing else accesses its settings while
/// the borrow lasts.
pub unsafe fn of<'a>(port: *mut c_void) -> &'a mut PortConfig {
    // SAFETY: per the caller; the port is its settings.
    unsafe { &mut *port.cast::<PortConfig>() }
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
    /// The port's listening socket, or `None` before it has one or after it closes
    /// (`OP_PORT_CLOSE_SOCKET`), which ends every handle to it: one is used within the
    /// op that took it.
    pub(crate) fn listening_socket(&self) -> Option<swoole_rs_reactor::Socket<'_>> {
        // SAFETY: null, or the live listening socket the port owns: closing it frees it
        // and nulls the field in one step.
        unsafe { swoole_rs_reactor::Socket::from_ptr(self.socket.cast()) }
    }
    /// A new port's settings.
    pub(crate) fn boxed(buffer_size: c_int) -> Box<Self> {
        Box::new(Self {
            object_id: 0,
            backlog: Setting::new(512),
            listening: false,
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
            ssl: 0,
            port: 0,
            socket: core::ptr::null_mut(),
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
            on_read: core::ptr::null_mut(),
            ssl_context: None,
            tls: Box::default(),
            length_owner: Shared::new(None),
        })
    }
}

/// # Safety
/// `config` is a unique live allocation from `config_new`; no views remain.
pub unsafe fn swr_v1_port_config_free(config: *mut PortConfig) {
    if !config.is_null() {
        // SAFETY: the native base releases its unique allocation after all derived members.
        drop(unsafe { Box::from_raw(config) });
    }
}
