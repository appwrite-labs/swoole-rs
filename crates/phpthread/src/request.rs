//! What a PHP thread's request shares with the main thread's and the
//! descriptors it passes between threads: the main script's path and `$argv`,
//! each thread's STDIN/STDOUT/STDERR, stream descriptors stored in containers
//! and `Swoole\Thread\Error`, the value a descriptor that cannot be read back
//! becomes.
use crate::host;
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_int},
    sync::Mutex,
};
use swoole_rs_corebase::sys;
use swoole_rs_zend::{
    Class, Minit, Owned, Value,
    sys::{
        PHP_STREAM_FLAG_NO_CLOSE, ZEND_ACC_PUBLIC, ZEND_ACC_READONLY,
        swr_zend_register_stream_constant, swr_zend_stream_close, swr_zend_stream_fd,
        swr_zend_stream_open, swr_zend_stream_to_zval,
    },
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

/// A new `Swoole\Thread\Error` with `code` set to `errno`.
fn error(errno: i64) -> Owned {
    let error = ERROR.create_object();
    if let Some(props) = error.props(Some(&ERROR)) {
        props.long(c"code", errno);
    }
    error
}

/// `php_swoole_thread_stream_cast`: a duplicate of a stream's descriptor, or
/// -1. A warning's handler that bailed out is deferred.
pub(crate) fn stream_cast(value: &Value) -> c_int {
    match value.stream_fd() {
        Ok(Some(fd)) => sys::dup(fd),
        Ok(None) => -1,
        Err(bailout) => {
            bailout.defer();
            -1
        }
    }
}

/// `php_swoole_thread_stream_create`: `php://fd/<fd>` (which duplicates the
/// descriptor again), or a Thread\Error. In a request.
pub(crate) fn stream_create(fd: c_int) -> Owned {
    let path = CString::new(format!("php://fd/{fd}")).unwrap_or_default();
    // SAFETY: in a request, with NUL-terminated strings.
    let stream = unsafe { swr_zend_stream_open(path.as_ptr(), c"".as_ptr()) };
    if stream.is_null() {
        return error(errno());
    }
    let out = Owned::default();
    // SAFETY: a stream just opened, whose resource a slot this owns takes.
    unsafe { swr_zend_stream_to_zval(stream, out.as_ptr()) };
    out
}

/// A duplicate of `fd` handed to `create`, which makes the object over it
/// (`php_swoole_thread_co_socket_create` and its ext/sockets twin), or a
/// Thread\Error; `None` once a bailout is pending.
pub(crate) fn socket_create(
    fd: c_int,
    create: impl FnOnce(c_int) -> Option<Owned>,
) -> Option<Owned> {
    let duplicate = sys::dup(fd);
    let created = if duplicate >= 0 {
        create(duplicate)
    } else {
        None
    };
    if created.is_some() || swoole_rs_zend::bailout::pending() {
        return created;
    }
    Some(error(errno()))
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
    // Opening `php://stdout` and `php://stderr` sets PHP's process-global
    // `cli_out` and `cli_err`: one thread at a time.
    static OPENING: Mutex<()> = Mutex::new(());
    let names = [
        (c"php://stdin", c"rb", c"STDIN"),
        (c"php://stdout", c"wb", c"STDOUT"),
        (c"php://stderr", c"wb", c"STDERR"),
    ];
    let opening = OPENING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: in a request, with NUL-terminated strings.
    let streams =
        names.map(|(path, mode, _)| unsafe { swr_zend_stream_open(path.as_ptr(), mode.as_ptr()) });
    drop(opening);
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
        // A descriptor this thread's streams left open.
        sys::close(fd);
    }
}

// The main thread's request, which every PHP thread starts with.

struct Request {
    path: Option<CString>,
    /// `php_swoole_serialize($argv)`.
    argv: Option<Box<[u8]>>,
    argc: c_int,
}

static REQUEST: Mutex<Request> = Mutex::new(Request {
    path: None,
    argv: None,
    argc: 0,
});

fn request() -> std::sync::MutexGuard<'static, Request> {
    REQUEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The main thread's RINIT keeps its script path, and `$argv` (when set)
/// serialized with `argc`. A bailout while serializing is deferred.
pub(crate) fn keep(path: Option<&CStr>, argv: Option<&Value>, argc: c_int) {
    let mut request = request();
    if let Some(path) = path {
        request.path = Some(path.to_owned());
    }
    let Some(argv) = argv else {
        return;
    };
    // A bailout is deferred.
    request.argv = host::serialize(argv);
    request.argc = argc;
}

/// What a new thread's request starts with: the script path, `argc`, and
/// `$argv` serialized.
pub(crate) fn main_request() -> (Option<CString>, c_int, Option<Box<[u8]>>) {
    let request = request();
    (request.path.clone(), request.argc, request.argv.clone())
}

/// The main thread's script path, which its RINIT kept.
pub(crate) fn main_path() -> Option<CString> {
    request().path.clone()
}

/// The main thread's RSHUTDOWN: release what RINIT kept.
pub(crate) fn release() {
    let mut request = request();
    request.path = None;
    request.argv = None;
}
