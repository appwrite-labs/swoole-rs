//! The coroutine core's operations, lent by C++ at MINIT
//! (`swr_v1_phpcoroutines_ops`), and thin wrappers over them.
use std::{
    ffi::{CStr, c_char, c_int, c_long, c_void},
    sync::OnceLock,
};
use swoole_rs_zend::sys::{zend_array, zend_execute_data, zval};

/// `swr_v1_phpcoroutines_stats`.
#[repr(C)]
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) event_num: i64,
    pub(crate) signal_listener_num: i64,
    pub(crate) aio_task_num: i64,
    pub(crate) aio_worker_num: i64,
    pub(crate) aio_queue_size: i64,
    pub(crate) c_stack_size: i64,
    pub(crate) coroutine_num: i64,
    pub(crate) coroutine_peak_num: i64,
    pub(crate) coroutine_last_cid: i64,
    pub(crate) iouring: u8,
    pub(crate) iouring_task_num: i64,
    pub(crate) iouring_sq_usage_percent: i64,
    pub(crate) iouring_waiting_task_num: i64,
}

type Co = *mut c_void;

/// `swr_v1_phpcoroutines_ops`; see the native header for each entry.
#[repr(C)]
pub struct Ops {
    pub(crate) flags: unsafe extern "C" fn(u32) -> u32,
    pub(crate) current: unsafe extern "C" fn() -> Co,
    pub(crate) current_safe: unsafe extern "C" fn() -> Co,
    pub(crate) current_task: unsafe extern "C" fn() -> *mut c_void,
    pub(crate) current_cid: unsafe extern "C" fn() -> c_long,
    pub(crate) task_by_cid: unsafe extern "C" fn(c_long) -> *mut c_void,
    pub(crate) by_cid: unsafe extern "C" fn(c_long) -> Co,
    pub(crate) create: unsafe extern "C" fn(*mut c_void) -> c_long,
    pub(crate) count: unsafe extern "C" fn() -> usize,
    pub(crate) elapsed: unsafe extern "C" fn(c_long) -> c_long,
    pub(crate) execute_time: unsafe extern "C" fn(c_long) -> c_long,
    pub(crate) list: unsafe extern "C" fn(*mut c_long, usize) -> usize,
    pub(crate) stats: unsafe extern "C" fn(*mut Stats),
    pub(crate) kernel_coroutine: unsafe extern "C" fn(c_long, f64),
    pub(crate) set_task: unsafe extern "C" fn(Co, *mut c_void),
    pub(crate) origin_task: unsafe extern "C" fn(Co) -> *mut c_void,
    pub(crate) cid: unsafe extern "C" fn(Co) -> c_long,
    pub(crate) origin_cid: unsafe extern "C" fn(Co) -> c_long,
    pub(crate) state: unsafe extern "C" fn(Co) -> c_int,
    pub(crate) stack: unsafe extern "C" fn(Co, *mut usize) -> *mut c_void,
    pub(crate) yield_: unsafe extern "C" fn(Co),
    pub(crate) yield_cancelable: unsafe extern "C" fn(Co, unsafe extern "C" fn(Co) -> u8),
    pub(crate) yield_ex: unsafe extern "C" fn(Co, f64) -> u8,
    pub(crate) resume: unsafe extern "C" fn(Co),
    pub(crate) cancel: unsafe extern "C" fn(Co) -> u8,
    pub(crate) is_canceled: unsafe extern "C" fn(Co) -> u8,
    pub(crate) is_end: unsafe extern "C" fn(Co) -> u8,
    pub(crate) check_reactor: unsafe extern "C" fn(),
    pub(crate) attach: unsafe extern "C" fn(
        unsafe extern "C" fn(*mut c_void),
        unsafe extern "C" fn(*mut c_void),
        unsafe extern "C" fn(*mut c_void),
    ),
    pub(crate) detach: unsafe extern "C" fn(),
    pub(crate) deactivate: unsafe extern "C" fn(),
    pub(crate) hook: unsafe extern "C" fn(u32),
    pub(crate) unhook: unsafe extern "C" fn(),
    pub(crate) signal_block_all: unsafe extern "C" fn(),
    pub(crate) load_library: unsafe extern "C" fn(*mut c_void),
    pub(crate) deadlock_check: unsafe extern "C" fn(*mut c_void),
    pub(crate) coro_hook: unsafe extern "C" fn(u8, *mut c_void),
    pub(crate) absolute_msec: unsafe extern "C" fn() -> c_long,
    pub(crate) defer: unsafe extern "C" fn(unsafe extern "C" fn(*mut c_void), *mut c_void),
    pub(crate) timer_add:
        unsafe extern "C" fn(f64, c_long, unsafe extern "C" fn(c_long)) -> *mut c_void,
    pub(crate) timer_del: unsafe extern "C" fn(*mut c_void),
    pub(crate) set_last_error: unsafe extern "C" fn(c_int),
    pub(crate) fatal_error: unsafe extern "C" fn(c_int, *const c_char),
    pub(crate) error_ex: unsafe extern "C" fn(c_int, c_int, *const c_char),
    pub(crate) exit_original: unsafe extern "C" fn(*mut zend_execute_data, *mut zval) -> u8,
    pub(crate) exit_fallback: unsafe extern "C" fn(*const c_char, usize, c_long),
    pub(crate) reactor_init: unsafe extern "C" fn() -> c_int,
    pub(crate) event_wait: unsafe extern "C" fn(),
    pub(crate) set_global_option: unsafe extern "C" fn(*mut zend_array),
    pub(crate) set_stack_size: unsafe extern "C" fn(*mut zval),
    pub(crate) add_name_resolver: unsafe extern "C" fn(*mut zval),
    pub(crate) set_dns_cache_expire: unsafe extern "C" fn(c_long),
    pub(crate) set_dns_cache_capacity: unsafe extern "C" fn(c_long),
    pub(crate) set_exit_condition: unsafe extern "C" fn(*mut zval),
}

/// `swr_v1_phpcoroutines_*` flag bits.
pub(crate) const CLI: u32 = 1;
pub(crate) const SHORTNAME: u32 = 2;
pub(crate) const LIBRARY: u32 = 4;
pub(crate) const FIBER_MOCK: u32 = 8;
pub(crate) const PREEMPTIVE: u32 = 16;
pub(crate) const SHUTDOWN_FUNCTIONS: u32 = 32;
pub(crate) const SERVER_STARTED: u32 = 64;
pub(crate) const MAIN_THREAD: u32 = 128;
pub(crate) const FATAL: u32 = 256;
pub(crate) const CORO_TIME: u32 = 512;

/// Swoole error codes (`include/swoole_error.h`).
pub(crate) const SW_ERROR_INVALID_PARAMS: c_int = 505;
pub(crate) const SW_ERROR_WRONG_OPERATION: c_int = 509;
pub(crate) const SW_ERROR_CO_NOT_EXISTS: c_int = 10016;
pub(crate) const SW_ERROR_CO_CANCELED: c_int = 10017;

static OPS: OnceLock<&'static Ops> = OnceLock::new();

/// Store the table C++ lends at MINIT.
pub(crate) fn install(ops: &'static Ops) {
    OPS.get_or_init(|| ops);
}

#[inline]
pub(crate) fn ops() -> &'static Ops {
    // Everything here runs after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

/// Whether `bit` of the host's flags is set.
#[inline]
pub(crate) fn flag(bit: u32) -> bool {
    // SAFETY: C++ reads its globals.
    unsafe { (ops().flags)(bit) != 0 }
}

pub(crate) fn set_last_error(code: c_int) {
    // SAFETY: C++ stores the thread's last error.
    unsafe { (ops().set_last_error)(code) };
}

/// `php_swoole_fatal_error(level, "%s", message)`. With `E_ERROR` Zend
/// longjmps out: the caller's frames must own nothing that needs a drop.
pub(crate) fn fatal_error(level: c_int, message: &CStr) {
    // SAFETY: a NUL-terminated message.
    unsafe { (ops().fatal_error)(level, message.as_ptr()) };
}

/// `php_swoole_error_ex(level, code, "%s", message)`.
pub(crate) fn error_ex(level: c_int, code: c_int, message: &CStr) {
    // SAFETY: a NUL-terminated message.
    unsafe { (ops().error_ex)(level, code, message.as_ptr()) };
}

pub(crate) fn count() -> usize {
    // SAFETY: C++ reads the coroutine table.
    unsafe { (ops().count)() }
}

pub(crate) fn current() -> Co {
    // SAFETY: C++ reads the running coroutine.
    unsafe { (ops().current)() }
}

/// `Coroutine::get_current_safe()`: outside a coroutine it raises a fatal
/// error and does not return, so the caller's frames must own nothing that
/// needs a drop.
pub(crate) fn current_safe() -> Co {
    // SAFETY: as above.
    unsafe { (ops().current_safe)() }
}

pub(crate) fn cid(co: Co) -> c_long {
    // SAFETY: a live coroutine.
    unsafe { (ops().cid)(co) }
}

pub(crate) fn absolute_msec() -> c_long {
    // SAFETY: C++ reads the clock.
    unsafe { (ops().absolute_msec)() }
}

/// Switch to `co`. The caller's frames must own nothing that needs a drop: a
/// bailout in the coroutine may longjmp across them on the main stack.
pub(crate) fn resume(co: Co) {
    // SAFETY: a live coroutine.
    unsafe { (ops().resume)(co) };
}
