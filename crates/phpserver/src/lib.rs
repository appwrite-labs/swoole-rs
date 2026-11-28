//! PHP `Swoole\Server` family classes in Rust: registration, objects and
//! method bodies over crates/server's Server and `ListenPort`. See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
#![cfg_attr(
    feature = "php",
    expect(
        clippy::disallowed_types,
        reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
    )
)]
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "server")]
mod port;

#[cfg(feature = "server")]
mod port_settings;

#[cfg(feature = "server")]
pub use port::*;

#[cfg(feature = "server")]
mod callable;

#[cfg(feature = "server")]
mod dispatch;

#[cfg(feature = "server")]
mod instance;

#[cfg(feature = "server")]
mod io;

#[cfg(feature = "server")]
mod lifecycle;

#[cfg(feature = "server")]
mod server;

#[cfg(feature = "server")]
mod settings;

#[cfg(feature = "server")]
mod task;

#[cfg(feature = "server")]
mod thread;

#[cfg(feature = "server")]
pub use dispatch::*;

#[cfg(feature = "server")]
pub use instance::*;

#[cfg(feature = "server")]
pub use io::*;

#[cfg(feature = "server")]
pub use lifecycle::*;

#[cfg(feature = "server")]
pub use server::*;

#[cfg(feature = "server")]
pub use settings::*;

#[cfg(feature = "server")]
pub use task::*;
