//! The admin server's commands (upstream `ext-src/swoole_admin_server.cc`): each reads
//! the server's state through [`Facts`] and answers `{"code":…,"data":…}` as
//! upstream's nlohmann handlers dumped it.
use crate::json::{self, Json};
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
use crate::tcp_info::{TCP_INFO, TcpInfo, tcp_fields};
use core::ffi::c_int;

#[cfg(not(test))]
#[path = "admin_live.rs"]
mod live;
#[cfg(not(test))]
pub use live::admin_register;

#[derive(Default)]
pub(crate) struct Socket {
    pub(crate) fd: i64,
    pub(crate) events: i64,
    pub(crate) socket_type: i64,
    pub(crate) fd_type: i64,
    pub(crate) port: i64,
    pub(crate) total_recv_bytes: u64,
    pub(crate) total_send_bytes: u64,
    pub(crate) out_buffer_size: u64,
    pub(crate) address: String,
}

#[derive(Default)]
pub(crate) struct Connection {
    pub(crate) session_id: i64,
    pub(crate) reactor_id: i64,
    pub(crate) fd: i64,
    pub(crate) server_port: i64,
    pub(crate) port: i64,
    pub(crate) overflow: u64,
    pub(crate) recv_queued_bytes: u64,
    pub(crate) send_queued_bytes: u64,
    pub(crate) total_recv_bytes: u64,
    pub(crate) total_send_bytes: u64,
    pub(crate) uid: u64,
    pub(crate) connect_time: f64,
    pub(crate) last_recv_time: f64,
    pub(crate) last_send_time: f64,
    pub(crate) last_dispatch_time: f64,
    pub(crate) server_address: String,
    pub(crate) address: String,
}

pub(crate) struct Port {
    pub(crate) host: String,
    pub(crate) protocols: String,
    pub(crate) port: i64,
    pub(crate) backlog: i64,
    pub(crate) socket_type: i64,
    pub(crate) ssl: i64,
    pub(crate) connection_num: i64,
}

pub(crate) struct Command {
    pub(crate) name: String,
    pub(crate) id: i64,
    pub(crate) accepted_process_types: i64,
}

pub(crate) struct Thread {
    pub(crate) id: i64,
    pub(crate) dispatch_count: u64,
    pub(crate) event_num: u64,
    pub(crate) timer_num: u64,
}

pub(crate) struct Manager {
    pub(crate) reload_count: u64,
    pub(crate) reload_last_time: i64,
}

/// `Server::Command::ProcessType`.
const MASTER: c_int = 1 << 1;
const REACTOR_THREAD: c_int = 1 << 2;
const EVENT_WORKER: c_int = 1 << 3;
const ALL_PROCESS: c_int = MASTER | REACTOR_THREAD | EVENT_WORKER | (1 << 4) | (1 << 5);
const MANAGER_PROCESS: c_int = 1 << 5;
/// `get_connections` and `get_connection_info` run where connections live.
const CONNECTION_OWNER: c_int = -1;

/// Upstream's registrations, in order; `get_thread_info` twice, as upstream adds it.
const COMMAND_TABLE: [(&str, c_int); 11] = [
    ("get_all_sockets", ALL_PROCESS),
    ("get_all_commands", ALL_PROCESS),
    ("get_socket_info", ALL_PROCESS),
    ("get_thread_info", ALL_PROCESS),
    ("get_manager_info", MANAGER_PROCESS),
    ("get_thread_info", ALL_PROCESS),
    ("get_memory_info", ALL_PROCESS),
    ("get_all_unix_sockets", ALL_PROCESS),
    ("get_all_ports", MASTER),
    ("get_connections", CONNECTION_OWNER),
    ("get_connection_info", CONNECTION_OWNER),
];

/// Upstream's admin commands in registration order, each with the process types
/// that accept it. `event_workers` is whether connections live in event workers
/// (base mode or a single-threaded server).
fn commands(event_workers: bool) -> impl Iterator<Item = (&'static str, c_int)> {
    COMMAND_TABLE.into_iter().map(move |(name, types)| {
        let types = match (types, event_workers) {
            (CONNECTION_OWNER, true) => EVENT_WORKER | MASTER,
            (CONNECTION_OWNER, false) => REACTOR_THREAD,
            (types, _) => types,
        };
        (name, types)
    })
}

/// What an admin command reads of the server it runs in.
pub(crate) trait Facts {
    /// The event or task worker count.
    fn pipe_workers(&mut self, task: bool) -> u64;
    /// One end of worker `index`'s pipe.
    fn pipe_socket(&mut self, task: bool, index: u64, master: bool) -> Socket;
    fn has_reactor(&mut self) -> bool;
    /// Collects the reactor's sockets; how many.
    fn sockets(&mut self) -> u64;
    /// The collected socket `index`.
    fn socket(&mut self, index: u64) -> Socket;
    /// `Socket::get_name()` on the collected socket `index`.
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "android")),
        expect(dead_code, reason = "only Linux lists the reactor's other sockets")
    )]
    fn socket_name(&mut self, index: u64);
    fn commands(&mut self) -> Vec<Command>;
    /// Collects the connections this thread or worker owns; how many.
    fn connections(&mut self) -> u64;
    /// Collects the connection of session `id` if it is valid.
    fn session(&mut self, id: i64) -> bool;
    /// The collected connection `index`.
    fn connection(&mut self, index: u64) -> Connection;
    fn thread(&mut self) -> Thread;
    fn manager(&mut self) -> Manager;
    /// Named sizes.
    fn memory(&mut self) -> Vec<(&'static str, u64)>;
    fn ports(&mut self) -> Vec<Port>;
}

fn answer(code: i64, data: Json) -> Json {
    Json::object([("code", Json::Int(code)), ("data", data)])
}

fn require(name: &str) -> Json {
    answer(4003, Json::Str(format!("require parameter {name}")))
}

/// Admin command `index`'s answer to request `msg`.
fn run<F: Facts>(f: &mut F, index: usize, msg: &[u8]) -> Json {
    match COMMAND_TABLE.get(index).map_or("", |(name, _)| name) {
        "get_all_sockets" => all_sockets(f),
        "get_all_commands" => all_commands(f),
        "get_socket_info" => socket_info(msg),
        "get_thread_info" => thread_info(f),
        "get_manager_info" => manager_info(f),
        "get_memory_info" => memory_info(f),
        "get_all_unix_sockets" => all_unix_sockets(f, msg),
        "get_all_ports" => all_ports(f),
        "get_connections" => connections(f),
        _ => connection_info(f, msg),
    }
}

fn i(value: i64) -> Json {
    Json::Int(value)
}

fn u(value: u64) -> Json {
    Json::Uint(value)
}

fn all_unix_sockets<F: Facts>(f: &mut F, msg: &[u8]) -> Json {
    let Some(kind) = json::param(msg, "type") else {
        return require("type");
    };
    let task = kind != "event";
    let mut sockets = Vec::new();
    for index in 0..f.pipe_workers(task) {
        for master in [true, false] {
            let s = f.pipe_socket(task, index, master);
            sockets.push(Json::object([
                ("fd", i(s.fd)),
                ("events", i(s.events)),
                ("total_recv_bytes", u(s.total_recv_bytes)),
                ("total_send_bytes", u(s.total_send_bytes)),
                ("out_buffer_size", u(s.out_buffer_size)),
            ]));
        }
    }
    answer(0, Json::Array(sockets))
}

/// `SW_SOCK_TCP..=SW_SOCK_UNIX_DGRAM`.
const NETWORK_TYPES: core::ops::RangeInclusive<i64> = 1..=6;

/// Whether to list a reactor socket of another type: upstream lists it when it is
/// not a socket, or is one whose domain and type read back (refreshing its name).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn other_socket_listed<F: Facts>(f: &mut F, index: u64, fd: c_int) -> bool {
    // `SO_DOMAIN` on Linux's generic socket ABI; the `libc` crate only binds it for Android.
    #[cfg(not(any(target_arch = "mips", target_arch = "mips64", target_arch = "sparc64")))]
    const SO_DOMAIN: c_int = 39;
    #[cfg(any(target_arch = "mips", target_arch = "mips64", target_arch = "sparc64"))]
    const SO_DOMAIN: c_int = 0x1029;
    // SAFETY: an all-zero `stat` is a valid out value.
    let mut stat: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `stat` is writable.
    if unsafe { libc::fstat(fd, &raw mut stat) } == -1 {
        return false;
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return true;
    }
    for option in [SO_DOMAIN, libc::SO_TYPE] {
        let mut value: c_int = 0;
        let mut len = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or(0);
        // SAFETY: `value` has `len` writable bytes.
        let read = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&raw mut value).cast(),
                &raw mut len,
            )
        };
        if read < 0 {
            return false;
        }
    }
    f.socket_name(index);
    true
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn other_socket_listed<F: Facts>(_: &mut F, _: u64, _: c_int) -> bool {
    false
}

fn all_sockets<F: Facts>(f: &mut F) -> Json {
    if !f.has_reactor() {
        return answer(4004, Json::Str("No event loop created".into()));
    }
    let mut sockets = Vec::new();
    for index in 0..f.sockets() {
        let s = f.socket(index);
        let s = if NETWORK_TYPES.contains(&s.socket_type) {
            s
        } else if other_socket_listed(f, index, c_int::try_from(s.fd).unwrap_or(-1)) {
            f.socket(index)
        } else {
            continue;
        };
        sockets.push(Json::object([
            ("fd", i(s.fd)),
            ("address", Json::Str(s.address)),
            ("port", i(s.port)),
            ("events", i(s.events)),
            ("socket_type", i(s.socket_type)),
            ("fd_type", i(s.fd_type)),
            ("total_recv_bytes", u(s.total_recv_bytes)),
            ("total_send_bytes", u(s.total_send_bytes)),
            ("out_buffer_size", u(s.out_buffer_size)),
        ]));
    }
    answer(0, Json::Array(sockets))
}

fn all_commands<F: Facts>(f: &mut F) -> Json {
    let list = f
        .commands()
        .into_iter()
        .map(|c| {
            Json::object([
                ("id", i(c.id)),
                ("name", Json::Str(c.name)),
                ("accepted_process_types", i(c.accepted_process_types)),
            ])
        })
        .collect();
    answer(0, Json::Array(list))
}

/// `sw_atoi`: `strtol` base 10, saturating, truncated to `int`.
fn atoi(text: &str) -> i32 {
    let text = text.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let mut value: i64 = 0;
    for byte in digits.bytes().take_while(u8::is_ascii_digit) {
        let digit = i64::from(byte - b'0');
        value = if negative {
            value.saturating_mul(10).saturating_sub(digit)
        } else {
            value.saturating_mul(10).saturating_add(digit)
        };
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`static_cast<int>` of a `long` keeps its low 32 bits"
    )]
    let value = value as i32;
    value
}

fn socket_info(msg: &[u8]) -> Json {
    let Some(fd) = json::param(msg, "fd") else {
        return require("fd");
    };
    if !TCP_INFO_SUPPORTED {
        return answer(5001, Json::Str("platform unsupported".into()));
    }
    let data = match tcp_info_of(atoi(&fd)) {
        Some(fields) => Json::Object(fields.into_iter().collect()),
        // Upstream returned the dumped error object as the data.
        None => Json::Str(
            answer(
                5001,
                Json::Str("failed to getsockopt(TCP_INFO) for socket".into()),
            )
            .dump(),
        ),
    };
    answer(0, data)
}

/// `std::this_thread::get_id()` as the C++ library prints it.
fn thread_id() -> String {
    let thread = crate::reactor_ops::pthread_self();
    #[cfg(target_vendor = "apple")]
    return format!("{thread:#x}");
    // musl's pthread_t is a pointer, which libstdc++ prints as one.
    #[cfg(target_env = "musl")]
    return format!("{thread:p}");
    #[cfg(not(any(target_vendor = "apple", target_env = "musl")))]
    return thread.to_string();
}

fn thread_info<F: Facts>(f: &mut F) -> Json {
    let t = f.thread();
    answer(
        0,
        Json::object([
            ("tid", Json::Str(thread_id())),
            ("id", i(t.id)),
            ("dispatch_count", u(t.dispatch_count)),
            ("event_num", u(t.event_num)),
            ("timer_num", u(t.timer_num)),
        ]),
    )
}

fn manager_info<F: Facts>(f: &mut F) -> Json {
    let m = f.manager();
    let pid = std::process::id().cast_signed();
    answer(
        0,
        Json::object([
            ("pid", i(i64::from(pid))),
            ("reload_count", u(m.reload_count)),
            ("reload_last_time", i(m.reload_last_time)),
        ]),
    )
}

fn memory_info<F: Facts>(f: &mut F) -> Json {
    let sizes = f.memory().into_iter().map(|(name, size)| (name, u(size)));
    answer(0, Json::Object(sizes.collect()))
}

fn connection_json(c: Connection) -> Json {
    let server_port = format!("{}:{}", c.server_address, c.server_port);
    Json::object([
        ("session_id", i(c.session_id)),
        ("reactor_id", i(c.reactor_id)),
        ("fd", i(c.fd)),
        ("server_port", Json::Str(server_port)),
        ("address", Json::Str(c.address)),
        ("port", i(c.port)),
        ("overflow", u(c.overflow)),
        ("connect_time", Json::Float(c.connect_time)),
        ("last_recv_time", Json::Float(c.last_recv_time)),
        ("last_send_time", Json::Float(c.last_send_time)),
        ("last_dispatch_time", Json::Float(c.last_dispatch_time)),
        ("recv_queued_bytes", u(c.recv_queued_bytes)),
        ("send_queued_bytes", u(c.send_queued_bytes)),
        ("total_recv_bytes", u(c.total_recv_bytes)),
        ("total_send_bytes", u(c.total_send_bytes)),
        ("uid", u(c.uid)),
    ])
}

fn connections<F: Facts>(f: &mut F) -> Json {
    let list = (0..f.connections())
        .map(|index| connection_json(f.connection(index)))
        .collect();
    answer(0, Json::Array(list))
}

fn connection_info<F: Facts>(f: &mut F, msg: &[u8]) -> Json {
    let Some(session_id) = json::param(msg, "session_id") else {
        return require("session_id");
    };
    // An `int` session id, widened as `SessionId`.
    if !f.session(atoi(&session_id).into()) {
        return answer(4004, Json::Str("connection not exists".into()));
    }
    answer(0, connection_json(f.connection(0)))
}

fn all_ports<F: Facts>(f: &mut F) -> Json {
    let list = f
        .ports()
        .into_iter()
        .map(|p| {
            Json::object([
                ("host", Json::Str(p.host)),
                ("port", i(p.port)),
                ("backlog", i(p.backlog)),
                ("type", i(p.socket_type)),
                ("ssl", i(p.ssl)),
                ("protocols", Json::Str(p.protocols)),
                ("connection_num", i(p.connection_num)),
            ])
        })
        .collect();
    answer(0, Json::Array(list))
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
const TCP_INFO_SUPPORTED: bool = true;
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
const TCP_INFO_SUPPORTED: bool = false;

/// `getsockopt(TCP_INFO)` on `fd`, as `get_socket_info` reports it.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn tcp_info_of(fd: i32) -> Option<Vec<(&'static str, Json)>> {
    // SAFETY: an all-zero `tcp_info` is a valid out value.
    let mut info: TcpInfo = unsafe { core::mem::zeroed() };
    let mut len = libc::socklen_t::try_from(size_of::<TcpInfo>()).unwrap_or(0);
    // SAFETY: `info` has `len` writable bytes.
    let read = unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            TCP_INFO,
            (&raw mut info).cast(),
            &raw mut len,
        )
    };
    if read < 0 {
        return None;
    }
    let fields = tcp_fields(&info);
    Some(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_str().unwrap_or_default(), u(value)))
            .collect(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn tcp_info_of(_: i32) -> Option<Vec<(&'static str, Json)>> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND_TABLE, Command, Connection, Facts, Manager, Port, Socket, Thread, atoi, commands,
        run,
    };

    #[derive(Default)]
    struct Script {
        log: Vec<(&'static str, u64)>,
        workers: u64,
        session: bool,
    }

    impl Facts for Script {
        fn pipe_workers(&mut self, task: bool) -> u64 {
            self.log.push(("pipe_workers", task.into()));
            self.workers
        }
        fn pipe_socket(&mut self, task: bool, index: u64, master: bool) -> Socket {
            let arg = index << 2 | u64::from(task) << 1 | u64::from(master);
            self.log.push(("pipe_socket", arg));
            Socket {
                fd: i64::try_from(arg).unwrap_or(0),
                events: 1,
                total_recv_bytes: 2,
                ..Socket::default()
            }
        }
        fn has_reactor(&mut self) -> bool {
            self.log.push(("has_reactor", 0));
            false
        }
        fn sockets(&mut self) -> u64 {
            0
        }
        fn socket(&mut self, _: u64) -> Socket {
            Socket::default()
        }
        fn socket_name(&mut self, _: u64) {}
        fn commands(&mut self) -> Vec<Command> {
            vec![Command {
                name: "get_all_ports".into(),
                id: 9,
                accepted_process_types: 2,
            }]
        }
        fn connections(&mut self) -> u64 {
            0
        }
        fn session(&mut self, id: i64) -> bool {
            self.log.push(("session", id.cast_unsigned()));
            self.session
        }
        fn connection(&mut self, _: u64) -> Connection {
            Connection {
                session_id: 7,
                reactor_id: 1,
                fd: 12,
                server_port: 9501,
                port: 40000,
                connect_time: 1_730_000_000.5,
                server_address: "127.0.0.1".into(),
                address: "127.0.0.2".into(),
                ..Connection::default()
            }
        }
        fn thread(&mut self) -> Thread {
            Thread {
                id: 0,
                dispatch_count: 0,
                event_num: 0,
                timer_num: 0,
            }
        }
        fn manager(&mut self) -> Manager {
            Manager {
                reload_count: 0,
                reload_last_time: 0,
            }
        }
        fn memory(&mut self) -> Vec<(&'static str, u64)> {
            vec![("server", 100), ("php_memory", 5)]
        }
        fn ports(&mut self) -> Vec<Port> {
            Vec::new()
        }
    }

    fn index(name: &str) -> usize {
        COMMAND_TABLE
            .iter()
            .position(|(found, _)| *found == name)
            .unwrap_or(0)
    }

    #[test]
    fn registers_upstreams_commands_in_order() {
        let names: Vec<_> = commands(true).collect();
        assert_eq!(names.len(), 11, "get_thread_info is registered twice");
        assert_eq!(names[4], ("get_manager_info", 32), "manager only");
        assert_eq!(
            names[9],
            ("get_connections", 8 | 2),
            "event worker and master"
        );
        let types = commands(false).nth(10).map(|(_, types)| types);
        assert_eq!(
            types,
            Some(4),
            "reactor threads own connections in process mode"
        );
    }

    #[test]
    fn unix_sockets_list_master_then_worker_pipes() {
        let mut f = Script {
            workers: 2,
            ..Script::default()
        };
        let out = run(&mut f, index("get_all_unix_sockets"), br#"{"type":"task"}"#).dump();
        let pipes: Vec<u64> = f
            .log
            .iter()
            .filter(|(op, _)| *op == "pipe_socket")
            .map(|(_, a)| *a)
            .collect();
        assert_eq!(pipes, [3, 2, 7, 6], "index << 2 | task << 1 | master");
        assert!(
            out.starts_with(
                r#"{"code":0,"data":[{"events":1,"fd":3,"out_buffer_size":0,"total_recv_bytes":2,"#
            ),
            "{out}"
        );
        let out = run(&mut f, index("get_all_unix_sockets"), b"{}").dump();
        assert_eq!(
            out, r#"{"code":4003,"data":"require parameter type"}"#,
            "missing type"
        );
    }

    #[test]
    fn commands_and_memory() {
        let mut f = Script::default();
        let out = run(&mut f, index("get_all_commands"), b"").dump();
        assert_eq!(
            out,
            r#"{"code":0,"data":[{"accepted_process_types":2,"id":9,"name":"get_all_ports"}]}"#,
            "commands"
        );
        let out = run(&mut f, index("get_memory_info"), b"").dump();
        assert_eq!(
            out, r#"{"code":0,"data":{"php_memory":5,"server":100}}"#,
            "memory facts sorted"
        );
        let out = run(&mut f, index("get_all_sockets"), b"").dump();
        assert_eq!(
            out, r#"{"code":4004,"data":"No event loop created"}"#,
            "no reactor"
        );
        assert!(f.log.contains(&("has_reactor", 0)), "asked for the reactor");
    }

    #[test]
    fn connection_info_answers() {
        let mut f = Script::default();
        let out = run(
            &mut f,
            index("get_connection_info"),
            br#"{"session_id":"7"}"#,
        )
        .dump();
        assert_eq!(
            out, r#"{"code":4004,"data":"connection not exists"}"#,
            "unknown session"
        );
        assert!(f.log.contains(&("session", 7)), "session id parsed");
        f.session = true;
        let out = run(
            &mut f,
            index("get_connection_info"),
            br#"{"session_id":"7"}"#,
        )
        .dump();
        assert_eq!(
            out,
            concat!(
                r#"{"code":0,"data":{"address":"127.0.0.2","connect_time":1730000000.5,"fd":12,"#,
                r#""last_dispatch_time":0.0,"last_recv_time":0.0,"last_send_time":0.0,"overflow":0,"#,
                r#""port":40000,"reactor_id":1,"recv_queued_bytes":0,"send_queued_bytes":0,"#,
                r#""server_port":"127.0.0.1:9501","session_id":7,"total_recv_bytes":0,"total_send_bytes":0,"uid":0}}"#
            ),
            "connection"
        );
    }

    #[test]
    fn socket_info_requires_fd() {
        let out = run(&mut Script::default(), index("get_socket_info"), b"{}").dump();
        assert_eq!(
            out, r#"{"code":4003,"data":"require parameter fd"}"#,
            "missing fd"
        );
        let out = run(
            &mut Script::default(),
            index("get_socket_info"),
            br#"{"fd":"-1"}"#,
        )
        .dump();
        assert_eq!(
            out,
            r#"{"code":0,"data":"{\"code\":5001,\"data\":\"failed to getsockopt(TCP_INFO) for socket\"}"}"#,
            "a bad fd answers the dumped error"
        );
    }

    #[test]
    fn atoi_is_strtol() {
        assert_eq!(atoi(" 42abc"), 42, "leading space, trailing junk");
        assert_eq!(atoi("-7"), -7, "sign");
        assert_eq!(atoi("+3"), 3, "plus");
        assert_eq!(atoi("x"), 0, "no digits");
        assert_eq!(atoi("4294967297"), 1, "low 32 bits");
        assert_eq!(atoi("99999999999999999999"), -1, "LONG_MAX's low bits");
    }
}
