//! The HTTP/1 TESTs that start a server (`get` .. `http_range2`, and `fail`), one Rust
//! function per TEST with the same inputs and expected values.
//!
//! The `Server` is a `SharedServer` (crates/server/src/server_object.rs) held in `SHARES`
//! until `delete` drops it. Its callback slots are set with `view::Server::set_callback`, and
//! its configuration, tables and static-handler sets are the Rust records
//! whose pointers head the object (crates/server/src/state.rs), as the
//! port's settings are a `ListenPort`, which the cases read and write through those
//! records' own types. The methods the
//! cases call are crates/server's `server_methods`. The fixtures read the request line
//! and headers where upstream's ran llhttp; the client is core-tests' httplib
//! (client.rs). An assertion in a server callback cannot unwind through the server's
//! frames, so it records the failure, which the case asserts on once the server stops.
//!
//! server.rs holds `core-tests/src/server/buffer.cpp`'s TEST, and server_config.rs and
//! server_conn.rs `server/server.cpp`'s connection and configuration TESTs, over the same
//! records.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;
use swoole_rs_corebase::log::last_error;
use swoole_rs_corebase::net_types::SocketAddress as Address;
use swoole_rs_corebase::process::task_file::Packed;
use swoole_rs_corebase::process::worker_record::Worker;
pub(super) use swoole_rs_corebase::utilities::swoole_set_log_level;
use swoole_rs_corebase::{DataHead, ServerEvent, microtime};
use swoole_rs_server::callbacks::{Callback, CallbackKind as Slot};
use swoole_rs_server::reactor_entry::RecvData;
use swoole_rs_server::{
    Connection, ServerConfig, ServerText, SharedServer, StaticCollection, server_methods, view,
};

use crate::client::{Client, USER_AGENT};

pub(super) mod case;
/// lifecycle.rs: `core-tests/src/server/server.cpp`'s reload, kill and restart TESTs.
pub(super) mod lifecycle;
/// schedule.rs: `core-tests/src/server/server.cpp`'s `schedule*` TESTs.
pub(super) mod schedule;
pub(super) mod server;
pub(super) mod server_config;
pub(super) mod server_conn;
pub(super) mod server_ssl;
pub(super) mod task;
/// timeline.rs: the worker-replacement cases' cross-process event log.
mod timeline;

const TEST_HOST: &str = "127.0.0.1";
const MODE_BASE: c_int = 1;
const MODE_PROCESS: c_int = 2;
const MODE_THREAD: c_int = 3;
const DISPATCH_FDMOD: u8 = 2;
const DISPATCH_IDLE_WORKER: u8 = 3;
const DISPATCH_RESULT_DISCARD_PACKET: c_int = -1;
const DISPATCH_RESULT_CLOSE_CONNECTION: c_int = -2;
const SW_SERVER_EVENT_PAUSE_RECV: ServerEvent = ServerEvent::PauseRecv;
const SW_SOCK_TCP: c_int = 1;
pub(super) const SW_LOG_WARNING: c_int = 4;
const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
const SW_ERROR_NAME_TOO_LONG: c_int = 504;
const SW_ERROR_DIR_NOT_EXIST: c_int = 703;
const PATH_MAX: usize = 4096;
const SIGTERM: c_int = 15;
const SW_HTTP_RFC1123_DATE_GMT: &CStr = c"%a, %d %b %Y %T GMT";
const SW_HTTP_RFC1123_DATE_UTC: &CStr = c"%a, %d %b %Y %T UTC";
const SW_HTTP_RFC850_DATE: &CStr = c"%A, %d-%b-%y %T GMT";
const SW_HTTP_ASCTIME_DATE: &CStr = c"%a %b %e %T %Y";

// Server's text members.
const TEXT_CHROOT: ServerText = ServerText::Chroot;
const TEXT_USER: ServerText = ServerText::User;
const TEXT_GROUP: ServerText = ServerText::Group;

// The static-handler sets.
const LOCATIONS: StaticCollection = StaticCollection::Locations;
const INDEX_FILES: StaticCollection = StaticCollection::IndexFiles;
const COMPRESSION_TYPES: StaticCollection = StaticCollection::CompressionTypes;

// The SWR_SERVER_NATIVE pointer members, opaque here.
type Native = *mut c_void;

/// `Protocol` (native/framing/swoole_protocol.h).
#[repr(C)]
pub(super) struct Protocol {
    split_by_eof: bool,
    package_eof: [u8; 8],
    package_eof_len: u8,
    package_length_type: u8,
    pub(super) package_length_size: u8,
    package_length_offset: u16,
    package_body_offset: u16,
    package_max_length: u32,
    private_data_1: Native,
    private_data_2: Native,
    on_package: Native,
    get_package_length: Native,
    pub(super) get_package_length_size: Native,
}

unsafe extern "C" {
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn getpid() -> c_int;
    fn time(out: *mut i64) -> i64;
    fn gmtime(time: *const i64) -> *const c_void;
    fn strftime(out: *mut c_char, size: usize, format: *const c_char, tm: *const c_void) -> usize;
}

unsafe extern "C" {
    fn fork() -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

/// A `Server`. Upstream's cases leak the ones they `new`; `delete` ends the others.
#[derive(Clone, Copy)]
struct Server(*mut c_void);

// SAFETY: the cases' client threads use the server as upstream's do: the fields they
// set are plain records, and the methods they call are the ones upstream calls there.
unsafe impl Send for Server {}
// SAFETY: as above.
unsafe impl Sync for Server {}

/// A `ListenPort`.
#[derive(Clone, Copy)]
struct Port(*mut c_void);

impl Port {
    /// The port's settings.
    fn config(self) -> &'static swoole_rs_server::port_config::PortConfig {
        // SAFETY: a ListenPort is its live settings.
        unsafe { &*self.0.cast::<swoole_rs_server::port_config::PortConfig>() }
    }

    fn port(self) -> u16 {
        u16::try_from(self.config().port.get()).unwrap()
    }

    /// `get_fd()`.
    fn get_fd(self) -> c_int {
        view::Port::from(self.config()).fd()
    }

    fn set_package_max_length(self, max_length: u32) {
        self.config()
            .protocol
            .update(|protocol| protocol.package_max_length = max_length);
    }

    /// The crates' view of this port's settings.
    pub(super) fn settings(self) -> &'static mut swoole_rs_server::port_config::PortConfig {
        // SAFETY: a live port, whose settings the cases set from one thread.
        unsafe { &mut *self.0.cast::<swoole_rs_server::port_config::PortConfig>() }
    }
}

impl Server {
    fn new(mode: c_int) -> Self {
        let share = SharedServer::new(mode);
        let serv = Self(share.view().raw());
        SHARES.lock().unwrap().push(share);
        serv
    }

    /// `delete serv`, or the end of a stack `Server`'s scope.
    fn delete(self) {
        let share = {
            let mut shares = SHARES.lock().unwrap();
            let at = shares.iter().position(|share| share.view().raw() == self.0);
            at.map(|at| shares.swap_remove(at))
        };
        drop(share);
    }

    fn config(self) -> &'static ServerConfig {
        self.view().config()
    }

    /// `serv.onX = fn`.
    fn set_callback(self, callback: Callback) {
        self.view().set_callback(callback);
    }

    /// `serv.onX == nullptr`.
    fn callback_is_empty(self, slot: Slot) -> bool {
        !self.view().has_callback(slot)
    }

    fn insert(self, collection: StaticCollection, value: &str) {
        self.view()
            .insert_static_string(collection, value.as_bytes());
    }

    /// `serv.user_ = value` and the other text members.
    fn set_text(self, kind: ServerText, value: &str) {
        self.view().set_text(kind, value.as_bytes());
    }

    /// `set_document_root(path)`.
    fn set_document_root(self, path: &[u8]) -> bool {
        server_methods::set_document_root(self.view(), path)
    }

    fn add_port(self, kind: c_int, host: &str, port: c_int) -> Option<Port> {
        let host = CString::new(host).unwrap();
        let port = server_methods::add_port(self.view(), kind, &host, port);
        port.map(|port| Port(port.raw()))
    }

    /// `ports.front()`.
    fn get_primary_port(self) -> Port {
        Port(self.view().port_at(0).unwrap().raw())
    }

    /// `gs->master_pid`.
    fn get_master_pid(self) -> c_int {
        self.view().counters().master_pid.load(Ordering::SeqCst)
    }

    fn create(self) -> c_int {
        if server_methods::create(self.view()) {
            SW_OK
        } else {
            SW_ERR
        }
    }

    fn start(self) -> c_int {
        if server_methods::start(self.view()) {
            SW_OK
        } else {
            SW_ERR
        }
    }

    fn send(self, session: c_long, data: &[u8]) -> bool {
        assert!(u32::try_from(data.len()).is_ok());
        server_methods::send(self.view(), session, data)
    }

    fn sendfile(self, session: c_long, file: &str) -> bool {
        server_methods::sendfile(self.view(), session, file.as_bytes(), 0, 0)
    }

    fn get_connection_by_session_id(self, session: c_long) -> Connection<'static> {
        self.view().by_session(session).expect("a live session")
    }

    fn get_idle_worker_num(self) -> u32 {
        server_methods::get_idle_worker_num(self.view())
    }

    fn is_healthy_connection(self, now: f64, conn: Connection<'_>) -> bool {
        self.view().healthy(now, conn)
    }

    fn feedback(self, conn: Connection<'_>, event: ServerEvent) -> bool {
        server_methods::feedback(self.view(), conn, event)
    }

    /// The crates' view of this server.
    fn view(self) -> view::Server<'static> {
        // SAFETY: the cases' Servers live until the case ends.
        unsafe { view::Server::from_raw(self.0) }
    }
}

/// `test::get_root_path()`: the directory the runner starts each case in, the tree's
/// root, read once as core-tests' main does (a worker's `chroot_` moves it to `/`).
fn root_path() -> String {
    static ROOT: OnceLock<String> = OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::current_dir()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    })
    .clone()
}

fn sleep(seconds: f64) {
    std::thread::sleep(Duration::from_secs_f64(seconds));
}

fn kill_pid(pid: c_int) {
    // SAFETY: signals that process.
    unsafe { kill(pid, SIGTERM) };
}

fn kill_self() {
    // SAFETY: this process.
    kill_pid(unsafe { getpid() });
}

fn block_all_signals() {
    _ = swoole_rs_corebase::signal_block_all();
}

fn set_log_warning() {
    swoole_set_log_level(SW_LOG_WARNING);
}

/// `fork()`: the child runs `child` and exits 0, as upstream's `if (pid == 0)` blocks do.
fn fork_with(child: impl FnOnce()) -> c_int {
    // SAFETY: the child runs only `child` before exiting.
    let pid = unsafe { fork() };
    assert!(pid >= 0);
    if pid == 0 {
        child();
        std::process::exit(0);
    }
    pid
}

/// `ON_SCOPE_EXIT { kill(server->get_master_pid(), SIGTERM); }`.
struct KillMaster(Server);

impl Drop for KillMaster {
    fn drop(&mut self) {
        kill_pid(self.0.get_master_pid());
    }
}

/// `swoole_shell_exec(command, &pid, 0)`: `/bin/sh -c command`, its stdout piped back.
fn shell_exec(command: &str) -> std::process::ChildStdout {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap()
        .stdout
        .unwrap()
}

/// One `read(pipe, buf, sizeof(buf) - 1)` into a 1024-byte buffer.
fn read_once(mut pipe: std::process::ChildStdout) -> Vec<u8> {
    let mut buf = [0u8; 1024];
    let n = std::io::Read::read(&mut pipe, &mut buf[..1023]).unwrap_or(0);
    buf[..n].to_vec()
}

fn read_text(path: &str) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// `test::counter_*`'s counter 0, and whether a server-side expectation failed.
/// The shares that keep each live `Server` alive until its `delete`.
static SHARES: Mutex<Vec<SharedServer>> = Mutex::new(Vec::new());

static COUNTER: AtomicI32 = AtomicI32::new(0);
static FAILED: AtomicBool = AtomicBool::new(false);

/// `EXPECT_*` in a server callback: report and record, then carry on as gtest does.
fn expect(held: bool, what: &str) {
    if !held {
        eprintln!("expectation failed: {what}");
        FAILED.store(true, Ordering::SeqCst);
    }
}

/// The request's URL and headers, where upstream's handler ran llhttp; `None` for a
/// request it could not parse.
fn parse_request(data: &[u8]) -> Option<(String, Vec<(String, String)>)> {
    let head_end = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&data[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let (_method, url, version) = (
        request_line.next()?,
        request_line.next()?,
        request_line.next()?,
    );
    if !version.starts_with("HTTP/1.") || request_line.next().is_some() {
        return None;
    }
    let headers = lines
        .map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.to_owned(), value.trim_start().to_owned()))
        })
        .collect::<Option<Vec<_>>>()?;
    Some((url.to_owned(), headers))
}

fn user_agent(headers: &[(String, String)]) -> &str {
    headers
        .iter()
        .find(|(k, _)| k == "User-Agent")
        .map_or("", |(_, v)| v.as_str())
}

/// `http_context::response(code, body)`: the status line and `Content-Length`, then the body.
fn respond(serv: Server, session: c_long, body: &str) {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
    serv.send(session, head.as_bytes());
    serv.send(session, body.as_bytes());
}

/// The request a callback receives, and its bytes.
fn request<'a>(req: &RecvData<'a>) -> (c_long, &'a [u8]) {
    (req.info().fd, req.payload())
}

/// `onClose` in both fixtures: an active close has no reactor.
fn on_close(serv: view::Server<'_>, info: &DataHead) {
    let conn = Server(serv.raw()).get_connection_by_session_id(info.fd);
    if conn.close_actively() {
        expect(info.reactor_id == -1, "info->reactor_id == -1");
    } else {
        expect(info.reactor_id >= 0, "info->reactor_id >= 0");
    }
}

/// `test_base_server`'s `onReceive`. The WebSocket frames its `websocket_*` TESTs send
/// are #1842's.
fn base_on_receive(serv: view::Server<'_>, req: &RecvData) -> c_int {
    let serv = Server(serv.raw());
    let (session, data) = request(req);
    COUNTER.fetch_add(1, Ordering::SeqCst);
    let Some((url, headers)) = parse_request(data) else {
        eprintln!("Parse error");
        return SW_ERR;
    };
    if url == "/just/get/file" {
        serv.sendfile(session, &(root_path() + "/examples/test.jpg"));
    } else {
        respond(serv, session, "hello world");
    }
    expect(
        user_agent(&headers) == USER_AGENT,
        "User-Agent == httplib::USER_AGENT",
    );
    SW_OK
}

static CLIENT: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static TEST: Mutex<Option<fn(Server)>> = Mutex::new(None);

/// Runs `body` in a thread that signals `pid` down when an assertion fails, so the
/// server stops and the failure is reported.
fn client_thread(pid: impl Fn() -> c_int + Send + 'static, body: impl FnOnce() + Send + 'static) {
    let handle = std::thread::spawn(move || {
        if let Err(panic) = catch_unwind(AssertUnwindSafe(body)) {
            kill_pid(pid());
            resume_unwind(panic);
        }
    });
    *CLIENT.lock().unwrap() = Some(handle);
}

/// Joins the client thread, then fails on its panic or a server-side expectation.
fn join_client() {
    let client = CLIENT.lock().unwrap().take().expect("the client started");
    if let Err(panic) = client.join() {
        resume_unwind(panic);
    }
    assert!(
        !FAILED.load(Ordering::SeqCst),
        "a server-side expectation failed"
    );
}

/// `test_base_server`'s `onWorkerStart`: the case's client runs in its own thread.
fn base_on_worker_start(serv: view::Server<'_>, _: Worker<'_>) {
    let test = TEST.lock().unwrap().expect("a case");
    let serv = Server(serv.raw());
    // SAFETY: getpid(2).
    client_thread(|| unsafe { getpid() }, move || test(serv));
}

/// `test_base_server(fn)`: a one-worker base-mode HTTP and WebSocket server with the
/// static handler on `/examples`, which runs `fn` in a thread once its worker starts.
fn test_base_server(test: fn(Server)) {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num.set(1);
    serv.config().enable_reuse_port.set(true);
    serv.config().heartbeat_check_interval.set(1);
    *TEST.lock().unwrap() = Some(test);

    COUNTER.store(0, Ordering::SeqCst);

    serv.config().enable_static_handler.set(true);
    assert!(serv.set_document_root(root_path().as_bytes()));

    serv.insert(LOCATIONS, "/examples");
    serv.insert(COMPRESSION_TYPES, "text/html");

    set_log_warning();

    let Some(port) = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0) else {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    };
    port.config().open_http_protocol.set(true);
    port.config().open_websocket_protocol.set(true);

    serv.create();

    serv.set_callback(Callback::WorkerStart(base_on_worker_start));
    serv.set_callback(Callback::Close(on_close));
    serv.set_callback(Callback::Receive(base_on_receive));

    serv.start();
    join_client();
    serv.delete();
}

fn ignore_server(_: view::Server<'_>) {}

fn ignore_event(_: view::Server<'_>, _: &DataHead) {}

/// `serv.onReceive = [](Server *, RecvData *) -> int { return SW_OK; }`.
fn receive_ok(_: view::Server<'_>, _: &RecvData) -> c_int {
    SW_OK
}

/// `serv.onTask = [](Server *, EventData *) -> int { return SW_OK; }`.
fn task_ok(_: view::Server<'_>, _: Packed<'_>) -> c_int {
    SW_OK
}

/// `test_http_server`'s `onReceive`.
fn http_on_receive(serv: view::Server<'_>, req: &RecvData) -> c_int {
    let serv = Server(serv.raw());
    let (session, data) = request(req);
    let conn = serv.get_connection_by_session_id(session);

    expect(
        serv.get_idle_worker_num() <= serv.config().worker_num.get(),
        "get_idle_worker_num() <= worker_num",
    );
    let now = microtime();
    expect(
        serv.is_healthy_connection(now, conn),
        "is_healthy_connection(microtime(), conn)",
    );

    let Some((url, _)) = parse_request(data) else {
        eprintln!("Parse error");
        return SW_ERR;
    };

    if url == "/overflow" {
        conn.set_overflow(true);
    }

    if url == "/pause" {
        serv.feedback(conn, SW_SERVER_EVENT_PAUSE_RECV);
    }

    respond(serv, session, "hello world");

    SW_OK
}

/// `test_http_server(dispatch_mode, false, worker_num, mode)`: an HTTP and WebSocket
/// server with the static handler on `/examples`; its workers run as root in `/`.
/// The TLS variant is #1842's.
fn test_http_server(dispatch_mode: u8, worker_num: u32, mode: c_int) -> Server {
    let server = Server::new(mode);
    server.set_text(TEXT_USER, "root");
    server.set_text(TEXT_GROUP, "root");
    server.set_text(TEXT_CHROOT, "/");
    server.config().worker_num.set(worker_num);
    server.config().dispatch_mode.set(dispatch_mode);
    server.config().open_cpu_affinity.set(true);
    set_log_warning();

    let port = server.add_port(SW_SOCK_TCP, TEST_HOST, 0).unwrap();

    port.config().open_http_protocol.set(true);
    port.config().open_websocket_protocol.set(true);
    port.config().open_tcp_keepalive.set(true);
    port.config().tcp_fastopen.set(1);
    port.config().tcp_defer_accept.set(1);

    server.config().enable_static_handler.set(true);
    server.set_document_root(root_path().as_bytes());
    server.insert(LOCATIONS, "/examples");

    server.create();

    server.set_callback(Callback::Start(ignore_server));
    server.set_callback(Callback::Close(on_close));
    server.set_callback(Callback::Connect(ignore_event));
    server.set_callback(Callback::Receive(http_on_receive));

    server
}

/// `test_http_server()`'s defaults.
fn default_http_server() -> Server {
    test_http_server(DISPATCH_FDMOD, 2, MODE_PROCESS)
}

/// `test_proxy_server`'s `onReceive`.
fn proxy_on_receive(serv: view::Server<'_>, req: &RecvData) -> c_int {
    let server = Server(serv.raw());
    let (session, data) = request(req);

    swoole_rs_corebase::process::methods::swoole_set_worker_id(server.config().worker_num.get());

    let Some((url, headers)) = parse_request(data) else {
        eprintln!("Parse error");
        return SW_ERR;
    };

    if url == "/just/get/file" {
        server.sendfile(session, &(root_path() + "/examples/test.jpg"));
    } else {
        respond(server, session, "hello world");
    }

    expect(
        user_agent(&headers) == USER_AGENT,
        "User-Agent == httplib::USER_AGENT",
    );
    SW_OK
}

/// `test_proxy_server()`: a one-worker base-mode server with the static handler on
/// `/examples` and a 64 KiB package limit.
fn test_proxy_server() -> Server {
    let server = Server::new(MODE_BASE);
    server.config().worker_num.set(1);

    let Some(port) = server.add_port(SW_SOCK_TCP, TEST_HOST, 0) else {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    };
    port.config().kernel_socket_send_buffer_size.set(c_int::MAX);
    port.config().kernel_socket_recv_buffer_size.set(c_int::MAX);
    port.config().open_tcp_nodelay.set(true);

    server.config().enable_static_handler.set(true);
    server.set_document_root(root_path().as_bytes());
    server.insert(LOCATIONS, "/examples");

    server.get_primary_port().set_package_max_length(64 * 1024);
    port.config().open_http_protocol.set(true);
    port.config().open_websocket_protocol.set(true);

    server.create();

    server.set_callback(Callback::Receive(proxy_on_receive));

    server
}

fn get() {
    test_base_server(|serv| {
        block_all_signals();

        let port = serv.get_primary_port();

        let mut cli = Client::new(TEST_HOST, port.port());
        let resp = cli.get("/index.html").unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello world");

        kill_self();
    });
}

fn reset_connection() {
    test_base_server(|serv| {
        block_all_signals();

        let port = serv.get_primary_port();
        // `Client c(SW_SOCK_TCP, false)`: a blocking TCP client.
        let mut c = std::net::TcpStream::connect((TEST_HOST, port.port())).unwrap();
        std::io::Write::write_all(&mut c, b"GET /index.html HTTP/1.1").unwrap();
        sleep(0.01);
        drop(c);

        kill_self();
    });

    assert_eq!(COUNTER.load(Ordering::SeqCst), 0);
}

fn heartbeat_check_interval() {
    test_base_server(|serv| {
        block_all_signals();

        let port = serv.get_primary_port();

        let mut cli = Client::new(TEST_HOST, port.port());
        cli.set_keep_alive(true);
        let resp = cli.get("/index.html").unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello world");
        sleep(3.0);

        kill_self();
    });
}

fn idle_time() {
    test_base_server(|serv| {
        block_all_signals();
        let port = serv.get_primary_port();
        port.config().max_idle_time.set(1.0);

        let mut cli = Client::new(TEST_HOST, port.port());
        cli.set_keep_alive(true);
        let resp = cli.get("/index.html").unwrap();
        assert_eq!(resp.status, 200);

        sleep(2.0);
        kill_self();
    });
}

fn post() {
    test_base_server(|serv| {
        block_all_signals();

        let port = serv.get_primary_port();

        let mut cli = Client::new(TEST_HOST, port.port());
        let resp = cli
            .post("/index.html", &[("name", "john"), ("note", "coder")])
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello world");

        kill_self();
    });
}

fn static_get() {
    test_base_server(|serv| {
        block_all_signals();

        let port = serv.get_primary_port();

        let mut cli = Client::new(TEST_HOST, port.port());
        let resp = cli.get("/examples/test.jpg").unwrap();
        assert_eq!(resp.status, 200);

        let file = root_path() + "/examples/test.jpg";
        let content = std::fs::read(file).expect("fp.ready()");

        assert_eq!(resp.body, content);

        let resp = cli.get("/just/get/file");
        assert!(resp.is_none());
        kill_self();
    });
}

fn static_files() {
    test_base_server(|serv| {
        serv.config().http_autoindex.set(true);
        serv.insert(LOCATIONS, "");

        block_all_signals();
        let port = serv.get_primary_port();
        let mut cli = Client::new(TEST_HOST, port.port());

        let resp = cli.get("/").unwrap();
        assert_eq!(resp.status, 200);
        assert!(find(&resp.body, b"Index of"));

        // directory not exists
        let resp = cli.get("/test/../").unwrap();
        assert_eq!(resp.status, 404);

        // must be document_root
        let resp = cli.get("//tests/../").unwrap();
        assert_eq!(resp.status, 404);

        let resp = cli.get("/tests/../README.md").unwrap();
        assert_eq!(resp.status, 200);

        // file not exists
        let resp = cli.get("/not-exists.jpg").unwrap();
        assert_eq!(resp.status, 404);

        // try again
        serv.insert(INDEX_FILES, "README.md");
        let resp = cli.get("/").unwrap();
        assert!(find(&resp.body, b"<h2 align=center>"));

        kill_self();
    });
}

fn find(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// `strftime(date_format, gmtime(&raw_time))`.
fn format_time(date_format: &CStr, raw_time: i64) -> String {
    let mut temp = [0u8; 128];
    // SAFETY: libc's calendar, into a 128-byte buffer.
    let written = unsafe {
        strftime(
            temp.as_mut_ptr().cast(),
            temp.len(),
            date_format.as_ptr(),
            gmtime(&raw_time),
        )
    };
    String::from_utf8(temp[..written].to_vec()).unwrap()
}

/// `request_with_header`: `If-Modified-Since` a week ahead in `date_format`; no response.
fn request_with_header(date_format: &CStr, cli: &mut Client) {
    // SAFETY: the clock.
    let raw_time = unsafe { time(std::ptr::null_mut()) } + 7 * 24 * 60 * 60;
    let temp = format_time(date_format, raw_time);
    let resp = cli.get_with("/", &[("If-Modified-Since", &temp)]);
    assert!(resp.is_none());
}

fn not_modify() {
    test_base_server(|serv| {
        serv.config().http_autoindex.set(true);
        serv.insert(LOCATIONS, "");

        block_all_signals();
        let port = serv.get_primary_port();
        let mut cli = Client::new(TEST_HOST, port.port());

        serv.insert(INDEX_FILES, "swoole-logo.svg");
        let resp = cli.get("/").unwrap();
        assert_eq!(resp.status, 200);

        // 304 not modified
        cli.set_read_timeout(0, 100);
        request_with_header(SW_HTTP_RFC1123_DATE_GMT, &mut cli);
        request_with_header(SW_HTTP_RFC1123_DATE_UTC, &mut cli);
        request_with_header(SW_HTTP_RFC850_DATE, &mut cli);
        request_with_header(SW_HTTP_ASCTIME_DATE, &mut cli);
        kill_self();
    });
}

fn proxy_file() {
    let server = test_proxy_server();
    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);

    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());

    let resp = cli.get("/just/get/file");
    assert!(resp.is_none());
}

// need fix
fn proxy_response() {
    let server = test_proxy_server();
    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);
    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    let resp = cli.get("/");
    assert!(resp.is_none());
    //        ASSERT_EQ(resp->body, string("hello world"));
}

fn heartbeat() {
    let server = default_http_server();
    server.config().heartbeat_check_interval.set(0);
    let port = server.get_primary_port();
    port.set_package_max_length(1024);
    port.config().heartbeat_idle_time.set(2);

    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);

    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    cli.set_keep_alive(true);
    let resp = cli.get("/").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");
    sleep(10.0);
    let resp = cli.get("/");
    assert!(resp.is_none());
}

fn overflow() {
    let server = default_http_server();

    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);

    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    cli.set_keep_alive(true);
    let resp = cli.get("/").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");
    let resp = cli.get("/overflow");
    assert!(resp.is_none());
}

fn process() {
    let server = default_http_server();
    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);

    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    cli.set_keep_alive(true);
    let resp = cli.get("/").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");

    let resp = cli.get("/").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");
}

fn process1() {
    let server = default_http_server();
    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);
    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    cli.set_keep_alive(true);
    let resp = cli.get("/index.html").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");

    sleep(1.0);
    let resp = cli.get("/examples/test.jpg").unwrap();
    assert_eq!(resp.status, 200);
}

fn return_zero(_: view::Server<'_>, _: &DataHead) {}

fn redundant_callback() {
    let server = test_http_server(DISPATCH_IDLE_WORKER, 2, MODE_PROCESS);
    for slot in [
        Slot::Connect,
        Slot::Close,
        Slot::BufferFull,
        Slot::BufferEmpty,
    ] {
        server.set_callback(match slot {
            Slot::Connect => Callback::Connect(return_zero),
            Slot::Close => Callback::Close(return_zero),
            Slot::BufferFull => Callback::BufferFull(return_zero),
            _ => Callback::BufferEmpty(return_zero),
        });
    }

    fork_with(|| {
        server.start();
        assert!(server.callback_is_empty(Slot::Connect));
        assert!(server.callback_is_empty(Slot::Close));
        assert!(server.callback_is_empty(Slot::BufferFull));
        assert!(server.callback_is_empty(Slot::BufferEmpty));
    });

    sleep(2.0);
    kill_pid(server.get_master_pid());
}

fn pause() {
    let server = default_http_server();
    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);

    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    cli.set_keep_alive(true);
    let resp = cli.get("/pause");
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");

    let resp = cli.get("/");
    assert!(resp.is_none());
}

fn bad_request() {
    let server = default_http_server();

    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);
    sleep(1.0);

    let port = server.get_primary_port().port();
    let command =
        format!("curl -X UNKNOWN http://{TEST_HOST}:{port} -k -vvv --stderr /tmp/bad_request.txt");

    let _pipe = shell_exec(&command);
    sleep(1.0);

    let response = read_text("/tmp/bad_request.txt");
    assert!(response.is_some());
    assert!(response.unwrap().contains("400 Bad Request"));
}

/// The chunked upload `chunked` and `max_queued_bytes` send with curl.
fn chunked_upload_command(port: u16) -> String {
    let jpg_path = root_path() + "/examples/test.jpg";
    format!(
        "curl -s -S -H 'Transfer-Encoding: chunked' -F \"file=@{jpg_path}\" http://{TEST_HOST}:{port}"
    )
}

static CHUNKED_SERVER: Mutex<Option<Server>> = Mutex::new(None);

fn chunked_on_start(_: view::Server<'_>) {
    let server = CHUNKED_SERVER.lock().unwrap().expect("the case's server");
    client_thread(
        move || server.get_master_pid(),
        move || {
            block_all_signals();
            sleep(0.3);

            let command = chunked_upload_command(server.get_primary_port().port());

            let pipe = shell_exec(&command);
            sleep(1.0);

            assert_eq!(read_once(pipe), b"hello world");
            kill_pid(server.get_master_pid());
        },
    );
}

fn chunked() {
    let server = test_http_server(DISPATCH_FDMOD, 1, MODE_BASE);
    *CHUNKED_SERVER.lock().unwrap() = Some(server);

    server.set_callback(Callback::Start(chunked_on_start));

    server.start();
    join_client();
    server.delete();
}

/// `test::wait_all_child_processes()`.
fn wait_all_child_processes() {
    let mut status = 0;
    // SAFETY: reaps this process's children.
    while unsafe { waitpid(-1, &mut status, 0) } > 0 {}
}

fn max_queued_bytes() {
    let server = default_http_server();
    server.config().max_queued_bytes.set(100);

    fork_with(|| {
        server.start();
    });

    {
        let _kill = KillMaster(server);

        sleep(1.0);

        let command = chunked_upload_command(server.get_primary_port().port());

        let pipe = shell_exec(&command);
        sleep(1.0);

        assert_eq!(read_once(pipe), b"hello world");
    }

    wait_all_child_processes();
}

fn dispatch_error(
    _: view::Server<'_>,
    _: Option<swoole_rs_server::Connection<'_>>,
    data: Option<&RecvData>,
) -> c_int {
    let fd = data.expect("the packet it dispatches").info().fd;
    if fd % 2 == 0 {
        DISPATCH_RESULT_DISCARD_PACKET
    } else {
        DISPATCH_RESULT_CLOSE_CONNECTION
    }
}

fn dispatch_func_return_error_worker_id() {
    let server = default_http_server();
    server.set_callback(Callback::DispatchFunc(dispatch_error));
    fork_with(|| {
        server.start();
    });

    let _kill = KillMaster(server);
    sleep(1.0);
    let port = server.get_primary_port();
    let mut cli = Client::new(TEST_HOST, port.port());
    cli.set_read_timeout(1, 0);
    let resp = cli.get("/");
    assert!(resp.is_none());
    let resp = cli.get("/");
    assert!(resp.is_none());
}

/// Whether curl's verbose log in /tmp/http_range.txt holds `expected`, after `command`.
fn curl_log_has(command: &str, expected: &[&str]) -> bool {
    drop(shell_exec(command));
    sleep(2.0);

    let Some(response) = read_text("/tmp/http_range.txt") else {
        return false;
    };
    expected.iter().all(|e| response.contains(e))
}

/// `request_with_if_range_header`: bytes 0-500 of docs/swoole-logo.svg if unchanged
/// since its mtime, in `date_format`.
fn request_with_if_range_header(date_format: &CStr, port: u16) -> bool {
    use std::os::unix::fs::MetadataExt;
    let file_path = root_path() + "/docs/swoole-logo.svg";
    let file_mtime = std::fs::metadata(file_path).map_or(0, |m| m.mtime());
    let temp = format_time(date_format, file_mtime);

    let command = format!(
        "curl http://{TEST_HOST}:{port}/docs/swoole-logo.svg -k -vvv --stderr /tmp/http_range.txt \
         -H 'Range: bytes=0-500' -H 'If-Range: {temp}'"
    );
    curl_log_has(&command, &["206 Partial Content", "Content-Length: 501"])
}

fn http_range() {
    let server = default_http_server();
    server.config().http_autoindex.set(true);
    server.insert(LOCATIONS, "/docs");

    fork_with(|| {
        server.start();
    });

    sleep(1.0);
    let _kill = KillMaster(server);

    let port = server.get_primary_port().port();
    assert!(request_with_if_range_header(SW_HTTP_RFC1123_DATE_GMT, port));
    assert!(request_with_if_range_header(SW_HTTP_RFC1123_DATE_UTC, port));
    assert!(request_with_if_range_header(SW_HTTP_RFC850_DATE, port));
    assert!(request_with_if_range_header(SW_HTTP_ASCTIME_DATE, port));
}

/// `request_with_diff_range`: docs/swoole-logo.svg's `range`.
fn request_with_diff_range(port: u16, range: &str) -> bool {
    let command = format!(
        "curl -X GET http://{TEST_HOST}:{port}/docs/swoole-logo.svg -k -vvv --stderr /tmp/http_range.txt \
         -H 'Range: bytes={range}'"
    );
    curl_log_has(&command, &["206 Partial Content"])
}

fn http_range2() {
    let server = default_http_server();
    server.insert(LOCATIONS, "/docs");
    server.insert(INDEX_FILES, "swoole-logo.svg");

    // test::spawn_exec.
    let pid = fork_with(|| {
        server.start();
    });

    let port = server.get_primary_port().port();
    assert!(request_with_diff_range(port, "0-15"));
    assert!(request_with_diff_range(port, "16-31"));
    assert!(request_with_diff_range(port, "-16"));
    assert!(request_with_diff_range(port, "128-"));
    assert!(request_with_diff_range(port, "0-0,-1"));

    sleep(0.1);
    kill_pid(pid);

    let mut status = 0;
    // SAFETY: reaps the server's process.
    assert_eq!(unsafe { waitpid(pid, &mut status, 0) }, pid);
}

fn fail() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num.set(1);

    let bad_path = vec![0u8; PATH_MAX + 4];
    assert!(!serv.set_document_root(&bad_path));
    assert_eq!(last_error(), SW_ERROR_NAME_TOO_LONG);

    assert!(!serv.set_document_root(b"/tmp/swoole-core-tests-not-exists"));
    assert_eq!(last_error(), SW_ERROR_DIR_NOT_EXIST);
    serv.delete();
}

/// `http_server.<name>`, in upstream's order.
pub(crate) const CASES: &[(&str, fn())] = &[
    ("get", get),
    ("reset_connection", reset_connection),
    ("heartbeat_check_interval", heartbeat_check_interval),
    ("idle_time", idle_time),
    ("post", post),
    ("static_get", static_get),
    ("static_files", static_files),
    ("not_modify", not_modify),
    ("proxy_file", proxy_file),
    ("proxy_response", proxy_response),
    ("heartbeat", heartbeat),
    ("overflow", overflow),
    ("process", process),
    ("process1", process1),
    ("redundant_callback", redundant_callback),
    ("pause", pause),
    ("bad_request", bad_request),
    ("chunked", chunked),
    ("max_queued_bytes", max_queued_bytes),
    (
        "dispatch_func_return_error_worker_id",
        dispatch_func_return_error_worker_id,
    ),
    ("http_range", http_range),
    ("http_range2", http_range2),
    ("fail", fail),
];
