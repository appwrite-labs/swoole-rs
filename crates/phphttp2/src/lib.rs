//! `Swoole\Coroutine\Http2\Client`, `Swoole\Http2\Request` and
//! `Swoole\Http2\Response` in Rust over the zend crate: registration, the
//! client's connection, settings, flow control, stream table and send queue,
//! frame parsing, request building and every method, with crates/http2's HPACK.
//! The coroutine socket stays C++ (`native`). `server` is the HTTP/2
//! server's frames and responses over `session`'s sessions and streams. See
//! docs/zend.md.
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

mod client;
mod files;
mod server;
mod session;

pub use client::*;
pub use server::*;
