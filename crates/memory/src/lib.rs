//! Swoole's memory pools and collections: fixed and global pools, the channel,
//! shared mappings and the shared-memory table.
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_types,
        reason = "the tests check against std model collections"
    )
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
/// Defines each listed function under its upstream C++ symbol with
/// `upstream-symbols`. `uint64_t` is `unsigned long long` on macOS and `unsigned long`
/// on Linux, which the symbols spell.
macro_rules! upstream {
    ($($linux:literal, $macos:literal => $item:item)*) => {$(
        #[cfg_attr(all(feature = "upstream-symbols", not(target_os = "macos")), unsafe(export_name = $linux))]
        #[cfg_attr(all(feature = "upstream-symbols", target_os = "macos"), unsafe(export_name = $macos))]
        $item
    )*};
}

pub mod channel;
pub mod fixed;
pub mod flat;
mod global;

#[cfg(test)]
#[path = "../../../tests/modules/memory/allocation.rs"]
mod allocation_tests;

pub use channel::{
    swr_v1_memory_channel_bytes, swr_v1_memory_channel_close, swr_v1_memory_channel_full,
    swr_v1_memory_channel_in, swr_v1_memory_channel_init, swr_v1_memory_channel_out,
};

pub mod members;
pub mod table;
