//! Upstream's `core-tests/src/server/server.cpp` TESTs whose clients talk to a running
//! server (#2536), one Rust function per TEST with the same inputs and expected values,
//! over the crates' `Server` (crates/server).
//!
//! core-tests' shared counters (`test::counter_*`) and the process-shared `Mutex(true)` the
//! clients wait on live in anonymous shared memory, mapped before the server forks; the
//! mutex is a flag the worker sets (`unlock`) and the client takes (`lock`). An expectation
//! in a server callback can't unwind through the server, so it is recorded there too and
//! the case fails on it once the server stops, from whichever process it failed in.
//! `SyncClient` is a blocking `TcpStream`: `send` writes it all, `recv` is one read.

use super::case::{TimerNode, swoole_timer_after};
use super::*;
use crate::{TEST_LOG_FILE, TEST_STR};
use std::io::{Read, Write};
use std::net::TcpStream;

const PACKET: &[u8] = b"hello world\n";
const MODE_THREAD: c_int = 3;
const SW_SOCK_UDP: c_int = 2;
const SW_LOG_DEBUG: c_int = 0;
const SW_LOG_TRACE: c_int = 1;
const SW_SERVER_EVENT_RESUME_RECV: c_int = SW_SERVER_EVENT_PAUSE_RECV + 1;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const SW_ERROR_OUTPUT_SEND_YIELD: c_int = 1010;
const CB_PACKET: usize = 6;
const CB_PIPE_MESSAGE: usize = 15;
/// `SW_IPC_MAX_SIZE` (Linux): an `EventData`.
const SW_IPC_MAX_SIZE: usize = 8192;
const TEST_COUNTER_NUM: usize = 32;
// glibc's values.
const PROT_READ_WRITE: c_int = 3;
const MAP_SHARED_ANONYMOUS: c_int = 0x21;
const SOL_SOCKET: c_int = 1;
const SO_SNDBUF: c_int = 7;
const SO_RCVBUF: c_int = 8;
const SIGWINCH: c_int = 28;
const R_OK: c_int = 4;

/// `EventData`: its head and payload.
#[repr(C)]
struct EventData {
    info: DataHead,
    data: [u8; SW_IPC_MAX_SIZE - size_of::<DataHead>()],
}

impl EventData {
    fn new() -> Box<Self> {
        // SAFETY: all-zero is `EventData{}`.
        Box::new(unsafe { std::mem::zeroed() })
    }
}

unsafe extern "C-unwind" {}

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn usleep(usec: u32) -> c_int;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, length: u32)
    -> c_int;
    fn sendto(
        fd: c_int,
        buf: *const c_void,
        length: usize,
        flags: c_int,
        addr: *const c_void,
        addr_len: u32,
    ) -> isize;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn __libc_current_sigrtmin() -> c_int;
}

/// What the server's processes share with the case.
#[repr(C)]
struct Shared {
    counters: [AtomicI32; TEST_COUNTER_NUM],
    unlocked: AtomicBool,
    failed: AtomicBool,
}

static SHARED: OnceLock<&'static Shared> = OnceLock::new();

fn shared() -> &'static Shared {
    SHARED.get_or_init(|| {
        // SAFETY: a new anonymous mapping, zero-filled: every counter 0, locked, nothing failed.
        let at = unsafe {
            mmap(
                ptr::null_mut(),
                size_of::<Shared>(),
                PROT_READ_WRITE,
                MAP_SHARED_ANONYMOUS,
                -1,
                0,
            )
        };
        assert!(!at.is_null() && at.addr() != usize::MAX, "mmap failed");
        // SAFETY: the mapping holds a `Shared`, for the life of the process and its forks.
        unsafe { &*at.cast::<Shared>() }
    })
}

/// `test::counter_init()`.
fn counter_init() {
    for counter in &shared().counters {
        counter.store(0, Ordering::SeqCst);
    }
}

/// `test::counter_incr(index, add)`: the new value.
fn counter_incr(index: usize, add: c_int) -> c_int {
    shared().counters[index].fetch_add(add, Ordering::SeqCst) + add
}

fn counter_get(index: usize) -> c_int {
    shared().counters[index].load(Ordering::SeqCst)
}

/// `Mutex lock(true); lock.lock()`, before the server forks.
fn new_lock() {
    shared().unlocked.store(false, Ordering::SeqCst);
}

/// `lock.unlock()`, in a worker.
fn unlock() {
    shared().unlocked.store(true, Ordering::SeqCst);
}

/// `lock.lock()`, in the client: until a worker unlocks it.
fn lock() {
    while !shared().unlocked.swap(false, Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `EXPECT_*` in a server callback, in whichever process it runs.
fn check(held: bool, what: &str) {
    if !held {
        eprintln!("expectation failed: {what}");
        shared().failed.store(true, Ordering::SeqCst);
    }
}

/// After the server stops: no callback's expectation failed.
fn checks_held() {
    assert!(
        !shared().failed.load(Ordering::SeqCst),
        "a server-side expectation failed"
    );
}

/// `Server(mode)`, with the shared memory mapped first, so the server's forks share it.
/// The MODE_THREAD TESTs need upstream's `enable_thread` build (`SW_THREAD`); the crates
/// have thread mode when the extension's MINIT says so, which the case does here.
fn new_server(mode: c_int) -> Server {
    shared();
    if mode == MODE_THREAD {
        swoole_rs_server::THREAD_SUPPORTED.store(true, Ordering::Relaxed);
    }
    Server::new(mode)
}

fn micro_sleep(usec: u32) {
    // SAFETY: usleep(3).
    unsafe { usleep(usec) };
}

// The crates' functions these cases call by path: under LTO, a symbol that only an `extern`
// block names, and no C++ uses, can be dropped from the binary.
fn worker_id() -> u32 {
    swoole_rs_process::methods::swoole_get_worker_id()
}

fn set_log_file(file: &str) {
    let file = CString::new(file).unwrap();
    // SAFETY: a terminated path.
    unsafe { swoole_rs_utilities::swoole_set_log_file(file.as_ptr()) };
}

fn set_log_level(level: c_int) {
    // SAFETY: sets the global log level.
    unsafe { swoole_set_log_level(level) };
}

/// `String::append_random_bytes(length)`.
fn random_bytes(length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .unwrap();
    bytes
}

/// `Socket::set_buffer_size(size)`: the send and receive buffers.
fn set_buffer_size(fd: c_int, size: c_int) {
    for name in [SO_SNDBUF, SO_RCVBUF] {
        // SAFETY: an int option of a live socket.
        let status = unsafe {
            setsockopt(
                fd,
                SOL_SOCKET,
                name,
                (&raw const size).cast(),
                size_of::<c_int>() as u32,
            )
        };
        assert_eq!(status, 0, "setsockopt");
    }
}

/// One `recv`: its length, 0 at EOF, -1 on error.
fn recv(c: &mut TcpStream, buf: &mut [u8]) -> isize {
    c.read(buf).map_or(-1, |n| n as isize)
}

/// A `MSG_WAITALL` recv: until `buf` is full or the peer closes.
fn recv_all(c: &mut TcpStream, buf: &mut [u8]) -> usize {
    let mut length = 0;
    while length < buf.len() {
        match c.read(&mut buf[length..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => length += n,
        }
    }
    length
}

fn reply() -> Vec<u8> {
    [b"Server: ", PACKET].concat()
}

unsafe extern "C" fn run_timer(_: *mut c_void, node: *mut TimerNode) {
    // SAFETY: `timer_after`'s closure, which only this callback takes.
    let body = unsafe { Box::from_raw((*node).data.cast::<Box<dyn FnOnce()>>()) };
    body();
}

/// `swoole_timer_after(ms, [...](TIMER_PARAMS) {...})`.
fn timer_after(ms: c_long, body: impl FnOnce() + 'static) {
    let body: Box<Box<dyn FnOnce()>> = Box::new(Box::new(body));
    // SAFETY: a callback of the timer's signature, which takes the closure.
    unsafe { swoole_timer_after(ms, run_timer, Box::into_raw(body).cast()) };
}

/// `serv->shutdown()`.
fn shutdown(serv: Server) -> bool {
    server_methods::shutdown(serv.view())
}

/// `serv->close(session_id)`.
fn close(serv: Server, session: c_long) -> bool {
    server_methods::close(serv.view(), session, false)
}

/// `serv->send_pipe_message(worker_id, msg)`.
fn send_pipe_message(serv: Server, worker_id: u32, msg: &mut EventData) -> bool {
    // SAFETY: a live server and a live `EventData`, headed by its `DataHead`.
    unsafe { server_methods::send_pipe_message(serv.0, worker_id, (&raw mut msg.info).cast()) }
}

/// `Server::task_pack(msg, data, length)`.
fn pack(msg: &mut EventData, data: &[u8]) -> bool {
    // SAFETY: a writable `EventData`, headed by its `DataHead`; `data` readable for its length.
    unsafe {
        swoole_rs_process::task_file::task_pack(
            (&raw mut msg.info).cast(),
            data.as_ptr().cast(),
            data.len(),
        )
    }
}

/// What the case's `onStart` thread runs.
static ON_START: Mutex<Option<fn(Server)>> = Mutex::new(None);

/// `serv.onStart = [](Server *serv) { t1 = thread(...); }`: the client in a master thread,
/// which SIGTERMs the master if it fails; `join_client` joins it.
unsafe extern "C" fn start_client(_: *mut c_void, serv: *mut c_void) {
    let body = ON_START.lock().unwrap().expect("a client");
    let serv = Server(serv);
    client_thread(
        move || serv.get_master_pid(),
        move || {
            block_all_signals();
            body(serv);
        },
    );
}

fn set_start_client(serv: Server, body: fn(Server)) {
    *ON_START.lock().unwrap() = Some(body);
    serv.set_callback(CB_START, start_client as *const c_void);
}

/// A client thread started before `serv.start()`; it signals this process if it fails.
fn thread_client(body: impl FnOnce() + Send + 'static) {
    // SAFETY: getpid(2).
    client_thread(
        || unsafe { getpid() },
        move || {
            block_all_signals();
            body();
        },
    );
}

unsafe extern "C" fn unlock_on_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    unlock();
}

/// `onWorkerStart`: `lock.unlock(); test::counter_incr(0);`.
unsafe extern "C" fn unlock_and_count(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    unlock();
    counter_incr(0, 1);
}

unsafe extern "C" fn return_ok(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    SW_OK
}

/// `onReceive` of `sendwait` and `system`: the packet, answered with `sendwait`.
unsafe extern "C" fn sendwait_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (session, data) = request(req);
    check(data == PACKET, "string(req->data, req->info.len) == packet");
    let _ = server_methods::sendwait(Server(serv).view(), session, &reply());
    SW_OK
}

/// The client of `sendwait`, `system` and `forward_message`: one packet, then the reply
/// (upstream's one read, read until it is whole), then SIGTERM to this process.
fn packet_client(port: impl FnOnce() -> u16 + Send + 'static) {
    thread_client(move || {
        lock();
        let mut c = TcpStream::connect((TEST_HOST, port())).unwrap();
        c.write_all(PACKET).unwrap();
        let expected = reply();
        let mut buf = vec![0; expected.len()];
        let n = recv_all(&mut c, &mut buf);
        drop(c);
        // Upstream discards the reply; a client that got none must not pass.
        assert_eq!(&buf[..n], &expected[..]);
        kill_self();
    });
}

/// `TEST(server, sendwait)`.
fn sendwait() {
    let serv = new_server(MODE_BASE);
    serv.config().worker_num = 1;
    set_log_warning();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    new_lock();

    assert_eq!(serv.create(), SW_OK);

    packet_client(move || serv.get_primary_port().port());

    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, sendwait_receive as *const c_void);

    serv.start();
    join_client();
    checks_held();
    serv.delete();
}

/// `TEST(server, system)`. Its listening socket takes a port the kernel picks, where
/// upstream's took `get_random_port()`.
fn system() {
    use std::os::fd::IntoRawFd;

    let serv = new_server(MODE_BASE);
    serv.config().worker_num = 1;
    set_log_warning();

    new_lock();

    let listener = std::net::TcpListener::bind((TEST_HOST, 0)).unwrap();
    let svr_port = listener.local_addr().unwrap().port();
    let fd = listener.into_raw_fd();

    // SAFETY: no other thread reads the environment meanwhile.
    unsafe {
        std::env::set_var("LISTEN_FDS_START", fd.to_string());
        std::env::set_var("LISTEN_FDS", "1");
        std::env::set_var("LISTEN_PID", getpid().to_string());
    }

    assert!(server_methods::add_systemd_socket(serv.view()) > 0);
    assert_eq!(serv.create(), SW_OK);

    packet_client(move || svr_port);

    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, sendwait_receive as *const c_void);

    serv.start();
    join_client();
    checks_held();
    serv.delete();
}

/// `TEST(server, udp_packet)`'s `onPacket`: the datagram, echoed with `sendto`.
unsafe extern "C" fn udp_on_packet(_: *mut c_void, _: *mut c_void, req: *mut RecvData) -> c_int {
    // SAFETY: a datagram's RecvData points at its `DgramPacket`: `socket_type`, the
    // 120-byte `Address` (its length at 112), `length`, then the data.
    let (packet, fd) = unsafe { ((*req).data.cast::<u8>(), c_int::from((*req).info.server_fd)) };
    // SAFETY: as above.
    let (addr, addr_len, data) = unsafe {
        let length = packet.add(124).cast::<u32>().read_unaligned();
        (
            packet.add(4),
            packet.add(116).cast::<u32>().read_unaligned(),
            std::slice::from_raw_parts(packet.add(128), length as usize),
        )
    };
    check(
        data == PACKET,
        "string(recv_data->data, recv_data->length) == packet",
    );
    // `serv->get_server_socket(req->info.server_fd)->sendto(recv_data->socket_addr, ...)`:
    // the server socket is that descriptor.
    // SAFETY: the server's datagram socket; the peer's address and its length.
    unsafe {
        sendto(
            fd,
            PACKET.as_ptr().cast(),
            PACKET.len(),
            0,
            addr.cast(),
            addr_len,
        )
    };
    SW_OK
}

/// `TEST(server, udp_packet)`. The forked client's failure fails the case, where
/// upstream's went unseen.
fn udp_packet() {
    let server = new_server(MODE_PROCESS);
    server.config().worker_num = 2;
    server.add_port(SW_SOCK_UDP, TEST_HOST, 0);

    server.create();
    server.set_callback(CB_PACKET, udp_on_packet as *const c_void);
    server.set_callback(CB_RECEIVE, return_ok as *const c_void);

    let pid = fork_with(move || {
        let client = catch_unwind(AssertUnwindSafe(move || {
            sleep(1.0);
            let port = server.get_primary_port().port();

            let cli = std::net::UdpSocket::bind((TEST_HOST, 0)).unwrap();
            cli.connect((TEST_HOST, port)).unwrap();
            let ret = cli.send(PACKET).unwrap();
            assert!(ret > 0);

            let mut buf = [0; 1024];
            sleep(1.0);
            let n = cli.recv(&mut buf[..128]).unwrap();
            assert_eq!(&buf[..n.min(PACKET.len())], PACKET);
        }));
        kill_pid(server.get_master_pid());
        if client.is_err() {
            std::process::exit(1);
        }
    });

    server.start();
    let mut status = 0;
    // SAFETY: the forked client.
    unsafe { waitpid(pid, &mut status, 0) };
    assert_eq!(status, 0, "the client's assertions held");
    checks_held();
}

/// `TEST(server, pipe_message)`'s `onPipeMessage`.
unsafe extern "C" fn pipe_message_check(_: *mut c_void, _: *mut c_void, req: *mut EventData) {
    // SAFETY: the message the master forwarded.
    let req = unsafe { &*req };
    check(
        &req.data[..req.info.len as usize] == PACKET,
        "string(req->data, req->info.len) == packet",
    );
}

/// `TEST(server, pipe_message)`'s `onWorkerStart`: worker 1 writes to worker 0.
unsafe extern "C" fn pipe_message_worker_start(_: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    if worker_id() == 1 {
        let serv = Server(serv);
        let mut buf = EventData::new();
        check(pack(&mut buf, PACKET), "Server::task_pack(&buf, data)");
        check(
            send_pipe_message(serv, worker_id() - 1, &mut buf),
            "server->send_pipe_message(worker->id - 1, &buf)",
        );
        sleep(1.0);

        kill_pid(serv.get_master_pid());
    }
}

/// `TEST(server, pipe_message)`.
fn pipe_message() {
    let server = new_server(MODE_PROCESS);
    server.config().worker_num = 2;
    server.add_port(SW_SOCK_TCP, TEST_HOST, 0);

    server.create();
    server.set_callback(CB_PIPE_MESSAGE, pipe_message_check as *const c_void);
    server.set_callback(CB_RECEIVE, return_ok as *const c_void);
    server.set_callback(CB_WORKER_START, pipe_message_worker_start as *const c_void);

    server.start();
    checks_held();
}

/// `TEST(server, forward_message)`'s `onPipeMessage`: the session it names gets the reply.
unsafe extern "C" fn forward_pipe_message(_: *mut c_void, serv: *mut c_void, req: *mut EventData) {
    // SAFETY: the message holds the session id `onReceive` packed.
    let client_fd = unsafe { (*req).data.as_ptr().cast::<c_long>().read_unaligned() };
    Server(serv).send(client_fd, &reply());
}

/// `TEST(server, forward_message)`'s `onReceive`: the session, to the other worker.
unsafe extern "C" fn forward_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let mut msg = EventData::new();
    let (client_fd, _) = request(req);
    pack(&mut msg, &client_fd.to_ne_bytes());
    check(
        send_pipe_message(Server(serv), 1 - worker_id(), &mut msg),
        "serv->send_pipe_message(1 - swoole_get_worker_id(), &msg)",
    );
    SW_OK
}

/// `TEST(server, forward_message)`.
fn forward_message() {
    let serv = new_server(MODE_BASE);
    serv.config().worker_num = 2;

    set_log_warning();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    new_lock();

    assert_eq!(serv.create(), SW_OK);

    packet_client(move || serv.get_primary_port().port());

    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_PIPE_MESSAGE, forward_pipe_message as *const c_void);
    serv.set_callback(CB_RECEIVE, forward_receive as *const c_void);

    serv.start();
    join_client();
    checks_held();
    serv.delete();
}

/// `TEST(server, reopen_log)`'s `onWorkerStart`: worker 0 deletes the log, and SIGRTMIN
/// to the master brings it back.
unsafe extern "C" fn reopen_log_worker_start(_: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    if worker_id() != 0 {
        return;
    }
    let readable = || {
        let file = CString::new(TEST_LOG_FILE).unwrap();
        // SAFETY: a terminated path.
        unsafe { access(file.as_ptr(), R_OK) != -1 }
    };
    let master = Server(serv).get_master_pid();
    check(readable(), "access(filename, R_OK) != -1");
    micro_sleep(10000);
    let _ = std::fs::remove_file(TEST_LOG_FILE);
    check(!readable(), "access(filename, R_OK) == -1");
    // SAFETY: signals the master.
    unsafe { kill(master, __libc_current_sigrtmin()) };
    sleep(2.0);
    check(readable(), "access(filename, R_OK) != -1");
    kill_pid(master);
}

/// `TEST(server, reopen_log)`.
fn reopen_log() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 2;
    set_log_warning();
    set_log_file(TEST_LOG_FILE);

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_WORKER_START, reopen_log_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.start(), 0);
    checks_held();
    let _ = std::fs::remove_file(TEST_LOG_FILE);
    serv.delete();
}

/// `TEST(server, reopen_log2)`'s `onStart`. `signal_handler_reopen_logger` has no entry
/// of its own: `master_signal_handler(SIGWINCH)` runs its body over `SwooleG.server`.
unsafe extern "C" fn reopen_log2_start(_: *mut c_void, serv: *mut c_void) {
    let serv = Server(serv);
    timer_after(50, move || {
        // SAFETY: in the running master, whose server `SwooleG.server` is.
        unsafe { server_methods::master_signal_handler(SIGWINCH) };
        timer_after(50, move || {
            shutdown(serv);
        });
    });
}

unsafe extern "C" fn count_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    counter_incr(0, 1);
}

/// `TEST(server, reopen_log2)`.
fn reopen_log2() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 2;
    set_log_level(SW_LOG_DEBUG);
    set_log_file(TEST_LOG_FILE);

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_START, reopen_log2_start as *const c_void);
    serv.set_callback(CB_WORKER_START, count_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.start(), 0);
    let _ = std::fs::remove_file(TEST_LOG_FILE);
    serv.delete();
}

/// `test_conn_overflow`'s `send_yield`, which its `onReceive` captures.
static SEND_YIELD: AtomicBool = AtomicBool::new(false);

/// `test_conn_overflow`'s client, and that of `no_idle_worker`'s and `send_timeout`'s kind:
/// one packet, one read, then SIGTERM to the master.
fn overflow_client(serv: Server) {
    lock();
    let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();
    c.write_all(PACKET).unwrap();
    let mut buf = [0; 1024];
    recv(&mut c, &mut buf);
    drop(c);
    kill_pid(serv.get_master_pid());
}

unsafe extern "C" fn overflow_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    if worker_id() == 0 {
        unlock();
    }
    counter_incr(3, 1);
}

unsafe extern "C" fn overflow_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let (sid, _) = request(req);
    serv.get_connection_by_session_id(sid).overflow = 1;

    check(
        !serv.send(sid, TEST_STR.as_bytes()),
        "!serv->send(sid, TEST_STR)",
    );
    let expected = if SEND_YIELD.load(Ordering::SeqCst) {
        SW_ERROR_OUTPUT_SEND_YIELD
    } else {
        SW_ERROR_OUTPUT_BUFFER_OVERFLOW
    };
    check(
        last_error() == expected,
        "swoole_get_last_error() == expected",
    );

    shared().counters[0].store(1, Ordering::SeqCst);

    timer_after(100, move || {
        close(serv, sid);
    });

    SW_OK
}

/// `test_conn_overflow(mode, send_yield)`.
fn test_conn_overflow(mode: c_int, send_yield: bool) {
    let serv = new_server(mode);
    serv.config().worker_num = 1;
    serv.config().send_yield = send_yield;
    SEND_YIELD.store(send_yield, Ordering::SeqCst);
    set_log_warning();

    counter_init();

    new_lock();

    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }

    assert_eq!(serv.create(), SW_OK);

    set_start_client(serv, overflow_client);
    serv.set_callback(CB_WORKER_START, overflow_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, overflow_receive as *const c_void);

    assert_eq!(serv.start(), 0);

    join_client();
    assert_eq!(counter_get(0), 1);
    assert_eq!(counter_get(3), 1);
    checks_held();
    serv.delete();
}

/// `TEST(server, overflow_1)`.
fn overflow_1() {
    test_conn_overflow(MODE_BASE, false);
}

/// `TEST(server, overflow_2)`.
fn overflow_2() {
    test_conn_overflow(MODE_PROCESS, false);
}

/// `TEST(server, overflow_3)`.
fn overflow_3() {
    test_conn_overflow(MODE_BASE, true);
}

/// `TEST(server, overflow_4)`.
fn overflow_4() {
    test_conn_overflow(MODE_PROCESS, true);
}

/// `no_idle_worker`'s client: 1024 packets on one connection, then SIGTERM to the master.
fn no_idle_worker_client(serv: Server) {
    lock();
    let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();
    for _ in 0..1024 {
        let _ = c.write_all(PACKET);
    }
    sleep(3.0);
    drop(c);
    kill_pid(serv.get_master_pid());
}

/// `onReceive`: `usleep(10000); return SW_OK;`.
unsafe extern "C" fn slow_receive(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    micro_sleep(10000);
    SW_OK
}

/// `TEST(server, no_idle_worker)`.
fn no_idle_worker() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 4;
    serv.config().dispatch_mode = DISPATCH_IDLE_WORKER;

    set_log_file(TEST_LOG_FILE);
    set_log_warning();

    new_lock();

    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }

    assert_eq!(serv.create(), SW_OK);

    set_start_client(serv, no_idle_worker_client);
    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, slow_receive as *const c_void);

    assert_eq!(serv.start(), 0);

    join_client();

    let log = read_text(TEST_LOG_FILE).unwrap_or_default();
    assert!(log.contains("No idle worker is available"));

    let _ = std::fs::remove_file(TEST_LOG_FILE);
    serv.delete();
}

/// `test_max_request`'s client: 8 connections of up to 128 packets, then shutdown.
fn max_request_client(serv: Server) {
    lock();
    let port = serv.get_primary_port().port();

    let client_fn = || {
        let mut c = TcpStream::connect((TEST_HOST, port)).unwrap();
        for _ in 0..128 {
            if c.write_all(PACKET).is_err() {
                break;
            }
            micro_sleep(1000);
        }
        drop(c);
    };

    for _ in 0..8 {
        client_fn();
        micro_sleep(10000);
    }

    sleep(1.0);

    shutdown(serv);
}

/// `test_max_request(mode)`.
fn test_max_request(mode: c_int) {
    let serv = new_server(mode);
    serv.config().worker_num = 2;
    serv.config().max_request = 128;

    new_lock();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert_eq!(serv.create(), SW_OK);

    set_start_client(serv, max_request_client);
    serv.set_callback(CB_WORKER_START, unlock_and_count as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.start(), 0);

    join_client();

    assert!(counter_get(0) >= 8);
    serv.delete();
}

/// `TEST(server, max_request_1)`.
fn max_request_1() {
    test_max_request(MODE_PROCESS);
}

/// `TEST(server, max_request_2)`.
fn max_request_2() {
    test_max_request(MODE_THREAD);
}

/// `watermark`'s 2 MiB response.
static WBUF: OnceLock<Vec<u8>> = OnceLock::new();

/// `watermark`'s client: a 64 KiB receive buffer, and 2 MiB read 64 KiB at a time, slowly.
fn watermark_client(serv: Server) {
    lock();
    let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();
    set_buffer_size(std::os::fd::AsRawFd::as_raw_fd(&c), 65536);
    c.write_all(PACKET).unwrap();
    micro_sleep(1000);

    let size = 2 * 1024 * 1024;
    let mut rbuf = vec![0; size];
    let mut length = 0;
    while length < size {
        let end = (length + 65536).min(size);
        let rn = recv(&mut c, &mut rbuf[length..end]);
        micro_sleep(10000);
        if rn <= 0 {
            break;
        }
        length += rn as usize;
    }

    sleep(1.0);
    drop(c);
    shutdown(serv);
}

unsafe extern "C" fn watermark_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (session, _) = request(req);
    check(
        Server(serv).send(session, WBUF.get().unwrap()),
        "serv->send(req->session_id(), wbuf.str, wbuf.length)",
    );
    SW_OK
}

unsafe extern "C" fn count_buffer_empty(_: *mut c_void, _: *mut c_void, _: *mut DataHead) {
    counter_incr(1, 1);
}

unsafe extern "C" fn count_buffer_full(_: *mut c_void, _: *mut c_void, _: *mut DataHead) {
    counter_incr(2, 1);
}

/// `TEST(server, watermark)`.
fn watermark() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 2;

    new_lock();

    WBUF.set(random_bytes(2 * 1024 * 1024)).unwrap();

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    let port = port.expect("port != nullptr");
    assert_eq!(serv.create(), SW_OK);

    // SAFETY: `port->get_socket()`: the listening socket, whose first member is its fd.
    let fd = unsafe { *port.config().socket.cast::<c_int>() };
    set_buffer_size(fd, 65536);
    port.config().buffer_high_watermark = 1024 * 1024;
    port.config().buffer_low_watermark = 65536;

    set_start_client(serv, watermark_client);
    serv.set_callback(CB_WORKER_START, unlock_and_count as *const c_void);
    serv.set_callback(CB_RECEIVE, watermark_receive as *const c_void);
    serv.set_callback(CB_BUFFER_EMPTY, count_buffer_empty as *const c_void);
    serv.set_callback(CB_BUFFER_FULL, count_buffer_full as *const c_void);

    assert_eq!(serv.start(), 0);

    join_client();

    assert!(counter_get(1) >= 1);
    assert!(counter_get(2) >= 1);
    checks_held();
    serv.delete();
}

/// `discard_data`'s 8 KiB packet.
static RDATA: OnceLock<Vec<u8>> = OnceLock::new();

/// `discard_data`'s client: 128 packets on one connection, then SIGTERM to the master.
fn discard_data_client(serv: Server) {
    lock();
    let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();
    let rdata = RDATA.get().unwrap();
    for _ in 0..128 {
        let _ = c.write_all(rdata);
        micro_sleep(10);
    }

    sleep(1.0);

    kill_pid(serv.get_master_pid());
}

unsafe extern "C" fn discard_data_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    micro_sleep(10000);
    let (session, _) = request(req);
    close(Server(serv), session);
    SW_OK
}

/// `TEST(server, discard_data)`.
fn discard_data() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().dispatch_mode = DISPATCH_IDLE_WORKER;
    serv.config().discard_timeout_request = true;
    serv.config().disable_notify = true;

    set_log_file(TEST_LOG_FILE);
    set_log_warning();

    new_lock();

    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }

    assert_eq!(serv.create(), SW_OK);

    RDATA.set(random_bytes(8192)).unwrap();

    set_start_client(serv, discard_data_client);
    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, discard_data_receive as *const c_void);

    assert_eq!(serv.start(), 0);

    join_client();

    let log = read_text(TEST_LOG_FILE).unwrap_or_default();
    assert!(log.contains("discard_data() (ERRNO 1007)"));
    let _ = std::fs::remove_file(TEST_LOG_FILE);
    serv.delete();
}

/// `pause_and_resume`'s `onConnect`: receiving pauses, and resumes 50 ms later.
unsafe extern "C" fn pause_connect(_: *mut c_void, serv: *mut c_void, ev: *mut DataHead) {
    let serv = Server(serv);
    // SAFETY: the event the server passes.
    let session_id = unsafe { (*ev).fd };
    check(
        serv.feedback(
            serv.get_connection_by_session_id(session_id),
            SW_SERVER_EVENT_PAUSE_RECV,
        ),
        "serv->feedback(conn, SW_SERVER_EVENT_PAUSE_RECV)",
    );
    timer_after(50, move || {
        check(
            serv.feedback(
                serv.get_connection_by_session_id(session_id),
                SW_SERVER_EVENT_RESUME_RECV,
            ),
            "serv->feedback(conn, SW_SERVER_EVENT_RESUME_RECV)",
        );
    });
}

/// `onReceive`: the packet, answered with `Server: ` and the packet.
unsafe extern "C" fn reply_receive(_: *mut c_void, serv: *mut c_void, req: *mut RecvData) -> c_int {
    let (session, data) = request(req);
    check(data == PACKET, "string(req->data, req->info.len) == packet");
    Server(serv).send(session, &reply());
    SW_OK
}

/// `TEST(server, pause_and_resume)`.
fn pause_and_resume() {
    let serv = new_server(MODE_BASE);
    serv.config().worker_num = 2;

    set_log_level(SW_LOG_TRACE);

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    new_lock();

    assert_eq!(serv.create(), SW_OK);

    thread_client(move || {
        lock();

        micro_sleep(1000);

        let port = serv.get_primary_port().port();
        let mut c =
            TcpStream::connect((TEST_HOST, port)).expect("c.connect(TEST_HOST, port->port)");
        c.write_all(PACKET).unwrap();
        let mut buf = [0; 1024];
        // SAFETY: the clock.
        let t1 = unsafe { microtime() };
        assert_eq!(recv(&mut c, &mut buf), (PACKET.len() + 8) as isize);
        // SAFETY: as above.
        let t2 = unsafe { microtime() };
        assert!(t2 - t1 >= 0.048);
        let resp = reply();
        assert_eq!(&buf[..resp.len()], &resp[..]);
        drop(c);

        micro_sleep(1000);
        shutdown(serv);
    });

    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_CONNECT, pause_connect as *const c_void);
    serv.set_callback(CB_RECEIVE, reply_receive as *const c_void);

    serv.start();
    join_client();
    checks_held();
    serv.delete();
}

const BUFFER_SIZE: c_int = 65536;
const SEND_COUNT: c_int = 256;

unsafe extern "C" fn slow_connect(_: *mut c_void, _: *mut c_void, _: *mut DataHead) {
    micro_sleep(100_000);
}

unsafe extern "C" fn max_queued_bytes_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (session, data) = request(req);
    if counter_incr(0, data.len() as c_int) == SEND_COUNT * BUFFER_SIZE {
        Server(serv).send(session, TEST_STR.as_bytes());
    }
    SW_OK
}

/// `TEST(server, max_queued_bytes)`.
fn max_queued_bytes() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().max_queued_bytes = 65536;

    counter_init();
    set_log_level(SW_LOG_TRACE);

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    new_lock();

    assert_eq!(serv.create(), SW_OK);

    thread_client(move || {
        lock();

        micro_sleep(1000);

        let wbuf = random_bytes(BUFFER_SIZE as usize);

        let port = serv.get_primary_port().port();
        let mut c =
            TcpStream::connect((TEST_HOST, port)).expect("c.connect(TEST_HOST, port->port)");
        for _ in 0..SEND_COUNT {
            c.write_all(&wbuf).unwrap();
        }
        let mut buf = [0; 1024];
        assert_eq!(recv(&mut c, &mut buf), TEST_STR.len() as isize);
        drop(c);

        micro_sleep(1000);
        shutdown(serv);
    });

    serv.set_callback(CB_WORKER_START, unlock_on_worker_start as *const c_void);
    serv.set_callback(CB_CONNECT, slow_connect as *const c_void);
    serv.set_callback(CB_RECEIVE, max_queued_bytes_receive as *const c_void);

    serv.start();
    join_client();
    assert_eq!(SEND_COUNT * BUFFER_SIZE, counter_get(0));
    serv.delete();
}

pub const CASES: &[(&str, fn())] = &[
    ("sendwait", sendwait),
    ("system", system),
    ("reopen_log", reopen_log),
    ("reopen_log2", reopen_log2),
    ("udp_packet", udp_packet),
    ("pipe_message", pipe_message),
    ("forward_message", forward_message),
    ("no_idle_worker", no_idle_worker),
    ("overflow_1", overflow_1),
    ("overflow_2", overflow_2),
    ("overflow_3", overflow_3),
    ("overflow_4", overflow_4),
    ("max_request_1", max_request_1),
    ("max_request_2", max_request_2),
    ("watermark", watermark),
    ("discard_data", discard_data),
    ("pause_and_resume", pause_and_resume),
    ("max_queued_bytes", max_queued_bytes),
];
