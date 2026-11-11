//! PHP `Swoole\Runtime` (with the `php` feature, see docs/zend.md): the class,
//! the `SWOOLE_HOOK_*` constants, `getHookFlags`/`setHookFlags`/
//! `enableCoroutine`, and `PHPCoroutine::enable_hook`: which hook families
//! change, the function-table hooks (handlers, arginfo copies, the library's
//! PHP replacements, the classes they graft on) and the sleep family's
//! replacements, the coroutine socket streams (`stream`) and the transports
//! they swap (`transport`). The plain-file and stdio wrappers, the PDO
//! drivers, the coroutine state and the coroutine socket stay C++, lent as a
//! table and as calls.
//!
//! A warning can run a user error handler that bails out; when it does from
//! inside a function here, the jump crosses only frames that own nothing to
//! drop, and no lock is held across such a call. Calls that run PHP code
//! (autoloaders, the library's replacements, original handlers) are trapped,
//! and the handler or export Zend or C++ called re-raises the bailout last.
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

mod connect;
mod functions;
mod putenv;
mod select;
mod sleep;
mod socket;
mod stream;
mod transport;
pub use connect::*;
pub use putenv::*;
pub use socket::{Report, check_reactor, co, convert_to_fd, coroutine_check, report, strerror};
pub use stream::*;

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_phphelpers::callable as handle;
use swoole_rs_phpstubs::runtime;
use swoole_rs_zend::{
    Call, Class, E_WARNING, Message, Minit,
    bailout::{self, Bailout},
    message,
    sys::{
        swr_zend_callable_warning, swr_zend_runtime_find_class, swr_zend_stream_factory,
        swr_zend_stream_factory_set, zend_class_entry, zend_execute_data,
        zend_register_class_alias_ex, zif_handler, zval,
    },
};
use swoole_rs_zend::{method_table, sys as z};

/// `PHPCoroutine::HOOK_*`.
pub mod flags {
    pub const TCP: u32 = 1 << 1;
    pub const UDP: u32 = 1 << 2;
    pub const UNIX: u32 = 1 << 3;
    pub const UDG: u32 = 1 << 4;
    pub const SSL: u32 = 1 << 5;
    pub const TLS: u32 = 1 << 6;
    pub const STREAM_FUNCTION: u32 = 1 << 7;
    pub const FILE: u32 = 1 << 8;
    pub const SLEEP: u32 = 1 << 9;
    pub const PROC: u32 = 1 << 10;
    pub const CURL: u32 = 1 << 11;
    pub const NATIVE_CURL: u32 = 1 << 12;
    pub const SOCKETS: u32 = 1 << 14;
    pub const STDIO: u32 = 1 << 15;
    pub const PDO_PGSQL: u32 = 1 << 16;
    pub const PDO_ODBC: u32 = 1 << 17;
    pub const PDO_ORACLE: u32 = 1 << 18;
    pub const PDO_SQLITE: u32 = 1 << 19;
    pub const PDO_FIREBIRD: u32 = 1 << 20;
    pub const NET_FUNCTION: u32 = 1 << 21;
    pub const MONGODB: u32 = 1 << 22;
    /// The PDO drivers, in upstream's order.
    pub const PDO: [u32; 5] = [PDO_PGSQL, PDO_ODBC, PDO_ORACLE, PDO_SQLITE, PDO_FIREBIRD];

    /// `HOOK_ALL`: everything but MongoDB and the curl hook the build lacks
    /// (the PHP one when native curl is built in).
    #[must_use]
    pub const fn all(native_curl: bool) -> u32 {
        0x7fff_ffff & !MONGODB & if native_curl { !CURL } else { !NATIVE_CURL }
    }
}

/// `PHPCoroutine::get_hook_flags()` (crates/phpcoroutines').
fn hook_flags() -> u32 {
    swoole_rs_phpswoole::families().map_or(0, |families| (families.hook_flags)())
}

static RUNTIME: Class = Class::new();
/// `runtime_hook_flags`: the families hooked now.
static HOOKED: AtomicU32 = AtomicU32::new(0);

/// `swoole_exception_ce`, which MINIT registered before the runtime's.
fn exception() -> *mut zend_class_entry {
    swoole_rs_phpswoole::exception_class()
}

/// `sw_is_main_thread()`.
fn main_thread() -> bool {
    // SAFETY: no preconditions.
    unsafe { swoole_rs_zend::sys::swr_zend_tsrm_is_main_thread() }
}

/// `sw_callable_create(value)`: a crates/phphelpers callable, or null after a
/// warning. Null with a bailout pending (the warning's handler bailed out) is
/// one the caller stops on, leaving the raise to its handler's last step.
/// # Safety
/// PHP is running and `value` is a live zval.
pub unsafe fn callable_create(value: *mut zval) -> *mut c_void {
    // SAFETY: per the caller; the reporter is C.
    let created = unsafe {
        handle::factory(
            value,
            std::ptr::null(),
            true,
            exception(),
            swr_zend_callable_warning,
        )
    };
    created
        .map_err(bailout::Bailout::defer)
        .unwrap_or(std::ptr::null_mut())
        .cast()
}

/// `sw_callable_free(callable)`; null is ignored. The callable is gone even
/// when a destructor leaves a bailout pending, which the caller stops on.
/// # Safety
/// PHP is running; `callable` is one [`callable_create`] made, freed once and
/// held by no slot.
pub unsafe fn callable_free(callable: *mut c_void) {
    if !callable.is_null() {
        // SAFETY: per the caller.
        bailout::defer(unsafe { handle::release(callable.cast()) });
    }
}

/// `callable->dup()`.
/// # Safety
/// `callable` is live.
pub unsafe fn callable_dup(callable: *mut c_void) -> *mut c_void {
    // SAFETY: per the caller.
    unsafe { handle::copy(callable.cast()) }.cast()
}

/// [`callable_call`]'s arguments and result.
#[repr(C)]
struct CallableCall {
    callable: *mut c_void,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    from_loop: bool,
    result: bool,
}

/// [`callable_call`]'s trapped body.
/// # Safety
/// `call` is a live [`CallableCall`] whose callable, arguments and return slot
/// are live.
unsafe extern "C" fn invoke(call: *mut c_void) {
    // SAFETY: per the caller.
    let call = unsafe { &mut *call.cast::<CallableCall>() };
    let callable = call.callable.cast();
    call.result = if call.from_loop {
        // SAFETY: per the caller.
        let cache = unsafe { handle::cache(callable) };
        // SAFETY: per the caller; the reporter is C.
        let called = unsafe {
            swoole_rs_phphelpers::swr_v1_phphelpers_call(
                std::ptr::null_mut(),
                cache,
                call.argc,
                call.argv,
                call.retval,
                true,
                swr_zend_callable_warning,
            )
        };
        called == 0
    } else {
        // SAFETY: per the caller.
        let called = unsafe { handle::invoke(callable, call.argc, call.argv, call.retval) };
        bailout::finish();
        called.unwrap_or(false)
    };
}

/// `callable->call(args.len(), args, retval)` inside `zend_try`, or with
/// `from_loop` `sw_zend_call_function_ex2`, which raises an uncaught exception at
/// once as the event loop's callbacks do: whether the call succeeded.
/// # Errors
/// The callable bailed out.
/// # Safety
/// PHP is running, `callable` is live, `args` are live zvals and `retval` is
/// null or writable.
pub unsafe fn callable_call(
    callable: *mut c_void,
    args: &mut [zval],
    retval: *mut zval,
    from_loop: bool,
) -> Result<bool, Bailout> {
    let mut call = CallableCall {
        callable,
        #[expect(clippy::cast_possible_truncation, reason = "a handful of arguments")]
        argc: args.len() as u32,
        argv: args.as_mut_ptr(),
        retval,
        from_loop,
        result: false,
    };
    // SAFETY: per the caller; `invoke` takes a `CallableCall`.
    unsafe { bailout::trap(invoke, (&raw mut call).cast()) }?;
    Ok(call.result)
}

const SW_ERROR_PHP_FATAL_ERROR: i64 = 503;
const SW_ERROR_UNDEFINED_BEHAVIOR: i64 = 600;

/// The `SWOOLE_HOOK_*` constants before `SWOOLE_HOOK_ALL`, in upstream's
/// order; a PDO driver's only when it is built in.
const CONSTANTS: [(&CStr, u32); 22] = {
    use flags::{
        CURL, FILE, MONGODB, NATIVE_CURL, NET_FUNCTION, PDO_FIREBIRD, PDO_ODBC, PDO_ORACLE,
        PDO_PGSQL, PDO_SQLITE, PROC, SLEEP, SOCKETS, SSL, STDIO, STREAM_FUNCTION, TCP, TLS, UDG,
        UDP, UNIX,
    };
    [
        (c"SWOOLE_HOOK_TCP", TCP),
        (c"SWOOLE_HOOK_UDP", UDP),
        (c"SWOOLE_HOOK_UNIX", UNIX),
        (c"SWOOLE_HOOK_UDG", UDG),
        (c"SWOOLE_HOOK_SSL", SSL),
        (c"SWOOLE_HOOK_TLS", TLS),
        (c"SWOOLE_HOOK_STREAM_FUNCTION", STREAM_FUNCTION),
        // backward compatibility
        (c"SWOOLE_HOOK_STREAM_SELECT", STREAM_FUNCTION),
        (c"SWOOLE_HOOK_FILE", FILE),
        (c"SWOOLE_HOOK_STDIO", STDIO),
        (c"SWOOLE_HOOK_SLEEP", SLEEP),
        (c"SWOOLE_HOOK_PROC", PROC),
        (c"SWOOLE_HOOK_CURL", CURL),
        (c"SWOOLE_HOOK_NATIVE_CURL", NATIVE_CURL),
        (c"SWOOLE_HOOK_SOCKETS", SOCKETS),
        (c"SWOOLE_HOOK_PDO_PGSQL", PDO_PGSQL),
        (c"SWOOLE_HOOK_PDO_ODBC", PDO_ODBC),
        (c"SWOOLE_HOOK_PDO_ORACLE", PDO_ORACLE),
        (c"SWOOLE_HOOK_PDO_SQLITE", PDO_SQLITE),
        (c"SWOOLE_HOOK_PDO_FIREBIRD", PDO_FIREBIRD),
        (c"SWOOLE_HOOK_NET_FUNCTION", NET_FUNCTION),
        (c"SWOOLE_HOOK_MONGODB", MONGODB),
    ]
};

method_table! {
    static METHODS = [
        c"enableCoroutine" => zim_swoole_runtime_enableCoroutine, &runtime::arginfo_class_Swoole_Runtime_enableCoroutine, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getHookFlags" => zim_swoole_runtime_getHookFlags, &runtime::arginfo_class_Swoole_Runtime_getHookFlags, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"setHookFlags" => zim_swoole_runtime_setHookFlags, &runtime::arginfo_class_Swoole_Runtime_setHookFlags, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

/// The coroutine transport factory zend-sys registers for PHP, over
/// [`stream::transport_factory_body`].
fn factory() -> *mut c_void {
    swr_zend_stream_factory as *mut c_void
}

unsafe extern "C" {
    /// `thirdparty/php/standard/proc_open.cc`.
    fn swoole_proc_open_init(module_number: i32);
    /// `thirdparty/php/streams/plain_wrapper.c`'s copy of PHP's plain-files
    /// wrapper (a `php_stream_wrapper`, which only its address matters here).
    static mut sw_php_plain_files_wrapper: u8;
    fn php_register_url_stream_wrapper(protocol: *const c_char, wrapper: *mut c_void) -> c_int;
}

#[cfg(feature = "curl")]
unsafe extern "C" {
    /// crates/curl's native curl (`swoole_rs_curl.h`).
    fn swoole_native_curl_minit(module_number: i32);
    fn swoole_native_curl_mshutdown();
}

/// `php_swoole_runtime_minit`: `Swoole\Runtime` (not instantiable), the
/// `SWOOLE_HOOK_*` constants, native curl, `proc_open`'s resource and the
/// `async.file` wrapper.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_runtime_minit(module_number: i32) {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module_number) };
    minit.register_static(&RUNTIME, c"Swoole\\Runtime", &METHODS.0);
    let features = features();
    let all = flags::all(features & flags::NATIVE_CURL != 0);
    for (name, value) in CONSTANTS.into_iter().chain([(c"SWOOLE_HOOK_ALL", all)]) {
        if flags::PDO.contains(&value) && features & value == 0 {
            continue;
        }
        minit.long_constant(name, i64::from(value));
    }
    // SAFETY: MINIT, before any PHP thread starts.
    unsafe { swr_zend_stream_factory_set(Some(stream::transport_factory_body)) };
    transport::register_async(factory());
    #[cfg(feature = "curl")]
    // SAFETY: MINIT, with the module's number.
    unsafe {
        swoole_native_curl_minit(module_number);
    }
    // SAFETY: as above.
    unsafe { swoole_proc_open_init(module_number) };
    // SAFETY: MINIT; PHP keeps the static wrapper.
    unsafe {
        php_register_url_stream_wrapper(
            c"async.file".as_ptr(),
            (&raw mut sw_php_plain_files_wrapper).cast(),
        )
    };
}

/// `php_swoole_runtime_mshutdown`: native curl's.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_runtime_mshutdown() {
    #[cfg(feature = "curl")]
    // SAFETY: MSHUTDOWN, after its MINIT.
    unsafe {
        swoole_native_curl_mshutdown();
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_runtime_getHookFlags => get_hook_flags;
    zim_swoole_runtime_enableCoroutine => enable_coroutine;
    zim_swoole_runtime_setHookFlags => set_hook_flags_method;
}

/// `Runtime::getHookFlags()`.
fn get_hook_flags(call: &Call) {
    call.set_long(i64::from(hook_flags()));
}

/// `Runtime::enableCoroutine()`.
fn enable_coroutine(call: &Call) {
    swoole_rs_zend::bailout::defer(set_hook_flags(call, 0));
}

/// `Runtime::setHookFlags()`.
fn set_hook_flags_method(call: &Call) {
    swoole_rs_zend::bailout::defer(set_hook_flags(call, 1));
}

/// `runtime_set_hook_flags`: CLI only, then `min` to one flags argument.
fn set_hook_flags(call: &Call, min: u32) -> Result<(), Bailout> {
    if !swoole_rs_phpswoole::globals().cli.get() {
        let reported = socket::report(
            socket::Report::Fatal,
            swoole_rs_zend::E_ERROR,
            c"must be used in PHP CLI mode",
        );
        call.set_bool(false);
        return reported;
    }
    let all = flags::all(features() & flags::NATIVE_CURL != 0);
    let Ok(requested) = call
        .arity(min, 1)
        .and_then(|()| call.long(1, i64::from(all)))
    else {
        call.set_bool(false);
        return Ok(());
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream passes the zend_long as uint32_t"
    )]
    let requested = requested as u32;
    if let Some(families) = swoole_rs_phpswoole::families() {
        (families.set_hook_flags)(requested);
    }
    call.set_bool(enable_hook(requested)?);
    Ok(())
}

/// `PHPCoroutine::enable_hook(flags)`; a trapped bailout is raised last.
#[unsafe(export_name = "swr_v1_phpruntime_enable_hook")]
#[must_use]
pub extern "C" fn php_coroutine_enable_hook(flags: u32) -> bool {
    let enabled = enable_hook(flags);
    let applied = matches!(enabled, Ok(true));
    swoole_rs_zend::bailout::raise(enabled.map(|_| ()));
    applied
}

/// `PHPCoroutine::disable_hook()`.
#[must_use]
pub extern "C" fn php_coroutine_disable_hook() -> bool {
    php_coroutine_enable_hook(0)
}

/// `PHPCoroutine::enable_hook`.
/// # Errors
/// A hook's setup bailed out.
pub fn enable_hook(mut requested: u32) -> Result<bool, Bailout> {
    let threads = swoole_rs_phpswoole::families()
        .filter(|_| built_with(Switch::Thread))
        .map_or(1, |families| (families.active_threads)());
    if !main_thread() || threads > 1 {
        log_warning(
            c"PHPCoroutine::enable_hook(): The runtime hook can only set on the main thread and no child threads have been created",
        );
        return Ok(false);
    }
    call_hook(BEFORE_ENABLE_HOOK, &mut requested);
    let mut unavailable = 0;
    if !swoole_rs_zend::module_loaded(c"sockets") {
        unavailable |= flags::SOCKETS;
    }
    if !swoole_rs_phpswoole::enable_library() {
        unavailable |= flags::NET_FUNCTION | flags::MONGODB;
    }
    requested &= !unavailable;
    let installed = HOOKED.load(Ordering::Relaxed);

    requested = transports(requested, installed, |index, hook| {
        transport::register(index, hook, factory())
    });
    wrappers(requested, installed);
    functions::hook_all(requested, installed)?;
    mongodb(requested, installed)?;

    call_hook(BEFORE_ENABLE_HOOK + 1, &mut requested);
    HOOKED.store(requested, Ordering::Relaxed);
    Ok(true)
}

/// `PHP_SWOOLE_HOOK_BEFORE_ENABLE_HOOK` (`SW_GLOBAL_HOOK_USER`); the after
/// hook follows it.
const BEFORE_ENABLE_HOOK: i32 = 24;

/// `swoole_call_hook(kind, flags)` if a hook of `kind` is set.
fn call_hook(kind: i32, flags: &mut u32) {
    if swoole_rs_corebase::swoole_isset_hook(kind) {
        // SAFETY: the enable-hook callbacks take the flags word.
        unsafe { swoole_rs_corebase::swoole_call_hook(kind, std::ptr::from_mut(flags).cast()) };
    }
}

/// `Some(true)` when `bit` is requested and not installed, `Some(false)` when
/// it is installed and no longer requested.
const fn transition(requested: u32, installed: u32, bit: u32) -> Option<bool> {
    match (requested & bit != 0, installed & bit != 0) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// `hook_stream_factory`: a transport that fails to register is dropped from
/// the flags, with an exception.
fn transports(
    mut requested: u32,
    installed: u32,
    mut register: impl FnMut(usize, bool) -> i32,
) -> u32 {
    for (index, name) in transport::NAMES.iter().enumerate() {
        let bit = flags::TCP << index;
        match transition(requested, installed, bit) {
            Some(true) => {
                if register(index, true) != 0 {
                    requested ^= bit;
                    let name = name.to_string_lossy();
                    let message = message!("failed to register `{name}` stream transport factory");
                    throw(&message, SW_ERROR_PHP_FATAL_ERROR);
                }
            }
            Some(false) => {
                register(index, false);
            }
            None => {}
        }
    }
    requested
}

/// `hook_stream_ops` and `hook_pdo_driver`: the plain-files wrapper, the stdio
/// ops and the PDO drivers built in.
fn wrappers(requested: u32, installed: u32) {
    for (bit, stdio) in [(flags::FILE, 0), (flags::STDIO, 1)] {
        if let Some(hook) = transition(requested, installed, bit) {
            stream_ops(stdio, u8::from(hook));
        }
    }
    for (bit, driver) in flags::PDO.into_iter().zip(DRIVERS) {
        if features() & bit == 0 {
            continue;
        }
        if let Some(hook) = transition(requested, installed, bit) {
            set_blocking(driver, !hook);
        }
    }
}

/// `hook_mongodb`: alias `MongoDB\Client` to the library's class.
fn mongodb(requested: u32, installed: u32) -> Result<(), Bailout> {
    const ALIAS: &CStr = c"MongoDB\\Client";
    if transition(requested, installed, flags::MONGODB) != Some(true) || class(ALIAS).is_some() {
        return Ok(());
    }
    let Some(ce) = class(c"Swoole\\MongoDB\\Client") else {
        return Ok(());
    };
    // SAFETY: a registered class and a NUL-terminated name.
    if unsafe { zend_register_class_alias_ex(ALIAS.as_ptr(), ALIAS.count_bytes(), ce, true) } != 0 {
        let text = swoole_rs_zend::message!(
            "Cannot declare class {}, because the name is already in use",
            ALIAS.to_string_lossy()
        );
        swoole_rs_zend::zend_error(E_WARNING, text.as_cstr())?;
    }
    Ok(())
}

/// A class already declared (no autoloading).
fn class(name: &CStr) -> Option<*mut zend_class_entry> {
    // SAFETY: a NUL-terminated name and its length.
    let ce = unsafe { swr_zend_runtime_find_class(name.as_ptr(), name.count_bytes()) };
    (!ce.is_null()).then_some(ce)
}

/// `zend_throw_exception(swoole_exception_ce, message, code)`.
#[cfg(not(test))]
fn throw(message: &Message, code: i64) {
    // SAFETY: a handler's context; Zend copies the message.
    unsafe { swoole_rs_zend::throw_exception(exception(), message.as_cstr(), code) };
}

/// Swap the plain-files wrapper (0) or the stdio ops (1).
#[cfg(not(test))]
fn stream_ops(stdio: u8, hook: u8) {
    // SAFETY: `thirdparty/php/streams/plain_wrapper.c`'s swap, which keeps
    // PHP's originals; it has no preconditions.
    unsafe extern "C" {
        safe fn php_swoole_runtime_stream_ops(stdio: u8, hook: u8);
    }
    php_swoole_runtime_stream_ops(stdio, hook);
}

/// Unit tests link no PHP, so they see the swap as a trace entry.
#[cfg(test)]
fn stream_ops(stdio: u8, hook: u8) {
    tests::record(format!("stream:{stdio}:{hook}"));
}

/// The PDO drivers' switches, in [`flags::PDO`]'s order.
const DRIVERS: [Switch; 5] = [
    Switch::Pgsql,
    Switch::Odbc,
    Switch::Oracle,
    Switch::Sqlite,
    Switch::Firebird,
];

/// The `HOOK_*` bits of the PDO drivers built in, and `HOOK_NATIVE_CURL`
/// when native curl is.
#[cfg(not(test))]
fn features() -> u32 {
    let native_curl = [(Switch::Curl, flags::NATIVE_CURL)];
    flags::PDO
        .into_iter()
        .zip(DRIVERS)
        .map(|(bit, driver)| (driver, bit))
        .chain(native_curl)
        .filter(|(switch, _)| swoole_rs_corebase::built_with(*switch))
        .fold(0, |features, (_, bit)| features | bit)
}

/// Unit tests have two PDO drivers built in, and no native curl.
#[cfg(test)]
fn features() -> u32 {
    flags::PDO_PGSQL | flags::PDO_SQLITE
}

/// `swoole_<driver>_set_blocking(blocking)`.
#[cfg(not(test))]
fn set_blocking(driver: Switch, blocking: bool) {
    swoole_rs_phpswoole::set_blocking(driver, blocking);
}

#[cfg(test)]
fn set_blocking(driver: Switch, blocking: bool) {
    let bit = flags::PDO[DRIVERS.iter().position(|d| *d == driver).unwrap_or(0)];
    tests::record(format!("pdo:{bit}:{}", u8::from(blocking)));
}

/// Unit tests link no PHP, so they see the exception as a trace entry.
#[cfg(test)]
fn throw(message: &Message, code: i64) {
    tests::record(format!(
        "throw:{code}:{}",
        message.as_cstr().to_string_lossy()
    ));
}

/// `sw_logger()->put(SW_LOG_WARNING, message)`, which the level filters.
fn log_warning(message: &CStr) {
    const SW_LOG_WARNING: i32 = 4;
    if let Some(logger) = swoole_rs_utilities::process_logger() {
        let message = message.to_bytes();
        // SAFETY: the live process logger, with the message's bytes.
        unsafe {
            swoole_rs_utilities::swr_v1_logger_put(
                logger,
                SW_LOG_WARNING,
                message.as_ptr().cast(),
                message.len(),
            );
        }
    }
}

fn in_coroutine() -> bool {
    swoole_rs_phpswoole::in_coroutine()
}

/// `PHPCoroutine::disable_unsafe_function()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole12PHPCoroutine23disable_unsafe_functionEv")
)]
pub extern "C" fn disable_unsafe_function() {
    functions::unsafe_functions(true);
}

/// `PHPCoroutine::enable_unsafe_function()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole12PHPCoroutine22enable_unsafe_functionEv")
)]
pub extern "C" fn enable_unsafe_function() {
    functions::unsafe_functions(false);
}

/// RINIT: on the main thread, a new hook table with `exit` and `putenv`
/// replaced where built (1); elsewhere the main thread's hooks copied into
/// this thread's function table before PHP 8.3 (0).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpruntime_rinit() -> u8 {
    if !main_thread() {
        functions::copy_to_thread();
        return 0;
    }
    functions::start();
    transport::remember();
    1
}

/// `php_swoole_runtime_rshutdown`, on the main thread: unhook everything,
/// restore every function and class; a trapped bailout is raised last.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "php_swoole_runtime_rshutdown")
)]
pub extern "C" fn rshutdown() {
    if !main_thread() {
        return;
    }
    let disabled = enable_hook(0);
    functions::finish();
    swoole_rs_zend::bailout::raise(disabled.map(|_| ()));
}

/// `php_swoole_get_original_handler(name, length)`.
/// # Safety
/// `name` points to `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z31php_swoole_get_original_handlerPKcm")
)]
#[must_use]
pub unsafe extern "C" fn original_handler(name: *const c_char, length: usize) -> zif_handler {
    // SAFETY: per the caller.
    let name = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), length) };
    functions::original_handler(name)
}

/// The handler `name` had before the first hook, or null.
#[must_use]
pub fn original(name: &[u8]) -> zif_handler {
    functions::original_handler(name)
}

/// `swoole_user_func_handler`: run the function's PHP replacement (inside a
/// coroutine) or its original handler.
/// # Safety
/// Called by Zend with a live frame and return slot.
unsafe extern "C" fn user_func_handler(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: per the caller.
    let result = functions::user_func(&unsafe { Call::new(frame, value) }, frame, value);
    swoole_rs_zend::bailout::raise(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_all_drops_the_curl_hook_the_build_lacks() {
        assert_eq!(flags::all(true) & flags::CURL, 0);
        assert_ne!(flags::all(true) & flags::NATIVE_CURL, 0);
        assert_eq!(flags::all(false) & flags::NATIVE_CURL, 0);
        assert_ne!(flags::all(false) & flags::CURL, 0);
        assert_eq!(flags::all(false) & flags::MONGODB, 0, "MongoDB is opt-in");
        assert_eq!(
            flags::all(false),
            0x7fff_ffff & !flags::NATIVE_CURL & !flags::MONGODB
        );
    }

    #[test]
    fn transitions_name_what_changes() {
        assert_eq!(transition(flags::TCP, 0, flags::TCP), Some(true));
        assert_eq!(transition(0, flags::TCP, flags::TCP), Some(false));
        assert_eq!(transition(flags::TCP, flags::TCP, flags::TCP), None);
        assert_eq!(transition(flags::UDP, 0, flags::TCP), None);
    }

    thread_local! {
        static TRACE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub(super) fn record(entry: String) {
        TRACE.with(|trace| trace.borrow_mut().push(entry));
    }

    fn trace() -> Vec<String> {
        TRACE.with(|trace| std::mem::take(&mut *trace.borrow_mut()))
    }

    fn run(requested: u32, installed: u32) -> (u32, Vec<String>) {
        let requested = transports(requested, installed, |index, hook| {
            record(format!("transport:{index}:{}", u8::from(hook)));
            0
        });
        wrappers(requested, installed);
        (requested, trace())
    }

    #[test]
    fn transports_and_wrappers_change_only_what_changed_in_order() {
        let all = flags::all(false);
        let (flags, calls) = run(all, 0);
        assert_eq!(flags, all);
        assert_eq!(
            calls,
            [
                "transport:0:1",
                "transport:1:1",
                "transport:2:1",
                "transport:3:1",
                "transport:4:1",
                "transport:5:1",
                "stream:0:1",
                "stream:1:1",
                "pdo:65536:0",
                "pdo:524288:0",
            ]
        );
        let (_, calls) = run(0, all);
        assert_eq!(
            calls,
            [
                "transport:0:0",
                "transport:1:0",
                "transport:2:0",
                "transport:3:0",
                "transport:4:0",
                "transport:5:0",
                "stream:0:0",
                "stream:1:0",
                "pdo:65536:1",
                "pdo:524288:1",
            ]
        );
        assert_eq!(run(all, all).1, Vec::<String>::new(), "nothing changes");
        let (_, calls) = run(flags::SSL | flags::STDIO, flags::TCP | flags::STDIO);
        assert_eq!(calls, ["transport:0:0", "transport:4:1"]);
        let (_, calls) = run(flags::PDO_ODBC | flags::PDO_FIREBIRD, 0);
        assert!(calls.is_empty(), "drivers not built in stay untouched");
    }

    #[test]
    fn stream_select_is_the_stream_function_alias() {
        assert_eq!(CONSTANTS[6].1, CONSTANTS[7].1);
        assert_eq!(CONSTANTS[7].0, c"SWOOLE_HOOK_STREAM_SELECT");
    }
}
