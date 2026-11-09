//! A thread-mode Server (`SW_THREAD`): the settings its worker threads share,
//! the PHP thread each worker runs in, and `start()` inside that thread, which
//! runs the worker's body.
use crate::lifecycle::{MODE_THREAD, data};
use std::{
    cell::Cell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_corebase::Thread;
use swoole_rs_process::workers::thread::{WorkerMain, set_worker_thread_start};
use swoole_rs_server::view;
use swoole_rs_zend::{
    bailout::{self, Bailout},
    string::bytes,
    sys::{
        IS_ARRAY, IS_ARRAY_EX, IS_NULL, IS_STRING, swr_zend_property_slot,
        swr_zend_string_persistent, swr_zend_string_release, swr_zend_thread_main_request,
        swr_zend_tsrm_is_main_thread, zend_array_dup, zend_fcall_info, zend_hash_apply,
        zend_object, zend_string, zend_value, zval, zval_ptr_dtor,
    },
    type_of,
    value::{str_of, table, update_string},
};

/// What every worker thread starts its PHP request with: the bootstrap script
/// and the `init_arguments` result (a thread array handle, or null).
struct Start {
    bootstrap: *mut zend_string,
    argv: *mut c_void,
}

/// The running thread-mode server's [`Start`], or null.
static START: AtomicPtr<Start> = AtomicPtr::new(ptr::null_mut());

thread_local! {
    /// Upstream's `worker_thread_fn`: this worker thread's body.
    static MAIN: Cell<Option<(WorkerMain, *mut c_void)>> = const { Cell::new(None) };
}

/// Thread mode, in a build with `SW_THREAD`.
fn thread_mode(serv: *mut c_void) -> bool {
    // SAFETY: its callers pass their live server.
    let serv = unsafe { view::Server::from_raw(serv) };
    // SAFETY: a live server.
    crate::server::THREAD.load(Ordering::Relaxed) && i64::from(serv.config().mode_) == MODE_THREAD
}

/// `__construct`'s check: 0 no server yet, 1 this worker thread's server
/// (in `existing`), 2 not the main thread, 3 a server already exists.
pub(crate) fn admit(existing: &mut *mut c_void) -> c_int {
    let current = swoole_rs_corebase::swr_v1_core_server();
    if !crate::server::THREAD.load(Ordering::Relaxed) {
        return if current.is_null() { 0 } else { 3 };
    }
    // SAFETY: the process's live server, when it has one.
    if !current.is_null() && unsafe { view::Server::from_raw(current) }.is_worker_thread() {
        *existing = current;
        return 1;
    }
    // SAFETY: a pure TSRM read.
    if unsafe { swr_zend_tsrm_is_main_thread() } {
        0
    } else {
        2
    }
}

/// `start()` on a worker thread: the settings the master serialized, this
/// thread's coroutine options, then the worker's body. `None`: not a worker
/// thread.
pub(crate) fn worker(object: *mut zend_object, serv: *mut c_void) -> Option<Result<(), Bailout>> {
    // SAFETY: `serv` is the live server here.
    if !unsafe { view::Server::from_raw(serv) }.is_worker_thread() {
        return None;
    }
    Some(run_worker(object, serv))
}

fn run_worker(object: *mut zend_object, serv: *mut c_void) -> Result<(), Bailout> {
    // SAFETY: its callers pass their live server.
    let serv = unsafe { view::Server::from_raw(serv) };
    let mut zserv = crate::dispatch::undef();
    // SAFETY: a borrowed view of the live object (`ZVAL_OBJ`, no reference).
    unsafe { swoole_rs_zend::set_object(&raw mut zserv, object) };
    // SAFETY: a live object.
    let ce = unsafe { (*object).ce };
    // SAFETY: the Server object and its class, which declares `setting`.
    let setting =
        unsafe { swoole_rs_phphelpers::array_property_of(ce, &raw mut zserv, c"setting") }?;
    // SAFETY: the property's array, replaced below.
    unsafe { zval_ptr_dtor(setting) };
    let undef = crate::dispatch::undef();
    // SAFETY: the slot just released.
    unsafe { setting.write(undef) };
    let serialized = serv.config().private_data_4.cast::<zend_string>();
    // SAFETY: the persistent string `copy_setting` made.
    let text = unsafe { bytes(serialized) };
    // SAFETY: the empty slot.
    let (_, bailed) = unsafe { swoole_rs_phpswoole::unserialize(text, setting) };
    bailed?;
    // SAFETY: the slot unserialize filled.
    if unsafe { type_of(setting) } == IS_ARRAY {
        // SAFETY: an array setting.
        let options = unsafe { table(setting) };
        // SAFETY: as above. The coroutine options are thread-local: each
        // worker thread sets them once.
        unsafe { swoole_rs_phpcoroutines::php_swoole_set_coroutine_option(options) };
    }
    match MAIN.get() {
        // SAFETY: the body the ThreadFactory handed this thread, which is C++.
        Some((run, context)) => unsafe { bailout::trap(run, context) },
        None => Ok(()),
    }
}

/// `start()` in thread mode, before the server starts: the bootstrap script,
/// the `init_arguments` result, and the thread starter; the runtime hooks are
/// enabled before any thread is created.
pub(crate) fn prepare(object: *mut zend_object, serv: *mut c_void) -> Result<(), Bailout> {
    if !thread_mode(serv) {
        return Ok(());
    }
    // SAFETY: the Server object, which declares `bootstrap`.
    let slot = unsafe { swr_zend_property_slot(object, c"bootstrap".as_ptr(), 9) };
    if string_of(slot).is_empty() {
        let (mut path, mut argc): (*const c_char, c_int) = (ptr::null(), 0);
        // SAFETY: two writable out-parameters.
        unsafe { swr_zend_thread_main_request(&raw mut path, &raw mut argc) };
        if !path.is_null() {
            // SAFETY: the request's NUL-terminated script.
            let path = unsafe { CStr::from_ptr(path) };
            // SAFETY: a live object.
            let ce = unsafe { (*object).ce };
            // SAFETY: the object and its class.
            unsafe { update_string(ce, object, c"bootstrap", path) };
        }
    }
    let script = string_of(slot);
    // SAFETY: the script's bytes; a persistent copy the threads share.
    let bootstrap = unsafe { swr_zend_string_persistent(script.as_ptr().cast(), script.len()) };
    let argv = match init_arguments(object) {
        Ok(argv) => argv,
        Err(bailout) => {
            // SAFETY: the copy made above.
            unsafe { swr_zend_string_release(bootstrap) };
            return Err(bailout);
        }
    };
    let start = Box::into_raw(Box::new(Start { bootstrap, argv }));
    release_start(START.swap(start, Ordering::AcqRel));
    set_worker_thread_start(Some(start_thread));
    let flags = swoole_rs_phpcoroutines::swr_v1_phpcoroutines_hook_flags();
    if flags > 0 {
        swoole_rs_phpruntime::enable_hook(flags)?;
    }
    Ok(())
}

/// A string property's bytes (upstream reads it as a string); empty otherwise.
fn string_of<'a>(slot: *mut zval) -> &'a [u8] {
    // SAFETY: a live property slot.
    if unsafe { type_of(slot) } != IS_STRING {
        return b"";
    }
    // SAFETY: a string slot.
    let string = unsafe { str_of(slot) };
    // SAFETY: a live string.
    unsafe { bytes(string) }
}

/// `init_arguments()`'s array as a thread array handle, or null.
fn init_arguments(object: *mut zend_object) -> Result<*mut c_void, Bailout> {
    // SAFETY: `$this` is a Server object.
    let callable = unsafe { data(object) }.init_arguments;
    // SAFETY: a live zval.
    if unsafe { type_of(&raw const callable) } == IS_NULL {
        return Ok(ptr::null_mut());
    }
    let mut result = crate::dispatch::undef();
    // SAFETY: all-zero is an empty call (no object, no arguments).
    let mut fci: zend_fcall_info = unsafe { std::mem::zeroed() };
    fci.size = size_of::<zend_fcall_info>();
    fci.function_name = callable;
    fci.retval = &raw mut result;
    // SAFETY: `call_user_function(NULL, nullptr, &init_arguments, &result, 0, nullptr)`.
    unsafe { bailout::call(&raw mut fci, ptr::null_mut()) }?;
    let mut argv = ptr::null_mut();
    // SAFETY: the call's result.
    if unsafe { type_of(&raw const result) } == IS_ARRAY {
        // SAFETY: an array result.
        let array = unsafe { table(&raw const result) };
        // SAFETY: as above, copied into a thread array.
        argv = unsafe { swoole_rs_phpthread::swr_v1_phpthread_array_from(array) };
    }
    // SAFETY: the result this call owns.
    unsafe { zval_ptr_dtor(&raw mut result) };
    Ok(argv)
}

/// The thread starter: this `ThreadFactory` thread becomes a PHP thread running
/// the bootstrap script, whose `start()` runs `run(context)`.
unsafe fn start_thread(thread: *mut c_void, run: WorkerMain, context: *mut c_void) {
    MAIN.set(Some((run, context)));
    // SAFETY: `prepare` set it before the server started threads; `release`
    // frees it once `start()` returned, after every thread ended.
    let start = unsafe { &*START.load(Ordering::Acquire) };
    if !start.argv.is_null() {
        // SAFETY: a live thread array; the thread releases its reference.
        unsafe { swoole_rs_phpthread::swr_v1_phpthread_array_add_ref(start.argv) };
    }
    // SAFETY: a live persistent string.
    let script = unsafe { bytes(start.bootstrap) };
    // SAFETY: the script's bytes, an array reference the thread takes, and the
    // thread's live `swoole::Thread`, which `entered` and `left` report to.
    unsafe {
        swoole_rs_phpthread::swr_v1_phpthread_run(
            script.as_ptr().cast(),
            script.len(),
            start.argv,
            thread,
            entered,
            left,
        );
    }
}

/// `Thread::enter()`.
extern "C" fn entered(thread: *mut c_void) {
    // SAFETY: the live `swoole::Thread` `start_thread` was given.
    unsafe { &*thread.cast::<Thread>() }.enter();
}

/// `Thread::exit(status)`.
extern "C" fn left(thread: *mut c_void, status: c_int) {
    // SAFETY: as in `entered`.
    unsafe { &*thread.cast::<Thread>() }.exit(status);
}

/// After `start()`: the bootstrap script and arguments go.
pub(crate) fn release() {
    set_worker_thread_start(None);
    release_start(START.swap(ptr::null_mut(), Ordering::AcqRel));
}

fn release_start(start: *mut Start) {
    if start.is_null() {
        return;
    }
    // SAFETY: made by `prepare`'s `Box::into_raw`, now unshared.
    let start = unsafe { Box::from_raw(start) };
    // SAFETY: the persistent copy `prepare` made.
    unsafe { swr_zend_string_release(start.bootstrap) };
    if !start.argv.is_null() {
        // SAFETY: the reference `init_arguments` took.
        unsafe { swoole_rs_phpthread::swr_v1_phpthread_array_release(start.argv) };
    }
}

/// `ZEND_HASH_APPLY_KEEP` and `ZEND_HASH_APPLY_REMOVE`.
const APPLY_KEEP: c_int = 0;
const APPLY_REMOVE: c_int = 1;

unsafe extern "C" fn serializable(value: *mut zval) -> c_int {
    // SAFETY: a live element of the copy.
    if unsafe { swoole_rs_zend::Value::from_ptr(value) }
        .is_none_or(swoole_rs_phpswoole::sw_zval_is_serializable)
    {
        APPLY_KEEP
    } else {
        APPLY_REMOVE
    }
}

/// `copy_setting`: the serializable settings, serialized for the worker
/// threads to read back.
pub(crate) fn copy_setting(serv: *mut c_void, setting: *mut zval) -> Result<(), Bailout> {
    // SAFETY: its callers pass their live server.
    let serv = unsafe { view::Server::from_raw(serv) };
    let mut copy = crate::dispatch::undef();
    // SAFETY: the settings array.
    let source = unsafe { table(setting) };
    // SAFETY: as above, duplicated.
    let array = unsafe { zend_array_dup(source) };
    // SAFETY: the fresh copy, filtered in place.
    unsafe { zend_hash_apply(array, Some(serializable)) };
    copy.value = zend_value { arr: array };
    copy.u1.type_info = IS_ARRAY_EX;
    // SAFETY: a live array this function owns.
    let value = unsafe { &*(&raw const copy).cast::<swoole_rs_zend::Value>() };
    let (serialized, bailed) = swoole_rs_phpswoole::serialize(value);
    // SAFETY: the copy this function owns.
    unsafe { zval_ptr_dtor(&raw mut copy) };
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    unsafe { serv.config_mut() }.private_data_4 = serialized.cast();
    bailed
}

/// Before the Server is deleted: the serialized settings go.
pub(crate) fn free_setting(serv: *mut c_void) {
    // SAFETY: its callers pass their live server.
    let serv = unsafe { view::Server::from_raw(serv) };
    if !thread_mode(serv.raw()) {
        return;
    }
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    let config = unsafe { serv.config_mut() };
    let serialized = std::mem::replace(&mut config.private_data_4, ptr::null_mut());
    if !serialized.is_null() {
        // SAFETY: the persistent string `copy_setting` made.
        unsafe { swr_zend_string_release(serialized.cast()) };
    }
}
