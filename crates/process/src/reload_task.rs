//! A pool reload in progress (upstream `swoole::ReloadTask`), owned in Rust behind
//! an opaque handle: the snapshot of the workers to restart (pid -> worker record)
//! and the queue that restarts them one at a time. The manager loops
//! (`ProcessPool::wait`, `Manager::wait`) drive it; native code arms the timeout
//! timer, which the task owns and deletes when it goes.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_int;

use crate::armed_timer::ArmedTimer;
use crate::worker_record::WorkerRecord;

/// `warn(kind, pid, worker_id)`, called with `errno` still set by a failed `kill`.
pub type Warn = fn(kind: KillWarning, pid: c_int, worker_id: u32);

/// What a reload's kill reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillWarning {
    /// `kill_all` could not signal a worker (a system warning).
    KillAllFailed,
    /// `kill_all` sent SIGKILL to a worker.
    ForceKill,
    /// `kill_one` could not signal a worker (a system warning).
    KillOneFailed,
}

pub struct ReloadTask {
    workers: HashMap<c_int, u32>,
    kill_queue: VecDeque<c_int>,
    timer: Option<ArmedTimer>,
    kill: fn(c_int, c_int) -> Result<(), c_int>,
}

impl Default for ReloadTask {
    fn default() -> Self {
        Self {
            workers: HashMap::new(),
            kill_queue: VecDeque::new(),
            timer: None,
            kill,
        }
    }
}

/// What a failed `kill` means for the reload: the process is already gone
/// (`ESRCH`/`ECHILD`), so forget it, or anything else, which is reported.
fn already_gone(error: c_int) -> bool {
    error == libc::ESRCH || error == libc::ECHILD
}

/// `kill(pid, signo)`: `Ok` or the `errno` it failed with.
fn kill(pid: c_int, signo: c_int) -> Result<(), c_int> {
    if crate::os::kill(pid, signo) == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

impl ReloadTask {
    pub(crate) fn add(&mut self, pid: c_int, worker_id: u32) {
        self.workers.insert(pid, worker_id);
        self.kill_queue.push_back(pid);
    }

    /// Drop `pid` from the snapshot once it has exited. Returns whether it was there.
    pub(crate) fn remove(&mut self, pid: c_int) -> bool {
        self.workers.remove(&pid).is_some()
    }

    /// Whether every snapshot worker has exited.
    pub(crate) fn completed(&self) -> bool {
        self.workers.is_empty()
    }

    /// Snapshot `workers` and queue them to restart.
    pub fn add_workers(&mut self, workers: &[WorkerRecord]) {
        for worker in workers {
            self.add(worker.pid(), worker.id());
        }
    }

    /// Owns the timeout timer, which is deleted with the task.
    pub fn set_timer(&mut self, timer: Option<ArmedTimer>) {
        self.timer = timer;
    }

    /// Signal every worker still in the snapshot, then drop the queue.
    pub fn kill_all(&mut self, signo: c_int, warn: Warn) {
        for (&pid, &id) in &self.workers {
            match (self.kill)(pid, signo) {
                Err(error) if already_gone(error) => {}
                Err(_) => warn(KillWarning::KillAllFailed, pid, id),
                Ok(()) if signo == libc::SIGKILL => warn(KillWarning::ForceKill, pid, id),
                Ok(()) => {}
            }
        }
        self.kill_queue.clear();
    }

    /// Signal the next queued worker still in the snapshot; workers that are already
    /// gone leave the snapshot and the next one is tried.
    pub(crate) fn kill_one(&mut self, signo: c_int, warn: Warn) {
        while let Some(pid) = self.kill_queue.pop_front() {
            let Some(&id) = self.workers.get(&pid) else {
                continue;
            };
            match (self.kill)(pid, signo) {
                Err(error) if already_gone(error) => {
                    self.workers.remove(&pid);
                    continue;
                }
                Err(_) => warn(KillWarning::KillOneFailed, pid, id),
                Ok(()) => {}
            }
            break;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Pids below [`GONE`] are not ours to signal (EPERM); pids from [`GONE`] up to
    /// [`LIVE`] have exited (ESRCH); pids from [`LIVE`] up are running.
    pub(crate) const GONE: c_int = 500;
    pub(crate) const LIVE: c_int = 1000;

    thread_local! {
        static WARNED: RefCell<Vec<(KillWarning, c_int, u32)>> = const { RefCell::new(Vec::new()) };
        static SIGNALLED: RefCell<Vec<(c_int, c_int)>> = const { RefCell::new(Vec::new()) };
    }

    fn fake_kill(pid: c_int, signo: c_int) -> Result<(), c_int> {
        if pid >= LIVE {
            SIGNALLED.with(|s| s.borrow_mut().push((pid, signo)));
            Ok(())
        } else if pid >= GONE {
            Err(libc::ESRCH)
        } else {
            Err(libc::EPERM)
        }
    }

    /// A reload task whose kills are simulated (see [`GONE`] and [`LIVE`]).
    pub(crate) fn task() -> ReloadTask {
        ReloadTask {
            kill: fake_kill,
            ..ReloadTask::default()
        }
    }

    fn record(kind: KillWarning, pid: c_int, id: u32) {
        WARNED.with(|w| w.borrow_mut().push((kind, pid, id)));
    }

    fn warned() -> Vec<(KillWarning, c_int, u32)> {
        WARNED.with(RefCell::take)
    }

    fn signalled() -> Vec<(c_int, c_int)> {
        SIGNALLED.with(RefCell::take)
    }

    #[test]
    fn kill_one_signals_the_next_live_worker_and_drops_gone_ones() {
        let mut task = task();
        task.add(GONE, 1);
        task.add(LIVE, 2);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(warned(), vec![]);
        assert_eq!(signalled(), vec![(LIVE, libc::SIGTERM)]);
        assert!(!task.workers.contains_key(&GONE));
        assert!(task.workers.contains_key(&LIVE));
        assert!(task.kill_queue.is_empty());
    }

    #[test]
    fn kill_one_stops_after_one_signal() {
        let mut task = task();
        task.add(LIVE, 1);
        task.add(LIVE + 1, 2);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(signalled(), vec![(LIVE, libc::SIGTERM)]);
        assert_eq!(task.kill_queue, VecDeque::from([LIVE + 1]));
    }

    #[test]
    fn kill_one_skips_removed_workers() {
        let mut task = task();
        task.add(LIVE, 1);
        task.add(LIVE + 1, 2);
        task.workers.remove(&LIVE);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(warned(), vec![]);
        assert_eq!(signalled(), vec![(LIVE + 1, libc::SIGTERM)]);
    }

    #[test]
    fn kill_all_reports_force_kills_and_clears_the_queue() {
        let mut task = task();
        task.add(LIVE, 7);
        task.add(GONE, 8);
        task.kill_all(libc::SIGKILL, record);
        assert_eq!(warned(), vec![(KillWarning::ForceKill, LIVE, 7)]);
        assert!(task.kill_queue.is_empty());
        assert_eq!(task.workers.len(), 2);
    }

    #[test]
    fn kill_failures_other_than_gone_are_reported() {
        let mut task = task();
        task.add(1, 3);
        task.kill_all(libc::SIGTERM, record);
        assert_eq!(warned(), vec![(KillWarning::KillAllFailed, 1, 3)]);
        task.add(1, 3);
        task.kill_one(libc::SIGTERM, record);
        assert_eq!(warned(), vec![(KillWarning::KillOneFailed, 1, 3)]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no kill")]
    fn the_real_kill_reports_errno() {
        let me = crate::os::getpid();
        assert_eq!(kill(me, 0), Ok(()));
        assert_eq!(kill(c_int::MAX, 0), Err(libc::ESRCH));
    }

    #[test]
    fn snapshot_remove_complete_and_timer() {
        let mut records = [*WorkerRecord::boxed(), *WorkerRecord::boxed()];
        for (index, record) in records.iter_mut().enumerate() {
            record.set_pid(100 + c_int::try_from(index).unwrap());
            record.set_id(u32::try_from(index).unwrap());
        }
        let mut task = ReloadTask::default();
        task.add_workers(&records);
        assert!(!task.completed());
        assert!(task.remove(100));
        assert!(!task.remove(100));
        assert!(task.remove(101));
        assert!(task.completed());
        task.set_timer(Some(ArmedTimer::new(
            1,
            9,
            crate::armed_timer::tests::record,
        )));
        drop(task);
        assert_eq!(crate::armed_timer::tests::cancelled(), [9]);
    }
}
