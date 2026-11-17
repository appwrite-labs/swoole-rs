//! The native resources a PHP Process and a server's user-worker registry share.
use std::{
    cell::{Cell, OnceCell},
    ptr,
    rc::Rc,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use swoole_rs_corebase::ossupport::QueueRecord;
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_tls::socket_pair::{self, SocketPair};

/// A stable worker record, its owned pipe and its owned message queue. No PHP
/// value is retained here. Atomic reference counts allow the server to release
/// its share after its worker threads stop; record access still follows the
/// server's existing startup and worker synchronization rules.
#[derive(Clone)]
pub struct ProcessWorker(Arc<Resources>);

struct Resources {
    record: WorkerRecord,
    pipe: OnceCell<Pipe>,
    // Never cloned: moving the Rc out of the cell leaves its allocation stable.
    queue: Cell<Option<Rc<QueueRecord>>>,
    parent: OnceCell<ProcessWorker>,
}

impl ProcessWorker {
    /// Keep a record. Existing pipe pointers are borrowed; only `open_pipe`
    /// below creates a pipe this owner frees.
    #[must_use]
    #[expect(
        clippy::arc_with_non_send_sync,
        reason = "the server shares lifetime atomically; its existing worker synchronization governs record access"
    )]
    pub fn new(record: WorkerRecord) -> Self {
        Self(Arc::new(Resources {
            record,
            queue: Cell::new(None),
            pipe: OnceCell::new(),
            parent: OnceCell::new(),
        }))
    }

    /// The worker, lent for as long as this owner is borrowed.
    #[must_use]
    pub fn get(&self) -> Worker<'_> {
        Worker::from(&self.0.record)
    }

    /// Create the worker's own Unix socket pair, returning its master fd.
    #[must_use]
    pub fn open_pipe(&self, socket_type: std::ffi::c_int) -> Option<std::ffi::c_int> {
        if self.0.pipe.get().is_some()
            || self.0.parent.get().is_some()
            || !self.get().pipe_object().is_null()
        {
            return None;
        }
        let pipe = self
            .0
            .pipe
            .get_or_init(|| Pipe(Mutex::new(socket_pair::unix_socket(true, socket_type))));
        let mut pair = lock(&pipe.0);
        let (master, end) = pair.raw_ends();
        if master.is_null() || end.is_null() {
            return None;
        }
        let fd = pair.master_end()?.fd();
        let raw = ptr::from_mut(&mut *pair);
        // SAFETY: the retained pair and its ends stay live until close_pipe clears
        // the record or all native owners drop after the server stops its threads.
        unsafe { self.get().set_pipes(raw.cast(), master.cast(), end.cast()) };
        self.get().use_pipe(true);
        Some(fd)
    }

    /// Close a retained pair's selected end. None means a borrowed native pair.
    /// Keep the closed pair's allocation until its last owner goes as well.
    #[must_use]
    pub fn close_pipe(&self, which: std::ffi::c_int) -> Option<bool> {
        let pipe = self
            .0
            .pipe
            .get()
            .or_else(|| self.0.parent.get()?.0.pipe.get())?;
        let closed = socket_pair::close(&mut lock(&pipe.0), which);
        if closed && which == 0 {
            self.get().clear_pipes();
        }
        Some(closed)
    }

    /// A thread's separate record shares the original worker's native resources.
    /// Retain the original queue as well as the pipe; PHP values stay outside.
    #[must_use]
    pub fn share_resources(&self, source: Self) -> bool {
        if Arc::ptr_eq(&self.0, &source.0)
            || self.0.pipe.get().is_some()
            || source.0.parent.get().is_some()
        {
            return false;
        }
        self.0.parent.set(source).is_ok()
    }

    /// Own a new queue, replacing any previously owned queue.
    pub fn own_queue(&self, queue: QueueRecord) {
        let queue = Rc::new(queue);
        self.get().set_queue(Rc::as_ptr(&queue).cast_mut().cast());
        drop(self.0.queue.replace(Some(queue)));
    }

    /// Destroy an owned queue, or detach a pool's borrowed queue.
    #[must_use]
    pub fn free_queue(&self) -> bool {
        let worker = self.get();
        if worker.queue().is_null() {
            return false;
        }
        let mut owned = self.0.queue.take();
        if owned
            .as_mut()
            .and_then(Rc::get_mut)
            .is_some_and(|queue| !queue.destroy())
        {
            self.0.queue.set(owned);
            return false;
        }
        worker.set_queue(ptr::null_mut());
        drop(owned);
        true
    }
}

struct Pipe(Mutex<SocketPair>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for Pipe {
    fn drop(&mut self) {
        socket_pair::drop(self.0.get_mut().unwrap_or_else(PoisonError::into_inner));
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/process/worker_owner.rs"]
mod tests;
