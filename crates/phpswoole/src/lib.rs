//! The swoole module host (with the `php` feature, see docs/zend.md): the global
//! constants, `Swoole\Exception` and `Swoole\Error`, the global functions of
//! `php_swoole.cc`, the module entry with its function table and lifecycle
//! (MINIT, MSHUTDOWN, RINIT, RSHUTDOWN, MINFO), INI definitions, information rows
//! and `SwooleG.fatal_error`. C++ keeps the module globals and lends them, with
//! what only configure's `#ifdef`s decide or C++ objects hold, as a table of
//! operations; Swoole's core, error and logger state are the Rust crates'.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod asynchronous;
mod constants;
mod entry;
mod functions;
mod implicit;
mod info;
mod info_inventory;
mod ini;
mod interfaces;
mod memory;
mod module;
mod options;
mod serialize;
mod table;
mod vm;

pub use asynchronous::php_swoole_set_aio_option;
pub use entry::{swr_v1_phpswoole_fatal_exit, swr_v1_phpswoole_module};
pub use functions::mime_type;
pub use info::{built_with, swr_v1_phpswoole_minfo};
pub use info_inventory::Capability;
pub use ini::swr_v1_phpswoole_register_ini;
pub use interfaces::sys_warning;
pub use memory::sw_zend_string_allocator;
pub use module::{
    CALL_USER_SHUTDOWNFUNC_BEGIN, Globals, display_errors, enable_library, globals,
    php_swoole_parse_to_size, php_swoole_register_shutdown_function, swr_v1_phpswoole_init_globals,
    swr_v1_phpswoole_mshutdown, swr_v1_phpswoole_rinit, swr_v1_phpswoole_rshutdown,
    zif_swoole_set_process_name,
};
pub use options::{php_swoole_set_global_option, set_global, sw_zval_is_serializable};
pub use serialize::{php_swoole_serialize, php_swoole_unserialize, serialize, unserialize};

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
    arginfo::Handler,
    sys::{
        zend_array, zend_ce_error, zend_ce_exception, zend_class_entry, zend_object_handlers, zval,
    },
};

/// What only C++ can reach (`swr_v1_phpswoole_ops`).
#[repr(C)]
pub struct HostOps {
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
    /// `zend::known_strings_dtor()`.
    known_strings_dtor: unsafe extern "C" fn(),
    /// `swoole_set_task_tmpdir(dir)`.
    set_task_tmpdir: unsafe extern "C" fn(dir: *const c_char),
    /// `php_swoole_load_library()`, a `zend_try` body.
    load_library: unsafe extern "C" fn(*mut c_void),
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
    /// `SwooleG.print_backtrace_on_error`, or C++'s exception when the build
    /// has no stack traces.
    set_print_backtrace: unsafe extern "C" fn(on: u8),
    set_dns_server: unsafe extern "C" fn(server: *const c_char, length: usize),
    // `swoole_async_set()` and the coroutine DNS cache's remaining native field.
    set_dns_cache_refresh_time: unsafe extern "C" fn(seconds: f64),
    dns_cache_refresh_time: unsafe extern "C" fn() -> f64,
    /// `SW_MUST_BE_MAIN_THREAD`'s test: always true without `SW_THREAD`.
    main_thread: unsafe extern "C" fn() -> u8,
    /// `sw_reactor() != nullptr`.
    reactor_created: unsafe extern "C" fn() -> u8,
    /// `Coroutine::get_current_safe()`: a fatal error outside a coroutine.
    current_coroutine_safe: unsafe extern "C" fn(),
    /// `php_swoole_check_reactor()`.
    check_reactor: unsafe extern "C" fn(),
    /// `swoole::coroutine::dns_lookup(domain, family, timeout)`, each address
    /// pushed onto `list` in order. The coroutine yields.
    dns_lookup: unsafe extern "C" fn(
        domain: *const c_char,
        family: i32,
        timeout: f64,
        push: extern "C" fn(list: *mut c_void, address: *const c_char),
        list: *mut c_void,
    ),
    /// MINIT's start: the bridge ABI check, the native effect provider, the
    /// `SWOOLE_RS_*` constants, the module globals and ini entries. 0 on success.
    begin: unsafe extern "C" fn(kind: c_int, module_number: c_int) -> c_int,
    /// The `swr_v1_phpswoole_minit` flags the build's `#ifdef`s set.
    minit_flags: unsafe extern "C" fn() -> u32,
    /// MINFO's compiler and library facts.
    info: unsafe extern "C" fn(info: *mut info::Info),
    /// The stdext, ssh2 and ftp handler arrays (null when the build lacks one).
    stdext: *const Option<Handler>,
    ssh2: *const Option<Handler>,
    ftp: *const Option<Handler>,
    /// `Coroutine::get_socket_bound_cid()`.
    socket_bound_cid: unsafe extern "C" fn() -> i64,
    /// `php_swoole_thread_bailout()` (nothing without `SW_THREAD`).
    thread_bailout: unsafe extern "C" fn(),
}

// SAFETY: a static table of functions and static arrays, never written.
unsafe impl Sync for HostOps {}

static OPS: OnceLock<&'static HostOps> = OnceLock::new();
/// The `swr_v1_phpswoole_minit` flags.
static FLAGS: AtomicU32 = AtomicU32::new(0);
static MODULE_NUMBER: AtomicI32 = AtomicI32::new(0);
static EXCEPTION: Class = Class::new();
static ERROR: Class = Class::new();

/// `sw_logger()->put(level, message)`; nothing before MINIT creates the logger.
fn log_put(level: c_int, message: &[u8]) {
    if let Some(logger) = swoole_rs_utilities::process_logger() {
        // SAFETY: the live process logger, with the message's bytes.
        unsafe {
            swoole_rs_utilities::swr_v1_logger_put(
                logger,
                level,
                message.as_ptr().cast(),
                message.len(),
            );
        }
    }
}

fn ops() -> &'static HostOps {
    // Functions run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

/// `Swoole\Exception`, its handlers (which its subclasses copy) and
/// `Swoole\Error`, as C++ reads them; MINIT writes them.
#[unsafe(no_mangle)]
pub static mut swoole_exception_ce: *mut zend_class_entry = ptr::null_mut();
#[unsafe(no_mangle)]
// SAFETY: a table of nullable pointers and an offset, for which zero is valid.
pub static mut swoole_exception_handlers: zend_object_handlers = unsafe { std::mem::zeroed() };
#[unsafe(no_mangle)]
pub static mut swoole_error_ce: *mut zend_class_entry = ptr::null_mut();

/// `PHP_MINIT_FUNCTION(swoole)`: C++'s start (the module globals and ini
/// entries), the global constants, then `Swoole\Exception` and `Swoole\Error`
/// (`swoole_exception_ce`, `swoole_exception_handlers` and `swoole_error_ce`),
/// the short-name function aliases, then Swoole's core and every family
/// (`module::startup`).
/// # Safety
/// Zend's MINIT, after `swr_v1_phpswoole_module` stored the operations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_minit(kind: c_int, module_number: c_int) -> c_int {
    let ops = ops();
    // SAFETY: MINIT, with Zend's arguments.
    if unsafe { (ops.begin)(kind, module_number) } != 0 {
        return -1;
    }
    // SAFETY: C++'s constant flags.
    let mut flags = unsafe { (ops.minit_flags)() };
    // SAFETY: this thread's globals, which the ini entries just set.
    if unsafe { (*module::globals()).use_shortname } {
        flags |= constants::SHORTNAME;
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
    // SAFETY: MINIT, before any family copies it.
    unsafe { swoole_exception_handlers = handlers };
    // SAFETY: Zend registers Error before any extension's MINIT.
    let parent = unsafe { zend_ce_error };
    // SAFETY: MINIT, extending Error, with no methods.
    unsafe { ERROR.register_child(c"Swoole\\Error", ptr::null(), parent, |_, _| {}) };
    // SAFETY: MINIT, before any family reads them.
    unsafe { swoole_exception_ce = EXCEPTION.entry() };
    // SAFETY: as above.
    unsafe { swoole_error_ce = ERROR.entry() };
    // SAFETY: MINIT, after Zend registered the module's functions.
    if let Err(error) = unsafe { table::register_aliases(flags) } {
        error.defer();
    } else {
        module::startup(module_number);
    }
    swoole_rs_zend::bailout::finish();
    0
}

/// `php_swoole_check_reactor()`.
pub fn check_reactor() {
    // SAFETY: C++'s check, after MINIT stored the table.
    unsafe { (ops().check_reactor)() };
}

/// `swoole_set_task_tmpdir(dir)`.
pub fn set_task_tmpdir(dir: &std::ffi::CStr) {
    // SAFETY: C++'s setter, which copies it.
    unsafe { (ops().set_task_tmpdir)(dir.as_ptr()) };
}

/// `php_swoole_load_library()`, inside `zend_try`.
/// # Errors
/// The library's code bailed out.
pub fn load_library() -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // SAFETY: C++'s loader, a `zend_try` body.
    unsafe { swoole_rs_zend::bailout::trap(ops().load_library, ptr::null_mut()) }
}

/// `sw_module_number()`: the number Zend gave the module at MINIT.
#[unsafe(no_mangle)]
pub extern "C" fn sw_module_number() -> c_int {
    MODULE_NUMBER.load(Ordering::Relaxed)
}
