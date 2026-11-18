//! A Process object's `Worker` and its pipe (were the host table's worker and
//! pipe entries in ext-src/swoole_server.cc): the record `corebase::process` lays
//! out, the `UnixSocket` `corebase::tls` defines, and the pipe's `network::Socket`.
use std::{cell::OnceCell, ffi::c_int};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::process::{
    methods::{swoole_set_worker_id, swoole_set_worker_pid},
    worker_record::{Worker, WorkerRecord},
};
use swoole_rs_corebase::reactor::{
    event_api,
    socket_upstream::{
        socket_set_fd_option, socket_set_kernel_read_timeout, socket_set_kernel_write_timeout,
    },
};
use swoole_rs_corebase::tls::upstream::{read_sync, send_sync};
use swoole_rs_server::view;

/// `SW_PIPE_CLOSE_READ` and `SW_PIPE_CLOSE_WRITE`.
const CLOSE_READ: i64 = 3;
const CLOSE_WRITE: i64 = 4;

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

/// The `Worker` a Process object owns, retained so that native code can keep its
/// address: none before the constructor (or `Pool::getProcess`) makes it, then
/// the same record until the Process and every retaining server are freed. It is set once and never moved
/// or replaced, so a handle borrowed from the slot names a live record.
#[derive(Default)]
pub(crate) struct Slot(OnceCell<swoole_rs_server::process_worker::ProcessWorker>);

impl Slot {
    /// The object's worker, once it has one.
    pub(crate) fn get(&self) -> Option<Worker<'_>> {
        // The slot's retained record, which lives until the slot drops, so for
        // longer than the borrow.
        Some(self.0.get()?.get())
    }

    /// An owned share for a server that will keep using the worker.
    pub(crate) fn retain(&self) -> Option<swoole_rs_server::process_worker::ProcessWorker> {
        self.0.get().cloned()
    }

    pub(crate) fn own_queue(&self, queue: swoole_rs_corebase::ossupport::QueueRecord) {
        if let Some(owner) = self.0.get() {
            owner.own_queue(queue);
        }
    }

    pub(crate) fn open_pipe(&self, socket_type: c_int) -> Option<c_int> {
        self.0.get()?.open_pipe(socket_type)
    }

    pub(crate) fn close_pipe(&self, which: c_int) -> Option<bool> {
        self.0.get()?.close_pipe(which)
    }

    pub(crate) fn share_resources(
        &self,
        source: swoole_rs_server::process_worker::ProcessWorker,
    ) -> bool {
        self.0
            .get()
            .is_some_and(|owner| owner.share_resources(source))
    }

    pub(crate) fn free_queue(&self) -> bool {
        self.0
            .get()
            .is_some_and(swoole_rs_server::process_worker::ProcessWorker::free_queue)
    }

    /// Take `record` as the object's worker; false, with `record` dropped,
    /// when the object has one already.
    pub(crate) fn set(&self, record: WorkerRecord) -> bool {
        self.0
            .set(swoole_rs_server::process_worker::ProcessWorker::new(record))
            .is_ok()
    }
}

/// `new Worker()`.
pub(crate) fn new() -> WorkerRecord {
    WorkerRecord::empty()
}

/// In a server worker thread, the worker becomes a copy of its server worker
/// (shared); false elsewhere, or past the server's last worker.
pub(crate) fn share(slot: &Slot, worker: Worker<'_>) -> bool {
    crate::host::worker_share(slot, worker)
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

/// The worker's current pipe end, while it has one.
fn pipe(worker: Worker<'_>) -> Option<swoole_rs_corebase::reactor::Heap<'_>> {
    view::worker_pipe_end(worker, view::PipeEnd::Current)
}

pub(crate) fn view(worker: Worker<'_>) -> WorkerView {
    WorkerView {
        id: worker.id(),
        pid: worker.pid(),
        pipe: pipe(worker).map(|pipe| pipe.fd()),
        pipe_object: !worker.pipe_object().is_null(),
        redirect_stdin: worker.redirect_stdin(),
        redirect_stdout: worker.redirect_stdout(),
        redirect_stderr: worker.redirect_stderr(),
    }
}

/// `swoole_fork(0)`, recording the child's pid in the parent: the pid there, 0 in the
/// child, `None` when the fork failed.
pub(crate) fn fork(worker: Worker<'_>) -> Option<c_int> {
    let pid = swoole_rs_corebase::swoole_fork(0);
    if pid > 0 {
        worker.set_pid(pid);
    }
    (pid >= 0).then(|| {
        worker.set_child_process(pid == 0);
        pid
    })
}

/// In the child: the worker end becomes the pipe and the worker this process's.
pub(crate) fn child_enter(worker: Worker<'_>) {
    let pid = swoole_rs_corebase::process::os::getpid();
    worker.use_pipe(false);
    worker.set_pid(pid);
    let id = worker.id();
    swoole_set_worker_id(id);
    swoole_set_worker_pid(pid);
    // SAFETY: the pool's worker this process now runs, which the pool keeps until
    // the worker loop ends.
    unsafe { swoole_rs_corebase::process::worker_globals::set_current_worker(worker.raw()) };
}

/// Read up to `buffer`'s length from the pipe: the length read, or `None`.
pub(crate) fn pipe_read(worker: Worker<'_>, buffer: &mut [u8], blocking: bool) -> Option<usize> {
    let socket = pipe(worker)?;
    if blocking {
        read_sync(*socket, buffer)
    } else {
        swoole_rs_corebase::sys::read(socket.fd(), buffer)
    }
}

/// Write `data` to the pipe: the length written, or `None`.
pub(crate) fn pipe_write(worker: Worker<'_>, data: &[u8], blocking: bool) -> Option<usize> {
    if !blocking && event_api::swoole_event_is_available() {
        event_api::event_write(pipe(worker)?, data)
    } else {
        send_sync(*pipe(worker)?, data, 0)
    }
}

/// Shut one direction of the pipe, or close the pair's `which` end (0: both,
/// then free the pair); false when it failed or the worker owns no pipe object.
pub(crate) fn pipe_close(slot: &Slot, worker: Worker<'_>, which: i64) -> bool {
    let fd = pipe(worker).map_or(-1, |pipe| pipe.fd());
    let pair = worker.pipe_object();
    if pair.is_null() {
        return false;
    }
    match which {
        CLOSE_READ => swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_RD).is_ok(),
        CLOSE_WRITE => swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_WR).is_ok(),
        _ => slot.close_pipe(which.c_cast::<c_int>()).unwrap_or(false),
    }
}

/// The pipe's timeouts and kernel timeouts; false without a pipe.
pub(crate) fn pipe_timeout(worker: Worker<'_>, seconds: f64) -> bool {
    pipe(worker).is_some_and(|socket| {
        socket.edit(|record| {
            record
                .timeouts
                .set(seconds, swoole_rs_corebase::reactor::TimeoutKind::ALL);
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
    if swoole_rs_corebase::sys::dup2(from, to).is_err() {
        swoole_rs_corebase::log::sys_warning(
            c"zim_swoole_process_daemon",
            format_args!("dup2({from}, {to}) failed"),
        );
    }
}
