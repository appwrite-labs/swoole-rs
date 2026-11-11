//! What `Process` and `Process\Pool` reach outside the crate: Swoole's globals, the
//! server, callables, the reactor and signals (was `ext-src/swoole_server.cc`'s
//! `process_host` table).
use std::{
    cell::Cell,
    ffi::{c_int, c_void},
    ptr::{self, NonNull},
};
use swoole_rs_corebase::{
    swr_v1_core_server, swr_v1_core_signal_listener_num, thread_async_threads,
};
use swoole_rs_phpcoroutines::create_coroutine;
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_process::{methods, worker_record::WorkerRecord};
use swoole_rs_reactor::event_api::{Deferred, swoole_event_defer_value};
use swoole_rs_server::view;
use swoole_rs_zend::host::{callable, function_call};
use swoole_rs_zend::{
    Value,
    bailout::{self, Bailout},
    sys::{
        swr_zend_callable_warning, swr_zend_tsrm_is_main_thread, zend_class_entry, zend_object,
        zval,
    },
};

/// A `swSignalHandler`: a function's address, `SIG_IGN` or 0 (none).
type Handler = usize;

// crates/phpevents' functions: declared, since crates/phpevents depends on this crate.
unsafe extern "C" {
    fn php_swoole_reactor_init() -> c_int;
    fn swr_v1_phpevents_wait();
    fn php_swoole_convert_to_fd(value: *mut zval) -> c_int;
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

/// In a server worker thread, `process` becomes a copy of its server worker
/// (shared); false elsewhere.
pub(crate) fn worker_share(process: &mut WorkerRecord) -> bool {
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
    // SAFETY: the server's live worker record, distinct from `process`.
    *process = unsafe { source.read() };
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
pub(crate) fn convert_to_fd(value: &Value) -> c_int {
    // SAFETY: a live zval.
    unsafe { php_swoole_convert_to_fd(value.as_ptr()) }
}

/// The value, or `None` with the bailout deferred: the caller stops on
/// `bailout::pending()`, and its handler or export's last step raises it.
fn deferred<T>(result: Result<T, Bailout>) -> Option<T> {
    result.map_err(Bailout::defer).ok()
}

/// A callable `sw_callable_create` made, freed (`sw_callable_free`) on drop,
/// which leaves a destructor's bailout pending.
pub(crate) struct Callable(NonNull<callable::Callable>);

impl Callable {
    /// `sw_callable_create(value)`, or `None` after a warning or with a bailout
    /// pending.
    pub(crate) fn create(value: &Value) -> Option<Self> {
        // SAFETY: a live zval; the reporter is C.
        let created = unsafe {
            callable::factory(
                value.as_ptr(),
                ptr::null(),
                true,
                exception(),
                swr_zend_callable_warning,
            )
        };
        deferred(created).and_then(NonNull::new).map(Self)
    }

    /// Take back a callable [`Callable::into_raw`] gave up (none for null).
    /// # Safety
    /// `raw` is null or a callable `into_raw` gave up, taken back once.
    pub(crate) unsafe fn from_raw(raw: *mut c_void) -> Option<Self> {
        NonNull::new(raw.cast()).map(Self)
    }

    pub(crate) fn into_raw(self) -> *mut c_void {
        std::mem::ManuallyDrop::new(self).0.as_ptr().cast()
    }

    /// `zend::function::call(callable->ptr(), argc, argv, nullptr, coroutine)`
    /// with at most two arguments, copied without a reference as upstream
    /// passes them: false with a bailout pending is one the caller stops on.
    pub(crate) fn call(&self, args: &[&Value], coroutine: bool) -> bool {
        let mut argv = [swoole_rs_zend::UNDEF; 2];
        for (slot, arg) in argv.iter_mut().zip(args) {
            // SAFETY: a live zval, copied bitwise; the copy is never released.
            *slot = unsafe { arg.as_ptr().read() };
        }
        let argc = u32::try_from(args.len().min(argv.len())).unwrap_or(0);
        // SAFETY: a live callable.
        let cache = unsafe { callable::cache(self.0.as_ptr()) };
        // SAFETY: a live cache and `argc` zvals; the leaves are phpcoroutines' and C's.
        let called = unsafe {
            function_call(
                cache,
                argc,
                argv.as_mut_ptr(),
                ptr::null_mut(),
                coroutine,
                create_coroutine,
                swr_zend_callable_warning,
                swoole_rs_zend::host::NO_THEN,
            )
        };
        deferred(called).unwrap_or(false)
    }
}

impl Drop for Callable {
    fn drop(&mut self) {
        // SAFETY: the callable this owns, freed once.
        bailout::defer(unsafe { callable::release(self.0.as_ptr()) });
    }
}

/// A callable `swoole_event_defer(sw_callable_free, ...)` frees once the loop
/// is past any dispatch using it.
struct DeferredFree(Callable);

impl Deferred for DeferredFree {
    /// The outermost Rust frame of its call.
    fn run(self) {
        drop(self.0);
        bailout::finish();
    }

    /// A loop freed first leaks it, as upstream's defer does.
    fn dispose(self) {
        std::mem::forget(self.0);
    }
}

/// `swoole_event_defer(sw_callable_free, callable)` on this thread's loop;
/// freed now without one.
pub(crate) fn callable_defer_free(callable: Callable) {
    _ = swoole_event_defer_value(DeferredFree(callable));
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

/// `swoole_signal_set(signo, handler, safety)`: the handler it replaced.
pub(crate) fn signal_swap(signo: c_int, handler: Handler, safety: bool) -> Handler {
    // SAFETY: a signal number and a handler or none.
    unsafe { swoole_rs_reactor::signal_upstream::swoole_signal_set(signo, handler, safety) }
}

fn set_signal(signo: c_int, handler: Handler, safety: bool) {
    _ = signal_swap(signo, handler, safety);
}

/// A `swoole_signal_set` the loop runs.
struct DeferredSignal(c_int, Handler, bool);

impl Deferred for DeferredSignal {
    fn run(self) {
        set_signal(self.0, self.1, self.2);
    }

    fn dispose(self) {}
}

/// `swoole_signal_set(signo, handler, safety)`, deferred to the event loop
/// under the thread context.
pub(crate) fn signal_set(signo: c_int, handler: Handler, safety: bool) {
    if built_with(Switch::ThreadContext) {
        if let Err(signal) = swoole_event_defer_value(DeferredSignal(signo, handler, safety)) {
            signal.run();
        }
    } else {
        set_signal(signo, handler, safety);
    }
}

/// The running server's `task_enable_coroutine`.
fn task_coroutine(serv: view::Server<'_>) -> bool {
    serv.config().task_enable_coroutine
}

/// `Server::is_sync_process()` of the running server.
#[must_use]
pub fn sync_server() -> bool {
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
