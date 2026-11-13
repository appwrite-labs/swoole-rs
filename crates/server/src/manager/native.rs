//! The manager's native side: its fork, the workers it spawns, reaps and kills, the
//! reap loop's host, its signals and the workers' messages.
use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering::Relaxed;

use swoole_rs_corebase::{DataHead, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::methods::{self, ExitStatus};
use swoole_rs_process::os::kill;
use swoole_rs_process::process_pool::{Hooks, PoolRef};
use swoole_rs_process::reaper::{self, Reap, ReloadStart, Waited};
use swoole_rs_process::reload_task::{ReloadTask, Warn};
use swoole_rs_process::worker_record::{Worker, WorkerType};
use swoole_rs_reactor::wait_upstream::swoole_waitpid;

use super::{Manager, Reload, stop_message, worker_ids};
use crate::callbacks::{CB_AFTER_RELOAD, CB_BEFORE_RELOAD, CB_MANAGER_START, CB_MANAGER_STOP};
use crate::event_worker::{HANDLER_NONE, HANDLER_OWN, SIGNAL_RTMIN};
use crate::process_factory::native::{
    check_exit_status, kill_task_workers, spawn_event_worker, spawn_user_worker,
};
use crate::{view, worker_methods};

/// `Server::Mode`.
const MODE_BASE: u8 = 1;
const MODE_PROCESS: u8 = 2;
const MODE_THREAD: u8 = 3;
/// `SWR_WORKER_EVENT`, `SWR_WORKER_USER`: the registry's categories.
const REGISTRY_EVENT: c_int = 1;
const REGISTRY_USER: c_int = 2;
/// `SW_MESSAGE_BOX_SIZE`; `SW_LOG_WARNING`, `SW_LOG_ERROR`.
const MESSAGE_BOX_SIZE: usize = 65536;
const WARNING: c_int = 4;
const ERROR: c_int = 5;
/// `swoole::WorkerMessageType::SW_WORKER_MESSAGE_STOP`; `SW_SERVER_EVENT_COMMAND_REQUEST`.
const WORKER_MESSAGE_STOP: u8 = 1;
const EVENT_COMMAND_REQUEST: u8 = 17;
/// `sizeof(EventData)`.
const EVENT_DATA_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_MAX_SIZE as usize;

/// `swoole::EventData`, aligned for its head, to pop a message into.
#[repr(C, align(8))]
struct EventData([u8; EVENT_DATA_SIZE]);

impl EventData {
    /// The message's type.
    fn kind(&self) -> u8 {
        self.0[core::mem::offset_of!(DataHead, kind)]
    }
    /// The payload after the head.
    fn data(&self) -> &[u8] {
        self.0.get(size_of::<DataHead>()..).unwrap_or_default()
    }
}

// libc's `alarm(3)`, which only (re)arms this process's SIGALRM timer: it takes a
// plain value and touches no caller memory.
unsafe extern "C" {
    safe fn alarm(seconds: libc::c_uint) -> libc::c_uint;
}

/// `Server::manager_`, while the manager runs in this process.
fn manager(serv: view::Server<'_>) -> Option<&Manager> {
    // SAFETY: a non-null `Server::manager_` is the `Manager` that `wait` keeps alive
    // on its stack while the manager runs; only shared references exist.
    unsafe { serv.config().manager_.get().cast::<Manager>().as_ref() }
}

/// Whether this process is the server's manager.
fn is_manager(serv: view::Server<'_>) -> bool {
    std::process::id().cast_signed() == serv.counters().manager_pid.load(Relaxed)
}

/// Installs the manager's `handler` (one of the event worker's) for `signo`.
fn set_signal(signo: c_int, handler: u64) {
    crate::event_worker::native::swr_workers_signal_set(signo, handler, signal_handler);
}

/// `Factory::kill_event_workers()` (`origin` `kill_event_workers`) and
/// `kill_user_workers()`: SIGTERM each worker of `category`, then wait for each.
fn kill_workers(serv: view::Server<'_>, category: c_int, origin: &core::ffi::CStr) {
    let wait = |pid| swoole_waitpid(pid, None, 0);
    let warn = |pid| log::sys_warning(origin, format_args!("waitpid({pid}) failed"));
    serv.registry()
        .terminate(category, libc::SIGTERM, kill, wait, warn);
}

/// `Server::start_manager_process()`: fork the manager (the base mode's master is
/// its own manager).
#[must_use]
pub(crate) fn start_manager_process(serv: view::Server<'_>) -> c_int {
    if !worker_methods::create_event_workers(serv) {
        return -1;
    }
    let pool = serv.event_pool_ref();
    if pool.message_box().is_null() && methods::create_message_box(pool, MESSAGE_BOX_SIZE) == -1 {
        return -1;
    }
    if serv.workers().user_count > 0 {
        let workers = worker_methods::create_user_workers(serv);
        let created = !workers.is_null();
        // SAFETY: in the master before it forks or starts a thread; `config` isn't
        // read after the write.
        unsafe { serv.config_mut() }.user_workers = workers;
        if !created {
            return -1;
        }
    }
    if serv.config().mode_ == MODE_BASE {
        main(serv);
    } else if swoole_rs_corebase::fork_exec(|| main(serv)) < 0 {
        let m = format_args!("failed fork manager process");
        log::sys_warning(c"Server::start_manager_process", m);
        return -1;
    }
    0
}

/// The manager process: start the task pool, fork the workers, then reap.
fn main(serv: view::Server<'_>) {
    const ORIGIN: &core::ffi::CStr = c"Server::start_manager_process";
    let config = serv.config();
    let pid = std::process::id().cast_signed();
    serv.counters().manager_pid.store(pid, Relaxed);
    // A SIGTERM while the workers are still forking must stop the manager once
    // they are up, not kill it outright and orphan the ones already forked.
    // The children's fork resets their handlers.
    set_signal(libc::SIGTERM, HANDLER_OWN);
    if config.task_worker_num.get() > 0 && methods::start(serv.task_pool_ref()) == -1 {
        log::sys_error(ORIGIN, format_args!("failed to start task worker"));
        return;
    }
    // After the task pool starts, which sets the process type to SW_MASTER.
    methods::set_worker_type(WorkerType::Manager);
    for id in worker_ids(true, config.worker_num.get(), 0) {
        if spawn(serv, spawn_event_worker, worker_at(serv, id)) < 0 {
            log::sys_error(ORIGIN, format_args!("failed to fork event worker"));
            return;
        }
    }
    for at in 0..serv.workers().user_count {
        if spawn(serv, spawn_user_worker, serv.pending_worker(at)) < 0 {
            log::sys_error(ORIGIN, format_args!("failed to fork user worker"));
            return;
        }
    }
    wait(serv);
}

/// `serv->get_worker(id)`, or `None` past the last.
fn worker_at(serv: view::Server<'_>, id: u32) -> Option<Worker<'_>> {
    serv.worker_record(u16::try_from(id).unwrap_or(u16::MAX))
}

/// `Factory::spawn_*_worker(worker)`: the child's pid, or -1 without a worker.
fn spawn<'a>(
    serv: view::Server<'a>,
    spawn: fn(view::Server<'_>, Worker<'_>) -> c_int,
    worker: Option<Worker<'a>>,
) -> c_int {
    worker.map_or(-1, |worker| spawn(serv, worker))
}

/// `Manager::wait`: the manager's life after forking.
fn wait(serv: view::Server<'_>) {
    let (state, config) = (serv.state(), serv.config());
    let manager = Manager::default();
    init(serv, &manager);
    swoole_rs_reactor::swoole_event_free();
    set_signal(libc::SIGHUP, HANDLER_NONE);
    for signo in [
        libc::SIGCHLD,
        libc::SIGTERM,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGIO,
        libc::SIGALRM,
        libc::SIGWINCH,
        SIGNAL_RTMIN,
    ] {
        set_signal(signo, HANDLER_OWN);
    }
    if config.mode_ == MODE_PROCESS {
        swoole_rs_ossupport::swr_v1_os_set_process_death_signal(libc::SIGTERM);
        swoole_rs_locks::barrier_wait(crate::server_object::barrier(serv, true));
        if config.reload_async.get()
            && swoole_rs_ossupport::swr_v1_os_is_root_user()
            && !state.text(1).is_empty()
        {
            state.set_isolation();
        }
    }
    state.callbacks.call_server(CB_MANAGER_START, serv);

    reaper::run(&mut Reaping {
        serv,
        manager: &manager,
    });

    methods::drop_reload_task(serv.event_pool_ref());
    if swoole_rs_timer::swoole_timer_is_available() {
        swoole_rs_timer::swoole_timer_free();
        // The timer overrode SIGALRM's handler: the manager needs it back.
        set_signal(libc::SIGALRM, HANDLER_OWN);
    }
    // Wait for the children.
    let max_wait_time = config.max_wait_time.get();
    if max_wait_time != 0 {
        let events = worker_ids(true, config.worker_num.get(), 0);
        let mut pids: Vec<c_int> = events
            .map(|id| worker_at(serv, id).map_or(0, Worker::pid))
            .collect();
        if config.task_worker_num.get() > 0 {
            let pool = serv.task_pool_ref();
            let count = pool.worker_num() as usize;
            pids.extend((0..count).map(|at| pool.worker(at).map_or(0, Worker::pid)));
        }
        pids.extend(serv.registry().pids(REGISTRY_USER));
        manager.arm_kill(pids);
        // Twice max_wait_time, so as not to race the workers' own deadline.
        alarm(max_wait_time.saturating_mul(2));
    }
    kill_workers(serv, REGISTRY_EVENT, c"Factory::kill_event_workers");
    kill_task_workers(serv);
    kill_workers(serv, REGISTRY_USER, c"Factory::kill_user_workers");
    // Force kill.
    if max_wait_time != 0 {
        alarm(0);
    }
    state.callbacks.call_server(CB_MANAGER_STOP, serv);
}

/// The manager's start: its pid, the event pool's message hook and its master pid,
/// and `Server::manager_`.
fn init(serv: view::Server<'_>, manager: &Manager) {
    let config = serv.config();
    let pid = std::process::id().cast_signed();
    serv.counters().manager_pid.store(pid, Relaxed);
    let event_pool = serv.event_pool_ref();
    let hooks = Hooks {
        on_worker_message: Some(read_worker_message),
        ..Hooks::default()
    };
    // SAFETY: the hook reads the event pool's `ptr` as its server, which
    // `swr_v1_process_server_workers_start` set and which outlives the pool.
    unsafe { event_pool.install(&hooks) };
    event_pool.set_max_wait_time(config.max_wait_time.get());
    event_pool.set_master_pid(pid);
    serv.task_pool_ref().set_master_pid(pid);
    let manager = core::ptr::from_ref(manager).cast_mut().cast();
    config.manager_.set(manager);
}

/// The reap loop's host over the manager's server.
struct Reaping<'a> {
    serv: view::Server<'a>,
    manager: &'a Manager,
}

impl Reaping<'_> {
    /// Adds the event workers' (`events`) or the task workers' records to the reload.
    fn add_workers(&mut self, events: bool) {
        let config = self.serv.config();
        let (workers, tasks) = (config.worker_num.get(), config.task_worker_num.get());
        for id in worker_ids(events, workers, tasks) {
            let Some(worker) = worker_at(self.serv, id) else {
                continue;
            };
            if let Some(task) = self.active_reload() {
                // A copy of the record, whose pid and id the task snapshots.
                task.add_workers(&[worker.get()]);
            }
        }
    }

    /// Starts a reload: the log, `onBeforeReload` and the workers it restarts.
    fn begin(&mut self, which: &str, events: bool) {
        let serv = self.serv;
        log::info(format_args!("Server is reloading {which} workers now"));
        serv.state().callbacks.call_server(CB_BEFORE_RELOAD, serv);
        if events {
            self.add_workers(true);
        }
        if serv.config().task_worker_num.get() > 0 {
            self.add_workers(false);
        }
    }
}

impl Reap for Reaping<'_> {
    fn leave_when_stopped(&self) -> bool {
        false
    }

    fn skip_no_child(&self) -> bool {
        true
    }

    fn running(&mut self) -> bool {
        self.serv.state().running()
    }

    fn reload_requested(&mut self) -> bool {
        self.serv.event_pool_ref().reload_init()
    }

    fn active_reload(&mut self) -> Option<&mut ReloadTask> {
        let task = self.serv.event_pool_ref().reload_task();
        // SAFETY: the event pool's reload, live until `drop_reload_task`, which only
        // the manager's loop touches, one step at a time.
        unsafe { task.cast::<ReloadTask>().as_mut() }
    }

    fn wait(&mut self, nonblock: bool) -> Waited {
        reaper::wait_child(nonblock)
    }

    /// Dispatch signals, take the workers' messages (a stopping worker's asks for
    /// its replacement) and run the timers.
    fn service(&mut self) {
        let serv = self.serv;
        swoole_rs_process::signal::swoole_signal_dispatch();
        let pool = serv.event_pool_ref();
        if pool.read_message() {
            let mut msg = EventData([0; EVENT_DATA_SIZE]);
            while methods::pop_message(pool, &mut msg.0) > 0 {
                if !serv.state().running() {
                    continue;
                }
                if msg.kind() != WORKER_MESSAGE_STOP
                    && let Some(on_message) = pool.on_worker_message()
                {
                    // SAFETY: the message popped from the pool.
                    unsafe { on_message(pool, msg.0.as_mut_ptr().cast()) };
                    continue;
                }
                let (pid, id) = stop_message(msg.data());
                let worker = worker_at(serv, id.into());
                if u32::from(id) >= serv.config().worker_num.get() {
                    let worker = worker.map_or(core::ptr::null_mut(), Worker::raw);
                    _ = methods::spawn(serv.task_pool_ref(), worker);
                } else {
                    _ = spawn(serv, spawn_event_worker, worker);
                }
                serv.registry().detach(pid);
            }
            pool.set_read_message(false);
        }
        if swoole_rs_timer::swoole_timer_is_available() {
            swoole_rs_timer::swoole_timer_select();
        }
    }

    fn wait_failed(&mut self) {
        log::sys_warning(c"Manager::wait", format_args!("wait() failed"));
    }

    /// Start a requested reload: of every worker, or of the task workers only.
    fn start_reload(&mut self) -> ReloadStart {
        let config = self.serv.config();
        let reload = self.manager.take_reload(config.task_worker_num.get());
        if reload != Reload::None {
            self.serv.event_pool_ref().set_reload_init(false);
        }
        match reload {
            Reload::None => ReloadStart::None,
            Reload::All => {
                self.begin("all", true);
                // With reload_async every worker gets SIGTERM now: it stops reading,
                // asks the manager for its replacement and exits once its work is
                // done, so the old and the new worker briefly run side by side.
                if !config.reload_async.get() {
                    return ReloadStart::KillOne;
                }
                if let Some(task) = self.active_reload() {
                    task.kill_all(libc::SIGTERM, methods::reload_warn);
                }
                ReloadStart::Signalled
            }
            Reload::NoTaskWorkers => {
                let m = format_args!("cannot reload task workers, task workers is not started");
                log::warning(c"Manager::wait", m);
                ReloadStart::Skip
            }
            Reload::Tasks => {
                self.begin("task", false);
                ReloadStart::KillOne
            }
        }
    }

    /// A child exited: restart it in place (not through a signal: `wait` names it).
    fn reap(&mut self, pid: c_int, status: c_int) -> bool {
        _ = restart_worker_process(self.serv, &ExitStatus::new(pid, status));
        true
    }

    fn finish_reload(&mut self) {
        let serv = self.serv;
        methods::drop_reload_task(serv.event_pool_ref());
        serv.state().callbacks.call_server(CB_AFTER_RELOAD, serv);
    }

    fn warn(&self) -> Warn {
        methods::reload_warn
    }
}

/// `Manager_signal_handler`, over `SwooleG.server`. A newly forked worker briefly
/// inherits these handlers; it must not act as the manager (reopening the log
/// would send SIGWINCH back to itself forever).
extern "C" fn signal_handler(signo: c_int) {
    // SAFETY: the manager's signal runs while its server lives; the view ends with it.
    let Some(serv) = (unsafe { view::Server::current() }) else {
        return;
    };
    if signo == libc::SIGTERM {
        // Before `wait` sets up the manager too: `main` takes it while forking.
        if is_manager(serv) {
            serv.state().set_running(false);
        }
        return;
    }
    let Some(manager) = manager(serv).filter(|_| is_manager(serv)) else {
        return;
    };
    match signo {
        libc::SIGUSR1 | libc::SIGUSR2 => _ = reload(serv, signo == libc::SIGUSR1),
        libc::SIGIO => serv.event_pool_ref().set_read_message(true),
        libc::SIGALRM => {
            // Manager::terminate_all_worker
            let Some(pids) = manager.kill_list() else {
                return;
            };
            alarm(0);
            log::error_log(
                WARNING,
                E::SW_ERROR_SERVER_WORKER_EXIT_TIMEOUT as c_int,
                c"Manager::terminate_all_worker",
                format_args!("wait timeout, all worker processes will be forcibly terminated"),
            );
            for pid in pids {
                kill(pid, libc::SIGKILL);
            }
        }
        _ if signo == libc::SIGWINCH || is_sigrtmin(signo) => {
            // Manager::reopen_logger
            if let Some(logger) = swoole_rs_utilities::process_logger() {
                logger.reopen();
            }
            let config = serv.config();
            let all = config.worker_num.get() + config.task_worker_num.get();
            let all = all + u32::try_from(serv.workers().user_count).unwrap_or(u32::MAX);
            for id in 0..all {
                kill(worker_at(serv, id).map_or(0, Worker::pid), libc::SIGWINCH);
            }
        }
        _ => {}
    }
}

/// Whether `signo` is SIGRTMIN, which only Linux has.
fn is_sigrtmin(signo: c_int) -> bool {
    #[cfg(target_os = "linux")]
    return signo == libc::SIGRTMIN();
    #[cfg(not(target_os = "linux"))]
    {
        _ = signo;
        false
    }
}

/// `pool->onWorkerMessage`: `Server::read_worker_message` over the pool's server
/// and the message it popped: a command request from a worker.
///
/// # Safety
/// `msg` is the live `EventData` the pool popped.
unsafe extern "C-unwind" fn read_worker_message(pool: PoolRef<'_>, msg: *mut c_void) {
    // SAFETY: the event worker pool's `ptr` is its live server.
    let serv = unsafe { view::Server::from_raw(pool.ptr()) };
    // SAFETY: per the contract, a whole `EventData`.
    let task = unsafe { &*msg.cast::<EventData>() };
    // SAFETY: an `EventData` starts with its head, which its alignment allows.
    let info = unsafe { &*msg.cast::<DataHead>() };
    if info.kind != EVENT_COMMAND_REQUEST {
        let m = format_args!("unknown worker message type[{}]", info.kind);
        log::warning(c"Server::read_worker_message", m);
        return;
    }
    let state = serv.state();
    let Some(handler) = crate::commands::handler(state, i32::from(info.server_fd)) else {
        log::error_log(
            ERROR,
            E::SW_ERROR_SERVER_INVALID_COMMAND as c_int,
            c"Server::read_worker_message",
            format_args!("Unknown command[command_id={}]", info.server_fd),
        );
        return;
    };
    let data = task.data();
    let message = data.get(..info.len as usize).unwrap_or(data);
    if let Some(sock) = crate::commands::reply_socket(state, serv) {
        let bus = (state.message_bus(), sock);
        crate::commands::answer(&handler, serv, message, bus, info.fd, 0);
    }
}

/// `Server::restart_worker_process(exit_status)`: replace the worker whose process
/// exited.
#[must_use]
pub fn restart_worker_process(serv: view::Server<'_>, status: &ExitStatus) -> c_int {
    const ORIGIN: &core::ffi::CStr = c"Server::restart_worker_process";
    let pid = status.pid();
    let worker = worker_methods::get_worker_by_pid(serv, pid);
    // SAFETY: null or one of the server's live workers, kept for its life.
    let Some(worker) = (unsafe { Worker::from_raw(worker) }) else {
        // A child that is not a worker: a detached worker finishing, or unknown.
        if !serv.registry().reap_detached(pid) {
            let m = format_args!("Unknown type child process [{pid}] exited");
            log::warning(ORIGIN, m);
        }
        return -1;
    };
    // Drop the stale pid; the respawn registers the new one.
    serv.registry().unregister(pid);
    check_exit_status(serv, Some(worker), status);
    match worker.kind() {
        kind if kind == WorkerType::Event.code() => spawn_event_worker(serv, worker),
        kind if kind == WorkerType::Task.code() => {
            methods::spawn(serv.task_pool_ref(), worker.raw())
        }
        kind if kind == WorkerType::User.code() => spawn_user_worker(serv, worker),
        kind => {
            log::warning(
                ORIGIN,
                format_args!("Unknown worker [pid={pid}] type [{kind}]"),
            );
            -1
        }
    }
}

/// `Server::reload(reload_all_workers)`: ask the manager to reload every worker, or
/// the task workers.
#[must_use]
pub fn reload(serv: view::Server<'_>, reload_all_workers: bool) -> bool {
    if serv.config().mode_ == MODE_THREAD {
        return crate::thread_mode::native::reload_worker_threads(serv, reload_all_workers);
    }
    let manager_pid = serv.counters().manager_pid.load(Relaxed);
    if manager_pid == 0 {
        log::error_log(
            WARNING,
            E::SW_ERROR_OPERATION_NOT_SUPPORT as c_int,
            c"Server::reload",
            format_args!("not supported with single process mode"),
        );
        return false;
    }
    if swoole_rs_process::os::getpid() != manager_pid {
        let signo = if reload_all_workers {
            libc::SIGUSR1
        } else {
            libc::SIGUSR2
        };
        return kill(manager_pid, signo) == 0;
    }
    if !methods::reload(serv.event_pool_ref()) {
        return false;
    }
    if let Some(manager) = manager(serv) {
        manager.request(reload_all_workers);
    }
    true
}
