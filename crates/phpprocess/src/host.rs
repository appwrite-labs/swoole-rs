//! What `Process` and `Process\Pool` reach outside the crate: Swoole's globals, the
//! server, callables, the reactor and signals (was `ext-src/swoole_server.cc`'s
//! `process_host` table).
use std::{
    cell::Cell,
    ffi::{c_int, c_void},
    ptr,
};
use swoole_rs_corebase::{
    swr_v1_core_server, swr_v1_core_signal_listener_num, thread_async_threads,
};
use swoole_rs_phpcoroutines::create_coroutine;
use swoole_rs_phphelpers::{callable, function_call};
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_process::{methods, worker_record::WorkerRecord};
use swoole_rs_server::view;
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{
        swr_zend_callable_warning, swr_zend_tsrm_is_main_thread, zend_class_entry, zend_object,
        zval,
    },
};

/// A `swSignalHandler`: a function's address, `SIG_IGN` or 0 (none).
type Handler = usize;

unsafe extern "C" {
    fn php_swoole_reactor_init() -> c_int;
    fn swr_v1_phpevents_wait();
    fn php_swoole_convert_to_fd(value: *mut zval) -> c_int;
    #[link_name = "_Z17swoole_signal_setiPFviEb"]
    fn swoole_signal_set(signo: c_int, handler: usize, safety: bool) -> usize;
}

/// `SW_MASTER`, `SW_TASK_WORKER`, `SW_MANAGER` and `SW_USER_WORKER`.
const MASTER: c_int = 1;
const TASK_WORKER: c_int = 4;
const MANAGER: c_int = 3;
const USER_WORKER: c_int = 5;
/// `SW_SOCK_UNIX_STREAM` and `SW_SOCK_UNIX_DGRAM`.
const SOCK_UNIX_STREAM: c_int = 5;
const SOCK_UNIX_DGRAM: c_int = 6;
/// `Reactor::EXIT_CONDITION_SIGNAL_LISTENER` and `SW_ERROR_WRONG_OPERATION`.
const EXIT_CONDITION_SIGNAL_LISTENER: c_int = 4;
const WRONG_OPERATION: c_int = 509;

/// `swoole_exception_ce`.
pub(crate) fn exception() -> *mut zend_class_entry {
    swoole_rs_phpswoole::exception_class()
}

/// `swoole_server_ce`, registered after Process.
pub(crate) fn server_class() -> *mut zend_class_entry {
    swoole_rs_phpswoole::families()
        .map_or(std::ptr::null_mut(), |families| (families.server_class)())
}

/// The running server, unless it is null.
fn server() -> Option<view::Server<'static>> {
    let serv = swr_v1_core_server();
    // SAFETY: `sw_server()`, live until the process deletes it at shutdown.
    (!serv.is_null()).then(|| unsafe { view::Server::from_raw(serv) })
}

/// Who may construct a Process: 0, or 1 in a started server's master, 2 once
/// async-io threads exist.
pub(crate) fn construct_guard() -> c_int {
    if server().is_some_and(view::Server::is_started) && methods::swoole_get_worker_type() == MASTER
    {
        return 1;
    }
    if thread_async_threads().is_null() {
        0
    } else {
        2
    }
}

/// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
pub(crate) fn unconstructed() {
    // SAFETY: the frames it may jump over own nothing.
    unsafe { swoole_rs_corebase::fatal_error(WRONG_OPERATION, c"must call constructor first") };
}

thread_local! {
    /// `server_user_worker_id`: the next user worker a thread-mode Process takes.
    static USER_WORKER_ID: Cell<u32> = const { Cell::new(0) };
}

/// In a server worker thread, `worker` becomes a copy of its server worker
/// (shared); false elsewhere.
/// # Safety
/// `worker` is a writable `Worker`.
pub(crate) unsafe fn worker_share(worker: *mut c_void) -> bool {
    let Some(serv) = server().filter(|&serv| serv.is_worker_thread()) else {
        return false;
    };
    let user = methods::swoole_get_worker_type() == USER_WORKER;
    let id = if user {
        methods::swoole_get_worker_id()
    } else {
        let config = serv.config();
        let id = USER_WORKER_ID.get();
        USER_WORKER_ID.set(id + 1);
        id + config.worker_num + config.task_worker_num
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::get_worker takes a uint16_t"
    )]
    let source = serv.worker(id as u16);
    let process = worker.cast::<WorkerRecord>();
    // SAFETY: two live, distinct worker records.
    unsafe { ptr::copy_nonoverlapping(source, process, 1) };
    // SAFETY: the record just written.
    let process = unsafe { &mut *process };
    process.pipe_current = if user {
        process.pipe_worker
    } else {
        process.pipe_master
    };
    process.shared = true;
    true
}

/// The id a Process takes after the server's workers: `get_all_worker_num()`
/// once started, else 1.
pub(crate) fn worker_base() -> u32 {
    let Some(serv) = server() else { return 1 };
    if !serv.is_started() {
        return 1;
    }
    let config = serv.config();
    let users = serv.user_worker_num();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "worker counts fit a uint32_t"
    )]
    let users = users as u32;
    config.worker_num + config.task_worker_num + users
}

/// `php_swoole_reactor_init()`.
pub(crate) fn reactor_init() -> c_int {
    // SAFETY: phpevents' init, from a PHP call.
    unsafe { php_swoole_reactor_init() }
}

/// `php_swoole_event_wait()`.
pub(crate) fn event_wait() {
    // SAFETY: as above.
    unsafe { swr_v1_phpevents_wait() };
}

/// `php_swoole_dup_socket(fd, SW_SOCK_UNIX_STREAM or _DGRAM)`.
pub(crate) fn dup_socket(fd: c_int, stream: bool) -> *mut zend_object {
    let kind = if stream {
        SOCK_UNIX_STREAM
    } else {
        SOCK_UNIX_DGRAM
    };
    swoole_rs_phpclients::dup(fd, kind)
}

/// `php_swoole_convert_to_fd(value)`.
/// # Safety
/// `value` is a live zval.
pub(crate) unsafe fn convert_to_fd(value: *mut zval) -> c_int {
    // SAFETY: per the caller.
    unsafe { php_swoole_convert_to_fd(value) }
}

/// The value, or `None` with the bailout deferred: the caller stops on
/// `bailout::pending()`, and its handler or export's last step raises it.
fn deferred<T>(result: Result<T, Bailout>) -> Option<T> {
    result.map_err(Bailout::defer).ok()
}

/// `sw_callable_create(value)`: a callable, or null after a warning or with a
/// bailout pending.
/// # Safety
/// `value` is a live zval.
pub(crate) unsafe fn callable_create(value: *mut zval) -> *mut c_void {
    // SAFETY: per the caller; the reporter is C.
    let created = unsafe {
        callable::factory(
            value,
            ptr::null(),
            true,
            exception(),
            swr_zend_callable_warning,
        )
    };
    deferred(created).map_or(ptr::null_mut(), <*mut callable::Callable>::cast)
}

/// `sw_callable_free(callable)`, which is gone even when its destructor leaves
/// a bailout pending.
/// # Safety
/// `callable` is one [`callable_create`] made, freed once and no longer held.
pub(crate) unsafe fn callable_free(callable: *mut c_void) {
    // SAFETY: per the caller.
    bailout::defer(unsafe { callable::release(callable.cast()) });
}

/// The deferred `sw_callable_free`, the outermost Rust frame of its call.
unsafe extern "C-unwind" fn deferred_free(callable: *mut c_void) {
    // SAFETY: the callable `callable_defer_free` handed over.
    unsafe { callable_free(callable) };
    bailout::finish();
}

/// `swoole_event_defer(sw_callable_free, callable)`.
/// # Safety
/// As [`callable_free`], with an event loop on this thread.
pub(crate) unsafe fn callable_defer_free(callable: *mut c_void) {
    defer(deferred_free, callable);
}

/// `Reactor::defer(call, data)` on this thread's loop.
fn defer(call: swoole_rs_reactor::DeferCall, data: *mut c_void) {
    let reactor = swoole_rs_reactor::event_api::swr_v1_event_current()
        .cast::<swoole_rs_reactor::ReactorPrepared>();
    // SAFETY: the live loop, which begins with its prepared state.
    let fields = unsafe { (*reactor).fields };
    // SAFETY: the loop's live fields.
    unsafe { swoole_rs_reactor::defer(fields, call, data, None) };
}

/// `zend::function::call(callable->ptr(), argc, argv, nullptr, coroutine)`:
/// false with a bailout pending is one the caller stops on.
/// # Safety
/// `callable` is live and `argv` holds `argc` zvals.
pub(crate) unsafe fn callable_call(
    callable: *mut c_void,
    argc: u32,
    argv: *mut zval,
    coroutine: bool,
) -> bool {
    // SAFETY: per the caller.
    let cache = unsafe { callable::cache(callable.cast()) };
    // SAFETY: per the caller; the leaves are phpcoroutines' and C's.
    let called = unsafe {
        function_call(
            cache,
            argc,
            argv,
            ptr::null_mut(),
            coroutine,
            create_coroutine,
            swr_zend_callable_warning,
        )
    };
    deferred(called).unwrap_or(false)
}

/// `php_swoole_is_enable_coroutine()`: the running server's
/// `Server::is_enable_coroutine()`, else `SwooleG.enable_coroutine`.
#[unsafe(export_name = "php_swoole_is_enable_coroutine")]
#[must_use]
pub extern "C" fn enable_coroutine() -> bool {
    let Some(serv) = server() else {
        return swoole_rs_corebase::swr_v1_core_enable_coroutine();
    };
    match methods::swoole_get_worker_type() {
        TASK_WORKER => task_coroutine(serv),
        MANAGER => false,
        _ => serv.config().enable_coroutine,
    }
}

/// `tsrm_is_main_thread()`, true without threads.
pub(crate) fn main_thread() -> bool {
    !built_with(Switch::Thread) || {
        // SAFETY: no preconditions.
        unsafe { swr_zend_tsrm_is_main_thread() }
    }
}

/// `swoole_signal_set(signo, handler, safety)`.
fn set_signal(signo: c_int, handler: Handler, safety: bool) {
    // SAFETY: a signal number and a handler or none.
    unsafe { swoole_signal_set(signo, handler, safety) };
}

unsafe extern "C-unwind" fn deferred_signal(data: *mut c_void) {
    // SAFETY: the box `signal_set` handed over.
    let (signo, handler, safety) = *unsafe { Box::from_raw(data.cast::<(c_int, Handler, bool)>()) };
    set_signal(signo, handler, safety);
}

/// `swoole_signal_set(signo, handler, safety)`, deferred to the event loop
/// under the thread context.
pub(crate) fn signal_set(signo: c_int, handler: Handler, safety: bool) {
    if built_with(Switch::ThreadContext) {
        defer(
            deferred_signal,
            Box::into_raw(Box::new((signo, handler, safety))).cast(),
        );
    } else {
        set_signal(signo, handler, safety);
    }
}

/// The running server's `task_enable_coroutine`.
fn task_coroutine(serv: view::Server<'_>) -> bool {
    serv.config().task_enable_coroutine
}

/// `Server::is_sync_process()` of the running server.
pub(crate) fn sync_server() -> bool {
    server().is_some_and(|serv| match methods::swoole_get_worker_type() {
        MANAGER => true,
        TASK_WORKER => !task_coroutine(serv),
        _ => false,
    })
}

/// Whether a timer added in this process runs on its reactor: a server's user
/// workers and coroutine task workers, a pool's async or loopless workers, and any
/// process outside both.
#[must_use]
pub fn timer_uses_reactor() -> bool {
    let kind = methods::swoole_get_worker_type();
    match server() {
        Some(serv) => kind == USER_WORKER || (kind == TASK_WORKER && task_coroutine(serv)),
        None => crate::pool::timer_uses_reactor(kind == MASTER).unwrap_or(true),
    }
}

extern "C-unwind" fn listening(_event_num: &mut usize) -> bool {
    swr_v1_core_signal_listener_num() == 0 || !swoole_rs_corebase::swr_v1_core_wait_signal()
}

/// `php_swoole_check_reactor()`, then the loop runs while a signal listener
/// waits (setting it again replaces it with the same condition).
pub(crate) fn signal_listen() {
    swoole_rs_phpswoole::check_reactor();
    swoole_rs_reactor::swr_v1_event_set_exit_condition(EXIT_CONDITION_SIGNAL_LISTENER, listening);
}

/// Outside thread builds, a process that is not a user worker forgets its
/// worker type.
pub(crate) fn reset_worker_type() {
    if !built_with(Switch::Thread) && methods::swoole_get_worker_type() != USER_WORKER {
        methods::swoole_set_worker_type(0);
    }
}
