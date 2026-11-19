//! Swoole's memory pools and collections: fixed and global pools, the channel,
//! shared mappings and the shared-memory table.
pub mod apart;
pub mod channel;
pub mod fixed;
pub mod flat;
mod global;

#[cfg(test)]
#[path = "../../../../tests/modules/memory/allocation.rs"]
mod allocation_tests;

pub use channel::{
    ChannelInitError, SharedChannel, swr_v1_memory_channel_bytes, swr_v1_memory_channel_full,
    swr_v1_memory_channel_in, swr_v1_memory_channel_out,
};

pub mod members;
pub mod table;
