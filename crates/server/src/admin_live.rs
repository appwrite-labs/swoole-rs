//! The admin commands' facts, read from the live server they run in, and their
//! registration (`register_admin_server_commands`).
use super::{Command, Connection, Facts, Manager, Port, Socket, Thread, commands, run};
use crate::json::Json;
use crate::reactor_ops::ThreadRecord;
use crate::records::{ConnectionRecord, ServerTables, SessionRecord};
use crate::{MODE_BASE, MODE_PROCESS, ServerConfig};
use crate::{ServerState, view};
use core::ffi::{c_int, c_void};
use core::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::sync::OnceLock;
use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
use swoole_rs_corebase::constants::SW_SESSION_LIST_SIZE;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_reactor::{Registry, Socket as Handle, SocketFields, SocketRecord};
use swoole_rs_tls::socket_pair::SocketPair;

/// `zend_memory_usage(true)`, from the PHP binding.
static PHP_MEMORY: OnceLock<extern "C" fn() -> usize> = OnceLock::new();

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;
/// `Server::THREAD_REACTOR`.
const THREAD_REACTOR: u8 = 2;

/// `register_admin_server_commands(serv)`: adds upstream's admin commands, whose
/// `get_memory_info` reports `php_memory()`.
pub fn admin_register(serv: view::Server<'_>, php_memory: extern "C" fn() -> usize) {
    PHP_MEMORY.get_or_init(|| php_memory);
    let config = serv.config();
    let event_workers = u64::from(config.mode_) == MODE_BASE || config.single_thread;
    for (index, (name, types)) in commands(event_workers).enumerate() {
        let handler = Arc::new(
            move |serv: view::Server<'_>, message: &[u8], reply: &mut Vec<u8>| {
                reply.extend_from_slice(answer(serv, index, message).as_bytes());
            },
        );
        crate::swr_v1_server_add_command(serv, name.as_bytes(), types, handler);
    }
}

/// Admin command `index`'s answer to `message`.
fn answer(serv: view::Server<'_>, index: usize, message: &[u8]) -> String {
    let state = serv.state();
    let mut live = Live {
        serv,
        state,
        config: state.config_ref(),
        tables: state.tables_ref(),
        sockets: Vec::new(),
        connections: Vec::new(),
    };
    run(&mut live, index, message).dump()
}

/// `Address::get_addr()`.
fn address(info: &SocketAddress) -> String {
    with_host(info, |host| Json::utf8(host.to_bytes()))
}

/// `Address::get_port()`.
fn port(info: &SocketAddress) -> i64 {
    swr_v1_clients_address_port(info).into()
}

/// `Socket::get_out_buffer_length()`.
fn out_buffer_length(socket: &SocketFields) -> u64 {
    let length = swoole_rs_buffer::cpp::swr_cpp_buffer_length;
    crate::reactor_ops::out_buffer(socket).map_or(0, |buffer| length(buffer).into())
}

fn socket(s: &SocketFields) -> Socket {
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
    let registry = swoole_rs_reactor::ReactorPrepared::of(reactor).registry;
    // SAFETY: this thread's live reactor's registry, which only this thread changes.
    unsafe { registry.as_ref() }
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
        let workers = self.pool(task).workers();
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
        live(nth(&self.sockets, index)).edit(SocketFields::get_name);
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
        let worker = swoole_rs_process::worker_globals::current_worker();
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
        let config = self.serv.port_of_server_fd(conn.server_fd);
        // A connection's port is live while a command runs, as `live`'s sockets are.
        let config = config.unwrap_or_else(|| std::process::abort());
        let listener = live(config.socket).read(|s| s.info);
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
            timer_num: swoole_rs_timer::thread_timer_count() as u64,
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
            self.serv.state().message_bus()
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
        self.tables
            .ports()
            .iter()
            .map(|port| {
                let config = port.config();
                let host = config.host_bytes();
                let protocols = (crate::port_config::methods().get_protocols)(config);
                let connections = serv.port_connections(config);
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
