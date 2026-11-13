//! The ops of master.cc's runner (`Server::rust_call`) that are a log line or a
//! read or write of a record Rust already lays out: the server's config, tables
//! and shared counters, and the context's connection, socket and port. Rust
//! answers them before any native call, so master.cc no longer has a case for
//! them. Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_int};
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::CCast;
use swoole_rs_process::worker_record::WorkerType;

use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;

use crate::hybrid::hybrid;
use crate::ops::{
    LOG_LC_ONLY_ONE, OP_AFTER_SHUTDOWN, OP_CONN_CLOSING, OP_DESTROY_FACTORY, OP_DESTROY_WORKER,
    OP_FREE_USER_WORKERS, OP_HAS_PIPE_COMMAND, OP_IS_CREATED, OP_LC_BEFORE_START, OP_LC_CHECK,
    OP_LC_PREPARE_WORKERS, OP_LC_START_THREAD, OP_MASTER_REACTOR, OP_MASTER_THREAD,
    OP_MASTER_WORKER, OP_PIPE_COMMAND_ADD, OP_PORT_IDLE_TIME, OP_SET_MASTER_PID, OP_SIGNAL_FD,
    OP_SIGRTMIN, OP_STATE,
};
use crate::port_config::PortConfig;
#[cfg(all(feature = "upstream-symbols", not(test)))]
use crate::port_config::methods;
use crate::records::SessionRecord;
use crate::server_methods::NativeContext;
use crate::shared::{PortCounters, ServerCounters};
use crate::view::ServerHook;
use crate::{MODE_BASE, MODE_PROCESS, MODE_THREAD, ServerState};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_process::workers::ops::{RUN_DESTROY_THREAD_FACTORY, RUN_START_WORKER_THREADS};

const SW_LOG_ERROR: c_int = 5;
const SW_EVENT_READ: c_int = 1 << 9;
/// `SW_FD_STREAM_SERVER`.
const SW_FD_STREAM_SERVER: c_int = 1;
// The native sizes of the arrays `Server::create` allocates.
const _: () = assert!(
    size_of::<SessionRecord>() == 16 && size_of::<PortCounters>() == 80,
    "server.cc sizeof(Session), sizeof(ServerPortGS)"
);

fn error(level: c_int, code: E, origin: &CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// `if (swoole_isset_hook(hook)) swoole_call_hook(hook, serv)`, over the context's server.
fn call_hook(o: &Objects<'_>, hook: ServerHook) {
    if let Some(serv) = o.server {
        serv.call_hook(hook);
    }
}

fn sign(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

/// The context's objects, each null when the op doesn't use it.
struct Objects<'a> {
    ctx: &'a mut NativeContext<'a>,
    /// The context's server, which the ops reading it run with.
    server: Option<crate::view::Server<'a>>,
    state: &'a ServerState,
}

impl<'a> Objects<'a> {
    /// The context's live server.
    fn server(&self) -> crate::view::Server<'a> {
        self.server
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a master op without a server"))
    }
    fn config(&self) -> &crate::ServerConfig {
        self.state.config_ref()
    }
    fn tables(&self) -> &crate::ServerTables {
        self.state.tables_ref()
    }
    fn gs(&self) -> &ServerCounters {
        let gs = self.config().gs.as_ptr().cast::<ServerCounters>();
        // SAFETY: the server's shared counters, mapped for its life.
        unsafe { &*gs }
    }
    /// The context's connection, which the ops reading it run with.
    fn conn(&self) -> crate::Connection<'a> {
        self.ctx
            .conn
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a master op without a connection"))
    }
    fn port(&self) -> &PortConfig {
        // SAFETY: the ops reading it run with a live port in the context, which no
        // op writes while it reads.
        unsafe { &*self.ctx.port.cast::<PortConfig>() }
    }
    /// `serv->ports[index]`, if there is one.
    fn port_at(&self, index: u64) -> Option<crate::view::Port<'_>> {
        self.tables()
            .ports()
            .get(usize::try_from(index).ok()?)
            .copied()
    }
    /// The context's live reactor.
    fn reactor(&self) -> crate::reactor_handle::Reactor {
        // SAFETY: the ops reading it run on the context's live reactor, on its thread.
        unsafe { crate::reactor_handle::Reactor::of(self.ctx.reactor) }
    }
}

/// Answers `op` when it is one of this module's, else `None`.
///
/// # Safety
/// `ctx` is the live context for the running operation, `server` its server (the
/// ops reading it run with one) and `state` that server's; the objects each op
/// reads are live, as for master.cc's case, and the server is at the point of its
/// life where it makes `op`: [`configure`]'s ops only while the master is the
/// server's only thread. The [`Objects`] made from them carry this proof to each
/// stage below.
#[inline(always)]
pub(crate) unsafe fn answer<'a>(
    ctx: *mut NativeContext<'a>,
    server: Option<crate::view::Server<'a>>,
    state: &'a ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: per the caller; nothing else touches the context during the op.
    let ctx = unsafe { &mut *ctx };
    let mut o = Objects { ctx, server, state };
    connection(&mut o, op).or_else(|| ops(&mut o, op, arg))
}

/// The ops each connection makes as it is closed (`Server::close`), answered where
/// they are made: inlined into a call with a constant op, each folds to its arm,
/// where [`ops`]' match and the stages it falls through to cost about 40
/// instructions an op (#2748).
#[inline(always)]
fn connection(o: &mut Objects<'_>, op: u32) -> Option<u64> {
    Some(match op {
        OP_PORT_IDLE_TIME => o.port().max_idle_time.get().to_bits(),
        _ => return None,
    })
}

/// The ops that write the configuration and tables, each of which runs only while
/// the master is the server's only thread: `destroy` once the factory has joined its
/// threads (`OP_DESTROY_FACTORY` in base mode, `OP_FREE_USER_WORKERS`,
/// `OP_DESTROY_TASK_WORKERS`, `OP_FREE_SERVER`); else `None`.
fn configure(o: &Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    // SAFETY: no other thread runs (above), and each arm's write through it is its
    // last config access, so no other reference to the config is used meanwhile.
    let config = || unsafe { &mut *o.state.config() };
    // SAFETY: as `config`, for the tables.
    let tables = || unsafe { &mut *o.state.tables() };
    let gs = || o.config().gs.as_ptr().cast::<ServerCounters>();
    // SAFETY: as `config`, for the counters `swr_v1_server_new` mapped; and before
    // the server forks or after its workers are reaped, no process shares them.
    let counters = || unsafe { &mut *gs() };
    let port_gs = |index| Some(o.port_at(index)?.config().gs.get().cast::<PortCounters>());
    // SAFETY: as `counters`, for a port's slot in `port_gs_list`: null, or mapped.
    let port_counters = |index| unsafe { port_gs(index)?.as_mut() };
    Some(match op {
        // `Server::destroy_base_factory()`: the connection list and the connection counts.
        OP_DESTROY_FACTORY if arg == MODE_BASE => {
            tables().connection_list.free();
            counters().connection_nums.free();
            for index in 0..o.tables().ports().len() {
                if let Some(gs) = port_counters(index as u64) {
                    gs.connection_nums.free();
                }
            }
            0
        }
        OP_FREE_USER_WORKERS => {
            config().user_workers.free();
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_DESTROY_TASK_WORKERS => {
            // `Server::destroy_task_workers()`: free the task results, then destroy the task pool.
            config().task_results.free();
            swoole_rs_process::methods::destroy(o.server().task_pool_ref());
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_FREE_SERVER => {
            let lists = tables();
            lists.session_list.free();
            lists.port_gs_list.free();
            let config = config();
            config.workers.free();
            let factory = core::mem::replace(&mut config.factory_, core::ptr::null_mut());
            if !factory.is_null() {
                // SAFETY: `delete factory_`: nothing names the factory afterwards.
                unsafe { crate::factory::delete(factory) };
            }
            swoole_rs_corebase::swr_v1_core_set_server(core::ptr::null_mut());
            0
        }
        _ => return None,
    })
}

/// The other ops of master.cc's switch.
fn ops(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    if let Some(done) = configure(o, op, arg) {
        return Some(done);
    }
    let state = o.state;
    Some(match op {
        // Logs.
        LOG_LC_ONLY_ONE => {
            let m = format_args!("can only start one server");
            error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_ONLY_START_ONE,
                c"Server::start",
                m,
            );
            0
        }
        // The server's own records.
        OP_SIGRTMIN => sigrtmin(),
        OP_SIGNAL_FD => sign(swoole_rs_corebase::swr_v1_core_signal_fd()),
        OP_HAS_PIPE_COMMAND => u64::from(!o.config().pipe_command.is_null()),
        OP_SET_MASTER_PID => {
            o.gs()
                .master_pid
                .store(std::process::id().cast_signed(), Relaxed);
            0
        }

        OP_CONN_CLOSING => {
            u64::from(o.conn().closed().load(Relaxed) != 0 || o.conn().close_force())
        }

        // The server's state and the worker records.
        OP_STATE => core::ptr::from_ref(state).expose_provenance() as u64,
        OP_IS_CREATED => u64::from(!o.config().factory_.is_null()),
        // Lifecycle effects over process-wide state Rust owns: hooks, the
        // worker globals, the master reactor and the server's shared allocations.
        OP_LC_BEFORE_START => {
            call_hook(o, ServerHook::BeforeStart);
            0
        }
        OP_AFTER_SHUTDOWN => {
            call_hook(o, ServerHook::AfterShutdown);
            0
        }
        OP_MASTER_WORKER => {
            swoole_rs_process::methods::set_worker_type(WorkerType::Master);
            if arg == MODE_THREAD {
                let tid = swoole_rs_ossupport::swr_v1_os_thread_native_id().c_cast::<c_int>();
                swoole_rs_process::methods::swoole_set_worker_pid(tid);
            } else if arg == MODE_PROCESS {
                swoole_rs_process::methods::swoole_set_worker_pid(std::process::id().cast_signed());
            }
            0
        }
        OP_MASTER_REACTOR => {
            let reactor = o.reactor();
            reactor.handle().set_object(o.ctx.serv);
            let accept: swoole_rs_reactor::Handler = crate::server_methods::accept_connection;
            reactor.set_handler(SW_FD_STREAM_SERVER, SW_EVENT_READ, accept);
            0
        }
        op => return lifecycle(o, op, arg),
    })
}

/// The ops that call a `Server` method crates/server or crates/process defines,
/// or else `None`. `ProcessPool`'s methods and the process factory are
/// [`factory_ops`]', which only the hybrid builds: their runners link the C++
/// event loop and timer, and the server's fault fixture and unit tests link no C++.
fn lifecycle(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_LC_CHECK => sign(crate::server_methods::start_check(o.server())),
        OP_LC_PREPARE_WORKERS => {
            crate::worker_methods::workers_start(o.server());
            0
        }
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        crate::ops::OP_LC_START_BASE => sign(crate::reactor_process_ops::start_reactor_processes(
            o.server(),
        )),
        OP_LC_START_THREAD => {
            let started = hybrid().thread_mode(o.server(), RUN_START_WORKER_THREADS);
            sign(started.c_cast::<c_int>())
        }
        OP_DESTROY_WORKER => {
            // `get_worker(i)` of an event worker, which shutdown indexes.
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            if let Some(worker) = o.server().worker_record(id) {
                crate::worker_methods::destroy_worker(worker);
            }
            0
        }
        OP_DESTROY_FACTORY if arg == MODE_THREAD => {
            hybrid().thread_mode(o.server(), RUN_DESTROY_THREAD_FACTORY);
            0
        }
        op => return effects(o, op, arg),
    })
}

/// The ops over the thread's globals and reactor, the server's ports and
/// the context socket's output buffer, which Rust lays out; else `None`.
fn effects(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    /// `THREAD_MASTER`.
    const THREAD_MASTER: u8 = 1;
    Some(match op {
        OP_MASTER_THREAD => {
            swoole_rs_corebase::set_thread(THREAD_MASTER, arg.c_cast::<u16>());
            0
        }
        op => return sockets(o, op, arg),
    })
}

/// `DgramPacket` up to its address.
#[repr(C)]
pub(crate) struct DgramPacket {
    socket_type: c_int,
    pub(crate) socket_addr: SocketAddress,
}

/// The ops over the context's reactor, sockets and pipes, the server's
/// workers and the dispatch address, which Rust lays out; else `None`.
fn sockets(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        // `ctx->reactor->add` of the command pipe's master end.
        OP_PIPE_COMMAND_ADD => {
            if let Some(master) = o
                .config()
                .command_pipe()
                .and_then(swoole_rs_tls::socket_pair::SocketPair::master_end)
            {
                o.reactor().change(master, 0, SW_EVENT_READ);
            }
            0
        }
        op => return native(o, op, arg),
    })
}

/// The ops that call the event loop, timer, signal and error functions under
/// their upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn native(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        OP_COMMAND_HANDLER, OP_EVENT_FREE, OP_EVENT_WAIT, OP_LC_CLEAR_ERROR, OP_MARK_SHUTDOWN,
        OP_SIGNAL_HANDLE, OP_SIGNAL_IGNORE, OP_TIMER_DETACHED, OP_TIMER_REINIT,
    };
    /// `SW_FD_PIPE`.
    const SW_FD_PIPE: c_int = 3;
    use swoole_rs_error::swoole_clear_last_error_msg;
    use swoole_rs_reactor::event_api::swoole_event_wait;
    use swoole_rs_reactor::signal_upstream::{self, Handler};
    use swoole_rs_reactor::swoole_event_free;
    Some(match op {
        OP_LC_CLEAR_ERROR => {
            log::set_last_error(0);
            swoole_clear_last_error_msg();
            0
        }
        OP_EVENT_FREE => {
            swoole_event_free();
            0
        }
        OP_EVENT_WAIT => sign(swoole_event_wait()),
        OP_TIMER_DETACHED => u64::from(crate::objects::timer_detached()),
        OP_TIMER_REINIT => {
            crate::objects::timer_reinit(false);
            0
        }
        OP_SIGNAL_IGNORE | OP_SIGNAL_HANDLE => {
            let handler = if op == OP_SIGNAL_HANDLE {
                let handler = crate::server_methods::master_signal_handler as *const () as usize;
                // SAFETY: the master's handler, which runs while `SwooleG.server` is live.
                unsafe { Handler::from_raw(handler) }
            } else {
                Handler::NONE
            };
            let signo = arg.c_cast::<c_int>();
            _ = signal_upstream::set(signo, handler, false);
            0
        }
        OP_MARK_SHUTDOWN => {
            signal_upstream::clear();
            o.gs().start.store(0, core::sync::atomic::Ordering::SeqCst);
            o.gs()
                .shutdown
                .store(1, core::sync::atomic::Ordering::SeqCst);
            0
        }
        OP_COMMAND_HANDLER => {
            let handler: swoole_rs_reactor::Handler = accept_command_result;
            o.reactor().set_handler(SW_FD_PIPE, SW_EVENT_READ, handler);
            0
        }
        op => return startup(o, op, arg),
    })
}

/// The startup ops over the server's factory, timer, callbacks, listening sockets
/// and environment, under their upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn startup(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::callbacks::CB_START;
    use crate::ops::{
        OP_LC_DAEMON, OP_LC_FACTORY_START, OP_LC_INIT_PROTOCOLS, OP_LC_START_PROCESS,
        OP_MASTER_START, OP_MASTER_TIMER,
    };
    use swoole_rs_corebase::swoole_redirect_stdout_file;
    use swoole_rs_ossupport::swoole_daemon;
    use swoole_rs_utilities::{process_logger, swr_v1_logger_is_opened, swr_v1_logger_redirect};
    Some(match op {
        OP_LC_DAEMON => {
            match process_logger() {
                Some(logger) if swr_v1_logger_is_opened(logger) => {
                    swr_v1_logger_redirect(logger, true);
                }
                _ => swoole_redirect_stdout_file(c"/dev/null"),
            }
            // The master, before it starts any thread.
            sign(swoole_daemon(0, 1))
        }
        OP_LC_FACTORY_START => u64::from(crate::factory::start(o.server())),
        // In the master, after the process factory forks and before any reactor
        // thread, base-mode worker or worker thread starts.
        OP_LC_INIT_PROTOCOLS => {
            for port in o.tables().ports() {
                (crate::port_config::methods().init_protocol)(port.config());
            }
            0
        }
        // The master starts its reactor threads.
        OP_LC_START_PROCESS => sign(crate::thread_ops::start_reactor_threads(o.server())),
        OP_MASTER_TIMER => {
            let node = crate::server_methods::master_timer(o.server());
            o.config().master_timer.store(node, Relaxed);
            u64::from(!node.is_null())
        }
        OP_MASTER_START => {
            o.state.callbacks.call_server(CB_START, o.server());
            0
        }
        op => return closing(o, op, arg),
    })
}

/// The ops over a connection's close and the server's shutdown, which
/// call crates/server's, crates/process's and crates/ports' methods under their
/// upstream symbols; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn closing(o: &mut Objects<'_>, op: u32, arg: u64) -> Option<u64> {
    use crate::callbacks::CB_SHUTDOWN;
    use crate::ops::{OP_JOIN_REACTOR_THREADS, OP_NOTIFY_CONNECT, OP_ON_SHUTDOWN, OP_PORT_CLOSE};
    /// `SW_SERVER_EVENT_CONNECT`.
    const EVENT_CONNECT: c_int = 5;
    Some(match op {
        OP_NOTIFY_CONNECT => u64::from(crate::server_methods::notify(
            o.server(),
            o.conn(),
            EVENT_CONNECT,
        )),
        OP_ON_SHUTDOWN => {
            o.state.callbacks.call_server(CB_SHUTDOWN, o.server());
            0
        }
        OP_JOIN_REACTOR_THREADS => {
            crate::reactor_thread::join_reactor_threads(o.server());
            0
        }
        OP_PORT_CLOSE => {
            let port = o.tables().ports().get(usize::try_from(arg).ok()?)?;
            (methods().close)(*port);
            0
        }
        op => return factory_ops(o, op),
    })
}

/// `serv->get_worker_message_bus()`: this thread's in thread mode, else the
/// server's.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn worker_message_bus(state: &ServerState) -> crate::objects::Bus {
    let thread_mode = u64::from(state.config_ref().mode_) == MODE_THREAD;
    let bus = if thread_mode {
        swoole_rs_corebase::thread_message_bus()
    } else {
        state.tables_ref().message_bus
    };
    // SAFETY: the thread's or the server's live `MessageBus`.
    unsafe { crate::objects::Bus::of(bus) }
}

/// Base mode's message bus takes `send`, then `worker_accept_event` of it.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn base_pass(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) {
    let bus = worker_message_bus(serv.state());
    bus.rust().pass(&send.info, send.data);
    let mut head = bus.head();
    crate::event_worker_ops::accept_event(serv, &mut head);
}

/// The message bus writes `send` to the pipe of the worker that owns its session.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn forward(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) -> bool {
    let state = serv.state();
    let Some(session) = state.session(send.info.fd) else {
        return false;
    };
    let id = session.reactor_bits() & 0xff;
    let Some(pipe) = serv.worker_pipe(u16::try_from(id).unwrap_or(u16::MAX), true) else {
        return false;
    };
    let bus = worker_message_bus(state);
    let socket = if u64::from(state.config_ref().mode_) == MODE_THREAD {
        bus.pipe_socket(pipe)
    } else {
        Some(pipe)
    };
    let Some(socket) = socket else {
        return false;
    };
    let mut info = send.info;
    // SAFETY: a live pipe socket and the send it writes.
    unsafe {
        bus.rust()
            .send(socket.as_ptr(), &mut info, send.data.cast())
    }
}

/// The ops over the server's factory and task pool: the process factory's and
/// `ProcessPool`'s methods (crates/process), the factory's virtual members and
/// the task notify pipes; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn factory_ops(o: &mut Objects<'_>, op: u32) -> Option<u64> {
    use crate::factory_ops as process;
    use crate::ops::OP_FACTORY_SHUTDOWN;
    use swoole_rs_process::workers::ops::RUN_DESTROY_PROCESS_FACTORY;
    Some(match op {
        OP_DESTROY_FACTORY => {
            // The live server, whose process factory shutdown destroys.
            process::run_server(o.server(), RUN_DESTROY_PROCESS_FACTORY);
            0
        }
        OP_FACTORY_SHUTDOWN => {
            crate::factory::shutdown(o.state);
            0
        }
        op => return objects(o, op),
    })
}

/// The ops over the server's timers, barriers and pipe buffers; else `None`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn objects(o: &mut Objects<'_>, op: u32) -> Option<u64> {
    use crate::ops::{OP_MANAGER_BARRIER, OP_REACTOR_BARRIER, OP_RELEASE_PIPE_BUFFERS};
    use crate::server_object::barrier;
    Some(match op {
        OP_REACTOR_BARRIER | OP_MANAGER_BARRIER => {
            swoole_rs_locks::barrier_wait(barrier(o.server(), op == OP_MANAGER_BARRIER));
            0
        }
        OP_RELEASE_PIPE_BUFFERS => {
            release_pipe_buffers(o.state);
            0
        }
        _ => return None,
    })
}

/// `Server::create_pipe_buffers()`: the bus's buffer, `ipc_max_size` bytes.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn create_pipe_buffers(serv: crate::view::Server<'_>) -> c_int {
    let state = serv.state();
    let size = state.config_ref().ipc_max_size();
    if state.message_bus().alloc_buffer(size as usize) {
        0
    } else {
        -1
    }
}

/// `MessageBus::free_buffer()` of the server's bus.
#[cfg(all(feature = "upstream-symbols", not(test)))]
fn release_pipe_buffers(state: &ServerState) {
    state.message_bus().free_buffer();
}

/// `Server::accept_command_result`: the master's command pipe is readable; a
/// whole reply goes to its waiting callback. The callback may resume a coroutine:
/// this frame owns nothing that needs a drop then.
///
/// Registered only on the master's reactor, whose `ptr` is the live server, for its
/// command pipe.
#[cfg(all(feature = "upstream-symbols", not(test)))]
extern "C" fn accept_command_result(
    reactor: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    let serv = reactor.object();
    // SAFETY: the master reactor's live server.
    let state = unsafe { crate::view::Server::from_raw(serv) }.state();
    let bus = state.message_bus();
    let Some(sock) = event.socket() else {
        return 0;
    };
    if bus.read(sock) <= 0 {
        return 0;
    }
    let request_id = bus.head().fd;
    bus.with_packet(|message| crate::commands::command_result(state, request_id, message));
    bus.pop();
    0
}

/// Outside the hybrid, whose unit tests link no C++, native code keeps these.
#[cfg(not(all(feature = "upstream-symbols", not(test))))]
fn native(_: &mut Objects<'_>, _: u32, _: u64) -> Option<u64> {
    None
}

/// What `Server::destroy` frees of a base-mode server whose single worker bailed out
/// of its loop: the base factory's connection list and counts, the server's lists and
/// its factory. It runs none of the shutdown (callbacks, hooks, signals, ports), only
/// marks the server shut down, so `swr_v1_server_delete` frees the rest.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn release_abandoned_base(serv: crate::view::Server<'_>) {
    let mut ctx = NativeContext::new(serv.raw());
    let mut o = Objects {
        ctx: &mut ctx,
        server: Some(serv),
        state: serv.state(),
    };
    o.gs()
        .shutdown
        .store(1, core::sync::atomic::Ordering::SeqCst);
    ops(&mut o, OP_DESTROY_FACTORY, MODE_BASE);
    ops(&mut o, crate::ops::OP_FREE_SERVER, 0);
}

#[cfg(target_os = "linux")]
fn sigrtmin() -> u64 {
    sign(libc::SIGRTMIN())
}

#[cfg(not(target_os = "linux"))]
fn sigrtmin() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>() -> NativeContext<'a> {
        NativeContext::new(core::ptr::null_mut())
    }

    #[test]
    fn unknown_ops_and_missing_contexts_fall_through_to_native() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state and a zeroed context.
        unsafe {
            assert_eq!(
                answer(&raw mut ctx, None, &*state, crate::ops::OP_PORT_COUNT, 0),
                None
            );
            assert_eq!(
                answer(
                    core::ptr::null_mut(),
                    None,
                    &*state,
                    OP_FREE_USER_WORKERS,
                    0
                ),
                None
            );
            crate::state::free_state(state);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn freeing_the_user_workers_twice_is_harmless() {
        let state = crate::swr_v1_server_state_new();
        let mut ctx = context();
        // SAFETY: a fresh state; the op frees what the test allocated.
        unsafe {
            let config = &mut *(*state).config();
            config.user_workers = swoole_rs_memory::members::Shm::calloc(1, 8);
            answer(&raw mut ctx, None, &*state, OP_FREE_USER_WORKERS, 0);
            assert!(config.user_workers.is_null());
            answer(&raw mut ctx, None, &*state, OP_FREE_USER_WORKERS, 0);
            crate::state::free_state(state);
        }
    }
}
