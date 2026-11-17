//! Operations supplied by crates above the reactor, registered before core startup.
use std::sync::OnceLock;

pub struct Host {
    pub sleep: fn(f64) -> bool,
    pub socket_free: fn(crate::reactor::OwnedSocket),
}

static HOST: OnceLock<Host> = OnceLock::new();

/// Registers the host once; subsequent initialization keeps the same operations.
pub fn set_host(host: Host) {
    HOST.get_or_init(|| host);
}

#[cfg(any(feature = "upstream-symbols", target_os = "linux"))]
pub(crate) fn get() -> &'static Host {
    HOST.get()
        .unwrap_or_else(|| crate::fatal("reactor host is not registered"))
}
