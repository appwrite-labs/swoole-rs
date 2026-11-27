//! The swoole module host (with the `php` feature, see docs/zend.md): the global
//! constants, `Swoole\Exception` and `Swoole\Error`, the global functions of
//! `php_swoole.cc`, the module entry with its function table and lifecycle
//! (MINIT, MSHUTDOWN, RINIT, RSHUTDOWN, MINFO), INI definitions, information rows
//! and `SwooleG.fatal_error`, and the module globals. native/phphelpers/host.c
//! keeps what only C can say (see its contract.md); Swoole's core, error and
//! logger state are the Rust crates'.
#![cfg(feature = "php")]
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod asynchronous;
pub use asynchronous::zif_swoole_async_dns_lookup_coro;
use swoole_rs_corebase::CCast;
mod constants;
mod entry;
mod functions;
mod implicit;
mod info;
mod ini;
mod interfaces;
pub mod length;
mod memory;
mod module;
mod optional;
mod options;
pub mod reactor;
mod sendfile;
pub use sendfile::valid_sendfile_range;
mod serialize;
mod table;
mod vm;

pub use asynchronous::php_swoole_set_aio_option;
pub use entry::{families, module_entry, swr_v1_phpswoole_fatal_exit};
pub use functions::{mime_type, zif_swoole_last_error};
pub use implicit::exit_status;
pub use info::swr_v1_phpswoole_minfo;
pub use interfaces::sys_warning;
pub use memory::{received_string, zend_string_allocator};
pub use module::{
    EventSteps, Families, Globals, Hook, Http, HttpMinit, Minit, MinitArgs, ProcessSteps,
    ReqStatus, RuntimeSteps, SocketSettings, codecs, display_errors, enable_library, globals,
    load_library, php_swoole_register_shutdown_function, temporary_directory,
    zif_swoole_set_process_name,
};
pub use optional::{set_blocking, socket_cast, socket_create};
pub use options::{
    invalid_setting, nonnegative_setting, php_swoole_parse_to_size, set_global, setting_callable,
    setting_ok, sw_zval_is_serializable,
};
pub use serialize::{serialize, unserialize, unserialize_from};
pub use swoole_rs_corebase::{Switch, built_with};

use std::{
    ffi::{CStr, c_int},
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};
use swoole_rs_zend::{
    Class,
    sys::{zend_class_entry, zend_object_handlers},
};

/// The `swr_v1_phpswoole_minit` flags.
static FLAGS: AtomicU32 = AtomicU32::new(0);
static EXCEPTION: Class = Class::new();
static ERROR: Class = Class::new();

/// `sw_logger()->put(level, message)`; nothing before MINIT creates the logger.
fn log_put(level: c_int, message: &[u8]) {
    if let Some(logger) = swoole_rs_corebase::utilities::process_logger() {
        logger.put(level, message);
    }
}

/// `Swoole\Exception` and `Swoole\Error`, for code that links them by name
/// (crates/phpredis, crates/phpevents and the shims), and Exception's handlers,
/// which upstream's bundled curl copies for its exception class
/// (`SW_INIT_CLASS_ENTRY_EX`, through the header's `swoole_exception_handlers`);
/// MINIT writes them. C reads each as a pointer: `AtomicPtr` has a pointer's
/// layout.
#[unsafe(no_mangle)]
pub static swoole_exception_ce: AtomicPtr<zend_class_entry> = AtomicPtr::new(ptr::null_mut());
#[unsafe(no_mangle)]
pub static swr_swoole_exception_handlers: AtomicPtr<zend_object_handlers> =
    AtomicPtr::new(ptr::null_mut());
#[unsafe(no_mangle)]
pub static swoole_error_ce: AtomicPtr<zend_class_entry> = AtomicPtr::new(ptr::null_mut());

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
    // Nothing is reported while a bailout is pending.
    swoole_rs_zend::bailout::outcome(())?;
    // SAFETY: as upstream raises it; the bailout jumps over this closure only,
    // which holds a number and a borrow.
    swoole_rs_zend::contain_with(|| unsafe { swoole_rs_corebase::fatal_error(code, message) })
}

swoole_rs_zend::exports! {
/// `PHP_MINIT_FUNCTION(swoole)`: the module globals and ini entries, the
/// global constants, then `Swoole\Exception` and `Swoole\Error`
/// (`swoole_exception_ce` and `swoole_error_ce`),
/// the short-name function aliases, then Swoole's core and every family
/// (`module::startup`).
pub(crate) extern "C" fn swr_v1_phpswoole_minit(
    kind: c_int,
    startup: swoole_rs_zend::Startup<'_>,
) -> c_int {
    startup.with(|minit| minit_body(minit, kind));
    0
}

}

fn minit_body(minit: &swoole_rs_zend::Minit, kind: c_int) {
    begin(minit, kind);
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    let mut flags = constants::flags();
    if module::globals().use_shortname.get() {
        flags |= constants::SHORTNAME;
    }
    FLAGS.store(flags, Ordering::Relaxed);
    constants::register(minit, flags);
    sendfile::register(minit);
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Exception",
        swoole_rs_zend::arginfo::Table::NONE,
        minit.exception(),
        |_, _| {},
    );
    // Registered above, so its handlers are set, for the process.
    let handlers = EXCEPTION.handlers().cast_mut();
    swr_swoole_exception_handlers.store(handlers, Ordering::Relaxed);

    minit.register_child(
        &ERROR,
        c"Swoole\\Error",
        swoole_rs_zend::arginfo::Table::NONE,
        minit.error(),
        |_, _| {},
    );
    // MINIT, before any family reads them.
    swoole_exception_ce.store(EXCEPTION.entry(), Ordering::Relaxed);
    swoole_error_ce.store(ERROR.entry(), Ordering::Relaxed);
    if let Err(error) = table::register_aliases(minit, flags) {
        error.defer();
    } else {
        module::startup(minit);
    }
}

/// MINIT's start: the `SWOOLE_RS_*` constants, then the module globals and
/// their ini entries.
fn begin(minit: &swoole_rs_zend::Minit, kind: c_int) {
    let features = swoole_rs_error::swr_v1_feature_bits().c_cast::<i64>();
    let abi = i64::from(swoole_rs_error::swr_v1_abi_version());
    for (name, value) in [
        (c"SWOOLE_RS_ABI_VERSION", abi),
        (c"SWOOLE_RS_FEATURE_BITS", features),
    ] {
        minit.long_constant(name, value);
    }
    ini::register(minit, kind);
}

/// `php_swoole_check_reactor()`: this thread's event loop, unless the request
/// is shutting down.
pub fn check_reactor() {
    let shutting_down = module::globals().req_status.get() == ReqStatus::RshutdownBegin;
    if !shutting_down && swoole_rs_corebase::reactor::thread_reactor().is_null() {
        _ = reactor_init();
    }
}

/// [`EventSteps::reactor_init`]: false before crates/ffi registers the families.
#[must_use]
pub fn reactor_init() -> bool {
    families().is_some_and(|families| (families.events.reactor_init)())
}

/// [`EventSteps::convert_to_fd`]: the fd, or `None` after its warning (or before
/// crates/ffi registers the families).
#[must_use]
pub fn convert_to_fd(value: Option<&swoole_rs_zend::Value>) -> Option<c_int> {
    families()
        .map(|families| (families.events.convert_to_fd)(value))
        .filter(|&fd| fd >= 0)
}

/// [`EventSteps::wait`], once crates/ffi has registered the families.
pub fn event_wait() {
    if let Some(families) = families() {
        (families.events.wait)();
    }
}

/// The bundled `proc_open` requests the same initialization as Rust callers.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_check_reactor() {
    check_reactor();
}

/// `Coroutine::get_current() != nullptr`: whether this thread runs a coroutine.
#[must_use]
pub fn in_coroutine() -> bool {
    !swoole_rs_corebase::coroutine::current().is_null()
}

/// `Coroutine::get_current_safe()`: Swoole's fatal error outside a coroutine,
/// which exits, or ends this PHP thread with the bailout this returns.
/// # Errors
/// The fatal error's bailout.
pub fn coroutine_check() -> Result<(), swoole_rs_zend::bailout::Bailout> {
    /// `SW_ERROR_CO_OUT_OF_COROUTINE`.
    const OUT_OF_COROUTINE: c_int =
        swoole_rs_error::ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as c_int;
    if in_coroutine() {
        return Ok(());
    }
    swoole_fatal_error(OUT_OF_COROUTINE, c"API must be called in the coroutine")
}
