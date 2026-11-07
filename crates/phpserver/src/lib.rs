//! PHP `Swoole\Server` family classes in Rust: registration, objects and
//! method bodies. C++ keeps upstream's method tables built from the stubs'
//! arginfo and the Server and `ListenPort` objects these configure, reached
//! through host tables (`native/ports/swoole_rs_phpserver.h`,
//! `native/process/swoole_rs_phpserver_server.h`). See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
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
mod dispatch;

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
pub use dispatch::*;

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
