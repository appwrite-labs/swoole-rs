//! Swoole's memory pools and collections: fixed, global and ring pools, the heap,
//! channel, shared mappings and the shared-memory table.
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
pub mod channel;
pub mod fixed;
pub mod global;
pub mod heap;
pub mod mapping;
mod pool;
pub mod ring;

#[cfg(test)]
#[path = "../../../tests/modules/memory/allocation.rs"]
mod allocation_tests;

pub use channel::{
    swr_v1_memory_channel_full, swr_v1_memory_channel_in, swr_v1_memory_channel_lifecycle,
    swr_v1_memory_channel_out, swr_v1_memory_channel_storage_in,
    swr_v1_memory_channel_storage_init, swr_v1_memory_channel_storage_out,
    swr_v1_memory_channel_storage_plan,
};
pub use fixed::{
    swr_v1_memory_arena_lifecycle, swr_v1_memory_fixed_alloc, swr_v1_memory_fixed_construct,
    swr_v1_memory_fixed_debug_step, swr_v1_memory_fixed_free, swr_v1_memory_fixed_init,
    swr_v1_memory_fixed_layout, swr_v1_memory_fixed_usage,
};
pub use global::{
    swr_v1_memory_global_add_page, swr_v1_memory_global_alloc, swr_v1_memory_global_capacity,
    swr_v1_memory_global_create, swr_v1_memory_global_memory_size, swr_v1_memory_global_open,
    swr_v1_memory_global_page_size, swr_v1_memory_global_pop_page, swr_v1_memory_global_release,
    swr_v1_memory_global_request_valid, swr_v1_memory_global_step,
};
pub use heap::{
    swr_v1_memory_heap_at, swr_v1_memory_heap_compare, swr_v1_memory_heap_count,
    swr_v1_memory_heap_create, swr_v1_memory_heap_debug_step, swr_v1_memory_heap_pop,
    swr_v1_memory_heap_push, swr_v1_memory_heap_release, swr_v1_memory_heap_remove,
    swr_v1_memory_heap_update,
};

pub use ring::{
    swr_v1_memory_ring_alloc, swr_v1_memory_ring_construct, swr_v1_memory_ring_free,
    swr_v1_memory_ring_layout, swr_v1_memory_ring_storage_alloc,
};

pub use mapping::{
    MappingEffect, MappingOperation, swr_v1_memory_mapping_copy, swr_v1_memory_mapping_extent,
    swr_v1_memory_mapping_initialize, swr_v1_memory_mapping_product,
    swr_v1_memory_mapping_relocate, swr_v1_memory_mapping_size, swr_v2_memory_mapping_step,
};

pub mod table;
