//! Creating, checking, starting and destroying a server.
use crate::callbacks::{CallbackKind, ServerSlot};
use crate::hybrid::{Step, hybrid};
use crate::reactor_handle::Reactor;
use crate::records::SessionRecord;
use crate::shared::PortCounters;
use crate::table::Table;
use crate::{Mode, TaskIpcMode, view};
use core::ffi::c_int;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use swoole_rs_clients::address::assign;
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::constants::SocketTypeCode;
use swoole_rs_corebase::constants::{SW_BUFFER_MIN_SIZE, SW_MAX_THREAD_NCPU, SW_MAX_WORKER_NCPU};
use swoole_rs_corebase::log::LogLevel::{SW_LOG_ERROR, SW_LOG_WARNING};
use swoole_rs_corebase::process::process_pool::IpcMode;
use swoole_rs_corebase::process::worker_record::WorkerRecord;
use swoole_rs_corebase::process::worker_record::WorkerType;
use swoole_rs_corebase::reactor::{ErrorHandler::ConnectionDefault, Events};
use swoole_rs_corebase::timer::MIN_SEC as TIMER_MIN_SEC;
use swoole_rs_corebase::{CCast, log};
use swoole_rs_error::ErrorCode;

/// `Server::start`: checks, the daemon, the factory and the mode's workers, then
/// the master's loop until shutdown, and `destroy`.
pub(crate) fn start(serv: view::Server<'_>) -> bool {
    let state = serv.state();
    let config = state.config_ref();
    log::set_last_error(0);
    swoole_rs_error::swoole_clear_last_error_msg();
    if !start_check(serv) {
        return false;
    }
    serv.call_hook(view::ServerHook::BeforeStart);
    let Some(gs) = crate::shared::server_counters(state) else {
        return false;
    };
    // Cannot start two servers at the same time; use process->exec.
    if gs
        .start
        .compare_exchange(false, true, SeqCst, SeqCst)
        .is_err()
    {
        let code = ErrorCode::SW_ERROR_SERVER_ONLY_START_ONE as c_int;
        let m = format_args!("can only start one server");
        log::error_log(SW_LOG_ERROR, code, c"Server::start", m);
        return false;
    }
    if config.daemonize.get() && !hybrid().step(serv, Step::Daemon) {
        return false;
    }
    gs.master_pid
        .store(std::process::id().cast_signed(), Relaxed);
    gs.start_time.store(swoole_rs_corebase::time_now(), Relaxed);
    crate::shared::workers_start(serv);
    state.set_running(true);
    #[cfg(target_os = "linux")]
    let _shutdown_mask =
        if config.mode_ == Mode::Process && swoole_rs_corebase::swr_v1_core_enable_signalfd() {
            let Some(mask) = crate::control::ShutdownMask::block() else {
                return false;
            };
            Some(mask)
        } else {
            None
        };
    if !hybrid().step(serv, Step::FactoryStart) {
        return false;
    }
    hybrid().step(serv, Step::InitProtocols);
    state.write_pid();
    let started = match config.mode_ {
        Mode::Base => crate::reactor_process::start_reactor_processes(serv),
        Mode::Process => hybrid().step(serv, Step::StartProcess),
        Mode::Thread => hybrid().start_worker_threads(serv),
    };
    if !started || swoole_rs_corebase::process::pool::bailout_pending() {
        return false;
    }
    if !config.factory_.get().is_null() {
        destroy(serv);
    }
    state.unlink_pid();
    true
}

/// `Server::create` after the global before-create hook: shared memory, bounded
/// thread and process counts, the factory and the task workers. It runs in the master
/// before any other thread or process exists, so the configuration and tables it
/// writes have no other reader.
pub(crate) fn create(serv: view::Server<'_>) -> bool {
    create_on(serv, u32::from(swoole_rs_corebase::swr_v1_core_cpu_num()))
}

/// [`create`] on `cpu` cores.
fn create_on(serv: view::Server<'_>, cpu: u32) -> bool {
    let state = serv.state();
    let (tables, config) = (state.tables_ref(), state.config_ref());
    if !crate::local_addr::init(state) {
        return false;
    }
    let sessions = Table::<SessionRecord>::calloc(SW_SESSION_LIST_SIZE as usize);
    if sessions.is_null() {
        let size = size_of::<SessionRecord>();
        let m = format_args!("sw_shm_calloc(1048576, {size}) for session_list failed");
        log::sys_warning(c"Server::create", m);
        return false;
    }
    tables.map_sessions(sessions);
    // Each port's counters; a retry keeps those a port already has, which may be lent.
    for port in &state.ports {
        let counters = &port.config().gs;
        counters.fill(Table::calloc(1));
        if counters.is_null() {
            let size = size_of::<PortCounters>();
            let m = format_args!("sw_shm_calloc(1, {size}) for a port's counters failed");
            log::sys_warning(c"Server::create", m);
            return false;
        }
    }
    http_defaults(state);
    if let Some(max) = max_connection(state, swoole_rs_corebase::max_sockets()) {
        tables.set_max_connection(max);
    }
    bound_workers(state.config_ref(), cpu);
    let worker_num = state.config_ref().worker_num.get();
    let workers = Table::<WorkerRecord>::calloc(worker_num as usize);
    if workers.is_null() {
        let size = size_of::<WorkerRecord>();
        let m = format_args!("sw_shm_calloc({worker_num}, {size}) for workers failed");
        log::sys_warning(c"Server::create", m);
        return false;
    }
    tables.workers.fill(workers);
    // A retry retains the first mapping, which `workers()` reads `worker_num` records of.
    if worker_num as usize > tables.workers.records().len() {
        log::warning(
            c"Server::create",
            format_args!("cannot add workers to an existing worker mapping"),
        );
        return false;
    }
    let factory = match state.config_ref().mode_ {
        Mode::Base => crate::base_factory::create(serv),
        Mode::Process => hybrid().process_factory(serv),
        Mode::Thread => hybrid().create_thread_factory(serv),
    };
    config.factory_.set(factory);
    if factory.is_null() {
        return false;
    }
    if state.config_ref().task_worker_num.get() > 0 {
        let Some(results) = create_task_workers(serv) else {
            return false;
        };
        tables.task_results.fill(results);
    }
    serv.call_hook(view::ServerHook::AfterCreate);
    true
}

/// The static handler's default locations and HTTP compression, from the settings.
fn http_defaults(state: &crate::ServerState) {
    use crate::StaticCollection::{CompressionTypes, Locations};
    use crate::static_config::Op;
    let strings =
        |collection, op| crate::static_config::config(&state.static_config, collection, op);
    let config = state.config_ref();
    if config.enable_static_handler.get() && strings(Locations, Op::Enabled) == 0 {
        strings(Locations, Op::Reset);
        strings(Locations, Op::Enable);
    }
    if strings(CompressionTypes, Op::Enabled) != 0 && strings(CompressionTypes, Op::Count) != 0 {
        config.http_compression.set(true);
    }
}

/// Room for every worker's pipes and the listen sockets, in `uint32_t` as upstream:
/// the raised `max_connection`, or `None` when there is room.
fn max_connection(state: &crate::ServerState, max_sockets: u32) -> Option<u32> {
    let config = state.config_ref();
    let tables = state.tables_ref();
    let (workers, tasks) = (config.worker_num.get(), config.task_worker_num.get());
    let mut minimum = workers.wrapping_add(tasks).wrapping_mul(2).wrapping_add(32);
    if let Some(last) = state.ports.last() {
        minimum = minimum.wrapping_add(last.fd().cast_unsigned());
    }
    if tables.max_connection() >= minimum {
        return None;
    }
    let real = minimum.wrapping_add(1).max(max_sockets);
    let m = format_args!("max_connection must be bigger than {minimum}, it's reset to {real}");
    log::warning(c"Server::create", m);
    Some(real)
}

/// Reactor and worker counts within what `cpu` cores can run.
fn bound_workers(config: &crate::ServerConfig, cpu: u32) {
    let max_reactors = cpu.saturating_mul(SW_MAX_THREAD_NCPU);
    let reactors = u32::from(config.reactor_num.get());
    if reactors > max_reactors {
        let m = format_args!(
            "reactor_num == {reactors}, Too many threads, reset to max value {max_reactors}"
        );
        log::warning(c"Server::create", m);
        config.reactor_num.set(max_reactors.c_cast::<u16>());
    } else if reactors == 0 {
        config.reactor_num.set(cpu.c_cast::<u16>());
    }
    if config.single_thread.get() {
        config.reactor_num.set(1);
    }
    let max_workers = cpu.saturating_mul(SW_MAX_WORKER_NCPU);
    let workers = config.worker_num.get();
    if workers > max_workers {
        let m = format_args!(
            "worker_num == {workers}, Too many processes, reset to max value {max_workers}"
        );
        log::warning(c"Server::create", m);
        config.worker_num.set(max_workers);
    }
    let workers = config.worker_num.get();
    if workers < u32::from(config.reactor_num.get()) {
        config.reactor_num.set(workers.c_cast::<u16>());
    }
    let tasks = config.task_worker_num.get();
    if tasks > max_workers {
        let m = format_args!(
            "task_worker_num == {tasks}, Too many processes, reset to max value {max_workers}"
        );
        log::warning(c"Server::create", m);
        config.task_worker_num.set(max_workers);
    }
}

/// `Server::create_task_workers` including `init_task_workers`: the task pool's memory, not
/// its processes, and the task results it returns and notify pipes for `Server::task_sync()`.
fn create_task_workers(serv: view::Server<'_>) -> Option<Table<crate::records::ResultWords>> {
    let state = serv.state();
    let config = state.config_ref();
    let ipc = match config.task_ipc() {
        Some(TaskIpcMode::MsgQueue | TaskIpcMode::Preemptive) => IpcMode::MsgQueue,
        Some(TaskIpcMode::Stream) => IpcMode::Socket,
        _ => IpcMode::UnixSock,
    };
    let socket = matches!(ipc, IpcMode::Socket).then(|| {
        std::ffi::CString::new(format!("/tmp/swoole.task.{}.sock", std::process::id()))
            .unwrap_or_default()
    });
    if !hybrid().task_pool(serv, ipc, socket.as_deref()) {
        log::warning(
            c"Server::create_task_workers",
            format_args!("[Master] create task_workers failed"),
        );
        return None;
    }
    let workers = config.worker_num.get();
    let size = swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
    let mut results = Table::calloc(workers as usize);
    if results.is_null() {
        let m = format_args!("sw_shm_calloc({workers}, {size}) for task_result failed");
        log::warning(c"Server::create_task_workers", m);
    } else if state.open_task_pipes(workers, || hybrid().task_workers(serv)) {
        return Some(results);
    } else {
        results.free();
    }
    hybrid().step(serv, Step::DestroyTaskPool);
    serv.task_pool_ref().reset();
    None
}

/// `Server::start_check`: settings the mode cannot honour, required callbacks, and
/// heartbeat and package-length defaults. It runs in the master before any other
/// thread or process exists.
pub(crate) fn start_check(serv: view::Server<'_>) -> bool {
    let state = serv.state();
    let config = state.config_ref();
    let callbacks = &state.callbacks;
    let has = |id| callbacks.slot(id).is_set();
    if config.mode_ == Mode::Process {
        let mode = config.dispatch_mode.get();
        if !mode.is_hash() && !config.enable_unsafe_event.get() {
            for (id, event) in [
                (CallbackKind::Connect, "onConnect"),
                (CallbackKind::Close, "onClose"),
                (CallbackKind::BufferFull, "onBufferFull"),
                (CallbackKind::BufferEmpty, "onBufferEmpty"),
            ] {
                if let Some(slot) = Some(callbacks.slot(id)).filter(|slot| slot.is_set()) {
                    let m = format_args!(
                        "cannot set '{event}' event when using dispatch_mode={}",
                        mode.0
                    );
                    invalid_callback(m);
                    slot.clear();
                }
            }
            config.disable_notify.set(true);
        }
        if !mode.is_hash() {
            config.send_yield.set(false);
        }
    } else {
        config.max_queued_bytes.set(0);
    }
    if config.task_worker_num.get() > 0 && !has(CallbackKind::Task) {
        invalid_callback(format_args!("require 'onTask' callback"));
        return false;
    }
    let send_timeout = config.send_timeout.get();
    if send_timeout > 0.0 && send_timeout < TIMER_MIN_SEC {
        config.send_timeout.set(TIMER_MIN_SEC);
    }
    let ports = &state.ports;
    let interval = config.heartbeat_check_interval.get();
    if interval > 0 {
        for port in ports {
            let idle = &port.config().heartbeat_idle_time;
            if idle.get() == 0 {
                idle.set(interval.wrapping_mul(2));
            }
        }
    }
    for port in ports {
        if !check_port(config, port, has) {
            return false;
        }
    }
    true
}

fn invalid_callback(message: core::fmt::Arguments<'_>) {
    let code = ErrorCode::SW_ERROR_SERVER_INVALID_CALLBACK as c_int;
    log::error_log(SW_LOG_WARNING, code, c"Server::start_check", message);
}

/// A port's package length, required callbacks and heartbeat interval.
fn check_port(
    config: &crate::ServerConfig,
    port: view::Port<'_>,
    has: impl Fn(CallbackKind) -> bool,
) -> bool {
    let settings = port.config();
    if settings.protocol.get().package_max_length < SW_BUFFER_MIN_SIZE {
        port.set_package_max_length(SW_BUFFER_MIN_SIZE);
    }
    let dgram = matches!(settings.r#type, 2 | 4 | 6);
    let stream = matches!(settings.r#type, 1 | 3 | 5);
    let ssl = settings.ssl;
    if ((dgram && ssl) || stream) && !has(CallbackKind::Receive) {
        invalid_callback(format_args!("require 'onReceive' callback"));
        return false;
    }
    if dgram && !ssl && !has(CallbackKind::Packet) {
        invalid_callback(format_args!("require 'onPacket' callback"));
        return false;
    }
    let idle = settings.heartbeat_idle_time.get();
    if idle > 0 {
        let expect = if idle > 2 { idle / 2 } else { 1 };
        let interval = config.heartbeat_check_interval.get();
        if interval == 0 || interval > expect {
            config.heartbeat_check_interval.set(expect);
        }
    }
    true
}

/// `Server::start_master_thread`: the master's reactor, signals and timer, then the
/// event loop.
pub(crate) fn start_master_thread(serv: view::Server<'_>, reactor: Reactor) -> bool {
    const FD_STREAM_SERVER: c_int =
        swoole_rs_corebase::constants::FdType::SW_FD_STREAM_SERVER as c_int;
    let state = serv.state();
    let config = state.config_ref();
    let single = config.single_thread.get();
    let thread_id = if single { 0 } else { config.reactor_num.get() };
    swoole_rs_corebase::set_thread(crate::THREAD_MASTER, thread_id);
    hybrid().step(serv, Step::TimerReinit);
    crate::control::init_signals(serv);
    let mode = config.mode_;
    swoole_rs_corebase::process::methods::set_worker_type(WorkerType::Master);
    if mode == Mode::Thread {
        let tid = swoole_rs_corebase::ossupport::swr_v1_os_thread_native_id().c_cast::<c_int>();
        swoole_rs_corebase::process::methods::swoole_set_worker_pid(tid);
    } else if mode == Mode::Process {
        swoole_rs_corebase::process::methods::swoole_set_worker_pid(
            std::process::id().cast_signed(),
        );
    }
    reactor.hold_server(serv);
    let accept: swoole_rs_corebase::reactor::Handler = crate::server_methods::accept_connection;
    reactor.set_handlers(FD_STREAM_SERVER, Some(accept), None, ConnectionDefault);
    if let Some(pipe) = config.command_pipe() {
        if !single {
            hybrid().step(serv, Step::CommandHandler);
        }
        if let Some(master) = pipe.master_end() {
            reactor.change(
                master,
                swoole_rs_corebase::reactor::ChangeOp::Add,
                Events::READ,
            );
        }
    }
    if !hybrid().step(serv, Step::MasterTimer) {
        hybrid().step(serv, Step::EventFree);
        return false;
    }
    if !single && mode != Mode::Thread {
        hybrid().step(serv, Step::ReactorBarrier);
    }
    if mode == Mode::Process {
        hybrid().step(serv, Step::ManagerBarrier);
    }
    state.callbacks.call_server(ServerSlot::Start, serv);
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return false;
    }
    hybrid().step(serv, Step::EventWait)
}

/// `Server::store_listen_socket`: each listen socket in the connection list, its
/// slot holding the port, and the fd bounds from the last.
pub(crate) fn store_listen_socket(serv: view::Server<'_>) {
    let state = serv.state();
    for (index, listen) in state.ports.iter().enumerate() {
        let (ls, port) = (listen.raw(), listen.config());
        let Some(socket) = port.listening_socket() else {
            continue;
        };
        let fd = socket.fd();
        let Some(conn) = state.hold_port(index, fd) else {
            continue;
        };
        conn.set_fd(fd);
        conn.set_socket_type(SocketTypeCode(port.r#type));
        let mut info = conn.info();
        info.kind = port.r#type;
        let host = port.host.bytes_with_nul();
        let assigned = assign(
            &mut info.addr,
            &mut info.len,
            port.r#type,
            host,
            port.port.get(),
            true,
        );
        conn.set_info(info);
        if let Err(error) = assigned {
            log::set_last_error(error);
        }
        socket.edit(|socket| socket.attach_object(ls));
        if let Some(gs) = crate::shared::server_counters(state) {
            let bounds = state.fd_bounds(gs);
            bounds.min_fd.store(fd, Relaxed);
            bounds.max_fd.store(fd, Relaxed);
        }
    }
}

/// `Server::destroy` once the factory exists. In process mode the reactor threads
/// finish their write events before the factory shuts down. It runs in the master
/// once no other thread uses the server, so the configuration and tables it frees
/// have no other reader.
pub(crate) fn destroy(serv: view::Server<'_>) {
    let state = serv.state();
    let config = state.config_ref();
    serv.call_hook(view::ServerHook::AfterShutdown);
    let mode = config.mode_;
    if mode == Mode::Process {
        hybrid().step(serv, Step::JoinReactorThreads);
    }
    let gs = crate::shared::server_counters(state);
    if gs.is_some_and(|gs| gs.start.load(Relaxed)) {
        hybrid().step(serv, Step::FactoryShutdown);
        if mode != Mode::Base {
            crate::reactor_io::discard_shutdown_files(serv);
        }
    }
    for id in 0..config.worker_num.get() {
        if let Some(worker) = serv.worker_record(u16::try_from(id).unwrap_or(u16::MAX)) {
            crate::worker_methods::destroy_worker(worker);
        }
    }
    hybrid().step(serv, Step::ReleasePipeBuffers);
    hybrid().step(serv, Step::ClosePorts);
    hybrid().step(serv, Step::ClearSignals);
    if let Some(gs) = gs {
        gs.start.store(false, SeqCst);
        gs.shutdown.store(true, SeqCst);
    }
    state.callbacks.call_server(ServerSlot::Shutdown, serv);
    if swoole_rs_corebase::process::pool::bailout_pending() {
        return;
    }
    // `destroy_base_factory` and `destroy_thread_factory` free only tables, which
    // `free_server` frees, and port counts, which go with their ports.
    if mode == Mode::Process {
        hybrid().step(serv, Step::DestroyProcessFactory);
    }
    if config.task_worker_num.get() > 0 {
        hybrid().step(serv, Step::DestroyTaskPool);
    }
    free_server(serv);
}

/// `SwooleG.server`, then the tables (the server's mappings but its shared record and
/// its ports' counts and counters, and the reactor threads, which every mode's factory
/// destroy left) and factory. The server leaves the slot first, so a signal handler
/// (`server_methods::on_signal`) that runs meanwhile finds no server, not one whose
/// tables are going.
fn free_server(serv: view::Server<'_>) {
    swoole_rs_corebase::swr_v1_core_set_server(core::ptr::null_mut());
    let state = serv.state();
    // SAFETY: the master's teardown, once the workers, reactor threads and shutdown
    // callbacks are done: nothing borrows the tables, or anything lent out of them,
    // while this lives.
    let tables = unsafe { &mut *state.tables() };
    tables.unmap();
    drop(tables.take_reactor_threads());
    let config = state.config_ref();
    if !config.factory_.get().is_null() {
        hybrid().step(serv, Step::DeleteFactory);
        config.factory_.set(core::ptr::null_mut());
    }
}

/// What `Server::destroy` frees of a base-mode server whose single worker bailed out
/// of its loop: the base factory's connection list and counts, the server's lists and
/// its factory. It runs none of the shutdown (callbacks, hooks, signals, ports), only
/// marks the server shut down, so the last `SharedServer` drop frees the rest.
pub(crate) fn release_abandoned_base(serv: view::Server<'_>) {
    let state = serv.state();
    if let Some(gs) = crate::shared::server_counters(state) {
        gs.shutdown.store(true, SeqCst);
    }
    free_server(serv);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls, fail, on_step};
    use Step::*;

    fn steps(steps: &[Step]) -> Vec<Call> {
        steps.iter().map(|&step| Call::Step(step)).collect()
    }

    #[test]
    fn start_runs_the_factory_protocols_then_the_mode() {
        let live = LiveServer::new();
        on(&live, CallbackKind::Receive);
        let base = [
            CreateEventPool,
            CreatePipeBuffers,
            StartManager,
            DestroyEventPool,
        ];
        for (mode, run) in [
            (Mode::Base, steps(&base)),
            (Mode::Process, steps(&[StartProcess])),
            (Mode::Thread, vec![Call::StartWorkerThreads]),
        ] {
            set_mode(&live, mode);
            live.counters().start.store(false, SeqCst);
            live.counters().master_pid.store(0, Relaxed);
            live.state().set_running(false);
            calls();
            assert!(start(live.server()), "mode {mode:?}");
            assert_eq!(
                calls(),
                [steps(&[FactoryStart, InitProtocols]), run].concat(),
                "mode {mode:?}"
            );
            assert!(live.state().running(), "running");
            assert!(live.counters().start.load(Relaxed), "claimed");
            assert_eq!(
                live.counters().master_pid.load(Relaxed),
                std::process::id().cast_signed(),
                "workers inherit the master pid in every mode"
            );
        }
    }

    #[test]
    fn start_failures_return_before_later_steps() {
        let live = LiveServer::new();
        calls();
        assert!(!start(live.server()), "TCP needs onReceive");
        assert!(!live.counters().start.load(Relaxed), "not claimed");
        on(&live, CallbackKind::Receive);
        live.counters().start.store(true, SeqCst);
        assert!(!start(live.server()), "only one server");
        assert_eq!(calls(), [], "nothing started");

        config(&live).daemonize.set(true);
        for (failing, ran) in [
            (Daemon, &[Daemon][..]),
            (FactoryStart, &[Daemon, FactoryStart]),
            (
                StartProcess,
                &[Daemon, FactoryStart, InitProtocols, StartProcess],
            ),
        ] {
            live.counters().start.store(false, SeqCst);
            fail(Some(failing));
            assert!(!start(live.server()), "{failing:?}");
            assert_eq!(calls(), steps(ran), "{failing:?}");
        }
        fail(None);
    }

    /// The live server's configuration, which the test alone reads and writes.
    #[expect(
        clippy::mut_from_ref,
        reason = "each use is the test's only reference to the configuration"
    )]
    fn config(live: &LiveServer) -> &mut crate::ServerConfig {
        unsafe { &mut *live.state().config() }
    }

    fn on(live: &LiveServer, id: CallbackKind) {
        crate::callbacks::tests::on(live.state().callbacks.slot(id));
    }

    fn is_on(live: &LiveServer, id: CallbackKind) -> bool {
        live.state().callbacks.slot(id).is_set()
    }

    fn set_mode(live: &LiveServer, mode: Mode) {
        config(live).mode_ = mode;
    }

    #[test]
    fn workers_bounded_by_the_cpus() {
        let state = crate::ServerState::new();
        let config = state.config_ref();
        config.reactor_num.set(100);
        config.worker_num.set(8);
        bound_workers(config, 4);
        assert_eq!(config.reactor_num.get(), 8, "16 then bounded by 8 workers");

        config.reactor_num.set(0);
        config.worker_num.set(5000);
        config.task_worker_num.set(5000);
        bound_workers(config, 4);
        assert_eq!(config.worker_num.get(), 4000, "workers bounded");
        assert_eq!(config.task_worker_num.get(), 4000, "task workers bounded");
        assert_eq!(config.reactor_num.get(), 4, "default reactors");

        config.single_thread.set(true);
        bound_workers(config, 4);
        assert_eq!(config.reactor_num.get(), 1, "a single thread");
    }

    #[test]
    fn max_connection_raised_to_the_minimum() {
        let live = LiveServer::new();
        let state = live.state();
        config(&live).worker_num.set(2);
        config(&live).task_worker_num.set(1);
        // (2 + 1) * 2 + 32 + the listener's fd
        let minimum = 38 + live.listener_fd.cast_unsigned();
        let max = |value| {
            unsafe { state.tables_ref().set_max_connection_unchecked(value) };
            max_connection(state, 0)
        };
        assert_eq!(max(10), Some(minimum + 1), "past the minimum");
        assert_eq!(
            max_connection(state, 1 << 20),
            Some(1 << 20),
            "max_sockets wins"
        );
        assert_eq!(max(minimum), None, "room already");
        max(u32::try_from(crate::fixture::CONNECTIONS).unwrap());
    }

    #[test]
    fn create_makes_the_mode_factory_and_its_tables() {
        let mut live = LiveServer::new();
        live.forget_tables(true);
        let state = live.state();
        config(&live).worker_num.set(2);
        calls();
        assert!(create_on(live.server(), 4), "process mode");
        assert_eq!(calls(), [Call::ProcessFactory]);
        assert_eq!(config(&live).reactor_num.get(), 1, "the reactor stays");
        let free = |live: &LiveServer| {
            let tables = unsafe { &mut *state.tables() };
            assert!(tables.sessions_mapped(), "sessions");
            assert!(!tables.workers.is_null(), "workers");
            let port = state.ports.get(0).unwrap().config();
            assert!(port.counters().is_some(), "port counters");
            assert_eq!(config(live).factory_.get().addr(), 0x70, "the factory");
            config(live).factory_.set(core::ptr::null_mut());
            tables.unmap();
        };
        free(&live);

        set_mode(&live, Mode::Base);
        assert!(create_on(live.server(), 4), "base mode");
        assert_eq!(calls(), [Call::FactoryNew]);
        assert_eq!(config(&live).reactor_num.get(), 2, "a reactor per worker");
        assert!(
            state.tables_ref().connections_mapped(),
            "the worker's own connections"
        );
        assert!(
            !state.tables_ref().connection_nums.is_null(),
            "server counts"
        );
        let port = state.ports.get(0).unwrap().config();
        assert_eq!(port.cells().len(), 2, "a port count per worker");
        free(&live);
    }

    #[test]
    fn create_retry_keeps_a_ports_counters_and_maps_a_new_ports() {
        let mut live = LiveServer::new();
        live.forget_tables(true);
        let state = live.state();
        let held = state.ports.get(0).unwrap().config().counters().unwrap();
        held.connection_num.store(7, Relaxed);
        let second = state.ports.push(crate::port_config::PortConfig::boxed(0));
        assert!(second.config().counters().is_none(), "none before create");
        calls();
        assert!(create_on(live.server(), 4));
        // The stand-in factory is not a real allocation.
        config(&live).factory_.set(core::ptr::null_mut());
        assert_eq!(calls(), [Call::ProcessFactory]);
        assert!(second.config().counters().is_some(), "the new port's own");
        assert_eq!(
            held.connection_num.load(Relaxed),
            7,
            "the old loan stays live"
        );
        // SAFETY: nothing borrows the tables now.
        unsafe { &mut *state.tables() }.unmap();
    }

    #[test]
    fn create_retry_rejects_more_workers_than_the_retained_mapping() {
        let mut live = LiveServer::new();
        live.forget_tables(true);
        let state = live.state();
        let tables = state.tables_ref();
        tables.workers.fill(Table::calloc(1));
        config(&live).worker_num.set(4);
        calls();
        let created = create_on(live.server(), 4);
        config(&live).factory_.set(core::ptr::null_mut());
        assert!(!created, "a retained one-worker map cannot serve four");
        assert_eq!(calls(), [], "no factory started");
    }

    #[test]
    fn failed_task_init_does_not_publish_notify_pipes() {
        let live = LiveServer::new();
        assert!(!live.state().open_task_pipes(2, || false));
        assert!(live.server().task_notify_pipe(0).is_none());
        assert!(live.state().open_task_pipes(1, || true));
        assert!(live.server().task_notify_pipe(0).is_some());
        assert!(live.server().task_notify_pipe(1).is_none());
    }

    #[test]
    fn task_workers_by_ipc_mode() {
        let live = LiveServer::new();
        config(&live).worker_num.set(2);
        calls();
        for (mode, ipc, socket) in [
            (TaskIpcMode::Stream, IpcMode::Socket, true),
            (TaskIpcMode::Preemptive, IpcMode::MsgQueue, false),
            (TaskIpcMode::MsgQueue, IpcMode::MsgQueue, false),
            (TaskIpcMode::UnixSock, IpcMode::UnixSock, false),
        ] {
            config(&live).task_ipc_mode.set(mode as u8);
            let mut results = create_task_workers(live.server()).unwrap();
            assert_eq!(
                calls(),
                [Call::TaskPool(ipc.code(), socket), Call::TaskWorkers],
                "only stream mode listens, mode {mode:?}"
            );
            assert!(!results.is_null(), "results");
            assert!(
                live.server().task_notify_pipe(1).is_some(),
                "a pipe per worker"
            );
            results.free();
        }
    }

    #[test]
    fn start_check_drops_callbacks_unsafe_for_the_dispatch_mode() {
        const UNSAFE: [CallbackKind; 4] = [
            CallbackKind::Connect,
            CallbackKind::Close,
            CallbackKind::BufferFull,
            CallbackKind::BufferEmpty,
        ];
        let live = LiveServer::new();
        let arm = |dispatch_mode| {
            for id in UNSAFE.into_iter().chain([CallbackKind::Receive]) {
                on(&live, id);
            }
            config(&live).dispatch_mode.set(dispatch_mode);
            config(&live).send_yield.set(true);
            config(&live).disable_notify.set(false);
        };
        arm(crate::DispatchMode::ROUND);
        assert!(start_check(live.server()), "round robin");
        assert!(
            UNSAFE.iter().all(|&id| !is_on(&live, id)),
            "unsafe callbacks cleared"
        );
        assert!(is_on(&live, CallbackKind::Receive), "onReceive stays");
        assert!(config(&live).disable_notify.get(), "notify off");
        assert!(!config(&live).send_yield.get(), "round robin cannot yield");

        arm(crate::DispatchMode::FD_MOD);
        assert!(start_check(live.server()), "fd hash");
        assert!(
            UNSAFE.iter().all(|&id| is_on(&live, id)),
            "hash mode keeps all"
        );
        assert!(!config(&live).disable_notify.get(), "notify on");
        assert!(config(&live).send_yield.get(), "may yield");

        arm(crate::DispatchMode::ROUND);
        config(&live).enable_unsafe_event.set(true);
        assert!(start_check(live.server()), "unsafe events");
        assert!(UNSAFE.iter().all(|&id| is_on(&live, id)), "kept on request");

        set_mode(&live, Mode::Base);
        config(&live).max_queued_bytes.set(1024);
        config(&live).task_worker_num.set(1);
        assert!(!start_check(live.server()), "task workers need onTask");
        assert_eq!(config(&live).max_queued_bytes.get(), 0, "base mode");
        on(&live, CallbackKind::Task);
        assert!(start_check(live.server()), "onTask set");
    }

    #[test]
    fn start_check_ports() {
        let live = LiveServer::new();
        let port = live.port();
        assert!(!start_check(live.server()), "TCP needs onReceive");
        on(&live, CallbackKind::Receive);
        config(&live).heartbeat_check_interval.set(60);
        config(&live).send_timeout.set(0.0001);
        state_port(&live).set_package_max_length(1024);
        assert!(start_check(live.server()), "valid");
        assert_eq!(port.heartbeat_idle_time.get(), 120, "the default idle time");
        assert_eq!(
            config(&live).heartbeat_check_interval.get(),
            60,
            "half of it"
        );
        assert_eq!(
            port.protocol.get().package_max_length,
            SW_BUFFER_MIN_SIZE,
            "raised"
        );
        assert_eq!(
            config(&live).send_timeout.get().to_bits(),
            TIMER_MIN_SEC.to_bits(),
            "send timeout raised"
        );

        port.heartbeat_idle_time.set(7);
        assert!(start_check(live.server()), "valid");
        assert_eq!(
            config(&live).heartbeat_check_interval.get(),
            3,
            "half the idle time"
        );

        // A UDP port.
        let mut udp = crate::port_config::PortConfig::boxed(c_int::MAX);
        udp.r#type = 2;
        let _ = live.state().ports.push(udp);
        assert!(!start_check(live.server()), "UDP needs onPacket");
        on(&live, CallbackKind::Packet);
        assert!(start_check(live.server()), "onPacket set");
    }

    fn state_port(live: &LiveServer) -> view::Port<'_> {
        live.state().ports.get(0).unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no gethostbyname2_r")]
    fn listen_sockets_fill_their_slots_and_the_fd_bounds() {
        let live = LiveServer::new();
        let fd = live.listener_fd;
        let conn = live.state().tables_ref().connection_at(fd).unwrap();
        conn.set_fd(0);
        conn.clear_object();
        conn.set_socket_type(SocketTypeCode(0));
        store_listen_socket(live.server());
        let port = (&raw const *live.port()).addr();
        assert_eq!(conn.fd(), fd, "the slot's fd");
        let held = conn.port().map_or(0, |held| held.raw().addr());
        assert_eq!(held, port, "the slot holds its port");
        assert_eq!(conn.socket_type().0, 1, "TCP");
        assert_eq!(conn.info().kind, 1, "its address");
        assert_eq!(
            live.port_socket()
                .read(
                    swoole_rs_corebase::reactor::SocketFields::attached_object::<core::ffi::c_void>
                )
                .addr(),
            port,
            "the socket's port"
        );
        assert_eq!(
            (
                live.server().fd_bounds().min_fd(),
                live.server().fd_bounds().max_fd()
            ),
            (fd, fd)
        );
    }

    #[test]
    fn master_thread_by_mode() {
        let live = LiveServer::new();
        config(&live).reactor_num.set(4);
        let master = |mode: Mode| {
            set_mode(&live, mode);
            live.counters().master_pid.store(0, Relaxed);
            calls();
            let result = start_master_thread(live.server(), live.reactor());
            let ran: Vec<Call> = calls()
                .into_iter()
                .filter(|call| !matches!(call, Call::Step(Ignore(_) | Handle(_))))
                .collect();
            (result, ran)
        };
        assert_eq!(
            master(Mode::Process),
            (
                true,
                steps(&[
                    TimerReinit,
                    MasterTimer,
                    ReactorBarrier,
                    ManagerBarrier,
                    EventWait
                ])
            ),
            "both barriers"
        );
        assert_eq!(swoole_rs_corebase::thread_id(), 4, "after the reactors");
        assert_eq!(
            swoole_rs_corebase::process::methods::worker_type(),
            Some(WorkerType::Master)
        );
        assert_eq!(
            master(Mode::Base),
            (
                true,
                steps(&[TimerReinit, MasterTimer, ReactorBarrier, EventWait])
            ),
            "no manager"
        );
        assert_eq!(
            master(Mode::Thread),
            (true, steps(&[TimerReinit, MasterTimer, EventWait])),
            "no barriers"
        );
        assert_eq!(
            live.counters().master_pid.load(Relaxed),
            0,
            "set before the worker threads start"
        );

        config(&live).single_thread.set(true);
        fail(Some(MasterTimer));
        assert_eq!(
            master(Mode::Base),
            (false, steps(&[TimerReinit, MasterTimer, EventFree])),
            "the event loop freed"
        );
        fail(None);
        assert_eq!(swoole_rs_corebase::thread_id(), 0, "single thread is 0");
    }

    #[test]
    fn destroy_joins_the_reactors_before_the_factory_stops() {
        for mode in [Mode::Base, Mode::Process] {
            let mut live = LiveServer::new();
            live.forget_tables(true);
            set_mode(&live, mode);
            config(&live).worker_num.set(2);
            assert!(create_on(live.server(), 4), "mode {mode:?}");
            if mode == Mode::Process {
                // What the process factory's create maps, which the stand-in doesn't.
                let max = live.server().max_connection() as usize;
                let list = Table::calloc(max);
                live.state().tables_ref().map_connections(list);
            }
            live.counters().start.store(true, SeqCst);
            // Published, as `SharedServer::new` leaves it: by the time the factory
            // goes, `on_signal` no longer reaches this server.
            swoole_rs_corebase::swr_v1_core_set_server(live.server().raw());
            on_step(Some(Box::new(|serv, step| {
                if step == DeleteFactory {
                    assert_ne!(swoole_rs_corebase::swr_v1_core_server(), serv.raw());
                }
            })));
            calls();
            destroy(live.server());
            on_step(None);
            let mut expected = Vec::new();
            if mode == Mode::Process {
                expected.push(JoinReactorThreads);
            }
            expected.extend([
                FactoryShutdown,
                ReleasePipeBuffers,
                ClosePorts,
                ClearSignals,
            ]);
            if mode == Mode::Process {
                expected.push(DestroyProcessFactory);
            }
            expected.push(DeleteFactory);
            assert_eq!(calls(), steps(&expected), "mode {mode:?}");
            let counters = live.counters();
            assert_eq!(
                (
                    counters.start.load(Relaxed),
                    counters.shutdown.load(Relaxed)
                ),
                (false, true),
                "shut down"
            );
            let tables = live.state().tables_ref();
            assert!(!tables.sessions_mapped(), "sessions freed");
            assert!(
                live.port().counters().is_some(),
                "a port's counters go with the port"
            );
            assert!(tables.workers.is_null(), "workers freed");
            assert!(config(&live).factory_.get().is_null(), "factory deleted");
            assert!(!tables.connections_mapped(), "connections freed");
        }
    }

    #[test]
    fn an_abandoned_base_server_frees_its_tables_and_factory_only() {
        let mut live = LiveServer::new();
        live.forget_tables(true);
        set_mode(&live, Mode::Base);
        assert!(create_on(live.server(), 4));
        calls();
        release_abandoned_base(live.server());
        assert_eq!(calls(), steps(&[DeleteFactory]), "no shutdown steps");
        assert!(live.counters().shutdown.load(Relaxed), "shut down");
        let tables = live.state().tables_ref();
        assert!(!tables.connections_mapped(), "connections freed");
        assert!(!tables.sessions_mapped(), "sessions freed");
        assert!(config(&live).factory_.get().is_null(), "factory deleted");
    }
}
