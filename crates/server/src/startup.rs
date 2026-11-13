//! Creating, checking, starting and destroying a server.
use crate::callbacks::{
    CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CLOSE, CB_CONNECT, CB_PACKET, CB_RECEIVE, CB_SHUTDOWN,
    CB_START, CB_TASK, CallbackSlot,
};
use crate::hybrid::{Step, hybrid};
use crate::reactor_handle::Reactor;
use crate::records::SessionRecord;
use crate::shared::{PortCounters, ServerCounters};
use crate::thread_mode::ThreadOp;
use crate::{MODE_BASE, MODE_PROCESS, MODE_THREAD, view};
use core::ffi::c_int;
use core::mem::offset_of;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use swoole_rs_clients::address::assign;
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::{CCast, log};
use swoole_rs_error::ErrorCode;
use swoole_rs_memory::members::Shm;
use swoole_rs_process::process_pool::IpcMode;
use swoole_rs_process::worker_record::{WorkerRecord, WorkerType};

/// `SW_MAX_THREAD_NCPU`, `SW_MAX_WORKER_NCPU`, `SW_TIMER_MIN_SEC`, `SW_BUFFER_MIN_SIZE`;
/// native asserts each.
const MAX_THREAD_NCPU: u32 = 4;
const MAX_WORKER_NCPU: u32 = 1000;
const TIMER_MIN_SEC: f64 = 0.001;
const BUFFER_MIN_SIZE: u32 = 65536;

/// `Server::DispatchMode` values that hash a connection to one worker.
const DISPATCH_FDMOD: u64 = 2;
const DISPATCH_IPMOD: u64 = 4;
const DISPATCH_CO_CONN_LB: u64 = 8;

/// `Server::TaskIpcMode`, and the pool's IPC mode for each.
const TASK_IPC_MSGQUEUE: u64 = 2;
const TASK_IPC_PREEMPTIVE: u64 = 3;
const TASK_IPC_STREAM: u64 = 4;
/// `SW_LOG_WARNING`, `SW_LOG_ERROR`, and the direct methods' `SW_OK` and `SW_ERR`.
const SW_LOG_WARNING: c_int = 4;
const SW_LOG_ERROR: c_int = 5;
const OK: c_int = 0;
const ERR: c_int = -1;

/// `Server::start`: checks, the daemon, the factory and the mode's workers, then
/// the master's loop until shutdown, and `destroy`.
pub(crate) fn start(serv: view::Server<'_>) -> c_int {
    let state = serv.state();
    let config = state.config_ref();
    log::set_last_error(0);
    swoole_rs_error::swoole_clear_last_error_msg();
    if start_check(serv) < 0 {
        return ERR;
    }
    serv.call_hook(view::ServerHook::BeforeStart);
    let Some(gs) = crate::shared::server_counters(state) else {
        return ERR;
    };
    // Cannot start two servers at the same time; use process->exec.
    if gs.start.compare_exchange(0, 1, SeqCst, SeqCst).is_err() {
        let code = ErrorCode::SW_ERROR_SERVER_ONLY_START_ONE as c_int;
        let m = format_args!("can only start one server");
        log::error_log(SW_LOG_ERROR, code, c"Server::start", m);
        return ERR;
    }
    if config.daemonize.get() && hybrid().step(serv, Step::Daemon) < 0 {
        return ERR;
    }
    gs.start_time.store(swoole_rs_corebase::time_now(), Relaxed);
    crate::worker_methods::workers_start(serv);
    state.set_running(true);
    if hybrid().step(serv, Step::FactoryStart) < 0 {
        return ERR;
    }
    hybrid().step(serv, Step::InitProtocols);
    state.write_pid();
    let started = match u64::from(config.mode_) {
        MODE_BASE => crate::reactor_process::start_reactor_processes(serv),
        MODE_PROCESS => hybrid().step(serv, Step::StartProcess),
        MODE_THREAD => {
            // Before the worker threads start, which read it in onWorkerStart.
            set_master_pid(gs);
            hybrid()
                .thread_mode(serv, ThreadOp::StartWorkers)
                .c_cast::<c_int>()
        }
        _ => swoole_rs_corebase::fatal("an unknown server start mode"),
    };
    if started < 0 {
        return ERR;
    }
    if !config.factory_.is_null() {
        destroy(serv);
    }
    state.unlink_pid();
    OK
}

fn set_master_pid(gs: &crate::shared::ServerCounters) {
    gs.master_pid
        .store(std::process::id().cast_signed(), Relaxed);
}

/// `Server::create` after the global before-create hook: shared memory, bounded
/// thread and process counts, the factory and the task workers. It runs in the master
/// before any other thread or process exists, so the configuration and tables it
/// writes have no other reader.
pub(crate) fn create(serv: view::Server<'_>) -> c_int {
    create_on(serv, u32::from(swoole_rs_corebase::swr_v1_core_cpu_num()))
}

/// [`create`] on `cpu` cores.
fn create_on(serv: view::Server<'_>, cpu: u32) -> c_int {
    let state = serv.state();
    // SAFETY: as above; each write through it is that statement's only access.
    let tables = || unsafe { &mut *state.tables() };
    // SAFETY: as `tables`, for the configuration.
    let config = || unsafe { &mut *state.config() };
    if !crate::local_addr::init(state) {
        return ERR;
    }
    let sessions = Shm::calloc(SW_SESSION_LIST_SIZE as usize, size_of::<SessionRecord>());
    if sessions.is_null() {
        let size = size_of::<SessionRecord>();
        let m = format_args!("sw_shm_calloc(1048576, {size}) for session_list failed");
        log::sys_warning(c"Server::create", m);
        return ERR;
    }
    tables().session_list = sessions;
    let count = state.tables_ref().ports().len();
    let port_gs = Shm::calloc(count, size_of::<PortCounters>());
    let list = port_gs.as_ptr().cast::<PortCounters>();
    tables().port_gs_list = port_gs;
    if list.is_null() {
        let size = size_of::<PortCounters>();
        let m = format_args!("sw_shm_calloc({count}, {size}) for port_connection_num_array failed");
        log::sys_warning(c"Server::create", m);
        return ERR;
    }
    for (index, port) in state.tables_ref().ports().iter().enumerate() {
        port.set_gs(list.wrapping_add(index).cast());
    }
    http_defaults(state);
    if let Some(max) = max_connection(state, swoole_rs_corebase::max_sockets()) {
        tables().max_connection = max;
    }
    bound_workers(state.config_ref(), cpu);
    let worker_num = state.config_ref().worker_num.get();
    let workers = Shm::calloc(worker_num as usize, size_of::<WorkerRecord>());
    if workers.is_null() {
        let size = size_of::<WorkerRecord>();
        let m = format_args!("sw_shm_calloc({worker_num}, {size}) for workers failed");
        log::sys_warning(c"Server::create", m);
        return ERR;
    }
    config().workers = workers;
    let factory = match u64::from(state.config_ref().mode_) {
        MODE_BASE => crate::base_factory::create(serv),
        MODE_PROCESS => hybrid().process_factory(serv),
        _ => {
            let made = hybrid().thread_mode(serv, ThreadOp::CreateFactory);
            core::ptr::with_exposed_provenance_mut(usize::try_from(made).unwrap_or(0))
        }
    };
    config().factory_ = factory;
    if factory.is_null() {
        return ERR;
    }
    if state.config_ref().task_worker_num.get() > 0 {
        let Some(results) = create_task_workers(serv) else {
            return ERR;
        };
        config().task_results = results;
        if !hybrid().task_workers(serv) {
            return ERR;
        }
    }
    serv.call_hook(view::ServerHook::AfterCreate);
    OK
}

/// The static handler's default locations and HTTP compression, from the settings.
fn http_defaults(state: &crate::ServerState) {
    use crate::StaticCollection::{CompressionTypes, Locations};
    use crate::static_config::Op;
    let strings = |collection, op| crate::static_config::config(state, collection, op);
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
    if let Some(last) = tables.ports().last() {
        minimum = minimum.wrapping_add(last.fd().cast_unsigned());
    }
    if tables.max_connection >= minimum {
        return None;
    }
    let real = minimum.wrapping_add(1).max(max_sockets);
    let m = format_args!("max_connection must be bigger than {minimum}, it's reset to {real}");
    log::warning(c"Server::create", m);
    Some(real)
}

/// Reactor and worker counts within what `cpu` cores can run.
fn bound_workers(config: &crate::ServerConfig, cpu: u32) {
    let max_reactors = cpu.saturating_mul(MAX_THREAD_NCPU);
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
    let max_workers = cpu.saturating_mul(MAX_WORKER_NCPU);
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

/// `Server::create_task_workers` up to `init_task_workers`: the task pool's memory, not
/// its processes, and the task results it returns and notify pipes for `Server::task_sync()`.
fn create_task_workers(serv: view::Server<'_>) -> Option<Shm> {
    let state = serv.state();
    let config = state.config_ref();
    let ipc = match u64::from(config.task_ipc_mode.get()) {
        TASK_IPC_MSGQUEUE | TASK_IPC_PREEMPTIVE => IpcMode::MsgQueue,
        TASK_IPC_STREAM => IpcMode::Socket,
        _ => IpcMode::UnixSock,
    };
    let master = crate::shared::server_counters(state).map_or(0, |gs| gs.master_pid.load(Relaxed));
    let socket = format!("/tmp/swoole.task.{master}.sock");
    let socket = std::ffi::CString::new(socket).unwrap_or_default();
    let socket = matches!(ipc, IpcMode::Socket).then_some(socket.as_c_str());
    if !hybrid().task_pool(serv, ipc, socket) {
        log::warning(
            c"Server::create_task_workers",
            format_args!("[Master] create task_workers failed"),
        );
        return None;
    }
    let workers = config.worker_num.get();
    let size = swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
    let mut results = Shm::calloc(workers as usize, size as usize);
    if results.is_null() {
        let m = format_args!("sw_shm_calloc({workers}, {size}) for task_result failed");
        log::warning(c"Server::create_task_workers", m);
        return None;
    }
    for _ in 0..workers {
        if !state.push_task_pipe() {
            results.free();
            return None;
        }
    }
    Some(results)
}

/// `Server::start_check`: settings the mode cannot honour, required callbacks, and
/// heartbeat and package-length defaults. It runs in the master before any other
/// thread or process exists.
pub(crate) fn start_check(serv: view::Server<'_>) -> c_int {
    let state = serv.state();
    let config = state.config_ref();
    let callbacks = &state.callbacks;
    let has = |id| callbacks.slot(id).is_some_and(CallbackSlot::is_set);
    if u64::from(config.mode_) == MODE_PROCESS {
        let mode = config.dispatch_mode.get();
        let hashed = matches!(
            u64::from(mode),
            DISPATCH_FDMOD | DISPATCH_IPMOD | DISPATCH_CO_CONN_LB
        );
        if !hashed && !config.enable_unsafe_event.get() {
            for (id, event) in [
                (CB_CONNECT, "onConnect"),
                (CB_CLOSE, "onClose"),
                (CB_BUFFER_FULL, "onBufferFull"),
                (CB_BUFFER_EMPTY, "onBufferEmpty"),
            ] {
                if let Some(slot) = callbacks.slot(id).filter(|slot| slot.is_set()) {
                    let m =
                        format_args!("cannot set '{event}' event when using dispatch_mode={mode}");
                    invalid_callback(m);
                    slot.clear();
                }
            }
            config.disable_notify.set(true);
        }
        if !hashed {
            config.send_yield.set(false);
        }
    } else {
        config.max_queued_bytes.set(0);
    }
    if config.task_worker_num.get() > 0 && !has(CB_TASK) {
        invalid_callback(format_args!("require 'onTask' callback"));
        return ERR;
    }
    let send_timeout = config.send_timeout.get();
    if send_timeout > 0.0 && send_timeout < TIMER_MIN_SEC {
        config.send_timeout.set(TIMER_MIN_SEC);
    }
    let ports = state.tables_ref().ports();
    let interval = config.heartbeat_check_interval.get();
    if interval > 0 {
        for port in ports {
            let idle = &port.config().heartbeat_idle_time;
            if idle.get() == 0 {
                idle.set(interval.wrapping_mul(2));
            }
        }
    }
    for &port in ports {
        if !check_port(config, port, has) {
            return ERR;
        }
    }
    OK
}

fn invalid_callback(message: core::fmt::Arguments<'_>) {
    let code = ErrorCode::SW_ERROR_SERVER_INVALID_CALLBACK as c_int;
    log::error_log(SW_LOG_WARNING, code, c"Server::start_check", message);
}

/// A port's package length, required callbacks and heartbeat interval.
fn check_port(
    config: &crate::ServerConfig,
    port: view::Port<'_>,
    has: impl Fn(u32) -> bool,
) -> bool {
    let settings = port.config();
    if settings.protocol.get().package_max_length < BUFFER_MIN_SIZE {
        port.set_package_max_length(BUFFER_MIN_SIZE);
    }
    let dgram = matches!(settings.r#type, 2 | 4 | 6);
    let stream = matches!(settings.r#type, 1 | 3 | 5);
    let ssl = settings.ssl != 0;
    if ((dgram && ssl) || stream) && !has(CB_RECEIVE) {
        invalid_callback(format_args!("require 'onReceive' callback"));
        return false;
    }
    if dgram && !ssl && !has(CB_PACKET) {
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
pub(crate) fn start_master_thread(serv: view::Server<'_>, reactor: Reactor) -> c_int {
    /// `THREAD_MASTER`, `SW_FD_STREAM_SERVER` and `SW_EVENT_READ`.
    const THREAD_MASTER: u8 = 1;
    const FD_STREAM_SERVER: c_int = 1;
    const EVENT_READ: c_int = 1 << 9;
    let state = serv.state();
    let config = state.config_ref();
    let single = config.single_thread.get();
    let thread_id = if single { 0 } else { config.reactor_num.get() };
    swoole_rs_corebase::set_thread(THREAD_MASTER, thread_id);
    hybrid().step(serv, Step::TimerReinit);
    crate::control::init_signals(serv);
    let mode = u64::from(config.mode_);
    swoole_rs_process::methods::set_worker_type(WorkerType::Master);
    if mode == MODE_THREAD {
        let tid = swoole_rs_ossupport::swr_v1_os_thread_native_id().c_cast::<c_int>();
        swoole_rs_process::methods::swoole_set_worker_pid(tid);
    } else if mode == MODE_PROCESS {
        swoole_rs_process::methods::swoole_set_worker_pid(std::process::id().cast_signed());
    }
    reactor.handle().set_object(serv.raw());
    let accept: swoole_rs_reactor::Handler = crate::server_methods::accept_connection;
    reactor.set_handler(FD_STREAM_SERVER, EVENT_READ, accept);
    if let Some(pipe) = config.command_pipe() {
        if !single {
            hybrid().step(serv, Step::CommandHandler);
        }
        if let Some(master) = pipe.master_end() {
            reactor.change(master, 0, EVENT_READ);
        }
    }
    if hybrid().step(serv, Step::MasterTimer) < 0 {
        hybrid().step(serv, Step::EventFree);
        return ERR;
    }
    if !single && mode != MODE_THREAD {
        hybrid().step(serv, Step::ReactorBarrier);
    }
    if mode == MODE_PROCESS {
        hybrid().step(serv, Step::ManagerBarrier);
    }
    if mode != MODE_THREAD
        && let Some(gs) = crate::shared::server_counters(state)
    {
        set_master_pid(gs);
    }
    state.callbacks.call_server(CB_START, serv);
    hybrid().step(serv, Step::EventWait)
}

/// `Server::store_listen_socket`: each listen socket in the connection list, its
/// slot holding the port, and the fd bounds from the last.
pub(crate) fn store_listen_socket(serv: view::Server<'_>) {
    let state = serv.state();
    let tables = state.tables_ref();
    for (index, &listen) in tables.ports().iter().enumerate() {
        let Some(socket) = serv.port_socket_at(index) else {
            continue;
        };
        let (ls, port, fd) = (listen.raw(), listen.config(), socket.fd());
        let Some(conn) = tables.connection_at(fd) else {
            continue;
        };
        conn.set_fd(fd);
        conn.set_socket(port.socket_ptr().cast());
        conn.set_socket_type(port.r#type);
        conn.set_object(ls);
        let mut info = conn.info();
        info.kind = port.r#type;
        let host = port.host.bytes_with_nul();
        let error = assign(
            &mut info.addr,
            &mut info.len,
            port.r#type,
            host,
            port.port.get(),
            true,
        );
        conn.set_info(info);
        if error != 0 {
            log::set_last_error(error);
        }
        socket.edit(|socket| socket.object = ls);
        if let Some(gs) = crate::shared::server_counters(state) {
            gs.min_fd.store(fd, Relaxed);
            gs.max_fd.store(fd, Relaxed);
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
    let mode = u64::from(config.mode_);
    if mode == MODE_PROCESS {
        hybrid().step(serv, Step::JoinReactorThreads);
    }
    let gs = crate::shared::server_counters(state);
    if gs.is_some_and(|gs| gs.start.load(Relaxed) != 0) {
        hybrid().step(serv, Step::FactoryShutdown);
    }
    for id in 0..config.worker_num.get() {
        if let Some(worker) = serv.worker_record(u16::try_from(id).unwrap_or(u16::MAX)) {
            crate::worker_methods::destroy_worker(worker);
        }
    }
    hybrid().step(serv, Step::ReleasePipeBuffers);
    hybrid().step(serv, Step::ClosePorts);
    // SAFETY: as above; each write through it is that statement's only access.
    let config = || unsafe { &mut *state.config() };
    config().user_workers.free();
    hybrid().step(serv, Step::ClearSignals);
    if let Some(gs) = gs {
        gs.start.store(0, SeqCst);
        gs.shutdown.store(1, SeqCst);
    }
    state.callbacks.call_server(CB_SHUTDOWN, serv);
    match mode {
        MODE_BASE => destroy_base_factory(state),
        MODE_PROCESS => _ = hybrid().step(serv, Step::DestroyProcessFactory),
        _ => _ = hybrid().thread_mode(serv, ThreadOp::DestroyFactory),
    }
    if config().task_worker_num.get() > 0 {
        config().task_results.free();
        hybrid().step(serv, Step::DestroyTaskPool);
    }
    free_server(serv);
}

/// `Server::destroy_base_factory()`: the connection list and the connection counts.
fn destroy_base_factory(state: &crate::ServerState) {
    // SAFETY: as `destroy`; the write is the statement's only access to the tables.
    unsafe { &mut *state.tables() }.connection_list.free();
    // SAFETY: `slot` is the `connection_nums` of the server's counters, or of a port's
    // in `port_gs_list`, which no other process shares once the workers are reaped.
    let free = |slot: *mut Shm| unsafe { (*slot).free() };
    let gs = state.config_ref().gs.as_ptr();
    if !gs.is_null() {
        free(
            gs.wrapping_byte_add(offset_of!(ServerCounters, connection_nums))
                .cast(),
        );
    }
    for port in state.tables_ref().ports() {
        let gs = port.config().gs.get();
        if !gs.is_null() {
            free(
                gs.wrapping_byte_add(offset_of!(PortCounters, connection_nums))
                    .cast(),
            );
        }
    }
}

/// The session list, port counters, workers and factory, then `SwooleG.server`.
fn free_server(serv: view::Server<'_>) {
    let state = serv.state();
    // SAFETY: as `destroy`; the reference is this statement's only access.
    let tables = unsafe { &mut *state.tables() };
    tables.session_list.free();
    tables.port_gs_list.free();
    // SAFETY: as `tables`, for the configuration; no reference outlives its statement,
    // so the factory's deletion, which reads the configuration, aliases none.
    let config = || unsafe { &mut *state.config() };
    config().workers.free();
    if !state.config_ref().factory_.is_null() {
        hybrid().step(serv, Step::DeleteFactory);
        config().factory_ = core::ptr::null_mut();
    }
    swoole_rs_corebase::swr_v1_core_set_server(core::ptr::null_mut());
}

/// What `Server::destroy` frees of a base-mode server whose single worker bailed out
/// of its loop: the base factory's connection list and counts, the server's lists and
/// its factory. It runs none of the shutdown (callbacks, hooks, signals, ports), only
/// marks the server shut down, so `swr_v1_server_delete` frees the rest.
pub(crate) fn release_abandoned_base(serv: view::Server<'_>) {
    let state = serv.state();
    if let Some(gs) = crate::shared::server_counters(state) {
        gs.shutdown.store(1, SeqCst);
    }
    destroy_base_factory(state);
    free_server(serv);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls, fail};
    use Step::*;

    fn steps(steps: &[Step]) -> Vec<Call> {
        steps.iter().map(|&step| Call::Step(step)).collect()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn start_runs_the_factory_protocols_then_the_mode() {
        let live = LiveServer::new();
        on(&live, CB_RECEIVE);
        let base = [
            CreateEventPool,
            CreatePipeBuffers,
            StartManager,
            DestroyEventPool,
        ];
        for (mode, run) in [
            (MODE_BASE, steps(&base)),
            (MODE_PROCESS, steps(&[StartProcess])),
            (MODE_THREAD, vec![Call::ThreadMode(ThreadOp::StartWorkers)]),
        ] {
            set_mode(&live, mode);
            live.counters().start.store(0, SeqCst);
            live.counters().master_pid.store(0, Relaxed);
            live.state().set_running(false);
            calls();
            assert_eq!(start(live.server()), OK, "mode {mode}");
            assert_eq!(
                calls(),
                [steps(&[FactoryStart, InitProtocols]), run].concat(),
                "mode {mode}"
            );
            assert!(live.state().running(), "running");
            assert_eq!(live.counters().start.load(Relaxed), 1, "claimed");
            assert_eq!(
                live.counters().master_pid.load(Relaxed) != 0,
                mode == MODE_THREAD,
                "only thread mode sets the master pid before its workers"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn start_failures_return_before_later_steps() {
        let live = LiveServer::new();
        calls();
        assert_eq!(start(live.server()), ERR, "TCP needs onReceive");
        assert_eq!(live.counters().start.load(Relaxed), 0, "not claimed");
        on(&live, CB_RECEIVE);
        live.counters().start.store(1, SeqCst);
        assert_eq!(start(live.server()), ERR, "only one server");
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
            live.counters().start.store(0, SeqCst);
            fail(Some(failing));
            assert_eq!(start(live.server()), ERR, "{failing:?}");
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

    fn on(live: &LiveServer, id: u32) {
        crate::callbacks::tests::on(live.state().callbacks.slot(id).unwrap());
    }

    fn is_on(live: &LiveServer, id: u32) -> bool {
        live.state().callbacks.slot(id).unwrap().is_set()
    }

    fn set_mode(live: &LiveServer, mode: u64) {
        config(live).mode_ = u8::try_from(mode).unwrap();
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
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn max_connection_raised_to_the_minimum() {
        let live = LiveServer::new();
        let state = live.state();
        config(&live).worker_num.set(2);
        config(&live).task_worker_num.set(1);
        // (2 + 1) * 2 + 32 + the listener's fd
        let minimum = 38 + live.listener_fd.cast_unsigned();
        let max = |value| {
            unsafe { &mut *state.tables() }.max_connection = value;
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
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn create_makes_the_mode_factory_and_its_tables() {
        let live = LiveServer::new();
        let state = live.state();
        config(&live).worker_num.set(2);
        calls();
        assert_eq!(create_on(live.server(), 4), OK, "process mode");
        assert_eq!(calls(), [Call::ProcessFactory]);
        assert_eq!(config(&live).reactor_num.get(), 1, "the reactor stays");
        let free = |live: &LiveServer| {
            let tables = unsafe { &mut *state.tables() };
            assert!(!tables.session_list.is_null(), "sessions");
            assert!(!config(live).workers.is_null(), "workers");
            let port = state.tables_ref().ports()[0].config();
            assert_eq!(
                port.gs.get().addr(),
                tables.port_gs_list.as_ptr().addr(),
                "port counters"
            );
            assert_eq!(config(live).factory_.addr(), 0x70, "the factory");
            config(live).factory_ = core::ptr::null_mut();
            config(live).workers.free();
            tables.session_list.free();
            tables.port_gs_list.free();
        };
        free(&live);

        set_mode(&live, MODE_BASE);
        assert_eq!(create_on(live.server(), 4), OK, "base mode");
        assert_eq!(calls(), [Call::FactoryNew]);
        assert_eq!(config(&live).reactor_num.get(), 2, "a reactor per worker");
        let tables = unsafe { &mut *state.tables() };
        assert!(
            !tables.connection_list.is_null(),
            "the worker's own connections"
        );
        tables.connection_list.free();
        let gs = config(&live)
            .gs
            .as_ptr()
            .cast::<crate::shared::ServerCounters>();
        unsafe { (*gs).connection_nums.free() };
        let port = state.tables_ref().ports()[0].config();
        let port_gs = port.gs.get().cast::<PortCounters>();
        unsafe { (*port_gs).connection_nums.free() };
        free(&live);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn task_workers_by_ipc_mode() {
        let live = LiveServer::new();
        config(&live).worker_num.set(2);
        calls();
        for (mode, ipc, socket) in [
            (TASK_IPC_STREAM, IpcMode::Socket, true),
            (TASK_IPC_PREEMPTIVE, IpcMode::MsgQueue, false),
            (TASK_IPC_MSGQUEUE, IpcMode::MsgQueue, false),
            (1, IpcMode::UnixSock, false),
        ] {
            config(&live).task_ipc_mode.set(u8::try_from(mode).unwrap());
            let mut results = create_task_workers(live.server()).unwrap();
            assert_eq!(
                calls(),
                [Call::TaskPool(ipc.code(), socket)],
                "only stream mode listens, mode {mode}"
            );
            assert!(!results.is_null(), "results");
            assert!(
                !live.server().task_notify_pipe(1).is_null(),
                "a pipe per worker"
            );
            results.free();
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn start_check_drops_callbacks_unsafe_for_the_dispatch_mode() {
        const UNSAFE: [u32; 4] = [CB_CONNECT, CB_CLOSE, CB_BUFFER_FULL, CB_BUFFER_EMPTY];
        let live = LiveServer::new();
        let arm = |dispatch_mode| {
            for id in UNSAFE.into_iter().chain([CB_RECEIVE]) {
                on(&live, id);
            }
            config(&live).dispatch_mode.set(dispatch_mode);
            config(&live).send_yield.set(true);
            config(&live).disable_notify.set(false);
        };
        arm(1);
        assert_eq!(start_check(live.server()), OK, "round robin");
        assert!(
            UNSAFE.iter().all(|&id| !is_on(&live, id)),
            "unsafe callbacks cleared"
        );
        assert!(is_on(&live, CB_RECEIVE), "onReceive stays");
        assert!(config(&live).disable_notify.get(), "notify off");
        assert!(!config(&live).send_yield.get(), "round robin cannot yield");

        arm(2);
        assert_eq!(start_check(live.server()), OK, "fd hash");
        assert!(
            UNSAFE.iter().all(|&id| is_on(&live, id)),
            "hash mode keeps all"
        );
        assert!(!config(&live).disable_notify.get(), "notify on");
        assert!(config(&live).send_yield.get(), "may yield");

        arm(1);
        config(&live).enable_unsafe_event.set(true);
        assert_eq!(start_check(live.server()), OK, "unsafe events");
        assert!(UNSAFE.iter().all(|&id| is_on(&live, id)), "kept on request");

        set_mode(&live, MODE_BASE);
        config(&live).max_queued_bytes.set(1024);
        config(&live).task_worker_num.set(1);
        assert_eq!(start_check(live.server()), ERR, "task workers need onTask");
        assert_eq!(config(&live).max_queued_bytes.get(), 0, "base mode");
        on(&live, CB_TASK);
        assert_eq!(start_check(live.server()), OK, "onTask set");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn start_check_ports() {
        let live = LiveServer::new();
        let port = live.port();
        assert_eq!(start_check(live.server()), ERR, "TCP needs onReceive");
        on(&live, CB_RECEIVE);
        config(&live).heartbeat_check_interval.set(60);
        config(&live).send_timeout.set(0.0001);
        state_port(&live).set_package_max_length(1024);
        assert_eq!(start_check(live.server()), OK, "valid");
        assert_eq!(port.heartbeat_idle_time.get(), 120, "the default idle time");
        assert_eq!(
            config(&live).heartbeat_check_interval.get(),
            60,
            "half of it"
        );
        assert_eq!(
            port.protocol.get().package_max_length,
            BUFFER_MIN_SIZE,
            "raised"
        );
        assert_eq!(
            config(&live).send_timeout.get().to_bits(),
            TIMER_MIN_SEC.to_bits(),
            "send timeout raised"
        );

        port.heartbeat_idle_time.set(7);
        assert_eq!(start_check(live.server()), OK, "valid");
        assert_eq!(
            config(&live).heartbeat_check_interval.get(),
            3,
            "half the idle time"
        );

        // A UDP port.
        unsafe { (*(&raw const *port).cast_mut()).r#type = 2 };
        assert_eq!(start_check(live.server()), ERR, "UDP needs onPacket");
        on(&live, CB_PACKET);
        assert_eq!(start_check(live.server()), OK, "onPacket set");
    }

    fn state_port(live: &LiveServer) -> view::Port<'_> {
        live.state().tables_ref().ports()[0]
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn listen_sockets_fill_their_slots_and_the_fd_bounds() {
        let live = LiveServer::new();
        let fd = live.listener_fd;
        let conn = live.state().tables_ref().connection_at(fd).unwrap();
        conn.set_fd(0);
        conn.set_object(core::ptr::null_mut());
        conn.set_socket_type(0);
        store_listen_socket(live.server());
        let port = (&raw const *live.port()).addr();
        assert_eq!(conn.fd(), fd, "the slot's fd");
        assert_eq!(conn.object().addr(), port, "the slot holds its port");
        assert_eq!(conn.socket_type(), 1, "TCP");
        assert_eq!(conn.info().kind, 1, "its address");
        assert_eq!(
            live.port_socket().read(|s| s.object).addr(),
            port,
            "the socket's port"
        );
        assert_eq!(
            (live.counters().min_fd(), live.counters().max_fd()),
            (fd, fd)
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn master_thread_by_mode() {
        let live = LiveServer::new();
        config(&live).reactor_num.set(4);
        let master = |mode| {
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
            master(MODE_PROCESS),
            (
                OK,
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
            swoole_rs_process::methods::worker_type(),
            Some(WorkerType::Master)
        );
        assert_ne!(live.counters().master_pid.load(Relaxed), 0, "master pid");
        assert_eq!(
            master(MODE_BASE),
            (
                OK,
                steps(&[TimerReinit, MasterTimer, ReactorBarrier, EventWait])
            ),
            "no manager"
        );
        assert_eq!(
            master(MODE_THREAD),
            (OK, steps(&[TimerReinit, MasterTimer, EventWait])),
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
            master(MODE_BASE),
            (ERR, steps(&[TimerReinit, MasterTimer, EventFree])),
            "the event loop freed"
        );
        fail(None);
        assert_eq!(swoole_rs_corebase::thread_id(), 0, "single thread is 0");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn destroy_joins_the_reactors_before_the_factory_stops() {
        for mode in [MODE_BASE, MODE_PROCESS] {
            let live = LiveServer::new();
            set_mode(&live, mode);
            config(&live).worker_num.set(2);
            assert_eq!(create_on(live.server(), 4), OK, "mode {mode}");
            live.counters().start.store(1, SeqCst);
            calls();
            destroy(live.server());
            let mut expected = Vec::new();
            if mode == MODE_PROCESS {
                expected.push(JoinReactorThreads);
            }
            expected.extend([
                FactoryShutdown,
                ReleasePipeBuffers,
                ClosePorts,
                ClearSignals,
            ]);
            if mode == MODE_PROCESS {
                expected.push(DestroyProcessFactory);
            }
            expected.push(DeleteFactory);
            assert_eq!(calls(), steps(&expected), "mode {mode}");
            let counters = live.counters();
            assert_eq!(
                (
                    counters.start.load(Relaxed),
                    counters.shutdown.load(Relaxed)
                ),
                (0, 1),
                "shut down"
            );
            let tables = live.state().tables_ref();
            assert!(tables.session_list.is_null(), "sessions freed");
            assert!(tables.port_gs_list.is_null(), "port counters freed");
            assert!(config(&live).workers.is_null(), "workers freed");
            assert!(config(&live).factory_.is_null(), "factory deleted");
            assert_eq!(
                tables.connection_list.is_null(),
                mode == MODE_BASE,
                "base mode's own connections"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sockets")]
    fn an_abandoned_base_server_frees_its_tables_and_factory_only() {
        let live = LiveServer::new();
        set_mode(&live, MODE_BASE);
        assert_eq!(create_on(live.server(), 4), OK);
        calls();
        release_abandoned_base(live.server());
        assert_eq!(calls(), steps(&[DeleteFactory]), "no shutdown steps");
        assert_eq!(live.counters().shutdown.load(Relaxed), 1, "shut down");
        let tables = live.state().tables_ref();
        assert!(tables.connection_list.is_null(), "connections freed");
        assert!(tables.session_list.is_null(), "sessions freed");
        assert!(config(&live).factory_.is_null(), "factory deleted");
    }
}
