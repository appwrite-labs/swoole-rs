//! Upstream's `core-tests/src/coroutine/socket.cpp`, one function per TEST with the
//! same inputs and expected values, over the hooks crate's coroutine socket.
//!
//! A `Socket` is the hooks crate's `Co`, whose methods the cases call (`Sock::co`).
//! Fields are read through the record layout (native/hooks/swoole_rs_co_socket.h).
//! The TLS setters write the socket's `SSLContext` owner (crates/tls/src/owned.rs) as
//! `enable_ssl_encrypt` and the `set_ssl_*` members did; `TimeoutSetter` and
//! `TimeoutController` are the crate's `TimeoutScope` and its `timed_out`.
//! The two `Server` fixtures (`recv_success`, `recv_fail`) are a forked std echo
//! and close server: the cases assert only on the client.

use super::{
    _exit, AF_INET, AF_INET6, AF_UNIX, EBADF, ENOENT, ETIMEDOUT, SO_RCVBUF, SO_SNDBUF, SOCK_STREAM,
    SOL_SOCKET, SW_EVENT_READ, SW_EVENT_WRITE, TEST_HTTP_DOMAIN, TEST_STR, co_sleep, fork, go,
    kill, last_error, random_bytes, run, setsockopt, sleep, socketpair,
};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{Read, Write};
use std::mem::ManuallyDrop;
use std::ptr;
use swoole_rs_hooks::co_socket::{self, Co};
use swoole_rs_hooks::{TimeoutScope, co_io, packet, vector::IoVector};

// glibc's values (the core-tests build is Linux).
pub(super) const ECONNREFUSED: c_int = 111;
pub(super) const EADDRNOTAVAIL: c_int = 99;
const ENOTCONN: c_int = 107;
const ECANCELED: c_int = 125;
pub(super) const SIGTERM: c_int = 15;
pub(super) const SHUT_RD: c_int = 0;
pub(super) const SHUT_WR: c_int = 1;
pub(super) const SHUT_RDWR: c_int = 2;

pub(super) const SW_SOCK_TCP: c_int = 1;
pub(super) const SW_SOCK_UDP: c_int = 2;
pub(super) const SW_SOCK_TCP6: c_int = 3;
pub(super) const SW_SOCK_UDP6: c_int = 4;
const SW_SOCK_UNIX_STREAM: c_int = 5;
pub(super) const SW_SOCK_UNIX_DGRAM: c_int = 6;
const SW_TIMEOUT_DNS: c_int = 1;
const SW_TIMEOUT_CONNECT: c_int = 2;
const SW_TIMEOUT_READ: c_int = 4;
const SW_TIMEOUT_WRITE: c_int = 8;
const SW_TIMEOUT_RDWR: c_int = SW_TIMEOUT_READ | SW_TIMEOUT_WRITE;
const SW_TIMEOUT_ALL: c_int = SW_TIMEOUT_DNS | SW_TIMEOUT_CONNECT | SW_TIMEOUT_RDWR;
const SW_ERROR_PACKAGE_LENGTH_TOO_LARGE: c_int = 1201;
const SW_ERROR_CO_SOCKET_CLOSE_WAIT: c_int = 10019;
const SW_STACK_BUFFER_SIZE: usize = 65536;

pub(super) const TEST_HOST: &str = "127.0.0.1";
pub(super) const TEST_PORT: c_int = 9501;
const HOST: &str = "www.baidu.com";
const CRLF: &[u8] = b"\r\n";
const EOF_PACKET: &[u8] = b"hello world\r\n";
const EOF_PACKET_2: &[u8] = b"php&swoole, java&golang\r\n";
const RECV_TIMEOUT: f64 = 10.0;
pub(super) const TEST_HTTP_EXPECT: &[u8] = b"Location: https://www.gov.cn/";
pub(super) const TEST_HTTPS_EXPECT: &[u8] = "中国政府网".as_bytes();
const TEST_HTTP_PROXY_HOST: &str = "127.0.0.1";
const TEST_HTTP_PROXY_PORT: c_int = 8888;
const TEST_HTTP_PROXY_USER: &str = "user";
const TEST_HTTP_PROXY_PASSWORD: &str = "password";
const TEST_SOCKS5_PROXY_HOST: &str = "127.0.0.1";
const TEST_SOCKS5_PROXY_PORT: c_int = 8080;
const TEST_SOCKS5_PROXY_NO_AUTH_PORT: c_int = 8081;
const TEST_SOCKS5_PROXY_USER: &str = "user";
const TEST_SOCKS5_PROXY_PASSWORD: &str = "password";
pub(super) const TEST_REQUEST_BAIDU: &[u8] = b"GET / HTTP/1.1\r\n\
Host: www.baidu.com\r\n\
Connection: close\r\n\
User-Agent: Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) \
Chrome/51.0.2704.106 Safari/537.36\r\n\r\n";

/// `swoole::Protocol`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Protocol {
    split_by_eof: bool,
    package_eof: [u8; 8],
    package_eof_len: u8,
    package_length_type: u8,
    package_length_size: u8,
    package_length_offset: u16,
    package_body_offset: u16,
    package_max_length: u32,
    private_data_1: *mut c_void,
    private_data_2: *mut c_void,
    on_package: *mut c_void,
    get_package_length: *const c_void,
    get_package_length_size: *mut c_void,
}

/// `swr_v1_co_socket` (native/hooks/swoole_rs_co_socket.h), which a `Socket` is.
#[repr(C)]
struct Record {
    err_code: c_int,
    err_msg: *const c_char,
    open_length_check: bool,
    open_eof_check: bool,
    http2: bool,
    protocol: Protocol,
    kind: c_int,
    socket: *mut NetHead,
    sock_domain: c_int,
    sock_type: c_int,
    sock_protocol: c_int,
    sock_fd: c_int,
    connect_port: c_int,
    backlog: c_int,
    buffer_allocator: *const c_void,
    buffer_init_size: usize,
    read_buffer: *mut CppString,
    write_buffer: *mut CppString,
    ssl_is_server: bool,
    ssl_handshaked: bool,
    zero_copy: bool,
    resolve_context: *mut c_void,
    state: *mut c_void,
    extra: *mut c_void,
}

const _: () = assert!(
    size_of::<Record>() == 192,
    "swr_v1_co_socket's layout moved"
);

/// The head of `network::Socket` (crates/reactor's `SocketRecord`) up to `event_hup`.
#[repr(C)]
struct NetHead {
    fd: c_int,
    fd_type: c_int,
    socket_type: c_int,
    events: c_int,
    flags: [u8; 17],
    event_hup: u8,
}

/// `swoole::String`.
#[repr(C)]
struct CppString {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut u8,
    allocator: *const c_void,
}

/// `OwnedTlsPrepared` (crates/tls/src/owned.rs).
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct TlsPrepared {
    pub(super) owner: *const c_void,
    pub(super) fields: *mut TlsFields,
    pub(super) strings: [*const c_void; 11],
}

/// `OwnedTlsFields`, up to `allow_self_signed`.
#[repr(C)]
pub(super) struct TlsFields {
    http: u8,
    http_v2: u8,
    prefer_server_ciphers: u8,
    session_tickets: u8,
    stapling: u8,
    stapling_verify: u8,
    pub(super) disable_tls_host_name: u8,
    grease: u8,
    verify_depth: u8,
    disable_compress: u8,
    verify_peer: u8,
    allow_self_signed: u8,
}

/// The owner's string slots (crates/tls/src/owned.rs).
const TLS_ECDH_CURVE: usize = 1;
const TLS_DHPARAM: usize = 3;
const TLS_CERT_FILE: u32 = 4;
const TLS_KEY_FILE: u32 = 5;
pub(super) const TLS_HOST_NAME: usize = 8;
const TLS_CAFILE: usize = 9;

#[repr(C)]
pub(super) struct IoVec {
    pub(super) base: *mut c_void,
    pub(super) len: usize,
}

#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    namelen: u32,
    iov: *mut IoVec,
    iovlen: usize,
    control: *mut c_void,
    controllen: usize,
    flags: c_int,
}

#[repr(C)]
struct SockAddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

unsafe extern "C" {
    fn swr_v1_hook_co_socket_connect_host(
        socket: *mut Record,
        host: *const c_char,
        length: usize,
        port: c_int,
    ) -> bool;

    #[link_name = "_ZN6swoole9coroutine6Socket15get_read_bufferEv"]
    fn socket_get_read_buffer(this: *mut c_void) -> *mut CppString;
    #[link_name = "_ZN6swoole9coroutine6Socket16get_write_bufferEv"]
    fn socket_get_write_buffer(this: *mut c_void) -> *mut CppString;
    #[link_name = "_ZNK6swoole7network6Socket24ssl_get_peer_certificateEPcm"]
    fn network_peer_certificate(socket: *const NetHead, buf: *mut c_char, n: usize) -> c_int;

    pub(super) safe fn swr_v1_tls_owner_new() -> TlsPrepared;
    fn swr_v1_tls_owner_set_path(
        owner: *const c_void,
        index: u32,
        path: *const c_char,
        length: usize,
    ) -> c_int;
    pub(super) fn swr_v1_core_string_set(
        slot: *const c_void,
        bytes: *const c_char,
        length: usize,
    ) -> c_int;

    #[link_name = "_ZN6swoole6String3popEm"]
    fn string_pop(this: *mut CppString, init_size: usize) -> *mut u8;
    #[link_name = "_ZN6swoole8Protocol19default_length_funcEPKS0_PNS_7network6SocketEPNS_12PacketLengthE"]
    static default_length_func: c_void;
    #[link_name = "_Z16swoole_type_sizec"]
    fn swoole_type_size(kind: c_char) -> c_int;
    #[link_name = "_Z11swoole_randii"]
    fn swoole_rand(min: c_int, max: c_int) -> c_int;
    #[link_name = "_Z17swoole_random_intv"]
    fn swoole_random_int() -> u64;
}

unsafe extern "C" {
    fn wait(status: *mut c_int) -> c_int;
    fn usleep(usec: u32) -> c_int;
}

/// `microtime()`.
fn microtime() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

pub(super) fn get_random_port() -> c_int {
    // SAFETY: Swoole's random source.
    TEST_PORT + c_int::try_from(unsafe { swoole_random_int() } % 10000).unwrap()
}

pub(super) fn get_ssl_dir() -> String {
    std::env::current_dir().unwrap().display().to_string() + "/tests/include/ssl_certs"
}

pub(super) fn http_get_request(domain: &CStr, path: &str) -> Vec<u8> {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\
         User-Agent: Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/51.0.2704.106 Safari/537.36\r\n\r\n",
        domain.to_str().unwrap()
    )
    .into_bytes()
}

pub(super) fn is_github_ci() -> bool {
    std::env::var_os("GITHUB_ACTIONS").is_some()
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn cstring(text: &str) -> std::ffi::CString {
    std::ffi::CString::new(text).unwrap()
}

fn size(n: isize) -> usize {
    usize::try_from(n).unwrap()
}

/// A coroutine `Socket`, deleted on drop as a C++ scope would.
struct Sock {
    ptr: *mut c_void,
    /// The `SSLContext` `enable_ssl_encrypt` made: its fields and string slots.
    tls: Cell<Option<TlsPrepared>>,
}

impl Sock {
    fn make(fd: c_int, domain: c_int, kind: c_int, protocol: c_int, family: bool) -> Self {
        let socket = co_socket::make(fd, domain, kind, protocol, family).this();
        assert!(!socket.is_null());
        Self::wrap(socket)
    }

    fn wrap(ptr: *mut c_void) -> Self {
        Self {
            ptr,
            tls: Cell::new(None),
        }
    }

    /// `Socket(SocketType)`.
    fn new(kind: c_int) -> Self {
        Self::make(-1, 0, kind, 0, false)
    }

    /// `Socket(fd, SocketType)`.
    fn with_fd(fd: c_int, kind: c_int) -> Self {
        Self::make(fd, 0, kind, 0, false)
    }

    /// `Socket(fd, domain, type, protocol)`.
    fn with_family(fd: c_int, domain: c_int, kind: c_int, protocol: c_int) -> Self {
        Self::make(fd, domain, kind, protocol, true)
    }

    fn rec(&self) -> *mut Record {
        self.ptr.cast()
    }

    /// The crate's handle on this socket.
    fn co(&self) -> Co {
        // SAFETY: the live socket `make` or `accept` made, which outlives the handle's use.
        unsafe { Co::of(self.ptr) }
    }

    fn record(&self) -> &mut Record {
        // SAFETY: the live socket's record; no other Rust reference to it is held
        // across a call.
        unsafe { &mut *self.rec() }
    }

    fn err_code(&self) -> c_int {
        self.record().err_code
    }

    fn set_err_code(&self, code: c_int) {
        self.record().err_code = code;
    }

    fn connect(&self, host: &str, port: c_int, flags: c_int) -> bool {
        let _ = flags;
        // SAFETY: the live socket and `host`'s bytes.
        unsafe {
            swr_v1_hook_co_socket_connect_host(self.rec(), host.as_ptr().cast(), host.len(), port)
        }
    }

    fn bind(&self, address: &str, port: c_int) -> bool {
        self.co().bind(address.as_bytes(), port)
    }

    fn listen(&self, backlog: c_int) -> bool {
        self.co().listen(backlog)
    }

    /// `accept()`: the connection, which a C++ caller deletes or leaks.
    fn accept(&self) -> ManuallyDrop<Sock> {
        let conn = self.co().accept(0.0).map_or(ptr::null_mut(), Co::this);
        ManuallyDrop::new(Sock::wrap(conn))
    }

    fn send(&self, buf: &[u8]) -> isize {
        self.co().send(buf)
    }

    fn recv(&self, buf: &mut [u8]) -> isize {
        self.co().recv(buf)
    }

    fn send_all(&self, buf: &[u8]) -> isize {
        self.co().send_all(buf)
    }

    fn recv_all(&self, buf: *mut u8, n: usize) -> isize {
        // SAFETY: `buf` holds `n` writable bytes.
        self.co()
            .recv_all(unsafe { std::slice::from_raw_parts_mut(buf, n) })
    }

    fn write(&self, buf: &[u8]) -> isize {
        self.co().write(buf)
    }

    fn read(&self, buf: &mut [u8]) -> isize {
        self.co().read(buf)
    }

    fn recv_line(&self, buf: *mut u8, n: usize) -> isize {
        // SAFETY: `buf` holds `n` writable bytes and one more.
        unsafe { self.co().recv_line(buf.cast(), n) }
    }

    fn recv_packet(&self, timeout: f64) -> isize {
        packet::recv_packet(self.co(), timeout)
    }

    fn read_buffer(&self) -> &mut CppString {
        // SAFETY: the live socket's read buffer, which it keeps.
        unsafe { &mut *socket_get_read_buffer(self.ptr) }
    }

    fn write_buffer(&self) -> *mut CppString {
        // SAFETY: the live socket.
        unsafe { socket_get_write_buffer(self.ptr) }
    }

    /// `pop_packet()`: the packet's buffer, which the caller now owns, or null.
    fn pop_packet(&self) -> *mut u8 {
        let record = self.record();
        // SAFETY: the record's read buffer, set once a packet was received.
        let buffer = unsafe { &mut *record.read_buffer };
        if buffer.offset == 0 {
            return ptr::null_mut();
        }
        // SAFETY: the live buffer and the socket's initial buffer size.
        unsafe { string_pop(buffer, record.buffer_init_size) }
    }

    fn shutdown(&self, how: c_int) -> bool {
        self.co().shutdown(how)
    }

    fn close(&self) -> bool {
        self.co().close()
    }

    fn cancel(&self, event: c_int) -> bool {
        self.co().cancel(event.cast_unsigned())
    }

    fn is_closed(&self) -> bool {
        self.record().sock_fd == -1
    }

    fn check_liveness(&self) -> bool {
        self.co().check_liveness()
    }

    fn set_timeout(&self, timeout: f64, kind: c_int) {
        self.co().set_timeout(timeout, kind);
    }

    fn get_timeout(&self, kind: c_int) -> f64 {
        self.co().timeout(kind)
    }

    fn network(&self) -> &mut NetHead {
        // SAFETY: the open socket's network::Socket.
        unsafe { &mut *self.record().socket }
    }

    fn fd(&self) -> c_int {
        self.record().sock_fd
    }

    /// `get_socket()->set_option(SOL_SOCKET, name, size)`.
    fn set_buffer_option(&self, name: c_int, size: u32) {
        // SAFETY: an int option on the socket's descriptor.
        unsafe { setsockopt(self.fd(), SOL_SOCKET, name, (&raw const size).cast(), 4) };
    }

    fn set_send_buffer_size(&self, size: u32) {
        self.set_buffer_option(SO_SNDBUF, size);
    }

    fn set_recv_buffer_size(&self, size: u32) {
        self.set_buffer_option(SO_RCVBUF, size);
    }

    /// `set_eof_protocol`: the record's `open_eof_check` and EOF.
    fn set_eof_protocol(&self) {
        let record = self.record();
        record.protocol.package_eof[..2].copy_from_slice(CRLF);
        record.protocol.package_eof_len = 2;
        record.open_eof_check = true;
    }

    /// `socket_set_length_protocol_1/2`.
    fn set_length_protocol(&self, kind: u8, body_offset: u16, max_length: u32) {
        let record = self.record();
        // SAFETY: every field is an integer, a bool, a byte array or a raw pointer.
        record.protocol = unsafe { std::mem::zeroed() };
        record.protocol.package_length_type = kind;
        // SAFETY: a pure function of the type character.
        record.protocol.package_length_size =
            u8::try_from(unsafe { swoole_type_size(kind as c_char) }).unwrap();
        record.protocol.package_body_offset = body_offset;
        record.protocol.get_package_length = &raw const default_length_func;
        record.protocol.package_max_length = max_length;
        record.open_length_check = true;
    }

    fn set_socks5_proxy(&self, host: &str, port: c_int, user: &str, pwd: &str) {
        self.set_proxy(false, host, port, user, pwd);
    }

    fn set_http_proxy(&self, host: &str, port: c_int, user: &str, pwd: &str) {
        self.set_proxy(true, host, port, user, pwd);
    }

    fn set_proxy(&self, http: bool, host: &str, port: c_int, user: &str, pwd: &str) {
        let (host, user, pwd) = (host.as_bytes(), user.as_bytes(), pwd.as_bytes());
        self.co().set_proxy(http, host, port, user, pwd);
    }

    /// The socket's `SSLContext` owner slot, its extra record's first field
    /// (crates/hooks/src/socket_extra.rs).
    fn tls_slot(&self) -> *mut *const c_void {
        self.record().extra.cast()
    }

    fn tls_owner(&self) -> *const c_void {
        // SAFETY: the socket's live extra record's TLS slot.
        unsafe { *self.tls_slot() }
    }

    /// `enable_ssl_encrypt()`: a fresh `SSLContext` unless the socket has one.
    fn enable_ssl_encrypt(&self) -> bool {
        if !self.tls_owner().is_null() {
            return false;
        }
        let prepared = swr_v1_tls_owner_new();
        // SAFETY: a fresh owner, whose one reference the socket's extra record takes.
        unsafe { *self.tls_slot() = prepared.owner };
        self.tls.set(Some(prepared));
        true
    }

    /// The `SSLContext`'s prepared view: its fields and string slots.
    fn tls(&self) -> Tls {
        Tls(self.tls.get().expect("enable_ssl_encrypt"))
    }

    fn ssl_handshake(&self) -> bool {
        self.co().ssl_handshake()
    }

    fn ssl_verify(&self, allow_self_signed: bool) -> bool {
        self.co().ssl_verify(allow_self_signed)
    }

    /// `ssl_get_peer_cert()`: the peer certificate's PEM, or empty.
    fn ssl_get_peer_cert(&self) -> Vec<u8> {
        let mut pem = vec![0_u8; SW_STACK_BUFFER_SIZE];
        // SAFETY: the open socket's network::Socket and the local buffer.
        let length = unsafe {
            network_peer_certificate(self.record().socket, pem.as_mut_ptr().cast(), pem.len())
        };
        pem.truncate(usize::try_from(length).unwrap_or(0));
        pem
    }

    /// The member over an `IOVector` of `iov`, as crates/phpruntime calls it.
    fn vector(&self, op: c_int, iov: &[IoVec]) -> isize {
        let count = c_int::try_from(iov.len()).unwrap();
        // SAFETY: `iov`'s live vectors, an iovec's layout.
        let mut vector = unsafe { IoVector::new(iov.as_ptr().cast(), count) }.unwrap();
        let write = matches!(op, WRITEV | WRITEV_ALL);
        self.co()
            .vector(&mut vector, write, matches!(op, READV_ALL | WRITEV_ALL))
    }
}

impl Drop for Sock {
    fn drop(&mut self) {
        // SAFETY: the socket `make` or `accept` made, deleted once.
        unsafe { co_socket::swr_v1_hook_co_socket_free(self.ptr) };
    }
}

/// A socket's `SSLContext`, through the view its owner was made with.
struct Tls(TlsPrepared);

impl Tls {
    fn fields(&self) -> &mut TlsFields {
        // SAFETY: the live owner's fields, which nothing else borrows meanwhile.
        unsafe { &mut *self.0.fields }
    }

    fn set_string(&self, index: usize, value: &str) {
        let slot = self.0.strings[index];
        // SAFETY: the owner's live slot and `value`'s bytes.
        assert_eq!(
            unsafe { swr_v1_core_string_set(slot, value.as_ptr().cast(), value.len()) },
            0
        );
    }

    fn set_path(&self, index: u32, file: &str) -> bool {
        let file = cstring(file);
        // SAFETY: the live owner and a NUL-terminated path.
        unsafe {
            swr_v1_tls_owner_set_path(self.0.owner, index, file.as_ptr(), file.count_bytes()) == 0
        }
    }

    fn set_ssl_cert_file(&self, file: &str) -> bool {
        self.set_path(TLS_CERT_FILE, file)
    }

    fn set_ssl_key_file(&self, file: &str) -> bool {
        self.set_path(TLS_KEY_FILE, file)
    }

    fn set_ssl_cafile(&self, file: &str) {
        self.set_string(TLS_CAFILE, file);
    }

    fn set_ssl_dhparam(&self, file: &str) {
        self.set_string(TLS_DHPARAM, file);
    }

    fn set_ssl_ecdh_curve(&self, curve: &str) {
        self.set_string(TLS_ECDH_CURVE, curve);
    }

    fn set_tls_host_name(&self, name: &str) {
        self.set_string(TLS_HOST_NAME, name);
        self.fields().disable_tls_host_name = u8::from(name.is_empty());
    }

    fn set_ssl_verify_peer(&self, value: bool) {
        self.fields().verify_peer = u8::from(value);
    }

    fn set_ssl_allow_self_signed(&self, value: bool) {
        self.fields().allow_self_signed = u8::from(value);
    }
}

/// `Socket::TimeoutSetter`, restored on drop.
struct TimeoutSetter(TimeoutScope);

impl TimeoutSetter {
    fn new(sock: &Sock, timeout: f64, kind: c_int) -> Self {
        Self(TimeoutScope::new(sock.co(), timeout, kind))
    }

    /// `TimeoutController::has_timedout(kind)`.
    fn has_timedout(&mut self, kind: c_int) -> bool {
        self.0.timed_out(kind, microtime())
    }
}

impl TimeoutSetter {
    /// A bitwise copy of the scope (`auto copy = scope;`): both restore on drop.
    fn copy(&self) -> Self {
        // SAFETY: the scope is plain data; a copy restores the same originals.
        Self(unsafe { std::ptr::read(&raw const self.0) })
    }
}

/// `delete conn`.
fn delete(conn: ManuallyDrop<Sock>) {
    drop(ManuallyDrop::into_inner(conn));
}

/// `delete string`.
fn delete_string(string: *mut CppString) {
    // SAFETY: a String the caller owns, made by `new_sized_string`.
    unsafe { swoole_rs_buffer::string::delete_string(string.cast()) };
}

fn socket_pair() -> [c_int; 2] {
    let mut pairs = [-1; 2];
    // SAFETY: two writable descriptors.
    unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, pairs.as_mut_ptr()) };
    pairs
}

/// `swoole::test::create_socket_pair()`.
fn create_socket_pair() -> (Sock, Sock) {
    let pairs = socket_pair();
    let sock0 = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
    let sock1 = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
    for sock in [&sock0, &sock1] {
        sock.set_send_buffer_size(65536);
        sock.set_recv_buffer_size(65536);
    }
    (sock0, sock1)
}

fn random_vec(n: usize) -> Vec<u8> {
    let mut buf = vec![0_u8; n];
    random_bytes(&mut buf);
    buf
}

/// The tests' `Process` running a `Server` on `port`: `reply` answers each
/// received chunk (an echo, or None to close the connection).
fn spawn_server(port: c_int, reply: fn(&[u8]) -> Option<Vec<u8>>) -> c_int {
    // SAFETY: fork(2); the child serves until it is killed.
    let pid = unsafe { fork() };
    assert_ne!(pid, -1);
    if pid == 0 {
        let listener =
            std::net::TcpListener::bind((TEST_HOST, u16::try_from(port).unwrap())).expect("bind");
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0_u8; 65536];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                match reply(&buf[..n]) {
                    Some(answer) => {
                        let _ = stream.write_all(&answer);
                    }
                    None => break,
                }
            }
        }
        // SAFETY: ends the child.
        unsafe { _exit(0) };
    }
    pid
}

fn stop_server(pid: c_int) {
    let mut status = 0;
    // SAFETY: the server child.
    unsafe {
        kill(pid, SIGTERM);
        wait(&raw mut status);
    }
}

fn connect_refused() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.connect("127.0.0.1", 9801, 0);
        assert!(!retval);
        assert_eq!(sock.err_code(), ECONNREFUSED);
    });
}

fn connect_timeout() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);

        sock.set_timeout(0.5, SW_TIMEOUT_ALL);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_DNS), 0.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_CONNECT), 0.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_READ), 0.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_WRITE), 0.5);

        sock.set_timeout(1.5, SW_TIMEOUT_RDWR);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_DNS), 0.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_CONNECT), 0.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_READ), 1.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_WRITE), 1.5);

        let retval = sock.connect("192.0.0.1", 9801, 0);
        assert!(!retval);
        assert_eq!(sock.err_code(), ETIMEDOUT);
    });
}

/// The timeout cases' server: accept, send `TEST_STR`, hold the connection 1s.
fn hold_server(port: c_int) {
    let sock = Sock::new(SW_SOCK_TCP);
    let retval = sock.bind("127.0.0.1", port);
    assert!(retval);
    assert!(sock.listen(128));

    let conn = sock.accept();
    conn.send(TEST_STR);
    co_sleep(1.0);
    delete(conn);
}

fn timeout_controller() {
    run(|| {
        let port = 74 + TEST_PORT;
        go(move || hold_server(port));

        let sock = Sock::new(SW_SOCK_TCP);
        let mut tc = TimeoutSetter::new(&sock, 0.5, SW_TIMEOUT_ALL);
        assert!(sock.connect("127.0.0.1", port, 0));

        let mut buf = [0_u8; 128];
        let mut offset = 0;
        sock.set_err_code(0);
        loop {
            if tc.has_timedout(SW_TIMEOUT_READ) {
                break;
            }
            let rv = sock.recv(&mut buf[offset..]);
            if rv <= 0 {
                break;
            }
            offset += size(rv);
        }
        assert!(tc.has_timedout(SW_TIMEOUT_READ));
        assert_eq!(sock.err_code(), ETIMEDOUT);
    });
}

fn timeout_setter() {
    run(|| {
        let port = 111 + TEST_PORT;
        go(move || hold_server(port));

        let sock = Sock::new(SW_SOCK_TCP);
        let _ts = TimeoutSetter::new(&sock, 0.5, SW_TIMEOUT_ALL);
        assert!(sock.connect("127.0.0.1", port, 0));

        let mut buf = [0_u8; 128];
        let mut offset = 0;
        sock.set_err_code(0);
        loop {
            let rv = sock.recv(&mut buf[offset..]);
            if rv <= 0 {
                break;
            }
            offset += size(rv);
        }
        assert_eq!(sock.err_code(), ETIMEDOUT);
    });
}

fn connect_with_dns() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.connect(HOST, 80, 0);
        assert!(retval);
        assert_eq!(sock.err_code(), 0);
    });
}

fn tcp6() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP6);
        let retval = sock.connect("::1", 80, 0);
        assert!(retval);
        assert_eq!(sock.err_code(), 0);
    });
}

fn unixsock_fail() {
    run(|| {
        let sock = Sock::new(SW_SOCK_UNIX_STREAM);
        let retval = sock.connect("/tmp/unix.sock", 0, 0);
        assert!(!retval);
        assert_eq!(sock.err_code(), ENOENT);
    });
}

fn recv_success() {
    let port = get_random_port();
    let pid = spawn_server(port, |data| Some(data.to_vec()));

    // SAFETY: waits for the test server to start.
    unsafe { sleep(1) };

    run(move || {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.connect(TEST_HOST, port, -1);
        assert!(retval);
        assert_eq!(sock.err_code(), 0);
        sock.send(b"hello world\n");
        let mut buf = [0_u8; 128];
        let n = sock.recv(&mut buf);
        assert_eq!(&buf[..size(n)], b"hello world\n");
    });

    stop_server(pid);
}

fn recv_fail() {
    let port = get_random_port();
    let pid = spawn_server(port, |_| None);

    // SAFETY: waits for the test server to start.
    unsafe { sleep(1) };

    run(move || {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.connect(TEST_HOST, port, -1);
        assert!(retval);
        assert_eq!(sock.err_code(), 0);
        sock.send(b"close\0");
        let mut buf = [0_u8; 128];
        let n = sock.recv(&mut buf);
        assert_eq!(n, 0);
    });

    stop_server(pid);
}

fn bind_success() {
    let port = 237 + TEST_PORT;
    run(move || {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.bind("127.0.0.1", port);
        assert!(retval);

        let sock_1 = Sock::new(SW_SOCK_UNIX_DGRAM);
        let retval = sock_1.bind("/tmp/swoole-core-tests.sock", 0);
        assert!(retval);
    });
}

fn bind_fail() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.bind("192.111.11.1", 9909);
        assert!(!retval);
        assert_eq!(sock.err_code(), EADDRNOTAVAIL);

        let sock_1 = Sock::new(SW_SOCK_TCP);
        let retval = sock_1.bind("127.0.0.1", 70000);
        assert!(!retval);
    });
}

fn listen() {
    let port = 263 + TEST_PORT;
    run(move || {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.bind("127.0.0.1", port);
        assert!(retval);
        assert!(sock.listen(128));
    });
}

/// A listening TCP socket on 127.0.0.1:`port`.
fn listener(port: c_int) -> Sock {
    let sock = Sock::new(SW_SOCK_TCP);
    let retval = sock.bind("127.0.0.1", port);
    assert!(retval);
    assert!(sock.listen(128));
    sock
}

/// A TCP socket connected to 127.0.0.1:`port`.
fn connected(port: c_int) -> Sock {
    let sock = Sock::new(SW_SOCK_TCP);
    let retval = sock.connect("127.0.0.1", port, -1);
    assert!(retval);
    assert_eq!(sock.err_code(), 0);
    sock
}

fn accept() {
    let port = 273 + TEST_PORT;
    run(move || {
        go(move || {
            let sock = listener(port);
            let conn = sock.accept();
            assert!(!conn.ptr.is_null());
            delete(conn);
        });

        let sock = connected(port);
        sock.close();
    });
}

/// The EOF cases' server: accept, expect "start\r\n", then `then(conn)`.
fn start_server(port: c_int, then: impl FnOnce(&Sock)) {
    let sock = listener(port);
    let conn = sock.accept();
    let mut buf = [0_u8; 1024];
    let l = conn.recv(&mut buf);
    assert_eq!(&buf[..size(l)], b"start\r\n");
    then(&conn);
}

/// The EOF cases' client: connected, "start\r\n" sent, the EOF protocol set.
fn start_client(port: c_int) -> Sock {
    let sock = connected(port);
    sock.send(b"start\r\n");
    sock.set_eof_protocol();
    sock
}

fn eof_1() {
    let port = 301 + TEST_PORT;
    run(move || {
        go(move || {
            start_server(port, |conn| {
                conn.send(EOF_PACKET);
            });
        });

        let sock = start_client(port);
        let l = sock.recv_packet(RECV_TIMEOUT);
        let eof_packet_len = EOF_PACKET.len();
        let buf = sock.read_buffer();

        assert_eq!(size(l), eof_packet_len);
        // SAFETY: the buffer holds the packet.
        assert_eq!(
            unsafe { std::slice::from_raw_parts(buf.str, size(l)) },
            EOF_PACKET
        );
        assert_eq!(buf.length, eof_packet_len);
        assert_eq!(buf.offset, i64::try_from(eof_packet_len).unwrap());
    });
}

fn eof_2() {
    let port = 336 + TEST_PORT;
    run(move || {
        go(move || {
            start_server(port, |conn| {
                conn.send(&[EOF_PACKET, EOF_PACKET_2].concat());
            });
        });

        let sock = start_client(port);
        // packet 1
        {
            let l = sock.recv_packet(RECV_TIMEOUT);
            let eof_packet_len = EOF_PACKET.len();
            let buf = sock.read_buffer();

            assert_eq!(size(l), eof_packet_len);
            // SAFETY: the buffer holds the packet.
            assert_eq!(
                unsafe { std::slice::from_raw_parts(buf.str, size(l)) },
                EOF_PACKET
            );
            assert_eq!(buf.length, EOF_PACKET.len() + EOF_PACKET_2.len());
            assert_eq!(buf.offset, i64::try_from(eof_packet_len).unwrap());
        }
        // packet 2
        {
            let l = sock.recv_packet(RECV_TIMEOUT);
            let eof_packet_len = EOF_PACKET_2.len();
            let buf = sock.read_buffer();

            assert_eq!(size(l), eof_packet_len);
            // SAFETY: the buffer holds the packet.
            assert_eq!(
                unsafe { std::slice::from_raw_parts(buf.str, size(l)) },
                EOF_PACKET_2
            );
            assert_eq!(buf.length, EOF_PACKET_2.len());
            assert_eq!(buf.offset, i64::try_from(eof_packet_len).unwrap());
        }
    });
}

fn eof_3() {
    let port = 384 + TEST_PORT;
    run(move || {
        go(move || {
            start_server(port, |conn| {
                conn.shutdown(SHUT_RDWR);
            });
        });

        let sock = start_client(port);
        let l = sock.recv_packet(RECV_TIMEOUT);
        assert_eq!(l, 0);
    });
}

fn eof_4() {
    let port = 413 + TEST_PORT;
    run(move || {
        go(move || {
            start_server(port, |conn| {
                conn.send(&EOF_PACKET[..EOF_PACKET.len() - CRLF.len()]); // no eof
                conn.shutdown(SHUT_RDWR);
            });
        });

        let sock = start_client(port);
        let l = sock.recv_packet(RECV_TIMEOUT);
        assert_eq!(l, 0);

        let buf = sock.read_buffer();
        // SAFETY: the buffer holds the partial packet.
        assert_eq!(
            unsafe { std::slice::from_raw_parts(buf.str, 10) },
            &EOF_PACKET[..10]
        );
    });
}

fn eof_5() {
    let port = 446 + TEST_PORT;
    let pkt_len = 512 * 1024;
    run(move || {
        go(move || {
            start_server(port, |conn| {
                let mut s = vec![b'A'; pkt_len - 16];
                s.extend_from_slice(CRLF);

                conn.set_send_buffer_size(65536);
                conn.send_all(&s);
            });
        });

        let sock = start_client(port);
        sock.set_recv_buffer_size(65536);
        let l = sock.recv_packet(RECV_TIMEOUT);
        assert_eq!(size(l), pkt_len - 14);
    });
}

fn eof_6() {
    let port = 483 + TEST_PORT;
    run(move || {
        go(move || {
            start_server(port, |conn| {
                let mut s = vec![b'A'; 128 * 1024 - 16];
                s.extend_from_slice(CRLF);

                conn.send_all(&s);
            });
        });

        let sock = start_client(port);
        sock.record().protocol.package_max_length = 1024 * 64;

        let l = sock.recv_packet(RECV_TIMEOUT);
        assert_eq!(l, -1);
        assert_eq!(sock.err_code(), SW_ERROR_PACKAGE_LENGTH_TOO_LARGE);
    });
}

fn set_length_protocol_1(sock: &Sock) {
    sock.set_length_protocol(b'n', 2, 65535);
}

fn set_length_protocol_2(sock: &Sock) {
    sock.set_length_protocol(b'N', 4, 2 * 1024 * 1024);
}

/// The length cases' server: accept, then `then(conn)`.
fn accept_server(port: c_int, then: impl FnOnce(&Sock)) {
    let sock = listener(port);
    let conn = sock.accept();
    then(&conn);
}

fn length_1() {
    let port = 543 + TEST_PORT;
    run(move || {
        go(move || {
            accept_server(port, |conn| {
                let mut buf = [0_u8; 1024];
                let l = random_bytes(&mut buf[2..]);
                buf[..2].copy_from_slice(&u16::try_from(l).unwrap().to_be_bytes());

                conn.send(&buf[..l + 2]);
            });
        });

        let sock = connected(port);
        set_length_protocol_1(&sock);

        let l = sock.recv_packet(RECV_TIMEOUT);
        let buf = sock.read_buffer();

        assert_eq!(l, 1024);
        assert_eq!(buf.length, size(l));
        assert_eq!(buf.offset, i64::try_from(l).unwrap());
    });
}

fn length_2() {
    let port = 576 + TEST_PORT;
    run(move || {
        go(move || {
            accept_server(port, |conn| {
                let buf = 0_u16.to_be_bytes();
                conn.send(&buf);
            });
        });

        let sock = connected(port);
        set_length_protocol_1(&sock);

        let l = sock.recv_packet(RECV_TIMEOUT);
        let buf = sock.read_buffer();

        assert_eq!(l, 2);
        assert_eq!(buf.length, 2);
        assert_eq!(buf.offset, 2);
    });
}

fn length_3() {
    let port = 608 + TEST_PORT;
    run(move || {
        go(move || {
            accept_server(port, |conn| {
                let mut buf = [b'A'; 1024];
                buf[..2].copy_from_slice(&65530_u16.to_be_bytes());
                conn.send(&buf);
            });
        });

        let sock = connected(port);
        set_length_protocol_1(&sock);
        sock.record().protocol.package_max_length = 4096;

        let l = sock.recv_packet(RECV_TIMEOUT);
        assert_eq!(l, -1);
        assert_eq!(sock.err_code(), SW_ERROR_PACKAGE_LENGTH_TOO_LARGE);
    });
}

/// `length_protocol_server_func`: two random length-prefixed packets, sent as one;
/// `pkt_1` and `pkt_2` receive them.
fn length_protocol_server_func(port: c_int, pkts: &[Cell<Vec<u8>>; 2]) {
    let sock = listener(port);
    let conn = sock.accept();
    let mut strbuf = Vec::with_capacity(256 * 1024);
    for pkt in pkts {
        // SAFETY: Swoole's random source.
        let l = usize::try_from(unsafe { swoole_rand(65536, 65536 * 2) }).unwrap();
        let start = strbuf.len();
        strbuf.extend_from_slice(&u32::try_from(l).unwrap().to_be_bytes());
        strbuf.extend_from_slice(&random_vec(l));
        pkt.set(strbuf[start..].to_vec());
    }
    conn.send_all(&strbuf);
}

fn length_4() {
    let port = 670 + TEST_PORT;
    let pkts = [Cell::new(Vec::new()), Cell::new(Vec::new())];
    let pkts = &pkts;
    run(move || {
        go(move || length_protocol_server_func(port, pkts));

        let sock = connected(port);
        set_length_protocol_2(&sock);

        let mut bytes = 0;
        for pkt in pkts {
            let l = sock.recv_packet(RECV_TIMEOUT);
            bytes += size(l);
            let buf = sock.read_buffer();
            // SAFETY: the buffer holds the packet.
            let data = unsafe { std::slice::from_raw_parts(buf.str, buf.length) };
            let unpack_len = u32::from_be_bytes(data[..4].try_into().unwrap());

            let expected = pkt.take();
            assert_eq!(expected, data);

            assert_eq!(unpack_len as usize, size(l) - 4);
            assert_eq!(buf.length, size(l));
            assert_eq!(buf.offset, i64::try_from(l).unwrap());
        }
        assert!(bytes >= 65536 * 2);
    });
}

fn length_5() {
    let port = 703 + TEST_PORT;
    let pkts = [Cell::new(Vec::new()), Cell::new(Vec::new())];
    let pkts = &pkts;
    run(move || {
        go(move || length_protocol_server_func(port, pkts));

        let sock = connected(port);
        set_length_protocol_2(&sock);

        let mut bytes = 0;
        for pkt in pkts {
            let l = sock.recv_packet(RECV_TIMEOUT);
            bytes += size(l);
            let data = sock.pop_packet();
            // SAFETY: the popped packet holds `l` bytes.
            let data = unsafe { std::slice::from_raw_parts(data, size(l)) };
            let unpack_len = u32::from_be_bytes(data[..4].try_into().unwrap());
            assert_eq!(unpack_len as usize, size(l) - 4);

            let expected = pkt.take();
            assert_eq!(expected, data);
        }
        assert!(bytes >= 65536 * 2);
    });
}

fn length_7() {
    let port = 733 + TEST_PORT;
    run(move || {
        go(move || {
            accept_server(port, |conn| {
                let mut buf = [0_u8; 1024];
                // `*(uint32_t *) buf = htons(0)`.
                buf[..4].copy_from_slice(&0_u32.to_ne_bytes());

                conn.send(&buf[..2]);
                co_sleep(0.01);
                conn.send(&buf[2..4]);
            });
        });

        let sock = connected(port);
        set_length_protocol_2(&sock);

        let l = sock.recv_packet(RECV_TIMEOUT);
        let buf = sock.read_buffer();

        assert_eq!(l, 4);
        assert_eq!(buf.length, 4);
        assert_eq!(buf.offset, 4);
    });
}

fn event_hup() {
    let port = 767 + TEST_PORT;
    run(move || {
        go(move || {
            let sock = listener(port);
            let conn = sock.accept();
            co_sleep(0.05);
            let mut buf = [0_u8; 1024];
            let ret_n = conn.recv(&mut buf);
            assert_eq!(ret_n, 0);
            delete(conn);
        });

        let sock = connected(port);
        let buf = sock.read_buffer();
        let (at, n) = (buf.str, buf.size);
        let shared = &sock;
        go(move || {
            co_sleep(0.01);
            shared.shutdown(SHUT_RDWR);
        });
        let n = sock.recv_all(at, n);
        assert_eq!(sock.network().event_hup, 1);
        assert_eq!(n, 0);
    });
}

fn recv_line() {
    let port = 800 + TEST_PORT;
    run(move || {
        go(move || {
            let sock = listener(port);
            let conn = sock.accept();
            conn.send(b"hello world\n");
            conn.send(b"\r");
            let mut buf = [0_u8; 256];
            buf[..128].fill(b'A');
            buf[128..253].fill(b'B');
            conn.send(&buf[..253]);
            delete(conn);
        });

        let sock = connected(port);
        let at = sock.read_buffer().str;
        // SAFETY: the read buffer, at least 128 bytes.
        let line = |n: isize| unsafe { std::slice::from_raw_parts(at, size(n)) };

        let n = sock.recv_line(at, 128);
        assert_eq!(n, 12);
        assert_eq!(line(n), b"hello world\n");

        let n = sock.recv_line(at, 128);
        assert_eq!(n, 1);
        assert_eq!(line(n), b"\r");

        let mut buf_2 = [0_u8; 256];
        buf_2[..128].fill(b'A');
        buf_2[128..253].fill(b'B');

        let n = sock.recv_line(at, 128);
        assert_eq!(n, 128);
        assert_eq!(line(n), &buf_2[..128]);

        let n = sock.recv_line(at, 128);
        assert_eq!(n, 125);
        assert_eq!(line(n), &buf_2[128..253]);

        let n = sock.recv_line(at, 128);
        assert_eq!(n, 0);
    });
}

fn getsockname() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        assert!(sock.connect(HOST, 80, 0));
        assert!(sock.co().getsockname());
        sock.close();
    });
}

fn buffer() {
    let sock = Sock::new(SW_SOCK_TCP);
    let rbuf = ptr::from_mut(sock.read_buffer());
    let wbuf = sock.write_buffer();

    // `pop_read_buffer` and `pop_write_buffer`: the record lets go of each buffer.
    let rbuf_pop = std::mem::replace(&mut sock.record().read_buffer, ptr::null_mut());
    let wbuf_pop = std::mem::replace(&mut sock.record().write_buffer, ptr::null_mut());

    assert_eq!(rbuf, rbuf_pop);
    assert_eq!(wbuf, wbuf_pop);

    let rbuf2 = ptr::from_mut(sock.read_buffer());
    let wbuf2 = sock.write_buffer();

    assert_ne!(rbuf2, rbuf);
    assert_ne!(wbuf2, wbuf);

    delete_string(rbuf_pop);
    delete_string(wbuf_pop);
}

fn check_liveness() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        let retval = sock.connect(HOST, 80, 0);
        assert!(retval);

        let result = sock.check_liveness();
        sock.close();
        assert!(result);
        let result = sock.check_liveness();
        assert!(!result);
    });
}

const HELLO: &[u8] = b"Hello World";

/// `write_and_read` and `write_and_read_2`, with `make(fd)` the constructor under test.
fn write_and_read_with(make: fn(c_int) -> Sock) {
    run(move || {
        let pairs = socket_pair();
        let length = HELLO.len();

        go(move || {
            let sock = make(pairs[0]);
            let result = sock.write(HELLO);
            sock.close();
            assert_eq!(size(result), length);
        });

        let mut data = [0_u8; 128];
        let sock = make(pairs[1]);
        let result = sock.read(&mut data);
        sock.close();
        assert!(result > 0);
        assert_eq!(&data[..size(result)], HELLO);
    });
}

fn write_and_read() {
    write_and_read_with(|fd| Sock::with_fd(fd, SW_SOCK_UNIX_STREAM));
}

fn write_and_read_2() {
    // test for Socket::Socket(int _fd, int _domain, int _type, int _protocol) construct function
    write_and_read_with(|fd| Sock::with_family(fd, AF_UNIX, SOCK_STREAM, 0));
}

/// `Sock::vector`'s operations.
const READV: c_int = 0;
const READV_ALL: c_int = 1;
const WRITEV: c_int = 2;
const WRITEV_ALL: c_int = 3;

fn iovecs(buffers: &mut [&mut [u8]]) -> Vec<IoVec> {
    buffers
        .iter_mut()
        .map(|buf| IoVec {
            base: buf.as_mut_ptr().cast(),
            len: buf.len(),
        })
        .collect()
}

fn writev_and_readv() {
    run(|| {
        let iovcnt = 3;
        let length = HELLO.len();
        let pairs = socket_pair();

        go(move || {
            let mut text = HELLO.to_vec();
            let iov: Vec<IoVec> = (0..iovcnt)
                .map(|_| IoVec {
                    base: text.as_mut_ptr().cast(),
                    len: length,
                })
                .collect();

            let sock = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
            let result = sock.vector(WRITEV, &iov);
            sock.close();
            assert_eq!(size(result), length * 3);
        });

        let mut results = [[0_u8; 16]; 3];
        let mut buffers: Vec<&mut [u8]> = results.iter_mut().map(|r| &mut r[..length]).collect();
        let iov = iovecs(&mut buffers);

        let sock = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
        let result = sock.vector(READV, &iov);
        sock.close();
        assert_eq!(size(result), length * 3);

        for result in &results {
            assert_eq!(&result[..length], HELLO);
        }
    });
}

fn send_and_recv_all() {
    run(|| {
        let pairs = socket_pair();
        let wbuf = random_vec(4 * 1024 * 1024);
        let wbuf = &wbuf;

        go(move || {
            let sock = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
            sock.set_send_buffer_size(65536);

            assert_eq!(size(sock.send_all(wbuf)), wbuf.len());

            co_sleep(0.1);

            sock.close();
        });

        let sock = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
        sock.set_recv_buffer_size(65536);

        let mut rbuf = vec![0_u8; wbuf.len()];
        let result = sock.recv_all(rbuf.as_mut_ptr(), wbuf.len());
        assert_eq!(size(result), wbuf.len());
        assert_eq!(wbuf, &rbuf);
        co_sleep(0.1);
        sock.close();
    });
}

fn writevall_and_readvall() {
    run(|| {
        let write_iovcnt = 4;
        let pairs = socket_pair();

        let mut buf = [0_u8; 65536];
        random_bytes(&mut buf);
        let buf = &buf;
        let length = buf.len();

        go(move || {
            let iov: Vec<IoVec> = (0..write_iovcnt)
                .map(|_| IoVec {
                    base: buf.as_ptr().cast_mut().cast(),
                    len: length,
                })
                .collect();

            let sock = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
            sock.set_send_buffer_size(u32::try_from(length).unwrap());

            assert_eq!(size(sock.vector(WRITEV_ALL, &iov)), write_iovcnt * length);

            co_sleep(0.01);

            assert_eq!(size(sock.vector(WRITEV_ALL, &iov)), write_iovcnt * length);

            sock.close();
        });

        let read_iovcnt = 8;
        let mut storage = vec![vec![0_u8; length]; read_iovcnt];
        let mut buffers: Vec<&mut [u8]> = storage.iter_mut().map(Vec::as_mut_slice).collect();
        let iov = iovecs(&mut buffers);

        let sock = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
        sock.set_recv_buffer_size(u32::try_from(length).unwrap());

        let result = sock.vector(READV_ALL, &iov);
        sock.close();
        assert_eq!(size(result), length * read_iovcnt);

        for read in &storage {
            assert_eq!(read.as_slice(), buf);
        }
    });
}

fn sendfile() {
    run(|| {
        let pairs = socket_pair();
        go(move || {
            let file = cstring(
                &(std::env::current_dir().unwrap().display().to_string() + "/examples/test.jpg"),
            );
            let sock = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
            let result = sock.co().sendfile(&file, 0, 0);
            sock.close();
            assert!(result);
        });

        let mut data = vec![0_u8; 250_000];
        let sock = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
        let result = sock.read(&mut data);
        sock.close();
        assert!(result > 0);
    });
}

fn test_sendto_recvfrom(sock_type: c_int) {
    run(move || {
        let server_text = b"hello world!!!";
        let server_length = server_text.len();
        let client_text = b"hello swoole!!!";
        let client_length = client_text.len();

        let ip = if sock_type == SW_SOCK_UDP {
            "127.0.0.1"
        } else {
            "::1"
        };
        let ip_c = cstring(ip);
        let local = c"localhost";

        let port = get_random_port();

        let sock_server = Sock::new(sock_type);
        let sock_client = Sock::new(sock_type);
        sock_server.bind(ip, port);
        sock_client.bind(ip, port + 1);

        let _ = sock_server
            .co()
            .sendto(ip_c.to_bytes(), port + 1, &server_text[..server_length]);

        let mut data_from_server = [0_u8; 128];
        // `inet_addr(ip)`: the IPv4 address, or INADDR_NONE for "::1".
        let addr = ip
            .parse::<std::net::Ipv4Addr>()
            .map_or([0xff; 4], |a| a.octets());
        let mut serveraddr = SockAddrIn {
            family: u16::try_from(AF_INET).unwrap(),
            port: u16::try_from(port).unwrap().to_be(),
            addr,
            zero: [0; 8],
        };
        let mut addr_length = u32::try_from(size_of::<SockAddrIn>()).unwrap();

        // receive data from server
        // SAFETY: the buffer and the address storage.
        let result = unsafe {
            sock_client.co().recvfrom(
                data_from_server.as_mut_ptr().cast(),
                server_length,
                (&raw mut serveraddr).cast(),
                &raw mut addr_length,
            )
        };
        assert_eq!(
            result,
            server_length.cast_signed(),
            "type {sock_type}, from server"
        );
        assert_eq!(&data_from_server[..size(result)], server_text);

        // receive data from client
        let mut data_from_client = [0_u8; 128];
        let _ = sock_client
            .co()
            .sendto(local.to_bytes(), port, &client_text[..client_length]);
        let result = sock_server
            .co()
            .recvfrom_peer(&mut data_from_client[..client_length]);
        assert_eq!(
            result,
            client_length.cast_signed(),
            "type {sock_type}, from client"
        );
        assert_eq!(&data_from_client[..client_length], client_text);

        // ON_SCOPE_EXIT
        sock_server.close();
        sock_client.close();
    });
}

fn sendto_recvfrom_udp() {
    test_sendto_recvfrom(SW_SOCK_UDP);
    test_sendto_recvfrom(SW_SOCK_UDP6);
}

fn socket_test_request_baidu(sock: &Sock) {
    assert!(sock.send(TEST_REQUEST_BAIDU) > 0);

    let mut buf = Vec::with_capacity(65536);
    loop {
        let mut rbuf = [0_u8; 4096];
        let nr = sock.recv(&mut rbuf);
        if nr <= 0 {
            break;
        }
        buf.extend_from_slice(&rbuf[..size(nr)]);
    }
    assert!(contains(&buf, b"www.baidu.com"));
}

fn proxy_test(sock: &Sock, https: bool) {
    if https {
        sock.enable_ssl_encrypt();
    }

    let retval = sock.connect(HOST, if https { 443 } else { 80 }, 0);
    // ON_SCOPE_EXIT
    struct Close<'a>(&'a Sock);
    impl Drop for Close<'_> {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = Close(sock);
    assert!(retval);

    if https {
        assert_ne!(sock.ssl_get_peer_cert(), b"");
    }

    socket_test_request_baidu(sock);
}

fn proxy_set_socks5_proxy(socket: &Sock, port: c_int, auth: bool) {
    let (username, password) = if auth {
        (TEST_SOCKS5_PROXY_USER, TEST_SOCKS5_PROXY_PASSWORD)
    } else {
        ("", "")
    };
    socket.set_socks5_proxy(TEST_SOCKS5_PROXY_HOST, port, username, password);
}

fn https_get_with_socks5_proxy() {
    run(|| {
        if is_github_ci() {
            let sock = Sock::new(SW_SOCK_TCP);
            proxy_set_socks5_proxy(&sock, TEST_SOCKS5_PROXY_PORT, true);
            proxy_test(&sock, true);
        }
        // no auth
        {
            let sock = Sock::new(SW_SOCK_TCP);
            proxy_set_socks5_proxy(&sock, TEST_SOCKS5_PROXY_NO_AUTH_PORT, false);
            proxy_test(&sock, true);
        }
    });
}

fn http_get_with_socks5_proxy() {
    run(|| {
        if is_github_ci() {
            let sock = Sock::new(SW_SOCK_TCP);
            proxy_set_socks5_proxy(&sock, TEST_SOCKS5_PROXY_PORT, true);
            proxy_test(&sock, false);
        }
        // no auth
        {
            let sock = Sock::new(SW_SOCK_TCP);
            proxy_set_socks5_proxy(&sock, TEST_SOCKS5_PROXY_NO_AUTH_PORT, false);
            proxy_test(&sock, false);
        }
    });
}

fn proxy_set_http_proxy(socket: &Sock) {
    let (username, password) = if is_github_ci() {
        (TEST_HTTP_PROXY_USER, TEST_HTTP_PROXY_PASSWORD)
    } else {
        ("", "")
    };
    socket.set_http_proxy(
        TEST_HTTP_PROXY_HOST,
        TEST_HTTP_PROXY_PORT,
        username,
        password,
    );
}

fn http_get_with_http_proxy() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        proxy_set_http_proxy(&sock);
        proxy_test(&sock, false);
    });
}

fn https_get_with_http_proxy() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        proxy_set_http_proxy(&sock);
        proxy_test(&sock, true);
    });
}

fn ssl() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);

        sock.enable_ssl_encrypt();
        let tls = sock.tls();
        tls.set_ssl_cert_file(&(get_ssl_dir() + "/client.crt"));
        tls.set_ssl_key_file(&(get_ssl_dir() + "/client.key"));
        tls.set_ssl_verify_peer(false);
        tls.set_ssl_allow_self_signed(true);
        tls.set_ssl_cafile(&(get_ssl_dir() + "/ca.crt"));

        proxy_test(&sock, true);
    });
}

fn ssl_accept() {
    let port = 1262 + TEST_PORT;
    run(move || {
        go(move || {
            let sock = Sock::new(SW_SOCK_TCP);
            let retval = sock.bind("127.0.0.1", port);
            assert!(retval);

            sock.enable_ssl_encrypt();
            let tls = sock.tls();
            tls.set_ssl_cert_file(&(get_ssl_dir() + "/server.crt"));
            tls.set_ssl_key_file(&(get_ssl_dir() + "/server.key"));
            tls.set_ssl_dhparam(&(get_ssl_dir() + "/dhparams.pem"));
            tls.set_ssl_ecdh_curve("secp256r1");

            assert!(sock.listen(128));

            let conn = sock.accept();
            assert!(!conn.ptr.is_null());
            assert!(conn.ssl_handshake());
            conn.send(EOF_PACKET);
            let mut rbuf = [0_u8; 1024];
            let n = conn.recv(&mut rbuf);

            assert_eq!(&rbuf[..size(n)], EOF_PACKET_2);
            conn.close();
            delete(conn);
        });

        let sock = Sock::new(SW_SOCK_TCP);
        sock.enable_ssl_encrypt();
        let retval = sock.connect("127.0.0.1", port, -1);
        assert!(retval);
        assert_eq!(sock.err_code(), 0);

        let mut rbuf = [0_u8; 1024];
        let n = sock.recv(&mut rbuf);
        assert_eq!(&rbuf[..size(n)], EOF_PACKET);
        sock.send(EOF_PACKET_2);

        sock.close();
    });
}

fn peek() {
    run(|| {
        let pairs = socket_pair();
        let length = HELLO.len();

        go(move || {
            let sock = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
            let result = sock.write(HELLO);
            sock.close();
            assert_eq!(size(result), length);
        });

        let mut data = [0_u8; 128];
        let sock = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
        let result = sock.co().peek(&mut data[..5]);
        sock.close();
        assert_eq!(result, 5);
        assert_eq!(&data[..size(result)], b"Hello");
    });
}

fn msghdr(ivec: &mut IoVec) -> MsgHdr {
    MsgHdr {
        name: ptr::null_mut(),
        namelen: 0,
        iov: ivec,
        iovlen: 1,
        control: ptr::null_mut(),
        controllen: 0,
        flags: 0,
    }
}

fn sendmsg_and_recvmsg() {
    run(|| {
        let pairs = socket_pair();
        let length = HELLO.len();

        go(move || {
            let sock = Sock::with_fd(pairs[0], SW_SOCK_UNIX_STREAM);
            let mut ivec = IoVec {
                base: HELLO.as_ptr().cast_mut().cast(),
                len: length,
            };
            let msg = msghdr(&mut ivec);

            // SAFETY: the message, over the live vector.
            let ret = unsafe { sock.co().sendmsg((&raw const msg).cast(), 0) };
            sock.close();
            assert_eq!(size(ret), length);
        });

        let sock = Sock::with_fd(pairs[1], SW_SOCK_UNIX_STREAM);
        let mut buf = vec![0_u8; length + 1];
        let mut ivec = IoVec {
            base: buf.as_mut_ptr().cast(),
            len: length,
        };
        let mut msg = msghdr(&mut ivec);

        // SAFETY: the message, over the live vector.
        let ret = unsafe { sock.co().recvmsg((&raw mut msg).cast(), 0) };
        sock.close();
        assert_eq!(&buf[..size(ret)], HELLO);
    });
}

fn close() {
    run(|| {
        let pair = create_socket_pair();
        let buffer = random_vec(256 * 1024);
        let buffer = &buffer;

        let write_done = Cell::new(false);
        let read_done = Cell::new(false);
        let (write_done, read_done) = (&write_done, &read_done);
        let sock = &pair.0;

        // write co
        go(move || {
            for _ in 0..32 {
                let result = sock.write(buffer);
                if result < 0 && sock.err_code() == ECANCELED {
                    assert!(!sock.close());
                    assert_eq!(sock.err_code(), SW_ERROR_CO_SOCKET_CLOSE_WAIT);
                    write_done.set(true);
                    assert_eq!(sock.write(buffer), -1);
                    assert_eq!(sock.err_code(), EBADF);
                    break;
                }
            }
        });

        // read co
        go(move || {
            for _ in 0..32 {
                let mut buf = [0_u8; 4096];
                let result = sock.read(&mut buf);
                if result < 0 && sock.err_code() == ECANCELED {
                    assert!(sock.close());
                    read_done.set(true);
                    break;
                }
            }
        });

        co_sleep(0.1);
        assert!(!sock.close());
        assert_eq!(sock.err_code(), SW_ERROR_CO_SOCKET_CLOSE_WAIT);
        assert!(sock.is_closed());
        assert!(write_done.get());
        assert!(read_done.get());
        assert!(!sock.close());
        assert_eq!(sock.err_code(), EBADF);
    });
}

fn cancel() {
    run(|| {
        let pair = create_socket_pair();
        let read_done = Cell::new(false);
        let read_done = &read_done;
        let sock = &pair.0;

        // read co
        go(move || {
            for _ in 0..32 {
                let mut buf = [0_u8; 4096];
                let result = sock.read(&mut buf);
                if result < 0 && sock.err_code() == ECANCELED {
                    read_done.set(true);
                    break;
                }
            }
        });

        co_sleep(0.1);
        sock.cancel(SW_EVENT_READ);
        assert!(read_done.get());
    });
}

fn get_event_str() {
    let sock = Sock::new(SW_SOCK_TCP);
    let event_str = |event: c_int| co_io::event_str(sock.co(), event.cast_unsigned());
    assert_eq!(event_str(SW_EVENT_READ), c"reading");
    assert_eq!(event_str(SW_EVENT_WRITE), c"writing");
}

fn option() {
    let sock = Sock::new(SW_SOCK_TCP);
    let mut optval: c_int = 0;

    assert!(sock.co().get_int_option(SOL_SOCKET, SO_RCVBUF, &mut optval));
    assert!(optval > 65536);

    optval *= 2;
    assert!(sock.co().set_int_option(SOL_SOCKET, SO_RCVBUF, optval));
}

fn test_ssl_verify() {
    let domain = TEST_HTTP_DOMAIN.to_str().unwrap();
    let sock = Sock::new(SW_SOCK_TCP);
    sock.enable_ssl_encrypt();
    let tls = sock.tls();
    tls.set_tls_host_name(domain);
    tls.set_ssl_verify_peer(true);
    assert!(sock.connect(domain, 443, 0));
    assert!(sock.ssl_verify(false));

    let req = http_get_request(TEST_HTTP_DOMAIN, "/");
    assert_eq!(size(sock.send(&req)), req.len());
    assert!(sock.check_liveness());

    let mut buf = Vec::with_capacity(65536);
    loop {
        let mut chunk = [0_u8; 65536];
        let n = sock.recv(&mut chunk);
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..size(n)]);
    }

    assert!(contains(&buf, TEST_HTTPS_EXPECT));

    // SAFETY: a plain sleep.
    unsafe { usleep(50000) };
    assert!(!sock.check_liveness());
}

fn ssl_verify() {
    run(test_ssl_verify);
}

fn shutdown() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        assert!(sock.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 80, 0));
        assert!(sock.shutdown(SHUT_RD));
        assert!(!sock.shutdown(SHUT_RD));
        assert_eq!(last_error(), ENOTCONN);

        assert!(sock.shutdown(SHUT_WR));
        assert!(!sock.shutdown(SHUT_WR));
        assert_eq!(last_error(), ENOTCONN);

        assert!(!sock.shutdown(SHUT_RDWR));
        assert_eq!(last_error(), ENOTCONN);
    });
}

fn recv_packet() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        assert!(sock.connect(TEST_HTTP_DOMAIN.to_str().unwrap(), 80, 0));
        let req = http_get_request(TEST_HTTP_DOMAIN, "/");
        assert_eq!(size(sock.send(&req)), req.len());
        assert!(sock.check_liveness());
        let n = sock.recv_packet(0.0);
        assert!(n > 0);
        let buf = sock.read_buffer();
        // SAFETY: the buffer's received bytes.
        let data = unsafe { std::slice::from_raw_parts(buf.str, buf.length) };
        assert!(contains(data, TEST_HTTP_EXPECT));
    });
}

fn set_error() {
    let sock = Sock::new(SW_SOCK_TCP);
    let status = sock.co().set_error(1000, TEST_STR);
    assert_eq!(status, 0);

    assert_eq!(sock.err_code(), 1000);
    // SAFETY: the record's error text, NUL-terminated.
    assert_eq!(
        unsafe { CStr::from_ptr(sock.record().err_msg) }.to_bytes(),
        TEST_STR
    );
}

fn reinit() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP6);
        assert_eq!(sock.record().sock_domain, AF_INET6);
        proxy_set_socks5_proxy(&sock, TEST_SOCKS5_PROXY_PORT, true);
        sock.connect("::1", 80, 0);
        assert_eq!(sock.record().sock_domain, AF_INET);
    });
}

/// `rust_socket_scopes.selected_timeout_restore_and_copy` (was socket_scopes.cpp).
fn selected_timeout_restore_and_copy() {
    run(|| {
        let sock = Sock::new(SW_SOCK_TCP);
        sock.set_timeout(3.5, SW_TIMEOUT_ALL);
        {
            let scope = TimeoutSetter::new(&sock, 0.25, SW_TIMEOUT_RDWR);
            let _copy = scope.copy();
            assert_eq!(sock.get_timeout(SW_TIMEOUT_DNS), 3.5);
            assert_eq!(sock.get_timeout(SW_TIMEOUT_READ), 0.25);
            {
                let _nested = TimeoutSetter::new(&sock, -1.0, SW_TIMEOUT_READ);
                assert_eq!(sock.get_timeout(SW_TIMEOUT_READ), -1.0);
            }
            assert_eq!(sock.get_timeout(SW_TIMEOUT_READ), 0.25);
        }
        assert_eq!(sock.get_timeout(SW_TIMEOUT_READ), 3.5);
        assert_eq!(sock.get_timeout(SW_TIMEOUT_WRITE), 3.5);
        {
            let _zero = TimeoutSetter::new(&sock, 0.0, SW_TIMEOUT_ALL);
            sock.set_timeout(8.0, SW_TIMEOUT_CONNECT);
        }
        assert_eq!(sock.get_timeout(SW_TIMEOUT_CONNECT), 8.0);
    });
}

// The repo-owned witnesses of C++ copies and std::bad_alloc containment, whose bodies are
// ../fixture.cc's (formerly tests/modules/hooks/socket_scopes.cpp and dns_request_owner.cpp).
unsafe extern "C-unwind" {
    fn swr_case_numeric_host_owner_and_independent_copies();
    fn swr_case_hints_vectors_and_failure_publication();
    fn swr_case_native_output_append_exception_is_contained();
}

fn numeric_host_owner_and_independent_copies() {
    // SAFETY: the fixture's case.
    unsafe { swr_case_numeric_host_owner_and_independent_copies() };
}

fn hints_vectors_and_failure_publication() {
    // SAFETY: the fixture's case.
    unsafe { swr_case_hints_vectors_and_failure_publication() };
}

fn native_output_append_exception_is_contained() {
    // SAFETY: the fixture's case.
    unsafe { swr_case_native_output_append_exception_is_contained() };
}

fn actual_async_numeric_results() {
    run(|| {
        let host = swoole_rs_hooks::resolve::system_gethostbyname(b"127.0.0.1", AF_INET, 2.0);
        assert_eq!(host, b"127.0.0.1");
        let addresses = swoole_rs_hooks::resolve::system_getaddrinfo(
            b"127.0.0.1",
            AF_INET,
            SOCK_STREAM,
            0,
            b"",
            2.0,
        );
        assert!(!addresses.is_empty());
        for address in addresses {
            assert_eq!(address, b"127.0.0.1");
        }
    });
}

/// `rust_socket_scopes.<name>`: the timeout scope, whose C++ API left the hybrid
/// header. The protocol scope and error text witnesses are crates/hooks unit tests
/// (`socket_scope::tests`, `co_io::tests::error_text_publication`).
pub(super) const SCOPE_CASES: super::Cases = &[(
    "selected_timeout_restore_and_copy",
    selected_timeout_restore_and_copy,
)];

/// `rust_dns_request.<name>`, the C++ witnesses.
pub(super) const DNS_REQUEST_CASES: super::Cases = &[
    (
        "numeric_host_owner_and_independent_copies",
        numeric_host_owner_and_independent_copies,
    ),
    (
        "hints_vectors_and_failure_publication",
        hints_vectors_and_failure_publication,
    ),
    (
        "native_output_append_exception_is_contained",
        native_output_append_exception_is_contained,
    ),
    ("actual_async_numeric_results", actual_async_numeric_results),
];

/// `coroutine_socket.<name>`, in upstream's order.
pub(super) const CASES: super::Cases = &[
    ("connect_refused", connect_refused),
    ("connect_timeout", connect_timeout),
    ("timeout_controller", timeout_controller),
    ("timeout_setter", timeout_setter),
    ("connect_with_dns", connect_with_dns),
    ("tcp6", tcp6),
    ("unixsock_fail", unixsock_fail),
    ("recv_success", recv_success),
    ("recv_fail", recv_fail),
    ("bind_success", bind_success),
    ("bind_fail", bind_fail),
    ("listen", listen),
    ("accept", accept),
    ("eof_1", eof_1),
    ("eof_2", eof_2),
    ("eof_3", eof_3),
    ("eof_4", eof_4),
    ("eof_5", eof_5),
    ("eof_6", eof_6),
    ("length_1", length_1),
    ("length_2", length_2),
    ("length_3", length_3),
    ("length_4", length_4),
    ("length_5", length_5),
    ("length_7", length_7),
    ("event_hup", event_hup),
    ("recv_line", recv_line),
    ("getsockname", getsockname),
    ("buffer", buffer),
    ("check_liveness", check_liveness),
    ("write_and_read", write_and_read),
    ("write_and_read_2", write_and_read_2),
    ("writev_and_readv", writev_and_readv),
    ("send_and_recv_all", send_and_recv_all),
    ("writevall_and_readvall", writevall_and_readvall),
    ("sendfile", sendfile),
    ("sendto_recvfrom_udp", sendto_recvfrom_udp),
    ("https_get_with_socks5_proxy", https_get_with_socks5_proxy),
    ("http_get_with_socks5_proxy", http_get_with_socks5_proxy),
    ("http_get_with_http_proxy", http_get_with_http_proxy),
    ("https_get_with_http_proxy", https_get_with_http_proxy),
    ("ssl", ssl),
    ("ssl_accept", ssl_accept),
    ("peek", peek),
    ("sendmsg_and_recvmsg", sendmsg_and_recvmsg),
    ("close", close),
    ("cancel", cancel),
    ("get_event_str", get_event_str),
    ("option", option),
    ("ssl_verify", ssl_verify),
    ("shutdown", shutdown),
    ("recv_packet", recv_packet),
    ("set_error", set_error),
    ("reinit", reinit),
];
