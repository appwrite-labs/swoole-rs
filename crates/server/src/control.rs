//! Signals, the master timer and shutdown.
use crate::ops::{
    LOG_MANAGER_EXIT, LOG_NO_IDLE_WORKER, LOG_SHUTDOWN_FAILED, OP_BEFORE_SHUTDOWN, OP_FORCED_EXIT,
    OP_IS_MASTER_THREAD, OP_IS_STARTED, OP_KILL_TERM, OP_KILL_WINCH, OP_MANAGER_PID, OP_MASTER_PID,
    OP_MAX_WAIT_TIME, OP_MODE, OP_PIPE_COMMAND_DEL, OP_PORT_COUNT, OP_PORT_DGRAM,
    OP_PORT_REACTOR_DEL, OP_PORT_REMOVED, OP_REACTOR_STOP, OP_REACTOR_WAIT_EXIT, OP_RUNNING,
    OP_SEND_SHUTDOWN, OP_SET_MINFD, OP_SET_RUNNING, OP_SIGNAL_ACTIVE, OP_SIGNAL_FD,
    OP_SIGNAL_HANDLE, OP_SIGNAL_IGNORE, OP_SIGNAL_READ_MESSAGE, OP_SIGNAL_RELOAD, OP_SIGNAL_REOPEN,
    OP_SIGRTMIN, OP_SINGLE_THREAD, OP_STOP_SINGLE_WORKER, OP_STOP_WORKER_THREADS,
    OP_THREAD0_SHUTDOWN, OP_WARNING, OP_WARNING_RESET, OP_WARNING_TIME, OP_WRONG_OPERATION,
};
use crate::{Host, MODE_BASE, MODE_PROCESS, MODE_THREAD};

fn signal_number(signo: i32) -> u64 {
    u64::from(signo.cast_unsigned())
}

/// `Server::shutdown`: ask the master to stop, by pipe in process mode (a worker may
/// not be allowed to signal it) and by `SIGTERM` otherwise.
pub(crate) fn shutdown<H: Host>(h: &mut H) -> crate::Effect<bool> {
    if !h.is(OP_IS_STARTED)? {
        h.get(OP_WRONG_OPERATION)?;
        return Ok(false);
    }
    let mode = h.get(OP_MODE)?;
    // A worker thread masks signals; the master thread stops itself.
    if mode == MODE_THREAD && h.is(OP_IS_MASTER_THREAD)? {
        stop_master_thread(h)?;
        return Ok(true);
    }
    if mode == MODE_PROCESS {
        return h.is(OP_SEND_SHUTDOWN);
    }
    // A worker thread that asks may exit before the master sees the signal; with
    // the server marked stopping now, the manager does not respawn it.
    if mode == MODE_THREAD {
        h.op(OP_SET_RUNNING, 0)?;
    }
    let manager = h.signed(OP_MANAGER_PID)?;
    let pid = if mode == MODE_BASE && manager != 0 {
        manager
    } else {
        h.signed(OP_MASTER_PID)?
    };
    if h.op(OP_KILL_TERM, pid.cast_unsigned())?.cast_signed() < 0 {
        h.op(LOG_SHUTDOWN_FAILED, pid.cast_unsigned())?;
        return Ok(false);
    }
    Ok(true)
}

/// `Server::master_signal_handler`.
pub(crate) fn signal<H: Host>(h: &mut H, signo: i32) -> crate::Effect<()> {
    if !h.is(OP_SIGNAL_ACTIVE)? {
        return Ok(());
    }
    match signo {
        libc::SIGTERM => {
            signal_shutdown(h)?;
        }
        libc::SIGCHLD => {
            child_exit(h)?;
        }
        libc::SIGUSR1 | libc::SIGUSR2 => {
            h.op(OP_SIGNAL_RELOAD, u64::from(signo == libc::SIGUSR1))?;
        }
        libc::SIGIO => {
            h.get(OP_SIGNAL_READ_MESSAGE)?;
        }
        libc::SIGWINCH => reopen_logger(h)?,
        _ => {
            let rtmin = h.get(OP_SIGRTMIN)?;
            if rtmin != 0 && signal_number(signo) == rtmin {
                reopen_logger(h)?;
            }
        }
    }
    Ok(())
}

/// `Server::signal_handler_reopen_logger`: the log file reopens, and in process mode
/// the manager is told to reopen its own.
pub(crate) fn reopen_logger<H: Host>(h: &mut H) -> crate::Effect<()> {
    h.get(OP_SIGNAL_REOPEN)?;
    if h.get(OP_MODE)? == MODE_PROCESS {
        let manager = h.get(OP_MANAGER_PID)?;
        h.op(OP_KILL_WINCH, manager)?;
    }
    Ok(())
}

/// `Server::signal_handler_shutdown`.
pub(crate) fn signal_shutdown<H: Host>(h: &mut H) -> crate::Effect<bool> {
    if h.get(OP_MODE)? == MODE_BASE {
        if h.signed(OP_MANAGER_PID)? > 0 {
            h.op(OP_SET_RUNNING, 0)?;
        } else {
            // A single worker process exits directly.
            h.get(OP_STOP_SINGLE_WORKER)?;
        }
        return Ok(true);
    }
    h.get(OP_BEFORE_SHUTDOWN)?;
    h.op(OP_SET_RUNNING, 0)?;
    stop_master_thread(h)?;
    Ok(true)
}

/// `Server::signal_handler_child_exit`: reap and report the manager's exit.
pub(crate) fn child_exit<H: Host>(h: &mut H) -> crate::Effect<bool> {
    if !h.is(OP_RUNNING)? || h.get(OP_MODE)? == MODE_BASE {
        return Ok(false);
    }
    let mut status: libc::c_int = 0;
    // SAFETY: `status` is a valid out pointer for the call.
    let pid = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
    if pid > 0 && i64::from(pid) == h.signed(OP_MANAGER_PID)? {
        h.op(LOG_MANAGER_EXIT, u64::from(status.cast_unsigned()))?;
    }
    Ok(true)
}

/// `Server::stop_master_thread`: stop accepting and let the reactor drain.
pub(crate) fn stop_master_thread<H: Host>(h: &mut H) -> crate::Effect<()> {
    h.get(OP_REACTOR_WAIT_EXIT)?;
    let mode = h.get(OP_MODE)?;
    for index in 0..h.get(OP_PORT_COUNT)? {
        if h.op(OP_PORT_DGRAM, index)? != 0 && mode != MODE_BASE {
            continue;
        }
        if h.op(OP_PORT_REMOVED, index)? == 0 {
            h.op(OP_PORT_REACTOR_DEL, index)?;
        }
    }
    h.get(OP_PIPE_COMMAND_DEL)?;
    crate::state::clear_timer(h)?;
    if h.signed(OP_MAX_WAIT_TIME)? > 0 {
        h.get(OP_FORCED_EXIT)?;
    }
    if mode == MODE_THREAD {
        h.get(OP_STOP_WORKER_THREADS)?;
    }
    if mode == MODE_PROCESS && h.is(OP_SINGLE_THREAD)? {
        h.get(OP_THREAD0_SHUTDOWN)?;
    }
    Ok(())
}

/// `Server::timer_callback`: rate-limited scheduler warnings, the timer hook, and
/// stopping once the server no longer runs.
pub(crate) fn master_timer<H: Host>(h: &mut H) -> crate::Effect<()> {
    let now = crate::shared::unix_now();
    for pool in [0, 1] {
        if h.op(OP_WARNING, pool)? != 0 && h.op(OP_WARNING_TIME, pool)?.cast_signed() < now {
            h.op(OP_WARNING_RESET, (now.cast_unsigned() << 1) | pool)?;
            h.op(LOG_NO_IDLE_WORKER, pool)?;
        }
    }
    if !h.is(OP_RUNNING)? {
        h.get(OP_REACTOR_STOP)?;
        stop_master_thread(h)?;
    }
    Ok(())
}

/// `Server::init_signal_handler`.
pub(crate) fn init_signals<H: Host>(h: &mut H) -> crate::Effect<()> {
    h.op(OP_SIGNAL_IGNORE, signal_number(libc::SIGPIPE))?;
    h.op(OP_SIGNAL_IGNORE, signal_number(libc::SIGHUP))?;
    let child = if h.get(OP_MODE)? == MODE_PROCESS {
        libc::SIGCHLD
    } else {
        libc::SIGIO
    };
    for signo in [
        child,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGTERM,
        libc::SIGWINCH,
    ] {
        h.op(OP_SIGNAL_HANDLE, signal_number(signo))?;
    }
    let rtmin = h.get(OP_SIGRTMIN)?;
    if rtmin != 0 {
        h.op(OP_SIGNAL_HANDLE, rtmin)?;
    }
    let signal_fd = h.signed(OP_SIGNAL_FD)?;
    if signal_fd > 0 {
        h.op(OP_SET_MINFD, signal_fd.cast_unsigned())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    #[test]
    fn shutdown_by_mode() {
        let mut s = Script::with(&[]);
        assert!(!shutdown(&mut s).unwrap(), "not started");
        assert_eq!(s.called(OP_WRONG_OPERATION), 1, "error set");

        let mut s = Script::with(&[
            (OP_IS_STARTED, 1),
            (OP_MODE, MODE_PROCESS),
            (OP_SEND_SHUTDOWN, 1),
        ]);
        assert!(shutdown(&mut s).unwrap(), "pipe message sent");
        assert_eq!(s.called(OP_KILL_TERM), 0, "no signal");

        let mut s = Script::with(&[(OP_IS_STARTED, 1), (OP_MODE, MODE_BASE), (OP_MASTER_PID, 7)]);
        assert!(shutdown(&mut s).unwrap(), "signalled");
        assert_eq!(s.args(OP_KILL_TERM), vec![7], "no manager: master");

        let mut s = Script::with(&[
            (OP_IS_STARTED, 1),
            (OP_MODE, MODE_BASE),
            (OP_MANAGER_PID, 8),
            (OP_KILL_TERM, u64::MAX),
        ]);
        assert!(!shutdown(&mut s).unwrap(), "kill failed");
        assert_eq!(
            s.args(LOG_SHUTDOWN_FAILED),
            vec![8],
            "manager signalled and warned"
        );

        let mut s = Script::with(&[
            (OP_IS_STARTED, 1),
            (OP_MODE, MODE_THREAD),
            (OP_IS_MASTER_THREAD, 1),
        ]);
        assert!(shutdown(&mut s).unwrap(), "master thread stops itself");
        assert_eq!(s.called(OP_REACTOR_WAIT_EXIT), 1, "stopped");

        let mut s = Script::with(&[
            (OP_IS_STARTED, 1),
            (OP_MODE, MODE_THREAD),
            (OP_MASTER_PID, 7),
        ]);
        assert!(shutdown(&mut s).unwrap(), "a worker thread signals");
        assert_eq!(
            s.args(OP_SET_RUNNING),
            vec![0],
            "stopping before the signal"
        );
        assert_eq!(s.args(OP_KILL_TERM), vec![7], "master signalled");
    }

    #[test]
    fn signals_dispatch() {
        let run = |signo: i32| {
            let mut s = Script::with(&[
                (OP_SIGNAL_ACTIVE, 1),
                (OP_MODE, MODE_PROCESS),
                (OP_SIGRTMIN, 34),
            ]);
            signal(&mut s, signo).unwrap();
            s
        };
        assert_eq!(
            run(libc::SIGUSR1).args(OP_SIGNAL_RELOAD),
            vec![1],
            "reload all"
        );
        assert_eq!(
            run(libc::SIGUSR2).args(OP_SIGNAL_RELOAD),
            vec![0],
            "reload task"
        );
        assert_eq!(
            run(libc::SIGIO).called(OP_SIGNAL_READ_MESSAGE),
            1,
            "message"
        );
        assert_eq!(run(libc::SIGWINCH).called(OP_SIGNAL_REOPEN), 1, "reopen");
        assert_eq!(run(34).called(OP_SIGNAL_REOPEN), 1, "SIGRTMIN reopens");
        assert_eq!(run(libc::SIGTERM).called(OP_BEFORE_SHUTDOWN), 1, "shutdown");

        let mut s = Script::with(&[]);
        signal(&mut s, libc::SIGTERM).unwrap();
        assert_eq!(
            s.called(OP_BEFORE_SHUTDOWN),
            0,
            "inactive server ignores signals"
        );
    }

    #[test]
    fn signal_shutdown_in_base_mode() {
        let mut s = Script::with(&[(OP_MODE, MODE_BASE), (OP_MANAGER_PID, 3)]);
        assert!(signal_shutdown(&mut s).unwrap(), "handled");
        assert_eq!(s.args(OP_SET_RUNNING), vec![0], "manager stops the workers");

        let mut s = Script::with(&[(OP_MODE, MODE_BASE)]);
        signal_shutdown(&mut s).unwrap();
        assert_eq!(s.called(OP_STOP_SINGLE_WORKER), 1, "single worker exits");
        assert_eq!(s.called(OP_SET_RUNNING), 0, "running untouched");
    }

    #[test]
    fn stop_master_thread_keeps_dgram_ports_outside_base_mode() {
        let mut s = Script::with(&[
            (OP_MODE, MODE_PROCESS),
            (OP_PORT_COUNT, 3),
            (OP_SINGLE_THREAD, 1),
            (OP_MAX_WAIT_TIME, 3),
        ])
        .queue(OP_PORT_DGRAM, &[1, 0, 0])
        .queue(OP_PORT_REMOVED, &[0, 1]);
        stop_master_thread(&mut s).unwrap();
        assert_eq!(
            s.args(OP_PORT_REACTOR_DEL),
            vec![1],
            "stream port not yet removed"
        );
        assert_eq!(s.called(OP_FORCED_EXIT), 1, "max_wait_time");
        assert_eq!(s.called(OP_THREAD0_SHUTDOWN), 1, "single thread");
        assert_eq!(s.called(OP_STOP_WORKER_THREADS), 0, "process mode");
    }

    #[test]
    fn master_timer_warns_once_per_second_and_stops() {
        let mut s =
            Script::with(&[(OP_WARNING, 1)]).queue(OP_WARNING_TIME, &[0, i64::MAX.cast_unsigned()]);
        master_timer(&mut s).unwrap();
        assert_eq!(
            s.args(LOG_NO_IDLE_WORKER),
            vec![0],
            "server warned, task pool not due"
        );
        assert_eq!(s.args(OP_WARNING_RESET)[0] & 1, 0, "server pool reset");
        assert_eq!(s.called(OP_REACTOR_STOP), 1, "not running");
    }

    #[test]
    fn signal_set() {
        let mut s = Script::with(&[(OP_MODE, MODE_BASE), (OP_SIGRTMIN, 34), (OP_SIGNAL_FD, 9)]);
        init_signals(&mut s).unwrap();
        assert_eq!(s.called(OP_SIGNAL_IGNORE), 2, "SIGPIPE and SIGHUP");
        let handled = s.args(OP_SIGNAL_HANDLE);
        assert_eq!(
            handled[0],
            signal_number(libc::SIGIO),
            "SIGIO outside process mode"
        );
        assert_eq!(handled.last(), Some(&34), "SIGRTMIN last");
        assert_eq!(s.args(OP_SET_MINFD), vec![9], "signalfd");
    }
}
