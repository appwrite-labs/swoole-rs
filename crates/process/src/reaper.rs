//! The manager's reap loop, shared by `ProcessPool::wait` and the server's
//! `Manager::wait`: wait for a child to exit, service signals, timers and worker
//! messages, start a requested reload, restart the exited worker, and walk the
//! reload one worker at a time until it completes. Rust owns the loop and the
//! reload's progress ([`ReloadTask`]); native supplies the waits, the
//! callbacks and the restart itself (fork and the Zend callbacks stay native).

use std::ffi::{c_int, c_void};

use crate::reload_task::{ReloadTask, Warn};

/// No reload was started this iteration.
pub const RELOAD_NONE: u32 = 0;
/// A reload started: signal its first worker now.
pub const RELOAD_KILL_ONE: u32 = 1;
/// The reload request was refused: start the next iteration.
pub const RELOAD_SKIP: u32 = 2;
/// A reload started and every worker was already signalled: carry on reaping.
pub const RELOAD_SIGNALLED: u32 = 3;

/// One manager's side of the loop. `ctx` is passed back to every call.
#[repr(C)]
pub struct ReapHost {
    pub ctx: *mut c_void,
    /// `ProcessPool::wait` leaves as soon as a failed wait finds it stopped; the
    /// server's manager finishes that iteration first.
    pub leave_when_stopped: bool,
    /// The server's manager ignores a non-blocking wait that found no exited child
    /// (pid 0); `ProcessPool::wait` hands it to `reap` like any other pid.
    pub skip_no_child: bool,
    pub running: unsafe extern "C" fn(ctx: *mut c_void) -> bool,
    /// Whether a reload was requested and not yet started (`ProcessPool::reload_init`).
    pub reload_requested: unsafe extern "C" fn(ctx: *mut c_void) -> bool,
    /// The reload in progress, or null.
    pub reload_task: unsafe extern "C" fn(ctx: *mut c_void) -> *mut ReloadTask,
    /// `wait()`, or `waitpid(-1, WNOHANG)` when `nonblock`: the pid, with the status
    /// and the wait's `errno` written out.
    pub wait: unsafe extern "C" fn(
        ctx: *mut c_void,
        nonblock: bool,
        status: *mut c_int,
        error: *mut c_int,
    ) -> c_int,
    /// Dispatch signals, run timers and read worker messages.
    pub service: unsafe extern "C" fn(ctx: *mut c_void),
    /// Report a failed wait (`errno` as the service step left it, as upstream).
    pub wait_failed: unsafe extern "C" fn(ctx: *mut c_void),
    /// Start a requested reload: one of the `RELOAD_*` values.
    pub start_reload: unsafe extern "C" fn(ctx: *mut c_void) -> u32,
    /// A child exited: restart its worker (or report it). False when the pid was no
    /// worker of the pool's, which ends the iteration.
    pub reap: unsafe extern "C" fn(ctx: *mut c_void, pid: c_int, status: c_int) -> bool,
    /// The reload completed: free it and run the after-reload callback.
    pub finish_reload: unsafe extern "C" fn(ctx: *mut c_void),
    pub warn: Warn,
}

impl ReapHost {
    fn running(&self) -> bool {
        // SAFETY: the host's callbacks take the context it supplied.
        unsafe { (self.running)(self.ctx) }
    }

    fn reload_requested(&self) -> bool {
        // SAFETY: as above.
        unsafe { (self.reload_requested)(self.ctx) }
    }

    #[expect(
        clippy::mut_from_ref,
        reason = "the task belongs to the native pool, not to the host; each use is one step of the loop"
    )]
    fn reload_task(&self) -> Option<&mut ReloadTask> {
        // SAFETY: as above.
        let task = unsafe { (self.reload_task)(self.ctx) };
        // SAFETY: a non-null task is the pool's live reload handle, used only for
        // this step of the loop.
        unsafe { task.as_mut() }
    }

    /// Run the loop until the host stops running.
    pub(crate) fn run(&self) {
        while self.running() {
            let nonblock = self.reload_requested();
            let mut status: c_int = 0;
            let mut error: c_int = 0;
            // SAFETY: the host's callbacks take its context; the out-pointers are locals.
            let pid = unsafe { (self.wait)(self.ctx, nonblock, &raw mut status, &raw mut error) };
            // SAFETY: as above.
            unsafe { (self.service)(self.ctx) };

            if pid < 0 {
                if self.leave_when_stopped && !self.running() {
                    break;
                }
                if self.reload_task().is_none() {
                    if error > 0 && error != libc::EINTR {
                        // SAFETY: as above.
                        unsafe { (self.wait_failed)(self.ctx) };
                    }
                    continue;
                }
            }

            let mut kill_next = false;
            if self.running() {
                // SAFETY: as above.
                match unsafe { (self.start_reload)(self.ctx) } {
                    RELOAD_KILL_ONE => kill_next = true,
                    RELOAD_SKIP => continue,
                    _ => {}
                }
                if !kill_next {
                    if pid < 0 || (self.skip_no_child && pid == 0) {
                        continue;
                    }
                    // SAFETY: as above.
                    if !unsafe { (self.reap)(self.ctx, pid, status) } {
                        continue;
                    }
                    if let Some(task) = self.reload_task() {
                        task.remove(pid);
                    }
                }
            }

            if kill_next {
                if let Some(task) = self.reload_task() {
                    task.kill_one(libc::SIGTERM, self.warn);
                }
            } else if !self.reload_requested()
                && let Some(task) = self.reload_task()
            {
                if task.completed() {
                    // SAFETY: as above.
                    unsafe { (self.finish_reload)(self.ctx) };
                } else {
                    task.kill_one(libc::SIGTERM, self.warn);
                }
            }
        }
    }
}

/// Run a manager's reap loop until it stops running.
///
/// # Safety
/// `host` points to a valid host whose callbacks stay valid for the call.
pub unsafe fn reap_loop(host: *const ReapHost) {
    // SAFETY: the caller lends a valid host for the call.
    unsafe { &*host }.run();
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
        start: VecDeque<u32>,
        task: Option<Box<ReloadTask>>,
        log: Vec<String>,
    }

    fn script(ctx: *mut c_void) -> &'static mut Script {
        unsafe { &mut *ctx.cast::<Script>() }
    }

    unsafe extern "C" fn running(ctx: *mut c_void) -> bool {
        script(ctx).running
    }
    unsafe extern "C" fn reload_requested(ctx: *mut c_void) -> bool {
        script(ctx).reload_requested
    }
    unsafe extern "C" fn reload_task(ctx: *mut c_void) -> *mut ReloadTask {
        script(ctx)
            .task
            .as_deref_mut()
            .map_or(std::ptr::null_mut(), std::ptr::from_mut)
    }
    unsafe extern "C" fn wait(
        ctx: *mut c_void,
        nonblock: bool,
        status: *mut c_int,
        error: *mut c_int,
    ) -> c_int {
        let s = script(ctx);
        let Some((pid, err)) = s.waits.pop_front() else {
            s.running = false;
            return -1;
        };
        s.log.push(format!("wait nonblock={nonblock}"));
        unsafe {
            *status = 9;
            *error = err;
        }
        pid
    }
    unsafe extern "C" fn service(ctx: *mut c_void) {
        script(ctx).log.push("service".into());
    }
    unsafe extern "C" fn wait_failed(ctx: *mut c_void) {
        script(ctx).log.push("wait failed".into());
    }
    unsafe extern "C" fn start_reload(ctx: *mut c_void) -> u32 {
        let s = script(ctx);
        let action = s.start.pop_front().unwrap_or(RELOAD_NONE);
        if action != RELOAD_NONE {
            s.reload_requested = false;
            s.log.push(format!("start {action}"));
        }
        action
    }
    unsafe extern "C" fn reap(ctx: *mut c_void, pid: c_int, status: c_int) -> bool {
        script(ctx).log.push(format!("reap {pid} {status}"));
        pid != 13
    }
    unsafe extern "C" fn finish_reload(ctx: *mut c_void) {
        let s = script(ctx);
        s.task = None;
        s.log.push("finish".into());
    }

    thread_local! {
        static WARNED: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
    }
    extern "C" fn warn(_kind: c_int, pid: c_int, _id: u32) {
        WARNED.with(|w| w.borrow_mut().push(pid));
    }

    fn run(s: &mut Script, leave_when_stopped: bool, skip_no_child: bool) {
        s.running = true;
        let host = ReapHost {
            ctx: std::ptr::from_mut(s).cast(),
            leave_when_stopped,
            skip_no_child,
            running,
            reload_requested,
            reload_task,
            wait,
            service,
            wait_failed,
            start_reload,
            reap,
            finish_reload,
            warn,
        };
        unsafe { reap_loop(&raw const host) };
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
            start: VecDeque::from([RELOAD_KILL_ONE]),
            task: Some(Box::new(reload)),
            ..Script::default()
        };
        run(&mut s, false, true);
        assert_eq!(
            s.log,
            [
                "wait nonblock=true",
                "service",
                "start 1",
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
            start: VecDeque::from([RELOAD_SKIP]),
            ..Script::default()
        };
        run(&mut s, false, true);
        assert_eq!(
            s.log,
            ["wait nonblock=false", "service", "start 2", "service"]
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
