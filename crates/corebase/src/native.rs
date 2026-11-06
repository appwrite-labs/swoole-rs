// SPDX-License-Identifier: Apache-2.0
//! The C++ effects Rust sequences (`lifecycle.rs`): native/corebase/globals.cc's
//! `swoole_init` registers them (`swr_v1_core_set_native_host`), so the archive
//! keeps no link-time reference to C++ and the native gates link it without them.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

/// `swr_v1_core_native_host` (`native/include/swoole_rs_corebase_state.h`): native
/// code promises each entry is safe to call once `swoole_init` has registered it.
#[repr(C)]
pub struct NativeHost {
    /// `swoole_get_env`; throws, as `std::stoi` does, on a malformed value.
    pub get_env: unsafe extern "C-unwind" fn(name: *const c_char, value: *mut c_int) -> bool,
    /// This thread's `SwooleTG`.
    pub thread: extern "C" fn() -> *mut c_void,
    /// `SwooleG = {}`.
    pub reset: extern "C-unwind" fn(),
    /// Allocate (`new String(SW_STACK_BUFFER_SIZE)`) or delete `SwooleTG.buffer_stack`.
    pub buffer: extern "C-unwind" fn(allocate: bool),
    /// `new GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true)`, and its `delete`.
    pub new_pool: extern "C-unwind" fn() -> *mut c_void,
    pub delete_pool: unsafe extern "C-unwind" fn(pool: *mut c_void),
    /// `SwooleG.logger`: 0 `close()`, 1 `reopen()`, 2 `delete`.
    pub logger: extern "C-unwind" fn(operation: c_int),
    pub signal_clear: extern "C-unwind" fn(),
    pub signal_block_all: extern "C-unwind" fn(),
    pub timer_available: extern "C-unwind" fn() -> bool,
    pub timer_free: extern "C-unwind" fn(),
    pub event_available: extern "C-unwind" fn() -> bool,
    pub event_free: extern "C-unwind" fn() -> c_int,
    pub coroutine_is_in: extern "C-unwind" fn() -> u8,
}

static HOST: OnceLock<&'static NativeHost> = OnceLock::new();

/// Registers the host; later registrations are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_native_host(host: &'static NativeHost) {
    HOST.get_or_init(|| host);
}

/// The registered host, once `swoole_init` has run.
#[must_use]
pub fn host() -> Option<&'static NativeHost> {
    HOST.get().copied()
}
