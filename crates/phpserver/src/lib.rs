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

#[cfg(feature = "port")]
mod port;

#[cfg(feature = "port")]
pub use port::*;

#[cfg(feature = "server")]
mod server;

#[cfg(feature = "server")]
pub use server::*;
