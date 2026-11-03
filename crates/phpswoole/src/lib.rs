//! The swoole module host (with the `php` feature, see docs/zend.md): the global
//! constants, `Swoole\Exception` and `Swoole\Error`, the global functions of
//! `php_swoole.cc`, the module's function table and its lifecycle (MINIT's
//! sequence, MSHUTDOWN, RINIT, RSHUTDOWN). C++ keeps the module globals' ini
//! entries and Swoole's core, error, logger and MIME state, which it lends as a
//! table of operations.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod constants;
mod functions;
mod interfaces;
mod memory;
mod module;
mod options;
mod table;
mod vm;

pub use module::{
    Globals, php_swoole_parse_to_size, php_swoole_register_shutdown_function,
    swr_v1_phpswoole_init_globals, zif_swoole_set_process_name, zm_activate_swoole,
    zm_deactivate_swoole, zm_shutdown_swoole,
};
pub use options::{php_swoole_set_global_option, sw_zval_is_serializable};
pub use table::swr_v1_phpswoole_functions;

use std::{
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicI32, AtomicU32, Ordering},
    },
};
use swoole_rs_zend::{
    Class,
    sys::{
        zend_array, zend_ce_error, zend_ce_exception, zend_class_entry, zend_object_handlers, zval,
    },
};

/// What only C++ can reach (`swr_v1_phpswoole_ops`).
#[repr(C)]
pub struct HostOps {
    /// `swoole_get_last_error()`.
    last_error: unsafe extern "C" fn() -> i32,
    /// `swoole_set_last_error(code)`.
    set_last_error: unsafe extern "C" fn(code: i32),
    /// `ZEND_MODULE_GLOBALS_BULK(swoole)`: this thread's module globals.
    globals: unsafe extern "C" fn() -> *mut module::Globals,
    /// MINIT, after the constants: `swoole_init()`, the after-fork hook and
    /// the bug report's PHP version.
    startup: unsafe extern "C" fn(),
    /// MINIT's end: Swoole's fatal-error and backtrace hooks, the socket
    /// buffer size and DNS cache refresh time, macOS's `pcre.jit`, the known
    /// strings.
    ready: unsafe extern "C" fn(),
    /// The hooks only C++'s `#ifdef`s decide, where upstream calls them:
    /// ssh2 and ftp after the clients; the drivers, `Swoole\Thread` and
    /// stdext before the tracer; and their MSHUTDOWN, RINIT and RSHUTDOWN.
    optional_client_minit: unsafe extern "C" fn(module_number: c_int),
    optional_minit: unsafe extern "C" fn(module_number: c_int),
    optional_mshutdown: unsafe extern "C" fn(),
    optional_rinit: unsafe extern "C" fn(),
    optional_rshutdown: unsafe extern "C" fn(),
    /// `zend::known_strings_dtor()` and `swoole_clean()`.
    known_strings_dtor: unsafe extern "C" fn(),
    clean: unsafe extern "C" fn(),
    /// `swoole_set_task_tmpdir(dir)`.
    set_task_tmpdir: unsafe extern "C" fn(dir: *const c_char),
    /// `php_swoole_load_library()`, a `zend_try` body.
    load_library: unsafe extern "C" fn(*mut c_void),
    /// `swoole_event_free()`.
    event_free: unsafe extern "C" fn(),
    /// `SW_CPU_NUM`.
    cpu_num: unsafe extern "C" fn() -> u32,
    /// `sw_logger()->put(level, message, length)`.
    log_put: unsafe extern "C" fn(level: i32, message: *const c_char, length: usize),
    /// `swoole_error_log`'s tail: set the error, then put the formatted
    /// message if the level passes and the error is not ignored.
    error_log: unsafe extern "C" fn(level: i32, error: i32, message: *const c_char, length: usize),
    /// `swoole_ignore_error(code)`.
    ignore_error: unsafe extern "C" fn(code: i32),
    mime_add: unsafe extern "C" fn(suffix: *const c_char, kind: *const c_char) -> u8,
    mime_set: unsafe extern "C" fn(suffix: *const c_char, kind: *const c_char),
    mime_del: unsafe extern "C" fn(suffix: *const c_char) -> u8,
    mime_get: unsafe extern "C" fn(filename: *const c_char) -> *const c_char,
    mime_exists: unsafe extern "C" fn(filename: *const c_char) -> u8,
    /// Push every MIME type onto `array`.
    mime_list: unsafe extern "C" fn(array: *mut zval),
    /// `zend::unserialize`.
    unserialize: unsafe extern "C" fn(
        value: *mut zval,
        buf: *const c_char,
        length: usize,
        options: *mut zend_array,
    ),
    /// `zend::json_decode`.
    json_decode: unsafe extern "C" fn(
        value: *mut zval,
        text: *const c_char,
        length: usize,
        options: i64,
        depth: i64,
    ),
    // `php_swoole_set_global_option`'s effects, in its order.
    set_signalfd: unsafe extern "C" fn(on: u8),
    set_kqueue: unsafe extern "C" fn(on: u8),
    set_trace_flags: unsafe extern "C" fn(flags: i64),
    set_log_file: unsafe extern "C" fn(file: *const c_char, length: usize),
    set_log_level: unsafe extern "C" fn(level: i64),
    set_log_date_format: unsafe extern "C" fn(format: *const c_char, length: usize),
    set_log_date_with_microseconds: unsafe extern "C" fn(on: u8),
    set_log_rotation: unsafe extern "C" fn(rotation: i64),
    /// `SwooleG.print_backtrace_on_error`, or C++'s exception when the build
    /// has no stack traces.
    set_print_backtrace: unsafe extern "C" fn(on: u8),
    set_dns_server: unsafe extern "C" fn(server: *const c_char, length: usize),
    /// `Socket::default_{dns,connect,read,write}_timeout` (`kind` 0..=3).
    set_socket_timeout: unsafe extern "C" fn(kind: i32, seconds: f64),
    set_socket_buffer_size: unsafe extern "C" fn(size: i64),
    /// `http2::put_default_setting(id, value)`.
    set_http2_default: unsafe extern "C" fn(id: i32, value: i64),
}

static OPS: OnceLock<&'static HostOps> = OnceLock::new();
/// The `swr_v1_phpswoole_minit` flags.
static FLAGS: AtomicU32 = AtomicU32::new(0);
static MODULE_NUMBER: AtomicI32 = AtomicI32::new(0);
static EXCEPTION: Class = Class::new();
static ERROR: Class = Class::new();

fn ops() -> &'static HostOps {
    // Functions run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

/// MINIT after the module globals and ini entries: the global constants, then
/// `Swoole\Exception` and `Swoole\Error`, whose entries (and
/// `Swoole\Exception`'s handlers, which its subclasses copy) are written to
/// the C++ globals, the short-name function aliases, then Swoole's core and
/// every family (`module::startup`).
/// # Safety
/// Called once, at MINIT, with the module's number, a static operations table
/// and writable class-entry globals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_minit(
    module_number: i32,
    flags: u32,
    ops: *const HostOps,
    exception: *mut *mut zend_class_entry,
    exception_handlers: *mut zend_object_handlers,
    error: *mut *mut zend_class_entry,
) {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    FLAGS.store(flags, Ordering::Relaxed);
    MODULE_NUMBER.store(module_number, Ordering::Relaxed);
    // SAFETY: MINIT, with the module's number.
    unsafe { constants::register(module_number, flags) };
    // SAFETY: Zend registers Exception before any extension's MINIT.
    let parent = unsafe { zend_ce_exception };
    // SAFETY: MINIT, extending Exception, with no methods.
    unsafe { EXCEPTION.register_child(c"Swoole\\Exception", ptr::null(), parent, |_, _| {}) };
    // SAFETY: registered above, so its handlers are set.
    let handlers = unsafe { *EXCEPTION.handlers() };
    // SAFETY: per the caller, a writable global; subclasses copy it.
    unsafe { exception_handlers.write(handlers) };
    // SAFETY: Zend registers Error before any extension's MINIT.
    let parent = unsafe { zend_ce_error };
    // SAFETY: MINIT, extending Error, with no methods.
    unsafe { ERROR.register_child(c"Swoole\\Error", ptr::null(), parent, |_, _| {}) };
    // SAFETY: per the caller, writable globals.
    unsafe { exception.write(EXCEPTION.entry()) };
    // SAFETY: as above.
    unsafe { error.write(ERROR.entry()) };
    // SAFETY: MINIT, after Zend registered the module's functions.
    unsafe { table::register_aliases(flags) };
    module::startup(module_number);
    swoole_rs_zend::bailout::finish();
}

/// `sw_module_number()`: the number Zend gave the module at MINIT.
#[unsafe(no_mangle)]
pub extern "C" fn sw_module_number() -> c_int {
    MODULE_NUMBER.load(Ordering::Relaxed)
}
