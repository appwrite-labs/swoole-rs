//! PHP registration and handlers for `Swoole\Coroutine\Http2\Client`,
//! `Swoole\Http2\Request` and `Swoole\Http2\Response`. The client uses the
//! hooks crate's coroutine socket and the http2 crate's HPACK codecs.
//! The server handles frames and responses using per-connection sessions and streams.
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
#![forbid(unsafe_code)]

mod client;
mod files;
mod server;
mod session;

pub use client::*;
pub use server::*;
