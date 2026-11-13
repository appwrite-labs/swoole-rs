//! A PHP thread's life (upstream's `thread_run`): TSRM, its request and the
//! script, under a bailout that `exit()` anywhere in the thread reaches, then
//! teardown; with the module's MINIT, RINIT and RSHUTDOWN for threads, the
//! running thread count and each thread's arguments. Zend's setjmp, TSRM and
//! request macros are zend-sys shims.
use crate::{Arguments, request, thread};
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, CString, c_int, c_void},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
use swoole_rs_zend::{
    Call, Minit, Owned, OwnedArray, Value, bailout,
    sys::{
        swr_zend_bailout_to, swr_zend_thread_enter, swr_zend_thread_leave,
        swr_zend_thread_main_request, swr_zend_thread_request, swr_zend_thread_set_argv,
    },
};

// crates/zend-sys' shims that take nothing and can't run PHP or bail out: the calling
// thread's exit mark and native scopes (`shim_guard.c`), and TSRM's process state.
unsafe extern "C" {
    /// The thread-exit mark: nonzero once `exit()` ran in this thread.
    safe fn swr_zend_guard_pending() -> c_int;
    /// Sets the thread-exit mark.
    safe fn swr_zend_guard_mark();
    /// Clears the thread-exit mark.
    safe fn swr_zend_guard_clear();
    /// The jump buffer of the innermost native scope of the running context
    /// ([`guard_context`]), or null. Only jumping to it is unsafe.
    safe fn swr_zend_guard_target() -> *mut c_void;
    /// `tsrm_is_shutdown()`; false on NTS.
    safe fn swr_zend_tsrm_is_shutdown() -> bool;
    /// `tsrm_api_name()`: a NUL-terminated literal (empty on NTS).
    safe fn swr_zend_tsrm_api_name() -> swoole_rs_corebase::sys::Text<'static>;
}

/// PHP threads running, the main thread included.
static THREADS: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    /// The thread's arguments (`Thread::getArguments()`), if any, released by
    /// its RSHUTDOWN.
    static ARGV: RefCell<Option<Owned>> = const { RefCell::new(None) };
    /// The jump buffer of the thread's `zend_first_try` while its script runs.
    static BAILOUT: Cell<*mut c_void> = const { Cell::new(ptr::null_mut()) };
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
/// malloc can map an arena.
/// # Safety
/// A new OS thread, on which nothing of PHP's has run.
pub unsafe fn run(
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
    // SAFETY: a new thread, before anything of PHP's runs on it.
    unsafe { swr_zend_thread_enter() };
    let (path, argc, argv_serialized) = request::main_request();
    let path = path.as_ref().map_or(ptr::null(), |path| path.as_ptr());
    swoole_rs_corebase::swoole_thread_init(false);
    let mut start = Start {
        argv_serialized,
        argc,
        argv,
    };
    // SAFETY: the shim keeps its jump buffer in C; `start` and the
    // thread-local slot outlive the call.
    let status = unsafe {
        swr_zend_thread_request(
            file.as_ptr(),
            path,
            argc,
            Some(prologue),
            Some(epilogue),
            (&raw mut start).cast(),
            BAILOUT.with(Cell::as_ptr),
        )
    };
    request::stdio_close();
    drop(file);
    left(status);
    // SAFETY: the request is over.
    unsafe { swr_zend_thread_leave() };
    swoole_rs_corebase::swoole_thread_clean(false);
    THREADS.fetch_sub(1, Ordering::SeqCst);
}

/// Inside the script's `zend_first_try`: `$argv` and `$argc` from the main
/// request, the thread's arguments, and its stdio constants. A bailout is
/// raised from here, which owns nothing.
unsafe extern "C" fn prologue(start: *mut c_void) {
    // SAFETY: `run`'s `Start`.
    let start = unsafe { &mut *start.cast::<Start>() };
    if let Some(data) = start.argv_serialized.take() {
        // A bailout is deferred.
        let global_argv = crate::host::unserialize(&data);
        drop(data);
        bailout::finish();
        let mut global_argv = global_argv.into_zval();
        // SAFETY: the symbol table takes the value.
        unsafe { swr_zend_thread_set_argv(&raw mut global_argv, i64::from(start.argc)) };
    }
    if let Some(argv) = start.argv.take() {
        let array = argv.0.to_array();
        ARGV.set(Some(array));
        drop(argv);
        bailout::finish();
    }
    // SAFETY: in the thread's request, inside its `zend_try`.
    unsafe { request::stdio_open() };
}

/// After the script: an `exit()` the thread's reactor has not seen stops it,
/// then the thread-exit mark is cleared. It never reads its argument.
extern "C" fn epilogue(_start: *mut c_void) {
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
    let target = swr_zend_guard_target();
    if !target.is_null() {
        // SAFETY: a live native scope's jump buffer; this frame owns nothing.
        unsafe { swr_zend_bailout_to(target) };
    }
    if swoole_rs_phpcoroutines::in_coroutine() {
        // Return to the immediate native resumer; Rust owners relay the
        // pending exit after cleanup. This request never resumes an exited
        // PHP frame.
        loop {
            swoole_rs_phpcoroutines::suspend();
        }
    }
    let script = BAILOUT.get();
    if script.is_null() {
        // The script is over: the exit ends the request-shutdown step Zend
        // runs it in, and the rest of the shutdown runs as usual.
        clear_exit();
    }
    // SAFETY: the thread's script scope, or Zend's current one; this frame
    // owns nothing.
    unsafe { swr_zend_bailout_to(script) };
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
/// # Safety
/// RINIT.
pub unsafe extern "C" fn thread_rinit() {
    if main_thread() {
        let (mut path, mut argc) = (ptr::null(), 0);
        // SAFETY: the main thread's request.
        let argv = unsafe { swr_zend_thread_main_request(&raw mut path, &raw mut argc) };
        // SAFETY: SAPI's NUL-terminated script path, live for the request.
        let path = (!path.is_null()).then(|| unsafe { CStr::from_ptr(path) });
        // SAFETY: null or `$argv`, live for the request.
        let argv = unsafe { Value::from_ptr(argv) };
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
    ARGV.with_borrow(|argv| {
        if let Some(argv) = argv {
            call.set_owned(argv.copy());
        }
    });
}

/// `Thread::getInfo()`.
pub(crate) fn info() -> Owned {
    let mut info = OwnedArray::default();
    info.insert_bool(b"is_main_thread", main_thread());
    info.insert_bool(b"is_shutdown", swr_zend_tsrm_is_shutdown());
    #[expect(clippy::cast_possible_wrap, reason = "a thread count")]
    info.insert_long(b"thread_num", sw_active_thread_count() as i64);
    info.into_owned()
}
