//! A Process object's `Worker` and its pipe (were the host table's worker and
//! pipe entries in ext-src/swoole_server.cc): the record crates/process lays
//! out, the `UnixSocket` crates/tls defines, and the pipe's `network::Socket`.
use std::ffi::{c_char, c_int, c_void};
use swoole_rs_process::{
    methods::{swoole_set_worker_id, swoole_set_worker_pid},
    worker_globals::thread_globals,
    worker_record::WorkerRecord,
};
use swoole_rs_reactor::{
    SocketRecord,
    socket_upstream::{
        socket_set_fd_option, socket_set_kernel_read_timeout, socket_set_kernel_write_timeout,
    },
};
use swoole_rs_tls::{
    socket_pair::{self, SocketPair},
    upstream::{socket_read_sync, socket_send_sync},
};

/// `SW_PIPE_CLOSE_READ`, `SW_PIPE_CLOSE_WRITE` and `SW_TIMEOUT_ALL`.
const CLOSE_READ: i64 = 3;
const CLOSE_WRITE: i64 = 4;
const TIMEOUT_ALL: c_int = 15;

// SAFETY: the C++ runtime's allocator, and crates/reactor's and
// crates/ossupport's functions, under their upstream symbols.
unsafe extern "C" {
    #[link_name = "_Znwm"]
    fn operator_new(size: usize) -> *mut c_void;
    #[link_name = "_ZdlPv"]
    fn operator_delete(storage: *mut c_void);
    #[link_name = "_Z25swoole_event_is_availablev"]
    pub(crate) fn swoole_event_is_available() -> bool;
    #[link_name = "_Z17swoole_event_freev"]
    pub(crate) fn swoole_event_free() -> c_int;
    #[link_name = "_Z18swoole_event_writePN6swoole7network6SocketEPKvm"]
    fn swoole_event_write(socket: *mut SocketRecord, data: *const c_void, length: usize) -> isize;
    #[link_name = "_Z13swoole_daemonii"]
    pub(crate) fn swoole_daemon(nochdir: c_int, noclose: c_int) -> c_int;
}

/// What the methods read of a `Worker`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkerView {
    pub id: u32,
    pub pid: i32,
    /// `pipe_current`'s fd, or -1 without one.
    pub pipe: i32,
    pub pipe_object: bool,
    pub redirect_stdin: bool,
    pub redirect_stdout: bool,
    pub redirect_stderr: bool,
}

pub(crate) fn record<'a>(worker: *mut c_void) -> &'a mut WorkerRecord {
    // SAFETY: every caller passes a live worker this process owns or copied.
    unsafe { &mut *worker.cast::<WorkerRecord>() }
}

/// `new Worker()`.
pub(crate) fn new() -> *mut c_void {
    Box::into_raw(WorkerRecord::boxed()).cast()
}

/// `delete pair`: the pair's ends, then `new`'s storage.
/// # Safety
/// `pair` came from operator new and nothing uses it afterwards.
unsafe fn delete_pair(pair: *mut SocketPair) {
    // SAFETY: as the caller promises.
    unsafe { socket_pair::drop(pair) };
    // SAFETY: as the caller promises.
    unsafe { operator_delete(pair.cast()) };
}

/// Frees a worker `new` made, and the pipe it owns unless it shares a server's.
/// # Safety
/// `worker` came from `new` and nothing uses it afterwards.
pub(crate) unsafe fn free(worker: *mut c_void) {
    // SAFETY: as the caller promises.
    let worker = unsafe { Box::from_raw(worker.cast::<WorkerRecord>()) };
    if !worker.pipe_object.is_null() && !worker.shared {
        // SAFETY: the worker's own `new UnixSocket`.
        unsafe { delete_pair(worker.pipe_object.cast()) };
    }
}

/// Redirect stdin, stdout and stderr to the pipe in the child.
pub(crate) fn redirect(worker: *mut c_void) {
    let worker = record(worker);
    worker.redirect_stdin = true;
    worker.redirect_stdout = true;
    worker.redirect_stderr = true;
}

pub(crate) fn set_id(worker: *mut c_void, id: u32) {
    let record = record(worker);
    record.set_id(id);
    record.shared = false;
}

/// A `UnixSocket` of `socket_type` for the worker; the master end's fd, or -1.
pub(crate) fn open_pipe(worker: *mut c_void, socket_type: c_int) -> c_int {
    // SAFETY: storage for one pair; a failed operator new throws, which aborts here.
    let pair = unsafe { operator_new(size_of::<SocketPair>()) }.cast::<SocketPair>();
    // SAFETY: fresh storage for a pair.
    unsafe { socket_pair::unix_socket_new(pair, true, socket_type) };
    // SAFETY: constructed above.
    let pair_ref = unsafe { &*pair };
    let (master, end) = (pair_ref.master_socket, pair_ref.worker_socket);
    if master.is_null() || end.is_null() {
        // SAFETY: the pair made above.
        unsafe { delete_pair(pair) };
        return -1;
    }
    let worker = record(worker);
    worker.pipe_master = master.cast();
    worker.pipe_worker = end.cast();
    worker.pipe_object = pair.cast();
    worker.pipe_current = master.cast();
    fd(master)
}

fn pipe(worker: *mut c_void) -> *mut SocketRecord {
    record(worker).pipe_current.cast()
}

/// A live socket's descriptor.
fn fd(socket: *const SocketRecord) -> c_int {
    // SAFETY: callers pass a live socket.
    unsafe { (*socket).fd }
}

pub(crate) fn view(worker: *mut c_void) -> WorkerView {
    let socket = pipe(worker);
    let record = record(worker);
    WorkerView {
        id: record.id(),
        pid: record.pid(),
        // A worker's current pipe end is null or a live socket.
        pipe: if socket.is_null() { -1 } else { fd(socket) },
        pipe_object: !record.pipe_object.is_null(),
        redirect_stdin: record.redirect_stdin,
        redirect_stdout: record.redirect_stdout,
        redirect_stderr: record.redirect_stderr,
    }
}

/// `swoole_fork(0)`, recording the child's pid in the parent.
pub(crate) fn fork(worker: *mut c_void) -> c_int {
    let pid = swoole_rs_corebase::swoole_fork(0);
    if pid > 0 {
        record(worker).set_pid(pid);
    }
    if pid >= 0 {
        record(worker).child_process = u8::from(pid == 0);
    }
    pid
}

/// In the child: the worker end becomes the pipe and the worker this process's.
pub(crate) fn child_enter(worker: *mut c_void) {
    let pid = swoole_rs_process::os::getpid();
    let process = record(worker);
    process.pipe_current = process.pipe_worker;
    process.set_pid(pid);
    swoole_set_worker_id(process.id());
    swoole_set_worker_pid(pid);
    // SAFETY: this thread's worker globals; the Process object keeps the worker alive.
    unsafe { (*thread_globals()).worker = worker.cast() };
}

/// # Safety
/// The worker has a pipe and `buffer` has room for `length` bytes.
pub(crate) unsafe fn pipe_read(
    worker: *mut c_void,
    buffer: *mut c_char,
    length: usize,
    blocking: bool,
) -> isize {
    let socket = pipe(worker);
    if blocking {
        // SAFETY: as the caller promises.
        unsafe { socket_read_sync(socket, buffer.cast(), length) }
    } else {
        // SAFETY: as the caller promises.
        unsafe { libc::read(fd(socket), buffer.cast(), length) }
    }
}

/// # Safety
/// The worker has a pipe and `data` holds `length` bytes.
pub(crate) unsafe fn pipe_write(
    worker: *mut c_void,
    data: *const c_char,
    length: usize,
    blocking: bool,
) -> isize {
    let socket = pipe(worker);
    // SAFETY: the event loop's state.
    if !blocking && unsafe { swoole_event_is_available() } {
        // SAFETY: as the caller promises.
        return unsafe { swoole_event_write(socket, data.cast(), length) };
    }
    // SAFETY: as the caller promises.
    unsafe { socket_send_sync(socket, data.cast(), length, 0) }
}

/// Shut one direction of the pipe, or close the pair's `which` end (0: both,
/// then free the pair).
/// # Safety
/// The worker owns a pipe object.
pub(crate) unsafe fn pipe_close(worker: *mut c_void, which: i64) -> c_int {
    let socket = pipe(worker);
    let worker = record(worker);
    let ret = match which {
        // SAFETY: as the caller promises.
        CLOSE_READ => unsafe { libc::shutdown(fd(socket), libc::SHUT_RD) },
        // SAFETY: as the caller promises.
        CLOSE_WRITE => unsafe { libc::shutdown(fd(socket), libc::SHUT_WR) },
        #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
        // SAFETY: as the caller promises.
        _ => c_int::from(unsafe { socket_pair::close(worker.pipe_object.cast(), which as c_int) }),
    };
    if ret >= 0 && which == 0 {
        // SAFETY: as the caller promises; upstream deletes it even when shared.
        unsafe { delete_pair(worker.pipe_object.cast()) };
        worker.pipe_object = std::ptr::null_mut();
        worker.pipe_current = std::ptr::null_mut();
    }
    ret
}

/// # Safety
/// The worker has a pipe.
pub(crate) unsafe fn pipe_timeout(worker: *mut c_void, seconds: f64) -> bool {
    // SAFETY: as the caller promises, the live socket.
    let socket = unsafe { &mut *pipe(worker) };
    socket.timeouts.set(seconds, TIMEOUT_ALL);
    socket_set_kernel_read_timeout(socket, seconds)
        && socket_set_kernel_write_timeout(socket, seconds)
}

/// # Safety
/// The worker has a pipe.
pub(crate) unsafe fn pipe_blocking(worker: *mut c_void, blocking: bool) -> bool {
    // SAFETY: as the caller promises.
    socket_set_fd_option(unsafe { &mut *pipe(worker) }, c_int::from(!blocking), -1)
}

/// `daemon(pipes:)`: `dup2(from, to)`, warning on failure.
pub(crate) fn dup(from: c_int, to: c_int) {
    // SAFETY: dup2 has no memory preconditions.
    if unsafe { libc::dup2(from, to) } < 0 {
        swoole_rs_corebase::log::sys_warning(
            c"zim_swoole_process_daemon",
            format_args!("dup2({from}, {to}) failed"),
        );
    }
}
