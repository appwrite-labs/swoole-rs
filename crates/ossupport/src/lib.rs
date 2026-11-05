//! OS support: the async thread pool, file IO, sendfile, entropy, process and
//! thread identity, and the `SysV` message queue.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod async_file;
pub use async_file::*;
mod file;
mod file_record;
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd"
))]
pub use file::swr_v1_os_sendfile;
pub use file::{
    FileContents, FileIo, FileSize, Sendfile, swr_v1_os_file_close, swr_v1_os_file_contents_open,
    swr_v1_os_file_open, swr_v1_os_file_put_open, swr_v1_os_file_read_all,
    swr_v1_os_file_read_line, swr_v1_os_file_size, swr_v1_os_file_size_path,
    swr_v1_os_file_write_all, swr_v1_os_random_bytes, swr_v1_os_sendfile_copy, swr_v1_os_tmpfile,
};
pub use file_record::*;
mod system;
pub use system::{
    IsolationWarn, swr_v1_os_daemon_detach, swr_v1_os_is_root_user, swr_v1_os_set_isolation,
    swr_v1_os_set_process_death_signal, swr_v1_os_thread_get_name, swr_v1_os_thread_native_id,
    swr_v1_os_thread_set_name,
};
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
pub use system::{swr_v1_os_get_cpu_affinity, swr_v1_os_set_cpu_affinity};
mod msg_queue;
pub use msg_queue::{
    MsgOpen, MsgPop, QueueRecord, swr_v1_os_msg_flags, swr_v1_os_msg_open, swr_v1_os_msg_pop,
    swr_v1_os_msg_push, swr_v1_os_msg_queue_blocking, swr_v1_os_msg_queue_init,
    swr_v1_os_msg_queue_remove, swr_v1_os_msg_queue_set_capacity, swr_v1_os_msg_queue_stat,
    swr_v1_os_msg_remove,
};
mod pool;
pub use pool::{
    Pool, PoolHooks, swr_v1_os_pool_acquire, swr_v1_os_pool_complete, swr_v1_os_pool_dispatch,
    swr_v1_os_pool_join, swr_v1_os_pool_notify_one, swr_v1_os_pool_queue_size,
    swr_v1_os_pool_release, swr_v1_os_pool_task_id, swr_v1_os_pool_worker_num,
};

#[cfg(test)]
mod isolated_process {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/isolated_process.rs"
    ));
}
