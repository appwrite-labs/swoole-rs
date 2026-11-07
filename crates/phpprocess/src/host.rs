//! What C++ provides: `swr_v1_phpprocess_host` in `native/process/swoole_rs_phpprocess.h`.
use std::{
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::sys::{zend_class_entry, zend_object, zval};

/// What the methods read of a `Worker`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WorkerView {
    pub id: u32,
    pub pid: i32,
    /// `pipe_current`'s fd, or -1 without one.
    pub pipe: i32,
    pub pipe_object: bool,
    pub redirect_stdin: bool,
    pub redirect_stdout: bool,
    pub redirect_stderr: bool,
}

type Worker = *mut c_void;

/// C++ state the methods reach: Swoole globals, `Worker`, `UnixSocket`,
/// callables, the reactor and signals. Field for field the C struct.
#[repr(C)]
pub struct Host {
    pub exception: *mut zend_class_entry,
    pub server: *mut *mut zend_class_entry,
    pub construct_guard: unsafe extern "C" fn() -> c_int,
    pub unconstructed: unsafe extern "C" fn(),
    pub worker_new: unsafe extern "C" fn() -> Worker,
    pub worker_free: unsafe extern "C" fn(Worker),
    pub worker_share: unsafe extern "C" fn(Worker) -> bool,
    pub worker_base: unsafe extern "C" fn() -> u32,
    pub worker_redirect: unsafe extern "C" fn(Worker),
    pub worker_set_id: unsafe extern "C" fn(Worker, id: u32),
    pub worker_pipe: unsafe extern "C" fn(Worker, socket_type: c_int) -> c_int,
    pub worker_view: unsafe extern "C" fn(Worker) -> WorkerView,
    pub fork: unsafe extern "C" fn(Worker) -> c_int,
    pub child_enter: unsafe extern "C" fn(Worker),
    pub reactor_init: unsafe extern "C" fn() -> c_int,
    pub event_wait: unsafe extern "C" fn(),
    pub event_available: unsafe extern "C" fn() -> bool,
    pub event_free: unsafe extern "C" fn(),
    pub pipe_read:
        unsafe extern "C" fn(Worker, buffer: *mut c_char, length: usize, blocking: bool) -> isize,
    pub pipe_write:
        unsafe extern "C" fn(Worker, data: *const c_char, length: usize, blocking: bool) -> isize,
    pub pipe_close: unsafe extern "C" fn(Worker, which: i64) -> c_int,
    pub pipe_timeout: unsafe extern "C" fn(Worker, seconds: f64) -> bool,
    pub pipe_blocking: unsafe extern "C" fn(Worker, blocking: bool) -> bool,
    pub dup_socket: unsafe extern "C" fn(fd: c_int, stream: bool) -> *mut zend_object,
    pub convert_to_fd: unsafe extern "C" fn(value: *mut zval) -> c_int,
    pub daemon_dup: unsafe extern "C" fn(from: c_int, to: c_int),
    pub daemon: unsafe extern "C" fn(nochdir: c_int, noclose: c_int) -> c_int,
    pub cpu_num: unsafe extern "C" fn() -> u32,
    pub callable_create: unsafe extern "C" fn(callable: *mut zval) -> *mut c_void,
    pub callable_free: unsafe extern "C" fn(callable: *mut c_void),
    pub callable_defer_free: unsafe extern "C" fn(callable: *mut c_void),
    pub callable_call: unsafe extern "C" fn(
        callable: *mut c_void,
        argc: u32,
        argv: *mut zval,
        coroutine: bool,
    ) -> bool,
    pub enable_coroutine: unsafe extern "C" fn() -> bool,
    pub main_thread: unsafe extern "C" fn() -> bool,
    pub has_timer: unsafe extern "C" fn() -> bool,
    pub signal_handler: unsafe extern "C" fn(signo: c_int) -> usize,
    pub signal_set: unsafe extern "C" fn(
        signo: c_int,
        handler: Option<unsafe extern "C" fn(c_int)>,
        safety: bool,
    ),
    pub sync_server: unsafe extern "C" fn() -> bool,
    pub signal_listen: unsafe extern "C" fn(),
    pub signal_listeners: unsafe extern "C" fn(delta: c_int),
    pub reset_worker_type: unsafe extern "C" fn(),
}

static HOST: AtomicPtr<Host> = AtomicPtr::new(ptr::null_mut());

pub(crate) fn install(host: *const Host) {
    HOST.store(host.cast_mut(), Ordering::Release);
}

/// The table MINIT installed.
pub(crate) fn host() -> &'static Host {
    // SAFETY: MINIT stores C++'s static table before any method or hook runs,
    // and it lives for the process.
    unsafe { &*HOST.load(Ordering::Acquire) }
}
