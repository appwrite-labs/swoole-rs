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
pub use stream::*;

use std::{
    ffi::{CStr, c_char, c_void},
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};
use swoole_rs_phpstubs::runtime;
use swoole_rs_zend::{
    Call, Class, E_WARNING, Message,
    bailout::Bailout,
    message, register_long_constant,
    sys::{
        swr_zend_module_loaded, swr_zend_runtime_find_class, swr_zend_stream_factory,
        swr_zend_stream_factory_set, zend_class_entry, zend_execute_data, zend_fcall_info_cache,
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

/// What only C++ can do (`swr_v1_phpruntime_ops`).
#[repr(C)]
pub struct Ops {
    /// The `HOOK_*` bits of the PDO drivers built in, and `HOOK_NATIVE_CURL`
    /// when native curl is (`SW_USE_*`).
    features: u32,
    /// `swoole_exception_ce`.
    exception: *mut zend_class_entry,
    /// `Coroutine::get_current() != nullptr`.
    in_coroutine: unsafe extern "C" fn() -> u8,
    /// `SWOOLE_G(cli)`.
    cli: unsafe extern "C" fn() -> u8,
    /// `System::sleep(seconds)`.
    coroutine_sleep: unsafe extern "C" fn(seconds: f64) -> i32,
    /// `PHPCoroutine::set_hook_flags` and `get_hook_flags`.
    set_hook_flags: unsafe extern "C" fn(flags: u32),
    get_hook_flags: unsafe extern "C" fn() -> u32,
    /// `SWOOLE_G(enable_library)`.
    enable_library: unsafe extern "C" fn() -> u8,
    /// `sw_is_main_thread()`, and `sw_active_thread_count()`.
    main_thread: unsafe extern "C" fn() -> u8,
    active_threads: unsafe extern "C" fn() -> u32,
    /// The before (0) or after (1) enable-hook global hook, if set.
    call_hook: unsafe extern "C" fn(after: u8, flags: *mut u32),
    /// Swap the plain-files wrapper (0) or the stdio ops (1).
    stream_ops: unsafe extern "C" fn(stdio: u8, hook: u8),
    /// `swoole_<driver>_set_blocking(blocking)` for a `HOOK_PDO_*` bit.
    pdo: unsafe extern "C" fn(flag: u32, blocking: u8),
    /// `sw_callable_create(name)`: a `zend::Callable *` or null.
    callable: unsafe extern "C" fn(name: *mut zval) -> *mut c_void,
    /// `sw_callable_free`.
    callable_free: unsafe extern "C" fn(callable: *mut c_void),
    /// `callable->ptr()`.
    callable_fcc: unsafe extern "C" fn(callable: *mut c_void) -> *mut zend_fcall_info_cache,
    /// `sw_logger()->put(SW_LOG_WARNING, ..)` at that level.
    log_warning: unsafe extern "C" fn(message: *const c_char),
    /// `php_swoole_fatal_error(level, "%s", message)`.
    fatal_error: unsafe extern "C" fn(level: i32, message: *const c_char),
    /// `php_swoole_error(level, "%s", message)`.
    error: unsafe extern "C" fn(level: i32, message: *const c_char),
    /// Handlers that stay C++: the replacements of those names (`exit` and
    /// `putenv` null when not built).
    proc_open: zif_handler,
    proc_close: zif_handler,
    proc_get_status: zif_handler,
    proc_terminate: zif_handler,
    gethostbyname: zif_handler,
    exit: zif_handler,
    putenv: zif_handler,
}

// SAFETY: a static table of function pointers and a class entry, both
// process-global and read-only.
unsafe impl Sync for Ops {}

static OPS: OnceLock<&'static Ops> = OnceLock::new();
static RUNTIME: Class = Class::new();
/// `runtime_hook_flags`: the families hooked now.
static HOOKED: AtomicU32 = AtomicU32::new(0);

fn ops() -> &'static Ops {
    // Functions run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
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

/// MINIT: `Swoole\Runtime` (not instantiable) and the `SWOOLE_HOOK_*`
/// constants, as `php_swoole_runtime_minit` registered them.
/// # Safety
/// Called once, at MINIT, with upstream's static method table and a static
/// operations table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_minit(module_number: i32, ops: *const Ops) {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    // SAFETY: MINIT, with a static table.
    unsafe { RUNTIME.register_static(c"Swoole\\Runtime", METHODS.as_ptr()) };
    let features = self::ops().features;
    let all = flags::all(features & flags::NATIVE_CURL != 0);
    for (name, value) in CONSTANTS.into_iter().chain([(c"SWOOLE_HOOK_ALL", all)]) {
        if flags::PDO.contains(&value) && features & value == 0 {
            continue;
        }
        // SAFETY: MINIT, with the module's number.
        unsafe { register_long_constant(name, i64::from(value), module_number) };
    }
    // SAFETY: MINIT, before any PHP thread starts.
    unsafe { swr_zend_stream_factory_set(Some(stream::transport_factory_body)) };
    transport::register_async(factory());
}

swoole_rs_zend::handlers! {
    zim_swoole_runtime_getHookFlags => get_hook_flags;
    zim_swoole_runtime_enableCoroutine => enable_coroutine;
    zim_swoole_runtime_setHookFlags => set_hook_flags_method;
}

/// `Runtime::getHookFlags()`.
fn get_hook_flags(call: &Call) {
    // SAFETY: C++'s accessor.
    call.set_long(i64::from(unsafe { (ops().get_hook_flags)() }));
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
    // SAFETY: C++'s accessor.
    if unsafe { (ops().cli)() } == 0 {
        let reported = socket::report(
            socket::Report::Fatal,
            swoole_rs_zend::E_ERROR,
            c"must be used in PHP CLI mode",
        );
        call.set_bool(false);
        return reported;
    }
    let all = flags::all(ops().features & flags::NATIVE_CURL != 0);
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
    // SAFETY: C++'s setter.
    unsafe { (ops().set_hook_flags)(requested) };
    call.set_bool(enable_hook(requested)?);
    Ok(())
}

/// `PHPCoroutine::enable_hook(flags)`; a trapped bailout is raised last.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole12PHPCoroutine11enable_hookEj")
)]
#[must_use]
pub extern "C" fn php_coroutine_enable_hook(flags: u32) -> bool {
    let enabled = enable_hook(flags);
    let applied = matches!(enabled, Ok(true));
    swoole_rs_zend::bailout::raise(enabled.map(|_| ()));
    applied
}

/// `PHPCoroutine::disable_hook()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole12PHPCoroutine12disable_hookEv")
)]
#[must_use]
pub extern "C" fn php_coroutine_disable_hook() -> bool {
    php_coroutine_enable_hook(0)
}

/// `PHPCoroutine::enable_hook`.
fn enable_hook(mut requested: u32) -> Result<bool, Bailout> {
    let ops = ops();
    // SAFETY: C++'s thread checks.
    if unsafe { (ops.main_thread)() } == 0 || unsafe { (ops.active_threads)() } > 1 {
        log_warning(
            c"PHPCoroutine::enable_hook(): The runtime hook can only set on the main thread and no child threads have been created",
        );
        return Ok(false);
    }
    // SAFETY: a writable flags word.
    unsafe { (ops.call_hook)(0, &raw mut requested) };
    let mut unavailable = 0;
    // SAFETY: a name and its length.
    if !unsafe { swr_zend_module_loaded(c"sockets".as_ptr(), 7) } {
        unavailable |= flags::SOCKETS;
    }
    // SAFETY: C++'s accessor.
    if unsafe { (ops.enable_library)() } == 0 {
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

    // SAFETY: as above.
    unsafe { (ops.call_hook)(1, &raw mut requested) };
    HOOKED.store(requested, Ordering::Relaxed);
    Ok(true)
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
    let ops = ops();
    for (bit, stdio) in [(flags::FILE, 0), (flags::STDIO, 1)] {
        if let Some(hook) = transition(requested, installed, bit) {
            // SAFETY: C++'s wrapper swap.
            unsafe { (ops.stream_ops)(stdio, u8::from(hook)) };
        }
    }
    for bit in flags::PDO {
        if ops.features & bit == 0 {
            continue;
        }
        if let Some(hook) = transition(requested, installed, bit) {
            // SAFETY: a driver that is built in.
            unsafe { (ops.pdo)(bit, u8::from(!hook)) };
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
        // SAFETY: from a handler.
        unsafe { swoole_rs_zend::zend_error(E_WARNING, text.as_cstr()) }?;
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
    unsafe { swoole_rs_zend::throw_exception(ops().exception, message.as_cstr(), code) };
}

/// Unit tests link no PHP, so they see the exception as a trace entry.
#[cfg(test)]
fn throw(message: &Message, code: i64) {
    tests::record(format!(
        "throw:{code}:{}",
        message.as_cstr().to_string_lossy()
    ));
}

fn log_warning(message: &CStr) {
    // SAFETY: a NUL-terminated message.
    unsafe { (ops().log_warning)(message.as_ptr()) };
}

fn in_coroutine() -> bool {
    // SAFETY: C++'s check.
    unsafe { (ops().in_coroutine)() != 0 }
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
    // SAFETY: C++'s check.
    if unsafe { (ops().main_thread)() } == 0 {
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
    // SAFETY: C++'s check.
    if unsafe { (ops().main_thread)() } == 0 {
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

    unsafe extern "C" fn no() -> u8 {
        0
    }
    unsafe extern "C" fn zero() -> u32 {
        0
    }
    unsafe extern "C" fn sleep(_: f64) -> i32 {
        0
    }
    unsafe extern "C" fn flags_in(_: u32) {}
    unsafe extern "C" fn hook(_: u8, _: *mut u32) {}
    unsafe extern "C" fn stream_ops(stdio: u8, hook: u8) {
        record(format!("stream:{stdio}:{hook}"));
    }
    unsafe extern "C" fn pdo(flag: u32, blocking: u8) {
        record(format!("pdo:{flag}:{blocking}"));
    }
    unsafe extern "C" fn callable(_: *mut zval) -> *mut c_void {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn free(_: *mut c_void) {}
    unsafe extern "C" fn fcc(_: *mut c_void) -> *mut zend_fcall_info_cache {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn log(_: *const c_char) {}
    unsafe extern "C" fn error(_: i32, _: *const c_char) {}

    /// Two PDO drivers built in, no native curl.
    static FAKE: Ops = Ops {
        features: flags::PDO_PGSQL | flags::PDO_SQLITE,
        exception: std::ptr::null_mut(),
        in_coroutine: no,
        cli: no,
        coroutine_sleep: sleep,
        set_hook_flags: flags_in,
        get_hook_flags: zero,
        enable_library: no,
        main_thread: no,
        active_threads: zero,
        call_hook: hook,
        stream_ops,
        pdo,
        callable,
        callable_free: free,
        callable_fcc: fcc,
        log_warning: log,
        fatal_error: error,
        error,
        proc_open: None,
        proc_close: None,
        proc_get_status: None,
        proc_terminate: None,
        gethostbyname: None,
        exit: None,
        putenv: None,
    };

    fn run(requested: u32, installed: u32) -> (u32, Vec<String>) {
        OPS.get_or_init(|| &FAKE);
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
