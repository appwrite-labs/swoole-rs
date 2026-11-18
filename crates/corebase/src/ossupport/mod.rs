//! OS support: the async thread pool, file IO, sendfile, entropy, process and
//! thread identity, and the `SysV` message queue.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod cpu_set;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub use cpu_set::CpuSet;
mod file;
mod file_api;
mod file_record;
mod temp_file;
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "freebsd"
))]
pub use file::swr_v1_os_sendfile;
pub use file::{FileIo, Sendfile, file_close, file_open, file_size, read_all, write_all};
pub use file_api::*;
pub use file_record::*;
pub use temp_file::TempFile;
mod system;
pub use system::swoole_daemon;
pub use system::{
    CurrentThread, pthread_self, set_isolation, swr_v1_os_is_root_user,
    swr_v1_os_set_process_death_signal, swr_v1_os_thread_native_id, thread_set_name,
};
mod msg_queue;
pub use msg_queue::QueueRecord;
mod pool;
pub use pool::{Pool, PoolHooks, complete as pool_complete};

#[cfg(test)]
mod isolated_process {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/isolated_process.rs"
    ));
}
