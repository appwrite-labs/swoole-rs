//! A PHP thread's life (upstream's `thread_run`): TSRM, its request and the
//! script, under a bailout that `exit()` anywhere in the thread reaches, then
//! teardown; with the module's MINIT, RINIT and RSHUTDOWN for threads, the
//! running thread count and each thread's arguments, over crates/zend's
//! `thread` module.
use crate::{Arguments, request, thread};
use std::{
    cell::Cell,
    ffi::{CString, c_int, c_void},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
use swoole_rs_zend::{
    Call, Minit, Owned, OwnedArray, Value, bailout,
    thread::{Entered, Prologue, Stdio, Steps},
};

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
    let status = thread.request(&file, path.as_deref(), start.argc, &mut start);
    request::stdio_close();
    drop(file);
    left(status.0);
    thread.leave();
    swoole_rs_corebase::swoole_thread_clean(false);
    THREADS.fetch_sub(1, Ordering::SeqCst);
}

impl Steps for Start {
    /// Inside the script's `zend_first_try`: `$argv` and `$argc` from the main
    /// request, the thread's arguments, and its stdio constants. A bailout is
    /// raised from here, which owns nothing.
    fn prologue(&mut self, mut prologue: Prologue<'_>) {
        if let Some(data) = self.argv_serialized.take() {
            // A bailout is deferred.
            let global_argv = crate::host::unserialize(&data);
            drop(data);
            bailout::finish();
            // The symbol table takes a reference of its own; this one goes with `global_argv`.
            prologue.set_argv(&global_argv, i64::from(self.argc));
        }
        if let Some(argv) = self.argv.take() {
            let array = argv.0.to_array();
            ARGV.set(Some(array));
            drop(argv);
            bailout::finish();
        }
        // `thread_register_stdio_file_handles(true)`: open `php://stdin`, `stdout`
        // and `stderr` and register them as `STDIN`, `STDOUT` and `STDERR`. During
        // shutdown other extensions may still use them, so they are never closed
        // with the request; their descriptors are kept to close afterwards.
        let Some(mut streams) = prologue.open_stdio() else {
            return;
        };
        let mut fds = [-1; 3];
        let mut casted = true;
        for (stream, fd) in streams.iter_mut().zip(&mut fds) {
            casted &= stream.fd(fd);
        }
        request::keep_stdio(fds);
        if !casted {
            swoole_rs_corebase::log::warning(
                c"thread_register_stdio_file_handles",
                format_args!(
                    "failed to cast the stdio streams to fd, the file descriptors may leak"
                ),
            );
        }
        // No such constant exists yet, so each registers, and its stream then
        // outlives its resource.
        Stdio::register(streams);
    }

    /// After the script: an `exit()` the thread's reactor has not seen stops it;
    /// the thread-exit mark is cleared after.
    fn epilogue(&mut self, exited: bool) {
        if exited {
            swoole_rs_corebase::reactor::event_api::swoole_event_bail_out();
        }
    }
}

/// Upstream's `php_swoole_thread_bailout()`: `exit()` in a thread unwinds to the nearest
/// native scope that contains it, else out of the coroutine, else out of the
/// thread's script.
pub extern "C" fn thread_bailout() {
    swoole_rs_zend::thread::exit(|| {
        if swoole_rs_phpcoroutines::in_coroutine() {
            // Return to the immediate native resumer; Rust owners relay the
            // pending exit after cleanup. This request never resumes an exited
            // PHP frame.
            loop {
                swoole_rs_phpcoroutines::suspend();
            }
        }
    });
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
    minit.class_constant_string(class, c"API_NAME", swoole_rs_zend::thread::api_name());
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
pub fn thread_rinit(rinit: swoole_rs_zend::Request<'_>) {
    if main_thread() {
        let (path, argc, argv) = swoole_rs_zend::thread::main_request(rinit);
        request::keep(path, argv, argc);
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
    info.insert_bool(b"is_shutdown", swoole_rs_zend::thread::is_shutdown());
    let threads = i64::try_from(sw_active_thread_count()).unwrap_or(i64::MAX);
    info.insert_long(b"thread_num", threads);
    info.into_owned()
}
