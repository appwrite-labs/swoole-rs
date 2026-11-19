//! PHP registration and handlers for `Swoole\Coroutine\Http2\Client`,
//! `Swoole\Http2\Request` and `Swoole\Http2\Response`. The client uses the
//! hooks crate's coroutine socket and the http2 crate's HPACK codecs.
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

pub use client::*;
pub use swoole_rs_phphttp::http2::server_on_close;
