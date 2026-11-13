//! `src/protocol/http.cc`'s helpers under their C++ symbols ([`http_server`]), the
//! request reader's state ([`HttpRequest`]) and the HTTP proxy records, over crates/http's
//! safe API.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use swoole_rs_http::STATUS_CODES;

pub mod http_server;
mod owned_proxy;
pub use owned_proxy::*;
mod request;
pub use request::{
    HttpForm, HttpNative, HttpRequest, HttpString, NativeOp, request_free, request_new,
};
#[cfg(test)]
mod pin_helpers;

/// Upstream's `list_of_status_code`, which native code may write.
#[used]
#[unsafe(export_name = "_ZN6swoole11http_server19list_of_status_codeE")]
pub static mut NATIVE_STATUS_CODES: [i32; 128] = STATUS_CODES;

#[cfg(test)]
mod tests;
