//! The C ABI over crates/protocols' codecs: `swoole::mqtt` under its upstream symbols,
//! `swoole::Socks5Proxy`'s methods for Rust callers, and `redis::recv_packet`'s loop.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod mqtt;
mod owned_proxy;
mod resp_recv;
pub use mqtt::*;
pub use owned_proxy::*;
pub use resp_recv::*;
