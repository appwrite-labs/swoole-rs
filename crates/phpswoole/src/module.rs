//! The module's lifecycle, in upstream's order: the rest of MINIT after the
//! constants and classes, and MSHUTDOWN, RINIT and RSHUTDOWN
//! (`swr_v1_phpswoole_mshutdown`, `_rinit`, `_rshutdown`). Each
//! family's hook is C (`php_swoole_private.h` declares them `extern "C"`) and
//! runs inside `zend_try`: a bailout stops the phase there, as it did
//! upstream, and is raised once this frame is the only Rust one left.
use crate::ops;
use std::{
    ffi::{CStr, CString, c_char, c_int},
    ptr,
};
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING,
    bailout::{self, Bailout},
    php_error,
    string::bytes,
    sys::{
        HashTable, swr_zend_compiling_preload, swr_zend_execute_ex_reset,
        swr_zend_execute_ex_restore, swr_zend_function_find, swr_zend_register_shutdown_function,
        swr_zend_runtime_call_handler, swr_zend_sapi_name, swr_zend_signals_uncheck,
        swr_zend_stream_constant_no_close, swr_zend_string_release, swr_zend_temporary_directory,
        swr_zend_try_hook, swr_zend_try_minit, zend_execute_data, zend_string, zval,
    },
    value,
};

/// `zend_swoole_globals` (`php_swoole.h`); `php_swoole.cc` asserts the layout.
#[repr(C)]
pub struct Globals {
    pub(crate) display_errors: bool,
    cli: bool,
    use_shortname: bool,
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

/// `php_swoole_req_status`.
const RINIT_BEGIN: c_int = 0;
pub(crate) const RINIT_END: c_int = 1;
pub(crate) const CALL_USER_SHUTDOWNFUNC_BEGIN: c_int = 2;
const RSHUTDOWN_BEGIN: c_int = 3;
const RSHUTDOWN_END: c_int = 4;
/// `SW_SOCKET_BUFFER_SIZE` and `SW_ERROR_PHP_FATAL_ERROR`.
const SOCKET_BUFFER_SIZE: i64 = 8 * 1024 * 1024;
const PHP_FATAL_ERROR: i32 = 503;
/// `SUCCESS`.
const SUCCESS: c_int = 0;

/// This thread's module globals, which C++ reads and writes too: access them
/// through the pointer, one field at a time.
pub(crate) fn globals() -> *mut Globals {
    // SAFETY: C++'s getter.
    unsafe { (ops().globals)() }
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
    fn php_swoole_socket_coro_minit(module_number: c_int);
    fn php_swoole_client_minit(module_number: c_int);
    fn php_swoole_client_async_minit(module_number: c_int);
    fn php_swoole_client_coro_minit(module_number: c_int);
    fn php_swoole_http_client_coro_minit(module_number: c_int);
    fn php_swoole_http2_client_coro_minit(module_number: c_int);
    fn php_swoole_server_minit(module_number: c_int);
    fn php_swoole_server_port_minit(module_number: c_int);
    fn php_swoole_http_request_minit(module_number: c_int);
    fn php_swoole_http_response_minit(module_number: c_int);
    fn php_swoole_http_cookie_minit(module_number: c_int);
    fn php_swoole_http_server_minit(module_number: c_int);
    fn php_swoole_http_server_coro_minit(module_number: c_int);
    fn php_swoole_websocket_server_minit(module_number: c_int);
    fn php_swoole_redis_server_minit(module_number: c_int);
    fn php_swoole_name_resolver_minit(module_number: c_int);
    fn php_swoole_tracer_minit(module_number: c_int);

    fn php_swoole_runtime_mshutdown();

    fn php_swoole_http_server_rinit();
    fn php_swoole_coroutine_rinit();
    fn php_swoole_runtime_rinit();
    fn php_swoole_tracer_rinit();

    fn php_swoole_server_rshutdown();
    fn php_swoole_http_server_rshutdown();
    fn php_swoole_http_response_rshutdown();
    fn php_swoole_async_coro_rshutdown();
    fn php_swoole_redis_server_rshutdown();
    fn php_swoole_coroutine_rshutdown();
    fn php_swoole_coroutine_scheduler_rshutdown();
    fn php_swoole_timer_rshutdown();
    fn php_swoole_runtime_rshutdown();
    fn php_swoole_process_rshutdown();
    fn php_swoole_tracer_rshutdown();
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
        M(php_swoole_socket_coro_minit),
        M(php_swoole_client_minit),
        M(php_swoole_client_async_minit),
        M(php_swoole_client_coro_minit),
        M(php_swoole_http_client_coro_minit),
        M(php_swoole_http2_client_coro_minit),
        M(ops.optional_client_minit),
        // server
        M(php_swoole_server_minit),
        M(php_swoole_server_port_minit),
        M(php_swoole_http_request_minit),
        M(php_swoole_http_response_minit),
        M(php_swoole_http_cookie_minit),
        M(php_swoole_http_server_minit),
        M(php_swoole_http_server_coro_minit),
        M(php_swoole_websocket_server_minit),
        M(php_swoole_redis_server_minit),
        M(php_swoole_name_resolver_minit),
        M(ops.optional_minit),
        M(php_swoole_tracer_minit),
        H(ops.ready),
    ];
    bailout::defer(run(&steps, module_number));
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
        Step::Hook(php_swoole_tracer_rinit),
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
        Step::Hook(php_swoole_http_response_rshutdown),
        Step::Hook(php_swoole_async_coro_rshutdown),
        Step::Hook(php_swoole_redis_server_rshutdown),
        Step::Hook(php_swoole_coroutine_rshutdown),
        Step::Hook(php_swoole_coroutine_scheduler_rshutdown),
        Step::Hook(php_swoole_timer_rshutdown),
        Step::Hook(php_swoole_runtime_rshutdown),
        Step::Hook(php_swoole_process_rshutdown),
        Step::Hook(ops.optional_rshutdown),
        Step::Hook(php_swoole_tracer_rshutdown),
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
        // php_swoole.cc asserts the same offsets on the C++ side.
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
