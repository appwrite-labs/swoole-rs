//! PHP `Swoole\Http` classes over the zend crates and the httpbindings policy:
//! `Cookie`, `Request` and `Response`, the HTTP and WebSocket servers' request
//! path and `Coroutine\Http\Server`, with method tables built from the stubs'
//! arginfo. The `swoole::http::Context` Request and Response point at is
//! `state::State` (`context::alloc`); `host` holds what their methods share.
//! A crate of its own so the native gates that link `libswoole_rs_ffi.a`
//! into executables never pull in objects with Zend imports (docs/zend.md).
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

#[macro_use]
mod local;
mod context;
mod cookie;
mod header;
mod host;
pub mod http2;
mod parse;
mod request;
mod response;
mod send;
mod server;
mod server_coro;
mod state;
mod websocket;

pub use cookie::*;
pub use header::response_rshutdown;
pub use request::*;
pub use response::*;
pub use server::*;
pub use server_coro::*;
pub use websocket::*;
