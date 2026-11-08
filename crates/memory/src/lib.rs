//! Swoole's memory pools and collections: fixed, global and ring pools, the heap,
//! channel, shared mappings and the shared-memory table.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![expect(
    clippy::missing_safety_doc,
    reason = "every export is covered by the module-local C contract"
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
pub mod heap;
mod pool;
pub mod ring;

#[cfg(test)]
#[path = "../../../tests/modules/memory/allocation.rs"]
mod allocation_tests;

pub use channel::{
    swr_v1_memory_channel_bytes, swr_v1_memory_channel_close, swr_v1_memory_channel_full,
    swr_v1_memory_channel_in, swr_v1_memory_channel_init, swr_v1_memory_channel_lock,
    swr_v1_memory_channel_out, swr_v1_memory_channel_print, swr_v1_memory_channel_signal,
};
pub use heap::{
    swr_v1_memory_heap_at, swr_v1_memory_heap_count, swr_v1_memory_heap_create,
    swr_v1_memory_heap_pop, swr_v1_memory_heap_push, swr_v1_memory_heap_release,
    swr_v1_memory_heap_remove, swr_v1_memory_heap_update,
};

pub mod members;
pub mod table;
