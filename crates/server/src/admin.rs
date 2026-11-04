//! The admin server's commands (upstream `ext-src/swoole_admin_server.cc`): each reads
//! the server's state through native facts and answers `{"code":…,"data":…}` as
//! upstream's nlohmann handlers dumped it. Also the `TCP_INFO` fields that both the
//! `get_socket_info` command and `Coroutine\Socket::getOption` report.
use crate::json::{self, Json};
use core::ffi::{CStr, c_char, c_int, c_void};
use std::collections::BTreeMap;

/// One native fact or effect for an admin command; `out` receives a record.
pub(crate) type AdminCall =
    unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64, out: *mut c_void) -> u64;

#[cfg(any(target_os = "linux", target_os = "android"))]
use crate::ops::ADMIN_SOCKET_NAME as SOCKET_NAME;
use crate::ops::{
    ADMIN_ANSWER as ANSWER, ADMIN_COMMAND as COMMAND, ADMIN_COMMANDS as COMMANDS,
    ADMIN_CONNECTION as CONNECTION, ADMIN_CONNECTIONS as CONNECTIONS,
    ADMIN_HAS_REACTOR as HAS_REACTOR, ADMIN_MANAGER as MANAGER, ADMIN_MEMORY as MEMORY,
    ADMIN_PIPE_SOCKET as PIPE_SOCKET, ADMIN_PIPE_WORKERS as PIPE_WORKERS, ADMIN_PORT as PORT,
    ADMIN_PORTS as PORTS, ADMIN_SESSION as SESSION, ADMIN_SOCKET as SOCKET,
    ADMIN_SOCKETS as SOCKETS, ADMIN_THREAD as THREAD,
};

/// A named number (`swr_v1_server_fact`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Fact {
    pub name: *const c_char,
    pub value: u64,
}

const NO_FACT: Fact = Fact {
    name: core::ptr::null(),
    value: 0,
};

#[repr(C)]
#[derive(Default)]
struct Socket {
    fd: i64,
    events: i64,
    socket_type: i64,
    fd_type: i64,
    port: i64,
    total_recv_bytes: u64,
    total_send_bytes: u64,
    out_buffer_size: u64,
    address: Text,
}

#[repr(C)]
#[derive(Default)]
struct Connection {
    session_id: i64,
    reactor_id: i64,
    fd: i64,
    server_port: i64,
    port: i64,
    overflow: u64,
    recv_queued_bytes: u64,
    send_queued_bytes: u64,
    total_recv_bytes: u64,
    total_send_bytes: u64,
    uid: u64,
    connect_time: f64,
    last_recv_time: f64,
    last_send_time: f64,
    last_dispatch_time: f64,
    server_address: Text,
    address: Text,
}

#[repr(C)]
#[derive(Default)]
struct Port {
    host: Text,
    protocols: Text,
    port: i64,
    backlog: i64,
    socket_type: i64,
    ssl: i64,
    connection_num: i64,
}

#[repr(C)]
#[derive(Default)]
struct Command {
    name: Text,
    id: i64,
    accepted_process_types: i64,
}

#[repr(C)]
#[derive(Default)]
struct Thread {
    id: i64,
    dispatch_count: u64,
    event_num: u64,
    timer_num: u64,
}

#[repr(C)]
#[derive(Default)]
struct Manager {
    reload_count: u64,
    reload_last_time: i64,
}

/// Bytes native code lends for the call that filled them.
#[repr(C)]
struct Text {
    data: *const c_char,
    len: usize,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            data: core::ptr::null(),
            len: 0,
        }
    }
}

impl Text {
    fn string(&self) -> String {
        if self.data.is_null() {
            return String::new();
        }
        // SAFETY: native code lends `len` bytes at `data` until its next call.
        Json::utf8(unsafe { core::slice::from_raw_parts(self.data.cast::<u8>(), self.len) })
    }

    fn json(&self) -> Json {
        Json::Str(self.string())
    }
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
const COMMAND_TABLE: [(&CStr, c_int); 11] = [
    (c"get_all_sockets", ALL_PROCESS),
    (c"get_all_commands", ALL_PROCESS),
    (c"get_socket_info", ALL_PROCESS),
    (c"get_thread_info", ALL_PROCESS),
    (c"get_manager_info", MANAGER_PROCESS),
    (c"get_thread_info", ALL_PROCESS),
    (c"get_memory_info", ALL_PROCESS),
    (c"get_all_unix_sockets", ALL_PROCESS),
    (c"get_all_ports", MASTER),
    (c"get_connections", CONNECTION_OWNER),
    (c"get_connection_info", CONNECTION_OWNER),
];

/// Admin command `index`'s name, with the process types that accept it in
/// `accepted_process_types`; null past the last. `event_workers` is whether
/// connections live in event workers (base mode or a single-threaded server).
///
/// # Safety
/// `accepted_process_types` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_admin_command(
    index: u32,
    event_workers: bool,
    accepted_process_types: *mut c_int,
) -> *const c_char {
    let Some(&(name, types)) = usize::try_from(index)
        .ok()
        .and_then(|index| COMMAND_TABLE.get(index))
    else {
        return core::ptr::null();
    };
    let types = match (types, event_workers) {
        (CONNECTION_OWNER, true) => EVENT_WORKER | MASTER,
        (CONNECTION_OWNER, false) => REACTOR_THREAD,
        (types, _) => types,
    };
    // SAFETY: the caller passes a writable int.
    unsafe { accepted_process_types.write(types) };
    name.as_ptr()
}

/// Answers admin command `index` to request `msg` (`len` bytes) through `call`'s
/// `ANSWER`.
///
/// # Safety
/// `msg` has `len` readable bytes; `call` acts on the server in `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_admin_run(
    index: u32,
    msg: *const c_char,
    len: usize,
    context: *mut c_void,
    call: AdminCall,
) {
    let msg = if msg.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller lends `len` bytes at `msg`.
        unsafe { core::slice::from_raw_parts(msg.cast::<u8>(), len) }
    };
    let mut host = Native { context, call };
    let answer = run(&mut host, index, msg).dump();
    let len = u64::try_from(answer.len()).unwrap_or(u64::MAX);
    host.call(ANSWER, len, answer.as_ptr().cast_mut().cast());
}

trait Facts {
    fn call(&mut self, op: u32, arg: u64, out: *mut c_void) -> u64;

    fn get(&mut self, op: u32, arg: u64) -> u64 {
        self.call(op, arg, core::ptr::null_mut())
    }

    fn record<T: Default>(&mut self, op: u32, arg: u64) -> T {
        let mut record = T::default();
        self.call(op, arg, (&raw mut record).cast());
        record
    }
}

struct Native {
    context: *mut c_void,
    call: AdminCall,
}

impl Facts for Native {
    fn call(&mut self, op: u32, arg: u64, out: *mut c_void) -> u64 {
        // SAFETY: a native fact on the live server in `context`; `out` is null or
        // the record `op` fills.
        unsafe { (self.call)(self.context, op, arg, out) }
    }
}

fn answer(code: i64, data: Json) -> Json {
    Json::object([("code", Json::Int(code)), ("data", data)])
}

fn require(name: &str) -> Json {
    answer(4003, Json::Str(format!("require parameter {name}")))
}

fn run<F: Facts>(f: &mut F, index: u32, msg: &[u8]) -> Json {
    let name = usize::try_from(index)
        .ok()
        .and_then(|index| COMMAND_TABLE.get(index))
        .map_or(&b""[..], |(name, _)| name.to_bytes());
    match name {
        b"get_all_sockets" => all_sockets(f),
        b"get_all_commands" => all_commands(f),
        b"get_socket_info" => socket_info(msg),
        b"get_thread_info" => thread_info(f),
        b"get_manager_info" => manager_info(f),
        b"get_memory_info" => memory_info(f),
        b"get_all_unix_sockets" => all_unix_sockets(f, msg),
        b"get_all_ports" => all_ports(f),
        b"get_connections" => connections(f),
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
    let task = u64::from(kind != "event");
    let mut sockets = Vec::new();
    for index in 0..f.get(PIPE_WORKERS, task) {
        for master in [1, 0] {
            let s: Socket = f.record(PIPE_SOCKET, index << 2 | task << 1 | master);
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
    f.get(SOCKET_NAME, index);
    true
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn other_socket_listed<F: Facts>(_: &mut F, _: u64, _: c_int) -> bool {
    false
}

fn all_sockets<F: Facts>(f: &mut F) -> Json {
    if f.get(HAS_REACTOR, 0) == 0 {
        return answer(4004, Json::Str("No event loop created".into()));
    }
    let mut sockets = Vec::new();
    for index in 0..f.get(SOCKETS, 0) {
        let s: Socket = f.record(SOCKET, index);
        let s = if NETWORK_TYPES.contains(&s.socket_type) {
            s
        } else if other_socket_listed(f, index, c_int::try_from(s.fd).unwrap_or(-1)) {
            f.record(SOCKET, index)
        } else {
            continue;
        };
        sockets.push(Json::object([
            ("fd", i(s.fd)),
            ("address", s.address.json()),
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
    let list = (0..f.get(COMMANDS, 0))
        .map(|index| {
            let c: Command = f.record(COMMAND, index);
            Json::object([
                ("id", i(c.id)),
                ("name", c.name.json()),
                ("accepted_process_types", i(c.accepted_process_types)),
            ])
        })
        .collect();
    answer(0, Json::Array(list))
}

/// `sw_atoi`: `strtol` base 10, saturating, truncated to `int`.
fn atoi(text: &str) -> i32 {
    let text = text.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
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
    // SAFETY: no preconditions.
    let thread = unsafe { libc::pthread_self() };
    #[cfg(target_vendor = "apple")]
    return format!("{thread:#x}");
    // musl's pthread_t is a pointer, which libstdc++ streams as one.
    #[cfg(target_env = "musl")]
    return format!("{thread:p}");
    #[cfg(not(any(target_vendor = "apple", target_env = "musl")))]
    return thread.to_string();
}

fn thread_info<F: Facts>(f: &mut F) -> Json {
    let t: Thread = f.record(THREAD, 0);
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
    let m: Manager = f.record(MANAGER, 0);
    // SAFETY: no preconditions.
    let pid = unsafe { libc::getpid() };
    answer(
        0,
        Json::object([
            ("pid", i(i64::from(pid))),
            ("reload_count", u(m.reload_count)),
            ("reload_last_time", i(m.reload_last_time)),
        ]),
    )
}

fn named(facts: &[Fact]) -> Json {
    let mut fields = BTreeMap::new();
    for fact in facts.iter().filter(|fact| !fact.name.is_null()) {
        // SAFETY: native names are string literals, so they live for the program.
        let name: &'static CStr = unsafe { CStr::from_ptr(fact.name) };
        if let Ok(name) = name.to_str() {
            fields.insert(name, u(fact.value));
        }
    }
    Json::Object(fields)
}

fn memory_info<F: Facts>(f: &mut F) -> Json {
    let mut facts = [NO_FACT; 16];
    let n = f.call(MEMORY, 0, facts.as_mut_ptr().cast());
    let n = usize::try_from(n).unwrap_or(0).min(facts.len());
    answer(0, named(facts.get(..n).unwrap_or_default()))
}

fn connection_json(c: &Connection) -> Json {
    let server_port = format!("{}:{}", c.server_address.string(), c.server_port);
    Json::object([
        ("session_id", i(c.session_id)),
        ("reactor_id", i(c.reactor_id)),
        ("fd", i(c.fd)),
        ("server_port", Json::Str(server_port)),
        ("address", c.address.json()),
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
    let list = (0..f.get(CONNECTIONS, 0))
        .map(|index| connection_json(&f.record(CONNECTION, index)))
        .collect();
    answer(0, Json::Array(list))
}

fn connection_info<F: Facts>(f: &mut F, msg: &[u8]) -> Json {
    let Some(session_id) = json::param(msg, "session_id") else {
        return require("session_id");
    };
    // An `int` session id, widened as `SessionId`.
    let session_id = i64::from(atoi(&session_id)).cast_unsigned();
    if f.get(SESSION, session_id) == 0 {
        return answer(4004, Json::Str("connection not exists".into()));
    }
    answer(0, connection_json(&f.record(CONNECTION, 0)))
}

fn all_ports<F: Facts>(f: &mut F) -> Json {
    let list = (0..f.get(PORTS, 0))
        .map(|index| {
            let p: Port = f.record(PORT, index);
            Json::object([
                ("host", p.host.json()),
                ("port", i(p.port)),
                ("backlog", i(p.backlog)),
                ("type", i(p.socket_type)),
                ("ssl", i(p.ssl)),
                ("protocols", p.protocols.json()),
                ("connection_num", i(p.connection_num)),
            ])
        })
        .collect();
    answer(0, Json::Array(list))
}

// ---------------------------------------------------------------------------------
// TCP_INFO

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
const TCP_INFO_SUPPORTED: bool = true;
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
const TCP_INFO_SUPPORTED: bool = false;

#[cfg(any(target_os = "linux", target_os = "android"))]
type TcpInfo = libc::tcp_info;
#[cfg(any(target_os = "linux", target_os = "android"))]
const TCP_INFO: c_int = libc::TCP_INFO;
#[cfg(target_vendor = "apple")]
type TcpInfo = libc::tcp_connection_info;
#[cfg(target_vendor = "apple")]
const TCP_INFO: c_int = libc::TCP_CONNECTION_INFO;

/// The fields upstream's `sw_socket_parse_tcp_info` reports, in its order.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn tcp_fields(t: &TcpInfo) -> [(&'static CStr, u64); 32] {
    let wscale = t.tcpi_snd_rcv_wscale;
    [
        (c"state", t.tcpi_state.into()),
        (c"ca_state", t.tcpi_ca_state.into()),
        (c"retransmits", t.tcpi_retransmits.into()),
        (c"probes", t.tcpi_probes.into()),
        (c"backoff", t.tcpi_backoff.into()),
        (c"options", t.tcpi_options.into()),
        // `tcpi_snd_wscale : 4, tcpi_rcv_wscale : 4`.
        (c"snd_wscale", (wscale & 0x0f).into()),
        (c"rcv_wscale", (wscale >> 4).into()),
        (c"rto", t.tcpi_rto.into()),
        (c"ato", t.tcpi_ato.into()),
        (c"snd_mss", t.tcpi_snd_mss.into()),
        (c"rcv_mss", t.tcpi_rcv_mss.into()),
        (c"unacked", t.tcpi_unacked.into()),
        (c"sacked", t.tcpi_sacked.into()),
        (c"lost", t.tcpi_lost.into()),
        (c"retrans", t.tcpi_retrans.into()),
        (c"fackets", t.tcpi_fackets.into()),
        (c"last_data_sent", t.tcpi_last_data_sent.into()),
        (c"last_ack_sent", t.tcpi_last_ack_sent.into()),
        (c"last_data_recv", t.tcpi_last_data_recv.into()),
        (c"last_ack_recv", t.tcpi_last_ack_recv.into()),
        (c"pmtu", t.tcpi_pmtu.into()),
        (c"rcv_ssthresh", t.tcpi_rcv_ssthresh.into()),
        (c"rtt", t.tcpi_rtt.into()),
        (c"rttvar", t.tcpi_rttvar.into()),
        (c"snd_ssthresh", t.tcpi_snd_ssthresh.into()),
        (c"snd_cwnd", t.tcpi_snd_cwnd.into()),
        (c"advmss", t.tcpi_advmss.into()),
        (c"reordering", t.tcpi_reordering.into()),
        (c"rcv_rtt", t.tcpi_rcv_rtt.into()),
        (c"rcv_space", t.tcpi_rcv_space.into()),
        (c"total_retrans", t.tcpi_total_retrans.into()),
    ]
}

#[cfg(target_vendor = "apple")]
fn tcp_fields(t: &TcpInfo) -> [(&'static CStr, u64); 14] {
    [
        (c"state", t.tcpi_state.into()),
        (c"snd_wscale", t.tcpi_snd_wscale.into()),
        (c"rcv_wscale", t.tcpi_rcv_wscale.into()),
        (c"options", t.tcpi_options.into()),
        (c"flags", t.tcpi_flags.into()),
        (c"rto", t.tcpi_rto.into()),
        (c"maxseg", t.tcpi_maxseg.into()),
        (c"snd_ssthresh", t.tcpi_snd_ssthresh.into()),
        (c"snd_cwnd", t.tcpi_snd_cwnd.into()),
        (c"snd_wnd", t.tcpi_snd_wnd.into()),
        (c"snd_sbbytes", t.tcpi_snd_sbbytes.into()),
        (c"rcv_wnd", t.tcpi_rcv_wnd.into()),
        (c"srtt", t.tcpi_srtt.into()),
        (c"rttvar", t.tcpi_rttvar.into()),
    ]
}

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

/// The fields of the `tcp_info` at `info`, in upstream's order.
///
/// # Safety
/// `info` points at a `tcp_info` (`tcp_connection_info` on Apple platforms).
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
unsafe fn tcp_fields_at(info: *const c_void) -> Vec<(&'static CStr, u64)> {
    // SAFETY: the caller passes a `tcp_info`.
    tcp_fields(unsafe { &*info.cast::<TcpInfo>() }).to_vec()
}

/// No fields where upstream's list is not ported.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
unsafe fn tcp_fields_at(_: *const c_void) -> Vec<(&'static CStr, u64)> {
    Vec::new()
}

/// Writes the fields of the `tcp_info` at `info` to `out` (room for `cap`), in
/// upstream's order; returns how many it wrote.
///
/// # Safety
/// `info` points at a `tcp_info` (`tcp_connection_info` on Apple platforms); `out`
/// has `cap` writable facts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_tcp_info(
    info: *const c_void,
    out: *mut Fact,
    cap: usize,
) -> usize {
    // SAFETY: the caller passes a `tcp_info`.
    let fields = unsafe { tcp_fields_at(info) };
    let n = fields.len().min(cap);
    for (index, (name, value)) in fields.into_iter().take(n).enumerate() {
        // SAFETY: `index < cap`, within `out`.
        let slot = unsafe { out.add(index) };
        let fact = Fact {
            name: name.as_ptr(),
            value,
        };
        // SAFETY: `slot` is one of the caller's writable facts.
        unsafe { slot.write(fact) };
    }
    n
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND, COMMANDS, CONNECTION, CONNECTION_OWNER, Command, Connection, Facts, HAS_REACTOR,
        MEMORY, PIPE_SOCKET, PIPE_WORKERS, SESSION, Socket, Text, atoi, run,
        swr_v1_server_admin_command,
    };
    use core::ffi::{CStr, c_int, c_void};

    #[derive(Default)]
    struct Script {
        log: Vec<(u32, u64)>,
        workers: u64,
        session: bool,
    }

    fn text(bytes: &'static [u8]) -> Text {
        Text {
            data: bytes.as_ptr().cast(),
            len: bytes.len(),
        }
    }

    impl Facts for Script {
        fn call(&mut self, op: u32, arg: u64, out: *mut c_void) -> u64 {
            self.log.push((op, arg));
            match op {
                PIPE_WORKERS => self.workers,
                PIPE_SOCKET => {
                    let socket = Socket {
                        fd: i64::try_from(arg).unwrap_or(0),
                        events: 1,
                        total_recv_bytes: 2,
                        ..Socket::default()
                    };
                    // SAFETY: the record PIPE_SOCKET fills.
                    unsafe { out.cast::<Socket>().write(socket) };
                    0
                }
                COMMANDS => 1,
                COMMAND => {
                    let command = Command {
                        name: text(b"get_all_ports"),
                        id: 9,
                        accepted_process_types: 2,
                    };
                    // SAFETY: the record COMMAND fills.
                    unsafe { out.cast::<Command>().write(command) };
                    0
                }
                SESSION => u64::from(self.session),
                CONNECTION => {
                    let connection = Connection {
                        session_id: 7,
                        reactor_id: 1,
                        fd: 12,
                        server_port: 9501,
                        port: 40000,
                        connect_time: 1_730_000_000.5,
                        server_address: text(b"127.0.0.1"),
                        address: text(b"127.0.0.2"),
                        ..Connection::default()
                    };
                    // SAFETY: the record CONNECTION fills.
                    unsafe { out.cast::<Connection>().write(connection) };
                    0
                }
                MEMORY => {
                    let facts = out.cast::<super::Fact>();
                    // SAFETY: MEMORY's buffer holds 16 facts.
                    unsafe {
                        facts.write(super::Fact {
                            name: c"server".as_ptr(),
                            value: 100,
                        });
                    }
                    // SAFETY: as above.
                    unsafe {
                        facts.add(1).write(super::Fact {
                            name: c"php_memory".as_ptr(),
                            value: 5,
                        });
                    }
                    2
                }
                _ => 0,
            }
        }
    }

    fn index(name: &str) -> u32 {
        (0..64)
            .find(|&i| {
                let mut types: c_int = 0;
                // SAFETY: `types` is writable; the result is a static name or null.
                let found = unsafe { swr_v1_server_admin_command(i, false, &raw mut types) };
                // SAFETY: a non-null name is a static C string.
                found.is_null() || unsafe { CStr::from_ptr(found) }.to_bytes() == name.as_bytes()
            })
            .unwrap_or(0)
    }

    #[test]
    fn registers_upstreams_commands_in_order() {
        let mut names = Vec::new();
        for i in 0.. {
            let mut types: c_int = 0;
            // SAFETY: `types` is writable.
            let name = unsafe { swr_v1_server_admin_command(i, true, &raw mut types) };
            if name.is_null() {
                break;
            }
            // SAFETY: a static C string.
            names.push((
                unsafe { CStr::from_ptr(name) }
                    .to_str()
                    .unwrap_or("")
                    .to_owned(),
                types,
            ));
        }
        assert_eq!(names.len(), 11, "get_thread_info is registered twice");
        assert_eq!(names[4], ("get_manager_info".into(), 32), "manager only");
        assert_eq!(
            names[9],
            ("get_connections".into(), 8 | 2),
            "event worker and master"
        );
        let mut types: c_int = 0;
        // SAFETY: `types` is writable.
        unsafe { swr_v1_server_admin_command(10, false, &raw mut types) };
        assert_eq!(types, 4, "reactor threads own connections in process mode");
        assert_ne!(types, CONNECTION_OWNER, "resolved");
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
            .filter(|(op, _)| *op == PIPE_SOCKET)
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
        assert!(f.log.contains(&(HAS_REACTOR, 0)), "asked for the reactor");
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
        assert!(f.log.contains(&(SESSION, 7)), "session id parsed");
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
