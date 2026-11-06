//! The module's lifecycle, in upstream's order: the rest of MINIT after the
//! constants and classes, and MSHUTDOWN, RINIT and RSHUTDOWN
//! (`swr_v1_phpswoole_mshutdown`, `_rinit`, `_rshutdown`). Each
//! family's hook is C (`php_swoole_private.h` declares them `extern "C"`) and
//! runs inside `zend_try`: a bailout stops the phase there, as it did
//! upstream, and is raised once this frame is the only Rust one left.
use crate::{Capability, ops};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
};
use swoole_rs_corebase::{swr_v1_core_set_fatal_error, swr_v1_core_set_print_backtrace};
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING,
    bailout::{self, Bailout},
    php_error,
    string::bytes,
    sys::{
        HashTable, swr_zend_compiling_preload, swr_zend_execute_ex_reset,
        swr_zend_execute_ex_restore, swr_zend_fatal_error, swr_zend_function_find,
        swr_zend_print_stack_trace, swr_zend_register_shutdown_function,
        swr_zend_runtime_call_handler, swr_zend_sapi_name, swr_zend_signals_uncheck,
        swr_zend_stream_constant_no_close, swr_zend_string_release, swr_zend_temporary_directory,
        swr_zend_try_hook, swr_zend_try_minit, zend_class_entry, zend_execute_data, zend_string,
        zval,
    },
    value,
};

/// `zend_swoole_globals` (`php_swoole.h`); `php_swoole_cxx.cc` asserts the layout.
#[repr(C)]
pub struct Globals {
    pub(crate) display_errors: bool,
    cli: bool,
    pub(crate) use_shortname: bool,
    enable_preemptive_scheduler: bool,
    enable_library: bool,
    enable_fiber_mock: bool,
    blocking_detection: bool,
    blocking_threshold: i64,
    profile: bool,
    leak_detection: bool,
    socket_buffer_size: i64,
    pub(crate) req_status: c_int,
    in_autoload: *mut HashTable,
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
pub(crate) const CALL_USER_SHUTDOWNFUNC_BEGIN: c_int = 2;
const RSHUTDOWN_BEGIN: c_int = 3;
const RSHUTDOWN_END: c_int = 4;
/// `SW_SOCKET_BUFFER_SIZE` and `SW_ERROR_PHP_FATAL_ERROR`.
const SOCKET_BUFFER_SIZE: i64 = 8 * 1024 * 1024;
pub(crate) const PHP_FATAL_ERROR: i32 = 503;
/// `SUCCESS`.
const SUCCESS: c_int = 0;

/// This thread's module globals, which C++ reads and writes too: access them
/// through the pointer, one field at a time.
pub(crate) fn globals() -> *mut Globals {
    // SAFETY: C++'s getter.
    unsafe { (ops().globals)() }
}

/// `SWOOLE_G(display_errors)`.
#[must_use]
pub fn display_errors() -> bool {
    // SAFETY: the module's globals, live from MINIT.
    unsafe { (*globals()).display_errors }
}

/// `SWOOLE_G(enable_library)`.
#[must_use]
pub fn enable_library() -> bool {
    // SAFETY: as above.
    unsafe { (*globals()).enable_library }
}

type Minit = unsafe extern "C" fn(c_int);
type Hook = unsafe extern "C" fn();

unsafe extern "C" {
    fn php_swoole_event_minit(module_number: c_int);
    fn php_swoole_atomic_minit(module_number: c_int);
    fn php_swoole_lock_minit(module_number: c_int);
    fn php_swoole_process_minit(module_number: c_int);
    fn php_swoole_process_pool_minit(module_number: c_int);
    fn php_swoole_table_minit(module_number: c_int);
    fn php_swoole_timer_minit(module_number: c_int);
    fn php_swoole_coroutine_minit(module_number: c_int);
    fn php_swoole_coroutine_system_minit(module_number: c_int);
    fn php_swoole_coroutine_scheduler_minit(module_number: c_int);
    fn php_swoole_coroutine_lock_minit(module_number: c_int);
    fn php_swoole_channel_coro_minit(module_number: c_int);
    fn php_swoole_runtime_minit(module_number: c_int);
    /// crates/phpclients' `Coroutine\Socket`.
    fn swr_v1_phpclients_socket_minit(
        exception: *mut zend_class_entry,
        shortname: bool,
        module: c_int,
    );
    fn php_swoole_client_minit(module_number: c_int);
    fn php_swoole_client_async_minit(module_number: c_int);
    /// crates/phpclients' `Coroutine\Client`.
    fn swr_v1_phpclients_client_minit(shortname: bool, client_class: *mut *mut zend_class_entry);
    /// crates/phpclients' `Coroutine\Http\Client`.
    fn swr_v1_phpclients_http_client_minit(
        exception: *mut zend_class_entry,
        shortname: bool,
        module: c_int,
        features: u32,
    );
    /// crates/phphttp2's `Coroutine\Http2\Client`, `Http2\Request` and `Http2\Response`.
    fn swr_v1_phphttp2_client_minit(
        exception: *mut zend_class_entry,
        shortname: bool,
        module: c_int,
    );
    fn php_swoole_server_minit(module_number: c_int);
    /// crates/phphttp's `Swoole\Http\Request`.
    fn swr_v1_phphttp_request_minit() -> *mut zend_class_entry;
    /// crates/phphttp's `Swoole\Http\Response`, and the codecs its parser knows.
    fn swr_v1_phphttp_response_minit(codecs: u32);
    fn swr_v1_phphttp_response_rshutdown();
    fn php_swoole_http_cookie_minit(module_number: c_int);
    fn php_swoole_http_server_minit(module_number: c_int);
    fn php_swoole_http_server_coro_minit(module_number: c_int);
    fn php_swoole_websocket_server_minit(module_number: c_int);
    fn php_swoole_redis_server_minit(module_number: c_int);
    fn php_swoole_name_resolver_minit(module_number: c_int);
    /// crates/phpcoroutines' tracer, with its INI switches.
    fn swr_v1_phpcoroutines_tracer_minit(
        blocking_threshold: i64,
        blocking_detection: bool,
        profile: bool,
        leak_detection: bool,
    );

    fn php_swoole_runtime_mshutdown();

    fn php_swoole_http_server_rinit();
    fn php_swoole_coroutine_rinit();
    fn php_swoole_runtime_rinit();
    fn swr_v1_phpcoroutines_tracer_rinit();

    fn php_swoole_server_rshutdown();
    fn php_swoole_http_server_rshutdown();
    fn php_swoole_redis_server_rshutdown();
    fn php_swoole_coroutine_rshutdown();
    fn php_swoole_coroutine_scheduler_rshutdown();
    fn php_swoole_timer_rshutdown();
    fn php_swoole_runtime_rshutdown();
    fn php_swoole_process_rshutdown();
    fn swr_v1_phpcoroutines_tracer_rshutdown();
}

/// One step of a phase: a family's MINIT hook, or any other hook.
#[derive(Clone, Copy)]
enum Step {
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
    let ops = ops();
    let steps = [
        H(ops.startup),
        M(php_swoole_event_minit),
        // base
        M(php_swoole_atomic_minit),
        M(php_swoole_lock_minit),
        M(php_swoole_process_minit),
        M(php_swoole_process_pool_minit),
        M(php_swoole_table_minit),
        M(php_swoole_timer_minit),
        // coroutine
        M(php_swoole_coroutine_minit),
        M(php_swoole_coroutine_system_minit),
        M(php_swoole_coroutine_scheduler_minit),
        M(php_swoole_coroutine_lock_minit),
        M(php_swoole_channel_coro_minit),
        M(php_swoole_runtime_minit),
        // client
        M(socket_coro_minit),
        M(php_swoole_client_minit),
        M(php_swoole_client_async_minit),
        M(client_coro_minit),
        M(http_client_coro_minit),
        M(http2_client_coro_minit),
        M(ops.optional_client_minit),
        // server
        M(php_swoole_server_minit),
        M(http_request_minit),
        M(http_response_minit),
        M(php_swoole_http_cookie_minit),
        M(php_swoole_http_server_minit),
        M(php_swoole_http_server_coro_minit),
        M(php_swoole_websocket_server_minit),
        M(php_swoole_redis_server_minit),
        M(php_swoole_name_resolver_minit),
        M(ops.optional_minit),
        M(tracer_minit),
        H(php_hooks),
        H(ops.ready),
    ];
    bailout::defer(run(&steps, module_number));
}

/// `php_swoole_tracer_minit`: blocking detection and the profiler need the
/// fiber mock.
unsafe extern "C" fn tracer_minit(_module_number: c_int) {
    let globals = globals();
    // SAFETY: the module's globals, live from MINIT.
    let (threshold, blocking, profile, leaks) = unsafe {
        let g = &*globals;
        (
            g.blocking_threshold,
            g.blocking_detection,
            g.profile,
            g.leak_detection,
        )
    };
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phpcoroutines_tracer_minit(threshold, blocking, profile, leaks) };
    if blocking || profile {
        // SAFETY: as above.
        unsafe { (*globals).enable_fiber_mock = true };
    }
}

/// `php_swoole_http_request_minit`.
unsafe extern "C" fn http_request_minit(_module_number: c_int) {
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phphttp_request_minit() };
}

/// `php_swoole_socket_coro_minit`.
unsafe extern "C" fn socket_coro_minit(module_number: c_int) {
    // SAFETY: MINIT registered `Swoole\Exception` first and nothing writes it now.
    let exception = unsafe { crate::swoole_exception_ce };
    // SAFETY: the module's globals, live from MINIT.
    let shortname = unsafe { (*globals()).use_shortname };
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phpclients_socket_minit(exception, shortname, module_number) };
}

/// `php_swoole_http_response_minit`: zlib and `SW_HAVE_COMPRESSION` are the
/// codecs built in (brotli and zstd never are: `tools/build_upstream.py`).
unsafe extern "C" fn http_response_minit(_module_number: c_int) {
    use crate::constants::{COMPRESSION, ZLIB};
    let flags = crate::FLAGS.load(std::sync::atomic::Ordering::Relaxed);
    let codecs = u32::from(flags & ZLIB != 0) | u32::from(flags & COMPRESSION != 0) << 3;
    // SAFETY: Rust's MINIT, once, after the request class's.
    unsafe { swr_v1_phphttp_response_minit(codecs) };
}

/// `php_swoole_client_coro_minit`.
unsafe extern "C" fn client_coro_minit(_module_number: c_int) {
    // SAFETY: the module's globals, live from MINIT.
    let shortname = unsafe { (*globals()).use_shortname };
    let mut class = ptr::null_mut();
    // SAFETY: Rust's MINIT, once, with a writable slot.
    unsafe { swr_v1_phpclients_client_minit(shortname, &raw mut class) };
}

/// `php_swoole_http_client_coro_minit`: the compression the build has.
unsafe extern "C" fn http_client_coro_minit(module_number: c_int) {
    // SAFETY: MINIT registered `Swoole\Exception` first and nothing writes it now.
    let exception = unsafe { crate::swoole_exception_ce };
    // SAFETY: the module's globals, live from MINIT.
    let shortname = unsafe { (*globals()).use_shortname };
    let features = [Capability::Zlib, Capability::Brotli, Capability::Zstd]
        .iter()
        .enumerate()
        .filter(|(_, capability)| crate::built_with(**capability))
        .fold(0, |features, (bit, _)| features | 1 << bit);
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phpclients_http_client_minit(exception, shortname, module_number, features) };
}

/// `php_swoole_http2_client_coro_minit`.
unsafe extern "C" fn http2_client_coro_minit(module_number: c_int) {
    // SAFETY: MINIT registered `Swoole\Exception` first and nothing writes it now.
    let exception = unsafe { crate::swoole_exception_ce };
    // SAFETY: the module's globals, live from MINIT.
    let shortname = unsafe { (*globals()).use_shortname };
    // SAFETY: Rust's MINIT, once.
    unsafe { swr_v1_phphttp2_client_minit(exception, shortname, module_number) };
}

/// MINIT's end: Swoole's fatal errors become `Swoole\Error`s and its
/// backtraces PHP's (`SwooleG.fatal_error` and `SwooleG.print_backtrace`).
unsafe extern "C" fn php_hooks() {
    swr_v1_core_set_fatal_error(swr_zend_fatal_error as *const () as *mut c_void);
    swr_v1_core_set_print_backtrace(swr_zend_print_stack_trace as *const () as *mut c_void);
}

/// `swoole_globals`' defaults (`php_swoole_init_globals`), before the ini
/// entries apply.
/// # Safety
/// Zend's `ZEND_INIT_MODULE_GLOBALS` constructor, with this thread's globals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_init_globals(globals: *mut Globals) {
    // SAFETY: per the caller, and nothing else reads them yet.
    let globals = unsafe { &mut *globals };
    globals.enable_library = true;
    globals.enable_fiber_mock = false;
    globals.enable_preemptive_scheduler = false;
    globals.socket_buffer_size = SOCKET_BUFFER_SIZE;
    globals.display_errors = true;
    globals.use_shortname = true;
    globals.in_autoload = ptr::null_mut();
    globals.blocking_detection = false;
    globals.blocking_threshold = 100_000;
    globals.profile = false;
    globals.leak_detection = false;
    // SAFETY: the SAPI's name, set before any module starts.
    let sapi = unsafe { swr_zend_sapi_name() };
    // SAFETY: as above, a NUL-terminated string.
    let sapi = unsafe { CStr::from_ptr(sapi) };
    if [c"cli", c"phpdbg", c"embed", c"micro"].contains(&sapi) {
        globals.cli = true;
    }
}

/// `PHP_MSHUTDOWN_FUNCTION(swoole)`.
/// # Safety
/// Zend's MSHUTDOWN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_mshutdown(_kind: c_int, module_number: c_int) -> c_int {
    let ops = ops();
    let steps = [
        Step::Hook(ops.known_strings_dtor),
        Step::Hook(php_swoole_runtime_mshutdown),
        Step::Hook(ops.optional_mshutdown),
        Step::Hook(ops.clean),
    ];
    bailout::raise(run(&steps, module_number));
    SUCCESS
}

/// `PHP_RINIT_FUNCTION(swoole)`.
/// # Safety
/// Zend's RINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_rinit(_kind: c_int, module_number: c_int) -> c_int {
    let globals = globals();
    // SAFETY: this thread's live globals (as for every access below).
    if !unsafe { (*globals).cli } {
        return SUCCESS;
    }
    // SAFETY: as above.
    unsafe { (*globals).req_status = RINIT_BEGIN };
    let ops = ops();
    // Task files go to `sys_get_temp_dir()`: `/tmp` does not exist everywhere
    // (Android, termux).
    // SAFETY: PHP's directory, which lives for the request.
    let directory = unsafe { swr_zend_temporary_directory() };
    // SAFETY: C++'s setter, which copies it.
    unsafe { (ops.set_task_tmpdir)(directory) };
    // SAFETY: RINIT, with a function this module registers.
    unsafe {
        php_swoole_register_shutdown_function(c"swoole_internal_call_user_shutdown_begin".as_ptr());
    }
    // SAFETY: as above, and this request's compiler globals.
    if unsafe { (*globals).enable_library } && unsafe { swr_zend_compiling_preload() } == 0 {
        // xdebug hooks zend_execute_ex, under which the library does not load
        // correctly (swoole/swoole-src#5182): load it under execute_ex.
        // SAFETY: RINIT, before any PHP code runs.
        let hook = unsafe { swr_zend_execute_ex_reset() };
        // SAFETY: C++'s loader, a zend_try body.
        if let Err(bailed) = unsafe { bailout::trap(ops.load_library, ptr::null_mut()) } {
            bailout::raise(Err(bailed));
            return SUCCESS;
        }
        // SAFETY: the hook replaced above.
        unsafe { swr_zend_execute_ex_restore(hook) };
    }
    // SAFETY: RINIT.
    unsafe { swr_zend_signals_uncheck() };
    let steps = [
        Step::Hook(php_swoole_http_server_rinit),
        Step::Hook(php_swoole_coroutine_rinit),
        Step::Hook(php_swoole_runtime_rinit),
        Step::Hook(ops.optional_rinit),
        Step::Hook(swr_v1_phpcoroutines_tracer_rinit),
    ];
    if let Err(bailed) = run(&steps, module_number) {
        bailout::raise(Err(bailed));
        return SUCCESS;
    }
    // SAFETY: as above.
    unsafe { (*globals).req_status = RINIT_END };
    SUCCESS
}

/// `PHP_RSHUTDOWN_FUNCTION(swoole)`.
/// # Safety
/// Zend's RSHUTDOWN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_rshutdown(_kind: c_int, module_number: c_int) -> c_int {
    let globals = globals();
    // SAFETY: this thread's live globals (as for every access below).
    if !unsafe { (*globals).cli } {
        return SUCCESS;
    }
    // SAFETY: as above.
    unsafe { (*globals).req_status = RSHUTDOWN_BEGIN };
    let ops = ops();
    let steps = [
        Step::Hook(php_swoole_server_rshutdown),
        Step::Hook(php_swoole_http_server_rshutdown),
        Step::Hook(swr_v1_phphttp_response_rshutdown),
        Step::Hook(crate::asynchronous::rshutdown),
        Step::Hook(php_swoole_redis_server_rshutdown),
        Step::Hook(php_swoole_coroutine_rshutdown),
        Step::Hook(php_swoole_coroutine_scheduler_rshutdown),
        Step::Hook(php_swoole_timer_rshutdown),
        Step::Hook(php_swoole_runtime_rshutdown),
        Step::Hook(php_swoole_process_rshutdown),
        Step::Hook(ops.optional_rshutdown),
        Step::Hook(swr_v1_phpcoroutines_tracer_rshutdown),
        Step::Hook(ops.event_free),
    ];
    if let Err(bailed) = run(&steps, module_number) {
        bailout::raise(Err(bailed));
        return SUCCESS;
    }
    // SAFETY: as above.
    unsafe { (*globals).req_status = RSHUTDOWN_END };
    // Do not close stdout and stderr.
    for name in [c"STDOUT", c"STDERR"] {
        // SAFETY: a constant's name; the shim checks the resource's type.
        unsafe { swr_zend_stream_constant_no_close(name.as_ptr(), name.count_bytes()) };
    }
    SUCCESS
}

/// `php_swoole_register_shutdown_function(function)`.
/// # Safety
/// During a request, with the name of a registered global function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_register_shutdown_function(function: *const c_char) {
    // SAFETY: per the caller.
    unsafe { swr_zend_register_shutdown_function(function) };
}

/// `php_swoole_fatal_error(E_ERROR, "failed to parse '%s' to size, Error: %s")`
/// of the size string `failed` and Zend's `error`, which is released: a
/// bailout, deferred for the caller's [`bailout::finish`].
/// # Safety
/// `failed` and `error` are live strings.
pub(crate) unsafe fn size_error(failed: *const zend_string, error: *mut zend_string) {
    // "%s" stops at a NUL.
    let c_str = |text: &[u8]| text.split(|&b| b == 0).next().unwrap_or_default().to_vec();
    // SAFETY: per the caller.
    let failed = c_str(unsafe { bytes(failed) });
    // SAFETY: as above.
    let reason = c_str(unsafe { bytes(error) });
    // SAFETY: per the caller; Zend's error string, owned here.
    unsafe { swr_zend_string_release(error) };
    // SAFETY: C++'s setter.
    unsafe { (ops().set_last_error)(PHP_FATAL_ERROR) };
    let message = [
        b"failed to parse '",
        &failed[..],
        b"' to size, Error: ",
        &reason[..],
    ]
    .concat();
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: an error from Rust, through the shim's zend_try.
    bailout::defer(unsafe { php_error(E_ERROR, &message) });
}

/// `php_swoole_parse_to_size(zv)`: a size string as an ini quantity
/// ("64M"), anything else as `zval_get_long`. A string that does not parse
/// is a fatal error.
/// # Safety
/// `value` is a live zval, and the caller is C or C++ (a fatal error bails
/// out from here).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_parse_to_size(value: *mut zval) -> i64 {
    let size = match crate::options::size(value) {
        Ok(size) => size,
        Err(error) => {
            // SAFETY: only a string fails to parse.
            let failed = unsafe { value::str_of(value) };
            // SAFETY: a live string, and Zend's error string.
            unsafe { size_error(failed, error) };
            0
        }
    };
    bailout::finish();
    size
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
        // SAFETY: C++'s setter.
        unsafe { (ops().set_last_error)(PHP_FATAL_ERROR) };
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
        // php_swoole_cxx.cc asserts the same offsets on the C++ side.
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
