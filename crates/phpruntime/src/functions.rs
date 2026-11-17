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
    io::IoSlice,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_zend::{
    Call, E_WARNING, Owned, ZStr, ZString,
    arginfo::raw,
    bailout,
    host::callable::Callable,
    sys::{
        PHP_VERSION_ID, ZEND_ACC_FAKE_CLOSURE, swr_zend_runtime_disabled,
        swr_zend_runtime_unsupported, zend_class_entry, zend_execute_data, zend_internal_arg_info,
        zend_internal_function, zif_handler, zval,
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
    /// The running request's internal function `name` (`EG(function_table)`),
    /// read from the lent bytes (an `IoSlice` is an iovec): null for none or a
    /// user function.
    safe fn swr_zend_runtime_internal_function(name: IoSlice<'_>) -> *mut zend_internal_function;
    /// `copy_arginfo`: a new persistent copy of `function`'s rows of `from`'s
    /// arginfo, its class names duplicated, or none when `from` has no arginfo
    /// or fewer rows. Both are live internal functions, which it only reads.
    #[link_name = "swr_zend_runtime_copy_arginfo"]
    safe fn copy_arginfo(function: Entry, from: Entry) -> Option<ArgInfoCopy>;
    /// `free_arg_info` of a copy, which it takes.
    #[link_name = "swr_zend_runtime_free_arginfo"]
    safe fn free_arginfo(copy: ArgInfoCopy);
}

/// A persistent arginfo copy [`copy_arginfo`] made: only it makes one, and only
/// [`free_arginfo`] takes one, by value, so each is freed at most once.
#[repr(transparent)]
struct ArgInfoCopy(NonNull<zend_internal_arg_info>);

impl ArgInfoCopy {
    /// The copy's argument rows, where a function's `arg_info` points.
    fn as_ptr(&self) -> *mut zend_internal_arg_info {
        self.0.as_ptr()
    }
}

/// An internal function the running request's function table holds. Only
/// [`Entry::find`] makes one, from that table: Zend removes none during a
/// request, and RSHUTDOWN ([`finish`]) drops every record holding one.
#[derive(Clone, Copy)]
#[repr(transparent)]
struct Entry(NonNull<zend_internal_function>);

impl Entry {
    /// `zend::get_function(name)`: the table's function `name`, if it is an
    /// internal one (one PHP code declared has no handler to swap).
    fn find(name: &[u8]) -> Option<Self> {
        NonNull::new(swr_zend_runtime_internal_function(IoSlice::new(name))).map(Self)
    }

    /// The function, borrowed for `f`.
    ///
    /// # Panics
    /// Inside another `with` on this thread: two entries may name one function.
    fn with<R>(self, f: impl FnOnce(&mut zend_internal_function) -> R) -> R {
        thread_local! {
            static BORROWED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
        }
        assert!(
            !BORROWED.replace(true),
            "an internal function borrowed twice"
        );
        let _lent = core::mem::DropGuard::new((), |()| BORROWED.set(false));
        // SAFETY: a live internal function (`find`), borrowed by no other `with` on
        // this thread; the borrow ends with `f`, which takes no `Entry`'s `&mut` again.
        f(unsafe { &mut *self.0.as_ptr() })
    }

    /// The function's handler and arginfo.
    fn handler(self) -> (zif_handler, *mut zend_internal_arg_info) {
        self.with(|function| (function.handler, function.arg_info))
    }
}

/// `PhpFunc`: a hooked or disabled function.
struct Hooked {
    function: Entry,
    ori_handler: zif_handler,
    ori_arg_info: *mut zend_internal_arg_info,
    ori_fn_flags: u32,
    /// The arginfo copy installed with the handler, freed at RSHUTDOWN.
    arg_info_copy: Option<ArgInfoCopy>,
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
    children: HashMap<Box<[u8]>, ClassRef>,
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

/// `hook_func(name, handler, arg_info)`, with the arginfo of `from` when given:
/// a `handler` of `None` runs the PHP library's `swoole_<name>` through the
/// trampoline.
fn hook(name: &CStr, handler: zif_handler, from: Option<Entry>) {
    let library = handler.is_none();
    let handler = handler.or(raw(crate::zif_swoole_user_func_handler));
    let key = name.to_bytes();
    let mut table = table();
    let Table {
        hooked, originals, ..
    } = &mut *table;
    let Some(hooked) = hooked.as_mut() else {
        return;
    };
    if let Some(existing) = hooked.get(key) {
        let arg_info = from.map_or(ptr::null_mut(), |from| from.handler().1);
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
        let arg_info_copy = from.and_then(|from| copy_arginfo(function, from));
        let record = function.with(|entry| {
            let (ori_handler, ori_arg_info) = *originals
                .entry(key.into())
                .or_insert((entry.handler, entry.arg_info));
            entry.handler = handler;
            if let Some(copy) = &arg_info_copy {
                entry.arg_info = copy.as_ptr();
            }
            Hooked {
                function,
                ori_handler,
                ori_arg_info,
                ori_fn_flags: 0,
                arg_info_copy,
                callable: None,
            }
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
            arg_info_copy: None,
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
        hook(c"putenv", raw(crate::putenv::zif_swoole_putenv), None);
    }
    if PHP_VERSION_ID >= 80400 {
        hook(c"exit", Some(zif_swoole_exit), None);
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
        let (handler, arg_info) = main.handler();
        if same(handler, record.ori_handler) {
            continue;
        }
        let Some(local) = Entry::find(key) else {
            continue;
        };
        // This thread's own copy of the main thread's installed arginfo, kept
        // for the thread's life.
        let copy = (arg_info != record.ori_arg_info)
            .then(|| copy_arginfo(main, main))
            .flatten();
        local.with(|local| {
            local.handler = handler;
            if let Some(copy) = copy {
                local.arg_info = copy.as_ptr();
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
        if let Some(copy) = record.arg_info_copy {
            free_arginfo(copy);
        }
        record.function.with(|function| {
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
    let name = call.function().name().map_or(&[][..], ZStr::bytes);
    if !in_coroutine() {
        let original = table()
            .originals
            .get(name)
            .and_then(|(handler, _)| *handler);
        if original.is_none() {
            return Ok(());
        }
        return call.forward(original);
    }
    let callable = table()
        .hooked
        .as_ref()
        .and_then(|hooked| hooked.get(name)?.callable.clone());
    let Some(callable) = callable else {
        let message = message!("{} func not exists", String::from_utf8_lossy(name));
        throw(&message, SW_ERROR_UNDEFINED_BEHAVIOR);
        return Ok(());
    };
    // Keep the callable alive if PHP changes the hook while it runs.
    let callee = callable.cache();
    // The frame's arguments and return slot, handed on.
    let result = Owned::default();
    bailout::call(callee, call.args(), None, &result)?;
    call.set_owned(result);
    Ok(())
}

/// A class Zend's class table held when [`ClassRef::lookup`] or
/// [`crate::class`] found it: Zend frees
/// none during a request, and RSHUTDOWN ([`finish`]) drops every one kept.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub(crate) struct ClassRef(NonNull<zend_class_entry>);

unsafe extern "C" {
    /// `ce->parent`, or null.
    safe fn swr_zend_runtime_parent(ce: ClassRef) -> Option<ClassRef>;
    /// `ce->parent = parent`, on the main thread, which the hooks edit on.
    safe fn swr_zend_runtime_set_parent(ce: ClassRef, parent: Option<ClassRef>);
    /// `instanceof_function(child, parent)`.
    safe fn swr_zend_runtime_instanceof(child: ClassRef, parent: ClassRef) -> bool;
}

impl ClassRef {
    /// `zend_lookup_class`, which may run an autoloader.
    fn lookup(name: &CStr) -> Result<Option<Self>, Bailout> {
        swoole_rs_zend::lookup_class(&ZString::new(name.to_bytes()))
            .map(|ce| NonNull::new(ce).map(Self))
    }
}

/// `inherit_class(child, parent)`: make `parent` the root of `child`'s chain.
fn inherit_class(child_name: &CStr, parent_name: &CStr) -> Result<(), Bailout> {
    let child = ClassRef::lookup(child_name)?;
    let (Some(child), Some(parent_ce)) = (child, ClassRef::lookup(parent_name)?) else {
        return Ok(());
    };
    if swr_zend_runtime_instanceof(child, parent_ce) {
        return Ok(());
    }
    let mut root = child;
    while let Some(parent) = swr_zend_runtime_parent(root) {
        root = parent;
    }
    swr_zend_runtime_set_parent(root, Some(parent_ce));
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
fn detach_class(ce: ClassRef) {
    let mut below = ce;
    let Some(mut above) = swr_zend_runtime_parent(ce) else {
        return;
    };
    while let Some(next) = swr_zend_runtime_parent(above) {
        below = swr_zend_runtime_parent(below).unwrap_or(below);
        above = next;
    }
    swr_zend_runtime_set_parent(below, None);
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
            Some(true) => hook(name, handler, None),
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
                raw(crate::select::zif_swoole_stream_select),
            ),
            (
                c"stream_socket_pair",
                raw(crate::stream::zif_swoole_stream_socket_pair),
            ),
        ],
    );
    family(
        requested,
        installed,
        flags::SLEEP,
        &[
            (c"sleep", raw(sleep::zif_swoole_sleep)),
            (c"usleep", raw(sleep::zif_swoole_usleep)),
            (c"time_nanosleep", raw(sleep::zif_swoole_time_nanosleep)),
            (c"time_sleep_until", raw(sleep::zif_swoole_time_sleep_until)),
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
                hook(name, replacement.handler().0, Some(replacement));
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
