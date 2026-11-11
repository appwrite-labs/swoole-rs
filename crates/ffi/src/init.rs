// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! `swoole_init`: the leaves corebase's lifecycles run live in the crates above
//! it, so this archive, which has them all, registers them before corebase's body
//! runs (`swoole_rs_corebase::native`). src/fatal.c holds what stays C.
use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE;
use swoole_rs_corebase::native::{self, LOGGER_NEW, NativeHost};
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_memory::members::pool;

// src/fatal.c.
unsafe extern "C" {
    safe fn swr_v1_ffi_fatal_error(code: c_int, format: *const c_char, ...);
    safe fn swr_v1_ffi_stdout() -> *mut c_void;
}

thread_local! {
    /// This thread's `SwooleTG.buffer_stack`, which lives as long as the thread.
    static BUFFER: RefCell<CppString> = const { RefCell::new(CppString::EMPTY) };
}

/// `void swoole_init()`.
#[unsafe(export_name = "_Z11swoole_initv")]
extern "C-unwind" fn swoole_init() {
    swoole_rs_corebase::log::swr_v1_core_set_log_host(swoole_rs_utilities::swr_v1_utils_log);
    native::set_native_host(NativeHost {
        init,
        new_buffer: || {
            BUFFER.with(|buffer| {
                buffer
                    .borrow_mut()
                    .alloc(SW_STACK_BUFFER_SIZE as usize, None);
                buffer.as_ptr().cast()
            })
        },
        delete_buffer: || BUFFER.with(|buffer| buffer.replace(CppString::EMPTY).dispose()),
        delete_pool: pool::delete,
        logger: swoole_rs_process::methods::logger_op,
        signal_clear: swoole_rs_reactor::signal_upstream::swoole_signal_clear,
        timer_free: || swoole_rs_timer::swoole_timer_free(),
        event_available: || swoole_rs_reactor::event_api::swoole_event_is_available(),
        event_free: || _ = swoole_rs_reactor::swoole_event_free(),
        coroutine_is_in: || !swoole_rs_coroutine::current().is_null(),
    });
    swoole_rs_clients::client::set_framing(swoole_rs_ports::framing_recv::FRAMING);
    swoole_rs_ports::register_methods();
    swoole_rs_hooks::stdio::swr_v1_hook_install();
    swoole_rs_corebase::swr_v1_core_swoole_init();
}

/// `swoole_init`'s native leaves, in upstream's order.
fn init() {
    swoole_rs_error::swoole_clear_last_error_msg();
    swoole_rs_corebase::swr_v1_core_set_stdout(swr_v1_ffi_stdout());
    swoole_rs_corebase::swr_v1_core_set_fatal_error(swr_v1_ffi_fatal_error as *mut c_void);
    swoole_rs_process::methods::logger_op(LOGGER_NEW);
    if built_with(Switch::Debug) {
        swoole_rs_utilities::swoole_set_log_level(0);
        swoole_rs_corebase::swr_v1_core_set_trace_flags(0x7fff_ffff);
    }
    pool::init();
    if !built_with(Switch::Futex) {
        pool::wait_registry_init();
    }
    #[cfg(target_os = "linux")]
    if built_with(Switch::Signalfd) {
        swoole_rs_process::signalfd::swoole_signalfd_init();
        swoole_rs_corebase::swr_v1_core_set_enable_signalfd(true);
    }
    swoole_rs_corebase::bug_report_init();
}

/// fatal.c's `swoole_exit`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_ffi_exit(status: c_int) -> ! {
    swoole_rs_corebase::swoole_exit(status)
}
