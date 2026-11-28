//! Copied worker/process transitions. Native alone executes callbacks and owns stores.
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
pub mod affinity;
pub mod armed_timer;
pub mod command;
pub mod methods;
pub mod os;
pub mod pool;
pub mod process_pool;
pub mod reaper;
pub mod reload_task;
pub mod server_workers;
pub mod signal;
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub mod signal_kqueue;
pub mod signal_loop;
#[cfg(target_os = "linux")]
pub mod signalfd;
pub mod stream;
pub mod task_file;
pub mod task_sync;
pub mod wait;
pub mod worker_globals;
pub mod worker_record;
pub use swoole_rs_corebase::worker_registry;
pub mod workers;
use std::ffi::c_int;
/// SIGRTMIN is a runtime value on Linux and undefined elsewhere (upstream guards
/// it with `#ifdef SIGRTMIN`).
#[cfg(target_os = "linux")]
fn is_sigrtmin(signo: c_int) -> bool {
    signo == libc::SIGRTMIN()
}
#[cfg(not(target_os = "linux"))]
fn is_sigrtmin(_signo: c_int) -> bool {
    false
}
/// Held by tests that install process-wide signal state (handlers, the signalfd
/// mask), which `cargo test`'s parallel threads would otherwise race on (#534).
#[cfg(test)]
pub(crate) fn signal_state_lock() -> std::sync::MutexGuard<'static, ()> {
    static STATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
