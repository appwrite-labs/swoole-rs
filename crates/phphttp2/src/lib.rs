//! `Swoole\Coroutine\Http2\Client`, `Swoole\Http2\Request` and
//! `Swoole\Http2\Response` in Rust over the zend crate: registration, the
//! client's connection, settings, flow control, stream table and send queue,
//! frame parsing, request building and every method. The coroutine socket and
//! nghttp2's HPACK codec stay C++ (`native`). See docs/zend.md.
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
mod native;

pub use client::*;
