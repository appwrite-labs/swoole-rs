//! Callback-free `ListenPort` decisions. All native effects and object custody stay native.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use swoole_rs_server::port_config::PortMethods;

pub mod framing_recv;
pub mod http_routes;
pub mod lifecycle;
pub mod methods;
pub mod read;
#[cfg(not(test))]
mod read_native;
mod resp_recv;

const _: () = {
    assert!(
        std::mem::size_of::<std::sync::atomic::AtomicU32>() == std::mem::size_of::<u32>(),
        "AtomicU32 size must match u32"
    );
    assert!(
        std::mem::align_of::<std::sync::atomic::AtomicU32>() == std::mem::align_of::<u32>(),
        "AtomicU32 alignment must match u32"
    );
};

/// Registers the `ListenPort` methods the server calls; `swoole_init` does this.
pub fn register_methods() {
    // SAFETY: each method takes a live port, socket or connection of the server,
    // all the server passes; nothing outside it can reach them.
    unsafe { swoole_rs_server::port_config::set_methods(METHODS) };
}

const METHODS: PortMethods = PortMethods {
    ssl_context_init: lifecycle::ssl_context_init,
    ssl_create: lifecycle::ssl_create,
    import: lifecycle::import,
    close: lifecycle::close,
    listen: methods::listen,
    create_socket: methods::create_socket,
    close_socket: methods::close_socket,
    get_protocols: methods::get_protocols,
    init_protocol: methods::init_protocol,
    destroy_http_request: methods::destroy_http_request,
    tls_free: lifecycle::tls_free,
    dtls_create: lifecycle::dtls_create,
    dtls_session: lifecycle::dtls_session,
    dtls_free: lifecycle::dtls_free,
};

/// A port's callbacks (`php_swoole_server_port_callback_type`), in [`EVENTS`] order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum PortCallback {
    Connect,
    Receive,
    Close,
    Packet,
    Request,
    Handshake,
    BeforeHandshakeResponse,
    Open,
    Message,
    Disconnect,
    BufferFull,
    BufferEmpty,
}

impl PortCallback {
    /// Every callback, in slot order.
    pub const ALL: [Self; 12] = [
        Self::Connect,
        Self::Receive,
        Self::Close,
        Self::Packet,
        Self::Request,
        Self::Handshake,
        Self::BeforeHandshakeResponse,
        Self::Open,
        Self::Message,
        Self::Disconnect,
        Self::BufferFull,
        Self::BufferEmpty,
    ];

    /// The callback's slot.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

const EVENTS: [(&[u8], &[u8]); 12] = [
    (b"connect", b"onConnect"),
    (b"receive", b"onReceive"),
    (b"close", b"onClose"),
    (b"packet", b"onPacket"),
    (b"request", b"onRequest"),
    (b"handshake", b"onHandshake"),
    (b"beforehandshakeresponse", b"onBeforeHandshakeResponse"),
    (b"open", b"onOpen"),
    (b"message", b"onMessage"),
    (b"disconnect", b"onDisconnect"),
    (b"bufferfull", b"onBufferFull"),
    (b"bufferempty", b"onBufferEmpty"),
];
/// Complete immutable PHP Port declaration metadata. Names share the existing
/// event catalog; no Zend pointer or mutable class/property registry enters Rust.
pub struct PortsProperty {
    pub name: &'static [u8],
    pub value: PropertyDefault,
    pub private: bool,
}

#[derive(Clone, Copy)]
pub enum PropertyDefault {
    Null,
    Long(i64),
    Bool(bool),
}

pub fn class_properties() -> impl Iterator<Item = PortsProperty> {
    use PropertyDefault::{Bool, Long, Null};
    // Declaration order differs from callback enum order and is observable.
    const EVENT_ORDER: [usize; 12] = [0, 1, 2, 3, 10, 11, 4, 5, 7, 8, 9, 6];
    const PUBLIC: &[(&[u8], PropertyDefault)] = &[
        (b"host", Null),
        (b"port", Long(0)),
        (b"type", Long(0)),
        (b"sock", Long(-1)),
        (b"ssl", Bool(false)),
        (b"setting", Null),
        (b"connections", Null),
    ];
    EVENT_ORDER
        .into_iter()
        .filter_map(|event| EVENTS.get(event))
        .map(|&(_, name)| PortsProperty {
            name,
            value: Null,
            private: true,
        })
        .chain(PUBLIC.iter().map(|&(name, value)| PortsProperty {
            name,
            value,
            private: false,
        }))
}

/// The event `name` registers (`on()`): its callback and callback property.
/// `native_casefold` matches as libc's `strncasecmp` does, else exactly.
#[must_use]
pub fn event(name: &[u8], native_casefold: bool) -> Option<(PortCallback, &'static [u8])> {
    PortCallback::ALL
        .into_iter()
        .zip(EVENTS)
        .find_map(|(index, (key, property))| {
            let found = name.len() == key.len()
                && if native_casefold {
                    swoole_rs_locale::strncasecmp_equal(name, key, key.len())
                } else {
                    name == key
                };
            found.then_some((index, property))
        })
}

/// The callback property `getCallback(name)` reads, matched as Zend folds ASCII:
/// reusing `on()`'s locale-sensitive match would change Turkish-locale behaviour.
#[must_use]
pub fn callback_property(name: &[u8]) -> Option<&'static [u8]> {
    EVENTS
        .iter()
        .find(|(key, _)| name.eq_ignore_ascii_case(key))
        .map(|&(_, property)| property)
}
