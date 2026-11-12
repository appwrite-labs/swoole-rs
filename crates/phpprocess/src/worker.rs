//! A Process object's `Worker` and its pipe (were the host table's worker and
//! pipe entries in ext-src/swoole_server.cc): the record crates/process lays
//! out, the `UnixSocket` crates/tls defines, and the pipe's `network::Socket`.
use std::{
    ffi::{c_int, c_void},
    mem::MaybeUninit,
};
use swoole_rs_process::{
    methods::{swoole_set_worker_id, swoole_set_worker_pid},
    worker_record::WorkerRecord,
};
use swoole_rs_reactor::{
    Socket, event_api,
    socket_upstream::{
        socket_set_fd_option, socket_set_kernel_read_timeout, socket_set_kernel_write_timeout,
    },
};
use swoole_rs_tls::{
    socket_pair::{self, SocketPair},
    upstream::{read_sync, send_sync},
};
use swoole_rs_zend::string::ZBuf;

/// `SW_PIPE_CLOSE_READ`, `SW_PIPE_CLOSE_WRITE` and `SW_TIMEOUT_ALL`.
const CLOSE_READ: i64 = 3;
const CLOSE_WRITE: i64 = 4;
const TIMEOUT_ALL: c_int = 15;

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

/// `delete pair`: the pair's ends, then its box.
/// # Safety
/// `pair` is a boxed pair, as [`open_pipe`] and crates/server's pools make them, and
/// nothing uses it afterwards.
unsafe fn delete_pair(pair: *mut SocketPair) {
    // SAFETY: as the caller promises.
    socket_pair::drop(&mut *unsafe { Box::from_raw(pair) });
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
    let mut built = Box::new(socket_pair::unix_socket(true, socket_type));
    let (master, end) = (built.master_socket, built.worker_socket);
    if master.is_null() || end.is_null() {
        socket_pair::drop(&mut built);
        return -1;
    }
    let pair = Box::into_raw(built);
    let record = record(worker);
    record.pipe_master = master.cast();
    record.pipe_worker = end.cast();
    record.pipe_object = pair.cast();
    record.pipe_current = master.cast();
    pipe(worker).map_or(-1, Socket::fd)
}

/// The worker's current pipe end, while it has one.
fn pipe<'a>(worker: *mut c_void) -> Option<Socket<'a>> {
    // SAFETY: a worker's current pipe end is null or a live socket of its pair.
    unsafe { Socket::from_ptr(record(worker).pipe_current.cast()) }
}

pub(crate) fn view(worker: *mut c_void) -> WorkerView {
    let pipe = pipe(worker).map_or(-1, Socket::fd);
    let record = record(worker);
    WorkerView {
        id: record.id(),
        pid: record.pid(),
        pipe,
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
    swoole_rs_process::worker_globals::set_current_worker(worker.cast());
}

/// Read up to `buffer`'s capacity from the pipe: the length read, or -1.
pub(crate) fn pipe_read(worker: *mut c_void, buffer: &mut ZBuf, blocking: bool) -> isize {
    let Some(socket) = pipe(worker) else {
        return -1;
    };
    let (data, length) = (buffer.as_mut_ptr().cast(), buffer.capacity());
    // SAFETY: the buffer's own `capacity` writable bytes, borrowed with it.
    let spare: &mut [MaybeUninit<u8>] = unsafe { std::slice::from_raw_parts_mut(data, length) };
    if blocking {
        read_sync(socket, spare)
    } else {
        swoole_rs_corebase::sys::read(socket.fd(), spare)
    }
}

/// Write `data` to the pipe: the length written, or -1.
pub(crate) fn pipe_write(worker: *mut c_void, data: &[u8], blocking: bool) -> isize {
    let Some(socket) = pipe(worker) else {
        return -1;
    };
    let (bytes, length) = (data.as_ptr().cast(), data.len());
    if !blocking && event_api::swoole_event_is_available() {
        // SAFETY: a live socket and `length` readable bytes.
        return unsafe { event_api::swoole_event_write(socket.as_ptr(), bytes, length) };
    }
    send_sync(socket, data, 0)
}

/// Shut one direction of the pipe, or close the pair's `which` end (0: both,
/// then free the pair); -1 when the worker owns no pipe object.
pub(crate) fn pipe_close(worker: *mut c_void, which: i64) -> c_int {
    let fd = pipe(worker).map_or(-1, Socket::fd);
    let worker = record(worker);
    let pair = worker.pipe_object.cast::<SocketPair>();
    if pair.is_null() {
        return -1;
    }
    let ret = match which {
        CLOSE_READ => swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_RD),
        CLOSE_WRITE => swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_WR),
        #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
        _ => c_int::from(socket_pair::close(
            // SAFETY: the worker's live pair.
            unsafe { &mut *pair },
            which as c_int,
        )),
    };
    if ret >= 0 && which == 0 {
        // SAFETY: the worker's pair, deleted once; upstream deletes it even when shared.
        unsafe { delete_pair(pair) };
        worker.pipe_object = std::ptr::null_mut();
        worker.pipe_current = std::ptr::null_mut();
    }
    ret
}

/// The pipe's timeouts and kernel timeouts; false without a pipe.
pub(crate) fn pipe_timeout(worker: *mut c_void, seconds: f64) -> bool {
    pipe(worker).is_some_and(|socket| {
        socket.edit(|record| {
            record.timeouts.set(seconds, TIMEOUT_ALL);
            socket_set_kernel_read_timeout(record, seconds)
                && socket_set_kernel_write_timeout(record, seconds)
        })
    })
}

/// The pipe's `O_NONBLOCK`; false without a pipe.
pub(crate) fn pipe_blocking(worker: *mut c_void, blocking: bool) -> bool {
    pipe(worker).is_some_and(|socket| {
        socket.edit(|record| socket_set_fd_option(record, c_int::from(!blocking), -1))
    })
}

/// `daemon(pipes:)`: `dup2(from, to)`, warning on failure.
pub(crate) fn dup(from: c_int, to: c_int) {
    if swoole_rs_corebase::sys::dup2(from, to) < 0 {
        swoole_rs_corebase::log::sys_warning(
            c"zim_swoole_process_daemon",
            format_args!("dup2({from}, {to}) failed"),
        );
    }
}
