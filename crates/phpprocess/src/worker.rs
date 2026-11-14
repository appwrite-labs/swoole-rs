//! A Process object's `Worker` and its pipe (were the host table's worker and
//! pipe entries in ext-src/swoole_server.cc): the record crates/process lays
//! out, the `UnixSocket` crates/tls defines, and the pipe's `network::Socket`.
use std::{
    cell::{OnceCell, UnsafeCell},
    ffi::c_int,
    mem::ManuallyDrop,
};
use swoole_rs_corebase::CCast;
use swoole_rs_process::{
    methods::{swoole_set_worker_id, swoole_set_worker_pid},
    worker_record::{Worker, WorkerRecord},
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
    /// `pipe_current`'s fd, while there is one.
    pub pipe: Option<c_int>,
    pub pipe_object: bool,
    pub redirect_stdin: bool,
    pub redirect_stdout: bool,
    pub redirect_stderr: bool,
}

/// The `Worker` a Process object owns, boxed so that native code can keep its
/// address: none before the constructor (or `Pool::getProcess`) makes it, then
/// the same record until the object is freed. It is set once and never moved
/// or replaced, so a handle borrowed from the slot names a live record.
#[derive(Default)]
pub(crate) struct Slot(OnceCell<Box<UnsafeCell<WorkerRecord>>>);

impl Slot {
    /// The object's worker, once it has one.
    pub(crate) fn get(&self) -> Option<Worker<'_>> {
        // The slot's own boxed record, which lives until the slot drops, so for
        // longer than the borrow; only the thread that runs the object's methods
        // reads or writes it, through handles, one field at a time.
        Some(Worker::from(&**self.0.get()?))
    }

    /// Take `record` as the object's worker; false, with `record` dropped,
    /// when the object has one already.
    pub(crate) fn set(&self, record: WorkerRecord) -> bool {
        self.0.set(Box::new(UnsafeCell::new(record))).is_ok()
    }
}

impl Drop for Slot {
    /// `delete worker`: the pipe it owns unless it shares a server's, then the
    /// record with the slot.
    fn drop(&mut self) {
        let Some(record) = self.0.get_mut() else {
            return;
        };
        let record = record.get_mut();
        if !record.pipe_object().is_null() && !record.shared {
            // SAFETY: the worker's own `new UnixSocket`, a boxed pair ([`open_pipe`]) that
            // nothing uses afterwards.
            delete_pair(unsafe { Box::from_raw(record.pipe_object().cast()) });
        }
    }
}

/// `new Worker()`.
pub(crate) fn new() -> WorkerRecord {
    WorkerRecord::empty()
}

/// `delete pair`: the pair's ends, then its box.
fn delete_pair(mut pair: Box<SocketPair>) {
    socket_pair::drop(&mut pair);
}

/// In a server worker thread, the worker becomes a copy of its server worker
/// (shared); false elsewhere, or past the server's last worker.
pub(crate) fn share(worker: Worker<'_>) -> bool {
    crate::host::worker_share(worker)
}

/// Redirect stdin, stdout and stderr to the pipe in the child.
pub(crate) fn redirect(worker: Worker<'_>) {
    worker.set_redirect_stdin(true);
    worker.set_redirect_stdout(true);
    worker.set_redirect_stderr(true);
}

pub(crate) fn set_id(worker: Worker<'_>, id: u32) {
    worker.set_id(id);
    worker.set_shared(false);
}

/// A `UnixSocket` of `socket_type` for the worker; the master end's fd, or none.
pub(crate) fn open_pipe(worker: Worker<'_>, socket_type: c_int) -> Option<c_int> {
    let mut built = Box::new(socket_pair::unix_socket(true, socket_type));
    let (master, end) = (built.master_socket, built.worker_socket);
    if master.is_null() || end.is_null() {
        socket_pair::drop(&mut built);
        return None;
    }
    let pair = Box::into_raw(built);
    // SAFETY: the two ends of the pair just boxed, which lives until `pipe_close`
    // or the slot's drop deletes it; `pipe_close` clears the record's pipes first.
    unsafe { worker.set_pipes(pair.cast(), master.cast(), end.cast()) };
    worker.use_pipe(true);
    pipe(worker).map(Socket::fd)
}

/// The worker's current pipe end, while it has one.
fn pipe(worker: Worker<'_>) -> Option<Socket<'_>> {
    swoole_rs_server::view::worker_pipe_end(worker, swoole_rs_server::view::PipeEnd::Current)
}

pub(crate) fn view(worker: Worker<'_>) -> WorkerView {
    WorkerView {
        id: worker.id(),
        pid: worker.pid(),
        pipe: pipe(worker).map(Socket::fd),
        pipe_object: !worker.pipe_object().is_null(),
        redirect_stdin: worker.redirect_stdin(),
        redirect_stdout: worker.redirect_stdout(),
        redirect_stderr: worker.redirect_stderr(),
    }
}

/// `swoole_fork(0)`, recording the child's pid in the parent.
pub(crate) fn fork(worker: Worker<'_>) -> c_int {
    let pid = swoole_rs_corebase::swoole_fork(0);
    if pid > 0 {
        worker.set_pid(pid);
    }
    if pid >= 0 {
        worker.set_child_process(pid == 0);
    }
    pid
}

/// In the child: the worker end becomes the pipe and the worker this process's.
pub(crate) fn child_enter(worker: Worker<'_>) {
    let pid = swoole_rs_process::os::getpid();
    worker.use_pipe(false);
    worker.set_pid(pid);
    let id = worker.id();
    swoole_set_worker_id(id);
    swoole_set_worker_pid(pid);
    swoole_rs_process::worker_globals::set_current_worker(worker.raw());
}

/// Read up to `buffer`'s capacity from the pipe: the length read, or `None`.
pub(crate) fn pipe_read(worker: Worker<'_>, buffer: &mut ZBuf, blocking: bool) -> Option<usize> {
    let socket = pipe(worker)?;
    // An empty buffer: its whole capacity.
    let spare = buffer.spare_capacity_mut();
    let read = if blocking {
        read_sync(socket, spare)
    } else {
        swoole_rs_corebase::sys::read(socket.fd(), spare)
    };
    usize::try_from(read).ok()
}

/// Write `data` to the pipe: the length written, or `None`.
pub(crate) fn pipe_write(worker: Worker<'_>, data: &[u8], blocking: bool) -> Option<usize> {
    let socket = pipe(worker)?;
    let (bytes, length) = (data.as_ptr().cast(), data.len());
    let written = if !blocking && event_api::swoole_event_is_available() {
        // SAFETY: a live socket and `length` readable bytes.
        unsafe { event_api::swoole_event_write(socket.as_ptr(), bytes, length) }
    } else {
        send_sync(socket, data, 0)
    };
    usize::try_from(written).ok()
}

/// Shut one direction of the pipe, or close the pair's `which` end (0: both,
/// then free the pair); false when it failed or the worker owns no pipe object.
pub(crate) fn pipe_close(worker: Worker<'_>, which: i64) -> bool {
    let fd = pipe(worker).map_or(-1, Socket::fd);
    let pair = worker.pipe_object().cast::<SocketPair>();
    if pair.is_null() {
        return false;
    }
    match which {
        CLOSE_READ => swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_RD) == 0,
        CLOSE_WRITE => swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_WR) == 0,
        _ => {
            // SAFETY: the worker's live pair, boxed as [`open_pipe`] and crates/server's
            // pools make them; the record keeps it unless it is deleted below, and
            // nothing else touches it meanwhile.
            let mut owned = ManuallyDrop::new(unsafe { Box::from_raw(pair) });
            let closed = socket_pair::close(&mut owned, which.c_cast::<c_int>());
            if closed && which == 0 {
                // The record forgets the pair's ends before they go; upstream deletes
                // the pair even when shared.
                worker.clear_pipes();
                delete_pair(ManuallyDrop::into_inner(owned));
            }
            closed
        }
    }
}

/// The pipe's timeouts and kernel timeouts; false without a pipe.
pub(crate) fn pipe_timeout(worker: Worker<'_>, seconds: f64) -> bool {
    pipe(worker).is_some_and(|socket| {
        socket.edit(|record| {
            record.timeouts.set(seconds, TIMEOUT_ALL);
            socket_set_kernel_read_timeout(record, seconds)
                && socket_set_kernel_write_timeout(record, seconds)
        })
    })
}

/// The pipe's `O_NONBLOCK`; false without a pipe.
pub(crate) fn pipe_blocking(worker: Worker<'_>, blocking: bool) -> bool {
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
