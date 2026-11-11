//! OS support: the async thread pool, file IO, sendfile, entropy, process and
//! thread identity, and the `SysV` message queue.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#[cfg(any(target_os = "linux", target_os = "android"))]
mod cpu_set;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub use cpu_set::CpuSet;
mod file;
mod file_api;
mod file_record;
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd"
))]
pub use file::swr_v1_os_sendfile;
pub use file::{FileIo, FileSize, Sendfile, file_close, file_open, file_size, read_all, write_all};
pub use file_api::*;
pub use file_record::*;
mod system;
#[cfg(feature = "upstream-symbols")]
pub use system::swoole_daemon;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
pub use system::swoole_get_cpu_affinity;
pub use system::{
    swoole_random_bytes, swr_v1_os_is_root_user, swr_v1_os_set_isolation,
    swr_v1_os_set_process_death_signal, swr_v1_os_thread_native_id, thread_get_name,
    thread_set_name,
};
mod msg_queue;
pub use msg_queue::{QueueRecord, swr_v1_os_msg_queue_pop, swr_v1_os_msg_queue_push};
mod pool;
pub use pool::{Pool, PoolHooks, complete as pool_complete};

#[cfg(test)]
mod isolated_process {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/isolated_process.rs"
    ));
}
