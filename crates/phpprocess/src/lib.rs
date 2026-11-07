//! PHP `Swoole\Process`: class registration, constants, objects and every
//! method. C++ keeps the `Worker`, pipe and message-queue objects, which Server
//! and Process\Pool share; the methods reach them through the [`Host`] table C++
//! passes at MINIT. `Swoole\Process\Pool` and its event callbacks run over
//! crates/process's `ProcessPool` directly.
//! See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(any(target_os = "linux", target_os = "android"))]
mod affinity;
mod host;
mod pool;
mod process;
mod report;
mod signal;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use affinity::*;
pub use host::{Host, WorkerView};
pub use pool::*;
pub use process::*;
pub use signal::{swr_v1_phpprocess_clean, swr_v1_phpprocess_signal_isset};
