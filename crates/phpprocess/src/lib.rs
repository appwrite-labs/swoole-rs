//! PHP classes `Swoole\Process` and `Swoole\Process\Pool` wrap the corebase worker
//! and process pool. The host module connects their methods to the server,
//! Zend callables, the reactor and signal handlers.
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
mod worker;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use affinity::*;
pub use host::{enable_coroutine, sync_server, timer_uses_reactor};
pub use pool::*;
pub use process::*;
pub use signal::{
    php_swoole_process_rshutdown, php_swoole_process_signal_clean, php_swoole_signal_isset_handler,
};
