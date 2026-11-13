//! `src/protocol/websocket.cc`'s entry points ([`frame_api`]) over crates/websocket's
//! safe codec, which forbids `unsafe`.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod frame_api;
