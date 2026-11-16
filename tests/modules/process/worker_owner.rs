//! A server retains a Process's native record, pipe and private queue.
use super::*;
use crate::{state::tests::TestServer, worker_methods};
use std::os::fd::AsRawFd;

unsafe extern "C" {
    fn msgsnd(
        id: std::ffi::c_int,
        message: *const std::ffi::c_void,
        size: usize,
        flags: std::ffi::c_int,
    ) -> std::ffi::c_int;
}

fn queue_exists(id: std::ffi::c_int) -> bool {
    // A zero-payload message: its positive type is the complete header. This also
    // works on Darwin, where libc does not expose the SysV message queue calls.
    let kind: libc::c_long = 1;
    unsafe { msgsnd(id, ptr::from_ref(&kind).cast(), 0, libc::IPC_NOWAIT) == 0 }
}

fn fd_open(fd: std::ffi::c_int) -> bool {
    unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
}

fn resources() -> (ProcessWorker, std::ffi::c_int, std::ffi::c_int) {
    let owner = ProcessWorker::new(WorkerRecord::empty());
    let fd = owner.open_pipe(libc::SOCK_STREAM).expect("a Unix pair");
    let queue = QueueRecord::new(libc::IPC_PRIVATE, true, 0);
    let id = queue.id();
    assert!(queue.ready());
    owner.own_queue(queue);
    (owner, fd, id)
}

#[test]
fn server_retains_record_pipe_and_queue_after_the_process_drops() {
    let server = TestServer::new();
    let (owner, fd, queue) = resources();
    let address = owner.get().raw();
    assert_eq!(worker_methods::add_worker(server.serv(), owner), Some(0));
    assert_eq!(server.serv().user_worker_num(), 1);
    let worker = server.serv().pending_worker(0).expect("retained worker");
    assert_eq!(worker.raw(), address);
    assert_eq!(worker.id(), 0);
    assert_eq!(
        worker.kind(),
        Some(swoole_rs_process::worker_record::WorkerType::User)
    );
    assert!(fd_open(fd));
    assert!(queue_exists(queue));
    drop(server);
    assert!(!fd_open(fd), "last owner closed the pipe");
    assert!(!queue_exists(queue), "last owner removed its private queue");
}

#[test]
fn a_thread_copy_retains_the_native_resources_and_closes_the_pair_once() {
    let (owner, fd, queue) = resources();
    let copy = ProcessWorker::new(owner.get().get());
    copy.get().set_shared(true);
    assert!(copy.share_resources(owner));
    assert!(fd_open(fd));
    assert!(queue_exists(queue));
    assert_eq!(copy.close_pipe(0), Some(true));
    assert!(copy.get().pipe_object().is_null());
    assert!(!fd_open(fd));
    let replacement = std::fs::File::open("/dev/null").expect("replacement fd");
    drop(copy);
    assert!(
        fd_open(replacement.as_raw_fd()),
        "drop must not close a reused fd"
    );
    assert!(!queue_exists(queue));
}

#[test]
fn copying_a_record_does_not_adopt_its_borrowed_resources() {
    let (owner, fd, queue) = resources();
    drop(ProcessWorker::new(owner.get().get()));
    assert!(fd_open(fd));
    assert!(queue_exists(queue));
    drop(owner);
    assert!(!fd_open(fd));
    assert!(!queue_exists(queue));
}
