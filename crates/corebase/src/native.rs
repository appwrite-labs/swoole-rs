// SPDX-License-Identifier: Apache-2.0
//! What corebase's lifecycles (`lifecycle.rs`) reach in the crates above it:
//! crates/ffi's `swoole_init` registers each before it runs the body, so corebase
//! and its tests link without them.

use std::sync::OnceLock;

/// One function per leaf, in the crate that owns it.
#[derive(Clone, Copy)]
pub struct NativeHost {
    /// `swoole_init`'s native leaves, in upstream's order: `sw_error`, the stdout
    /// stream and the fatal-error hook, the logger, the debug log level, the memory
    /// pool, the futex-less wait registry, signalfd and the bug report.
    pub init: fn(),
    pub timer_free: fn(),
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
