//! The function-table hooks: each hooked function's original handler and
//! arginfo, the handler swapped in (a C++ or Rust replacement, or the
//! library-call trampoline running `swoole_<name>` from the PHP library), the
//! functions disabled inside coroutines, and the classes grafted onto PHP's
//! (`Swoole\Coroutine\Socket` under `Socket`, `Swoole\Curl\Handler` under
//! `CurlHandle`). Edits happen on the main thread; other threads read the table
//! (their RINIT before PHP 8.3, and the trampoline).
use crate::{
    Bailout, SW_ERROR_UNDEFINED_BEHAVIOR, flags, in_coroutine, log_warning, message, ops, sleep,
    throw, transition,
};
use std::{
    collections::HashMap,
    ffi::{CStr, c_void},
    ptr,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};
use swoole_rs_zend::{
    Call, E_WARNING, bailout,
    sys::{
        ZEND_ACC_FAKE_CLOSURE, swr_zend_function_find, swr_zend_runtime_call_handler,
        swr_zend_runtime_copy_arginfo, swr_zend_runtime_disabled, swr_zend_runtime_free_arginfo,
        swr_zend_runtime_instanceof, swr_zend_runtime_parent, swr_zend_runtime_set_parent,
        swr_zend_runtime_unsupported, swr_zend_runtime_version, swr_zend_set_stringl,
        swr_zend_string_release, swr_zend_try_lookup_class, zend_class_entry, zend_execute_data,
        zend_fcall_info, zend_function, zend_internal_arg_info, zend_string, zif_handler, zval,
        zval_ptr_dtor,
    },
};

/// `ZEND_INTERNAL_FUNCTION`.
const INTERNAL_FUNCTION: u8 = 1;
/// `IS_STRING`.
const IS_STRING: u32 = 6;

/// `PhpFunc`: a hooked or disabled function.
struct Hooked {
    function: *mut zend_function,
    ori_handler: zif_handler,
    ori_arg_info: *mut zend_internal_arg_info,
    ori_fn_flags: u32,
    /// The arginfo copy installed with the handler, freed at RSHUTDOWN.
    arg_info_copy: *mut zend_internal_arg_info,
    /// The library replacement's `zend::Callable *`, and its name.
    callable: *mut c_void,
    name: zval,
}

#[derive(Default)]
struct Table {
    /// `hook_function_table`, between RINIT and RSHUTDOWN.
    hooked: Option<HashMap<Box<[u8]>, Hooked>>,
    /// `ori_func_handlers` and `ori_func_arg_infos`: each function's handler
    /// and arginfo before its first hook this request.
    originals: HashMap<Box<[u8]>, (zif_handler, *mut zend_internal_arg_info)>,
    /// `child_class_entries`: classes given a parent, by name.
    children: HashMap<Box<[u8]>, *mut zend_class_entry>,
}

// SAFETY: the pointers are Zend's function and class entries and Swoole's
// callables, which live for the request; the mutex orders every access.
unsafe impl Send for Table {}

static TABLE: LazyLock<Mutex<Table>> = LazyLock::new(Mutex::default);

/// The table, locked. Nothing that can run PHP code (and so bail out) runs
/// while it is held.
fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn same(a: zif_handler, b: zif_handler) -> bool {
    a.map(|f| f as usize) == b.map(|f| f as usize)
}

/// `ZSTR_VAL`/`ZSTR_LEN` of a live string.
fn bytes<'a>(string: *const zend_string) -> &'a [u8] {
    // SAFETY: Zend's live, interned function and class names.
    unsafe { swoole_rs_zend::string::bytes(string) }
}

/// `zend::get_function(name)`: the function table's entry, if any.
fn function(name: &[u8]) -> *mut zend_function {
    // SAFETY: a name and its length.
    unsafe { swr_zend_function_find(name.as_ptr().cast(), name.len()) }
}

/// The internal function of a function-table entry.
macro_rules! internal {
    ($function:expr) => {
        // SAFETY: an entry of the function table, which outlives the request;
        // every hooked function is internal.
        unsafe { &mut (*$function).internal_function }
    };
}

/// `hook_func(name, handler, arg_info)`: `None` runs the PHP library's
/// `swoole_<name>` through the trampoline.
fn hook(name: &CStr, handler: zif_handler, arg_info: *mut zend_internal_arg_info) {
    let library = handler.is_none();
    let handler = handler.or(ops().user_func);
    let key = name.to_bytes();
    let mut table = table();
    let Table {
        hooked, originals, ..
    } = &mut *table;
    let Some(hooked) = hooked.as_mut() else {
        return;
    };
    if let Some(existing) = hooked.get(key) {
        let function = internal!(existing.function);
        function.handler = handler;
        if !arg_info.is_null() {
            function.arg_info = arg_info;
        }
    } else {
        let function = self::function(key);
        if function.is_null() {
            drop(table);
            let message = message!(
                "hook_func(): The function named `{}` is not found",
                name.to_string_lossy()
            );
            log_warning(message.as_cstr());
            return;
        }
        let entry = internal!(function);
        let (ori_handler, ori_arg_info) = *originals
            .entry(bytes(entry.function_name).into())
            .or_insert((entry.handler, entry.arg_info));
        let mut record = Hooked {
            function,
            ori_handler,
            ori_arg_info,
            ori_fn_flags: 0,
            arg_info_copy: ptr::null_mut(),
            callable: ptr::null_mut(),
            // SAFETY: all-zero is `IS_UNDEF`.
            name: unsafe { std::mem::zeroed() },
        };
        entry.handler = handler;
        if !arg_info.is_null() {
            // SAFETY: the function's own entry, and arginfo of the same shape.
            entry.arg_info = unsafe { swr_zend_runtime_copy_arginfo(entry, arg_info) };
            record.arg_info_copy = entry.arg_info;
        }
        hooked.insert(bytes(entry.function_name).into(), record);
    }
    drop(table);
    if library {
        library_callable(key);
    }
}

/// `init_php_func_cache`: the `swoole_<name>` callable the trampoline runs,
/// made outside the lock (resolving it can warn).
fn library_callable(key: &[u8]) {
    let has = table()
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(key))
        .is_none_or(|record| !record.callable.is_null());
    if has {
        return;
    }
    let mut name = [b"swoole_".as_slice(), key].concat();
    // SAFETY: all-zero is `IS_UNDEF`.
    let mut zname: zval = unsafe { std::mem::zeroed() };
    // SAFETY: a writable zval and the name's bytes, which Zend copies.
    unsafe { swr_zend_set_stringl(&raw mut zname, name.as_ptr().cast(), name.len()) };
    name.clear();
    // SAFETY: a string zval; C++ keeps its own copy.
    let callable = unsafe { (ops().callable)(&raw mut zname) };
    let mut table = table();
    if let Some(record) = table.hooked.as_mut().and_then(|hooked| hooked.get_mut(key)) {
        record.name = zname;
        record.callable = callable;
    }
}

/// `unhook_func(name)`: the original handler and arginfo back.
fn unhook(name: &CStr) {
    let table = table();
    let Some(record) = table
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(name.to_bytes()))
    else {
        return;
    };
    let function = internal!(record.function);
    function.handler = record.ori_handler;
    function.arg_info = record.ori_arg_info;
}

/// `disable_func(name, handler)`.
fn disable(name: &CStr, handler: zif_handler) {
    let mut table = table();
    let Some(hooked) = table.hooked.as_mut() else {
        return;
    };
    if let Some(record) = hooked.get(name.to_bytes()) {
        internal!(record.function).handler = handler;
        return;
    }
    let function = self::function(name.to_bytes());
    if function.is_null() {
        return;
    }
    let entry = internal!(function);
    let record = Hooked {
        function,
        ori_handler: entry.handler,
        ori_arg_info: entry.arg_info,
        ori_fn_flags: entry.fn_flags,
        arg_info_copy: ptr::null_mut(),
        callable: ptr::null_mut(),
        // SAFETY: all-zero is `IS_UNDEF`.
        name: unsafe { std::mem::zeroed() },
    };
    entry.handler = handler;
    entry.fn_flags = ZEND_ACC_FAKE_CLOSURE;
    hooked.insert(bytes(entry.function_name).into(), record);
}

/// `enable_func(name)`.
fn enable(name: &CStr) {
    let table = table();
    if let Some(record) = table
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(name.to_bytes()))
    {
        let function = internal!(record.function);
        function.handler = record.ori_handler;
        function.fn_flags = record.ori_fn_flags;
    }
}

fn php_version() -> i32 {
    // SAFETY: a constant.
    unsafe { swr_zend_runtime_version() }
}

/// `PHPCoroutine::disable_unsafe_function` and `enable_unsafe_function`.
pub(crate) fn unsafe_functions(disable: bool) {
    const UNSAFE: [&CStr; 6] = [
        c"pcntl_fork",
        c"pcntl_rfork",
        c"pcntl_wait",
        c"pcntl_waitpid",
        c"pcntl_sigtimedwait",
        c"pcntl_sigwaitinfo",
    ];
    // HTTP wrapper response headers are request-global; Swoole does not
    // isolate them per coroutine.
    const HEADERS: &CStr = c"http_get_last_response_headers";
    let headers = php_version() >= 80400;
    for name in UNSAFE {
        if disable {
            self::disable(name, Some(swr_zend_runtime_disabled));
        } else {
            enable(name);
        }
    }
    if headers {
        if disable {
            self::disable(HEADERS, Some(swr_zend_runtime_unsupported));
        } else {
            enable(HEADERS);
        }
    }
}

/// RINIT on the main thread: a new hook table, and `exit` and `putenv`
/// replaced where the build does.
pub(crate) fn start() {
    table().hooked = Some(HashMap::new());
    let ops = ops();
    if ops.putenv.is_some() {
        hook(c"putenv", ops.putenv, ptr::null_mut());
    }
    if ops.exit.is_some() {
        hook(c"exit", ops.exit, ptr::null_mut());
    }
}

/// RINIT on another thread before PHP 8.3: functions were copied per thread,
/// so set the main thread's hooked handlers and arginfo again.
pub(crate) fn copy_to_thread() {
    if php_version() >= 80300 {
        return;
    }
    let table = table();
    let Some(hooked) = table.hooked.as_ref() else {
        return;
    };
    for (key, record) in hooked {
        if record.function.is_null() {
            continue;
        }
        let main = internal!(record.function);
        if same(main.handler, record.ori_handler) {
            continue;
        }
        let function = function(key);
        if function.is_null() {
            continue;
        }
        let local = internal!(function);
        local.handler = main.handler;
        if main.arg_info != record.ori_arg_info && !main.arg_info.is_null() {
            // SAFETY: the main thread's entry and its installed arginfo.
            local.arg_info = unsafe { swr_zend_runtime_copy_arginfo(main, main.arg_info) };
        }
    }
}

/// RSHUTDOWN: every hooked function restored, its copies and callable freed,
/// and every grafted class detached.
pub(crate) fn finish() {
    let (hooked, children) = {
        let mut table = table();
        table.originals.clear();
        (table.hooked.take(), std::mem::take(&mut table.children))
    };
    for (_, mut record) in hooked.unwrap_or_default() {
        if !record.callable.is_null() {
            // SAFETY: the callable `ops.callable` made, freed once.
            unsafe { (ops().callable_free)(record.callable) };
        }
        // SAFETY: every zval sets its type.
        if unsafe { record.name.u1.type_info } & 0xff == IS_STRING {
            // SAFETY: the string zval made for the callable.
            unsafe { zval_ptr_dtor(&raw mut record.name) };
        }
        let function = internal!(record.function);
        if !record.arg_info_copy.is_null() {
            // SAFETY: the copy made for this function, freed once.
            unsafe { swr_zend_runtime_free_arginfo(function, record.arg_info_copy) };
        }
        function.handler = record.ori_handler;
        function.arg_info = record.ori_arg_info;
    }
    for ce in children.into_values() {
        detach_class(ce);
    }
}

/// `php_swoole_get_original_handler`: the handler before the first hook, or
/// the internal function's current one.
pub(crate) fn original_handler(name: &[u8]) -> zif_handler {
    let original = table()
        .originals
        .get(name)
        .and_then(|(handler, _)| *handler);
    if original.is_some() {
        return original;
    }
    let function = function(name);
    if function.is_null() {
        return None;
    }
    let function = internal!(function);
    if function.type_ == INTERNAL_FUNCTION {
        function.handler
    } else {
        None
    }
}

/// `swoole_user_func_handler`: outside a coroutine the original handler,
/// inside one the library's `swoole_<name>` with the same arguments.
pub(crate) fn user_func(
    call: &Call,
    frame: *mut zend_execute_data,
    value: *mut zval,
) -> Result<(), Bailout> {
    // SAFETY: the frame of the running internal function.
    let function = unsafe { (*frame).func };
    let name = bytes(internal!(function).function_name);
    if !in_coroutine() {
        let original = table()
            .originals
            .get(name)
            .and_then(|(handler, _)| *handler);
        if original.is_none() {
            return Ok(());
        }
        // SAFETY: the function's own frame and return slot, handed on as Zend
        // passed them; the shim traps a bailout.
        if unsafe { swr_zend_runtime_call_handler(original, frame, value) } != 0 {
            return Err(Bailout);
        }
        return Ok(());
    }
    let callable = table()
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(name))
        .map_or(ptr::null_mut(), |record| record.callable);
    if callable.is_null() {
        let message = message!("{} func not exists", String::from_utf8_lossy(name));
        throw(&message, SW_ERROR_UNDEFINED_BEHAVIOR);
        return Ok(());
    }
    // SAFETY: a live callable.
    let fcc = unsafe { (ops().callable_fcc)(callable) };
    // SAFETY: all-zero is an empty call (`function_name` undefined, no object).
    let mut fci: zend_fcall_info = unsafe { std::mem::zeroed() };
    fci.size = size_of::<zend_fcall_info>();
    fci.retval = value;
    fci.param_count = call.num_args();
    fci.params = call.zval(1);
    // SAFETY: the frame's arguments and return slot, and a resolved callable.
    unsafe { bailout::call(&raw mut fci, fcc) }.map(|_| ())
}

/// `zend_lookup_class`, which may run an autoloader.
fn lookup_class(name: &CStr) -> Result<*mut zend_class_entry, Bailout> {
    let mut found = ptr::null_mut();
    let name = swoole_rs_zend::string::copy(name.to_bytes());
    // SAFETY: a fresh string and a writable slot; the shim traps a bailout.
    let bailed = unsafe { swr_zend_try_lookup_class(name, &raw mut found) } != 0;
    // SAFETY: the string made above, released once.
    unsafe { swr_zend_string_release(name) };
    if bailed {
        return Err(Bailout);
    }
    Ok(found)
}

fn parent(ce: *mut zend_class_entry) -> *mut zend_class_entry {
    // SAFETY: a registered class.
    unsafe { swr_zend_runtime_parent(ce) }
}

/// `inherit_class(child, parent)`: make `parent` the root of `child`'s chain.
fn inherit_class(child: &CStr, parent_name: &CStr) -> Result<(), Bailout> {
    let child = lookup_class(child)?;
    let parent_ce = lookup_class(parent_name)?;
    if child.is_null() || parent_ce.is_null() {
        return Ok(());
    }
    // SAFETY: registered classes.
    if unsafe { swr_zend_runtime_instanceof(child, parent_ce) } {
        return Ok(());
    }
    let mut root = child;
    while !parent(root).is_null() {
        root = parent(root);
    }
    // SAFETY: a registered class, edited on the main thread.
    unsafe { swr_zend_runtime_set_parent(root, parent_ce) };
    // SAFETY: a registered class.
    let name = bytes(unsafe { (*child).name });
    table().children.entry(name.into()).or_insert(child);
    Ok(())
}

/// `detach_parent_class(child)`.
fn detach_parent(child: &CStr) {
    let ce = table().children.remove(child.to_bytes());
    if let Some(ce) = ce {
        detach_class(ce);
    }
}

/// `start_detach_parent_class`: cut the root grafted on last.
fn detach_class(ce: *mut zend_class_entry) {
    let mut below = ce;
    let mut above = parent(ce);
    while !parent(above).is_null() {
        below = parent(below);
        above = parent(above);
    }
    // SAFETY: a registered class, edited on the main thread.
    unsafe { swr_zend_runtime_set_parent(below, ptr::null_mut()) };
}

/// The library's PHP replacements of ext-sockets.
const SOCKETS: [&CStr; 26] = [
    c"socket_create",
    c"socket_create_listen",
    c"socket_create_pair",
    c"socket_connect",
    c"socket_write",
    c"socket_read",
    c"socket_send",
    c"socket_recv",
    c"socket_sendto",
    c"socket_recvfrom",
    c"socket_bind",
    c"socket_listen",
    c"socket_accept",
    c"socket_getpeername",
    c"socket_getsockname",
    c"socket_getopt",
    c"socket_get_option",
    c"socket_setopt",
    c"socket_set_option",
    c"socket_set_block",
    c"socket_set_nonblock",
    c"socket_shutdown",
    c"socket_close",
    c"socket_clear_error",
    c"socket_last_error",
    c"socket_import_stream",
];

/// The native curl replacements, registered as `swoole_native_<name>`.
const NATIVE_CURL: [&CStr; 23] = [
    c"swoole_native_curl_close",
    c"swoole_native_curl_copy_handle",
    c"swoole_native_curl_errno",
    c"swoole_native_curl_error",
    c"swoole_native_curl_exec",
    c"swoole_native_curl_getinfo",
    c"swoole_native_curl_init",
    c"swoole_native_curl_setopt",
    c"swoole_native_curl_setopt_array",
    c"swoole_native_curl_reset",
    c"swoole_native_curl_pause",
    c"swoole_native_curl_escape",
    c"swoole_native_curl_unescape",
    c"swoole_native_curl_multi_init",
    c"swoole_native_curl_multi_add_handle",
    c"swoole_native_curl_multi_exec",
    c"swoole_native_curl_multi_errno",
    c"swoole_native_curl_multi_select",
    c"swoole_native_curl_multi_setopt",
    c"swoole_native_curl_multi_getcontent",
    c"swoole_native_curl_multi_info_read",
    c"swoole_native_curl_multi_remove_handle",
    c"swoole_native_curl_multi_close",
];

/// The library's PHP replacements of ext-curl.
const CURL: [&CStr; 10] = [
    c"curl_init",
    c"curl_setopt",
    c"curl_setopt_array",
    c"curl_exec",
    c"curl_getinfo",
    c"curl_errno",
    c"curl_error",
    c"curl_reset",
    c"curl_close",
    c"curl_multi_getcontent",
];

/// The library's PHP replacements of the network functions (after
/// `gethostbyname`, which is C++).
const NET: [&CStr; 8] = [
    c"gethostbynamel",
    c"mail",
    c"dns_check_record",
    c"checkdnsrr",
    c"dns_get_mx",
    c"getmxrr",
    c"dns_get_record",
    c"gethostbyaddr",
];

/// Hook or unhook a family's functions as its bit changes: what changed.
fn family(
    requested: u32,
    installed: u32,
    bit: u32,
    hooks: &[(&CStr, zif_handler)],
) -> Option<bool> {
    let change = transition(requested, installed, bit);
    for &(name, handler) in hooks {
        match change {
            Some(true) => hook(name, handler, ptr::null_mut()),
            Some(false) => unhook(name),
            None => {}
        }
    }
    change
}

/// `hook_all_func`.
pub(crate) fn hook_all(mut requested: u32, installed: u32) -> Result<(), Bailout> {
    let ops = ops();
    family(
        requested,
        installed,
        flags::STREAM_FUNCTION,
        &[
            (c"stream_select", ops.stream_select),
            (c"stream_socket_pair", ops.stream_socket_pair),
        ],
    );
    family(
        requested,
        installed,
        flags::SLEEP,
        &[
            (c"sleep", Some(sleep::zif_swoole_sleep)),
            (c"usleep", Some(sleep::zif_swoole_usleep)),
            (c"time_nanosleep", Some(sleep::zif_swoole_time_nanosleep)),
            (
                c"time_sleep_until",
                Some(sleep::zif_swoole_time_sleep_until),
            ),
        ],
    );
    family(
        requested,
        installed,
        flags::PROC,
        &[
            (c"proc_open", ops.proc_open),
            (c"proc_close", ops.proc_close),
            (c"proc_get_status", ops.proc_get_status),
            (c"proc_terminate", ops.proc_terminate),
        ],
    );
    let sockets = SOCKETS.map(|name| (name, None));
    match family(requested, installed, flags::SOCKETS, &sockets) {
        Some(true) => inherit_class(c"Swoole\\Coroutine\\Socket", c"Socket")?,
        Some(false) => detach_parent(c"Swoole\\Coroutine\\Socket"),
        None => {}
    }
    if ops.features & flags::NATIVE_CURL != 0 {
        native_curl(&mut requested, installed);
    }
    let curl = CURL.map(|name| (name, None));
    match family(requested, installed, flags::CURL, &curl) {
        Some(true) => inherit_class(c"Swoole\\Curl\\Handler", c"CurlHandle")?,
        Some(false) => detach_parent(c"Swoole\\Curl\\Handler"),
        None => {}
    }
    let mut net = [(c"gethostbyname", ops.gethostbyname); 9];
    for (slot, name) in net.iter_mut().skip(1).zip(NET) {
        *slot = (name, None);
    }
    family(requested, installed, flags::NET_FUNCTION, &net);
    Ok(())
}

/// Native curl, which excludes the PHP curl hook.
fn native_curl(requested: &mut u32, installed: u32) {
    if *requested & flags::NATIVE_CURL != 0 && *requested & flags::CURL != 0 {
        // SAFETY: a NUL-terminated message.
        unsafe {
            (ops().fatal_error)(
                E_WARNING,
                c"cannot enable both hooks HOOK_NATIVE_CURL and HOOK_CURL at same time".as_ptr(),
            );
        }
        *requested ^= flags::CURL;
    }
    let change = transition(*requested, installed, flags::NATIVE_CURL);
    for native in NATIVE_CURL {
        let name = &native.to_bytes_with_nul()[b"swoole_native_".len()..];
        let name = CStr::from_bytes_with_nul(name).unwrap_or(c"");
        match change {
            Some(true) => {
                let replacement = function(native.to_bytes());
                if replacement.is_null() {
                    continue;
                }
                let replacement = internal!(replacement);
                hook(name, replacement.handler, replacement.arg_info);
            }
            Some(false) => unhook(name),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_names_strip_to_curls() {
        for native in NATIVE_CURL {
            let name = &native.to_bytes()[b"swoole_native_".len()..];
            assert!(name.starts_with(b"curl_"), "{native:?}");
        }
    }
}
