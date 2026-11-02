//! A pool reload in progress (upstream `swoole::ReloadTask`), owned in Rust behind
//! an opaque handle: the snapshot of the workers to restart (pid -> worker record)
//! and the queue that restarts them one at a time. The manager loops
//! (`ProcessPool::wait`, `Manager::wait`) drive it; the timeout timer stays native
//! and its node is only stored here.

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_int, c_void};

use crate::worker_record::{WorkerRecord, swr_v1_worker_id, swr_v1_worker_pid};

/// `warn(kind, pid, worker_id)`, called with `errno` still set by the failed `kill`:
/// [`WARN_KILL_ALL_FAILED`], [`WARN_FORCE_KILL`] or [`WARN_KILL_ONE_FAILED`].
pub type Warn = extern "C" fn(kind: c_int, pid: c_int, worker_id: u32);

/// `kill_all` could not signal a worker (a system warning).
pub const WARN_KILL_ALL_FAILED: c_int = 1;
/// `kill_all` sent SIGKILL to a worker.
pub const WARN_FORCE_KILL: c_int = 2;
/// `kill_one` could not signal a worker (a system warning).
pub const WARN_KILL_ONE_FAILED: c_int = 3;

#[derive(Default)]
pub struct ReloadTask {
    workers: HashMap<c_int, u32>,
    kill_queue: VecDeque<c_int>,
    timer: Option<std::ptr::NonNull<c_void>>,
}

/// What a failed `kill` means for the reload: the process is already gone
/// (`ESRCH`/`ECHILD`), so forget it, or anything else, which is reported.
fn already_gone(error: c_int) -> bool {
    error == libc::ESRCH || error == libc::ECHILD
}

/// `kill(pid, signo)`: `Ok` or the `errno` it failed with.
fn kill(pid: c_int, signo: c_int) -> Result<(), c_int> {
    // SAFETY: kill(2) takes plain integers and touches no memory.
    if unsafe { libc::kill(pid, signo) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

impl ReloadTask {
    fn add(&mut self, pid: c_int, worker_id: u32) {
        self.workers.insert(pid, worker_id);
        self.kill_queue.push_back(pid);
    }

    /// Signal every worker still in the snapshot, then drop the queue.
    fn kill_all(&mut self, signo: c_int, warn: Warn) {
        for (&pid, &id) in &self.workers {
            match kill(pid, signo) {
                Err(error) if already_gone(error) => {}
                Err(_) => warn(WARN_KILL_ALL_FAILED, pid, id),
                Ok(()) if signo == libc::SIGKILL => warn(WARN_FORCE_KILL, pid, id),
                Ok(()) => {}
            }
        }
        self.kill_queue.clear();
    }

    /// Signal the next queued worker still in the snapshot; workers that are already
    /// gone leave the snapshot and the next one is tried.
    fn kill_one(&mut self, signo: c_int, warn: Warn) {
        while let Some(pid) = self.kill_queue.pop_front() {
            let Some(&id) = self.workers.get(&pid) else {
                continue;
            };
            match kill(pid, signo) {
                Err(error) if already_gone(error) => {
                    self.workers.remove(&pid);
                    continue;
                }
                Err(_) => warn(WARN_KILL_ONE_FAILED, pid, id),
                Ok(()) => {}
            }
            break;
        }
    }
}

/// Start a reload. Native frees it with [`swr_v1_process_reload_task_free`].
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_reload_task_new() -> *mut ReloadTask {
    Box::into_raw(Box::<ReloadTask>::default())
}

/// Free a reload task and return its timeout timer node (null without one) for
/// native to delete.
///
/// # Safety
/// `task` is null or a live handle from `_new`, not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_free(task: *mut ReloadTask) -> *mut c_void {
    if task.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller hands back a live handle leaked by `_new`.
    let task = unsafe { Box::from_raw(task) };
    task.timer
        .map_or(std::ptr::null_mut(), std::ptr::NonNull::as_ptr)
}

/// Store (or, with null, clear) the timeout timer node.
///
/// # Safety
/// `task` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_set_timer(
    task: *mut ReloadTask,
    timer: *mut c_void,
) {
    // SAFETY: the caller lends a live handle.
    let task = unsafe { &mut *task };
    task.timer = std::ptr::NonNull::new(timer);
}

/// Snapshot `count` workers of the array at `workers` and queue them to restart.
///
/// # Safety
/// `task` is a live handle; `workers` points to `count` live worker records.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_add_workers(
    task: *mut ReloadTask,
    workers: *const WorkerRecord,
    count: usize,
) {
    // SAFETY: the caller lends a live handle.
    let task = unsafe { &mut *task };
    for index in 0..count {
        // SAFETY: `index < count`, inside the caller's array.
        let worker = unsafe { workers.add(index) };
        // SAFETY: `worker` is a live record of the caller's array.
        let pid = unsafe { swr_v1_worker_pid(worker) };
        // SAFETY: as above.
        let id = unsafe { swr_v1_worker_id(worker) };
        task.add(pid, id);
    }
}

/// Drop `pid` from the snapshot once it has exited. Returns whether it was there.
///
/// # Safety
/// `task` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_remove(
    task: *mut ReloadTask,
    pid: c_int,
) -> bool {
    // SAFETY: the caller lends a live handle.
    let task = unsafe { &mut *task };
    task.workers.remove(&pid).is_some()
}

/// Whether every snapshot worker has exited.
///
/// # Safety
/// `task` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_completed(task: *const ReloadTask) -> bool {
    // SAFETY: the caller lends a live handle.
    unsafe { &*task }.workers.is_empty()
}

/// Send `signo` to every snapshot worker and clear the queue.
///
/// # Safety
/// `task` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_kill_all(
    task: *mut ReloadTask,
    signo: c_int,
    warn: Warn,
) {
    // SAFETY: the caller lends a live handle.
    unsafe { &mut *task }.kill_all(signo, warn);
}

/// Send `signo` to the next queued worker that is still running.
///
/// # Safety
/// `task` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_reload_task_kill_one(
    task: *mut ReloadTask,
    signo: c_int,
    warn: Warn,
) {
    // SAFETY: the caller lends a live handle.
    unsafe { &mut *task }.kill_one(signo, warn);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::process::{Child, Command};

    thread_local! {
        static WARNED: RefCell<Vec<(c_int, c_int, u32)>> = const { RefCell::new(Vec::new()) };
    }

    extern "C" fn record(kind: c_int, pid: c_int, id: u32) {
        WARNED.with(|w| w.borrow_mut().push((kind, pid, id)));
    }

    fn warned() -> Vec<(c_int, c_int, u32)> {
        WARNED.with(RefCell::take)
    }

    fn sleeper() -> Child {
        Command::new("sleep").arg("30").spawn().unwrap()
    }

    fn pid(child: &Child) -> c_int {
        c_int::try_from(child.id()).unwrap()
    }

    /// A pid no process has: reaped children are gone.
    fn gone() -> c_int {
        let mut child = Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        pid(&child)
    }

    #[test]
    fn kill_one_signals_the_next_live_worker_and_drops_gone_ones() {
        let mut live = sleeper();
        let mut task = ReloadTask::default();
        let dead = gone();
        task.add(dead, 1);
        task.add(pid(&live), 2);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(warned(), vec![]);
        assert!(!task.workers.contains_key(&dead));
        assert!(task.workers.contains_key(&pid(&live)));
        assert!(task.kill_queue.is_empty());
        assert!(!live.wait().unwrap().success());
    }

    #[test]
    fn kill_one_stops_after_one_signal() {
        let mut first = sleeper();
        let mut second = sleeper();
        let mut task = ReloadTask::default();
        task.add(pid(&first), 1);
        task.add(pid(&second), 2);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(task.kill_queue, VecDeque::from([pid(&second)]));
        first.wait().unwrap();
        second.kill().unwrap();
        second.wait().unwrap();
    }

    #[test]
    fn kill_one_skips_removed_workers() {
        let mut live = sleeper();
        let mut task = ReloadTask::default();
        task.add(1, 1);
        task.add(pid(&live), 2);
        task.workers.remove(&1);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(warned(), vec![]);
        live.wait().unwrap();
    }

    #[test]
    fn kill_all_reports_force_kills_and_clears_the_queue() {
        let mut live = sleeper();
        let mut task = ReloadTask::default();
        task.add(pid(&live), 7);
        task.add(gone(), 8);
        task.kill_all(libc::SIGKILL, record);
        assert_eq!(warned(), vec![(WARN_FORCE_KILL, pid(&live), 7)]);
        assert!(task.kill_queue.is_empty());
        assert_eq!(task.workers.len(), 2);
        live.wait().unwrap();
    }

    #[test]
    fn kill_failures_other_than_gone_are_reported() {
        // pid 1 exists but is not ours to signal (EPERM), unless the tests run as root.
        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } == 0 {
            return;
        }
        let mut task = ReloadTask::default();
        task.add(1, 3);
        task.kill_all(libc::SIGTERM, record);
        assert_eq!(warned(), vec![(WARN_KILL_ALL_FAILED, 1, 3)]);
        task.add(1, 3);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(warned(), vec![(WARN_KILL_ONE_FAILED, 1, 3)]);
    }

    #[test]
    fn ffi_snapshot_remove_complete_and_timer() {
        let mut records: [WorkerRecord; 2] = unsafe { std::mem::zeroed() };
        for (index, record) in records.iter_mut().enumerate() {
            unsafe {
                crate::worker_record::swr_v1_worker_set_pid(
                    record,
                    100 + c_int::try_from(index).unwrap(),
                );
                crate::worker_record::swr_v1_worker_set_id(record, u32::try_from(index).unwrap());
            }
        }
        let task = swr_v1_process_reload_task_new();
        let mut node = 0u8;
        let timer = std::ptr::from_mut(&mut node).cast::<c_void>();
        unsafe {
            swr_v1_process_reload_task_add_workers(task, records.as_ptr(), records.len());
            assert!(!swr_v1_process_reload_task_completed(task));
            assert!(swr_v1_process_reload_task_remove(task, 100));
            assert!(!swr_v1_process_reload_task_remove(task, 100));
            assert!(swr_v1_process_reload_task_remove(task, 101));
            assert!(swr_v1_process_reload_task_completed(task));
            swr_v1_process_reload_task_set_timer(task, timer);
            assert_eq!(swr_v1_process_reload_task_free(task), timer);
            assert!(swr_v1_process_reload_task_free(std::ptr::null_mut()).is_null());
        }
    }
}
