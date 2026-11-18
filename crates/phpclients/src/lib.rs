//! PHP registration and method handlers for `Swoole\Client`, `Swoole\Async\Client`,
//! `Swoole\Coroutine\Client`, `Swoole\Coroutine\Socket` and `Swoole\Coroutine\Http\Client`.
//! The method tables use arginfo from the PHP stubs. See docs/zend.md.
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
pub use http_native::{PushError, WebSocket, register_websocket, websocket_protocol};
pub use native::{Co, NewSocket, make as new_socket, pop_string};
pub use net::*;
pub use resolver::*;
pub use select::*;
pub use socket::*;
pub use socket_options::*;
