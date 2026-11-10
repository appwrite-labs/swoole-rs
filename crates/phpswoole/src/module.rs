//! The module's lifecycle, in upstream's order: the rest of MINIT after the
//! constants and classes, and MSHUTDOWN, RINIT and RSHUTDOWN
//! (`swr_v1_phpswoole_mshutdown`, `_rinit`, `_rshutdown`). Each
//! family's hook is C (`php_swoole_private.h` declares them `extern "C"`) and
//! runs inside `zend_try`: a bailout stops the phase there, as it did
//! upstream, and is raised once this frame is the only Rust one left.
use crate::{Switch, optional};
use std::{
    cell::Cell,
    ffi::{CStr, c_int, c_long, c_void},
    ptr,
    sync::OnceLock,
};
use swoole_rs_corebase::{
    log::set_last_error, swr_v1_core_set_fatal_error, swr_v1_core_set_print_backtrace,
};
use swoole_rs_zend::{
    Call, E_WARNING,
    bailout::{self, Bailout},
    php_error,
    sys::{
        HashTable, swr_zend_after_fork, swr_zend_compiling_preload, swr_zend_execute_ex_reset,
        swr_zend_execute_ex_restore, swr_zend_fatal_error, swr_zend_function_find,
        swr_zend_leak_check, swr_zend_module_globals, swr_zend_module_globals_init,
        swr_zend_php_version, swr_zend_print_stack_trace, swr_zend_register_shutdown_function,
        swr_zend_runtime_call_handler, swr_zend_sapi_name, swr_zend_signals_uncheck,
        swr_zend_stream_constant_no_close, swr_zend_temporary_directory, swr_zend_try_hook,
        swr_zend_try_minit, zend_class_entry, zend_execute_data, zval,
    },
};

/// `zend_swoole_globals` (`php_swoole.h`'s layout, which Zend's ini handlers
/// write by offset). Each field is a cell: this thread reads and writes them
/// through shared references, and Zend through the offsets.
#[repr(C)]
pub struct Globals {
    pub(crate) display_errors: Cell<bool>,
    pub cli: Cell<bool>,
    pub use_shortname: Cell<bool>,
    pub enable_preemptive_scheduler: Cell<bool>,
    pub enable_library: Cell<bool>,
    pub enable_fiber_mock: Cell<bool>,
    blocking_detection: Cell<bool>,
    blocking_threshold: Cell<i64>,
    profile: Cell<bool>,
    leak_detection: Cell<bool>,
    socket_buffer_size: Cell<i64>,
    pub req_status: Cell<c_int>,
    in_autoload: Cell<*mut HashTable>,
}

pub(crate) const INI_OFFSETS: [usize; 10] = [
    std::mem::offset_of!(Globals, enable_library),
    std::mem::offset_of!(Globals, enable_fiber_mock),
    std::mem::offset_of!(Globals, enable_preemptive_scheduler),
    std::mem::offset_of!(Globals, display_errors),
    std::mem::offset_of!(Globals, use_shortname),
    std::mem::offset_of!(Globals, socket_buffer_size),
    std::mem::offset_of!(Globals, blocking_detection),
    std::mem::offset_of!(Globals, blocking_threshold),
    std::mem::offset_of!(Globals, profile),
    std::mem::offset_of!(Globals, leak_detection),
];

/// `php_swoole_req_status`.
const RINIT_BEGIN: c_int = 0;
pub(crate) const RINIT_END: c_int = 1;
pub const CALL_USER_SHUTDOWNFUNC_BEGIN: c_int = 2;
pub const RSHUTDOWN_BEGIN: c_int = 3;
pub const RSHUTDOWN_END: c_int = 4;
/// `SW_SOCKET_BUFFER_SIZE` and `SW_ERROR_PHP_FATAL_ERROR`.
const SOCKET_BUFFER_SIZE: i64 = 8 * 1024 * 1024;
pub(crate) const PHP_FATAL_ERROR: i32 = 503;
/// `SUCCESS`.
const SUCCESS: c_int = 0;

/// `swoole_globals`: the module globals outside ZTS, which C++ reads by name;
/// under ZTS each thread's are TSRM's.
#[unsafe(no_mangle)]
// SAFETY: booleans, integers and a nullable pointer, for which zero is valid.
static mut swoole_globals: Globals = unsafe { std::mem::zeroed() };

/// This thread's module globals, live from MINIT for the thread's life
/// (`Globals` is not `Sync`, so the reference stays on this thread).
#[must_use]
pub fn globals() -> &'static Globals {
    // SAFETY: `ZEND_MODULE_GLOBALS_BULK(swoole)`, with the NTS storage.
    let globals = unsafe { swr_zend_module_globals((&raw mut swoole_globals).cast()) };
    // SAFETY: this thread's globals, which Zend allocated at MINIT (or at the
    // thread's start under ZTS) and initialised with `init_globals`.
    unsafe { &*globals.cast::<Globals>() }
}

/// `ZEND_INIT_MODULE_GLOBALS(swoole, init_globals, nullptr)`: what the ini
/// entries take, the TSRM id under ZTS or else the storage.
pub(crate) fn globals_init() -> *mut c_void {
    // SAFETY: MINIT, once, before any thread reads them.
    unsafe {
        swr_zend_module_globals_init(
            (&raw mut swoole_globals).cast(),
            size_of::<Globals>(),
            Some(init_globals),
        )
    }
}

/// `SWOOLE_G(display_errors)`.
#[must_use]
pub fn display_errors() -> bool {
    globals().display_errors.get()
}

/// `SWOOLE_G(enable_library)`.
#[must_use]
pub fn enable_library() -> bool {
    globals().enable_library.get()
}

/// A family's MINIT hook, with the module's number (C ABI: it runs inside `zend_try`).
pub type Minit = unsafe extern "C" fn(c_int);
/// Any other phase hook, likewise.
pub type Hook = unsafe extern "C" fn();

/// The hooks of the crates above this one in the dependency graph
/// (crates/phpcoroutines, phpthread, phpserver and phpclients), which it runs where
/// upstream's phases did, and which crates/phpruntime and phpprocess, between
/// them, read too. crates/ffi, which links them all, registers them with
/// [`crate::module_entry`] when Zend asks for the module entry, before any phase.
pub struct Families {
    /// `php_swoole_coroutine_minit`, `_system_minit` and `_scheduler_minit`.
    pub coroutine_minit: [Minit; 3],
    /// `php_swoole_server_minit`.
    pub server_minit: Minit,
    /// `php_swoole_tracer_minit(threshold, blocking, profile, leaks)`, at MINIT, once.
    pub tracer_minit: unsafe fn(i64, bool, bool, bool),
    /// `Swoole\Thread`'s MINIT, then (after the thread locks') its barrier's,
    /// queue's, map's and list's, each at MINIT before any PHP thread starts.
    pub thread_minit: unsafe fn(),
    pub thread_classes_minit: [unsafe fn(); 4],
    /// RINIT's coroutine, thread and tracer steps.
    pub coroutine_rinit: Hook,
    pub thread_rinit: Hook,
    pub tracer_rinit: Hook,
    /// RSHUTDOWN's server, coroutine and scheduler, thread and tracer steps.
    pub server_rshutdown: Hook,
    pub coroutine_rshutdown: [Hook; 2],
    pub thread_rshutdown: Hook,
    pub tracer_rshutdown: Hook,
    /// `php_swoole_thread_bailout()`: the bailout that ends this PHP thread.
    pub thread_bailout: extern "C" fn(),
    /// `sw_php_print_backtrace(cid)`.
    /// # Errors
    /// Its output bailed out.
    pub print_backtrace: fn(c_long) -> Result<(), Bailout>,
    /// `PHPCoroutine::get_hook_flags()` and `set_hook_flags(flags)` (which
    /// records them in the coroutine options too), for `Swoole\Runtime`.
    pub hook_flags: fn() -> u32,
    pub set_hook_flags: fn(u32),
    /// `sw_active_thread_count()`: the PHP threads running.
    pub active_threads: fn() -> usize,
    /// `swoole_server_ce`, for `Swoole\Process` (crates/phpprocess).
    pub server_class: fn() -> *mut zend_class_entry,
    /// crates/phpclients' MINITs, in order: `php_swoole_socket_coro_minit`,
    /// `_client_minit`, `_client_async_minit`, `_client_coro_minit` and
    /// `_http_client_coro_minit`.
    pub client_minit: [Minit; 5],
    /// `php_swoole_name_resolver_minit`.
    pub name_resolver_minit: Minit,
    /// `php_swoole_socket_set(socket, settings)` over a coroutine socket, for
    /// crates/phpruntime; `which` 2 applies only the SSL ones
    /// (`php_swoole_socket_set_ssl`). Raises a bailout it caught.
    pub socket_settings: unsafe extern "C" fn(*mut c_void, *const zval, c_int) -> bool,
    /// `php_swoole_name_resolver_add(resolver)` and
    /// `swoole_name_resolver_php_remove(resolver)`, for crates/phpcoroutines.
    pub name_resolver_add: unsafe fn(*mut zval) -> bool,
    pub name_resolver_remove: unsafe fn(*mut zval) -> *mut zval,
    /// `swoole_socket_coro_ce` and `swoole_client_ce`, for `Swoole\Event`
    /// (crates/phpevents).
    pub socket_class: fn() -> *mut zend_class_entry,
    pub client_class: fn() -> *mut zend_class_entry,
}

static FAMILIES: OnceLock<&'static Families> = OnceLock::new();

/// Keep `families` for the phases; the first registration stands.
pub(crate) fn register(families: &'static Families) {
    FAMILIES.get_or_init(|| families);
}

/// The registered families: always some once Zend holds the module entry.
#[must_use]
pub fn families() -> Option<&'static Families> {
    FAMILIES.get().copied()
}

unsafe extern "C" {
    fn php_swoole_event_minit(module_number: c_int);
    fn php_swoole_atomic_minit(module_number: c_int);
    fn php_swoole_lock_minit(module_number: c_int);
    fn php_swoole_process_minit(module_number: c_int);
    fn php_swoole_process_pool_minit(module_number: c_int);
    fn php_swoole_table_minit(module_number: c_int);
    fn php_swoole_timer_minit(module_number: c_int);
    fn php_swoole_coroutine_lock_minit(module_number: c_int);
    fn php_swoole_channel_coro_minit(module_number: c_int);
    fn php_swoole_runtime_minit(module_number: c_int);
    /// crates/phphttp2's `Coroutine\Http2\Client`, `Http2\Request` and `Http2\Response`.
    fn swr_v1_phphttp2_client_minit(
        exception: *mut zend_class_entry,
        shortname: bool,
        module: c_int,
    );
    /// crates/phphttp's `Swoole\Http\Request`.
    fn swr_v1_phphttp_request_minit() -> *mut zend_class_entry;
    /// crates/phphttp's `Swoole\Http\Response`, and the codecs its parser knows.
    fn swr_v1_phphttp_response_minit(codecs: u32);
    fn swr_v1_phphttp_response_rshutdown();
    fn php_swoole_http_cookie_minit(module_number: c_int);
    /// crates/phphttp's `Swoole\Http\Server`, and its request state.
    fn swr_v1_phphttp_server_minit();
    fn swr_v1_phphttp_server_rinit();
    fn swr_v1_phphttp_server_rshutdown();
    /// crates/phphttp's `Swoole\Coroutine\Http\Server`.
    fn swr_v1_phphttp_server_coro_minit(
        exception: *mut zend_class_entry,
        short_name: bool,
        compression: bool,
    ) -> *mut zend_class_entry;
    /// crates/phphttp's `Swoole\WebSocket\Server`, `Frame` and `CloseFrame`.
    fn swr_v1_phphttp_websocket_minit(module_number: c_int, serializable: bool, zlib: bool);
    fn php_swoole_redis_server_minit(module_number: c_int);
    fn php_swoole_runtime_mshutdown();
    fn php_swoole_runtime_rinit();
    fn php_swoole_redis_server_rshutdown();
    fn php_swoole_timer_rshutdown();
    fn php_swoole_runtime_rshutdown();
    fn php_swoole_process_rshutdown();
}

/// One step of a phase: a family's MINIT hook, or any other hook.
#[derive(Clone, Copy)]
pub(crate) enum Step {
    Minit(Minit),
    Hook(Hook),
}

/// Run `steps` in order, each inside `zend_try`, stopping at a bailout.
fn run(steps: &[Step], module_number: c_int) -> Result<(), Bailout> {
    for step in steps {
        // C and C++ hooks that Zend may run at this phase; the shim's setjmp
        // sits between this frame and the hook.
        let bailed = match *step {
            // SAFETY: as above.
            Step::Minit(hook) => unsafe { swr_zend_try_minit(Some(hook), module_number) },
            // SAFETY: as above.
            Step::Hook(hook) => unsafe { swr_zend_try_hook(Some(hook)) },
        };
        if bailed != 0 {
            return Err(Bailout);
        }
    }
    Ok(())
}

/// MINIT after the constants and classes: Swoole's core, then each family
/// "sorted by dependency", then the hooks Swoole's core calls back. A bailout
/// is deferred for `swr_v1_phpswoole_minit` to raise.
pub(crate) fn startup(module_number: c_int) {
    use Step::{Hook as H, Minit as M};
    let Some(families) = families() else {
        return;
    };
    let mut steps = vec![
        H(core_startup),
        M(php_swoole_event_minit),
        // base
        M(php_swoole_atomic_minit),
        M(php_swoole_lock_minit),
        M(php_swoole_process_minit),
        M(php_swoole_process_pool_minit),
        M(php_swoole_table_minit),
        M(php_swoole_timer_minit),
    ];
    // coroutine
    steps.extend(families.coroutine_minit.map(M));
    steps.extend([
        M(php_swoole_coroutine_lock_minit),
        M(php_swoole_channel_coro_minit),
        M(php_swoole_runtime_minit),
    ]);
    // client
    steps.extend(families.client_minit.map(M));
    steps.extend([M(http2_client_coro_minit)]);
    // The bundled C clients, ssh2 and ftp.
    steps.extend(optional::client_minit());
    steps.extend([
        // server
        M(families.server_minit),
        M(http_request_minit),
        M(http_response_minit),
        M(php_swoole_http_cookie_minit),
        M(http_server_minit),
        M(http_server_coro_minit),
        M(websocket_server_minit),
        M(php_swoole_redis_server_minit),
        M(families.name_resolver_minit),
    ]);
    steps.extend(optional::minit());
    steps.extend([M(tracer_minit), H(php_hooks), H(ready)]);
    bailout::defer(run(&steps, module_number));
}

/// `php_swoole_tracer_minit`: blocking detection and the profiler need the
/// fiber mock.
extern "C" fn tracer_minit(_module_number: c_int) {
    let g = globals();
    let (blocking, profile) = (g.blocking_detection.get(), g.profile.get());
    let (threshold, leaks) = (g.blocking_threshold.get(), g.leak_detection.get());
    if let Some(families) = families() {
        // SAFETY: Rust's MINIT, once.
        unsafe { (families.tracer_minit)(threshold, blocking, profile, leaks) };
    }
    if blocking || profile {
        g.enable_fiber_mock.set(true);
    }
}

/// `php_swoole_http_request_minit`.
extern "C" fn http_request_minit(_module_number: c_int) {
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phphttp_request_minit() };
}

/// The codecs the build has: 1 zlib, 2 brotli, 4 zstd.
#[must_use]
pub fn codecs() -> u32 {
    [Switch::Zlib, Switch::Brotli, Switch::Zstd]
        .iter()
        .enumerate()
        .filter(|(_, capability)| crate::built_with(**capability))
        .fold(0, |features, (bit, _)| features | 1 << bit)
}

/// `php_swoole_http_response_minit`: the codecs and `SW_HAVE_COMPRESSION`.
extern "C" fn http_response_minit(_module_number: c_int) {
    let flags = crate::FLAGS.load(std::sync::atomic::Ordering::Relaxed);
    let codecs = codecs() | u32::from(flags & crate::constants::COMPRESSION != 0) << 3;
    // SAFETY: Rust's MINIT, once, after the request class's.
    unsafe { swr_v1_phphttp_response_minit(codecs) };
}

/// `php_swoole_websocket_server_minit`: serializable with `SW_THREAD`.
extern "C" fn websocket_server_minit(module_number: c_int) {
    use crate::constants::{THREAD, ZLIB};
    let flags = crate::FLAGS.load(std::sync::atomic::Ordering::Relaxed);
    let (serializable, zlib) = (flags & THREAD != 0, flags & ZLIB != 0);
    // SAFETY: Rust's MINIT, once, after `Swoole\Http\Server`'s.
    unsafe { swr_v1_phphttp_websocket_minit(module_number, serializable, zlib) };
}

/// `php_swoole_http_server_minit`.
extern "C" fn http_server_minit(_module_number: c_int) {
    // SAFETY: Rust's MINIT, once, after `Swoole\Server`'s.
    unsafe { swr_v1_phphttp_server_minit() };
}

/// `php_swoole_http_server_coro_minit`.
extern "C" fn http_server_coro_minit(_module_number: c_int) {
    let flags = crate::FLAGS.load(std::sync::atomic::Ordering::Relaxed);
    let exception = crate::EXCEPTION.entry();
    let shortname = globals().use_shortname.get();
    // SAFETY: Rust's MINIT, once.
    unsafe {
        swr_v1_phphttp_server_coro_minit(
            exception,
            shortname,
            flags & crate::constants::COMPRESSION != 0,
        )
    };
}

/// `php_swoole_http2_client_coro_minit`.
extern "C" fn http2_client_coro_minit(module_number: c_int) {
    let exception = crate::EXCEPTION.entry();
    let shortname = globals().use_shortname.get();
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phphttp2_client_minit(exception, shortname, module_number) };
}

/// MINIT's end: Swoole's fatal errors become `Swoole\Error`s and its
/// backtraces PHP's (`SwooleG.fatal_error` and `SwooleG.print_backtrace`).
extern "C" fn php_hooks() {
    swr_v1_core_set_fatal_error(swr_zend_fatal_error as *const () as *mut c_void);
    swr_v1_core_set_print_backtrace(swr_zend_print_stack_trace as *const () as *mut c_void);
}

unsafe extern "C" {
    /// `void swoole_init()`, crates/ffi's.
    #[link_name = "_Z11swoole_initv"]
    safe fn swoole_init();
    /// native/phphelpers/host.c.
    safe fn swr_phpswoole_pcre_jit_off();
    fn php_swoole_load_library();
}

/// MINIT's first step: Swoole's core, its after-fork hook, and the bug
/// report's PHP version.
extern "C" fn core_startup() {
    /// `SW_GLOBAL_HOOK_AFTER_FORK`.
    const AFTER_FORK: c_int = 12;
    swoole_init();
    swoole_rs_corebase::swoole_add_hook(AFTER_FORK, swr_zend_after_fork, 0);
    // SAFETY: no preconditions.
    let version = unsafe { swr_zend_php_version() };
    // SAFETY: PHP_VERSION, a static string.
    let version = unsafe { CStr::from_ptr(version) }.to_bytes();
    swoole_rs_corebase::bug_report_append(&[b"PHP_VERSION : ", version, b"\n"].concat());
}

/// MINIT's end: the ini's socket buffer size, macOS's `pcre.jit` and the known
/// strings.
extern "C" fn ready() {
    let size = globals().socket_buffer_size.get();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream assigns the zend_long to a uint32_t"
    )]
    // SAFETY: MINIT, before any thread reads the process-wide defaults.
    unsafe {
        swoole_rs_reactor::swr_v1_socket_defaults.buffer_size = size as u32;
    }
    swr_phpswoole_pcre_jit_off();
    swoole_rs_phphelpers::known_init();
}

/// `php_swoole_load_library()`, the bundled PHP library (host.c), inside `zend_try`.
/// # Errors
/// The library's code bailed out.
pub fn load_library() -> Result<(), Bailout> {
    // SAFETY: the loader, a zend_try body.
    if unsafe { swr_zend_try_hook(Some(php_swoole_load_library)) } == 0 {
        Ok(())
    } else {
        Err(Bailout)
    }
}

/// `swoole_globals`' defaults (`php_swoole_init_globals`), before the ini
/// entries apply.
/// # Safety
/// Zend's `ZEND_INIT_MODULE_GLOBALS` constructor, with this thread's globals.
unsafe extern "C" fn init_globals(globals: *mut c_void) {
    // SAFETY: per the caller, and nothing else reads them yet.
    let globals = unsafe { &*globals.cast::<Globals>() };
    globals.enable_library.set(true);
    globals.enable_fiber_mock.set(false);
    globals.enable_preemptive_scheduler.set(false);
    globals.socket_buffer_size.set(SOCKET_BUFFER_SIZE);
    globals.display_errors.set(true);
    globals.use_shortname.set(true);
    globals.in_autoload.set(ptr::null_mut());
    globals.blocking_detection.set(false);
    globals.blocking_threshold.set(100_000);
    globals.profile.set(false);
    globals.leak_detection.set(false);
    // SAFETY: the SAPI's name, set before any module starts.
    let sapi = unsafe { swr_zend_sapi_name() };
    // SAFETY: as above, a NUL-terminated string.
    let sapi = unsafe { CStr::from_ptr(sapi) };
    if [c"cli", c"phpdbg", c"embed", c"micro"].contains(&sapi) {
        globals.cli.set(true);
    }
}

unsafe extern "C" {
    /// `int swoole_event_free()`.
    #[link_name = "_Z17swoole_event_freev"]
    fn swoole_event_free() -> c_int;
}

/// MSHUTDOWN's `swoole_clean()`.
extern "C" fn clean() {
    swoole_rs_corebase::swoole_clean();
}

/// RSHUTDOWN's `swoole_event_free()`.
extern "C" fn event_free() {
    // SAFETY: the request's end, on the thread that owns the event loop.
    unsafe { swoole_event_free() };
}

/// `PHP_MSHUTDOWN_FUNCTION(swoole)`.
/// # Safety
/// Zend's MSHUTDOWN.
pub unsafe extern "C" fn swr_v1_phpswoole_mshutdown(_kind: c_int, module_number: c_int) -> c_int {
    let mut steps = vec![
        Step::Hook(swoole_rs_phphelpers::known_free),
        Step::Hook(php_swoole_runtime_mshutdown),
    ];
    steps.extend(optional::mshutdown());
    steps.extend(optional::client_mshutdown());
    steps.extend([
        // The tables' process-private parts, which upstream leaves to the exit.
        Step::Hook(swoole_rs_memory::table::release),
        Step::Hook(clean),
        Step::Hook(swr_zend_leak_check),
    ]);
    bailout::raise(run(&steps, module_number));
    SUCCESS
}

/// `PHP_RINIT_FUNCTION(swoole)`.
/// # Safety
/// Zend's RINIT.
pub unsafe extern "C" fn swr_v1_phpswoole_rinit(_kind: c_int, module_number: c_int) -> c_int {
    let globals = globals();
    if !globals.cli.get() {
        return SUCCESS;
    }
    globals.req_status.set(RINIT_BEGIN);
    // Task files go to `sys_get_temp_dir()`: `/tmp` does not exist everywhere
    // (Android, termux).
    // SAFETY: PHP's directory, which lives for the request.
    let directory = unsafe { swr_zend_temporary_directory() };
    // SAFETY: a NUL-terminated path, which the setter copies.
    unsafe { swoole_rs_corebase::swr_v1_core_set_task_tmpdir(directory) };
    php_swoole_register_shutdown_function(c"swoole_internal_call_user_shutdown_begin");
    // SAFETY: this request's compiler globals.
    if globals.enable_library.get() && unsafe { swr_zend_compiling_preload() } == 0 {
        // xdebug hooks zend_execute_ex, under which the library does not load
        // correctly (swoole/swoole-src#5182): load it under execute_ex.
        // SAFETY: RINIT, before any PHP code runs.
        let hook = unsafe { swr_zend_execute_ex_reset() };
        if let Err(bailed) = load_library() {
            bailout::raise(Err(bailed));
            return SUCCESS;
        }
        // SAFETY: the hook replaced above.
        unsafe { swr_zend_execute_ex_restore(hook) };
    }
    // SAFETY: RINIT.
    unsafe { swr_zend_signals_uncheck() };
    let Some(families) = families() else {
        return SUCCESS;
    };
    let mut steps = vec![
        Step::Hook(swr_v1_phphttp_server_rinit),
        Step::Hook(families.coroutine_rinit),
        Step::Hook(php_swoole_runtime_rinit),
    ];
    steps.extend(optional::rinit(families));
    steps.push(Step::Hook(families.tracer_rinit));
    if let Err(bailed) = run(&steps, module_number) {
        bailout::raise(Err(bailed));
        return SUCCESS;
    }
    globals.req_status.set(RINIT_END);
    SUCCESS
}

/// `PHP_RSHUTDOWN_FUNCTION(swoole)`.
/// # Safety
/// Zend's RSHUTDOWN.
pub unsafe extern "C" fn swr_v1_phpswoole_rshutdown(_kind: c_int, module_number: c_int) -> c_int {
    let globals = globals();
    if !globals.cli.get() {
        return SUCCESS;
    }
    globals.req_status.set(RSHUTDOWN_BEGIN);
    let Some(families) = families() else {
        return SUCCESS;
    };
    let mut steps = vec![
        Step::Hook(families.server_rshutdown),
        Step::Hook(swr_v1_phphttp_server_rshutdown),
        Step::Hook(swr_v1_phphttp_response_rshutdown),
        Step::Hook(crate::asynchronous::rshutdown),
        Step::Hook(php_swoole_redis_server_rshutdown),
        Step::Hook(families.coroutine_rshutdown[0]),
        Step::Hook(families.coroutine_rshutdown[1]),
        Step::Hook(php_swoole_timer_rshutdown),
        Step::Hook(php_swoole_runtime_rshutdown),
        Step::Hook(php_swoole_process_rshutdown),
    ];
    steps.extend(optional::rshutdown(families));
    steps.extend([
        Step::Hook(families.tracer_rshutdown),
        Step::Hook(event_free),
    ]);
    if let Err(bailed) = run(&steps, module_number) {
        bailout::raise(Err(bailed));
        return SUCCESS;
    }
    globals.req_status.set(RSHUTDOWN_END);
    // Do not close stdout and stderr.
    for name in [c"STDOUT", c"STDERR"] {
        // SAFETY: a constant's name; the shim checks the resource's type.
        unsafe { swr_zend_stream_constant_no_close(name.as_ptr(), name.count_bytes()) };
    }
    SUCCESS
}

/// `php_swoole_register_shutdown_function(function)`, during a request, with
/// the name of a registered global function.
pub fn php_swoole_register_shutdown_function(function: &CStr) {
    // SAFETY: Zend copies the name into the request's shutdown functions.
    unsafe { swr_zend_register_shutdown_function(function.as_ptr()) };
}

/// `swoole_set_process_name(name)`: `cli_set_process_title`, which only the
/// CLI SAPI has.
/// # Safety
/// Called by Zend with a live frame and return slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zif_swoole_set_process_name(
    frame: *mut zend_execute_data,
    value: *mut zval,
) {
    let name = c"cli_set_process_title";
    // SAFETY: this request's function table.
    let function = unsafe { swr_zend_function_find(name.as_ptr(), name.count_bytes()) };
    if function.is_null() {
        set_last_error(PHP_FATAL_ERROR);
        // SAFETY: an error from Rust, through the shim.
        bailout::defer(unsafe {
            php_error(
                E_WARNING,
                c"swoole_set_process_name only support in CLI mode",
            )
        });
        // SAFETY: Zend's frame and return slot.
        unsafe { Call::new(frame, value) }.set_bool(false);
    } else {
        // SAFETY: a found function.
        let function = unsafe { &*function };
        // SAFETY: an internal function (the CLI SAPI's).
        let handler = unsafe { function.internal_function.handler };
        // SAFETY: its handler, with this call's frame and return slot; the
        // shim traps a bailout.
        if unsafe { swr_zend_runtime_call_handler(handler, frame, value) } != 0 {
            Bailout.defer();
        }
    }
    bailout::finish();
}

#[cfg(test)]
mod tests {
    use super::{CALL_USER_SHUTDOWNFUNC_BEGIN, Globals, RINIT_END, RSHUTDOWN_END};
    use std::mem::{offset_of, size_of};

    #[test]
    fn globals_match_zend_swoole_globals() {
        // php_swoole.h's zend_swoole_globals, which C++ reads through SWOOLE_G.
        assert_eq!(offset_of!(Globals, blocking_threshold), 8);
        assert_eq!(offset_of!(Globals, socket_buffer_size), 24);
        assert_eq!(offset_of!(Globals, req_status), 32);
        assert_eq!(offset_of!(Globals, in_autoload), 40);
        assert_eq!(size_of::<Globals>(), 48);
        assert_eq!(
            (RINIT_END, CALL_USER_SHUTDOWNFUNC_BEGIN, RSHUTDOWN_END),
            (1, 2, 4)
        );
    }
}
