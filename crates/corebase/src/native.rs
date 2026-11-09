// SPDX-License-Identifier: Apache-2.0
//! The C++ effects Rust sequences (`lifecycle.rs`): native/corebase/globals.cc
//! registers them as it loads (`swr_v1_core_set_native_host`), so the archive
//! keeps no link-time reference to C++ and the native gates link it without them.
//! They come as one function, not a table: a sanitizer keeps every global alive,
//! and a table would keep the timer, event and signal code with it.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

/// `swr_v1_core_native_host` (`native/include/swoole_rs_corebase_state.h`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeHost {
    /// `swoole_get_env`; throws, as `std::stoi` does, on a malformed value.
    pub get_env: unsafe extern "C-unwind" fn(name: *const c_char, value: *mut c_int) -> bool,
    /// Runs one numbered effect; native code promises each is safe to run.
    effect: extern "C-unwind" fn(effect: c_int, argument: usize) -> usize,
    /// One `LOGGER_*` operation on the process logger (crates/process).
    logger: extern "C" fn(op: c_int),
}

/// `new Logger()` on `SwooleG.stdout`, into `SwooleG.logger`.
pub const LOGGER_NEW: c_int = 0;
/// `SwooleG.logger->reopen()`, `->close()` and `delete SwooleG.logger`.
pub const LOGGER_REOPEN: c_int = 1;
pub const LOGGER_CLOSE: c_int = 2;
pub const LOGGER_DELETE: c_int = 3;

impl NativeHost {
    fn run(&self, effect: c_int, argument: usize) -> usize {
        (self.effect)(effect, argument)
    }
    /// `swoole_init`'s native leaves, in upstream's order: `sw_error`, the stdout
    /// stream and the fatal-error hook, then the logger, then the debug log level,
    /// the memory pool, the futex-less wait registry, signalfd and the bug report.
    pub(crate) fn init(&self) {
        self.run(15, 0);
        self.logger(LOGGER_NEW);
        self.run(16, 0);
    }
    /// Allocate (`new String(SW_STACK_BUFFER_SIZE)`) or delete `SwooleTG.buffer_stack`.
    pub(crate) fn buffer(&self, allocate: bool) {
        self.run(2, usize::from(allocate));
    }
    /// `delete pool`: the pool `swoole_init` or the last fork made.
    pub(crate) fn delete_pool(&self, pool: *mut c_void) {
        self.run(4, pool.expose_provenance());
    }
    pub(crate) fn logger(&self, op: c_int) {
        (self.logger)(op);
    }
    pub(crate) fn signal_clear(&self) {
        self.run(8, 0);
    }
    pub(crate) fn timer_available(&self) -> bool {
        self.run(10, 0) != 0
    }
    pub(crate) fn timer_free(&self) {
        self.run(11, 0);
    }
    pub(crate) fn event_available(&self) -> bool {
        self.run(12, 0) != 0
    }
    pub(crate) fn event_free(&self) {
        self.run(13, 0);
    }
    pub(crate) fn coroutine_is_in(&self) -> bool {
        self.run(14, 0) != 0
    }
}

static HOST: OnceLock<NativeHost> = OnceLock::new();

/// Registers the host; later registrations are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_native_host(host: NativeHost) {
    HOST.get_or_init(|| host);
}

/// The registered host, once `swoole_init` has run.
#[must_use]
pub fn host() -> Option<&'static NativeHost> {
    HOST.get()
}
