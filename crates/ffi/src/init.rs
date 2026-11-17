// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! `swoole_init`: register hooks and client adapters, then initialize corebase.
//! src/fatal.c holds the C variadic fatal handler passed to that initialization.
use std::ffi::{c_char, c_int};

// src/fatal.c.
unsafe extern "C" {
    fn swr_v1_ffi_fatal_error(code: c_int, format: *const c_char, ...);
}

/// `void swoole_init()`.
#[unsafe(export_name = "_Z11swoole_initv")]
pub(crate) extern "C-unwind" fn swoole_init() {
    swoole_rs_corebase::reactor::readiness::set_init_reactor(
        swoole_rs_hooks::reactor_handlers::SOCKET,
        swoole_rs_hooks::reactor_handlers::IOURING,
        swoole_rs_clients::client::init_reactor,
    );
    swoole_rs_clients::client::set_host(
        swoole_rs_ports::framing_recv::FRAMING,
        swoole_rs_hooks::resolve::dispatch_client,
    );
    swoole_rs_ports::register_methods();
    swoole_rs_hooks::stdio::swr_v1_hook_install();
    swoole_rs_corebase::swr_v1_core_swoole_init(Some(swr_v1_ffi_fatal_error));
}

/// fatal.c's `swoole_exit`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_ffi_exit(status: c_int) -> ! {
    swoole_rs_corebase::swoole_exit(status)
}
