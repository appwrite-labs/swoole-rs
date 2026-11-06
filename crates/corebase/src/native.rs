// SPDX-License-Identifier: Apache-2.0
//! The C++ effects Rust sequences (`lifecycle.rs`): native/corebase/globals.cc's
//! `swoole_init` registers them (`swr_v1_core_set_native_host`), so the archive
//! keeps no link-time reference to C++ and the native gates link it without them.
//! They come as one function, not a table: a sanitizer keeps every global alive,
//! and a table would keep the timer, event and signal code with it.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

/// `swr_v1_core_native_host` (`native/include/swoole_rs_corebase_state.h`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeHost {
    /// `swoole_get_env`; throws, as `std::stoi` does, on a malformed value.
    pub get_env: unsafe extern "C-unwind" fn(name: *const c_char, value: *mut c_int) -> bool,
    /// Runs one numbered effect; native code promises each is safe to run.
    effect: extern "C-unwind" fn(effect: c_int, argument: usize) -> usize,
}

impl NativeHost {
    fn run(&self, effect: c_int, argument: usize) -> usize {
        (self.effect)(effect, argument)
    }
    /// This thread's `SwooleTG`.
    pub(crate) fn thread(&self) -> *mut c_void {
        ptr::with_exposed_provenance_mut(self.run(0, 0))
    }
    /// `SwooleG = {}`.
    pub(crate) fn reset(&self) {
        self.run(1, 0);
    }
    /// Allocate (`new String(SW_STACK_BUFFER_SIZE)`) or delete `SwooleTG.buffer_stack`.
    pub(crate) fn buffer(&self, allocate: bool) {
        self.run(2, usize::from(allocate));
    }
    /// `new GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true)`.
    pub(crate) fn new_pool(&self) -> *mut c_void {
        ptr::with_exposed_provenance_mut(self.run(3, 0))
    }
    /// `delete pool`: the pool `swoole_init` or the last fork made.
    pub(crate) fn delete_pool(&self, pool: *mut c_void) {
        self.run(4, pool.expose_provenance());
    }
    /// `SwooleG.logger->close()`, `->reopen()` and `delete SwooleG.logger`.
    pub(crate) fn close_logger(&self) {
        self.run(5, 0);
    }
    pub(crate) fn reopen_logger(&self) {
        self.run(6, 0);
    }
    pub(crate) fn delete_logger(&self) {
        self.run(7, 0);
    }
    pub(crate) fn signal_clear(&self) {
        self.run(8, 0);
    }
    pub(crate) fn signal_block_all(&self) {
        self.run(9, 0);
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
