//! The manager's reap loop, shared by `ProcessPool::wait` and the server's
//! `Manager::wait`: wait for a child to exit, service signals, timers and worker
//! messages, start a requested reload, restart the exited worker, and walk the
//! reload one worker at a time until it completes. Rust owns the loop and the
//! reload's progress ([`ReloadTask`]); native supplies the waits, the
//! callbacks and the restart itself (fork and the Zend callbacks stay native).

use std::ffi::c_int;

use crate::reload_task::{ReloadTask, Warn};

/// What [`Reap::start_reload`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadStart {
    /// No reload was started this iteration.
    None,
    /// A reload started: signal its first worker now.
    KillOne,
    /// The reload request was refused: start the next iteration.
    Skip,
    /// A reload started and every worker was already signalled: carry on reaping.
    Signalled,
}

/// One manager's side of the loop.
pub trait Reap {
    /// `ProcessPool::wait` leaves as soon as a failed wait finds it stopped; the
    /// server's manager finishes that iteration first.
    fn leave_when_stopped(&self) -> bool;
    /// The server's manager ignores a non-blocking wait that found no exited child
    /// (pid 0); `ProcessPool::wait` hands it to `reap` like any other pid.
    fn skip_no_child(&self) -> bool;
    fn running(&mut self) -> bool;
    /// Whether a reload was requested and not yet started (`ProcessPool::reload_init`).
    fn reload_requested(&mut self) -> bool;
    /// One operation on the reload in progress, ending before callbacks run.
    fn with_reload<R>(&mut self, f: impl FnOnce(&mut ReloadTask) -> R) -> Option<R>;
    /// `wait()`, or `waitpid(-1, WNOHANG)` when `nonblock`: the pid, the status and
    /// the wait's `errno`.
    fn wait(&mut self, nonblock: bool) -> Waited;
    /// Dispatch signals, run timers and read worker messages.
    fn service(&mut self);
    /// Report a failed wait (`errno` as the service step left it, as upstream).
    fn wait_failed(&mut self);
    /// Start a requested reload.
    fn start_reload(&mut self) -> ReloadStart;
    /// A child exited: restart its worker (or report it). False when the pid was no
    /// worker of the pool's, which ends the iteration.
    fn reap(&mut self, pid: c_int, status: c_int) -> bool;
    /// The reload completed: free it and run the after-reload callback.
    fn finish_reload(&mut self);
    fn warn(&self) -> Warn;
}

/// What one wait returned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Waited {
    pub pid: c_int,
    pub status: c_int,
    pub error: c_int,
}

/// `wait()`, or `waitpid(-1, WNOHANG)` when `nonblock`.
#[must_use]
pub fn wait_child(nonblock: bool) -> Waited {
    let mut status = 0;
    // `wait(&status)` is `waitpid(-1, &status, 0)`.
    let options = if nonblock { libc::WNOHANG } else { 0 };
    let pid = swoole_rs_corebase::sys::waitpid(-1, Some(&mut status), options);
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    Waited { pid, status, error }
}

/// Run a manager's reap loop until it stops running.
pub fn run(host: &mut impl Reap) {
    let warn = host.warn();
    while host.running() {
        let nonblock = host.reload_requested();
        let Waited { pid, status, error } = host.wait(nonblock);
        host.service();

        if pid < 0 {
            if host.leave_when_stopped() && !host.running() {
                break;
            }
            if host.with_reload(|_| ()).is_none() {
                if error > 0 && error != libc::EINTR {
                    host.wait_failed();
                }
                continue;
            }
        }

        let mut kill_next = false;
        if host.running() {
            match host.start_reload() {
                ReloadStart::KillOne => kill_next = true,
                ReloadStart::Skip => continue,
                ReloadStart::None | ReloadStart::Signalled => {}
            }
            if !kill_next {
                if pid < 0 || (host.skip_no_child() && pid == 0) {
                    continue;
                }
                if !host.reap(pid, status) {
                    continue;
                }
                host.with_reload(|task| task.remove(pid));
            }
        }

        if kill_next {
            host.with_reload(|task| task.kill_one(libc::SIGTERM, warn));
        } else if !host.reload_requested() {
            let completed = host.with_reload(|task| {
                let completed = task.completed();
                if !completed {
                    task.kill_one(libc::SIGTERM, warn);
                }
                completed
            });
            if completed == Some(true) {
                host.finish_reload();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reload_task::tests::{GONE, task};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// A scripted manager: each wait pops the next (pid, error) and the log
    /// records every call the loop makes.
    #[derive(Default)]
    struct Script {
        waits: VecDeque<(c_int, c_int)>,
        running: bool,
        reload_requested: bool,
        start: VecDeque<ReloadStart>,
        task: Option<Box<ReloadTask>>,
        log: Vec<String>,
        leave_when_stopped: bool,
        skip_no_child: bool,
    }

    impl Reap for Script {
        fn leave_when_stopped(&self) -> bool {
            self.leave_when_stopped
        }
        fn skip_no_child(&self) -> bool {
            self.skip_no_child
        }
        fn running(&mut self) -> bool {
            self.running
        }
        fn reload_requested(&mut self) -> bool {
            self.reload_requested
        }
        fn with_reload<R>(&mut self, f: impl FnOnce(&mut ReloadTask) -> R) -> Option<R> {
            self.task.as_deref_mut().map(f)
        }
        fn wait(&mut self, nonblock: bool) -> Waited {
            let Some((pid, error)) = self.waits.pop_front() else {
                self.running = false;
                return Waited {
                    pid: -1,
                    ..Waited::default()
                };
            };
            self.log.push(format!("wait nonblock={nonblock}"));
            Waited {
                pid,
                status: 9,
                error,
            }
        }
        fn service(&mut self) {
            self.log.push("service".into());
        }
        fn wait_failed(&mut self) {
            self.log.push("wait failed".into());
        }
        fn start_reload(&mut self) -> ReloadStart {
            let action = self.start.pop_front().unwrap_or(ReloadStart::None);
            if action != ReloadStart::None {
                self.reload_requested = false;
                self.log.push(format!("start {action:?}"));
            }
            action
        }
        fn reap(&mut self, pid: c_int, status: c_int) -> bool {
            self.log.push(format!("reap {pid} {status}"));
            pid != 13
        }
        fn finish_reload(&mut self) {
            self.task = None;
            self.log.push("finish".into());
        }
        fn warn(&self) -> Warn {
            warn
        }
    }

    thread_local! {
        static WARNED: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
    }
    fn warn(_kind: crate::reload_task::KillWarning, pid: c_int, _id: u32) {
        WARNED.with(|w| w.borrow_mut().push(pid));
    }

    fn run(s: &mut Script, leave_when_stopped: bool, skip_no_child: bool) {
        s.running = true;
        s.leave_when_stopped = leave_when_stopped;
        s.skip_no_child = skip_no_child;
        super::run(s);
    }

    #[test]
    fn reaps_each_exit_and_reports_failed_waits_except_eintr() {
        let mut s = Script {
            waits: VecDeque::from([(10, 0), (-1, libc::EINTR), (-1, libc::ECHILD), (11, 0)]),
            ..Script::default()
        };
        run(&mut s, false, true);
        assert_eq!(
            s.log,
            [
                "wait nonblock=false",
                "service",
                "reap 10 9",
                "wait nonblock=false",
                "service",
                "wait nonblock=false",
                "service",
                "wait failed",
                "wait nonblock=false",
                "service",
                "reap 11 9",
                "service",
            ]
        );
    }

    #[test]
    fn a_reload_signals_one_worker_per_exit_until_complete() {
        // Gone pids: kill_one drops them from the snapshot without a warning.
        let mut reload = task();
        let gone = GONE;
        reload.add(gone, 1);
        let mut s = Script {
            waits: VecDeque::from([(0, 0), (gone, 0)]),
            reload_requested: true,
            start: VecDeque::from([ReloadStart::KillOne]),
            task: Some(Box::new(reload)),
            ..Script::default()
        };
        run(&mut s, false, true);
        assert_eq!(
            s.log,
            [
                "wait nonblock=true",
                "service",
                "start KillOne",
                "wait nonblock=false",
                "service",
                format!("reap {gone} 9").as_str(),
                "finish",
                "service",
            ]
        );
        assert_eq!(WARNED.with(|w| w.borrow().len()), 0);
    }

    #[test]
    fn an_unknown_pid_ends_the_iteration() {
        let mut s = Script {
            waits: VecDeque::from([(13, 0)]),
            task: Some(Box::new(task())),
            ..Script::default()
        };
        run(&mut s, true, false);
        assert_eq!(
            s.log,
            ["wait nonblock=false", "service", "reap 13 9", "service"]
        );
        assert!(s.task.is_some());
    }

    #[test]
    fn a_skipped_reload_starts_the_next_iteration() {
        let mut s = Script {
            waits: VecDeque::from([(12, 0)]),
            start: VecDeque::from([ReloadStart::Skip]),
            ..Script::default()
        };
        run(&mut s, false, true);
        assert_eq!(
            s.log,
            ["wait nonblock=false", "service", "start Skip", "service"]
        );
    }

    #[test]
    fn pid_zero_is_reaped_only_by_the_pool() {
        let mut reload = task();
        reload.add(GONE, 1);
        let mut manager = Script {
            waits: VecDeque::from([(0, 0)]),
            task: Some(Box::new(task())),
            ..Script::default()
        };
        run(&mut manager, false, true);
        assert!(!manager.log.iter().any(|line| line.starts_with("reap")));
        let mut pool = Script {
            waits: VecDeque::from([(0, 0)]),
            task: Some(Box::new(reload)),
            ..Script::default()
        };
        run(&mut pool, true, false);
        assert!(pool.log.contains(&"reap 0 9".to_owned()));
    }

    #[test]
    fn the_pool_leaves_on_a_failed_wait_once_stopped() {
        let mut s = Script {
            task: Some(Box::new(task())),
            ..Script::default()
        };
        run(&mut s, true, false);
        assert_eq!(s.log, ["service"]);
        assert!(s.task.is_some());
        let mut manager = Script {
            task: Some(Box::new(task())),
            ..Script::default()
        };
        run(&mut manager, false, true);
        assert_eq!(manager.log, ["service", "finish"]);
    }
}
