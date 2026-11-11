//! MINIT: `Swoole\Coroutine` and its iterator, context and exception
//! classes, the constants, `Swoole\ExitException`, the exit and `@` opcode
//! handlers and the coroutine-aware autoloader.
use crate::{context, host, runtime};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{CStr, c_int, c_long, c_void},
    hash::{BuildHasherDefault, DefaultHasher},
    mem, ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_phpstubs::{coroutine, coroutine_system};
use swoole_rs_zend::{
    Call, Class, Minit,
    bailout::Bailout,
    sys::{
        ZEND_ACC_FINAL, ZEND_ACC_PRIVATE, swr_zend_autoload_forget, swr_zend_autoload_handler,
        swr_zend_autoload_try, swr_zend_coroutine_opcodes, swr_zend_coroutine_set_autoload,
        swr_zend_error_reporting, swr_zend_exit_status, swr_zval_try_addref, zend_class_entry,
        zend_execute_data, zend_read_property, zend_string, zend_throw_exception,
        zend_update_property, zend_update_property_long, zend_update_property_str,
    },
};
use swoole_rs_zend::{method_table, sys as z};

static COROUTINE: Class = Class::new();
static ITERATOR: Class = Class::new();
pub(crate) static CONTEXT: Class = Class::new();
static EXIT: Class = Class::new();
static CANCELED: Class = Class::new();
static TIMEOUT: Class = Class::new();

pub(crate) fn iterator() -> &'static Class {
    &ITERATOR
}

pub(crate) fn canceled() -> *mut zend_class_entry {
    CANCELED.entry()
}

pub(crate) fn timeout() -> *mut zend_class_entry {
    TIMEOUT.entry()
}

/// `sw_exit_flags`.
const EXIT_IN_COROUTINE: i64 = 1 << 1;
const EXIT_IN_SERVER: i64 = 1 << 2;
/// `Coroutine::MAX_NUM_LIMIT` (`LONG_MAX`).
const MAX_NUM_LIMIT: i64 = c_long::MAX;
/// `ZEND_USER_OPCODE_DISPATCH`: run the original handler.
const DISPATCH: c_int = 2;

method_table! {
    static COROUTINE_METHODS = [
        c"create" => crate::methods::zif_swoole_coroutine_create, &coroutine::arginfo_class_Swoole_Coroutine_create, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"defer" => crate::methods::zif_swoole_coroutine_defer, &coroutine::arginfo_class_Swoole_Coroutine_defer, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"set" => crate::scheduler::zim_swoole_coroutine_scheduler_set, &coroutine::arginfo_class_Swoole_Coroutine_set, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getOptions" => crate::scheduler::zim_swoole_coroutine_scheduler_getOptions, &coroutine::arginfo_class_Swoole_Coroutine_getOptions, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"exists" => crate::methods::zim_swoole_coroutine_exists, &coroutine::arginfo_class_Swoole_Coroutine_exists, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"yield" => crate::methods::zim_swoole_coroutine_yield, &coroutine::arginfo_class_Swoole_Coroutine_yield, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"cancel" => crate::methods::zim_swoole_coroutine_cancel, &coroutine::arginfo_class_Swoole_Coroutine_cancel, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"join" => crate::methods::zim_swoole_coroutine_join, &coroutine::arginfo_class_Swoole_Coroutine_join, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"isCanceled" => crate::methods::zim_swoole_coroutine_isCanceled, &coroutine::arginfo_class_Swoole_Coroutine_isCanceled, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"setTimeLimit" => crate::methods::zim_swoole_coroutine_setTimeLimit, &coroutine::arginfo_class_Swoole_Coroutine_setTimeLimit, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"suspend" => crate::methods::zim_swoole_coroutine_yield, &coroutine::arginfo_class_Swoole_Coroutine_suspend, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"resume" => crate::methods::zim_swoole_coroutine_resume, &coroutine::arginfo_class_Swoole_Coroutine_resume, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"stats" => crate::methods::zim_swoole_coroutine_stats, &coroutine::arginfo_class_Swoole_Coroutine_stats, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getCid" => crate::methods::zim_swoole_coroutine_getCid, &coroutine::arginfo_class_Swoole_Coroutine_getCid, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getuid" => crate::methods::zim_swoole_coroutine_getCid, &coroutine::arginfo_class_Swoole_Coroutine_getuid, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getPcid" => crate::methods::zim_swoole_coroutine_getPcid, &coroutine::arginfo_class_Swoole_Coroutine_getPcid, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getContext" => crate::methods::zim_swoole_coroutine_getContext, &coroutine::arginfo_class_Swoole_Coroutine_getContext, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getBackTrace" => crate::methods::zim_swoole_coroutine_getBackTrace, &coroutine::arginfo_class_Swoole_Coroutine_getBackTrace, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"printBackTrace" => crate::methods::zim_swoole_coroutine_printBackTrace, &coroutine::arginfo_class_Swoole_Coroutine_printBackTrace, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getElapsed" => crate::methods::zim_swoole_coroutine_getElapsed, &coroutine::arginfo_class_Swoole_Coroutine_getElapsed, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getStackUsage" => crate::methods::zim_swoole_coroutine_getStackUsage, &coroutine::arginfo_class_Swoole_Coroutine_getStackUsage, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"list" => crate::methods::zim_swoole_coroutine_list, &coroutine::arginfo_class_Swoole_Coroutine_list, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"listCoroutines" => crate::methods::zim_swoole_coroutine_list, &coroutine::arginfo_class_Swoole_Coroutine_listCoroutines, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"enableScheduler" => crate::methods::zim_swoole_coroutine_enableScheduler, &coroutine::arginfo_class_Swoole_Coroutine_enableScheduler, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"disableScheduler" => crate::methods::zim_swoole_coroutine_disableScheduler, &coroutine::arginfo_class_Swoole_Coroutine_disableScheduler, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        // SW_CORO_TIME builds only; MINIT drops it from the others.
        c"getExecuteTime" => crate::methods::zim_swoole_coroutine_getExecuteTime, &coroutine::arginfo_class_Swoole_Coroutine_getExecuteTime, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"gethostbyname" => crate::system::zif_swoole_coroutine_gethostbyname, &coroutine_system::arginfo_class_Swoole_Coroutine_System_gethostbyname, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"dnsLookup" => crate::system::zif_swoole_async_dns_lookup_coro, &coroutine_system::arginfo_class_Swoole_Coroutine_System_dnsLookup, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"exec" => crate::system::zim_swoole_coroutine_system_exec, &coroutine_system::arginfo_class_Swoole_Coroutine_System_exec, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"sleep" => crate::system::zim_swoole_coroutine_system_sleep, &coroutine_system::arginfo_class_Swoole_Coroutine_System_sleep, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getaddrinfo" => crate::system::zim_swoole_coroutine_system_getaddrinfo, &coroutine_system::arginfo_class_Swoole_Coroutine_System_getaddrinfo, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"statvfs" => crate::system::zim_swoole_coroutine_system_statvfs, &coroutine_system::arginfo_class_Swoole_Coroutine_System_statvfs, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"readFile" => crate::system::zim_swoole_coroutine_system_readFile, &coroutine_system::arginfo_class_Swoole_Coroutine_System_readFile, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"writeFile" => crate::system::zim_swoole_coroutine_system_writeFile, &coroutine_system::arginfo_class_Swoole_Coroutine_System_writeFile, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"wait" => crate::system::zim_swoole_coroutine_system_wait, &coroutine_system::arginfo_class_Swoole_Coroutine_System_wait, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"waitPid" => crate::system::zim_swoole_coroutine_system_waitPid, &coroutine_system::arginfo_class_Swoole_Coroutine_System_waitPid, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"waitSignal" => crate::system::zim_swoole_coroutine_system_waitSignal, &coroutine_system::arginfo_class_Swoole_Coroutine_System_waitSignal, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"waitEvent" => crate::system::zim_swoole_coroutine_system_waitEvent, &coroutine_system::arginfo_class_Swoole_Coroutine_System_waitEvent, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

method_table! {
    static EXIT_EXCEPTION_METHODS = [
        c"getFlags" => zim_swoole_exit_exception_getFlags, &coroutine::arginfo_class_Swoole_ExitException_getFlags, z::ZEND_ACC_PUBLIC;
        c"getStatus" => zim_swoole_exit_exception_getStatus, &coroutine::arginfo_class_Swoole_ExitException_getStatus, z::ZEND_ACC_PUBLIC;
    ];
}

swoole_rs_zend::minit!(php_swoole_coroutine_minit => register);

/// `php_swoole_coroutine_minit`, after `Swoole\Exception` and SPL's classes.
fn register(minit: &Minit) -> Result<(), Bailout> {
    let shortname = host::flag(host::SHORTNAME);
    let alias = |class: &Class, name: &CStr| -> Result<(), Bailout> {
        if shortname {
            minit.alias(class, name)?;
        }
        Ok(())
    };
    let methods = if host::flag(host::CORO_TIME) {
        &COROUTINE_METHODS.0
    } else {
        COROUTINE_METHODS.without(&[c"getExecuteTime"])
    };
    minit.register_static(&COROUTINE, c"Swoole\\Coroutine", methods);
    alias(&COROUTINE, c"Co")?;
    minit.register_child(
        &ITERATOR,
        c"Swoole\\Coroutine\\Iterator",
        &[],
        minit.array_iterator(),
        |_, _| {},
    );
    alias(&ITERATOR, c"Co\\Iterator")?;
    minit.register_child(
        &CONTEXT,
        c"Swoole\\Coroutine\\Context",
        &[],
        minit.array_object(),
        |_, _| {},
    );
    alias(&CONTEXT, c"Co\\Context")?;
    let constants: [(&CStr, i64); 6] = [
        (c"SWOOLE_DEFAULT_MAX_CORO_NUM", runtime::DEFAULT_MAX_NUM),
        (c"SWOOLE_CORO_MAX_NUM_LIMIT", MAX_NUM_LIMIT),
        (c"SWOOLE_CORO_INIT", 0),
        (c"SWOOLE_CORO_WAITING", 1),
        (c"SWOOLE_CORO_RUNNING", 2),
        (c"SWOOLE_CORO_END", 3),
    ];
    for (name, value) in constants {
        minit.long_constant(name, value);
    }
    // Exit is prohibited in a coroutine: it throws this instead.
    let exception = minit.parent(swoole_rs_phpswoole::exception());
    minit.register_child(
        &EXIT,
        c"Swoole\\ExitException",
        &EXIT_EXCEPTION_METHODS.0,
        exception,
        |_, _| {},
    );
    minit.property_long(&EXIT, c"flags", 0, ZEND_ACC_PRIVATE);
    minit.property_long(&EXIT, c"status", 0, ZEND_ACC_PRIVATE);
    let final_class = |entry: &mut zend_class_entry, _: &mut _| entry.ce_flags |= ZEND_ACC_FINAL;
    let canceled = c"Swoole\\Coroutine\\CanceledException";
    minit.register_child(&CANCELED, canceled, &[], minit.exception(), final_class);
    let timeout = c"Swoole\\Coroutine\\TimeoutException";
    minit.register_child(&TIMEOUT, timeout, &[], minit.exception(), final_class);
    minit.long_constant(c"SWOOLE_EXIT_IN_COROUTINE", EXIT_IN_COROUTINE);
    minit.long_constant(c"SWOOLE_EXIT_IN_SERVER", EXIT_IN_SERVER);
    if host::flag(host::CLI) {
        // SAFETY: MINIT; the handlers are static.
        unsafe {
            swr_zend_coroutine_opcodes(Some(exit_handler), Some(begin_silence), Some(end_silence));
        };
    }
    // SAFETY: MINIT; restored never, as upstream.
    let original = unsafe { swr_zend_coroutine_set_autoload(Some(autoload)) };
    ORIGINAL_AUTOLOAD.store(
        original.map_or(ptr::null_mut(), |f| f as *mut c_void),
        Ordering::Relaxed,
    );

    Ok(())
}

/// `php_swoole_coroutine_rinit`.
pub extern "C" fn php_swoole_coroutine_rinit() {
    context::init_main();
}

/// `php_swoole_coroutine_rshutdown`: a bailout in deactivation is raised.
pub extern "C" fn php_swoole_coroutine_rshutdown() {
    AUTOLOADING.with(|a| a.borrow_mut().clear());
    if runtime::shutdown() {
        runtime::raise();
    }
}

/// `exit` in a coroutine or a started server: `SWOOLE_EXIT_*` flags, or 0.
fn exit_flags() -> i64 {
    let mut flags = 0;
    if !host::current().is_null() {
        flags |= EXIT_IN_COROUTINE;
    }
    if host::flag(host::SERVER_STARTED) {
        flags |= EXIT_IN_SERVER;
    }
    flags
}

/// Throw `Swoole\ExitException` with its flags; the caller sets its status.
fn throw_exit(
    message: *const std::ffi::c_char,
    flags: i64,
) -> *mut swoole_rs_zend::sys::zend_object {
    // SAFETY: a NUL-terminated message and the registered class.
    let object = unsafe { zend_throw_exception(EXIT.entry(), message, 0) };
    // SAFETY: the new exception; Zend copies the name.
    unsafe { zend_update_property_long(EXIT.entry(), object, c"flags".as_ptr(), 5, flags) };
    object
}

/// `coro_exit_handler` (before PHP 8.4, where `exit` is an opcode).
unsafe extern "C" fn exit_handler(execute_data: *mut zend_execute_data) -> c_int {
    let flags = exit_flags();
    if flags != 0 {
        let mut status = swoole_rs_zend::UNDEF;
        // SAFETY: the exit opcode's frame.
        unsafe { swr_zend_exit_status(execute_data, &raw mut status) };
        let object = throw_exit(c"swoole exit".as_ptr(), flags);
        // SAFETY: upstream's extra reference, kept as it was.
        unsafe { swr_zval_try_addref(&raw mut status) };
        // SAFETY: the new exception; Zend copies the value.
        unsafe {
            zend_update_property(EXIT.entry(), object, c"status".as_ptr(), 6, &raw mut status);
        };
    }
    DISPATCH
}

swoole_rs_zend::handlers! {
    zif_swoole_exit => exit;
    zim_swoole_exit_exception_getFlags => get_flags;
    zim_swoole_exit_exception_getStatus => get_status;
}

/// `swoole_exit` (PHP 8.4+, where `exit` is a function):
/// `(string|int $status = 0)`.
fn exit(call: &Call) {
    let flags = exit_flags();
    let Ok(status) = call.arity(0, 1).and_then(|()| call.str_or_long(1)) else {
        return;
    };
    let (message, code): (*mut zend_string, i64) = match status {
        Some(Err(message)) => (message, 0),
        Some(Ok(code)) => (ptr::null_mut(), code),
        None => (ptr::null_mut(), 0),
    };
    if flags != 0 {
        let text = if message.is_null() {
            c"swoole exit".as_ptr()
        } else {
            // SAFETY: a live string argument.
            swoole_rs_zend::string::val(message).cast_const().cast()
        };
        let object = throw_exit(text, flags);
        if message.is_null() {
            // SAFETY: the new exception; Zend copies the name.
            unsafe { zend_update_property_long(EXIT.entry(), object, c"status".as_ptr(), 6, code) };
        } else {
            // SAFETY: as above; Zend takes a reference to the string.
            unsafe {
                zend_update_property_str(EXIT.entry(), object, c"status".as_ptr(), 6, message);
            };
        }
        return;
    }
    if host::exit_original(call.frame(), call.return_value()) {
        return;
    }
    // SAFETY: as above.
    let text = (!message.is_null()).then(|| unsafe { swoole_rs_zend::string::bytes(message) });
    host::exit_fallback(text, code);
}

/// `SW_RETURN_PROPERTY(name)` of `$this`.
fn property(call: &Call, name: &CStr) {
    let this = call.this();
    // The read's scratch slot.
    let mut scratch = swoole_rs_zend::UNDEF;
    // SAFETY: a live object.
    let scope = unsafe { (*this).ce };
    // SAFETY: as above, read in its own class's scope.
    let value = unsafe {
        zend_read_property(
            scope,
            this,
            name.as_ptr(),
            name.count_bytes(),
            false,
            &raw mut scratch,
        )
    };
    // SAFETY: the property's live value.
    unsafe { call.set_copy(value) };
}

/// `ExitException::getFlags()`.
fn get_flags(call: &Call) {
    property(call, c"flags");
}

/// `ExitException::getStatus()`.
fn get_status(call: &Call) {
    property(call, c"status");
}

/// `coro_begin_silence_handler`: `@` remembers the reporting level a switch
/// restores.
extern "C" fn begin_silence(_: *mut zend_execute_data) -> c_int {
    // SAFETY: Zend's executor globals.
    let reporting = unsafe { swr_zend_error_reporting() };
    context::current().set_silence(Some(reporting));
    DISPATCH
}

/// `coro_end_silence_handler`.
extern "C" fn end_silence(_: *mut zend_execute_data) -> c_int {
    context::current().set_silence(None);
    DISPATCH
}

/// The autoloader Zend had (`original_zend_autoload`).
static ORIGINAL_AUTOLOAD: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// A coroutine waiting for another's autoload of the same class.
struct Waiter {
    co: *mut c_void,
    ce: *mut zend_class_entry,
}

/// The coroutine autoloading a class, and those waiting for it.
struct Loading {
    owner: *mut c_void,
    waiters: VecDeque<*mut Waiter>,
}

per_thread! {
    /// Classes being autoloaded, by lowercase name (`SWOOLE_G(in_autoload)`).
    static AUTOLOADING: RefCell<HashMap<Vec<u8>, Loading, BuildHasherDefault<DefaultHasher>>> =
        RefCell::new(HashMap::with_hasher(BuildHasherDefault::new()));
}

/// Run Zend's autoloader, trapping its bailouts.
fn load(name: *mut zend_string, lc_name: *mut zend_string) -> Result<*mut zend_class_entry, ()> {
    let handler = ORIGINAL_AUTOLOAD.load(Ordering::Relaxed);
    // SAFETY: stored from a `swr_zend_autoload_handler` at MINIT.
    let handler = unsafe { mem::transmute::<*mut c_void, swr_zend_autoload_handler>(handler) };
    let mut result = ptr::null_mut();
    // SAFETY: Zend's autoloader, trapped in C.
    if unsafe { swr_zend_autoload_try(handler, name, lc_name, &raw mut result) } != 0 {
        return Err(());
    }
    Ok(result)
}

/// `swoole_coroutine_autoload`: one coroutine loads a class while others
/// asking for it wait, instead of each running the autoloader.
unsafe extern "C" fn autoload(
    name: *mut zend_string,
    lc_name: *mut zend_string,
) -> *mut zend_class_entry {
    let current = host::current();
    if current.is_null() {
        let Ok(ce) = load(name, lc_name) else {
            runtime::raise();
            return ptr::null_mut();
        };
        return ce;
    }
    // SAFETY: Zend marked the class as loading.
    unsafe { swr_zend_autoload_forget(lc_name) };
    // SAFETY: a live string.
    let key = unsafe { swoole_rs_zend::string::bytes(lc_name) }.to_vec();
    let mut waiter = Waiter {
        co: current,
        ce: ptr::null_mut(),
    };
    let waiting = AUTOLOADING.with(|a| match a.borrow_mut().get_mut(&key) {
        Some(loading) if loading.owner == current => Some(false),
        Some(loading) => {
            loading.waiters.push_back(&raw mut waiter);
            Some(true)
        }
        None => None,
    });
    match waiting {
        Some(false) => return ptr::null_mut(),
        Some(true) => {
            drop(key);
            host::yield_(current);
            return waiter.ce;
        }
        None => {}
    }
    AUTOLOADING.with(|a| {
        a.borrow_mut().insert(
            key.clone(),
            Loading {
                owner: current,
                waiters: VecDeque::new(),
            },
        )
    });
    let loaded = load(name, lc_name);
    let loading = AUTOLOADING.with(|a| a.borrow_mut().remove(&key));
    drop(key);
    let Ok(ce) = loaded else {
        drop(loading);
        runtime::raise();
        return ptr::null_mut();
    };
    if let Some(loading) = loading {
        for waiter in loading.waiters {
            // SAFETY: a waiter suspended on its own stack until resumed.
            unsafe { (*waiter).ce = ce };
            // SAFETY: as above.
            host::resume(unsafe { (*waiter).co });
        }
    }
    ce
}
