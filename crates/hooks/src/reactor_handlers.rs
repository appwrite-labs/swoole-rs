//! Readiness callbacks supplied by the hooks crate.
use crate::socket_members;
use swoole_rs_corebase::reactor::{Handler, UringOps};

/// Coroutine socket callbacks in read, write and error order.
pub const SOCKET: [Handler; 3] = [
    socket_members::readable_event_callback,
    socket_members::writable_event_callback,
    socket_members::error_event_callback,
];

#[cfg(feature = "iouring")]
pub const IOURING: Option<&UringOps> = Some(&crate::iouring::REACTOR);
#[cfg(not(feature = "iouring"))]
pub const IOURING: Option<&UringOps> = None;
