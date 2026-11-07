//! What C++ provides: `swr_v1_phpprocess_host` in `native/process/swoole_rs_phpprocess.h`.
use std::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::sys::{zend_class_entry, zend_object, zval};

type Worker = *mut c_void;

/// C++ state the methods reach: Swoole globals, the server, callables, the
/// reactor and signals. Field for field the C struct.
#[repr(C)]
pub struct Host {
    pub exception: *mut zend_class_entry,
    pub server: *mut *mut zend_class_entry,
    pub construct_guard: unsafe extern "C" fn() -> c_int,
    pub unconstructed: unsafe extern "C" fn(),
    pub worker_share: unsafe extern "C" fn(Worker) -> bool,
    pub worker_base: unsafe extern "C" fn() -> u32,
    pub reactor_init: unsafe extern "C" fn() -> c_int,
    pub event_wait: unsafe extern "C" fn(),
    pub dup_socket: unsafe extern "C" fn(fd: c_int, stream: bool) -> *mut zend_object,
    pub convert_to_fd: unsafe extern "C" fn(value: *mut zval) -> c_int,
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
    pub signal_set: unsafe extern "C" fn(
        signo: c_int,
        handler: Option<unsafe extern "C" fn(c_int)>,
        safety: bool,
    ),
    pub sync_server: unsafe extern "C" fn() -> bool,
    pub signal_listen: unsafe extern "C" fn(),
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
