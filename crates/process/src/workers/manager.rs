//! The server's manager (upstream's `src/server/manager.cc`; its native ops are
//! workers.cc's `ManagerContext`): it forks the workers, reaps and
//! restarts them, runs reloads and stops them all on shutdown. Rust owns the
//! manager's state ([`Manager`], which native holds as the opaque `Manager *`
//! `Server::manager_`) and drives `reaper`'s loop with its own host; native code
//! keeps the server, its pools, fork and the callbacks.
use core::cell::{Cell, RefCell};
use core::ffi::c_int;

use super::event_worker::EVENT_WORKER;
use super::log::{
    LOG_FORK_EVENT_WORKER, LOG_FORK_MANAGER, LOG_FORK_USER_WORKER, LOG_KILL_TIMEOUT,
    LOG_NO_TASK_RELOAD, LOG_RELOAD_SINGLE_PROCESS, LOG_RELOADING, LOG_START_TASK_POOL,
    LOG_UNKNOWN_CHILD, LOG_UNKNOWN_MESSAGE, LOG_UNKNOWN_WORKER_TYPE, LOG_WAIT_FAILED,
};
use super::ops::{
    EVENT_COMMAND_REQUEST, LOG_UNKNOWN_COMMAND, OP_ALL_WORKER_NUM, OP_ANY_WORKER_PID,
    OP_CHECK_EXIT_STATUS, OP_COMMAND_REPLY, OP_CREATE_EVENT_WORKERS, OP_CREATE_MESSAGE_BOX,
    OP_CREATE_USER_WORKERS, OP_DEATH_SIGNAL, OP_DETACH_STOPPED, OP_DROP_RELOAD_TASK, OP_EVENT_FREE,
    OP_EVENT_WORKER_PID, OP_FORK_MANAGER, OP_HAS_COMMAND, OP_HAS_MESSAGE_BOX,
    OP_HAS_ON_WORKER_MESSAGE, OP_HAS_TIMER, OP_HAS_USER, OP_INFO_TYPE, OP_IS_MANAGER,
    OP_KILL_EVENT_WORKERS, OP_KILL_TASK_WORKERS, OP_KILL_USER_WORKERS, OP_MANAGER,
    OP_MANAGER_BARRIER, OP_MANAGER_INIT, OP_MANAGER_PID, OP_MAX_WAIT_TIME, OP_MESSAGE_TYPE,
    OP_MODE, OP_ON_AFTER_RELOAD, OP_ON_BEFORE_RELOAD, OP_ON_MANAGER_START, OP_ON_MANAGER_STOP,
    OP_ON_WORKER_MESSAGE, OP_POOL_RELOAD, OP_POP_MESSAGE, OP_READ_MESSAGE, OP_REAP_DETACHED,
    OP_REAP_WAIT, OP_RELOAD_ADD_EVENT_WORKERS, OP_RELOAD_ADD_TASK_WORKERS, OP_RELOAD_ASYNC,
    OP_RELOAD_INIT, OP_RELOAD_KILL_ALL, OP_RELOAD_TASK, OP_RELOAD_THREADS, OP_RELOAD_WARN,
    OP_REOPEN_LOGGER, OP_RESPAWN_EVENT_WORKER, OP_RESPAWN_TASK_WORKER, OP_RESPAWN_USER_WORKER,
    OP_RESTART_WORKER, OP_ROOT_USER, OP_RUNNING, OP_SERVER_RELOAD, OP_SET_ISOLATION,
    OP_SET_MANAGER_PID, OP_SET_MANAGER_TYPE, OP_SET_READ_MESSAGE, OP_SET_RELOAD_INIT,
    OP_SET_RUNNING, OP_SIGNAL_DISPATCH, OP_SPAWN_EVENT_WORKER, OP_SPAWN_TASK_WORKER,
    OP_SPAWN_USER_WORKER, OP_START_TASK_POOL, OP_STOP_MESSAGE_WORKER, OP_TASK_POOL_WORKER_NUM,
    OP_TASK_WORKER_NUM, OP_TASK_WORKER_PID, OP_TIMER_AVAILABLE, OP_TIMER_FREE, OP_TIMER_SELECT,
    OP_UNREGISTER_PID, OP_USER_WORKER_NUM, OP_WAIT_ERROR, OP_WAIT_STATUS, OP_WORKER_BY_PID,
    OP_WORKER_NUM, OP_WORKER_REGISTRY, OP_WORKER_TYPE, RUN_MANAGER_MAIN, RUN_MANAGER_SIGNAL,
    RUN_READ_WORKER_MESSAGE, RUN_RELOAD, RUN_RESTART_WORKER, RUN_START_MANAGER,
};
use super::{
    HANDLER_NONE, HANDLER_OWN, Host, MODE_BASE, MODE_PROCESS, MODE_THREAD, SW_ERR, SW_OK,
    set_signal_all,
};
use crate::os::kill;
use crate::reaper::{
    self, RELOAD_KILL_ONE, RELOAD_NONE, RELOAD_SIGNALLED, RELOAD_SKIP, Reap, Waited,
};
use crate::reload_task::{ReloadTask, Warn};
use crate::worker_registry::{CATEGORY_USER, WorkerRegistry};

/// `swoole::WorkerType`.
const TASK_WORKER: u64 = 4;
const USER_WORKER: u64 = 5;

#[cfg(test)]
pub(super) fn worker_types() -> (u64, u64, u64) {
    (EVENT_WORKER, TASK_WORKER, USER_WORKER)
}

/// What upstream's `struct Manager` held: the requested reloads, and the pids
/// SIGALRM kills once `max_wait_time` runs out on shutdown. Signal handlers reach
/// it through `Server::manager_` while the manager runs, so every field is a cell.
#[derive(Default)]
pub(super) struct Manager {
    reload_all_workers: Cell<bool>,
    reload_task_workers: Cell<bool>,
    force_kill: Cell<bool>,
    kill_workers: RefCell<Vec<c_int>>,
}

/// `Server::manager_`, when the manager runs in this process.
fn manager<'a, H: Host + ?Sized>(h: &mut H) -> Option<&'a Manager> {
    let address = usize::try_from(h.get(OP_MANAGER)).ok()?;
    // SAFETY: a non-null `Server::manager_` is the `Manager` that `wait` keeps
    // alive on its stack while the manager runs; only shared references exist.
    unsafe { (address as *const Manager).as_ref() }
}

pub(super) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
        RUN_START_MANAGER => start(h),
        RUN_MANAGER_MAIN => {
            main(h);
            SW_OK
        }
        RUN_MANAGER_SIGNAL => {
            signal(h, i32::try_from(arg).unwrap_or(0));
            SW_OK
        }
        RUN_RESTART_WORKER => restart(h, arg),
        RUN_READ_WORKER_MESSAGE => {
            let kind = h.get(OP_INFO_TYPE);
            if u32::try_from(kind).ok() != Some(EVENT_COMMAND_REQUEST) {
                h.log(LOG_UNKNOWN_MESSAGE, kind.cast_signed(), 0);
            } else if !h.is(OP_HAS_COMMAND) {
                h.get(LOG_UNKNOWN_COMMAND);
            } else {
                h.get(OP_COMMAND_REPLY);
            }
            SW_OK
        }
        RUN_RELOAD => i64::from(reload(h, arg != 0)),
        _ => SW_ERR,
    }
}

/// `Server::start_manager_process`: fork the manager (the base mode's master is
/// its own manager).
fn start<H: Host>(h: &mut H) -> i64 {
    if !h.is(OP_CREATE_EVENT_WORKERS) {
        return SW_ERR;
    }
    if !h.is(OP_HAS_MESSAGE_BOX) && h.signed(OP_CREATE_MESSAGE_BOX) == SW_ERR {
        return SW_ERR;
    }
    if h.get(OP_USER_WORKER_NUM) > 0 && !h.is(OP_CREATE_USER_WORKERS) {
        return SW_ERR;
    }
    if h.get(OP_MODE) == MODE_BASE {
        main(h);
    } else if h.signed(OP_FORK_MANAGER) < 0 {
        h.log(LOG_FORK_MANAGER, 0, 0);
        return SW_ERR;
    }
    SW_OK
}

/// The manager process: start the task pool, fork the workers, then reap.
fn main<H: Host>(h: &mut H) {
    h.get(OP_SET_MANAGER_PID);
    // A SIGTERM while the workers are still forking must stop the manager once
    // they are up, not kill it outright and orphan the ones already forked.
    // The children's fork resets their handlers.
    set_signal_all(h, &[(libc::SIGTERM, HANDLER_OWN)]);
    if h.get(OP_TASK_WORKER_NUM) > 0 && h.signed(OP_START_TASK_POOL) == SW_ERR {
        h.log(LOG_START_TASK_POOL, 0, 0);
        return;
    }
    // After the task pool starts, which sets the process type to SW_MASTER.
    h.get(OP_SET_MANAGER_TYPE);
    for worker in 0..h.get(OP_WORKER_NUM) {
        if h.op(OP_SPAWN_EVENT_WORKER, worker).cast_signed() < 0 {
            h.log(LOG_FORK_EVENT_WORKER, 0, 0);
            return;
        }
    }
    for worker in 0..h.get(OP_USER_WORKER_NUM) {
        if h.op(OP_SPAWN_USER_WORKER, worker).cast_signed() < 0 {
            h.log(LOG_FORK_USER_WORKER, 0, 0);
            return;
        }
    }
    wait(h);
}

/// `Manager::wait`: the manager's life after forking.
fn wait<H: Host>(h: &mut H) {
    let manager = Manager::default();
    h.op(OP_MANAGER_INIT, core::ptr::from_ref(&manager) as u64);
    h.get(OP_EVENT_FREE);
    set_signal_all(h, &[(libc::SIGHUP, HANDLER_NONE)]);
    set_signal_all(
        h,
        &[
            (libc::SIGCHLD, HANDLER_OWN),
            (libc::SIGTERM, HANDLER_OWN),
            (libc::SIGUSR1, HANDLER_OWN),
            (libc::SIGUSR2, HANDLER_OWN),
            (libc::SIGIO, HANDLER_OWN),
            (libc::SIGALRM, HANDLER_OWN),
            (libc::SIGWINCH, HANDLER_OWN),
            (super::SIGNAL_RTMIN, HANDLER_OWN),
        ],
    );
    if h.get(OP_MODE) == MODE_PROCESS {
        h.get(OP_DEATH_SIGNAL);
        h.get(OP_MANAGER_BARRIER);
        if h.is(OP_RELOAD_ASYNC) && h.is(OP_ROOT_USER) && h.is(OP_HAS_USER) {
            h.get(OP_SET_ISOLATION);
        }
    }
    h.get(OP_ON_MANAGER_START);

    reap(h, &manager);

    h.get(OP_DROP_RELOAD_TASK);
    if h.is(OP_TIMER_AVAILABLE) {
        h.get(OP_TIMER_FREE);
        // The timer overrode SIGALRM's handler: the manager needs it back.
        set_signal_all(h, &[(libc::SIGALRM, HANDLER_OWN)]);
    }
    // Wait for the children.
    let max_wait_time = h.get(OP_MAX_WAIT_TIME);
    if max_wait_time != 0 {
        manager.force_kill.set(true);
        let mut pids = Vec::new();
        for worker in 0..h.get(OP_WORKER_NUM) {
            pids.push(pid(h.op(OP_EVENT_WORKER_PID, worker)));
        }
        if h.get(OP_TASK_WORKER_NUM) > 0 {
            for worker in 0..h.get(OP_TASK_POOL_WORKER_NUM) {
                pids.push(pid(h.op(OP_TASK_WORKER_PID, worker)));
            }
        }
        pids.extend(user_worker_pids(h));
        *manager.kill_workers.borrow_mut() = pids;
        // Twice max_wait_time, so as not to race the workers' own deadline.
        alarm(u32::try_from(max_wait_time.saturating_mul(2)).unwrap_or(u32::MAX));
    }
    h.get(OP_KILL_EVENT_WORKERS);
    h.get(OP_KILL_TASK_WORKERS);
    h.get(OP_KILL_USER_WORKERS);
    // Force kill.
    if max_wait_time != 0 {
        alarm(0);
    }
    h.get(OP_ON_MANAGER_STOP);
}

fn user_worker_pids<H: Host>(h: &mut H) -> Vec<c_int> {
    let address = usize::try_from(h.get(OP_WORKER_REGISTRY)).unwrap_or(0);
    // SAFETY: the manager's server owns this registry throughout the call. No
    // callback runs or registry mutation occurs while its pids are copied.
    unsafe { (address as *const WorkerRegistry).as_ref() }
        .map_or_else(Vec::new, |registry| registry.pids(CATEGORY_USER).collect())
}

fn pid(value: u64) -> c_int {
    c_int::try_from(value.cast_signed()).unwrap_or(-1)
}

fn alarm(seconds: u32) {
    // SAFETY: alarm only (re)arms this process's SIGALRM timer.
    unsafe { libc::alarm(seconds) };
}

/// The reap loop's host: each step asks the native server through the `Host`.
struct Reaping<'a> {
    host: &'a mut dyn Host,
    manager: &'a Manager,
    warn: Warn,
}

fn pid_status(value: u64) -> c_int {
    c_int::try_from(value.cast_signed()).unwrap_or(0)
}

/// `swoole::WorkerMessageType::SW_WORKER_MESSAGE_STOP`.
const WORKER_MESSAGE_STOP: u64 = 1;

impl Reap for Reaping<'_> {
    fn leave_when_stopped(&self) -> bool {
        false
    }

    fn skip_no_child(&self) -> bool {
        true
    }

    fn running(&mut self) -> bool {
        self.host.is(OP_RUNNING)
    }

    fn reload_requested(&mut self) -> bool {
        self.host.is(OP_RELOAD_INIT)
    }

    fn active_reload(&mut self) -> Option<&mut ReloadTask> {
        let address = usize::try_from(self.host.get(OP_RELOAD_TASK)).unwrap_or(0);
        // SAFETY: the server pool's reload, live until `OP_DROP_RELOAD_TASK`; each
        // use is one step of the loop.
        unsafe { (address as *mut ReloadTask).as_mut() }
    }

    fn wait(&mut self, nonblock: bool) -> Waited {
        let pid = pid(self.host.op(OP_REAP_WAIT, u64::from(nonblock)));
        Waited {
            pid,
            status: pid_status(self.host.get(OP_WAIT_STATUS)),
            error: pid_status(self.host.get(OP_WAIT_ERROR)),
        }
    }

    /// Dispatch signals, take the workers' messages (a stopping worker's asks for
    /// its replacement) and run the timers.
    fn service(&mut self) {
        let host = &mut *self.host;
        host.get(OP_SIGNAL_DISPATCH);
        if host.is(OP_READ_MESSAGE) {
            while host.signed(OP_POP_MESSAGE) > 0 {
                if !host.is(OP_RUNNING) {
                    continue;
                }
                if host.get(OP_MESSAGE_TYPE) != WORKER_MESSAGE_STOP
                    && host.is(OP_HAS_ON_WORKER_MESSAGE)
                {
                    host.get(OP_ON_WORKER_MESSAGE);
                    continue;
                }
                let worker = host.get(OP_STOP_MESSAGE_WORKER);
                if worker >= host.get(OP_WORKER_NUM) {
                    host.op(OP_SPAWN_TASK_WORKER, worker);
                } else {
                    host.op(OP_SPAWN_EVENT_WORKER, worker);
                }
                host.get(OP_DETACH_STOPPED);
            }
            host.op(OP_SET_READ_MESSAGE, 0);
        }
        if host.is(OP_HAS_TIMER) {
            host.get(OP_TIMER_SELECT);
        }
    }

    fn wait_failed(&mut self) {
        self.host.log(LOG_WAIT_FAILED, 0, 0);
    }

    /// Start a requested reload: of every worker, or of the task workers only.
    fn start_reload(&mut self) -> u32 {
        let Reaping { host, manager, .. } = self;
        if manager.reload_all_workers.get() {
            host.op(OP_SET_RELOAD_INIT, 0);
            manager.reload_all_workers.set(false);
            host.log(LOG_RELOADING, 1, 0);
            host.get(OP_ON_BEFORE_RELOAD);
            host.get(OP_RELOAD_ADD_EVENT_WORKERS);
            if host.get(OP_TASK_WORKER_NUM) > 0 {
                host.get(OP_RELOAD_ADD_TASK_WORKERS);
            }
            // With reload_async every worker gets SIGTERM now: it stops reading, asks
            // the manager for its replacement and exits once its work is done, so
            // the old and the new worker briefly run side by side.
            if !host.is(OP_RELOAD_ASYNC) {
                return RELOAD_KILL_ONE;
            }
            host.get(OP_RELOAD_KILL_ALL);
            RELOAD_SIGNALLED
        } else if manager.reload_task_workers.get() {
            host.op(OP_SET_RELOAD_INIT, 0);
            manager.reload_task_workers.set(false);
            if host.get(OP_TASK_WORKER_NUM) == 0 {
                host.log(LOG_NO_TASK_RELOAD, 0, 0);
                return RELOAD_SKIP;
            }
            host.log(LOG_RELOADING, 0, 0);
            host.get(OP_ON_BEFORE_RELOAD);
            host.get(OP_RELOAD_ADD_TASK_WORKERS);
            RELOAD_KILL_ONE
        } else {
            RELOAD_NONE
        }
    }

    /// A child exited: restart it in place (not through a signal: `wait` names it).
    fn reap(&mut self, pid: c_int, status: c_int) -> bool {
        let exited = u64::from(pid.cast_unsigned()) | (u64::from(status.cast_unsigned()) << 32);
        self.host.op(OP_RESTART_WORKER, exited);
        true
    }

    fn finish_reload(&mut self) {
        self.host.get(OP_DROP_RELOAD_TASK);
        self.host.get(OP_ON_AFTER_RELOAD);
    }

    fn warn(&self) -> Warn {
        self.warn
    }
}

extern "C" fn no_warn(_kind: c_int, _pid: c_int, _worker_id: u32) {}

/// Native `reload_warn` (`OP_RELOAD_WARN`), which logs a reload's failed kill.
fn reload_warn<H: Host>(h: &mut H) -> Warn {
    let address = usize::try_from(h.get(OP_RELOAD_WARN)).unwrap_or(0);
    if address == 0 {
        return no_warn;
    }
    // SAFETY: native passes the address of `reload_warn`, a C-ABI function
    // with exactly `Warn`'s signature.
    unsafe { core::mem::transmute::<usize, Warn>(address) }
}

fn reap<H: Host>(h: &mut H, manager: &Manager) {
    let warn = reload_warn(h);
    reaper::run(&mut Reaping {
        host: h,
        manager,
        warn,
    });
}

/// `Manager::signal_handler`. A newly forked worker briefly inherits these
/// handlers; it must not act as the manager (reopening the log would send
/// SIGWINCH back to itself forever).
fn signal<H: Host>(h: &mut H, signo: i32) {
    if signo == libc::SIGTERM {
        // Before `wait` sets up the manager too: `main` takes it while forking.
        if h.is(OP_IS_MANAGER) {
            h.op(OP_SET_RUNNING, 0);
        }
        return;
    }
    let Some(manager) = manager(h) else {
        return;
    };
    if !h.is(OP_IS_MANAGER) {
        return;
    }
    if signo == libc::SIGUSR1 {
        h.op(OP_SERVER_RELOAD, 1);
    } else if signo == libc::SIGUSR2 {
        h.op(OP_SERVER_RELOAD, 0);
    } else if signo == libc::SIGIO {
        h.op(OP_SET_READ_MESSAGE, 1);
    } else if signo == libc::SIGALRM {
        if manager.force_kill.get() {
            // Manager::terminate_all_worker
            alarm(0);
            h.log(LOG_KILL_TIMEOUT, 0, 0);
            for &pid in manager.kill_workers.borrow().iter() {
                kill(pid, libc::SIGKILL);
            }
        }
    } else if signo == libc::SIGWINCH || crate::is_sigrtmin(signo) {
        // Manager::reopen_logger
        h.get(OP_REOPEN_LOGGER);
        for worker in 0..h.get(OP_ALL_WORKER_NUM) {
            kill(pid(h.op(OP_ANY_WORKER_PID, worker)), libc::SIGWINCH);
        }
    }
}

/// `Server::restart_worker_process`: replace the worker whose process exited.
fn restart<H: Host>(h: &mut H, exited: u64) -> i64 {
    let pid = exited & 0xffff_ffff;
    if h.op(OP_WORKER_BY_PID, pid) == 0 {
        // A child that is not a worker: a detached worker finishing, or unknown.
        if h.op(OP_REAP_DETACHED, pid) == 0 {
            h.log(LOG_UNKNOWN_CHILD, pid.cast_signed(), 0);
        }
        return SW_ERR;
    }
    // Drop the stale pid; the respawn registers the new one.
    h.op(OP_UNREGISTER_PID, pid);
    h.op(OP_CHECK_EXIT_STATUS, exited);
    match h.get(OP_WORKER_TYPE) {
        EVENT_WORKER => h.signed(OP_RESPAWN_EVENT_WORKER),
        TASK_WORKER => h.signed(OP_RESPAWN_TASK_WORKER),
        USER_WORKER => h.signed(OP_RESPAWN_USER_WORKER),
        kind => {
            h.log(
                LOG_UNKNOWN_WORKER_TYPE,
                pid.cast_signed(),
                kind.cast_signed(),
            );
            SW_ERR
        }
    }
}

/// `Server::reload`: ask the manager to reload every worker, or the task workers.
fn reload<H: Host>(h: &mut H, all: bool) -> bool {
    if h.get(OP_MODE) == MODE_THREAD {
        return h.op(OP_RELOAD_THREADS, u64::from(all)) != 0;
    }
    let manager_pid = h.signed(OP_MANAGER_PID);
    if manager_pid == 0 {
        h.log(LOG_RELOAD_SINGLE_PROCESS, 0, 0);
        return false;
    }
    if i64::from(crate::os::getpid()) != manager_pid {
        let signo = if all { libc::SIGUSR1 } else { libc::SIGUSR2 };
        return kill(pid(manager_pid.cast_unsigned()), signo) == 0;
    }
    if !h.is(OP_POOL_RELOAD) {
        return false;
    }
    if let Some(manager) = manager(h) {
        if all {
            manager.reload_all_workers.set(true);
        } else {
            manager.reload_task_workers.set(true);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::ops::OP_SIGNAL_SET;
    use crate::workers::script::Script;

    fn run_op(h: &mut Script, operation: u32, arg: u64) -> i64 {
        crate::workers::run(h, operation, arg)
    }

    #[test]
    fn start_stops_at_the_first_failure_and_forks_outside_base_mode() {
        let mut h = Script::default();
        assert_eq!(run_op(&mut h, RUN_START_MANAGER, 0), SW_ERR);
        assert_eq!(h.called(OP_HAS_MESSAGE_BOX), 0);
        let mut h = Script::with(&[
            (OP_CREATE_EVENT_WORKERS, 1),
            (OP_CREATE_MESSAGE_BOX, u64::MAX),
        ]);
        assert_eq!(run_op(&mut h, RUN_START_MANAGER, 0), SW_ERR);
        let mut h = Script::with(&[
            (OP_CREATE_EVENT_WORKERS, 1),
            (OP_HAS_MESSAGE_BOX, 1),
            (OP_MODE, MODE_PROCESS),
        ]);
        assert_eq!(run_op(&mut h, RUN_START_MANAGER, 0), SW_OK);
        assert_eq!(
            (h.called(OP_FORK_MANAGER), h.called(OP_SET_MANAGER_PID)),
            (1, 0)
        );
        let mut h = Script::with(&[
            (OP_CREATE_EVENT_WORKERS, 1),
            (OP_HAS_MESSAGE_BOX, 1),
            (OP_MODE, MODE_PROCESS),
            (OP_FORK_MANAGER, u64::MAX),
        ]);
        assert_eq!(run_op(&mut h, RUN_START_MANAGER, 0), SW_ERR);
        assert_eq!(h.called(LOG_FORK_MANAGER), 1);
    }

    #[test]
    fn shutdown_user_pids_snapshot_the_registry_without_filtering_pid_values() {
        let registry = Box::into_raw(Box::<WorkerRegistry>::default());
        let mut worker = 0u8;
        let pointer = core::ptr::from_mut(&mut worker).cast();
        let mut h = Script::with(&[(OP_WORKER_REGISTRY, registry as u64)]);
        unsafe {
            for pid in [10, 0, -7] {
                (*registry).register(pid, pointer, CATEGORY_USER);
            }
            (*registry).register(20, pointer, 1);
            let mut before = user_worker_pids(&mut h);
            before.sort_unstable();
            assert_eq!(before, [-7, 0, 10]);
            (*registry).unregister(10);
            (*registry).detach(0);
            (*registry).register(-7, pointer, 1);
            (*registry).register(11, pointer, CATEGORY_USER);
            assert_eq!(user_worker_pids(&mut h), [11]);
            assert_eq!(before, [-7, 0, 10]);
            drop(Box::from_raw(registry));
        }
        assert_eq!(user_worker_pids(&mut Script::default()), [] as [i32; 0]);
    }

    #[test]
    fn main_spawns_every_worker_then_waits_and_kills_them() {
        let mut h = Script::with(&[
            (OP_MODE, MODE_BASE),
            (OP_CREATE_EVENT_WORKERS, 1),
            (OP_HAS_MESSAGE_BOX, 1),
            (OP_WORKER_NUM, 2),
            (OP_USER_WORKER_NUM, 1),
            (OP_CREATE_USER_WORKERS, 1),
            (OP_SPAWN_EVENT_WORKER, 10),
            (OP_SPAWN_USER_WORKER, 11),
        ]);
        assert_eq!(run_op(&mut h, RUN_START_MANAGER, 0), SW_OK);
        assert_eq!(h.args(OP_SPAWN_EVENT_WORKER), vec![0, 1]);
        assert_eq!(h.args(OP_SPAWN_USER_WORKER), vec![0]);
        assert_eq!(h.called(OP_MANAGER_INIT), 1);
        let tail = h.effects(&[OP_MAX_WAIT_TIME, OP_TIMER_AVAILABLE]);
        assert_eq!(
            tail[tail.len() - 4..],
            [
                OP_KILL_EVENT_WORKERS,
                OP_KILL_TASK_WORKERS,
                OP_KILL_USER_WORKERS,
                OP_ON_MANAGER_STOP
            ]
        );
        let mut h = Script::with(&[(OP_WORKER_NUM, 1), (OP_SPAWN_EVENT_WORKER, u64::MAX)]);
        run_op(&mut h, RUN_MANAGER_MAIN, 0);
        assert_eq!(
            (h.called(LOG_FORK_EVENT_WORKER), h.called(OP_MANAGER_INIT)),
            (1, 0)
        );
        // The manager takes SIGTERM before it forks anything.
        let sigterm = u64::from(libc::SIGTERM.cast_unsigned()) | (HANDLER_OWN << 32);
        assert_eq!(h.args(OP_SIGNAL_SET), vec![sigterm]);
        let order = h.effects(&[]);
        let at = |op| order.iter().position(|&o| o == op);
        assert!(at(OP_SIGNAL_SET) < at(OP_SPAWN_EVENT_WORKER));
    }

    #[test]
    fn signals_reach_the_manager_only_in_its_own_process() {
        let manager = Manager::default();
        let address = core::ptr::from_ref(&manager) as u64;
        let mut h = Script::with(&[(OP_MANAGER, address)]);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGTERM as u64);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGUSR2 as u64);
        assert_eq!(
            (h.called(OP_SET_RUNNING), h.called(OP_SERVER_RELOAD)),
            (0, 0)
        );
        let mut h = Script::with(&[(OP_MANAGER, address), (OP_IS_MANAGER, 1)]);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGTERM as u64);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGUSR2 as u64);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGIO as u64);
        assert_eq!(h.args(OP_SET_RUNNING), vec![0]);
        assert_eq!(h.args(OP_SERVER_RELOAD), vec![0]);
        assert_eq!(h.args(OP_SET_READ_MESSAGE), vec![1]);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGALRM as u64);
        assert_eq!(h.called(LOG_KILL_TIMEOUT), 0);
        let mut h = Script::default();
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGUSR2 as u64);
        assert_eq!(h.called(OP_IS_MANAGER), 0);
        // SIGTERM needs only the manager's pid: it arrives while the workers fork.
        let mut h = Script::with(&[(OP_IS_MANAGER, 1)]);
        run_op(&mut h, RUN_MANAGER_SIGNAL, libc::SIGTERM as u64);
        assert_eq!(h.args(OP_SET_RUNNING), vec![0]);
    }

    #[test]
    fn reload_sets_the_managers_request_in_its_own_process() {
        let manager = Manager::default();
        // SAFETY (test): getpid has no preconditions.
        let me = u64::from(crate::os::getpid().cast_unsigned());
        let mut h = Script::with(&[
            (OP_MANAGER, core::ptr::from_ref(&manager) as u64),
            (OP_MANAGER_PID, me),
            (OP_POOL_RELOAD, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_RELOAD, 0), 1);
        assert!(manager.reload_task_workers.get() && !manager.reload_all_workers.get());
        let mut h = Script::with(&[(OP_MODE, MODE_PROCESS)]);
        assert_eq!(run_op(&mut h, RUN_RELOAD, 1), 0);
        assert_eq!(h.called(LOG_RELOAD_SINGLE_PROCESS), 1);
        let mut h = Script::with(&[(OP_MODE, MODE_THREAD), (OP_RELOAD_THREADS, 1)]);
        assert_eq!(run_op(&mut h, RUN_RELOAD, 1), 1);
        assert_eq!(h.args(OP_RELOAD_THREADS), vec![1]);
    }

    #[test]
    fn restart_respawns_by_type_or_reports_strangers() {
        let mut h = Script::default();
        assert_eq!(run_op(&mut h, RUN_RESTART_WORKER, 42), SW_ERR);
        assert_eq!(h.args(LOG_UNKNOWN_CHILD), vec![42]);
        let mut h = Script::with(&[(OP_REAP_DETACHED, 1)]);
        run_op(&mut h, RUN_RESTART_WORKER, 42);
        assert_eq!(h.called(LOG_UNKNOWN_CHILD), 0);
        for (kind, op) in [
            (EVENT_WORKER, OP_RESPAWN_EVENT_WORKER),
            (TASK_WORKER, OP_RESPAWN_TASK_WORKER),
            (USER_WORKER, OP_RESPAWN_USER_WORKER),
        ] {
            let mut h = Script::with(&[(OP_WORKER_BY_PID, 1), (OP_WORKER_TYPE, kind), (op, 77)]);
            assert_eq!(run_op(&mut h, RUN_RESTART_WORKER, (9 << 32) | 42), 77);
            assert_eq!(h.args(OP_UNREGISTER_PID), vec![42]);
            assert_eq!(h.args(OP_CHECK_EXIT_STATUS), vec![(9 << 32) | 42]);
        }
        let mut h = Script::with(&[(OP_WORKER_BY_PID, 1), (OP_WORKER_TYPE, 9)]);
        assert_eq!(run_op(&mut h, RUN_RESTART_WORKER, 42), SW_ERR);
        assert_eq!(h.called(LOG_UNKNOWN_WORKER_TYPE), 1);
    }

    #[test]
    fn the_reap_host_starts_reloads_from_the_managers_requests() {
        let manager = Manager::default();
        manager.reload_all_workers.set(true);
        let mut h = Script::with(&[(OP_TASK_WORKER_NUM, 1), (OP_RELOAD_ASYNC, 1)]);
        let mut reaping = Reaping {
            host: &mut h,
            manager: &manager,
            warn: no_warn,
        };
        assert_eq!(reaping.start_reload(), RELOAD_SIGNALLED);
        assert_eq!(reaping.start_reload(), RELOAD_NONE);
        manager.reload_task_workers.set(true);
        assert_eq!(reaping.start_reload(), RELOAD_KILL_ONE);
        assert_eq!(h.args(LOG_RELOADING), vec![1, 0]);
        assert_eq!(h.called(OP_RELOAD_ADD_TASK_WORKERS), 2);
        assert_eq!(h.called(OP_RELOAD_KILL_ALL), 1);
    }

    #[test]
    fn the_reap_service_replaces_stopping_workers() {
        let manager = Manager::default();
        let mut h = Script::with(&[
            (OP_READ_MESSAGE, 1),
            (OP_RUNNING, 1),
            (OP_MESSAGE_TYPE, WORKER_MESSAGE_STOP),
            (OP_WORKER_NUM, 2),
        ])
        .queue(OP_POP_MESSAGE, &[1, 1])
        .queue(OP_STOP_MESSAGE_WORKER, &[1, 3]);
        Reaping {
            host: &mut h,
            manager: &manager,
            warn: no_warn,
        }
        .service();
        assert_eq!(h.args(OP_SPAWN_EVENT_WORKER), vec![1]);
        assert_eq!(h.args(OP_SPAWN_TASK_WORKER), vec![3]);
        assert_eq!(h.called(OP_DETACH_STOPPED), 2);
        assert_eq!(h.args(OP_SET_READ_MESSAGE), vec![0]);
    }
}
