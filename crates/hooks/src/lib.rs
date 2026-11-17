//! Coroutine I/O policy over copied scalars, and whole hooked operations moving
//! into Rust (`sleep`). Policy functions make no native callbacks, syscalls or
//! allocations and retain no borrows.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod address;
pub mod buffered;
pub mod co_io;
pub mod co_socket;
pub mod connect;
pub mod dns_cache;
pub mod exec;
pub mod fd_control;
pub mod file_io;
pub mod flock;
pub mod hooked;
#[cfg(all(feature = "iouring", feature = "upstream-symbols"))]
pub mod iouring;
pub mod lifecycle;
pub mod packet;
pub use swoole_rs_corebase::reactor::socket_poll as poll;
pub mod reactor_handlers;
pub mod resolve;
pub mod sendfile;
pub mod setup;
pub mod shutdown;
pub mod sleep;
pub mod socket_extra;
pub mod socket_map;
pub mod socket_members;
pub mod socket_tls;
pub mod socket_wait;
#[cfg(feature = "ssh2")]
pub mod ssh2;
pub mod stdio;
pub mod transfer;
#[cfg(feature = "iouring")]
pub mod uring;
#[cfg(feature = "iouring")]
pub mod uring_socket;
#[cfg(feature = "iouring")]
pub mod uring_tls;
pub mod vector;
pub use swoole_rs_corebase::reactor::wait_event;
pub mod wait_signal;

/// Preserve the pinned two divisions rather than changing timeout rounding.
#[must_use]
pub fn timeval(seconds: f64, micros: f64) -> f64 {
    seconds + micros / 1000.0 / 1000.0
}

mod socket_scope;
pub use socket_scope::*;
