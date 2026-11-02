//! PHP client classes in Rust over the zend crate: `Swoole\Coroutine\Socket`
//! and `Swoole\Coroutine\Client` (registration, constants, objects and methods). C++ keeps the method table
//! built from the stub's arginfo and the coroutine socket itself, which the
//! methods reach through `native`. See docs/zend.md.
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

mod client;
mod constants;
mod http_client;
mod http_native;
mod http_request;
mod llhttp;
mod native;
mod socket;

pub use client::*;
pub use http_client::*;
pub use socket::*;
