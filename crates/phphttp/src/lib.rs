//! PHP `Swoole\Http` classes over the zend crates and the httpbindings policy:
//! `Cookie`, `Request` and `Response`. C++ keeps the method tables built from
//! the stubs' arginfo and, until it moves to Rust, the `swoole::http::Context`
//! Request and Response point at; `host` declares what their methods ask of
//! it. A crate of its own so the native gates that link `libswoole_rs_ffi.a`
//! into executables never pull in objects with Zend imports (docs/zend.md).
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

mod context;
mod cookie;
mod header;
mod host;
mod parse;
mod request;
mod response;
mod state;

pub use context::{swr_v1_phphttp_context, swr_v1_phphttp_set_context};
pub use cookie::*;
pub use header::{
    swr_v1_phphttp_build_header, swr_v1_phphttp_build_trailer, swr_v1_phphttp_compress,
};
pub use parse::{
    swr_v1_phphttp_compression_method, swr_v1_phphttp_multipart_boundary,
    swr_v1_phphttp_multipart_init, swr_v1_phphttp_multipart_parse, swr_v1_phphttp_parse,
    swr_v1_phphttp_parse_free, swr_v1_phphttp_parse_minit,
};
pub use request::*;
pub use response::*;
