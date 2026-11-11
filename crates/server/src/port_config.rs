//! Rust-owned `ListenPort` settings and native socket/statistic loans.
use core::ffi::{CStr, c_int, c_void};
use std::sync::OnceLock;

use swoole_rs_corebase::constants::{SW_DATA_EOF, SW_INPUT_BUFFER_SIZE};
use swoole_rs_corebase::net_types::Protocol;
use swoole_rs_corebase::{CoreStringView, StringSlot, swr_v1_core_string_view};
use swoole_rs_reactor::SocketRecord;
use swoole_rs_tls::{DtlsSession, OwnedTlsContext, SslVersion, swr_v1_tls_owner_view};

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
    pub dtls_session: fn(&mut PortConfig, c_int) -> *mut DtlsSession,
    /// Frees the DTLS session on an fd, if any.
    pub dtls_free: fn(&mut PortConfig, c_int),
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
    METHODS.get().unwrap_or_else(|| std::process::abort())
}
#[repr(C)]
pub struct PortConfig {
    pub object_id: u16,
    pub backlog: u16,
    pub listening: bool,
    pub tcp_defer_accept: c_int,
    pub tcp_fastopen: c_int,
    pub tcp_keepidle: c_int,
    pub tcp_keepinterval: c_int,
    pub tcp_keepcount: c_int,
    pub tcp_user_timeout: c_int,
    pub max_idle_time: f64,
    pub socket_buffer_size: c_int,
    pub buffer_high_watermark: u32,
    pub buffer_low_watermark: u32,
    pub r#type: c_int,
    pub ssl: u8,
    pub port: c_int,
    pub socket: *mut c_void,
    pub thread_id: libc::pthread_t,
    pub heartbeat_idle_time: u16,
    pub open_eof_check: bool,
    pub open_http_protocol: bool,
    pub open_http2_protocol: bool,
    pub open_websocket_protocol: bool,
    pub open_length_check: bool,
    pub open_mqtt_protocol: bool,
    pub open_redis_protocol: bool,
    pub open_tcp_nodelay: bool,
    pub open_tcp_nopush: bool,
    pub open_tcp_keepalive: bool,
    pub kernel_socket_recv_buffer_size: c_int,
    pub kernel_socket_send_buffer_size: c_int,
    pub gs: *mut c_void,
    pub ptr: *mut c_void,
    /// `open_websocket_{close,ping,pong}_frame` as bits 0, 1 and 2.
    pub websocket_frames: u8,
    /// `websocket_subprotocol`, the handshake's `Sec-WebSocket-Protocol`.
    pub websocket_protocol: Option<Box<Vec<u8>>>,
    /// `ListenPort::host`, which native code reads and writes through a `RustStringView`.
    pub host: Box<StringSlot>,
    /// `ListenPort::protocol`, which native code reads and writes by reference.
    pub protocol: Protocol,
    /// `ListenPort::onRead`, the readable callback `init_protocol` picks.
    pub on_read: *mut c_void,
    /// `ListenPort::ssl_context`: the port's `SSLContext` (one reference to its
    /// Rust owner), or null.
    pub ssl_context: *const OwnedTlsContext,
    /// The TLS state crates/ports owns (the SNI contexts and the DTLS sessions);
    /// `ListenPort`'s destructor releases it with the context.
    pub tls: *mut c_void,
}

impl PortConfig {
    /// The host: its NUL-terminated bytes and their length.
    #[must_use]
    pub fn host(&self) -> CoreStringView {
        // SAFETY: the slot lives as long as `self`, and `&self` keeps writers out.
        unsafe { swr_v1_core_string_view(&raw const *self.host) }
    }

    /// The host's bytes, without the NUL.
    #[must_use]
    pub fn host_bytes(&self) -> &[u8] {
        let host = self.host();
        // SAFETY: the slot's view covers its length while `self` is borrowed.
        unsafe { core::slice::from_raw_parts(host.data.cast::<u8>(), host.length) }
    }

    /// `ListenPort::is_dtls()`: the port's context speaks DTLS.
    #[must_use]
    pub fn is_dtls(&self) -> bool {
        if self.ssl_context.is_null() {
            return false;
        }
        // SAFETY: the port's live owner.
        let fields = swr_v1_tls_owner_view(unsafe { &*self.ssl_context }).fields;
        // SAFETY: the owner's fields live as long as the port's reference.
        let protocols = unsafe { (*fields).protocols };
        protocols & SslVersion::SW_SSL_DTLS as u32 != 0
    }
}

/// The settings of a `ListenPort`, a box of the pointer to them (`server_object`).
///
/// # Safety
/// `port` is a live `ListenPort`, and nothing else accesses its settings while
/// the borrow lasts.
pub unsafe fn of<'a>(port: *mut c_void) -> &'a mut PortConfig {
    // SAFETY: per the caller; a `ListenPort` is crate::object's box of this pointer.
    let config = unsafe { *port.cast::<*mut PortConfig>() };
    // SAFETY: the port's settings live as long as it.
    unsafe { &mut *config }
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
        private_data_1: core::ptr::null_mut(),
        private_data_2: core::ptr::null_mut(),
        on_package: core::ptr::null_mut(),
        get_package_length: core::ptr::null_mut(),
        get_package_length_size: core::ptr::null_mut(),
    }
}

/// Allocates settings before the native port acquires any resources.
#[must_use]
pub fn swr_v1_port_config_new(buffer_size: c_int) -> *mut PortConfig {
    Box::into_raw(PortConfig::boxed(buffer_size))
}

impl PortConfig {
    /// The port's slot in `port_gs_list`, once the server has made it.
    #[expect(
        clippy::mut_from_ref,
        reason = "shared-memory counters, atomic but for `connection_nums`, set at create"
    )]
    pub(crate) fn counters(&self) -> Option<&mut crate::shared::PortCounters> {
        // SAFETY: null, or the port's slot in `port_gs_list`, mapped for the server's life.
        unsafe { self.gs.cast::<crate::shared::PortCounters>().as_mut() }
    }
    /// A new port's settings.
    pub(crate) fn boxed(buffer_size: c_int) -> Box<Self> {
        Box::new(Self {
            object_id: 0,
            backlog: 512,
            listening: false,
            tcp_defer_accept: 0,
            tcp_fastopen: 0,
            tcp_keepidle: 3600,
            tcp_keepinterval: 60,
            tcp_keepcount: 5,
            tcp_user_timeout: 0,
            max_idle_time: 0.0,
            socket_buffer_size: buffer_size,
            buffer_high_watermark: 0,
            buffer_low_watermark: 0,
            r#type: 1,
            ssl: 0,
            port: 0,
            socket: core::ptr::null_mut(),
            // SAFETY: pthread_t is an integer or pointer; zero is the original native empty sentinel.
            thread_id: unsafe { core::mem::zeroed() },
            heartbeat_idle_time: 0,
            open_eof_check: false,
            open_http_protocol: false,
            open_http2_protocol: false,
            open_websocket_protocol: false,
            open_length_check: false,
            open_mqtt_protocol: false,
            open_redis_protocol: false,
            open_tcp_nodelay: false,
            open_tcp_nopush: true,
            open_tcp_keepalive: false,
            kernel_socket_recv_buffer_size: 0,
            kernel_socket_send_buffer_size: 0,
            gs: core::ptr::null_mut(),
            ptr: core::ptr::null_mut(),
            websocket_frames: 0,
            websocket_protocol: None,
            host: Box::default(),
            protocol: default_protocol(),
            on_read: core::ptr::null_mut(),
            ssl_context: core::ptr::null(),
            tls: core::ptr::null_mut(),
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
