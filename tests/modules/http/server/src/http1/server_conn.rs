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
use swoole_rs_process::task_file::TaskBuffer;

const PACKET: &[u8] = b"hello world\n";
const SW_SOCK_UDP: c_int = 2;
const SW_LOG_DEBUG: c_int = 0;
const SW_LOG_TRACE: c_int = 1;
const SW_SERVER_EVENT_RESUME_RECV: ServerEvent = ServerEvent::ResumeRecv;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const SW_ERROR_OUTPUT_SEND_YIELD: c_int = 1010;
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
fn new_server(mode: c_int) -> Server {
    shared();
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
    swoole_rs_utilities::set_log_file(&file);
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
fn send_pipe_message(serv: Server, worker_id: u32, msg: &mut TaskBuffer) -> bool {
    server_methods::send_pipe_message(serv.view(), worker_id, msg)
}

/// `Server::task_pack(msg, data, length)`.
fn pack(msg: &mut TaskBuffer, data: &[u8]) -> bool {
    msg.pack(data)
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
    serv.set_callback(Slot::Start, start_client as *const c_void);
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

    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, sendwait_receive as *const c_void);

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

    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, sendwait_receive as *const c_void);

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
    server.set_callback(Slot::Packet, udp_on_packet as *const c_void);
    server.set_callback(Slot::Receive, return_ok as *const c_void);

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
        let mut buf = TaskBuffer::default();
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
    server.set_callback(Slot::PipeMessage, pipe_message_check as *const c_void);
    server.set_callback(Slot::Receive, return_ok as *const c_void);
    server.set_callback(
        Slot::WorkerStart,
        pipe_message_worker_start as *const c_void,
    );

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
    let mut msg = TaskBuffer::default();
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

    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::PipeMessage, forward_pipe_message as *const c_void);
    serv.set_callback(Slot::Receive, forward_receive as *const c_void);

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

    serv.set_callback(Slot::WorkerStart, reopen_log_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, return_ok as *const c_void);

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
        // In the running master, whose server `SwooleG.server` is.
        server_methods::master_signal_handler(SIGWINCH);
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

    serv.set_callback(Slot::Start, reopen_log2_start as *const c_void);
    serv.set_callback(Slot::WorkerStart, count_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, return_ok as *const c_void);

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
    serv.set_callback(Slot::WorkerStart, overflow_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, overflow_receive as *const c_void);

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
/// The warning needs five reads within one 10 ms `onReceive`, but the reactor can take the
/// whole burst in a few reads (arm64 Docker always does), so where upstream sleeps 3 s this
/// sends a packet a millisecond until the master's once-a-second timer has logged it, for up
/// to 10 s.
fn no_idle_worker_client(serv: Server) {
    lock();
    let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();
    c.set_nodelay(true).unwrap();
    for _ in 0..1024 {
        let _ = c.write_all(PACKET);
    }
    for _ in 0..10_000 {
        if read_text(TEST_LOG_FILE).is_some_and(|log| log.contains("No idle worker is available")) {
            break;
        }
        let _ = c.write_all(PACKET);
        micro_sleep(1000);
    }
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
    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, slow_receive as *const c_void);

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
    serv.set_callback(Slot::WorkerStart, unlock_and_count as *const c_void);
    serv.set_callback(Slot::Receive, return_ok as *const c_void);

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
    serv.set_callback(Slot::WorkerStart, unlock_and_count as *const c_void);
    serv.set_callback(Slot::Receive, watermark_receive as *const c_void);
    serv.set_callback(Slot::BufferEmpty, count_buffer_empty as *const c_void);
    serv.set_callback(Slot::BufferFull, count_buffer_full as *const c_void);

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
    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, discard_data_receive as *const c_void);

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

    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::Connect, pause_connect as *const c_void);
    serv.set_callback(Slot::Receive, reply_receive as *const c_void);

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

    serv.set_callback(Slot::WorkerStart, unlock_on_worker_start as *const c_void);
    serv.set_callback(Slot::Connect, slow_connect as *const c_void);
    serv.set_callback(Slot::Receive, max_queued_bytes_receive as *const c_void);

    serv.start();
    join_client();
    assert_eq!(SEND_COUNT * BUFFER_SIZE, counter_get(0));
    serv.delete();
}

// The `Server::Command` process types.
const COMMAND_MASTER: c_int = 1 << 1;
const COMMAND_REACTOR_THREAD: c_int = 1 << 2;
const COMMAND_EVENT_WORKER: c_int = 1 << 3;
const COMMAND_TASK_WORKER: c_int = 1 << 4;
const COMMAND_MANAGER: c_int = 1 << 5;
const COMMAND_ALL_PROCESS: c_int = COMMAND_MASTER
    | COMMAND_REACTOR_THREAD
    | COMMAND_EVENT_WORKER
    | COMMAND_TASK_WORKER
    | COMMAND_MANAGER;
const SW_EVENT_DATA_CHUNK: u8 = 1 << 2;
const SW_EVENT_DATA_BEGIN: u8 = 1 << 3;
const SW_EVENT_DATA_OBJ_PTR: u8 = 1 << 5;
const SW_SESSION_LIST_SIZE: usize = 1 << 20;
/// `offsetof(swr_v1_socket, recv_timer)` (native/reactor/swoole_rs_socket.h asserts it).
const SOCKET_RECV_TIMER: usize = 152;
const SIGKILL: c_int = 9;
const EAGAIN: c_int = 11;

/// `Session` (crates/server/src/records.rs's `SessionRecord`).
#[repr(C)]
struct Session {
    id: c_long,
    fd: c_int,
    reactor_id: u32,
}

unsafe extern "C" {
    fn free(ptr: *mut c_void);
    fn __errno_location() -> *mut c_int;
}

/// `swoole_rand(min, max)`.
fn rand_between(min: u32, max: u32) -> u32 {
    let bytes = random_bytes(4);
    min + u32::from_ne_bytes(bytes.try_into().unwrap()) % (max - min + 1)
}

// `flags[...]`, the AsyncClient TESTs' `std::unordered_map<std::string, bool>`, as shared
// counters: the server's callbacks may run in a worker.
const CONNECTED: usize = 8;
const ON_CONNECT: usize = 9;
const ON_CLOSE: usize = 10;
const ON_ERROR: usize = 11;
const ON_RECEIVE: usize = 12;
const SERVER_ON_CONNECT: usize = 13;
const SERVER_ON_RECEIVE: usize = 14;
const SERVER_ON_CLOSE: usize = 15;
const RECV_COUNT: usize = 16;

fn set_flag(index: usize) {
    shared().counters[index].store(1, Ordering::SeqCst);
}

fn flag(index: usize) -> bool {
    counter_get(index) != 0
}

/// `AsyncClient ac(SW_SOCK_TCP)` and its callbacks, in a thread of the process whose
/// reactor upstream's ran in: `connected` once `connect` returns, then `on_connect` (which
/// `on_connect` follows) or `on_error`; `on_receive` for each read, which ends the client
/// when it returns false; `on_close` once the peer or the client closes.
fn async_client(
    serv: Server,
    on_connect: impl FnOnce(&mut TcpStream),
    mut on_receive: impl FnMut(&mut TcpStream, &[u8]) -> bool,
) {
    let port = serv.get_primary_port().port();
    let connect = TcpStream::connect((TEST_HOST, port));
    set_flag(CONNECTED);
    let Ok(mut c) = connect else {
        set_flag(ON_ERROR);
        return;
    };
    set_flag(ON_CONNECT);
    on_connect(&mut c);
    let mut buf = vec![0; 65536];
    loop {
        match c.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                set_flag(ON_RECEIVE);
                if !on_receive(&mut c, &buf[..n]) {
                    break;
                }
            }
            Err(_) => {
                set_flag(ON_ERROR);
                return;
            }
        }
    }
    drop(c);
    set_flag(ON_CLOSE);
}

/// `test_heartbeat_check`'s client: it sends nothing, the heartbeat closes it, and its
/// `on_close` shuts the server down.
fn heartbeat_client(serv: Server) {
    async_client(serv, |_| {}, |_, _| true);
    shutdown(serv);
}

/// `test_heartbeat_check(mode, single_thread)`.
fn test_heartbeat_check(mode: c_int, single_thread: bool) {
    let serv = new_server(mode);
    serv.config().worker_num = 1;
    serv.config().heartbeat_check_interval = 1;
    serv.config().single_thread = single_thread;

    swoole_rs_corebase::swoole_set_print_backtrace_on_error(true);

    counter_init();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(Slot::Receive, return_ok as *const c_void);
    set_start_client(serv, heartbeat_client);

    serv.start();
    join_client();

    assert!(flag(CONNECTED));
    assert!(flag(ON_CONNECT));
    assert!(!flag(ON_ERROR));
    assert!(!flag(ON_RECEIVE));
    assert!(flag(ON_CLOSE));
    serv.delete();
}

/// `TEST(server, heartbeat_check_1)`.
fn heartbeat_check_1() {
    test_heartbeat_check(MODE_BASE, false);
}

/// `TEST(server, heartbeat_check_2)`.
fn heartbeat_check_2() {
    test_heartbeat_check(MODE_PROCESS, false);
}

/// `TEST(server, heartbeat_check_3)`.
fn heartbeat_check_3() {
    test_heartbeat_check(MODE_THREAD, false);
}

/// `TEST(server, heartbeat_check_4)`.
fn heartbeat_check_4() {
    test_heartbeat_check(MODE_PROCESS, false);
}

/// `test_close`'s `close_in_client`, which its callbacks capture.
static CLOSE_IN_CLIENT: AtomicBool = AtomicBool::new(false);

fn close_in_client() -> bool {
    CLOSE_IN_CLIENT.load(Ordering::SeqCst)
}

unsafe extern "C" fn close_on_connect(_: *mut c_void, _: *mut c_void, _: *mut DataHead) {
    set_flag(SERVER_ON_CONNECT);
}

unsafe extern "C" fn close_on_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let (session, data) = request(req);
    serv.send(session, data);
    if !close_in_client() {
        close(serv, session);
    }
    set_flag(SERVER_ON_RECEIVE);
    SW_OK
}

unsafe extern "C" fn close_on_close(_: *mut c_void, _: *mut c_void, ev: *mut DataHead) {
    if !close_in_client() {
        // SAFETY: the event the server passes.
        check(unsafe { (*ev).reactor_id } < 0, "ev->reactor_id < 0");
    }
    set_flag(SERVER_ON_CLOSE);
}

unsafe extern "C" fn close_on_worker_stop(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    check(flag(SERVER_ON_CONNECT), "flags[\"server_on_connect\"]");
    check(flag(SERVER_ON_RECEIVE), "flags[\"server_on_receive\"]");
    check(flag(SERVER_ON_CLOSE), "flags[\"server_on_close\"]");
}

/// `test_close`'s client: TEST_STR once connected; on the echo it shuts down and closes
/// when the case closes in the client, else the server closes it; 50 ms after its close,
/// the server shuts down.
fn close_client(serv: Server) {
    async_client(
        serv,
        |c| {
            let _ = c.write_all(TEST_STR.as_bytes());
        },
        |c, _| {
            if close_in_client() {
                // A client of its own port in this process sees no close until it shuts down.
                let _ = c.shutdown(std::net::Shutdown::Both);
                return false;
            }
            true
        },
    );
    if flag(ON_CLOSE) {
        sleep(0.05);
    }
    shutdown(serv);
}

/// `test_close(mode, close_in_client, single_thread)`.
fn test_close(mode: c_int, close_in_client: bool, single_thread: bool) {
    let serv = new_server(mode);
    serv.config().worker_num = 1;
    serv.config().single_thread = single_thread;
    CLOSE_IN_CLIENT.store(close_in_client, Ordering::SeqCst);

    counter_init();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(Slot::Connect, close_on_connect as *const c_void);
    serv.set_callback(Slot::Receive, close_on_receive as *const c_void);
    serv.set_callback(Slot::Close, close_on_close as *const c_void);
    serv.set_callback(Slot::WorkerStop, close_on_worker_stop as *const c_void);
    set_start_client(serv, close_client);

    assert_eq!(serv.start(), SW_OK);
    join_client();

    assert!(flag(CONNECTED));
    assert!(flag(ON_CONNECT));
    assert!(!flag(ON_ERROR));
    assert!(flag(ON_RECEIVE));
    assert!(flag(ON_CLOSE));
    checks_held();
    serv.delete();
}

/// `TEST(server, close_1)`.
fn close_1() {
    test_close(MODE_PROCESS, false, false);
}

/// `TEST(server, close_2)`.
fn close_2() {
    test_close(MODE_BASE, false, false);
}

/// `TEST(server, close_3)`.
fn close_3() {
    test_close(MODE_THREAD, false, false);
}

/// `TEST(server, close_4)`.
fn close_4() {
    test_close(MODE_PROCESS, false, true);
}

/// `TEST(server, close_5)`.
fn close_5() {
    test_close(MODE_PROCESS, true, false);
}

/// `TEST(server, close_6)`.
fn close_6() {
    test_close(MODE_BASE, true, false);
}

/// `TEST(server, close_7)`.
fn close_7() {
    test_close(MODE_THREAD, true, false);
}

/// `TEST(server, close_8)`.
fn close_8() {
    test_close(MODE_PROCESS, true, true);
}

/// `eof_check`'s client, started in `onWorkerStart`: after each `OK`, one, two, then three
/// lines; after the fourth it closes, and its `on_close` shuts the server down.
fn eof_check_client(serv: Server) {
    let mut count = 0;
    async_client(
        serv,
        |_| {},
        |c, data| {
            assert_eq!(data, b"OK");
            count += 1;
            match count {
                1 => c.write_all(b"hello world\r\n").unwrap(),
                2 => c.write_all(b"hello world\r\nhello world\r\n").unwrap(),
                3 => c
                    .write_all(b"hello world\r\nhello world\r\nhello world\r\n")
                    .unwrap(),
                _ => return false,
            }
            true
        },
    );
    shutdown(serv);
}

unsafe extern "C" fn eof_check_worker_start(_: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    let serv = Server(serv);
    // SAFETY: getpid(2).
    client_thread(|| unsafe { getpid() }, move || eof_check_client(serv));
}

unsafe extern "C" fn eof_check_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (session, _) = request(req);
    Server(serv).send(session, b"OK");
    counter_incr(RECV_COUNT, 1);
    SW_OK
}

unsafe extern "C" fn eof_check_connect(_: *mut c_void, serv: *mut c_void, ev: *mut DataHead) {
    // SAFETY: the event the server passes.
    Server(serv).send(unsafe { (*ev).fd }, b"OK");
}

/// `TEST(server, eof_check)`.
fn eof_check() {
    let serv = new_server(MODE_BASE);
    serv.config().worker_num = 1;

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    let port = port.expect("port");
    // `port->is_stream()`: SW_SOCK_TCP, SW_SOCK_TCP6 or SW_SOCK_UNIX_STREAM.
    assert!(matches!(port.config().type_, 1 | 3 | 5));

    // `port->set_eof_protocol("\r\n", true)`.
    let config = port.config();
    config.open_eof_check = true;
    config.protocol.split_by_eof = false;
    config.protocol.package_eof_len = 2;
    config.protocol.package_eof[..2].copy_from_slice(b"\r\n");
    assert_eq!(serv.create(), SW_OK);

    counter_init();

    serv.set_callback(Slot::WorkerStart, eof_check_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, eof_check_receive as *const c_void);
    serv.set_callback(Slot::Connect, eof_check_connect as *const c_void);

    serv.start();
    join_client();

    assert!(flag(CONNECTED));
    assert!(flag(ON_CONNECT));
    assert!(!flag(ON_ERROR));
    assert!(flag(ON_RECEIVE));
    assert!(flag(ON_CLOSE));
    assert_eq!(counter_get(RECV_COUNT), 3);
    serv.delete();
}

/// `test_command`'s `add_command("test", ALL_PROCESS, ...)` handler.
fn command_handler(_: view::Server<'_>, msg: &[u8], reply: &mut Vec<u8>) {
    reply.extend_from_slice(b"json result, ");
    reply.extend_from_slice(msg);
}

/// `test_command`'s mode, for `serv->is_base_mode()`.
static COMMAND_BASE_MODE: AtomicBool = AtomicBool::new(false);

/// `serv->command(1, process_type, "test", msg, fn)`.
fn command(serv: Server, process_type: c_int, msg: &str) {
    let callback = std::sync::Arc::new(move |msg: &[u8]| command_callback(serv, msg));
    let sent = swoole_rs_server::swr_v1_server_command(
        serv.view(),
        1,
        process_type,
        b"test",
        msg.as_bytes(),
        callback,
    );
    check(sent, "serv->command(...)");
}

/// `test_command`'s `fn`: each reply sends the next command, then the server shuts down.
fn command_callback(serv: Server, msg: &[u8]) {
    micro_sleep(50000);
    match msg {
        b"json result, hello world [0]" if COMMAND_BASE_MODE.load(Ordering::SeqCst) => {
            command(serv, COMMAND_EVENT_WORKER, "hello world [2]");
        }
        b"json result, hello world [0]" => {
            command(serv, COMMAND_REACTOR_THREAD, "hello world [1]");
        }
        b"json result, hello world [1]" => command(serv, COMMAND_EVENT_WORKER, "hello world [2]"),
        b"json result, hello world [2]" => command(serv, COMMAND_TASK_WORKER, "hello world [3]"),
        b"json result, hello world [3]" => command(serv, COMMAND_MANAGER, "hello world [4]"),
        b"json result, hello world [4]" => timer_after(50, move || {
            shutdown(serv);
        }),
        _ => check(false, "ASSERT_TRUE(0)"),
    }
}

unsafe extern "C" fn command_start(_: *mut c_void, serv: *mut c_void) {
    command(Server(serv), COMMAND_MASTER, "hello world [0]");
}

unsafe extern "C" fn ignore_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {}

/// `test_command(mode)`.
fn test_command(mode: c_int) {
    let serv = new_server(mode);
    serv.config().worker_num = 4;
    serv.config().task_worker_num = 4;
    serv.config().reactor_num = 2;
    COMMAND_BASE_MODE.store(mode == MODE_BASE, Ordering::SeqCst);
    set_log_warning();

    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }

    assert_eq!(serv.create(), SW_OK);

    let handler = std::sync::Arc::new(command_handler);
    swoole_rs_server::swr_v1_server_add_command(serv.view(), b"test", COMMAND_ALL_PROCESS, handler);

    serv.set_callback(Slot::Start, command_start as *const c_void);
    serv.set_callback(Slot::WorkerStart, ignore_worker_start as *const c_void);
    serv.set_callback(Slot::Task, return_ok as *const c_void);
    serv.set_callback(Slot::Receive, reply_receive as *const c_void);

    assert_eq!(serv.start(), 0);
    checks_held();
    serv.delete();
}

/// `TEST(server, command_1)`.
fn command_1() {
    test_command(MODE_PROCESS);
}

/// `TEST(server, command_2)`.
fn command_2() {
    test_command(MODE_BASE);
}

/// `abnormal_pipeline_data`'s `msg_id`.
static MSG_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `abnormal_pipeline_data`'s `send_fn`: a chunk of 1000 to 8000 random bytes, straight
/// into worker 0's pipe.
fn abnormal_send(serv: Server, flags: u8, msg_id: u64) {
    // SAFETY: `server->get_worker_pipe_master(0)`: worker 0's pipe to the master, whose
    // first member is its fd.
    let fd = unsafe {
        *swoole_rs_process::worker_record::Worker::new(serv.view().worker(0))
            .pipe_master()
            .cast::<c_int>()
    };
    let len = rand_between(1000, 8000) as usize;
    let mut ev = EventData::new();
    ev.info.msg_id = msg_id;
    ev.info.flags = flags;
    ev.info.len = len as u32;
    ev.data[..len].copy_from_slice(&random_bytes(len));

    // `sock->send_sync(&ev, sizeof(ev.info) + len)`: one datagram, retried while the pipe is full.
    loop {
        // SAFETY: the head and `len` bytes of the event.
        let n = unsafe { crate::write(fd, (&raw const *ev).cast(), size_of::<DataHead>() + len) };
        // SAFETY: this thread's errno.
        if n >= 0 || unsafe { *__errno_location() } != EAGAIN {
            break;
        }
        micro_sleep(1000);
    }
}

unsafe extern "C" fn abnormal_worker_start(_: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    if worker_id() == 1 {
        let serv = Server(serv);
        let msg_id = MSG_ID.load(Ordering::SeqCst);
        abnormal_send(serv, SW_EVENT_DATA_CHUNK | SW_EVENT_DATA_BEGIN, msg_id);
        abnormal_send(serv, SW_EVENT_DATA_CHUNK, msg_id + 9999);

        micro_sleep(100_000);
        shutdown(serv);
    }
}

/// `TEST(server, abnormal_pipeline_data)`.
fn abnormal_pipeline_data() {
    let server = new_server(MODE_PROCESS);
    server.config().worker_num = 2;
    server.add_port(SW_SOCK_TCP, TEST_HOST, 0);

    let msg_id = u64::from(rand_between(1, i32::MAX as u32));
    MSG_ID.store(msg_id, Ordering::SeqCst);
    set_log_file(TEST_LOG_FILE);

    server.create();

    server.set_callback(Slot::Receive, return_ok as *const c_void);
    server.set_callback(Slot::WorkerStart, abnormal_worker_start as *const c_void);

    server.start();

    let cont = read_text(TEST_LOG_FILE).unwrap_or_default();
    assert!(cont.contains(&format!("abnormal pipeline data, msg_id={}", msg_id + 9999)));

    let _ = std::fs::remove_file(TEST_LOG_FILE);
}

unsafe extern "C" fn shutdown_after_manager_start(_: *mut c_void, serv: *mut c_void) {
    let serv = Server(serv);
    timer_after(50, move || {
        shutdown(serv);
    });
}

/// `TEST(server, base_event_worker_pool_cleanup)`. The pool's `map_` (pid to worker) is
/// its `registry` in crates/process.
fn base_event_worker_pool_cleanup() {
    let server = new_server(MODE_BASE);
    server.config().worker_num = 2;
    assert!(server.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert_eq!(server.create(), SW_OK);

    server.set_callback(Slot::Receive, return_ok as *const c_void);
    server.set_callback(
        Slot::ManagerStart,
        shutdown_after_manager_start as *const c_void,
    );

    assert_eq!(server.start(), SW_OK);
    // SAFETY: `server.get_event_worker_pool()`, in the server's shared memory.
    let pool = unsafe { &*server.view().event_pool_ref().raw() };
    assert!(!pool.has_pipes());
    assert!(pool.registry().is_null());
    assert!(pool.message_box().is_null());
    server.delete();
}

/// `abort_worker`'s `onConnect`: the worker holding session 1 kills itself 100 ms later.
unsafe extern "C" fn abort_worker_connect(_: *mut c_void, _: *mut c_void, ev: *mut DataHead) {
    // SAFETY: the event the server passes.
    if unsafe { (*ev).fd } == 1 {
        timer_after(100, || {
            // SAFETY: this process.
            unsafe { kill(getpid(), SIGKILL) };
        });
    }
}

/// `abort_worker`'s `onReceive`: only the live session is left in the session list.
unsafe extern "C" fn abort_worker_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    // SAFETY: the shared session list, SW_SESSION_LIST_SIZE of them.
    let sessions = unsafe {
        std::slice::from_raw_parts(
            (*serv.head().tables).session_list.cast::<Session>(),
            SW_SESSION_LIST_SIZE,
        )
    };
    let count = sessions.iter().filter(|s| s.fd != 0 && s.id != 0).count();
    check(count == 1, "count == 1");
    let (session, _) = request(req);
    serv.send(session, if count == 1 { b"OK" } else { b"ERR" });
    0
}

/// `abort_worker`'s `onWorkerStart`, which also counts the starts in counter 0.
unsafe extern "C" fn abort_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    counter_incr(0, 1);
    if worker_id() == 0 {
        unlock();
    }
}

/// `TEST(server, abort_worker)`. The `new` Server leaks, as upstream's.
///
/// c1 reads EOF as soon as the worker holding it dies, but the manager retires that worker's
/// sessions only once it has reaped it, so upstream's c2 can reach `onReceive` first (arm64
/// Docker's does every time). The manager retires them before it starts the replacement, so
/// c2 waits for that third worker start, for up to 10 s.
fn abort_worker() {
    let server = new_server(MODE_BASE);
    server.config().worker_num = 2;

    server.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert_eq!(server.create(), 0);

    new_lock();
    counter_init();

    thread_client(move || {
        lock();
        let port = server.get_primary_port().port();

        let mut c1 = TcpStream::connect((TEST_HOST, port)).unwrap();

        let mut buf = [0; 1024];
        let rn = recv_all(&mut c1, &mut buf);
        assert_eq!(rn, 0);

        drop(c1);
        for _ in 0..10_000 {
            if counter_get(0) > 2 {
                break;
            }
            micro_sleep(1000);
        }

        let mut c2 = TcpStream::connect((TEST_HOST, port)).unwrap();
        c2.write_all(b"info").unwrap();
        let n = recv(&mut c2, &mut buf);
        drop(c2);

        assert_eq!(&buf[..usize::try_from(n).unwrap_or(0)], b"OK");

        shutdown(server);
    });

    server.set_callback(Slot::Connect, abort_worker_connect as *const c_void);
    server.set_callback(Slot::Receive, abort_worker_receive as *const c_void);
    server.set_callback(Slot::WorkerStart, abort_worker_start as *const c_void);

    assert_eq!(server.start(), 0);
    join_client();
    checks_held();
}

/// `reactor_thread_pipe_writable`'s 4 MiB packet.
static RDATA_4M: OnceLock<Vec<u8>> = OnceLock::new();

unsafe extern "C" fn pipe_writable_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    unlock();
    micro_sleep(300_000);
}

unsafe extern "C" fn pipe_writable_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let rdata = RDATA_4M.get().unwrap();
    let len = (rdata.len() as u32).to_be_bytes();
    // SAFETY: the request the server passes.
    let flags = unsafe { (*req).info.flags };
    check(
        flags & SW_EVENT_DATA_OBJ_PTR != 0,
        "req->info.flags & SW_EVENT_DATA_OBJ_PTR",
    );
    let (session, data) = request(req);
    check(
        serv.send(session, &len),
        "serv->send(fd, &len, sizeof(len))",
    );
    check(
        serv.send(session, rdata),
        "serv->send(fd, rdata.str, rdata.length)",
    );
    check(data.get(4..) == Some(&rdata[..]), "req->data + 4 == rdata");

    // `serv->get_worker_message_bus()->move_packet()`: the packet's block, now ours to free.
    let data = serv.view().move_worker_packet();
    check(!data.is_null(), "data != nullptr");
    // SAFETY: `sw_free(data)`: the block, from the libc allocator.
    unsafe { free(data.cast()) };

    SW_OK
}

/// `TEST(server, reactor_thread_pipe_writable)`.
fn reactor_thread_pipe_writable() {
    let serv = new_server(MODE_PROCESS);
    serv.config().worker_num = 1;

    RDATA_4M.set(random_bytes(4 * 1024 * 1024)).unwrap();

    set_log_warning();

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    let port = port.expect("port");
    let config = port.config();
    config.open_length_check = true;
    config.protocol.package_max_length = 8 * 1024 * 1024;
    // `network::Stream::set_protocol(&port->protocol)`: a 4-byte big-endian length, then
    // the body (`init_protocol` gives a length check its default length function).
    config.protocol.package_length_type = b'N';
    config.protocol.package_length_size = 4;
    config.protocol.package_body_offset = 4;
    config.protocol.package_length_offset = 0;

    new_lock();

    assert_eq!(serv.create(), SW_OK);

    thread_client(move || {
        lock();

        let rdata = RDATA_4M.get().unwrap();
        let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();

        c.write_all(&(rdata.len() as u32).to_be_bytes()).unwrap();
        c.write_all(rdata).unwrap();

        let mut rbuf = vec![0; rdata.len() + 1024];

        let mut pkt_len = [0; 4];
        let rn = recv(&mut c, &mut pkt_len);
        assert_eq!(rn, 4);

        let length = u32::from_be_bytes(pkt_len) as usize;
        let rn = recv_all(&mut c, &mut rbuf[..length]);
        assert_eq!(rn, rdata.len());

        drop(c);

        assert_eq!(&rbuf[..rdata.len()], &rdata[..]);

        shutdown(serv);
    });

    serv.set_callback(
        Slot::WorkerStart,
        pipe_writable_worker_start as *const c_void,
    );
    serv.set_callback(Slot::Receive, pipe_writable_receive as *const c_void);

    serv.start();
    join_client();
    checks_held();
    serv.delete();
}

/// `send_timeout`'s client: 1024 bytes, then two seconds before reading the rest.
fn send_timeout_client(serv: Server) {
    lock();

    let mut c = TcpStream::connect((TEST_HOST, serv.get_primary_port().port())).unwrap();
    c.write_all(PACKET).unwrap();

    let mut rbuf = vec![0; 3 * 1024 * 1024];

    let rn = recv(&mut c, &mut rbuf[..1024]);
    assert_eq!(rn, 1024);
    let mut length = 1024;

    sleep(2.0);

    loop {
        let rn = recv(&mut c, &mut rbuf[length..]);
        if rn <= 0 {
            break;
        }
        length += rn as usize;
    }

    assert_eq!(&rbuf[..length], &WBUF.get().unwrap()[..length]);
    drop(c);

    kill_pid(serv.get_master_pid());
}

unsafe extern "C" fn send_timeout_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let (sid, _) = request(req);
    let conn = serv.get_connection_by_session_id(sid);

    // SAFETY: the connection's live socket, whose `recv_timer` is at SOCKET_RECV_TIMER.
    let recv_timer = unsafe {
        &mut *conn
            .socket
            .cast::<u8>()
            .add(SOCKET_RECV_TIMER)
            .cast::<Option<swoole_rs_corebase::timer_key::Held>>()
    };
    if let Some(timer) = recv_timer.take() {
        swoole_rs_ffi::del_id(timer);
    }
    // SAFETY: the socket's first member is its fd.
    set_buffer_size(unsafe { *conn.socket.cast::<c_int>() }, 65536);

    check(
        serv.send(sid, WBUF.get().unwrap()),
        "serv->send(sid, wbuf.str, wbuf.length)",
    );

    counter_incr(0, 1);

    SW_OK
}

/// `TEST(server, send_timeout)`.
fn send_timeout() {
    let serv = new_server(MODE_BASE);
    serv.config().worker_num = 1;
    set_log_warning();

    counter_init();

    new_lock();

    let Some(port) = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0) else {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    };

    port.config().max_idle_time = 1.0;

    WBUF.set(random_bytes(2 * 1024 * 1024)).unwrap();

    assert_eq!(serv.create(), SW_OK);

    set_start_client(serv, send_timeout_client);
    serv.set_callback(Slot::WorkerStart, overflow_worker_start as *const c_void);
    serv.set_callback(Slot::Receive, send_timeout_receive as *const c_void);

    assert_eq!(serv.start(), 0);

    join_client();
    assert_eq!(counter_get(0), 1);
    assert_eq!(counter_get(3), 1);
    checks_held();
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
    ("command_1", command_1),
    ("command_2", command_2),
    ("abnormal_pipeline_data", abnormal_pipeline_data),
    (
        "base_event_worker_pool_cleanup",
        base_event_worker_pool_cleanup,
    ),
    ("abort_worker", abort_worker),
    ("reactor_thread_pipe_writable", reactor_thread_pipe_writable),
    ("heartbeat_check_1", heartbeat_check_1),
    ("heartbeat_check_2", heartbeat_check_2),
    ("heartbeat_check_3", heartbeat_check_3),
    ("heartbeat_check_4", heartbeat_check_4),
    ("close_1", close_1),
    ("close_2", close_2),
    ("close_3", close_3),
    ("close_4", close_4),
    ("close_5", close_5),
    ("close_6", close_6),
    ("close_7", close_7),
    ("close_8", close_8),
    ("eof_check", eof_check),
    ("send_timeout", send_timeout),
];
