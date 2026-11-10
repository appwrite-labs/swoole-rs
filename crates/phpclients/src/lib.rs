//! PHP client classes in Rust over the zend crate: `Swoole\Client`,
//! `Swoole\Async\Client`, `Swoole\Coroutine\Socket` and `Swoole\Coroutine\Client`
//! (registration, constants, objects and methods). C++ keeps the method tables
//! built from the stubs' arginfo, `network::Client` and the coroutine socket,
//! which the methods reach through `native`. See docs/zend.md.
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

mod async_net;
mod client;
mod constants;
mod http_client;
mod http_native;
mod http_request;
mod multicast;
mod native;
mod net;
mod resolver;
mod select;
mod settings;
mod socket;
mod socket_options;

pub use async_net::*;
pub use client::*;
pub use http_client::*;
pub use http_native::{WebSocket, register_websocket, websocket_protocol};
pub use native::Co;
pub use net::*;
pub use resolver::*;
pub use select::*;
pub use socket::*;
pub use socket_options::*;
