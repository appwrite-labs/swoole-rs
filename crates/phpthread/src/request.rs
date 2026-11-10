//! What a PHP thread's request shares with the main thread's and the
//! descriptors it passes between threads: the main script's path and `$argv`,
//! each thread's STDIN/STDOUT/STDERR, stream descriptors stored in containers
//! and `Swoole\Thread\Error`, the value a descriptor that cannot be read back
//! becomes.
use crate::{host, item::set};
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_int},
    ptr,
    sync::Mutex,
};
use swoole_rs_zend::{
    Class, Minit,
    bailout::Bailout,
    object_of,
    sys::{
        IS_RESOURCE_EX, PHP_STREAM_FLAG_NO_CLOSE, ZEND_ACC_PUBLIC, ZEND_ACC_READONLY,
        object_init_ex, swr_zend_register_stream_constant, swr_zend_stream_close,
        swr_zend_stream_expose, swr_zend_stream_fd, swr_zend_stream_from_zval,
        swr_zend_stream_open, swr_zend_stream_select_fd, swr_zend_string_release,
        swr_zend_thread_main_request, zend_string, zend_value, zval,
    },
    update_property_long,
};

static ERROR: Class = Class::new();

/// `Swoole\Thread\Error` with its readonly `code`, registered after
/// `Swoole\Thread` as upstream's MINIT does.
pub(crate) fn error_minit(minit: &Minit) {
    minit.register(&ERROR, c"Swoole\\Thread\\Error", &[], |_, _| {});
    minit.property_long(&ERROR, c"code", 0, ZEND_ACC_PUBLIC | ZEND_ACC_READONLY);
}

fn errno() -> i64 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .map_or(0, i64::from)
}

/// `object_init_ex(out, swoole_thread_error_ce)` with `code` set to `errno`.
/// # Safety
/// In a request; `out` is writable and holds nothing.
unsafe fn error(out: *mut zval, errno: i64) {
    // SAFETY: per the caller; a registered class.
    unsafe { object_init_ex(out, ERROR.entry()) };
    // SAFETY: an object zval just made.
    let std = unsafe { object_of(out) };
    // SAFETY: the class's own readonly property, set in its scope.
    unsafe { update_property_long(ERROR.entry(), std, c"code", errno) };
}

/// `php_swoole_thread_stream_cast`: a duplicate of a stream's descriptor, or
/// -1. A warning's handler that bailed out is deferred.
/// # Safety
/// In a request, with a live resource zval.
pub(crate) unsafe fn stream_cast(value: *mut zval) -> c_int {
    // SAFETY: per the caller; null when it is not a stream.
    let stream = unsafe { swr_zend_stream_from_zval(value) };
    if stream.is_null() {
        return -1;
    }
    let mut fd = -1;
    // SAFETY: a live stream; the shim traps a bailout.
    match unsafe { swr_zend_stream_select_fd(stream, &raw mut fd) } {
        // SAFETY: a descriptor the stream owns.
        0 if fd >= 0 => unsafe { libc::dup(fd) },
        2 => {
            Bailout.defer();
            -1
        }
        _ => -1,
    }
}

/// `php_swoole_thread_stream_create`: `php://fd/<fd>` (which duplicates the
/// descriptor again), or a Thread\Error.
/// # Safety
/// In a request; `out` is writable and holds nothing.
pub(crate) unsafe fn stream_create(out: *mut zval, fd: c_int) {
    let path = CString::new(format!("php://fd/{fd}")).unwrap_or_default();
    // SAFETY: in a request, with NUL-terminated strings.
    let stream = unsafe { swr_zend_stream_open(path.as_ptr(), c"".as_ptr()) };
    if stream.is_null() {
        // SAFETY: per the caller.
        unsafe { error(out, errno()) };
        return;
    }
    // SAFETY: a stream just opened; `php_stream_to_zval`.
    let res = unsafe { swr_zend_stream_expose(stream) };
    set(out, IS_RESOURCE_EX, zend_value { res });
}

/// A duplicate of `fd` handed to `create`, which makes the object over it
/// (`php_swoole_thread_co_socket_create` and its ext/sockets twin), or a
/// Thread\Error.
/// # Safety
/// In a request; `out` is writable and holds nothing; `create` writes `out`
/// on success and leaves it a value `zval_ptr_dtor` takes on failure.
pub(crate) unsafe fn socket_create(
    out: *mut zval,
    fd: c_int,
    create: impl FnOnce(*mut zval, c_int) -> bool,
) {
    // SAFETY: a descriptor the item owns.
    let duplicate = unsafe { libc::dup(fd) };
    if (duplicate >= 0 && create(out, duplicate)) || swoole_rs_zend::bailout::pending() {
        return;
    }
    let errno = errno();
    // SAFETY: per the caller.
    unsafe { swoole_rs_zend::sys::zval_ptr_dtor(out) };
    // SAFETY: per the caller.
    unsafe { error(out, errno) };
}

// Each thread's STDIN, STDOUT and STDERR.

// Their descriptors, which `PHP_STREAM_FLAG_NO_CLOSE` keeps open past request
// shutdown, closed by `swr_v1_phpthread_stdio_close`.
thread_local! {
    static STDIO: Cell<[c_int; 3]> = const { Cell::new([-1; 3]) };
}

/// `thread_register_stdio_file_handles(true)`: open `php://stdin`, `stdout`
/// and `stderr` and register them as `STDIN`, `STDOUT` and `STDERR`. During
/// shutdown other extensions may still use them, so they are never closed with
/// the request; their descriptors are kept to close afterwards.
/// # Safety
/// In a thread's request, inside its `zend_try`.
pub(crate) unsafe fn stdio_open() {
    let names = [
        (c"php://stdin", c"rb", c"STDIN"),
        (c"php://stdout", c"wb", c"STDOUT"),
        (c"php://stderr", c"wb", c"STDERR"),
    ];
    // SAFETY: in a request, with NUL-terminated strings.
    let streams =
        names.map(|(path, mode, _)| unsafe { swr_zend_stream_open(path.as_ptr(), mode.as_ptr()) });
    if streams.iter().any(|stream| stream.is_null()) {
        for stream in streams.into_iter().filter(|stream| !stream.is_null()) {
            // SAFETY: a stream opened above.
            unsafe { swr_zend_stream_close(stream) };
        }
        return;
    }
    let mut fds = [-1; 3];
    let mut casted = true;
    for (stream, fd) in streams.iter().zip(&mut fds) {
        // SAFETY: a stream opened above.
        unsafe { (**stream).flags |= PHP_STREAM_FLAG_NO_CLOSE };
        // SAFETY: as above.
        casted &= unsafe { swr_zend_stream_fd(*stream, fd) };
    }
    STDIO.set(fds);
    if !casted {
        swoole_rs_corebase::log::warning(
            c"thread_register_stdio_file_handles",
            format_args!("failed to cast the stdio streams to fd, the file descriptors may leak"),
        );
    }
    for (stream, (_, _, name)) in streams.into_iter().zip(names) {
        // SAFETY: a stream opened above; Zend takes the constant's value.
        unsafe { swr_zend_register_stream_constant(name.as_ptr(), stream) };
    }
}

/// `thread_unregister_stdio_file_handles`: close the descriptors
/// [`stdio_open`] kept, after request shutdown.
pub(crate) fn stdio_close() {
    for fd in STDIO.replace([-1; 3]).into_iter().filter(|fd| *fd >= 0) {
        // SAFETY: a descriptor this thread's streams left open.
        unsafe { libc::close(fd) };
    }
}

// The main thread's request, which every PHP thread starts with.

struct Request {
    path: Option<CString>,
    /// `php_swoole_serialize($argv)`: a persistent string.
    argv: *mut zend_string,
    argc: c_int,
}

// SAFETY: the persistent string is written by the main thread's RINIT and
// released by its RSHUTDOWN, when no other PHP thread runs.
unsafe impl Send for Request {}

static REQUEST: Mutex<Request> = Mutex::new(Request {
    path: None,
    argv: ptr::null_mut(),
    argc: 0,
});

fn request() -> std::sync::MutexGuard<'static, Request> {
    REQUEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The main thread's RINIT: keep its script path, and `$argv` (when set)
/// serialized with `argc`. A bailout while serializing is deferred.
/// # Safety
/// The main thread's RINIT.
pub(crate) unsafe fn keep() {
    let (mut path, mut argc) = (ptr::null(), 0);
    // SAFETY: the main thread's request.
    let argv = unsafe { swr_zend_thread_main_request(&raw mut path, &raw mut argc) };
    let mut request = request();
    if !path.is_null() {
        // SAFETY: SAPI's NUL-terminated script path.
        request.path = Some(unsafe { CStr::from_ptr(path) }.to_owned());
    }
    if argv.is_null() {
        return;
    }
    // SAFETY: a live zval; a bailout is deferred.
    request.argv = unsafe { host::serialize(argv) };
    request.argc = argc;
}

/// What a new thread's request starts with: the script path (null if none),
/// `argc`, and `$argv` serialized (null if none). Valid until the main
/// thread's RSHUTDOWN.
pub(crate) fn main_request() -> (*const c_char, c_int, *mut zend_string) {
    let request = request();
    let script = request
        .path
        .as_ref()
        .map_or(ptr::null(), |path| path.as_ptr());
    (script, request.argc, request.argv)
}

/// The main thread's RSHUTDOWN: release what RINIT kept.
pub(crate) fn release() {
    let mut request = request();
    request.path = None;
    let argv = std::mem::replace(&mut request.argv, ptr::null_mut());
    if !argv.is_null() {
        // SAFETY: the reference `keep` kept.
        unsafe { swr_zend_string_release(argv) };
    }
}
