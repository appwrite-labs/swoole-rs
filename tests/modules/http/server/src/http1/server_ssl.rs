//! Upstream's `core-tests/src/server/server.cpp` TLS TESTs (`ssl`, `ssl_error`, `ssl_write`,
//! `dtls`, `dtls2`, `ssl_client_cert_1` .. `_3`), one Rust function per TEST with the same
//! inputs and expected values, over the crates' libswoole.
//!
//! `network::SyncClient` is crates/clients' `network::Client` through its `swr_v1_clients_*`
//! exports, with `SyncClient`'s `connected`/`created` bookkeeping. The `Mutex(true)` the client
//! thread waits on and `onWorkerStart` unlocks is a socket pair, and `test::counter_*`'s
//! counter is `sw_shm_malloc` memory, as upstream's is: in process mode the worker is another
//! process. An `EXPECT` in a callback is recorded there too, so a worker's failure fails the
//! case once the server stops.

use super::case::{
    CERT_FILE, CLIENT_CERT_FILE, KEY_FILE, SW_LOG_INFO, SW_SOCK_SSL, log_level, set_ssl_path,
    tls_fields,
};
use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use swoole_rs_clients::client::{Handle, Op};

const PACKET: &[u8] = b"hello world\n";
const TEST_HOST6: &str = "::1";
const SW_SOCK_UDP: c_int = 2;
const SW_SOCK_UDP6: c_int = 4;
const SW_SOCK_UNIX_DGRAM: c_int = 6;
const SW_ERROR_WRONG_OPERATION: c_int = 509;
const F_GETFD: c_int = 1;

// crates/clients' `closed` flag.
const SWR_CLIENTS_CLOSED: u32 = 1;

unsafe extern "C" {
    fn swr_v1_clients_new(kind: c_int, is_async: bool) -> *mut c_void;
    #[link_name = "_Z13sw_shm_mallocm"]
    fn sw_shm_malloc(size: usize) -> *mut c_void;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

// What `test::dump_cert_info` calls.
#[link(name = "crypto")]
unsafe extern "C" {
    fn BIO_new_mem_buf(buf: *const c_void, len: c_int) -> *mut c_void;
    fn BIO_free(bio: *mut c_void) -> c_int;
    fn PEM_read_bio_X509(
        bio: *mut c_void,
        out: *mut *mut c_void,
        callback: *const c_void,
        data: *mut c_void,
    ) -> *mut c_void;
    fn X509_free(cert: *mut c_void);
    fn X509_get_subject_name(cert: *const c_void) -> *mut c_void;
    fn X509_get_issuer_name(cert: *const c_void) -> *mut c_void;
    fn X509_NAME_oneline(name: *const c_void, buf: *mut c_char, size: c_int) -> *mut c_char;
    fn X509_NAME_entry_count(name: *const c_void) -> c_int;
    fn X509_NAME_get_entry(name: *const c_void, index: c_int) -> *mut c_void;
    fn X509_NAME_ENTRY_get_object(entry: *const c_void) -> *mut c_void;
    fn X509_NAME_ENTRY_get_data(entry: *const c_void) -> *mut c_void;
    fn OBJ_obj2txt(buf: *mut c_char, len: c_int, obj: *const c_void, no_name: c_int) -> c_int;
    fn ASN1_STRING_to_UTF8(out: *mut *mut u8, data: *const c_void) -> c_int;
    fn CRYPTO_free(ptr: *mut c_void, file: *const c_char, line: c_int);
}

/// `test::counter_*`'s counter 0, and whether an `EXPECT` failed, in any process.
#[repr(C)]
struct Shared {
    counter: AtomicI32,
    failed: AtomicBool,
}

/// The shared page, mapped before the server forks.
fn shared() -> &'static Shared {
    static AT: OnceLock<usize> = OnceLock::new();
    let at = *AT.get_or_init(|| {
        // SAFETY: a fresh zeroed shared mapping, never freed.
        let at = unsafe { sw_shm_malloc(size_of::<Shared>()) };
        assert!(!at.is_null(), "sw_shm_malloc");
        at as usize
    });
    // SAFETY: the mapping holds a `Shared`, zeroed as its atomics' initial values.
    unsafe { &*(at as *const Shared) }
}

/// `EXPECT_*` anywhere: report and record, then carry on as gtest does.
fn check(held: bool, what: &str) {
    if !held {
        eprintln!("expectation failed: {what}");
        shared().failed.store(true, Ordering::SeqCst);
    }
}

/// The client thread's end and the server's end of the `Mutex(true)`.
fn gate() -> &'static (UnixStream, UnixStream) {
    static GATE: OnceLock<(UnixStream, UnixStream)> = OnceLock::new();
    GATE.get_or_init(|| UnixStream::pair().unwrap())
}

/// `lock->lock()` once the mutex is held: waits for an `unlock`.
fn lock() {
    let mut byte = [0_u8];
    (&gate().0).read_exact(&mut byte).unwrap();
}

/// `onWorkerStart`'s `lock->unlock()`.
unsafe extern "C" fn unlock(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    (&gate().1).write_all(b"x").unwrap();
}

fn ssl_file(file: &str) -> CString {
    CString::new(format!("{}/tests/include/ssl_certs/{file}", root_path())).unwrap()
}

/// `test::is_valid_fd`.
fn is_valid_fd(fd: c_int) -> bool {
    // SAFETY: fcntl(2) of any fd.
    unsafe { fcntl(fd, F_GETFD) != -1 }
}

impl Port {
    fn is_dgram(self) -> bool {
        [SW_SOCK_UDP, SW_SOCK_UDP6, SW_SOCK_UNIX_DGRAM].contains(&self.config().type_)
    }

    /// `get_fd()`: its socket's fd, the socket's first member.
    fn get_fd(self) -> c_int {
        // SAFETY: a listening port's live socket.
        unsafe { *self.config().socket.cast::<c_int>() }
    }

    /// `set_ssl_cert_file` and `set_ssl_key_file`, under `get_ssl_dir()`.
    fn set_ssl_files(self, cert: &str, key: &str) {
        set_ssl_path(self.ssl_context(), CERT_FILE, ssl_file(cert).as_ptr());
        set_ssl_path(self.ssl_context(), KEY_FILE, ssl_file(key).as_ptr());
    }

    fn ssl_init(self) -> bool {
        // SAFETY: a live SSL port, which nothing else borrows meanwhile.
        swoole_rs_ports::lifecycle::ssl_init(unsafe { swoole_rs_server::port_config::of(self.0) })
    }
}

impl Server {
    /// `ports.at(index)`.
    fn port_at(self, index: usize) -> Port {
        // SAFETY: the state's tables live as long as the server.
        let ports = unsafe { &(*self.head().tables).ports };
        assert!(index < ports.count as usize);
        // SAFETY: `count` listening ports.
        Port(unsafe { *ports.items.add(index) })
    }

    /// `add_port(type, TEST_HOST, 0)`, or exit 2 as the cases do on a failed listen.
    fn listen(self, kind: c_int) -> Port {
        self.add_port(kind, TEST_HOST, 0).unwrap_or_else(|| {
            eprintln!("listen failed, [error={}]", last_error());
            std::process::exit(2)
        })
    }
}

/// `network::SyncClient`, closed and deleted on drop.
struct SyncClient {
    state: *mut c_void,
    dgram: bool,
    connected: bool,
    created: bool,
}

impl SyncClient {
    fn new(kind: c_int) -> Self {
        // SAFETY: no preconditions.
        let state = unsafe { swr_v1_clients_new(kind, false) };
        assert!(!state.is_null(), "the client has a socket");
        Self {
            state,
            dgram: kind == SW_SOCK_UDP || kind == SW_SOCK_UDP6,
            connected: false,
            created: true,
        }
    }

    /// crates/clients' handle on the client.
    fn handle(&self) -> Handle {
        // SAFETY: the client `new` made, live until the drop frees it.
        unsafe { Handle::new(self.state.cast()) }
    }

    fn run(&self, op: Op<'_>) -> i64 {
        self.handle().run(op)
    }

    fn connect(&mut self, host: &str, port: c_int) -> bool {
        if self.connected || !self.created {
            return false;
        }
        let host = CString::new(host).unwrap();
        let connected = self.run(Op::Connect {
            host: &host,
            port,
            timeout: -1.0,
            flags: self.dgram.into(),
        });
        if connected < 0 {
            return false;
        }
        self.connected = true;
        true
    }

    fn enable_ssl_encrypt(&self) -> bool {
        if self.handle().enable_ssl().is_none() {
            return false;
        }
        !self.connected || self.run(Op::SslHandshake) == i64::from(SW_OK)
    }

    /// `get_client()->set_ssl_cert_file(file)` (`CERT_FILE`) or `set_ssl_key_file`.
    fn set_ssl_file(&self, index: u32, file: &str) {
        // The live client's context, made by `enable_ssl_encrypt`.
        let context = self.handle().ssl_context();
        let context: *const c_void = context.map_or(std::ptr::null(), |context| {
            context.as_ptr().cast_const().cast()
        });
        assert!(!context.is_null(), "enable_ssl_encrypt made the context");
        set_ssl_path(context, index, ssl_file(file).as_ptr());
    }

    fn send(&self, data: &[u8]) -> i64 {
        self.run(Op::Send { data, flags: 0 })
    }

    fn recv(&self, buf: &mut [u8]) -> i64 {
        self.run(Op::Recv {
            buffer: buf,
            flags: 0,
        })
    }

    fn close(&mut self) -> bool {
        // SAFETY: the client `new` made, live until the drop frees it.
        let client = unsafe { &*self.state.cast::<swoole_rs_clients::client::Client>() };
        if !self.created || client.flag(SWR_CLIENTS_CLOSED) {
            return false;
        }
        let _ = self.run(Op::Close);
        self.created = false;
        true
    }
}

impl Drop for SyncClient {
    fn drop(&mut self) {
        if self.created {
            self.close();
        }
        // SAFETY: the client `new` made, deleted once.
        unsafe { self.handle().free() };
    }
}

/// The `onReceive` of `ssl`, `dtls`, `dtls2` and `ssl_client_cert`: `Server: ` and the packet.
fn reply(serv: Server, req: *mut RecvData) -> c_long {
    let (session, data) = request(req);
    check(data == PACKET, "string(req->data, req->info.len) == packet");
    serv.send(session, &[b"Server: ", PACKET].concat());
    session
}

unsafe extern "C" fn echo_receive(_: *mut c_void, serv: *mut c_void, req: *mut RecvData) -> c_int {
    reply(Server(serv), req);
    SW_OK
}

/// The cases' `onStart`: `body` in a thread that blocks all signals and waits for the worker.
fn start_client(serv: Server, body: impl FnOnce(Server) + Send + 'static) {
    client_thread(
        move || serv.get_master_pid(),
        move || {
            block_all_signals();
            lock();
            body(serv);
        },
    );
}

/// `serv.start()`, the client thread joined, then every recorded expectation.
fn run_server(serv: Server) {
    assert_eq!(serv.start(), 0);
    join_client();
    assert!(
        !shared().failed.load(Ordering::SeqCst),
        "an expectation failed"
    );
}

/// A one-worker server at log level `level` and its TCP SSL port with `cert` and `key`.
fn tls_server(mode: c_int, level: c_int, cert: &str, key: &str) -> (Server, Port) {
    shared();
    gate();
    let serv = Server::new(mode);
    serv.config().worker_num = 1;
    log_level(level);
    let port = serv.listen(SW_SOCK_TCP | SW_SOCK_SSL);
    port.set_ssl_files(cert, key);
    (serv, port)
}

unsafe extern "C" fn ssl_start(_: *mut c_void, serv: *mut c_void) {
    start_client(Server(serv), |serv| {
        let port = serv.get_primary_port();
        check(port.config().ssl == 1, "port->ssl == 1");

        let mut c = SyncClient::new(SW_SOCK_TCP);
        c.connect(TEST_HOST, port.config().port);
        c.enable_ssl_encrypt();
        c.send(PACKET);
        let mut buf = [0_u8; 1024];
        c.recv(&mut buf);
        c.close();

        // bad SSL connection, send plain text packet to SSL server
        let mut c2 = SyncClient::new(SW_SOCK_TCP);
        c2.connect(TEST_HOST, port.config().port);
        c2.send(PACKET);
        assert_eq!(c2.recv(&mut buf), 0);
        c2.close();

        kill_pid(serv.get_master_pid());
    });
}

/// `TEST(server, ssl)`.
fn ssl() {
    let (serv, port) = tls_server(MODE_PROCESS, SW_LOG_WARNING, "server.crt", "server.key");
    port.ssl_init();
    assert_eq!(serv.create(), SW_OK);
    serv.set_callback(CB_START, ssl_start as *const c_void);
    serv.set_callback(CB_WORKER_START, unlock as *const c_void);
    serv.set_callback(CB_RECEIVE, echo_receive as *const c_void);
    run_server(serv);
    serv.delete();
}

unsafe extern "C" fn ssl_error_start(_: *mut c_void, serv: *mut c_void) {
    start_client(Server(serv), |serv| {
        let port = serv.get_primary_port();
        check(port.config().ssl == 1, "port->ssl == 1");

        let mut c = SyncClient::new(SW_SOCK_TCP);
        c.connect(TEST_HOST, port.config().port);
        c.enable_ssl_encrypt();
        c.send(PACKET);
        let mut buf = [0_u8; 1024];
        assert_eq!(c.recv(&mut buf), 0);
        c.close();

        kill_pid(serv.get_master_pid());
    });
}

unsafe extern "C" fn ignore_receive(_: *mut c_void, _: *mut c_void, _: *mut RecvData) -> c_int {
    SW_OK
}

unsafe extern "C" fn count_connect(_: *mut c_void, _: *mut c_void, _: *mut DataHead) {
    shared().counter.fetch_add(1, Ordering::SeqCst);
}

/// `TEST(server, ssl_error)`.
fn ssl_error() {
    let (serv, port) = tls_server(
        MODE_PROCESS,
        SW_LOG_WARNING,
        "server-not-exists.crt",
        "server-not-exists.key",
    );
    assert!(!port.ssl_init());
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);

    assert_eq!(serv.create(), SW_OK);
    serv.set_callback(CB_START, ssl_error_start as *const c_void);
    serv.set_callback(CB_WORKER_START, unlock as *const c_void);
    serv.set_callback(CB_RECEIVE, ignore_receive as *const c_void);
    serv.set_callback(CB_CONNECT, count_connect as *const c_void);
    run_server(serv);
    assert_eq!(shared().counter.load(Ordering::SeqCst), 0);
    serv.delete();
}

/// `ssl_write`'s 4 MiB of random bytes, made before the server forks.
fn wbuf() -> &'static [u8] {
    static WBUF: OnceLock<Vec<u8>> = OnceLock::new();
    WBUF.get_or_init(|| {
        let mut wbuf = vec![0_u8; 4 * 1024 * 1024];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut wbuf))
            .unwrap();
        wbuf
    })
}

unsafe extern "C" fn ssl_write_start(_: *mut c_void, serv: *mut c_void) {
    start_client(Server(serv), |serv| {
        let port = serv.get_primary_port();
        check(port.config().ssl == 1, "port->ssl == 1");

        let mut c = SyncClient::new(SW_SOCK_TCP);
        c.connect(TEST_HOST, port.config().port);
        c.enable_ssl_encrypt();
        c.send(PACKET);

        let mut rbuf = vec![0_u8; 2 * 1024 * 1024];
        let mut length = 0;
        // Upstream caps a `recv_n` at 64 KiB but passes the whole remainder, as here.
        loop {
            let n = c.recv(&mut rbuf[length..]);
            if n <= 0 {
                break;
            }
            length += usize::try_from(n).unwrap();
            std::thread::sleep(Duration::from_micros(5000));
        }

        // Not upstream's: an early EOF would leave its ASSERT_MEMEQ comparing nothing.
        assert_eq!(length, rbuf.len(), "the 2 MiB buffer filled");
        assert!(rbuf[..length] == wbuf()[..length], "ASSERT_MEMEQ");
        c.close();

        kill_pid(serv.get_master_pid());
    });
}

unsafe extern "C" fn ssl_write_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (session, _) = request(req);
    check(
        Server(serv).send(session, wbuf()),
        "serv->send(req->session_id(), wbuf.str, wbuf.length)",
    );
    shared().counter.fetch_add(1, Ordering::SeqCst);
    SW_OK
}

/// `TEST(server, ssl_write)`.
fn ssl_write() {
    let (serv, port) = tls_server(MODE_PROCESS, SW_LOG_WARNING, "server.crt", "server.key");
    assert!(port.ssl_init());
    assert_eq!(serv.create(), SW_OK);
    wbuf();
    serv.set_callback(CB_START, ssl_write_start as *const c_void);
    serv.set_callback(CB_WORKER_START, unlock as *const c_void);
    serv.set_callback(CB_RECEIVE, ssl_write_receive as *const c_void);
    run_server(serv);
    assert_eq!(shared().counter.load(Ordering::SeqCst), 1);
    serv.delete();
}

/// `dtls`' `cli_fn`.
fn dtls_client(c: &mut SyncClient) {
    c.enable_ssl_encrypt();
    c.send(PACKET);
    let mut buf = [0_u8; 1024];
    c.recv(&mut buf);
    c.close();
}

unsafe extern "C" fn dtls_start(_: *mut c_void, serv: *mut c_void) {
    let serv = Server(serv);
    // SAFETY: getpid(2).
    client_thread(
        || unsafe { getpid() },
        move || {
            block_all_signals();
            lock();

            let port = serv.port_at(0);
            check(port.config().ssl == 1, "port->ssl == 1");
            let mut c = SyncClient::new(SW_SOCK_UDP);
            c.connect(TEST_HOST, port.config().port);
            dtls_client(&mut c);

            let port6 = serv.port_at(1);
            check(port6.config().ssl == 1, "port6->ssl == 1");
            let mut c2 = SyncClient::new(SW_SOCK_UDP6);
            c2.connect(TEST_HOST6, port6.config().port);
            dtls_client(&mut c2);

            std::thread::sleep(Duration::from_micros(10000));
            server_methods::shutdown(serv.view());
        },
    );
}

/// `TEST(server, dtls)`.
fn dtls() {
    shared();
    gate();
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    set_log_warning();

    let port = serv.add_port(SW_SOCK_UDP | SW_SOCK_SSL, TEST_HOST, 0);
    let port = port.expect("port != nullptr");
    assert!(port.is_dgram());
    assert_eq!(port.config().object_id, 1);
    assert!(is_valid_fd(port.get_fd()));

    let port6 = serv.add_port(SW_SOCK_UDP6 | SW_SOCK_SSL, TEST_HOST6, 0);
    let port6 = port6.expect("port6 != nullptr");
    // Upstream asserts `port->is_dgram()` here too.
    assert!(port.is_dgram());
    assert_eq!(port6.config().object_id, 2);
    assert!(is_valid_fd(port6.get_fd()));

    port.set_ssl_files("server.crt", "server.key");
    port.ssl_init();
    port6.set_ssl_files("server.crt", "server.key");
    port6.ssl_init();

    assert_eq!(serv.create(), SW_OK);
    serv.set_callback(CB_START, dtls_start as *const c_void);
    serv.set_callback(CB_WORKER_START, unlock as *const c_void);
    serv.set_callback(CB_RECEIVE, echo_receive as *const c_void);
    run_server(serv);
    serv.delete();
}

/// `TEST(server, dtls2)`: the client is a child forked before the server starts.
fn dtls2() {
    shared();
    let server = Server::new(MODE_PROCESS);
    server.config().worker_num = 2;
    server.config().single_thread = false;
    let port = server.listen(SW_SOCK_UDP | SW_SOCK_SSL);
    port.set_ssl_files("server.crt", "server.key");
    port.ssl_init();

    server.create();
    server.set_callback(CB_RECEIVE, echo_receive as *const c_void);

    let pid = fork_with(move || {
        sleep(1.0);
        let port = server.get_primary_port();

        let mut c = SyncClient::new(SW_SOCK_UDP);
        c.connect(TEST_HOST, port.config().port);
        c.enable_ssl_encrypt();
        c.send(PACKET);
        let mut buf = [0_u8; 1024];
        c.recv(&mut buf);
        c.close();

        kill_pid(server.get_master_pid());
    });

    server.start();
    let mut status = 0;
    // SAFETY: the client child, reaped once.
    unsafe { waitpid(pid, &raw mut status, 0) };
    assert_eq!(status, 0, "the client exited 0");
    assert!(
        !shared().failed.load(Ordering::SeqCst),
        "an expectation failed"
    );
    server.delete();
}

/// `test::dump_cert_info`'s subject and issuer lines (`name: value` per entry and the
/// subject's one-line form): the validity and public-key lines are left out, as nothing
/// reads them.
fn dump_cert_info(data: &[u8]) -> String {
    let mut out = String::new();
    // SAFETY: `data` outlives the BIO; each OpenSSL object is freed once, after its last use.
    unsafe {
        let bio = BIO_new_mem_buf(data.as_ptr().cast(), c_int::try_from(data.len()).unwrap());
        assert!(!bio.is_null(), "Failed to create BIO");
        let cert = PEM_read_bio_X509(bio, ptr::null_mut(), ptr::null(), ptr::null_mut());
        if cert.is_null() {
            eprintln!("Failed to parse X509 certificate");
            BIO_free(bio);
            return out;
        }
        let subject = X509_get_subject_name(cert);
        print_all_subject_entries(subject, &mut out);
        let line = X509_NAME_oneline(subject, ptr::null_mut(), 0);
        if !line.is_null() {
            let text = CStr::from_ptr(line).to_string_lossy();
            out.push_str(&format!("Peer certificate subject: {text}\n"));
            CRYPTO_free(line.cast(), c"server_ssl.rs".as_ptr(), 0);
        }
        print_all_subject_entries(X509_get_issuer_name(cert), &mut out);
        X509_free(cert);
        BIO_free(bio);
    }
    out
}

/// `printAllSubjectEntries`.
///
/// # Safety
/// `name` is null or a live certificate's `X509_NAME`.
unsafe fn print_all_subject_entries(name: *const c_void, out: &mut String) {
    if name.is_null() {
        return;
    }
    // SAFETY: as the caller guarantees; each entry belongs to `name`.
    unsafe {
        for i in 0..X509_NAME_entry_count(name) {
            let entry = X509_NAME_get_entry(name, i);
            let mut obj_txt: [c_char; 80] = [0; 80];
            OBJ_obj2txt(
                obj_txt.as_mut_ptr(),
                80,
                X509_NAME_ENTRY_get_object(entry),
                0,
            );
            let mut utf8 = ptr::null_mut();
            let length = ASN1_STRING_to_UTF8(&raw mut utf8, X509_NAME_ENTRY_get_data(entry));
            if length >= 0 && !utf8.is_null() {
                let value = std::slice::from_raw_parts(utf8, usize::try_from(length).unwrap());
                let key = CStr::from_ptr(obj_txt.as_ptr()).to_string_lossy();
                out.push_str(&format!("{key}: {}\n", String::from_utf8_lossy(value)));
                CRYPTO_free(utf8.cast(), c"server_ssl.rs".as_ptr(), 0);
            }
        }
    }
}

unsafe extern "C" fn client_cert_start(_: *mut c_void, serv: *mut c_void) {
    start_client(Server(serv), |serv| {
        let port = serv.get_primary_port();
        check(port.config().ssl == 1, "port->ssl == 1");

        let mut c = SyncClient::new(SW_SOCK_TCP);
        c.enable_ssl_encrypt();
        c.set_ssl_file(CERT_FILE, "client-cert.pem");
        c.set_ssl_file(KEY_FILE, "client-key.pem");
        c.connect(TEST_HOST, port.config().port);
        check(
            c.send(PACKET) == PACKET.len() as i64,
            "c.send(packet, strlen(packet)) == strlen(packet)",
        );

        let mut buf = [0_u8; 1024];
        check(c.recv(&mut buf) > 0, "c.recv(buf, sizeof(buf)) > 0");
        c.close();

        kill_pid(serv.get_master_pid());
    });
}

unsafe extern "C" fn client_cert_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let session = reply(serv, req);

    let cert = serv.get_connection_by_session_id(session).ssl_client_cert;
    check(!cert.is_null(), "conn->ssl_client_cert != nullptr");
    if cert.is_null() {
        return SW_OK;
    }
    // SAFETY: the connection's `String *`, live for the callback.
    let cert = unsafe { &*cert.cast::<crate::CppString>() };
    check(cert.length > 16, "conn->ssl_client_cert->length > 16");
    // SAFETY: the String's `length` bytes.
    let pem = unsafe { std::slice::from_raw_parts(cert.str.cast::<u8>(), cert.length) };
    check(
        dump_cert_info(pem).contains("organizationName: swoole"),
        r#"strstr(buffer, "organizationName: swoole") != nullptr"#,
    );
    SW_OK
}

/// `test_ssl_client_cert(mode)`.
fn test_ssl_client_cert(mode: c_int) {
    let (serv, port) = tls_server(mode, SW_LOG_INFO, "server.crt", "server.key");
    let context = port.ssl_context();
    tls_fields(context).verify_peer = 1;
    tls_fields(context).allow_self_signed = 1;
    set_ssl_path(context, CLIENT_CERT_FILE, ssl_file("ca-cert.pem").as_ptr());
    port.ssl_init();

    assert_eq!(serv.create(), SW_OK);
    serv.set_callback(CB_START, client_cert_start as *const c_void);
    serv.set_callback(CB_WORKER_START, unlock as *const c_void);
    serv.set_callback(CB_RECEIVE, client_cert_receive as *const c_void);
    run_server(serv);
    serv.delete();
}

/// `TEST(server, ssl_client_cert_1)`.
fn ssl_client_cert_1() {
    test_ssl_client_cert(MODE_BASE);
}

/// `TEST(server, ssl_client_cert_2)`.
fn ssl_client_cert_2() {
    test_ssl_client_cert(MODE_PROCESS);
}

/// `TEST(server, ssl_client_cert_3)`.
fn ssl_client_cert_3() {
    test_ssl_client_cert(MODE_THREAD);
}

pub const CASES: &[(&str, fn())] = &[
    ("ssl", ssl),
    ("ssl_error", ssl_error),
    ("ssl_write", ssl_write),
    ("dtls", dtls),
    ("dtls2", dtls2),
    ("ssl_client_cert_1", ssl_client_cert_1),
    ("ssl_client_cert_2", ssl_client_cert_2),
    ("ssl_client_cert_3", ssl_client_cert_3),
];
