//! What `Process` and `Process\Pool` reach outside the crate: Swoole's globals, the
//! server, callables, the reactor and signals (was `ext-src/swoole_server.cc`'s
//! `process_host` table).
use std::{
    cell::Cell,
    ffi::c_int,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::{swr_v1_core_signal_listener_num, thread_async_threads};
use swoole_rs_phpcoroutines::call_handler_lent;
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_process::worker_record::WorkerType;
use swoole_rs_process::{methods, worker_record::Worker};
use swoole_rs_reactor::event_api::{Deferred, swoole_event_defer_value};
use swoole_rs_server::view;
use swoole_rs_zend::host::callable;
use swoole_rs_zend::{
    Owned, Value,
    bailout::{self, Bailout},
};

use swoole_rs_reactor::signal_upstream::{self, Handler};

// crates/phpevents' functions: declared, since crates/phpevents depends on this crate.
// Safe as crates/phpcoroutines declares them: each takes nothing or a live value
// and is called from a PHP call.
unsafe extern "C" {
    safe fn php_swoole_reactor_init() -> c_int;
    safe fn swr_v1_phpevents_wait();
    safe fn php_swoole_convert_to_fd(value: Option<&Value>) -> c_int;
}

/// `SW_SOCK_UNIX_STREAM` and `SW_SOCK_UNIX_DGRAM`.
const SOCK_UNIX_STREAM: c_int = 5;
const SOCK_UNIX_DGRAM: c_int = 6;
/// `SW_ERROR_WRONG_OPERATION`.
const WRONG_OPERATION: c_int = 509;

/// `f` over the process's server (`sw_server()`), or `None` while it has none.
/// Each `f` here reads the server's config and worker records and returns; none
/// reaches `swr_v1_server_delete`, and the view can't outlive the call.
fn with_server<R>(f: impl FnOnce(view::Server<'_>) -> R) -> Option<R> {
    // SAFETY: nothing deletes the server while `f`, a read, runs.
    unsafe { view::Server::current() }.map(f)
}

/// Why this process may not construct a Process.
pub(crate) enum ConstructRefusal {
    /// A started server's master.
    Master,
    /// Async-io threads exist.
    AsyncThreads,
}

/// Whether this process may construct a Process.
pub(crate) fn construct_guard() -> Result<(), ConstructRefusal> {
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the method's path fixes the view's lifetime; with_server's `f` takes any"
    )]
    let started = with_server(|serv| serv.is_started()).unwrap_or(false);
    if started && methods::worker_type() == Some(WorkerType::Master) {
        return Err(ConstructRefusal::Master);
    }
    if thread_async_threads().is_null() {
        Ok(())
    } else {
        Err(ConstructRefusal::AsyncThreads)
    }
}

/// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`,
/// whose bailout the handler raises last.
pub(crate) fn unconstructed() {
    swoole_rs_zend::bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
        WRONG_OPERATION,
        c"must call constructor first",
    ));
}

thread_local! {
    /// `server_user_worker_id`: the next user worker a thread-mode Process takes.
    static USER_WORKER_ID: Cell<u32> = const { Cell::new(0) };
}

/// In a server worker thread, `process` becomes a copy of its server worker
/// (shared); false elsewhere, or past the server's last worker.
pub(crate) fn worker_share(process: Worker<'_>) -> bool {
    with_server(|serv| serv.is_worker_thread() && share(serv, process)).unwrap_or(false)
}

/// [`worker_share`] in a worker thread of `serv`.
fn share(serv: view::Server<'_>, process: Worker<'_>) -> bool {
    let user = methods::worker_type() == Some(WorkerType::User);
    let id = if user {
        methods::swoole_get_worker_id()
    } else {
        let config = serv.config();
        let id = USER_WORKER_ID.get();
        USER_WORKER_ID.set(id + 1);
        id + config.worker_num.get() + config.task_worker_num.get()
    };
    let Some(source) = serv.worker_record(id.c_cast::<u16>()) else {
        return false;
    };
    process.put(source.get());
    process.use_pipe(!user);
    process.set_shared(true);
    true
}

/// The id a Process takes after the server's workers: `get_all_worker_num()`
/// once started, else 1.
pub(crate) fn worker_base() -> u32 {
    with_server(|serv| {
        if !serv.is_started() {
            return 1;
        }
        let config = serv.config();
        let users = serv.user_worker_num();
        let users = users.c_cast::<u32>();
        config.worker_num.get() + config.task_worker_num.get() + users
    })
    .unwrap_or(1)
}

/// `php_swoole_reactor_init()`.
pub(crate) fn reactor_init() -> c_int {
    php_swoole_reactor_init()
}

/// `php_swoole_event_wait()`.
pub(crate) fn event_wait() {
    swr_v1_phpevents_wait();
}

/// `php_swoole_dup_socket(fd, SW_SOCK_UNIX_STREAM or _DGRAM)`.
pub(crate) fn dup_socket(fd: c_int, stream: bool) -> Option<Owned> {
    let kind = if stream {
        SOCK_UNIX_STREAM
    } else {
        SOCK_UNIX_DGRAM
    };
    swoole_rs_phpclients::dup(fd, kind)
}

/// `php_swoole_convert_to_fd(value)`.
pub(crate) fn convert_to_fd(value: &Value) -> c_int {
    php_swoole_convert_to_fd(Some(value))
}

/// The value, or `None` with the bailout deferred: the caller stops on
/// `bailout::pending()`, and its handler or export's last step raises it.
fn deferred<T>(result: Result<T, Bailout>) -> Option<T> {
    result.map_err(Bailout::defer).ok()
}

/// A callable `sw_callable_create` made, freed (`sw_callable_free`) on drop,
/// which leaves a destructor's bailout pending. Its only owner: the `Rc` is
/// never cloned.
pub(crate) struct Callable(Rc<callable::Callable>);

impl Callable {
    /// `sw_callable_create(value)`, or `None` after a warning or with a bailout
    /// pending.
    pub(crate) fn create(value: &Value) -> Option<Self> {
        deferred(callable::Callable::new(value)).flatten().map(Self)
    }

    /// `zend::function::call(callable->ptr(), argc, argv, nullptr, coroutine)`
    /// with at most two arguments, copied without a reference as upstream
    /// passes them: false with a bailout pending is one the caller stops on.
    pub(crate) fn call(&self, args: &[&Value], coroutine: bool) -> bool {
        call(&self.0, args, coroutine)
    }
}

/// [`Callable::call`] of the callable itself.
fn call(callable: &callable::Callable, args: &[&Value], coroutine: bool) -> bool {
    let mut argv = [swoole_rs_zend::UNDEF; 2];
    for (slot, arg) in argv.iter_mut().zip(args) {
        // Copied bitwise; the copy is never released.
        *slot = arg.copy_value();
    }
    let (argv, _) = argv.split_at_mut(args.len().min(2));
    deferred(call_handler_lent(callable, argv, None, coroutine)).unwrap_or(false)
}

/// A callable a static keeps (a signal's): null, or the `Rc` of a [`Callable`]
/// that [`Slot::replace`], the only writer, gave up to it.
pub(crate) struct Slot(AtomicPtr<callable::Callable>);

impl Slot {
    /// A slot with no callable.
    pub(crate) const fn empty() -> Self {
        Self(AtomicPtr::new(ptr::null_mut()))
    }

    /// Whether a callable is set.
    pub(crate) fn is_set(&self) -> bool {
        !self.0.load(Ordering::Acquire).is_null()
    }

    /// Store `callable` (`None`: empty the slot), and take what was there.
    pub(crate) fn replace(&self, callable: Option<Callable>) -> Option<Callable> {
        let new = callable.map_or(ptr::null(), |callable| Rc::into_raw(callable.0));
        let old = self.0.swap(new.cast_mut(), Ordering::AcqRel);
        // SAFETY: null or the `Rc` a `replace` gave up, which the swap took out of
        // the slot, so it is taken back once.
        (!old.is_null()).then(|| Callable(unsafe { Rc::from_raw(old) }))
    }

    /// [`Callable::call`] of the slot's callable, which the slot keeps: `None`
    /// when it has none.
    pub(crate) fn call(&self, args: &[&Value], coroutine: bool) -> Option<bool> {
        let stored = self.0.load(Ordering::Acquire);
        if stored.is_null() {
            return None;
        }
        // SAFETY: the `Rc` `replace` stored, still the slot's, borrowed without
        // taking it (the slot keeps its reference).
        let stored = std::mem::ManuallyDrop::new(unsafe { Rc::from_raw(stored) });
        // A reference of its own, so the callable outlives a handler that replaces it.
        let callable = Rc::clone(&*stored);
        Some(call(&callable, args, coroutine))
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
        #[expect(
            clippy::mem_forget,
            reason = "a loop freed first leaks it, as upstream's defer does"
        )]
        std::mem::forget(self.0);
    }
}

/// `swoole_event_defer(sw_callable_free, callable)` on this thread's loop;
/// freed now without one.
pub(crate) fn callable_defer_free(callable: Callable) {
    _ = swoole_event_defer_value(DeferredFree(callable));
}

/// `php_swoole_is_enable_coroutine()`: the running server's
/// `Server::is_enable_coroutine()`, else `SwooleG.enable_coroutine.get()`.
#[unsafe(export_name = "php_swoole_is_enable_coroutine")]
#[must_use]
pub extern "C" fn enable_coroutine() -> bool {
    with_server(|serv| match methods::worker_type() {
        Some(WorkerType::Task) => task_coroutine(serv),
        Some(WorkerType::Manager) => false,
        _ => serv.config().enable_coroutine.get(),
    })
    .unwrap_or_else(|| swoole_rs_corebase::swr_v1_core_enable_coroutine())
}

/// `tsrm_is_main_thread()`, true without threads.
pub(crate) fn main_thread() -> bool {
    !built_with(Switch::Thread) || swoole_rs_zend::is_main_thread()
}

/// `swoole_signal_set(signo, handler, safety)`: the handler it replaced.
pub(crate) fn signal_swap(signo: c_int, handler: Handler, safety: bool) -> Handler {
    signal_upstream::set(signo, handler, safety)
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
    serv.config().task_enable_coroutine.get()
}

/// `Server::is_sync_process()` of the running server.
#[must_use]
pub fn sync_server() -> bool {
    with_server(|serv| match methods::worker_type() {
        Some(WorkerType::Manager) => true,
        Some(WorkerType::Task) => !task_coroutine(serv),
        _ => false,
    })
    .unwrap_or(false)
}

/// Whether a timer added in this process runs on its reactor: a server's user
/// workers and coroutine task workers, a pool's async or loopless workers, and any
/// process outside both.
#[must_use]
pub fn timer_uses_reactor() -> bool {
    let kind = methods::worker_type();
    with_server(|serv| match kind {
        Some(WorkerType::User) => true,
        Some(WorkerType::Task) => task_coroutine(serv),
        _ => false,
    })
    .unwrap_or_else(|| {
        crate::pool::timer_uses_reactor(kind == Some(WorkerType::Master)).unwrap_or(true)
    })
}

extern "C-unwind" fn listening(_event_num: &mut usize) -> bool {
    swr_v1_core_signal_listener_num() == 0 || !swoole_rs_corebase::swr_v1_core_wait_signal()
}

/// `php_swoole_check_reactor()`, then the loop runs while a signal listener
/// waits (setting it again replaces it with the same condition).
pub(crate) fn signal_listen() {
    swoole_rs_phpswoole::check_reactor();
    swoole_rs_reactor::swr_v1_event_set_exit_condition(
        swoole_rs_reactor::ExitCondition::SignalListener,
        listening,
    );
}

/// Outside thread builds, a process that is not a user worker forgets its
/// worker type.
pub(crate) fn reset_worker_type() {
    if !built_with(Switch::Thread) && methods::worker_type() != Some(WorkerType::User) {
        methods::swoole_set_worker_type(0);
    }
}
