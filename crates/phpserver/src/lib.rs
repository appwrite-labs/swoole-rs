//! PHP `Swoole\Server` family classes in Rust: registration, objects and
//! method bodies over crates/server's Server and `ListenPort`. See docs/zend.md.
//!
//! Registration requires the `server` feature, which turns `php` on.
#![cfg(feature = "server")]
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![forbid(unsafe_code)]

mod callable;
mod dispatch;
mod instance;
mod io;
mod lifecycle;
mod port;
mod port_settings;
mod server;
mod settings;
mod task;
mod thread;

pub use dispatch::*;
pub use instance::*;
pub use io::*;
pub use lifecycle::*;
pub use port::*;
pub use server::*;
pub use settings::*;
pub use task::*;
