//! Callback-free memory/collection policies. Native facades own locks and syscalls.
#![allow(clippy::missing_safety_doc)] // The module-local C contract covers all entry points.
pub mod channel;
pub mod fixed;
pub mod global;
pub mod heap;
pub mod mapping;
pub mod ring;
#[path = "../../storage.rs"]
mod storage;

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
pub use table::{
    swr_v1_memory_table_arena, swr_v1_memory_table_clear, swr_v1_memory_table_clear_columns,
    swr_v1_memory_table_clear_step, swr_v1_memory_table_clear_value,
    swr_v1_memory_table_column_size, swr_v1_memory_table_config,
    swr_v1_memory_table_construction_step, swr_v1_memory_table_core_teardown_step,
    swr_v1_memory_table_creation_step, swr_v1_memory_table_declaration_step,
    swr_v1_memory_table_delete, swr_v1_memory_table_factory_step, swr_v1_memory_table_get,
    swr_v1_memory_table_get_number, swr_v1_memory_table_get_string, swr_v1_memory_table_init,
    swr_v1_memory_table_iterator_release_step, swr_v1_memory_table_iterator_reset,
    swr_v1_memory_table_iterator_step, swr_v1_memory_table_layout, swr_v1_memory_table_lookup_step,
    swr_v1_memory_table_metadata_initialize, swr_v1_memory_table_mutation_step,
    swr_v1_memory_table_next, swr_v1_memory_table_set_plan, swr_v1_memory_table_set_value,
    swr_v1_memory_table_storage_layout, swr_v1_memory_table_teardown_step,
    swr_v1_memory_table_value_step,
};

pub mod table_columns;
pub use table_columns::{
    swr_v1_memory_table_columns_count, swr_v1_memory_table_columns_create,
    swr_v1_memory_table_columns_discard, swr_v1_memory_table_columns_get,
    swr_v1_memory_table_columns_insert, swr_v1_memory_table_columns_pop,
    swr_v1_memory_table_columns_release,
};

pub mod table_columns_v2;
pub use table_columns_v2::{
    swr_v1_memory_columns_v2_clear_step, swr_v1_memory_columns_v2_close_step,
    swr_v1_memory_columns_v2_cursor_next, swr_v1_memory_columns_v2_cursor_open,
    swr_v1_memory_columns_v2_declare_step, swr_v1_memory_columns_v2_get,
    swr_v1_memory_columns_v2_open,
};

pub mod table_records;
pub use table_records::{
    swr_v1_memory_records_clear, swr_v1_memory_records_close, swr_v1_memory_records_copy_name,
    swr_v1_memory_records_cursor_next, swr_v1_memory_records_cursor_open,
    swr_v1_memory_records_declare, swr_v1_memory_records_get, swr_v1_memory_records_open,
};
