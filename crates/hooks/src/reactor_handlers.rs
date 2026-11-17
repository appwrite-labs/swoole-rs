//! Readiness callbacks supplied by the hooks crate.
use crate::socket_members;
use swoole_rs_corebase::reactor::Handler;

/// Coroutine socket callbacks in read, write and error order.
pub const SOCKET: [Handler; 3] = [
    socket_members::readable_event_callback,
    socket_members::writable_event_callback,
    socket_members::error_event_callback,
];

#[cfg(all(feature = "upstream-symbols", feature = "iouring"))]
pub const IOURING: Option<Handler> = Some(|_, _| if crate::iouring::ready() { 0 } else { -1 });
#[cfg(not(all(feature = "upstream-symbols", feature = "iouring")))]
pub const IOURING: Option<Handler> = None;
