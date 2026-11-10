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
#[cfg(feature = "upstream-symbols")]
pub mod async_threads;
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
pub mod offload;
pub mod packet;
pub mod poll;
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
pub mod wait_event;
pub mod wait_signal;

#[derive(Debug, PartialEq, Eq)]
pub struct EventPlan {
    pub revents: u32,
    pub wake: bool,
}

/// Native `EventWaiter` queues at most one deferred wake before publishing bits.
#[must_use]
pub fn event_waiter(received: u32, event: u32) -> EventPlan {
    EventPlan {
        revents: received | event,
        wake: received == 0,
    }
}

/// `System::wait_event` strips error readiness and adds only requested IO bits.
#[must_use]
pub fn wait_revents(requested: u32, received: u32, read_write: u32, error: u32) -> u32 {
    if received & error != 0 {
        (received ^ error) | (requested & read_write)
    } else {
        received
    }
}

/// Preserve the pinned two divisions rather than changing timeout rounding.
#[must_use]
pub fn timeval(seconds: f64, micros: f64) -> f64 {
    seconds + micros / 1000.0 / 1000.0
}

#[must_use]
pub fn sleep_timeout(seconds: f64, minimum: f64) -> f64 {
    if seconds < minimum { minimum } else { seconds }
}

mod socket_scope;
pub use socket_scope::*;
