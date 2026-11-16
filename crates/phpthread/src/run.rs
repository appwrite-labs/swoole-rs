//! A PHP thread's life (upstream's `thread_run`): TSRM, its request and the
//! script, under a bailout that `exit()` anywhere in the thread reaches, then
//! teardown; with the module's MINIT, RINIT and RSHUTDOWN for threads, the
//! running thread count and each thread's arguments. Zend's setjmp, TSRM and
//! request macros are zend-sys shims.
use crate::{Arguments, request, thread};
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_int, c_void},
    marker::PhantomData,
    ptr::{self, NonNull},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{Call, Minit, Owned, OwnedArray, Value, bailout, sys::php_stream};

/// A `php://stdin`, `stdout` or `stderr` stream a thread's prologue opened,
/// before its script runs: no user stream wrapper, error handler or constant
/// exists yet, so closing it, casting it or registering it as a constant runs
/// no PHP code. Only the prologue makes one.
#[repr(transparent)]
struct Stdio(NonNull<php_stream>);

/// A [`Stdio`] lent for one call.
#[repr(transparent)]
struct Lent<'a>(NonNull<php_stream>, PhantomData<&'a mut Stdio>);

impl Stdio {
    /// The stream's descriptor in `fd`: whether the cast succeeded.
    fn fd(&mut self, fd: &mut c_int) -> bool {
        stdio_fd(Lent(self.0, PhantomData), fd)
    }
}

// The prologue's stdio streams (`Stdio`).
unsafe extern "C" {
    /// `php_stream_close`.
    #[link_name = "swr_zend_stream_close"]
    safe fn stdio_close(stream: Stdio);
    /// `php_stream_cast(stream, PHP_STREAM_AS_FD, fd, 0) == SUCCESS`.
    #[link_name = "swr_zend_stream_fd"]
    safe fn stdio_fd(stream: Lent<'_>, fd: &mut c_int) -> bool;
    /// Register the stream's resource as the constant `name`, which takes it.
    #[link_name = "swr_zend_register_stream_constant"]
    safe fn stdio_constant(name: swoole_rs_corebase::sys::Text<'_>, stream: Stdio);
    /// `PHP_STREAM_FLAG_NO_CLOSE` on the stream of the constant `name`, when it
    /// names a file stream: it outlives its resource.
    safe fn swr_zend_stream_constant_no_close(name: Text<'_>);
}

unsafe extern "C" {
    /// `$argv` (a copy of `argv`, which stays the caller's) and `$argc` in the
    /// request's symbol table. Replacing an entry there runs no PHP code: the
    /// prologue's symbol table holds no `$argv` or `$argc` yet.
    #[link_name = "swr_zend_thread_set_argv"]
    safe fn set_argv(argv: &Value, argc: i64);
}

// crates/zend-sys' shims that take nothing and can't run PHP or bail out: the calling
// thread's exit mark and native scopes (`shim_guard.c`), and TSRM's process state.
unsafe extern "C" {
    /// The thread-exit mark: nonzero once `exit()` ran in this thread.
    safe fn swr_zend_guard_pending() -> c_int;
    /// Sets the thread-exit mark.
    safe fn swr_zend_guard_mark();
    /// Clears the thread-exit mark.
    safe fn swr_zend_guard_clear();
    /// `zend_bailout()` to the innermost native scope of the running context
    /// ([`guard_context`]), if there is one; else nothing.
    safe fn swr_zend_guard_bailout();
    /// Whether the thread's script runs, inside its script scope.
    safe fn swr_zend_thread_script_running() -> bool;
    /// `zend_bailout()` to the thread's script scope while its script runs, else
    /// to Zend's current one; this frame owns nothing.
    safe fn swr_zend_thread_script_bailout();
    /// `tsrm_is_shutdown()`; false on NTS.
    safe fn swr_zend_tsrm_is_shutdown() -> bool;
    /// `tsrm_api_name()`: a NUL-terminated literal (empty on NTS).
    safe fn swr_zend_tsrm_api_name() -> swoole_rs_corebase::sys::Text<'static>;
}

/// Opening `php://stdout` and `php://stderr` sets PHP's process-global
/// `cli_out` and `cli_err`: one thread at a time, the one holding this lock.
static OPENING: Mutex<Opening> = Mutex::new(Opening(()));

/// The right to open the stdio streams, which [`OPENING`] holds.
struct Opening(());

impl Opening {
    /// `path` opened in `mode` (`php_stream_open_wrapper_ex(path, mode, 0, NULL,
    /// NULL)`), in a thread's request before its script runs, or none.
    #[expect(
        clippy::unused_self,
        reason = "the receiver proves the caller holds `OPENING`"
    )]
    fn open(&mut self, path: &CStr, mode: &CStr) -> Option<Stdio> {
        unsafe extern "C" {
            /// Opens a stream in the request; only [`OPENING`]'s holder
            /// (`&mut Opening`) opens the stdio streams it shares across threads.
            #[link_name = "swr_zend_stream_open"]
            #[expect(
                clashing_extern_declarations,
                reason = "the same shim, ABI-identical: stream_create's returns its own `NonNull` wrapper"
            )]
            safe fn stdio_open(path: Text<'_>, mode: Text<'_>) -> Option<Stdio>;
        }
        stdio_open(path.into(), mode.into())
    }
}

/// PHP threads running, the main thread included.
static THREADS: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    /// The thread's arguments (`Thread::getArguments()`), if any, released by
    /// its RSHUTDOWN. Taken out and put back, never borrowed; a value it lets go
    /// of, which may run a destructor, drops once the cell holds its successor.
    static ARGV: Cell<Option<Owned>> = const { Cell::new(None) };
}

/// What the script's `zend_first_try` starts with.
struct Start {
    argv_serialized: Option<Box<[u8]>>,
    argc: c_int,
    /// The thread's arguments, taken by the prologue.
    argv: Option<Arguments>,
}

/// `sw_active_thread_count()`: the PHP threads running.
#[must_use]
pub fn sw_active_thread_count() -> usize {
    THREADS.load(Ordering::SeqCst)
}

/// A PHP thread's body: Swoole\Thread's (Rust starts the OS thread) and a
/// thread-mode Server's workers: `file` as the primary script with `argv` as
/// its arguments. `entered` and `left` (with the exit status) report its life
/// to the owner. Nothing allocates before `entered`: a new thread's first
/// malloc can map an arena. `fresh` proves the thread new, PHP not yet entered.
pub fn run(
    fresh: swoole_rs_corebase::Fresh,
    file: &[u8],
    argv: Option<Arguments>,
    entered: impl FnOnce(),
    left: impl FnOnce(c_int),
) {
    THREADS.fetch_add(1, Ordering::SeqCst);
    entered();
    // A C string, as Zend opens it.
    let text = file.split(|byte| *byte == 0).next().unwrap_or_default();
    let file = CString::new(text).unwrap_or_default();
    let thread = Entered::enter(fresh);
    let (path, argc, argv_serialized) = request::main_request();
    swoole_rs_corebase::swoole_thread_init(false);
    let mut start = Start {
        argv_serialized,
        argc,
        argv,
    };
    let status = thread.request(&file, path.as_deref(), &mut start);
    request::stdio_close();
    drop(file);
    left(status);
    thread.leave();
    swoole_rs_corebase::swoole_thread_clean(false);
    THREADS.fetch_sub(1, Ordering::SeqCst);
}

/// This thread inside TSRM (`swr_zend_thread_enter`) until [`Entered::leave`]: only
/// [`Entered::enter`] makes one, from the proof that PHP never ran on the thread.
/// Neither `Clone` nor `Send`.
struct Entered(PhantomData<*mut ()>);

impl Entered {
    /// TSRM's state for this new thread. Each shim below is declared inside the
    /// method whose arguments prove its precondition, so nothing else calls it.
    fn enter(_fresh: swoole_rs_corebase::Fresh) -> Self {
        unsafe extern "C" {
            /// `ts_resource(0)` and the thread's PHP globals, on a new thread
            /// (`_fresh`, which this takes).
            safe fn swr_zend_thread_enter();
        }
        swr_zend_thread_enter();
        Self(PhantomData)
    }

    /// The thread's request: `file` as the primary script, with the main
    /// request's script path `path` and `start.argc`, which Zend keeps only
    /// while the request runs, inside this call. The script's `zend_first_try`
    /// (the thread's script scope) runs [`prologue`] first and [`epilogue`]
    /// after, over `start`. The exit status.
    #[expect(
        clippy::unused_self,
        reason = "the receiver proves the thread entered TSRM"
    )]
    fn request(&self, file: &CStr, path: Option<&CStr>, start: &mut Start) -> c_int {
        type Step = extern "C" fn(&mut Start);
        #[expect(
            improper_ctypes,
            reason = "C passes the `Start` pointer on to the steps it was given and never reads it"
        )]
        unsafe extern "C" {
            /// A request on a thread inside TSRM (`&self`), between requests: it
            /// keeps none of its arguments past its return.
            safe fn swr_zend_thread_request(
                file: Text<'_>,
                path_translated: Option<Text<'_>>,
                argc: c_int,
                prologue: Step,
                epilogue: Step,
                context: &mut Start,
            ) -> c_int;
        }
        let argc = start.argc;
        swr_zend_thread_request(
            file.into(),
            path.map(Text::from),
            argc,
            prologue,
            epilogue,
            start,
        )
    }

    /// `ts_free_thread()`, once the thread's request is over.
    #[expect(
        clippy::unused_self,
        reason = "the receiver proves the thread entered TSRM"
    )]
    fn leave(self) {
        unsafe extern "C" {
            /// Frees the TSRM state of a thread that entered (`self`, which this takes).
            safe fn swr_zend_thread_leave();
        }
        swr_zend_thread_leave();
    }
}

/// Inside the script's `zend_first_try`: `$argv` and `$argc` from the main
/// request, the thread's arguments, and its stdio constants. A bailout is
/// raised from here, which owns nothing.
extern "C" fn prologue(start: &mut Start) {
    if let Some(data) = start.argv_serialized.take() {
        // A bailout is deferred.
        let global_argv = crate::host::unserialize(&data);
        drop(data);
        bailout::finish();
        // The symbol table takes a reference of its own; this one goes with `global_argv`.
        set_argv(&global_argv, i64::from(start.argc));
    }
    if let Some(argv) = start.argv.take() {
        let array = argv.0.to_array();
        ARGV.set(Some(array));
        drop(argv);
        bailout::finish();
    }
    // `thread_register_stdio_file_handles(true)`: open `php://stdin`, `stdout`
    // and `stderr` and register them as `STDIN`, `STDOUT` and `STDERR`. During
    // shutdown other extensions may still use them, so they are never closed
    // with the request; their descriptors are kept to close afterwards.
    let names = [
        (c"php://stdin", c"rb", c"STDIN"),
        (c"php://stdout", c"wb", c"STDOUT"),
        (c"php://stderr", c"wb", c"STDERR"),
    ];
    let mut opening = OPENING.lock().unwrap_or_else(PoisonError::into_inner);
    let streams = names.map(|(path, mode, _)| opening.open(path, mode));
    drop(opening);
    let [Some(stdin), Some(stdout), Some(stderr)] = streams else {
        streams
            .into_iter()
            .flatten()
            .for_each(|stream| stdio_close(stream));
        return;
    };
    let mut streams = [stdin, stdout, stderr];
    let mut fds = [-1; 3];
    let mut casted = true;
    for (stream, fd) in streams.iter_mut().zip(&mut fds) {
        casted &= stream.fd(fd);
    }
    request::keep_stdio(fds);
    if !casted {
        swoole_rs_corebase::log::warning(
            c"thread_register_stdio_file_handles",
            format_args!("failed to cast the stdio streams to fd, the file descriptors may leak"),
        );
    }
    // No such constant exists yet, so each registers, and its stream then
    // outlives its resource.
    for (stream, (_, _, name)) in streams.into_iter().zip(names) {
        stdio_constant(name.into(), stream);
        swr_zend_stream_constant_no_close(name.into());
    }
}

/// After the script: an `exit()` the thread's reactor has not seen stops it,
/// then the thread-exit mark is cleared. It never reads its argument.
extern "C" fn epilogue(_start: &mut Start) {
    if swr_zend_guard_pending() != 0 {
        swoole_rs_reactor::event_api::swoole_event_bail_out();
    }
    clear_exit();
}

/// The thread-exit mark goes: nothing is left for the exit to leave.
fn clear_exit() {
    swr_zend_guard_clear();
}

/// Upstream's `php_swoole_thread_bailout()`: `exit()` in a thread unwinds to the nearest
/// native scope that contains it, else out of the coroutine, else out of the
/// thread's script.
pub extern "C" fn thread_bailout() {
    swr_zend_guard_mark();
    // This frame owns nothing.
    swr_zend_guard_bailout();
    if swoole_rs_phpcoroutines::in_coroutine() {
        // Return to the immediate native resumer; Rust owners relay the
        // pending exit after cleanup. This request never resumes an exited
        // PHP frame.
        loop {
            swoole_rs_phpcoroutines::suspend();
        }
    }
    if !swr_zend_thread_script_running() {
        // The script is over: the exit ends the request-shutdown step Zend
        // runs it in, and the rest of the shutdown runs as usual.
        clear_exit();
    }
    swr_zend_thread_script_bailout();
}

/// The running coroutine's id as the scopes' context token, null outside one.
extern "C" fn guard_context() -> *mut c_void {
    swoole_rs_phpcoroutines::Coroutine::current().map_or(ptr::null_mut(), |co| {
        ptr::without_provenance_mut(usize::try_from(co.cid()).unwrap_or(usize::MAX))
    })
}

/// MINIT (`php_swoole_thread_minit`): the exit scopes' hooks, `Swoole\Thread`
/// with its constants, then `Swoole\Thread\Error`.
pub fn thread_minit(minit: &Minit) {
    minit.guard_register(guard_context, thread_bailout);
    let class = thread::register(minit);
    // `std::thread::hardware_concurrency()`.
    let cpus = swoole_rs_corebase::sys::sysconf(libc::_SC_NPROCESSORS_ONLN).max(0);
    minit.class_constant_long(class, c"HARDWARE_CONCURRENCY", cpus);
    // TSRM's static name.
    minit.class_constant_string(class, c"API_NAME", swr_zend_tsrm_api_name().get());
    let policies = [
        (c"SCHED_OTHER", libc::SCHED_OTHER),
        (c"SCHED_FIFO", libc::SCHED_FIFO),
        (c"SCHED_RR", libc::SCHED_RR),
        // glibc's <sched.h> under _GNU_SOURCE.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        (c"SCHED_BATCH", 3),
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        (c"SCHED_ISO", 4),
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        (c"SCHED_IDLE", 5),
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        (c"SCHED_DEADLINE", 6),
    ];
    for (name, value) in policies {
        minit.class_constant_long(class, name, value.into());
    }
}

/// `tsrm_is_main_thread()`.
fn main_thread() -> bool {
    swoole_rs_zend::is_main_thread()
}

/// The main thread's script path, which its RINIT kept for new threads.
#[must_use]
pub fn main_script() -> Option<CString> {
    request::main_path()
}

/// RINIT: the main thread keeps its script path and `$argv` for new threads.
pub fn thread_rinit(_request: swoole_rs_zend::Request<'_>) {
    unsafe extern "C" {
        /// In RINIT, which this function's `Request` stands for: SAPI's script path and
        /// `argc`, and `$argv`, each live for the request, or none.
        safe fn swr_zend_thread_main_request<'r>(
            path: &mut Option<Text<'r>>,
            argc: &mut c_int,
        ) -> Option<&'r Value>;
    }
    if main_thread() {
        let (mut path, mut argc) = (None, 0);
        let argv = swr_zend_thread_main_request(&mut path, &mut argc);
        request::keep(path.map(Text::get), argv, argc);
        bailout::finish();
    }
}

/// RSHUTDOWN: drop the thread's arguments; the main thread exits the process
/// while other threads run, else releases what RINIT kept.
pub extern "C" fn thread_rshutdown() {
    drop(ARGV.take());
    if !main_thread() {
        return;
    }
    let running = sw_active_thread_count();
    if running > 1 {
        swoole_rs_corebase::log::warning(
            c"php_swoole_thread_rshutdown",
            format_args!("Fatal Error: {running} active threads are running, cannot exit safely."),
        );
        std::process::exit(200);
    }
    request::release();
}

/// `Thread::getArguments()`: the thread's arguments, if it has any.
pub(crate) fn arguments(call: &Call) {
    // `ZVAL_COPY` only adds a reference; the copy is returned once the arguments are back.
    let argv = ARGV.take();
    let copy = argv.as_deref().map(Value::copy);
    ARGV.set(argv);
    if let Some(copy) = copy {
        call.set_owned(copy);
    }
}

/// `Thread::getInfo()`.
pub(crate) fn info() -> Owned {
    let mut info = OwnedArray::default();
    info.insert_bool(b"is_main_thread", main_thread());
    info.insert_bool(b"is_shutdown", swr_zend_tsrm_is_shutdown());
    let threads = i64::try_from(sw_active_thread_count()).unwrap_or(i64::MAX);
    info.insert_long(b"thread_num", threads);
    info.into_owned()
}
