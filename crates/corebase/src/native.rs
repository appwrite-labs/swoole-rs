// SPDX-License-Identifier: Apache-2.0
//! The C++ functions of `src/core/misc.cc` that Rust calls. `swoole_init` registers
//! them (`swr_v1_core_set_native_host`), so the archive keeps no link-time reference
//! to C++: the native gates link it without misc.cc, or with misc.cc but no `swoole_init`.

use std::ffi::{c_char, c_int};
use std::sync::OnceLock;

/// misc.cc's `swoole_fork` and `swoole_get_env`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeHost {
    pub fork: unsafe extern "C" fn(flags: c_int) -> libc::pid_t,
    /// Throws, as `std::stoi` does, on a malformed value.
    pub get_env: unsafe extern "C-unwind" fn(name: *const c_char, value: *mut c_int) -> bool,
}

static HOST: OnceLock<NativeHost> = OnceLock::new();

/// Registers misc.cc's functions; later registrations are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_native_host(host: NativeHost) {
    HOST.get_or_init(|| host);
}

/// misc.cc's functions, once it has registered them.
#[must_use]
pub fn host() -> Option<&'static NativeHost> {
    HOST.get()
}
