// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! `swoole_init`: the leaves corebase's lifecycles run live in the crates above
//! it, so this archive, which has them all, registers them before corebase's body
//! runs (`swoole_rs_corebase::native`). src/fatal.c holds what stays C.
use std::ffi::{c_char, c_int};
use swoole_rs_corebase::memory::members::pool;
use swoole_rs_corebase::native::{self, NativeHost};
use swoole_rs_corebase::{Switch, built_with};

// src/fatal.c.
unsafe extern "C" {
    fn swr_v1_ffi_fatal_error(code: c_int, format: *const c_char, ...);
}

/// `void swoole_init()`.
#[unsafe(export_name = "_Z11swoole_initv")]
pub(crate) extern "C-unwind" fn swoole_init() {
    native::set_native_host(NativeHost { init });
    swoole_rs_corebase::coroutine::set_host(swoole_rs_corebase::coroutine::Host {
        async_threads_ensure: || _ = swoole_rs_hooks::async_threads::ensure(),
    });
    swoole_rs_corebase::reactor::event_api::set_writers(swoole_rs_tls::reactor_write::WRITERS);
    swoole_rs_corebase::reactor::event_api::set_init_reactor([
        |reactor| swoole_rs_hooks::reactor_handlers::system_init_reactor(reactor),
        swoole_rs_clients::client::init_reactor,
    ]);
    swoole_rs_corebase::reactor::host::set_host(swoole_rs_corebase::reactor::host::Host {
        sleep: swoole_rs_hooks::sleep::system_sleep,
        socket_free: |socket| swoole_rs_tls::upstream::socket_free(Some(socket)),
    });
    swoole_rs_corebase::process::methods::set_listener(|kind, host, port, backlog| {
        let listener = swoole_rs_tls::upstream::server_socket(kind, host, port, backlog)?;
        Some(std::rc::Rc::new(listener))
    });
    swoole_rs_clients::client::set_host(
        swoole_rs_ports::framing_recv::FRAMING,
        swoole_rs_hooks::resolve::dispatch_client,
    );
    swoole_rs_ports::register_methods();
    swoole_rs_hooks::stdio::swr_v1_hook_install();
    swoole_rs_corebase::swr_v1_core_swoole_init();
}

/// `swoole_init`'s native leaves, in upstream's order.
fn init() {
    swoole_rs_error::swoole_clear_last_error_msg();
    swoole_rs_corebase::swr_v1_core_set_stdout(Some(swoole_rs_corebase::Stream::stdout()));
    swoole_rs_corebase::set_fatal_error(Some(swr_v1_ffi_fatal_error));
    swoole_rs_corebase::process::methods::logger_new();
    if built_with(Switch::Debug) {
        swoole_rs_corebase::utilities::swoole_set_log_level(0);
        swoole_rs_corebase::swr_v1_core_set_trace_flags(0x7fff_ffff);
    }
    pool::init();
    if !built_with(Switch::Futex) {
        pool::wait_registry_init();
    }
    #[cfg(target_os = "linux")]
    if built_with(Switch::Signalfd) {
        swoole_rs_corebase::process::signalfd::swoole_signalfd_init();
        swoole_rs_corebase::swr_v1_core_set_enable_signalfd(true);
    }
    swoole_rs_corebase::bug_report_init();
}

/// fatal.c's `swoole_exit`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_ffi_exit(status: c_int) -> ! {
    swoole_rs_corebase::swoole_exit(status)
}
