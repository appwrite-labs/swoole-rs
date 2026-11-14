//! PHP `Swoole\Process`: class registration, constants, objects and every
//! method, over crates/process's `Worker` record and crates/tls's `UnixSocket`
//! pipe. What is still C++ (the server, callables, the reactor and signals) the
//! methods reach through the [`Host`] table C++ passes at MINIT.
//! `Swoole\Process\Pool` and its event callbacks run over crates/process's
//! `ProcessPool` directly.
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
