//! Upstream's `core-tests/src/network/client.cpp`, one function per TEST with the same
//! inputs and expected values, over the hybrid's `network::Client`. Built into the
//! hooks case binary (tests/modules/hooks/system), which links the crates.
//!
//! A `Client` is crates/clients' state, made and listened to through its
//! `swr_v1_clients_*` exports and operated on and deleted through its `Handle`;
//! the `Socket` TLS reads are called under their upstream symbols.
//! `SyncClient` and `AsyncClient` are the `created`/`connected` bookkeeping their
//! header gives `Client`. The `Server` fixtures are forked std servers bound before
//! the fork: the cases assert only on the client. [`timer_after`] schedules a
//! Rust closure through crates/timer's `swr_v1_timer_add_plain`.

use super::socket::{
    EADDRNOTAVAIL, ECONNREFUSED, IoVec, SHUT_RD, SHUT_RDWR, SHUT_WR, SIGTERM, SW_SOCK_TCP,
    SW_SOCK_TCP6, SW_SOCK_UDP, SW_SOCK_UDP6, SW_SOCK_UNIX_DGRAM, TEST_HOST, TEST_HTTP_EXPECT,
    TEST_HTTPS_EXPECT, TEST_PORT, TEST_REQUEST_BAIDU, TLS_HOST_NAME, TlsPrepared, contains,
    get_random_port, http_get_request, is_github_ci, swr_v1_core_string_set, swr_v1_tls_owner_new,
};
use super::{
    _exit, AF_INET, CppString, EINVAL, ENOENT, ETIMEDOUT, SW_ERR, TEST_HTTP_DOMAIN, TEST_STR,
    c_str, errno, fork, kill, last_error, random_bytes, string_new,
};
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::io::{Read, Write};
use swoole_rs_clients::client::{self as native, ClientEvent, Flag, Handle};
use swoole_rs_corebase::net_types::SocketAddress;

const GREETER: &[u8] = b"Hello Swoole\0";
const GREETER_SIZE: usize = GREETER.len();
const TEST_HOST6: &str = "::1";
const TEST_DOMAIN_BAIDU: &str = "www.baidu.com";
const TEST_WRITEV_OFFSET: usize = 87;
const TEST_READV_OFFSET: usize = 1949;
const TEST_HTTP_PROXY_HOST: &str = "127.0.0.1";
const TEST_HTTP_PROXY_PORT: c_int = 8888;
const TEST_HTTP_PROXY_USER: &str = "user";
const TEST_HTTP_PROXY_PASSWORD: &str = "password";
const TEST_SOCKS5_PROXY_HOST: &str = "127.0.0.1";
const TEST_SOCKS5_PROXY_PORT: c_int = 8080;
const TEST_SOCKS5_PROXY_USER: &str = "user";
const TEST_SOCKS5_PROXY_PASSWORD: &str = "password";

const SW_OK: c_int = 0;

/// A method's `SW_OK` or `SW_ERR`, as the C++ client returns it.
fn code(ok: bool) -> c_int {
    if ok { SW_OK } else { SW_ERR }
}
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_SOCK_RAW6: c_int = 8;
const SW_BUFFER_SIZE_BIG: usize = 65536;
const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_DNSLOOKUP_RESOLVE_FAILED: c_int = 711;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const SW_ERROR_SSL_HANDSHAKE_FAILED: c_int = 1018;
const SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR: c_int = 7101;
const SW_DNS_RESOLV_CONF: &CStr = c"/etc/resolv.conf";

// glibc's values (the core-tests build is Linux).
const EACCES: c_int = 13;
const EPIPE: c_int = 32;
const ESOCKTNOSUPPORT: c_int = 94;
const EINPROGRESS: c_int = 115;
const SIGIO: c_int = 29;
const SIGPIPE: c_int = 13;

/// crates/clients' `ClientRecord`, the head of `network::Client`, with Linux's
/// 120-byte `network::Address`.
#[repr(C)]
struct Record {
    http2: bool,
    open_length_check: bool,
    open_eof_check: bool,
    protocol: [u64; 8],
    server_addr: [u8; 120],
    remote_addr: [u8; 120],
    socket: *mut c_void,
    object: *mut c_void,
    buffer: *mut c_void,
}

const _: () = assert!(
    std::mem::offset_of!(Record, socket) == 312 && size_of::<Record>() == 336,
    "ClientRecord's layout moved"
);

/// `swr_v1_core_proxy_fields`.
#[repr(C)]
struct ProxyFields {
    state: u8,
    dont_handshake: u8,
    version: u8,
    method: u8,
    dns_tunnel: u8,
    port: i32,
    target_port: i32,
    socket_type: i32,
}

/// `OwnedTlsCopy` (crates/tls/src/owned.rs).
#[repr(C)]
struct TlsCopy {
    status: i32,
    index: u32,
}

unsafe extern "C" {
    fn swr_v1_clients_new(kind: c_int, is_async: bool) -> *mut c_void;
    fn swr_v1_clients_address_host(address: *const c_void) -> *const c_char;
    fn swr_v1_clients_address_port(address: *const c_void) -> c_int;
    fn swr_v1_tls_owner_assign(destination: *const c_void, source: *const c_void) -> TlsCopy;
    fn swr_v1_tls_owner_free(owner: *const c_void);

    #[link_name = "_ZNK6swoole7network6Socket12has_timedoutEv"]
    fn socket_has_timedout(socket: *const c_void) -> bool;
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    fn swoole_event_wait() -> c_int;
    fn swr_v1_timer_add_plain(
        ms: c_long,
        persistent: bool,
        callback: unsafe extern "C" fn(*mut c_void),
        data: *mut c_void,
    ) -> *mut c_void;
    #[link_name = "_Z17swoole_signal_setiPFviEii"]
    fn swoole_signal_set(
        signo: c_int,
        handler: extern "C" fn(c_int),
        restart: c_int,
        mask: c_int,
    ) -> *const c_void;
}

unsafe extern "C" {
    fn signal(signo: c_int, handler: usize) -> usize;
    fn getpid() -> c_int;
    fn usleep(usec: u32) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn listen(fd: c_int, backlog: c_int) -> c_int;
}

const SIG_IGN: usize = 1;

fn cstring(text: &str) -> CString {
    CString::new(text).unwrap()
}

fn event_init() {
    // SAFETY: a fresh event loop for this thread.
    unsafe { swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) };
}

fn event_wait() {
    // SAFETY: runs the loop `event_init` made.
    unsafe { swoole_event_wait() };
}

unsafe extern "C" fn closure_invoker<F: FnMut()>(data: *mut c_void) {
    // SAFETY: `timer_after` leaked an `F` as the timer's data.
    let body = unsafe { &mut *data.cast::<F>() };
    body();
}

/// `swoole_timer_after(ms, fn)` of the leaked closure.
fn timer_after<F: FnMut()>(ms: c_long, body: F) {
    let data = Box::into_raw(Box::new(body)).cast();
    // SAFETY: the invoker takes the closure leaked as `data`.
    assert!(!unsafe { swr_v1_timer_add_plain(ms, false, closure_invoker::<F>, data) }.is_null());
}

/// A boxed callback slot, called reentrantly with the others.
type Slot<'a> = RefCell<Option<Box<dyn FnMut(&Client<'a>) + 'a>>>;
type ReceiveSlot<'a> = RefCell<Option<Box<dyn FnMut(&Client<'a>, &[u8]) + 'a>>>;

/// `network::Client`, deleted on drop; `created`/`connected` are `SyncClient`'s.
pub(super) struct Client<'a> {
    state: *mut c_void,
    created: Cell<bool>,
    connected: Cell<bool>,
    sync: bool,
    on_connect: Slot<'a>,
    on_error: Slot<'a>,
    on_close: Slot<'a>,
    on_buffer_full: Slot<'a>,
    on_buffer_empty: Slot<'a>,
    on_receive: ReceiveSlot<'a>,
}

unsafe extern "C" fn listener(
    object: *mut c_void,
    kind: ClientEvent,
    data: *const c_char,
    length: usize,
) {
    // SAFETY: `listen` passed the boxed client, alive while its loop runs.
    let cli = unsafe { &*object.cast::<Client>() };
    if kind == ClientEvent::Receive {
        // SAFETY: the client's received bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
        (cli.on_receive.borrow_mut().as_mut().unwrap())(cli, bytes);
        return;
    }
    let slot = match kind {
        ClientEvent::Connect => &cli.on_connect,
        ClientEvent::Error => &cli.on_error,
        ClientEvent::Close => &cli.on_close,
        ClientEvent::BufferFull => &cli.on_buffer_full,
        ClientEvent::BufferEmpty | ClientEvent::Receive => &cli.on_buffer_empty,
    };
    (slot.borrow_mut().as_mut().unwrap())(cli);
}

impl<'a> Client<'a> {
    /// `Client cli(type, async)`; None when it has no socket (`!cli.ready()`).
    pub(super) fn new(kind: c_int, is_async: bool) -> Option<Box<Self>> {
        // SAFETY: no preconditions.
        let state = unsafe { swr_v1_clients_new(kind, is_async) };
        if state.is_null() {
            return None;
        }
        Some(Box::new(Self {
            state,
            created: Cell::new(true),
            connected: Cell::new(false),
            sync: false,
            on_connect: RefCell::new(None),
            on_error: RefCell::new(None),
            on_close: RefCell::new(None),
            on_buffer_full: RefCell::new(None),
            on_buffer_empty: RefCell::new(None),
            on_receive: RefCell::new(None),
        }))
    }

    /// `SyncClient(type, async)`, or `AsyncClient(type)`: closed on drop.
    fn wrapped(kind: c_int, is_async: bool) -> Box<Self> {
        let mut cli = Self::new(kind, is_async).expect("created");
        cli.sync = true;
        cli
    }

    fn record(&self) -> &Record {
        // SAFETY: the live client, whose head is the record.
        unsafe { &*self.state.cast::<Record>() }
    }

    pub(super) fn socket(&self) -> *mut c_void {
        self.record().socket
    }

    /// Installs the callbacks set so far, as assigning `onConnect` and the rest does.
    fn listen(&self) {
        let mut events = 0;
        for (kind, set) in [
            (ClientEvent::Connect, self.on_connect.borrow().is_some()),
            (ClientEvent::Receive, self.on_receive.borrow().is_some()),
            (ClientEvent::Close, self.on_close.borrow().is_some()),
            (ClientEvent::Error, self.on_error.borrow().is_some()),
            (
                ClientEvent::BufferFull,
                self.on_buffer_full.borrow().is_some(),
            ),
            (
                ClientEvent::BufferEmpty,
                self.on_buffer_empty.borrow().is_some(),
            ),
        ] {
            if set {
                events |= kind.bit();
            }
        }
        let object = std::ptr::from_ref(self).cast::<c_void>();
        // SAFETY: a live state.
        let state = unsafe { &*self.state.cast::<swoole_rs_clients::client::Client>() };
        swoole_rs_clients::client::listen(
            state,
            events,
            // SAFETY: the boxed client outlives its callbacks.
            std::rc::Rc::new(move |kind, data, length| unsafe {
                listener(object.cast_mut(), kind, data, length);
            }),
        );
    }

    fn on_connect(&self, body: impl FnMut(&Client<'a>) + 'a) {
        *self.on_connect.borrow_mut() = Some(Box::new(body));
    }

    fn on_error(&self, body: impl FnMut(&Client<'a>) + 'a) {
        *self.on_error.borrow_mut() = Some(Box::new(body));
    }

    fn on_close(&self, body: impl FnMut(&Client<'a>) + 'a) {
        *self.on_close.borrow_mut() = Some(Box::new(body));
    }

    fn on_buffer_full(&self, body: impl FnMut(&Client<'a>) + 'a) {
        *self.on_buffer_full.borrow_mut() = Some(Box::new(body));
    }

    fn on_buffer_empty(&self, body: impl FnMut(&Client<'a>) + 'a) {
        *self.on_buffer_empty.borrow_mut() = Some(Box::new(body));
    }

    fn on_receive(&self, body: impl FnMut(&Client<'a>, &[u8]) + 'a) {
        *self.on_receive.borrow_mut() = Some(Box::new(body));
    }

    /// crates/clients' handle on the client.
    /// The crates' client `state` is.
    fn client(&self) -> &swoole_rs_clients::client::Client {
        // SAFETY: the client `new` made, live until the drop frees it.
        unsafe { &*self.state.cast() }
    }

    /// The client's `SSLContext` owner, null while TLS is off.
    fn ssl_context(&self) -> *const c_void {
        let context = self.handle().ssl_context();
        context.map_or(std::ptr::null(), |context| {
            context.as_ptr().cast_const().cast()
        })
    }

    fn handle(&self) -> Handle {
        // SAFETY: the client `new` made, live until the drop frees it.
        unsafe { Handle::new(self.state.cast()) }
    }

    pub(super) fn connect(&self, host: &str, port: c_int, timeout: f64, flags: c_int) -> c_int {
        // `Client::connect`'s SW_OK or SW_ERR.
        if native::connect(self.handle(), &cstring(host), port, timeout, flags) {
            SW_OK
        } else {
            -1
        }
    }

    fn send(&self, data: &[u8], flags: c_int) -> isize {
        native::send(self.handle(), data, flags)
    }

    fn sendfile(&self, file: &str, offset: i64, length: usize) -> c_int {
        code(native::sendfile(
            self.handle(),
            &cstring(file),
            offset,
            length,
        ))
    }

    fn recv(&self, buf: &mut [u8], flags: c_int) -> isize {
        native::recv(self.handle(), buf, flags)
    }

    fn close(&self) -> c_int {
        if native::close(self.handle()) {
            SW_OK
        } else {
            SW_ERR
        }
    }

    pub(super) fn shutdown(&self, how: c_int) -> c_int {
        code(native::shutdown(self.handle(), how))
    }

    fn sleep(&self) -> c_int {
        code(native::sleep(self.handle()))
    }

    fn wakeup(&self) -> c_int {
        code(native::wakeup(self.handle()))
    }

    fn bind(&self, host: &str, port: c_int) -> c_int {
        if self.handle().bind(&cstring(host), port) {
            SW_OK
        } else {
            SW_ERR
        }
    }

    fn sendto(&self, host: &str, port: c_int, data: &[u8]) -> c_int {
        code(native::sendto(self.handle(), &cstring(host), port, data))
    }

    /// `get_peer_name(&addr)`, then the address's `get_addr()` and `get_port()`.
    fn get_peer_name(&self) -> (c_int, String, c_int) {
        let mut address = SocketAddress::default();
        let result = if self.handle().peer_name(&mut address) {
            SW_OK
        } else {
            SW_ERR
        };
        let address = (&raw const address).cast();
        // SAFETY: the address Rust filled.
        let host = unsafe { CStr::from_ptr(swr_v1_clients_address_host(address)) };
        // SAFETY: as above.
        let port = unsafe { swr_v1_clients_address_port(address) };
        (result, host.to_str().unwrap().to_owned(), port)
    }

    pub(super) fn enable_ssl_encrypt(&self) -> c_int {
        if self.handle().enable_ssl().is_some() {
            0
        } else {
            -1
        }
    }

    fn ssl_handshake(&self) -> c_int {
        code(native::ssl_handshake(self.handle()))
    }

    fn ssl_verify(&self, allow_self_signed: bool) -> c_int {
        code(native::ssl_verify(self.handle(), allow_self_signed))
    }

    /// `set_tls_host_name(name)`: a fresh `SSLContext`'s view with the name set, assigned
    /// to the client's (whose other values are still a fresh one's).
    fn set_tls_host_name(&self, name: &str) {
        let prepared: TlsPrepared = swr_v1_tls_owner_new();
        // SAFETY: the fresh owner's live fields and slot, then the client's live owner.
        unsafe {
            (*prepared.fields).disable_tls_host_name = u8::from(name.is_empty());
            assert_eq!(
                swr_v1_core_string_set(
                    prepared.strings[TLS_HOST_NAME],
                    name.as_ptr().cast(),
                    name.len()
                ),
                0
            );
            assert_eq!(
                swr_v1_tls_owner_assign(self.ssl_context(), prepared.owner).status,
                0
            );
            swr_v1_tls_owner_free(prepared.owner);
        }
    }

    fn set_proxy(&self, http: bool, host: &str, port: c_int, user: &str, password: &str) {
        // SAFETY: `state` is this test's live client.
        let client = unsafe { &*self.state.cast::<swoole_rs_clients::client::Client>() };
        swoole_rs_clients::client::set_proxy(
            client,
            http,
            host.as_bytes(),
            i64::from(port),
            user.as_bytes(),
            password.as_bytes(),
        );
    }

    fn set_socks5_proxy(&self, host: &str, port: c_int, user: &str, password: &str) {
        self.set_proxy(false, host, port, user, password);
    }

    fn set_http_proxy(&self, host: &str, port: c_int, user: &str, password: &str) {
        self.set_proxy(true, host, port, user, password);
    }

    /// `client.socks5_proxy`'s `dns_tunnel`, or None without one.
    fn socks5_dns_tunnel(&self) -> Option<&mut u8> {
        let fields = self.handle().proxy_fields(false).cast::<ProxyFields>();
        // SAFETY: the client's live proxy's fields.
        (!fields.is_null()).then(|| unsafe { &mut (*fields).dns_tunnel })
    }

    fn set_watermarks(&self, high: u32, low: u32) {
        self.client().set_watermark(true, high);
        self.client().set_watermark(false, low);
    }

    fn async_connect(&self) -> bool {
        self.client().flag(Flag::AsyncConnect)
    }

    fn has_timedout(&self) -> bool {
        // SAFETY: the client's live socket.
        unsafe { socket_has_timedout(self.socket()) }
    }

    fn is_dgram(&self) -> bool {
        // SAFETY: the client's live socket's type.
        let kind = unsafe { *self.socket().cast::<c_int>().add(2) };
        [SW_SOCK_UDP, SW_SOCK_UDP6, SW_SOCK_UNIX_DGRAM].contains(&kind)
    }

    /// `SyncClient::connect`, and `AsyncClient::connect` once its callbacks are set.
    fn sync_connect(&self, host: &str, port: c_int, timeout: f64) -> bool {
        if self.on_connect.borrow().is_some() {
            self.listen();
        }
        if self.connected.get() || !self.created.get() {
            return false;
        }
        if self.connect(host, port, timeout, c_int::from(self.is_dgram())) < 0 {
            return false;
        }
        self.connected.set(true);
        true
    }

    /// `SyncClient::enable_ssl_encrypt`.
    fn sync_enable_ssl_encrypt(&self) -> bool {
        if self.enable_ssl_encrypt() < 0 {
            return false;
        }
        !self.connected.get() || self.ssl_handshake() == SW_OK
    }

    /// `SyncClient::close`.
    fn sync_close(&self) -> bool {
        if !self.created.get() || self.client().flag(Flag::Closed) {
            return false;
        }
        self.close();
        self.created.set(false);
        true
    }
}

impl Drop for Client<'_> {
    fn drop(&mut self) {
        if self.sync && self.created.get() {
            self.sync_close();
        }
        // SAFETY: the client `new` made, deleted once.
        unsafe { self.handle().free() };
    }
}

/// A forked child serving until it is killed: the parent bound its socket first.
fn spawn(serve: impl FnOnce()) -> c_int {
    // SAFETY: fork(2); the child serves until it is killed.
    let pid = unsafe { fork() };
    assert_ne!(pid, -1);
    if pid == 0 {
        serve();
        // SAFETY: ends the child.
        unsafe { _exit(0) };
    }
    pid
}

/// `Server(host, port, MODE_BASE, SW_SOCK_TCP[6])` whose `onReceive` replies with
/// `reply(data)` (nothing when empty).
fn tcp_server(host: &str, port: c_int, reply: fn(&[u8]) -> Vec<u8>) -> c_int {
    let listener = std::net::TcpListener::bind((host, u16::try_from(port).unwrap())).expect("bind");
    spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0_u8; 65536];
            while let Ok(n @ 1..) = stream.read(&mut buf) {
                let answer = reply(&buf[..n]);
                if !answer.is_empty() && stream.write_all(&answer).is_err() {
                    break;
                }
            }
        }
    })
}

/// `Server(host, port, MODE_BASE, type)` for a datagram type, whose `onPacket` echoes.
fn dgram_server(host: &str, port: c_int, kind: c_int) -> c_int {
    if kind == SW_SOCK_UNIX_DGRAM {
        let _ = std::fs::remove_file(host);
        let socket = std::os::unix::net::UnixDatagram::bind(host).expect("bind");
        return spawn(move || {
            let mut buf = [0_u8; 65536];
            while let Ok((n, peer)) = socket.recv_from(&mut buf) {
                if let Some(path) = peer.as_pathname() {
                    let _ = socket.send_to(&buf[..n], path);
                }
            }
        });
    }
    // "localhost" as a UDP (IPv4) server's host is 127.0.0.1.
    let host = if kind == SW_SOCK_UDP && host == "localhost" {
        TEST_HOST
    } else {
        host
    };
    let socket = std::net::UdpSocket::bind((host, u16::try_from(port).unwrap())).expect("bind");
    spawn(move || {
        let mut buf = [0_u8; 65536];
        while let Ok((n, peer)) = socket.recv_from(&mut buf) {
            let _ = socket.send_to(&buf[..n], peer);
        }
    })
}

fn stop(pid: c_int) {
    let mut status = 0;
    // SAFETY: the server child.
    unsafe {
        kill(pid, SIGTERM);
        waitpid(pid, &raw mut status, 0);
    }
}

fn echo(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}

fn tcp() {
    let mut buf = [0_u8; 128];
    let port = get_random_port();
    let pid = tcp_server(TEST_HOST, port, echo);

    let cli = Client::new(SW_SOCK_TCP, false).expect("cli.socket");
    assert_eq!(cli.connect(TEST_HOST, port, -1.0, 0), 0);
    assert!(cli.send(GREETER, 0) > 0);
    assert_eq!(cli.recv(&mut buf, 0), GREETER_SIZE as isize);
    assert_eq!(c_str(&buf), c_str(GREETER));

    let (result, host, peer_port) = cli.get_peer_name();
    assert_eq!(result, 0);
    assert_eq!(host, "127.0.0.1");
    assert_eq!(peer_port, port);

    assert_eq!(cli.close(), SW_OK);
    assert_eq!(cli.close(), SW_ERR);

    stop(pid);
}

fn test_sync_client_dgram(host: &str, port: c_int, kind: c_int) {
    let mut buf = [0_u8; 128];
    let pid = dgram_server(host, port, kind);

    let cli = Client::new(kind, false).expect("cli.socket");
    assert_eq!(cli.connect(host, port, -1.0, 0), 0);
    assert!(cli.send(GREETER, 0) > 0);
    assert_eq!(cli.recv(&mut buf, 0), GREETER_SIZE as isize);
    assert_eq!(c_str(&buf), c_str(GREETER));

    stop(pid);
}

fn udp() {
    test_sync_client_dgram("127.0.0.1", get_random_port(), SW_SOCK_UDP);
}

fn udp6() {
    test_sync_client_dgram("::1", get_random_port(), SW_SOCK_UDP6);
}

fn udg() {
    test_sync_client_dgram("/tmp/swoole_core_tests.sock", 0, SW_SOCK_UNIX_DGRAM);
}

fn test_async_client_tcp(host: &str, port: c_int, kind: c_int) {
    let server_host = if kind == SW_SOCK_TCP6 {
        TEST_HOST6
    } else {
        TEST_HOST
    };
    let pid = tcp_server(server_host, port, echo);

    event_init();

    let ac = Client::wrapped(kind, true);
    ac.on_connect(|ac| {
        ac.send(GREETER, 0);
    });
    ac.on_close(|_| {});
    ac.on_error(|_| {});
    ac.on_receive(|ac, data| {
        assert_eq!(data.len(), GREETER_SIZE);
        assert_eq!(c_str(data), c_str(GREETER));
        ac.sync_close();
    });

    assert!(ac.sync_connect(host, port, 1.0));

    event_wait();

    stop(pid);
}

fn async_tcp() {
    test_async_client_tcp(TEST_HOST, get_random_port(), SW_SOCK_TCP);
}

fn async_tcp_dns() {
    test_async_client_tcp("localhost", get_random_port(), SW_SOCK_TCP);
}

fn async_tcp6() {
    test_async_client_tcp("::1", get_random_port(), SW_SOCK_TCP6);
}

fn async_tcp6_dns() {
    test_async_client_tcp("localhost", get_random_port(), SW_SOCK_TCP6);
}

/// The callbacks the handshake-failure cases share: onError checks the last error.
fn listen_failing<'a>(ac: &Client<'a>, error: c_int, success: &'a Cell<bool>) {
    ac.on_connect(move |ac| {
        ac.send(GREETER, 0);
        success.set(true);
    });
    ac.on_close(|_| {});
    ac.on_error(move |_| {
        eprintln!("connect failed, ERROR: {}", errno());
        assert_eq!(last_error(), error);
        success.set(false);
    });
    ac.on_receive(|ac, data| {
        assert_eq!(data.len(), GREETER_SIZE);
        assert_eq!(c_str(data), c_str(GREETER));
        ac.close();
    });
    ac.listen();
}

fn async_tcp_dns_fail() {
    event_init();

    let success = Cell::new(true);
    let ac = Client::new(SW_SOCK_TCP, true).unwrap();
    assert_eq!(ac.connect(TEST_HOST, 9999, -1.0, 0), SW_ERR);

    listen_failing(&ac, SW_ERROR_DNSLOOKUP_RESOLVE_FAILED, &success);

    assert_eq!(ac.connect("www.baidu.com-not-found", 80, 1.0, 0), SW_OK);

    event_wait();

    assert!(!success.get());
}

fn async_tcp_ssl_handshake_fail() {
    event_init();

    let success = Cell::new(true);
    let ac = Client::new(SW_SOCK_TCP, true).unwrap();
    listen_failing(&ac, SW_ERROR_SSL_HANDSHAKE_FAILED, &success);

    ac.enable_ssl_encrypt();

    assert_eq!(ac.connect("www.baidu.com", 80, 1.0, 0), SW_OK);

    event_wait();

    assert!(!success.get());
}

fn async_tcp_http_proxy_handshake_fail() {
    let port = get_random_port();
    let pid = tcp_server(TEST_HOST, port, |_| {
        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec()
    });

    event_init();

    let handshake_failed = Cell::new(false);
    let ac = Client::new(SW_SOCK_TCP, true).unwrap();

    ac.on_connect(|ac| {
        eprintln!("HTTP proxy handshake unexpectedly succeeded");
        ac.close();
        // SAFETY: ADD_FAILURE: the case fails.
        unsafe { _exit(1) };
    });
    ac.on_close(|_| {});
    ac.on_error(|_| {
        assert_eq!(last_error(), SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR);
        handshake_failed.set(true);
    });
    ac.on_receive(|ac, _| {
        eprintln!("HTTP proxy response reached the application");
        ac.close();
        // SAFETY: ADD_FAILURE: the case fails.
        unsafe { _exit(1) };
    });
    ac.listen();

    ac.set_http_proxy(TEST_HOST, port, "", "");

    assert_eq!(ac.connect("target.invalid", 80, 1.0, 0), SW_OK);

    event_wait();

    assert!(handshake_failed.get());

    stop(pid);
}

fn async_tcp_socks5_proxy_handshake_fail() {
    event_init();

    let success = Cell::new(true);
    let ac = Client::new(SW_SOCK_TCP, true).unwrap();
    listen_failing(&ac, ETIMEDOUT, &success);

    ac.set_socks5_proxy("www.baidu.com", 80, "", "");

    assert_eq!(ac.connect("www.baidu.com", 80, 1.0, 0), SW_OK);

    event_wait();
}

fn sleep() {
    event_init();

    let buf = RefCell::new(Vec::with_capacity(65536));
    let domain = TEST_HTTP_DOMAIN;

    let client = Client::new(SW_SOCK_TCP, true).unwrap();
    client.on_connect(|cli| {
        cli.sleep();
        let cli = std::ptr::from_ref(cli);
        timer_after(200, move || {
            // SAFETY: the client outlives the loop that runs this timer.
            let cli = unsafe { &*cli };
            let req = http_get_request(domain, "/");
            cli.send(&req, 0);
            cli.wakeup();
        });
    });
    client.on_error(|_| {});
    client.on_close(|_| {});
    client.on_receive(|_, data| buf.borrow_mut().extend_from_slice(data));
    client.listen();

    assert_eq!(client.connect(domain.to_str().unwrap(), 80, -1.0, 0), 0);

    event_wait();

    assert!(contains(&buf.borrow(), TEST_HTTP_EXPECT));
}

fn sleep_2() {
    // __LINE__ + TEST_PORT, upstream's line.
    let port = 385 + TEST_PORT;
    let server_pid = tcp_server(TEST_HOST, port, |_| {
        // SAFETY: usleep(3).
        unsafe { usleep(10000) };
        Vec::new()
    });
    assert!(server_pid > 0);

    let counter = [Cell::new(0), Cell::new(0)];
    event_init();

    let mut wbuf = vec![0_u8; 8 * 1024 * 1024];
    random_bytes(&mut wbuf);

    let client = Client::new(SW_SOCK_TCP, true).unwrap();
    client.set_watermarks(1024 * 1024, 32 * 1024);

    client.on_buffer_full(|_| {
        eprintln!("Buffer is full, waiting for data to be sent...");
        counter[0].set(counter[0].get() + 1);
    });
    client.on_buffer_empty(|cli| {
        eprintln!("Buffer is empty, ready to send more data...");
        counter[1].set(counter[1].get() + 1);
        let cli = std::ptr::from_ref(cli);
        timer_after(200, move || {
            // SAFETY: the client outlives the loop that runs this timer.
            unsafe { &*cli }.close();
            eprintln!("Client closed, terminating server...");
            // SAFETY: the server child.
            unsafe { kill(server_pid, SIGTERM) };
        });
    });
    client.on_connect(|cli| {
        eprintln!("Client connected, sending data...");
        assert_eq!(cli.send(&wbuf, 0), wbuf.len() as isize);
        assert_eq!(cli.send(&wbuf, 0), -1);
        assert_eq!(last_error(), SW_ERROR_OUTPUT_BUFFER_OVERFLOW);
        let cli = std::ptr::from_ref(cli);
        timer_after(10, move || {
            // SAFETY: the client outlives the loop that runs this timer.
            unsafe { &*cli }.sleep();
            eprintln!("Client is sleeping...");
            timer_after(15, move || {
                // SAFETY: as above.
                unsafe { &*cli }.wakeup();
                eprintln!("Client woke up, closing connection...");
            });
        });
    });
    client.on_error(|_| eprintln!("Client error occurred, ERROR: {}", last_error()));
    client.on_close(|_| eprintln!("Client connection closed."));
    client.on_receive(|_, data| eprintln!("Client received data, length: {}", data.len()));
    client.listen();

    assert_eq!(client.connect(TEST_HOST, port, -1.0, 0), 0);

    event_wait();

    // wait_all_child_processes
    let mut status = 0;
    // SAFETY: reaps the server child.
    while unsafe { waitpid(-1, &raw mut status, 0) } > 0 {}

    assert!(counter[0].get() >= 1);
    assert!(counter[1].get() >= 1);
}

fn connect_refuse() {
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    assert_eq!(cli.connect(TEST_HOST, get_random_port(), -1.0, 0), -1);
    assert_eq!(last_error(), ECONNREFUSED);
}

fn bind() {
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    assert_eq!(cli.bind("127.0.0.1", 9999), SW_OK);
    assert_eq!(cli.bind("192.0.0.1", 9999), SW_ERR);
    assert_eq!(last_error(), EADDRNOTAVAIL);
    assert_eq!(cli.bind("127.0.0.1", 80), SW_ERR);
    if is_github_ci() {
        assert_eq!(last_error(), EINVAL);
    } else {
        assert_eq!(last_error(), EACCES);
    }
}

/// `encodeDomainName`: the domain's labels, each length-prefixed, then the root.
fn encode_domain_name(domain: &str) -> Vec<u8> {
    let mut result = Vec::new();
    for label in domain.split('.').filter(|label| !label.is_empty()) {
        result.push(u8::try_from(label.len()).unwrap());
        result.extend_from_slice(label.as_bytes());
    }
    result.push(0);
    result
}

/// `buildDNSQuery(domain)`: one recursive A question under a random id.
fn build_dns_query(domain: &str) -> Vec<u8> {
    let mut id = [0_u8; 2];
    random_bytes(&mut id);
    let mut query = id.to_vec();
    for field in [0x0100_u16, 1, 0, 0, 0] {
        query.extend_from_slice(&field.to_be_bytes());
    }
    query.extend(encode_domain_name(domain));
    query.extend_from_slice(&1_u16.to_be_bytes());
    query.extend_from_slice(&1_u16.to_be_bytes());
    query
}

/// `swoole_get_dns_server()`: the configured server, loading resolv.conf when unset.
fn dns_server() -> (String, c_int) {
    let read = || {
        let (host, port) = swoole_rs_clients::dns::server();
        let host = CStr::from_bytes_until_nul(&host).unwrap();
        (host.to_string_lossy().into_owned(), port)
    };
    if read().0.is_empty() {
        let _ = swoole_rs_clients::dns::load_resolv_conf(SW_DNS_RESOLV_CONF);
    }
    read()
}

fn sendto() {
    let test_str = TEST_STR;
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    assert_eq!(cli.sendto("127.0.0.1", 9999, test_str), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_OPERATION_NOT_SUPPORT);

    let (dns_host, dns_port) = dns_server();
    let dsock = Client::new(SW_SOCK_UDP, false).unwrap();
    let dns_query = build_dns_query("www.baidu.com");
    assert_eq!(dsock.sendto(&dns_host, dns_port, &dns_query), SW_OK);
    let mut buf = vec![0_u8; SW_BUFFER_SIZE_BIG];
    assert!(dsock.recv(&mut buf, 0) > 0);

    let (result, host, port) = dsock.get_peer_name();
    assert_eq!(result, SW_OK);
    assert_eq!(host, dns_host);
    assert_eq!(port, dns_port);

    let cli2 = Client::new(SW_SOCK_UDP, false).unwrap();
    assert_eq!(
        cli2.sendto("www.baidu.com-not-exists", 9999, test_str),
        SW_ERR
    );
    assert_eq!(last_error(), SW_ERROR_DNSLOOKUP_RESOLVE_FAILED);

    let cli3 = Client::new(SW_SOCK_UNIX_DGRAM, false).unwrap();
    assert_eq!(cli3.sendto("/tmp/swoole.sock", 0, test_str), SW_ERR);
    assert_eq!(last_error(), ENOENT);
}

/// AsyncClient's four callbacks, sending GREETER on connect and checking the echo.
fn listen_greeter<'a>(ac: &Client<'a>, flags: &'a Flags, error: Option<c_int>) {
    ac.on_connect(move |ac| {
        flags.connect.set(true);
        ac.send(GREETER, 0);
    });
    ac.on_close(move |_| flags.close.set(true));
    ac.on_error(move |_| {
        flags.error.set(true);
        if let Some(error) = error {
            assert_eq!(last_error(), error);
        }
    });
    ac.on_receive(move |ac, data| {
        flags.receive.set(true);
        assert_eq!(data.len(), GREETER_SIZE);
        assert_eq!(c_str(data), c_str(GREETER));
        ac.sync_close();
    });
}

/// The cases' `flags` map: which callbacks ran.
#[derive(Default)]
struct Flags {
    connect: Cell<bool>,
    receive: Cell<bool>,
    close: Cell<bool>,
    error: Cell<bool>,
}

fn async_unix_connect_refuse() {
    event_init();

    let flags = Flags::default();
    let ac = Client::wrapped(SW_SOCK_UNIX_DGRAM, true);
    listen_greeter(&ac, &flags, None);

    let retval = ac.sync_connect("/tmp/swoole-not-exists.sock", 0, -1.0);

    assert!(!retval);
    assert!(flags.error.get());
    assert_eq!(errno(), ENOENT);

    event_wait();
}

fn async_connect_timeout() {
    event_init();

    let flags = Flags::default();
    let ac = Client::wrapped(SW_SOCK_TCP, true);
    listen_greeter(&ac, &flags, Some(ETIMEDOUT));

    let (_peer, port) = unanswering_peer();
    assert!(ac.sync_connect(TEST_HOST, port, 0.2));
    event_wait();

    assert!(flags.error.get());
}

fn test_async_client_dgram(host: &str, port: c_int, kind: c_int) {
    let pid = dgram_server(host, port, kind);

    event_init();

    let flags = Flags::default();
    let ac = Client::wrapped(kind, true);
    listen_greeter(&ac, &flags, Some(ETIMEDOUT));

    assert!(ac.sync_connect(host, port, 0.2));
    event_wait();

    stop(pid);

    assert!(flags.connect.get());
    assert!(flags.receive.get());
    assert!(flags.close.get());
    assert!(!flags.error.get());
}

fn async_udp() {
    test_async_client_dgram(TEST_HOST, get_random_port(), SW_SOCK_UDP);
}

fn async_udp_dns() {
    test_async_client_dgram("localhost", get_random_port(), SW_SOCK_UDP);
}

fn async_udp6() {
    test_async_client_dgram("::1", get_random_port(), SW_SOCK_UDP6);
}

/// A local peer that never answers a SYN, in place of the address upstream assumes
/// drops it (19.168.0.99, 192.168.1.199): a network that rejects the route answers
/// ENETUNREACH instead. A listener whose accept queue is full: the kernel drops each
/// further SYN, so a connect to it times out. Hold the result until the connect is done.
fn unanswering_peer() -> ((std::net::TcpListener, Vec<std::net::TcpStream>), c_int) {
    use std::os::fd::AsRawFd;
    let listener = std::net::TcpListener::bind((TEST_HOST, 0)).expect("bind");
    // SAFETY: listen(2) again on a listening socket only shrinks its backlog.
    assert_eq!(unsafe { listen(listener.as_raw_fd(), 0) }, 0);
    let address = listener.local_addr().unwrap();
    let mut held = Vec::new();
    loop {
        match std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(200))
        {
            Ok(stream) => held.push(stream),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
            Err(error) => panic!("filling the accept queue: {error}"),
        }
        assert!(held.len() < 16, "the accept queue never filled");
    }
    ((listener, held), c_int::from(address.port()))
}

fn connect_timeout() {
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    let (_peer, port) = unanswering_peer();
    assert_eq!(cli.connect(TEST_HOST, port, 0.2, 0), -1);
    assert_eq!(last_error(), ETIMEDOUT);
}

fn ignore_sigpipe() {
    // SAFETY: signal(2) with SIG_IGN.
    unsafe { signal(SIGPIPE, SIG_IGN) };
}

fn shutdown_write() {
    ignore_sigpipe();
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    assert_eq!(cli.connect("www.baidu.com", 80, -1.0, 0), 0);

    assert_eq!(cli.shutdown(SHUT_WR), 0);
    assert_eq!(cli.shutdown(SHUT_WR), SW_ERR); // already shutdown

    assert_eq!(cli.send(b"hello world", 0), -1);
    assert_eq!(last_error(), EPIPE);
}

fn shutdown_read() {
    ignore_sigpipe();
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    assert_eq!(cli.connect("www.baidu.com", 80, -1.0, 0), 0);

    assert_eq!(cli.shutdown(SHUT_RD), SW_OK);
    assert_eq!(cli.shutdown(SHUT_RD), SW_ERR); // already shutdown

    assert!(cli.send(b"hello world\r\n\r\n", 0) > 0);

    let mut buf = [0_u8; 1024];
    assert_eq!(cli.recv(&mut buf, 0), 0);
}

fn shutdown_all() {
    ignore_sigpipe();
    let cli = Client::new(SW_SOCK_TCP, false).unwrap();
    assert_eq!(cli.connect("www.baidu.com", 80, -1.0, 0), 0);

    assert_eq!(cli.shutdown(SHUT_RDWR), SW_OK);
    assert_eq!(cli.shutdown(SHUT_RDWR + 99), SW_ERR);
    assert_eq!(last_error(), EINVAL);

    assert_eq!(cli.send(b"hello world\r\n\r\n", 0), -1);
    assert_eq!(last_error(), EPIPE);

    let mut buf = [0_u8; 1024];
    assert_eq!(cli.recv(&mut buf, 0), 0);
}

/// The async HTTP(S) cases' callbacks: `on_connect` sends, the body collects in `buf`.
fn listen_http<'a>(
    client: &Client<'a>,
    connected: &'a Cell<bool>,
    closed: &'a Cell<bool>,
    buf: &'a RefCell<Vec<u8>>,
    on_connect: impl Fn(&Client<'a>) + 'a,
) {
    client.on_connect(move |cli| {
        connected.set(true);
        on_connect(cli);
    });
    client.on_error(|_| {});
    client.on_close(move |_| closed.set(true));
    client.on_receive(move |_, data| buf.borrow_mut().extend_from_slice(data));
    client.listen();
}

fn ssl_1() {
    let connected = Cell::new(false);
    let closed = Cell::new(false);
    let buf = RefCell::new(Vec::with_capacity(65536));

    event_init();

    let client = Client::new(SW_SOCK_TCP, true).unwrap();

    assert_eq!(client.enable_ssl_encrypt(), SW_OK);
    assert_eq!(client.enable_ssl_encrypt(), SW_ERR); // already enabled

    listen_http(&client, &connected, &closed, &buf, |cli| {
        let req = http_get_request(TEST_HTTP_DOMAIN, "/");
        cli.send(&req, 0);
    });

    assert_eq!(
        client.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 443, -1.0, 0),
        0
    );

    event_wait();

    assert!(connected.get());
    assert!(closed.get());
    assert!(contains(&buf.borrow(), TEST_HTTPS_EXPECT));
}

/// `swoole::make_tmpfile()` holding TEST_REQUEST_BAIDU: its path and size.
fn request_file() -> (String, usize) {
    let path = format!("/tmp/swoole-core-tests-client-{}", std::process::id());
    std::fs::write(&path, TEST_REQUEST_BAIDU).unwrap();
    (path, TEST_REQUEST_BAIDU.len())
}

fn ssl_sendfile() {
    let connected = Cell::new(false);
    let closed = Cell::new(false);
    let buf = RefCell::new(Vec::with_capacity(65536));

    event_init();

    let (file, size) = request_file();

    let client = Client::new(SW_SOCK_TCP, true).unwrap();
    client.enable_ssl_encrypt();
    listen_http(&client, &connected, &closed, &buf, |cli| {
        cli.sendfile(&file, 0, size);
    });

    assert_eq!(client.connect(TEST_DOMAIN_BAIDU, 443, -1.0, 0), 0);

    event_wait();

    assert!(connected.get());
    assert!(closed.get());
    assert!(contains(&buf.borrow(), b"Baidu"));
}

fn sync_ssl_sendfile() {
    let (file, _) = request_file();

    let client = Client::wrapped(SW_SOCK_TCP, false);
    assert!(client.sync_connect(TEST_DOMAIN_BAIDU, 443, -1.0));
    assert!(client.sync_enable_ssl_encrypt());
    assert_eq!(client.sendfile(&file, 0, 0), SW_OK);

    // As upstream: each read lands at the buffer's start, and grows its length.
    let mut buf = vec![0_u8; 65536];
    let mut length = 0;
    loop {
        let size = buf.len() - length;
        let nr = client.recv(&mut buf[..size], 0);
        if nr <= 0 {
            break;
        }
        length += nr as usize;
    }
    client.sync_close();
    assert!(contains(&buf[..length], b"baidu.com"));
    let _ = std::fs::remove_file(&file);
}

/// `Client client(SW_SOCK_TCP, true)`, its proxy set, then `proxy_async_test(client, https)`.
fn proxy_async_test(set_proxy: fn(&Client<'_>), https: bool) {
    let connected = Cell::new(false);
    let closed = Cell::new(false);
    let buf = RefCell::new(Vec::with_capacity(65536));

    let client = Client::new(SW_SOCK_TCP, true).unwrap();
    set_proxy(&client);

    event_init();

    if https {
        client.enable_ssl_encrypt();
    }

    listen_http(&client, &connected, &closed, &buf, |cli| {
        cli.send(TEST_REQUEST_BAIDU, 0);
    });

    assert_eq!(
        client.connect(TEST_DOMAIN_BAIDU, if https { 443 } else { 80 }, -1.0, 0),
        0
    );

    event_wait();

    assert!(connected.get());
    assert!(closed.get());
    assert!(contains(&buf.borrow(), b"www.baidu.com"));
}

fn proxy_sync_test(client: &Client<'_>, https: bool) {
    let mut buf = Vec::with_capacity(65536);
    if https {
        client.enable_ssl_encrypt();
    }

    let mut host = TEST_DOMAIN_BAIDU.to_owned();
    if client
        .socks5_dns_tunnel()
        .is_some_and(|tunnel| *tunnel == 0)
    {
        let name = cstring(TEST_DOMAIN_BAIDU);
        if let Some(addr) = swoole_rs_corebase::gethostbyname(AF_INET, &name) {
            let addr = addr.map(|byte| byte.to_ne_bytes()[0]);
            host = std::net::Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3]).to_string();
        } else {
            host.clear();
        }
        eprintln!("Resolved domain {TEST_DOMAIN_BAIDU} to {host}");
    }

    assert_eq!(
        client.connect(&host, if https { 443 } else { 80 }, -1.0, 0),
        0
    );
    assert!(client.send(TEST_REQUEST_BAIDU, 0) > 0);

    loop {
        let mut rbuf = [0_u8; 4096];
        let nr = client.recv(&mut rbuf, 0);
        if nr <= 0 {
            break;
        }
        buf.extend_from_slice(&rbuf[..nr as usize]);
    }

    assert!(contains(&buf, b"www.baidu.com"));
}

fn proxy_set_socks5_proxy(client: &Client<'_>) {
    client.set_socks5_proxy(
        TEST_SOCKS5_PROXY_HOST,
        TEST_SOCKS5_PROXY_PORT,
        TEST_SOCKS5_PROXY_USER,
        TEST_SOCKS5_PROXY_PASSWORD,
    );
}

fn proxy_set_http_proxy(client: &Client<'_>) {
    let (username, password) = if is_github_ci() {
        (TEST_HTTP_PROXY_USER, TEST_HTTP_PROXY_PASSWORD)
    } else {
        ("", "")
    };
    client.set_http_proxy(
        TEST_HTTP_PROXY_HOST,
        TEST_HTTP_PROXY_PORT,
        username,
        password,
    );
}

fn https_get_async_with_http_proxy() {
    proxy_async_test(proxy_set_http_proxy, true);
}

fn https_get_async_with_socks5_proxy() {
    proxy_async_test(proxy_set_socks5_proxy, true);
}

fn https_get_sync_with_http_proxy() {
    let client = Client::new(SW_SOCK_TCP, false).unwrap();
    proxy_set_http_proxy(&client);
    proxy_sync_test(&client, true);
}

fn https_get_sync_with_socks5_proxy() {
    let client = Client::new(SW_SOCK_TCP, false).unwrap();
    proxy_set_socks5_proxy(&client);
    proxy_sync_test(&client, true);
}

fn http_get_sync_with_socks5_proxy_no_dns_tunnel() {
    let client = Client::new(SW_SOCK_TCP, false).unwrap();
    proxy_set_socks5_proxy(&client);
    *client.socks5_dns_tunnel().unwrap() = 0;
    proxy_sync_test(&client, false);
}

fn http_get_async_with_http_proxy() {
    proxy_async_test(proxy_set_http_proxy, false);
}

fn http_get_async_with_socks5_proxy() {
    proxy_async_test(proxy_set_socks5_proxy, false);
}

fn http_get_sync_with_http_proxy() {
    let client = Client::new(SW_SOCK_TCP, false).unwrap();
    proxy_set_http_proxy(&client);
    proxy_sync_test(&client, false);
}

fn http_get_sync_with_socks5_proxy() {
    let client = Client::new(SW_SOCK_TCP, false).unwrap();
    proxy_set_socks5_proxy(&client);
    proxy_sync_test(&client, false);
}

/// A `swoole::String` of `size` bytes, as `sw_tg_buffer()` is.
fn cpp_string(size: usize) -> CppString {
    let mut string = std::mem::MaybeUninit::<CppString>::uninit();
    // SAFETY: constructs into the local storage with the default allocator.
    unsafe {
        string_new(string.as_mut_ptr(), size, std::ptr::null());
        string.assume_init()
    }
}

/// `ssl_readv` or `ssl_writev` (when `write`) on `sock` over `IOVector(iov, count)`,
/// destroyed after: crates/hooks' vector, which is both.
fn ssl_vector(sock: *mut c_void, iov: &[IoVec], write: bool) -> isize {
    use swoole_rs_hooks::vector::{IoVector, Vectors};
    let (at, count) = (iov.as_ptr(), iov.len());
    // SAFETY: `IoVec` is `struct iovec` and so an `IoSlice`, and `iov` a live array of
    // them over buffers nothing else touches meanwhile.
    let vectors = if write {
        Vectors::Write(unsafe { std::slice::from_raw_parts(at.cast(), count) })
    } else {
        Vectors::Read(unsafe { std::slice::from_raw_parts_mut(at.cast_mut().cast(), count) })
    };
    // SAFETY: the connected socket.
    let sock = unsafe { swoole_rs_reactor::Socket::from_ptr(sock.cast()) }.unwrap();
    let vector = IoVector::new(vectors);
    vector.expect("IOVector(iov, count)").transfer(sock)
}

/// `Socket::ssl_get_peer_cert_chain(limit)`, inline C++ upstream: the PEM of each of
/// the first `limit` certificates that reads.
fn peer_cert_chain(sock: swoole_rs_reactor::Socket<'_>, limit: usize) -> Vec<Vec<u8>> {
    let mut chain = Vec::new();
    let mut buffer = vec![0_u8; SW_BUFFER_SIZE_BIG];
    if let Some(ssl) = swoole_rs_tls::Ssl::of(sock) {
        ssl.each_peer_cert(|cert| {
            // `_ssl_read_x509_file`, which fails without a certificate.
            let rv = cert.pem(&mut buffer).ok();
            chain.push(rv.filter(|&n| n > 0).map(|n| buffer[..n].to_vec()));
        });
    }
    chain.into_iter().take(limit).flatten().collect()
}

fn ssl() {
    let client = Client::new(SW_SOCK_TCP, false).unwrap();
    client.enable_ssl_encrypt();
    client.set_tls_host_name(TEST_HTTP_DOMAIN.to_str().unwrap());
    assert_eq!(
        client.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 443, -1.0, 0),
        SW_OK
    );

    let sock = client.socket();
    let mut certificate = cpp_string(SW_BUFFER_SIZE_BIG);
    // SAFETY: a live `swoole::String`, which is crates/buffer's.
    let string = unsafe { &mut *(&raw mut certificate).cast() };
    // SAFETY: the connected socket.
    let handle = unsafe { swoole_rs_reactor::Socket::from_ptr(sock.cast()) }.unwrap();
    assert!(swoole_rs_tls::upstream::socket_ssl_peer_certificate_string(
        handle, string
    ));
    let ls = peer_cert_chain(handle, 10);
    assert!(!ls.is_empty());
    assert_eq!(client.ssl_verify(false), SW_OK);

    let mut req = http_get_request(TEST_HTTP_DOMAIN, "/");

    let offset1 = TEST_WRITEV_OFFSET;
    let (head, tail) = req.split_at_mut(offset1);
    let wr_iov = [
        IoVec {
            base: head.as_mut_ptr().cast(),
            len: head.len(),
        },
        IoVec {
            base: tail.as_mut_ptr().cast(),
            len: tail.len(),
        },
    ];
    let written = ssl_vector(sock, &wr_iov, true);
    assert_eq!(written, req.len() as isize);

    let mut buf = vec![0_u8; 1024 * 1024];
    let offset2 = TEST_READV_OFFSET;
    let (head, tail) = buf.split_at_mut(offset2);
    let rd_iov = [
        IoVec {
            base: head.as_mut_ptr().cast(),
            len: head.len(),
        },
        IoVec {
            base: tail.as_mut_ptr().cast(),
            len: tail.len(),
        },
    ];
    let rv = ssl_vector(sock, &rd_iov, false);
    assert!(rv > 1024);

    assert!(contains(&buf[..rv as usize], TEST_HTTPS_EXPECT));
}

fn fail() {
    let c = Client::new(SW_SOCK_RAW6 + 1, false);
    assert!(c.is_none());
    assert_eq!(last_error(), ESOCKTNOSUPPORT);
}

static SIGNALS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn count_signal(_: c_int) {
    SIGNALS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

fn test_recv_timeout(c: &Client<'_>) {
    // SAFETY: getpid(2).
    let pid = unsafe { getpid() };
    let t = std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_micros(50000));
            // SAFETY: signals this process.
            unsafe { kill(pid, SIGIO) };
        }
    });

    // SAFETY: a plain handler, not restarting, masked while it runs.
    unsafe { swoole_signal_set(SIGIO, count_signal, 0, 1) };

    let mut buf = vec![0_u8; SW_BUFFER_SIZE_BIG];
    loop {
        let rv = c.recv(&mut buf, 0);
        eprintln!("rv: {rv}, error={}", errno());
        if c.has_timedout() {
            break;
        }
    }

    t.join().unwrap();
}

fn recv_timeout() {
    let c = Client::new(SW_SOCK_TCP, false).expect("c.ready()");
    assert_eq!(
        c.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 80, 1.0, 0),
        SW_OK
    );
    test_recv_timeout(&c);
}

fn ssl_recv_timeout() {
    let c = Client::new(SW_SOCK_TCP, false).expect("c.ready()");
    c.enable_ssl_encrypt();

    assert_eq!(
        c.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 443, 1.0, 0),
        SW_OK
    );
    test_recv_timeout(&c);
}

fn sync_nonblock() {
    let cli = Client::new(SW_SOCK_TCP, false).expect("cli.socket");
    assert_eq!(
        cli.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 80, -1.0, 1),
        -1
    );
    assert_eq!(errno(), EINPROGRESS);
    assert!(cli.async_connect());
}

pub(super) const CASES: super::Cases = &[
    ("tcp", tcp),
    ("udp", udp),
    ("udp6", udp6),
    ("udg", udg),
    ("async_tcp", async_tcp),
    ("async_tcp_dns", async_tcp_dns),
    ("async_tcp6", async_tcp6),
    ("async_tcp6_dns", async_tcp6_dns),
    ("async_tcp_dns_fail", async_tcp_dns_fail),
    ("async_tcp_ssl_handshake_fail", async_tcp_ssl_handshake_fail),
    (
        "async_tcp_http_proxy_handshake_fail",
        async_tcp_http_proxy_handshake_fail,
    ),
    (
        "async_tcp_socks5_proxy_handshake_fail",
        async_tcp_socks5_proxy_handshake_fail,
    ),
    ("sleep", sleep),
    ("sleep_2", sleep_2),
    ("connect_refuse", connect_refuse),
    ("bind", bind),
    ("sendto", sendto),
    ("async_unix_connect_refuse", async_unix_connect_refuse),
    ("async_connect_timeout", async_connect_timeout),
    ("async_udp", async_udp),
    ("async_udp_dns", async_udp_dns),
    ("async_udp6", async_udp6),
    ("connect_timeout", connect_timeout),
    ("shutdown_write", shutdown_write),
    ("shutdown_read", shutdown_read),
    ("shutdown_all", shutdown_all),
    ("ssl_1", ssl_1),
    ("ssl_sendfile", ssl_sendfile),
    ("sync_ssl_sendfile", sync_ssl_sendfile),
    (
        "https_get_async_with_http_proxy",
        https_get_async_with_http_proxy,
    ),
    (
        "https_get_async_with_socks5_proxy",
        https_get_async_with_socks5_proxy,
    ),
    (
        "https_get_sync_with_http_proxy",
        https_get_sync_with_http_proxy,
    ),
    (
        "https_get_sync_with_socks5_proxy",
        https_get_sync_with_socks5_proxy,
    ),
    (
        "http_get_sync_with_socks5_proxy_no_dns_tunnel",
        http_get_sync_with_socks5_proxy_no_dns_tunnel,
    ),
    (
        "http_get_async_with_http_proxy",
        http_get_async_with_http_proxy,
    ),
    (
        "http_get_async_with_socks5_proxy",
        http_get_async_with_socks5_proxy,
    ),
    (
        "http_get_sync_with_http_proxy",
        http_get_sync_with_http_proxy,
    ),
    (
        "http_get_sync_with_socks5_proxy",
        http_get_sync_with_socks5_proxy,
    ),
    ("ssl", ssl),
    ("fail", fail),
    ("recv_timeout", recv_timeout),
    ("ssl_recv_timeout", ssl_recv_timeout),
    ("sync_nonblock", sync_nonblock),
];
