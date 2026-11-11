//! The swoole module host (with the `php` feature, see docs/zend.md): the global
//! constants, `Swoole\Exception` and `Swoole\Error`, the global functions of
//! `php_swoole.cc`, the module entry with its function table and lifecycle
//! (MINIT, MSHUTDOWN, RINIT, RSHUTDOWN, MINFO), INI definitions, information rows
//! and `SwooleG.fatal_error`, and the module globals. native/phphelpers/host.c
//! keeps what only C can say (see its contract.md); Swoole's core, error and
//! logger state are the Rust crates'.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
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
mod ini;
mod interfaces;
mod memory;
mod module;
mod optional;
mod options;
pub mod reactor;
mod serialize;
mod table;
mod vm;

pub use asynchronous::php_swoole_set_aio_option;
pub use entry::{module_entry, swr_v1_phpswoole_fatal_exit};
pub use functions::mime_type;
pub use implicit::sw_php_exit;
pub use info::swr_v1_phpswoole_minfo;
pub use ini::swr_v1_phpswoole_register_ini;
pub use interfaces::sys_warning;
pub use memory::sw_zend_string_allocator;
pub use module::{
    CALL_USER_SHUTDOWNFUNC_BEGIN, Families, Globals, Hook, Http, HttpMinit, Minit, MinitArgs,
    RSHUTDOWN_BEGIN, RSHUTDOWN_END, codecs, display_errors, enable_library, families, globals,
    load_library, php_swoole_register_shutdown_function, swr_v1_phpswoole_mshutdown,
    swr_v1_phpswoole_rinit, swr_v1_phpswoole_rshutdown, zif_swoole_set_process_name,
};
pub use optional::{set_blocking, socket_cast, socket_create};
pub use options::{
    php_swoole_parse_to_size, php_swoole_set_global_option, set_global, sw_zval_is_serializable,
};
pub use serialize::{serialize, unserialize};
pub use swoole_rs_corebase::{Switch, built_with};

use std::{
    ffi::{CStr, c_char, c_int},
    ptr,
    sync::atomic::{AtomicI32, AtomicU32, Ordering},
};
use swoole_rs_zend::{
    Class,
    sys::{zend_class_entry, zend_object_handlers},
};

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

/// `Swoole\Exception` and `Swoole\Error`, for code that links them by name
/// (crates/phpredis, crates/phpevents and the shims), and Exception's handlers,
/// which upstream's bundled curl copies by name for its exception class
/// (`SW_INIT_CLASS_ENTRY_EX`); MINIT writes them.
#[unsafe(no_mangle)]
pub static mut swoole_exception_ce: *mut zend_class_entry = ptr::null_mut();
#[unsafe(no_mangle)]
// SAFETY: a table of nullable pointers and an offset, for which zero is valid.
pub static mut swoole_exception_handlers: zend_object_handlers = unsafe { std::mem::zeroed() };
#[unsafe(no_mangle)]
pub static mut swoole_error_ce: *mut zend_class_entry = ptr::null_mut();

/// `swoole_exception_ce`: `Swoole\Exception`, registered at MINIT.
#[must_use]
pub fn exception_class() -> *mut zend_class_entry {
    EXCEPTION.entry()
}

/// `Swoole\Exception`, to throw.
#[must_use]
pub fn exception() -> &'static Class {
    &EXCEPTION
}

/// `swoole_fatal_error(code, "%s", message)` from a handler: a `Swoole\Error`
/// printed, then the process exits, or this PHP thread ends with the bailout
/// this returns, trapped below the caller's frames.
/// # Errors
/// The thread's bailout; defer it ([`swoole_rs_zend::bailout::defer`]).
pub fn swoole_fatal_error(
    code: c_int,
    message: &CStr,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    /// The hook, from the shim's `zend_try`, through this frame, which owns nothing.
    unsafe extern "C" fn fatal(code: c_int, _: c_int, message: *const c_char) {
        // SAFETY: `report_code`'s NUL-terminated message.
        let message = unsafe { CStr::from_ptr(message) };
        // SAFETY: as above.
        unsafe { swoole_rs_corebase::fatal_error(code, message) };
    }
    // SAFETY: `fatal` is a C-ABI reporter.
    unsafe { swoole_rs_zend::bailout::report_code(fatal, code, 0, message) }
}

/// `PHP_MINIT_FUNCTION(swoole)`: the module globals and ini entries, the
/// global constants, then `Swoole\Exception` and `Swoole\Error`
/// (`swoole_exception_ce` and `swoole_error_ce`),
/// the short-name function aliases, then Swoole's core and every family
/// (`module::startup`).
/// # Safety
/// Zend's MINIT.
pub unsafe extern "C" fn swr_v1_phpswoole_minit(kind: c_int, module_number: c_int) -> c_int {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { swoole_rs_zend::Minit::new(module_number) };
    begin(&minit, kind);
    let mut flags = constants::flags();
    if module::globals().use_shortname.get() {
        flags |= constants::SHORTNAME;
    }
    FLAGS.store(flags, Ordering::Relaxed);
    MODULE_NUMBER.store(module_number, Ordering::Relaxed);
    constants::register(&minit, flags);
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Exception",
        &[],
        minit.exception(),
        |_, _| {},
    );
    // SAFETY: registered above, so its handlers are set.
    let handlers = unsafe { *EXCEPTION.handlers() };
    // SAFETY: MINIT, before any family copies it.
    unsafe { swoole_exception_handlers = handlers };
    minit.register_child(&ERROR, c"Swoole\\Error", &[], minit.error(), |_, _| {});
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

/// MINIT's start: the `SWOOLE_RS_*` constants, then the module globals and
/// their ini entries.
fn begin(minit: &swoole_rs_zend::Minit, kind: c_int) {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "upstream's zend_long of the bits"
    )]
    let features = swoole_rs_error::swr_v1_feature_bits() as i64;
    let abi = i64::from(swoole_rs_error::swr_v1_abi_version());
    for (name, value) in [
        (c"SWOOLE_RS_ABI_VERSION", abi),
        (c"SWOOLE_RS_FEATURE_BITS", features),
    ] {
        minit.long_constant(name, value);
    }
    let globals = module::globals_init();
    // SAFETY: MINIT, once, after the globals' constructor, with their
    // process-lifetime address.
    unsafe { ini::swr_v1_phpswoole_register_ini(globals, minit.module(), kind) };
}

/// `php_swoole_check_reactor()`: this thread's event loop, unless the request
/// is shutting down.
pub fn check_reactor() {
    let shutting_down = module::globals().req_status.get() == RSHUTDOWN_BEGIN;
    if !shutting_down && swoole_rs_corebase::thread_reactor().is_null() {
        php_swoole_reactor_init();
    }
}

unsafe extern "C" {
    /// crates/phpevents' loop setup, on this thread.
    safe fn php_swoole_reactor_init() -> c_int;
}

/// `Coroutine::get_current() != nullptr`: whether this thread runs a coroutine.
#[must_use]
pub fn in_coroutine() -> bool {
    !swoole_rs_coroutine::current().is_null()
}

/// `Coroutine::get_current_safe()`: Swoole's fatal error outside a coroutine,
/// which exits or bails out across the caller's frames.
pub fn coroutine_check() {
    /// `SW_ERROR_CO_OUT_OF_COROUTINE`.
    const OUT_OF_COROUTINE: c_int = 10001;
    if !in_coroutine() {
        // SAFETY: as upstream raises it here; the callers' frames own nothing.
        unsafe {
            swoole_rs_corebase::fatal_error(
                OUT_OF_COROUTINE,
                c"API must be called in the coroutine",
            );
        }
    }
}

/// `swoole_set_task_tmpdir(dir)`.
pub fn set_task_tmpdir(dir: &CStr) {
    // SAFETY: a NUL-terminated path, which the setter copies.
    unsafe { swoole_rs_corebase::swr_v1_core_set_task_tmpdir(dir.as_ptr()) };
}

/// `sw_module_number()`: the number Zend gave the module at MINIT.
#[unsafe(no_mangle)]
pub extern "C" fn sw_module_number() -> c_int {
    MODULE_NUMBER.load(Ordering::Relaxed)
}
