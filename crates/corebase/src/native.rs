// SPDX-License-Identifier: Apache-2.0
//! The C++ functions of `src/core/base.cc` that Rust calls. base.cc registers
//! them once, with its log macros (`swr_v1_core_set_native_host`), so the archive
//! keeps no link-time reference to C++: the native gates link it without base.cc.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

/// base.cc's `swoole_fork`, `swoole_isset_hook`, `swoole_call_hook` and `swoole_get_env`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeHost {
    pub fork: unsafe extern "C" fn(flags: c_int) -> libc::pid_t,
    pub isset_hook: unsafe extern "C" fn(kind: c_int) -> bool,
    pub call_hook: unsafe extern "C" fn(kind: c_int, arg: *mut c_void),
    /// Throws, as `std::stoi` does, on a malformed value.
    pub get_env: unsafe extern "C-unwind" fn(name: *const c_char, value: *mut c_int) -> bool,
}

static HOST: OnceLock<NativeHost> = OnceLock::new();

/// Registers base.cc's functions; later registrations are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_native_host(host: NativeHost) {
    HOST.get_or_init(|| host);
}

/// base.cc's functions, once it has registered them.
#[must_use]
pub fn host() -> Option<&'static NativeHost> {
    HOST.get()
}
