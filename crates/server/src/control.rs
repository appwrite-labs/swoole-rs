//! Signals, the master timer and shutdown.
use crate::Mode;
use crate::hybrid::hybrid;
use crate::reactor_handle::Reactor;
use crate::view::Server;
use core::ffi::{CStr, c_int};
use core::sync::atomic::AtomicI64;
use core::sync::atomic::Ordering::{Relaxed, Release};
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;

fn mode(serv: Server<'_>) -> Mode {
    serv.config().mode_
}

/// Keep an early shutdown pending until the master's signalfd is ready. The
/// caller's mask is restored on startup errors as well as normal shutdown.
#[cfg(target_os = "linux")]
pub(crate) struct ShutdownMask(libc::sigset_t);

#[cfg(target_os = "linux")]
impl ShutdownMask {
    fn signal() -> libc::sigset_t {
        let mut mask = swoole_rs_corebase::sys::zeroed();
        swoole_rs_corebase::sys::sigemptyset(&mut mask);
        swoole_rs_corebase::sys::sigaddset(&mut mask, libc::SIGTERM);
        mask
    }

    pub(crate) fn block() -> Option<Self> {
        let mut previous = swoole_rs_corebase::sys::zeroed();
        let error = swoole_rs_corebase::sys::pthread_sigmask(
            libc::SIG_BLOCK,
            Some(&Self::signal()),
            Some(&mut previous),
        );
        if error != 0 {
            log::sys_warning_errno(
                error,
                c"Server::start",
                format_args!("block SIGTERM failed"),
            );
            return None;
        }
        Some(Self(previous))
    }

    /// The forked manager has installed its own handler and must receive SIGTERM.
    #[cfg(any(test, feature = "upstream-symbols"))]
    pub(crate) fn unblock_manager() {
        let error = swoole_rs_corebase::sys::pthread_sigmask(
            libc::SIG_UNBLOCK,
            Some(&Self::signal()),
            None,
        );
        if error != 0 {
            log::sys_warning_errno(
                error,
                c"Server::start_manager_process",
                format_args!("unblock SIGTERM failed"),
            );
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for ShutdownMask {
    fn drop(&mut self) {
        let error =
            swoole_rs_corebase::sys::pthread_sigmask(libc::SIG_SETMASK, Some(&self.0), None);
        if error != 0 {
            log::sys_warning_errno(
                error,
                c"Server::start",
                format_args!("restore signal mask failed"),
            );
        }
    }
}

fn warn(code: E, origin: &CStr, message: core::fmt::Arguments<'_>) {
    /// `SW_LOG_WARNING`.
    use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
    log::error_log(LOG_WARNING, code as c_int, origin, message);
}

/// `Server::shutdown`: ask the master to stop, by pipe in process mode (a worker may
/// not be allowed to signal it) and by `SIGTERM` otherwise.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn shutdown(serv: Server<'_>) -> bool {
    /// `THREAD_MASTER`.
    const THREAD_MASTER: u8 = 1;
    if !serv.is_started() {
        log::set_last_error(E::SW_ERROR_WRONG_OPERATION as c_int);
        return false;
    }
    let mode = mode(serv);
    // A worker thread masks signals; the master thread stops itself.
    if mode == Mode::Thread && swoole_rs_corebase::thread_kind() == THREAD_MASTER {
        stop_master_thread(serv);
        return true;
    }
    if mode == Mode::Process {
        return hybrid().send_shutdown(serv);
    }
    // A worker thread that asks may exit before the master sees the signal; marked
    // stopping now, the manager does not respawn it. `running` stays set until the
    // master handles the SIGTERM, which it ignores once the server is not running.
    if mode == Mode::Thread {
        serv.state().stopping.store(true, Release);
    }
    let gs = serv.counters();
    let manager = gs.manager_pid.load(Relaxed);
    let pid = if mode == Mode::Base && manager != 0 {
        manager
    } else {
        gs.master_pid.load(Relaxed)
    };
    if swoole_rs_process::os::kill(pid, libc::SIGTERM).is_err() {
        let m = format_args!("failed to shutdown, kill({pid}, SIGTERM) failed");
        warn(E::SW_ERROR_SYSTEM_CALL_FAIL, c"Server::shutdown", m);
        return false;
    }
    true
}

/// `Server::master_signal_handler`, while the server runs.
pub(crate) fn signal(serv: Server<'_>, signo: c_int) {
    if swoole_rs_corebase::swr_v1_core_running() && serv.state().running() {
        dispatch(serv, signo);
    }
}

/// What each signal does to a running server.
fn dispatch(serv: Server<'_>, signo: c_int) {
    match signo {
        libc::SIGTERM => signal_shutdown(serv),
        libc::SIGCHLD => {
            child_exit(serv);
        }
        libc::SIGUSR1 | libc::SIGUSR2 => {
            _ = hybrid().reload(serv, signo == libc::SIGUSR1);
            reopen_log();
        }
        libc::SIGIO => serv.event_pool_ref().set_read_message(true),
        libc::SIGWINCH => reopen_logger(serv),
        _ if crate::event_worker::sigrtmin() == Some(signo) => reopen_logger(serv),
        _ => {}
    }
}

/// This process's log file reopens.
fn reopen_log() {
    if let Some(logger) = swoole_rs_utilities::process_logger() {
        logger.reopen();
    }
}

/// `Server::signal_handler_reopen_logger`: the log file reopens, and in process mode
/// the manager is told to reopen its own.
fn reopen_logger(serv: Server<'_>) {
    reopen_log();
    if mode(serv) == Mode::Process {
        let manager = serv.counters().manager_pid.load(Relaxed);
        _ = swoole_rs_process::os::kill(manager, libc::SIGWINCH);
    }
}

/// `Server::signal_handler_shutdown`.
fn signal_shutdown(serv: Server<'_>) {
    if mode(serv) == Mode::Base {
        if serv.counters().manager_pid.load(Relaxed) > 0 {
            serv.state().set_running(false);
        } else {
            // A single worker process exits directly.
            serv.event_pool_ref().set_running(false);
            hybrid().stop_single_worker(serv);
        }
        return;
    }
    hybrid().before_shutdown(serv);
    serv.state().set_running(false);
    stop_master_thread(serv);
}

/// `Server::signal_handler_child_exit`: reap and report the manager's exit.
fn child_exit(serv: Server<'_>) -> bool {
    if !serv.state().running() || mode(serv) == Mode::Base {
        return false;
    }
    let mut status: c_int = 0;
    let pid = swoole_rs_corebase::sys::waitpid(-1, Some(&mut status), libc::WNOHANG);
    if let Ok(Some(pid)) = pid
        && pid == serv.counters().manager_pid.load(Relaxed)
    {
        let signal = swoole_rs_process::signal::signal_name(libc::WTERMSIG(status));
        let signal = signal.as_cstr().to_string_lossy();
        let exit = libc::WEXITSTATUS(status);
        let m = format_args!("Fatal Error: manager process exit. status={exit}, signal=[{signal}]");
        log::warning(c"Server::signal_handler_child_exit", m);
    }
    true
}

/// `Server::stop_master_thread`: stop accepting and let the reactor drain.
pub(crate) fn stop_master_thread(serv: Server<'_>) {
    let reactor = Reactor::thread();
    reactor.handle().set_wait_exit(true);
    let mode = mode(serv);
    for (index, port) in serv.ports().iter().enumerate() {
        if matches!(port.config().r#type, 2 | 4 | 6) && mode != Mode::Base {
            continue;
        }
        let removed = port.with_socket(|s| s.read(|s| s.removed));
        if removed == Some(false)
            && let Some(sock) = serv.port_socket_at(index)
        {
            reactor.del(*sock);
        }
    }
    if let Some(master) = serv
        .config()
        .command_pipe()
        .and_then(swoole_rs_tls::socket_pair::SocketPair::master_end)
    {
        reactor.del(*master);
    }
    clear_timer(serv);
    if serv.config().max_wait_time.get() > 0 {
        exit_after_max_wait_time(serv);
    }
    if mode == Mode::Thread {
        hybrid().stop_worker_threads(serv);
    }
    if mode == Mode::Process && serv.config().single_thread.get() {
        hybrid().thread0_shutdown(serv);
    }
}

/// When the forced-termination exit condition stops the loop: `max_wait_time` after
/// the shutdown began.
static FORCED_EXIT_DEADLINE: AtomicI64 = AtomicI64::new(0);

/// The loop stops by force once the server's `max_wait_time` has passed.
fn exit_after_max_wait_time(serv: Server<'_>) {
    let wait = libc::time_t::from(serv.config().max_wait_time.get());
    FORCED_EXIT_DEADLINE.store(swoole_rs_corebase::time_now() + wait, Relaxed);
    swoole_rs_reactor::swr_v1_event_set_exit_condition(
        swoole_rs_reactor::ExitCondition::ForcedTermination,
        forced_exit,
    );
}

/// The forced-termination exit condition: past `max_wait_time`, stop the loop.
fn forced_exit(_: &mut usize) -> bool {
    if swoole_rs_corebase::time_now() > FORCED_EXIT_DEADLINE.load(Relaxed) {
        let m = format_args!("graceful shutdown failed, forced termination");
        warn(
            E::SW_ERROR_SERVER_WORKER_EXIT_TIMEOUT,
            c"Server::stop_master_thread",
            m,
        );
        Reactor::thread().handle().set_running(false);
    }
    true
}

/// `Server::clear_timer`: each armed timer is deleted and forgotten.
pub(crate) fn clear_timer(serv: Server<'_>) {
    let config = serv.config();
    let timers = [
        &config.master_timer,
        &config.heartbeat_timer,
        &serv.state().tables_ref().enable_accept_timer,
    ];
    for timer in timers {
        if let Some(timer) = timer.take() {
            swoole_rs_timer::del_id(timer);
        }
    }
}

/// `Server::timer_callback`: rate-limited scheduler warnings, then stopping once the
/// server no longer runs.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn master_timer(serv: Server<'_>) {
    let now = swoole_rs_corebase::time_now();
    let config = serv.config();
    if config.scheduler_warning.load(Relaxed) && config.warning_time.load(Relaxed) < now {
        config.scheduler_warning.store(false, Relaxed);
        config.warning_time.store(now, Relaxed);
        let m = format_args!("No idle worker is available");
        warn(
            E::SW_ERROR_SERVER_NO_IDLE_WORKER,
            c"Server::timer_callback",
            m,
        );
    }
    let pool = serv.task_pool_ref();
    if pool.scheduler_warning() && pool.warning_time() < now {
        pool.set_scheduler_warning(false);
        pool.set_warning_time(now);
        let m = format_args!("No idle task worker is available");
        warn(
            E::SW_ERROR_SERVER_NO_IDLE_WORKER,
            c"Server::timer_callback",
            m,
        );
    }
    if !serv.state().running() {
        Reactor::thread().handle().set_running(false);
        stop_master_thread(serv);
    }
}

/// `Server::init_signal_handler`.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn init_signals(serv: Server<'_>) {
    use crate::hybrid::Step::{Handle, Ignore};
    let h = hybrid();
    // Register this first: signalfd updates replace the blocked mask. An early
    // shutdown must stay blocked through every intermediate registration.
    h.step(serv, Handle(libc::SIGTERM));
    h.step(serv, Ignore(libc::SIGPIPE));
    h.step(serv, Ignore(libc::SIGHUP));
    let child = if mode(serv) == Mode::Process {
        libc::SIGCHLD
    } else {
        libc::SIGIO
    };
    for signo in [child, libc::SIGUSR1, libc::SIGUSR2, libc::SIGWINCH] {
        h.step(serv, Handle(signo));
    }
    #[cfg(target_os = "linux")]
    h.step(serv, Handle(libc::SIGRTMIN()));
    let signal_fd = swoole_rs_corebase::swr_v1_core_signal_fd();
    if signal_fd > 0 {
        serv.counters().min_fd.store(signal_fd, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls};

    #[test]
    #[cfg(target_os = "linux")]
    #[cfg_attr(miri, ignore = "Miri does not deliver POSIX signals")]
    fn early_shutdown_stays_pending_and_the_original_mask_is_restored() {
        std::thread::spawn(|| {
            use swoole_rs_corebase::sys;
            let mut initial = sys::zeroed();
            sys::sigemptyset(&mut initial);
            sys::sigaddset(&mut initial, libc::SIGUSR1);
            assert_eq!(
                sys::pthread_sigmask(libc::SIG_SETMASK, Some(&initial), None),
                0
            );
            let current = || {
                let mut mask = sys::zeroed();
                assert_eq!(
                    sys::pthread_sigmask(libc::SIG_BLOCK, None, Some(&mut mask)),
                    0
                );
                mask
            };
            let guard = ShutdownMask::block().unwrap();
            assert_eq!(sys::sigismember(&current(), libc::SIGTERM), 1);
            assert_eq!(sys::sigismember(&current(), libc::SIGUSR1), 1);
            assert_eq!(sys::sigismember(&current(), libc::SIGUSR2), 0);
            let sent = unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGTERM) };
            let mut pending = sys::zeroed();
            let queried = unsafe { libc::sigpending(&raw mut pending) };
            let mut received = 0;
            // Consume the pending default-fatal signal before any assertion can
            // unwind the guard and restore this thread's unblocked mask.
            let waited = unsafe { libc::sigwait(&ShutdownMask::signal(), &raw mut received) };
            assert_eq!((sent, queried, waited), (0, 0, 0));
            assert_eq!(sys::sigismember(&pending, libc::SIGTERM), 1);
            assert_eq!(received, libc::SIGTERM);
            drop(guard);
            assert_eq!(sys::sigismember(&current(), libc::SIGTERM), 0);
            assert_eq!(sys::sigismember(&current(), libc::SIGUSR1), 1);
            let _manager_mask = ShutdownMask::block().unwrap();
            ShutdownMask::unblock_manager();
            assert_eq!(sys::sigismember(&current(), libc::SIGTERM), 0);
            assert_eq!(sys::sigismember(&current(), libc::SIGUSR1), 1);
        })
        .join()
        .unwrap();
    }

    fn set_mode(live: &LiveServer, mode: Mode) {
        // SAFETY: the fixture's config, which nothing else reads meanwhile.
        unsafe { (*live.state().config()).mode_ = mode };
    }

    /// A pid no process has: `kill` fails with `ESRCH`.
    const NO_PROCESS: libc::pid_t = libc::pid_t::MAX;

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn shutdown_by_mode() {
        let live = LiveServer::new();
        let serv = live.server();
        calls();
        assert!(!shutdown(serv), "not started");
        assert_eq!(
            log::last_error(),
            E::SW_ERROR_WRONG_OPERATION as c_int,
            "error set"
        );

        live.counters().start.store(true, Relaxed);
        assert!(shutdown(serv), "process mode: the pipe message went");
        assert_eq!(calls(), [Call::SendShutdown], "by pipe, no signal");

        // Base mode signals the manager, or the master without one.
        set_mode(&live, Mode::Base);
        let gs = live.counters();
        let mut master = sleeper();
        gs.master_pid.store(pid(&master), Relaxed);
        assert!(shutdown(serv), "signalled");
        assert_eq!(
            terminated_by(&mut master),
            Some(libc::SIGTERM),
            "no manager: the master"
        );
        let mut manager = sleeper();
        gs.manager_pid.store(pid(&manager), Relaxed);
        gs.master_pid.store(NO_PROCESS, Relaxed);
        assert!(shutdown(serv), "signalled");
        assert_eq!(
            terminated_by(&mut manager),
            Some(libc::SIGTERM),
            "the manager"
        );
        gs.manager_pid.store(NO_PROCESS, Relaxed);
        assert!(!shutdown(serv), "kill failed");

        // A worker thread marks the server stopping, then signals the master.
        set_mode(&live, Mode::Thread);
        live.state().set_running(true);
        let mut master = sleeper();
        gs.master_pid.store(pid(&master), Relaxed);
        assert!(shutdown(serv), "a worker thread signals the master");
        assert_eq!(terminated_by(&mut master), Some(libc::SIGTERM));
        assert!(
            live.state().stopping.load(Relaxed),
            "stopping before the signal"
        );
        assert!(
            live.state().running(),
            "running until the master handles it"
        );
        assert_eq!(calls(), [], "no pipe outside process mode");
    }

    /// A child that sleeps until a signal ends it.
    fn sleeper() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap()
    }

    fn pid(child: &std::process::Child) -> libc::pid_t {
        libc::pid_t::try_from(child.id()).unwrap()
    }

    fn terminated_by(child: &mut std::process::Child) -> Option<c_int> {
        use std::os::unix::process::ExitStatusExt;
        child.wait().unwrap().signal()
    }

    #[test]
    fn signal_shutdown_by_mode() {
        let live = LiveServer::new();
        let serv = live.server();
        calls();
        set_mode(&live, Mode::Base);
        live.state().set_running(true);
        live.counters().manager_pid.store(3, Relaxed);
        signal_shutdown(serv);
        assert!(!live.state().running(), "the manager stops the workers");
        assert_eq!(calls(), [], "no worker stopped here");

        live.counters().manager_pid.store(0, Relaxed);
        live.state().set_running(true);
        signal_shutdown(serv);
        assert_eq!(calls(), [Call::StopSingleWorker], "a single worker exits");
        assert!(live.state().running(), "running untouched");

        set_mode(&live, Mode::Process);
        signal_shutdown(serv);
        assert_eq!(
            calls(),
            [Call::BeforeShutdown],
            "hooks and onBeforeShutdown"
        );
        assert!(!live.state().running(), "stopped");
        assert!(
            live.reactor().handle().wait_exit(),
            "the master thread stops"
        );
    }

    #[test]
    fn stop_master_thread_by_mode() {
        let live = LiveServer::new();
        let serv = live.server();
        calls();
        // SAFETY: the fixture's config, which nothing else reads meanwhile.
        unsafe {
            (*live.state().config()).single_thread.set(true);
            (*live.state().config()).max_wait_time.set(3);
        }
        live.reactor()
            .add(live.port_socket(), swoole_rs_reactor::Events::from_bits(1));
        stop_master_thread(serv);
        assert!(live.reactor().handle().wait_exit(), "draining");
        assert!(
            !live.reactor().del(*live.port_socket()),
            "the stream port left the reactor"
        );
        assert_eq!(calls(), [Call::Thread0Shutdown], "single thread");
        assert!(
            FORCED_EXIT_DEADLINE.load(Relaxed) >= swoole_rs_corebase::time_now() + 2,
            "max_wait_time"
        );

        set_mode(&live, Mode::Thread);
        stop_master_thread(serv);
        let stop = Call::StopWorkerThreads;
        assert_eq!(calls(), [stop], "the worker threads stop");
    }

    #[test]
    fn master_timer_warns_once_per_second_and_stops() {
        let live = LiveServer::new();
        let serv = live.server();
        let config = serv.config();
        config.scheduler_warning.store(true, Relaxed);
        let pool = serv.task_pool_ref();
        pool.set_scheduler_warning(true);
        pool.set_warning_time(i64::MAX);
        live.state().set_running(true);
        live.reactor().handle().set_running(true);
        master_timer(serv);
        assert!(!config.scheduler_warning.load(Relaxed), "server warned");
        assert!(config.warning_time.load(Relaxed) > 0, "and reset");
        assert!(pool.scheduler_warning(), "the task pool's not due");
        assert!(
            live.reactor().handle().running(),
            "running: the loop goes on"
        );
        assert!(!live.reactor().handle().wait_exit());

        live.state().set_running(false);
        master_timer(serv);
        assert!(!live.reactor().handle().running(), "not running: stopped");
        assert!(live.reactor().handle().wait_exit(), "and draining");
    }

    #[test]
    fn signals_dispatch() {
        let live = LiveServer::new();
        let serv = live.server();
        set_mode(&live, Mode::Base);
        calls();
        dispatch(serv, libc::SIGUSR1);
        dispatch(serv, libc::SIGUSR2);
        assert_eq!(
            calls(),
            [Call::Reload(true), Call::Reload(false)],
            "reload all, then the task workers"
        );
        dispatch(serv, libc::SIGIO);
        assert!(serv.event_pool_ref().read_message(), "a message to read");
        dispatch(serv, libc::SIGWINCH);
        dispatch(serv, libc::SIGTERM);
        assert_eq!(
            calls(),
            [Call::StopSingleWorker],
            "the log reopens; SIGTERM stops base mode's single worker"
        );
    }

    #[test]
    fn signals_need_a_running_server() {
        let live = LiveServer::new();
        // The thread's event pool outlives a test: clear what one before it left.
        live.server().event_pool_ref().set_read_message(false);
        signal(live.server(), libc::SIGIO);
        assert!(!live.server().event_pool_ref().read_message(), "ignored");
    }

    #[test]
    fn signal_set() {
        use crate::hybrid::Step::{Handle, Ignore};
        let live = LiveServer::new();
        set_mode(&live, Mode::Base);
        calls();
        init_signals(live.server());
        let expected = [
            Handle(libc::SIGTERM),
            Ignore(libc::SIGPIPE),
            Ignore(libc::SIGHUP),
            Handle(libc::SIGIO),
            Handle(libc::SIGUSR1),
            Handle(libc::SIGUSR2),
            Handle(libc::SIGWINCH),
        ];
        #[cfg(target_os = "linux")]
        let expected = [&expected[..], &[Handle(libc::SIGRTMIN())]].concat();
        let expected: Vec<_> = expected.into_iter().map(Call::Step).collect();
        assert_eq!(
            calls(),
            expected,
            "SIGIO outside process mode, SIGRTMIN last"
        );
        set_mode(&live, Mode::Process);
        init_signals(live.server());
        assert_eq!(
            calls()[3],
            Call::Step(Handle(libc::SIGCHLD)),
            "SIGCHLD in process mode"
        );
    }
}
