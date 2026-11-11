//! The admin commands' facts, read from the live server they run in, and their
//! registration (`register_admin_server_commands`).
use super::{Command, Connection, Facts, Manager, Port, Socket, Thread, commands, run};
use crate::json::Json;
use crate::reactor_ops::ThreadRecord;
use crate::records::{ConnectionRecord, ServerTables, SessionRecord};
use crate::{CommandFn, CommandReply, MODE_BASE, MODE_PROCESS, ServerConfig};
use crate::{ServerState, view};
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::Ordering::Relaxed;
use std::sync::OnceLock;
use swoole_rs_clients::address::{swr_v1_clients_address_host, swr_v1_clients_address_port};
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_reactor::{Registry, Socket as Handle, SocketRecord, reactor_upstream};
use swoole_rs_timer::{TimerStats, owner_queue, swr_v1_timer_stats_get};
use swoole_rs_tls::socket_pair::SocketPair;

/// `zend_memory_usage(true)`, from the PHP binding.
static PHP_MEMORY: OnceLock<extern "C" fn() -> usize> = OnceLock::new();

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;
/// `Server::THREAD_REACTOR`.
const THREAD_REACTOR: u8 = 2;

/// `register_admin_server_commands(serv)`: adds upstream's admin commands, whose
/// `get_memory_info` reports `php_memory()`.
///
/// # Safety
/// `serv` is a live `Server` that has not started.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_admin_register(
    serv: *mut c_void,
    php_memory: extern "C" fn() -> usize,
) {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    PHP_MEMORY.get_or_init(|| php_memory);
    let config = serv.config();
    let event_workers = u64::from(config.mode_) == MODE_BASE || config.single_thread;
    for (index, (name, types)) in commands(event_workers).enumerate() {
        let handler = CommandFn {
            invoke: answer as *const c_void,
            ctx: core::ptr::without_provenance_mut(index),
        };
        let (name, length) = (name.as_ptr().cast(), name.len());
        // SAFETY: per the caller; `answer` is a command handler.
        unsafe { crate::swr_v1_server_add_command(serv.raw(), name, length, types, handler) };
    }
}

/// A `commands::Handler`: admin command `ctx`'s answer to `message`.
///
/// # Safety
/// `serv` is a live `Server` and `message` holds `length` bytes (or is null).
unsafe extern "C" fn answer(
    ctx: *mut c_void,
    serv: *mut c_void,
    message: *const c_char,
    length: usize,
    reply: CommandReply,
    out: *mut c_void,
) {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    let message = if message.is_null() {
        &[][..]
    } else {
        // SAFETY: per the caller.
        unsafe { core::slice::from_raw_parts(message.cast(), length) }
    };
    let state = serv.state();
    let mut live = Live {
        serv,
        state,
        config: state.config_ref(),
        tables: state.tables_ref(),
        sockets: Vec::new(),
        connections: Vec::new(),
    };
    let text = run(&mut live, ctx.addr(), message).dump();
    // SAFETY: the server's reply writer, with its output.
    unsafe { reply(out, text.as_ptr().cast(), text.len()) };
}

/// `Address::get_addr()`, which formats into a per-thread buffer.
fn address(info: &SocketAddress) -> String {
    let host = swr_v1_clients_address_host(info);
    // SAFETY: the text lives until this thread formats another address.
    Json::utf8(unsafe { CStr::from_ptr(host) }.to_bytes())
}

/// `Address::get_port()`.
fn port(info: &SocketAddress) -> i64 {
    swr_v1_clients_address_port(info).into()
}

/// `Socket::get_out_buffer_length()`.
fn out_buffer_length(socket: &SocketRecord) -> u64 {
    let buffer = socket.output.out_buffer;
    if buffer.is_null() {
        return 0;
    }
    // SAFETY: the socket's live output buffer.
    swoole_rs_buffer::cpp::swr_cpp_buffer_length(unsafe { &*buffer.cast() }).into()
}

fn socket(s: &SocketRecord) -> Socket {
    Socket {
        fd: s.fd.into(),
        events: s.events.into(),
        socket_type: s.socket_type.into(),
        fd_type: s.fd_type.into(),
        port: port(&s.info),
        total_recv_bytes: s.total_recv_bytes as u64,
        total_send_bytes: s.total_send_bytes as u64,
        out_buffer_size: out_buffer_length(s),
        address: address(&s.info),
    }
}

/// The collected item `index`; `run` asks only below the count it was given.
fn nth<T>(items: &[*mut T], index: u64) -> *mut T {
    let item = usize::try_from(index).ok().and_then(|i| items.get(i));
    item.copied().unwrap_or_else(|| std::process::abort())
}

/// A live socket of the server's.
fn live<'a>(socket: *mut c_void) -> Handle<'a> {
    // SAFETY: sockets the server, its pools and its reactor hold are live while
    // a command runs.
    unsafe { Handle::from_ptr(socket.cast()) }.unwrap_or_else(|| std::process::abort())
}

/// This thread's reactor's sockets, if it has a reactor.
fn registry<'a>() -> Option<&'a Registry> {
    let reactor = swoole_rs_corebase::thread_reactor();
    if reactor.is_null() {
        return None;
    }
    // SAFETY: this thread's live reactor.
    let registry = unsafe { reactor_upstream::prepared(reactor) }.registry;
    // SAFETY: its live registry, which only this thread changes.
    unsafe { registry.as_ref() }
}

/// `SwooleTG.timer->count()`, or 0 without a timer.
fn timer_count() -> u64 {
    let timer = swoole_rs_corebase::thread_timer();
    if timer.is_null() {
        return 0;
    }
    // SAFETY: this thread's live timer.
    let queue = owner_queue(unsafe { &*timer.cast::<swoole_rs_timer::Owner>() });
    let mut stats = TimerStats::default();
    // SAFETY: the timer's live queue, not borrowed elsewhere during this read.
    let queue = unsafe { queue.as_ref() };
    // SAFETY: a `TimerStats` to fill.
    unsafe { swr_v1_timer_stats_get(queue, &raw mut stats) };
    stats.count as u64
}

struct Live<'a> {
    serv: view::Server<'a>,
    state: &'a ServerState,
    config: &'a ServerConfig,
    tables: &'a ServerTables,
    sockets: Vec<*mut c_void>,
    connections: Vec<*mut ConnectionRecord>,
}

impl Live<'_> {
    /// `serv->get_thread(SwooleTG.id)`.
    fn thread_record(&self) -> &ThreadRecord {
        let id = usize::from(swoole_rs_corebase::thread_id());
        let thread = self.tables.reactor_threads.cast::<ThreadRecord>();
        // SAFETY: a reactor thread's own record, of the live array.
        unsafe { &*thread.wrapping_add(id) }
    }

    /// `serv->get_event_worker_pool()` or `get_task_worker_pool()`.
    fn pool(&self, task: bool) -> &swoole_rs_process::process_pool::PoolRecord {
        let workers = self.serv.workers();
        let pool = if task {
            workers.task_pool
        } else {
            workers.event_pool
        };
        // SAFETY: as above.
        unsafe { &*pool }
    }

    fn connection_record(&self, index: u64) -> &ConnectionRecord {
        let conn = nth(&self.connections, index);
        // SAFETY: a connection collected from the live table.
        unsafe { &*conn }
    }
}

impl Facts for Live<'_> {
    fn pipe_workers(&mut self, task: bool) -> u64 {
        let config = self.config;
        u64::from(if task {
            config.task_worker_num
        } else {
            config.worker_num
        })
    }

    fn pipe_socket(&mut self, task: bool, index: u64, master: bool) -> Socket {
        let workers = self.pool(task).workers;
        let index = usize::try_from(index).unwrap_or(usize::MAX);
        // SAFETY: one of the pool's live workers, `index` below its count.
        let worker: &WorkerRecord = unsafe { &*workers.wrapping_add(index) };
        // SAFETY: the worker's live `UnixSocket`.
        let pipe = unsafe { &*worker.pipe_object.cast::<SocketPair>() };
        let end = if master {
            pipe.master_socket
        } else {
            pipe.worker_socket
        };
        live(end.cast()).read(socket)
    }

    fn has_reactor(&mut self) -> bool {
        !swoole_rs_corebase::thread_reactor().is_null()
    }

    fn sockets(&mut self) -> u64 {
        self.sockets = registry().map(Registry::sockets).unwrap_or_default();
        self.sockets.len() as u64
    }

    fn socket(&mut self, index: u64) -> Socket {
        live(nth(&self.sockets, index)).read(socket)
    }

    fn socket_name(&mut self, index: u64) {
        live(nth(&self.sockets, index)).edit(SocketRecord::get_name);
    }

    fn commands(&mut self) -> Vec<Command> {
        crate::commands::listing(self.state)
            .into_iter()
            .map(|(name, id, types)| Command {
                name: Json::utf8(&name),
                id: id.into(),
                accepted_process_types: types.into(),
            })
            .collect()
    }

    fn connections(&mut self) -> u64 {
        self.connections.clear();
        let gs = self.serv.counters();
        let mode = u64::from(self.config.mode_);
        let reactor_id = i64::from(swoole_rs_corebase::thread_id());
        // SAFETY: this thread's worker record, if it is a worker.
        let worker = unsafe { (*swoole_rs_process::worker_globals::thread_globals()).worker };
        // Server::foreach_connection: each valid connection, max_fd read afresh.
        let mut fd = gs.min_fd;
        while fd <= gs.max_fd {
            let conn = self.serv.at(fd);
            fd += 1;
            let Some(conn) = conn.filter(|conn| conn.active != 0 && !conn.socket.is_null()) else {
                continue;
            };
            let owner = i64::from(conn.reactor_id);
            let other_worker = || {
                // SAFETY: this worker's live record.
                !worker.is_null() && owner != i64::from(unsafe { &*worker }.id())
            };
            if live(conn.socket).read(|s| s.fd_type) != FD_SESSION
                || (mode == MODE_PROCESS && owner != reactor_id)
                || (mode == MODE_BASE && other_worker())
            {
                continue;
            }
            self.connections.push(conn);
        }
        self.connections.len() as u64
    }

    fn session(&mut self, id: i64) -> bool {
        let conn = self.serv.verified(id);
        self.connections = conn.map(core::ptr::from_mut).into_iter().collect();
        !self.connections.is_empty()
    }

    fn connection(&mut self, index: u64) -> Connection {
        let conn = self.connection_record(index);
        let listener = self.serv.port_by_server_fd(conn.server_fd);
        // SAFETY: the connection's live port.
        let listener = live(unsafe { view::port(listener) }.socket).read(|s| s.info);
        let (received, sent) = live(conn.socket).read(|s| (s.total_recv_bytes, s.total_send_bytes));
        Connection {
            session_id: conn.session_id,
            reactor_id: conn.reactor_id.into(),
            fd: conn.fd.into(),
            server_port: port(&listener),
            port: port(&conn.info),
            overflow: conn.overflow.into(),
            recv_queued_bytes: conn.recv_queued_bytes.load(Relaxed).into(),
            send_queued_bytes: conn.send_queued_bytes.into(),
            total_recv_bytes: received as u64,
            total_send_bytes: sent as u64,
            uid: conn.uid.into(),
            connect_time: conn.connect_time,
            last_recv_time: conn.last_recv_time,
            last_send_time: conn.last_send_time,
            last_dispatch_time: conn.last_dispatch_time,
            server_address: address(&listener),
            address: address(&conn.info),
        }
    }

    fn thread(&mut self) -> Thread {
        let thread = self.thread_record();
        Thread {
            id: thread.id.into(),
            dispatch_count: thread.dispatch_count,
            event_num: registry().map_or(0, Registry::count) as u64,
            timer_num: timer_count(),
        }
    }

    fn manager(&mut self) -> Manager {
        let pool = self.pool(false);
        Manager {
            reload_count: pool.reload_count,
            reload_last_time: pool.reload_last_time,
        }
    }

    fn memory(&mut self) -> Vec<(&'static str, u64)> {
        let config = self.config;
        let is_thread = swoole_rs_corebase::thread_kind() == THREAD_REACTOR;
        let bus = if is_thread {
            self.thread_record().message_bus()
        } else {
            // SAFETY: the server's live `MessageBus`.
            unsafe { crate::objects::Bus::of(self.tables.message_bus) }
        };
        let bus_size = bus.rust().memory_size();
        let user_workers = self.serv.user_worker_num();
        let workers = u64::from(config.worker_num) + u64::from(config.task_worker_num);
        let workers = workers + user_workers as u64;
        let global = swoole_rs_memory::members::pool::memory_size();
        let buffer = swoole_rs_corebase::thread_buffer();
        let buffer = buffer.cast::<swoole_rs_buffer::string::CppString>();
        // SAFETY: null, or this thread's live buffer.
        let thread_buffer =
            unsafe { buffer.as_ref() }.map_or(0, swoole_rs_buffer::string::CppString::size);
        let sockets = registry().map(Registry::sockets).unwrap_or_default();
        let out_buffers = sockets
            .iter()
            .map(|&s| live(s).read(out_buffer_length))
            .sum();
        let php = PHP_MEMORY.get().map_or(0, |usage| usage());
        // The server's own records stand for upstream's `sizeof(Server)`.
        let server =
            size_of::<ServerState>() + size_of::<ServerConfig>() + size_of::<ServerTables>();
        let size = |n: usize| n as u64;
        vec![
            ("server", size(server)),
            ("workers", workers * size(size_of::<WorkerRecord>())),
            (
                "connection_list",
                u64::from(self.tables.max_connection) * size(size_of::<ConnectionRecord>()),
            ),
            (
                "session_list",
                u64::from(SW_SESSION_LIST_SIZE) * size(size_of::<SessionRecord>()),
            ),
            ("global_memory", size(global)),
            ("thread_global_memory", size(thread_buffer)),
            ("message_bus", size(bus_size)),
            (
                "socket_list",
                size(sockets.len() * size_of::<SocketRecord>()),
            ),
            ("socket_out_buffer", out_buffers),
            ("php_memory", if is_thread { 0 } else { size(php) }),
        ]
    }

    fn ports(&mut self) -> Vec<Port> {
        let serv = self.serv;
        let ports = self.tables.ports().iter().copied();
        ports
            .map(|listener| {
                // SAFETY: one of the live server's ports.
                let config = unsafe { view::port(listener) };
                let host = config.host();
                // SAFETY: the host's bytes, which the port keeps.
                let host = unsafe { core::slice::from_raw_parts(host.data.cast(), host.length) };
                let protocols = (crate::port_config::methods().get_protocols)(config);
                // SAFETY: as above.
                let connections = unsafe { serv.port_connection_num(listener) };
                Port {
                    host: Json::utf8(host),
                    protocols: Json::utf8(protocols.to_bytes()),
                    port: config.port.into(),
                    backlog: config.backlog.into(),
                    socket_type: config.r#type.into(),
                    ssl: config.ssl.into(),
                    connection_num: i64::try_from(connections).unwrap_or(i64::MAX),
                }
            })
            .collect()
    }
}
