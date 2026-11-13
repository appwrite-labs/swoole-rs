//! The server's manager (upstream's `src/server/manager.cc`): it forks the workers,
//! reaps and restarts them, runs reloads and stops them all on shutdown. The
//! manager's state and the reload it starts are here; forking, reaping and the
//! signals are in `native`.
#![cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(dead_code, reason = "only the hybrid runs the manager")
)]
use core::cell::{Cell, RefCell};
use core::ffi::c_int;
use core::ops::Range;

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod native;

/// What upstream's `struct Manager` held: the requested reloads, and the pids
/// SIGALRM kills once `max_wait_time` runs out on shutdown. Signal handlers reach
/// it through `Server::manager_` while the manager runs, so every field is a cell.
#[derive(Default)]
pub(crate) struct Manager {
    reload_all_workers: Cell<bool>,
    reload_task_workers: Cell<bool>,
    force_kill: Cell<bool>,
    kill_workers: RefCell<Vec<c_int>>,
}

/// The reload the manager starts next.
#[derive(Debug, PartialEq, Eq)]
enum Reload {
    None,
    /// Every worker.
    All,
    /// The task workers.
    Tasks,
    /// The task workers, of which there are none.
    NoTaskWorkers,
}

impl Manager {
    /// `Server::reload`'s request, which the manager's next round starts.
    fn request(&self, all: bool) {
        if all {
            self.reload_all_workers.set(true);
        } else {
            self.reload_task_workers.set(true);
        }
    }

    /// Arms SIGALRM's force kill of `pids`, the children shutdown waits for.
    fn arm_kill(&self, pids: Vec<c_int>) {
        self.force_kill.set(true);
        *self.kill_workers.borrow_mut() = pids;
    }

    /// The pids SIGALRM kills: none until shutdown arms it.
    fn kill_list(&self) -> Option<Vec<c_int>> {
        let pids = || self.kill_workers.borrow().clone();
        self.force_kill.get().then(pids)
    }

    /// Takes the pending request: every worker's first, then the task workers'.
    fn take_reload(&self, task_workers: u32) -> Reload {
        if self.reload_all_workers.replace(false) {
            Reload::All
        } else if !self.reload_task_workers.replace(false) {
            Reload::None
        } else if task_workers == 0 {
            Reload::NoTaskWorkers
        } else {
            Reload::Tasks
        }
    }
}

/// `serv->get_worker(id)`'s ids of the event workers, or of the task workers, which
/// it numbers after them.
fn worker_ids(events: bool, worker_num: u32, task_worker_num: u32) -> Range<u32> {
    if events {
        0..worker_num
    } else {
        worker_num..worker_num.saturating_add(task_worker_num)
    }
}

/// A `WorkerStopMessage`'s pid and worker id, from a stop message's payload.
fn stop_message(data: &[u8]) -> (libc::pid_t, u16) {
    let pid = data.first_chunk().copied().map_or(0, i32::from_ne_bytes);
    let id = data.get(4..6).and_then(|id| id.first_chunk().copied());
    (pid, id.map_or(0, u16::from_ne_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reloads_start_from_the_managers_requests() {
        let manager = Manager::default();
        manager.request(true);
        assert_eq!(manager.take_reload(1), Reload::All);
        assert_eq!(manager.take_reload(1), Reload::None);
        manager.request(false);
        assert_eq!(manager.take_reload(1), Reload::Tasks);
        manager.request(false);
        assert_eq!(manager.take_reload(0), Reload::NoTaskWorkers);
        // Every worker's request first; the task workers' waits for the next round.
        manager.request(false);
        manager.request(true);
        assert_eq!(manager.take_reload(1), Reload::All);
        assert_eq!(manager.take_reload(1), Reload::Tasks);
    }

    #[test]
    fn sigalrm_kills_only_once_shutdown_arms_it() {
        let manager = Manager::default();
        assert_eq!(manager.kill_list(), None);
        manager.arm_kill(vec![10, 11]);
        assert_eq!(manager.kill_list(), Some(vec![10, 11]));
    }

    #[test]
    fn task_workers_are_numbered_after_the_event_workers() {
        assert_eq!(worker_ids(true, 2, 3), 0..2);
        assert_eq!(worker_ids(false, 2, 3), 2..5);
    }

    #[test]
    fn a_stop_message_names_the_worker_to_replace() {
        let mut data = [0u8; 16];
        data[..4].copy_from_slice(&4242i32.to_ne_bytes());
        data[4..6].copy_from_slice(&3u16.to_ne_bytes());
        assert_eq!(stop_message(&data), (4242, 3));
        assert_eq!(stop_message(&[]), (0, 0));
    }
}
