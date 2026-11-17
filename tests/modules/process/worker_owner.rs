//! A server retains a Process's native record, pipe and private queue.
use super::*;
use crate::{state::tests::TestServer, worker_methods};
use swoole_rs_corebase::process::worker_record::PipeEnd;

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

/// The open file `fd` names, by device and inode: tests in parallel may reuse a
/// closed number, so "closed" means it no longer names the same file.
fn file_of(fd: std::ffi::c_int) -> Option<(libc::dev_t, libc::ino_t)> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(fd, &raw mut stat) } == 0).then_some((stat.st_dev, stat.st_ino))
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
#[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
fn server_retains_record_pipe_and_queue_after_the_process_drops() {
    let server = TestServer::new();
    let (owner, fd, queue) = resources();
    let file = file_of(fd);
    let address = owner.get().raw();
    assert_eq!(worker_methods::add_worker(server.serv(), owner), Some(0));
    assert_eq!(server.serv().user_worker_num(), 1);
    let worker = server.serv().pending_worker(0).expect("retained worker");
    assert_eq!(worker.raw(), address);
    assert_eq!(worker.id(), 0);
    assert_eq!(
        worker.kind(),
        Some(swoole_rs_corebase::process::worker_record::WorkerType::User)
    );
    assert!(file.is_some() && file_of(fd) == file);
    assert!(queue_exists(queue));
    drop(server);
    assert_ne!(file_of(fd), file, "last owner closed the pipe");
    assert!(!queue_exists(queue), "last owner removed its private queue");
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
fn a_thread_copy_retains_the_native_resources_and_leaves_the_pair_to_its_owner() {
    let (owner, fd, queue) = resources();
    let file = file_of(fd);
    let copy = ProcessWorker::new(owner.get().get());
    copy.get().set_shared(true);
    assert!(copy.share_resources(owner.clone()));
    assert!(file.is_some() && file_of(fd) == file);
    assert!(queue_exists(queue));
    assert_eq!(copy.close_pipe(1), Some(true));
    assert_eq!(copy.close_pipe(1), Some(false), "already closed");
    assert_eq!(copy.close_pipe(0), Some(true));
    assert!(copy.get().pipe_object().is_null());
    assert_eq!(file_of(fd), file, "a copy's close frees no end");
    let named = |end| owner.get().pipe(end).is_some();
    assert!(
        named(PipeEnd::Master) && named(PipeEnd::Worker),
        "the owner's record still names its live ends"
    );
    drop(copy);
    assert!(file_of(fd) == file && queue_exists(queue));
    drop(owner);
    assert_ne!(file_of(fd), file, "last owner closed the pipe");
    assert!(!queue_exists(queue));
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
fn copying_a_record_does_not_adopt_its_borrowed_resources() {
    let (owner, fd, queue) = resources();
    let file = file_of(fd);
    drop(ProcessWorker::new(owner.get().get()));
    assert!(file.is_some() && file_of(fd) == file);
    assert!(queue_exists(queue));
    drop(owner);
    assert_ne!(file_of(fd), file);
    assert!(!queue_exists(queue));
}
