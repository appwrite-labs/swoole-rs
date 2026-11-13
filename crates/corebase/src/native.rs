// SPDX-License-Identifier: Apache-2.0
//! What corebase's lifecycles (`lifecycle.rs`) reach in the crates above it:
//! crates/ffi's `swoole_init` registers each before it runs the body, so corebase
//! and its tests link without them.

use std::ffi::c_void;
use std::sync::OnceLock;

/// One function per leaf, in the crate that owns it.
#[derive(Clone, Copy)]
pub struct NativeHost {
    /// `swoole_init`'s native leaves, in upstream's order: `sw_error`, the stdout
    /// stream and the fatal-error hook, the logger, the debug log level, the memory
    /// pool, the futex-less wait registry, signalfd and the bug report.
    pub init: fn(),
    /// `new String(SW_STACK_BUFFER_SIZE)` for this thread's `SwooleTG.buffer_stack`,
    /// and its `delete`.
    pub new_buffer: fn() -> *mut c_void,
    pub delete_buffer: fn(),
    /// `delete SwooleG.memory_pool`.
    pub delete_pool: fn(),
    /// One operation on the process logger (crates/process).
    pub logger: fn(op: LoggerOp),
    /// `swoole_signal_clear()`, which C++ calls too.
    pub signal_clear: extern "C" fn(),
    pub timer_free: fn(),
    pub event_available: fn() -> bool,
    pub event_free: fn(),
    pub coroutine_is_in: fn() -> bool,
}

/// An operation on the process logger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoggerOp {
    /// `new Logger()` on `SwooleG.stdout`, into `SwooleG.logger`.
    New,
    /// `SwooleG.logger->reopen()`.
    Reopen,
    /// `SwooleG.logger->close()`.
    Close,
    /// `delete SwooleG.logger`.
    Delete,
}

static HOST: OnceLock<NativeHost> = OnceLock::new();

/// Registers the host; later registrations are ignored.
pub fn set_native_host(host: NativeHost) {
    HOST.get_or_init(|| host);
}

/// The registered host, once `swoole_init` has run.
#[must_use]
pub fn host() -> Option<&'static NativeHost> {
    HOST.get()
}
