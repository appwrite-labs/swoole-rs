//! The function-table hooks: each hooked function's original handler and
//! arginfo, the handler swapped in (a C++ or Rust replacement, or the
//! library-call trampoline running `swoole_<name>` from the PHP library), the
//! functions disabled inside coroutines, and the classes grafted onto PHP's
//! (`Swoole\Coroutine\Socket` under `Socket`, `Swoole\Curl\Handler` under
//! `CurlHandle`). Edits happen on the main thread; other threads read the table
//! (their RINIT before PHP 8.3, and the trampoline).
use crate::{
    Bailout, SW_ERROR_UNDEFINED_BEHAVIOR, flags, in_coroutine, log_warning, message, sleep, throw,
    transition,
};
use std::{
    collections::HashMap,
    ffi::CStr,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_zend::{
    Call, E_WARNING, Function, Owned, ZStr, ZString, bailout,
    host::callable::Callable,
    sys::{
        PHP_VERSION_ID, ZEND_ACC_FAKE_CLOSURE, swr_zend_function_find,
        swr_zend_runtime_call_handler, swr_zend_runtime_copy_arginfo, swr_zend_runtime_disabled,
        swr_zend_runtime_free_arginfo, swr_zend_runtime_instanceof, swr_zend_runtime_parent,
        swr_zend_runtime_set_parent, swr_zend_runtime_unsupported, zend_class_entry,
        zend_execute_data, zend_fcall_info, zend_internal_arg_info, zend_internal_function,
        zif_handler, zval,
    },
};

// The replacements that live elsewhere: `proc_open`'s (thirdparty proc_open.cc),
// and crates/phpcoroutines' `gethostbyname` and (PHP 8.4+) `exit`.
unsafe extern "C" {
    fn zif_swoole_proc_open(frame: *mut zend_execute_data, value: *mut zval);
    fn zif_swoole_proc_close(frame: *mut zend_execute_data, value: *mut zval);
    fn zif_swoole_proc_get_status(frame: *mut zend_execute_data, value: *mut zval);
    fn zif_swoole_proc_terminate(frame: *mut zend_execute_data, value: *mut zval);
    fn zif_swoole_coroutine_gethostbyname(frame: *mut zend_execute_data, value: *mut zval);
    fn zif_swoole_exit(frame: *mut zend_execute_data, value: *mut zval);
}

/// An internal function the running request's function table holds. Only
/// [`Entry::find`] makes one, from that table: Zend removes none during a
/// request, and RSHUTDOWN ([`finish`]) drops every record holding one.
#[derive(Clone, Copy)]
struct Entry(NonNull<zend_internal_function>);

impl Entry {
    /// `zend::get_function(name)`: the table's function `name`, if it is an
    /// internal one (one PHP code declared has no handler to swap).
    fn find(name: &[u8]) -> Option<Self> {
        // SAFETY: a name and its length, which Zend reads.
        let found = unsafe { swr_zend_function_find(name.as_ptr().cast(), name.len()) };
        // SAFETY: null, or a function the table holds for the request.
        let function = unsafe { Function::from_ptr(found) }?;
        if function.kind() != Function::INTERNAL {
            return None;
        }
        // An internal function's record is a `zend_internal_function`.
        NonNull::new(found.cast()).map(Self)
    }

    /// The function, borrowed for `f`.
    ///
    /// # Panics
    /// Inside another `with` on this thread: two entries may name one function.
    fn with<R>(self, f: impl FnOnce(&mut zend_internal_function) -> R) -> R {
        thread_local! {
            static BORROWED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
        }
        struct Lent;
        impl Drop for Lent {
            fn drop(&mut self) {
                BORROWED.set(false);
            }
        }
        assert!(
            !BORROWED.replace(true),
            "an internal function borrowed twice"
        );
        let _lent = Lent;
        // SAFETY: a live internal function (`find`), borrowed by no other `with` on
        // this thread; the borrow ends with `f`, which takes no `Entry`'s `&mut` again.
        f(unsafe { &mut *self.0.as_ptr() })
    }

    /// A copy of the function's record, for its handler and arginfo: no borrow.
    fn read(self) -> zend_internal_function {
        // SAFETY: a live internal function (`find`), copied whole; nothing is borrowed.
        unsafe { self.0.as_ptr().read() }
    }
}

/// `PhpFunc`: a hooked or disabled function.
struct Hooked {
    function: Entry,
    ori_handler: zif_handler,
    ori_arg_info: *mut zend_internal_arg_info,
    ori_fn_flags: u32,
    /// The arginfo copy installed with the handler, freed at RSHUTDOWN.
    arg_info_copy: *mut zend_internal_arg_info,
    /// The library replacement's callable (`sw_callable_create`), released
    /// (`sw_callable_free`) with the record. Never cloned: only its cache is lent.
    callable: Option<Rc<Callable>>,
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

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the callables are only touched under the table's mutex, by their one record"
)]
// SAFETY: the pointers are Zend's function and class entries, which live for the
// request, and each callable's only owner is its record (the `Rc` is never cloned,
// so its count is touched only as the record is made and dropped); the mutex
// orders every access.
unsafe impl Send for Table {}

static TABLE: LazyLock<Mutex<Table>> = LazyLock::new(Mutex::default);

/// The table, locked. Nothing that can run PHP code (and so bail out) runs
/// while it is held.
fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn same(a: zif_handler, b: zif_handler) -> bool {
    a.map(|f| f as *const ()) == b.map(|f| f as *const ())
}

/// `hook_func(name, handler, arg_info)`: `None` runs the PHP library's
/// `swoole_<name>` through the trampoline.
fn hook(name: &CStr, handler: zif_handler, arg_info: *mut zend_internal_arg_info) {
    let library = handler.is_none();
    let handler = handler.or(Some(crate::zif_swoole_user_func_handler));
    let key = name.to_bytes();
    let mut table = table();
    let Table {
        hooked, originals, ..
    } = &mut *table;
    let Some(hooked) = hooked.as_mut() else {
        return;
    };
    if let Some(existing) = hooked.get(key) {
        existing.function.with(|function| {
            function.handler = handler;
            if !arg_info.is_null() {
                function.arg_info = arg_info;
            }
        });
    } else {
        let Some(function) = Entry::find(key) else {
            drop(table);
            let message = message!(
                "hook_func(): The function named `{}` is not found",
                name.to_string_lossy()
            );
            log_warning(message.as_cstr());
            return;
        };
        let record = function.with(|entry| {
            let (ori_handler, ori_arg_info) = *originals
                .entry(key.into())
                .or_insert((entry.handler, entry.arg_info));
            let mut record = Hooked {
                function,
                ori_handler,
                ori_arg_info,
                ori_fn_flags: 0,
                arg_info_copy: ptr::null_mut(),
                callable: None,
            };
            entry.handler = handler;
            if !arg_info.is_null() {
                // SAFETY: the function's own entry, and arginfo of the same shape.
                entry.arg_info = unsafe { swr_zend_runtime_copy_arginfo(entry, arg_info) };
                record.arg_info_copy = entry.arg_info;
            }
            record
        });
        hooked.insert(key.into(), record);
    }
    drop(table);
    if library {
        library_callable(key);
    }
}

/// `init_php_func_cache`: the `swoole_<name>` callable the trampoline runs,
/// made outside the lock (resolving it can warn). None is made once an
/// earlier warning's handler bailed out: upstream never got this far.
fn library_callable(key: &[u8]) {
    let has = table()
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(key))
        .is_none_or(|record| record.callable.is_some());
    if has || bailout::pending() {
        return;
    }
    let name = Owned::from(ZString::new(&[b"swoole_".as_slice(), key].concat()));
    // The callable keeps its own copy of the name. None after a warning, or with a
    // bailout pending (the warning's handler bailed out), which the caller stops on.
    let callable = Callable::new(&name).map_err(Bailout::defer).ok().flatten();
    let mut table = table();
    if let Some(record) = table.hooked.as_mut().and_then(|hooked| hooked.get_mut(key)) {
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
    record.function.with(|function| {
        function.handler = record.ori_handler;
        function.arg_info = record.ori_arg_info;
    });
}

/// `disable_func(name, handler)`.
fn disable(name: &CStr, handler: zif_handler) {
    let mut table = table();
    let Some(hooked) = table.hooked.as_mut() else {
        return;
    };
    if let Some(record) = hooked.get(name.to_bytes()) {
        record.function.with(|function| function.handler = handler);
        return;
    }
    let Some(function) = Entry::find(name.to_bytes()) else {
        return;
    };
    let record = function.with(|entry| {
        let record = Hooked {
            function,
            ori_handler: entry.handler,
            ori_arg_info: entry.arg_info,
            ori_fn_flags: entry.fn_flags,
            arg_info_copy: ptr::null_mut(),
            callable: None,
        };
        entry.handler = handler;
        entry.fn_flags = ZEND_ACC_FAKE_CLOSURE;
        record
    });
    hooked.insert(name.to_bytes().into(), record);
}

/// `enable_func(name)`.
fn enable(name: &CStr) {
    let table = table();
    if let Some(record) = table
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(name.to_bytes()))
    {
        record.function.with(|function| {
            function.handler = record.ori_handler;
            function.fn_flags = record.ori_fn_flags;
        });
    }
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
    let headers = PHP_VERSION_ID >= 80400;
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
    if built_with(Switch::Thread) {
        hook(
            c"putenv",
            Some(crate::putenv::zif_swoole_putenv),
            ptr::null_mut(),
        );
    }
    if PHP_VERSION_ID >= 80400 {
        hook(c"exit", Some(zif_swoole_exit), ptr::null_mut());
    }
}

/// RINIT on another thread before PHP 8.3: functions were copied per thread,
/// so set the main thread's hooked handlers and arginfo again.
pub(crate) fn copy_to_thread() {
    if PHP_VERSION_ID >= 80300 {
        return;
    }
    let table = table();
    let Some(hooked) = table.hooked.as_ref() else {
        return;
    };
    for (key, record) in hooked {
        let main = record.function;
        let main_record = main.read();
        let (handler, arg_info) = (main_record.handler, main_record.arg_info);
        if same(handler, record.ori_handler) {
            continue;
        }
        let Some(local) = Entry::find(key) else {
            continue;
        };
        // This thread's own copy: never the main thread's entry, read above.
        let copy = (arg_info != record.ori_arg_info && !arg_info.is_null()).then(|| {
            // SAFETY: the main thread's entry and its installed arginfo.
            unsafe { swr_zend_runtime_copy_arginfo(main.0.as_ptr(), arg_info) }
        });
        local.with(|local| {
            local.handler = handler;
            if let Some(copy) = copy {
                local.arg_info = copy;
            }
        });
    }
}

/// RSHUTDOWN: every hooked function restored, its copies and callable freed,
/// and every grafted class detached; a destructor's bailout stays pending for
/// `rshutdown` to raise last.
pub(crate) fn finish() {
    let (hooked, children) = {
        let mut table = table();
        table.originals.clear();
        (table.hooked.take(), std::mem::take(&mut table.children))
    };
    for (_, record) in hooked.unwrap_or_default() {
        // `sw_callable_free`: a destructor's bailout stays pending.
        drop(record.callable);
        record.function.with(|function| {
            if !record.arg_info_copy.is_null() {
                // SAFETY: the copy made for this function, freed once.
                unsafe { swr_zend_runtime_free_arginfo(function, record.arg_info_copy) };
            }
            function.handler = record.ori_handler;
            function.arg_info = record.ori_arg_info;
        });
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
    Entry::find(name)?.with(|function| function.handler)
}

/// `swoole_user_func_handler`: outside a coroutine the original handler,
/// inside one the library's `swoole_<name>` with the same arguments.
pub(crate) fn user_func(call: &Call) -> Result<(), Bailout> {
    let (frame, value) = (call.frame(), call.return_value());
    let name = call.function().name().map_or(&[][..], ZStr::bytes);
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
    let fcc = table()
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(name)?.callable.as_deref())
        .map(Callable::cache_ptr);
    let Some(fcc) = fcc else {
        let message = message!("{} func not exists", String::from_utf8_lossy(name));
        throw(&message, SW_ERROR_UNDEFINED_BEHAVIOR);
        return Ok(());
    };
    let mut fci = swoole_rs_zend::EMPTY_FCI;
    fci.size = size_of::<zend_fcall_info>();
    fci.retval = value;
    fci.param_count = call.num_args();
    fci.params = call.zval(1);
    // SAFETY: the frame's arguments and return slot, and the cache of a resolved
    // callable its record keeps until RSHUTDOWN.
    unsafe { bailout::call(&raw mut fci, fcc) }.map(|_| ())
}

/// `zend_lookup_class`, which may run an autoloader.
fn lookup_class(name: &CStr) -> Result<*mut zend_class_entry, Bailout> {
    swoole_rs_zend::lookup_class(&ZString::new(name.to_bytes()))
}

fn parent(ce: *mut zend_class_entry) -> *mut zend_class_entry {
    // SAFETY: a registered class.
    unsafe { swr_zend_runtime_parent(ce) }
}

/// `inherit_class(child, parent)`: make `parent` the root of `child`'s chain.
fn inherit_class(child_name: &CStr, parent_name: &CStr) -> Result<(), Bailout> {
    let child = lookup_class(child_name)?;
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
    // Under the name `detach_parent` is given, which names the class as it is declared.
    table()
        .children
        .entry(child_name.to_bytes().into())
        .or_insert(child);
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
    family(
        requested,
        installed,
        flags::STREAM_FUNCTION,
        &[
            (
                c"stream_select",
                Some(crate::select::zif_swoole_stream_select),
            ),
            (
                c"stream_socket_pair",
                Some(crate::stream::zif_swoole_stream_socket_pair),
            ),
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
            (c"proc_open", Some(zif_swoole_proc_open)),
            (c"proc_close", Some(zif_swoole_proc_close)),
            (c"proc_get_status", Some(zif_swoole_proc_get_status)),
            (c"proc_terminate", Some(zif_swoole_proc_terminate)),
        ],
    );
    let sockets = SOCKETS.map(|name| (name, None));
    match family(requested, installed, flags::SOCKETS, &sockets) {
        Some(true) => inherit_class(c"Swoole\\Coroutine\\Socket", c"Socket")?,
        Some(false) => detach_parent(c"Swoole\\Coroutine\\Socket"),
        None => {}
    }
    if crate::features() & flags::NATIVE_CURL != 0 {
        native_curl(&mut requested, installed);
    }
    let curl = CURL.map(|name| (name, None));
    match family(requested, installed, flags::CURL, &curl) {
        Some(true) => inherit_class(c"Swoole\\Curl\\Handler", c"CurlHandle")?,
        Some(false) => detach_parent(c"Swoole\\Curl\\Handler"),
        None => {}
    }
    let gethostbyname: zif_handler = Some(zif_swoole_coroutine_gethostbyname);
    let mut net = [(c"gethostbyname", gethostbyname); 9];
    for (slot, name) in net.iter_mut().skip(1).zip(NET) {
        *slot = (name, None);
    }
    family(requested, installed, flags::NET_FUNCTION, &net);
    Ok(())
}

/// Native curl, which excludes the PHP curl hook.
fn native_curl(requested: &mut u32, installed: u32) {
    if *requested & flags::NATIVE_CURL != 0 && *requested & flags::CURL != 0 {
        bailout::defer(crate::report(
            crate::Report::Fatal,
            E_WARNING,
            c"cannot enable both hooks HOOK_NATIVE_CURL and HOOK_CURL at same time",
        ));
        *requested ^= flags::CURL;
    }
    let change = transition(*requested, installed, flags::NATIVE_CURL);
    for native in NATIVE_CURL {
        let name = native.to_bytes_with_nul().strip_prefix(b"swoole_native_");
        let name = name
            .and_then(|name| CStr::from_bytes_with_nul(name).ok())
            .unwrap_or(c"");
        match change {
            Some(true) => {
                let Some(replacement) = Entry::find(native.to_bytes()) else {
                    continue;
                };
                let replaced = replacement.read();
                let (handler, arg_info) = (replaced.handler, replaced.arg_info);
                hook(name, handler, arg_info);
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
