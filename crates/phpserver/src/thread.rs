//! A thread-mode Server (`SW_THREAD`): the settings its worker threads share,
//! the PHP thread each worker runs in, and `start()` inside that thread, which
//! runs the worker's body.
use crate::lifecycle::{MODE_THREAD, Props, ServerObject};
use std::{
    cell::Cell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
};
use swoole_rs_corebase::Thread;
use swoole_rs_phpthread::Arguments;
use swoole_rs_process::workers::thread::{WorkerMain, set_worker_thread_start};
use swoole_rs_server::view;
use swoole_rs_zend::{
    Key, Owned, OwnedArray, Value,
    bailout::{self, Bailout},
    kind,
    sys::{IS_NULL, swr_zend_thread_main_request, zend_fcall_info},
};

/// What every worker thread starts its PHP request with: the bootstrap script
/// and the `init_arguments` result.
struct Start {
    bootstrap: Vec<u8>,
    argv: Option<Arguments>,
}

/// The running thread-mode server's [`Start`], from `prepare` until `release`.
static START: Mutex<Option<Arc<Start>>> = Mutex::new(None);

/// The settings `copy_setting` serialized for the worker threads, until `free_setting`.
static SETTING: Mutex<Option<Arc<[u8]>>> = Mutex::new(None);

/// Replace the serialized settings.
fn set_setting(setting: Option<Arc<[u8]>>) {
    *SETTING.lock().unwrap_or_else(PoisonError::into_inner) = setting;
}

/// Replace the running server's [`Start`].
fn set_start(start: Option<Start>) {
    *START.lock().unwrap_or_else(PoisonError::into_inner) = start.map(Arc::new);
}

thread_local! {
    /// Upstream's `worker_thread_fn`: this worker thread's body.
    static MAIN: Cell<Option<(WorkerMain, *mut c_void)>> = const { Cell::new(None) };
}

/// Thread mode, in a build with `SW_THREAD`.
fn thread_mode(serv: view::Server<'_>) -> bool {
    crate::server::THREAD.load(Ordering::Relaxed) && i64::from(serv.config().mode_) == MODE_THREAD
}

/// `__construct`'s check: 0 no server yet, 1 this worker thread's server
/// (in `existing`), 2 not the main thread, 3 a server already exists.
pub(crate) fn admit(existing: &mut Option<view::Server<'static>>) -> c_int {
    // SAFETY: the process's server, which outlives every request.
    let current = unsafe { view::Server::current() };
    if !crate::server::THREAD.load(Ordering::Relaxed) {
        return if current.is_none() { 0 } else { 3 };
    }
    if let Some(server) = current
        && server.is_worker_thread()
    {
        *existing = Some(server);
        return 1;
    }
    if swoole_rs_zend::is_main_thread() {
        0
    } else {
        2
    }
}

/// `start()` on a worker thread: the settings the master serialized, this
/// thread's coroutine options, then the worker's body. `None`: not a worker
/// thread.
pub(crate) fn worker(object: ServerObject, serv: view::Server<'_>) -> Option<Result<(), Bailout>> {
    if !serv.is_worker_thread() {
        return None;
    }
    Some(run_worker(object))
}

fn run_worker(object: ServerObject) -> Result<(), Bailout> {
    let text = SETTING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let text = text.as_deref().unwrap_or_default();
    // `$this->setting`, which upstream releases and unserializes in place: it is
    // released first, and nothing is written once the unserialize bailed out.
    Props::own(object).set(c"setting", &Owned::null());
    let (setting, bailed) = swoole_rs_phpswoole::unserialize(text);
    bailed?;
    Props::own(object).set(c"setting", &setting);
    // The coroutine options are thread-local: each worker thread sets them once.
    if let Some(options) = setting.array() {
        swoole_rs_phpcoroutines::set_coroutine_option(options);
    }
    // A conversion's bailout ends the start here, as upstream's did.
    if bailout::pending() {
        return Err(Bailout);
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
pub(crate) fn prepare(object: ServerObject, serv: view::Server<'_>) -> Result<(), Bailout> {
    if !thread_mode(serv) {
        return Ok(());
    }
    let mut this = object.zval();
    let this = Value::of(&mut this);
    // The Server class declares `bootstrap`; read again after the write.
    let bootstrap = || {
        this.property(c"bootstrap")
            .map_or(&[][..], string_of)
            .to_vec()
    };
    if bootstrap().is_empty() {
        let (mut path, mut argc): (*const c_char, c_int) = (ptr::null(), 0);
        // SAFETY: two writable out-parameters.
        unsafe { swr_zend_thread_main_request(&raw mut path, &raw mut argc) };
        if !path.is_null() {
            // SAFETY: the request's NUL-terminated script.
            let path = unsafe { CStr::from_ptr(path) };
            Props::own(object).string(c"bootstrap", path);
        }
    }
    let bootstrap = bootstrap();
    let argv = init_arguments(object)?;
    set_start(Some(Start { bootstrap, argv }));
    set_worker_thread_start(Some(start_thread));
    let flags = swoole_rs_phpcoroutines::hook_flags();
    if flags > 0 {
        swoole_rs_phpruntime::enable_hook(flags)?;
    }
    Ok(())
}

/// A string property's bytes (upstream reads it as a string); empty otherwise.
fn string_of(slot: &Value) -> &[u8] {
    slot.bytes().unwrap_or_default()
}

/// `init_arguments()`'s array, copied for the threads.
fn init_arguments(object: ServerObject) -> Result<Option<Arguments>, Bailout> {
    let callable = object.init_arguments();
    if kind(&callable) == IS_NULL {
        return Ok(None);
    }
    let result = Owned::default();
    let mut fci = swoole_rs_zend::EMPTY_FCI;
    fci.size = size_of::<zend_fcall_info>();
    fci.function_name = callable;
    fci.retval = result.as_ptr();
    // SAFETY: `call_user_function(NULL, nullptr, &init_arguments, &result, 0, nullptr)`.
    unsafe { bailout::call(&raw mut fci, ptr::null_mut()) }?;
    result.array().map(Arguments::copy).transpose()
}

/// The thread starter: this `ThreadFactory` thread becomes a PHP thread running
/// the bootstrap script, whose `start()` runs `run(context)`.
unsafe fn start_thread(thread: *mut c_void, run: WorkerMain, context: *mut c_void) {
    MAIN.set(Some((run, context)));
    // `prepare` set it before the server started threads; `release` clears it
    // once `start()` returned, after every thread ended.
    let start = START.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(start) = start else { return };
    // SAFETY: the factory's live `swoole::Thread` for this thread.
    let thread = unsafe { &*thread.cast::<Thread>() };
    // SAFETY: the factory's new OS thread, before anything of PHP's.
    unsafe {
        swoole_rs_phpthread::run(
            &start.bootstrap,
            start.argv.clone(),
            || thread.enter(),
            |status| thread.exit(status),
        );
    }
}

/// After `start()`: the bootstrap script and arguments go.
pub(crate) fn release() {
    set_worker_thread_start(None);
    set_start(None);
}

/// `copy_setting`: the serializable settings, serialized for the worker
/// threads to read back.
pub(crate) fn copy_setting(setting: &Value) -> Result<(), Bailout> {
    // `zend_array_dup` of the settings, without the elements that don't serialize.
    let mut copy = OwnedArray::default();
    if let Some(setting) = setting.array() {
        let mut entries = setting.entries();
        while let Some((key, value)) = entries.next_keyed() {
            if !swoole_rs_phpswoole::sw_zval_is_serializable(value) {
                continue;
            }
            match key {
                Key::Name(name) => copy.insert(name, value.copy()),
                Key::Index(index) => copy.insert_index(index, value.copy()),
            }
        }
    }
    let copy = copy.into_owned();
    let (serialized, bailed) = swoole_rs_phpswoole::serialize(&copy);
    drop(copy);
    set_setting(serialized.map(Arc::from));
    bailed
}

/// Before the Server is deleted: the serialized settings go.
pub(crate) fn free_setting(serv: view::Server<'_>) {
    if !thread_mode(serv) {
        return;
    }
    set_setting(None);
}
