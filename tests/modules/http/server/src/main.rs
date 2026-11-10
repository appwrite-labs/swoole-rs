//! Upstream's `core-tests/src/server/http_server.cpp` TESTs that start a server, one Rust
//! function per TEST with the same inputs and expected values, over the crates. http1/case.rs builds the Servers upstream's helpers built
//! (`test_base_server`, `test_http_server`, `test_ssl_http`); the callbacks, the
//! clients and every assertion are here. The WebSocket client is cpp-httplib's
//! (`core-tests/src/_lib/http.cpp`): the same upgrade request, and frames from the
//! crates' `websocket::encode`, `mask`, `get_package_length` and `decode`.
//!
//! protocol.rs (tests/modules/framing) adds `core-tests/src/protocol/base.cpp`'s server TESTs,
//! server.rs (tests/modules/protocols) `protocol/redis.cpp`'s and `server/mqtt.cpp`'s.
//!
//! `main --list` prints the cases; `main NAME` runs one, in this process, as
//! core-tests' main does (`swoole_init`, the case, `swoole_clean`), from the tree
//! root (core-tests' `get_root_path()`). It exits 0 when every assertion held.
#![expect(
    clippy::disallowed_types,
    reason = "the clients' headers and the Redis and MQTT servers' tables: test code, off the data-layout budget"
)]

#[path = "../../../framing/protocol.rs"]
mod protocol;
#[path = "../../../protocols/server.rs"]
mod protocols;

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_long, c_void};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
// The crates, once: the C++ in libswoole.a and the cases call them by symbol.
use swoole_rs_ffi as _;
use swoole_rs_ports::http_routes::exports as http;

mod client;
mod http1;

use http1::case::*;

type Server = c_void;
type Receive = extern "C" fn(*mut Server, *const c_void) -> c_int;
type Event = extern "C" fn(*mut Server, *const c_void);
type Hook = extern "C" fn(*mut Server);

const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
const MODE_BASE: c_int = 1;
const MODE_PROCESS: c_int = 2;
const DISPATCH_FDMOD: c_int = 2;
const STATUS_ACTIVE: u8 = 3;
const OPCODE_TEXT: u8 = 1;
const OPCODE_CLOSE: u8 = 8;
const OPCODE_PING: u8 = 9;
const OPCODE_PONG: u8 = 10;
const FLAG_FIN: u8 = 1;
const FLAG_MASK: u8 = 1 << 5;
const FLAG_ENCODE_HEADER_ONLY: u8 = 1 << 6;
const SW_WEBSOCKET_HEADER_LEN: usize = 2;
const SW_WEBSOCKET_MASK_LEN: usize = 4;
const SW_WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const USER_AGENT: &str = "cpp-httplib/0.7";
const TEST_PORT: c_int = 9501;
const TEST_STR: &str = "hello world, hello swoole\n";
const TEST_LOG_FILE: &str = "/tmp/swoole.log";
// glibc's values (the core-tests build is Linux).
const SIGTERM: c_int = 15;
const SIG_BLOCK: c_int = 0;
const RLIMIT_NOFILE: c_int = 7;

/// `swoole::String`.
#[repr(C)]
struct CppString {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut c_char,
    allocator: *const c_void,
}

/// `swoole::websocket::Frame`.
#[repr(C)]
struct Frame {
    header: [u8; 2],
    mask_key: [u8; 4],
    header_length: u16,
    payload_length: usize,
    payload: *mut c_char,
}

/// `swoole::PacketLength`.
#[repr(C)]
struct PacketLength {
    buf: *const u8,
    buf_size: u32,
    header_len: u32,
}

/// What `http2::init_settings` then `http2::pack_setting_frame(.., false)` write: a
/// SETTINGS frame header and the six default settings (header table 4096, push off,
/// unlimited streams, window 65535, frame 16384, unlimited header list).
const HTTP2_SETTING_FRAME: [u8; 45] = [
    0, 0, 36, 4, 0, 0, 0, 0, 0, //
    0, 1, 0, 0, 0x10, 0, //
    0, 2, 0, 0, 0, 0, //
    0, 3, 0xff, 0xff, 0xff, 0xff, //
    0, 4, 0, 0, 0xff, 0xff, //
    0, 5, 0, 0, 0x40, 0, //
    0, 6, 0xff, 0xff, 0xff, 0xff,
];

unsafe extern "C" {
    #[link_name = "_Z11swoole_initv"]
    fn swoole_init();
    #[link_name = "_Z12swoole_cleanv"]
    fn swoole_clean();
    #[link_name = "_ZN6swoole6StringC1EmPKNS_9AllocatorE"]
    fn string_new(this: *mut CppString, size: usize, allocator: *const c_void);
    #[link_name = "_ZN6swoole6String6repeatEPKcmm"]
    fn string_repeat(this: *mut CppString, data: *const c_char, length: usize, count: usize);
    #[link_name = "_ZN6swoole6String19append_random_bytesEmb"]
    fn string_append_random_bytes(this: *mut CppString, length: usize, base64: bool) -> bool;
    #[link_name = "_ZN6swoole9websocket6encodeEPNS_6StringEPKcmhh"]
    fn ws_encode(
        buffer: *mut CppString,
        data: *const c_char,
        length: usize,
        opcode: u8,
        flags: u8,
    ) -> bool;
    #[link_name = "_ZN6swoole9websocket6decodeEPNS0_5FrameEPcm"]
    fn ws_decode(frame: *mut Frame, data: *mut c_char, length: usize) -> bool;
    #[link_name = "_ZN6swoole9websocket4maskEPcmPKc"]
    fn ws_mask(data: *mut c_char, length: usize, mask_key: *const c_char);
    #[link_name = "_ZN6swoole9websocket18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE"]
    fn ws_get_package_length(
        protocol: *const c_void,
        socket: *mut c_void,
        pl: *mut PacketLength,
    ) -> isize;
}

/// `http_server::get_package_length`, over the cases' records.
///
/// # Safety
/// The port's protocol, a connection socket and a live packet length, laid out as
/// the crates' records.
unsafe fn http_get_package_length(
    protocol: *const http1::Protocol,
    socket: *mut c_void,
    pl: *mut PacketLength,
) -> isize {
    // SAFETY: per the caller.
    unsafe { http::get_package_length(protocol.cast(), socket.cast(), &mut *pl.cast()) }
}

/// `http_server::get_package_length_size`.
///
/// # Safety
/// `socket` is a connection socket.
unsafe fn http_get_package_length_size(socket: *mut c_void) -> u8 {
    // SAFETY: per the caller.
    unsafe { http::get_package_length_size(socket.cast()) }
}

// The handshake's `Sec-WebSocket-Accept`, as upstream's `http_context` computes it.
#[link(name = "crypto")]
unsafe extern "C" {
    fn SHA1(data: *const u8, length: usize, digest: *mut u8) -> *mut u8;
    fn EVP_EncodeBlock(out: *mut u8, data: *const u8, length: c_int) -> c_int;
}

unsafe extern "C" {
    fn fork() -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn getpid() -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn pipe(fds: *mut [c_int; 2]) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn sigfillset(set: *mut [u64; 16]) -> c_int;
    fn pthread_sigmask(how: c_int, set: *const [u64; 16], old: *mut [u64; 16]) -> c_int;
    fn setrlimit(resource: c_int, limit: *const [u64; 2]) -> c_int;
}

/// A Server pointer a client thread borrows; the Server outlives the thread.
struct Shared(*mut Server);
// SAFETY: the Server's methods the threads call are the ones upstream's threads call.
unsafe impl Send for Shared {}

fn c(text: &str) -> CString {
    CString::new(text).unwrap()
}

/// core-tests' `get_root_path()`: the runner starts each case in the tree root.
fn root() -> String {
    std::env::current_dir()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn ssl_dir(file: &str) -> CString {
    c(&format!("{}/tests/include/ssl_certs/{file}", root()))
}

/// `swoole_signal_block_all()`, in the calling thread.
fn signal_block_all() {
    let mut set = [0u64; 16];
    // SAFETY: a writable sigset_t, then a readable one.
    unsafe {
        sigfillset(&mut set);
        pthread_sigmask(SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// A DataHead's session, reactor id, length and websocket ext_flags.
fn info(head: *const c_void) -> (c_long, c_int, u32, u16) {
    let (mut reactor_id, mut len, mut ext_flags) = (0, 0, 0);
    // SAFETY: the DataHead the server passed the callback.
    let session_id = unsafe { swr_case_info(head, &mut reactor_id, &mut len, &mut ext_flags) };
    (session_id, reactor_id, len, ext_flags)
}

/// A received packet's session, data and websocket ext_flags.
fn request(req: *const c_void) -> (c_long, &'static [u8], u16) {
    let (session_id, _, len, ext_flags) = info(req);
    // SAFETY: the RecvData's `len` data bytes, live for the callback.
    let data = match len {
        0 => &[][..],
        _ => unsafe { std::slice::from_raw_parts(swr_case_data(req).cast::<u8>(), len as usize) },
    };
    (session_id, data, ext_flags)
}

fn send(serv: *mut Server, session_id: c_long, data: &[u8]) -> bool {
    // SAFETY: `data`'s bytes.
    unsafe { swr_case_send(serv, session_id, data.as_ptr().cast(), data.len() as u32) }
}

/// `onClose` of both helpers: a connection the server closed has no reactor.
extern "C" fn on_close(serv: *mut Server, head: *const c_void) {
    let (session_id, reactor_id, _, _) = info(head);
    // SAFETY: the connection being closed.
    if unsafe { swr_case_close_actively(serv, session_id) } {
        assert_eq!(reactor_id, -1);
    } else {
        assert!(reactor_id >= 0);
    }
}

/// The request upstream's llhttp callbacks collected: its URL and headers (the last of
/// a repeated name, names as sent), and whether llhttp paused for an upgrade.
struct Request {
    url: String,
    headers: HashMap<String, String>,
    upgrade: bool,
}

fn parse(data: &[u8]) -> Result<Request, String> {
    let end = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("HPE_INVALID_EOF_STATE")?;
    let head = std::str::from_utf8(&data[..end]).map_err(|_| "HPE_INVALID_HEADER_TOKEN")?;
    let mut lines = head.split("\r\n");
    let mut line = lines.next().unwrap_or_default().split(' ');
    let (Some(_method), Some(url), Some(version)) = (line.next(), line.next(), line.next()) else {
        return Err("HPE_INVALID_METHOD".into());
    };
    if !version.starts_with("HTTP/1.") {
        return Err("HPE_INVALID_VERSION".into());
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("HPE_INVALID_HEADER_TOKEN")?;
        headers.insert(name.to_owned(), value.trim().to_owned());
    }
    let named = |wanted: &str| {
        headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.to_ascii_lowercase())
    };
    let upgrade = named("upgrade").is_some()
        && named("connection").is_some_and(|value| value.contains("upgrade"));
    Ok(Request {
        url: url.to_owned(),
        headers,
        upgrade,
    })
}

/// `http_context::response`: the status line and headers, then the body.
fn respond(
    serv: *mut Server,
    session_id: c_long,
    status: &str,
    headers: &[(&str, String)],
    body: &str,
) {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head += &format!("{name}: {value}\r\n");
    }
    head += "\r\n";
    send(serv, session_id, head.as_bytes());
    if !body.is_empty() {
        send(serv, session_id, body.as_bytes());
    }
}

/// `http_context::createWebSocketAccept`.
fn websocket_accept(key: &str) -> String {
    let input = format!("{key}{SW_WEBSOCKET_GUID}");
    let (mut digest, mut encoded) = ([0u8; 20], [0u8; 29]);
    // SAFETY: readable input, a 20-byte digest, and 29 bytes for its base64 and NUL.
    let length = unsafe {
        SHA1(input.as_ptr(), input.len(), digest.as_mut_ptr());
        EVP_EncodeBlock(encoded.as_mut_ptr(), digest.as_ptr(), 20)
    };
    String::from_utf8_lossy(&encoded[..length as usize]).into_owned()
}

/// `sw_tg_buffer()`, cleared.
fn tg_buffer() -> *mut CppString {
    // SAFETY: the thread's buffer.
    let buffer = unsafe { swr_case_tg_buffer() };
    // SAFETY: as String::clear.
    unsafe { ((*buffer).length, (*buffer).offset) = (0, 0) };
    buffer
}

fn encode_and_send(serv: *mut Server, session_id: c_long, data: &[u8], opcode: u8) {
    let buffer = tg_buffer();
    // SAFETY: the live buffer; `data`'s bytes.
    unsafe {
        ws_encode(buffer, data.as_ptr().cast(), data.len(), opcode, FLAG_FIN);
        swr_case_send(serv, session_id, (*buffer).str, (*buffer).length as u32);
    }
}

static COUNTER: AtomicI32 = AtomicI32::new(0);

/// `test_base_server`'s `onReceive`: echo WebSocket frames, answer the upgrade, else
/// `hello world` (or the jpg for `/just/get/file`) to cpp-httplib.
extern "C" fn base_receive(serv: *mut Server, req: *const c_void) -> c_int {
    let (session_id, data, ext_flags) = request(req);
    COUNTER.fetch_add(1, Ordering::Relaxed);
    // SAFETY: the receiving connection's field.
    let status = unsafe { &mut *swr_case_websocket_status(serv, session_id) };
    if *status == STATUS_ACTIVE {
        let opcode = (ext_flags >> 8) as u8;
        if opcode == OPCODE_PING {
            encode_and_send(serv, session_id, data, OPCODE_PONG);
        } else if opcode != OPCODE_CLOSE {
            encode_and_send(serv, session_id, &[b"Swoole: ", data].concat(), OPCODE_TEXT);
        }
        return SW_OK;
    }
    let request = match parse(data) {
        Ok(request) => request,
        Err(reason) => {
            eprintln!("Parse error: {reason}");
            return SW_ERR;
        }
    };
    if request.upgrade {
        let key = request
            .headers
            .get("Sec-WebSocket-Key")
            .cloned()
            .unwrap_or_default();
        let headers = [
            ("Connection", "Upgrade".to_owned()),
            ("Sec-WebSocket-Accept", websocket_accept(&key)),
            ("Sec-WebSocket-Version", "13".to_owned()),
            ("Upgrade", "websocket".to_owned()),
            ("Content-Length", "0".to_owned()),
        ];
        respond(serv, session_id, "101 Switching Protocols", &headers, "");
        *status = STATUS_ACTIVE;
        if request.url == "/ws/close" {
            // SAFETY: the server's own session.
            unsafe { swr_case_close_after(serv, session_id, 200) };
        }
        return SW_OK;
    }
    if request.url == "/just/get/file" {
        let file = c(&format!("{}/examples/test.jpg", root()));
        // SAFETY: a NUL-terminated path.
        unsafe { swr_case_sendfile(serv, session_id, file.as_ptr()) };
    } else {
        respond(
            serv,
            session_id,
            "200 OK",
            &[("Content-Length", "11".to_owned())],
            "hello world",
        );
    }
    assert_eq!(
        request.headers.get("User-Agent").map(String::as_str),
        Some(USER_AGENT)
    );
    SW_OK
}

static BODY: Mutex<Option<fn(*mut Server)>> = Mutex::new(None);
static CHILD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

extern "C" fn base_worker_start(serv: *mut Server) {
    let body = BODY.lock().unwrap().expect("a case body");
    let serv = Shared(serv);
    let child = std::thread::spawn(move || {
        let serv = serv;
        // A failed assertion still stops the server, so the case ends instead of hanging.
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| body(serv.0))) {
            // SAFETY: SIGTERM to this process, as the case body would send.
            unsafe { kill(getpid(), SIGTERM) };
            resume_unwind(panic);
        }
    });
    *CHILD.lock().unwrap() = Some(child);
}

/// `test_base_server(fn)`: start the server and run `fn` in a thread once the worker starts.
fn test_base_server(body: fn(*mut Server)) {
    *BODY.lock().unwrap() = Some(body);
    COUNTER.store(0, Ordering::Relaxed);
    let root = c(&root());
    // SAFETY: a NUL-terminated root and callbacks of the slots' signatures.
    let serv =
        unsafe { swr_case_base_server(root.as_ptr(), base_receive, on_close, base_worker_start) };
    assert!(!serv.is_null(), "set_document_root");
    // SAFETY: the created server.
    unsafe { swr_case_start(serv) };
    let child = CHILD.lock().unwrap().take().expect("onWorkerStart ran");
    if let Err(panic) = child.join() {
        resume_unwind(panic);
    }
}

/// `test_http_server`'s `onReceive`: `hello world` to any request llhttp parses.
extern "C" fn http_receive(serv: *mut Server, req: *const c_void) -> c_int {
    let (session_id, data, _) = request(req);
    // SAFETY: the running server and the receiving connection.
    unsafe {
        assert!(swr_case_idle_worker_num(serv) <= swr_case_worker_num(serv));
        assert!(swr_case_healthy(serv, session_id));
    }
    match parse(data) {
        Ok(request) if !request.upgrade => {}
        Ok(_) => {
            eprintln!("Parse error: HPE_PAUSED_UPGRADE");
            return SW_ERR;
        }
        Err(reason) => {
            eprintln!("Parse error: {reason}");
            return SW_ERR;
        }
    }
    respond(
        serv,
        session_id,
        "200 OK",
        &[("Content-Length", "11".to_owned())],
        "hello world",
    );
    SW_OK
}

/// `test_http_server(dispatch_mode, ssl)`: two workers in process mode.
fn test_http_server(dispatch_mode: c_int, ssl: bool) -> *mut Server {
    let root = c(&root());
    // SAFETY: a NUL-terminated root and callbacks of the slots' signatures.
    unsafe {
        swr_case_http_server(
            dispatch_mode,
            ssl,
            2,
            MODE_PROCESS,
            root.as_ptr(),
            http_receive,
            on_close,
        )
    }
}

/// cpp-httplib's `Client::Upgrade("/websocket")`: true on `101`.
fn upgrade(cli: &mut TcpStream, port: c_int) -> bool {
    let request = format!(
        "GET /websocket HTTP/1.1\r\nAccept: */*\r\nConnection: Upgrade\r\nHost: 127.0.0.1:{port}\r\n\
         Sec-WebSocket-Version: 13\r\nSec-Websocket-Key: sN9cRrP/n9NdMgdcy2VJFQ==\r\nUpgrade: websocket\r\n\
         User-Agent: {USER_AGENT}\r\n\r\n"
    );
    if cli.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        if cli.read_exact(&mut byte).is_err() {
            return false;
        }
        head.push(byte[0]);
    }
    head.starts_with(b"HTTP/1.1 101 ")
}

/// cpp-httplib's `Client::Push`: a text frame's header from `encode`, then the
/// payload, masked with the header's key when `mask`.
fn push(cli: &mut TcpStream, data: &[u8], mask: bool) -> bool {
    let mut buf = [0 as c_char; 32];
    let mut buffer = CppString {
        length: 0,
        size: buf.len(),
        offset: 0,
        str: buf.as_mut_ptr(),
        allocator: std::ptr::null(),
    };
    let mut flags = FLAG_FIN | FLAG_ENCODE_HEADER_ONLY;
    if mask {
        flags |= FLAG_MASK;
    }
    // SAFETY: a 32-byte String over `buf`, enough for any frame header; `data`'s bytes.
    unsafe {
        ws_encode(
            &mut buffer,
            data.as_ptr().cast(),
            data.len(),
            OPCODE_TEXT,
            flags,
        )
    };
    let header: Vec<u8> = buf[..buffer.length].iter().map(|&b| b as u8).collect();
    if cli.write_all(&header).is_err() {
        return false;
    }
    if mask {
        let mut marked = data.to_vec();
        // SAFETY: `marked`'s bytes and the header's last four, its mask key.
        unsafe {
            ws_mask(
                marked.as_mut_ptr().cast(),
                marked.len(),
                header[header.len() - SW_WEBSOCKET_MASK_LEN..]
                    .as_ptr()
                    .cast(),
            )
        };
        cli.write_all(&marked).is_ok()
    } else {
        cli.write_all(data).is_ok()
    }
}

/// cpp-httplib's `Client::Recv`: one frame's payload.
fn recv(cli: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = [0u8; 1024];
    cli.read_exact(&mut buf[..SW_WEBSOCKET_HEADER_LEN]).ok()?;
    let mut pl = PacketLength {
        buf: buf.as_ptr(),
        buf_size: SW_WEBSOCKET_HEADER_LEN as u32,
        header_len: 0,
    };
    // SAFETY: `pl` covers `buf_size` bytes of `buf`.
    let mut packet_len =
        unsafe { ws_get_package_length(std::ptr::null(), std::ptr::null_mut(), &mut pl) };
    if packet_len < 0 {
        return None;
    }
    if packet_len == 0 {
        cli.read_exact(&mut buf[SW_WEBSOCKET_HEADER_LEN..pl.header_len as usize])
            .ok()?;
        pl.buf_size = pl.header_len;
        // SAFETY: as above, over the whole header.
        packet_len =
            unsafe { ws_get_package_length(std::ptr::null(), std::ptr::null_mut(), &mut pl) };
        if packet_len <= 0 {
            return None;
        }
    }
    let packet_len = packet_len as usize;
    let mut data = vec![0u8; packet_len + 1];
    let header_len = if pl.header_len > 0 {
        pl.header_len as usize
    } else {
        SW_WEBSOCKET_HEADER_LEN
    };
    data[..header_len].copy_from_slice(&buf[..header_len]);
    cli.read_exact(&mut data[header_len..packet_len]).ok()?;
    let mut frame = Frame {
        header: [0; 2],
        mask_key: [0; 4],
        header_length: 0,
        payload_length: 0,
        payload: std::ptr::null_mut(),
    };
    // SAFETY: the packet's bytes and a writable Frame.
    if !unsafe { ws_decode(&mut frame, data.as_mut_ptr().cast(), packet_len) } {
        return None;
    }
    // SAFETY: the decoded payload inside `data`.
    Some(
        unsafe { std::slice::from_raw_parts(frame.payload.cast::<u8>(), frame.payload_length) }
            .to_vec(),
    )
}

fn websocket_test(server_port: c_int, data: &[u8], mask: bool) {
    let mut cli = TcpStream::connect(("127.0.0.1", server_port as u16)).unwrap();
    assert!(upgrade(&mut cli, server_port));
    assert!(push(&mut cli, data, mask));
    let msg = recv(&mut cli).expect("a frame");
    assert_eq!(msg, [b"Swoole: ".as_slice(), data].concat());
}

/// `String str(size)`.
fn string(size: usize) -> CppString {
    let mut str = CppString {
        length: 0,
        size: 0,
        offset: 0,
        str: std::ptr::null_mut(),
        allocator: std::ptr::null(),
    };
    // SAFETY: constructs into `str`; the default allocator.
    unsafe { string_new(&mut str, size, std::ptr::null()) };
    str
}

fn value(str: &CppString) -> &[u8] {
    // SAFETY: the String's `length` bytes.
    unsafe { std::slice::from_raw_parts(str.str.cast::<u8>(), str.length) }
}

fn primary_port(serv: *mut Server) -> c_int {
    // SAFETY: the running server.
    unsafe { swr_case_port(serv) }
}

fn websocket_small() {
    test_base_server(|serv| {
        signal_block_all();
        websocket_test(primary_port(serv), b"hello world, swoole is best!", false);
        // SAFETY: SIGTERM to this process.
        unsafe { kill(getpid(), SIGTERM) };
    });
}

fn websocket_medium() {
    test_base_server(|serv| {
        signal_block_all();
        let mut str = string(8192);
        // SAFETY: one readable byte, repeated into the String.
        unsafe { string_repeat(&mut str, c"A".as_ptr(), 1, 8192) };
        websocket_test(primary_port(serv), value(&str), false);
        // SAFETY: SIGTERM to this process.
        unsafe { kill(getpid(), SIGTERM) };
    });
}

fn websocket_big() {
    test_base_server(|serv| {
        signal_block_all();
        let mut str = string(128 * 1024);
        let capacity = str.size;
        // SAFETY: as above.
        unsafe { string_repeat(&mut str, c"A".as_ptr(), 1, capacity - 1) };
        websocket_test(primary_port(serv), value(&str), false);
        // SAFETY: SIGTERM to this process.
        unsafe { kill(getpid(), SIGTERM) };
    });
}

fn websocket_mask() {
    test_base_server(|serv| {
        signal_block_all();
        let mut str = string(64 * 128);
        let capacity = str.size;
        // SAFETY: the live String.
        unsafe { string_append_random_bytes(&mut str, capacity, true) };
        websocket_test(primary_port(serv), value(&str), true);
        // SAFETY: SIGTERM to this process.
        unsafe { kill(getpid(), SIGTERM) };
    });
}

/// core-tests' `exec_js_script`: its exit status.
fn exec_js_script(file: &str, args: &str) -> Option<i32> {
    let script = format!("node {}/core-tests/js/{file} {args}", root());
    Command::new("bash")
        .arg("-c")
        .arg(script)
        .status()
        .ok()?
        .code()
}

fn node_websocket_client_1() {
    let _ = std::fs::remove_file(TEST_LOG_FILE);
    test_base_server(|serv| {
        signal_block_all();
        assert_eq!(
            exec_js_script("ws_1.js", &primary_port(serv).to_string()),
            Some(0)
        );
        // SAFETY: SIGTERM to the master.
        unsafe { kill(swr_case_master_pid(serv), SIGTERM) };
    });
    let str = std::fs::read_to_string(TEST_LOG_FILE).expect("the log file");
    assert!(str.contains("received: Swoole: hello world"));
    assert!(str.contains("the node websocket client is closed"));
    let _ = std::fs::remove_file(TEST_LOG_FILE);
}

fn node_websocket_client_2() {
    let _ = std::fs::remove_file(TEST_LOG_FILE);
    test_base_server(|serv| {
        signal_block_all();
        assert_eq!(
            exec_js_script("ws_2.js", &primary_port(serv).to_string()),
            Some(0)
        );
        // SAFETY: SIGTERM to the master.
        unsafe { kill(swr_case_master_pid(serv), SIGTERM) };
    });
    let str = std::fs::read_to_string(TEST_LOG_FILE).expect("the log file");
    assert!(str.contains("the node websocket client is closed, code: 1008, reason: swoole close"));
    let _ = std::fs::remove_file(TEST_LOG_FILE);
}

/// Starts `serv` in a child process, as the TESTs' `fork()`; dropping it SIGTERMs the
/// master (the TESTs' `ON_SCOPE_EXIT` or closing `kill`).
struct Forked(*mut Server, c_int);

impl Forked {
    fn start(serv: *mut Server) -> Self {
        // SAFETY: fork(2); the child only starts the server and exits.
        let pid = unsafe { fork() };
        if pid == 0 {
            // SAFETY: the created server, in its own process.
            unsafe { swr_case_start(serv) };
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_secs(1));
        Forked(serv, pid)
    }
}

impl Drop for Forked {
    fn drop(&mut self) {
        // SAFETY: the started server's master, then its forked parent.
        unsafe {
            kill(swr_case_master_pid(self.0), SIGTERM);
            waitpid(self.1, std::ptr::null_mut(), 0);
        }
    }
}

/// `swoole_shell_exec(command)`, waited for, then what it left in `log`.
fn curl_log(command: &str, log: &str) -> String {
    let _ = Command::new("/bin/sh").arg("-c").arg(command).status();
    String::from_utf8_lossy(&std::fs::read(log).expect("the curl log")).into_owned()
}

fn sni() {
    let server = test_http_server(DISPATCH_FDMOD, true);
    // SAFETY: the created server's primary port; NUL-terminated paths.
    unsafe {
        swr_case_ssl_files(
            server,
            ssl_dir("server.crt").as_ptr(),
            ssl_dir("server.key").as_ptr(),
        );
        let (cert, key) = (
            ssl_dir("sni_server_cs_cert.pem"),
            ssl_dir("sni_server_cs_key.pem"),
        );
        swr_case_ssl_sni(server, c"localhost".as_ptr(), cert.as_ptr(), key.as_ptr());
        swr_case_ssl_protocols(server, 0);
        swr_case_ssl_init(server);
    }
    let started = Forked::start(server);
    let port_num = primary_port(server);
    let response = curl_log(
        &format!("curl https://localhost:{port_num} -k -vvv --stderr /tmp/wwwsnitestcom.txt"),
        "/tmp/wwwsnitestcom.txt",
    );
    assert!(response.contains("CN=cs.php.net"));
    let response2 = curl_log(
        &format!("curl https://127.0.0.1:{port_num} -k -vvv --stderr /tmp/wwwsnitest2com.txt"),
        "/tmp/wwwsnitest2com.txt",
    );
    assert!(response2.contains("CN=127.0.0.1"));
    drop(started);
}

fn client_ca() {
    let server = test_http_server(DISPATCH_FDMOD, true);
    // SAFETY: as in sni.
    unsafe {
        swr_case_ssl_files(
            server,
            ssl_dir("server.crt").as_ptr(),
            ssl_dir("server.key").as_ptr(),
        );
        swr_case_ssl_client_ca(server, ssl_dir("ca.crt").as_ptr());
        swr_case_ssl_init(server);
    }
    let started = Forked::start(server);
    let port_num = primary_port(server);
    let ssl = format!("{}/tests/include/ssl_certs", root());
    let response = curl_log(
        &format!(
            "curl https://127.0.0.1:{port_num}  --cert {ssl}/client.crt --key {ssl}/client.key -k -vvv --stderr /tmp/client_ca.txt"
        ),
        "/tmp/client_ca.txt",
    );
    assert!(response.contains("200 OK"));
    drop(started);
}

static LOCK: Mutex<[c_int; 2]> = Mutex::new([-1, -1]);
static SSL_THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
/// `test_ssl_http`'s port: `__LINE__ + TEST_PORT` on its line.
const SSL_PORT: c_int = 1910 + TEST_PORT;

/// `onStart`: once the worker has started, curl the port and shut down.
extern "C" fn ssl_start(serv: *mut Server) {
    let serv = Shared(serv);
    let lock = LOCK.lock().unwrap()[0];
    let thread = std::thread::spawn(move || {
        let serv = serv;
        signal_block_all();
        let mut byte = 0u8;
        // SAFETY: the pipe's read end; one writable byte.
        unsafe { read(lock, (&raw mut byte).cast(), 1) };
        let cmd = format!("curl -k https://127.0.0.1:{SSL_PORT}/");
        let out = Command::new("/bin/sh").arg("-c").arg(cmd).output().unwrap();
        let buf = String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned();
        std::thread::sleep(Duration::from_millis(10));
        // SAFETY: the running server.
        unsafe { swr_case_shutdown(serv.0) };
        assert!(buf.contains(TEST_STR));
    });
    *SSL_THREAD.lock().unwrap() = Some(thread);
}

/// `onWorkerStart`: unlock.
extern "C" fn ssl_worker_start(_: *mut Server) {
    let lock = LOCK.lock().unwrap()[1];
    // SAFETY: the pipe's write end; one readable byte.
    unsafe { write(lock, c"x".as_ptr().cast(), 1) };
}

extern "C" fn ssl_receive(serv: *mut Server, req: *const c_void) -> c_int {
    let (fd, _, _) = request(req);
    let resp = format!(
        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{TEST_STR}",
        TEST_STR.len()
    );
    send(serv, fd, resp.as_bytes());
    // SAFETY: the receiving session.
    unsafe { swr_case_close(serv, fd) };
    SW_OK
}

/// `test_ssl_http(MODE_BASE)`.
fn ssl() {
    let mut fds = [-1; 2];
    // SAFETY: a writable pair; upstream's process-shared Mutex, locked.
    assert_eq!(unsafe { pipe(&mut fds) }, 0);
    *LOCK.lock().unwrap() = fds;
    let mut serv = std::ptr::null_mut();
    let (cert, key) = (ssl_dir("server.crt"), ssl_dir("server.key"));
    // SAFETY: NUL-terminated paths and callbacks of the slots' signatures.
    let created = unsafe {
        swr_case_ssl_server(
            &mut serv,
            MODE_BASE,
            SSL_PORT,
            cert.as_ptr(),
            key.as_ptr(),
            ssl_start,
            ssl_worker_start,
            ssl_receive,
        )
    };
    assert_eq!(created, SW_OK);
    // SAFETY: the created server.
    assert_eq!(unsafe { swr_case_start(serv) }, 0);
    let thread = SSL_THREAD.lock().unwrap().take().expect("onStart ran");
    if let Err(panic) = thread.join() {
        resume_unwind(panic);
    }
}

static ABORT_LOCK: Mutex<[c_int; 2]> = Mutex::new([-1, -1]);

/// abort_connection's `onStart`: unlock the client thread.
extern "C" fn abort_start(_: *mut Server) {
    let lock = ABORT_LOCK.lock().unwrap()[1];
    // SAFETY: the pipe's write end; one readable byte.
    unsafe { write(lock, c"x".as_ptr().cast(), 1) };
}

extern "C" fn abort_receive(_: *mut Server, _: *const c_void) -> c_int {
    SW_OK
}

/// With `SwooleG.max_sockets` 2 and one connection allowed, 16 blocking clients fill the
/// server and cpp-httplib's `Get` fails.
fn abort_connection() {
    let mut fds = [-1; 2];
    // SAFETY: a writable pair; upstream's Mutex, locked.
    assert_eq!(unsafe { pipe(&mut fds) }, 0);
    *ABORT_LOCK.lock().unwrap() = fds;
    let (mut max_sockets, mut ports) = (0, [std::ptr::null_mut(); 2]);
    // SAFETY: writable out-parameters and callbacks of the slots' signatures.
    let serv = unsafe {
        swr_case_abort_server(2, &mut max_sockets, &mut ports, abort_start, abort_receive)
    };
    assert!(!ports[0].is_null());
    assert!(!ports[1].is_null());
    let port = primary_port(serv) as u16;
    let shared = Shared(serv);
    let thread = std::thread::spawn(move || {
        let serv = shared;
        signal_block_all();
        let mut byte = 0u8;
        // SAFETY: the pipe's read end; one writable byte.
        unsafe { read(fds[0], (&raw mut byte).cast(), 1) };
        // SyncClient::connect(TEST_HOST, port, -1): a blocking connect, its result unused.
        let clients: Vec<_> = (0..16)
            .map(|_| TcpStream::connect(("127.0.0.1", port)))
            .collect();
        let resp = client::Client::new("127.0.0.1", port).get("/");
        drop(clients);
        // SAFETY: the running server.
        unsafe { swr_case_shutdown(serv.0) };
        assert!(resp.is_none());
    });
    // SAFETY: the created server.
    unsafe { swr_case_start(serv) };
    // SAFETY: restores the value the Server builder saved.
    unsafe { swr_case_set_max_sockets(max_sockets) };
    if let Err(panic) = thread.join() {
        resume_unwind(panic);
    }
}

/// `http_server::get_package_length(_size)` over a fake connection socket: a WebSocket
/// frame, an HTTP/2 settings frame, then an HTTP/1.1 connection, which is an error.
fn get_package_length() {
    const SW_HTTP2_FRAME_HEADER_SIZE: u8 = 9;
    const SW_WEBSOCKET_FRAME_HEADER_SIZE: u8 = 14;
    const STATUS_HANDSHAKE: u8 = 2;
    const SW_ERROR_PROTOCOL_ERROR: c_int = 508;
    let mut serv = std::ptr::null_mut();
    // SAFETY: a writable out-parameter.
    let port = unsafe { swr_case_length_port(&mut serv) };
    assert!(!port.is_null());
    // SAFETY: the live port; its Protocol lives with it.
    let protocol = unsafe { &*swr_case_init_protocol(port, false) };
    assert_eq!(protocol.package_length_size, SW_HTTP2_FRAME_HEADER_SIZE);
    // SAFETY: as above.
    let protocol = unsafe { &*swr_case_init_protocol(port, true) };
    assert_eq!(
        protocol.get_package_length_size,
        http::get_package_length_size as *mut c_void
    );

    let mut conn = std::ptr::null_mut();
    // SAFETY: the live port and a writable out-parameter.
    let fake_sock = unsafe { swr_case_fake_socket(port, &mut conn) };
    // SAFETY: the server, configured.
    assert_eq!(unsafe { swr_case_create(serv) }, SW_OK);

    // websocket
    let buffer = tg_buffer();
    // SAFETY: the fake socket's Connection.
    unsafe { swr_case_connection_status(conn, STATUS_HANDSHAKE, 0) };
    // SAFETY: the fake socket, whose object is its Connection.
    let size = unsafe { http_get_package_length_size(fake_sock) };
    assert_eq!(size, SW_WEBSOCKET_FRAME_HEADER_SIZE);
    // SAFETY: the live buffer; TEST_STR's bytes.
    assert!(unsafe {
        ws_encode(
            buffer,
            TEST_STR.as_ptr().cast(),
            TEST_STR.len(),
            OPCODE_TEXT,
            FLAG_FIN,
        )
    });
    // SAFETY: the live buffer.
    let mut pl = unsafe {
        PacketLength {
            buf: (*buffer).str.cast(),
            buf_size: (*buffer).length as u32,
            header_len: 0,
        }
    };
    // SAFETY: the port's protocol, the fake socket and the frame.
    let length = unsafe { http_get_package_length(protocol, fake_sock, &mut pl) };
    assert_eq!(length, (SW_WEBSOCKET_HEADER_LEN + TEST_STR.len()) as isize);

    // http2
    // SAFETY: the fake socket's Connection.
    unsafe { swr_case_connection_status(conn, 0, 1) };
    // SAFETY: as above.
    let size = unsafe { http_get_package_length_size(fake_sock) };
    assert_eq!(size, SW_HTTP2_FRAME_HEADER_SIZE);
    let length = HTTP2_SETTING_FRAME.len();
    pl.buf = HTTP2_SETTING_FRAME.as_ptr();
    pl.buf_size = length as u32;
    assert!(length > 16);
    // SAFETY: the port's protocol, the fake socket and the frame.
    let got = unsafe { http_get_package_length(protocol, fake_sock, &mut pl) };
    assert_eq!(got, length as isize);

    // http1.1
    // SAFETY: the fake socket's Connection.
    unsafe { swr_case_connection_status(conn, 0, 0) };
    // SAFETY: as above.
    assert_eq!(unsafe { http_get_package_length_size(fake_sock) }, 0);
    // SAFETY: as above.
    let got = unsafe { http_get_package_length(protocol, fake_sock, &mut pl) };
    assert_eq!(got, SW_ERR as isize);

    let mut msg = std::ptr::null();
    // SAFETY: a writable out-parameter.
    let error = unsafe { swr_case_last_error(&mut msg) };
    // SAFETY: the thread's NUL-terminated error message.
    let str = unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy();
    assert_eq!(error, SW_ERROR_PROTOCOL_ERROR);
    assert!(str.contains("unexpected protocol status of session"));
}

type Cases = &'static [(&'static str, fn())];

const CASES: Cases = &[
    ("abort_connection", abort_connection),
    ("get_package_length", get_package_length),
    ("websocket_small", websocket_small),
    ("websocket_medium", websocket_medium),
    ("websocket_big", websocket_big),
    ("websocket_mask", websocket_mask),
    ("node_websocket_client_1", node_websocket_client_1),
    ("node_websocket_client_2", node_websocket_client_2),
    ("sni", sni),
    ("client_ca", client_ca),
    ("ssl", ssl),
];

const SUITES: &[(&str, Cases)] = &[
    ("http_server", http1::CASES),
    ("http_server", CASES),
    ("protocol", protocol::CASES),
    ("redis", protocols::REDIS),
    ("mqtt", protocols::MQTT),
];

fn main() {
    let name = std::env::args().nth(1).expect("usage: main --list | NAME");
    let cases = SUITES.iter().flat_map(|(suite, cases)| {
        cases
            .iter()
            .map(move |(case, body)| (format!("{suite}.{case}"), body))
    });
    if name == "--list" {
        for (case, _) in cases {
            println!("{case}");
        }
        return;
    }
    let (_, body) = cases
        .into_iter()
        .find(|(known, _)| *known == name)
        .expect("unknown case");
    // core-tests' main sets SwooleG.max_sockets = 20000 after swoole_init; swoole_init
    // takes it from the soft descriptor limit.
    // SAFETY: setrlimit(2) with a readable limit.
    unsafe { setrlimit(RLIMIT_NOFILE, &[20000, 20000]) };
    // SAFETY: once per process, before any other Swoole call.
    unsafe { swoole_init() };
    body();
    // SAFETY: after the case, as core-tests' main.
    unsafe { swoole_clean() };
}
